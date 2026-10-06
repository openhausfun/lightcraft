//! Missing originals: photos whose file is no longer where the library expects it (moved,
//! renamed, on an unplugged drive), and relinking them — one at a time (`photo.relink`) or by
//! searching a folder for files with the same name and size, or the same content
//! (`library.findMissing`).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lightcraft_catalog::{Catalog, Op, Photo, PhotoId, Source};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

/// The file Missing Photos checks for `p`, if it is in scope: a library photo (not in Recently
/// Deleted, not a Local-only record seen while browsing a folder) whose original is a file.
///
/// Local browse records are out of scope on purpose: browsing can leave tens of thousands of
/// them in the catalog, they never show in library views (Missing Photos included), and a
/// folder that is gone is simply not browsed again. The sidebar count, the Missing Photos view,
/// `library.missing` and Find Missing Photos all use this one rule.
pub fn checked_path(p: &Photo) -> Option<&str> {
    match &p.source {
        Source::File { path } if p.in_library() => Some(path),
        _ => None,
    }
}

/// Every in-scope photo's file ([`checked_path`]), without touching the disk.
pub fn candidates(cat: &Catalog) -> Vec<String> {
    cat.photos().filter_map(|p| checked_path(p).map(str::to_string)).collect()
}

/// [`missing`] with the existence check supplied (tests count the file-system calls).
pub fn missing_with(cat: &Catalog, mut exists: impl FnMut(&str) -> bool) -> Vec<(PhotoId, String)> {
    cat.photos().filter_map(|p| checked_path(p).filter(|f| !exists(f)).map(|f| (p.id, f.to_string()))).collect()
}

/// Library photos whose original file can't be found: (id, path).
pub fn missing(s: &Session) -> Vec<(PhotoId, String)> {
    if cfg!(target_arch = "wasm32") {
        return Vec::new();
    }
    missing_with(&s.catalog, |f| Path::new(f).exists())
}

/// Whether photo `id` is in scope and its file is gone (the Missing Photos view checks only
/// the photos its query already narrowed to). In the app `avail` answers from its cache (files
/// not checked yet count as present until the background check finds them gone).
pub fn is_missing(cat: &Catalog, avail: &crate::availability::Availability, id: PhotoId) -> bool {
    !cfg!(target_arch = "wasm32") && cat.photo(id).and_then(|p| checked_path(p)).is_some_and(|f| avail.is_offline(f))
}

fn relink_op(id: PhotoId, path: &str) -> Op {
    let file_name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string());
    Op::Relink { id, file_name, source: Source::File { path: path.to_string() }, format: None }
}

fn relink(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "photo.relink";
    let id = p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(C, "no photo (give `id`)"))?;
    let path = str_param(p, "path").ok_or_else(|| bad(C, "missing `path`"))?;
    let abs = std::path::absolute(path).map_err(|e| bad(C, e.to_string()))?;
    if !abs.is_file() {
        return Err(bad(C, format!("{path}: no such file")));
    }
    if !matches!(s.catalog.photo(id).map(|p| &p.source), Some(Source::File { .. })) {
        return Err(bad(C, "only photos from files can be relinked"));
    }
    let abs = abs.to_string_lossy().to_string();
    // the file may differ from the one imported (another export, a raw that decodes now or not):
    // its size, dimensions and preview-only state follow it
    let mut ops = vec![relink_op(id, &abs)];
    if s.media.file_probe.is_some()
        && let (Some(Ok(info)), Some(ph)) = (crate::import::probe_paths(s, std::slice::from_ref(&abs)).pop(), s.catalog.photo(id))
    {
        ops.extend(crate::cmd::convert::content_op(id, ph, info));
    }
    let op = match <[Op; 1]>::try_from(ops) {
        Ok([op]) => op,
        Err(ops) => Op::Batch { ops },
    };
    s.commit("Relink Photo", op)?;
    s.media.forget(id);
    Ok(json!({"id": id.0, "path": abs}))
}

