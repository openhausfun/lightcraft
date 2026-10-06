//! Format detection from leading bytes.

use crate::exif::Tiff;
use serde::{Deserialize, Serialize};

/// Image container formats LightCraft recognises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Format {
    Jpeg,
    Png,
    /// Ordinary (non-camera-raw) TIFF, incl. BigTIFF.
    Tiff,
    WebP,
    Avif,
    /// HEIC / HEIF (ISO-BMFF with HEVC).
    Heif,
    /// JPEG XL (bare codestream or ISO-BMFF container).
    Jxl,
    Gif,
    Bmp,
    /// Photoshop PSD / PSB (merged composite).
    Psd,
    /// TIFF-structured camera raw (DNG, CR2, NEF, ARW, PEF, ORF, RW2, SRW, 3FR, IIQ, ERF, …):
    /// route to `lightcraft-raw`.
    RawTiffLike,
    /// Non-TIFF camera raw containers (CR3, RAF, CRW, MRW, X3F): route to `lightcraft-raw`.
    RawOther,
}

impl Format {
    /// Canonical lowercase file extension.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff => "tif",
            Format::WebP => "webp",
            Format::Avif => "avif",
            Format::Heif => "heic",
            Format::Jxl => "jxl",
            Format::Gif => "gif",
            Format::Bmp => "bmp",
            Format::Psd => "psd",
            Format::RawTiffLike => "dng",
            Format::RawOther => "raw",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Png => "image/png",
            Format::Tiff => "image/tiff",
            Format::WebP => "image/webp",
            Format::Avif => "image/avif",
            Format::Heif => "image/heic",
            Format::Jxl => "image/jxl",
            Format::Gif => "image/gif",
            Format::Bmp => "image/bmp",
            Format::Psd => "image/vnd.adobe.photoshop",
            Format::RawTiffLike | Format::RawOther => "image/x-raw",
        }
    }

    /// Whether [`crate::decode`] can decode this format in this build.
    pub fn can_decode(self) -> bool {
        match self {
            Format::Jxl => cfg!(feature = "jxl"),
            Format::Avif | Format::Heif | Format::RawTiffLike | Format::RawOther => false,
            _ => true,
        }
    }

    /// Camera raw formats (decode with `lightcraft-raw`).
    pub fn is_raw(self) -> bool {
        matches!(self, Format::RawTiffLike | Format::RawOther)
    }
}

/// Detect the format from the first bytes of a file (a few KiB suffices; TIFF raw detection walks IFDs
/// when more of the file is available).
pub fn sniff(b: &[u8]) -> Option<Format> {
    if b.len() < 4 {
        return None;
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some(Format::WebP);
    }
    if b.starts_with(&[0xFF, 0x0A]) || b.starts_with(&[0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A]) {
        return Some(Format::Jxl);
    }
    if b.starts_with(b"8BPS") && b.len() >= 6 && matches!(u16::from_be_bytes([b[4], b[5]]), 1 | 2) {
        return Some(Format::Psd);
    }
    if b.starts_with(b"FUJIFILMCCD-RAW") || b.starts_with(b"\0MRM") || b.starts_with(b"FOVb") {
        return Some(Format::RawOther);
    }
    if b.len() >= 14 && &b[0..2] == b"II" && &b[6..14] == b"HEAPCCDR" {
        return Some(Format::RawOther);
    }
    if b.len() >= 12 && &b[4..8] == b"ftyp" {
        return sniff_bmff(b);
    }
    if b.starts_with(b"IIRO") || b.starts_with(b"IIRS") || b.starts_with(b"MMOR") || b.starts_with(b"IIU\0") {
        // Olympus ORF / Panasonic RW2: TIFF-structured with a private magic.
        return Some(Format::RawTiffLike);
    }
    if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        return Some(if tiff_is_raw(b) { Format::RawTiffLike } else { Format::Tiff });
    }
    if b.starts_with(b"II+\0") || b.starts_with(b"MM\0+") {
        return Some(Format::Tiff);
    }
    if b.len() >= 26 && b.starts_with(b"BM") {
        let dib = u32::from_le_bytes([b[14], b[15], b[16], b[17]]);
        if matches!(dib, 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124) {
            return Some(Format::Bmp);
        }
    }
    None
}

