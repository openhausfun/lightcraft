//! Importing other editors' presets: XMP preset files, classic `.lrtemplate` files (a Lua table
//! literal), photos that carry their edits as an XMP packet ("DNG presets" from mobile apps,
//! edited JPEG/TIFF), and zip bundles of any of these. Everything ends up as `crs:` properties
//! mapped by [`crate::crs`]. Luminar looks (`.lmp`, `.mplumpack` collections) are read by
//! [`crate::preset_luminar`].
//!
//! Written from the formats' public structure (Lua's table syntax, the ZIP application note,
//! ISO 16684-1 XMP) and black-box observation; no third-party code or preset content was used.

use std::path::Path;

use lightcraft_develop::Preset;
use serde_json::{Map, Value};

use crate::crs::Props;

/// One preset read from a file, with the settings we could not carry over.
#[derive(Clone, Debug)]
pub struct Imported {
    pub preset: Preset,
    /// Names of the other editor's settings that have no counterpart here (local masks, its own
    /// profiles…).
    pub unmapped: Vec<String>,
}

/// Extensions of files read as presets (beyond our own `.lcpreset`).
pub const PRESET_EXTS: &[&str] = &["xmp", "lrtemplate", "zip", "dng", "lmp", "mplumpack"];

// ------------------------------------------------------------------------------------- zip

/// The files of a zip archive: (path inside the archive, bytes). Stored and deflated entries;
/// directories, macOS resource forks and hidden files are skipped.
pub fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let u16_at = |o: usize| bytes.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize);
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    // end of central directory: the last "PK\x05\x06" within the trailing 64 KiB + 22 bytes
    let tail = bytes.len().saturating_sub(65_557);
    let eocd = (tail..bytes.len().saturating_sub(21)).rev().find(|&i| bytes[i..].starts_with(b"PK\x05\x06")).ok_or("not a zip file")?;
    let count = u16_at(eocd + 10).ok_or("bad zip")?;
    let mut at = u32_at(eocd + 16).ok_or("bad zip")?;
    let mut out = Vec::new();
    for _ in 0..count {
        if !bytes.get(at..).is_some_and(|b| b.starts_with(b"PK\x01\x02")) {
            return Err("bad zip central directory".into());
        }
        let method = u16_at(at + 10).ok_or("bad zip")?;
        let csize = u32_at(at + 20).ok_or("bad zip")?;
        let usize_ = u32_at(at + 24).ok_or("bad zip")?;
        let (nlen, xlen, clen) = (u16_at(at + 28).ok_or("bad zip")?, u16_at(at + 30).ok_or("bad zip")?, u16_at(at + 32).ok_or("bad zip")?);
        let local = u32_at(at + 42).ok_or("bad zip")?;
        let name = String::from_utf8_lossy(bytes.get(at + 46..at + 46 + nlen).ok_or("bad zip")?).replace('\\', "/");
        at += 46 + nlen + xlen + clen;
        let skip = name.ends_with('/') || name.split('/').any(|p| p.starts_with('.') || p == "__MACOSX");
        if skip {
            continue;
        }
        if !bytes.get(local..).is_some_and(|b| b.starts_with(b"PK\x03\x04")) {
            return Err(format!("{name}: bad local header"));
        }
        let data_at = local + 30 + u16_at(local + 26).ok_or("bad zip")? + u16_at(local + 28).ok_or("bad zip")?;
        let data = bytes.get(data_at..data_at + csize).ok_or_else(|| format!("{name}: truncated"))?;
        let content = match method {
            0 => data.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, usize_.max(1) * 2 + 1024).map_err(|e| format!("{name}: {e:?}"))?,
            m => return Err(format!("{name}: unsupported compression method {m}")),
        };
        out.push((name, content));
    }
    Ok(out)
}

// ------------------------------------------------------------------------------- Lua tables

/// A value of a Lua table literal (what `.lrtemplate` files contain).
#[derive(Clone, Debug, PartialEq)]
pub enum Lua {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// Positional items and named fields, in file order.
    Table(Vec<Lua>, Vec<(String, Lua)>),
}

