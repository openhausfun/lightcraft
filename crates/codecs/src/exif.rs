//! Minimal, bounds-checked TIFF/EXIF walking: just what decoding needs (orientation, embedded
//! thumbnail, colour-space hint). Full EXIF parsing lives in `lightcraft-meta`.

/// Byte-order aware reader over a TIFF-structured blob.
#[derive(Clone, Copy)]
pub(crate) struct Tiff<'a> {
    pub data: &'a [u8],
    le: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// Offset of the 4-byte value/offset field within `data`.
    pub value_pos: usize,
}

impl<'a> Tiff<'a> {
    pub fn new(data: &'a [u8]) -> Option<Tiff<'a>> {
        let le = match data.get(0..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        Some(Tiff { data, le })
    }
    pub fn u16_at(&self, pos: usize) -> Option<u16> {
        let b = self.data.get(pos..pos.checked_add(2)?)?;
        Some(if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    }
    pub fn u32_at(&self, pos: usize) -> Option<u32> {
        let b = self.data.get(pos..pos.checked_add(4)?)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    pub fn first_ifd(&self) -> Option<usize> {
        let magic = self.u16_at(2)?;
        if magic != 42 {
            return None;
        }
        Some(self.u32_at(4)? as usize)
    }
    /// Entries of the IFD at `pos` and the offset of the next IFD (0 = none).
    pub fn ifd(&self, pos: usize) -> Option<(Vec<Entry>, usize)> {
        let n = self.u16_at(pos)? as usize;
        if n > 4096 {
            return None;
        }
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            let e = pos + 2 + i * 12;
            v.push(Entry { tag: self.u16_at(e)?, typ: self.u16_at(e + 2)?, count: self.u32_at(e + 4)?, value_pos: e + 8 });
        }
        let next = self.u32_at(pos + 2 + n * 12).unwrap_or(0) as usize;
        Some((v, next))
    }
    /// First value of a SHORT/LONG entry.
    pub fn uint(&self, e: &Entry) -> Option<u32> {
        match e.typ {
            3 => self.u16_at(e.value_pos).map(u32::from),
            4 | 13 => self.u32_at(e.value_pos),
            1 | 7 => self.data.get(e.value_pos).map(|b| *b as u32),
            _ => None,
        }
    }
    /// Raw bytes of an entry's value (for ASCII/UNDEFINED/BYTE types).
    pub fn bytes(&self, e: &Entry) -> Option<&'a [u8]> {
        let size = match e.typ {
            1 | 2 | 6 | 7 => 1usize,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        let len = size.checked_mul(e.count as usize)?;
        let start = if len <= 4 { e.value_pos } else { self.u32_at(e.value_pos)? as usize };
        self.data.get(start..start.checked_add(len)?)
    }
}

/// What the decoder cares about from an EXIF (TIFF-structured) blob.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ExifSummary {
    pub orientation: Option<u16>,
    /// (offset, length) of the IFD1 JPEG thumbnail within the blob.
    pub thumbnail: Option<(usize, usize)>,
    /// EXIF ColorSpace = 0xFFFF with InteropIndex "R03" (Adobe RGB (1998) "option" files).
    pub adobe_rgb_hint: bool,
}

pub(crate) fn summarize(blob: &[u8]) -> ExifSummary {
    let mut s = ExifSummary::default();
    let Some(t) = Tiff::new(blob) else { return s };
    let Some(ifd0) = t.first_ifd() else { return s };
    let Some((entries, next)) = t.ifd(ifd0) else { return s };
    let mut exif_ifd = None;
    for e in &entries {
        match e.tag {
            0x0112 => s.orientation = t.uint(e).map(|v| v as u16).filter(|v| (1..=8).contains(v)),
            0x8769 => exif_ifd = t.uint(e).map(|v| v as usize),
            _ => {}
        }
    }
    if next != 0
        && next != ifd0
        && let Some((e1, _)) = t.ifd(next)
    {
        let off = e1.iter().find(|e| e.tag == 0x0201).and_then(|e| t.uint(e));
        let len = e1.iter().find(|e| e.tag == 0x0202).and_then(|e| t.uint(e));
        if let (Some(o), Some(l)) = (off, len) {
            let (o, l) = (o as usize, l as usize);
            if l > 4 && o.checked_add(l).is_some_and(|end| end <= blob.len()) && blob[o..o + 2] == [0xFF, 0xD8] {
                s.thumbnail = Some((o, l));
            }
        }
    }
    if let Some(p) = exif_ifd
        && let Some((ee, _)) = t.ifd(p)
    {
        let cs = ee.iter().find(|e| e.tag == 0xA001).and_then(|e| t.uint(e));
        let interop = ee.iter().find(|e| e.tag == 0xA005).and_then(|e| t.uint(e));
        if cs == Some(0xFFFF)
            && let Some((ie, _)) = interop.and_then(|p| t.ifd(p as usize))
        {
            s.adobe_rgb_hint = ie.iter().any(|e| e.tag == 1 && t.bytes(e).is_some_and(|b| b.starts_with(b"R03")));
        }
    }
    s
}

/// Build a tiny EXIF blob (little-endian TIFF) with just an orientation tag. Used by tests and by
/// callers that need to stamp orientation on export when no source EXIF exists.
pub fn minimal_exif(orientation: u16) -> Vec<u8> {
    let mut v = Vec::with_capacity(26);
    v.extend_from_slice(b"II");
    v.extend_from_slice(&42u16.to_le_bytes());
    v.extend_from_slice(&8u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&0x0112u16.to_le_bytes());
    v.extend_from_slice(&3u16.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.extend_from_slice(&orientation.to_le_bytes());
    v.extend_from_slice(&[0, 0]);
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_roundtrip() {
        for o in 1..=8 {
            assert_eq!(summarize(&minimal_exif(o)).orientation, Some(o));
        }
    }

    #[test]
    fn garbage_ok() {
        assert_eq!(summarize(b"II*\0\xff\xff\xff\xff"), ExifSummary::default());
        assert_eq!(summarize(&[]), ExifSummary::default());
    }
}
