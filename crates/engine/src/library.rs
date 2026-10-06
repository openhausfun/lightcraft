//! A persistent library: a directory holding the catalog (op log + snapshot), user presets, the
//! last view state and the preview cache.
//!
//! ```text
//! LightCraft Library/
//!   catalog.snap   catalog.log      (lightcraft-catalog journal)
//!   presets.json   view.json        (user presets + favourites; last source/filter/sort/selection)
//!   prefs.json     (library preferences: XMP sidecars, import defaults, cache size, last export)
//!   thumbs/        (rendered thumbnail cache, safe to delete)
//!   Originals/     (photos imported with "copy into library")
//! ```
//!
//! The files live in [`Store`]s: a directory ([`FsStore`]) natively, or any other implementation
//! via [`Session::open_library_in`] (the browser build keeps them in OPFS or IndexedDB).
//!
//! Every top-level [`Session::execute`] persists the ops it produced (fsynced) before returning,
//! so a crash loses at most the command in flight. When that write fails the command returns
//! [`EngineError::NotSaved`]: its change stays applied in memory and queued, and every later save
//! retries the queue ([`Session::unsaved`] reports it meanwhile), so nothing is lost once the
//! disk is writable again. The log is compacted into a snapshot when it
//! grows (see [`lightcraft_catalog::SnapshotPolicy`]; written by a worker thread on native, see
//! [`lightcraft_catalog::journal`]) and on [`Session::close_library`].

use std::path::{Path, PathBuf};

use lightcraft_catalog::{FsStore, Journal, LibraryLock, LoadReport, Store};
use lightcraft_develop::Preset;
use serde::{Deserialize, Serialize};

use crate::{EngineError, LibrarySource, Result, Selection, Session};

/// Library directory name inside the user's Pictures folder.
pub const DEFAULT_NAME: &str = "LightCraft Library";

/// The default library location: `$LIGHTCRAFT_LIBRARY` if set, else `~/Pictures/LightCraft Library`
/// (`%USERPROFILE%\Pictures\LightCraft Library` on Windows).
pub fn default_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LIGHTCRAFT_LIBRARY").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") }?;
    Some(PathBuf::from(home).join("Pictures").join(DEFAULT_NAME))
}

pub struct Library {
    /// The library directory, or a descriptive pseudo-path when the library isn't on disk.
    pub dir: PathBuf,
    /// The library is a real directory: photos can be copied into `Originals/` and thumbnails are
    /// cached in `thumbs/`.
    pub on_disk: bool,
    journal: Journal,
    /// presets.json, view.json, prefs.json.
    files: Box<dyn Store>,
    /// What happened when the catalog was loaded.
    pub report: LoadReport,
    /// Last persistence error (shown by the UI; ops stay pending and are retried).
    pub last_error: Option<String>,
    /// Why the last append of queued ops failed; `None` once a save succeeds. While set, changes
    /// exist only in memory ([`Session::unsaved`]).
    pub unsaved_error: Option<String>,
    /// When the frame loop may retry a failed append ([`Session::persist_if_dirty`] backs off).
    retry_at: Option<web_time::Instant>,
    /// What forgetting untouched Local records did when the library opened.
    pub forgot_local: Option<lightcraft_catalog::ForgetPlan>,
    presets_written: String,
    view_written: Vec<u8>,
    /// Settings files that were unreadable or damaged when the library opened (`library.info` →
    /// `settingsWarnings`; shown by the UI once, see [`Session::take_library_warnings`]).
    pub settings_warnings: Vec<String>,
    warnings_reported: usize,
    /// Settings files that couldn't be read or set aside: never overwritten this session.
    blocked: Vec<&'static str>,
    /// Keeps other processes out of this library while it's open (last: released after the
    /// journal's background snapshot has landed).
    lock: Option<LibraryLock>,
}

