//! Output encoding: the colour space (primaries + transfer curve) and sample format a render is
//! delivered in. Previews are always 8-bit sRGB; exports may ask for a wide-gamut space and/or
//! 16-bit or linear float samples.
//!
//! The per-pixel stage works in scene-linear Rec.2020 and ends by converting to the target's
//! primaries, gamut mapping into the *target* gamut, and encoding. Tone curves and grain operate on
//! sRGB-curve-encoded values of the target primaries (identical to the sRGB path when the target is
//! sRGB); afterwards the values are re-encoded with the target's own curve.

use lightcraft_color::{ADOBE_RGB, DISPLAY_P3, PROPHOTO, REC2020, RgbSpace, SRGB};
use serde::{Deserialize, Serialize};

/// The RGB space an image is rendered into.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputSpace {
    #[default]
    Srgb,
    DisplayP3,
    /// Primaries and gamma of the published Adobe RGB (1998) specification ("compatible": our own
    /// profile, built from the published numbers).
    AdobeRgb,
    ProPhoto,
    Rec2020,
}

/// The encoding curve of an [`OutputSpace`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OutputTrc {
    /// IEC 61966-2-1 (sRGB, Display P3).
    Srgb,
    /// Pure power law: encoded = linear^(1/g).
    Gamma(f32),
    /// ITU-R BT.709 / BT.2020 OETF.
    Rec709,
}

impl OutputTrc {
    /// Linear → encoded (input clamped to 0..1).
    pub fn encode(self, v: f32) -> f32 {
        let v = v.clamp(0.0, 1.0);
        match self {
            OutputTrc::Srgb => lightcraft_color::transfer::linear_to_srgb(v),
            OutputTrc::Gamma(g) => v.powf(1.0 / g),
            OutputTrc::Rec709 => {
                if v < 0.018 {
                    v * 4.5
                } else {
                    1.099 * v.powf(0.45) - 0.099
                }
            }
        }
    }

    /// Encoded → linear (input clamped to 0..1).
    pub fn decode(self, e: f32) -> f32 {
        let e = e.clamp(0.0, 1.0);
        match self {
            OutputTrc::Srgb => lightcraft_color::transfer::srgb_to_linear(e),
            OutputTrc::Gamma(g) => e.powf(g),
            OutputTrc::Rec709 => {
                if e < 0.081 {
                    e / 4.5
                } else {
                    ((e + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
        }
    }

    /// Code for the GPU kernel: 0 sRGB, 1 gamma, 2 Rec.709.
    pub fn code(self) -> (u32, f32) {
        match self {
            OutputTrc::Srgb => (0, 1.0),
            OutputTrc::Gamma(g) => (1, g),
            OutputTrc::Rec709 => (2, 1.0),
        }
    }
}

impl OutputSpace {
    pub const ALL: [OutputSpace; 5] = [OutputSpace::Srgb, OutputSpace::DisplayP3, OutputSpace::AdobeRgb, OutputSpace::ProPhoto, OutputSpace::Rec2020];

    pub fn rgb_space(self) -> RgbSpace {
        match self {
            OutputSpace::Srgb => SRGB,
            OutputSpace::DisplayP3 => DISPLAY_P3,
            OutputSpace::AdobeRgb => ADOBE_RGB,
            OutputSpace::ProPhoto => PROPHOTO,
            OutputSpace::Rec2020 => REC2020,
        }
    }

    pub fn trc(self) -> OutputTrc {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => OutputTrc::Srgb,
            OutputSpace::AdobeRgb => OutputTrc::Gamma(563.0 / 256.0),
            OutputSpace::ProPhoto => OutputTrc::Gamma(1.8),
            OutputSpace::Rec2020 => OutputTrc::Rec709,
        }
    }

    /// User-facing name.
    pub fn label(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::AdobeRgb => "Adobe RGB (1998) compatible",
            OutputSpace::ProPhoto => "ProPhoto RGB",
            OutputSpace::Rec2020 => "Rec. 2020",
        }
    }

    pub fn parse(s: &str) -> Option<OutputSpace> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "srgb" => OutputSpace::Srgb,
            "displayp3" | "p3" => OutputSpace::DisplayP3,
            "adobergb" | "adobergb1998" | "adobergb1998compatible" | "widegamutrgb" => OutputSpace::AdobeRgb,
            "prophoto" | "prophotorgb" | "rommrgb" => OutputSpace::ProPhoto,
            "rec2020" | "bt2020" | "itur2020" => OutputSpace::Rec2020,
            _ => return None,
        })
    }

    /// Linear Rec.2020 → linear target RGB (Bradford-adapted for D50 targets), row-major.
    pub fn from_working(self) -> [[f32; 3]; 3] {
        REC2020.to_space(&self.rgb_space()).to_f32()
    }

    /// Luminance weights of the target's linear RGB, as the gamut mapper uses them. sRGB keeps the
    /// rounded BT.709 weights the pipeline has always used (byte-identical previews and exports).
    pub fn luma(self) -> [f32; 3] {
        match self {
            OutputSpace::Srgb => [0.2126, 0.7152, 0.0722],
            o => o.rgb_space().luma().map(|v| v as f32),
        }
    }
}

