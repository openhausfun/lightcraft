//! DNG (Adobe Digital Negative Specification 1.7): raw IFD selection, pixel data (via [`crate::tiffraw`]),
//! linearization, black/white levels, active area, default crop, CFA description, colour tags, opcode lists.

use crate::profile::{HsvTable, ProfileLook, ToneCurve};
use crate::tiffraw::{Packing, read_image_in};
use crate::{BlackLevel, Cfa, ColorData, Mat3, Mode, RawData, RawError, RawFormat, RawImage, Rect, Result, opcodes};
use lightcraft_color::Xy;
use lightcraft_geom::Orientation;
use lightcraft_tiff::tags::{self as t, photometric};
use lightcraft_tiff::{Ifd, Tiff};

/// The main raw IFD: full-resolution (NewSubfileType 0) CFA or LinearRaw image with the most pixels.
pub(crate) fn raw_ifd(tiff: &Tiff) -> Option<&Ifd> {
    tiff.all_ifds()
        .into_iter()
        .filter(|i| i.u32(t::NEW_SUBFILE_TYPE).unwrap_or(0) == 0)
        .filter(|i| matches!(i.u16(t::PHOTOMETRIC), Some(photometric::CFA) | Some(photometric::LINEAR_RAW)))
        .max_by_key(|i| i.u64(t::IMAGE_WIDTH).unwrap_or(0).saturating_mul(i.u64(t::IMAGE_LENGTH).unwrap_or(0)))
}

fn mat3(v: Option<Vec<f64>>) -> Option<Mat3> {
    let v = v?;
    if v.len() != 9 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let m = Mat3([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]]);
    (m.determinant().abs() > 1e-12).then_some(m)
}

fn vec3(v: Option<Vec<f64>>) -> Option<[f64; 3]> {
    let v = v?;
    (v.len() == 3 && v.iter().all(|x| x.is_finite() && *x > 0.0)).then(|| [v[0], v[1], v[2]])
}

/// Colour tags: DNG puts them in IFD0, but some writers use the raw IFD; prefer the raw IFD.
pub(crate) fn color_data(ifd0: &Ifd, raw: &Ifd) -> ColorData {
    let get = |tag: u16| raw.f64s(tag).or_else(|| ifd0.f64s(tag));
    let geti = |tag: u16| raw.u16(tag).or_else(|| ifd0.u16(tag));
    ColorData {
        illuminant: [geti(t::CALIBRATION_ILLUMINANT_1).unwrap_or(0), geti(t::CALIBRATION_ILLUMINANT_2).unwrap_or(0)],
        color_matrix: [mat3(get(t::COLOR_MATRIX_1)), mat3(get(t::COLOR_MATRIX_2))],
        forward_matrix: [mat3(get(t::FORWARD_MATRIX_1)), mat3(get(t::FORWARD_MATRIX_2))],
        camera_calibration: [mat3(get(t::CAMERA_CALIBRATION_1)), mat3(get(t::CAMERA_CALIBRATION_2))],
        analog_balance: vec3(get(t::ANALOG_BALANCE)),
        as_shot_neutral: vec3(get(t::AS_SHOT_NEUTRAL)),
        as_shot_white_xy: get(t::AS_SHOT_WHITE_XY).filter(|v| v.len() == 2 && v[0] > 0.0 && v[1] > 0.0).map(|v| Xy::new(v[0], v[1])),
        baseline_exposure: get(t::BASELINE_EXPOSURE).and_then(|v| v.first().copied()).filter(|v| v.is_finite()).unwrap_or(0.0)
            + get(t::BASELINE_EXPOSURE_OFFSET).and_then(|v| v.first().copied()).filter(|v| v.is_finite()).unwrap_or(0.0),
        profile: profile_look(ifd0, raw),
    }
}

