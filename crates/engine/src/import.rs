//! Import: files and folders (recursive) → photos.
//!
//! 1. Expand folders recursively into supported files (hidden entries and the library's own
//!    folder are skipped), sorted for a stable order.
//! 2. Skip paths already in the catalog.
//! 3. Probe the rest — dimensions, metadata, and the **content hash** of the bytes — in parallel
//!    on native targets.
//! 4. Skip **duplicates by content** (same bytes already in the library, or twice in this batch).
//! 5. *Add* in place (the photo points at the original file), *copy* into the library's
//!    `Originals/YYYY/YYYY-MM-DD/` folder (names made unique) and point at the copy, or *move*
//!    there: placed like a copy (with its XMP sidecars), and the original removed only once its
//!    catalog record is committed and saved (see [`crate::import_move`] for the safety rules).
//! 6. Apply each photo's XMP sidecar (or a raw/DNG file's embedded XMP): the sidecar wins for
//!    metadata and develop settings are restored (see [`crate::sidecar`]). Its capture time fills
//!    in when the file has none (read before step 5, so it also files and names the copy).
//! 7. Optionally apply a preset and add keywords (the import dialog's options).
//! 8. Commit all new photos as one undoable op. (Undoing a move removes the catalog records;
//!    the files stay at the destination — undo never moves files back over a card.)
//!
//! The import dialog first calls [`scan`] (`library.importPreview`): the same expansion, probing
//! and duplicate detection without adding anything, so the user can review the candidates. The
//! probes are kept and reused by the import that follows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::media::ProbeInfo;

/// File extensions LightCraft imports (lower case).
pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef", "nrw", "arw", "raf", "orf", "rw2", "pef", "psd", "jxl", "gif", "bmp",
    "heic", "avif",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportMode {
    /// Reference the files where they are.
    #[default]
    Add,
    /// Copy the files into the library's `Originals/` folder.
    Copy,
    /// Move the files there: as Copy, then each original (and its sidecars) is removed from the
    /// source once the copy is verified and catalogued.
    Move,
}

/// What an import does besides adding the photos.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    pub mode: ImportMode,
    /// Applied to every imported photo (one History entry).
    pub preset: Option<lightcraft_develop::Preset>,
    /// Added to every imported photo.
    pub keywords: Vec<String>,
    /// Browsing a folder: photos come in as `local` (not in the library), and a file with the
    /// same content as one already known is still listed.
    pub local: bool,
    /// Copy: where the copies go (default: the library's `Originals/`).
    pub destination: Option<String>,
    /// Copy: the folders inside the destination.
    pub organize: Organize,
    /// Copy: a file-name template for the copies ([`crate::rename::expand`] tokens: `{name}`,
    /// `{seq:N}`, `{date:%Y%m%d}`, `{camera}`…), numbered from `rename_start`.
    pub rename: Option<String>,
    pub rename_start: usize,
    /// A metadata preset (by name) applied to every imported photo.
    pub metadata_preset: Option<String>,
    /// Copy: raws are copied as DNG (Copy as DNG).
    pub convert_dng: bool,
}

/// How copies are filed in the destination. The date is the capture time, else the time of the
/// import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Organize {
    /// `YYYY/YYYY-MM-DD/` by capture date.
    #[default]
    ByDay,
    /// `YYYY/YYYY-MM/`.
    ByMonth,
    /// All in the destination itself.
    Flat,
    /// A folder template, e.g. `{date:%Y}/{date:%Y%m%d}` → `2026/20260114/`: the template's own
    /// `/` make the levels, each expanded with the file-name tokens ([`crate::rename::expand_folder`];
    /// never outside the destination).
    Template(String),
}

impl Organize {
    /// `date` | `month` | `flat`, or a folder template (anything with a `{`, `/` or `\`).
    pub fn parse(s: &str) -> Option<Organize> {
        match s {
            "date" | "day" | "byDay" => Some(Organize::ByDay),
            "month" | "byMonth" => Some(Organize::ByMonth),
            "flat" | "none" | "intoOneFolder" => Some(Organize::Flat),
            t if t.contains(['{', '/', '\\']) => Some(Organize::Template(t.trim().to_string())),
            _ => None,
        }
    }

    /// The folders (inside the destination) a copy of `p` goes to.
    pub fn folders(&self, p: &Photo) -> Vec<String> {
        let date = p.date();
        let day = date.get(..10).filter(|d| d.len() == 10).unwrap_or("undated");
        let year = day.get(..4).unwrap_or("undated");
        match self {
            Organize::ByDay => vec![year.into(), day.into()],
            Organize::ByMonth => vec![year.into(), day.get(..7).unwrap_or("undated").into()],
            Organize::Flat => Vec::new(),
            Organize::Template(t) => crate::rename::expand_folder(t, p, 1),
        }
    }
}

