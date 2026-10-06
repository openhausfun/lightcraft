//! Colour tools in OkLCh on display-linear Rec.2020 values: vibrance, saturation, the 8-band colour
//! mixer, B&W mix, and 3-way colour grading; plus the camera-calibration matrix (scene linear).

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use lightcraft_color::perceptual::{hsv_to_rgb, lab_to_lch, lch_to_lab, oklab_from_2020, oklab_to_2020};
use lightcraft_color::{Mat3, REC2020, SRGB};
use lightcraft_develop::{Calibration, DevelopSettings, MIXER_HUES, PointColor};

/// OkLCh hue angle (radians) of a pure sRGB colour with HSV hue `deg`.
pub fn oklch_hue_of_srgb_hue(deg: f64) -> f32 {
    let c = hsv_to_rgb(deg as f32, 1.0, 1.0);
    let lin = c.map(lightcraft_color::transfer::srgb_to_linear);
    let m = SRGB.to_space(&REC2020).apply_f32(lin);
    lab_to_lch(oklab_from_2020(m))[2]
}

/// OkLCh hue (radians) of each colour-mixer band centre.
pub fn band_hues() -> &'static [f32; 8] {
    static H: OnceLock<[f32; 8]> = OnceLock::new();
    H.get_or_init(|| MIXER_HUES.map(oklch_hue_of_srgb_hue))
}

#[inline]
fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

/// Partition-of-unity weights of hue `h` over the 8 bands (raised cosine between neighbours).
#[inline]
pub fn band_weights(h: f32) -> [f32; 8] {
    let hues = band_hues();
    let mut w = [0.0f32; 8];
    for i in 0..8 {
        let a = hues[i];
        let b = hues[(i + 1) % 8];
        let span = wrap(b - a).rem_euclid(TAU);
        let d = wrap(h - a).rem_euclid(TAU);
        if d <= span {
            let t = d / span;
            let s = 0.5 - 0.5 * (t * PI).cos();
            w[i] += 1.0 - s;
            w[(i + 1) % 8] += s;
            break;
        }
    }
    w
}

/// A colour-grading wheel as an OkLab offset (a, b) and a lightness shift.
#[derive(Clone, Copy, Debug)]
pub struct WheelK {
    pub a: f32,
    pub b: f32,
    pub lum: f32,
}

/// Number of `f32`s per Point Color sample in [`PointK::words`] (the GPU parameter layout).
pub const POINT_WORDS: usize = 10;

/// A Point Color sample resolved for the per-pixel stage: the sample (OkLab lightness, chroma, hue
/// in radians), the adjustment (hue rotation in radians, chroma scale − 1, lightness shift,
/// variance factor − 1) and the half-widths of the range box (hue radians, chroma, lightness).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointK {
    pub l: f32,
    pub c: f32,
    pub h: f32,
    pub dh: f32,
    pub sat: f32,
    pub dl: f32,
    pub var: f32,
    pub wh: f32,
    pub wc: f32,
    pub wl: f32,
}

impl PointK {
    pub fn new(p: &PointColor) -> PointK {
        let k = (p.range.clamp(0.0, 100.0) / 50.0).max(0.05) as f32;
        let f = |v: f64| (v.clamp(0.0, 100.0) / 100.0) as f32;
        PointK {
            l: p.lum as f32,
            c: p.chroma.max(0.0) as f32,
            h: (p.hue as f32).to_radians(),
            dh: (p.hue_shift.clamp(-100.0, 100.0) / 100.0) as f32 * 0.5,
            sat: (p.sat_shift.clamp(-100.0, 100.0) / 100.0) as f32,
            dl: (p.lum_shift.clamp(-100.0, 100.0) / 100.0) as f32 * 0.2,
            var: (p.variance.clamp(-100.0, 100.0) / 100.0) as f32 * 0.8,
            wh: (0.1 + 0.6 * f(p.hue_range)) * k,
            wc: (0.02 + 0.12 * f(p.sat_range)) * k,
            wl: (0.05 + 0.4 * f(p.lum_range)) * k,
        }
    }

    pub fn words(&self) -> [f32; POINT_WORDS] {
        [self.l, self.c, self.h, self.dh, self.sat, self.dl, self.var, self.wh, self.wc, self.wl]
    }

