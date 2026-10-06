//! XMP sidecars: `<stem>.xmp` (default) or `<file>.xmp` next to the original.
//!
//! **Write** ([`Session::save_sidecar`], `photo.saveMetadataToFile`, or automatically after every
//! change when [`XmpPrefs::auto_write`] is on): rating, colour label, title, caption, copyright,
//! creator, keywords, capture time and GPS in the standard namespaces, plus our complete develop
//! settings (`lc:settings`, JSON), the pick/reject flag (`lc:flag`) and location (`lc:location`).
//!
//! An existing sidecar is never replaced wholesale (issue #92): LightCraft's properties
//! ([`OWNED`], [`OWNED_IF_STATED`]) are merged into it ([`lightcraft_meta::merge_xmp`]) and
//! everything else — another editor's `crs:` develop settings, `xmpMM` history, unknown
//! namespaces — is kept byte for byte. A sidecar that can't be read as XMP is first copied to
//! `<name>.xmp.bak-<time>` (never overwritten), then replaced. Writes are atomic (temp file,
//! fsync, rename).
//!
//! **Shared names** (Stem naming): `IMG_0001.CR3` and `IMG_0001.JPG` would share `IMG_0001.xmp`.
//! When two catalogued files map to one stem sidecar, it belongs to one of them (a raw first, then
//! the first by file name — [`Session::sidecar_naming`]); the others use `<file>.xmp`
//! (`IMG_0001.JPG.xmp`) for writing and reading, so neither overwrites the other.
//!
//! **Read** (on import, and `photo.readMetadataFromFile`): the sidecar wins for metadata, except
//! the capture time: a time embedded in the file wins, and the sidecar's (`exif:DateTimeOriginal`,
//! else `photoshop:DateCreated`, else `xmp:CreateDate`) fills in only when the file has none; develop
//! settings are restored from `lc:settings` when present, else mapped from interoperable `crs:`
//! fields ([`crate::crs`]). For raw/DNG files without a sidecar the file's embedded XMP is used.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::{ColorLabel, Flag, MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::DevelopSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{EngineError, Result, Session};

/// Sidecar file naming.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidecarNaming {
    /// `IMG_0001.xmp` (the common convention; shared by `IMG_0001.CR3` and `IMG_0001.JPG`).
    #[default]
    Stem,
    /// `IMG_0001.CR3.xmp` (unambiguous when raw+JPEG pairs live side by side).
    Full,
}

/// Library preferences for XMP sidecars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct XmpPrefs {
    /// Write the sidecar after every change to a photo's metadata or develop settings.
    pub auto_write: bool,
    pub naming: SidecarNaming,
}

/// The sidecar path for an original, by naming convention.
pub fn sidecar_path(original: &str, naming: SidecarNaming) -> PathBuf {
    match naming {
        SidecarNaming::Stem => Path::new(original).with_extension("xmp"),
        SidecarNaming::Full => PathBuf::from(format!("{original}.xmp")),
    }
}

/// An existing sidecar for `original`: the preferred naming first, then the other one (also
/// accepting an upper-case `.XMP`).
pub fn find_sidecar(original: &str, naming: SidecarNaming) -> Option<PathBuf> {
    let other = if naming == SidecarNaming::Stem { SidecarNaming::Full } else { SidecarNaming::Stem };
    [naming, other]
        .into_iter()
        .flat_map(|n| {
            let p = sidecar_path(original, n);
            let upper = p.with_extension("XMP");
            [p, upper]
        })
        .find(|p| p.is_file())
}

/// What a sidecar (or embedded XMP packet) says about a photo. `None` = not stated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidecarData {
    pub rating: Option<u8>,
    pub flag: Option<Flag>,
    pub label: Option<Option<ColorLabel>>,
    /// The label's text as written (`xmp:Label`), for [`SidecarData::resolve_label`].
    pub label_text: Option<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    pub copyright: Option<String>,
    /// `xmpRights:Marked`: `Some(true)` copyrighted, `Some(false)` public domain.
    pub copyright_marked: Option<bool>,
    pub usage_terms: Option<String>,
    pub copyright_url: Option<String>,
    pub creator: Option<String>,
    pub location: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub alt_text: Option<String>,
    pub extended_description: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// Capture time (ISO 8601) from `exif:DateTimeOriginal`, `photoshop:DateCreated` or
    /// `xmp:CreateDate` (first found). Used only when the file itself has no capture time.
    pub captured: Option<String>,
    pub develop: Option<DevelopPatch>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DevelopPatch {
    /// Our own complete settings (`lc:settings`).
    Full(Box<DevelopSettings>),
    /// Mapped `crs:` fields (partial JSON, merged like a preset).
    Partial(Value),
}

