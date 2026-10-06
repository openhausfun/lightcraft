//! Local: browse a folder on disk without adding it to the library (Lightroom's Local tab).
//! The folder's photos are probed in place as `local` photos — only folder views list them —
//! and edits go to the files' XMP sidecars like any photo added in place. "Add to My Photos"
//! makes them ordinary library photos.
//!
//! Each browse records when the folders it listed were browsed. Untouched records of folders
//! not browsed for [`Session::forget_local_days`] are forgotten (when the library opens, or with
//! `library.forgetLocal`); see [`lightcraft_catalog::local`] for what "untouched" means. Files
//! and sidecars on disk are never touched, and browsing the folder again brings them back.

use std::path::Path;

use lightcraft_catalog::{Op, PhotoId};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, str_param};
use crate::import::{ImportMode, ImportOptions, import_with, is_supported};
use crate::{Browse, LibrarySource, Result, Session};

fn browse(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.browse";
    let path = str_param(p, "path").ok_or_else(|| bad(C, "missing `path`"))?;
    let dir = std::path::absolute(Path::new(path)).map_err(|e| bad(C, e.to_string()))?;
    if !dir.is_dir() {
        return Err(bad(C, format!("{path}: not a folder")));
    }
    let dir_s = dir.to_string_lossy().trim_end_matches(['/', '\\']).to_string();
    let subfolders = bool_or(p, "subfolders", s.browse.as_ref().is_some_and(|b| b.subfolders));
    let files: Vec<String> = if subfolders {
        crate::import::expand(std::slice::from_ref(&dir_s), None)
    } else {
        let mut v: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| bad(C, format!("{path}: {e}")))?
            .flatten()
            .map(|e| e.path())
            .filter(|f| f.is_file() && is_supported(f) && !f.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        v.sort();
        v
    };
    let report = import_with(s, &files, &ImportOptions { mode: ImportMode::Add, local: true, ..Default::default() })?;
    stamp_browsed(s, &dir_s, &files);
    s.browse = Some(Browse { path: dir_s.clone(), subfolders });
    s.source = LibrarySource::Folder;
    let shown = s.visible().len();
    Ok(json!({"path": dir_s, "subfolders": subfolders, "photos": shown, "new": report.imported.len(), "failed": report.failed.len()}))
}

/// Folders whose browse time is refreshed at most this often (keeps repeated browsing from
/// growing the log; far finer than the days the eviction counts in).
const RESTAMP_SECS: i64 = 3600;

/// Record that `root` and the folders of `files` were browsed now (journaled, not an undo step).
fn stamp_browsed(s: &mut Session, root: &str, files: &[String]) {
    let now = (s.clock)();
    let now_s = lightcraft_catalog::stacks::iso_seconds(&now);
    let mut folders: std::collections::BTreeSet<String> =
        files.iter().filter_map(|f| Path::new(f).parent().map(|d| d.to_string_lossy().trim_end_matches(['/', '\\']).to_string())).collect();
    folders.insert(root.to_string());
    let ops: Vec<Op> = folders
        .into_iter()
        .filter(|f| {
            let at = s.catalog.last_browsed(f).and_then(lightcraft_catalog::stacks::iso_seconds);
            match (at, now_s) {
                (Some(at), Some(n)) => n - at >= RESTAMP_SECS || n < at,
                _ => true,
            }
        })
        .map(|folder| Op::SetBrowsed { folder, at: Some(now.clone()) })
        .collect();
    if !ops.is_empty() {
        let op = Op::Batch { ops };
        if s.catalog.apply(op.clone()).is_ok() {
            s.pending_log.push(op);
        }
    }
}

/// Photos the session still needs, whatever their state: shown, selected, being edited, or
/// referred to by an undo / redo step.
fn in_use(s: &mut Session) -> std::collections::HashSet<PhotoId> {
    let mut keep: std::collections::HashSet<PhotoId> = s.visible().iter().copied().collect();
    keep.extend(s.selection.ids.iter().copied());
    keep.extend(s.selection.active);
    keep.extend(s.previous_active);
    keep.extend(s.interaction.as_ref().map(|i| i.photo));
    keep.extend(s.before.keys().copied());
    for e in s.undo.iter().chain(s.redo.iter()) {
        e.op.photo_ids(&mut |id| {
            keep.insert(id);
        });
    }
    keep
}

