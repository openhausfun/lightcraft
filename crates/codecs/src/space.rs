//! Source colour spaces, transfer curves and the conversion to the working space.

use lightcraft_color::{ADOBE_RGB, D50, DISPLAY_P3, Mat3, PROPHOTO, REC2020, RgbSpace, SRGB, WORKING, bradford};
use serde::{Deserialize, Serialize};

/// A transfer curve (encoded value → linear light), as found in ICC `curv`/`para` tags and container
/// colour descriptions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Trc {
    /// Identity (already linear).
    Linear,
    /// IEC 61966-2-1 sRGB curve (also used by Display P3).
    Srgb,
    /// Pure power law `y = x^g`.
    Gamma(f32),
    /// ITU-R BT.709 / BT.2020 OETF inverse.
    Rec709,
    /// ICC parametric curve parameters `[g, a, b, c, d, e, f]` (1, 3, 4, 5 or 7 values; ICC function types 0–4).
    Parametric(Vec<f32>),
    /// Sampled curve over `[0, 1]` (ICC `curv` with ≥ 2 entries), linearly interpolated.
    Table(Vec<u16>),
}

impl Trc {
    /// Encoded value → linear. Negative values are mirrored and values above 1 are extrapolated,
    /// so extended-range float data survives.
    pub fn to_linear(&self, v: f32) -> f32 {
        if v < 0.0 {
            return -self.to_linear(-v);
        }
        match self {
            Trc::Linear => v,
            Trc::Srgb => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Trc::Gamma(g) => v.powf(*g),
            Trc::Rec709 => {
                if v < 0.081 {
                    v / 4.5
                } else {
                    ((v + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
            Trc::Parametric(p) => eval_parametric(p, v),
            Trc::Table(t) => eval_table(t, v),
        }
    }

    /// Linear → encoded value (inverse of [`Trc::to_linear`]); tables are inverted numerically.
    pub fn from_linear(&self, v: f32) -> f32 {
        if v < 0.0 {
            return -self.from_linear(-v);
        }
        match self {
            Trc::Linear => v,
            Trc::Srgb => lightcraft_color::transfer::linear_to_srgb(v),
            Trc::Gamma(g) => {
                if *g > 0.0 {
                    v.powf(1.0 / g)
                } else {
                    v
                }
            }
            Trc::Rec709 => {
                if v < 0.018 {
                    v * 4.5
                } else {
                    1.099 * v.powf(0.45) - 0.099
                }
            }
            Trc::Parametric(_) | Trc::Table(_) => {
                // Monotone curves: bisection on [0, max(1, v)].
                let (mut lo, mut hi) = (0.0f32, 1.0f32);
                while self.to_linear(hi) < v && hi < 1e6 {
                    hi *= 2.0;
                }
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    if self.to_linear(mid) < v {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                0.5 * (lo + hi)
            }
        }
    }

    /// True for the identity curve (including degenerate ICC encodings of it).
    pub fn is_linear(&self) -> bool {
        match self {
            Trc::Linear => true,
            Trc::Gamma(g) => (*g - 1.0).abs() < 1e-4,
            Trc::Parametric(p) => p.len() == 1 && (p[0] - 1.0).abs() < 1e-4,
            Trc::Table(t) => t.is_empty(),
            _ => false,
        }
    }

    /// Whether two curves agree to within `tol` on a sample grid.
    pub fn approx_eq(&self, other: &Trc, tol: f32) -> bool {
        (0..=32).all(|i| {
            let x = i as f32 / 32.0;
            (self.to_linear(x) - other.to_linear(x)).abs() <= tol
        })
    }

    /// 256-entry linearization table for 8-bit data.
    pub(crate) fn lut8(&self) -> Vec<f32> {
        (0..256).map(|i| self.to_linear(i as f32 / 255.0)).collect()
    }

    /// 65536-entry linearization table for 16-bit data.
    pub(crate) fn lut16(&self) -> Vec<f32> {
        if self.is_linear() {
            return (0..65536).map(|i| i as f32 / 65535.0).collect();
        }
        (0..65536).map(|i| self.to_linear(i as f32 / 65535.0)).collect()
    }
}

fn eval_parametric(p: &[f32], x: f32) -> f32 {
    let pw = |b: f32, g: f32| if b > 0.0 { b.powf(g) } else { 0.0 };
    match p.len() {
        1 => pw(x, p[0]),
        3 => {
            let (g, a, b) = (p[0], p[1], p[2]);
            if a != 0.0 && x >= -b / a { pw(a * x + b, g) } else { 0.0 }
        }
        4 => {
            let (g, a, b, c) = (p[0], p[1], p[2], p[3]);
            if a != 0.0 && x >= -b / a { pw(a * x + b, g) + c } else { c }
        }
        5 => {
            let (g, a, b, c, d) = (p[0], p[1], p[2], p[3], p[4]);
            if x >= d { pw(a * x + b, g) } else { c * x }
        }
        7 => {
            let (g, a, b, c, d, e, f) = (p[0], p[1], p[2], p[3], p[4], p[5], p[6]);
            if x >= d { pw(a * x + b, g) + e } else { c * x + f }
        }
        _ => x,
    }
}

fn eval_table(t: &[u16], x: f32) -> f32 {
    match t.len() {
        0 => x,
        1 => x.powf(t[0] as f32 / 256.0),
        n => {
            let pos = x * (n - 1) as f32;
            let i = (pos.floor() as usize).min(n - 2);
            let f = pos - i as f32;
            let a = t[i] as f32 / 65535.0;
            let b = t[i + 1] as f32 / 65535.0;
            a + (b - a) * f
        }
    }
}

/// Well-known RGB spaces we recognise (from ICC colorants or container tags).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NamedSpace {
    Srgb,
    DisplayP3,
    AdobeRgb,
    ProPhoto,
    Rec2020,
}

impl NamedSpace {
    pub const ALL: [NamedSpace; 5] = [NamedSpace::Srgb, NamedSpace::DisplayP3, NamedSpace::AdobeRgb, NamedSpace::ProPhoto, NamedSpace::Rec2020];

    pub fn rgb_space(self) -> RgbSpace {
        match self {
            NamedSpace::Srgb => SRGB,
            NamedSpace::DisplayP3 => DISPLAY_P3,
            NamedSpace::AdobeRgb => ADOBE_RGB,
            NamedSpace::ProPhoto => PROPHOTO,
            NamedSpace::Rec2020 => REC2020,
        }
    }

    /// The space's standard encoding curve.
    pub fn trc(self) -> Trc {
        match self {
            NamedSpace::Srgb | NamedSpace::DisplayP3 => Trc::Srgb,
            NamedSpace::AdobeRgb => Trc::Gamma(563.0 / 256.0),
            NamedSpace::ProPhoto => Trc::Gamma(1.8),
            NamedSpace::Rec2020 => Trc::Rec709,
        }
    }

    /// Linear RGB → XYZ (D50 PCS, Bradford-adapted), as an ICC matrix/TRC profile would store it.
    pub fn to_xyz_d50(self) -> Mat3 {
        rgb_to_xyz_d50(&self.rgb_space())
    }

    pub fn name(self) -> &'static str {
        self.rgb_space().name
    }

    /// Recognise a space from its D50-adapted colorant matrix (tolerant to s15Fixed16 rounding and
    /// the small variations between vendors' profiles).
    pub fn recognize(to_xyz_d50: &Mat3) -> Option<NamedSpace> {
        NamedSpace::ALL.into_iter().find(|n| {
            let m = n.to_xyz_d50();
            (0..3).all(|r| (0..3).all(|c| (m.0[r][c] - to_xyz_d50.0[r][c]).abs() < 0.004))
        })
    }
}

/// `space`'s linear RGB → XYZ relative to D50 (Bradford), the ICC PCS.
pub fn rgb_to_xyz_d50(space: &RgbSpace) -> Mat3 {
    bradford(space.white, D50).mul(&space.to_xyz())
}

/// Where a decoded image's colour interpretation came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpaceOrigin {
    /// No colour information in the file: assumed sRGB.
    Untagged,
    /// A container-level description (PNG `sRGB`/`cHRM`/`gAMA`, JPEG XL enum colour encoding, …).
    Container,
    /// An embedded ICC matrix/TRC profile, applied exactly.
    IccMatrixTrc,
    /// An embedded LUT-based (or CMYK/Lab) ICC profile, converted by the CMS to linear Rec.2020.
    IccCms,
    /// An embedded ICC profile we could not apply (malformed/unsupported): **fell back to sRGB**.
    IccUnsupported,
    /// A naive conversion without a profile (e.g. CMYK → RGB without an ICC profile), then sRGB.
    Naive,
}

/// The colour space of [`crate::Decoded::image`]: linear RGB with these primaries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceSpace {
    /// Recognised well-known space, if the primaries match one.
    pub named: Option<NamedSpace>,
    /// Linear image RGB → CIE XYZ relative to D50 (ICC PCS; white Y = 1).
    pub to_xyz_d50: Mat3,
    /// The encoding curves that were removed during decode (R, G, B), if known.
    pub trc: Option<[Trc; 3]>,
    pub origin: SpaceOrigin,
}

impl SourceSpace {
    pub fn named(n: NamedSpace, origin: SpaceOrigin) -> SourceSpace {
        let t = n.trc();
        SourceSpace { named: Some(n), to_xyz_d50: n.to_xyz_d50(), trc: Some([t.clone(), t.clone(), t]), origin }
    }