/// Parse a sidecar / XMP packet. `raw` selects absolute (raw) vs relative white balance for `crs:`.
pub fn parse_sidecar(xmp: &str, raw: bool) -> std::result::Result<SidecarData, String> {
    let d = lightcraft_meta::parse_xmp(xmp).map_err(|e| e.to_string())?;
    let m = &d.metadata;
    let lc = |k: &str| d.properties.get(&format!("lc:{k}")).and_then(|v| v.first()).cloned();
    let mut out = SidecarData {
        title: m.title.clone(),
        caption: m.caption.clone(),
        copyright: m.copyright.clone(),
        copyright_marked: m.copyright_marked,
        usage_terms: m.usage_terms.clone(),
        copyright_url: m.copyright_url.clone(),
        creator: m.artist.clone(),
        location: lc("location").or_else(|| m.sublocation.clone()),
        city: m.city.clone(),
        state: m.state.clone(),
        country: m.country.clone(),
        alt_text: m.alt_text.clone(),
        extended_description: m.extended_description.clone(),
        keywords: (!m.keywords.is_empty()).then(|| m.keywords.clone()),
        captured: m.capture_time.map(|d| d.to_iso()),
        ..Default::default()
    };
    if let Some(r) = m.rating {
        if r < 0 {
            out.flag = Some(Flag::Reject);
            out.rating = Some(0);
        } else {
            out.rating = Some(r.clamp(0, 5) as u8);
        }
    }
    if let Some(f) = lc("flag").and_then(|f| Flag::parse(&f)) {
        out.flag = Some(f);
    }
    if let Some(l) = &m.label {
        out.label = Some(ColorLabel::parse(l.trim()));
        out.label_text = Some(l.trim().to_string());
    }
    let full = d.lc_settings.as_deref().and_then(|j| serde_json::from_str::<Value>(j).ok()).and_then(|v| DevelopSettings::from_json(&v).ok());
    out.develop = match full {
        Some(s) => Some(DevelopPatch::Full(Box::new(s))),
        None if crate::crs::has_adjustments(&d.properties) => {
            Some(DevelopPatch::Partial(crate::crs::to_partial_report(&d.properties, Some(&d.values), Some(raw), crate::crs_masks::DEFAULT_ASPECT).0))
        }
        None => None,
    };
    Ok(out)
}

/// Apply sidecar data to a photo record (the sidecar wins, except that a capture time embedded in
/// the file is kept). Returns true if develop changed.
pub fn merge_into(p: &mut Photo, sc: &SidecarData, now: &str) -> bool {
    if p.captured.is_none() {
        p.captured = sc.captured.clone();
    }
    if let Some(r) = sc.rating {
        p.rating = r;
    }
    if let Some(f) = sc.flag {
        p.flag = f;
    }
    if let Some(l) = sc.label {
        p.label = l;
    }
    let m = &mut p.meta;
    for (src, dst) in [
        (&sc.title, &mut m.title),
        (&sc.caption, &mut m.caption),
        (&sc.copyright, &mut m.copyright),
        (&sc.usage_terms, &mut m.usage_terms),
        (&sc.copyright_url, &mut m.copyright_url),
        (&sc.creator, &mut m.creator),
        (&sc.location, &mut m.location),
        (&sc.city, &mut m.city),
        (&sc.state, &mut m.state),
        (&sc.country, &mut m.country),
        (&sc.alt_text, &mut m.alt_text),
        (&sc.extended_description, &mut m.extended_description),
    ] {
        if let Some(v) = src {
            *dst = v.clone();
        }
    }
    if let Some(marked) = sc.copyright_marked {
        m.copyright_status = lightcraft_catalog::CopyrightStatus::from_marked(Some(marked));
    }
    if let Some(k) = &sc.keywords {
        m.keywords = k.clone();
    }
    let develop = match &sc.develop {
        Some(DevelopPatch::Full(s)) => (**s).clone(),
        Some(DevelopPatch::Partial(v)) => {
            let mut v = v.clone();
            if p.width > 0 && p.height > 0 {
                crate::crs_masks::refit_radials(&mut v, crate::crs_masks::DEFAULT_ASPECT, p.width as f64 / p.height as f64);
            }
            lightcraft_develop::apply_partial(&p.develop, &v, 1.0)
        }
        None => return false,
    };
    if develop == *p.develop {
        return false;
    }
    p.develop = Arc::new(develop);
    p.edited = Some(now.to_string());
    true
}

