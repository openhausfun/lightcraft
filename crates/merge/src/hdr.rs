//! HDR merge of exposure brackets.
//!
//! 1. **Order and reference.** Frames are ordered by brightness (EXIF exposure, or median
//!    luminance without EXIF); the middle one is the reference (geometry, colour and the output's
//!    default brightness come from it).
//! 2. **Auto align** ([`crate::align`]): each frame is registered to the reference and resampled
//!    into its geometry.
//! 3. **Exposure normalisation.** The exposure ratio between brightness-adjacent frames is
//!    measured robustly — the median of log ratios over pixels well exposed in both — and chained
//!    to the reference; EXIF is the fallback when too few pixels overlap. Radiance
//!    `Rᵢ = vᵢ / kᵢ` is expressed in reference units.
//! 4. **Weights.** `wᵢ = kᵢ · (1 − smoothstep(0.80·clip, 0.96·clip, max channel))`: longer
//!    exposures get more weight (shot-noise optimal for a linear sensor), saturated pixels none
//!    (all channels together, so a clipped channel never shifts colour). The darkest frame keeps a
//!    tiny weight so fully saturated pixels still get a value.
//! 5. **Deghost** (None/Low/Medium/High): on a ≤ 1024 px version, a per-pixel consistency test
//!    against the reference: the reference radiance (or, where the reference is clipped/too dark,
//!    the nearest well-exposed frame's) predicts what each frame should have recorded; frames that
//!    disagree by more than a threshold (in log units, with a noise floor) are masked there. Masks
//!    are dilated and feathered (Gaussian), upsampled, and remove the frame's weight. Their union
//!    is the deghost overlay.