    /// Linear source RGB → linear `dst` RGB (Bradford from D50 to `dst`'s white).
    pub fn to_space(&self, dst: &RgbSpace) -> Mat3 {
        if let Some(n) = self.named {
            let s = n.rgb_space();
            if s == *dst {
                return Mat3::IDENTITY;
            }
            return s.to_space(dst);
        }
        dst.from_xyz().mul(&bradford(D50, dst.white)).mul(&self.to_xyz_d50)
    }

    /// Linear source RGB → linear Rec.2020 D65 (the pipeline working space).
    pub fn to_working(&self) -> Mat3 {
        self.to_space(&WORKING)
    }

    /// True when the pixels need a matrix to reach the working space.
    pub fn is_working(&self) -> bool {
        self.named == Some(NamedSpace::Rec2020)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_recognition_roundtrip() {
        for n in NamedSpace::ALL {
            assert_eq!(NamedSpace::recognize(&n.to_xyz_d50()), Some(n));
        }
    }

    #[test]
    fn trc_inverse() {
        let curves = [
            Trc::Srgb,
            Trc::Gamma(2.2),
            Trc::Rec709,
            Trc::Parametric(vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]),
            Trc::Table((0..1024).map(|i| ((i as f32 / 1023.0).powf(2.2) * 65535.0) as u16).collect()),
        ];
        for c in curves {
            for i in 0..=20 {
                let x = i as f32 / 20.0;
                let y = c.to_linear(x);
                assert!((c.from_linear(y) - x).abs() < 2e-3, "{c:?} {x}");
            }
        }
        assert!(Trc::Srgb.approx_eq(&Trc::Parametric(vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]), 1e-5));
    }

    #[test]
    fn working_matrix_identity_for_rec2020() {
        let s = SourceSpace::named(NamedSpace::Rec2020, SpaceOrigin::Container);
        assert_eq!(s.to_working(), Mat3::IDENTITY);
        let p3 = SourceSpace::named(NamedSpace::DisplayP3, SpaceOrigin::Container);
        let w = p3.to_working().apply([1.0, 1.0, 1.0]);
        for c in w {
            assert!((c - 1.0).abs() < 1e-6);
        }
    }
}
