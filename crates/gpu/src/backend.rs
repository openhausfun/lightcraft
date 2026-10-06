//! Which wgpu backends LightCraft lets wgpu use — for its compute device and for the desktop
//! window (eframe) — and the crash sentinel around device creation (issue #136).
//!
//! wgpu loads the system driver of *every* backend in the instance's set while it enumerates
//! adapters, even when it then picks another one: with Vulkan in the set, a broken Vulkan driver
//! can crash the process (an access violation inside the driver DLL, which nothing can catch)
//! although DX12 would have been used. So Windows defaults to DX12 alone. macOS uses Metal, Linux
//! Vulkan (plus GL for the window when the build has it).
//!
//! Overrides, read once per process:
//! - `LIGHTCRAFT_GPU_BACKEND` = `dx12` | `vulkan` | `metal` | `gl` (comma lists allowed), `auto`
//!   (the platform default), or `off` (no GPU compute; the window still needs one backend and
//!   keeps the platform default);
//! - else `WGPU_BACKEND` (wgpu's own variable, same names) for both the window and compute.
//!
//! Backends this build doesn't contain (e.g. `gl`: the GL backend isn't compiled in) are ignored
//! with a warning and the platform default is used.

use std::path::PathBuf;
use std::sync::Mutex;

pub use wgpu::Backends;

/// What the environment asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// The platform default.
    Auto,
    /// No GPU compute.
    Off,
    /// These backends.
    Use(Backends),
}

/// Parse a backend list (`"dx12"`, `"vulkan,gl"`, `"auto"`, `"off"`…). `None`: nothing usable in
/// it (empty or unknown names).
pub fn parse_backends(s: &str) -> Option<BackendChoice> {
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "" => None,
        "auto" | "default" => Some(BackendChoice::Auto),
        "off" | "none" | "cpu" | "0" | "false" | "no" => Some(BackendChoice::Off),
        _ => {
            let b = Backends::from_comma_list(&s);
            (!b.is_empty()).then_some(BackendChoice::Use(b))
        }
    }
}

/// The choice from `LIGHTCRAFT_GPU_BACKEND` (`lightcraft`) or else `WGPU_BACKEND` (`wgpu`): the
/// first one that parses wins; an unknown value is skipped (with a warning).
pub fn choose(lightcraft: Option<&str>, wgpu: Option<&str>) -> BackendChoice {
    for (name, v) in [("LIGHTCRAFT_GPU_BACKEND", lightcraft), ("WGPU_BACKEND", wgpu)] {
        let Some(v) = v else { continue };
        match parse_backends(v) {
            // WGPU_BACKEND is wgpu's variable: it selects backends, it never turns the GPU off
            Some(BackendChoice::Off) if name == "WGPU_BACKEND" => log::warn!("gpu: WGPU_BACKEND={v} ignored (use LIGHTCRAFT_GPU_BACKEND=off)"),
            Some(c) => return c,
            None => log::warn!("gpu: {name}={v} names no known backend; ignored"),
        }
    }
    BackendChoice::Auto
}

/// The backends compute devices use when nothing is asked for on `os` (`std::env::consts::OS`).
pub fn default_compute_backends(os: &str) -> Backends {
    match os {
        // never touch the Vulkan driver unless asked: see the module docs (issue #136)
        "windows" => Backends::DX12,
        "macos" | "ios" => Backends::METAL,
        _ => Backends::PRIMARY,
    }
}

/// The backends the window uses when nothing is asked for on `os`.
pub fn default_window_backends(os: &str) -> Backends {
    match os {
        "windows" => Backends::DX12,
        "macos" | "ios" => Backends::METAL,
        // eframe's default (GL only when the build has it)
        _ => Backends::PRIMARY | Backends::GL,
    }
}

/// Resolve `choice` against the backends this build contains (`compiled`): `None` = GPU off.
/// Asked-for backends that aren't compiled in fall back to `default`.
pub fn resolve(choice: BackendChoice, default: Backends, compiled: Backends) -> Option<Backends> {
    match choice {
        BackendChoice::Off => None,
        BackendChoice::Auto => Some(default),
        BackendChoice::Use(b) if b.intersects(compiled) => Some(b & compiled),
        BackendChoice::Use(b) => {
            log::warn!("gpu: backend {b:?} is not in this build ({compiled:?}); using {default:?}");
            Some(default)
        }
    }
}

fn env_choice() -> BackendChoice {
    static C: std::sync::OnceLock<BackendChoice> = std::sync::OnceLock::new();
    *C.get_or_init(|| {
        let lc = std::env::var("LIGHTCRAFT_GPU_BACKEND").ok();
        let wg = std::env::var("WGPU_BACKEND").ok();
        choose(lc.as_deref(), wg.as_deref())
    })
}

/// The backends of the compute device (`None`: `LIGHTCRAFT_GPU_BACKEND=off`).
pub fn compute_backends() -> Option<Backends> {
    resolve(env_choice(), default_compute_backends(std::env::consts::OS), wgpu::Instance::enabled_backend_features())
}

