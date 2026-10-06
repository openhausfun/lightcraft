//! Batch rename: a file-name template applied to the selected photos, renaming the files on
//! disk (with their XMP sidecars) safely.
//!
//! Template tokens (shared with import renaming and export file naming): `{name}` (current name
//! without extension), `{num}` (the number at the end of the name: `IMG_0042` → `0042`), `{seq}` /
//! `{seq:3}` (sequence number, zero-padded to 3 digits), `{date}` / `{date:%Y%m%d}` (capture date;
//! `%Y %y %m %d %H %M %S`), `{folder}` (the original's folder name), `{camera}`, `{lens}`, `{iso}`,
//! `{rating}`, `{title}`, `{creator}`, `{ext}` (extension, without the dot). Unknown tokens stay as
//! typed. Characters that aren't allowed in file names become `-`. The original extension is always
//! kept.
//!
//! Safety:
//! - a target that exists on disk, or that another photo in the batch gets, receives a `-1`, `-2`…
//!   suffix — no file is ever overwritten. Whether an existing target is the photo itself (a
//!   change of letter case on a case-insensitive volume) is decided by file identity, not by
//!   comparing names, so on a case-sensitive volume `img_1.jpg` next to `IMG_1.JPG` is a collision
//!   (issue #95); moves never replace a file even if one appears after the check (hard link +
//!   unlink where the volume supports it);
//! - the moves happen before the catalog changes; if one fails, the ones already done are moved
//!   back and nothing is committed;
//! - the catalog op ([`Op::SetFile`]) is undoable: undo/redo move the files back and forth (see
//!   [`crate::Session::undo_step`]);
//! - virtual copies follow their master's file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde::Serialize;

use crate::sidecar::SidecarNaming;
use crate::{EngineError, Result, Session};

/// One planned rename.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RenamePlan {
    pub id: u64,
    pub from: String,
    pub to: String,
    /// File sources: the old and new paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_path: Option<String>,
}

fn split_ext(name: &str) -> (&str, &str) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext),
        _ => (name, ""),
    }
}

fn sanitize(s: &str) -> String {
    let t: String =
        s.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '-' } else { c }).collect();
    t.trim().trim_matches('.').trim().to_string()
}

/// `%Y%m%d`-style formatting of an ISO local time.
fn format_date(iso: &str, fmt: &str) -> String {
    let part = |a: usize, b: usize| iso.get(a..b).unwrap_or("00").to_string();
    let mut out = String::new();
    let mut it = fmt.chars();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('Y') => out.push_str(&part(0, 4)),
            Some('y') => out.push_str(&part(2, 4)),
            Some('m') => out.push_str(&part(5, 7)),
            Some('d') => out.push_str(&part(8, 10)),
            Some('H') => out.push_str(&part(11, 13)),
            Some('M') => out.push_str(&part(14, 16)),
            Some('S') => out.push_str(&part(17, 19)),
            Some('%') => out.push('%'),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

/// One template token, for help texts and tag pickers. [`TOKENS`] is the single list every UI and
/// command description shows; a test checks that [`expand_tokens`] knows each of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TokenHelp {
    /// The token as typed, e.g. `{seq:3}`.
    pub tag: &'static str,
    /// Other spellings with the same meaning.
    pub aliases: &'static [&'static str],
    pub meaning: &'static str,
}

/// Every template token, in the order the help shows them.
pub const TOKENS: &[TokenHelp] = &[
    TokenHelp { tag: "{name}", aliases: &["{filename}"], meaning: "Original file name without its extension" },
    TokenHelp { tag: "{num}", aliases: &[], meaning: "The number at the end of the original name (IMG_0042 → 0042)" },
    TokenHelp { tag: "{seq}", aliases: &["{n}"], meaning: "Sequence number, counting from the start number" },
    TokenHelp { tag: "{seq:3}", aliases: &["{n:3}"], meaning: "Sequence number zero-padded to N digits (1–9)" },
    TokenHelp { tag: "{date}", aliases: &[], meaning: "Capture date as YYYYMMDD" },
    TokenHelp { tag: "{date:%Y%m%d_%H%M%S}", aliases: &[], meaning: "Capture date and time in your own format (directives below)" },
    TokenHelp { tag: "{folder}", aliases: &[], meaning: "Name of the folder the original is in" },
    TokenHelp { tag: "{camera}", aliases: &[], meaning: "Camera make and model" },
    TokenHelp { tag: "{lens}", aliases: &[], meaning: "Lens" },
    TokenHelp { tag: "{iso}", aliases: &[], meaning: "ISO speed" },
    TokenHelp { tag: "{rating}", aliases: &[], meaning: "Star rating (0–5)" },
    TokenHelp { tag: "{title}", aliases: &[], meaning: "Title metadata" },
    TokenHelp { tag: "{creator}", aliases: &[], meaning: "Creator metadata" },
    TokenHelp { tag: "{ext}", aliases: &[], meaning: "Original extension without the dot" },
];

/// The `%` directives of `{date:…}`.
pub const DATE_DIRECTIVES: &[(&str, &str)] = &[
    ("%Y", "year, 4 digits"),
    ("%y", "year, 2 digits"),
    ("%m", "month 01–12"),
    ("%d", "day 01–31"),
    ("%H", "hour 00–23"),
    ("%M", "minute"),
    ("%S", "second"),
    ("%%", "a literal %"),
];

/// How templates behave, one sentence each (shown under the token list).
pub const TEMPLATE_NOTES: &[&str] = &[
    "The original extension is always added; {ext} only puts it inside the name as well.",
    "A blank template keeps the original names.",
    "{date} is the capture time; a photo without one uses the time it was imported.",
    "Missing metadata ({camera}, {title}…) leaves an empty gap; a name that comes out empty keeps the original name.",
    "Unknown tags stay as typed — check the preview for a {typo}.",
    "Characters not allowed in file names (/ \\ : * ? \" < > |) become -; an existing name gets -1, -2….",
];

/// The tokens as one line (`{name} {num} {seq} …`), for compact hints.
pub fn token_summary() -> String {
    TOKENS.iter().map(|t| t.tag).collect::<Vec<_>>().join(" ")
}