    /// How much an OkLCh colour belongs to this sample's range (0..1, soft edges).
    #[inline]
    pub fn weight(&self, l: f32, c: f32, h: f32) -> f32 {
        // hue is meaningless for near-neutral colours: a chromatic sample ignores them, a
        // near-neutral sample selects by chroma and lightness only
        let wh = if self.c < 0.02 { 1.0 } else { (1.0 - smooth(0.5 * self.wh, self.wh, wrap(h - self.h).abs())) * smooth(0.005, 0.025, c) };
        if wh <= 0.0 {
            return 0.0;
        }
        let wc = 1.0 - smooth(0.5 * self.wc, self.wc, (c - self.c).abs());
        let wl = 1.0 - smooth(0.5 * self.wl, self.wl, (l - self.l).abs());
        wh * wc * wl
    }

    /// Apply this sample's adjustment to an OkLCh colour.
    #[inline]
    pub fn apply(&self, lch: [f32; 3]) -> [f32; 3] {
        let [mut l, mut c, mut h] = lch;
        let w = self.weight(l, c, h);
        if w <= 0.0 {
            return lch;
        }
        h += w * (self.var * wrap(h - self.h) + self.dh);
        c += w * self.var * (c - self.c);
        c = (c * (1.0 + w * self.sat)).max(0.0);
        l += w * (self.var * (l - self.l) + self.dl);
        [l, c, h]
    }
}

/// The colour tools' parameters, resolved once per render (fields are read by the GPU kernel).
#[derive(Clone, Debug)]
pub struct ColorOps {
    pub vibrance: f32,
    pub saturation: f32,
    pub hue: [f32; 8],
    pub sat: [f32; 8],
    pub lum: [f32; 8],
    pub mixer: bool,
    /// Point Color samples that change something (applied after the mixer).
    pub points: Vec<PointK>,
    pub bw: Option<[f32; 8]>,
    /// Wheels (shadows, midtones, highlights, global), blending, balance.
    pub grading: Option<([WheelK; 4], f32, f32)>,
    /// OkLCh hue of skin tones (protected by vibrance).
    pub skin: f32,
}

fn wheel(w: &lightcraft_develop::Wheel) -> WheelK {
    let h = oklch_hue_of_srgb_hue(w.hue);
    let s = (w.sat / 100.0) as f32 * 0.09;
    WheelK { a: s * h.cos(), b: s * h.sin(), lum: (w.lum / 100.0) as f32 * 0.12 }
}

impl ColorOps {
    pub fn new(s: &DevelopSettings) -> ColorOps {
        let bands = s.mixer.bands();
        let g = &s.grading;
        ColorOps {
            vibrance: (s.color.vibrance / 100.0) as f32,
            saturation: (s.color.saturation / 100.0) as f32,
            hue: bands.map(|b| (b.hue / 100.0) as f32 * 0.5),
            sat: bands.map(|b| (b.sat / 100.0) as f32),
            lum: bands.map(|b| (b.lum / 100.0) as f32 * 0.18),
            mixer: !s.mixer.is_neutral(),
            points: if crate::is_bw(s) {
                Vec::new()
            } else {
                s.point_colors.iter().take(lightcraft_develop::MAX_POINT_COLORS).filter(|p| !p.is_neutral()).map(PointK::new).collect()
            },
            bw: crate::is_bw(s).then(|| s.bw_mix.bands().map(|v| (v / 100.0) as f32)),
            grading: (!g.is_neutral()).then(|| {
                (
                    [wheel(&g.shadows), wheel(&g.midtones), wheel(&g.highlights), wheel(&g.global)],
                    (g.blending / 100.0) as f32,
                    (g.balance / 100.0) as f32,
                )
            }),
            skin: oklch_hue_of_srgb_hue(25.0),
        }
    }

    pub fn is_identity(&self) -> bool {
        self.vibrance == 0.0 && self.saturation == 0.0 && !self.mixer && self.points.is_empty() && self.bw.is_none() && self.grading.is_none()
    }