/// A file found by [`scan`], for the import review.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportCandidate {
    pub path: String,
    pub name: String,
    pub format: String,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub captured: Option<String>,
    /// `"path"` (already in the library), `"content"` (same bytes in the library or earlier in this
    /// list): skipped by the import.
    pub duplicate: Option<String>,
    /// The photo that already has it.
    pub existing: Option<u64>,
    /// Not readable.
    pub error: Option<String>,
    /// A raw variant that can't be decoded yet (why): it imports as preview only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_only: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Duplicate {
    pub path: String,
    /// The photo that already has these bytes (or this path).
    pub existing: Option<u64>,
    /// `"path"` (already imported from there) or `"content"` (same bytes elsewhere).
    pub reason: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub imported: Vec<u64>,
    pub duplicates: Vec<Duplicate>,
    /// Move: the originals that were moved (and removed from the source).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<Moved>,
    /// Move: originals (or sidecars) left at the source, and why — the photo may still have been
    /// imported (from its copy, or in place).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
    /// (path, error)
    pub failed: Vec<(String, String)>,
    /// Files found after expanding folders.
    pub scanned: usize,
    /// Photos whose metadata/develop settings were read from an XMP sidecar (or embedded XMP).
    pub sidecars: usize,
}

/// A file moved by an import.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Moved {
    pub from: String,
    pub to: String,
    /// Its sidecars' new paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<String>,
}

/// A source a Move left where it was.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Kept {
    pub path: String,
    pub reason: String,
}

/// The develop settings a photo gets on import: raws start from their as-shot white balance with
/// default sharpening / colour noise reduction, and file-embedded lens corrections on (as the
/// camera intended); a user default preset ([`ImportDefaults`]) goes on top.
pub fn import_defaults(p: &Photo) -> lightcraft_develop::DevelopSettings {
    p.import_defaults()
}

/// Does the photo still look as the camera rendered it (its embedded camera preview is then a
/// fair stand-in)? Not when a default preset changed it on import.
pub fn has_import_look(p: &Photo) -> bool {
    *p.develop == p.camera_defaults()
}

/// User defaults applied on import (Settings → Import; saved in the library's `prefs.json`).
/// Presets are referenced by id; a preset that no longer exists is ignored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportDefaults {
    /// Preset applied to raw files (`None` = the LightCraft default).
    pub raw_preset: Option<String>,
    /// Use a camera's own default (below) when the photo's camera has one.
    pub per_camera: bool,
    /// Per-camera raw defaults, keyed by the camera's make + model as shown in Info
    /// (`"Canon EOS R5"`).
    pub cameras: Vec<CameraDefault>,
    /// Preset applied to non-raw images (JPEG, PNG, TIFF, HEIC…; `None` = none).
    pub other_preset: Option<String>,
    /// Copyright notice given to imported photos that don't carry one (empty = none).
    pub copyright: String,
    /// Creator given to imported photos that don't name one (empty = none).
    pub creator: String,
    /// Metadata preset applied to every imported photo (`metadata.*`; `None` = none).
    pub metadata_preset: Option<String>,
    /// Auto import: a watched folder whose new photos are added as they arrive (`None` = off),
    /// copied into the library's Originals instead of added in place when `auto_copy`, and an
    /// album they go to.
    pub auto_folder: Option<String>,
    pub auto_copy: bool,
    pub auto_album: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraDefault {
    /// Make + model (`Meta::camera`).
    pub camera: String,
    /// Preset id; `None` = the LightCraft default for this camera.
    pub preset: Option<String>,
}

impl ImportDefaults {
    /// The preset id that applies to a photo of this kind and camera, if any.
    pub fn preset_for(&self, raw: bool, camera: &str) -> Option<&str> {
        if !raw {
            return self.other_preset.as_deref();
        }
        if self.per_camera
            && let Some(c) = self.cameras.iter().find(|c| !camera.is_empty() && c.camera.eq_ignore_ascii_case(camera))
        {
            return c.preset.as_deref();
        }
        self.raw_preset.as_deref()
    }
}

