//! ICC profiles: parsing (via `moxcms`), matrix/TRC extraction, CMS fallback for LUT/CMYK profiles,
//! and writing matrix/TRC profiles for export embedding.

use crate::space::{NamedSpace, Trc, rgb_to_xyz_d50};
use lightcraft_color::{D50, Mat3, RgbSpace, bradford};
use moxcms::{
    ColorProfile, DataColorSpace, Layout, LocalizableString, Matrix3d, ProfileText, RenderingIntent, ToneReprCurve, TransformOptions, Xyzd,
};
use serde::{Deserialize, Serialize};

/// Colour model of the profile's device space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IccColorModel {
    Rgb,
    Gray,
    Cmyk,
    Other,
}

/// How a profile can be applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum IccKind {
    /// RGB matrix/TRC (shaper) profile: exact, applied by us.
    MatrixTrc { to_xyz_d50: Mat3, trc: [Trc; 3] },
    /// Grayscale profile with a `kTRC`.
    Gray { trc: Trc },
    /// LUT-based (A2B), CMYK or otherwise needs a full CMS conversion.
    NeedsCms,
}

/// Summary of a parsed ICC profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IccInfo {
    pub model: IccColorModel,
    pub kind: IccKind,
    /// Recognised well-known RGB space (by colorants), if any.
    pub named: Option<NamedSpace>,
    pub description: Option<String>,
}

/// Parse an ICC profile. Returns `None` for malformed or unsupported data (never panics).
pub fn parse(bytes: &[u8]) -> Option<IccInfo> {
    let p = std::panic::catch_unwind(|| ColorProfile::new_from_slice(bytes)).ok()?.ok()?;
    Some(info_of(&p))
}

fn info_of(p: &ColorProfile) -> IccInfo {
    let description = p.description.as_ref().and_then(text_of);
    let model = match p.color_space {
        DataColorSpace::Rgb => IccColorModel::Rgb,
        DataColorSpace::Gray => IccColorModel::Gray,
        DataColorSpace::Cmyk => IccColorModel::Cmyk,
        _ => IccColorModel::Other,
    };
    let has_lut = p.lut_a_to_b_perceptual.is_some() || p.lut_a_to_b_colorimetric.is_some();
    let kind = if model == IccColorModel::Rgb && p.is_matrix_shaper() && p.pcs == DataColorSpace::Xyz {
        let c = [p.red_colorant, p.green_colorant, p.blue_colorant];
        let to_xyz_d50 = Mat3([[c[0].x, c[1].x, c[2].x], [c[0].y, c[1].y, c[2].y], [c[0].z, c[1].z, c[2].z]]);
        let trcs = [&p.red_trc, &p.green_trc, &p.blue_trc];
        let trc = trcs.map(|t| t.as_ref().map(trc_from_icc).unwrap_or(Trc::Srgb));
        if to_xyz_d50.determinant().abs() > 1e-6 && trc.iter().all(valid_trc) { IccKind::MatrixTrc { to_xyz_d50, trc } } else { IccKind::NeedsCms }
    } else if model == IccColorModel::Gray && p.gray_trc.is_some() && !has_lut {
        let trc = p.gray_trc.as_ref().map(trc_from_icc).unwrap_or(Trc::Srgb);
        if valid_trc(&trc) { IccKind::Gray { trc } } else { IccKind::NeedsCms }
    } else {
        IccKind::NeedsCms
    };
    let named = match &kind {
        IccKind::MatrixTrc { to_xyz_d50, .. } => NamedSpace::recognize(to_xyz_d50),
        _ => None,
    };
    IccInfo { model, kind, named, description }
}

fn valid_trc(t: &Trc) -> bool {
    match t {
        Trc::Parametric(p) => matches!(p.len(), 1 | 3 | 4 | 5 | 7) && p.iter().all(|v| v.is_finite()),
        Trc::Gamma(g) => g.is_finite() && *g > 0.0,
        _ => true,
    }
}

fn text_of(t: &ProfileText) -> Option<String> {
    let s = match t {
        ProfileText::PlainString(s) => s.clone(),
        ProfileText::Localizable(v) => v.first().map(|l| l.value.clone())?,
        ProfileText::Description(d) => {
            if d.unicode_string.is_empty() {
                d.ascii_string.clone()
            } else {
                d.unicode_string.clone()
            }
        }
    };
    let s = s.trim_end_matches('\0').trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

fn trc_from_icc(t: &ToneReprCurve) -> Trc {
    match t {
        ToneReprCurve::Lut(v) if v.is_empty() => Trc::Linear,
        ToneReprCurve::Lut(v) if v.len() == 1 => Trc::Gamma(v[0] as f32 / 256.0),
        ToneReprCurve::Lut(v) => Trc::Table(v.clone()),
        ToneReprCurve::Parametric(p) => Trc::Parametric(p.clone()),
    }
}

fn trc_to_icc(t: &Trc) -> ToneReprCurve {
    match t {
        Trc::Linear => ToneReprCurve::Parametric(vec![1.0]),
        Trc::Srgb => ToneReprCurve::Parametric(vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]),
        Trc::Gamma(g) => ToneReprCurve::Parametric(vec![*g]),
        Trc::Rec709 => ToneReprCurve::Parametric(vec![1.0 / 0.45, 1.0 / 1.099, 0.099 / 1.099, 1.0 / 4.5, 0.081]),
        Trc::Parametric(p) => ToneReprCurve::Parametric(p.clone()),
        Trc::Table(v) => ToneReprCurve::Lut(v.clone()),
    }
}

