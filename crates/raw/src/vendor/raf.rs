//! Fujifilm RAF — uncompressed Bayer and X-Trans.
//!
//! Sources: the ExifTool FujiFilm tag-name documentation (RAF header record tags `0x0100` RawImageFullSize,
//! `0x0110` RawImageCropTopLeft, `0x0111` RawImageCroppedSize, `0x0131` XTransLayout; raw-IFD tags `0xf001`
//! width, `0xf002` height, `0xf003` bits, `0xf007` strip offset, `0xf008` strip byte count, `0xf00a` black
//! levels, `0xf00e` WB_GRBLevels) and our own black-box analysis of CC0 samples from raw.pixls.us:
//!
//! - The file starts with a 16-byte magic, version and camera name; at byte 84 eight big-endian `u32`s give the
//!   embedded JPEG (offset, length), the header-record directory (offset, length) and the raw block (offset,
//!   length). The record directory is a `u32` count then `(u16 tag, u16 size, data)` records, big-endian.
//! - In current bodies the raw block is a little-endian TIFF whose IFD0 has a `0xf000` IFD pointer to the raw
//!   IFD; offsets in it are relative to the raw block.
//! - Sample packing, found by testing candidate bit orders for the smoothest image: 16-bit little-endian words;
//!   12-bit LSB-first bit stream; 14-bit (and other depths) stored as little-endian 32-bit words read MSB-first.
//!   `0xf005`, when non-zero, is the row stride in 32-bit words.
//! - CFA: the X-Trans layout record (36 bytes, 0 = R, 1 = G, 2 = B) is stored in reverse order and, reversed,
//!   is anchored at raw pixel (0, 0) — verified on X-Trans I and III samples by minimising the difference
//!   between horizontally / vertically adjacent "green" samples over all 72 orientations/phases, and by colour
//!   renders. Bayer bodies (X-A series) use RGGB at (0, 0), verified by colour renders.
//! - Fujifilm's compressed RAF variants are reported as unsupported (their embedded preview still works).

use super::white_from_data;
use crate::unpack::{unpack_lsb, unpack_msb};
use crate::{BlackLevel, Cfa, ColorData, Mode, OpcodeLists, RawData, RawError, RawFormat, RawImage, Rect, Result};
use lightcraft_geom::Orientation;
use lightcraft_tiff::{ByteOrder, Ifd, Tiff};
use rayon::prelude::*;

const CROP_TOP_LEFT: u16 = 0x0110;
const CROPPED_SIZE: u16 = 0x0111;
const XTRANS_LAYOUT: u16 = 0x0131;

/// The fixed-position header pointers.
pub(crate) struct Header<'a> {
    pub jpeg: Option<&'a [u8]>,
    pub records: Vec<(u16, &'a [u8])>,
    pub raw: Option<&'a [u8]>,
}

fn be32(b: &[u8], at: usize) -> Option<usize> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize)
}

fn slice(b: &[u8], off: Option<usize>, len: Option<usize>) -> Option<&[u8]> {
    let (o, l) = (off?, len?);
    if o == 0 || l == 0 {
        return None;
    }
    b.get(o..o.checked_add(l)?.min(b.len())).filter(|s| !s.is_empty())
}

pub(crate) fn header(b: &[u8]) -> Result<Header<'_>> {
    if !b.starts_with(b"FUJIFILMCCD-RAW") || b.len() < 108 {
        return Err(RawError::NotRaw);
    }
    let jpeg = slice(b, be32(b, 84), be32(b, 88));
    let raw = slice(b, be32(b, 100), be32(b, 104));
    let mut records = Vec::new();
    if let Some(dir) = slice(b, be32(b, 92), be32(b, 96)) {
        let n = be32(dir, 0).unwrap_or(0).min(4096);
        let mut p = 4;
        for _ in 0..n {
            let Some(h) = dir.get(p..p + 4) else { break };
            let (tag, size) = (u16::from_be_bytes([h[0], h[1]]), u16::from_be_bytes([h[2], h[3]]) as usize);
            let Some(d) = dir.get(p + 4..p + 4 + size) else { break };
            records.push((tag, d));
            p += 4 + size;
        }
    }
    Ok(Header { jpeg, records, raw })
}

