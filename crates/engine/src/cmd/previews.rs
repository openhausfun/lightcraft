//! Build previews ahead of time: each photo's grid thumbnail and its loupe view (standard size,
//! or 1:1), rendered into the memory + disk preview cache so browsing and the loupe are instant.
//! Runs on a background thread (the app keeps working; progress / cancel by command), or inline
//! with `wait` (CLI, MCP, tests).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::media::{RenderJob, THUMB_SIZES};
use crate::{Result, Session};

/// The long edge of a standard-sized preview.
pub const STANDARD_EDGE: usize = 2048;

/// A preview build in progress.
#[derive(Debug, Default)]
pub struct PreviewBuild {
    pub total: usize,
    pub done: AtomicUsize,
    pub failed: AtomicUsize,
    pub cancel: AtomicBool,
    pub finished: AtomicBool,
    /// Damaged smart previews rebuilt (Build Smart Previews only).
    pub repaired: AtomicUsize,
    /// What is built ("" = previews; "smart previews" — Build Smart Previews runs the same way).
    pub what: &'static str,
    /// Why the whole build failed (e.g. the smart previews folder isn't there).
    pub error: std::sync::Mutex<Option<String>>,
}

impl PreviewBuild {
    pub fn json(&self) -> Value {
        json!({
            "total": self.total,
            "done": self.done.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "running": !self.finished.load(Ordering::Relaxed),
            "cancelled": self.cancel.load(Ordering::Relaxed),
            "what": if self.what.is_empty() { "previews" } else { self.what },
            "repaired": self.repaired.load(Ordering::Relaxed),
            "error": self.error(),
        })
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }
}

/// The jobs that build `id`'s previews: the largest grid thumbnail, then the view render at
/// `edge` (`None` = 1:1, the photo's full size).
fn jobs_for(s: &mut Session, id: lightcraft_catalog::PhotoId, edge: Option<usize>) -> Vec<RenderJob> {
    let Some(p) = s.catalog.photo(id) else { return Vec::new() };
    let full = p.width.max(p.height) as usize;
    let e = edge.unwrap_or(full).min(full.max(1)).max(1);
    let mut v = Vec::new();
    v.extend(s.thumb_job(id, THUMB_SIZES[THUMB_SIZES.len() - 1]));
    v.extend(s.loupe_job(id, e, e, true));
    v
}

fn run_all(jobs: Vec<Vec<RenderJob>>, state: &PreviewBuild) {
    for photo_jobs in jobs {
        if state.cancel.load(Ordering::Relaxed) {
            break;
        }
        let ok = photo_jobs.into_iter().all(|j| j.run().rendered.is_ok());
        if !ok {
            state.failed.fetch_add(1, Ordering::Relaxed);
        }
        state.done.fetch_add(1, Ordering::Relaxed);
    }
    state.finished.store(true, Ordering::Relaxed);
}

fn build(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.buildPreviews";
    let edge = match str_param(p, "size").unwrap_or("standard") {
        "standard" => Some(p.get("edge").and_then(Value::as_u64).map_or(STANDARD_EDGE, |e| e.clamp(256, 8192) as usize)),
        "full" | "1:1" => None,
        other => return Err(bad(C, format!("unknown size `{other}` (standard|full)"))),
    };
    if s.preview_build.as_ref().is_some_and(|b| !b.finished.load(Ordering::Relaxed)) {
        return Err(bad(C, "a preview build is already running"));
    }
    // explicit ids, else the selection, else everything in view
    let ids = if p.get("ids").is_some() || p.get("id").is_some() || !s.selection.ids.is_empty() { s.targets(p) } else { s.visible_cloned() };
    let jobs: Vec<Vec<RenderJob>> = ids.iter().map(|id| jobs_for(s, *id, edge)).filter(|j| !j.is_empty()).collect();
    let state = Arc::new(PreviewBuild { total: jobs.len(), ..Default::default() });
    s.preview_build = Some(state.clone());
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(false) || cfg!(target_arch = "wasm32");
    if wait {
        run_all(jobs, &state);
        return Ok(state.json());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let st = state.clone();
        std::thread::Builder::new()
            .name("lc-build-previews".into())
            .spawn(move || run_all(jobs, &st))
            .map_err(|e| bad(C, format!("could not start: {e}")))?;
    }
    Ok(state.json())
}