impl Lua {
    pub fn get(&self, key: &str) -> Option<&Lua> {
        match self {
            Lua::Table(_, m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Lua::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct LuaParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl LuaParser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("{what} at byte {}", self.i))
    }
    fn skip_ws(&mut self) {
        loop {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"--") {
                self.i += 2;
                if let Some(level) = self.long_bracket_level() {
                    let _ = self.long_string(level);
                } else {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                continue;
            }
            break;
        }
    }
    /// `[[` / `[==[` at the cursor: its level (number of `=`).
    fn long_bracket_level(&self) -> Option<usize> {
        let r = &self.s[self.i..];
        if r.first() != Some(&b'[') {
            return None;
        }
        let eq = r[1..].iter().take_while(|c| **c == b'=').count();
        (r.get(1 + eq) == Some(&b'[')).then_some(eq)
    }
    fn long_string(&mut self, level: usize) -> Result<String, String> {
        self.i += level + 2;
        let close = format!("]{}]", "=".repeat(level));
        let rest = &self.s[self.i..];
        let end = rest.windows(close.len()).position(|w| w == close.as_bytes()).ok_or("unterminated long string")?;
        let mut text = String::from_utf8_lossy(&rest[..end]).to_string();
        if text.starts_with('\n') {
            text.remove(0);
        }
        self.i += end + close.len();
        Ok(text)
    }
    fn string(&mut self) -> Result<String, String> {
        let q = self.s[self.i];
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                c if c == q => return Ok(String::from_utf8_lossy(&out).to_string()),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'0'..=b'9' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.s.get(self.i) {
                                    Some(d @ b'0'..=b'9') => {
                                        v = v * 10 + (d - b'0') as u32;
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v.min(255) as u8);
                        }
                        b'\n' => out.push(b'\n'),
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
        self.err("unterminated string")
    }
    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_') {
            self.i += 1;
        }
        (self.i > start && !self.s[start].is_ascii_digit()).then(|| String::from_utf8_lossy(&self.s[start..self.i]).to_string())
    }
    fn number(&mut self) -> Result<f64, String> {
        let start = self.i;
        if self.s[self.i..].starts_with(b"0x") || self.s[self.i..].starts_with(b"0X") {
            self.i += 2;
            let h = self.i;
            while self.i < self.s.len() && self.s[self.i].is_ascii_hexdigit() {
                self.i += 1;
            }
            return i64::from_str_radix(std::str::from_utf8(&self.s[h..self.i]).unwrap_or(""), 16).map(|v| v as f64).map_err(|e| e.to_string());
        }
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || matches!(self.s[self.i], b'.' | b'e' | b'E' | b'+' | b'-')) {
            if matches!(self.s[self.i], b'+' | b'-') && self.i > start && !matches!(self.s[self.i - 1], b'e' | b'E') {
                break;
            }
            self.i += 1;
        }
        std::str::from_utf8(&self.s[start..self.i]).ok().and_then(|t| t.parse().ok()).map_or_else(|| self.err("bad number"), Ok)
    }
    fn value(&mut self) -> Result<Lua, String> {
        self.skip_ws();
        let Some(&c) = self.s.get(self.i) else { return self.err("unexpected end") };
        match c {
            b'{' => self.table(),
            b'"' | b'\'' => self.string().map(Lua::Str),
            b'[' if self.long_bracket_level().is_some() => {
                let level = self.long_bracket_level().unwrap_or(0);
                self.long_string(level).map(Lua::Str)
            }
            b'-' => {
                self.i += 1;
                self.skip_ws();
                match self.value()? {
                    Lua::Num(n) => Ok(Lua::Num(-n)),
                    _ => self.err("bad negation"),
                }
            }
            b'0'..=b'9' | b'.' => self.number().map(Lua::Num),
            _ => match self.ident().as_deref() {
                Some("true") => Ok(Lua::Bool(true)),
                Some("false") => Ok(Lua::Bool(false)),
                Some("nil") => Ok(Lua::Nil),
                // a call with one string / table argument, e.g. ZSTR "…" (localisation helpers)
                Some(_) => {
                    self.skip_ws();
                    match self.s.get(self.i) {
                        Some(b'"' | b'\'' | b'{') => self.value(),
                        Some(b'(') => {
                            self.i += 1;
                            let v = self.value()?;
                            self.skip_ws();
                            if self.s.get(self.i) == Some(&b')') {
                                self.i += 1;
                            }
                            Ok(v)
                        }
                        _ => Ok(Lua::Nil),
                    }
                }
                None => self.err("unexpected character"),
            },
        }
    }
    fn table(&mut self) -> Result<Lua, String> {
        self.i += 1; // {
        let (mut arr, mut map) = (Vec::new(), Vec::new());
        loop {
            self.skip_ws();
            match self.s.get(self.i) {
                None => return self.err("unterminated table"),
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Lua::Table(arr, map));
                }
                Some(b',' | b';') => {
                    self.i += 1;
                    continue;
                }
                Some(b'[') if self.long_bracket_level().is_none() => {
                    self.i += 1;
                    let k = self.value()?;
                    self.skip_ws();
                    if self.s.get(self.i) != Some(&b']') {
                        return self.err("expected ]");
                    }
                    self.i += 1;
                    self.expect_eq()?;
                    let v = self.value()?;
                    match k {
                        Lua::Str(s) => map.push((s, v)),
                        Lua::Num(n) => map.push((format!("{n}"), v)),
                        _ => {}
                    }
                }
                _ => {
                    // `name = value`, or a positional value
                    let save = self.i;
                    if let Some(name) = self.ident() {
                        self.skip_ws();
                        if self.s.get(self.i) == Some(&b'=') && self.s.get(self.i + 1) != Some(&b'=') {
                            self.i += 1;
                            let v = self.value()?;
                            map.push((name, v));
                            continue;
                        }
                    }
                    self.i = save;
                    arr.push(self.value()?);
                }
            }
        }
    }
    fn expect_eq(&mut self) -> Result<(), String> {
        self.skip_ws();
        if self.s.get(self.i) == Some(&b'=') {
            self.i += 1;
            Ok(())
        } else {
            self.err("expected =")
        }
    }
}