/// A fixed photo the help's examples are computed from (`IMG_0042.CR3`, 14 Jan 2026 05:58:48).
pub fn sample_photo() -> Photo {
    let mut p =
        Photo::new(PhotoId(0), Source::File { path: "/Card/DCIM/IMG_0042.CR3".into() }, "IMG_0042.CR3", "CR3", 6000, 4000, "2026-01-20T10:00:00");
    p.captured = Some("2026-01-14T05:58:48".into());
    p.rating = 4;
    p.meta.camera = "Canon EOS R5".into();
    p.meta.lens = "RF24-70mm F2.8".into();
    p.meta.iso = Some(400);
    p.meta.title = "Harbour".into();
    p.meta.creator = "Ann Lee".into();
    p
}

/// The token as expanded for [`sample_photo`] (sequence number 1), for the help's example column.
pub fn token_example(tag: &str) -> String {
    expand_tokens(tag, &sample_photo(), 1, 1)
}

/// Why a folder template (`{date:%Y}/{date:%Y%m%d}`) can't be used, if it can't: it must be
/// relative (no leading `/` or `\`, drive letter or `~`) and have no `.` / `..` levels.
pub fn folder_template_error(template: &str) -> Option<String> {
    let t = template.trim();
    if t.is_empty() {
        return Some("the folder template is empty".into());
    }
    let b = t.as_bytes();
    if t.starts_with(['/', '\\', '~']) || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') {
        return Some("the folder template must be relative to the destination (no leading /, \\, ~ or drive)".into());
    }
    if t.split(['/', '\\']).any(|s| matches!(s.trim(), "." | "..")) {
        return Some("the folder template may not contain . or .. folders".into());
    }
    None
}

/// The folders (relative to the destination) a folder template gives `p`: the template's own `/`
/// (or `\`) separate levels; each level's tokens are expanded and the result made a safe folder
/// name — a value can never add a level or climb out (`/`, `\`, `:`… become `-`, leading and
/// trailing dots are dropped). A level that comes out empty (missing metadata) is `unknown`; empty
/// levels in the template (`a//b`) are skipped. `{date}` is the capture time, else [`Photo::date`]'s
/// fallback (the import time). Check [`folder_template_error`] first; levels it rejects are skipped.
pub fn expand_folder(template: &str, p: &Photo, seq: usize) -> Vec<String> {
    template
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && !matches!(*s, "." | ".."))
        .map(|s| {
            let v = sanitize(&expand_tokens(s, p, seq, 1));
            if v.is_empty() { "unknown".to_string() } else { v }
        })
        .collect()
}

/// The `{…}` tags in `template` that aren't tokens (they stay as typed), for a warning.
pub fn unknown_tokens(template: &str) -> Vec<String> {
    let p = sample_photo();
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        let tag = &rest[i..i + j + 1];
        if expand_tokens(tag, &p, 1, 1) == tag && !out.iter().any(|t| t == tag) {
            out.push(tag.to_string());
        }
        rest = &rest[i + j + 1..];
    }
    out
}

/// Every token with its meaning and example, the date directives and the notes, as JSON (the
/// `photo.renameTokens` command).
pub fn token_help_json() -> serde_json::Value {
    let tokens: Vec<serde_json::Value> = TOKENS
        .iter()
        .map(|t| serde_json::json!({"tag": t.tag, "aliases": t.aliases, "meaning": t.meaning, "example": token_example(t.tag)}))
        .collect();
    let directives: Vec<serde_json::Value> = DATE_DIRECTIVES.iter().map(|(d, m)| serde_json::json!({"directive": d, "meaning": m})).collect();
    serde_json::json!({
        "tokens": tokens,
        "dateDirectives": directives,
        "notes": TEMPLATE_NOTES,
        "sample": "IMG_0042.CR3, captured 2026-01-14 05:58:48",
    })
}

/// Expand `template` for `p` (sequence number `seq`) into a file name with `p`'s extension.
pub fn expand(template: &str, p: &Photo, seq: usize) -> String {
    let (stem, ext) = split_ext(&p.file_name);
    let out = expand_tokens(template, p, seq, 1);
    let mut stem_out = sanitize(&out);
    if stem_out.is_empty() {
        stem_out = sanitize(stem);
    }
    if stem_out.is_empty() {
        stem_out = "photo".into();
    }
    if ext.is_empty() { stem_out } else { format!("{stem_out}.{ext}") }
}

/// The template's tokens replaced for `p` (no extension added, nothing sanitized). A bare `{seq}`
/// is zero-padded to `seq_width` digits.
pub fn expand_tokens(template: &str, p: &Photo, seq: usize, seq_width: usize) -> String {
    let (stem, ext) = split_ext(&p.file_name);
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        };
        let tok = &rest[i + 1..i + j];
        let (name, arg) = tok.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((tok, None));
        let v = match name.trim().to_ascii_lowercase().as_str() {
            "name" | "filename" => stem.to_string(),
            "num" => {
                let digits = stem.chars().rev().take_while(char::is_ascii_digit).count();
                stem[stem.len() - digits..].to_string()
            }
            "seq" | "n" => {
                let w: usize = arg.and_then(|a| a.parse().ok()).unwrap_or(seq_width).min(9);
                format!("{seq:0w$}")
            }
            "date" => format_date(p.date(), arg.unwrap_or("%Y%m%d")),
            "folder" => match &p.source {
                Source::File { path } => {
                    Path::new(path).parent().and_then(Path::file_name).map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
                }
                Source::Demo { .. } => String::new(),
            },
            "camera" => p.meta.camera.clone(),
            "lens" => p.meta.lens.clone(),
            "iso" => p.meta.iso.map(|v| v.to_string()).unwrap_or_default(),
            "rating" => p.rating.to_string(),
            "title" => p.meta.title.clone(),
            "creator" => p.meta.creator.clone(),
            "ext" => ext.to_string(),
            _ => format!("{{{tok}}}"),
        };
        out.push_str(&v);
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

/// Every sidecar that may belong to `path` (both naming conventions, `.xmp` and `.XMP`).
fn sidecars(path: &str) -> Vec<(PathBuf, SidecarNaming, bool)> {
    let mut v = Vec::new();
    for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
        let p = crate::sidecar::sidecar_path(path, naming);
        v.push((p.with_extension("XMP"), naming, true));
        v.push((p, naming, false));
    }
    v
}

