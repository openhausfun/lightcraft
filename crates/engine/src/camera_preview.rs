//! Estimate an ARW starting look from its own JPEG. Colour and luminance are fitted separately;
//! the JPEG supplies correspondences only, never output pixels or a replacement for RAW editing.
use lightcraft_color::{Mat3, luminance_2020};
use lightcraft_pipeline::tone::{CameraTone, ToneMap};
use lightcraft_raster::{
    Rgb32f,
    resample::{Filter, fit},
};
use lightcraft_raw::{RawImage, color::CameraTransform};

#[derive(Clone, Copy, Debug)]
pub(crate) struct CameraLook {
    pub matrix: Mat3,
    pub tone: CameraTone,
}

pub(crate) fn fit_preview(raw: &RawImage, bytes: &[u8], transform: &CameraTransform) -> Option<CameraLook> {
    if !transform.matrix_is_fallback || raw.format != lightcraft_raw::RawFormat::Arw {
        return None;
    }
    let jpeg = lightcraft_raw::embedded_preview(bytes)?;
    let decoded = lightcraft_codecs::decode(&jpeg, lightcraft_codecs::DecodeOptions { max_size: Some((384, 384)), max_pixels: 64_000_000 }).ok()?;
    let reference = decoded.to_working();
    let crop = raw.crop.clipped(raw.active_area.width, raw.active_area.height);
    if crop.width == 0 || crop.height == 0 || reference.width == 0 || reference.height == 0 {
        return None;
    }
    let aspect = crop.width as f64 / crop.height as f64;
    if (reference.width as f64 / reference.height as f64 / aspect - 1.0).abs() > 0.02 {
        return None;
    }
    // Fixed, bounded proxy: the selected look cannot depend on thumbnail/export resolution.
    let k = (crop.width.max(crop.height).div_ceil(384).max(2)).div_ceil(2) * 2;
    let sensor = raw.develop_binned(k, 0.99).ok()??;
    let mut sensor = fit(&sensor, 96, 96, Filter::Box);
    let reference = fit(&reference, sensor.width, sensor.height, Filter::Box);
    let gain = 2f32.powf(transform.baseline_exposure as f32);
    sensor.map_in_place(|p| transform.matrix.apply_f32(std::array::from_fn(|i| p[i] * transform.wb[i] * gain)));
    let look = fit_pairs(&sensor, &reference)?;
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] ARW camera look: {:?}, {:?}", look.matrix.0, look.tone);
    }
    Some(look)
}

/// A fit must cut the held-out squared error to below this share of the fallback's.
const MIN_IMPROVEMENT: f64 = 0.7;
/// ...and stay within this per-channel RMS of the camera JPEG (linear display values). On public
/// raw.pixls.us samples (eight Sony bodies) good fits that still beat the fallback 1.5–3× landed
/// at 0.065–0.093 (camera local tone, vignetting and lens processing that a global matrix + curve
/// cannot follow): visibly better renders that 0.055 rejected.
const MAX_HOLDOUT_RMS: f64 = 0.10;

fn luma(p: [f64; 3]) -> f64 {
    p[0] * 0.2627 + p[1] * 0.6780 + p[2] * 0.0593
}

fn displayed(scene: [f64; 3], tone: &ToneMap) -> [f64; 3] {
    let scene = scene.map(|v| v.max(0.0));
    let y = luma(scene);
    if y <= 0.0 {
        return [0.0; 3];
    }
    scene.map(|v| v * f64::from(tone.apply(y as f32)) / y)
}

