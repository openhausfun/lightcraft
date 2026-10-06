//! Library backups for the browser build: a plain zip (entries stored, not compressed — photos
//! don't shrink) of the library files and every stored original, built one entry at a time so
//! only one file is in wasm memory at once, and read back the same way for a restore.
//!
//! Layout of a backup:
//! - `library/catalog.snap`, `library/catalog.log`, `library/presets.json`, … (see
//!   [`crate::files::LIBRARY_FILES`]);
//! - `originals/<content hash>/<file name>`: each imported photo's original bytes, under its own
//!   name so an unzipped backup is browsable;
//! - `README.txt`.
//!
//! Restoring never deletes anything: the library files go to a new library folder in browser
//! storage, originals that aren't stored yet are added (they're named by content, so existing
//! ones are identical), and only then does a small pointer file ([`ACTIVE_LIBRARY`]) switch to
//! the restored library. The library that was there before stays in storage.
//!
//! No zip64: a backup holds at most [`MAX_BYTES`] and 65 535 entries.

/// Storage key of the file naming the library folder in use (absent: `library`).
pub const ACTIVE_LIBRARY: &str = "active-library";

/// The default library folder.
pub const DEFAULT_LIBRARY_DIR: &str = "library";

/// Largest backup (offsets are 32-bit without zip64).
pub const MAX_BYTES: u64 = 0xFFFF_FFFF - (64 << 20);

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const END_SIG: u32 = 0x0605_4b50;
/// General-purpose flag bit 11: names are UTF-8.
const UTF8: u16 = 0x0800;
/// 1980-01-01 00:00 in MS-DOS format (date, time).
const DOS_DATE: u16 = (1 << 5) | 1;

/// The readme inside every backup.
pub const README: &str = "LightCraft library backup (browser version)\n\n\
library/      the catalog (catalog.snap + catalog.log), presets, view state and preferences\n\
originals/    every imported photo, as originals/<content hash>/<file name>\n\n\
Restore it in LightCraft for the web with File > Restore Library from Backup...\n\
The photos in originals/ are the untouched files you imported; edits live in the catalog.\n";

