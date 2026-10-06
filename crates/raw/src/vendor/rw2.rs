//! Panasonic RW2 — the packed (unquantised) variant.
//!
//! Sources: the ExifTool PanasonicRaw tag-name documentation (`0x0002/0x0003` sensor size, `0x0004–0x0007`
//! sensor borders, `0x0009` CFA pattern, `0x000a` bits per sample, `0x000e` linearity limit, `0x001c–0x001e`
//! black levels, `0x0024–0x0026` WB levels, `0x0017` ISO, `0x002d` raw format, `0x002e` JpgFromRaw, `0x0118` raw data
//! offset) and our own black-box analysis of CC0 samples from raw.pixls.us (DC-G9 12-bit, DC-GH5S 14-bit):
//!
//! - The raw data is a sequence of 0x4000-byte chunks; each chunk is stored rotated: its logical content is
//!   bytes `0x1ff8..0x4000` followed by bytes `0..0x1ff8`. (Found by trying every 8-byte rotation and keeping
//!   the one under which the block at each chunk seam is as smooth as its neighbours.)
//! - The logical stream is made of 16-byte blocks, each a little-endian 128-bit word read LSB-first from bit 0:
//!   12-bit data packs 10 samples (8 padding bits at the top), 14-bit data 9 samples (2 padding bits at the top).
//!   Found from the per-bit-position probability of a 1 (padding is always 0; the top bits of 12-bit samples and
//!   the two lowest bits of the GH5S's 14-bit samples are almost always 0), confirmed by image smoothness and by
//!   the decoded masked-free dark level matching the recorded black levels.
//! - Pixels fill rows left to right, rows top to bottom.
//!
//! Panasonic's quantised variant (raw format 4: 14 pixels per block, two 12-bit seeds plus four groups of a 2-bit
//! scale and three 8-bit codes) is not decoded: our black-box analysis identified the block layout but not the
//! reconstruction rule for every scale, and no permissively licensed description exists. Those files report
//! [`RawError::Unsupported`]; their embedded preview still works.

use crate::{BlackLevel, Cfa, ColorData, Mode, OpcodeLists, RawData, RawError, RawFormat, RawImage, Rect, Result};
use lightcraft_geom::Orientation;
use lightcraft_tiff::{Tiff, tags as t};
use rayon::prelude::*;

const SENSOR_WIDTH: u16 = 0x0002;
const SENSOR_HEIGHT: u16 = 0x0003;
const BORDER_TOP: u16 = 0x0004;
const BORDER_LEFT: u16 = 0x0005;
const BORDER_BOTTOM: u16 = 0x0006;
const BORDER_RIGHT: u16 = 0x0007;
const CFA_PATTERN: u16 = 0x0009;
const BITS: u16 = 0x000a;
const LINEARITY_LIMIT: u16 = 0x000e;
const BLACK_RGB: [u16; 3] = [0x001c, 0x001d, 0x001e];
const WB_RGB: [u16; 3] = [0x0024, 0x0025, 0x0026];
const RAW_FORMAT: u16 = 0x002d;
const ISO: u16 = 0x0017;
const RAW_OFFSET: u16 = 0x0118;

const CHUNK: usize = 0x4000;
const ROTATE: usize = 0x1ff8;

/// The logical byte stream of the raw data (chunks un-rotated). A trailing partial chunk is kept as is.
pub(crate) fn unrotate(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for c in src.chunks(CHUNK) {
        if c.len() == CHUNK {
            out.extend_from_slice(&c[ROTATE..]);
            out.extend_from_slice(&c[..ROTATE]);
        } else {
            out.extend_from_slice(c);
        }
    }
    out
}

/// Samples per 16-byte block and the bit position of the first sample (kept for other layouts).
fn layout(bits: u32) -> Option<(usize, u32)> {
    match bits {
        12 => Some((10, 0)),
        14 => Some((9, 0)),
        _ => None,
    }
}

/// Unpack `out.len()` samples from the logical stream.
pub(crate) fn unpack(stream: &[u8], bits: u32, out: &mut [u16]) -> Result<()> {
    let (per, start) = layout(bits).ok_or_else(|| RawError::Unsupported(format!("RW2 packed {bits}-bit data")))?;
    let mask = (1u128 << bits) - 1;
    out.par_chunks_mut(per * 1024).enumerate().for_each(|(ci, o)| {
        let first_block = ci * 1024;
        for (bi, px) in o.chunks_mut(per).enumerate() {
            let at = (first_block + bi) * 16;
            let Some(b) = stream.get(at..at + 16).and_then(|b| <[u8; 16]>::try_from(b).ok()) else {
                px.fill(0);
                continue;
            };
            let word = u128::from_le_bytes(b);
            for (i, p) in px.iter_mut().enumerate() {
                *p = ((word >> (start + i as u32 * bits)) & mask) as u16;
            }
        }
    });
    Ok(())
}

