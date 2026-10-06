//! The LightCraft library (catalog).
//!
//! State changes only through [`Op`]s. [`Catalog::apply`] returns the inverse op, which gives:
//! - **persistence**: ops are appended to a log (JSON lines) and replayed on load after the last
//!   snapshot — crash-safe and diff-friendly (see [`journal`]);
//! - **undo/redo**: the engine keeps inverse ops;
//! - **determinism**: replaying the log reproduces the state exactly (property-tested).
//!
//! **Catalog format version** ([`journal::VERSION`], see [`journal`] → *Format versions*): adding
//! an [`Op`] variant or a serialized field means bumping it. Newer builds read every older format
//! (and upgrade it on open); older builds refuse a newer library with [`CatalogError::Newer`]
//! instead of reading part of it.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod dates;
pub mod journal;
pub mod keywords;
pub mod local;
pub mod lock;
pub mod model;
pub mod query;
pub mod rules;
pub mod safe_file;
pub mod stacks;
pub mod store;

use std::collections::BTreeMap;
use std::sync::Arc;

pub use dates::{DateRun, GroupBy};
pub use journal::{Journal, LoadReport, PersistStats, SnapshotPolicy, SnapshotTiming};
pub use keywords::KeywordNode;
use lightcraft_develop::DevelopSettings;
pub use local::{DEFAULT_FORGET_DAYS, ForgetPlan, folder_of};
pub use lock::{LibraryLock, LockError, LockOwner};
pub use model::*;
pub use query::{DateGroup, Filter, RatingOp, Sort, SortKey};
pub use rules::{Match, Rule, RuleSet};
use serde::{Deserialize, Serialize};
pub use store::{FsStore, MemStore, Store};

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CatalogError {
    #[error("no such photo {0:?}")]
    NoPhoto(PhotoId),
    #[error("no such album {0:?}")]
    NoAlbum(AlbumId),
    #[error("no such stack {0:?}")]
    NoStack(StackId),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("corrupt catalog data: {0}")]
    Corrupt(String),
    #[error("catalog storage: {0}")]
    Io(String),
    /// The library was written by a newer LightCraft (a newer catalog format, or a change this
    /// version doesn't know). Nothing was read into the session and nothing was modified.
    #[error("this library was written by a newer version of LightCraft ({0}); update LightCraft to open it. The library was left unchanged.")]
    Newer(String),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

/// Maximum History entries kept per photo.
pub const HISTORY_LIMIT: usize = 200;