/// Give a freshly imported photo its default settings: the camera defaults, then the matching
/// default preset ([`ImportDefaults::preset_for`]), remembered as the photo's import look.
pub fn apply_import_defaults(s: &Session, p: &mut Photo) {
    // metadata defaults fill gaps only: the file's own copyright / creator win
    let d = &s.import_defaults;
    if p.meta.copyright.trim().is_empty() && !d.copyright.trim().is_empty() {
        p.meta.copyright = d.copyright.trim().to_string();
    }
    if p.meta.creator.trim().is_empty() && !d.creator.trim().is_empty() {
        p.meta.creator = d.creator.trim().to_string();
    }
    if let Some(mp) = d.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
        crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
    }
    let base = p.camera_defaults();
    // a raw shown from its embedded JPEG gets the rendered-file default, not the raw one
    let raw = p.develops_raw();
    let preset = s.import_defaults.preset_for(raw, &p.meta.camera).and_then(|id| s.presets.iter().find(|x| x.id == id));
    match preset {
        Some(pr) => {
            let look = std::sync::Arc::new(pr.apply(&base, 1.0));
            p.develop = look.clone();
            p.import_look = (*look != base).then_some(look);
        }
        None => {
            p.develop = std::sync::Arc::new(base);
            p.import_look = None;
        }
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension().is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Expand files and folders (recursively) into supported files. `skip` (e.g. the library folder)
/// is never descended into.
pub fn expand(paths: &[String], skip: Option<&Path>) -> Vec<String> {
    fn walk(p: &Path, skip: Option<&Path>, out: &mut Vec<String>, top: bool) {
        let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if (hidden && !top) || skip.is_some_and(|s| p == s) {
            return;
        }
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            for c in v {
                walk(&c, skip, out, false);
            }
        } else if top || is_supported(p) {
            // explicitly named files are attempted even with an unknown extension (sniffed)
            out.push(p.to_string_lossy().to_string());
        }
    }
    let mut out = Vec::new();
    for p in paths {
        walk(Path::new(p), skip, &mut out, true);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// Probe files (headers + content hash) as an import would.
pub(crate) fn probe_paths(s: &Session, paths: &[String]) -> Vec<Result<ProbeInfo, String>> {
    probe_all(s.media.file_probe.as_ref(), paths, &ScanProgress::default())
}

fn probe_all(probe: Option<&crate::media::FileProbe>, paths: &[String], progress: &ScanProgress) -> Vec<Result<ProbeInfo, String>> {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(probe) = probe.cloned() else {
        return paths
            .iter()
            .map(|p| {
                Ok(ProbeInfo {
                    format: Path::new(p).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
                    ..Default::default()
                })
            })
            .collect();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(paths.len().max(1));
        if n > 1 {
            let mut results: Vec<Option<Result<ProbeInfo, String>>> = vec![None; paths.len()];
            let next = std::sync::atomic::AtomicUsize::new(0);
            let out = std::sync::Mutex::new(&mut results);
            std::thread::scope(|sc| {
                for _ in 0..n {
                    sc.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= paths.len() {
                                break;
                            }
                            if progress.cancel.load(Relaxed) {
                                break;
                            }
                            let r = probe(&paths[i]);
                            progress.done.fetch_add(1, Relaxed);
                            out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                        }
                    });
                }
            });
            return results.into_iter().map(|r| r.unwrap_or_else(|| Err("not probed".into()))).collect();
        }
    }
    paths
        .iter()
        .map(|p| {
            if progress.cancel.load(Relaxed) {
                return Err("cancelled".into());
            }
            let r = probe(p);
            progress.done.fetch_add(1, Relaxed);
            r
        })
        .collect()
}

/// Copy `src` into `root`/`folders` as `name` (default: its own name), made unique with -1, -2…;
/// returns the new path. The copy is verified ([`crate::import_move::copy_new`]: a new file,
/// synced and checked against `probe_hash`, the content hash the probe computed — or compared
/// byte for byte with the source when there is none); a bad one is removed and the photo
/// reported as failed.
fn copy_into(root: &Path, src: &str, folders: &[String], name: Option<&str>, probe_hash: Option<&str>) -> Result<String, String> {
    let dir = folders.iter().fold(root.to_path_buf(), |d, f| d.join(f));
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| Path::new(src).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into()));
    let expect = probe_hash.and_then(lightcraft_preview::Hash128::parse);
    crate::import_move::copy_new(Path::new(src), &dir, &name, expect).map(|p| p.to_string_lossy().to_string())
}

/// What a [`scan`] needs from the session, so it can run on another thread (a folder on a network
/// share can take minutes to read).
pub struct ScanInput {
    probe: Option<crate::media::FileProbe>,
    skip: Option<PathBuf>,
    /// path → (photo, its summary) for files already in the library.
    by_path: HashMap<String, (PhotoId, ImportCandidate)>,
    by_hash: HashMap<String, PhotoId>,
    cache: HashMap<String, ProbeInfo>,
}

/// Progress and cancellation of a running [`scan_with`].
#[derive(Default)]
pub struct ScanProgress {
    /// Files to probe, set once the folders are expanded.
    pub total: std::sync::atomic::AtomicUsize,
    pub done: std::sync::atomic::AtomicUsize,
    pub cancel: std::sync::atomic::AtomicBool,
}

/// The result of [`scan_with`]: the candidates, and the probes to keep for the import that follows.
pub struct ScanOutput {
    pub candidates: Vec<ImportCandidate>,
    pub probes: HashMap<String, ProbeInfo>,
}

