//! Perceptual colour models: OkLab / OkLCh (Björn Ottosson, 2020), HSL and HSV.

use std::sync::OnceLock;

use crate::{Mat3, REC2020};

const XYZ_TO_LMS: Mat3 = Mat3([
    [0.818_933_010_1, 0.361_866_742_4, -0.128_859_713_7],
    [0.032_984_543_6, 0.929_311_871_5, 0.036_145_638_7],
    [0.048_200_301_8, 0.264_366_269_1, 0.633_851_707_0],
]);
const LMS_TO_LAB: Mat3 = Mat3([
    [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
    [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
    [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
]);

struct OkMats {
    to_lms: [[f32; 3]; 3],
    from_lms: [[f32; 3]; 3],
    to_lab: [[f32; 3]; 3],
    from_lab: [[f32; 3]; 3],
}

fn mats() -> &'static OkMats {
    static M: OnceLock<OkMats> = OnceLock::new();
    M.get_or_init(|| {
        let to_lms = XYZ_TO_LMS.mul(&REC2020.to_xyz());
        OkMats {
            to_lms: to_lms.to_f32(),
            // constant, invertible matrices (round trips tested below): the identity fallback can't happen
            from_lms: to_lms.inverse().unwrap_or(Mat3::IDENTITY).to_f32(),
            to_lab: LMS_TO_LAB.to_f32(),
            from_lab: LMS_TO_LAB.inverse().unwrap_or(Mat3::IDENTITY).to_f32(),
        }
    })
}

#[inline]
fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// The f32 matrices [`oklab_from_2020`] and [`oklab_to_2020`] use (row-major): Rec.2020 → LMS,
/// LMS → Rec.2020, cube-rooted LMS → Lab, Lab → cube-rooted LMS. GPU kernels embed these.
pub fn oklab_matrices() -> [[[f32; 3]; 3]; 4] {
    let m = mats();
    [m.to_lms, m.from_lms, m.to_lab, m.from_lab]
}

/// Linear Rec.2020 → OkLab `[L, a, b]`.
#[inline]
pub fn oklab_from_2020(rgb: [f32; 3]) -> [f32; 3] {
    let m = mats();
    let lms = mul(&m.to_lms, rgb).map(f32::cbrt);
    mul(&m.to_lab, lms)
}

/// OkLab → linear Rec.2020.
#[inline]
pub fn oklab_to_2020(lab: [f32; 3]) -> [f32; 3] {
    let m = mats();
    let lms = mul(&m.from_lab, lab).map(|v| v * v * v);
    mul(&m.from_lms, lms)
}

/// OkLab → OkLCh (hue in radians, −π..π).
#[inline]
pub fn lab_to_lch(lab: [f32; 3]) -> [f32; 3] {
    [lab[0], lab[1].hypot(lab[2]), lab[2].atan2(lab[1])]
}

#[inline]
pub fn lch_to_lab(lch: [f32; 3]) -> [f32; 3] {
    let (s, c) = lch[2].sin_cos();
    [lch[0], lch[1] * c, lch[1] * s]
}

/// RGB (0..1) → HSV (h in degrees 0..360, s, v 0..1).
pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let h = hue_of(c, max, d);
    [h, if max > 0.0 { d / max } else { 0.0 }, max]
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// RGB (0..1) → HSL (h degrees, s, l).
pub fn rgb_to_hsl(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let l = (max + min) / 2.0;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-9) };
    [hue_of(c, max, d), s, l]
}

pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let v = l + c / 2.0;
    hsv_to_rgb(h, if v > 0.0 { c / v } else { 0.0 }, v)
}

fn hue_of(c: [f32; 3], max: f32, d: f32) -> f32 {
    if d == 0.0 {
        return 0.0;
    }
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    h * 60.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_white_and_grey() {
        let w = oklab_from_2020([1.0, 1.0, 1.0]);
        assert!((w[0] - 1.0).abs() < 2e-3 && w[1].abs() < 2e-3 && w[2].abs() < 2e-3, "{w:?}");
        let g = oklab_from_2020([0.18, 0.18, 0.18]);
        assert!(g[1].abs() < 2e-3 && g[2].abs() < 2e-3);
    }

    #[test]
    fn oklab_roundtrip() {
        for c in [[0.1, 0.5, 0.9], [1.0, 0.0, 0.0], [0.02, 0.03, 0.01], [2.0, 1.5, 0.3]] {
            let back = oklab_to_2020(oklab_from_2020(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1e-4 * c[i].max(1.0), "{c:?} {back:?}");
            }
        }
        let lch = lab_to_lch([0.5, 0.1, -0.05]);
        let lab = lch_to_lab(lch);
        assert!((lab[1] - 0.1).abs() < 1e-6 && (lab[2] + 0.05).abs() < 1e-6);
    }

    #[test]
    fn hsv_hsl_roundtrip() {
        for c in [[0.2, 0.4, 0.6], [1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.9, 0.8, 0.1]] {
            let h = rgb_to_hsv(c);
            let r = hsv_to_rgb(h[0], h[1], h[2]);
            let l = rgb_to_hsl(c);
            let r2 = hsl_to_rgb(l[0], l[1], l[2]);
            for i in 0..3 {
                assert!((r[i] - c[i]).abs() < 1e-5 && (r2[i] - c[i]).abs() < 1e-5, "{c:?}");
            }
        }
        assert_eq!(rgb_to_hsv([0.0, 1.0, 0.0])[0], 120.0);
    }
}
