//! The LightCraft develop pipeline on the GPU (wgpu compute, WGSL kernels).
//!
//! The CPU pipeline (`lightcraft-pipeline`) is the reference: every kernel here is a port of a CPU
//! stage, both read the same resolved parameters ([`lightcraft_pipeline::Plan`],
//! [`lightcraft_pipeline::finish::FinishParams`]), and the equivalence tests (`tests/`) render the
//! same settings on both and bound the difference in 8-bit sRGB. Stages without a kernel run on the
//! CPU inside the same render (per-stage hybrid); see `docs/gpu-pipeline.md`.
//!
//! Which backends wgpu may load (DX12 only on Windows; `LIGHTCRAFT_GPU_BACKEND`, `WGPU_BACKEND`) and
//! the crash sentinel around device creation: [`backend`] (issue #136).
//!
//! Use [`render`]: it returns `None` when the GPU is unavailable, disabled (`LIGHTCRAFT_GPU=0`,
//! `LIGHTCRAFT_GPU_BACKEND=off` or [`set_enabled`]), the render does not fit the device, or the device reported an error, ran out
//! of memory or returned an incomplete image — callers then render on the CPU. [`unavailable_reason`]
//! and [`last_fallback`] say why (`ui.inspect` → `perf.gpuReason` / `perf.gpuFallback`).
//! The browser build has no GPU path yet (WebGPU device creation is asynchronous): everything here
//! compiles to the CPU fallback on wasm32.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, Rendered, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

#[cfg(not(target_arch = "wasm32"))]
pub mod backend;
#[cfg(not(target_arch = "wasm32"))]
mod ctx;
#[cfg(not(target_arch = "wasm32"))]
mod params;
#[cfg(not(target_arch = "wasm32"))]
mod render;

#[cfg(not(target_arch = "wasm32"))]
pub use render::GpuStages;

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Set when a GPU render failed: the process stays on the CPU from then on.
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Why the GPU stopped being used (the first fatal failure).
static BROKEN_REASON: Mutex<Option<String>> = Mutex::new(None);
/// The latest render that fell back to the CPU, and why.
static LAST_FALLBACK: Mutex<Option<String>> = Mutex::new(None);

/// Stop using the GPU for the rest of the process (after a device error).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mark_broken(reason: &str) {
    let mut r = BROKEN_REASON.lock().unwrap_or_else(|e| e.into_inner());
    if r.is_none() {
        *r = Some(reason.to_string());
    }
    BROKEN.store(true, Ordering::Relaxed);
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn record_fallback(reason: String) {
    *LAST_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
}

/// Why renders do not use the GPU, or `None` when they do (or will, once the device is created).
/// E.g. "disabled by LIGHTCRAFT_GPU=0", "software adapter (llvmpipe …) skipped", "device lost …".
/// Never blocks.
pub fn unavailable_reason() -> Option<String> {
    if env_disabled() {
        #[cfg(not(target_arch = "wasm32"))]
        if backend::env_off() {
            return Some("disabled by LIGHTCRAFT_GPU_BACKEND=off".into());
        }
        return Some("disabled by LIGHTCRAFT_GPU=0".into());
    }
    if !ENABLED.load(Ordering::Relaxed) {
        return Some("disabled by the GPU rendering preference (app.gpu)".into());
    }
    if BROKEN.load(Ordering::Relaxed) {
        let r = BROKEN_REASON.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| "device error".into());
        return Some(format!("stopped after a GPU failure: {r}"));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        match GPU.get() {
            Some(Err(e)) => Some(e.clone()),
            _ => None,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        Some("the browser build has no GPU path yet".into())
    }
}

/// The latest render that fell back from the GPU to the CPU and why (device limit, out of memory,
/// device error, incomplete or blank result), if any.
pub fn last_fallback() -> Option<String> {
    LAST_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A simulated GPU failure for the next [`render`] on this thread (tests of the CPU fallback).
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// A command the device rejects (validation error).
    Validation,
    /// The per-pixel stage's work never runs (what a driver that silently drops or resets a
    /// submission looks like: the readback holds no image).
    DropWork,
    /// A storage-buffer limit of this many bytes for this render.
    Limit(u64),
}

thread_local! {
    static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
}

/// Make the next [`render`] on this thread fail with `f` (tests).
#[doc(hidden)]
pub fn inject_fault(f: Fault) {
    FAULT.with(|c| c.set(Some(f)));
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn take_fault() -> Option<Fault> {
    FAULT.with(|c| c.take())
}

/// Use the GPU again after a failure (tests that inject faults).
#[doc(hidden)]
pub fn reset_failures() {
    BROKEN.store(false, Ordering::Relaxed);
    *BROKEN_REASON.lock().unwrap_or_else(|e| e.into_inner()) = None;
    *LAST_FALLBACK.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Allow or forbid GPU rendering at runtime (a preference). `LIGHTCRAFT_GPU=0` forbids it for the
/// whole process regardless.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether GPU rendering is allowed (environment, preference, no earlier failure).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && !BROKEN.load(Ordering::Relaxed) && !env_disabled()
}

/// `LIGHTCRAFT_GPU=0` (or `LIGHTCRAFT_GPU_BACKEND=off`): no GPU for the whole process.
fn env_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        let off = std::env::var("LIGHTCRAFT_GPU").is_ok_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no"));
        #[cfg(not(target_arch = "wasm32"))]
        let off = off || backend::env_off();
        off
    })
}