/// Where a library's files live (see [`Session::open_library_in`]).
pub struct LibraryStores {
    /// Shown as the library's location (`library.info`).
    pub dir: PathBuf,
    /// The catalog journal (`catalog.snap`, `catalog.log`).
    pub catalog: Box<dyn Store>,
    /// presets.json, view.json, prefs.json.
    pub files: Box<dyn Store>,
    /// `dir` is a real directory (enables `Originals/` copies and the `thumbs/` disk cache).
    pub on_disk: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PresetsFile {
    user: Vec<Preset>,
    favorites: Vec<String>,
    /// Favourite profiles, and the recently applied ones (newest first).
    profile_favorites: Vec<String>,
    profile_recent: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ViewFile {
    source: LibrarySource,
    browse: Option<crate::Browse>,
    filter: lightcraft_catalog::Filter,
    sort: lightcraft_catalog::Sort,
    selection: Selection,
}

impl Library {
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
    #[cfg(test)]
    pub(crate) fn journal_mut(&mut self) -> &mut Journal {
        &mut self.journal
    }
    pub fn thumbs_dir(&self) -> PathBuf {
        self.dir.join("thumbs")
    }
    pub fn originals_dir(&self) -> PathBuf {
        self.dir.join("Originals")
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PrefsFile {
    xmp: crate::sidecar::XmpPrefs,
    /// Parameters of the last export (for Export with Previous).
    #[serde(alias = "last_export")]
    last_export: Option<serde_json::Value>,
    /// The user's export presets.
    export_presets: Vec<crate::export::ExportPreset>,
    /// Metadata presets.
    metadata_presets: Vec<crate::cmd::metadata::MetadataPreset>,
    /// Filter presets.
    filter_presets: Vec<crate::cmd::filters::FilterPreset>,
    /// Point-curve presets.
    curve_presets: Vec<crate::cmd::curves::CurvePreset>,
    /// Colour-label name sets.
    label_sets: Vec<crate::cmd::manage::LabelSet>,
    /// Imported LUT profiles.
    lut_profiles: Vec<crate::cmd::lut_profiles::LutProfile>,
    /// Keyword sets, the one in use, recent keywords.
    keyword_sets: Vec<crate::cmd::keywords::KeywordSet>,
    keyword_set: Option<String>,
    recent_keywords: Vec<String>,
    /// Develop defaults for imported photos.
    import: crate::import::ImportDefaults,
    /// Thumbnail disk cache budget (MB, 0 = default).
    cache_mb: u32,
    /// Folder for smart previews (default: `Smart Previews` in the library).
    smart_previews_dir: Option<String>,
    /// Days after which untouched Local records of unbrowsed folders are forgotten (missing =
    /// the default, 0 = never).
    forget_local_days: Option<u32>,
}

fn presets_json(s: &Session) -> String {
    let presets = &s.presets;
    let f = PresetsFile {
        user: presets.iter().filter(|p| !p.builtin).cloned().collect(),
        favorites: presets.iter().filter(|p| p.builtin && p.favorite).map(|p| p.id.clone()).collect(),
        profile_favorites: s.profile_favorites.clone(),
        profile_recent: s.profile_recent.clone(),
    };
    serde_json::to_string_pretty(&f).unwrap_or_default()
}

/// Finish a background compaction that is done (cheap otherwise). A failed one lost nothing (the
/// log is kept whole) and is retried later; it is reported like other persistence errors.
fn poll_compaction(lib: &mut Library) {
    if let Err(e) = lib.journal.poll() {
        log::error!("library: compaction: {e}");
        lib.last_error = Some(format!("compaction: {e}"));
    }
}

/// How long the frame loop waits before retrying a failed append (commands retry at once).
const RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// `a` and `b` name the same directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// This program, for the lock owner note ("LightCraft", "lightcraft-cli").
fn program_name() -> String {
    let exe = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()));
    match exe.as_deref() {
        Some("lightcraft") | None => "LightCraft".into(),
        Some(other) => other.to_string(),
    }
}

/// Problems reading the settings files (`presets.json`, `prefs.json`, `view.json`) when the library
/// opened (issue #103). A missing file is not a problem (defaults). A file that doesn't parse is
/// kept as `<name>.corrupt-<unix time>` before the defaults are used (so the next save can't
/// lose it); a file that can't be read at all (locked by another program, I/O error) is never
/// written this session, so its content survives until the library is opened again.
#[derive(Default)]
struct SettingsLoad {
    warnings: Vec<String>,
    /// Files not to overwrite this session.
    blocked: Vec<&'static str>,
}

impl SettingsLoad {
    fn read<T: serde::de::DeserializeOwned>(&mut self, store: &mut dyn Store, name: &'static str) -> Option<T> {
        let bytes = match store.read(name) {
            Ok(b) => b?,
            Err(e) => {
                log::error!("library: {name}: {e}");
                self.blocked.push(name);
                self.warnings.push(format!(
                    "{name} couldn't be read ({e}). LightCraft uses the defaults for now and won't overwrite the file; reopen the library to try again."
                ));
                return None;
            }
        };
        let err = match serde_json::from_slice(&bytes) {
            Ok(v) => return Some(v),
            Err(e) => e,
        };
        let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let keep = format!("{name}.corrupt-{secs}");
        log::error!("library: {name} is damaged: {err}");
        match store.write_atomic(&keep, &bytes) {
            Ok(()) => self.warnings.push(format!("{name} is damaged ({err}). It was kept as {keep}, and the defaults are used.")),
            Err(w) => {
                self.blocked.push(name);
                self.warnings.push(format!(
                    "{name} is damaged ({err}) and couldn't be set aside ({w}). LightCraft uses the defaults and won't overwrite the file."
                ));
            }
        }
        None
    }
}

impl Session {
    /// Open (or create) the library at `dir` into this session, replacing its catalog. With
    /// `seed_demo`, a newly created library starts with the procedural demo photos.
    ///
    /// The library is locked for this session ([`lightcraft_catalog::lock`]): if another process
    /// has it open, this fails with [`EngineError::LibraryInUse`] and nothing is read or changed.
    /// Reopening the library this session already has open keeps its lock.
    pub fn open_library(&mut self, dir: impl AsRef<Path>, seed_demo: bool) -> Result<&LoadReport> {
        let dir = dir.as_ref().to_path_buf();
        let open = || FsStore::open(&dir).map_err(|e| EngineError::Other(format!("can't open library {}: {e}", dir.display())));
        let stores = LibraryStores { catalog: Box::new(open()?), files: Box::new(open()?), on_disk: true, dir: dir.clone() };
        // the library open in this session (same directory): hand its lock over
        let reused = self.library.as_mut().filter(|l| same_dir(&l.dir, &dir)).and_then(|l| l.lock.take());
        let reusing = reused.is_some();
        let mut lock = match reused {
            Some(l) => Some(l),
            None => Some(LibraryLock::acquire(&dir, &program_name()).map_err(|e| EngineError::LibraryInUse(e.to_string()))?),
        };
        if let Err(e) = self.open_stores(stores, seed_demo, &mut lock) {
            if reusing && let Some(lib) = self.library.as_mut() {
                lib.lock = lock.take();
            }
            return Err(e);
        }
        self.loaded_report()
    }

    /// Open (or create) a library whose files live in `stores` (e.g. browser storage), replacing
    /// this session's catalog. Not locked: the host guards against a second opener (see
    /// [`Session::open_library`] for directories).
    pub fn open_library_in(&mut self, stores: LibraryStores, seed_demo: bool) -> Result<&LoadReport> {
        self.open_stores(stores, seed_demo, &mut None)?;
        self.loaded_report()
    }

    fn loaded_report(&self) -> Result<&LoadReport> {
        match &self.library {
            Some(lib) => Ok(&lib.report),
            None => Err(EngineError::Other("library closed while opening".into())),
        }
    }

    /// Open `stores`; the new library takes `lock` once nothing can fail any more.
    fn open_stores(&mut self, stores: LibraryStores, seed_demo: bool, lock: &mut Option<LibraryLock>) -> Result<()> {
        let LibraryStores { dir, catalog, mut files, on_disk } = stores;
        self.media.smart_dir = on_disk.then(|| crate::smart::dir(&dir));
        // the current library may be these same files: let its background snapshot land first
        if let Some(old) = self.library.as_mut()
            && let Err(e) = old.journal.wait()
        {
            log::error!("library: {e}");
        }
        let (mut journal, catalog, report) = Journal::open(catalog)?;
        self.catalog = catalog;
        self.undo.clear();
        self.redo.clear();
        self.interaction = None;
        self.pending_log.clear();
        self.selection = Selection::default();
        self.source = LibrarySource::All;
        if report.created && seed_demo {
            crate::demo::load(self);
            journal.snapshot(&self.catalog)?;
        }
        let mut settings = SettingsLoad::default();
        // presets
        if let Some(f) = settings.read::<PresetsFile>(files.as_mut(), "presets.json") {
            for p in &mut self.presets {
                p.favorite = p.builtin && f.favorites.contains(&p.id) || (!p.builtin && p.favorite);
            }
            for u in f.user {
                if !self.presets.iter().any(|p| p.id == u.id) {
                    self.presets.push(u);
                }
            }
            let known = |id: &String| crate::presets::profile(id).is_some();
            self.profile_favorites = f.profile_favorites.into_iter().filter(known).collect();
            self.profile_recent = f.profile_recent.into_iter().filter(known).take(crate::presets::RECENT_PROFILES).collect();
        }
        // preferences
        let prefs = settings.read::<PrefsFile>(files.as_mut(), "prefs.json").unwrap_or_default();
        self.xmp = prefs.xmp;
        self.last_export = prefs.last_export;
        self.export_presets = prefs.export_presets;
        self.metadata_presets = prefs.metadata_presets;
        self.filter_presets = prefs.filter_presets;
        self.curve_presets = prefs.curve_presets;
        self.label_sets = prefs.label_sets;
        self.lut_profiles = prefs.lut_profiles;
        crate::cmd::lut_profiles::register_all(self);
        self.keyword_sets = prefs.keyword_sets;
        self.keyword_set = prefs.keyword_set;
        self.recent_keywords = prefs.recent_keywords;
        self.import_defaults = prefs.import;
        self.cache_mb = prefs.cache_mb;
        self.forget_local_days = prefs.forget_local_days.unwrap_or(lightcraft_catalog::DEFAULT_FORGET_DAYS);
        self.smart_previews_dir = prefs.smart_previews_dir.filter(|_| on_disk).map(PathBuf::from);
        if let Some(d) = &self.smart_previews_dir {
            self.media.smart_dir = Some(d.clone());
        }
        // view state
        if let Some(v) = settings.read::<ViewFile>(files.as_mut(), "view.json") {
            self.source = v.source;
            self.browse = v.browse;
            self.filter = v.filter;
            self.sort = v.sort;
            self.selection = v.selection;
            self.selection.ids.retain(|id| self.catalog.photo(*id).is_some());
            self.selection.active = self.selection.active.filter(|id| self.catalog.photo(*id).is_some());
        }
        if self.selection.active.is_none()
            && let Some(first) = self.visible_cloned().first()
        {
            self.selection = Selection::single(*first);
        }
        let presets_written = presets_json(self);
        // photo ids are per library: drop decoded sources of the previous one
        self.media.clear_sources();
        if on_disk {
            self.media.attach_disk_cache(&dir.join("thumbs"), self.cache_bytes());
        }
        let view_written = self.view_json();
        self.library = Some(Library {
            dir,
            on_disk,
            journal,
            files,
            report,
            last_error: None,
            unsaved_error: None,
            retry_at: None,
            forgot_local: None,
            presets_written,
            view_written,
            settings_warnings: settings.warnings,
            warnings_reported: 0,
            blocked: settings.blocked,
            lock: lock.take(),
        });
        // forget untouched Local records of folders not browsed for a while (journaled at once)
        if self.forget_local_days > 0 {
            let plan = self.forget_local(false, None);
            if !plan.evict.is_empty() {
                self.compact_soon();
            } else if let Err(e) = self.persist() {
                log::error!("library: {e}");
            }
            if let Some(lib) = self.library.as_mut() {
                lib.forgot_local = Some(plan);
            }
        }
        Ok(())
    }

    /// Write pending ops to the log (fsynced), compact when due, and save changed presets.
    /// Called after every top-level command; cheap when nothing changed. Fails (with
    /// [`EngineError::NotSaved`]) only when the queued ops couldn't be written; they stay queued.
    pub fn persist(&mut self) -> Result<()> {
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        if !self.pending_log.is_empty() {
            if let Err(e) = lib.journal.append(&self.pending_log) {
                // the ops stay queued (and applied in memory): the next persist retries them
                let reason = e.to_string();
                log::error!("library: {} change(s) not written to disk: {reason}", self.pending_log.len());
                lib.last_error = Some(reason.clone());
                lib.unsaved_error = Some(reason.clone());
                lib.retry_at = Some(web_time::Instant::now() + RETRY_BACKOFF);
                return Err(EngineError::NotSaved(reason));
            }
            if lib.unsaved_error.take().is_some() {
                log::info!("library: queued changes written to disk");
            }
            lib.retry_at = None;
            self.pending_log.clear();
            lib.last_error = None;
        }
        poll_compaction(lib);
        // Never snapshot mid-interaction: the catalog then holds an uncommitted preview value.
        // The snapshot is written by a worker thread (natively); appends go on meanwhile.
        // A failed compaction loses nothing (the log is kept whole and compacted later): it is
        // reported (`library.info` → `lastError`), not returned.
        if lib.journal.wants_snapshot()
            && self.interaction.is_none()
            && let Err(e) = lib.journal.snapshot_in_background(&self.catalog)
        {
            log::error!("library: compaction: {e}");
            lib.last_error = Some(format!("compaction: {e}"));
        }
        let presets = presets_json(self);
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        if presets != lib.presets_written && !lib.blocked.contains(&"presets.json") {
            if let Err(e) = lib.files.write_atomic("presets.json", presets.as_bytes()) {
                log::error!("library: presets: {e}");
            } else {
                lib.presets_written = presets;
            }
        }
        Ok(())
    }

    /// Persist if commands left ops pending (cheap; frontends call it once per frame for state
    /// changed outside [`Session::execute`]).
    pub fn persist_if_dirty(&mut self) {
        let backing_off = self.library.as_ref().and_then(|l| l.retry_at).is_some_and(|t| web_time::Instant::now() < t);
        if self.library.is_some() && !self.pending_log.is_empty() && !backing_off {
            let _ = self.persist();
        } else if let Some(lib) = self.library.as_mut() {
            poll_compaction(lib);
        }
    }

    /// Changes applied in memory but not yet written to disk because saving failed: the number of
    /// queued ops and why (`None` when everything is saved, or no library is open). Retried by
    /// every command and by [`Session::persist_if_dirty`].
    pub fn unsaved(&self) -> Option<(usize, &str)> {
        let lib = self.library.as_ref()?;
        lib.unsaved_error.as_deref().filter(|_| !self.pending_log.is_empty()).map(|e| (self.pending_log.len(), e))
    }

    /// Flush everything and write a snapshot (on quit). Ends an open interaction first.
    ///
    /// The snapshot is written even when appending the queued ops fails: it is a fresh file
    /// holding everything in memory, so it saves those ops too (the log handle may be the only
    /// thing that's broken). Fails only if nothing could be saved; the ops then stay queued.
    pub fn close_library(&mut self) -> Result<()> {
        if self.library.is_none() {
            return Ok(());
        }
        let _ = self.end_interaction();
        let persisted = self.persist();
        self.save_view();
        let unlogged = if persisted.is_err() { self.pending_log.len() as u64 } else { 0 };
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        let before = lib.journal.seq();
        let snapshot = lib.journal.snapshot_with_unlogged(&self.catalog, unlogged);
        if unlogged > 0 && lib.journal.seq() == before + unlogged {
            // the snapshot holds the queued ops: they are saved, never append them again
            log::info!("library: {unlogged} queued change(s) saved in the closing snapshot");
            self.pending_log.clear();
            lib.unsaved_error = None;
            lib.retry_at = None;
            lib.last_error = None;
        }
        match (persisted, snapshot) {
            (_, Ok(())) => Ok(()),
            // neither the log nor the snapshot took the queued ops
            (Err(EngineError::NotSaved(e)), Err(s)) if lib.journal.seq() == before => Err(EngineError::NotSaved(format!("{e}; snapshot: {s}"))),
            (_, Err(s)) => Err(s.into()),
        }
    }

    fn view_json(&self) -> Vec<u8> {
        let view = ViewFile {
            source: self.source,
            browse: self.browse.clone(),
            filter: self.filter.clone(),
            sort: self.sort,
            selection: self.selection.clone(),
        };
        serde_json::to_vec_pretty(&view).unwrap_or_default()
    }

    /// Save the view state (source, filter, sort, selection) if it changed since it was last
    /// written. Native hosts get this from [`Session::close_library`]; the browser host calls it
    /// periodically, since a tab can be closed without notice.
    pub fn save_view(&mut self) {
        if self.library.is_none() {
            return;
        }
        let v = self.view_json();
        let Some(lib) = self.library.as_mut() else { return };
        if v != lib.view_written && !lib.blocked.contains(&"view.json") {
            match lib.files.write_atomic("view.json", &v) {
                Ok(()) => lib.view_written = v,
                Err(e) => log::error!("library: view: {e}"),
            }
        }
    }

    /// Save the library preferences (no-op for in-memory sessions).
    pub fn save_prefs(&mut self) -> Result<()> {
        let v = serde_json::to_vec_pretty(&PrefsFile {
            xmp: self.xmp,
            last_export: self.last_export.clone(),
            export_presets: self.export_presets.clone(),
            metadata_presets: self.metadata_presets.clone(),
            filter_presets: self.filter_presets.clone(),
            curve_presets: self.curve_presets.clone(),
            label_sets: self.label_sets.clone(),
            lut_profiles: self.lut_profiles.clone(),
            keyword_sets: self.keyword_sets.clone(),
            keyword_set: self.keyword_set.clone(),
            recent_keywords: self.recent_keywords.clone(),
            import: self.import_defaults.clone(),
            cache_mb: self.cache_mb,
            smart_previews_dir: self.smart_previews_dir.as_ref().map(|d| d.to_string_lossy().to_string()),
            forget_local_days: Some(self.forget_local_days),
        })
        .unwrap_or_default();
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        if lib.blocked.contains(&"prefs.json") {
            return Err(EngineError::Other(
                "prefs: prefs.json couldn't be read when the library opened, so it isn't overwritten; reopen the library to save preferences".into(),
            ));
        }
        lib.files.write_atomic("prefs.json", &v).map_err(|e| EngineError::Other(format!("prefs: {e}")))
    }

    /// Settings-file warnings of the open library not handed out yet (the UI shows each once).
    pub fn take_library_warnings(&mut self) -> Vec<String> {
        let Some(lib) = self.library.as_mut() else { return vec![] };
        let new = lib.settings_warnings[lib.warnings_reported..].to_vec();
        lib.warnings_reported = lib.settings_warnings.len();
        new
    }

    /// The thumbnail disk cache budget in bytes ([`Session::cache_mb`], else the default).
    pub fn cache_bytes(&self) -> u64 {
        if self.cache_mb == 0 { crate::media::DISK_CACHE_BYTES } else { u64::from(self.cache_mb) << 20 }
    }

    /// Change the thumbnail disk cache budget (MB, 0 = default): re-attaches the cache, which
    /// trims it to the new size. Saved with the library preferences.
    pub fn set_cache_mb(&mut self, mb: u32) -> Result<()> {
        self.cache_mb = mb;
        if let Some(lib) = self.library.as_ref().filter(|l| l.on_disk) {
            let dir = lib.thumbs_dir();
            self.media.attach_disk_cache(&dir, self.cache_bytes());
        }
        self.save_prefs()
    }

    /// Save, then start compacting in the background (natively; synchronously elsewhere): after
    /// a change that shrinks the catalog a lot, so the smaller snapshot replaces the old one.
    /// Errors are logged and reported like other persistence errors (nothing is lost).
    pub fn compact_soon(&mut self) {
        if let Err(e) = self.persist() {
            log::error!("library: {e}");
            return;
        }
        if self.interaction.is_some() {
            return;
        }
        let Some(lib) = self.library.as_mut() else { return };
        if let Err(e) = lib.journal.snapshot_in_background(&self.catalog) {
            log::error!("library: compaction: {e}");
            lib.last_error = Some(format!("compaction: {e}"));
        }
    }

    /// Compact the log into a snapshot now.
    pub fn compact_library(&mut self) -> Result<()> {
        self.persist()?;
        if self.interaction.is_some() {
            return Err(EngineError::Other("can't compact during an interaction".into()));
        }
        if let Some(lib) = self.library.as_mut() {
            lib.journal.snapshot(&self.catalog)?;
        }
        Ok(())
    }
}