impl ScanInput {
    pub fn new(s: &mut Session, paths: &[String]) -> (Self, Vec<String>) {
        let skip = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        let mut by_path = HashMap::new();
        let mut by_hash = HashMap::new();
        for p in s.catalog.photos() {
            if let Source::File { path } = &p.source {
                let c = ImportCandidate {
                    path: path.clone(),
                    format: p.format.clone(),
                    kind: p.kind,
                    width: p.width,
                    height: p.height,
                    file_size: p.file_size,
                    captured: p.captured.clone(),
                    duplicate: Some("path".into()),
                    existing: Some(p.id.0),
                    ..Default::default()
                };
                by_path.insert(path.clone(), (p.id, c));
            }
            if let Some(h) = &p.content_hash {
                by_hash.insert(h.clone(), p.id);
            }
        }
        let input = ScanInput { probe: s.media.file_probe.clone(), skip, by_path, by_hash, cache: std::mem::take(&mut s.import_probes) };
        (input, paths.to_vec())
    }
}

/// Find and probe what importing `paths` would add, marking duplicates (by path, or by content
/// against the library and earlier candidates). Nothing is added; the probes are kept for the
/// import that follows.
pub fn scan(s: &mut Session, paths: &[String]) -> Vec<ImportCandidate> {
    let (input, paths) = ScanInput::new(s, paths);
    let out = scan_with(input, &paths, &ScanProgress::default());
    s.import_probes = out.probes;
    out.candidates
}

/// [`scan`] without the session, so it can run on a worker thread. Stops early (returning what it
/// has) when `progress.cancel` is set.
pub fn scan_with(mut input: ScanInput, paths: &[String], progress: &ScanProgress) -> ScanOutput {
    use std::sync::atomic::Ordering::Relaxed;
    let files = expand(paths, input.skip.as_deref());
    let todo: Vec<String> = files.iter().filter(|f| !input.by_path.contains_key(f.as_str())).cloned().collect();
    progress.total.store(todo.len(), Relaxed);
    // probes from a preceding `scan` are reused when the file is unchanged (same size)
    let cached: Vec<Option<ProbeInfo>> = todo
        .iter()
        .map(|f| {
            let info = input.cache.remove(f)?;
            let size = std::fs::metadata(f).map(|m| m.len()).ok();
            (size.is_none() || size == Some(info.file_size)).then_some(info)
        })
        .collect();
    let missing: Vec<String> = todo.iter().zip(&cached).filter(|(_, c)| c.is_none()).map(|(f, _)| f.clone()).collect();
    progress.done.store(todo.len() - missing.len(), Relaxed);
    let mut fresh = probe_all(input.probe.as_ref(), &missing, progress).into_iter();
    let probed: Vec<Result<ProbeInfo, String>> =
        cached.into_iter().map(|c| c.map(Ok).unwrap_or_else(|| fresh.next().unwrap_or_else(|| Err("not probed".into())))).collect();
    crate::memory::release();
    let mut probes: HashMap<String, Result<ProbeInfo, String>> = todo.into_iter().zip(probed).collect();
    let mut seen_hash: HashMap<String, Option<u64>> = input.by_hash.into_iter().map(|(h, id)| (h, Some(id.0))).collect();
    let mut out = Vec::with_capacity(files.len());
    let mut kept = HashMap::new();
    for f in files {
        if let Some((_, c)) = input.by_path.get(&f) {
            let mut c = c.clone();
            c.name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
            out.push(c);
            continue;
        }
        let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
        let mut c = ImportCandidate { path: f.clone(), name, ..Default::default() };
        match probes.remove(&f) {
            Some(Ok(info)) => {
                (c.format, c.kind, c.width, c.height, c.file_size, c.captured) =
                    (info.format.clone(), info.kind, info.width, info.height, info.file_size, info.captured.clone());
                c.preview_only = info.preview_only.clone();
                if let Some(h) = &info.content_hash {
                    match seen_hash.get(h) {
                        Some(existing) => {
                            c.duplicate = Some("content".into());
                            c.existing = *existing;
                        }
                        None => {
                            seen_hash.insert(h.clone(), None);
                        }
                    }
                }
                kept.insert(f, info);
            }
            Some(Err(e)) => c.error = Some(e),
            None => c.error = Some("not probed".into()),
        }
        out.push(c);
    }
    ScanOutput { candidates: out, probes: kept }
}

/// Import files/folders. See the module docs.
pub fn import(s: &mut Session, paths: &[String], mode: ImportMode) -> crate::Result<ImportReport> {
    import_with(s, paths, &ImportOptions { mode, ..Default::default() })
}

/// Import files/folders with options (preset, keywords). See the module docs.
///
/// The file-system half ([`ImportJob::prepare`]: expanding folders, probing, reading sidecars,
/// copying / placing files) and the catalog half ([`commit_prepared`]) are separate so the app can
/// run the first on a worker thread; here both run in turn.
pub fn import_with(s: &mut Session, paths: &[String], opts: &ImportOptions) -> crate::Result<ImportReport> {
    let mut job = ImportJob::new(s, opts.clone())?;
    let prepared = job.prepare(paths, &std::sync::atomic::AtomicBool::new(false));
    // probes a scan left for files not imported now stay for a later import
    s.import_probes.extend(std::mem::take(&mut job.cache));
    commit_prepared(s, &job.opts, &job.now, prepared)
}