/// The stored content hash of a photo, without the `:dupN` suffix duplicates get.
fn stored_hash(p: &Photo) -> Option<&str> {
    p.content_hash.as_deref().map(|h| h.split(':').next().unwrap_or(h)).filter(|h| !h.is_empty())
}

/// The content hash of the file at `path` (as import computes it), if it can be read.
fn file_hash(path: &str) -> Option<String> {
    std::fs::read(path).ok().map(|b| lightcraft_preview::hash_bytes(&b).to_string())
}

/// An in-scope photo ([`checked_path`]) as Find Missing Photos sees it: its file, the size and
/// content hash the library knows (0 / `None` = unknown).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindCandidate {
    pub id: PhotoId,
    pub path: String,
    pub size: u64,
    pub hash: Option<String>,
}

/// Every in-scope photo, for [`plan_find_missing`]: the missing ones are looked for, and no
/// photo's file is used for another. No file-system calls, so the app takes this snapshot on the
/// UI thread and plans on a worker without holding the session.
pub fn find_candidates(cat: &Catalog) -> Vec<FindCandidate> {
    cat.photos()
        .filter_map(|p| {
            checked_path(p).map(|f| FindCandidate { id: p.id, path: f.to_string(), size: p.file_size, hash: stored_hash(p).map(str::to_string) })
        })
        .collect()
}

/// How a missing photo's file was recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchBy {
    /// Same name and size (and content, when the library knows its hash).
    Name,
    /// Renamed: same size and content hash.
    Content,
}

impl MatchBy {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchBy::Name => "name",
            MatchBy::Content => "content",
        }
    }
}

/// A missing photo and the file found for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundFile {
    pub id: PhotoId,
    pub from: String,
    pub to: String,
    pub by: MatchBy,
}

/// A missing photo with several look-alike files (same name and size, no hash to tell them
/// apart): skipped, never guessed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AmbiguousFile {
    pub id: PhotoId,
    pub from: String,
    pub candidates: Vec<String>,
}

/// What Find Missing Photos found ([`plan_find_missing`]), before relinking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindPlan {
    pub found: Vec<FoundFile>,
    /// In-scope photos that stay missing (the ambiguous ones included).
    pub missing: usize,
    pub ambiguous: Vec<AmbiguousFile>,
}

impl FindPlan {
    /// `{found: [{id, from, to, by}], missing, ambiguous: [{id, from, candidates}]}` — the
    /// command's result, and what `library.findMissing {found, …}` takes back.
    pub fn to_json(&self) -> Value {
        let found: Vec<Value> = self.found.iter().map(|f| json!({"id": f.id.0, "from": f.from, "to": f.to, "by": f.by.as_str()})).collect();
        let ambiguous: Vec<Value> = self.ambiguous.iter().map(|a| json!({"id": a.id.0, "from": a.from, "candidates": a.candidates})).collect();
        json!({"found": found, "missing": self.missing, "ambiguous": ambiguous})
    }

    /// The inverse of [`FindPlan::to_json`] (entries it can't read are dropped).
    pub fn from_json(v: &Value) -> FindPlan {
        let list = |k: &str| v.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let found = list("found")
            .iter()
            .filter_map(|f| {
                let by = if f.get("by").and_then(Value::as_str) == Some("content") { MatchBy::Content } else { MatchBy::Name };
                Some(FoundFile {
                    id: PhotoId(f.get("id")?.as_u64()?),
                    from: str_param(f, "from")?.to_string(),
                    to: str_param(f, "to")?.to_string(),
                    by,
                })
            })
            .collect();
        let ambiguous = list("ambiguous")
            .iter()
            .filter_map(|a| {
                let candidates = a.get("candidates")?.as_array()?.iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
                Some(AmbiguousFile { id: PhotoId(a.get("id")?.as_u64()?), from: str_param(a, "from")?.to_string(), candidates })
            })
            .collect();
        let missing = v.get("missing").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
        FindPlan { found, missing, ambiguous }
    }
}

