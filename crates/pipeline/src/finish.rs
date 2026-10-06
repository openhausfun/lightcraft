//! The per-pixel stage: everything after the spatial planes are ready, in one parallel pass.

use lightcraft_color::luminance_2020;
use lightcraft_color::spline::{Lut1, MonotoneCurve};
use lightcraft_color::transfer::linear_to_srgb;
use lightcraft_develop::{DevelopSettings, LocalAdjustments, ToneCurve, VignetteStyle};
use lightcraft_geom::Point;
use lightcraft_raster::Rgba8;

use crate::colorops::ColorOps;
use crate::geometry::Frame;
use crate::local::log_lum;
use crate::output::{DeepImage, DeepSamples, OutputDepth, OutputSpace, OutputTrc};
use crate::tone::ToneMap;
use crate::{Prepared, SourceInfo, for_rows};

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Parametric region curve (encoded domain) composed with the master point curve.
fn curve_luts(c: &ToneCurve) -> Option<[Lut1; 3]> {
    let parametric = c.highlights != 0.0 || c.lights != 0.0 || c.darks != 0.0 || c.shadows != 0.0;
    let master = !ToneCurve::point_curve_is_identity(&c.master);
    let chans = [&c.red, &c.green, &c.blue].map(|p| !ToneCurve::point_curve_is_identity(p));
    if !parametric && !master && !chans.iter().any(|b| *b) {
        return None;
    }
    const N: usize = 1024;
    let (s1, s2, s3) = ((c.split_shadows / 100.0) as f32, (c.split_mid / 100.0) as f32, (c.split_highlights / 100.0) as f32);
    let regions = [(0.0, s1, c.shadows), (s1, s2, c.darks), (s2, s3, c.lights), (s3, 1.0, c.highlights)];
    let mut base = Lut1::from_fn(N, |x| {
        let mut d = 0.0;
        for (a, b, amt) in regions {
            if amt == 0.0 {
                continue;
            }
            let (ctr, half) = ((a + b) / 2.0, (b - a) * 0.75 + 0.05);
            let t = ((x - ctr) / half).clamp(-1.0, 1.0);
            let win = 0.5 + 0.5 * (t * std::f32::consts::PI).cos();
            d += (amt / 100.0) as f32 * 0.22 * win;
        }
        (x + d * 4.0 * x * (1.0 - x)).clamp(0.0, 1.0)
    });
    // keep monotone
    for i in 1..N {
        base.v[i] = base.v[i].max(base.v[i - 1]);
    }
    let to_pts = |p: &[Point]| p.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>();
    if master {
        base = MonotoneCurve::new(&to_pts(&c.master)).to_lut(N).compose(&base);
    }
    let per = [&c.red, &c.green, &c.blue];
    Some(std::array::from_fn(|i| if chans[i] { MonotoneCurve::new(&to_pts(per[i])).to_lut(N).compose(&base) } else { base.clone() }))
}

/// Vignette (post-crop) parameters.
#[derive(Clone, Copy, Debug)]
pub struct Vig {
    pub amount: f32,
    pub start: f32,
    pub width: f32,
    pub aspect_mix: f32,
    pub power: f32,
    pub highlights: f32,
    pub style: VignetteStyle,
}

fn vignette(s: &DevelopSettings) -> Option<Vig> {
    let v = &s.vignette;
    (v.amount != 0.0).then(|| {
        let r = (v.roundness / 100.0) as f32;
        Vig {
            amount: (v.amount / 100.0) as f32,
            start: 0.15 + (v.midpoint / 100.0) as f32 * 0.95,
            width: 0.05 + (v.feather / 100.0) as f32 * 1.1,
            aspect_mix: ((r + 1.0) / 2.0).clamp(0.0, 1.0),
            power: if r >= 0.0 { 2.0 } else { 2.0 + (-r) * 6.0 },
            highlights: (v.highlights / 100.0) as f32,
            style: v.style,
        }
    })
}