/// Everything the file-system half of an import needs, detached from the session so it can run
/// on another thread (a network share or a card can take minutes). Built by [`ImportJob::new`];
/// [`ImportJob::prepare`] may be called for several batches in turn (renamed copies keep counting,
/// duplicates are found across batches); each batch is then [`commit_prepared`] on the session.
pub struct ImportJob {
    pub opts: ImportOptions,
    /// The mode in effect (Copy / Move become Add in a library without its own folder).
    mode: ImportMode,
    lib_dir: Option<PathBuf>,
    copy_root: Option<PathBuf>,
    /// path → (photo, a Local browse record?)
    by_path: HashMap<String, (PhotoId, bool)>,
    /// content hash → the photo that has it (`None`: a file earlier in this import)
    by_hash: HashMap<String, Option<PhotoId>>,
    probe: Option<crate::media::FileProbe>,
    file_bytes: Option<crate::merge::ByteReader>,
    /// Probes from a preceding scan (not read again).
    cache: HashMap<String, ProbeInfo>,
    naming: crate::sidecar::SidecarNaming,
    now: String,
    seq: usize,
    /// Files found so far (folders expanded) and handled so far, for progress.
    pub total: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

/// The outcome of [`ImportJob::prepare`] for one batch, in file order.
#[derive(Debug, Default)]
pub struct Prepared {
    pub scanned: usize,
    items: Vec<PreparedItem>,
}

#[derive(Debug)]
enum PreparedItem {
    /// A Local (browsed) record imported for real.
    Promote(PhotoId),
    Duplicate {
        path: String,
        existing: Option<PhotoId>,
        reason: &'static str,
        hash: Option<String>,
    },
    Failed(String, String),
    Kept(Kept),
    Ready(Box<ReadyFile>),
}

#[derive(Debug)]
struct ReadyFile {
    path: String,
    stored: String,
    info: ProbeInfo,
    sidecar: Option<crate::sidecar::SidecarData>,
    placed: Option<crate::import_move::Placed>,
}

impl Prepared {
    /// Undo what the batch did on disk that only a commit would make permanent: files placed by a
    /// Move (their sources were never touched). Copies stay, like copies of a failed commit.
    pub fn rollback(&self) {
        for it in &self.items {
            if let PreparedItem::Ready(r) = it
                && let Some(pl) = &r.placed
            {
                crate::import_move::rollback(pl);
            }
        }
    }

    /// Entries in the batch (after expanding folders).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl ImportJob {
    /// Snapshot what an import with `opts` needs from the session (no file-system calls). Takes
    /// the probes a preceding scan left ([`Session::import_probes`]).
    pub fn new(s: &mut Session, opts: ImportOptions) -> crate::Result<Self> {
        let mode = opts.mode;
        // Only a library on disk has an `Originals/` folder. The browser build keeps the bytes of
        // every added file in its own storage already, so "copy" there means "add".
        let transfer = matches!(mode, ImportMode::Copy | ImportMode::Move);
        let mode = if transfer && s.library.as_ref().is_some_and(|l| !l.on_disk) { ImportMode::Add } else { mode };
        if mode == ImportMode::Move && opts.local {
            return Err(crate::EngineError::Other("browsing a folder never moves its files".into()));
        }
        let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        // where copies go: the chosen folder, else the library's Originals/
        let copy_root = opts
            .destination
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| lib_dir.as_ref().map(|l| l.join("Originals")));
        if matches!(mode, ImportMode::Copy | ImportMode::Move) && copy_root.is_none() {
            return Err(crate::EngineError::Other("copying or moving needs a destination folder or an open library".into()));
        }
        let mut by_path = HashMap::new();
        let mut by_hash = HashMap::new();
        for p in s.catalog.photos() {
            if let Source::File { path } = &p.source {
                by_path.insert(path.clone(), (p.id, p.local));
            }
            if let Some(h) = &p.content_hash {
                by_hash.insert(h.clone(), Some(p.id));
            }
        }
        Ok(ImportJob {
            seq: opts.rename_start.max(1),
            opts,
            mode,
            lib_dir,
            copy_root,
            by_path,
            by_hash,
            probe: s.media.file_probe.clone(),
            file_bytes: s.media.file_bytes.clone(),
            cache: std::mem::take(&mut s.import_probes),
            naming: s.xmp.naming,
            now: (s.clock)(),
            total: Default::default(),
            done: Default::default(),
        })
    }

    /// The file-system half of importing `paths`: expand folders, skip known paths, probe, skip
    /// duplicates by content, read sidecars, copy or place (Move) the files. Nothing in the
    /// catalog changes. Stops before the next file once `cancel` is set.
    pub fn prepare(&mut self, paths: &[String], cancel: &std::sync::atomic::AtomicBool) -> Prepared {
        let files = self.expand(paths);
        self.prepare_files(files, cancel)
    }

    /// Expand `paths` (folders recursively, the library's own folder skipped) into the files an
    /// import would handle; counted in [`ImportJob::total`].
    pub fn expand(&self, paths: &[String]) -> Vec<String> {
        let files = expand(paths, self.lib_dir.as_deref());
        self.total.fetch_add(files.len(), std::sync::atomic::Ordering::Relaxed);
        files
    }

