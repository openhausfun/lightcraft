//! Maker-note IFD helpers.
//!
//! Most vendors store their maker note (Exif tag 37500) as a TIFF IFD, but each uses its own header,
//! byte order and *offset base* (what stored value offsets are relative to). The header layouts here
//! follow the public descriptions in the ExifTool tag documentation (`MakerNotes` table) and the
//! vendors' own published notes; no third-party code was consulted.

use crate::{ByteOrder, Ifd, ParseOptions, parse_ifd_at};
use serde::{Deserialize, Serialize};

/// Maker-note dialects we know how to locate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MakerNoteKind {
    /// Plain IFD at the start of the note, offsets relative to the enclosing TIFF header (Canon, many others).
    Plain,
    /// `Nikon\0` + version + embedded TIFF header at +10; offsets relative to that header.
    NikonV3,
    /// `Nikon\0\x01\0`: IFD at +8, offsets relative to the enclosing TIFF header.
    NikonV1,
    /// `SONY DSC `/`SONY CAM `/`SONY MOBILE` etc.: 12-byte header, offsets relative to the TIFF header.
    Sony,
    /// `FUJIFILM` + LE u32 IFD offset; little-endian, offsets relative to the note start.
    Fujifilm,
    /// `OLYMPUS\0` + byte order + version: IFD at +12, offsets relative to the note start.
    OlympusNew,
    /// `OLYMP\0` / `EPSON\0` etc.: IFD at +8, offsets relative to the TIFF header.
    OlympusOld,
    /// `Panasonic\0\0\0`: IFD at +12, offsets relative to the TIFF header.
    Panasonic,
    /// `AOC\0` + byte order: IFD at +6, offsets relative to the TIFF header.
    PentaxAoc,
    /// `PENTAX \0` + byte order: IFD at +10, offsets relative to the note start.
    Pentax,
}

/// A parsed maker-note IFD.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MakerNote {
    pub kind: MakerNoteKind,
    pub order: ByteOrder,
    /// Absolute offset that stored value offsets are relative to.
    pub base: u64,
    pub ifd: Ifd,
}