/// Hash constants of [`grain_noise`] (shared with the GPU kernel).
pub const GRAIN_HASH: [u32; 4] = [0x8da6_b343, 0xd816_3841, 0xcb1a_b31f, 0x5bd1_e995];

#[inline]
fn grain_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let h = |i: i32, j: i32| {
        let mut v = (i as u32).wrapping_mul(GRAIN_HASH[0]) ^ (j as u32).wrapping_mul(GRAIN_HASH[1]) ^ seed.wrapping_mul(GRAIN_HASH[2]);
        v ^= v >> 13;
        v = v.wrapping_mul(GRAIN_HASH[3]);
        v ^= v >> 15;
        (v & 0xffff) as f32 / 32768.0 - 1.0
    };
    let (i, j) = (x0 as i32, y0 as i32);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = h(i, j) + (h(i + 1, j) - h(i, j)) * u;
    let b = h(i, j + 1) + (h(i + 1, j + 1) - h(i, j + 1)) * u;
    a + (b - a) * v
}

/// Number of per-mask terms in [`mask_terms`].
pub const MASK_TERMS: usize = 21;

/// Number of alpha-weighted terms at the start of [`mask_terms`].
pub const MASK_SUMS: usize = 17;

/// A mask's local adjustments as the per-pixel stage uses them (alpha-weighted sums), in order:
/// exposure, temp, tint, contrast, highlights, shadows, whites, blacks, texture, clarity, dehaze,
/// saturation, hue, sharpness, noise, moiré, defringe, then the colour overlay: on (0/1), cos and
/// sin of its OkLCh hue, and its strength.
pub fn mask_terms(j: &LocalAdjustments) -> [f32; MASK_TERMS] {
    let (on, cos, sin, amt) = if j.color_sat > 0.0 {
        let hue = crate::colorops::oklch_hue_of_srgb_hue(j.color_hue);
        (1.0, hue.cos(), hue.sin(), (j.color_sat / 100.0) as f32)
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };
    [
        j.exposure as f32,
        (j.temp / 100.0) as f32,
        (j.tint / 100.0) as f32,
        (j.contrast / 100.0) as f32,
        (j.highlights / 100.0) as f32,
        (j.shadows / 100.0) as f32,
        (j.whites / 100.0) as f32,
        (j.blacks / 100.0) as f32,
        (j.texture / 100.0) as f32,
        (j.clarity / 100.0) as f32,
        (j.dehaze / 100.0) as f32,
        (j.saturation / 100.0) as f32,
        (j.hue / 100.0) as f32 * 0.6,
        (j.sharpness / 100.0) as f32,
        (j.noise / 100.0) as f32,
        (j.moire / 100.0) as f32,
        (j.defringe / 100.0) as f32,
        on,
        cos,
        sin,
        amt,
    ]
}

/// Everything the per-pixel stage computes once per render: the CPU loop below and the GPU kernel
/// (`lightcraft-gpu`) both read their parameters from here, so the two cannot drift apart.
pub struct FinishParams {
    /// A LUT profile and its amount (0..2), applied to the display-encoded colour.
    pub lut: Option<(std::sync::Arc<crate::lut::Lut3d>, f32)>,
    pub tone: ToneMap,
    pub ops: ColorOps,
    /// Calibration: primaries matrix (row-major, linear Rec.2020) and shadows tint (−1..1).
    pub calib: Option<[[f32; 3]; 3]>,
    pub shadow_tint: f32,
    /// Tone curves (parametric ∘ point, per channel) on encoded values, 1024 entries each.
    pub curves: Option<[Lut1; 3]>,
    /// Refine Saturation as 0..1 (1 = the curves' own saturation).
    pub refine_sat: f32,
    pub vig: Option<Vig>,
    /// Linear Rec.2020 → linear output RGB, the output's luminance weights (gamut mapping) and its
    /// encoding curve (see [`crate::output`]).
    pub to_out: [[f32; 3]; 3],
    pub out_luma: [f32; 3],
    pub out_trc: OutputTrc,
    /// Soft proofing (CPU only; the GPU path declines proof renders).
    pub proof: Option<crate::output::ProofParams>,
    pub hl: f32,
    pub sh: f32,
    pub clar: f32,
    pub tex: f32,
    pub dehaze: f32,
    pub sharpen: f32,
    pub sharpen_mask: f32,
    /// Airlight after and before exposure, exposure gain and EV (see [`crate::Prepared`]).
    pub air: f32,
    pub air_pre: f32,
    pub gain: f32,
    pub ev: f32,
    /// Grain: amount, cell size (px), roughness, seed.
    pub grain: Option<(f32, f32, f32, u32)>,
    /// Output px → normalized oriented coordinates.
    pub out_to_norm: lightcraft_geom::Affine,
    pub ow: f64,
    pub oh: f64,
    pub w: usize,
    pub h: usize,
    pub px_per_long: f64,
}