impl SidecarData {
    /// Read the label through the catalog's label names: other editors write the label's name
    /// (a custom set's "To Do" as well as "Red"); unknown names clear the label.
    pub fn resolve_label(mut self, cat: &lightcraft_catalog::Catalog) -> Self {
        if let Some(t) = &self.label_text {
            self.label = Some(if t.is_empty() { None } else { cat.label_from_name(t) });
        }
        self
    }
}

/// The sidecar packet for a photo; the colour label is written by its name in `cat` (custom
/// label names included, as other editors do).
pub fn sidecar_packet(p: &Photo, cat: &lightcraft_catalog::Catalog) -> String {
    let nz = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    let meta = lightcraft_meta::Metadata {
        software: Some("LightCraft".into()),
        title: nz(&p.meta.title),
        caption: nz(&p.meta.caption),
        alt_text: nz(&p.meta.alt_text),
        extended_description: nz(&p.meta.extended_description),
        sublocation: nz(&p.meta.location),
        city: nz(&p.meta.city),
        state: nz(&p.meta.state),
        country: nz(&p.meta.country),
        copyright: nz(&p.meta.copyright),
        copyright_marked: p.meta.copyright_status.marked(),
        usage_terms: nz(&p.meta.usage_terms),
        copyright_url: nz(&p.meta.copyright_url),
        artist: nz(&p.meta.creator),
        keywords: p.meta.keywords.clone(),
        rating: Some(p.rating.min(5) as i8),
        label: p.label.map(|l| cat.label_name(l)),
        capture_time: p.captured.as_deref().and_then(lightcraft_meta::DateTime::parse_iso),
        gps: p.meta.gps.map(|(latitude, longitude)| lightcraft_meta::Gps { latitude, longitude, altitude: None }),
        ..Default::default()
    };
    let settings = serde_json::to_string(&*p.develop).unwrap_or_default();
    let flag = match p.flag {
        Flag::None => "none",
        Flag::Pick => "pick",
        Flag::Reject => "reject",
    };
    let mut lc = vec![("settings", settings.as_str()), ("flag", flag)];
    if !p.meta.location.trim().is_empty() {
        lc.push(("location", p.meta.location.as_str()));
    }
    lightcraft_meta::write_xmp_lc(&meta, &lc)
}

/// The properties LightCraft writes and owns in a sidecar: replaced on every save (removed when
/// LightCraft has no value, so clearing a field clears it in the file too). All of them are read
/// back on import and by Read Metadata from File.
pub const OWNED: &[&str] = &[
    "xmp:Rating",
    "xmp:Label",
    "dc:title",
    "dc:description",
    "dc:rights",
    "dc:creator",
    "dc:subject",
    "Iptc4xmpCore:Location",
    "Iptc4xmpCore:AltTextAccessibility",
    "Iptc4xmpCore:ExtDescrAccessibility",
    "photoshop:City",
    "photoshop:State",
    "photoshop:Country",
    "xmpRights:Marked",
    "xmpRights:UsageTerms",
    "xmpRights:WebStatement",
    "lc:*",
];

/// Properties replaced only when LightCraft has a value (capture time; GPS, which LightCraft
/// doesn't read from sidecars): otherwise the sidecar's own value stays. Anything else LightCraft
/// writes (`xmp:CreatorTool`) goes only into new sidecars.
pub const OWNED_IF_STATED: &[&str] = &["exif:DateTimeOriginal", "photoshop:DateCreated", "exif:GPSLatitude", "exif:GPSLongitude"];

/// Write `data` to `path` atomically: a temp file next to it, synced, then renamed over it.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    lightcraft_catalog::safe_file::write_atomic(path, data)
}