/// The file-system primitives renaming uses. [`RealFs`] is the disk; tests substitute a model of
/// a case-insensitive or case-sensitive volume (and inject failures).
pub(crate) trait MoveFs {
    /// Something (file, folder, link) answers to `p`.
    fn exists(&self, p: &Path) -> bool;
    fn is_file(&self, p: &Path) -> bool;
    /// `a` and `b` name the same file (e.g. `IMG.JPG` and `img.jpg` on a case-insensitive
    /// volume). Decided by file identity, never by comparing names: on a case-sensitive volume
    /// they are two different photos. When in doubt: `false`.
    fn same_file(&self, a: &Path, b: &Path) -> bool;
    /// Rename `a` to `b`, failing (never replacing) when `b` exists.
    fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()>;
    /// Copy `a` to a new file `b` (an independent copy, never a link), failing when `b` exists.
    fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()>;
    fn remove_file(&self, p: &Path) -> std::io::Result<()>;
    /// Another photo file beside the stem-named sidecar `sidecar` that shares its stem (e.g.
    /// `IMG_1.JPG` for `IMG_1.xmp`), which still uses that sidecar.
    fn stem_sibling(&self, sidecar: &Path) -> Option<String>;
}

/// The real file system.
pub(crate) struct RealFs;

impl MoveFs for RealFs {
    fn exists(&self, p: &Path) -> bool {
        std::fs::symlink_metadata(p).is_ok()
    }

    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }

    fn same_file(&self, a: &Path, b: &Path) -> bool {
        if a == b {
            return true;
        }
        let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ma.dev() == mb.dev() && ma.ino() == mb.ino()
        }
        #[cfg(not(unix))]
        {
            // no stable file id in std here: `b` is `a` under another spelling when they look
            // alike and the folder has no entry spelled exactly like `b` (on a case-sensitive
            // folder holding both `IMG.JPG` and `img.jpg`, both entries are listed)
            let (Some(dir), Some(name)) = (b.parent(), b.file_name()) else { return false };
            if a.parent() != Some(dir) || ma.len() != mb.len() || ma.modified().ok() != mb.modified().ok() {
                return false;
            }
            let Ok(entries) = std::fs::read_dir(dir) else { return false };
            !entries.flatten().any(|e| e.file_name() == name)
        }
    }

    fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        if self.exists(b) {
            return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} already exists", b.display())));
        }
        // A hard link fails when `b` exists — even if it appeared since the check above — so
        // nothing is ever replaced. Volumes without hard links (FAT, some network shares) fall back
        // to a plain rename after the check.
        match std::fs::hard_link(a, b) {
            Ok(()) => std::fs::remove_file(a).inspect_err(|_| {
                // both names are the same file: dropping the new one loses nothing
                let _ = std::fs::remove_file(b);
            }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => std::fs::rename(a, b),
        }
    }

    fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let bytes = std::fs::read(a)?;
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(b)?;
        f.write_all(&bytes).and_then(|_| f.sync_all()).inspect_err(|_| {
            let _ = std::fs::remove_file(b);
        })
    }

    fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        std::fs::remove_file(p)
    }

    fn stem_sibling(&self, sidecar: &Path) -> Option<String> {
        crate::import_move::sibling_photo(sidecar)
    }
}

/// `from` → `to` differs only in letter case and `to` is that very file (a case-insensitive
/// volume): the rename must go through a temporary name.
fn is_respelling(fs: &dyn MoveFs, from: &Path, to: &Path) -> bool {
    from != to && from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase() && fs.exists(to) && fs.same_file(from, to)
}

/// Rename `a` to `b` without ever replacing a file; a change of letter case only goes through a
/// unique temporary name so case-insensitive volumes see a real change.
fn move_one(fs: &dyn MoveFs, a: &Path, b: &Path) -> std::io::Result<()> {
    if !is_respelling(fs, a, b) {
        return fs.rename_no_replace(a, b);
    }
    let tmp = (0..1000u32)
        .map(|k| a.with_file_name(format!(".lc-rename-{}-{k}", std::process::id())))
        .find(|t| !fs.exists(t))
        .ok_or_else(|| std::io::Error::other("no free temporary name"))?;
    fs.rename_no_replace(a, &tmp)?;
    fs.rename_no_replace(&tmp, b).map_err(|e| match fs.rename_no_replace(&tmp, a) {
        Ok(()) => e,
        Err(e2) => std::io::Error::other(format!("{e}; it could not be given its old name back either ({e2}): it is now {}", tmp.display())),
    })
}

/// Why [`move_file`] failed, and what it left behind.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveError {
    /// What went wrong, including every step that could not be undone and where that file is now.
    pub message: String,
    /// The file itself ended up at the new path: it moved, a sidecar didn't, and moving the file
    /// back failed too. Its sidecars that did move stay with it.
    pub moved: bool,
}

impl std::fmt::Display for MoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Move `from` to `to` (never over an existing file) together with its sidecars. On error the
/// moves are undone; whatever can't be undone is named in the error (see [`MoveError`]).
pub fn move_file(from: &str, to: &str) -> std::result::Result<(), MoveError> {
    move_file_with(&RealFs, from, to)
}