/// The backends for the desktop window's renderer (eframe/egui-wgpu). The window always needs
/// one: `off` keeps the platform default.
pub fn window_backends() -> Backends {
    let default = default_window_backends(std::env::consts::OS);
    let choice = match env_choice() {
        BackendChoice::Off => BackendChoice::Auto,
        c => c,
    };
    resolve(choice, default, wgpu::Instance::enabled_backend_features()).unwrap_or(default)
}

/// `LIGHTCRAFT_GPU_BACKEND=off`.
pub(crate) fn env_off() -> bool {
    env_choice() == BackendChoice::Off
}

static MARKER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Write this file before the compute device is created and remove it once creation returned
/// (successfully or not). A file still there at the next launch means the process died inside
/// the driver: see [`take_init_marker`]. `None` (the default) writes nothing.
pub fn set_init_marker(path: Option<PathBuf>) {
    *MARKER.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

/// Run device creation `f` between writing and removing the init marker (if one is set).
pub(crate) fn with_init_marker<T>(backends: Backends, f: impl FnOnce() -> T) -> T {
    let marker = MARKER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(m) = &marker {
        if let Some(d) = m.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = std::fs::write(m, format!("GPU device creation started (backends {backends:?}, unix time {secs})\n"));
    }
    let r = f();
    if let Some(m) = &marker {
        let _ = std::fs::remove_file(m);
    }
    r
}

/// If the init marker `path` is present — the last process died while creating the GPU device —
/// remove it and return what it recorded.
pub fn take_init_marker(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    Some(text.trim().to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_backend_names() {
        assert_eq!(parse_backends("dx12"), Some(BackendChoice::Use(Backends::DX12)));
        assert_eq!(parse_backends(" D3D12 "), Some(BackendChoice::Use(Backends::DX12)));
        assert_eq!(parse_backends("vulkan"), Some(BackendChoice::Use(Backends::VULKAN)));
        assert_eq!(parse_backends("vk,gl"), Some(BackendChoice::Use(Backends::VULKAN | Backends::GL)));
        assert_eq!(parse_backends("metal"), Some(BackendChoice::Use(Backends::METAL)));
        assert_eq!(parse_backends("auto"), Some(BackendChoice::Auto));
        for off in ["off", "OFF", "none", "cpu", "0"] {
            assert_eq!(parse_backends(off), Some(BackendChoice::Off), "{off}");
        }
        assert_eq!(parse_backends(""), None);
        assert_eq!(parse_backends("directx9"), None);
    }

    #[test]
    fn lightcraft_variable_wins_over_wgpu_backend() {
        assert_eq!(choose(None, None), BackendChoice::Auto);
        assert_eq!(choose(None, Some("dx12")), BackendChoice::Use(Backends::DX12));
        assert_eq!(choose(Some("vulkan"), Some("dx12")), BackendChoice::Use(Backends::VULKAN));
        assert_eq!(choose(Some("off"), Some("dx12")), BackendChoice::Off);
        // unknown LightCraft value: WGPU_BACKEND still applies
        assert_eq!(choose(Some("bogus"), Some("gl")), BackendChoice::Use(Backends::GL));
        // WGPU_BACKEND never turns the GPU off
        assert_eq!(choose(None, Some("off")), BackendChoice::Auto);
    }

    /// Issue #136: Windows must not load the Vulkan driver unless asked to.
    #[test]
    fn windows_defaults_to_dx12_only() {
        assert_eq!(default_compute_backends("windows"), Backends::DX12);
        assert_eq!(default_window_backends("windows"), Backends::DX12);
        assert!(!default_window_backends("windows").contains(Backends::VULKAN));
        assert_eq!(default_compute_backends("macos"), Backends::METAL);
        assert!(default_compute_backends("linux").contains(Backends::VULKAN));
        assert!(default_window_backends("linux").contains(Backends::VULKAN | Backends::GL));
    }

    #[test]
    fn resolves_against_the_build() {
        let compiled = Backends::DX12 | Backends::VULKAN;
        let def = Backends::DX12;
        assert_eq!(resolve(BackendChoice::Auto, def, compiled), Some(def));
        assert_eq!(resolve(BackendChoice::Off, def, compiled), None);
        assert_eq!(resolve(BackendChoice::Use(Backends::VULKAN), def, compiled), Some(Backends::VULKAN));
        assert_eq!(resolve(BackendChoice::Use(Backends::VULKAN | Backends::GL), def, compiled), Some(Backends::VULKAN));
        // GL isn't compiled in: the default, not an empty set (no adapter at all)
        assert_eq!(resolve(BackendChoice::Use(Backends::GL), def, compiled), Some(def));
    }

    #[test]
    fn init_marker_is_written_during_creation_and_removed_after() {
        let dir = std::env::temp_dir().join(format!("lc-gpu-marker-{}", std::process::id()));
        let m = dir.join("gpu-init.marker");
        let _ = std::fs::remove_dir_all(&dir);
        set_init_marker(Some(m.clone()));
        let seen = with_init_marker(Backends::DX12, || m.exists());
        set_init_marker(None);
        assert!(seen, "the marker exists while the device is created");
        assert!(!m.exists(), "and is removed afterwards");
        // a marker left behind by a crashed process is reported once
        std::fs::write(&m, "GPU device creation started (backends DX12)").unwrap();
        assert!(take_init_marker(&m).unwrap().contains("DX12"));
        assert_eq!(take_init_marker(&m), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