/// Parse a Lua table literal, optionally preceded by `name =` or `return`.
pub fn parse_lua(text: &str) -> Result<Lua, String> {
    let mut p = LuaParser { s: text.as_bytes(), i: 0 };
    p.skip_ws();
    let save = p.i;
    match p.ident().as_deref() {
        Some("return") => {}
        Some(_) => {
            p.skip_ws();
            if p.s.get(p.i) == Some(&b'=') {
                p.i += 1;
            } else {
                p.i = save;
            }
        }
        None => {}
    }
    p.value()
}

/// A localisable string `"$$$/Key/Path=Default text"` → its default text.
fn delocalize(s: &str) -> String {
    match s.strip_prefix("$$$/") {
        Some(rest) => rest.split_once('=').map_or(rest, |(_, t)| t).to_string(),
        None => s.to_string(),
    }
}

/// The develop settings of an `.lrtemplate` as `crs:` properties (flat, and structured for local
/// corrections), plus its title.
pub fn lrtemplate_props(text: &str) -> Result<(Option<String>, Props, crate::crs_masks::Values), String> {
    let root = parse_lua(text)?;
    let title = root.get("title").or_else(|| root.get("internalName")).and_then(Lua::str).map(delocalize).filter(|t| !t.trim().is_empty());
    let settings = root.get("value").and_then(|v| v.get("settings")).ok_or("no develop settings in this template")?;
    let Lua::Table(_, fields) = settings else { return Err("no develop settings in this template".into()) };
    let mut props = Props::new();
    let mut values = crate::crs_masks::Values::new();
    for (k, v) in fields {
        let key = format!("crs:{k}");
        if crate::crs_masks::CONTAINERS.contains(&key.as_str()) {
            values.insert(key.clone(), crate::crs_masks::from_lua(v));
        }
        let vals: Vec<String> = match v {
            Lua::Num(n) => vec![format!("{n}")],
            Lua::Bool(b) => vec![if *b { "True" } else { "False" }.to_string()],
            Lua::Str(s) => vec![delocalize(s)],
            // point curves are flat lists x1, y1, x2, y2, …
            Lua::Table(arr, map) if map.is_empty() && !arr.is_empty() && arr.iter().all(|a| matches!(a, Lua::Num(_))) => arr
                .chunks(2)
                .filter_map(|c| match c {
                    [Lua::Num(x), Lua::Num(y)] => Some(format!("{x}, {y}")),
                    _ => None,
                })
                .collect(),
            Lua::Table(a, m) if a.is_empty() && m.is_empty() => continue,
            // anything else (local corrections, retouch…) is only reported
            Lua::Table(..) => vec!["(table)".into()],
            Lua::Nil => continue,
        };
        props.insert(key, vals);
    }
    Ok((title, props, values))
}