/// One photo's smart preview to build or discard: (photo, its file in the smart previews folder,
/// how to load the original at preview size).
#[cfg(not(target_arch = "wasm32"))]
type SmartJob = (lightcraft_catalog::PhotoId, std::path::PathBuf, Option<crate::media::SourceRef>);

/// What Build / Discard Smart Previews did.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct SmartCounts {
    /// Proxies there now (complete ones kept, missing or damaged ones written).
    built: usize,
    /// Of those, damaged ones (cut short by a crash or a full drive) that were rebuilt.
    repaired: usize,
    removed: usize,
    /// `[[id, why]]`
    failed: Vec<Value>,
}

/// The file-system half of Build / Discard Smart Previews (runs on a worker thread in the app).
/// `custom`: the folder was chosen (on another drive, perhaps): it must be there, it is never
/// recreated. With `state`, progress and cancel.
#[cfg(not(target_arch = "wasm32"))]
fn smart_run(
    dir: &std::path::Path,
    custom: bool,
    discard: bool,
    jobs: Vec<SmartJob>,
    state: Option<&PreviewBuild>,
) -> std::result::Result<SmartCounts, String> {
    let mut n = SmartCounts::default();
    if !discard {
        // a chosen folder on another drive must be there: never recreate it on this one
        if custom {
            crate::smart::check_writable(dir)?;
        } else {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    for (id, path, source) in jobs {
        if state.is_some_and(|st| st.cancel.load(Ordering::Relaxed)) {
            break;
        }
        if discard {
            if std::fs::remove_file(&path).is_ok() {
                n.removed += 1;
            }
        } else {
            // a complete proxy is kept; a missing or damaged one (cut short by a crash or a full
            // drive) is (re)built
            let damaged = path.exists();
            if damaged && crate::smart::is_valid(&path) {
                n.built += 1;
            } else {
                // atomic: a failed write leaves no partial proxy that would pass for a built one;
                // and synced (issue #134): built so the photo can be edited while its original
                // is offline, when it is the only copy — the sync is small next to the decode
                let r = source
                    .ok_or_else(|| "nothing to build from".to_string())
                    .and_then(|s| s.load_source())
                    .and_then(|src| crate::smart::encode(&src.image, src.info_or(Default::default()).camera_tone.as_ref()))
                    .and_then(|b| lightcraft_catalog::safe_file::write_atomic(&path, &b).map_err(|e| format!("{}: {e}", path.display())));
                match r {
                    Ok(()) => {
                        n.built += 1;
                        if damaged {
                            n.repaired += 1;
                            if let Some(st) = state {
                                st.repaired.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => {
                        n.failed.push(json!([id.0, e]));
                        if let Some(st) = state {
                            st.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        if let Some(st) = state {
            st.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(n)
}

/// Smart previews: build (from the original, at preview size) or discard the proxies that keep
/// photos editable while their originals are offline. With `background` (the app) the files are
/// read and written on a worker thread; progress like Build Previews (`library.previewProgress`).
#[cfg(not(target_arch = "wasm32"))]
fn smart(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.smartPreviews";
    let dir = s.media.smart_dir.clone().ok_or_else(|| bad(C, "smart previews need a library on disk"))?;
    let discard = p.get("discard").and_then(Value::as_bool).unwrap_or(false);
    let background = p.get("background").and_then(Value::as_bool).unwrap_or(false);
    if background && s.preview_build.as_ref().is_some_and(|b| !b.finished.load(Ordering::Relaxed)) {
        return Err(bad(C, "a preview build is already running"));
    }
    let ids = if p.get("ids").is_some() || !s.selection.ids.is_empty() { s.targets(p) } else { s.visible_cloned() };
    let mut jobs: Vec<SmartJob> = Vec::new();
    for id in ids {
        let Some(ph) = s.catalog.photo(id).cloned() else { continue };
        if !matches!(ph.source, lightcraft_catalog::Source::File { .. }) {
            continue;
        }
        let path = dir.join(crate::smart::file_name(&ph));
        let source = (!discard).then(|| s.media.source_ref(&ph, crate::media::SourceLevel::Preview));
        jobs.push((id, path, source));
    }
    let custom = s.smart_previews_dir.is_some();
    if background {
        let what = if discard { "discarding smart previews" } else { "smart previews" };
        let state = Arc::new(PreviewBuild { total: jobs.len(), what, ..Default::default() });
        s.preview_build = Some(state.clone());
        let st = state.clone();
        std::thread::Builder::new()
            .name("lc-smart-previews".into())
            .spawn(move || {
                if let Err(e) = smart_run(&dir, custom, discard, jobs, Some(&st)) {
                    log::warn!("{C}: {e}");
                    *st.error.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(e);
                }
                st.finished.store(true, Ordering::Relaxed);
            })
            .map_err(|e| bad(C, format!("could not start: {e}")))?;
        return Ok(state.json());
    }
    let r = smart_run(&dir, custom, discard, jobs, None).map_err(|e| bad(C, e))?;
    Ok(json!({"built": r.built, "repaired": r.repaired, "removed": r.removed, "failed": r.failed}))
}

/// The smart previews folder: where it is, whether it is custom, what it holds, whether it can be
/// used now (a chosen folder on an unplugged drive is `available: false`).
#[cfg(not(target_arch = "wasm32"))]
fn location_json(s: &Session) -> Value {
    let dir = s.media.smart_dir.clone();
    let default = s.library.as_ref().filter(|l| l.on_disk).map(|l| crate::smart::dir(&l.dir));
    let (count, bytes) = dir.as_deref().map_or((0, 0), crate::smart::stats);
    json!({
        "path": dir.as_ref().map(|d| d.to_string_lossy()),
        "default": default.map(|d| d.to_string_lossy().to_string()),
        "custom": s.smart_previews_dir.is_some(),
        "available": dir.as_ref().is_some_and(|d| d.is_dir()) || s.smart_previews_dir.is_none(),
        "count": count,
        "bytes": bytes,
    })
}

/// Show or change where smart previews are kept (per library).
#[cfg(not(target_arch = "wasm32"))]
fn smart_location(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.smartPreviewsLocation";
    let current = s.media.smart_dir.clone().ok_or_else(|| bad(C, "smart previews need a library on disk"))?;
    let reset = p.get("reset").and_then(Value::as_bool).unwrap_or(false);
    let chosen = str_param(p, "path").filter(|x| !x.trim().is_empty());
    if !reset && chosen.is_none() {
        return Ok(location_json(s));
    }
    let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone()).ok_or_else(|| bad(C, "smart previews need a library on disk"))?;
    let default = crate::smart::dir(&lib_dir);
    let (new_dir, custom) = match chosen {
        Some(x) if !reset => {
            let d = std::path::PathBuf::from(x.trim());
            if !d.is_absolute() {
                return Err(bad(C, "path must be an absolute folder path"));
            }
            (d.clone(), (d != default).then_some(d))
        }
        _ => (default, None),
    };
    if new_dir == current {
        return Ok(location_json(s));
    }
    // the new place must work before anything moves; there is no fallback to another drive
    crate::smart::check_writable(&new_dir).map_err(|e| bad(C, e))?;
    let (held, _) = crate::smart::stats(&current);
    let existing = match str_param(p, "existing") {
        Some(x) => Some(crate::smart::Existing::parse(x).ok_or_else(|| bad(C, "existing must be move, leave or discard"))?),
        None if held == 0 => Some(crate::smart::Existing::Leave),
        None => None,
    };
    let Some(existing) = existing else {
        return Err(bad(C, format!("{held} smart preview(s) are in {}: pass existing = move, leave or discard", current.display())));
    };
    let (moved, failed) = crate::smart::migrate(&current, &new_dir, existing);
    s.smart_previews_dir = custom;
    s.media.smart_dir = Some(new_dir);
    s.save_prefs()?;
    let mut r = location_json(s);
    r["handled"] = json!(moved);
    r["failed"] = json!(failed);
    Ok(r)
}

/// Whether photo `id` has a smart preview.
pub fn has_smart_preview(s: &Session, id: lightcraft_catalog::PhotoId) -> bool {
    match (&s.media.smart_dir, s.catalog.photo(id)) {
        (Some(dir), Some(p)) => crate::smart::is_valid(&dir.join(crate::smart::file_name(p))),
        _ => false,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        #[cfg(not(target_arch = "wasm32"))]
        cmd!(
            "library.smartPreviews",
            "Build Smart Previews",
            [],
            None,
            "{ids?, discard?: bool, background?: bool} — build (or discard) the smart previews of the selected photos (else all in view): compact proxies in the library that keep photos editable and exportable (at proxy size) while their originals are offline; a damaged proxy (cut short) is rebuilt → {built, repaired, removed, failed}; with background (the app's menu) the files are read and written on a worker thread → {total, done, failed, repaired, running, what} like library.buildPreviews (progress: library.previewProgress, stop: library.cancelPreviews)",
            always,
            smart
        ),
        #[cfg(not(target_arch = "wasm32"))]
        cmd!(
            "library.smartPreviewsLocation",
            "Smart Previews Location",
            [],
            None,
            "{path?: absolute folder, reset?: bool, existing?: move|leave|discard} — without path/reset: where this library keeps its smart previews → {path, default, custom, available, count, bytes}. With path (or reset = back to `Smart Previews` in the library): use that folder instead (must exist or have an existing parent, and accept writes; never falls back to another drive). Smart previews already in the old folder need `existing`: move them, leave them (build again), or discard them → also {handled, failed}",
            always,
            smart_location
        ),
        cmd!(query "photo.smartPreview", "Smart Preview Status", [], None, "{id?} → {smartPreview: bool, originalOnline: bool}", always, |s, p| {
            let id = s.targets(p).first().copied().ok_or_else(|| bad("photo.smartPreview", "no photo"))?;
            let online = match s.catalog.photo(id).map(|p| p.source.clone()) {
                Some(lightcraft_catalog::Source::File { path }) => std::path::Path::new(&path).exists(),
                _ => true,
            };
            Ok(json!({"smartPreview": has_smart_preview(s, id), "originalOnline": online}))
        }),
        cmd!(
            "library.buildPreviews",
            "Build Previews",
            [],
            None,
            "{size?: standard (2048 px, or `edge`) | full (1:1), ids?, wait?: bool} — render the grid thumbnail and loupe view of the selected photos (else all in view) into the preview cache, in the background unless `wait` → {total, done, failed, running}",
            always,
            build
        ),
        cmd!(query "library.previewProgress", "Preview Build Progress", [], None, "{} → {total, done, failed, repaired, running, cancelled, what, error} | null", always, |s, _| {
            Ok(s.preview_build.as_ref().map_or(Value::Null, |b| b.json()))
        }),
        cmd!("library.cancelPreviews", "Cancel Preview Build", [], None, "{}", always, |s, _| {
            if let Some(b) = &s.preview_build {
                b.cancel.store(true, Ordering::Relaxed);
            }
            Ok(s.preview_build.as_ref().map_or(Value::Null, |b| b.json()))
        }),
    ]
}