impl FinishParams {
    /// Parameters for a `w × h` render into `space`; `ev`/`air_pre` as in [`crate::Prepared`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        s: &DevelopSettings,
        frame: &Frame,
        info: &SourceInfo,
        w: usize,
        h: usize,
        px_per_long: f64,
        air_pre: f32,
        space: OutputSpace,
    ) -> FinishParams {
        let effects = s.section_enabled("effects");
        let (clar, tex, dehaze) = if effects {
            ((s.effects.clarity / 100.0) as f32, (s.effects.texture / 100.0) as f32, (s.effects.dehaze / 100.0) as f32)
        } else {
            (0.0, 0.0, 0.0)
        };
        let ev = s.light.exposure as f32;
        let gain = 2f32.powf(ev);
        let grain = (s.grain.amount > 0.0 && effects).then(|| {
            let cell = (0.0006 + (s.grain.size / 100.0) as f32 * 0.0024) * px_per_long as f32;
            ((s.grain.amount / 100.0) as f32 * 0.13, cell.max(0.6), (s.grain.roughness / 100.0) as f32, s.grain.seed)
        });
        let calibration = s.section_enabled("calibration");
        FinishParams {
            calib: if calibration { crate::colorops::calibration_matrix(&s.calibration) } else { None },
            shadow_tint: if calibration { (s.calibration.shadows_tint / 100.0) as f32 } else { 0.0 },
            tone: if let Some(curve) = info.camera_tone.as_ref().filter(|_| info.raw) {
                ToneMap::camera(curve, s.light.contrast, s.light.whites, s.light.blacks)
            } else if info.raw {
                ToneMap::new(s.light.contrast, s.light.whites, s.light.blacks)
            } else {
                ToneMap::display(s.light.contrast, s.light.whites, s.light.blacks)
            },
            lut: crate::lut::get(&s.profile.id).map(|l| (l, (s.profile.amount / 100.0).clamp(0.0, 2.0) as f32)),
            ops: ColorOps::new(s),
            curves: curve_luts(&s.curve),
            refine_sat: (s.curve.refine_saturation / 100.0).clamp(0.0, 1.0) as f32,
            vig: if effects { vignette(s) } else { None },
            to_out: space.from_working(),
            out_luma: space.luma(),
            out_trc: space.trc(),
            proof: None,
            hl: (s.light.highlights / 100.0) as f32,
            sh: (s.light.shadows / 100.0) as f32,
            clar,
            tex,
            dehaze,
            sharpen: (s.detail.sharpen_amount / 150.0) as f32,
            sharpen_mask: (s.detail.sharpen_masking / 100.0) as f32,
            air: air_pre * gain,
            air_pre,
            gain,
            ev,
            grain,
            out_to_norm: frame.out_to_norm(w, h),
            ow: frame.ow,
            oh: frame.oh,
            w,
            h,
            px_per_long,
        }
    }
}