// ------------------------------------------------------------------------------- XMP in files

/// The XMP packet embedded in a file of any container (DNG, JPEG, TIFF…): the text from
/// `<x:xmpmeta` to `</x:xmpmeta>`.
pub fn embedded_xmp(bytes: &[u8]) -> Option<String> {
    let start = bytes.windows(10).position(|w| w == b"<x:xmpmeta")?;
    let end = bytes[start..].windows(12).position(|w| w == b"</x:xmpmeta>")? + start + 12;
    Some(String::from_utf8_lossy(&bytes[start..end]).to_string())
}

// ----------------------------------------------------------------------------------- files

/// A preset group named after the innermost folder of `dir` (a path inside the imported folder or
/// bundle) that isn't a container named for an app or a format (`Lightroom Classic`, `XMP`,
/// `Mobile (DNG)`, `Develop Presets`…).
pub fn group_from_dir(dir: &str) -> Option<String> {
    const FILLER: &[&str] = &[
        "lightroom",
        "lr",
        "lrc",
        "classic",
        "cc",
        "camera",
        "raw",
        "acr",
        "photoshop",
        "desktop",
        "mobile",
        "xmp",
        "lrtemplate",
        "lrtemplates",
        "dng",
        "dngs",
        "preset",
        "presets",
        "settings",
        "lcpreset",
        "lightcraft",
        "user",
        "develop",
        "files",
        "for",
        "and",
        "version",
        "versions",
        "format",
        "formats",
        "new",
        "old",
        "adobe",
        "luminar",
        "skylum",
        "looks",
        "lmp",
        "mplumpack",
        "contents",
        "resources",
    ];
    let generic = |name: &str| {
        let lower = name.to_ascii_lowercase();
        lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).all(|w| FILLER.contains(&w) || w.chars().all(|c| c.is_ascii_digit()))
    };
    // a macOS bundle (`Look.lmp/Contents/…`) is a file, not a group
    let bundle = |p: &str| p.to_ascii_lowercase().ends_with(".lmp");
    dir.split(['/', '\\']).rev().find(|p| !p.is_empty() && !generic(p) && !bundle(p)).map(str::to_string)
}

pub(crate) fn build(props: &Props, values: &crate::crs_masks::Values, name: String, group: Option<String>, from_photo: bool) -> Option<Imported> {
    let (mut settings, unmapped) = crate::crs::to_partial_report(props, Some(values), None, crate::crs_masks::DEFAULT_ASPECT);
    if from_photo && let Some(o) = settings.as_object_mut() {
        // a photo's own framing and absolute white balance don't belong in a look
        o.remove("crop");
        o.remove("geometry");
        if o.get("wb").and_then(|w| w.get("mode")).and_then(Value::as_str).is_none_or(|m| m == "custom" || m == "asShot") {
            o.remove("wb");
        }
    }
    if settings.as_object().is_none_or(Map::is_empty) {
        return None;
    }
    let group = props
        .get("crs:Group")
        .and_then(|v| v.first())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(group)
        .unwrap_or_else(|| "Imported Presets".into());
    let uuid = props.get("crs:UUID").and_then(|v| v.first()).map(|u| u.to_ascii_lowercase());
    let id = format!("user.xmp.{}", uuid.unwrap_or_else(|| crate::presets::slug(&format!("{group}-{name}"))));
    Some(Imported { preset: Preset { id, name, group, settings, favorite: false, builtin: false }, unmapped })
}

