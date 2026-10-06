//! Embedded preview extraction: the largest baseline/progressive JPEG stored in a TIFF-based raw (IFD strips
//! with JPEG compression, `JPEGInterchangeFormat` pointers in any IFD, Nikon/others' maker-note preview IFDs).

use lightcraft_tiff::image::chunk_bytes;
use lightcraft_tiff::tags as t;
use lightcraft_tiff::{Ifd, Tiff, makernote};

/// Whether `b` looks like a displayable (DCT) JPEG: SOI, and the first SOF marker is not lossless.
fn is_dct_jpeg(b: &[u8]) -> bool {
    if b.len() < 4 || b[0] != 0xff || b[1] != 0xd8 {
        return false;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            return false;
        }
        let m = b[i + 1];
        if m == 0xff {
            i += 1;
            continue;
        }
        match m {
            0xc0..=0xc2 => return true,
            0xc3 | 0xc5..=0xc7 | 0xcb | 0xcd..=0xcf => return false,
            0xda | 0xd9 => return false,
            _ => {}
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        i += 2 + len;
    }
    false
}

fn candidates<'a>(data: &'a [u8], ifd: &Ifd, base: u64, out: &mut Vec<&'a [u8]>) {
    // whole JPEG files stored as an undefined-type tag value (e.g. Panasonic `JpgFromRaw` 0x002e)
    for e in &ifd.entries {
        if matches!(e.value, lightcraft_tiff::Value::Undefined(_))
            && e.count() > 1024
            && let Some(s) = data.get(e.offset as usize..(e.offset.saturating_add(e.count() as u64) as usize).min(data.len()))
            && s.starts_with(&[0xff, 0xd8])
        {
            out.push(s);
        }
    }
    if let (Some(off), Some(len)) = (ifd.u64(t::JPEG_INTERCHANGE_FORMAT), ifd.u64(t::JPEG_INTERCHANGE_FORMAT_LENGTH)) {
        let off = off.saturating_add(base);
        if let Some(s) = data.get(off as usize..(off.saturating_add(len) as usize).min(data.len())) {
            out.push(s);
        }
    }
    if matches!(ifd.u16(t::COMPRESSION), Some(6) | Some(7) | Some(34892))
        && let Ok(info) = ifd.image()
    {
        let chunks = info.chunks(data.len() as u64);
        if chunks.len() == 1
            && let Some(s) = chunk_bytes(data, &chunks[0])
        {
            out.push(s);
        }
    }
}

/// The largest embedded JPEG preview, if any.
pub fn embedded_preview(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.starts_with(b"FUJIFILMCCD-RAW") {
        let j = crate::vendor::raf::header(bytes).ok()?.jpeg?;
        return is_dct_jpeg(j).then(|| trim_eoi(j).to_vec());
    }
    if crate::probe(bytes) == Some(crate::RawFormat::Cr3) {
        return cr3_preview(bytes).map(|j| trim_eoi(j).to_vec());
    }
    let tiff = Tiff::parse(bytes).ok()?;
    let mut found: Vec<&[u8]> = Vec::new();
    for ifd in tiff.all_ifds() {
        candidates(bytes, ifd, 0, &mut found);
    }
    // maker-note preview IFDs (e.g. Nikon PreviewIFD 0x0011 holds JPEGInterchangeFormat relative to the note base)
    if let Some(exif) = tiff.exif()
        && let Some(e) = exif.get(t::MAKER_NOTE)
    {
        let make = tiff.find(t::MAKE).and_then(|e| e.value.as_str()).unwrap_or_default();
        if let Some(mn) = makernote::parse_makernote(bytes, e.offset, e.count() as u64, tiff.order, &make) {
            candidates(bytes, &mn.ifd, mn.base, &mut found);
            if let Some(off) = mn.ifd.u64(0x0011)
                && let Ok((pifd, _)) = lightcraft_tiff::parse_ifd_at(bytes, mn.base + off, mn.order, mn.base, false, &Default::default())
            {
                candidates(bytes, &pifd, mn.base, &mut found);
            }
        }
    }
    // Olympus: CameraSettings preview
    if let Some(p) = crate::vendor::orf::preview(bytes) {
        found.push(p);
    }
    found.into_iter().filter(|s| is_dct_jpeg(s)).max_by_key(|s| s.len()).map(|s| trim_eoi(s).to_vec())
}