/// The profile look tags (`ProfileHueSatMap*`, `ProfileLookTable*`, `ProfileToneCurve`); malformed
/// ones are ignored. Like the colour tags, read from the raw IFD first, else IFD 0.
pub(crate) fn profile_look(ifd0: &Ifd, raw: &Ifd) -> ProfileLook {
    let pick = |tag: u16| if raw.contains(tag) { raw } else { ifd0 };
    let table = |dims: u16, data: u16, enc: u16| {
        let ifd = pick(dims);
        let dims = ifd.u64s(dims)?;
        let data = ifd.f64s(data)?;
        HsvTable::from_tags(&dims, &data, ifd.u64s(enc).and_then(|v| v.first().copied()).unwrap_or(0))
    };
    ProfileLook {
        hue_sat_map: [
            table(t::PROFILE_HUE_SAT_MAP_DIMS, t::PROFILE_HUE_SAT_MAP_DATA_1, t::PROFILE_HUE_SAT_MAP_ENCODING),
            table(t::PROFILE_HUE_SAT_MAP_DIMS, t::PROFILE_HUE_SAT_MAP_DATA_2, t::PROFILE_HUE_SAT_MAP_ENCODING),
        ],
        look_table: table(t::PROFILE_LOOK_TABLE_DIMS, t::PROFILE_LOOK_TABLE_DATA, t::PROFILE_LOOK_TABLE_ENCODING),
        tone_curve: pick(t::PROFILE_TONE_CURVE).f64s(t::PROFILE_TONE_CURVE).and_then(|v| ToneCurve::from_tag(&v)),
    }
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let tiff = Tiff::parse(bytes)?;
    let ifd0 = &tiff.ifds[0];
    let raw = raw_ifd(&tiff).ok_or_else(|| RawError::Corrupt("DNG without a raw image IFD".into()))?;
    let info = raw.image()?;
    let (w, h, cpp) = (info.width as usize, info.height as usize, info.samples_per_pixel as usize);
    if !(1..=4).contains(&cpp) {
        return Err(RawError::Unsupported(format!("{cpp} samples per pixel")));
    }
    if info.compression == t::compression::JPEG_XL {
        return Err(RawError::Unsupported("JPEG XL DNG".into()));
    }
    let mut data = read_image_in(mode, bytes, &info, tiff.order, Packing::Msb)?;
    let bits = info.bits() as u32;

    // linearization table (integer data only)
    let mut linearized = false;
    if let (RawData::U16(v), Some(table)) = (&mut data, raw.u64s(t::LINEARIZATION_TABLE).filter(|t| !t.is_empty())) {
        let table: Vec<u16> = table.iter().map(|&x| x.min(65535) as u16).collect();
        let last = table.len() - 1;
        v.iter_mut().for_each(|s| *s = table[(*s as usize).min(last)]);
        linearized = true;
    }

    let float = matches!(data, RawData::F32(_));
    let default_white = if float { 1.0 } else { ((1u64 << bits.min(16)) - 1) as f32 };
    let mut white: Vec<f32> = raw.f64s(t::WHITE_LEVEL).unwrap_or_default().into_iter().map(|v| v as f32).filter(|v| *v > 0.0).collect();
    if white.is_empty() {
        white = vec![default_white];
    }

    let dim = raw.u64s(t::BLACK_LEVEL_REPEAT_DIM).unwrap_or_default();
    let (br, bc) = match dim.as_slice() {
        [r, c] if (1..=16).contains(r) && (1..=16).contains(c) => (*r as usize, *c as usize),
        _ => (1, 1),
    };
    let mut values: Vec<f32> = raw.f64s(t::BLACK_LEVEL).unwrap_or_default().into_iter().map(|v| v as f32).collect();
    if values.len() != br * bc * cpp {
        values = vec![values.first().copied().unwrap_or(0.0)];
    }
    let (br, bc) = if values.len() == 1 { (1, 1) } else { (br, bc) };
    let floats = |tag| raw.f64s(tag).unwrap_or_default().into_iter().map(|v| v as f32).collect::<Vec<f32>>();
    let black =
        BlackLevel { repeat_rows: br, repeat_cols: bc, values, delta_h: floats(t::BLACK_LEVEL_DELTA_H), delta_v: floats(t::BLACK_LEVEL_DELTA_V) };

    let active_area = match raw.u64s(t::ACTIVE_AREA).as_deref() {
        Some([top, left, bottom, right]) if bottom > top && right > left && (*bottom as usize) <= h && (*right as usize) <= w => {
            Rect::new(*left as usize, *top as usize, (*right - *left) as usize, (*bottom - *top) as usize)
        }
        _ => Rect::new(0, 0, w, h),
    };
    let crop = match (raw.f64s(t::DEFAULT_CROP_ORIGIN).as_deref(), raw.f64s(t::DEFAULT_CROP_SIZE).as_deref()) {
        (Some([x, y]), Some([cw, ch])) if *cw >= 1.0 && *ch >= 1.0 && *x >= 0.0 && *y >= 0.0 => {
            Rect::new(x.round() as usize, y.round() as usize, cw.round() as usize, ch.round() as usize).clipped(active_area.width, active_area.height)
        }
        _ => Rect::new(0, 0, active_area.width, active_area.height),
    };

    let cfa = if info.photometric == photometric::CFA {
        if raw.u16(t::CFA_LAYOUT).unwrap_or(1) != 1 {
            return Err(RawError::Unsupported("non-rectangular CFA layout".into()));
        }
        let dim = raw.u64s(t::CFA_REPEAT_PATTERN_DIM).unwrap_or_else(|| vec![2, 2]);
        let (rows, cols) = match dim.as_slice() {
            [r, c] if (1..=16).contains(r) && (1..=16).contains(c) => (*r as usize, *c as usize),
            _ => return Err(RawError::Corrupt("bad CFARepeatPatternDim".into())),
        };
        let pat = raw.bytes(t::CFA_PATTERN_EP).ok_or_else(|| RawError::Corrupt("missing CFAPattern".into()))?;
        if pat.len() != rows * cols {
            return Err(RawError::Corrupt("CFAPattern size mismatch".into()));
        }
        let planes = raw.bytes(t::CFA_PLANE_COLOR).map(|b| b.to_vec()).unwrap_or_else(|| vec![0, 1, 2]);
        let pattern: Vec<u8> = pat
            .iter()
            .map(|c| planes.iter().position(|p| p == c).map(|i| i as u8).filter(|&i| i < 3))
            .collect::<Option<_>>()
            .ok_or_else(|| RawError::Unsupported("CFA with more than three colours".into()))?;
        if cpp != 1 {
            return Err(RawError::Unsupported("CFA data with several samples per pixel".into()));
        }
        Some(Cfa { width: cols, height: rows, pattern })
    } else {
        None
    };

    let opcode_list = |tag| raw.bytes(tag).map(opcodes::parse_list).unwrap_or_default();
    let opcodes =
        crate::OpcodeLists { list1: opcode_list(t::OPCODE_LIST_1), list2: opcode_list(t::OPCODE_LIST_2), list3: opcode_list(t::OPCODE_LIST_3) };

    let mut metadata = lightcraft_meta::from_tiff(&tiff);
    metadata.width = Some(crop.width as u32);
    metadata.height = Some(crop.height as u32);
    let img = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp,
        data,
        cfa,
        bits,
        black,
        white,
        active_area,
        crop,
        orientation: Orientation::from_exif(ifd0.u16(t::ORIENTATION).unwrap_or(1)),
        color: color_data(ifd0, raw),
        wb_multipliers: None,
        linearized,
        opcodes,
        metadata,
    };
    img.validate_for(mode)?;
    Ok(img)
}