fn fit_pairs(sensor: &Rgb32f, reference: &Rgb32f) -> Option<CameraLook> {
    if (sensor.width, sensor.height) != (reference.width, reference.height) || sensor.data.len() != reference.data.len() {
        return None;
    }
    let mut pairs = Vec::new();
    let mut colour = 0;
    for (input, output) in sensor.data.iter().zip(&reference.data) {
        let y = luminance_2020(*output);
        if !input.iter().all(|v| v.is_finite() && *v > 0.001 && *v < 1.5)
            || !output.iter().all(|v| v.is_finite() && *v > 0.004 && *v < 0.98)
            || !(0.015..0.85).contains(&y)
        {
            continue;
        }
        let min = output.iter().copied().fold(f32::INFINITY, f32::min);
        let max = output.iter().copied().fold(0.0, f32::max);
        colour += usize::from(max - min > 0.05);
        pairs.push((input.map(f64::from), output.map(f64::from)));
    }
    if pairs.len() < 256 || colour < pairs.len() / 20 {
        return None;
    }
    let mut gram = [[0.0; 3]; 3];
    let mut cross = [[0.0; 3]; 3];
    for (i, (x, y)) in pairs.iter().enumerate() {
        if i % 3 == 0 {
            continue;
        }
        let (lx, ly) = (luma(*x), luma(*y));
        for row in 0..3 {
            for col in 0..3 {
                // Normalising by luminance prevents a camera S-curve from corrupting colour.
                gram[row][col] += x[row] * x[col] / (lx * lx);
                cross[row][col] += y[row] * x[col] / (ly * lx);
            }
        }
    }
    let trace: f64 = (0..3).map(|i| gram[i][i]).sum();
    let inverse = Mat3(gram).inverse()?;
    let condition = trace * (0..3).map(|i| inverse.0[i][i].abs()).sum::<f64>();
    if trace <= 0.0 || !condition.is_finite() || condition > 1e6 {
        return None;
    }
    let regularization = trace * 1e-4;
    for i in 0..3 {
        gram[i][i] += regularization;
        cross[i][i] += regularization;
    }
    let matrix = Mat3(cross).mul(&Mat3(gram).inverse()?);
    if !matrix.0.iter().flatten().all(|v| v.is_finite() && v.abs() < 8.0) {
        return None;
    }
    let tone_pairs: Vec<_> =
        pairs.iter().enumerate().filter(|(i, _)| i % 3 != 0).map(|(_, (x, y))| (luma(matrix.apply(*x).map(|v| v.max(0.0))), luma(*y))).collect();
    let curve = fit_tone(tone_pairs)?;
    let tone = ToneMap::camera(&curve, 0.0, 0.0, 0.0);
    let original_tone = ToneMap::new(0.0, 0.0, 0.0);
    let mut before = 0.0;
    let mut after = 0.0;
    let mut samples = 0;
    for (i, (x, target)) in pairs.iter().enumerate() {
        if i % 3 != 0 {
            continue;
        }
        let corrected = displayed(matrix.apply(*x), &tone);
        let original = displayed(*x, &original_tone);
        for c in 0..3 {
            before += (original[c] - target[c]).powi(2);
            after += (corrected[c] - target[c]).powi(2);
            samples += 1;
        }
    }
    if lightcraft_pipeline::profiling() {
        eprintln!("[profile] ARW holdout RMS {:.5} -> {:.5} ({samples} channels)", (before / samples as f64).sqrt(), (after / samples as f64).sqrt());
    }
    if !after.is_finite() || samples == 0 || after >= before * MIN_IMPROVEMENT || after / samples as f64 > MAX_HOLDOUT_RMS.powi(2) {
        return None;
    }
    Some(CameraLook { matrix, tone: curve })
}

fn median(values: &mut [f64]) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

