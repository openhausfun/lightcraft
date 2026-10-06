//! Merge results as linear DNGs (`PhotometricInterpretation = LinearRaw`, three samples per
//! pixel), written with LightCraft's own TIFF/DNG writer.
//!
//! - Pixel values are scaled so the brightest pixel sits just below 1.0 (the white level); the
//!   scale is carried in `BaselineExposure`, so a default rendering looks like the reference frame.
//! - Raw sources: the reference frame's DNG colour tags (`ColorMatrix*`, `ForwardMatrix*`,
//!   `CameraCalibration*`, `AsShotNeutral`, illuminants) are carried over unchanged — the data is
//!   still camera RGB.
//! - Other sources: the "camera" is the file's linear RGB space; `ColorMatrix1` = XYZ(D65) → RGB,
//!   `ForwardMatrix1` = RGB → XYZ(D50), `AsShotNeutral` = (1, 1, 1).

use lightcraft_color::{D50, D65, Mat3, bradford};
use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::Rgb32f;
use lightcraft_raw::{BlackLevel, ColorData, DngCompression, DngWriteOptions, OpcodeLists, RawData, RawFormat, RawImage, Rect};

use crate::frame::FrameColor;
use crate::{MergeError, Result};

/// Sample encoding of the output DNG.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DngSamples {
    /// 16-bit floating point, Deflate + floating-point predictor (HDR).
    Half,
    /// 32-bit floating point, Deflate + floating-point predictor.
    Float,
    /// 16-bit integer, Deflate + horizontal predictor (panoramas of normal-range photos).
    U16,
}

/// The DNG colour tags for frames of `color`.
pub fn color_data(color: &FrameColor) -> ColorData {
    match color {
        FrameColor::Camera(c) => (**c).clone(),
        FrameColor::Linear { to_xyz_d50 } => {
            let to_xyz_d65 = bradford(D50, D65).mul(to_xyz_d50);
            let cm = to_xyz_d65.inverse().unwrap_or(Mat3::IDENTITY);
            ColorData {
                illuminant: [21, 0],
                color_matrix: [Some(cm), None],
                forward_matrix: [Some(*to_xyz_d50), None],
                camera_calibration: [None, None],
                analog_balance: None,
                as_shot_neutral: Some([1.0, 1.0, 1.0]),
                as_shot_white_xy: None,
                baseline_exposure: 0.0,
                profile: Default::default(),
            }
        }
    }
}

/// Encode `img` (scene-linear, 1.0 = source white) as a LinearRaw DNG.
pub fn write_linear_dng(
    img: &Rgb32f,
    color: &FrameColor,
    orientation: Orientation,
    metadata: &Metadata,
    baseline_exposure: f64,
    samples: DngSamples,
) -> Result<Vec<u8>> {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return Err(MergeError::Output("empty image".into()));
    }
    let peak = img.data.iter().flat_map(|p| p.iter()).copied().filter(|v| v.is_finite()).fold(0.0f32, f32::max);
    let target = 0.95f32;
    // never brighten: values ≤ 1 keep their scale (so a plain panorama keeps its white level)
    let scale = if peak > target { target / peak } else { 1.0 };
    let mut cd = color_data(color);
    cd.baseline_exposure = baseline_exposure + (1.0 / scale as f64).log2();
    let clean = |v: f32| if v.is_finite() { (v * scale).max(0.0) } else { 0.0 };
    let (data, bits, white) = match samples {
        DngSamples::Half | DngSamples::Float => {
            (RawData::F32(img.data.iter().flat_map(|p| p.map(clean)).collect()), if samples == DngSamples::Half { 16 } else { 32 }, 1.0)
        }
        DngSamples::U16 => {
            (RawData::U16(img.data.iter().flat_map(|p| p.map(|v| (clean(v) * 65535.0).round().min(65535.0) as u16)).collect()), 16, 65535.0)
        }
    };
    let mut meta = metadata.clone();
    meta.width = Some(w as u32);
    meta.height = Some(h as u32);
    meta.orientation = Some(orientation);
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 3,
        data,
        cfa: None,
        bits,
        black: BlackLevel::uniform(0.0),
        white: vec![white],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation,
        color: cd,
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata: meta,
    };
    let compression = DngCompression::Deflate { tile: 256, half: samples == DngSamples::Half };
    lightcraft_raw::write_dng(&raw, &DngWriteOptions { compression, ..Default::default() }).map_err(|e| MergeError::Output(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_dng_round_trips_through_the_raw_decoder() {
        let img = Rgb32f::from_fn(80, 60, |x, y| [x as f32 / 20.0, y as f32 / 30.0, 0.3]);
        let color = FrameColor::Linear { to_xyz_d50: bradford(D65, D50).mul(&lightcraft_color::SRGB.to_xyz()) };
        let bytes = write_linear_dng(&img, &color, Orientation::Normal, &Metadata::default(), 0.0, DngSamples::Half).unwrap();
        let raw = lightcraft_raw::decode(&bytes).unwrap();
        assert_eq!((raw.width, raw.height, raw.cpp), (80, 60, 3));
        let gain = 2f32.powf(raw.color.baseline_exposure as f32);
        let back = raw.develop(lightcraft_raw::Method::Bilinear).unwrap();
        for (a, b) in img.data.iter().zip(&back.data) {
            for c in 0..3 {
                assert!((a[c] - b[c] * gain).abs() <= a[c] * 2e-3 + 1e-5, "{a:?} {b:?}");
            }
        }
        // the colour model maps the source white to neutral (no white-balance shift)
        let xy = lightcraft_raw::color::as_shot_white_xy(&raw);
        assert!((xy.x - D65.x).abs() < 2e-3 && (xy.y - D65.y).abs() < 2e-3, "{xy:?}");
        let t = lightcraft_raw::color::camera_transform(&raw, xy);
        let white = t.matrix.apply([t.wb[0] as f64, t.wb[1] as f64, t.wb[2] as f64]);
        assert!(white.iter().all(|v| (v - white[0]).abs() < 1e-3), "{white:?}");
        // sRGB red maps to the Rec.2020 coordinates of sRGB red
        let red = t.matrix.apply([t.wb[0] as f64, 0.0, 0.0]);
        let want = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).apply([1.0, 0.0, 0.0]);
        let k = white[0];
        for c in 0..3 {
            assert!((red[c] / k - want[c]).abs() < 5e-3, "{red:?} vs {want:?}");
        }
        let u16 = write_linear_dng(&img, &color, Orientation::Normal, &Metadata::default(), 0.0, DngSamples::U16).unwrap();
        assert!(lightcraft_raw::decode(&u16).is_ok());
    }
}