fn xyzd(v: [f64; 3]) -> Xyzd {
    Xyzd { x: v[0], y: v[1], z: v[2] }
}

/// ICC PCS illuminant (D50) as stored in profiles.
const ICC_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// Build an ICC v4 display-class matrix/TRC profile for `space` with the given encoding curve
/// (D50 PCS, Bradford `chad`), for embedding in exported files.
pub fn write_matrix_trc(space: &RgbSpace, trc: &Trc) -> Vec<u8> {
    let m = rgb_to_xyz_d50(space).0;
    let mut p = ColorProfile::new_srgb();
    p.cicp = None;
    p.red_colorant = xyzd([m[0][0], m[1][0], m[2][0]]);
    p.green_colorant = xyzd([m[0][1], m[1][1], m[2][1]]);
    p.blue_colorant = xyzd([m[0][2], m[1][2], m[2][2]]);
    let curve = trc_to_icc(trc);
    p.red_trc = Some(curve.clone());
    p.green_trc = Some(curve.clone());
    p.blue_trc = Some(curve);
    p.white_point = xyzd(ICC_D50);
    p.media_white_point = Some(xyzd(ICC_D50));
    p.chromatic_adaptation = Some(Matrix3d { v: bradford(space.white, D50).0 });
    p.rendering_intent = RenderingIntent::Perceptual;
    let desc = match trc {
        t if t.is_linear() => format!("{} (linear)", space.name),
        _ => space.name.to_string(),
    };
    p.description = Some(ProfileText::Localizable(vec![LocalizableString::new("en".into(), "US".into(), desc)]));
    p.copyright = Some(ProfileText::Localizable(vec![LocalizableString::new("en".into(), "US".into(), "No copyright, use freely".into())]));
    align_tags(&p.encode().unwrap_or_default())
}

/// Re-lay a profile's tag data on 4-byte boundaries (ICC.1 §7.3.1). The encoder packs tags back to
/// back, so a tag of odd length (e.g. a `mluc` description with an odd number of characters)
/// misaligns every following tag — which some readers (macOS ImageIO for PNG `iCCP`) reject.
fn align_tags(b: &[u8]) -> Vec<u8> {
    let be = |i: usize| b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize);
    let Some(n) = be(128) else { return b.to_vec() };
    let table_end = 132 + 12 * n;
    let tags: Option<Vec<(usize, usize)>> = (0..n).map(|t| Some((be(132 + 12 * t + 4)?, be(132 + 12 * t + 8)?))).collect();
    let Some(tags) = tags.filter(|t| table_end <= b.len() && t.iter().all(|&(o, l)| o >= table_end && o + l <= b.len())) else {
        return b.to_vec();
    };
    let mut out = b[..table_end].to_vec();
    let mut placed: Vec<((usize, usize), usize)> = Vec::new();
    for (t, &(off, len)) in tags.iter().enumerate() {
        let new_off = match placed.iter().find(|(k, _)| *k == (off, len)) {
            // tags sharing data keep sharing it
            Some(&(_, o)) => o,
            None => {
                while !out.len().is_multiple_of(4) {
                    out.push(0);
                }
                let o = out.len();
                out.extend_from_slice(&b[off..off + len]);
                placed.push(((off, len), o));
                o
            }
        };
        out[132 + 12 * t + 4..132 + 12 * t + 8].copy_from_slice(&(new_off as u32).to_be_bytes());
    }
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let size = out.len() as u32;
    out[0..4].copy_from_slice(&size.to_be_bytes());
    // the profile ID (MD5) would no longer match: clear it (= "not computed")
    out[84..100].fill(0);
    out
}

/// A profile for one of the well-known spaces with its standard curve.
pub fn write_named(n: NamedSpace) -> Vec<u8> {
    write_matrix_trc(&n.rgb_space(), &n.trc())
}

