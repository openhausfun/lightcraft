//! Colour science for LightCraft.
//!
//! - [`Mat3`] and RGB colour spaces defined by primaries + white point ([`RgbSpace`]).
//! - Chromatic adaptation (Bradford), correlated colour temperature ↔ chromaticity, and the
//!   temperature/tint white-balance model ([`cct`]).
//! - Transfer functions (sRGB, gamma, PQ, HLG) in [`transfer`].
//! - Perceptual spaces: OkLab / OkLCh, HSL/HSV in [`perceptual`].
//! - Tone-curve splines in [`spline`].
//!
//! The pipeline's working space is **linear Rec.2020, D65** ([`WORKING`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod cct;
pub mod perceptual;
pub mod spline;
pub mod transfer;

use serde::{Deserialize, Serialize};

/// Row-major 3×3 matrix.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn diag(a: f64, b: f64, c: f64) -> Mat3 {
        Mat3([[a, 0.0, 0.0], [0.0, b, 0.0], [0.0, 0.0, c]])
    }
    pub fn mul(&self, o: &Mat3) -> Mat3 {
        let mut r = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                r[i][j] = (0..3).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Mat3(r)
    }
    pub fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }
    pub fn apply_f32(&self, v: [f32; 3]) -> [f32; 3] {
        let r = self.apply([v[0] as f64, v[1] as f64, v[2] as f64]);
        [r[0] as f32, r[1] as f32, r[2] as f32]
    }
    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }
    pub fn inverse(&self) -> Option<Mat3> {
        let d = self.determinant();
        if d.abs() < 1e-300 {
            return None;
        }
        let m = &self.0;
        let i = 1.0 / d;
        Some(Mat3([
            [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) * i, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * i, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * i],
            [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) * i, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * i, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * i],
            [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) * i, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * i, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * i],
        ]))
    }
    pub fn transpose(&self) -> Mat3 {
        let m = &self.0;
        Mat3([[m[0][0], m[1][0], m[2][0]], [m[0][1], m[1][1], m[2][1]], [m[0][2], m[1][2], m[2][2]]])
    }
    /// As f32 (row-major), for kernels.
    pub fn to_f32(&self) -> [[f32; 3]; 3] {
        self.0.map(|r| r.map(|v| v as f32))
    }
}

/// CIE xy chromaticity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

impl Xy {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    /// XYZ with Y = 1.
    pub fn to_xyz(self) -> [f64; 3] {
        [self.x / self.y, 1.0, (1.0 - self.x - self.y) / self.y]
    }
    pub fn from_xyz(v: [f64; 3]) -> Xy {
        let s = v[0] + v[1] + v[2];
        if s <= 0.0 { D65 } else { Xy::new(v[0] / s, v[1] / s) }
    }
}

pub const D65: Xy = Xy::new(0.3127, 0.3290);
pub const D50: Xy = Xy::new(0.3457, 0.3585);

/// An RGB colour space: primaries and white point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RgbSpace {
    pub name: &'static str,
    pub r: Xy,
    pub g: Xy,
    pub b: Xy,
    pub white: Xy,
}

pub const SRGB: RgbSpace = RgbSpace { name: "sRGB", r: Xy::new(0.64, 0.33), g: Xy::new(0.30, 0.60), b: Xy::new(0.15, 0.06), white: D65 };
pub const DISPLAY_P3: RgbSpace =
    RgbSpace { name: "Display P3", r: Xy::new(0.680, 0.320), g: Xy::new(0.265, 0.690), b: Xy::new(0.150, 0.060), white: D65 };
pub const REC2020: RgbSpace = RgbSpace { name: "Rec.2020", r: Xy::new(0.708, 0.292), g: Xy::new(0.170, 0.797), b: Xy::new(0.131, 0.046), white: D65 };
pub const PROPHOTO: RgbSpace =
    RgbSpace { name: "ProPhoto RGB", r: Xy::new(0.734699, 0.265301), g: Xy::new(0.159597, 0.840403), b: Xy::new(0.036598, 0.000105), white: D50 };
/// The 1998 "Adobe RGB" primaries (a public colour-space definition; name used descriptively).
pub const ADOBE_RGB: RgbSpace =
    RgbSpace { name: "Adobe RGB (1998) compatible", r: Xy::new(0.64, 0.33), g: Xy::new(0.21, 0.71), b: Xy::new(0.15, 0.06), white: D65 };

/// The develop pipeline's working space: linear Rec.2020 / D65.
pub const WORKING: RgbSpace = REC2020;