/// Copy `path` to a new `<name>.bak-<stamp>` (`-1`, `-2`… if taken; never overwriting).
fn backup(path: &Path, stamp: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let bytes = std::fs::read(path)?;
    let stamp: String = stamp.chars().filter(char::is_ascii_alphanumeric).collect();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    for k in 0..1000 {
        let b = path.with_file_name(if k == 0 { format!("{name}.bak-{stamp}") } else { format!("{name}.bak-{stamp}-{k}") });
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&b) {
            Ok(mut f) => {
                f.write_all(&bytes)?;
                f.sync_all()?;
                return Ok(b);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free backup name"))
}

/// What [`Session::save_sidecar`] did.
#[derive(Clone, Debug, PartialEq)]
pub struct SidecarSaved {
    pub path: PathBuf,
    /// An existing sidecar was merged into (its other contents kept).
    pub merged: bool,
    /// An existing sidecar that couldn't be read as XMP was copied here before being replaced.
    pub backup: Option<PathBuf>,
}

/// The new contents for the sidecar at `path`: `fresh` merged into the existing file, if any.
/// Returns (contents, merged, backup).
fn sidecar_contents(path: &Path, fresh: String, stamp: &str) -> std::io::Result<(String, bool, Option<PathBuf>)> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((fresh, false, None)),
        // can't see what is there: never overwrite it blindly
        Err(e) => return Err(e),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok((fresh, false, None));
    }
    let rules = lightcraft_meta::MergeRules { owned: OWNED, owned_if_present: OWNED_IF_STATED };
    match std::str::from_utf8(&bytes)
        .map_err(|e| e.to_string())
        .and_then(|old| lightcraft_meta::merge_xmp(old, &fresh, rules).map_err(|e| e.to_string()))
    {
        Ok(merged) => Ok((merged, true, None)),
        Err(why) => {
            let b = backup(path, stamp)?;
            log::warn!("{}: not readable as XMP ({why}); kept a copy as {} before replacing it", path.display(), b.display());
            Ok((fresh, false, Some(b)))
        }
    }
}

/// Key of the stem sidecar a file maps to (case-insensitive).
fn stem_key(original: &str) -> String {
    sidecar_path(original, SidecarNaming::Stem).to_string_lossy().to_lowercase()
}

fn file_path(p: &Photo) -> Option<&str> {
    match &p.source {
        Source::File { path } => Some(path),
        Source::Demo { .. } => None,
    }
}

/// The XMP packet that describes a photo on disk: its sidecar, else (raw/DNG) the embedded XMP.
pub fn read_packet(original: &str, kind: MediaKind, naming: SidecarNaming) -> Option<(String, PathBuf)> {
    if let Some(sc) = find_sidecar(original, naming)
        && let Ok(s) = std::fs::read_to_string(&sc)
    {
        return Some((s, sc));
    }
    if kind == MediaKind::Raw {
        let bytes = std::fs::read(original).ok()?;
        let x = lightcraft_meta::embedded(&bytes).xmp?;
        return Some((x, PathBuf::from(original)));
    }
    None
}

/// Photo ids touched by an op (for auto-write).
pub(crate) fn op_photos(op: &Op, out: &mut Vec<PhotoId>) {
    match op {
        Op::SetRating { id, .. } | Op::SetFlag { id, .. } | Op::SetLabel { id, .. } | Op::SetDevelop { id, .. } | Op::SetMeta { id, .. } => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        Op::Batch { ops } => ops.iter().for_each(|o| op_photos(o, out)),
        _ => {}
    }
}

/// Which photo owns each stem sidecar that several catalogued files map to (see
/// [`Session::sidecar_naming`]).
pub(crate) struct StemOwners(HashMap<String, PhotoId>);

impl StemOwners {
    pub(crate) fn of(cat: &lightcraft_catalog::Catalog) -> StemOwners {
        // per stem: (raw first, then file name, then id) of the best candidate; and how many
        let mut best: HashMap<String, ((bool, String, u64), PhotoId, usize)> = HashMap::new();
        for p in cat.photos().filter(|p| p.copy_of.is_none()) {
            let Some(path) = file_path(p) else { continue };
            let rank = (p.kind != MediaKind::Raw, path.to_lowercase(), p.id.0);
            let e = best.entry(stem_key(path)).or_insert_with(|| (rank.clone(), p.id, 0));
            e.2 += 1;
            if rank < e.0 {
                (e.0, e.1) = (rank, p.id);
            }
        }
        StemOwners(best.into_iter().filter(|(_, v)| v.2 > 1).map(|(k, v)| (k, v.1)).collect())
    }