    /// `local_sat` (−1..1) and `local_hue` (radians) come from masks.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], local_sat: f32, local_hue: f32) -> [f32; 3] {
        if self.is_identity() && local_sat == 0.0 && local_hue == 0.0 {
            return rgb;
        }
        let lab = oklab_from_2020(rgb);
        // only chroma changes (vibrance / saturation, maybe grading): scale a, b directly — the same
        // result as the OkLCh round trip without its sin / cos (and atan2 unless vibrance needs the hue)
        if !self.mixer && self.points.is_empty() && self.bw.is_none() && local_hue == 0.0 {
            let mut lab = lab;
            let c0 = (lab[1] * lab[1] + lab[2] * lab[2]).sqrt();
            let mut c = c0;
            if self.vibrance != 0.0 {
                let low = 1.0 - (c / 0.22).clamp(0.0, 1.0);
                let skin = if self.vibrance > 0.0 { 1.0 - 0.6 * (-(wrap(lab[2].atan2(lab[1]) - self.skin) / 0.35).powi(2)).exp() } else { 1.0 };
                c *= (1.0 + self.vibrance * low * low * skin * 1.2).max(0.0);
            }
            if self.saturation != 0.0 || local_sat != 0.0 {
                c *= (1.0 + self.saturation + local_sat).max(0.0);
            }
            if c0 > 0.0 {
                let k = c / c0;
                lab[1] *= k;
                lab[2] *= k;
            }
            return self.grade(lab);
        }
        let [mut l, mut c, mut h] = lab_to_lch(lab);
        if self.mixer {
            let w = band_weights(h);
            let (mut dh, mut ds, mut dl) = (0.0, 0.0, 0.0);
            for i in 0..8 {
                dh += w[i] * self.hue[i];
                ds += w[i] * self.sat[i];
                dl += w[i] * self.lum[i];
            }
            let chroma_w = (c / 0.12).min(1.0);
            h += dh * chroma_w;
            c *= (1.0 + ds).max(0.0);
            l += dl * chroma_w * l.max(0.05).sqrt();
        }
        for p in &self.points {
            [l, c, h] = p.apply([l, c, h]);
        }
        if self.vibrance != 0.0 {
            let low = 1.0 - (c / 0.22).clamp(0.0, 1.0);
            let skin = if self.vibrance > 0.0 { 1.0 - 0.6 * (-(wrap(h - self.skin) / 0.35).powi(2)).exp() } else { 1.0 };
            c *= (1.0 + self.vibrance * low * low * skin * 1.2).max(0.0);
        }
        if self.saturation != 0.0 || local_sat != 0.0 {
            c *= (1.0 + self.saturation + local_sat).max(0.0);
        }
        h += local_hue;
        if let Some(bw) = &self.bw {
            let w = band_weights(h);
            let mix: f32 = (0..8).map(|i| w[i] * bw[i]).sum();
            l = (l + mix * (c / 0.2).min(1.0) * 0.25).max(0.0);
            c = 0.0;
        }
        self.grade(lch_to_lab([l, c, h]))
    }

    /// Colour grading on OkLab, then back to linear Rec.2020.
    #[inline]
    fn grade(&self, mut lab: [f32; 3]) -> [f32; 3] {
        if let Some((wheels, blending, balance)) = &self.grading {
            let m = 0.5 - balance * 0.25;
            let width = 0.15 + blending * 0.5;
            let ws = 1.0 - smooth(m - width, m + width * 0.25, lab[0]);
            let wh = smooth(m - width * 0.25, m + width, lab[0]);
            let wm = (1.0 - ws - wh).max(0.0);
            for (k, wt) in wheels.iter().zip([ws, wm, wh, 1.0]) {
                lab[1] += k.a * wt;
                lab[2] += k.b * wt;
                lab[0] += k.lum * wt;
            }
        }
        oklab_to_2020(lab)
    }
}

/// Hue rotation (OkLCh radians) of a calibration primary at ±100.
pub const CALIB_HUE: f32 = 0.5;
/// Chroma scale of a calibration primary at ±100 (`1 ± CALIB_SAT`).
pub const CALIB_SAT: f32 = 0.6;

/// The calibration panel's primaries as a white-preserving 3×3 matrix on linear Rec.2020 (row-major):
/// each primary is rotated in OkLCh hue and scaled in chroma at constant OkLab lightness, then the
/// columns are rescaled so that neutral (1, 1, 1) maps to itself. `None` when neutral.
pub fn calibration_matrix(c: &Calibration) -> Option<[[f32; 3]; 3]> {
    let prim = c.primaries();
    if prim.iter().all(|(h, s)| *h == 0.0 && *s == 0.0) {
        return None;
    }
    let mut p = [[0.0f64; 3]; 3];
    for (i, (hue, sat)) in prim.iter().enumerate() {
        let mut e = [0.0f32; 3];
        e[i] = 1.0;
        let [l, ch, h] = lab_to_lch(oklab_from_2020(e));
        let h2 = h + (hue.clamp(-100.0, 100.0) / 100.0) as f32 * CALIB_HUE;
        let c2 = ch * (1.0 + (sat.clamp(-100.0, 100.0) / 100.0) as f32 * CALIB_SAT);
        let q = oklab_to_2020(lch_to_lab([l, c2, h2]));
        for (r, row) in p.iter_mut().enumerate() {
            row[i] = q[r] as f64;
        }
    }
    let m = Mat3(p);
    let k = m.inverse()?.apply([1.0, 1.0, 1.0]);
    Some(std::array::from_fn(|r| std::array::from_fn(|col| (p[r][col] * k[col]) as f32)))
}