    /// When the import started ([`Session::clock`]), for [`commit_prepared`].
    pub fn now(&self) -> &str {
        &self.now
    }

    /// [`ImportJob::prepare`] for files [`ImportJob::expand`] returned.
    pub fn prepare_files(&mut self, files: Vec<String>, cancel: &std::sync::atomic::AtomicBool) -> Prepared {
        use std::sync::atomic::Ordering::Relaxed;
        let opts = self.opts.clone();
        let mode = self.mode;
        let mut out = Prepared { scanned: files.len(), items: Vec::new() };
        let mut todo = Vec::new();
        for f in files {
            match self.by_path.get(f.as_str()).copied() {
                Some((id, true)) if !opts.local => {
                    // a file that was only browsed (Local) joins the library when it is imported for real
                    self.by_path.insert(f, (id, false));
                    out.items.push(PreparedItem::Promote(id));
                    self.done.fetch_add(1, Relaxed);
                }
                Some((id, _)) => {
                    out.items.push(PreparedItem::Duplicate { path: f, existing: Some(id), reason: "path", hash: None });
                    self.done.fetch_add(1, Relaxed);
                }
                None => todo.push(f),
            }
        }

        // files a preceding scan already probed are not read again (a network share is slow to read)
        let cached: Vec<Option<ProbeInfo>> = todo.iter().map(|f| self.cache.remove(f)).collect();
        let missing: Vec<String> = todo.iter().zip(&cached).filter(|(_, c)| c.is_none()).map(|(f, _)| f.clone()).collect();
        let progress = ScanProgress::default();
        let mut fresh = probe_all(self.probe.as_ref(), &missing, &progress).into_iter();
        let probed: Vec<Result<ProbeInfo, String>> =
            cached.into_iter().map(|c| c.map(Ok).unwrap_or_else(|| fresh.next().unwrap_or_else(|| Err("not probed".into())))).collect();
        crate::memory::release();
        for (path, info) in todo.into_iter().zip(probed) {
            if cancel.load(Relaxed) {
                break;
            }
            self.done.fetch_add(1, Relaxed);
            let mut info = match info {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("import {path}: {e}");
                    out.items.push(PreparedItem::Failed(path, e));
                    continue;
                }
            };
            if let Some(h) = &info.content_hash
                && !opts.local
                && let Some(existing) = self.by_hash.get(h)
            {
                out.items.push(PreparedItem::Duplicate { path, existing: *existing, reason: "content", hash: Some(h.clone()) });
                continue;
            }
            // the XMP sidecar (or a raw's embedded XMP), read before copying: its capture time files
            // and names the copy when the file itself has none
            let raw = info.kind == MediaKind::Raw;
            let packet = crate::sidecar::find_sidecar(&path, self.naming)
                .and_then(|f| std::fs::read_to_string(f).ok())
                .or_else(|| info.xmp.clone().filter(|_| raw));
            let sidecar = packet.and_then(|x| match crate::sidecar::parse_sidecar(&x, raw) {
                Ok(sc) => Some(sc),
                Err(e) => {
                    log::warn!("import {path}: XMP: {e}");
                    None
                }
            });
            if info.captured.is_none() {
                info.captured = sidecar.as_ref().and_then(|sc| sc.captured.clone());
            }
            let mut placed = None;
            let stored = match (mode, &self.copy_root) {
                (ImportMode::Copy | ImportMode::Move, Some(root))
                    if !crate::import_move::inside(Path::new(&path), root)
                        && !self.lib_dir.as_ref().is_some_and(|l| crate::import_move::inside(Path::new(&path), l)) =>
                {
                    // the templates see the photo as it will be catalogued (undated: the import time)
                    let own = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let mut q = Photo::new(PhotoId(0), Source::File { path: path.clone() }, &own, &info.format, 0, 0, &self.now);
                    q.captured = info.captured.clone();
                    q.meta = info.meta.clone();
                    let name = opts.rename.as_deref().filter(|t| !t.trim().is_empty()).map(|t| {
                        let n = crate::rename::expand(t, &q, self.seq);
                        self.seq += 1;
                        n
                    });
                    let folders = opts.organize.folders(&q);
                    if mode == ImportMode::Move && !crate::import_move::is_symlink(Path::new(&path)) {
                        let dir = folders.iter().fold(root.to_path_buf(), |d, f| d.join(f));
                        let name = name.unwrap_or(own);
                        match crate::import_move::place(Path::new(&path), &dir, &name) {
                            Ok(pl) => {
                                let dst = pl.dst.to_string_lossy().to_string();
                                placed = Some(pl);
                                dst
                            }
                            Err(e) => {
                                out.items.push(PreparedItem::Failed(path, e));
                                continue;
                            }
                        }
                    } else {
                        if mode == ImportMode::Move {
                            let reason = "a symbolic link: the file it points to was copied, the link and its target stay";
                            out.items.push(PreparedItem::Kept(Kept { path: path.clone(), reason: reason.into() }));
                        }
                        match copy_into(root, &path, &folders, name.as_deref(), info.content_hash.as_deref()) {
                            Ok(p) => p,
                            Err(e) => {
                                out.items.push(PreparedItem::Failed(path, e));
                                continue;
                            }
                        }
                    }
                }
                (ImportMode::Move, _) => {
                    let reason = "already inside the destination or the library: added where it is";
                    out.items.push(PreparedItem::Kept(Kept { path: path.clone(), reason: reason.into() }));
                    path.clone()
                }
                _ => path.clone(),
            };
            if let Some(h) = &info.content_hash {
                self.by_hash.insert(h.clone(), None);
            }
            // Copy as DNG: the copied raw becomes a DNG (the copy is ours to replace)
            let mut stored = stored;
            if opts.convert_dng
                && mode == ImportMode::Copy
                && stored != path
                && info.kind == MediaKind::Raw
                && !info.format.eq_ignore_ascii_case("DNG")
            {
                match crate::cmd::convert::write_dng_with(self.file_bytes.as_ref(), &stored, String::new()) {
                    Ok(dng) => {
                        let _ = std::fs::remove_file(&stored);
                        stored = dng;
                        info.format = "DNG".into();
                    }
                    Err(e) => log::warn!("import {path}: copy as DNG: {e}"),
                }
            }
            out.items.push(PreparedItem::Ready(Box::new(ReadyFile { path, stored, info, sidecar, placed })));
        }
        out
    }
}