fn sniff_bmff(b: &[u8]) -> Option<Format> {
    let size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let end = size.clamp(12, b.len().min(4096));
    let mut brands: Vec<&[u8]> = vec![&b[8..12]];
    let mut p = 16;
    while p + 4 <= end {
        brands.push(&b[p..p + 4]);
        p += 4;
    }
    let has = |x: &[u8]| brands.contains(&x);
    if has(b"crx ") {
        return Some(Format::RawOther);
    }
    if has(b"jxl ") {
        return Some(Format::Jxl);
    }
    if has(b"avif") || has(b"avis") {
        return Some(Format::Avif);
    }
    if [&b"heic"[..], b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1"].iter().any(|x| has(x)) {
        return Some(Format::Heif);
    }
    None
}

/// Heuristic camera-raw detection for classic TIFF: DNGVersion, the CR2 signature, CFA/LinearRaw
/// photometric or raw-only compressions in IFD0 or its SubIFDs, or a camera Make with 10–16-bit data
/// and no ordinary photometric.
fn tiff_is_raw(b: &[u8]) -> bool {
    if b.len() >= 10 && &b[8..10] == b"CR" {
        return true;
    }
    let Some(t) = Tiff::new(b) else { return false };
    let Some(ifd0) = t.first_ifd() else { return false };
    let mut stack = vec![ifd0];
    let mut seen = 0;
    while let Some(pos) = stack.pop() {
        seen += 1;
        if seen > 16 {
            break;
        }
        let Some((entries, _)) = t.ifd(pos) else { continue };
        for e in &entries {
            match e.tag {
                0xC612 => return true, // DNGVersion
                0x0106 if matches!(t.uint(e), Some(32803 | 34892)) => return true,
                0x0103 if matches!(t.uint(e), Some(32767 | 32769 | 32770 | 34316 | 34713 | 65000 | 65535)) => return true,
                0x014A => {
                    // SubIFDs (LONG or IFD type): walk them.
                    let n = e.count.min(8) as usize;
                    if n == 1 {
                        if let Some(p) = t.uint(e) {
                            stack.push(p as usize);
                        }
                    } else if let Some(off) = t.u32_at(e.value_pos) {
                        for i in 0..n {
                            if let Some(p) = t.u32_at(off as usize + 4 * i) {
                                stack.push(p as usize);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_numbers() {
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0\0\x10JFIF"), Some(Format::Jpeg));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
        assert_eq!(sniff(b"GIF89a......"), Some(Format::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf"), Some(Format::Avif));
        assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0\0\0mif1heic"), Some(Format::Heif));
        assert_eq!(sniff(b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom"), Some(Format::RawOther));
        assert_eq!(sniff(&[0xFF, 0x0A, 0xFA, 0x7F]), Some(Format::Jxl));
        assert_eq!(sniff(b"8BPS\0\x01\0\0\0\0\0\0"), Some(Format::Psd));
        assert_eq!(sniff(b"FUJIFILMCCD-RAW 0201"), Some(Format::RawOther));
        assert_eq!(sniff(b"II*\0\x08\0\0\0\0\0"), Some(Format::Tiff));
        assert_eq!(sniff(b"II*\0\x10\0\0\0CR\x02\0"), Some(Format::RawTiffLike));
        assert_eq!(sniff(b"IIRO\x08\0\0\0"), Some(Format::RawTiffLike));
        assert_eq!(sniff(b"nope"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn dng_detected() {
        // II*, IFD at 8 with a single DNGVersion entry.
        let mut b = b"II*\0\x08\0\0\0".to_vec();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0xC612u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&4u32.to_le_bytes());
        b.extend_from_slice(&[1, 4, 0, 0]);
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(sniff(&b), Some(Format::RawTiffLike));
    }
}
