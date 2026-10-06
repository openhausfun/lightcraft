//! Filtering, search and sorting.

use serde::{Deserialize, Serialize};

use crate::{AlbumId, Catalog, ColorLabel, Flag, MediaKind, Photo, PhotoId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatingOp {
    #[default]
    AtLeast,
    Exactly,
    AtMost,
}

/// What the grid shows. Empty/None fields don't filter.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filter {
    /// Free-text search: every token must match filename, title, caption, keywords, camera, lens,
    /// location or format. Tokens like `rating:3`, `flag:pick`, `iso:>800`, `camera:x2` are fielded.
    pub text: String,
    pub rating: u8,
    pub rating_op: RatingOp,
    pub flag: Option<Flag>,
    pub label: Option<ColorLabel>,
    /// Only these photos (Find Similar results…); empty = no constraint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<PhotoId>,
    /// Any of these labels (the filter bar's multi-select); empty = no constraint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<ColorLabel>,
    pub kind: Option<MediaKind>,
    pub edited: Option<bool>,
    pub album: Option<AlbumId>,
    /// Show "Recently Deleted" instead of the library.
    pub deleted: bool,
    /// Capture date prefix (`2026`, `2026-04`, `2026-04-12`).
    pub date: Option<String>,
    /// A keyword; hierarchical keywords match their children too (`travel` finds `travel|italy`).
    pub keyword: Option<String>,
    pub camera: Option<String>,
    /// Lens (case-insensitive substring).
    pub lens: Option<String>,
    /// Capture date range, inclusive: `dateFrom` is compared as a lower bound (`2026-04-01`),
    /// `dateTo` as a prefix upper bound (`2026-04` includes all of April).
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    /// Import date prefix.
    pub imported: Option<String>,
    /// Imported at or after this time (ISO; Recently Added).
    pub imported_from: Option<String>,
    /// Photo Merge results: `hdr`, `panorama`, `hdrPanorama` or `any` (see [`merged_kind`]).
    pub merged: Option<String>,
    /// A folder on disk: its files only (browsed ones too); `subfolders` includes everything below.
    pub folder: Option<String>,
    pub subfolders: bool,
    /// Smart-album rules (all / any / none, nested groups; see [`crate::rules`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule_set: Option<crate::RuleSet>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortKey {
    #[default]
    CaptureDate,
    ImportDate,
    EditDate,
    FileName,
    Rating,
    FileSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
    /// Date headers in the grid (date sort keys only).
    pub group: crate::GroupBy,
}

impl Default for Sort {
    fn default() -> Self {
        Sort { key: SortKey::CaptureDate, ascending: false, group: crate::GroupBy::Auto }
    }
}

fn token_matches(p: &Photo, tok: &str) -> bool {
    let t = tok.to_lowercase();
    if let Some((field, val)) = t.split_once(':') {
        let num = |s: &str| -> Option<(char, f64)> {
            let (op, rest) = match s.chars().next()? {
                c @ ('>' | '<' | '=') => (c, &s[1..]),
                _ => ('=', s),
            };
            rest.parse().ok().map(|v| (op, v))
        };
        let cmp = |x: f64, s: &str| match num(s) {
            Some(('>', v)) => x > v,
            Some(('<', v)) => x < v,
            Some((_, v)) => (x - v).abs() < 1e-9,
            None => false,
        };
        return match field {
            "rating" | "stars" => cmp(p.rating as f64, val),
            "flag" => Flag::parse(val) == Some(p.flag),
            "label" | "color" => p.label.is_some_and(|l| format!("{l:?}").eq_ignore_ascii_case(val)),
            "iso" => p.meta.iso.is_some_and(|i| cmp(i as f64, val)),
            "f" | "aperture" => p.meta.aperture.is_some_and(|a| cmp(a as f64, val)),
            "focal" => p.meta.focal_mm.is_some_and(|a| cmp(a as f64, val)),
            "camera" => p.meta.camera.to_lowercase().contains(val),
            "lens" => p.meta.lens.to_lowercase().contains(val),
            "keyword" | "kw" => p.meta.keywords.iter().any(|k| crate::keywords::is_under(k, val)),
            "type" | "kind" => format!("{:?}", p.kind).eq_ignore_ascii_case(val),
            "edited" => (val == "true" || val == "yes") == p.is_edited(),
            "date" => p.date().starts_with(val),
            "copy" | "virtual" => (val == "true" || val == "yes") == p.copy_of.is_some(),
            "name" | "file" => p.file_name.to_lowercase().contains(val),
            _ => false,
        };
    }
    let hay = [&p.file_name, &p.meta.title, &p.meta.caption, &p.meta.camera, &p.meta.lens, &p.meta.location, &p.format];
    hay.iter().any(|h| h.to_lowercase().contains(&t)) || p.meta.keywords.iter().any(|k| k.to_lowercase().contains(&t))
}