/// Shadows-tint strength at ±100: the green channel's relative change in deep shadows.
pub const SHADOW_TINT: f32 = 0.3;

/// Calibration in scene-linear light (before tone mapping): the primaries matrix, then the shadows
/// tint (green ↔ magenta, weighted towards dark tones, luminance kept).
#[inline]
pub fn calibrate(c: [f32; 3], m: Option<&[[f32; 3]; 3]>, shadow_tint: f32) -> [f32; 3] {
    let mut c = c;
    if let Some(m) = m {
        c = std::array::from_fn(|r| (m[r][0] * c[0] + m[r][1] * c[1] + m[r][2] * c[2]).max(0.0));
    }
    if shadow_tint != 0.0 {
        let y0 = lightcraft_color::luminance_2020(c);
        let w = 1.0 - smooth(-5.0, -0.5, (y0.max(1e-7) / 0.18).log2());
        c[1] *= (1.0 - SHADOW_TINT * shadow_tint * w).max(0.0);
        let y1 = lightcraft_color::luminance_2020(c).max(1e-9);
        c = c.map(|v| v * y0 / y1);
    }
    c
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_color::perceptual::lab_to_lch;

    #[test]
    fn weights_partition_unity() {
        for i in 0..360 {
            let h = (i as f32).to_radians() - PI;
            let w = band_weights(h);
            let s: f32 = w.iter().sum();
            assert!((s - 1.0).abs() < 1e-4, "{i}: {s}");
        }
        // at a band centre, that band dominates
        let w = band_weights(band_hues()[5]);
        assert!(w[5] > 0.99);
    }

    #[test]
    fn neutral_is_identity() {
        let ops = ColorOps::new(&DevelopSettings::default());
        assert!(ops.is_identity());
        assert_eq!(ops.apply([0.2, 0.3, 0.4], 0.0, 0.0), [0.2, 0.3, 0.4]);
    }

    #[test]
    fn saturation_and_bw() {
        let mut s = DevelopSettings::default();
        s.color.saturation = -100.0;
        let g = ColorOps::new(&s).apply([0.4, 0.1, 0.05], 0.0, 0.0);
        assert!((g[0] - g[1]).abs() < 1e-3 && (g[1] - g[2]).abs() < 1e-3, "{g:?}");
        let mut s = DevelopSettings { treatment: lightcraft_develop::Treatment::Bw, ..Default::default() };
        let ops = ColorOps::new(&s);
        let b = ops.apply([0.05, 0.1, 0.5], 0.0, 0.0);
        assert!((b[0] - b[2]).abs() < 1e-3);
        // raising blue in the B&W mix brightens blue things
        s.bw_mix.blue = 80.0;
        let b2 = ColorOps::new(&s).apply([0.05, 0.1, 0.5], 0.0, 0.0);
        assert!(b2[1] > b[1]);
    }

    #[test]
    fn mixer_targets_its_band() {
        let mut s = DevelopSettings::default();
        s.mixer.blue.sat = -100.0;
        let ops = ColorOps::new(&s);
        let blue = [0.02, 0.05, 0.4];
        let red = [0.4, 0.03, 0.02];
        let cb = lab_to_lch(oklab_from_2020(ops.apply(blue, 0.0, 0.0)))[1];
        let cr0 = lab_to_lch(oklab_from_2020(red))[1];
        let cr = lab_to_lch(oklab_from_2020(ops.apply(red, 0.0, 0.0)))[1];
        assert!(cb < 0.02, "{cb}");
        assert!((cr - cr0).abs() < 0.01);
    }

    #[test]
    fn calibration_keeps_white_and_moves_primaries() {
        assert!(calibration_matrix(&Calibration::default()).is_none());
        let cal = Calibration { red_hue: 60.0, blue_sat: -50.0, ..Default::default() };
        let m = calibration_matrix(&cal).unwrap();
        let grey = calibrate([0.3, 0.3, 0.3], Some(&m), 0.0);
        assert!(grey.iter().all(|v| (v - 0.3).abs() < 1e-4), "{grey:?}");
        // red rotates in hue, blue loses chroma
        let red = [0.4, 0.05, 0.03];
        let h0 = lab_to_lch(oklab_from_2020(red))[2];
        let h1 = lab_to_lch(oklab_from_2020(calibrate(red, Some(&m), 0.0)))[2];
        assert!(h1 - h0 > 0.05, "{h0} -> {h1}");
        let blue = [0.03, 0.05, 0.4];
        let c0 = lab_to_lch(oklab_from_2020(blue))[1];
        let c1 = lab_to_lch(oklab_from_2020(calibrate(blue, Some(&m), 0.0)))[1];
        assert!(c1 < c0 * 0.97, "{c0} -> {c1}");
    }

    #[test]
    fn shadow_tint_targets_shadows() {
        let dark = calibrate([0.01, 0.01, 0.01], None, 1.0);
        let bright = calibrate([0.8, 0.8, 0.8], None, 1.0);
        assert!(dark[1] < dark[0] * 0.85, "magenta shadows: {dark:?}");
        assert!((bright[1] - bright[0]).abs() < 1e-3, "{bright:?}");
        let y = |c: [f32; 3]| lightcraft_color::luminance_2020(c);
        assert!((y(dark) - 0.01).abs() < 1e-5);
        let green = calibrate([0.01, 0.01, 0.01], None, -1.0);
        assert!(green[1] > green[0]);
    }

    #[test]
    fn point_color_targets_its_range() {
        let skin = [0.35, 0.16, 0.09];
        let [l, c, h] = lab_to_lch(oklab_from_2020(skin));
        let sample =
            PointColor { lum: l as f64, chroma: c as f64, hue: (h as f64).to_degrees(), hue_shift: 60.0, sat_shift: -50.0, ..Default::default() };
        let mut s = DevelopSettings::default();
        s.point_colors.push(sample);
        let ops = ColorOps::new(&s);
        let out = lab_to_lch(oklab_from_2020(ops.apply(skin, 0.0, 0.0)));
        assert!(out[1] < c * 0.7, "chroma {c} -> {}", out[1]);
        assert!(wrap(out[2] - h) > 0.2, "hue {h} -> {}", out[2]);
        // far colours are untouched
        for far in [[0.02, 0.05, 0.4], [0.05, 0.3, 0.05], [0.3, 0.3, 0.3]] {
            let o = ops.apply(far, 0.0, 0.0);
            assert!(far.iter().zip(&o).all(|(a, b)| (a - b).abs() < 1e-4), "{far:?} -> {o:?}");
        }
        // weight: 1 at the sample, falling to 0 outside the range; a wider range reaches further
        let k = PointK::new(&sample);
        assert!((k.weight(l, c, h) - 1.0).abs() < 1e-6);
        assert_eq!(k.weight(l, c, h + 1.0), 0.0);
        let wide = PointK::new(&PointColor { range: 100.0, ..sample });
        assert!(wide.weight(l, c, h + 0.45) > k.weight(l, c, h + 0.45));
    }

    #[test]
    fn point_color_variance_compresses_towards_the_sample() {
        let p = PointColor { lum: 0.6, chroma: 0.1, hue: 40.0, variance: -100.0, range: 100.0, ..Default::default() };
        let k = PointK::new(&p);
        let (h0, near) = (40f32.to_radians(), 40f32.to_radians() + 0.1);
        let o = k.apply([0.62, 0.11, near]);
        assert!((o[2] - h0).abs() < 0.1 && (o[1] - 0.1).abs() < 0.01 && (o[0] - 0.6).abs() < 0.02, "{o:?}");
        let k = PointK::new(&PointColor { variance: 100.0, ..p });
        let o = k.apply([0.62, 0.11, near]);
        assert!(o[2] - h0 > 0.1, "{o:?}");
    }

    #[test]
    fn grading_tints_shadows_only() {
        let mut s = DevelopSettings::default();
        s.grading.shadows = lightcraft_develop::Wheel { hue: 220.0, sat: 60.0, lum: 0.0 };
        let ops = ColorOps::new(&s);
        let dark = ops.apply([0.01, 0.01, 0.01], 0.0, 0.0);
        let bright = ops.apply([0.8, 0.8, 0.8], 0.0, 0.0);
        assert!(dark[2] > dark[0]);
        assert!((bright[2] - bright[0]).abs() < 0.02);
    }
}