/// [`move_file`] on the file system `fs`.
pub(crate) fn move_file_with(fs: &dyn MoveFs, from: &str, to: &str) -> std::result::Result<(), MoveError> {
    if from == to {
        return Ok(());
    }
    let fail = |message: String| MoveError { message, moved: false };
    let (f, t) = (Path::new(from), Path::new(to));
    // `to` may be this very file spelled differently (case-insensitive volume); any other file
    // there — e.g. `img.jpg` next to `IMG.JPG` on a case-sensitive volume — is never replaced
    if fs.exists(t) && !is_respelling(fs, f, t) {
        return Err(fail(format!("{to} already exists")));
    }
    move_one(fs, f, t).map_err(|e| fail(format!("rename {from}: {e}")))?;
    // sidecars moved (or copied: `true`)
    let mut done: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    for (sc, naming, upper) in sidecars(from) {
        if !fs.is_file(&sc) {
            continue;
        }
        let mut dst = crate::sidecar::sidecar_path(to, naming);
        if upper {
            dst = dst.with_extension("XMP");
        }
        // a stem sidecar another file still uses (`IMG_1.xmp` of `IMG_1.CR3` and `IMG_1.JPG`)
        // is copied, not taken away from it (issue #92)
        let shared = naming == SidecarNaming::Stem && fs.stem_sibling(&sc).is_some();
        if fs.exists(&dst) && (shared || !is_respelling(fs, &sc, &dst)) {
            continue; // never overwrite: the old sidecar stays where it was
        }
        let r = if shared { fs.copy_no_replace(&sc, &dst) } else { move_one(fs, &sc, &dst) };
        if let Err(e) = r {
            let mut message = format!("rename {}: {e}", sc.display());
            // the file first: if it can't go back, the sidecars that moved stay with it
            if let Err(e) = move_one(fs, t, f) {
                message.push_str(&format!("; {from} could not be moved back ({e}): it is now {to}"));
                if !done.is_empty() {
                    let names: Vec<String> = done.iter().map(|(_, b, _)| b.display().to_string()).collect();
                    message.push_str(&format!(" with its sidecar {}", names.join(", ")));
                }
                return Err(MoveError { message, moved: true });
            }
            for (a, b, copied) in done.iter().rev() {
                if *copied {
                    // the original is still there: the copy just goes (a leftover copy loses nothing)
                    let _ = fs.remove_file(b);
                } else if let Err(e) = move_one(fs, b, a) {
                    message.push_str(&format!("; sidecar {} could not be moved back ({e}): it is now {}", a.display(), b.display()));
                }
            }
            return Err(fail(message));
        }
        done.push((sc, dst, shared));
    }
    Ok(())
}

/// What a failed batch of moves ([`move_all`]) left behind.
#[derive(Debug)]
pub(crate) struct BatchMoveError {
    /// The first failure and every move that could not be undone.
    pub message: String,
    /// Moves that stayed done, (from, to): those files are at `to` now.
    pub stuck: Vec<(String, String)>,
}

/// Move the files `moves` names, all or nothing: after a failure the earlier moves are undone.
/// Moves that can't be undone (e.g. the network share went away) are reported, not ignored.
pub(crate) fn move_all(fs: &dyn MoveFs, moves: &[(String, String)]) -> std::result::Result<(), BatchMoveError> {
    let mut done: Vec<&(String, String)> = Vec::new();
    for m in moves {
        let Err(e) = move_file_with(fs, &m.0, &m.1) else {
            done.push(m);
            continue;
        };
        let mut message = e.message;
        let mut stuck = Vec::new();
        if e.moved {
            stuck.push(m.clone());
        }
        for d in done.iter().rev() {
            if let Err(e) = move_file_with(fs, &d.1, &d.0) {
                message.push_str(&format!("; {} could not be moved back to {}: {}", d.1, d.0, e.message));
                if !e.moved {
                    stuck.push((*d).clone());
                }
            }
        }
        return Err(BatchMoveError { message, stuck });
    }
    Ok(())
}

/// The `SetFile` ops in `op` (nested batches included) that point a photo at one of `to`.
fn set_file_ops_to(op: &Op, to: &HashSet<&str>, out: &mut Vec<Op>) {
    match op {
        Op::Batch { ops } => ops.iter().for_each(|o| set_file_ops_to(o, to, out)),
        Op::SetFile { source: Source::File { path }, .. } if to.contains(path.as_str()) => out.push(op.clone()),
        _ => {}
    }
}

/// The photos among `ids` a rename handles, in order: photos sharing one file (virtual copies)
/// rename once. No file-system calls.
pub fn rename_photos(cat: &lightcraft_catalog::Catalog, ids: &[PhotoId]) -> Vec<std::sync::Arc<Photo>> {
    let mut seen_paths: HashSet<String> = HashSet::new();
    ids.iter()
        .filter_map(|id| cat.photo(*id))
        .filter(|p| match &p.source {
            Source::File { path } => seen_paths.insert(path.clone()),
            _ => true,
        })
        .cloned()
        .collect()
}

/// [`Session::plan_rename`] for [`rename_photos`], with the "is this name taken on disk?" check
/// supplied: the Rename dialog plans its preview on a worker thread (a check can block on a slow
/// drive). Whether a case-only change names this very file is decided on the disk.
pub fn plan_rename_photos(photos: &[std::sync::Arc<Photo>], template: &str, start: usize, exists: &dyn Fn(&str) -> bool) -> Vec<RenamePlan> {
    plan_rename_core(photos, template, start, &|p| exists(&p.to_string_lossy()), &|a, b| RealFs.same_file(a, b))
}

/// Plan renaming `photos`: `exists` says whether something answers to a path, `same_file` whether
/// two spellings name one file (see [`MoveFs`]).
fn plan_rename_core(
    photos: &[std::sync::Arc<Photo>],
    template: &str,
    start: usize,
    exists: &dyn Fn(&Path) -> bool,
    same_file: &dyn Fn(&Path, &Path) -> bool,
) -> Vec<RenamePlan> {
    let mut taken: HashSet<String> = HashSet::new(); // lower-case target paths/names claimed in this batch
    let mut out = Vec::new();
    for (seq, p) in (start..).zip(photos) {
        let want = expand(template, p, seq);
        let (stem, ext) = split_ext(&want);
        let (stem, ext) = (stem.to_string(), ext.to_string());
        let candidate = |k: usize| {
            if k == 0 {
                want.clone()
            } else if ext.is_empty() {
                format!("{stem}-{k}")
            } else {
                format!("{stem}-{k}.{ext}")
            }
        };
        let (to, to_path) = match &p.source {
            Source::File { path } => {
                let dir = Path::new(path).parent().map(Path::to_path_buf).unwrap_or_default();
                let mut k = 0;
                loop {
                    let name = candidate(k);
                    let tp = dir.join(&name).to_string_lossy().to_string();
                    let key = tp.to_lowercase();
                    // this very file, maybe spelled differently (case-insensitive volume)? By
                    // identity: on a case-sensitive volume `img_1.jpg` may be another photo
                    let (from, to) = (Path::new(path), Path::new(&tp));
                    let same = tp == *path || (from != to && key == path.to_lowercase() && exists(to) && same_file(from, to));
                    // free: not claimed in this batch and not on disk (unless it is this very file).
                    // A file this batch moves away still counts as taken: simple and safe.
                    if !taken.contains(&key) && (same || !exists(to)) {
                        taken.insert(key);
                        break (name, Some(tp));
                    }
                    k += 1;
                }
            }
            Source::Demo { .. } => {
                let mut k = 0;
                loop {
                    let name = candidate(k);
                    if taken.insert(format!("demo:{}", name.to_lowercase())) {
                        break (name, None);
                    }
                    k += 1;
                }
            }
        };
        let from_path = match &p.source {
            Source::File { path } => Some(path.clone()),
            _ => None,
        };
        out.push(RenamePlan { id: p.id.0, from: p.file_name.clone(), to, from_path, to_path });
    }
    out
}