impl Filter {
    /// Human-readable summary of the active rules (smart album tooltips, `album.list`).
    pub fn describe(&self) -> String {
        let mut v: Vec<String> = Vec::new();
        if self.rating > 0 {
            let op = match self.rating_op {
                RatingOp::AtLeast => "≥",
                RatingOp::Exactly => "=",
                RatingOp::AtMost => "≤",
            };
            v.push(format!("rating {op} {}", self.rating));
        }
        if let Some(f) = self.flag {
            v.push(format!("flag {}", format!("{f:?}").to_lowercase()));
        }
        if let Some(l) = self.label {
            v.push(format!("label {}", format!("{l:?}").to_lowercase()));
        }
        if !self.labels.is_empty() {
            v.push(format!("label {}", self.labels.iter().map(|l| format!("{l:?}").to_lowercase()).collect::<Vec<_>>().join(" or ")));
        }
        if let Some(k) = self.kind {
            v.push(format!("kind {}", format!("{k:?}").to_lowercase()));
        }
        if let Some(e) = self.edited {
            v.push(if e { "edited".into() } else { "unedited".into() });
        }
        for (name, val) in
            [("keyword", &self.keyword), ("camera", &self.camera), ("lens", &self.lens), ("date", &self.date), ("imported", &self.imported)]
        {
            if let Some(x) = val {
                v.push(format!("{name} {x}"));
            }
        }
        match (&self.date_from, &self.date_to) {
            (Some(a), Some(b)) => v.push(format!("captured {a} – {b}")),
            (Some(a), None) => v.push(format!("captured from {a}")),
            (None, Some(b)) => v.push(format!("captured until {b}")),
            _ => {}
        }
        if !self.text.trim().is_empty() {
            v.push(format!("“{}”", self.text.trim()));
        }
        if let Some(rs) = self.rule_set.as_ref().filter(|r| !r.rules.is_empty()) {
            v.push(rs.describe());
        }
        if v.is_empty() { "all photos".into() } else { v.join(", ") }
    }

    /// Whether matches depend on the clock (only "in the last…" rules do).
    pub fn depends_on_now(&self) -> bool {
        self.rule_set.as_ref().is_some_and(crate::RuleSet::depends_on_now)
    }