pub(crate) fn finish(p: &Prepared, s: &DevelopSettings, frame: &Frame, info: &SourceInfo, space: OutputSpace, proof: Option<crate::Proof>) -> Rgba8 {
    let (w, h) = (p.img.width, p.img.height);
    let mut fp = FinishParams::new(s, frame, info, w, h, p.px_per_long, p.air, space);
    fp.proof = proof.map(|pr| pr.params(space));
    let trc = fp.out_trc;
    let data = finish_with(p, &fp, false, |e| match trc {
        OutputTrc::Srgb => [enc(e[0]), enc(e[1]), enc(e[2]), 255],
        t => {
            let x = e.map(|v| enc(t.encode(lightcraft_color::transfer::srgb_to_linear(v.clamp(0.0, 1.0)))));
            [x[0], x[1], x[2], 255]
        }
    });
    Rgba8 { width: w, height: h, data }
}

/// [`finish`] into 16-bit display-encoded or 32-bit float linear samples (output primaries).
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_deep(
    p: &Prepared,
    s: &DevelopSettings,
    frame: &Frame,
    info: &SourceInfo,
    space: OutputSpace,
    depth: OutputDepth,
    proof: Option<crate::Proof>,
) -> DeepImage {
    use lightcraft_color::transfer::srgb_to_linear;
    let (w, h) = (p.img.width, p.img.height);
    let mut fp = FinishParams::new(s, frame, info, w, h, p.px_per_long, p.air, space);
    fp.proof = proof.map(|pr| pr.params(space));
    let trc = fp.out_trc;
    let samples = match depth {
        OutputDepth::F32Linear => {
            let v = finish_with(p, &fp, true, |e| e.map(|v| srgb_to_linear(v.clamp(0.0, 1.0))));
            DeepSamples::F32(v.into_flattened())
        }
        _ => {
            let q = |v: f32| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            let v = finish_with(p, &fp, true, |e| match trc {
                OutputTrc::Srgb => e.map(q),
                t => e.map(|v| q(t.encode(srgb_to_linear(v.clamp(0.0, 1.0))))),
            });
            DeepSamples::U16(v.into_flattened())
        }
    };
    DeepImage { width: w, height: h, space, samples }
}