use lightcraft_raster::blur::gaussian;
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::align::{self, Alignment};
use crate::frame::{Frame, FrameColor};
use crate::{MergeError, Progress, Result, check};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Deghost {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Deghost {
    /// (log-ratio threshold, dilation/feather radius in deghost-map pixels).
    fn params(self) -> Option<(f32, usize)> {
        match self {
            Deghost::None => None,
            Deghost::Low => Some((1.0, 2)),
            Deghost::Medium => Some((0.6, 3)),
            Deghost::High => Some((0.35, 5)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HdrOptions {
    pub align: bool,
    pub deghost: Deghost,
}

impl Default for HdrOptions {
    fn default() -> Self {
        HdrOptions { align: true, deghost: Deghost::None }
    }
}

/// The merged result, in the reference frame's geometry and units.
#[derive(Clone, Debug)]
pub struct HdrResult {
    /// Scene-linear radiance; 1.0 = the reference frame's white level.
    pub radiance: Rgb32f,
    /// Deghost overlay (0..1, the union of the masks) at the radiance's size; empty without deghosting.
    pub ghost: Plane,
    /// Index (into the inputs) of the reference frame.
    pub reference: usize,
    /// Exposure of each input relative to the reference, in EV (as used for the merge).
    pub ev: Vec<f64>,
    /// The same from EXIF, when every frame had it.
    pub ev_exif: Option<Vec<f64>>,
    pub alignments: Vec<Alignment>,
    pub color: FrameColor,
    pub raw: bool,
    pub orientation: lightcraft_geom::Orientation,
    pub metadata: lightcraft_meta::Metadata,
    pub baseline_exposure: f64,
}

#[inline]
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn maxc(p: [f32; 3]) -> f32 {
    p[0].max(p[1]).max(p[2])
}

/// Merge exposure brackets. Frames must share their pixel size (and orientation).
pub fn merge_hdr(mut frames: Vec<Frame>, opts: &HdrOptions, progress: &Progress) -> Result<HdrResult> {
    let n = frames.len();
    if n < 2 {
        return Err(MergeError::TooFew(n, 2));
    }
    let (w, h) = (frames[0].width(), frames[0].height());
    if frames.iter().any(|f| f.width() != w || f.height() != h) {
        return Err(MergeError::Mismatch("HDR merge needs photos of the same size".into()));
    }
    if frames.iter().any(|f| f.orientation != frames[0].orientation) {
        for f in frames.iter_mut() {
            f.orient();
        }
        let (w0, h0) = (frames[0].width(), frames[0].height());
        if frames.iter().any(|f| f.width() != w0 || f.height() != h0) {
            return Err(MergeError::Mismatch("HDR merge needs photos of the same size".into()));
        }
    }
    let (w, h) = (frames[0].width(), frames[0].height());
    check(progress, 0.02, "Analysing exposures")?;

    // 1. brightness order and reference
    let exif: Option<Vec<f64>> = frames.iter().map(|f| f.exposure).collect();
    let medians: Vec<f32> = frames.par_iter().map(|f| align::median_luma(&f.image, f.clip)).collect();
    let brightness: Vec<f64> = match &exif {
        Some(e) if e.iter().any(|v| (v - e[0]).abs() > 0.1) => e.clone(),
        _ => medians.iter().map(|m| (*m as f64).log2()).collect(),
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| brightness[a].total_cmp(&brightness[b]).then(a.cmp(&b)));
    let reference = order[n / 2];

    // 2. alignment
    let mut alignments = vec![Alignment::identity(); n];
    let mut valid: Vec<Option<Vec<bool>>> = vec![None; n];
    if opts.align {
        check(progress, 0.05, "Aligning")?;
        let g0 = 0.18 / medians[reference].max(1e-6);
        let rel: Vec<f32> = (0..n).map(|i| 2f64.powf(brightness[reference] - brightness[i]) as f32).collect();
        let rel: Vec<f32> = if exif.is_some() && brightness.iter().any(|b| (b - brightness[0]).abs() > 0.1) {
            rel
        } else {
            (0..n).map(|i| medians[reference] / medians[i].max(1e-6)).collect()
        };
        let feats: Vec<(crate::features::Features, f64)> = frames
            .par_iter()
            .enumerate()
            .map(|(i, f)| {
                let (p, s) = align::work_image(&f.image, g0 * rel[i], align::WORK_EDGE);
                (crate::features::detect(&p, 1500), s)
            })
            .collect();
        check(progress, 0.25, "Aligning")?;
        let al: Vec<(usize, Alignment)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference)
            .map(|i| (i, align::align_pair(&feats[reference].0, &feats[i].0, feats[i].1, w, h, i as u64 + 1)))
            .collect();
        for (i, a) in al {
            alignments[i] = a;
        }
        check(progress, 0.35, "Resampling")?;
        let warped: Vec<(usize, Rgb32f, Vec<bool>)> = (0..n)
            .into_par_iter()
            .filter(|&i| i != reference && alignments[i].model != "identity")
            .map(|i| {
                let (img, v) = align::warp(&frames[i].image, &alignments[i].h);
                (i, img, v)
            })
            .collect();
        for (i, img, v) in warped {
            frames[i].image = img;
            valid[i] = Some(v);
        }
    }
    check(progress, 0.45, "Normalising exposures")?;

    // 3. exposure ratios between brightness neighbours, chained to the reference (ln units)
    let mut step_ln = vec![0f64; n.saturating_sub(1)];
    for p in 0..n - 1 {
        let (a, b) = (order[p], order[p + 1]);
        let est = log_ratio(&frames[a], valid[a].as_deref(), &frames[b], valid[b].as_deref());
        step_ln[p] = match (est, &exif) {
            (Some(r), _) => r,
            (None, Some(e)) => (e[b] - e[a]) * std::f64::consts::LN_2,
            (None, None) => ((medians[b] / medians[a].max(1e-6)) as f64).max(1e-6).ln(),
        };
    }
    let mut ln_k = vec![0f64; n];
    let rpos = n / 2;
    for p in (0..rpos).rev() {
        ln_k[order[p]] = ln_k[order[p + 1]] - step_ln[p];
    }
    for p in rpos + 1..n {
        ln_k[order[p]] = ln_k[order[p - 1]] + step_ln[p - 1];
    }
    let k: Vec<f32> = ln_k.iter().map(|v| v.exp() as f32).collect();
    let ev: Vec<f64> = ln_k.iter().map(|v| v / std::f64::consts::LN_2).collect();
    let ev_exif = exif.as_ref().map(|e| e.iter().map(|v| v - e[reference]).collect());

    // 4./5. deghost maps (small scale)
    check(progress, 0.55, "Deghosting")?;
    let ghost_small: Option<(Vec<Plane>, usize)> = opts.deghost.params().map(|(thr, r)| deghost_maps(&frames, &valid, &k, &order, reference, thr, r));
    check(progress, 0.7, "Merging")?;

    let darkest = order[0];
    let clips: Vec<f32> = frames.iter().map(|f| f.clip).collect();
    let mut out = Rgb32f::new(w, h);
    let fr = &frames;
    let vd = &valid;
    let gs = ghost_small.as_ref();
    out.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for x in 0..w {
            let i_px = y * w + x;
            let mut acc = [0f64; 3];
            let mut ws = 0f64;
            for i in 0..n {
                if vd[i].as_ref().is_some_and(|v| !v[i_px]) {
                    continue;
                }
                let v = fr[i].image.data[i_px];
                let m = maxc(v);
                let mut wt = k[i] * (1.0 - smoothstep(0.80 * clips[i], 0.96 * clips[i], m));
                if i == darkest {
                    wt += 1e-6;
                }
                if i == reference {
                    wt += 1e-9;
                }
                if i != reference
                    && let Some((maps, fac)) = gs
                {
                    let g = sample_plane(&maps[i], (x as f32 + 0.5) / *fac as f32, (y as f32 + 0.5) / *fac as f32);
                    wt *= 1.0 - g;
                }
                if wt <= 0.0 {
                    continue;
                }
                let inv = 1.0 / k[i];
                for c in 0..3 {
                    acc[c] += (wt * v[c] * inv) as f64;
                }
                ws += wt as f64;
            }
            if ws > 0.0 {
                row[x] = acc.map(|a| (a / ws) as f32);
            } else {
                row[x] = fr[reference].image.data[i_px];
            }
        }
    });
    check(progress, 0.92, "Merging")?;

    let ghost = match &ghost_small {
        Some((maps, fac)) => {
            let fac = *fac as f32;
            let mut g = Plane::new(w, h);
            g.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
                for (x, o) in row.iter_mut().enumerate() {
                    let (sx, sy) = ((x as f32 + 0.5) / fac, (y as f32 + 0.5) / fac);
                    *o = (0..n).filter(|&i| i != reference).map(|i| sample_plane(&maps[i], sx, sy)).fold(0.0, f32::max);
                }
            });
            g
        }
        None => Plane::new(0, 0),
    };
    let rf = &frames[reference];
    Ok(HdrResult {
        radiance: out,
        ghost,
        reference,
        ev,
        ev_exif,
        alignments,
        color: rf.color.clone(),
        raw: rf.raw,
        orientation: rf.orientation,
        metadata: rf.metadata.clone(),
        baseline_exposure: rf.baseline_exposure,
    })
}