/// Read the presets in file `name` (`bytes`); `group` names the group when the file doesn't
/// (its folder). Zip bundles are read recursively.
pub fn read_presets(name: &str, bytes: &[u8], group: Option<String>) -> Result<Vec<Imported>, String> {
    let path = Path::new(name);
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Preset".into());
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "zip" => {
            let mut out = Vec::new();
            let mut errors = Vec::new();
            for (inner, data) in read_zip(bytes)? {
                let iext = Path::new(&inner).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
                if !(PRESET_EXTS.contains(&iext.as_str()) || iext == crate::presets::LCPRESET_EXT) || iext == "zip" {
                    continue;
                }
                let dir_group =
                    inner.rsplit_once('/').and_then(|(d, _)| group_from_dir(d)).or_else(|| group_from_dir(&stem)).or_else(|| group.clone());
                match read_presets(&inner, &data, dir_group) {
                    Ok(v) => out.extend(v),
                    Err(e) => errors.push(format!("{inner}: {e}")),
                }
            }
            if out.is_empty() {
                return Err(if errors.is_empty() { "no presets in this archive".into() } else { errors.join("; ") });
            }
            Ok(out)
        }
        "lmp" => crate::preset_luminar::read_lmp(name, bytes, group).map(|i| vec![i]),
        "mplumpack" => crate::preset_luminar::read_mplumpack(name, bytes),
        "lrtemplate" => {
            let text = String::from_utf8_lossy(bytes);
            let (title, props, values) = lrtemplate_props(text.trim_start_matches('\u{feff}'))?;
            build(&props, &values, title.unwrap_or(stem), group, false)
                .map(|i| vec![i])
                .ok_or_else(|| "no develop settings we can use in this template".into())
        }
        "xmp" => {
            let text = String::from_utf8_lossy(bytes);
            let d = lightcraft_meta::parse_xmp(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
            let props = &d.properties;
            let name = props.get("crs:Name").and_then(|v| v.first()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or(stem);
            build(props, &d.values, name, group, false).map(|i| vec![i]).ok_or_else(|| "no develop settings in this XMP file".into())
        }
        // a photo carrying its edits (mobile "DNG presets", edited JPEG / TIFF)
        "dng" | "jpg" | "jpeg" | "tif" | "tiff" => {
            let xmp = embedded_xmp(bytes).ok_or("this photo carries no edits (no XMP)")?;
            let d = lightcraft_meta::parse_xmp(&xmp).map_err(|e| e.to_string())?;
            build(&d.properties, &d.values, stem, group, true).map(|i| vec![i]).ok_or_else(|| "this photo carries no edits we can use".into())
        }
        _ => crate::presets::parse_preset_file(name, bytes).map(|v| {
            v.into_iter()
                .map(|mut p| {
                    if p.group == "Imported Presets"
                        && let Some(g) = &group
                    {
                        p.group = g.clone();
                    }
                    Imported { preset: p, unmapped: Vec::new() }
                })
                .collect()
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// A zip archive (`deflate`: compress entries) — enough of a writer for the tests.
    pub(crate) fn zip(entries: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let (method, body) = if deflate { (8u16, miniz_oxide::deflate::compress_to_vec(data, 6)) } else { (0, data.to_vec()) };
            let off = out.len() as u32;
            let head = |sig: &[u8], out: &mut Vec<u8>, central: bool| {
                out.extend_from_slice(sig);
                if central {
                    out.extend_from_slice(&20u16.to_le_bytes());
                }
                out.extend_from_slice(&20u16.to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&method.to_le_bytes());
                out.extend_from_slice(&[0; 8]); // time, date, crc (not checked)
                out.extend_from_slice(&(body.len() as u32).to_le_bytes());
                out.extend_from_slice(&(data.len() as u32).to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(&0u16.to_le_bytes());
            };
            head(b"PK\x03\x04", &mut out, false);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);
            head(b"PK\x01\x02", &mut central, true);
            central.extend_from_slice(&[0; 10]); // comment length, disk, internal + external attributes
            central.extend_from_slice(&off.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    /// A template in the classic Lua-table form (written for this test).
    pub(crate) const TEMPLATE: &str = r#"s = {
	id = "0D2F9C1E-TEST",
	internalName = "Warm Fade",
	title = "$$$/Test/Warm=Warm Fade",
	type = "Develop",
	value = {
		settings = {
			Exposure2012 = 0.35,
			Contrast2012 = -20,
			ConvertToGrayscale = false,
			ToneCurvePV2012 = { 0, 20, 128, 128, 255, 240, },
			SplitToningShadowHue = 210,
			SplitToningShadowSaturation = 15,
			HueAdjustmentOrange = -8,
			CameraProfile = "Some Profile",
			RetouchInfo = {},
			ProcessVersion = "11.0",
			-- a comment
			EnableColorAdjustments = true,
		},
		uuid = "0D2F9C1E-TEST",
	},
	version = 0,
}
"#;

    #[test]
    fn lua_tables() {
        let v = parse_lua(
            r#"return { a = 1, ["b c"] = 'x\'y', -2.5e1, 0x10, t = { true, nil }, [[long
text]], z = ZSTR "loc" }"#,
        )
        .unwrap();
        assert_eq!(v.get("a"), Some(&Lua::Num(1.0)));
        assert_eq!(v.get("b c"), Some(&Lua::Str("x'y".into())));
        assert_eq!(v.get("z"), Some(&Lua::Str("loc".into())));
        let Lua::Table(arr, _) = &v else { panic!() };
        assert_eq!(arr[..2], [Lua::Num(-25.0), Lua::Num(16.0)]);
        assert_eq!(arr[2], Lua::Str("long\ntext".into()));
        assert!(parse_lua("s = { a = ").is_err());
    }

    #[test]
    fn lrtemplate_maps_settings_and_reports_the_rest() {
        let v = read_presets("Warm Fade.lrtemplate", TEMPLATE.as_bytes(), None).unwrap();
        assert_eq!(v.len(), 1);
        let p = &v[0].preset;
        assert_eq!((p.name.as_str(), p.group.as_str()), ("Warm Fade", "Imported Presets"));
        assert_eq!(p.settings["light"]["exposure"], json!(0.35));
        assert_eq!(p.settings["light"]["contrast"], json!(-20.0));
        assert_eq!(p.settings["treatment"], json!("color"));
        assert_eq!(p.settings["grading"]["shadows"]["hue"], json!(210.0));
        assert_eq!(p.settings["mixer"]["orange"]["hue"], json!(-8.0));
        let c = p.settings["curve"]["master"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        assert!((c[0]["y"].as_f64().unwrap() - 20.0 / 255.0).abs() < 1e-9);
        assert_eq!(v[0].unmapped, vec!["CameraProfile".to_string()], "profiles are ours; the rest is mapped or bookkeeping");
        // it parses into real develop settings
        let d = lightcraft_develop::DevelopSettings::default().merged(&p.settings).expect("valid develop settings");
        assert_eq!(d.light.exposure, 0.35);
    }

    #[test]
    fn old_process_version_fields_are_approximated() {
        let t = "s = { title = \"Old\", value = { settings = { Exposure = 0.5, Contrast = 50, FillLight = 20, HighlightRecovery = 30, Clarity = 10, ToneCurve = { 0, 0, 64, 50, 255, 255 } } } }";
        let p = &read_presets("Old.lrtemplate", t.as_bytes(), None).unwrap()[0].preset;
        assert_eq!(p.settings["light"]["exposure"], json!(0.5));
        assert_eq!(p.settings["light"]["contrast"], json!(25.0), "relative to the old default of 25");
        assert_eq!(p.settings["light"]["shadows"], json!(20.0));
        assert_eq!(p.settings["light"]["highlights"], json!(-30.0));
        assert_eq!(p.settings["effects"]["clarity"], json!(10.0));
        assert_eq!(p.settings["curve"]["master"].as_array().unwrap().len(), 3);
        // newer presets carry the old fields at their defaults: those must not shift anything
        let t2 = "s = { value = { settings = { Contrast2012 = 10, Contrast = 25, Shadows = 5, Brightness = 50 } } }";
        let p2 = &read_presets("New.lrtemplate", t2.as_bytes(), None).unwrap()[0];
        assert_eq!(p2.preset.settings, json!({"light": {"contrast": 10.0}}));
        assert!(p2.unmapped.is_empty(), "{:?}", p2.unmapped);
    }

    const XMP: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
 crs:Exposure2012="+0.50" crs:Vibrance="20" crs:WhiteBalance="As Shot" crs:Temperature="5600" crs:HasCrop="True" crs:CropLeft="0.1" crs:CropRight="0.9" crs:CropTop="0" crs:CropBottom="1"/>
</rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn photos_carrying_edits_become_looks() {
        // a "DNG preset": any container with an XMP packet; framing and white balance stay behind
        let mut file = b"II*\0 binary junk ".to_vec();
        file.extend_from_slice(XMP.as_bytes());
        file.extend_from_slice(b" more junk");
        let v = read_presets("/x/Moody.dng", &file, None).unwrap();
        let p = &v[0].preset;
        assert_eq!(p.name, "Moody");
        assert_eq!(p.settings, json!({"light": {"exposure": 0.5}, "color": {"vibrance": 20.0}}));
        assert!(read_presets("plain.jpg", b"\xff\xd8 no xmp", None).is_err());
    }

    #[test]
    fn zip_bundles_with_folders_as_groups() {
        for deflate in [false, true] {
            let z = zip(
                &[
                    ("Pack/Film/Warm Fade.lrtemplate", TEMPLATE.as_bytes()),
                    ("Pack/Film/Bright.xmp", XMP.as_bytes()),
                    ("__MACOSX/Pack/._Bright.xmp", b"junk"),
                    ("Pack/readme.txt", b"thanks for downloading"),
                ],
                deflate,
            );
            let v = read_presets("pack.zip", &z, None).unwrap();
            let names: Vec<_> = v.iter().map(|i| (i.preset.name.as_str(), i.preset.group.as_str())).collect();
            assert_eq!(names, [("Warm Fade", "Film"), ("Bright", "Film")], "deflate={deflate}");
        }
        assert!(read_presets("empty.zip", &zip(&[("a.txt", b"x")], true), None).is_err());
        assert!(read_presets("bad.zip", b"not a zip", None).is_err());
    }

    #[test]
    fn generic_folder_names_are_not_groups() {
        assert_eq!(group_from_dir("Settings"), None);
        assert_eq!(group_from_dir("Develop Presets/"), None);
        assert_eq!(group_from_dir("Looks/My Film Looks"), Some("My Film Looks".into()));
        // commercial packs nest by format: the pack's name wins
        assert_eq!(group_from_dir("Film Pack/Lightroom Classic"), Some("Film Pack".into()));
        assert_eq!(group_from_dir("Film Pack/Mobile (DNG)/"), Some("Film Pack".into()));
        assert_eq!(group_from_dir("Film Pack/XMP Presets for Lightroom CC 7.3+"), Some("Film Pack".into()));
        assert_eq!(group_from_dir("Sacred Light"), Some("Sacred Light".into()), "whole words only");
    }

    #[test]
    fn import_command_reads_folders_bundles_and_applies() {
        let dir = std::env::temp_dir().join(format!("lc-preset-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Looks/Film")).unwrap();
        std::fs::write(dir.join("Looks/Film/Warm Fade.lrtemplate"), TEMPLATE).unwrap();
        std::fs::write(dir.join("Looks/bundle.zip"), zip(&[("Street/Bright.xmp", XMP.as_bytes())], true)).unwrap();
        std::fs::write(dir.join("Looks/notes.txt"), "not a preset").unwrap();
        let mut s = crate::Session::with_demo();
        let before = s.presets.len();
        let paths = json!([dir.join("Looks").to_string_lossy()]);
        // a dry run reports without adding
        let r = s.execute("preset.import", &json!({"paths": paths, "dryRun": true})).unwrap();
        assert_eq!(r["imported"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(s.presets.len(), before);
        let r = s.execute("preset.import", &json!({"paths": paths})).unwrap();
        let got: Vec<(String, String)> =
            r["imported"].as_array().unwrap().iter().map(|i| (i["name"].as_str().unwrap().into(), i["group"].as_str().unwrap().into())).collect();
        assert_eq!(got, [("Warm Fade".to_string(), "Film".to_string()), ("Bright".into(), "Street".into())]);
        assert_eq!(r["imported"][0]["unmapped"], json!(["CameraProfile"]));
        assert_eq!(s.presets.len(), before + 2);
        // importing again adds nothing
        let r = s.execute("preset.import", &json!({"paths": paths})).unwrap();
        assert_eq!((r["imported"].as_array().unwrap().len(), r["skipped"].as_u64()), (0, Some(2)));
        // and the imported look applies
        let id = s.catalog.photos().next().unwrap().id;
        s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
        let pid = s.presets.iter().find(|p| p.name == "Warm Fade").unwrap().id.clone();
        s.execute("preset.apply", &json!({"id": pid})).unwrap();
        let d = s.develop_of(id).unwrap();
        assert_eq!((d.light.exposure, d.light.contrast), (0.35, -20.0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