/// The per-pixel stage: every output pixel's colour in the output primaries, encoded with the sRGB
/// curve (tone curves and grain applied; not clamped), handed to `store` for the final encoding.
/// `exact`: encode with the exact sRGB curve instead of the (8/10-bit accurate) table.
pub(crate) fn finish_with<T: Copy + Default + Send>(
    p: &Prepared,
    fp: &FinishParams,
    exact: bool,
    store: impl Fn([f32; 3]) -> T + Sync + Send,
) -> Vec<T> {
    let (w, h) = (p.img.width, p.img.height);
    let p_lut = fp.lut.clone();
    let FinishParams {
        tone,
        ops,
        curves,
        vig,
        to_out,
        out_luma,
        hl,
        sh,
        clar,
        tex,
        dehaze,
        sharpen,
        sharpen_mask,
        air,
        air_pre,
        gain,
        ev,
        grain,
        ..
    } = fp;
    let (hl, sh, clar, tex, dehaze, sharpen, sharpen_mask, air, air_pre, gain, ev) =
        (*hl, *sh, *clar, *tex, *dehaze, *sharpen, *sharpen_mask, *air, *air_pre, *gain, *ev);
    let terms: Vec<[f32; MASK_TERMS]> = p.masks.iter().map(|m| mask_terms(&m.adjust)).collect();
    let out_to_norm = fp.out_to_norm;
    let long = fp.ow.max(fp.oh);
    let aspect = w as f32 / h as f32;

    let srgb = srgb_lut();
    let mut out = vec![T::default(); w * h];
    for_rows(&mut out, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let raw = p.img.data[i];
            let mut c = if gain == 1.0 { raw } else { raw.map(|v| v * gain) };
            let l_pre = p.log_l.data[i];
            let l0 = l_pre + ev;

            // --- local (mask) contributions: alpha-weighted sums of the masks' terms
            let mut l = [0.0f32; MASK_SUMS];
            let mut tint_col: Option<([f32; 3], f32)> = None;
            for (m, t) in p.masks.iter().zip(&terms) {
                let a = m.alpha.data[i];
                if a <= 0.0 {
                    continue;
                }
                for k in 0..MASK_SUMS {
                    l[k] += a * t[k];
                }
                if t[MASK_SUMS] > 0.0 {
                    tint_col = Some(([t[MASK_SUMS + 1], t[MASK_SUMS + 2], 0.0], a * t[MASK_SUMS + 3]));
                }
            }
            let [l_exp, l_temp, l_tint, l_con, l_hl, l_sh, l_wh, l_bl, l_tex, l_clar, l_dehaze, l_sat, l_hue, l_sharp, l_noise, l_moire, l_defringe] =
                l;

            // --- local Moiré (and the colour part of Noise): chromaticity towards its blur
            let mt = (l_moire + 0.5 * l_noise.max(0.0)).clamp(-1.0, 1.0);
            if mt != 0.0
                && let Some(cb) = &p.chroma_blur
            {
                let (y, ch0, cb) = (luminance_2020(c), crate::masks::chromaticity(raw), cb.data[i]);
                c = std::array::from_fn(|k| ((ch0[k] + (cb[k] - ch0[k]) * mt) * y).max(0.0));
            }
            // --- local Defringe: desaturate purple / green fringes along edges
            let df = l_defringe.clamp(0.0, 1.0);
            if df > 0.0
                && let Some(b) = &p.texture_blur
            {
                let k = df * defringe_weight(c, p.log_l.data[i] - b.data[i]);
                if k > 0.0 {
                    let y = luminance_2020(c);
                    c = c.map(|v| v + (y - v) * k);
                }
            }

            // --- dehaze (scene linear)
            let dz = dehaze + l_dehaze;
            if dz != 0.0
                && let Some(dark) = &p.dark
            {
                let d = (dark.data[i] / air_pre).clamp(0.0, 1.0);
                if dz > 0.0 {
                    let t = (1.0 - 0.95 * dz.min(1.0) * d).max(0.12);
                    c = c.map(|v| ((v - air * (1.0 - t)) / t).max(0.0));
                } else {
                    let k = (-dz).min(1.0) * 0.7 * (0.35 + 0.65 * d);
                    c = c.map(|v| v + (air * 0.9 - v) * k);
                }
            }

            // --- local exposure / temp / tint
            if l_exp != 0.0 {
                let g = l_exp.exp2();
                c = c.map(|v| v * g);
            }
            if l_temp != 0.0 || l_tint != 0.0 {
                let y0 = luminance_2020(c);
                c = [c[0] * (1.0 + 0.3 * l_temp), c[1] * (1.0 - 0.22 * l_tint), c[2] * (1.0 - 0.3 * l_temp).max(0.0)];
                let y1 = luminance_2020(c).max(1e-9);
                c = c.map(|v| v * y0 / y1);
            }

            // --- local tone in log luminance
            let l1 = if dz != 0.0 || l_exp != 0.0 { log_lum(c) } else { l0 };
            let shift = l1 - l0;
            let base = p.base.data[i] + ev + shift;
            let mut delta = 0.0f32;
            let (hh, ss) = (hl + l_hl, sh + l_sh);
            if hh != 0.0 || ss != 0.0 {
                let ws = 1.0 - smooth(-4.8, 0.3, base);
                let wh = smooth(-1.0, 2.8, base);
                delta += ss * 1.7 * ws * ws.sqrt() + hh * 1.7 * wh;
            }
            if l_wh != 0.0 {
                delta += l_wh * 0.8 * smooth(0.5, 3.0, l1);
            }
            if l_bl != 0.0 {
                delta += l_bl * 0.8 * (1.0 - smooth(-6.0, -1.5, l1));
            }
            if l_con != 0.0 {
                delta += l_con * 0.14 * l1.clamp(-6.0, 4.0);
            }
            let cl = clar + l_clar;
            if cl != 0.0
                && let Some(b) = &p.clarity_blur
            {
                let det = (l_pre - b.data[i]).clamp(-2.5, 2.5);
                let mid = (-(base / 3.2).powi(2)).exp();
                delta += cl * 0.85 * det * (0.35 + 0.65 * mid);
            }
            let tx = tex + l_tex;
            let sp = l_sharp * 0.6 + sharpen;
            if (tx != 0.0 || sp != 0.0)
                && let Some(b) = &p.texture_blur
            {
                let det = l_pre - b.data[i];
                let tame = 1.0 - 0.6 * smooth(0.4, 1.6, det.abs());
                delta += tx * 1.1 * det.clamp(-1.0, 1.0) * tame;
                if sp != 0.0 {
                    let m = if sharpen_mask > 0.0 { smooth(sharpen_mask * 0.25, sharpen_mask * 0.25 + 0.15, det.abs()) } else { 1.0 };
                    delta += sp * 1.3 * det.clamp(-0.8, 0.8) * m;
                }
            }
            // local Noise: smooth (or, negative, boost) small-amplitude detail, keep edges
            if l_noise != 0.0
                && let Some(b) = &p.texture_blur
            {
                let det = l_pre - b.data[i];
                delta -= l_noise.clamp(-1.0, 1.0) * 0.9 * det * (1.0 - smooth(0.1, 0.5, det.abs()));
            }
            if delta != 0.0 {
                let g = delta.exp2();
                c = c.map(|v| v * g);
            }

            // --- calibration (scene linear, before the tone map)
            if fp.calib.is_some() || fp.shadow_tint != 0.0 {
                c = crate::colorops::calibrate(c, fp.calib.as_ref(), fp.shadow_tint);
            }

            // --- tone map on luminance, highlight desaturation
            let yl = luminance_2020(c);
            let o = tone.apply(yl);
            let mut d = if yl > 1e-9 { c.map(|v| v * o / yl) } else { [0.0; 3] };
            let mx = d[0].max(d[1]).max(d[2]);
            if mx > 1.0 {
                let t = ((mx - 1.0) / (mx - o).max(1e-6)).clamp(0.0, 1.0);
                d = d.map(|v| v + (o - v) * t);
            }

            // --- colour
            d = ops.apply(d, l_sat, l_hue);
            if let Some((dir, amt)) = tint_col {
                let lab = lightcraft_color::perceptual::oklab_from_2020(d);
                d = lightcraft_color::perceptual::oklab_to_2020([lab[0], lab[1] + dir[0] * 0.08 * amt, lab[2] + dir[1] * 0.08 * amt]);
            }

            // --- vignette (display linear, post-crop)
            if let Some(v) = vig {
                let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let vv = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
                let sx = 1.0 + (aspect - 1.0) * v.aspect_mix;
                let sy = 1.0 + (1.0 / aspect - 1.0) * v.aspect_mix;
                let (ax, ay) = ((u * sx.max(1.0) / sx.max(sy)).abs(), (vv * sy.max(1.0) / sx.max(sy)).abs());
                let dist = (ax.powf(v.power) + ay.powf(v.power)).powf(1.0 / v.power);
                let t = smooth(v.start, v.start + v.width, dist);
                if t > 0.0 {
                    let lum = luminance_2020(d).clamp(0.0, 1.0);
                    if v.amount < 0.0 {
                        let mut f = 1.0 + v.amount * t;
                        if v.style == VignetteStyle::HighlightPriority {
                            f += (1.0 - f) * v.highlights * smooth(0.4, 1.0, lum);
                        }
                        if v.style == VignetteStyle::PaintOverlay {
                            d = d.map(|c| c * (1.0 - (-v.amount) * t) + 0.0);
                        } else {
                            d = d.map(|c| c * f);
                        }
                    } else {
                        d = d.map(|c| c + (1.0 - c) * v.amount * t * 0.85);
                    }
                }
            }

            // --- gamut map to the output space (desaturate towards luminance until in range);
            // soft proofing maps into the proof space first and shows that in the output space
            let mut warn = None;
            let mut r = match &fp.proof {
                Some(pp) => {
                    let q0 = mul3(&pp.to_proof, d);
                    let (q, t) = gamut_map(q0, pp.luma);
                    if pp.dest_warning && out_of_gamut(q0, t) {
                        warn = Some(crate::output::PROOF_DEST_WARNING);
                    }
                    mul3(&pp.proof_to_out, q)
                }
                None => mul3(to_out, d),
            };
            let (mapped, t) = gamut_map(r, *out_luma);
            if fp.proof.is_some_and(|pp| pp.display_warning) && warn.is_none() && out_of_gamut(r, t) {
                warn = Some(crate::output::PROOF_DISPLAY_WARNING);
            }
            r = mapped;

            // --- encode, curves, grain
            let mut e = if exact { r.map(|v| linear_to_srgb(v.clamp(0.0, 1.0))) } else { r.map(|v| encode_srgb(srgb, v)) };
            if let Some(l) = curves {
                let e0 = e;
                e = [l[0].eval(e[0]), l[1].eval(e[1]), l[2].eval(e[2])];
                if fp.refine_sat < 1.0 {
                    e = refine_saturation(e0, e, fp.refine_sat);
                }
            }
            if let Some((amt, cell, rough, seed)) = *grain {
                let n = out_to_norm.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                let (gx, gy) = ((n.x * fp.ow) as f32 / long as f32, (n.y * fp.oh) as f32 / long as f32);
                let sc = fp.px_per_long as f32 / cell;
                let mut g = grain_noise(gx * sc, gy * sc, seed);
                g = g * (1.0 - rough * 0.5) + grain_noise(gx * sc * 2.3, gy * sc * 2.3, seed ^ 0x55) * rough * 0.7;
                let lum = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
                let k = amt * g * (0.35 + 2.6 * lum * (1.0 - lum));
                e = e.map(|v| v + k);
            }
            let e = match &p_lut {
                Some((l, k)) => {
                    let m = l.apply(e.map(|v| v.clamp(0.0, 1.0)));
                    [e[0] + (m[0] - e[0]) * k, e[1] + (m[1] - e[1]) * k, e[2] + (m[2] - e[2]) * k]
                }
                None => e,
            };
            *px = store(warn.unwrap_or(e));
        }
    });
    out
}