/// Every catalog mutation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    AddPhoto {
        photo: Box<Photo>,
    },
    RemovePhoto {
        id: PhotoId,
    },
    SetRating {
        id: PhotoId,
        rating: u8,
    },
    SetFlag {
        id: PhotoId,
        flag: Flag,
    },
    SetLabel {
        id: PhotoId,
        label: Option<ColorLabel>,
    },
    SetDevelop {
        id: PhotoId,
        settings: Arc<DevelopSettings>,
        label: String,
        edited: Option<String>,
    },
    SetMeta {
        id: PhotoId,
        meta: Box<Meta>,
    },
    SetDeleted {
        id: PhotoId,
        deleted: bool,
    },
    /// Browsed-only (`true`) or part of the library.
    SetLocal {
        id: PhotoId,
        local: bool,
    },
    SetVersions {
        id: PhotoId,
        versions: Vec<Version>,
    },
    SetHistory {
        id: PhotoId,
        history: Vec<HistoryStep>,
    },
    /// Append one History entry (dropping the oldest beyond [`HISTORY_LIMIT`]). Logged instead of
    /// a full `SetHistory` so each edit costs one step in the op log.
    PushHistory {
        id: PhotoId,
        step: HistoryStep,
    },
    AddAlbum {
        album: Album,
    },
    RemoveAlbum {
        id: AlbumId,
    },
    RenameAlbum {
        id: AlbumId,
        name: String,
    },
    MoveAlbum {
        id: AlbumId,
        parent: Option<AlbumId>,
    },
    SetAlbumPhotos {
        id: AlbumId,
        photos: Vec<PhotoId>,
    },
    SetAlbumCover {
        id: AlbumId,
        cover: Option<PhotoId>,
    },
    /// Replace a smart album's rules.
    SetAlbumRules {
        id: AlbumId,
        rules: Box<Filter>,
    },
    AddStack {
        stack: Stack,
    },
    RemoveStack {
        id: StackId,
    },
    /// Replace a stack's members (first = top) and collapsed state.
    SetStack {
        id: StackId,
        photos: Vec<PhotoId>,
        collapsed: bool,
    },
    /// Set a photo's capture time (ISO 8601 local time; `None` = unknown).
    SetCaptured {
        id: PhotoId,
        captured: Option<String>,
    },
    /// Assisted culling scores.
    SetAnalysis {
        id: PhotoId,
        analysis: Option<crate::Analysis>,
    },
    /// Rename a photo: its file name and the source it points to. Applying the op never touches
    /// the disk — the engine moves the file before it commits (and on undo/redo).
    SetFile {
        id: PhotoId,
        file_name: String,
        source: Source,
    },
    /// Point a photo at its file's new location (a moved or renamed original found again). Unlike
    /// [`Op::SetFile`], undo and redo never move files.
    Relink {
        id: PhotoId,
        file_name: String,
        source: Source,
        /// The file's format when it changes too (Convert to DNG).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// What a photo's file is now (after it changed on disk: Reload).
    SetContent {
        id: PhotoId,
        width: u32,
        height: u32,
        file_size: u64,
        #[serde(default)]
        content_hash: Option<String>,
        /// Why the file can only be shown from its embedded preview (see [`Photo::preview_only`]);
        /// `None` = its raw data decodes (or it isn't a raw).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preview_only: Option<String>,
    },
    /// The name shown for a colour label (`None` = its colour's name).
    SetLabelName {
        label: ColorLabel,
        name: Option<String>,
    },
    /// When a Local folder was last browsed (ISO 8601; `None` = forget the time). Not an undo
    /// step: it drives forgetting untouched Local records (see [`local`]).
    SetBrowsed {
        folder: String,
        at: Option<String>,
    },
    /// Several ops as one step (undo applies the inverses in reverse).
    Batch {
        ops: Vec<Op>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    photos: BTreeMap<PhotoId, Arc<Photo>>,
    albums: BTreeMap<AlbumId, Album>,
    next_photo: u64,
    next_album: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    stacks: BTreeMap<StackId, Stack>,
    #[serde(default)]
    next_stack: u64,
    /// Custom colour label names.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    label_names: BTreeMap<ColorLabel, String>,
    /// When each Local folder was last browsed (folder path → ISO 8601), see [`local`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    browsed: BTreeMap<String, String>,
    /// Increments on every applied op.
    #[serde(skip)]
    pub revision: u64,
}

impl Catalog {
    pub fn new() -> Catalog {
        Catalog { next_photo: 1, next_album: 1, next_stack: 1, ..Default::default() }
    }

    // ---- ids

    pub fn alloc_photo_id(&mut self) -> PhotoId {
        let id = PhotoId(self.next_photo.max(1));
        self.next_photo = id.0 + 1;
        id
    }
    pub fn alloc_album_id(&mut self) -> AlbumId {
        let id = AlbumId(self.next_album.max(1));
        self.next_album = id.0 + 1;
        id
    }
    pub fn alloc_stack_id(&mut self) -> StackId {
        let id = StackId(self.next_stack.max(1));
        self.next_stack = id.0 + 1;
        id
    }

    // ---- reads

    pub fn photo(&self, id: PhotoId) -> Option<&Arc<Photo>> {
        self.photos.get(&id)
    }
    pub fn photos(&self) -> impl Iterator<Item = &Arc<Photo>> {
        self.photos.values()
    }
    pub fn len(&self) -> usize {
        self.photos.len()
    }
    pub fn is_empty(&self) -> bool {
        self.photos.is_empty()
    }
    pub fn album(&self, id: AlbumId) -> Option<&Album> {
        self.albums.get(&id)
    }
    pub fn albums(&self) -> impl Iterator<Item = &Album> {
        self.albums.values()
    }
    /// The Quick Collection, once something was added to it.
    pub fn quick_collection(&self) -> Option<AlbumId> {
        self.albums.values().find(|a| a.quick).map(|a| a.id)
    }

    /// Albums (regular and smart) that contain the photo.
    pub fn albums_of(&self, id: PhotoId) -> Vec<AlbumId> {
        let Some(p) = self.photos.get(&id) else { return Vec::new() };
        self.albums.values().filter(|a| self.album_contains(a.id, p)).map(|a| a.id).collect()
    }

    /// Whether album `id` contains `p` (a smart album evaluates its rules; deleted photos are in
    /// no smart album).
    pub fn album_contains(&self, id: AlbumId, p: &Photo) -> bool {
        match self.albums.get(&id) {
            Some(Album { smart: Some(rules), .. }) => !p.deleted && rules.matches(p, self),
            Some(a) => a.photos.contains(&p.id),
            None => false,
        }
    }

    /// The photos of an album: the stored list, or a smart album's current matches (id order).
    pub fn album_photos(&self, id: AlbumId) -> Vec<PhotoId> {
        match self.albums.get(&id) {
            Some(Album { smart: Some(_), .. }) => self.photos.values().filter(|p| self.album_contains(id, p)).map(|p| p.id).collect(),
            Some(a) => a.photos.clone(),
            None => Vec::new(),
        }
    }

    /// Number of photos shown for an album in the sources list (excludes deleted photos).
    pub fn album_count(&self, id: AlbumId) -> usize {
        match self.albums.get(&id) {
            // counted in place: no id list is built just for its length
            Some(Album { smart: Some(_), .. }) => self.photos.values().filter(|p| self.album_contains(id, p)).count(),
            Some(a) => a.photos.iter().filter(|p| self.photos.get(p).is_some_and(|p| !p.deleted)).count(),
            None => 0,
        }
    }

    /// Smart-album rules must not reference another smart album (no recursion) or deleted photos.
    fn validate_rules(&self, rules: &Filter) -> Result<()> {
        if rules.deleted {
            return Err(CatalogError::Invalid("smart album rules can't select deleted photos".into()));
        }
        if let Some(a) = rules.album
            && self.albums.get(&a).is_some_and(Album::is_smart)
        {
            return Err(CatalogError::Invalid("smart album rules can't reference another smart album".into()));
        }
        Ok(())
    }

    /// The name of a colour label: its custom name, else the colour (`Red`).
    pub fn label_name(&self, l: ColorLabel) -> String {
        self.label_names.get(&l).cloned().unwrap_or_else(|| format!("{l:?}"))
    }
    /// The custom name of a colour label, if any.
    pub fn custom_label_name(&self, l: ColorLabel) -> Option<&str> {
        self.label_names.get(&l).map(String::as_str)
    }
    /// The label a name stands for: a custom name first, then a colour's own name (any case).
    pub fn label_from_name(&self, name: &str) -> Option<ColorLabel> {
        let name = name.trim();
        self.label_names.iter().find(|(_, n)| n.trim().eq_ignore_ascii_case(name)).map(|(l, _)| *l).or_else(|| ColorLabel::parse(name))
    }

    // ---- writes

    fn photo_mut(&mut self, id: PhotoId) -> Result<&mut Photo> {
        self.photos.get_mut(&id).map(Arc::make_mut).ok_or(CatalogError::NoPhoto(id))
    }
    fn album_mut(&mut self, id: AlbumId) -> Result<&mut Album> {
        self.albums.get_mut(&id).ok_or(CatalogError::NoAlbum(id))
    }

    /// Apply an op; returns its inverse. On error nothing changes.
    pub fn apply(&mut self, op: Op) -> Result<Op> {
        let inv = self.apply_inner(op)?;
        self.revision += 1;
        Ok(inv)
    }

    fn apply_inner(&mut self, op: Op) -> Result<Op> {
        Ok(match op {
            Op::AddPhoto { photo } => {
                if self.photos.contains_key(&photo.id) {
                    return Err(CatalogError::Invalid(format!("photo {:?} exists", photo.id)));
                }
                let id = photo.id;
                self.next_photo = self.next_photo.max(id.0 + 1);
                self.photos.insert(id, Arc::new(*photo));
                Op::RemovePhoto { id }
            }
            Op::RemovePhoto { id } => {
                let p = self.photos.remove(&id).ok_or(CatalogError::NoPhoto(id))?;
                // album membership is restored by the batch the engine builds (see `delete_permanently`)
                Op::AddPhoto { photo: Box::new((*p).clone()) }
            }
            Op::SetRating { id, rating } => {
                if rating > 5 {
                    return Err(CatalogError::Invalid("rating must be 0..=5".into()));
                }
                let p = self.photo_mut(id)?;
                let old = std::mem::replace(&mut p.rating, rating);
                Op::SetRating { id, rating: old }
            }
            Op::SetFlag { id, flag } => {
                let p = self.photo_mut(id)?;
                Op::SetFlag { id, flag: std::mem::replace(&mut p.flag, flag) }
            }
            Op::SetLabel { id, label } => {
                let p = self.photo_mut(id)?;
                Op::SetLabel { id, label: std::mem::replace(&mut p.label, label) }
            }
            Op::SetDevelop { id, settings, label, edited } => {
                let p = self.photo_mut(id)?;
                let old = std::mem::replace(&mut p.develop, settings);
                let old_edit = std::mem::replace(&mut p.edited, edited);
                Op::SetDevelop { id, settings: old, label, edited: old_edit }
            }
            Op::SetMeta { id, meta } => {
                let p = self.photo_mut(id)?;
                Op::SetMeta { id, meta: Box::new(std::mem::replace(&mut p.meta, *meta)) }
            }
            Op::SetDeleted { id, deleted } => {
                let p = self.photo_mut(id)?;
                Op::SetDeleted { id, deleted: std::mem::replace(&mut p.deleted, deleted) }
            }
            Op::SetLocal { id, local } => {
                let p = self.photo_mut(id)?;
                Op::SetLocal { id, local: std::mem::replace(&mut p.local, local) }
            }
            Op::SetVersions { id, versions } => {
                let p = self.photo_mut(id)?;
                Op::SetVersions { id, versions: std::mem::replace(&mut p.versions, versions) }
            }
            Op::SetHistory { id, history } => {
                let p = self.photo_mut(id)?;
                Op::SetHistory { id, history: std::mem::replace(&mut p.history, history) }
            }
            Op::PushHistory { id, step } => {
                let p = self.photo_mut(id)?;
                let old = p.history.clone();
                p.history.push(step);
                if p.history.len() > HISTORY_LIMIT {
                    let n = p.history.len() - HISTORY_LIMIT;
                    p.history.drain(..n);
                }
                Op::SetHistory { id, history: old }
            }
            Op::AddAlbum { album } => {
                if self.albums.contains_key(&album.id) {
                    return Err(CatalogError::Invalid(format!("album {:?} exists", album.id)));
                }
                if let Some(parent) = album.parent
                    && !self.albums.get(&parent).is_some_and(|a| a.folder)
                {
                    return Err(CatalogError::Invalid("parent must be an existing folder".into()));
                }
                if let Some(rules) = &album.smart {
                    if album.folder || !album.photos.is_empty() {
                        return Err(CatalogError::Invalid("a smart album holds rules, not photos".into()));
                    }
                    self.validate_rules(rules)?;
                }
                let id = album.id;
                self.next_album = self.next_album.max(id.0 + 1);
                self.albums.insert(id, album);
                Op::RemoveAlbum { id }
            }
            Op::RemoveAlbum { id } => {
                if self.albums.values().any(|a| a.parent == Some(id)) {
                    return Err(CatalogError::Invalid("folder is not empty".into()));
                }
                let a = self.albums.remove(&id).ok_or(CatalogError::NoAlbum(id))?;
                Op::AddAlbum { album: a }
            }
            Op::RenameAlbum { id, name } => {
                let a = self.album_mut(id)?;
                Op::RenameAlbum { id, name: std::mem::replace(&mut a.name, name) }
            }
            Op::MoveAlbum { id, parent } => {
                if let Some(p) = parent {
                    if p == id || !self.albums.get(&p).is_some_and(|a| a.folder) {
                        return Err(CatalogError::Invalid("parent must be another folder".into()));
                    }
                    // no cycles
                    let mut cur = Some(p);
                    while let Some(c) = cur {
                        if c == id {
                            return Err(CatalogError::Invalid("cannot move a folder into itself".into()));
                        }
                        cur = self.albums.get(&c).and_then(|a| a.parent);
                    }
                }
                let a = self.album_mut(id)?;
                Op::MoveAlbum { id, parent: std::mem::replace(&mut a.parent, parent) }
            }
            Op::SetAlbumPhotos { id, photos } => {
                let a = self.album_mut(id)?;
                if (a.folder || a.smart.is_some()) && !photos.is_empty() {
                    return Err(CatalogError::Invalid(
                        if a.folder { "folders can't hold photos" } else { "smart albums update automatically" }.into(),
                    ));
                }
                Op::SetAlbumPhotos { id, photos: std::mem::replace(&mut a.photos, photos) }
            }
            Op::SetAlbumCover { id, cover } => {
                let a = self.album_mut(id)?;
                Op::SetAlbumCover { id, cover: std::mem::replace(&mut a.cover, cover) }
            }
            Op::SetAlbumRules { id, rules } => {
                self.validate_rules(&rules)?;
                let a = self.album_mut(id)?;
                let Some(old) = a.smart.as_mut() else {
                    return Err(CatalogError::Invalid("not a smart album".into()));
                };
                Op::SetAlbumRules { id, rules: std::mem::replace(old, rules) }
            }
            Op::AddStack { stack } => {
                if self.stacks.contains_key(&stack.id) {
                    return Err(CatalogError::Invalid(format!("stack {:?} exists", stack.id)));
                }
                self.validate_stack(stack.id, &stack.photos)?;
                let id = stack.id;
                self.next_stack = self.next_stack.max(id.0 + 1);
                self.stacks.insert(id, stack);
                Op::RemoveStack { id }
            }
            Op::RemoveStack { id } => {
                let s = self.stacks.remove(&id).ok_or(CatalogError::NoStack(id))?;
                Op::AddStack { stack: s }
            }
            Op::SetStack { id, photos, collapsed } => {
                if !self.stacks.contains_key(&id) {
                    return Err(CatalogError::NoStack(id));
                }
                self.validate_stack(id, &photos)?;
                let s = self.stacks.get_mut(&id).ok_or(CatalogError::NoStack(id))?;
                let old = Op::SetStack { id, photos: std::mem::replace(&mut s.photos, photos), collapsed: s.collapsed };
                s.collapsed = collapsed;
                old
            }
            Op::SetAnalysis { id, analysis } => {
                let p = self.photo_mut(id)?;
                Op::SetAnalysis { id, analysis: std::mem::replace(&mut p.analysis, analysis) }
            }
            Op::SetCaptured { id, captured } => {
                if let Some(c) = &captured
                    && stacks::iso_seconds(c).is_none()
                {
                    return Err(CatalogError::Invalid(format!("not a date: {c}")));
                }
                let p = self.photo_mut(id)?;
                Op::SetCaptured { id, captured: std::mem::replace(&mut p.captured, captured) }
            }
            Op::Relink { id, file_name, source, format } => {
                let p = self.photo_mut(id)?;
                let old_name = std::mem::replace(&mut p.file_name, file_name);
                let old_format = format.map(|f| std::mem::replace(&mut p.format, f));
                Op::Relink { id, file_name: old_name, source: std::mem::replace(&mut p.source, source), format: old_format }
            }
            Op::SetContent { id, width, height, file_size, content_hash, preview_only } => {
                let p = self.photo_mut(id)?;
                Op::SetContent {
                    id,
                    width: std::mem::replace(&mut p.width, width),
                    height: std::mem::replace(&mut p.height, height),
                    file_size: std::mem::replace(&mut p.file_size, file_size),
                    content_hash: std::mem::replace(&mut p.content_hash, content_hash),
                    preview_only: std::mem::replace(&mut p.preview_only, preview_only),
                }
            }
            Op::SetFile { id, file_name, source } => {
                if file_name.trim().is_empty() {
                    return Err(CatalogError::Invalid("empty file name".into()));
                }
                let p = self.photo_mut(id)?;
                let old_name = std::mem::replace(&mut p.file_name, file_name);
                Op::SetFile { id, file_name: old_name, source: std::mem::replace(&mut p.source, source) }
            }
            Op::SetLabelName { label, name } => {
                let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
                let old = match name {
                    Some(n) => self.label_names.insert(label, n),
                    None => self.label_names.remove(&label),
                };
                Op::SetLabelName { label, name: old }
            }
            Op::SetBrowsed { folder, at } => {
                let folder = crate::query::folder_key(&folder);
                let old = match at {
                    Some(t) => self.browsed.insert(folder.clone(), t),
                    None => self.browsed.remove(&folder),
                };
                Op::SetBrowsed { folder, at: old }
            }
            Op::Batch { ops } => {
                let mut inverses = Vec::with_capacity(ops.len());
                for op in ops {
                    match self.apply_inner(op) {
                        Ok(inv) => inverses.push(inv),
                        Err(e) => {
                            // roll back what was applied
                            for inv in inverses.into_iter().rev() {
                                let _ = self.apply_inner(inv);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Op::Batch { ops: inverses }
            }
        })
    }

    /// Ops that remove a photo permanently including album and stack memberships (one undoable
    /// batch).
    pub fn delete_permanently_ops(&self, id: PhotoId) -> Op {
        let mut ops: Vec<Op> = self
            .albums
            .values()
            .filter(|a| a.photos.contains(&id))
            .map(|a| Op::SetAlbumPhotos { id: a.id, photos: a.photos.iter().copied().filter(|p| *p != id).collect() })
            .collect();
        ops.extend(self.remove_from_stacks_ops(&[id]));
        ops.push(Op::RemovePhoto { id });
        Op::Batch { ops }
    }

    // ---- persistence

    /// Full snapshot as JSON.
    pub fn to_snapshot(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_snapshot(s: &str) -> Result<Catalog> {
        serde_json::from_str(s).map_err(|e| CatalogError::Corrupt(e.to_string()))
    }

    /// Replay an op log (JSON lines) on top of `self`. A torn final line (crash mid-write) is ignored.
    pub fn replay(&mut self, log: &str) -> Result<usize> {
        let lines: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).collect();
        let mut n = 0;
        for (i, line) in lines.iter().enumerate() {
            match serde_json::from_str::<Op>(line) {
                Ok(op) => {
                    self.apply(op)?;
                    n += 1;
                }
                Err(e) if i + 1 == lines.len() => {
                    let _ = e;
                    break;
                }
                Err(e) => return Err(CatalogError::Corrupt(format!("log line {}: {e}", i + 1))),
            }
        }
        Ok(n)
    }

    pub fn op_to_log_line(op: &Op) -> String {
        let mut s = serde_json::to_string(op).unwrap_or_default();
        s.push('\n');
        s
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_background;
#[cfg(test)]
mod tests_format_version;
#[cfg(test)]
mod tests_journal;
#[cfg(test)]
mod tests_local;
#[cfg(test)]
mod tests_lock;
#[cfg(test)]
mod tests_torn_append;
