//! Cameras and memory cards: mounted volumes with a `DCIM` folder (the DCF layout every camera
//! writes). Listed by `library.devices`; the app offers them under File → Import from Device, and
//! the import review copies from them into the library.
//!
//! Menus are rebuilt every frame (even with their popups closed), so [`devices`] never touches the
//! disk: it serves the last scan and, when that is older than [`MAX_AGE`], starts one background
//! rescan (coalesced; no lock is held while it runs). A changed result calls the [`on_change`]
//! hook, which the UI uses to repaint, so hot-plugged cards still appear. [`devices_now`] scans
//! synchronously for explicit requests (`library.devices`, Add from Device without a path).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Device {
    /// The volume's name ("EOS_DIGITAL", "Untitled").
    pub name: String,
    /// Its `DCIM` folder (what gets imported).
    pub path: String,
    /// The volume's mount point.
    pub root: String,
}

/// How long a scan is served before [`devices`] starts another one.
pub const MAX_AGE: Duration = Duration::from_secs(2);

/// Folders whose children are mounted volumes on this platform. `LIGHTCRAFT_DEVICE_ROOTS`
/// (paths joined like `PATH`) replaces them (tests, unusual mounts).
fn mount_parents() -> Vec<std::path::PathBuf> {
    if let Some(v) = std::env::var_os("LIGHTCRAFT_DEVICE_ROOTS") {
        return std::env::split_paths(&v).collect();
    }
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        out.push("/Volumes".into());
    } else if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        // Linux desktops mount under /media/$USER or /run/media/$USER; FreeBSD's automounter
        // (automount/autofs, or a desktop's) under /media. Missing folders are skipped.
        if let Ok(user) = std::env::var("USER") {
            out.push(format!("/media/{user}").into());
            out.push(format!("/run/media/{user}").into());
        }
        out.push("/media".into());
        out.push("/mnt".into());
    }
    out
}

type Scanner = dyn Fn() -> Vec<Device> + Send + Sync;
type Notify = dyn Fn() + Send + Sync;

#[derive(Default)]
struct WatchState {
    list: Vec<Device>,
    /// When the served list was scanned (`None`: never). No clock (and no threads) on wasm, where
    /// there are no mounted volumes either.
    #[cfg(not(target_arch = "wasm32"))]
    scanned: Option<std::time::Instant>,
    /// A background scan is running.
    refreshing: bool,
}

/// A cached device list, refreshed off the calling thread.
pub struct DeviceWatch {
    scanner: Box<Scanner>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    max_age: Duration,
    state: Mutex<WatchState>,
    notify: Mutex<Option<Arc<Notify>>>,
}

impl DeviceWatch {
    pub fn new(scanner: impl Fn() -> Vec<Device> + Send + Sync + 'static, max_age: Duration) -> Arc<Self> {
        Arc::new(DeviceWatch { scanner: Box::new(scanner), max_age, state: Mutex::default(), notify: Mutex::new(None) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, WatchState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called (on the scanning thread) whenever a scan changes the list.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        *self.notify.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// The last scan, immediately; starts a background rescan when it is stale and none is running.
    pub fn get(self: &Arc<Self>) -> Vec<Device> {
        let mut s = self.state();
        #[cfg(not(target_arch = "wasm32"))]
        if !s.refreshing && s.scanned.is_none_or(|t| t.elapsed() >= self.max_age) {
            s.refreshing = true;
            let me = self.clone();
            // the scan runs without the state lock; `get` only ever waits for a list clone
            let spawned = std::thread::Builder::new().name("lightcraft-devices".into()).spawn(move || {
                let list = (me.scanner)();
                me.store(list);
            });
            if spawned.is_err() {
                s.refreshing = false;
            }
        }
        s.list.clone()
    }

    /// Scan now on this thread, and serve the result to later [`DeviceWatch::get`] calls.
    pub fn scan_now(&self) -> Vec<Device> {
        let list = (self.scanner)();
        self.store(list.clone());
        list
    }

    fn store(&self, list: Vec<Device>) {
        let changed = {
            let mut s = self.state();
            s.refreshing = false;
            #[cfg(not(target_arch = "wasm32"))]
            {
                s.scanned = Some(std::time::Instant::now());
            }
            let changed = s.list != list;
            s.list = list;
            changed
        };
        if changed {
            let f = self.notify.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(f) = f {
                f();
            }
        }
    }

    /// A background scan is running.
    pub fn refreshing(&self) -> bool {
        self.state().refreshing
    }
}

fn global() -> &'static Arc<DeviceWatch> {
    static WATCH: std::sync::OnceLock<Arc<DeviceWatch>> = std::sync::OnceLock::new();
    WATCH.get_or_init(|| DeviceWatch::new(scan, MAX_AGE))
}

/// Mounted volumes that look like a camera or a memory card, without blocking: the last scan,
/// refreshed in the background when older than [`MAX_AGE`] (empty until the first scan lands).
pub fn devices() -> Vec<Device> {
    global().get()
}

/// Scan the mounted volumes now (blocking), for explicit requests.
pub fn devices_now() -> Vec<Device> {
    global().scan_now()
}

/// Run `f` whenever a scan changes the device list (the UI requests a repaint).
pub fn on_change(f: impl Fn() + Send + Sync + 'static) {
    global().on_change(f);
}

fn scan() -> Vec<Device> {
    let mut extra = Vec::new();
    if cfg!(windows) && std::env::var_os("LIGHTCRAFT_DEVICE_ROOTS").is_none() {
        extra.extend((b'D'..=b'Z').map(|c| std::path::PathBuf::from(format!("{}:\\", c as char))));
    }
    devices_in(&mount_parents(), extra)
}

/// Devices among the children of `parents`, plus the volume roots `extra`.
pub fn devices_in(parents: &[std::path::PathBuf], extra: Vec<std::path::PathBuf>) -> Vec<Device> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    for parent in parents {
        if let Ok(rd) = std::fs::read_dir(parent) {
            let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            // the startup disk shows up in /Volumes as a symlink to /
            roots.extend(v.into_iter().filter(|p| !p.is_symlink()));
        }
    }
    roots.extend(extra);
    let mut out: Vec<Device> = Vec::new();
    for root in roots {
        let Some(dcim) = dcim_in(&root) else { continue };
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| root.to_string_lossy().to_string());
        if !out.iter().any(|d| d.root == root.to_string_lossy()) {
            out.push(Device { name, path: dcim.to_string_lossy().to_string(), root: root.to_string_lossy().to_string() });
        }
    }
    out
}