/// The catalog half of an import: add the photos [`ImportJob::prepare`] readied (one undoable op;
/// a Move's sources are removed once it is saved). On a failed commit a Move's placed files are
/// taken back.
pub fn commit_prepared(s: &mut Session, opts: &ImportOptions, now: &str, prepared: Prepared) -> crate::Result<ImportReport> {
    let now = now.to_string();
    let mut report = ImportReport { scanned: prepared.scanned, ..Default::default() };
    let mut ops = Vec::new();
    // Move: placed at the destination, sources untouched until the records are committed
    let mut placed: Vec<crate::import_move::Placed> = Vec::new();
    // photos added by this batch, by content (a duplicate of a file earlier in the import names it)
    let mut new_hash: HashMap<String, PhotoId> = HashMap::new();
    for it in prepared.items {
        match it {
            PreparedItem::Promote(id) => {
                if s.catalog.photo(id).is_some_and(|p| p.local) {
                    ops.push(Op::SetLocal { id, local: false });
                }
                report.imported.push(id.0);
            }
            PreparedItem::Duplicate { path, existing, reason, hash } => {
                let existing = existing.or_else(|| {
                    let h = hash?;
                    new_hash.get(&h).copied().or_else(|| s.catalog.photos().find(|p| p.content_hash.as_deref() == Some(h.as_str())).map(|p| p.id))
                });
                report.duplicates.push(Duplicate { path, existing: existing.map(|i| i.0), reason });
            }
            PreparedItem::Failed(path, e) => report.failed.push((path, e)),
            PreparedItem::Kept(k) => report.kept.push(k),
            PreparedItem::Ready(r) => {
                let ReadyFile { path, stored, info, sidecar, placed: pl } = *r;
                placed.extend(pl);
                let id = s.catalog.alloc_photo_id();
                if let Some(h) = &info.content_hash {
                    new_hash.insert(h.clone(), id);
                }
                let sidecar = sidecar.map(|sc| sc.resolve_label(&s.catalog)).filter(|sc| *sc != crate::sidecar::SidecarData::default());
                // a copy is catalogued under its new name
                let name = Path::new(&stored).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
                let mut p = Photo::new(id, Source::File { path: stored }, &name, &info.format, info.width, info.height, &now);
                p.kind = info.kind;
                p.file_size = info.file_size;
                p.captured = info.captured;
                p.meta = info.meta;
                p.as_shot_wb = info.as_shot_wb;
                p.content_hash = info.content_hash;
                p.embedded_lens = info.embedded_lens;
                p.preview_only = info.preview_only.clone();
                apply_import_defaults(s, &mut p);
                if let Some(sc) = &sidecar {
                    crate::sidecar::merge_into(&mut p, sc, &now);
                    report.sidecars += 1;
                }
                if let Some(mp) = opts.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
                    crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
                }
                for k in &opts.keywords {
                    let k = lightcraft_catalog::keywords::clean(k);
                    if !k.is_empty() && !p.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                        p.meta.keywords.push(k);
                    }
                }
                if let Some(preset) = &opts.preset {
                    let d = std::sync::Arc::new(preset.apply(&p.develop, 1.0));
                    let label = format!("Preset: {}", preset.name);
                    p.develop = d.clone();
                    p.edited = Some(now.clone());
                    p.history.push(lightcraft_catalog::HistoryStep { label, settings: d });
                }
                report.imported.push(id.0);
                p.local = opts.local;
                if opts.local {
                    // the state as browsed: a later change keeps the record from being forgotten
                    p.set_local_baseline();
                }
                ops.push(Op::AddPhoto { photo: Box::new(p) });
            }
        }
    }
    let log0 = s.pending_log.len();
    if !ops.is_empty()
        && let Err(e) = s.commit(&format!("Add {} Photo{}", ops.len(), if ops.len() == 1 { "" } else { "s" }), Op::Batch { ops })
    {
        // nothing was catalogued: take the moves back (their sources were never touched)
        placed.iter().for_each(crate::import_move::rollback);
        return Err(e);
    }
    if !placed.is_empty() {
        finish_moves(s, placed, log0, &mut report);
    }
    Ok(report)
}