impl Session {
    /// Forget untouched Local records whose folder wasn't browsed for `days` (default
    /// [`Session::forget_local_days`]; 0 = never). With `dry_run`, only report what would go.
    /// Journaled (durable, replayed after a crash) but not an undo step; nothing on disk changes.
    pub fn forget_local(&mut self, dry_run: bool, days: Option<u32>) -> lightcraft_catalog::ForgetPlan {
        let days = days.unwrap_or(self.forget_local_days);
        let now = (self.clock)();
        let keep = in_use(self);
        let smart = self.media.smart_dir.clone();
        let plan = self
            .catalog
            .forget_local_plan(&now, days, &|p| keep.contains(&p.id) || smart.as_ref().is_some_and(|d| d.join(crate::smart::file_name(p)).exists()));
        if dry_run {
            return plan;
        }
        for op in plan.ops() {
            match self.catalog.apply(op.clone()) {
                Ok(_) => self.pending_log.push(op),
                Err(e) => log::warn!("forget Local records: {e}"),
            }
        }
        if !plan.evict.is_empty() {
            log::info!("forgot {} untouched Local record(s) of folders not browsed for {days} days", plan.evict.len());
        }
        plan
    }
}

fn forget_local(s: &mut Session, p: &Value) -> Result<Value> {
    let dry_run = bool_or(p, "dryRun", false);
    let days = match p.get("days") {
        None | Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_u64().ok_or_else(|| bad("library.forgetLocal", "days must be a whole number (0 = never)"))?.min(u64::from(u32::MAX)) as u32)
        }
    };
    let used = days.unwrap_or(s.forget_local_days);
    let plan = s.forget_local(dry_run, days);
    if !dry_run && !plan.evict.is_empty() {
        s.compact_soon();
    }
    Ok(json!({
        "dryRun": dry_run,
        "days": used,
        "local": plan.local,
        "forgotten": plan.evict.len(),
        "keptRecent": plan.kept_recent,
        "keptTouched": plan.kept_touched,
        "keptInUse": plan.kept_in_use,
        "foldersStamped": plan.stamp.len(),
    }))
}

/// The browsed photos among `ids` (default: the selection) become library photos.
fn add_to_library(s: &mut Session, p: &Value) -> Result<Value> {
    let ids: Vec<PhotoId> = s.targets(p).into_iter().filter(|id| s.catalog.photo(*id).is_some_and(|ph| ph.local)).collect();
    if ids.is_empty() {
        return Ok(json!({"added": 0}));
    }
    let n = ids.len();
    let ops = ids.into_iter().map(|id| Op::SetLocal { id, local: false }).collect();
    s.commit("Add to My Photos", Op::Batch { ops })?;
    Ok(json!({"added": n}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "folder.rename",
            "Rename Folder",
            [],
            None,
            "{path, name} — rename a folder on disk (its files and sidecars go along) and relink the photos in it; one undo step (undo renames it back, refused if the old name is taken) → {path, relinked}",
            always,
            |s, p| {
                let from = str_param(p, "path").ok_or_else(|| bad("folder.rename", "missing path"))?.trim_end_matches(['/', '\\']).to_string();
                let name = str_param(p, "name")
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && !n.contains(['/', '\\']) && *n != "." && *n != "..")
                    .ok_or_else(|| bad("folder.rename", "give a folder name (no slashes)"))?;
                let to = Path::new(&from).with_file_name(name).to_string_lossy().to_string();
                let n = move_folder(s, &from, &to).map_err(|e| bad("folder.rename", e))?;
                Ok(json!({"path": to, "relinked": n}))
            }
        ),
        cmd!(
            "folder.move",
            "Move Folder",
            [],
            None,
            "{path, into: destination folder} — move a folder on disk into another and relink the photos in it; one undo step (undo moves it back, refused if the old place is taken) → {path, relinked}",
            always,
            |s, p| {
                let from = str_param(p, "path").ok_or_else(|| bad("folder.move", "missing path"))?.trim_end_matches(['/', '\\']).to_string();
                let into = str_param(p, "into").ok_or_else(|| bad("folder.move", "missing into"))?;
                let name = Path::new(&from).file_name().ok_or_else(|| bad("folder.move", "bad path"))?;
                let to = Path::new(into).join(name).to_string_lossy().to_string();
                let n = move_folder(s, &from, &to).map_err(|e| bad("folder.move", e))?;
                Ok(json!({"path": to, "relinked": n}))
            }
        ),
        cmd!(
            "library.browse",
            "Browse Folder",
            [],
            None,
            "{path, subfolders?: bool} — show a folder's photos without adding them to the library (they're read in place; edits go to XMP sidecars) → {path, photos, new}",
            always,
            browse
        ),
        cmd!(
            "library.forgetLocal",
            "Forget Unchanged Local Photos",
            [],
            None,
            "{dryRun?: bool, days?: n (default: the library preference, 0 = never)} — remove Local (browsed, not added) photos nobody changed, of folders not browsed for `days`, from the catalog; files and sidecars stay, browsing the folder again brings them back; not an undo step → {local, forgotten, keptRecent, keptTouched, keptInUse, foldersStamped, days, dryRun}",
            always,
            forget_local
        ),
        cmd!("photo.addToLibrary", "Add to My Photos", ["Photo"], None, "{ids?} — browsed (Local) photos join the library", always, add_to_library),
    ]
}