impl Session {
    /// Plan renaming `ids` with `template` (sequence numbers from `start`), resolving collisions.
    pub fn plan_rename(&self, ids: &[PhotoId], template: &str, start: usize) -> Vec<RenamePlan> {
        self.plan_rename_with(&RealFs, ids, template, start)
    }

    /// [`Session::plan_rename`] on the file system `fs`.
    pub(crate) fn plan_rename_with(&self, fs: &dyn MoveFs, ids: &[PhotoId], template: &str, start: usize) -> Vec<RenamePlan> {
        plan_rename_core(&rename_photos(&self.catalog, ids), template, start, &|p| fs.exists(p), &|a, b| fs.same_file(a, b))
    }

    /// Carry out `plans`: move the files (rolled back on failure), then commit one undoable op
    /// that updates the photos (and their virtual copies).
    ///
    /// When a failure can't be fully rolled back, the files that kept their new names are
    /// committed as a smaller undoable rename, so the library points at them (nothing goes
    /// missing), and the error lists them (old → new).
    pub fn apply_rename(&mut self, plans: &[RenamePlan]) -> Result<usize> {
        self.apply_rename_with(&RealFs, plans)
    }

    /// [`Session::apply_rename`] on the file system `fs`.
    pub(crate) fn apply_rename_with(&mut self, fs: &dyn MoveFs, plans: &[RenamePlan]) -> Result<usize> {
        let mut ops = Vec::new();
        let by_path: HashMap<&str, &RenamePlan> = plans.iter().filter_map(|p| p.from_path.as_deref().map(|f| (f, p))).collect();
        for p in self.catalog.photos() {
            let plan = match &p.source {
                Source::File { path } => by_path.get(path.as_str()).copied(),
                Source::Demo { .. } => plans.iter().find(|x| x.id == p.id.0 && x.from_path.is_none()),
            };
            let Some(plan) = plan else { continue };
            let source = match &plan.to_path {
                Some(t) => Source::File { path: t.clone() },
                None => p.source.clone(),
            };
            if plan.to != p.file_name || source != p.source {
                ops.push(Op::SetFile { id: p.id, file_name: plan.to.clone(), source });
            }
        }
        let n = ops.len();
        let op = Op::Batch { ops };
        let moves: Vec<(String, String)> = plans
            .iter()
            .filter_map(|pl| match (&pl.from_path, &pl.to_path) {
                (Some(a), Some(b)) if a != b => Some((a.clone(), b.clone())),
                _ => None,
            })
            .collect();
        if let Err(e) = move_all(fs, &moves) {
            return Err(EngineError::Other(self.follow_stuck(&op, e, true)));
        }
        if n > 0
            && let Err(err) = self.commit(&format!("Rename {n} Photo{}", if n == 1 { "" } else { "s" }), op)
        {
            let back: Vec<(String, String)> = moves.iter().rev().map(|(a, b)| (b.clone(), a.clone())).collect();
            if let Err(e) = move_all(fs, &back) {
                return Err(EngineError::Other(format!("{err}; the renamed files could not all be moved back: {}", e.message)));
            }
            return Err(err);
        }
        Ok(n)
    }

    /// After a failed batch of moves for `op`: point the photos whose files stayed moved at
    /// their new paths (an undoable "partly" rename when `undoable`, otherwise a plain logged
    /// change), so the library matches the disk. Returns the message for the user.
    pub(crate) fn follow_stuck(&mut self, op: &Op, e: BatchMoveError, undoable: bool) -> String {
        let mut msg = e.message;
        if e.stuck.is_empty() {
            return msg;
        }
        let to: HashSet<&str> = e.stuck.iter().map(|m| m.1.as_str()).collect();
        let mut keep = Vec::new();
        set_file_ops_to(op, &to, &mut keep);
        let list: Vec<String> = e.stuck.iter().map(|(a, b)| format!("{a} → {b}")).collect();
        let n = e.stuck.len();
        let s = if n == 1 { "" } else { "s" };
        let fix = Op::Batch { ops: keep };
        let applied = if undoable {
            self.commit(&format!("Rename {n} Photo{s} (partly)"), fix)
        } else {
            match self.catalog.apply(fix.clone()) {
                Ok(_) => {
                    self.pending_log.push(fix);
                    Ok(())
                }
                Err(e) => Err(e.into()),
            }
        };
        match applied {
            Ok(()) => msg
                .push_str(&format!(". {n} file{s} could not be moved back and kept the new name; the library now points there: {}", list.join(", "))),
            Err(err) => msg.push_str(&format!(
                ". {n} file{s} could not be moved back and the library could not follow ({err}); relink with Find Missing Photos: {}",
                list.join(", ")
            )),
        }
        msg
    }