/// Locate and parse the maker note stored at absolute `offset` (length `len`) inside `data`, the buffer whose
/// start is the enclosing TIFF header. `order` is the enclosing stream's byte order. `make` (the Exif Make) is
/// used only as a hint for header-less notes. Returns `None` for unknown or unparsable notes.
pub fn parse_makernote(data: &[u8], offset: u64, len: u64, order: ByteOrder, make: &str) -> Option<MakerNote> {
    let start = usize::try_from(offset).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?.min(data.len());
    let note = data.get(start..end)?;
    let order_at = |i: usize| match note.get(i..i + 2) {
        Some(b"II") => Some(ByteOrder::Little),
        Some(b"MM") => Some(ByteOrder::Big),
        _ => None,
    };
    let opts = ParseOptions { max_ifds: 64, max_depth: 3, ..Default::default() };
    let try_parse = |kind: MakerNoteKind, ifd_at: u64, base: u64, ord: ByteOrder| -> Option<MakerNote> {
        let (ifd, _) = parse_ifd_at(data, ifd_at, ord, base, false, &opts).ok()?;
        (!ifd.entries.is_empty()).then_some(MakerNote { kind, order: ord, base, ifd })
    };
    if note.starts_with(b"Nikon\0") {
        if note.get(6) == Some(&1) {
            return try_parse(MakerNoteKind::NikonV1, offset + 8, 0, order);
        }
        let ord = order_at(10)?;
        let ifd_rel = ord.read_u32(note, 14)? as u64;
        let base = offset + 10;
        return try_parse(MakerNoteKind::NikonV3, base + ifd_rel, base, ord);
    }
    if note.starts_with(b"FUJIFILM") || note.starts_with(b"GENERALE") {
        let rel = ByteOrder::Little.read_u32(note, 8)? as u64;
        return try_parse(MakerNoteKind::Fujifilm, offset + rel, offset, ByteOrder::Little);
    }
    if note.starts_with(b"OLYMPUS\0") || note.starts_with(b"OM SYSTEM\0") {
        let (ord_at, ifd_at) = if note.starts_with(b"OM SYSTEM\0") { (12, 16) } else { (8, 12) };
        let ord = order_at(ord_at).unwrap_or(order);
        return try_parse(MakerNoteKind::OlympusNew, offset + ifd_at, offset, ord);
    }
    for p in [b"OLYMP\0", b"EPSON\0", b"MINOL\0", b"CAMER\0", b"SANYO\0"] {
        if note.starts_with(p) {
            return try_parse(MakerNoteKind::OlympusOld, offset + 8, 0, order);
        }
    }
    if note.starts_with(b"Panasonic\0") {
        return try_parse(MakerNoteKind::Panasonic, offset + 12, 0, order);
    }
    if note.starts_with(b"AOC\0") {
        let ord = order_at(4).unwrap_or(order);
        return try_parse(MakerNoteKind::PentaxAoc, offset + 6, 0, ord);
    }
    if note.starts_with(b"PENTAX \0") {
        let ord = order_at(8).unwrap_or(order);
        return try_parse(MakerNoteKind::Pentax, offset + 10, offset, ord);
    }
    if (note.starts_with(b"SONY") || note.starts_with(b"VHAB     \0"))
        && let Some(m) = try_parse(MakerNoteKind::Sony, offset + 12, 0, order)
    {
        return Some(m);
    }
    let _ = make;
    try_parse(MakerNoteKind::Plain, offset, 0, order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IfdBuilder, TiffWriter, Value, tags};

    /// Build a TIFF whose Exif IFD carries `note` as the maker note, return (bytes, note offset, len).
    fn with_note(order: ByteOrder, note: Vec<u8>) -> (Vec<u8>, u64, u64) {
        let mut exif = IfdBuilder::new();
        exif.set(tags::MAKER_NOTE, Value::Undefined(note));
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(tags::MAKE, Value::Ascii("Test".into()));
        ifd0.set_child(tags::EXIF_IFD, exif);
        let bytes = TiffWriter { order, bigtiff: false }.write(&[ifd0]).unwrap();
        let t = crate::Tiff::parse(&bytes).unwrap();
        let e = t.exif().unwrap().get(tags::MAKER_NOTE).unwrap();
        let (off, len) = (e.offset, e.count() as u64);
        (bytes, off, len)
    }

    /// An IFD with one SHORT entry (tag 1, value 7) and one out-of-line LONG[2] entry at `rel` (relative to base).
    fn tiny_ifd(order: ByteOrder, rel_values: u32) -> Vec<u8> {
        let mut v = Vec::new();
        order.put_u16(&mut v, 2);
        order.put_u16(&mut v, 1);
        order.put_u16(&mut v, 3);
        order.put_u32(&mut v, 1);
        order.put_u16(&mut v, 7);
        order.put_u16(&mut v, 0);
        order.put_u16(&mut v, 2);
        order.put_u16(&mut v, 4);
        order.put_u32(&mut v, 2);
        order.put_u32(&mut v, rel_values);
        order.put_u32(&mut v, 0);
        v
    }

    #[test]
    fn nikon_v3_uses_embedded_header_base() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut note = b"Nikon\0\x02\x10\0\0".to_vec();
            let hdr_at = note.len();
            note.extend_from_slice(if order == ByteOrder::Little { b"II" } else { b"MM" });
            order.put_u16(&mut note, 42);
            order.put_u32(&mut note, 8);
            // IFD at +8 (relative to embedded header), values after IFD: 8 + 2 + 24 + 4 = 38
            note.extend(tiny_ifd(order, 38));
            order.put_u32(&mut note, 111);
            order.put_u32(&mut note, 222);
            let (bytes, off, len) = with_note(order, note);
            let m = parse_makernote(&bytes, off, len, order, "NIKON CORPORATION").unwrap();
            assert_eq!(m.kind, MakerNoteKind::NikonV3);
            assert_eq!(m.base, off + hdr_at as u64);
            assert_eq!(m.ifd.u32(1), Some(7));
            assert_eq!(m.ifd.u64s(2).unwrap(), vec![111, 222]);
        }
    }

    #[test]
    fn fujifilm_offsets_relative_to_note() {
        let mut note = b"FUJIFILM".to_vec();
        ByteOrder::Little.put_u32(&mut note, 12);
        // IFD at 12 (relative to note), values at 12 + 30 = 42
        note.extend(tiny_ifd(ByteOrder::Little, 42));
        ByteOrder::Little.put_u32(&mut note, 5);
        ByteOrder::Little.put_u32(&mut note, 6);
        // enclosing file is big-endian: Fuji notes are always little-endian
        let (bytes, off, len) = with_note(ByteOrder::Big, note);
        let m = parse_makernote(&bytes, off, len, ByteOrder::Big, "FUJIFILM").unwrap();
        assert_eq!(m.kind, MakerNoteKind::Fujifilm);
        assert_eq!(m.order, ByteOrder::Little);
        assert_eq!(m.ifd.u64s(2).unwrap(), vec![5, 6]);
    }

    #[test]
    fn plain_canon_style_note() {
        // Plain IFD whose out-of-line offset is relative to the TIFF header: we need the absolute
        // position, which we only know after writing. Write once to learn the offset, then patch.
        let order = ByteOrder::Little;
        let mut note = tiny_ifd(order, 0);
        note.extend_from_slice(&[0; 8]);
        let (mut bytes, off, len) = with_note(order, note);
        let values_abs = off as u32 + 30;
        bytes[off as usize + 22..off as usize + 26].copy_from_slice(&values_abs.to_le_bytes());
        bytes[off as usize + 30..off as usize + 34].copy_from_slice(&9u32.to_le_bytes());
        let m = parse_makernote(&bytes, off, len, order, "Canon").unwrap();
        assert_eq!(m.kind, MakerNoteKind::Plain);
        assert_eq!(m.ifd.u64s(2).unwrap(), vec![9, 0]);
    }

    #[test]
    fn garbage_note_is_none() {
        let (bytes, off, len) = with_note(ByteOrder::Little, vec![0xff; 40]);
        assert!(parse_makernote(&bytes, off, len, ByteOrder::Little, "X").is_none());
        assert!(parse_makernote(&bytes, 1 << 40, 10, ByteOrder::Little, "X").is_none());
        assert!(parse_makernote(&bytes, off, u64::MAX, ByteOrder::Little, "X").is_none());
    }
}
