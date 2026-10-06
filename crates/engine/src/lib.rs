//! The LightCraft engine façade.
//!
//! Every user-visible action is a command with a stable id (`photo.rate`, `develop.set`,
//! `album.create`, `mask.add`…) and JSON parameters. The egui UI, the CLI, the control channel and
//! the MCP server all go through [`Session::execute`].
//!
//! State: a [`Catalog`] (mutated only by ops, so every change is undoable and journaled), the
//! library view (filter/sort/source), the selection, the develop clipboard, presets, and caches
//! of decoded source proxies. Rendering is done by [`RenderJob`]s that are `Send` so frontends can
//! run them off the UI thread.
#![forbid(unsafe_code)]

pub mod availability;
mod camera_preview;
pub mod cmd;
pub mod crs;
pub mod crs_masks;
pub mod demo;
pub mod devices;
pub mod export;
pub mod files;
pub mod fonts;
pub mod guard;
pub mod import;
mod import_move;
pub mod library;
pub mod media;
pub mod memory;
pub mod merge;
pub mod originals;
pub mod preset_import;
pub mod preset_luminar;
pub mod presets;
pub mod rename;
pub mod sidecar;
pub mod smart;
mod view;

use std::sync::Arc;

pub use cmd::{CommandInfo, CommandSpec, command_specs, find_command};
pub use fonts::{CRAFT_FONTS, CraftFont};
use lightcraft_catalog::{Catalog, Filter, Op, PhotoId, Sort};
use lightcraft_develop::DevelopSettings;
pub use media::{RenderJob, SourceLevel};
use serde_json::Value;
pub use view::{Browse, FilterChip, LibrarySource, Selection, filter_chips};
pub use {lightcraft_catalog as catalog, lightcraft_develop as develop, lightcraft_gpu as gpu, lightcraft_pipeline as pipeline};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("{0}")]
    Catalog(#[from] lightcraft_catalog::CatalogError),
    /// The command's change is applied (in memory, undoable) but its journal records could not
    /// be written. They stay queued and are written by the next successful save.
    #[error("saved in memory but not written to disk: {0}; LightCraft will retry")]
    NotSaved(String),
    /// Another process (the app, `lightcraft-cli`, another computer) has the library open.
    #[error("{0}")]
    LibraryInUse(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// One undo step: the inverse op and a label.
#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub label: String,
    pub op: Op,
    /// A folder to rename on disk (`from` → `to`) before `op` is applied: Rename / Move Folder
    /// (whose `op` relinks the photos inside). Never overwrites; refused when `to` exists.
    pub folder: Option<FolderMove>,
}

/// A folder renamed or moved on disk as part of an undo step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderMove {
    pub from: String,
    pub to: String,
}

impl FolderMove {
    fn reversed(&self) -> Self {
        Self { from: self.to.clone(), to: self.from.clone() }
    }
}

/// An in-progress slider drag / brush stroke: one undo step when it ends.
#[derive(Clone, Debug)]
pub struct Interaction {
    pub label: String,
    pub photo: PhotoId,
    pub original: Arc<DevelopSettings>,
}