fn fit_tone(mut pairs: Vec<(f64, f64)>) -> Option<CameraTone> {
    if pairs.len() < 128 || !pairs.iter().all(|(x, y)| x.is_finite() && *x > 0.0 && y.is_finite()) {
        return None;
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut knots = [[0.0; 2]; 32];
    for (i, knot) in knots.iter_mut().enumerate() {
        let bin = pairs.get(i * pairs.len() / 32..(i + 1) * pairs.len() / 32)?;
        let mut xs: Vec<_> = bin.iter().map(|p| p.0).collect();
        let mut ys: Vec<_> = bin.iter().map(|p| p.1).collect();
        *knot = [median(&mut xs)? as f32, median(&mut ys)? as f32];
    }
    if knots[31][0] < knots[0][0] * 1.5 {
        return None;
    }
    // Pool adjacent violating bins (isotonic regression): no reversals or arbitrary polynomial.
    let mut blocks: Vec<(f32, usize)> = Vec::new();
    for knot in knots {
        blocks.push((knot[1], 1));
        while blocks.len() >= 2 {
            let (a, an) = *blocks.get(blocks.len() - 2)?;
            let (b, bn) = *blocks.last()?;
            if a <= b {
                break;
            }
            blocks.truncate(blocks.len() - 2);
            blocks.push(((a * an as f32 + b * bn as f32) / (an + bn) as f32, an + bn));
        }
    }
    let mut i = 0;
    for (y, n) in blocks {
        for knot in knots.get_mut(i..i + n)? {
            knot[1] = y;
        }
        i += n;
    }
    CameraTone::new(knots)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn separates_nonlinear_tone_from_colour_and_keeps_sensor_headroom() {
        let known = Mat3([[1.8, -0.4, -0.1], [-0.2, 1.5, -0.1], [-0.05, -0.3, 1.7]]);
        let mut sensor = Rgb32f::new(64, 64);
        let mut reference = sensor.clone();
        for (i, (src, dst)) in sensor.data.iter_mut().zip(&mut reference.data).enumerate() {
            let ev = 0.05 + (i % 31) as f32 * 0.017;
            *src = [ev * (0.8 + (i % 11) as f32 * 0.025), ev, ev * (0.8 + (i % 17) as f32 * 0.014)];
            let p = known.apply_f32(*src);
            let y = luminance_2020(p);
            *dst = p.map(|v| v * (1.0 - (-2.5 * y).exp()) / y);
        }
        let original = sensor.clone();
        let fit = fit_pairs(&sensor, &reference).unwrap();
        assert_eq!(sensor.data, original.data);
        let tone = ToneMap::camera(&fit.tone, 0.0, 0.0, 0.0);
        let error: f64 = sensor
            .data
            .iter()
            .zip(&reference.data)
            .map(|(x, y)| {
                let p = displayed(fit.matrix.apply(x.map(f64::from)), &tone);
                (0..3).map(|c| (p[c] - f64::from(y[c])).powi(2)).sum::<f64>() / 3.0
            })
            .sum::<f64>()
            / sensor.data.len() as f64;
        assert!(error.sqrt() < 0.025, "{error}");
        // The colour transform is homogeneous; tone mapping happens only after exposure.
        let p = fit.matrix.apply([2.0, 2.0, 2.0]);
        assert!(luma(p) > 1.0);
        assert!(tone.apply(0.2) < tone.apply(0.4));
    }
    #[test]
    fn accepts_a_much_better_fit_despite_local_camera_processing() {
        // The camera JPEG departs from any global matrix + curve (local tone, vignetting): ±0.12
        // per-pixel deviations, ~0.07 RMS. The fit is still far closer than the fallback.
        let known = Mat3([[1.8, -0.4, -0.1], [-0.2, 1.5, -0.1], [-0.05, -0.3, 1.7]]);
        let mut sensor = Rgb32f::new(64, 64);
        let mut reference = sensor.clone();
        let mut seed = 0x2545_f491_u32;
        for (i, (src, dst)) in sensor.data.iter_mut().zip(&mut reference.data).enumerate() {
            let ev = 0.05 + (i % 31) as f32 * 0.017;
            *src = [ev * (0.8 + (i % 11) as f32 * 0.025), ev, ev * (0.8 + (i % 17) as f32 * 0.014)];
            let p = known.apply_f32(*src);
            let y = luminance_2020(p);
            *dst = p.map(|v| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = (seed % 2001) as f32 / 1000.0 - 1.0;
                (v * (1.0 - (-2.5 * y).exp()) / y + 0.12 * noise).clamp(0.005, 0.97)
            });
        }
        assert!(fit_pairs(&sensor, &reference).is_some());
    }

    #[test]
    fn rejects_monochrome_invalid_and_unrelated_previews() {
        let mut sensor = Rgb32f::new(32, 32);
        let mut reference = sensor.clone();
        for (i, p) in sensor.data.iter_mut().enumerate() {
            *p = [0.1 + (i % 13) as f32 * 0.02, 0.15, 0.1];
        }
        reference.data.fill([0.2; 3]);
        assert!(fit_pairs(&sensor, &reference).is_none());
        reference.data.fill([f32::NAN; 3]);
        assert!(fit_pairs(&sensor, &reference).is_none());
        for (i, (src, dst)) in sensor.data.iter_mut().zip(&mut reference.data).enumerate() {
            *src = [0.04 + (i % 11) as f32 * 0.02, 0.05 + (i % 17) as f32 * 0.01, 0.03 + (i % 23) as f32 * 0.01];
            *dst = [0.05 + (i % 7) as f32 * 0.07, 0.05 + (i % 19) as f32 * 0.02, 0.05 + (i % 29) as f32 * 0.01];
        }
        assert!(fit_pairs(&sensor, &reference).is_none());
        sensor.data.fill([0.1, 0.15, 0.12]);
        assert!(fit_pairs(&sensor, &reference).is_none());
        reference.data.truncate(8);
        assert!(fit_pairs(&sensor, &reference).is_none());
    }
}