/// A colour needing more desaturation than this (scale < `GAMUT_WARN`) to fit is out of gamut for
/// the gamut warnings (tolerates rounding at the gamut boundary).
const GAMUT_WARN: f32 = 0.995;

/// For the gamut warnings: `r` needed desaturating by `t` to fit, and isn't simply a neutral
/// beyond white or below black (that's clipping, which the clipping warnings show).
#[inline]
fn out_of_gamut(r: [f32; 3], t: f32) -> bool {
    let (lo, hi) = (r[0].min(r[1]).min(r[2]), r[0].max(r[1]).max(r[2]));
    t < GAMUT_WARN && (hi - lo) > 0.02 * hi.abs().max(1e-3) && hi - lo > 1e-3 && (lo < -1e-3 || (hi > 1.0 && lo < 1.0))
}

#[inline]
fn mul3(m: &[[f32; 3]; 3], d: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * d[0] + m[0][1] * d[1] + m[0][2] * d[2],
        m[1][0] * d[0] + m[1][1] * d[1] + m[1][2] * d[2],
        m[2][0] * d[0] + m[2][1] * d[1] + m[2][2] * d[2],
    ]
}

/// Desaturate linear `r` towards its luminance (weights `luma`) until every channel is in 0..1;
/// returns the mapped colour and the chroma scale used (1 = already in gamut).
#[inline]
pub fn gamut_map(r: [f32; 3], luma: [f32; 3]) -> ([f32; 3], f32) {
    let yy = (luma[0] * r[0] + luma[1] * r[1] + luma[2] * r[2]).clamp(0.0, 1.0);
    let mut t = 1.0f32;
    for c in r {
        if c < 0.0 {
            t = t.min(yy / (yy - c).max(1e-9));
        } else if c > 1.0 {
            t = t.min((1.0 - yy) / (c - yy).max(1e-9));
        }
    }
    if t < 1.0 { (r.map(|c| yy + (c - yy) * t), t) } else { (r, 1.0) }
}

