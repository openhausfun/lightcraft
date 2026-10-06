//! Canon CR2.
//!
//! Source: Laurent Clévy, "Understanding what is stored in a Canon RAW .CR2 file" (prose sections on the file
//! structure, IFD#3, the `0xc640` slice layout and the maker-note `SensorInfo` table), plus the ExifTool Canon
//! tag-name documentation for maker-note tag meanings. Only the structural description was used.
//!
//! - The raw data is IFD#3's single strip: a lossless-JPEG frame (usually 2 or 4 components).
//! - The decoded sample stream fills vertical slices left to right: `cr2_slice = [n, w, last]` means `n` slices of
//!   width `w` then one of width `last`; each slice is filled top to bottom, row by row.
//! - Colour filter layout: IFD#3 tag `0xc5e0` (`CR2CFAPattern` in the ExifTool EXIF tag-name docs): 1 = RGGB,
//!   2 = BGGR, 3 = GBRG, 4 = GRBG, anchored at the top-left of the full decoded sensor (masked borders included).
//!   It differs by model (e.g. 3 on the 50D/60D/7D/550D/5D Mark II, 1 on the 40D/5D Mark III/6D/5DS R), so it is
//!   read per file (no model table); files without the tag fall back to RGGB (issue #85). The parity of the
//!   `SensorInfo` borders does *not* predict it (RGGB files come with both even and odd top borders).
//! - `SensorInfo` (maker note `0x00e0`): sensor width/height and the left/top/right/bottom borders of the image
//!   area; the masked columns left of it give the black level.
//! - `ColorBalance` (maker note `0x4001`): as-shot `RGGB` levels at a model-dependent offset; we probe the known
//!   offsets and accept the first plausible quadruple.
//! - sRAW / mRAW (YCbCr, subsampled) are not supported yet.

use super::{black_from_columns, white_from_data};
use crate::{BlackLevel, Cfa, ColorData, Mode, OpcodeLists, RawData, RawError, RawFormat, RawImage, Rect, Result, ljpeg};
use lightcraft_geom::Orientation;
use lightcraft_tiff::image::chunk_bytes;
use lightcraft_tiff::{Ifd, Tiff, makernote, tags as t};

const CR2_SLICE: u16 = 0xc640;
const SRAW_TYPE: u16 = 0xc6c5;
const CR2_CFA_PATTERN: u16 = 0xc5e0;
const SENSOR_INFO: u16 = 0x00e0;
const COLOR_BALANCE: u16 = 0x4001;

fn raw_ifd(tiff: &Tiff) -> Option<&Ifd> {
    tiff.ifds.get(3).or_else(|| tiff.ifds.iter().rev().find(|i| i.contains(CR2_SLICE)))
}

/// Bayer layout from the raw IFD's `CR2CFAPattern` (`0xc5e0`) value; `None` for missing/unknown values.
fn cfa_from_tag(v: Option<u64>) -> Option<Cfa> {
    let name = match v? {
        1 => "RGGB",
        2 => "BGGR",
        3 => "GBRG",
        4 => "GRBG",
        _ => return None,
    };
    Some(Cfa::bayer_static(name))
}

