//! Forgetting untouched Local browse records.
//!
//! Browsing a folder (Local) catalogues its photos as `local` records so they can be shown,
//! rated and edited in place. Most are only ever looked at, and on a large disk they can be most
//! of the catalog. A record is **forgotten** (removed from the catalog, never from the disk —
//! files and XMP sidecars are not touched) when all of these hold:
//!
//! - it is a Local record ([`Photo::local`]) of a file, not deleted, not a virtual copy;
//! - its folder was last browsed more than `days` ago ([`Catalog::last_browsed`]). A folder with
//!   no recorded time (a catalog from before this existed) counts as browsed *now*: it is stamped
//!   with the current time and nothing in it is forgotten this time;
//! - it is **untouched** ([`Photo::untouched_local`]): nothing about it changed since it was
//!   browsed — see below;
//! - nothing refers to it: no album holds it or shows it as cover, no stack holds it, no virtual
//!   copy was made of it; and the caller (the engine) says it isn't in use (visible, selected,
//!   in the undo history, has a smart preview…).
//!
//! **Untouched.** A record catalogued by a browse carries a fingerprint of its whole state at
//! that moment ([`Photo::local_baseline`]). It is untouched while its state still hashes to that
//! fingerprint, i.e. *any* change since — develop settings or history, versions, rating, flag,
//! label, any metadata (title, caption, keywords, location, copyright…), capture time, crop,
//! rename or relink, culling scores, file content — keeps it. Records without a fingerprint
//! (browsed before fingerprints existed) are judged by their data alone and kept unless every
//! user-editable field is still empty / default: no rating, flag or label; develop settings equal
//! to the import defaults with no history, versions or edit time; no title, caption, alt text,
//! description, keywords, creator, copyright, rights, location or GPS; no culling scores.
//! When in doubt a record is kept.
//!
//! Browsing the folder again catalogues its files afresh (new records, new fingerprints).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::Serialize;

use crate::{Catalog, Op, Photo, PhotoId, Source};

/// Default for how long a Local folder may go unbrowsed before its untouched records are
/// forgotten (days).
pub const DEFAULT_FORGET_DAYS: u32 = 30;

/// Records removed per logged batch (keeps log lines a reasonable size).
const BATCH: usize = 2000;

/// 64-bit FNV-1a over everything written to it (stable across builds and platforms, unlike
/// `std`'s hasher), so a photo is hashed as it serialises, without a buffer.
struct Fnv(u64);

impl std::io::Write for Fnv {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        for b in buf {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The folder a file-backed photo is in (`None` for demo photos).
pub fn folder_of(p: &Photo) -> Option<String> {
    match &p.source {
        // `/` and `\` are both separators whatever the platform (a catalog can move between them)
        Source::File { path } => {
            let key = crate::query::folder_key(path);
            key.rsplit_once('/').map(|(dir, _)| if dir.is_empty() { "/".to_string() } else { dir.to_string() })
        }
        Source::Demo { .. } => None,
    }
}

impl Photo {
    /// A hash of everything about the photo except [`Photo::local_baseline`] itself.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fnv(0xcbf2_9ce4_8422_2325);
        let r = if self.local_baseline.is_some() {
            let mut p = self.clone();
            p.local_baseline = None;
            serde_json::to_writer(&mut h, &p)
        } else {
            serde_json::to_writer(&mut h, self)
        };
        // (serialising a photo can't fail; if it ever did, no baseline would match: kept)
        if r.is_err() { 0 } else { h.0 }
    }

    /// Record the current state as the browse baseline (done when a browse catalogues it).
    pub fn set_local_baseline(&mut self) {
        self.local_baseline = None;
        self.local_baseline = Some(self.fingerprint());
    }

    /// A Local record nothing was changed on since it was browsed (see the module docs). Says
    /// nothing about albums, stacks or virtual copies referring to it.
    pub fn untouched_local(&self) -> bool {
        if !self.local || self.deleted || self.copy_of.is_some() || self.copy_name.is_some() || folder_of(self).is_none() {
            return false;
        }
        match self.local_baseline {
            Some(b) => self.fingerprint() == b,
            None => self.looks_unedited(),
        }
    }