/// Local Defringe weight of a scene-linear colour `c` whose log luminance differs by `det` from
/// its fine blur: high on strong edges with a purple or green cast.
#[inline]
pub fn defringe_weight(c: [f32; 3], det: f32) -> f32 {
    let y = luminance_2020(c).max(1e-6);
    let purple = (c[0].min(c[2]) - c[1]) / y;
    let green = (c[1] - c[0].max(c[2])) / y;
    smooth(0.04, 0.3, det.abs()) * smooth(0.02, 0.2, purple).max(smooth(0.02, 0.2, green))
}

/// Refine Saturation: scale the curved colour's chroma (around its luma, encoded values) so its
/// saturation (chroma / luma) moves from the curve's towards the pre-curve one: the ratio is
/// `(s_curve / s_before)^refine`, so 1 keeps the curve, 0 restores the original saturation.
#[inline]
pub fn refine_saturation(before: [f32; 3], after: [f32; 3], refine: f32) -> [f32; 3] {
    let luma = |e: [f32; 3]| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    let chroma = |e: [f32; 3]| e[0].max(e[1]).max(e[2]) - e[0].min(e[1]).min(e[2]);
    let (y0, y1) = (luma(before), luma(after));
    let s0 = chroma(before) / y0.max(1e-4);
    let s1 = chroma(after) / y1.max(1e-4);
    if s1 <= 1e-6 || s0 <= 1e-6 {
        return after;
    }
    let k = (s0 / s1).powf(1.0 - refine).clamp(0.0, 4.0);
    after.map(|v| y1 + (v - y1) * k)
}