/// As-shot WB multipliers (R, G, B; G = 1) from the ColorBalance array.
fn wb_from_color_balance(v: &[u64]) -> Option<[f32; 3]> {
    for off in [63usize, 25, 24, 34, 71, 85, 105, 69, 77] {
        let q = v.get(off..off + 4)?;
        let (r, g1, g2, b) = (q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32);
        if g1 < 256.0 || g2 < 256.0 || r < 64.0 || b < 64.0 || g1 > 16384.0 || (g1 - g2).abs() > 0.05 * g1 {
            continue;
        }
        let g = (g1 + g2) / 2.0;
        let (mr, mb) = (r / g, b / g);
        if (0.25..6.0).contains(&mr) && (0.25..6.0).contains(&mb) {
            return Some([mr, 1.0, mb]);
        }
    }
    None
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let tiff = Tiff::parse(bytes)?;
    let ifd0 = &tiff.ifds[0];
    let raw = raw_ifd(&tiff).ok_or_else(|| RawError::Corrupt("CR2 without raw IFD".into()))?;
    if raw.u32(SRAW_TYPE).is_some_and(|v| v != 1) {
        return Err(RawError::Unsupported("Canon sRAW/mRAW".into()));
    }
    let off = raw.u64(t::STRIP_OFFSETS).ok_or(RawError::Tiff(lightcraft_tiff::TiffError::MissingTag(t::STRIP_OFFSETS)))?;
    let len = raw.u64(t::STRIP_BYTE_COUNTS).unwrap_or(bytes.len() as u64 - off.min(bytes.len() as u64));
    let chunk = lightcraft_tiff::image::Chunk { index: 0, x: 0, y: 0, width: 0, height: 0, plane: 0, offset: off, len };
    let src = chunk_bytes(bytes, &chunk).ok_or_else(|| RawError::Corrupt("raw strip outside file".into()))?;
    let (fw, fh, nc, prec) = ljpeg::frame_info(src)?;
    let frame = match mode {
        Mode::Full => Some(ljpeg::decode(src, (fw * fh * nc).min(crate::MAX_SAMPLES))?),
        Mode::Header => None,
    };
    let total = match &frame {
        Some(f) => f.data.len(),
        None => {
            let total = fw
                .checked_mul(fh)
                .and_then(|v| v.checked_mul(nc))
                .filter(|t| *t > 0)
                .ok_or_else(|| RawError::Corrupt("bad CR2 frame size".into()))?;
            if total > crate::MAX_SAMPLES {
                return Err(RawError::Limit("lossless JPEG frame larger than expected"));
            }
            total
        }
    };
    let slices = raw.u64s(CR2_SLICE).filter(|s| s.len() == 3 && s[1] > 0 && s[2] > 0);
    let widths: Vec<usize> = match &slices {
        Some(s) => {
            let n = s[0].min(64) as usize;
            let mut v = vec![s[1] as usize; n];
            v.push(s[2] as usize);
            v
        }
        None => vec![fw * nc],
    };
    let width: usize = widths.iter().sum();
    if width == 0 || total % width != 0 {
        return Err(RawError::Corrupt(format!("CR2 slices ({width}) do not divide the frame ({total} samples)")));
    }
    let height = total / width;
    let mut data = Vec::new();
    if let Some(frame) = &frame {
        data = vec![0u16; total];
        let mut i = 0;
        let mut x0 = 0;
        for &sw in &widths {
            for y in 0..height {
                data[y * width + x0..y * width + x0 + sw].copy_from_slice(&frame.data[i..i + sw]);
                i += sw;
            }
            x0 += sw;
        }
    }
    drop(frame);

    // maker note: sensor borders and white balance
    let make = ifd0.string(t::MAKE).unwrap_or_default();
    let mn =
        tiff.exif().and_then(|e| e.get(t::MAKER_NOTE)).and_then(|e| makernote::parse_makernote(bytes, e.offset, e.count() as u64, tiff.order, &make));
    let mut active = Rect::new(0, 0, width, height);
    if let Some(si) = mn.as_ref().and_then(|m| m.ifd.u64s(SENSOR_INFO)).filter(|v| v.len() >= 9) {
        let (l, tp, r, b) = (si[5] as usize, si[6] as usize, si[7] as usize, si[8] as usize);
        if r > l && b > tp && r < width && b < height {
            active = Rect::new(l, tp, r - l + 1, b - tp + 1);
        }
    }
    let wb = mn.as_ref().and_then(|m| m.ifd.u64s(COLOR_BALANCE)).and_then(|v| wb_from_color_balance(&v));
    let cfa = cfa_from_tag(raw.u64(CR2_CFA_PATTERN)).unwrap_or_else(|| Cfa::bayer_static("RGGB"));
    let black = if active.x >= 8 {
        black_from_columns(&data, width, 2..active.x - 2, active.y..active.y + active.height, active)
    } else {
        BlackLevel::uniform(0.0)
    };
    let white = white_from_data(&data, prec as u32);
    let mut metadata = lightcraft_meta::from_tiff(&tiff);
    metadata.width = Some(active.width as u32);
    metadata.height = Some(active.height as u32);
    let img = RawImage {
        format: RawFormat::Cr2,
        width,
        height,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits: prec as u32,
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
pub(crate) mod tests {
    use super::*;
    use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};

    /// Build a synthetic CR2: IFD0..IFD2 placeholders + IFD3 holding a sliced 2-component LJ92 frame.
    pub(crate) fn synthetic_cr2(w: usize, h: usize, slices: Option<[u16; 3]>) -> (Vec<u8>, Vec<u16>) {
        let img: Vec<u16> = (0..w * h).map(|i| 1024 + ((i * 7919) % 12000) as u16).collect();
        let widths: Vec<usize> = match slices {
            Some([n, sw, last]) => {
                let mut v = vec![sw as usize; n as usize];
                v.push(last as usize);
                v
            }
            None => vec![w],
        };
        let mut stream = Vec::with_capacity(w * h);
        let mut x0 = 0;
        for &sw in &widths {
            for y in 0..h {
                stream.extend_from_slice(&img[y * w + x0..y * w + x0 + sw]);
            }
            x0 += sw;
        }
        let enc = crate::ljpeg::encode(&stream, w / 2, h, 2, 14, 1, 0);
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::MAKE, Value::Ascii("Canon".into()));
        ifd0.set(t::MODEL, Value::Ascii("Canon EOS Test".into()));
        ifd0.set(t::ORIENTATION, Value::Short(vec![8]));
        // maker note: SensorInfo + ColorBalance, plain IFD with offsets relative to the TIFF header
        let mut exif = IfdBuilder::new();
        exif.set(t::ISO_SPEED, Value::Short(vec![400]));
        ifd0.set_child(t::EXIF_IFD, exif);
        let mut ifd3 = IfdBuilder::new();
        ifd3.set(t::COMPRESSION, Value::Short(vec![6]));
        ifd3.set(CR2_CFA_PATTERN, Value::Long(vec![3]));
        if let Some(s) = slices {
            ifd3.set(CR2_SLICE, Value::Short(s.to_vec()));
        }
        ifd3.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![enc] });
        // (real files also carry a "CR\x02\0" signature at offset 8; probe recognises the structure without it)
        let bytes = TiffWriter::new(ByteOrder::Little, false)
            .write(&[ifd0, IfdBuilder::new().with(1, Value::Short(vec![0])), IfdBuilder::new().with(1, Value::Short(vec![0])), ifd3])
            .unwrap();
        (bytes, img)
    }

    #[test]
    fn synthetic_roundtrip() {
        for slices in [None, Some([2u16, 24, 16]), Some([1, 40, 24])] {
            let (bytes, img) = synthetic_cr2(64, 10, slices);
            assert_eq!(crate::probe(&bytes), Some(RawFormat::Cr2));
            let r = crate::decode(&bytes).unwrap();
            assert_eq!((r.width, r.height), (64, 10));
            assert_eq!(crate::probe_info(&bytes).unwrap(), r.info());
            assert_eq!(r.data, RawData::U16(img));
            assert_eq!(r.orientation, Orientation::Rotate270);
            assert_eq!(r.cfa.as_ref().map(Cfa::name).as_deref(), Some("GBRG"), "CR2CFAPattern 3");
            assert_eq!(r.metadata.iso, Some(400));
            assert!(r.develop(crate::Method::Ahd).is_ok());
        }
    }

    #[test]
    fn cfa_pattern_tag() {
        let name = |v| cfa_from_tag(v).map(|c| c.name());
        assert_eq!(name(Some(1)).as_deref(), Some("RGGB"));
        assert_eq!(name(Some(2)).as_deref(), Some("BGGR"));
        assert_eq!(name(Some(3)).as_deref(), Some("GBRG"));
        assert_eq!(name(Some(4)).as_deref(), Some("GRBG"));
        assert_eq!(name(Some(0)), None);
        assert_eq!(name(Some(7)), None);
        assert_eq!(name(None), None);
    }

    #[test]
    fn wb_probe() {
        let mut v = vec![0u64; 120];
        v[63..67].copy_from_slice(&[2000, 1024, 1026, 1500]);
        let wb = wb_from_color_balance(&v).unwrap();
        assert!((wb[0] - 2000.0 / 1025.0).abs() < 1e-4 && (wb[2] - 1500.0 / 1025.0).abs() < 1e-4);
        assert!(wb_from_color_balance(&[0; 10]).is_none());
    }
}