impl RgbSpace {
    /// Matrix taking linear RGB in this space to XYZ (white Y = 1) relative to its own white.
    /// Degenerate (collinear) primaries, which readers reject before building a space from a
    /// file, leave the primaries unscaled instead of panicking.
    pub fn to_xyz(&self) -> Mat3 {
        let p = [self.r.to_xyz(), self.g.to_xyz(), self.b.to_xyz()];
        let m = Mat3([[p[0][0], p[1][0], p[2][0]], [p[0][1], p[1][1], p[2][1]], [p[0][2], p[1][2], p[2][2]]]);
        let s = m.inverse().map_or([1.0; 3], |inv| inv.apply(self.white.to_xyz()));
        m.mul(&Mat3::diag(s[0], s[1], s[2]))
    }
    /// Inverse of [`to_xyz`](Self::to_xyz); the identity for degenerate primaries.
    pub fn from_xyz(&self) -> Mat3 {
        self.to_xyz().inverse().unwrap_or(Mat3::IDENTITY)
    }
    /// Luminance weights (the Y row of `to_xyz`).
    pub fn luma(&self) -> [f64; 3] {
        self.to_xyz().0[1]
    }
    /// Linear RGB in `self` → linear RGB in `dst`, Bradford-adapting between white points.
    pub fn to_space(&self, dst: &RgbSpace) -> Mat3 {
        let adapt = bradford(self.white, dst.white);
        dst.from_xyz().mul(&adapt).mul(&self.to_xyz())
    }
}

const BRADFORD: Mat3 = Mat3([[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]]);

/// Bradford chromatic adaptation matrix (XYZ → XYZ) from white `src` to white `dst`.
pub fn bradford(src: Xy, dst: Xy) -> Mat3 {
    if src == dst {
        return Mat3::IDENTITY;
    }
    let s = BRADFORD.apply(src.to_xyz());
    let d = BRADFORD.apply(dst.to_xyz());
    // a constant, invertible matrix: the fallback (no adaptation) can't happen
    let Some(inv) = BRADFORD.inverse() else { return Mat3::IDENTITY };
    inv.mul(&Mat3::diag(d[0] / s[0], d[1] / s[1], d[2] / s[2])).mul(&BRADFORD)
}

/// Rec.2020 luminance weights as f32 (hot path).
pub const LUMA_2020: [f32; 3] = [0.2627, 0.6780, 0.0593];
/// Rec.709 / sRGB luminance weights.
pub const LUMA_709: [f32; 3] = [0.2126, 0.7152, 0.0722];

#[inline]
pub fn luminance_2020(c: [f32; 3]) -> f32 {
    c[0] * LUMA_2020[0] + c[1] * LUMA_2020[1] + c[2] * LUMA_2020[2]
}

/// An sRGB 8-bit colour, for UI and swatches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb8(pub u8, pub u8, pub u8);

impl Rgb8 {
    pub fn from_hex(s: &str) -> Option<Rgb8> {
        let s = s.trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb8((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_matrix_matches_standard() {
        let m = SRGB.to_xyz();
        let want = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m.0[i][j] - want[i][j]).abs() < 2e-4, "{i}{j} {}", m.0[i][j]);
            }
        }
    }

    #[test]
    fn rec2020_luma() {
        let l = REC2020.luma();
        assert!((l[0] - 0.2627).abs() < 1e-3 && (l[1] - 0.6780).abs() < 1e-3 && (l[2] - 0.0593).abs() < 1e-3);
    }

    #[test]
    fn white_maps_to_white() {
        for sp in [SRGB, DISPLAY_P3, REC2020, PROPHOTO, ADOBE_RGB] {
            let w = sp.to_space(&REC2020).apply([1.0, 1.0, 1.0]);
            for c in w {
                assert!((c - 1.0).abs() < 1e-3, "{} {:?}", sp.name, w);
            }
        }
    }

    #[test]
    fn space_roundtrip() {
        let a = SRGB.to_space(&PROPHOTO);
        let b = PROPHOTO.to_space(&SRGB);
        let v = b.apply(a.apply([0.2, 0.5, 0.9]));
        assert!((v[0] - 0.2).abs() < 1e-9 && (v[1] - 0.5).abs() < 1e-9 && (v[2] - 0.9).abs() < 1e-9);
    }

    #[test]
    fn hex() {
        assert_eq!(Rgb8::from_hex("#1e1e1e"), Some(Rgb8(30, 30, 30)));
        assert_eq!(Rgb8(1, 2, 255).to_hex(), "#0102ff");
        assert_eq!(Rgb8::from_hex("xyz"), None);
    }
}