/// Source of [`Session::visible_shared`] generations (process-wide, so two sessions never share one).
static VISIBLE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct Session {
    /// Auto Sync: edits to the active photo also change the other selected photos (the settings
    /// that changed, nothing else).
    pub auto_sync: bool,
    pub catalog: Catalog,
    pub source: LibrarySource,
    pub filter: Filter,
    pub sort: Sort,
    pub selection: Selection,
    /// The grid order for the current source/filter/sort (cached by catalog revision).
    /// Shared, so frontends can hold it across frames without copying ([`Session::visible_shared`]).
    visible: Arc<[PhotoId]>,
    visible_key: Option<(u64, String)>,
    /// Identifies the current `visible` list: a new value (unique in the process) every time it
    /// is recomputed, so frontends can key their per-view caches on it instead of hashing the ids.
    visible_gen: u64,
    /// Photos in the current source with no filter on (cached like `visible`).
    total: Option<((u64, String), usize)>,
    pub undo: Vec<UndoEntry>,
    pub redo: Vec<UndoEntry>,
    pub interaction: Option<Interaction>,
    /// Copied develop settings (partial JSON) for Paste.
    pub clipboard: Option<Value>,
    /// The folder on disk the [`LibrarySource::Folder`] view browses.
    pub browse: Option<Browse>,
    /// Copied metadata (`photo.copyMetadata`): photo.setMeta params.
    pub meta_clipboard: Option<Value>,
    /// The photo that was active before the current one (Paste Settings from Previous).
    pub previous_active: Option<PhotoId>,
    /// Groups last used for Copy (Lightroom remembers them).
    pub copy_groups: Vec<lightcraft_develop::SettingsGroup>,
    pub presets: Vec<lightcraft_develop::Preset>,
    /// Favourite profile ids (persisted with the library, like preset favourites).
    pub profile_favorites: Vec<String>,
    /// Recently applied profile ids, newest first (at most [`presets::RECENT_PROFILES`]).
    pub profile_recent: Vec<String>,
    /// Executed commands (actions / debugging / replay).
    pub journal: Vec<(String, Value)>,
    /// Ops applied since the last `drain_log` (for persistence).
    pending_log: Vec<Op>,
    pub media: media::MediaCache,
    /// Current time provider (ISO 8601); injectable for tests and wasm.
    pub clock: Box<dyn Fn() -> String + Send>,
    depth: u32,
    /// Selected mask (Masking panel), by mask id.
    pub active_mask: Option<u32>,
    /// Selected spot (Remove panel), by index into the active photo's spots.
    pub active_spot: Option<usize>,
    /// The persistent library this session writes to (`None` = in-memory only).
    pub library: Option<library::Library>,
    /// XMP sidecar preferences (persisted with the library).
    pub xmp: sidecar::XmpPrefs,
    /// Parameters of the last export (`app.export` params, minus targets), persisted in prefs.json.
    pub last_export: Option<serde_json::Value>,
    /// The user's export presets (built-ins: [`export::builtin_presets`]), persisted in prefs.json.
    pub export_presets: Vec<export::ExportPreset>,
    /// Metadata presets (`metadata.*`), persisted in prefs.json.
    pub metadata_presets: Vec<cmd::metadata::MetadataPreset>,
    /// Saved filter-bar settings (`filter.*`), persisted in prefs.json.
    pub filter_presets: Vec<cmd::filters::FilterPreset>,
    /// The user's point-curve presets (`curve.*`; built-ins: [`cmd::curves::builtin_presets`]),
    /// persisted in prefs.json.
    pub curve_presets: Vec<cmd::curves::CurvePreset>,
    /// Saved colour-label name sets.
    pub label_sets: Vec<cmd::manage::LabelSet>,
    /// Look-alike signatures by content key (Find Similar).
    pub signatures: std::collections::HashMap<String, [f32; 64]>,
    /// Imported `.cube` LUT profiles.
    pub lut_profiles: Vec<cmd::lut_profiles::LutProfile>,
    /// The target album B adds to (`None` = the Quick Collection).
    pub target_album: Option<lightcraft_catalog::AlbumId>,
    /// Auto import: files seen in the watched folder and their size then (a file is imported once
    /// its size held between two scans).
    pub auto_import_seen: std::collections::HashMap<String, u64>,
    /// Keyword sets (⌥1–⌥9 apply the current set's keywords), the one in use (`None` = Recent
    /// Keywords) and the recently added keywords, newest first.
    pub keyword_sets: Vec<cmd::keywords::KeywordSet>,
    pub keyword_set: Option<String>,
    pub recent_keywords: Vec<String>,
    /// Before/After: the "before" settings chosen per photo (this session; default: the photo's
    /// import state). See `cmd/before.rs`.
    pub before: std::collections::HashMap<PhotoId, Arc<DevelopSettings>>,
    /// File probes from the last import review (`library.importPreview`), reused by the import.
    pub import_probes: std::collections::HashMap<String, media::ProbeInfo>,
    /// The last (or running) Build Previews.
    pub preview_build: Option<std::sync::Arc<cmd::previews::PreviewBuild>>,
    /// Develop defaults applied on import (persisted in prefs.json).
    pub import_defaults: import::ImportDefaults,
    /// Disk budget of the library's thumbnail cache in MB (0 = default; persisted in prefs.json).
    pub cache_mb: u32,
    /// Where smart previews are kept when not in the library folder (persisted in prefs.json).
    pub smart_previews_dir: Option<std::path::PathBuf>,
    /// Untouched Local records of folders not browsed for this many days are forgotten when the
    /// library opens (0 = never; persisted in prefs.json). See `cmd/browse.rs`.
    pub forget_local_days: u32,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            auto_sync: false,
            catalog: Catalog::new(),
            source: LibrarySource::All,
            filter: Filter::default(),
            sort: Sort::default(),
            selection: Selection::default(),
            visible: Vec::new().into(),
            visible_key: None,
            visible_gen: 0,
            total: None,
            undo: Vec::new(),
            redo: Vec::new(),
            interaction: None,
            clipboard: None,
            meta_clipboard: None,
            browse: None,
            previous_active: None,
            copy_groups: lightcraft_develop::SettingsGroup::default_copy(),
            presets: presets::builtin(),
            profile_favorites: Vec::new(),
            profile_recent: Vec::new(),
            journal: Vec::new(),
            pending_log: Vec::new(),
            media: media::MediaCache::default(),
            clock: Box::new(|| "2026-09-30T12:00:00".to_string()),
            depth: 0,
            active_mask: None,
            active_spot: None,
            library: None,
            xmp: sidecar::XmpPrefs::default(),
            last_export: None,
            export_presets: Vec::new(),
            metadata_presets: Vec::new(),
            filter_presets: Vec::new(),
            curve_presets: Vec::new(),
            label_sets: Vec::new(),
            signatures: Default::default(),
            lut_profiles: Vec::new(),
            target_album: None,
            auto_import_seen: Default::default(),
            keyword_sets: Vec::new(),
            keyword_set: None,
            recent_keywords: Vec::new(),
            before: Default::default(),
            import_probes: Default::default(),
            preview_build: None,
            import_defaults: import::ImportDefaults::default(),
            cache_mb: 0,
            smart_previews_dir: None,
            forget_local_days: lightcraft_catalog::DEFAULT_FORGET_DAYS,
        }
    }

    /// A session with the procedurally generated demo library loaded.
    pub fn with_demo() -> Session {
        let mut s = Session::new();
        demo::load(&mut s);
        s
    }

    /// Run a command by id. THE entry point for every frontend.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(id.to_string(), why))?;
        let empty = Value::Object(Default::default());
        let params = if params.is_null() { &empty } else { params };
        self.run_command(id, spec.journal.then_some(params), |s| (spec.run)(s, params))
    }

    /// Run `f` as command `id` with what [`Session::execute`] does around every command: the
    /// panic guard, auto versions, XMP sidecar auto-write and the durable save (a failed save is
    /// `NotSaved`). Not journaled. The app's import task commits its batches this way.
    pub fn execute_fn(&mut self, id: &str, f: impl FnOnce(&mut Session) -> Result<Value>) -> Result<Value> {
        self.run_command(id, None, f)
    }

    fn run_command(&mut self, id: &str, journal: Option<&Value>, f: impl FnOnce(&mut Session) -> Result<Value>) -> Result<Value> {
        let log_start = self.pending_log.len();
        let was_active = self.active();
        self.depth += 1;
        // last-resort guard: a panic in a command is that command's error, not a crash
        let r = guard::catch(&format!("`{id}`"), || f(self)).unwrap_or_else(|e| Err(EngineError::Other(e)));
        self.depth -= 1;
        if self.depth == 0 && was_active.is_some() && self.active() != was_active {
            self.previous_active = was_active;
            if let Some(left) = was_active {
                self.auto_version(left);
            }
        }
        if r.is_ok()
            && let Some(params) = journal
            && self.depth == 0
        {
            self.journal.push((id.to_string(), params.clone()));
            if self.journal.len() > 10_000 {
                self.journal.drain(..1000);
            }
        }
        if r.is_ok() && self.depth == 0 && self.xmp.auto_write && self.interaction.is_none() && self.pending_log.len() > log_start {
            self.auto_write_sidecars(&self.pending_log[log_start..]);
        }
        if self.depth == 0 && self.library.is_some() {
            // Make the command durable before reporting success. When the command's own records
            // can't be appended, it fails with `NotSaved`: the change stays applied in memory and
            // queued, and the next save (any later command, or the frame loop) retries it. Other
            // persistence trouble (a failed compaction, an older queued change still unwritten by
            // a command that changed nothing) doesn't fail the command; `library.info` reports it.
            let produced = self.pending_log.len() > log_start;
            if let Err(e @ EngineError::NotSaved(_)) = self.persist()
                && produced
                && r.is_ok()
            {
                return Err(e);
            }
        }
        r
    }

    pub fn commands(&self) -> Vec<CommandInfo> {
        command_specs().iter().map(|c| c.info(self)).collect()
    }

    // ---------------------------------------------------------------- ops, undo

    /// Apply an op as one undoable step.
    pub fn commit(&mut self, label: &str, op: Op) -> Result<()> {
        let fwd = op.clone();
        let inv = self.catalog.apply(op)?;
        self.pending_log.push(fwd);
        self.undo.push(UndoEntry { label: label.to_string(), op: inv, folder: None });
        if self.undo.len() > 1000 {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(())
    }

    /// [`Session::commit`] for an op that goes with a folder already renamed on disk; the undo
    /// step renames it back (`folder` is the undo direction: current place → old place).
    pub(crate) fn commit_with_folder(&mut self, label: &str, op: Op, folder: FolderMove) -> Result<()> {
        self.commit(label, op)?;
        if let Some(e) = self.undo.last_mut() {
            e.folder = Some(folder);
        }
        Ok(())
    }

    /// Leaving photo `id` after editing it: keep its settings as an automatic version (when they
    /// differ from its latest version; at most [`AUTO_VERSIONS`] auto versions, oldest dropped).
    /// Saved with the library but not an undo step.
    pub fn auto_version(&mut self, id: PhotoId) {
        let Some(p) = self.catalog.photo(id) else { return };
        if !p.is_edited() || p.versions.last().is_some_and(|v| *v.settings == *p.develop) || self.interaction.is_some() {
            return;
        }
        let mut versions = p.versions.clone();
        let created = (self.clock)();
        let name = lightcraft_catalog::dates::display_time(&created);
        versions.push(lightcraft_catalog::Version { name, created, settings: p.develop.clone(), auto: true });
        let autos = versions.iter().filter(|v| v.auto).count();
        if autos > AUTO_VERSIONS
            && let Some(i) = versions.iter().position(|v| v.auto)
        {
            versions.remove(i);
        }
        let op = Op::SetVersions { id, versions };
        if self.catalog.apply(op.clone()).is_ok() {
            self.pending_log.push(op);
        }
    }

    /// Fold the last `n` undo steps into one (commands that commit step by step because each op
    /// depends on the state the previous one left).
    pub fn merge_undo(&mut self, n: usize, label: &str) {
        if n < 2 || n > self.undo.len() {
            return;
        }
        // a step that moves a folder on disk stays on its own
        if self.undo[self.undo.len() - n..].iter().any(|e| e.folder.is_some()) {
            return;
        }
        let tail = self.undo.split_off(self.undo.len() - n);
        let ops = tail.into_iter().rev().map(|e| e.op).collect();
        self.undo.push(UndoEntry { label: label.to_string(), op: Op::Batch { ops }, folder: None });
    }

    /// Apply without recording undo (interactive previews).
    fn apply_silent(&mut self, op: Op) -> Result<()> {
        self.catalog.apply(op)?;
        Ok(())
    }

    pub fn undo_step(&mut self) -> Result<String> {
        let e = self.undo.pop().ok_or_else(|| EngineError::Other("nothing to undo".into()))?;
        let redo = match self.apply_with_files(&e.op, e.folder.as_ref()) {
            Ok(r) => r,
            Err(err) => {
                self.undo.push(e);
                return Err(err);
            }
        };
        self.pending_log.push(e.op);
        self.redo.push(UndoEntry { label: e.label.clone(), op: redo, folder: e.folder.as_ref().map(FolderMove::reversed) });
        Ok(e.label)
    }

    pub fn redo_step(&mut self) -> Result<String> {
        let e = self.redo.pop().ok_or_else(|| EngineError::Other("nothing to redo".into()))?;
        let undo = match self.apply_with_files(&e.op, e.folder.as_ref()) {
            Ok(r) => r,
            Err(err) => {
                self.redo.push(e);
                return Err(err);
            }
        };
        self.pending_log.push(e.op);
        self.undo.push(UndoEntry { label: e.label.clone(), op: undo, folder: e.folder.as_ref().map(FolderMove::reversed) });
        Ok(e.label)
    }

    /// Apply an undo/redo op, first renaming the step's folder and moving the files its renames
    /// imply (all or nothing). Files that can't be moved back after a failure are reported and the
    /// library follows them (a logged change outside the undo history), so none goes missing; the
    /// folder is only moved back when no file was left behind at its new path.
    fn apply_with_files(&mut self, op: &Op, folder: Option<&FolderMove>) -> Result<Op> {
        let fs = rename::RealFs;
        if let Some(f) = folder {
            cmd::browse::rename_folder_on_disk(&f.from, &f.to).map_err(|e| EngineError::Other(format!("can't move the folder back: {e}")))?;
        }
        let undo_folder = || {
            if let Some(f) = folder {
                let _ = cmd::browse::rename_folder_on_disk(&f.to, &f.from);
            }
        };
        let moves = self.file_moves(op);
        if let Err(e) = rename::move_all(&fs, &moves) {
            if e.stuck.is_empty() {
                undo_folder();
            }
            return Err(EngineError::Other(format!("can't move the files back: {}", self.follow_stuck(op, e, false))));
        }
        match self.catalog.apply(op.clone()) {
            Ok(inv) => {
                if let Some(f) = folder {
                    cmd::browse::follow_folder(self, &f.from, &f.to);
                }
                Ok(inv)
            }
            Err(e) => {
                let back: Vec<(String, String)> = moves.iter().rev().map(|(a, b)| (b.clone(), a.clone())).collect();
                if let Err(be) = rename::move_all(&fs, &back) {
                    return Err(EngineError::Other(format!("{e}; the files could not all be moved back: {}", be.message)));
                }
                undo_folder();
                Err(e.into())
            }
        }
    }

    /// Ops applied since the last call, for the op-log store.
    pub fn drain_log(&mut self) -> Vec<Op> {
        std::mem::take(&mut self.pending_log)
    }

    // ---------------------------------------------------------------- develop edits

    /// The photo being edited (the active photo).
    pub fn active(&self) -> Option<PhotoId> {
        self.selection.active.filter(|id| self.catalog.photo(*id).is_some())
    }

    pub fn develop_of(&self, id: PhotoId) -> Option<Arc<DevelopSettings>> {
        self.catalog.photo(id).map(|p| p.develop.clone())
    }

    /// Change a photo's develop settings. During an interaction the change is previewed without an
    /// undo step; otherwise it is committed with a history entry.
    pub fn set_develop(&mut self, id: PhotoId, settings: DevelopSettings, label: &str) -> Result<()> {
        let now = (self.clock)();
        let settings = Arc::new(settings);
        if let Some(i) = &self.interaction
            && i.photo == id
        {
            return self.apply_silent(Op::SetDevelop { id, settings, label: label.into(), edited: Some(now) });
        }
        let mut ops = vec![self.develop_op(id, (*settings).clone(), label).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?];
        ops.extend(self.auto_sync_ops(id, &settings, label));
        let op = if ops.len() == 1 { ops.remove(0) } else { Op::Batch { ops } };
        self.commit(label, op)
    }

    /// With Auto Sync on, the ops that carry an edit of the active photo `id` (to `new`) over to the
    /// other selected photos: only the settings that changed; never spot removal or red eye (they
    /// belong to one photo's pixels), nor history / snapshot restores.
    fn auto_sync_ops(&self, id: PhotoId, new: &DevelopSettings, label: &str) -> Vec<Op> {
        if !self.auto_sync
            || self.active() != Some(id)
            || self.selection.ids.len() < 2
            || label.starts_with("History:")
            || label.starts_with("Restore ")
        {
            return Vec::new();
        }
        let Some(old) = self.develop_of(id) else { return Vec::new() };
        let Some(mut delta) = json_delta(&old.to_json(), &new.to_json()) else { return Vec::new() };
        if let Some(o) = delta.as_object_mut() {
            for k in ["spots", "red_eye", "version"] {
                o.remove(k);
            }
            if o.is_empty() {
                return Vec::new();
            }
        }
        self.selection
            .ids
            .iter()
            .filter(|x| **x != id)
            .filter_map(|x| self.develop_of(*x).map(|d| (*x, d)))
            .filter_map(|(x, d)| {
                let synced = lightcraft_develop::apply_partial(&d, &delta, 1.0);
                (synced != *d).then(|| self.develop_op(x, synced, label)).flatten()
            })
            .collect()
    }

    /// The op that sets a photo's develop settings and appends a History entry (for batches).
    pub fn develop_op(&self, id: PhotoId, settings: DevelopSettings, label: &str) -> Option<Op> {
        self.catalog.photo(id)?;
        let settings = Arc::new(settings);
        let step = lightcraft_catalog::HistoryStep { label: label.into(), settings: settings.clone() };
        Some(Op::Batch {
            ops: vec![Op::SetDevelop { id, settings, label: label.into(), edited: Some((self.clock)()) }, Op::PushHistory { id, step }],
        })
    }

    pub fn begin_interaction(&mut self, label: &str) -> Result<()> {
        if self.interaction.is_some() {
            self.end_interaction()?;
        }
        let id = self.active().ok_or_else(|| EngineError::Other("no active photo".into()))?;
        let original = self.develop_of(id).unwrap_or_default();
        self.interaction = Some(Interaction { label: label.into(), photo: id, original });
        Ok(())
    }

    /// Commit the interaction as one undo step (no-op if nothing changed).
    pub fn end_interaction(&mut self) -> Result<()> {
        let Some(i) = self.interaction.take() else { return Ok(()) };
        let Some(cur) = self.develop_of(i.photo) else { return Ok(()) };
        if *cur == *i.original {
            return Ok(());
        }
        // Restore the original silently, then commit the final value as one step.
        self.apply_silent(Op::SetDevelop { id: i.photo, settings: i.original.clone(), label: i.label.clone(), edited: None })?;
        self.set_develop(i.photo, (*cur).clone(), &i.label)
    }

    pub fn cancel_interaction(&mut self) -> Result<()> {
        if let Some(i) = self.interaction.take() {
            self.apply_silent(Op::SetDevelop { id: i.photo, settings: i.original, label: i.label, edited: None })?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- library view

    /// Photos shown in the grid/filmstrip for the current source, filter and sort.
    pub fn visible(&mut self) -> &[PhotoId] {
        let mut key = (self.catalog.revision, format!("{:?}|{:?}|{:?}|{:?}", self.source, self.filter, self.sort, self.browse));
        if self.source == LibrarySource::Missing && self.media.availability.is_background() {
            // the view fills in as the background checks find files gone
            key.1.push_str(&format!("|{}", self.media.availability.generation()));
        }
        if self.visible_key.as_ref() != Some(&key) {
            // "in the last N days" rules count back from the session's clock
            lightcraft_catalog::rules::set_now(Some((self.clock)()));
            let mut f = self.source.to_filter(&self.filter, &self.catalog);
            if self.source == LibrarySource::Folder {
                // no folder chosen: nothing (an empty path matches nothing)
                let b = self.browse.clone().unwrap_or_default();
                f.folder = Some(b.path);
                f.subfolders = b.subfolders;
            }
            let mut visible = self.catalog.query(&f, &self.sort);
            if matches!(self.source, LibrarySource::Album(_))
                && self.sort.key == lightcraft_catalog::SortKey::CaptureDate
                && let LibrarySource::Album(a) = self.source
                && let Some(al) = self.catalog.album(a)
                && !al.is_smart()
                && self.filter == Filter::default()
            {
                let order = al.photos.clone();
                visible.sort_by_key(|id| order.iter().position(|x| x == id).unwrap_or(usize::MAX));
                if !self.sort.ascending {
                    visible.reverse();
                }
            }
            if self.source == LibrarySource::RecentlyAdded {
                // newest import first, whatever the sort (the grid groups by import day)
                let cat = &self.catalog;
                visible.sort_by(|a, b| {
                    let key = |id: &PhotoId| cat.photo(*id).map(|p| p.imported.clone()).unwrap_or_default();
                    key(b).cmp(&key(a)).then(a.cmp(b))
                });
            }
            if self.source == LibrarySource::Missing {
                // only the photos the query kept (library photos, not Local browse records) are checked
                let (cat, avail) = (&self.catalog, &self.media.availability);
                visible.retain(|id| cmd::missing::is_missing(cat, avail, *id));
            }
            if self.source != LibrarySource::RecentlyDeleted {
                visible = self.catalog.arrange_stacks(&visible);
            }
            self.visible = visible.into();
            self.visible_gen = VISIBLE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.visible_key = Some(key);
        }
        &self.visible
    }

    /// Photos in the current source (folder, album, …) before the filter bar and search narrow
    /// them; `None` where that is not a separate number (Missing Photos).
    pub fn source_total(&mut self) -> Option<usize> {
        if self.source == LibrarySource::Missing {
            return None;
        }
        let key = (self.catalog.revision, format!("{:?}|{:?}", self.source, self.browse));
        if self.total.as_ref().map(|t| &t.0) != Some(&key) {
            let mut f = self.source.to_filter(&Filter::default(), &self.catalog);
            if self.source == LibrarySource::Folder {
                let b = self.browse.clone().unwrap_or_default();
                f.folder = Some(b.path);
                f.subfolders = b.subfolders;
            }
            let n = self.catalog.query(&f, &self.sort).len();
            self.total = Some((key, n));
        }
        self.total.as_ref().map(|t| t.1)
    }

    pub fn visible_cloned(&mut self) -> Vec<PhotoId> {
        self.visible().to_vec()
    }

    /// The visible photos without copying them, with their generation: equal generations mean
    /// the very same list (same catalog revision, source, filter, sort), so a frontend can cache
    /// whatever it derives from the list under that number.
    pub fn visible_shared(&mut self) -> (u64, Arc<[PhotoId]>) {
        self.visible();
        (self.visible_gen, self.visible.clone())
    }

    /// Targets of photo commands: explicit `ids`/`id` param, else the selection.
    pub fn targets(&self, p: &Value) -> Vec<PhotoId> {
        if let Some(a) = p.get("ids").and_then(Value::as_array) {
            return a.iter().filter_map(Value::as_u64).map(PhotoId).collect();
        }
        if let Some(id) = p.get("id").and_then(Value::as_u64) {
            return vec![PhotoId(id)];
        }
        if self.selection.ids.is_empty() { self.selection.active.into_iter().collect() } else { self.selection.ids.clone() }
    }
}

/// Automatic versions kept per photo.
pub const AUTO_VERSIONS: usize = 20;

/// The parts of `new` that differ from `old` (objects recurse; anything else is taken whole).
pub fn json_delta(old: &Value, new: &Value) -> Option<Value> {
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            let m: serde_json::Map<String, Value> =
                b.iter().filter_map(|(k, nv)| json_delta(a.get(k).unwrap_or(&Value::Null), nv).map(|d| (k.clone(), d))).collect();
            (!m.is_empty()).then_some(Value::Object(m))
        }
        (a, b) if a == b => None,
        (_, b) => Some(b.clone()),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_color;
#[cfg(test)]
mod tests_export;
#[cfg(test)]
mod tests_forget_local;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_import_move;
#[cfg(test)]
mod tests_libops;
#[cfg(test)]
mod tests_library;
#[cfg(test)]
mod tests_merge;
#[cfg(test)]
mod tests_organize;
#[cfg(test)]
mod tests_persist;
#[cfg(test)]
mod tests_prefs;
#[cfg(test)]
mod tests_settings_files;
#[cfg(test)]
mod tests_spots;
#[cfg(test)]
mod tests_xmp;