#[cfg(not(target_arch = "wasm32"))]
static GPU: std::sync::OnceLock<Result<ctx::Gpu, String>> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn device() -> Option<&'static ctx::Gpu> {
    // turned off (preference, environment): don't even load the driver (issue #136)
    if env_disabled() || !ENABLED.load(Ordering::Relaxed) {
        return existing_device();
    }
    GPU.get_or_init(|| {
        let Some(backends) = backend::compute_backends() else { return Err("disabled by LIGHTCRAFT_GPU_BACKEND=off".into()) };
        backend::with_init_marker(backends, || {
            std::panic::catch_unwind(|| ctx::Gpu::new(backends)).unwrap_or_else(|_| Err("device creation panicked".into()))
        })
    })
    .as_ref()
    .ok()
}

/// The device if it has been created (never creates it).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn existing_device() -> Option<&'static ctx::Gpu> {
    GPU.get().and_then(|g| g.as_ref().ok())
}

/// Create the device and compile the kernels on a background thread now (once the window is up),
/// so the first render doesn't wait ~0.3–0.4 s for it and the UI thread never does. Does nothing
/// while GPU rendering is turned off.
pub fn warm_up() {
    #[cfg(not(target_arch = "wasm32"))]
    if !env_disabled() && ENABLED.load(Ordering::Relaxed) && GPU.get().is_none() {
        let _ = std::thread::Builder::new().name("lc-gpu-init".into()).spawn(|| {
            let _ = device();
        });
    }
}

/// Has device creation finished (successfully or not)? Never blocks — for status displays.
pub fn ready() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        env_disabled() || GPU.get().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
}

/// Whether a usable GPU adapter exists and GPU rendering is enabled (creates the device on first
/// use).
pub fn available() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        enabled() && device().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// The adapter's name and backend, e.g. "Apple M4 Pro (Metal)".
pub fn adapter_name() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        device().map(|g| format!("{} ({:?})", g.info.name, g.info.backend))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Device buffers held by the renderer.
#[cfg(not(target_arch = "wasm32"))]
pub use ctx::GpuMemory;

/// Device buffers held by the renderer.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuMemory {
    pub allocated: u64,
    pub pooled: u64,
    pub retired: u64,
}

/// Device memory held by the renderer's buffers (zero without a device).
pub fn memory() -> GpuMemory {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::memory()
    }
    #[cfg(target_arch = "wasm32")]
    {
        GpuMemory::default()
    }
}

/// Keep at most `bytes` of recycled buffers in the free pool from now on (trimming it now).
pub fn set_pool_limit(bytes: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::POOL_LIMIT.store(bytes, Ordering::Relaxed);
        trim_pool(bytes);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
}

/// Free recycled buffers until at most `keep` bytes stay pooled (e.g. when the app goes idle).
pub fn trim_pool(keep: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(g) = existing_device() {
        g.trim(keep);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = keep;
}

/// Device bytes held by a view's GPU stages (kept with its [`StageCache`]); 0 when it has none.
pub fn stage_bytes(stages: &StageCache) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stages.peek_extension::<GpuStages>().map(|g| g.bytes()).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = stages;
        0
    }
}

/// Render `src` with `s` on the GPU, reusing the device-resident stages kept with `stages` (the
/// view's CPU stage cache). `None`: render on the CPU instead.
pub fn render(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, stages: Option<&StageCache>) -> Option<Rendered> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // the kernel writes 8-bit output: high-bit-depth exports (and soft proofs) render on the CPU
        if !enabled() || req.depth != lightcraft_pipeline::OutputDepth::U8 || req.proof.is_some() {
            return None;
        }
        let gpu = device()?;
        let ext = stages.map(|c| c.extension::<GpuStages>());
        let fault = take_fault();
        let _ = ctx::take_failure(); // nothing left over from an earlier render on this thread
        let (r, scoped) = {
            let _scope = ctx::RenderScope::new(gpu);
            let errors = ctx::ErrorScopes::push(gpu);
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render::render(gpu, src, info, s, req, ext.as_deref(), fault)));
            (r, errors.pop())
        };
        // An error the device raised explains the readback failures that follow it; a buffer
        // over the limit explains the errors its placeholder raises.
        let failure = match (ctx::take_failure(), scoped) {
            (Some(f), _) if f.kind == ctx::FailKind::Limit => Some(f),
            (f, scoped) => scoped.or(f),
        };
        match (r, failure) {
            (Err(_), _) => {
                log::error!("gpu: render failed; using the CPU pipeline from now on");
                mark_broken("a GPU render panicked");
                record_fallback("a GPU render panicked; the GPU is not used again".into());
                None
            }
            (Ok(r), Some(f)) => {
                // the view's cached device stages may hold results of the failed passes
                if let Some(e) = &ext {
                    e.clear();
                }
                let what = match &r {
                    Some(r) => format!("{}×{}", r.image.width, r.image.height),
                    None => "GPU render".into(),
                };
                let reason = match f.kind {
                    ctx::FailKind::Fatal => {
                        mark_broken(&f.reason);
                        format!("{what}: {}; the GPU is not used again", f.reason)
                    }
                    ctx::FailKind::OutOfMemory => {
                        gpu.trim(0);
                        format!("{what}: {}", f.reason)
                    }
                    ctx::FailKind::Limit | ctx::FailKind::Blank => format!("{what}: {}", f.reason),
                };
                log::warn!("gpu: rendering on the CPU instead: {reason}");
                if render::profiling() {
                    eprintln!("  gpu fallback to the CPU: {reason}");
                }
                record_fallback(reason);
                None
            }
            // a device error on another thread or a lost device during the render
            (Ok(Some(_)), None) if BROKEN.load(Ordering::Relaxed) => {
                record_fallback(format!("device failure during the render: {}", unavailable_reason().unwrap_or_default()));
                None
            }
            (Ok(r), None) => r,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (src, info, s, req, stages);
        None
    }
}