/// Canon CR3 (ISO base media file): the `PRVW` box (Laurent Clévy's CR3 notes; layout confirmed on a CC0
/// sample) is `u32 size, "PRVW", u32 0, u16 ?, u16 width, u16 height, u16 ?, u32 jpeg length, JPEG`; the smaller
/// `THMB` box has the same shape. Returns the larger valid one.
fn cr3_preview(bytes: &[u8]) -> Option<&[u8]> {
    let mut best: Option<&[u8]> = None;
    for tag in [b"PRVW", b"THMB"] {
        let mut from = 0;
        while let Some(i) = bytes.get(from..).and_then(|s| s.windows(4).position(|w| w == tag)).map(|p| p + from) {
            from = i + 4;
            let Some(start) = i.checked_sub(4) else { continue };
            let be32 = |at: usize| bytes.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize);
            let (Some(size), Some(len)) = (be32(start), be32(start + 20)) else { continue };
            let j = start + 24;
            if len < 4 || j + len > start + size.max(24) || j + len > bytes.len() {
                continue;
            }
            let jpeg = &bytes[j..j + len];
            if is_dct_jpeg(jpeg) {
                if best.is_none_or(|b| b.len() < jpeg.len()) {
                    best = Some(jpeg);
                }
                break;
            }
        }
    }
    best
}

/// Trim trailing garbage after the last EOI when a stored length over-reports.
fn trim_eoi(s: &[u8]) -> &[u8] {
    let end = s.windows(2).rposition(|w| w == [0xff, 0xd9]).map(|p| p + 2).unwrap_or(s.len());
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, Value};

    fn fake_jpeg(n: usize) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x0b, 8, 0, 1, 0, 1, 1, 1, 0x11, 0];
        j.extend(std::iter::repeat_n(0x55u8, n));
        j.extend_from_slice(&[0xff, 0xd9]);
        j
    }

    #[test]
    fn picks_largest_dct_jpeg() {
        let small = fake_jpeg(10);
        let big = fake_jpeg(500);
        let lossless = crate::ljpeg::encode(&[1u16; 64], 8, 8, 1, 12, 1, 0);
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::COMPRESSION, Value::Short(vec![6]));
        ifd0.set(t::IMAGE_WIDTH, Value::Long(vec![1]));
        ifd0.set(t::IMAGE_LENGTH, Value::Long(vec![1]));
        ifd0.set_image(ImageData::Strips { rows_per_strip: 1, strips: vec![small.clone()] });
        let mut raw = IfdBuilder::new();
        raw.set(t::COMPRESSION, Value::Short(vec![7]));
        raw.set(t::IMAGE_WIDTH, Value::Long(vec![8]));
        raw.set(t::IMAGE_LENGTH, Value::Long(vec![8]));
        raw.set_image(ImageData::Strips { rows_per_strip: 8, strips: vec![lossless] });
        ifd0.add_sub_ifd(raw);
        let mut sub = IfdBuilder::new();
        sub.set(t::COMPRESSION, Value::Short(vec![7]));
        sub.set(t::IMAGE_WIDTH, Value::Long(vec![2]));
        sub.set(t::IMAGE_LENGTH, Value::Long(vec![2]));
        sub.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![1]));
        sub.set_image(ImageData::Strips { rows_per_strip: 2, strips: vec![big.clone()] });
        ifd0.add_sub_ifd(sub);
        let bytes = TiffWriter::default().write(&[ifd0]).unwrap();
        assert_eq!(embedded_preview(&bytes).unwrap(), big);
        assert!(embedded_preview(b"nope").is_none());
        // CR3: a PRVW box after the ftyp
        let mut cr3 = b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom".to_vec();
        let j = fake_jpeg(300);
        cr3.extend_from_slice(&((24 + j.len()) as u32).to_be_bytes());
        cr3.extend_from_slice(b"PRVW\0\0\0\0\0\x01\x06\x54\x04\x38\0\x01");
        cr3.extend_from_slice(&(j.len() as u32).to_be_bytes());
        cr3.extend_from_slice(&j);
        assert_eq!(embedded_preview(&cr3).unwrap(), j);
        for n in 0..cr3.len() {
            let _ = embedded_preview(&cr3[..n]);
        }
    }
}