/// Rename `from` to `to` on disk (a folder; everything in it goes along). Never overwrites:
/// refused when `to` exists (a case-only rename on a case-insensitive file system goes through
/// a temporary name). Missing parents of `to` are created.
pub(crate) fn rename_folder_on_disk(from: &str, to: &str) -> std::result::Result<(), String> {
    let (src, dst) = (Path::new(from), Path::new(to));
    if !src.is_dir() {
        return Err(format!("{from} is not a folder (moved or deleted?)"));
    }
    let case_only = from != to && from.to_lowercase() == to.to_lowercase();
    if dst.exists() && !case_only {
        return Err(format!("{to} already exists"));
    }
    if dst.starts_with(src) {
        return Err("a folder can't move into itself".into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    if case_only {
        let tmp = src.with_file_name(format!(".lc-rename-{}", std::process::id()));
        if tmp.exists() {
            return Err(format!("{} already exists", tmp.display()));
        }
        std::fs::rename(src, &tmp).map_err(|e| format!("{from} → {to}: {e}"))?;
        return std::fs::rename(&tmp, dst).map_err(|e| {
            let _ = std::fs::rename(&tmp, src);
            format!("{from} → {to}: {e}")
        });
    }
    std::fs::rename(src, dst).map_err(|e| format!("{from} → {to}: {e}"))
}

/// After a folder moved from `from` to `to`: a browsed folder at or below it follows.
pub(crate) fn follow_folder(s: &mut Session, from: &str, to: &str) {
    if let Some(b) = &mut s.browse
        && let Ok(rel) = Path::new(&b.path).strip_prefix(from)
    {
        b.path = Path::new(to).join(rel).to_string_lossy().to_string();
        let f = b.path.clone();
        s.filter.folder = Some(f);
    }
}

/// Rename or move a folder on disk (everything in it goes along, sidecars included) and relink
/// the photos inside, as one undo step: undo renames the folder back and restores the photos'
/// paths, redo repeats the move — each refused (and reported) when the destination is taken.
pub(crate) fn move_folder(s: &mut Session, from: &str, to: &str) -> std::result::Result<usize, String> {
    let (src, dst) = (Path::new(from), Path::new(to));
    if !src.is_dir() {
        return Err(format!("{from} is not a folder"));
    }
    if s.library.as_ref().is_some_and(|l| l.dir.starts_with(src) || src.starts_with(&l.dir)) {
        return Err("the library's own folders can't be moved here".into());
    }
    rename_folder_on_disk(from, to)?;
    let ops: Vec<Op> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Path::new(path).strip_prefix(src).ok().map(|rel| (p.id, p.file_name.clone(), dst.join(rel))),
            _ => None,
        })
        .map(|(id, file_name, np)| Op::Relink {
            id,
            file_name,
            source: lightcraft_catalog::Source::File { path: np.to_string_lossy().to_string() },
            format: None,
        })
        .collect();
    let n = ops.len();
    let name = Path::new(to).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let renamed = src.parent() == dst.parent();
    let label = if renamed { format!("Rename Folder to “{name}”") } else { format!("Move Folder “{name}”") };
    let folder = crate::FolderMove { from: to.to_string(), to: from.to_string() };
    if let Err(e) = s.commit_with_folder(&label, Op::Batch { ops }, folder) {
        // the catalog didn't take it: put the folder back where the photos still point
        let back = rename_folder_on_disk(to, from).err().map(|b| format!("; moving it back failed too: {b}")).unwrap_or_default();
        return Err(format!("{e}{back}"));
    }
    follow_folder(s, from, to);
    Ok(n)
}