    pub fn matches(&self, p: &Photo, cat: &Catalog) -> bool {
        if p.deleted != self.deleted {
            return false;
        }
        match &self.folder {
            // browsed photos only show in folder views
            None if p.local => return false,
            None => {}
            Some(dir) => {
                let crate::Source::File { path } = &p.source else { return false };
                if !in_folder(path, dir, self.subfolders) {
                    return false;
                }
            }
        }
        if self.rating > 0 {
            let ok = match self.rating_op {
                RatingOp::AtLeast => p.rating >= self.rating,
                RatingOp::Exactly => p.rating == self.rating,
                RatingOp::AtMost => p.rating <= self.rating,
            };
            if !ok {
                return false;
            }
        }
        if self.flag.is_some_and(|f| f != p.flag) || self.label.is_some_and(|l| Some(l) != p.label) || self.kind.is_some_and(|k| k != p.kind) {
            return false;
        }
        if self.edited.is_some_and(|e| e != p.is_edited()) {
            return false;
        }
        if let Some(rs) = &self.rule_set
            && !rs.matches(p, cat)
        {
            return false;
        }
        if !self.labels.is_empty() && !p.label.is_some_and(|l| self.labels.contains(&l)) {
            return false;
        }
        if !self.only.is_empty() && !self.only.contains(&p.id) {
            return false;
        }
        if let Some(want) = &self.merged {
            match merged_kind(&p.file_name) {
                Some(k) if want == "any" || want == k => {}
                _ => return false,
            }
        }
        if let Some(a) = self.album
            && !cat.album_contains(a, p)
        {
            return false;
        }
        if let Some(from) = &self.date_from
            && p.date() < from.as_str()
        {
            return false;
        }
        if let Some(to) = &self.date_to
            && p.date().get(..to.len()).unwrap_or(p.date()) > to.as_str()
        {
            return false;
        }
        if let Some(l) = &self.lens
            && !p.meta.lens.to_lowercase().contains(&l.to_lowercase())
        {
            return false;
        }
        if let Some(d) = &self.date
            && !p.date().starts_with(d.as_str())
        {
            return false;
        }
        if let Some(d) = &self.imported
            && !p.imported.starts_with(d.as_str())
        {
            return false;
        }
        if let Some(d) = &self.imported_from
            && p.imported.as_str() < d.as_str()
        {
            return false;
        }
        if let Some(k) = &self.keyword
            && !p.meta.keywords.iter().any(|x| crate::keywords::is_under(x, k))
        {
            return false;
        }
        if let Some(c) = &self.camera
            && !p.meta.camera.eq_ignore_ascii_case(c)
        {
            return false;
        }
        self.text.split_whitespace().all(|tok| token_matches(p, tok))
    }
}

impl Catalog {
    /// Photos matching `filter`, in `sort` order (ties broken by id for stability).
    pub fn query(&self, filter: &Filter, sort: &Sort) -> Vec<PhotoId> {
        let mut v: Vec<&Photo> = self.photos().map(|p| p.as_ref()).filter(|p| filter.matches(p, self)).collect();
        v.sort_by(|a, b| {
            let o = match sort.key {
                SortKey::CaptureDate => a.date().cmp(b.date()),
                SortKey::ImportDate => a.imported.cmp(&b.imported),
                SortKey::EditDate => a.edited.cmp(&b.edited),
                SortKey::FileName => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                SortKey::Rating => a.rating.cmp(&b.rating),
                SortKey::FileSize => a.file_size.cmp(&b.file_size),
            }
            .then(a.id.cmp(&b.id));
            if sort.ascending { o } else { o.reverse() }
        });
        v.into_iter().map(|p| p.id).collect()
    }

    /// Year → month → day counts for the "By Date" section (newest first).
    pub fn date_groups(&self) -> Vec<DateGroup> {
        use std::collections::BTreeMap;
        let mut years: BTreeMap<&str, (BTreeMap<&str, usize>, BTreeMap<&str, usize>)> = BTreeMap::new();
        for p in self.photos().filter(|p| p.in_library()) {
            let d = p.date();
            if d.len() >= 10 {
                let (months, days) = years.entry(&d[..4]).or_default();
                *months.entry(&d[..7]).or_default() += 1;
                *days.entry(&d[..10]).or_default() += 1;
            }
        }
        let newest_first = |m: BTreeMap<&str, usize>| m.into_iter().rev().map(|(k, n)| (k.to_string(), n)).collect::<Vec<_>>();
        years
            .into_iter()
            .rev()
            .map(|(year, (months, days))| DateGroup {
                year: year.to_string(),
                count: months.values().sum(),
                months: newest_first(months),
                days: newest_first(days),
            })
            .collect()
    }