/// The volume's `DCIM` folder: the usual spellings are checked directly, so a volume's root is
/// only listed (other spellings on a case-sensitive volume) off Windows, whose drives aren't.
fn dcim_in(root: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Some(p) = ["DCIM", "dcim"].iter().map(|n| root.join(n)).find(|p| p.is_dir()) {
        return Some(p);
    }
    if cfg!(windows) {
        return None;
    }
    std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("DCIM")) && p.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn volumes_with_a_dcim_folder_are_devices() {
        let base = std::env::temp_dir().join(format!("lc-devices-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("EOS_DIGITAL/DCIM/100CANON")).unwrap();
        std::fs::create_dir_all(base.join("Backup/Photos")).unwrap();
        std::fs::create_dir_all(base.join("SD/dcim")).unwrap();
        std::fs::create_dir_all(base.join("X/Dcim")).unwrap();
        let d = devices_in(std::slice::from_ref(&base), Vec::new());
        let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["EOS_DIGITAL", "SD", "X"], "{d:?}");
        // the DCIM folder itself, in a spelling that opens on that volume
        for x in &d {
            let p = std::path::Path::new(&x.path);
            assert!(p.is_dir() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("DCIM")), "{x:?}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    fn card(name: &str) -> Device {
        Device { name: name.into(), path: format!("/v/{name}/DCIM"), root: format!("/v/{name}") }
    }

    /// Wait (bounded) for `f`.
    fn until(f: impl Fn() -> bool) -> bool {
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(20) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn slow_scans_never_block_and_hot_plug_still_arrives() {
        // a scanner that takes 1.5 s (a sleeping network drive) and counts its runs
        const SLOW: Duration = Duration::from_millis(1500);
        let mounted = Arc::new(Mutex::new(Vec::<Device>::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let (m, r) = (mounted.clone(), runs.clone());
        let watch = DeviceWatch::new(
            move || {
                r.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(SLOW);
                m.lock().unwrap().clone()
            },
            Duration::from_millis(20),
        );
        let changes = Arc::new(AtomicUsize::new(0));
        let c = changes.clone();
        watch.on_change(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        // cold: served at once (empty) while one scan runs in the background
        let t = std::time::Instant::now();
        for _ in 0..50 {
            assert!(watch.get().is_empty());
        }
        assert!(t.elapsed() < SLOW / 2, "get() waited for the scan: {:?}", t.elapsed());
        assert!(watch.refreshing());
        assert!(until(|| !watch.refreshing()));
        assert_eq!(runs.load(Ordering::SeqCst), 1, "requests while scanning are coalesced");
        assert_eq!(changes.load(Ordering::SeqCst), 0, "nothing changed");
        // a card is plugged in: the next stale request starts a rescan, still without waiting
        mounted.lock().unwrap().push(card("EOS_DIGITAL"));
        std::thread::sleep(Duration::from_millis(40));
        let t = std::time::Instant::now();
        assert!(watch.get().is_empty(), "the old list is served while rescanning");
        assert!(t.elapsed() < SLOW / 2, "warm get() waited: {:?}", t.elapsed());
        assert!(until(|| changes.load(Ordering::SeqCst) == 1), "a changed scan notifies");
        assert_eq!(watch.get(), [card("EOS_DIGITAL")]);
        // explicit requests scan synchronously
        assert!(until(|| !watch.refreshing()));
        mounted.lock().unwrap().clear();
        assert!(watch.scan_now().is_empty());
        assert_eq!(changes.load(Ordering::SeqCst), 2);
    }
}