    /// Every user-editable field is still empty / default (records without a baseline).
    fn looks_unedited(&self) -> bool {
        let m = &self.meta;
        self.rating == 0
            && self.flag == crate::Flag::None
            && self.label.is_none()
            && self.edited.is_none()
            && self.history.is_empty()
            && self.versions.is_empty()
            && *self.develop == self.import_defaults()
            && self.analysis.is_none()
            && m.title.is_empty()
            && m.caption.is_empty()
            && m.alt_text.is_empty()
            && m.extended_description.is_empty()
            && m.keywords.is_empty()
            && m.creator.is_empty()
            && m.copyright.is_empty()
            && m.copyright_status.is_unknown()
            && m.usage_terms.is_empty()
            && m.copyright_url.is_empty()
            && m.location.is_empty()
            && m.city.is_empty()
            && m.state.is_empty()
            && m.country.is_empty()
            && m.gps.is_none()
    }
}

/// What forgetting Local records would do / did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForgetPlan {
    /// Local records looked at.
    pub local: usize,
    /// Records to forget.
    pub evict: Vec<PhotoId>,
    /// Kept: their folder was browsed within the period (or has no time yet).
    pub kept_recent: usize,
    /// Kept: changed by the user since they were browsed.
    pub kept_touched: usize,
    /// Kept: in an album, a stack, the source of a virtual copy, or in use.
    pub kept_in_use: usize,
    /// Folders with Local records but no last-browsed time: stamped now.
    pub stamp: Vec<String>,
    /// Folders whose last Local record goes: their time is dropped.
    pub unstamp: Vec<String>,
    /// The time stamps are given.
    #[serde(skip)]
    pub now: String,
}

impl ForgetPlan {
    /// The ops that carry the plan out (empty when there is nothing to do).
    pub fn ops(&self) -> Vec<Op> {
        let mut ops: Vec<Op> = self.stamp.iter().map(|f| Op::SetBrowsed { folder: f.clone(), at: Some(self.now.clone()) }).collect();
        ops.extend(self.evict.chunks(BATCH).map(|c| Op::Batch { ops: c.iter().map(|id| Op::RemovePhoto { id: *id }).collect() }));
        ops.extend(self.unstamp.iter().map(|f| Op::SetBrowsed { folder: f.clone(), at: None }));
        ops
    }
}

impl Catalog {
    /// When `folder` was last browsed (ISO 8601), if known.
    pub fn last_browsed(&self, folder: &str) -> Option<&str> {
        self.browsed.get(&crate::query::folder_key(folder)).map(String::as_str)
    }

    /// Folders with a last-browsed time.
    pub fn browsed_folders(&self) -> &BTreeMap<String, String> {
        &self.browsed
    }

    /// Plan forgetting untouched Local records whose folder wasn't browsed for `days` days (as of
    /// `now`, ISO 8601). `in_use` keeps records the caller still needs. `days == 0` (never) or an
    /// unreadable `now` forgets nothing. See the module docs.
    pub fn forget_local_plan(&self, now: &str, days: u32, in_use: &dyn Fn(&Photo) -> bool) -> ForgetPlan {
        let mut plan = ForgetPlan { now: now.to_string(), ..Default::default() };
        let now_s = crate::stacks::iso_seconds(now);
        let max_age = i64::from(days) * 86_400;
        let mut referenced: HashSet<PhotoId> = HashSet::new();
        for a in self.albums.values() {
            referenced.extend(a.photos.iter().copied());
            referenced.extend(a.cover);
        }
        for s in self.stacks.values() {
            referenced.extend(s.photos.iter().copied());
        }
        referenced.extend(self.photos.values().filter_map(|p| p.copy_of));
        // the fingerprints are the costly part: work them out for the candidates in parallel
        let candidates: Vec<&Photo> = self
            .photos
            .values()
            .filter(|p| p.local && !referenced.contains(&p.id))
            .filter(|p| folder_of(p).and_then(|f| self.browsed.get(&f)).is_some_and(|at| is_old(at, now_s, days, max_age)))
            .map(|p| &**p)
            .collect();
        let untouched: HashSet<PhotoId> = untouched_of(&candidates);
        let mut stamp = BTreeSet::new();
        let mut remaining: BTreeSet<String> = BTreeSet::new();
        for p in self.photos.values().filter(|p| p.local) {
            plan.local += 1;
            let Some(folder) = folder_of(p) else {
                plan.kept_in_use += 1;
                continue;
            };
            let recent = match (self.browsed.get(&folder), now_s) {
                (None, _) => {
                    stamp.insert(folder.clone());
                    true
                }
                (Some(at), _) => !is_old(at, now_s, days, max_age),
            };
            let keep = if recent {
                plan.kept_recent += 1;
                true
            } else if referenced.contains(&p.id) || in_use(p) {
                plan.kept_in_use += 1;
                true
            } else if !untouched.contains(&p.id) {
                plan.kept_touched += 1;
                true
            } else {
                plan.evict.push(p.id);
                false
            };
            if keep {
                remaining.insert(folder);
            }
        }
        plan.stamp = stamp.into_iter().collect();
        plan.unstamp = self.browsed.keys().filter(|f| !remaining.contains(*f)).cloned().collect();
        plan
    }
}