/// Bilinear sample of a plane at continuous coordinates (pixel centres at +0.5).
pub(crate) fn sample_plane(p: &Plane, x: f32, y: f32) -> f32 {
    if p.width == 0 || p.height == 0 {
        return 0.0;
    }
    crate::features::bilinear(p, x, y)
}

/// Median of ln(b / a) on the green channel over pixels well exposed in both (≥ 500 samples).
fn log_ratio(a: &Frame, va: Option<&[bool]>, b: &Frame, vb: Option<&[bool]>) -> Option<f64> {
    let len = a.image.data.len();
    let step = (len / 400_000).max(1);
    let (ca, cb) = (a.clip, b.clip);
    let lo = |f: &Frame| if f.raw { 0.01 } else { 0.04 };
    let (la, lb) = (lo(a) * ca, lo(b) * cb);
    let mut v: Vec<f32> = (0..len)
        .into_par_iter()
        .step_by(step)
        .filter_map(|i| {
            if va.is_some_and(|v| !v[i]) || vb.is_some_and(|v| !v[i]) {
                return None;
            }
            let (pa, pb) = (a.image.data[i], b.image.data[i]);
            if maxc(pa) >= 0.8 * ca || maxc(pb) >= 0.8 * cb || pa[1] <= la || pb[1] <= lb {
                return None;
            }
            Some((pb[1] / pa[1]).ln())
        })
        .collect();
    if v.len() < 500 {
        return None;
    }
    v.sort_by(|x, y| x.total_cmp(y));
    Some(v[v.len() / 2] as f64)
}