    /// `naming` for photo `p`, except that a stem sidecar another file owns becomes Full.
    fn naming(&self, p: &Photo, naming: SidecarNaming) -> SidecarNaming {
        match (naming, file_path(p)) {
            (SidecarNaming::Stem, Some(path)) if self.0.get(&stem_key(path)).is_some_and(|owner| *owner != p.id) => SidecarNaming::Full,
            _ => naming,
        }
    }
}

impl Session {
    /// The sidecar naming for photo `id`: the library's, except under Stem naming when another
    /// catalogued file shares the stem (`IMG_0001.CR3` + `IMG_0001.JPG`) and owns the stem
    /// sidecar — a raw first, else the first by file name. The others use Full naming
    /// (`IMG_0001.JPG.xmp`), so their metadata never overwrites each other.
    pub fn sidecar_naming(&self, id: PhotoId) -> SidecarNaming {
        match self.catalog.photo(id) {
            Some(p) if self.xmp.naming == SidecarNaming::Stem => StemOwners::of(&self.catalog).naming(p, self.xmp.naming),
            _ => self.xmp.naming,
        }
    }

    /// Write the photo's XMP sidecar, merged into an existing one (see the module docs).
    pub fn save_sidecar(&self, id: PhotoId) -> Result<SidecarSaved> {
        self.save_sidecar_with(id, &StemOwners::of(&self.catalog))
    }

    pub(crate) fn save_sidecar_with(&self, id: PhotoId, owners: &StemOwners) -> Result<SidecarSaved> {
        let p = self.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        if p.copy_of.is_some() {
            return Err(EngineError::Other(format!("{} is a virtual copy: its settings live only in the library", p.file_name)));
        }
        let orig = file_path(p).ok_or_else(|| EngineError::Other(format!("{} is not a file on disk", p.file_name)))?;
        let path = sidecar_path(orig, owners.naming(p, self.xmp.naming));
        let err = |e: std::io::Error| EngineError::Other(format!("{}: {e}", path.display()));
        let (data, merged, backup) = sidecar_contents(&path, sidecar_packet(p, &self.catalog), &(self.clock)()).map_err(err)?;
        write_atomic(&path, data.as_bytes()).map_err(err)?;
        Ok(SidecarSaved { path, merged, backup })
    }

    /// The op that applies a photo's sidecar (or embedded XMP) to the catalog, if there is one.
    pub fn read_sidecar_op(&self, id: PhotoId) -> Result<Option<(Op, PathBuf)>> {
        let p = self.catalog.photo(id).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        let Some(orig) = file_path(p) else { return Ok(None) };
        let Some((packet, from)) = read_packet(orig, p.kind, self.sidecar_naming(id)) else { return Ok(None) };
        let sc = parse_sidecar(&packet, p.kind == MediaKind::Raw)
            .map_err(|e| EngineError::Other(format!("{}: {e}", from.display())))?
            .resolve_label(&self.catalog);
        let mut q = (**p).clone();
        let develop_changed = merge_into(&mut q, &sc, &(self.clock)());
        let mut ops = vec![
            Op::SetRating { id, rating: q.rating },
            Op::SetFlag { id, flag: q.flag },
            Op::SetLabel { id, label: q.label },
            Op::SetMeta { id, meta: Box::new(q.meta.clone()) },
        ];
        if q.captured != p.captured {
            ops.push(Op::SetCaptured { id, captured: q.captured.clone() });
        }
        if develop_changed {
            ops.extend(self.develop_op(id, (*q.develop).clone(), "Read Metadata from File"));
        }
        Ok(Some((Op::Batch { ops }, from)))
    }