pub const SRGB_LUT_N: usize = 4096;

/// Linear → sRGB-encoded table (`SRGB_LUT_N + 1` entries over 0..1), interpolated linearly.
pub fn srgb_lut() -> &'static [f32; SRGB_LUT_N + 1] {
    static LUT: std::sync::OnceLock<Box<[f32; SRGB_LUT_N + 1]>> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = Box::new([0.0f32; SRGB_LUT_N + 1]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = linear_to_srgb(i as f32 / SRGB_LUT_N as f32);
        }
        t
    })
}

/// Linear → sRGB-encoded (clamped to 0..1) by an interpolated table: within 2e-5 of the exact
/// curve (≪ one 8-bit or 10-bit step), several times faster than `powf`.
#[inline]
fn encode_srgb(lut: &[f32; SRGB_LUT_N + 1], v: f32) -> f32 {
    let f = v.clamp(0.0, 1.0) * SRGB_LUT_N as f32;
    let i = (f as usize).min(SRGB_LUT_N - 1);
    let t = f - i as f32;
    lut[i] + (lut[i + 1] - lut[i]) * t
}

#[inline]
fn enc(v: f32) -> u8 {
    // `v` is already sRGB-encoded; round to 8 bits.
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refine_saturation_restores_the_pre_curve_saturation() {
        let before = [0.5, 0.3, 0.2];
        let after = [0.7, 0.35, 0.15]; // a contrasty curve: more saturated
        assert_eq!(refine_saturation(before, after, 1.0), after);
        let r = refine_saturation(before, after, 0.0);
        let sat = |e: [f32; 3]| (e[0] - e[2]) / (0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2]);
        assert!((sat(r) - sat(before)).abs() < 1e-4, "{r:?}");
        let half = refine_saturation(before, after, 0.5);
        assert!(sat(half) > sat(before) && sat(half) < sat(after));
        // luma is kept
        let y = |e: [f32; 3]| 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
        assert!((y(r) - y(after)).abs() < 1e-5);
    }

    #[test]
    fn srgb_table_matches_the_exact_curve() {
        let lut = srgb_lut();
        let mut worst = 0.0f32;
        for i in 0..=200_000 {
            // dense near black, where the curve bends most
            let v = (i as f32 / 200_000.0).powi(3);
            worst = worst.max((encode_srgb(lut, v) - linear_to_srgb(v)).abs());
        }
        assert!(worst < 2e-5, "{worst}");
        assert_eq!(encode_srgb(lut, -1.0), 0.0);
        assert!((encode_srgb(lut, 2.0) - 1.0).abs() < 1e-6);
    }
}