fn record<'a>(h: &Header<'a>, tag: u16) -> Option<&'a [u8]> {
    h.records.iter().find(|r| r.0 == tag).map(|r| r.1)
}

/// A record holding two big-endian `u16`s.
fn pair(h: &Header, tag: u16) -> Option<(usize, usize)> {
    let d = record(h, tag).filter(|d| d.len() >= 4)?;
    Some((u16::from_be_bytes([d[0], d[1]]) as usize, u16::from_be_bytes([d[2], d[3]]) as usize))
}

fn raw_ifd(raw: &[u8]) -> Result<Ifd> {
    let tiff = Tiff::parse(raw).map_err(|_| RawError::Unsupported("RAF without a raw IFD (older FinePix layout)".into()))?;
    let off = tiff.ifds.first().and_then(|i| i.u64(0xf000)).ok_or_else(|| RawError::Unsupported("RAF without a raw IFD".into()))?;
    let (ifd, _) = lightcraft_tiff::parse_ifd_at(raw, off, ByteOrder::Little, 0, false, &Default::default())?;
    Ok(ifd)
}

/// The CFA: X-Trans from the (reversed) layout record, else RGGB.
fn cfa(h: &Header) -> Cfa {
    match record(h, XTRANS_LAYOUT) {
        Some(l) if l.len() == 36 && l.iter().all(|&c| c <= 2) => Cfa { width: 6, height: 6, pattern: l.iter().rev().copied().collect() },
        _ => Cfa::bayer_static("RGGB"),
    }
}