/// A folder browsed at `at` is past the period (never when `days` is 0 or a time is unreadable).
fn is_old(at: &str, now_s: Option<i64>, days: u32, max_age: i64) -> bool {
    days > 0 && matches!((now_s, crate::stacks::iso_seconds(at)), (Some(n), Some(a)) if n - a >= max_age)
}

/// The untouched ones among `photos` ([`Photo::untouched_local`]), checked on several threads
/// where there are many.
fn untouched_of(photos: &[&Photo]) -> HashSet<PhotoId> {
    let check = |chunk: &[&Photo]| chunk.iter().filter(|p| p.untouched_local()).map(|p| p.id).collect::<Vec<_>>();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
        if threads > 1 && photos.len() >= 256 {
            let size = photos.len().div_ceil(threads);
            return std::thread::scope(|sc| {
                let jobs: Vec<_> = photos.chunks(size).map(|c| sc.spawn(move || check(c))).collect();
                // a job that panicked (it can't) leaves its photos out: they are kept
                jobs.into_iter().filter_map(|j| j.join().ok()).flatten().collect()
            });
        }
    }
    check(photos).into_iter().collect()
}

impl Op {
    /// Every photo the op refers to.
    pub fn photo_ids(&self, out: &mut impl FnMut(PhotoId)) {
        match self {
            Op::AddPhoto { photo } => out(photo.id),
            Op::RemovePhoto { id }
            | Op::SetRating { id, .. }
            | Op::SetFlag { id, .. }
            | Op::SetLabel { id, .. }
            | Op::SetDevelop { id, .. }
            | Op::SetMeta { id, .. }
            | Op::SetDeleted { id, .. }
            | Op::SetLocal { id, .. }
            | Op::SetVersions { id, .. }
            | Op::SetHistory { id, .. }
            | Op::PushHistory { id, .. }
            | Op::SetCaptured { id, .. }
            | Op::SetAnalysis { id, .. }
            | Op::SetFile { id, .. }
            | Op::Relink { id, .. }
            | Op::SetContent { id, .. } => out(*id),
            Op::AddAlbum { album } => {
                album.photos.iter().for_each(|p| out(*p));
                album.cover.into_iter().for_each(&mut *out);
            }
            Op::SetAlbumPhotos { photos, .. } | Op::SetStack { photos, .. } => photos.iter().for_each(|p| out(*p)),
            Op::SetAlbumCover { cover, .. } => cover.iter().for_each(|p| out(*p)),
            Op::AddStack { stack } => stack.photos.iter().for_each(|p| out(*p)),
            Op::Batch { ops } => ops.iter().for_each(|o| o.photo_ids(out)),
            Op::RemoveAlbum { .. }
            | Op::RenameAlbum { .. }
            | Op::MoveAlbum { .. }
            | Op::SetAlbumRules { .. }
            | Op::RemoveStack { .. }
            | Op::SetLabelName { .. }
            | Op::SetBrowsed { .. } => {}
        }
    }
}