/// Soft proofing: render as if the result were converted to `space` (colours outside its gamut
/// are mapped into it, then shown in the render's own space), optionally painting what the
/// destination can't hold (`dest_warning`, red) and what the display can't show
/// (`display_warning`, blue) — Lightroom's destination and monitor gamut warnings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Proof {
    pub space: OutputSpace,
    pub dest_warning: bool,
    pub display_warning: bool,
}

/// Warning colours (sRGB-encoded): destination gamut red, display gamut blue.
pub const PROOF_DEST_WARNING: [f32; 3] = [1.0, 0.0, 0.0];
pub const PROOF_DISPLAY_WARNING: [f32; 3] = [0.0, 0.25, 1.0];

/// [`Proof`] as the per-pixel stage uses it.
#[derive(Clone, Copy, Debug)]
pub struct ProofParams {
    /// Linear Rec.2020 → linear proof RGB, its luminance weights, linear proof → linear output RGB.
    pub to_proof: [[f32; 3]; 3],
    pub luma: [f32; 3],
    pub proof_to_out: [[f32; 3]; 3],
    pub dest_warning: bool,
    pub display_warning: bool,
}

impl Proof {
    pub fn key(self) -> u64 {
        (self.space as u64 + 1) | (self.dest_warning as u64) << 8 | (self.display_warning as u64) << 9
    }

    pub fn params(self, out: OutputSpace) -> ProofParams {
        ProofParams {
            to_proof: self.space.from_working(),
            luma: self.space.luma(),
            proof_to_out: self.space.rgb_space().to_space(&out.rgb_space()).to_f32(),
            dest_warning: self.dest_warning,
            display_warning: self.display_warning,
        }
    }
}

/// Sample format of a render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OutputDepth {
    /// 8-bit display-encoded ([`crate::Rendered::image`] only).
    #[default]
    U8,
    /// 16-bit display-encoded RGB in [`crate::Rendered::deep`].
    U16,
    /// 32-bit float *linear* RGB (target primaries, 0..1) in [`crate::Rendered::deep`].
    F32Linear,
}

/// High-bit-depth RGB samples (3 per pixel, interleaved, row-major).
#[derive(Clone, Debug, PartialEq)]
pub enum DeepSamples {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

/// A high-bit-depth render ([`OutputDepth::U16`] / [`OutputDepth::F32Linear`]).
#[derive(Clone, Debug, PartialEq)]
pub struct DeepImage {
    pub width: usize,
    pub height: usize,
    pub space: OutputSpace,
    pub samples: DeepSamples,
}

impl DeepImage {
    /// The same image reduced to 8 bits, display-encoded with the space's curve.
    pub fn to_rgba8(&self) -> lightcraft_raster::Rgba8 {
        let mut out = lightcraft_raster::Rgba8::new(self.width, self.height);
        let trc = self.space.trc();
        match &self.samples {
            DeepSamples::U16(v) => {
                for (p, c) in out.data.iter_mut().zip(v.as_chunks::<3>().0) {
                    *p = [c[0], c[1], c[2]].map(|x| ((x as u32 * 255 + 32767) / 65535) as u8).into_rgba();
                }
            }
            DeepSamples::F32(v) => {
                for (p, c) in out.data.iter_mut().zip(v.as_chunks::<3>().0) {
                    *p = [c[0], c[1], c[2]].map(|x| (trc.encode(x) * 255.0 + 0.5) as u8).into_rgba();
                }
            }
        }
        out
    }
}

trait IntoRgba {
    fn into_rgba(self) -> [u8; 4];
}

impl IntoRgba for [u8; 3] {
    fn into_rgba(self) -> [u8; 4] {
        [self[0], self[1], self[2], 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trcs_invert() {
        for o in OutputSpace::ALL {
            let t = o.trc();
            for i in 0..=100 {
                let x = i as f32 / 100.0;
                assert!((t.decode(t.encode(x)) - x).abs() < 1e-5, "{o:?} {x}");
            }
        }
    }

    #[test]
    fn parse_and_serde_names() {
        for o in OutputSpace::ALL {
            let s = serde_json::to_value(o).unwrap();
            assert_eq!(OutputSpace::parse(s.as_str().unwrap()), Some(o));
            assert_eq!(OutputSpace::parse(o.label()), Some(o), "{}", o.label());
        }
    }

    #[test]
    fn white_maps_to_white() {
        for o in OutputSpace::ALL {
            let m = o.from_working();
            for row in m {
                assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-4, "{o:?}");
            }
            assert!((o.luma().iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
    }
}