    /// All keywords with usage counts, sorted by name.
    pub fn keywords(&self) -> Vec<(String, usize)> {
        let mut m: std::collections::BTreeMap<String, usize> = Default::default();
        for p in self.photos().filter(|p| p.in_library()) {
            for k in &p.meta.keywords {
                *m.entry(k.clone()).or_default() += 1;
            }
        }
        m.into_iter().collect()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DateGroup {
    pub year: String,
    pub count: usize,
    /// `YYYY-MM` and counts, newest first.
    pub months: Vec<(String, usize)>,
    /// `YYYY-MM-DD` and counts, newest first.
    pub days: Vec<(String, usize)>,
}

/// Whether file `path` is directly in `dir` (or anywhere below it with `deep`). Both `/` and `\\`
/// separate.
pub fn in_folder(path: &str, dir: &str, deep: bool) -> bool {
    let dir = dir.trim_end_matches(['/', '\\']);
    let Some(rest) = path.strip_prefix(dir) else { return false };
    let Some(rest) = rest.strip_prefix(['/', '\\']) else { return false };
    !rest.is_empty() && (deep || !rest.contains(['/', '\\']))
}

/// A folder path's identity, for telling whether two spellings name the same folder: `/` and
/// `\\` are both separators, repeated separators, `.` and a trailing separator are dropped,
/// `..` is resolved lexically, a Windows verbatim prefix (`\\?\`) is removed and the drive
/// letter lower-cased; on Windows (case-insensitive file names) the whole path is lower-cased.
/// No file-system access: the paths compared should already be absolute.
pub fn folder_key(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/").or_else(|| s.strip_prefix("//./")) {
        s = match rest.strip_prefix("UNC/") {
            Some(unc) => format!("//{unc}"),
            None => rest.to_string(),
        };
    }
    // the part `..` can't climb above: `//server/share`, `c:` or `/`
    let (prefix, rest) = if let Some(unc) = s.strip_prefix("//").filter(|u| !u.is_empty() && !u.starts_with('/')) {
        let mut it = unc.splitn(3, '/');
        let (server, share, rest) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
        (format!("//{server}/{share}"), rest.to_string())
    } else if s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic() {
        (s[..2].to_ascii_lowercase(), s[2..].to_string())
    } else {
        (String::new(), s.clone())
    };
    let absolute = rest.starts_with('/') || !prefix.is_empty();
    let mut parts: Vec<&str> = Vec::new();
    for c in rest.split('/') {
        match c {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            ".." if absolute => {}
            c => parts.push(c),
        }
    }
    let key = if absolute { format!("{prefix}/{}", parts.join("/")) } else { parts.join("/") };
    if cfg!(windows) { key.to_lowercase() } else { key }
}

/// Whether `path` is folder `root` or lies somewhere inside it, however either is spelled
/// (see [`folder_key`]).
pub fn folder_within(path: &str, root: &str) -> bool {
    let (p, r) = (folder_key(path), folder_key(root));
    p == r || (p.starts_with(&r) && (r.ends_with('/') || p[r.len()..].starts_with('/')))
}

/// The kind of Photo Merge result a file is, from the name merges give it (`IMG_1-HDR.dng`,
/// `IMG_1-Pano.dng`, `IMG_1-HDR-Pano.dng` — also the names other raw editors use):
/// `hdr`, `panorama` or `hdrPanorama`.
pub fn merged_kind(file_name: &str) -> Option<&'static str> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s).to_ascii_lowercase();
    let stem = stem.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == ' ');
    if stem.ends_with("-hdr-pano") {
        Some("hdrPanorama")
    } else if stem.ends_with("-hdr") {
        Some("hdr")
    } else if stem.ends_with("-pano") {
        Some("panorama")
    } else {
        None
    }
}