/// Move, after the commit: save the library, then remove each source whose destination is
/// still intact. Nothing is removed when the library can't be saved.
fn finish_moves(s: &mut Session, placed: Vec<crate::import_move::Placed>, log0: usize, report: &mut ImportReport) {
    // (saving here clears the pending ops, so the sidecars `execute` would auto-write are written here)
    let auto: Vec<Op> = if s.xmp.auto_write { s.pending_log.get(log0..).map(<[Op]>::to_vec).unwrap_or_default() } else { Vec::new() };
    let before = s.pending_log.len();
    if let Err(e) = s.persist() {
        for p in placed {
            let reason = format!("the library could not be saved ({e}): the original stays; the photo uses the copy at {}", p.dst.display());
            report.kept.push(Kept { path: p.src.to_string_lossy().to_string(), reason });
        }
        return;
    }
    for p in placed {
        let from = p.src.to_string_lossy().to_string();
        match crate::import_move::finish(&p) {
            Ok(kept) => {
                let sidecars = p.sidecars.iter().map(|sc| sc.dst.to_string_lossy().to_string()).collect();
                report.moved.push(Moved { from, to: p.dst.to_string_lossy().to_string(), sidecars });
                report.kept.extend(kept.into_iter().map(|(path, reason)| Kept { path, reason }));
            }
            Err(reason) => {
                log::warn!("import: move {from}: {reason}");
                report.kept.push(Kept { path: from, reason });
            }
        }
    }
    if s.pending_log.len() < before && s.interaction.is_none() && !auto.is_empty() {
        s.auto_write_sidecars(&auto);
    }
}

/// The current local time as ISO 8601 (`YYYY-MM-DDTHH:MM:SS`, UTC on targets without a clock
/// offset); for [`Session::clock`] on native hosts.
pub fn system_clock() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        civil(secs)
    }
    #[cfg(target_arch = "wasm32")]
    {
        "2026-01-01T00:00:00".to_string()
    }
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SS` (UTC), proleptic Gregorian.
pub fn civil(secs: i64) -> String {
    lightcraft_catalog::dates::civil(secs)
}

impl Session {
    /// A thumbnail for an import candidate (not in the catalog): a raw's embedded preview, else a
    /// small render of the file. `slot` makes the job's photo id unique (`u64::MAX - slot`).
    pub fn candidate_thumb_job(&mut self, c: &ImportCandidate, edge: usize, slot: u64) -> Option<crate::media::QuickJob> {
        if c.error.is_some() {
            return None;
        }
        let id = PhotoId(u64::MAX - slot);
        let mut p = Photo::new(id, Source::File { path: c.path.clone() }, &c.name, &c.format, c.width.max(1), c.height.max(1), "");
        p.kind = c.kind;
        if let Some(info) = self.import_probes.get(&c.path) {
            p.as_shot_wb = info.as_shot_wb;
            p.embedded_lens = info.embedded_lens;
            p.preview_only = info.preview_only.clone();
        }
        p.develop = std::sync::Arc::new(p.import_defaults());
        let edge = edge.clamp(64, crate::media::SourceLevel::Thumb.max_edge());
        let level = crate::media::SourceLevel::Thumb;
        let source = self.media.origin_ref(&p.source, level.max_edge());
        let key = lightcraft_preview::Hasher128::new().str(&c.path).u64(c.file_size).u64(edge as u64).finish().0 as u64;
        let small = crate::media::RenderJob {
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: crate::media::source_info(&p),
            settings: p.develop.clone(),
            request: lightcraft_pipeline::RenderRequest::fit(edge, edge),
            key,
            cache: None,
            stages: None,
            view_cache: None,
        };
        let embedded = match (&self.media.preview_loader, c.kind) {
            (Some(l), MediaKind::Raw) => Some((c.path.clone(), l.clone(), edge)),
            _ => None,
        };
        Some(crate::media::QuickJob { photo: id, key, cached: Vec::new(), embedded, small: Some(Box::new(small)) })
    }

    /// Use the system clock for import/edit times (native hosts).
    pub fn with_system_clock(mut self) -> Self {
        self.clock = Box::new(system_clock);
        self
    }
}