/// The file-system half of Find Missing Photos: which `candidates` are gone, and which file in
/// `folder` (recursively) is each one's. Touches only the disk — never the session — so the app
/// runs it on a worker thread (walking a big folder on a network share and hashing look-alikes
/// takes a while) and relinks the result with `library.findMissing {found, missing, ambiguous}`.
///
/// Matching, per photo:
/// 1. a file with the same name (any letter case), the same size and the same content hash;
/// 2. renamed: a file of the same size with the same content hash (only same-size files are
///    read, so this stays cheap);
/// 3. without a hash to confirm (or the file was edited in place): the one file with the same
///    name and size. Several such files are ambiguous: the photo is skipped and reported in
///    `ambiguous`, never guessed (two cameras' `IMG_0001.CR3` of one size are different photos).
///
/// Files that are already some photo's original, or that another photo was just matched with,
/// are not used.
pub fn plan_find_missing(candidates: &[FindCandidate], folder: &str) -> std::result::Result<FindPlan, String> {
    if !Path::new(folder).is_dir() {
        return Err(format!("{folder}: not a folder"));
    }
    if cfg!(target_arch = "wasm32") {
        return Ok(FindPlan::default());
    }
    let lost: Vec<&FindCandidate> = candidates.iter().filter(|c| !Path::new(&c.path).exists()).collect();
    if lost.is_empty() {
        return Ok(FindPlan::default());
    }
    let in_use: HashSet<&str> = candidates.iter().map(|c| c.path.as_str()).collect();
    // file name (lower case) → candidate paths; size → paths (filled lazily)
    let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
    let files: Vec<String> = crate::import::expand(&[folder.to_string()], None).into_iter().filter(|f| !in_use.contains(f.as_str())).collect();
    for f in &files {
        if let Some(n) = Path::new(f).file_name() {
            by_name.entry(n.to_string_lossy().to_lowercase()).or_default().push(f.clone());
        }
    }
    let mut by_size: Option<HashMap<u64, Vec<String>>> = None;
    let mut hashes: HashMap<String, Option<String>> = HashMap::new();
    let mut hash_of = |f: &str| hashes.entry(f.to_string()).or_insert_with(|| file_hash(f)).clone();
    let size_of = |f: &str| std::fs::metadata(f).map(|m| m.len()).ok();
    let mut used: HashSet<String> = HashSet::new();
    let mut plan = FindPlan::default();
    for c in &lost {
        let name = Path::new(&c.path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let size = c.size;
        let hash = c.hash.as_deref();
        let named: Vec<&String> = by_name
            .get(&name)
            .map(|v| v.iter().filter(|f| !used.contains(*f) && (size == 0 || size_of(f) == Some(size))).collect())
            .unwrap_or_default();
        // 1. same name and size, confirmed by the content hash
        let mut hit: Option<(String, MatchBy)> =
            hash.and_then(|h| named.iter().find(|f| hash_of(f).as_deref() == Some(h))).map(|f| (f.to_string(), MatchBy::Name));
        // 2. renamed: same size and content
        if hit.is_none()
            && let Some(h) = hash
            && size > 0
        {
            let sizes = by_size.get_or_insert_with(|| {
                let mut m: HashMap<u64, Vec<String>> = HashMap::new();
                for f in &files {
                    if let Some(n) = size_of(f) {
                        m.entry(n).or_default().push(f.clone());
                    }
                }
                m
            });
            hit = sizes
                .get(&size)
                .and_then(|v| v.iter().find(|f| !used.contains(*f) && hash_of(f).as_deref() == Some(h)))
                .map(|f| (f.clone(), MatchBy::Content));
        }
        // 3. same name and size only (no hash, or the file was edited in place): one candidate
        //    is taken, several are ambiguous and skipped rather than guessed
        if hit.is_none() {
            match named.as_slice() {
                [one] => hit = Some(((*one).clone(), MatchBy::Name)),
                [] => {}
                _ => {
                    plan.ambiguous.push(AmbiguousFile { id: c.id, from: c.path.clone(), candidates: named.iter().map(|f| (*f).clone()).collect() });
                    continue;
                }
            }
        }
        let Some((path, by)) = hit else { continue };
        used.insert(path.clone());
        plan.found.push(FoundFile { id: c.id, from: c.path.clone(), to: path, by });
    }
    plan.missing = lost.len() - plan.found.len();
    Ok(plan)
}

/// Relink what [`plan_find_missing`] found, in one undoable step, re-checked against the
/// catalog as it is now (the search may have run on a worker while the library changed): a
/// photo that no longer points at the path it was missing from (relinked, removed) is left
/// alone, and a file that is now some photo's original — or that the plan names twice — is not
/// used (that photo stays missing). `missing` is the plan's count, corrected for what changed
/// among the photos the plan names.
fn commit_find(s: &mut Session, plan: FindPlan) -> Result<Value> {
    let still_at = |cat: &Catalog, id: PhotoId, from: &str| cat.photo(id).and_then(|p| checked_path(p)) == Some(from);
    let mut in_use: HashSet<String> = candidates(&s.catalog).into_iter().collect();
    let mut found = Vec::new();
    let mut missing = plan.missing;
    for f in plan.found {
        if !still_at(&s.catalog, f.id, &f.from) {
            continue;
        }
        if in_use.contains(&f.to) {
            missing += 1;
            continue;
        }
        in_use.insert(f.to.clone());
        found.push(f);
    }
    let planned = plan.ambiguous.len();
    let ambiguous: Vec<AmbiguousFile> = plan.ambiguous.into_iter().filter(|a| still_at(&s.catalog, a.id, &a.from)).collect();
    // ambiguous photos relinked meanwhile aren't missing any more
    missing = missing.saturating_sub(planned - ambiguous.len());
    if !found.is_empty() {
        s.commit("Find Missing Photos", Op::Batch { ops: found.iter().map(|f| relink_op(f.id, &f.to)).collect() })?;
        for f in &found {
            s.media.forget(f.id);
        }
    }
    Ok(FindPlan { found, missing, ambiguous }.to_json())
}

/// Search `folder` (recursively) for each missing photo's file ([`plan_find_missing`]) and
/// relink every match in one undoable step. With `found` instead (what a search on a worker
/// thread returned, as `{found, missing, ambiguous}`), only the relinking is done.
fn find_missing(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.findMissing";
    if p.get("found").is_some_and(Value::is_array) {
        return commit_find(s, FindPlan::from_json(p));
    }
    let folder = str_param(p, "folder").ok_or_else(|| bad(C, "missing `folder`"))?;
    let plan = plan_find_missing(&find_candidates(&s.catalog), folder).map_err(|e| bad(C, e))?;
    commit_find(s, plan)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "library.missing", "Missing Photos", [], None, "{} → [{id, path}] — photos whose original file isn't where the library expects it", always, |s, _| {
            Ok(Value::Array(missing(s).into_iter().map(|(id, path)| json!({"id": id.0, "path": path})).collect()))
        }),
        cmd!(
            "photo.relink",
            "Locate Photo",
            [],
            None,
            "{id?, path} — point a photo at its file's new location (undo never moves files)",
            always,
            relink
        ),
        cmd!(
            "library.findMissing",
            "Find Missing Photos",
            [],
            None,
            "{folder} — relink every missing photo whose file is somewhere in the folder: same name and size (and content, when the library knows its hash), or — renamed — same size and content; or {found, missing, ambiguous} — relink what a search already found (the app searches on a worker thread; photos relinked meanwhile and files now in use are skipped) → {found: [{id, from, to, by: name|content}], missing, ambiguous: [{id, from, candidates}] (several same-name candidates and no hash to tell them apart: skipped)}",
            always,
            find_missing
        ),
    ]
}