fn cfa(code: u16) -> Cfa {
    let s = match code {
        2 => "GRBG",
        3 => "GBRG",
        4 => "BGGR",
        _ => "RGGB",
    };
    Cfa::bayer_static(s)
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let tiff = Tiff::parse(bytes)?;
    let ifd0 = &tiff.ifds[0];
    let get = |tag: u16| ifd0.u64(tag).map(|v| v as usize);
    let (w, h) = (get(SENSOR_WIDTH).unwrap_or(0), get(SENSOR_HEIGHT).unwrap_or(0));
    let bits = get(BITS).unwrap_or(12) as u32;
    let format = get(RAW_FORMAT).unwrap_or(0);
    if w == 0 || h == 0 {
        return Err(RawError::Corrupt("RW2 without sensor size".into()));
    }
    let n = w.checked_mul(h).filter(|n| *n <= crate::MAX_SAMPLES).ok_or(RawError::Limit("image too large"))?;
    if format != 5 {
        return Err(RawError::Unsupported(format!("Panasonic RW2 raw format {format} (quantised)")));
    }
    let (per, _) = layout(bits).ok_or_else(|| RawError::Unsupported(format!("RW2 packed {bits}-bit data")))?;
    let off = get(RAW_OFFSET).or_else(|| get(t::STRIP_OFFSETS)).ok_or_else(|| RawError::Corrupt("RW2 without raw data offset".into()))?;
    let src = bytes.get(off..).ok_or_else(|| RawError::Corrupt("RW2 raw data outside file".into()))?;
    let need = n.div_ceil(per) * 16;
    if src.len() < need {
        return Err(RawError::Corrupt(format!("RW2 raw data truncated ({} of {need} bytes)", src.len())));
    }
    let mut data = Vec::new();
    if mode == Mode::Full {
        let stream = unrotate(&src[..(need.div_ceil(CHUNK) * CHUNK).min(src.len())]);
        data = vec![0u16; n];
        unpack(&stream, bits, &mut data)?;
    }

    let active = match (get(BORDER_TOP), get(BORDER_LEFT), get(BORDER_BOTTOM), get(BORDER_RIGHT)) {
        (Some(tp), Some(l), Some(b), Some(r)) if b > tp && r > l => Rect::new(l, tp, r - l, b - tp).clipped(w, h),
        _ => Rect::new(0, 0, w, h),
    };
    let active = if active.width == 0 || active.height == 0 { Rect::new(0, 0, w, h) } else { active };
    let cfa = cfa(ifd0.u16(CFA_PATTERN).unwrap_or(1));
    let black = match BLACK_RGB.map(|tag| ifd0.f64(tag)) {
        [Some(r), Some(g), Some(b)] => {
            // per CFA position, anchored at the active area
            let a = cfa.shifted(active.x, active.y);
            let values = a.pattern.iter().map(|&c| [r, g, b][c as usize] as f32).collect();
            BlackLevel { repeat_rows: 2, repeat_cols: 2, values, ..Default::default() }
        }
        _ => BlackLevel::uniform(0.0),
    };
    let white = ifd0.f64(LINEARITY_LIMIT).filter(|v| *v > 0.0).map(|v| v as f32).unwrap_or(((1u32 << bits) - 1) as f32);
    let wb = match WB_RGB.map(|tag| ifd0.f64(tag)) {
        [Some(r), Some(g), Some(b)] if r > 0.0 && g > 0.0 && b > 0.0 => Some([(r / g) as f32, 1.0, (b / g) as f32]),
        _ => None,
    };
    let mut metadata = lightcraft_meta::from_tiff(&tiff);
    metadata.width = Some(active.width as u32);
    metadata.height = Some(active.height as u32);
    if metadata.iso.is_none() {
        metadata.iso = ifd0.u32(ISO).filter(|v| *v > 0);
    }
    let img = RawImage {
        format: RawFormat::Rw2,
        width: w,
        height: h,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits,
        black,
        white: vec![white],
        active_area: active,
        crop: Rect::new(0, 0, active.width, active.height),
        orientation: Orientation::from_exif(ifd0.u16(t::ORIENTATION).unwrap_or(1)),
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
    use lightcraft_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value};

    /// Inverse of the decoder: pack, then rotate every whole chunk.
    pub(crate) fn encode(px: &[u16], bits: u32) -> Vec<u8> {
        let (per, start) = layout(bits).unwrap();
        let mut stream = Vec::new();
        for blk in px.chunks(per) {
            let mut word = 0u128;
            for (i, &v) in blk.iter().enumerate() {
                word |= (v as u128) << (start + i as u32 * bits);
            }
            stream.extend_from_slice(&word.to_le_bytes());
        }
        stream.resize(stream.len().div_ceil(CHUNK) * CHUNK, 0);
        let mut out = Vec::with_capacity(stream.len());
        for c in stream.chunks(CHUNK) {
            // logical = file[ROTATE..] ++ file[..ROTATE]  =>  file = logical[CHUNK-ROTATE..] ++ logical[..CHUNK-ROTATE]
            out.extend_from_slice(&c[CHUNK - ROTATE..]);
            out.extend_from_slice(&c[..CHUNK - ROTATE]);
        }
        out
    }

    pub(crate) fn rw2(w: u16, h: u16, bits: u16, format: u16, payload: &[u8]) -> Vec<u8> {
        let build = |off: u32| {
            let mut ifd = IfdBuilder::new();
            for (tag, v) in [(SENSOR_WIDTH, w), (SENSOR_HEIGHT, h), (BORDER_TOP, 2), (BORDER_LEFT, 4), (BORDER_BOTTOM, h), (BORDER_RIGHT, w)] {
                ifd.set(tag, Value::Short(vec![v]));
            }
            ifd.set(CFA_PATTERN, Value::Short(vec![1]));
            ifd.set(BITS, Value::Short(vec![bits]));
            ifd.set(RAW_FORMAT, Value::Short(vec![format]));
            ifd.set(LINEARITY_LIMIT, Value::Short(vec![(1 << bits) - 1]));
            for (i, tag) in BLACK_RGB.iter().enumerate() {
                ifd.set(*tag, Value::Short(vec![100 + i as u16]));
            }
            for (tag, v) in WB_RGB.iter().zip([512u16, 256, 384]) {
                ifd.set(*tag, Value::Short(vec![v]));
            }
            ifd.set(t::MAKE, Value::Ascii("Panasonic".into()));
            ifd.set(RAW_OFFSET, Value::Long(vec![off]));
            let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
            b[2] = 0x55;
            b
        };
        let head = build(0);
        let off = head.len().div_ceil(16) * 16;
        let mut b = build(off as u32);
        b.resize(off, 0);
        b.extend_from_slice(payload);
        b
    }

    #[test]
    fn packed_12_and_14_bit() {
        let (w, h) = (90usize, 200usize); // > one chunk of data
        for bits in [12u32, 14] {
            let px: Vec<u16> = (0..w * h).map(|i| ((i * 2654435761usize) >> 7) as u16 & ((1 << bits) - 1)).collect();
            let bytes = rw2(w as u16, h as u16, bits as u16, 5, &encode(&px, bits));
            assert_eq!(crate::probe(&bytes), Some(RawFormat::Rw2));
            let r = crate::decode(&bytes).unwrap_or_else(|e| panic!("{bits}: {e}"));
            assert_eq!(crate::probe_info(&bytes).unwrap(), r.info());
            assert_eq!(r.data, RawData::U16(px), "{bits}-bit");
            assert_eq!(r.active_area, Rect::new(4, 2, w - 4, h - 2));
            assert_eq!(r.cfa.as_ref().unwrap().name(), "RGGB");
            assert_eq!(r.black.at(0, 0, 0, 1), 100.0);
            assert_eq!(r.black.at(1, 0, 0, 1), 101.0);
            assert_eq!(r.black.at(1, 1, 0, 1), 102.0);
            assert_eq!(r.wb_multipliers, Some([2.0, 1.0, 1.5]));
        }
        let bytes = rw2(28, 2, 12, 4, &[0; 64]);
        assert!(matches!(crate::decode(&bytes), Err(RawError::Unsupported(_))));
        let bytes = rw2(280, 20, 12, 5, &[0; 64]);
        assert!(matches!(crate::decode(&bytes), Err(RawError::Corrupt(_))));
    }
}