/// Is `name` a safe library folder name (`library` or `library-<letters, digits, ->`)?
pub fn valid_library_dir(name: &str) -> bool {
    name == DEFAULT_LIBRARY_DIR
        || name.strip_prefix("library-").is_some_and(|s| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Is `h` a plausible content hash (used as a storage key segment)?
pub fn valid_hash(h: &str) -> bool {
    !h.is_empty() && h.len() <= 128 && h.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Zip entry name for an original.
pub fn original_entry(hash: &str, file_name: &str) -> String {
    let name: String = file_name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let name = if name.is_empty() || name == "." || name == ".." { "original".to_string() } else { name };
    format!("originals/{hash}/{name}")
}

/// Where a backup entry goes when restoring into library folder `dir`: `Some(storage key)`, or
/// `None` to skip it (README, thumbnails, anything unknown or unsafe).
pub fn restore_key(entry: &str, dir: &str) -> Option<String> {
    if let Some(name) = entry.strip_prefix("library/") {
        return crate::files::LIBRARY_FILES.contains(&name).then(|| format!("{dir}/{name}"));
    }
    let rest = entry.strip_prefix("originals/")?;
    let (hash, _) = rest.split_once('/').unwrap_or((rest, ""));
    valid_hash(hash).then(|| format!("originals/{hash}"))
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

struct Central {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// Writes a stored (uncompressed) zip piece by piece: [`ZipWriter::entry`] returns the local
/// header to emit before the entry's bytes, [`ZipWriter::finish`] the central directory.
#[derive(Default)]
pub struct ZipWriter {
    entries: Vec<Central>,
    offset: u64,
}

impl ZipWriter {
    /// The local header for `name` holding `data` (emit it, then `data`).
    pub fn entry(&mut self, name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        let size = u32::try_from(data.len()).map_err(|_| format!("{name}: larger than 4 GB"))?;
        let name_len = u16::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;
        if self.entries.len() >= usize::from(u16::MAX) {
            return Err("too many files for one backup".into());
        }
        let header_len = 30 + u64::from(name_len);
        if self.offset + header_len + u64::from(size) > MAX_BYTES {
            return Err("the library is too large for one backup file (4 GB)".into());
        }
        let offset = u32::try_from(self.offset).map_err(|_| "backup too large".to_string())?;
        let crc = crc32fast::hash(data);
        let mut h = Vec::with_capacity(header_len as usize);
        u32le(&mut h, LOCAL_SIG);
        u16le(&mut h, 20); // version needed
        u16le(&mut h, UTF8);
        u16le(&mut h, 0); // stored
        u16le(&mut h, 0); // time
        u16le(&mut h, DOS_DATE);
        u32le(&mut h, crc);
        u32le(&mut h, size);
        u32le(&mut h, size);
        u16le(&mut h, name_len);
        u16le(&mut h, 0); // extra
        h.extend_from_slice(name.as_bytes());
        self.offset += header_len + u64::from(size);
        self.entries.push(Central { name: name.to_string(), crc, size, offset });
        Ok(h)
    }

    /// The central directory and end record (emit last).
    pub fn finish(self) -> Result<Vec<u8>, String> {
        let mut cd = Vec::new();
        for e in &self.entries {
            u32le(&mut cd, CENTRAL_SIG);
            u16le(&mut cd, 20); // made by
            u16le(&mut cd, 20); // needed
            u16le(&mut cd, UTF8);
            u16le(&mut cd, 0);
            u16le(&mut cd, 0);
            u16le(&mut cd, DOS_DATE);
            u32le(&mut cd, e.crc);
            u32le(&mut cd, e.size);
            u32le(&mut cd, e.size);
            u16le(&mut cd, e.name.len() as u16);
            u16le(&mut cd, 0); // extra
            u16le(&mut cd, 0); // comment
            u16le(&mut cd, 0); // disk
            u16le(&mut cd, 0); // internal attrs
            u32le(&mut cd, 0); // external attrs
            u32le(&mut cd, e.offset);
            cd.extend_from_slice(e.name.as_bytes());
        }
        let cd_offset = u32::try_from(self.offset).map_err(|_| "backup too large".to_string())?;
        let cd_size = u32::try_from(cd.len()).map_err(|_| "backup too large".to_string())?;
        let n = self.entries.len() as u16;
        let mut out = cd;
        u32le(&mut out, END_SIG);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, n);
        u16le(&mut out, n);
        u32le(&mut out, cd_size);
        u32le(&mut out, cd_offset);
        u16le(&mut out, 0); // comment
        Ok(out)
    }
}

/// One entry of a zip's central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub crc: u32,
    pub size: u64,
    /// Offset of the entry's local header.
    pub header: u64,
    /// 0 = stored (the only method a restore reads).
    pub method: u16,
}

fn rd16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn rd32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// How many bytes from the end of a zip to read to find its end record.
pub fn tail_len(file_len: u64) -> u64 {
    file_len.min(22 + 65_535)
}

/// The central directory's (offset, size) from the last [`tail_len`] bytes of a zip.
pub fn find_central(tail: &[u8]) -> Result<(u64, u64), String> {
    let at = (0..tail.len().saturating_sub(21)).rev().find(|&i| rd32(tail, i) == Some(END_SIG)).ok_or("not a zip file (no end record)")?;
    let size = rd32(tail, at + 12).ok_or("truncated zip")?;
    let offset = rd32(tail, at + 16).ok_or("truncated zip")?;
    if size == u32::MAX || offset == u32::MAX {
        return Err("zip64 backups are not supported".into());
    }
    Ok((u64::from(offset), u64::from(size)))
}

/// Parse a central directory.
pub fn parse_central(cd: &[u8]) -> Result<Vec<ZipEntry>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 46 <= cd.len() {
        if rd32(cd, i) != Some(CENTRAL_SIG) {
            return Err("damaged zip directory".into());
        }
        let bad = || "damaged zip directory".to_string();
        let method = rd16(cd, i + 10).ok_or_else(bad)?;
        let crc = rd32(cd, i + 16).ok_or_else(bad)?;
        let size = rd32(cd, i + 24).ok_or_else(bad)?;
        let n = usize::from(rd16(cd, i + 28).ok_or_else(bad)?);
        let extra = usize::from(rd16(cd, i + 30).ok_or_else(bad)?);
        let comment = usize::from(rd16(cd, i + 32).ok_or_else(bad)?);
        let header = rd32(cd, i + 42).ok_or_else(bad)?;
        let name = cd.get(i + 46..i + 46 + n).ok_or_else(bad)?;
        out.push(ZipEntry { name: String::from_utf8_lossy(name).into_owned(), crc, size: u64::from(size), header: u64::from(header), method });
        i += 46 + n + extra + comment;
    }
    Ok(out)
}

/// Offset of an entry's data from its local header's first 30 bytes.
pub fn data_offset(entry: &ZipEntry, local: &[u8]) -> Result<u64, String> {
    if rd32(local, 0) != Some(LOCAL_SIG) {
        return Err(format!("{}: damaged entry", entry.name));
    }
    let n = rd16(local, 26).ok_or("truncated entry")?;
    let extra = rd16(local, 28).ok_or("truncated entry")?;
    Ok(entry.header + 30 + u64::from(n) + u64::from(extra))
}

/// Check an entry's bytes.
pub fn verify(entry: &ZipEntry, data: &[u8]) -> Result<(), String> {
    if entry.method != 0 {
        return Err(format!("{}: compressed entries are not supported (not a LightCraft backup?)", entry.name));
    }
    if data.len() as u64 != entry.size || crc32fast::hash(data) != entry.crc {
        return Err(format!("{}: damaged (checksum mismatch)", entry.name));
    }
    Ok(())
}

/// A name for the restored library's folder.
pub fn restored_dir_name(now_ms: f64) -> String {
    format!("library-restored-{}", now_ms.max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = ZipWriter::default();
        let mut out = Vec::new();
        for (n, d) in files {
            out.extend(w.entry(n, d).unwrap());
            out.extend_from_slice(d);
        }
        out.extend(w.finish().unwrap());
        out
    }

    #[test]
    fn written_backups_read_back() {
        let files: [(&str, &[u8]); 3] =
            [("README.txt", README.as_bytes()), ("library/catalog.log", b"{\"seq\":1}\n"), ("originals/abc123/IMG 1.jpg", &[0, 1, 2, 3, 255])];
        let zip = build(&files);
        let tail = &zip[zip.len() - tail_len(zip.len() as u64) as usize..];
        let (off, size) = find_central(tail).unwrap();
        let entries = parse_central(&zip[off as usize..(off + size) as usize]).unwrap();
        assert_eq!(entries.len(), 3);
        for (e, (name, data)) in entries.iter().zip(files) {
            assert_eq!(e.name, name);
            let start = data_offset(e, &zip[e.header as usize..e.header as usize + 30]).unwrap() as usize;
            let got = &zip[start..start + e.size as usize];
            assert_eq!(got, data);
            verify(e, got).unwrap();
        }
        // damage is caught
        let e = &entries[2];
        assert!(verify(e, &[0, 1, 2, 3, 254]).is_err());
        assert!(find_central(b"not a zip at all, just some text").is_err());
    }

    #[test]
    fn restore_keys_are_safe() {
        assert_eq!(restore_key("library/catalog.snap", "library-restored-1").as_deref(), Some("library-restored-1/catalog.snap"));
        assert_eq!(restore_key("library/../../x", "library-r"), None);
        assert_eq!(restore_key("library/evil.bin", "library-r"), None);
        assert_eq!(restore_key("originals/abc123/IMG.jpg", "d").as_deref(), Some("originals/abc123"));
        assert_eq!(restore_key("originals/../library/catalog.log", "d"), None);
        assert_eq!(restore_key("README.txt", "d"), None);
        assert_eq!(restore_key("thumbs/x.jpg", "d"), None);
        assert_eq!(original_entry("abc", "a/b\\c.jpg"), "originals/abc/a_b_c.jpg");
        assert_eq!(original_entry("abc", ".."), "originals/abc/original");
        assert!(valid_library_dir("library") && valid_library_dir("library-restored-17"));
        assert!(!valid_library_dir("library-../x") && !valid_library_dir("originals") && !valid_library_dir("library-"));
    }

    #[test]
    fn size_limits_are_errors_not_panics() {
        let mut w = ZipWriter { offset: MAX_BYTES - 10, ..Default::default() };
        assert!(w.entry("x", &[0; 64]).is_err());
    }
}