fn linear_rec2020_profile() -> ColorProfile {
    let mut p = ColorProfile::new_bt2020();
    p.cicp = None;
    let lin = ToneReprCurve::Parametric(vec![1.0]);
    p.red_trc = Some(lin.clone());
    p.green_trc = Some(lin.clone());
    p.blue_trc = Some(lin);
    p
}

/// Convert device samples (0..1, interleaved, `channels` = 1, 3 or 4 matching the profile's colour
/// model) to linear Rec.2020 through a full CMS transform. `None` if the profile cannot be applied.
pub(crate) fn cms_to_linear_rec2020(icc: &[u8], samples: &[f32], channels: usize) -> Option<Vec<[f32; 3]>> {
    let src = ColorProfile::new_from_slice(icc).ok()?;
    let layout = match (src.color_space, channels) {
        (DataColorSpace::Rgb, 3) => Layout::Rgb,
        (DataColorSpace::Gray, 1) => Layout::Gray,
        (DataColorSpace::Cmyk, 4) => Layout::Rgba,
        _ => return None,
    };
    let dst = linear_rec2020_profile();
    let opts = TransformOptions { rendering_intent: RenderingIntent::RelativeColorimetric, ..TransformOptions::default() };
    let run = || -> Option<Vec<[f32; 3]>> {
        let t = src.create_transform_f32(layout, &dst, Layout::Rgb, opts).ok()?;
        let n = samples.len() / channels;
        let mut out = vec![0f32; n * 3];
        // Chunk to bound temporary memory and allow parallelism.
        const ROWS: usize = 1 << 16;
        let src_chunks = samples.chunks(ROWS * channels);
        let dst_chunks = out.chunks_mut(ROWS * 3);
        #[cfg(feature = "parallel")]
        {
            use rayon::prelude::*;
            let pairs: Vec<_> = src_chunks.zip(dst_chunks).collect();
            let ok = pairs.into_par_iter().all(|(s, d)| t.transform(s, d).is_ok());
            if !ok {
                return None;
            }
        }
        #[cfg(not(feature = "parallel"))]
        for (s, d) in src_chunks.zip(dst_chunks) {
            t.transform(s, d).ok()?;
        }
        Some(out.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect())
    };
    std::panic::catch_unwind(run).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_color::{DISPLAY_P3, PROPHOTO, SRGB};

    #[test]
    fn write_then_parse_recognises() {
        for n in NamedSpace::ALL {
            let bytes = write_named(n);
            assert!(bytes.len() > 128, "{n:?}");
            let info = parse(&bytes).expect("parses");
            assert_eq!(info.named, Some(n), "{n:?}");
            match info.kind {
                IccKind::MatrixTrc { trc, .. } => assert!(trc[0].approx_eq(&n.trc(), 2e-4), "{n:?} {:?}", trc[0]),
                k => panic!("{k:?}"),
            }
        }
    }

    #[test]
    fn tags_are_aligned() {
        for n in NamedSpace::ALL {
            for trc in [n.trc(), Trc::Linear] {
                let b = write_matrix_trc(&n.rgb_space(), &trc);
                assert_eq!(b.len() % 4, 0, "{n:?}");
                assert_eq!(u32::from_be_bytes(b[0..4].try_into().unwrap()) as usize, b.len());
                let count = u32::from_be_bytes(b[128..132].try_into().unwrap()) as usize;
                for t in 0..count {
                    let off = u32::from_be_bytes(b[132 + 12 * t + 4..132 + 12 * t + 8].try_into().unwrap());
                    assert_eq!(off % 4, 0, "{n:?} tag {t}");
                }
                assert!(parse(&b).is_some_and(|i| i.named == Some(n)), "{n:?}");
            }
        }
    }

    #[test]
    fn linear_profile() {
        let bytes = write_matrix_trc(&PROPHOTO, &Trc::Linear);
        let info = parse(&bytes).unwrap();
        assert_eq!(info.named, Some(NamedSpace::ProPhoto));
        let IccKind::MatrixTrc { trc, .. } = info.kind else { panic!() };
        assert!(trc[1].is_linear());
    }

    #[test]
    fn cms_path_matches_matrix_path() {
        let bytes = write_named(NamedSpace::DisplayP3);
        let px = [0.2f32, 0.5, 0.8];
        let out = cms_to_linear_rec2020(&bytes, &px, 3).unwrap()[0];
        let lin = px.map(|v| Trc::Srgb.to_linear(v));
        let m = DISPLAY_P3.to_space(&lightcraft_color::REC2020);
        let want = m.apply_f32(lin);
        for i in 0..3 {
            assert!((out[i] - want[i]).abs() < 2e-3, "{out:?} vs {want:?}");
        }
        let _ = SRGB;
    }

    #[test]
    fn garbage_is_none() {
        assert!(parse(&[]).is_none());
        assert!(parse(&[0u8; 200]).is_none());
    }
}