/// Unpack one row of `bits`-bit samples in Fujifilm's packing.
pub(crate) fn unpack_row(src: &[u8], bits: u32, out: &mut [u16]) {
    match bits {
        16 => out.iter_mut().zip(src.as_chunks::<2>().0).for_each(|(o, c)| *o = u16::from_le_bytes([c[0], c[1]])),
        12 => unpack_lsb(src, 12, out),
        _ => {
            let swapped: Vec<u8> = src
                .chunks(4)
                .flat_map(|c| {
                    let mut w = [0u8; 4];
                    w[..c.len()].copy_from_slice(c);
                    [w[3], w[2], w[1], w[0]]
                })
                .collect();
            unpack_msb(&swapped, bits, out)
        }
    }
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let h = header(bytes)?;
    let raw = h.raw.ok_or_else(|| RawError::Corrupt("RAF without raw data".into()))?;
    let ifd = raw_ifd(raw)?;
    let get = |t: u16| ifd.u64(t).map(|v| v as usize);
    let (w, hgt) = (get(0xf001).unwrap_or(0), get(0xf002).unwrap_or(0));
    let bits = get(0xf003).unwrap_or(0) as u32;
    let (off, len) = (get(0xf007).unwrap_or(0), get(0xf008).unwrap_or(0));
    if w == 0 || hgt == 0 || !(8..=16).contains(&bits) {
        return Err(RawError::Corrupt(format!("RAF raw IFD: {w}x{hgt}, {bits} bits")));
    }
    let n = w.checked_mul(hgt).filter(|n| *n <= crate::MAX_SAMPLES).ok_or(RawError::Limit("image too large"))?;
    // samples of any depth may be stored in 16-bit words (e.g. 14-bit X-T20); otherwise packed at `bits`
    let store = if len as u64 >= (n as u64) * 2 { 16 } else { bits };
    let packed_row = if store == 16 { w * 2 } else { (w * store as usize).div_ceil(8) };
    let stride = match get(0xf005).unwrap_or(0) {
        0 => packed_row,
        words => (words * 4).max(packed_row),
    };
    if (len as u64) < (stride as u64) * (hgt as u64 - 1) + packed_row as u64 {
        return Err(RawError::Unsupported(format!("Fujifilm compressed RAF ({len} bytes for {w}x{hgt} {bits}-bit)")));
    }
    let src = raw.get(off..off.saturating_add(len).min(raw.len())).ok_or_else(|| RawError::Corrupt("RAF strip outside file".into()))?;
    let mut data = Vec::new();
    if mode == Mode::Full {
        data = vec![0u16; n];
        data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
            let s = src.get(y * stride..(y * stride + packed_row).min(src.len())).unwrap_or(&[]);
            unpack_row(s, store, row);
        });
    }

    let cfa = cfa(&h);
    let active = match (pair(&h, CROP_TOP_LEFT), pair(&h, CROPPED_SIZE)) {
        (Some((top, left)), Some((ch, cw))) if ch > 0 && cw > 0 => Rect::new(left, top, cw, ch).clipped(w, hgt),
        _ => Rect::new(0, 0, w, hgt),
    };
    let active = if active.width == 0 || active.height == 0 { Rect::new(0, 0, w, hgt) } else { active };
    // black levels: one per CFA position, anchored at (0, 0) of the raw data; re-anchor at the active area
    let black = match ifd.f64s(0xf00a) {
        Some(v) if v.len() == cfa.pattern.len() && v.iter().any(|b| *b != v[0]) => {
            let (cw, chh) = (cfa.width, cfa.height);
            let values = (0..chh * cw).map(|i| v[((i / cw + active.y) % chh) * cw + (i % cw + active.x) % cw] as f32).collect();
            BlackLevel { repeat_rows: chh, repeat_cols: cw, values, ..Default::default() }
        }
        Some(v) if !v.is_empty() => BlackLevel::uniform(v[0] as f32),
        _ => BlackLevel::uniform(0.0),
    };
    let wb =
        ifd.f64s(0xf00e).filter(|v| v.len() >= 3 && v.iter().take(3).all(|x| *x > 0.0)).map(|v| [(v[1] / v[0]) as f32, 1.0, (v[2] / v[0]) as f32]);
    let white = white_from_data(&data, bits);

    let mut metadata = h.jpeg.map(lightcraft_meta::extract).unwrap_or_default();
    metadata.width = Some(active.width as u32);
    metadata.height = Some(active.height as u32);
    let orientation = metadata.orientation.unwrap_or(Orientation::Normal);
    let img = RawImage {
        format: RawFormat::Raf,
        width: w,
        height: hgt,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits,
        black,
        white: vec![white],
        active_area: active,
        crop: Rect::new(0, 0, active.width, active.height),
        orientation,
        color: ColorData::default(),
        wb_multipliers: wb,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata,
    };
    img.validate_for(mode)?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal RAF: header, records, raw block (LE TIFF whose IFD0 points to a 0xf000 raw IFD).
    pub(crate) fn raf(w: u32, h: u32, bits: u32, strip: Vec<u8>, layout: Option<[u8; 36]>, jpeg: &[u8]) -> Vec<u8> {
        let mut block = b"II*\0\x08\0\0\0".to_vec();
        let sub_off: u32 = 8 + 2 + 12 + 4;
        block.extend_from_slice(&1u16.to_le_bytes());
        block.extend_from_slice(&[0x00, 0xf0, 13, 0, 1, 0, 0, 0]);
        block.extend_from_slice(&sub_off.to_le_bytes());
        block.extend_from_slice(&0u32.to_le_bytes());
        let n = 7u32;
        let wb_off = sub_off + 2 + 12 * n + 4;
        let strip_off = wb_off + 12;
        let entries: [(u16, u32, u32); 7] = [
            (0xf001, 1, w),
            (0xf002, 1, h),
            (0xf003, 1, bits),
            (0xf007, 1, strip_off),
            (0xf008, 1, strip.len() as u32),
            (0xf00a, 1, 64),
            (0xf00e, 3, wb_off),
        ];
        block.extend_from_slice(&(n as u16).to_le_bytes());
        for (tag, count, v) in entries {
            block.extend_from_slice(&tag.to_le_bytes());
            block.extend_from_slice(&4u16.to_le_bytes());
            block.extend_from_slice(&count.to_le_bytes());
            block.extend_from_slice(&v.to_le_bytes());
        }
        block.extend_from_slice(&0u32.to_le_bytes());
        for v in [300u32, 600, 450] {
            block.extend_from_slice(&v.to_le_bytes());
        }
        block.extend_from_slice(&strip);
        let mut recs: Vec<u8> = Vec::new();
        let mut nrec = 0u32;
        let mut add = |tag: u16, d: &[u8]| {
            recs.extend_from_slice(&tag.to_be_bytes());
            recs.extend_from_slice(&(d.len() as u16).to_be_bytes());
            recs.extend_from_slice(d);
            nrec += 1;
        };
        add(0x0100, &[(h >> 8) as u8, h as u8, (w >> 8) as u8, w as u8]);
        add(0x0110, &[0, 2, 0, 4]);
        add(0x0111, &[0, (h - 2) as u8, 0, (w - 4) as u8]);
        if let Some(l) = layout {
            add(0x0131, &l);
        }
        let mut dir = nrec.to_be_bytes().to_vec();
        dir.extend_from_slice(&recs);
        let mut out = b"FUJIFILMCCD-RAW 0201FF000000TEST".to_vec();
        out.resize(108, 0);
        let jpeg_off = 160u32;
        let dir_off = jpeg_off + jpeg.len() as u32;
        let raw_off = dir_off + dir.len() as u32;
        for (i, v) in [jpeg_off, jpeg.len() as u32, dir_off, dir.len() as u32, raw_off, block.len() as u32].iter().enumerate() {
            out[84 + i * 4..88 + i * 4].copy_from_slice(&v.to_be_bytes());
        }
        out.resize(160, 0);
        out.extend_from_slice(jpeg);
        out.extend_from_slice(&dir);
        out.extend_from_slice(&block);
        out
    }

    fn pack(px: &[u16], bits: u32) -> Vec<u8> {
        match bits {
            16 => px.iter().flat_map(|v| v.to_le_bytes()).collect(),
            12 => {
                let mut o = Vec::new();
                for p in px.chunks(2) {
                    let v = p[0] as u32 | (p[1] as u32) << 12;
                    o.extend_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8]);
                }
                o
            }
            _ => {
                // MSB-first stream, then byte-swap every 32-bit word
                let mut bitsv: Vec<u8> = Vec::new();
                let (mut acc, mut n) = (0u64, 0u32);
                for &v in px {
                    acc = (acc << bits) | v as u64;
                    n += bits;
                    while n >= 8 {
                        bitsv.push((acc >> (n - 8)) as u8);
                        n -= 8;
                    }
                }
                if n > 0 {
                    bitsv.push((acc << (8 - n)) as u8);
                }
                bitsv.resize(bitsv.len().div_ceil(4) * 4, 0);
                bitsv.chunks(4).flat_map(|c| [c[3], c[2], c[1], c[0]]).collect()
            }
        }
    }

    #[test]
    fn decodes_all_packings() {
        let (w, h) = (16u32, 6u32);
        for (bits, store) in [(12u32, 12u32), (14, 14), (14, 16), (16, 16)] {
            let px: Vec<u16> = (0..w * h).map(|i| ((i * 977) % (1 << bits.min(14))) as u16).collect();
            let layout: [u8; 36] = std::array::from_fn(|i| Cfa::xtrans().pattern[35 - i]);
            let bytes = raf(w, h, bits, pack(&px, store), Some(layout), b"\xff\xd8\xff\xd9");
            assert_eq!(crate::probe(&bytes), Some(RawFormat::Raf));
            let r = crate::decode(&bytes).unwrap_or_else(|e| panic!("{bits}: {e}"));
            assert_eq!(crate::probe_info(&bytes).unwrap(), r.info());
            assert_eq!(r.data, RawData::U16(px), "{bits}-bit");
            assert_eq!(r.cfa, Some(Cfa::xtrans()));
            assert_eq!(r.active_area, Rect::new(4, 2, 12, 4));
            assert_eq!(r.black.mean(), 64.0);
            assert_eq!(r.wb_multipliers, Some([2.0, 1.0, 1.5]));
        }
        let bytes = raf(w, h, 16, pack(&[0; 96], 16), None, b"");
        assert_eq!(crate::decode(&bytes).unwrap().cfa.unwrap().name(), "RGGB");
        // too little data for the size: compressed
        let bytes = raf(w, h, 14, vec![0; 20], None, b"");
        assert!(matches!(crate::decode(&bytes), Err(RawError::Unsupported(_))));
    }
}