    /// File moves an undo/redo op implies: `SetFile` ops whose source path differs from the photo's
    /// current one.
    pub(crate) fn file_moves(&self, op: &Op) -> Vec<(String, String)> {
        fn rec(s: &Session, op: &Op, v: &mut Vec<(String, String)>) {
            match op {
                Op::Batch { ops } => ops.iter().for_each(|o| rec(s, o, v)),
                Op::SetFile { id, source: Source::File { path: to }, .. } => {
                    if let Some(Source::File { path: from }) = s.catalog.photo(*id).map(|p| &p.source)
                        && from != to
                        && !v.iter().any(|(f, _)| f == from)
                    {
                        v.push((from.clone(), to.clone()));
                    }
                }
                _ => {}
            }
        }
        let mut v = Vec::new();
        rec(self, op, &mut v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A model volume: files by path with contents; case-insensitive (default APFS/NTFS) or
    /// case-sensitive (Linux, case-sensitive APFS). `fail` makes renames from a matching source fail.
    pub(crate) struct FakeFs {
        pub ci: bool,
        pub files: RefCell<Vec<(String, Vec<u8>)>>,
        pub fail: RefCell<Vec<String>>,
    }

    impl FakeFs {
        pub fn new(ci: bool, files: &[(&str, &str)]) -> FakeFs {
            FakeFs {
                ci,
                files: RefCell::new(files.iter().map(|(p, c)| (p.to_string(), c.as_bytes().to_vec())).collect()),
                fail: RefCell::new(Vec::new()),
            }
        }
        fn idx(&self, p: &Path) -> Option<usize> {
            let p = p.to_string_lossy();
            self.files.borrow().iter().position(|(f, _)| if self.ci { f.to_lowercase() == p.to_lowercase() } else { *f == p })
        }
        /// The listing: (exact path, contents), sorted.
        pub fn listing(&self) -> Vec<(String, String)> {
            let mut v: Vec<_> = self.files.borrow().iter().map(|(p, c)| (p.clone(), String::from_utf8_lossy(c).to_string())).collect();
            v.sort();
            v
        }
    }

    impl MoveFs for FakeFs {
        fn exists(&self, p: &Path) -> bool {
            self.idx(p).is_some()
        }
        fn is_file(&self, p: &Path) -> bool {
            self.exists(p)
        }
        fn same_file(&self, a: &Path, b: &Path) -> bool {
            self.idx(a).is_some() && self.idx(a) == self.idx(b)
        }
        fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
            if self.fail.borrow().iter().any(|f| a.to_string_lossy().contains(f.as_str())) {
                return Err(std::io::Error::other("injected failure"));
            }
            if self.exists(b) {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "exists"));
            }
            let i = self.idx(a).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))?;
            self.files.borrow_mut()[i].0 = b.to_string_lossy().to_string();
            Ok(())
        }
        fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
            if self.exists(b) {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "exists"));
            }
            let i = self.idx(a).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))?;
            let c = self.files.borrow()[i].1.clone();
            self.files.borrow_mut().push((b.to_string_lossy().to_string(), c));
            Ok(())
        }
        fn remove_file(&self, p: &Path) -> std::io::Result<()> {
            let i = self.idx(p).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))?;
            self.files.borrow_mut().remove(i);
            Ok(())
        }
        fn stem_sibling(&self, sidecar: &Path) -> Option<String> {
            let stem = sidecar.with_extension("").to_string_lossy().to_string();
            self.files.borrow().iter().map(|(f, _)| f.clone()).find(|f| {
                let p = Path::new(f);
                p.with_extension("").to_string_lossy() == stem && !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
            })
        }
    }

    fn file_photo(id: u64, path: &str) -> Photo {
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        Photo::new(PhotoId(id), Source::File { path: path.into() }, &name, "JPG", 1, 1, "2026-01-01T00:00:00")
    }

    /// Issue #95: a case-only rename is a real change on a case-insensitive volume and never
    /// replaces a different file that differs only by case on a case-sensitive one.
    #[test]
    fn case_only_renames_never_replace_another_file() {
        // case-sensitive: IMG_1.JPG and img_1.JPG are two photos
        let fs = FakeFs::new(false, &[("/p/IMG_1.JPG", "first"), ("/p/img_1.JPG", "second"), ("/p/IMG_1.xmp", "sc1"), ("/p/img_1.xmp", "sc2")]);
        assert!(move_file_with(&fs, "/p/IMG_1.JPG", "/p/img_1.JPG").unwrap_err().message.contains("already exists"));
        assert_eq!(fs.listing().len(), 4);
        assert!(fs.listing().contains(&("/p/img_1.JPG".into(), "second".into())));
        // the planner picks a free name instead
        let mut s = Session::new();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(file_photo(1, "/p/IMG_1.JPG")) }).unwrap();
        let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "img_1", 1);
        assert_eq!(plans[0].to, "img_1-1.JPG");
        move_file_with(&fs, "/p/IMG_1.JPG", plans[0].to_path.as_deref().unwrap()).unwrap();
        assert_eq!(
            fs.listing(),
            vec![
                ("/p/img_1-1.JPG".into(), "first".into()),
                ("/p/img_1-1.xmp".into(), "sc1".into()),
                ("/p/img_1.JPG".into(), "second".into()),
                ("/p/img_1.xmp".into(), "sc2".into())
            ]
        );
        // case-sensitive, target free: a plain rename
        let fs = FakeFs::new(false, &[("/p/IMG_2.JPG", "x")]);
        move_file_with(&fs, "/p/IMG_2.JPG", "/p/img_2.JPG").unwrap();
        assert_eq!(fs.listing(), vec![("/p/img_2.JPG".into(), "x".into())]);

        // case-insensitive: the "existing" target is the photo itself → renamed via a temp name
        let fs = FakeFs::new(true, &[("/p/IMG_1.JPG", "first"), ("/p/IMG_1.xmp", "sc1")]);
        let mut s = Session::new();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(file_photo(1, "/p/IMG_1.JPG")) }).unwrap();
        let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "img_1", 1);
        assert_eq!(plans[0].to, "img_1.JPG", "not a collision with itself");
        move_file_with(&fs, "/p/IMG_1.JPG", "/p/img_1.JPG").unwrap();
        assert_eq!(fs.listing(), vec![("/p/img_1.JPG".into(), "first".into()), ("/p/img_1.xmp".into(), "sc1".into())]);
        // case-insensitive, another file under another name: still a collision
        let fs = FakeFs::new(true, &[("/p/A.JPG", "a"), ("/p/b.JPG", "b")]);
        assert!(move_file_with(&fs, "/p/A.JPG", "/p/B.JPG").is_err());
    }

    /// Issue #105: a rollback that fails is reported, and the library follows the files that kept
    /// their new names (an undoable partial rename), so nothing goes missing.
    #[test]
    fn failed_rollbacks_are_reported_and_followed() {
        let fs = FakeFs::new(false, &[("/p/a.jpg", "A"), ("/p/a.xmp", "As"), ("/p/b.jpg", "B"), ("/p/c.jpg", "C")]);
        let mut s = Session::new();
        for (i, f) in ["/p/a.jpg", "/p/b.jpg", "/p/c.jpg"].iter().enumerate() {
            s.catalog.apply(Op::AddPhoto { photo: Box::new(file_photo(i as u64 + 1, f)) }).unwrap();
        }
        let ids = [PhotoId(1), PhotoId(2), PhotoId(3)];
        let plans = s.plan_rename_with(&fs, &ids, "Trip-{seq}", 1);
        // c can't be renamed (the share went away), and neither can Trip-1 be moved back
        fs.fail.borrow_mut().extend(["c.jpg".to_string(), "Trip-1".to_string()]);
        let err = s.apply_rename_with(&fs, &plans).unwrap_err().to_string();
        assert!(err.contains("rename /p/c.jpg"), "{err}");
        assert!(err.contains("/p/Trip-1.jpg could not be moved back"), "{err}");
        assert!(err.contains("/p/a.jpg → /p/Trip-1.jpg"), "lists what stayed renamed: {err}");
        assert_eq!(
            fs.listing(),
            vec![
                ("/p/Trip-1.jpg".into(), "A".into()),
                ("/p/Trip-1.xmp".into(), "As".into()),
                ("/p/b.jpg".into(), "B".into()),
                ("/p/c.jpg".into(), "C".into())
            ]
        );
        // the catalog matches the disk
        let path = |s: &Session, id: u64| match &s.catalog.photo(PhotoId(id)).unwrap().source {
            Source::File { path } => path.clone(),
            Source::Demo { .. } => String::new(),
        };
        assert_eq!(path(&s, 1), "/p/Trip-1.jpg");
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().file_name, "Trip-1.jpg");
        assert_eq!((path(&s, 2), path(&s, 3)), ("/p/b.jpg".into(), "/p/c.jpg".into()));
        assert_eq!(s.undo.last().map(|e| e.label.as_str()), Some("Rename 1 Photo (partly)"));

        // a sidecar that won't move, and a file that won't go back: the file keeps its new name
        // with the sidecars that did move, and says so
        let fs = FakeFs::new(false, &[("/p/d.jpg", "D"), ("/p/d.xmp", "Ds"), ("/p/d.jpg.xmp", "Df")]);
        fs.fail.borrow_mut().extend(["d.jpg.xmp".to_string(), "/p/e.jpg".to_string()]);
        let e = move_file_with(&fs, "/p/d.jpg", "/p/e.jpg").unwrap_err();
        assert!(e.moved, "{e}");
        assert!(e.message.contains("it is now /p/e.jpg with its sidecar /p/e.xmp"), "{e}");
        assert_eq!(fs.listing(), vec![("/p/d.jpg.xmp".into(), "Df".into()), ("/p/e.jpg".into(), "D".into()), ("/p/e.xmp".into(), "Ds".into())]);
        // … and when the file does go back, its sidecars follow it
        let fs = FakeFs::new(false, &[("/p/d.jpg", "D"), ("/p/d.xmp", "Ds"), ("/p/d.jpg.xmp", "Df")]);
        fs.fail.borrow_mut().push("d.jpg.xmp".to_string());
        let e = move_file_with(&fs, "/p/d.jpg", "/p/e.jpg").unwrap_err();
        assert!(!e.moved);
        assert_eq!(fs.listing().len(), 3);
        assert!(fs.listing().iter().all(|(p, _)| p.starts_with("/p/d.")));
    }

    /// Issue #92 (comment): renaming one of `IMG_1.CR3` + `IMG_1.JPG` copies their shared
    /// `IMG_1.xmp` instead of taking it away from the other; the last one renamed takes it.
    #[test]
    fn shared_stem_sidecar_is_copied_not_taken() {
        let fs = FakeFs::new(false, &[("/p/IMG_1.CR3", "raw"), ("/p/IMG_1.JPG", "jpg"), ("/p/IMG_1.xmp", "shared"), ("/p/IMG_1.CR3.xmp", "own")]);
        move_file_with(&fs, "/p/IMG_1.CR3", "/p/Trip_1.CR3").unwrap();
        assert_eq!(
            fs.listing(),
            vec![
                ("/p/IMG_1.JPG".into(), "jpg".into()),
                ("/p/IMG_1.xmp".into(), "shared".into()),
                ("/p/Trip_1.CR3".into(), "raw".into()),
                ("/p/Trip_1.CR3.xmp".into(), "own".into()),
                ("/p/Trip_1.xmp".into(), "shared".into()),
            ]
        );
        // the JPEG is the last one using it: it moves
        move_file_with(&fs, "/p/IMG_1.JPG", "/p/Trip_2.JPG").unwrap();
        assert!(fs.listing().contains(&("/p/Trip_2.xmp".into(), "shared".into())));
        assert!(!fs.exists(Path::new("/p/IMG_1.xmp")));
        // a failure after the copy removes the copy and keeps the original
        let fs = FakeFs::new(false, &[("/p/A.CR3", "raw"), ("/p/A.JPG", "jpg"), ("/p/A.xmp", "shared"), ("/p/A.CR3.xmp", "own")]);
        fs.fail.borrow_mut().push("A.CR3.xmp".into());
        assert!(move_file_with(&fs, "/p/A.CR3", "/p/B.CR3").is_err());
        assert_eq!(fs.listing().len(), 4);
        assert!(fs.listing().contains(&("/p/A.xmp".into(), "shared".into())) && !fs.exists(Path::new("/p/B.xmp")));
    }

    /// Issue #95 on the real disk, whichever kind of volume the temp folder is on.
    #[test]
    fn case_only_rename_on_disk() {
        let dir = std::env::temp_dir().join(format!("lc-rename-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (upper, lower) = (dir.join("IMG_1.JPG"), dir.join("img_1.JPG"));
        std::fs::write(&upper, b"first").unwrap();
        let case_sensitive = !lower.exists();
        let p = |q: &Path| q.to_string_lossy().to_string();
        if case_sensitive {
            std::fs::write(&lower, b"second").unwrap();
            assert!(move_file(&p(&upper), &p(&lower)).is_err());
            assert_eq!(std::fs::read(&lower).unwrap(), b"second");
            assert_eq!(std::fs::read(&upper).unwrap(), b"first");
        } else {
            move_file(&p(&upper), &p(&lower)).unwrap();
            let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
            assert_eq!(names, vec!["img_1.JPG".to_string()]);
            assert_eq!(std::fs::read(&lower).unwrap(), b"first");
        }
        // a plain rename never replaces
        std::fs::write(dir.join("other.JPG"), b"other").unwrap();
        assert!(move_file(&p(&dir.join("other.JPG")), &p(&lower)).is_err());
        assert_eq!(std::fs::read(dir.join("other.JPG")).unwrap(), b"other");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_templates_stay_inside_the_destination() {
        let mut p = sample_photo();
        assert_eq!(expand_folder("{date:%Y}/{date:%Y%m%d}", &p, 1), vec!["2026", "20260114"]);
        assert_eq!(expand_folder("{date:%Y}\\{date:%Y-%m}", &p, 1), vec!["2026", "2026-01"], "backslash separates too");
        // metadata can't add levels or climb out
        p.meta.camera = "../../etc/x".into();
        p.meta.title = "..".into();
        assert_eq!(expand_folder("{camera}/{title}/{date:%Y}", &p, 1), vec!["-..-etc-x", "unknown", "2026"]);
        p.meta.camera = "C:\\Windows".into();
        assert_eq!(expand_folder("{camera}", &p, 1), vec!["C--Windows"]);
        // missing metadata → `unknown`; missing capture time → the import time
        p.meta.camera.clear();
        p.captured = None;
        assert_eq!(expand_folder("{camera}//{date:%Y%m%d}", &p, 1), vec!["unknown", "20260120"]);
        for bad in ["", "  ", "/abs/{date}", "\\\\server\\x", "C:/x", "c:x", "~/x", "../{date}", "{date}/../x", "a/./b", "{date:%Y}/.."] {
            assert!(folder_template_error(bad).is_some(), "{bad:?} should be rejected");
        }
        for ok in ["{date:%Y}/{date:%Y%m%d}", "Trips/{camera}", "{date}", "a//b"] {
            assert_eq!(folder_template_error(ok), None, "{ok:?}");
        }
        // even unchecked, rejected levels are skipped rather than followed
        assert_eq!(expand_folder("/../{date:%Y}/./x", &p, 1), vec!["2026", "x"]);
    }

    /// The help lists exactly what `expand_tokens` understands: every listed tag (and alias) is
    /// replaced, never left literal, and the help's examples match the documented behaviour.
    #[test]
    fn token_help_matches_the_implementation() {
        let p = sample_photo();
        for t in TOKENS {
            for tag in std::iter::once(&t.tag).chain(t.aliases) {
                let v = expand_tokens(tag, &p, 1, 1);
                assert!(!v.contains('{'), "{tag} is not a known token (expanded to {v:?})");
                assert!(!v.is_empty(), "{tag}: the sample photo should give an example");
            }
        }
        assert_eq!(token_example("{seq:3}"), "001");
        assert_eq!(token_example("{date}"), "20260114");
        assert_eq!(token_example("{date:%Y%m%d_%H%M%S}"), "20260114_055848");
        assert_eq!(token_example("{ext}"), "CR3");
        assert_eq!(expand("{date:%Y%m%d_%H%M%S}_{seq:3}", &p, 1), "20260114_055848_001.CR3");
        for (d, _) in DATE_DIRECTIVES {
            let v = format_date("2026-01-14T05:58:48", d);
            assert!(!v.contains('%') || *d == "%%", "{d} is not a known directive");
        }
        assert_eq!(unknown_tokens("{date}_{camra}-{seq:2}{x}{camra}"), vec!["{camra}".to_string(), "{x}".to_string()]);
        assert!(unknown_tokens("{name}{ext}{date:%Y}").is_empty());
        let json = token_help_json();
        assert_eq!(json["tokens"].as_array().unwrap().len(), TOKENS.len());
        assert_eq!(json["tokens"][0]["example"], "IMG_0042");
        // the command descriptions that list the tokens list all of them
        for id in ["photo.rename", "photo.renamePreview", "library.import"] {
            let spec = crate::cmd::command_specs().iter().find(|c| c.id == id).unwrap();
            for t in TOKENS.iter().filter(|t| !t.tag.contains(':')) {
                assert!(spec.params.contains(t.tag), "{id} does not mention {}", t.tag);
            }
        }
    }

    #[test]
    fn templates() {
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 1 }, "IMG_0042.CR2", "CR2", 1, 1, "2026-01-01T00:00:00");
        p.captured = Some("2026-09-30T14:05:09".into());
        p.meta.camera = "Model X/2".into();
        p.meta.title = "Sunset: beach".into();
        assert_eq!(expand("{name}", &p, 1), "IMG_0042.CR2");
        assert_eq!(expand("Trip-{seq:3}", &p, 7), "Trip-007.CR2");
        assert_eq!(expand("{date}_{name}", &p, 1), "20260930_IMG_0042.CR2");
        assert_eq!(expand("{date:%Y-%m-%d %H.%M.%S}", &p, 1), "2026-09-30 14.05.09.CR2");
        assert_eq!(expand("{camera} {title}", &p, 1), "Model X-2 Sunset- beach.CR2");
        assert_eq!(expand("{unknown}-{seq}", &p, 12), "{unknown}-12.CR2");
        assert_eq!(expand("  ", &p, 1), "IMG_0042.CR2", "empty template keeps the name");
        assert_eq!(expand("../{name}", &p, 1), "-IMG_0042.CR2", "no path components");
    }

    #[test]
    fn number_folder_and_metadata_tokens() {
        let path = std::path::Path::new("shoots").join("2026-09 Coast").join("DSC_0815.NEF");
        let mut p = Photo::new(PhotoId(1), Source::File { path: path.to_string_lossy().into() }, "DSC_0815.NEF", "NEF", 1, 1, "2026-01-01T00:00:00");
        p.meta.lens = "24-70mm F2.8".into();
        p.meta.iso = Some(400);
        p.meta.creator = "A. Person".into();
        p.rating = 4;
        assert_eq!(expand("{folder}_{num}", &p, 1), "2026-09 Coast_0815.NEF");
        assert_eq!(expand("{rating}star-{iso}-{lens}-{creator}", &p, 1), "4star-400-24-70mm F2.8-A. Person.NEF");
        // no number at the end of the name / no folder: empty
        p.file_name = "beach.NEF".into();
        p.source = Source::Demo { scene: 0 };
        assert_eq!(expand("{folder}{name}{num}", &p, 1), "beach.NEF");
        // a bare {seq} is padded to the caller's width
        assert_eq!(expand_tokens("{seq}|{seq:2}", &p, 7, 3), "007|07");
    }
}