    /// Auto-write: sidecars for photos changed by `ops` (errors are logged, not returned).
    pub(crate) fn auto_write_sidecars(&self, ops: &[Op]) {
        let mut ids = Vec::new();
        ops.iter().for_each(|o| op_photos(o, &mut ids));
        if ids.is_empty() {
            return;
        }
        let owners = StemOwners::of(&self.catalog);
        for id in ids {
            if self.catalog.photo(id).is_some_and(|p| file_path(p).is_some() && p.copy_of.is_none())
                && let Err(e) = self.save_sidecar_with(id, &owners)
            {
                log::warn!("auto-write XMP: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::Meta;

    fn photo() -> Photo {
        let mut p = Photo::new(PhotoId(7), Source::File { path: "/x/IMG_1.jpg".into() }, "IMG_1.jpg", "JPEG", 40, 30, "2026-01-01T00:00:00");
        p.rating = 4;
        p.flag = Flag::Pick;
        p.label = Some(ColorLabel::Purple);
        p.captured = Some("2025-06-07T08:09:10".into());
        p.meta = Meta {
            title: "Dune & sky".into(),
            caption: "Line <one>\nline two".into(),
            copyright: "© 2026 Me".into(),
            creator: "Ann Lee".into(),
            location: "Namib".into(),
            keywords: vec!["desert".into(), "red".into()],
            gps: Some((-24.75, 15.3)),
            ..Default::default()
        };
        let mut d = DevelopSettings::default();
        d.light.exposure = 0.75;
        d.effects.clarity = 22.0;
        d.crop.geometry.angle = 2.5;
        d.masks.push(lightcraft_develop::Mask { id: 1, ..Default::default() });
        p.develop = Arc::new(d);
        p
    }

    #[test]
    fn naming() {
        assert_eq!(sidecar_path("/a/b/IMG_1.CR3", SidecarNaming::Stem), PathBuf::from("/a/b/IMG_1.xmp"));
        assert_eq!(sidecar_path("/a/b/IMG_1.CR3", SidecarNaming::Full), PathBuf::from("/a/b/IMG_1.CR3.xmp"));
    }

    #[test]
    fn packet_roundtrip_restores_everything() {
        let p = photo();
        let x = sidecar_packet(&p, &lightcraft_catalog::Catalog::new());
        let sc = parse_sidecar(&x, false).unwrap();
        assert_eq!(sc.rating, Some(4));
        assert_eq!(sc.flag, Some(Flag::Pick));
        assert_eq!(sc.label, Some(Some(ColorLabel::Purple)));
        let mut q = Photo::new(PhotoId(7), p.source.clone(), "IMG_1.jpg", "JPEG", 40, 30, "2026-01-01T00:00:00");
        q.captured = p.captured.clone();
        q.meta.gps = p.meta.gps;
        assert!(merge_into(&mut q, &sc, "now"));
        assert_eq!(q.develop, p.develop);
        assert_eq!(q.meta, p.meta);
        assert_eq!((q.rating, q.flag, q.label), (p.rating, p.flag, p.label));
        assert_eq!(q.edited.as_deref(), Some("now"));
    }

    #[test]
    fn foreign_sidecar_rating_reject_label_and_crs() {
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
            xmp:Rating="-1" xmp:Label="Green" crs:Exposure2012="-0.40" crs:Vibrance="+12"/></rdf:RDF></x:xmpmeta>"#;
        let sc = parse_sidecar(x, true).unwrap();
        assert_eq!((sc.rating, sc.flag, sc.label), (Some(0), Some(Flag::Reject), Some(Some(ColorLabel::Green))));
        let mut p = photo();
        let before = p.develop.clone();
        assert!(merge_into(&mut p, &sc, "t"));
        assert_eq!(p.develop.light.exposure, -0.4);
        assert_eq!(p.develop.color.vibrance, 12.0);
        assert_eq!(p.develop.effects.clarity, before.effects.clarity, "crs partial keeps unrelated settings");
        assert_eq!(p.meta.title, "Dune & sky", "fields the sidecar doesn't state are kept");
    }

    #[test]
    fn op_photo_collection() {
        let mut v = Vec::new();
        op_photos(
            &Op::Batch {
                ops: vec![
                    Op::SetRating { id: PhotoId(1), rating: 2 },
                    Op::SetFlag { id: PhotoId(1), flag: Flag::Pick },
                    Op::SetLabel { id: PhotoId(3), label: None },
                ],
            },
            &mut v,
        );
        assert_eq!(v, vec![PhotoId(1), PhotoId(3)]);
    }
}