/// Box-downsample by `fac` (with the fraction of valid source pixels).
fn shrink(img: &Rgb32f, valid: Option<&[bool]>, fac: usize) -> (Rgb32f, Vec<f32>) {
    let (w, h) = (img.width.div_ceil(fac), img.height.div_ceil(fac));
    let mut out = Rgb32f::new(w, h);
    let mut vf = vec![0f32; w * h];
    out.data.par_chunks_mut(w).zip(vf.par_chunks_mut(w)).enumerate().for_each(|(y, (row, vrow))| {
        for x in 0..w {
            let mut s = [0f32; 3];
            let (mut nv, mut nt) = (0usize, 0usize);
            for sy in y * fac..((y + 1) * fac).min(img.height) {
                for sx in x * fac..((x + 1) * fac).min(img.width) {
                    let i = sy * img.width + sx;
                    nt += 1;
                    if valid.is_none_or(|v| v[i]) {
                        let p = img.data[i];
                        for c in 0..3 {
                            s[c] += p[c];
                        }
                        nv += 1;
                    }
                }
            }
            if nv > 0 {
                row[x] = s.map(|v| v / nv as f32);
            }
            vrow[x] = nv as f32 / nt.max(1) as f32;
        }
    });
    (out, vf)
}

/// Per-frame ghost masks (0..1) at 1/`fac` scale; the reference's is all zero.
fn deghost_maps(
    frames: &[Frame],
    valid: &[Option<Vec<bool>>],
    k: &[f32],
    order: &[usize],
    reference: usize,
    thr: f32,
    r: usize,
) -> (Vec<Plane>, usize) {
    let n = frames.len();
    let (w, h) = (frames[0].width(), frames[0].height());
    let fac = w.max(h).div_ceil(1024).max(1);
    let small: Vec<(Rgb32f, Vec<f32>)> = (0..n).into_par_iter().map(|i| shrink(&frames[i].image, valid[i].as_deref(), fac)).collect();
    let (sw, sh) = (small[0].0.width, small[0].0.height);
    let rpos = order.iter().position(|&i| i == reference).unwrap_or(0);
    let good = |i: usize, p: usize| -> bool {
        let f = &frames[i];
        let m = maxc(small[i].0.data[p]);
        let dark = if f.raw { 0.01 } else { 0.04 };
        small[i].1[p] > 0.99 && m < 0.8 * f.clip && m > dark * f.clip
    };
    // predicted reference radiance per pixel
    let pred: Vec<[f32; 3]> = (0..sw * sh)
        .into_par_iter()
        .map(|p| {
            let rad = |i: usize| small[i].0.data[p].map(|v| v / k[i]);
            if good(reference, p) {
                return rad(reference);
            }
            let bright = maxc(small[reference].0.data[p]) >= 0.8 * frames[reference].clip;
            if bright {
                for q in (0..rpos).rev() {
                    if good(order[q], p) {
                        return rad(order[q]);
                    }
                }
                rad(order[0])
            } else {
                for &i in &order[rpos + 1..] {
                    if good(i, p) {
                        return rad(i);
                    }
                }
                rad(reference)
            }
        })
        .collect();
    let maps: Vec<Plane> = (0..n)
        .into_par_iter()
        .map(|i| {
            if i == reference {
                return Plane::new(sw, sh);
            }
            let f = &frames[i];
            let e = if f.raw { 0.01 } else { 0.03 } * f.clip;
            let mut m = Plane::new(sw, sh);
            for p in 0..sw * sh {
                if small[i].1[p] < 0.5 {
                    continue;
                }
                let obs = small[i].0.data[p];
                let mut d = 0f32;
                for c in 0..3 {
                    let o = obs[c].min(f.clip);
                    let pr = (pred[p][c] * k[i]).min(f.clip);
                    d = d.max(((o + e) / (pr + e)).ln().abs());
                }
                m.data[p] = smoothstep(thr * 0.7, thr, d);
            }
            // dilate (max filter, square r) then feather
            let dil = dilate(&m, r);
            let mut g = gaussian(&dil, r as f32);
            for v in g.data.iter_mut() {
                *v = v.clamp(0.0, 1.0);
            }
            g
        })
        .collect();
    (maps, fac)
}

fn dilate(p: &Plane, r: usize) -> Plane {
    let (w, h) = (p.width, p.height);
    let mut tmp = Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp.data[y * w + x] = p.data[y * w + a..=y * w + b].iter().copied().fold(0.0, f32::max);
        }
    }
    let mut out = Plane::new(w, h);
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            out.data[y * w + x] = (a..=b).map(|yy| tmp.data[yy * w + x]).fold(0.0, f32::max);
        }
    }
    out
}
