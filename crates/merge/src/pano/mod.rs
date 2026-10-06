//! Panorama stitching.
//!
//! 1. Features ([`crate::features`]) on each frame at ≤ 1000 px, pairwise matching and RANSAC
//!    homographies, largest connected set ([`camera`]).
//! 2. Camera rotations + focal lengths: initialised from the homographies, bundle-adjusted,
//!    straightened ([`camera`]).
//! 3. Projection: spherical, cylindrical or perspective (rectilinear), or chosen automatically
//!    from the field of view ([`choose_projection`]).
//! 4. Exposure compensation: EXIF exposure, then per-image gains solved from the mean intensities
//!    of every overlap (Brown & Lowe's gain compensation, a small linear least-squares problem
//!    with a prior pulling gains towards 1).
//! 5. Seams: each canvas pixel belongs to the image whose centre-weighted feather weight is
//!    highest there (Voronoi-like seams away from image borders); multi-band blending in log space
//!    ([`blend`]).
//! 6. Boundary warp, auto crop, fill edges ([`finish`]).

pub mod blend;
pub mod camera;
pub mod finish;
pub mod project;

use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

pub use project::Projection;

use crate::frame::{Frame, FrameColor};
use crate::linalg;
use crate::{MergeError, Progress, Result, check};
use blend::{Blender, Buf, push_pull};
use camera::Camera;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PanoOptions {
    pub projection: Projection,
    /// 0..100.
    pub boundary_warp: f64,
    pub auto_crop: bool,
    pub fill_edges: bool,
    /// Output size limit (megapixels); the panorama is scaled down to fit.
    pub max_megapixels: f64,
}

impl Default for PanoOptions {
    fn default() -> Self {
        PanoOptions { projection: Projection::Auto, boundary_warp: 0.0, auto_crop: false, fill_edges: false, max_megapixels: 40.0 }
    }
}

/// The stitched panorama.
#[derive(Clone, Debug)]
pub struct PanoResult {
    /// Scene-linear pixels (same units as the frames, after exposure compensation).
    pub image: Rgb32f,
    /// Coverage (1 = image data, 0 = empty canvas).
    pub alpha: Plane,
    /// Auto-crop rectangle (normalised x, y, w, h), when requested.
    pub crop: Option<[f64; 4]>,
    /// The projection used (never `Auto`).
    pub projection: Projection,
    /// Indices (into the inputs) of the frames in the panorama; the others didn't connect.
    pub used: Vec<usize>,
    /// Cameras of the inputs (`None` = not used), in their full-resolution pixels, straightened.
    pub cameras: Vec<Option<Camera>>,
    /// Per-input exposure gains applied.
    pub gains: Vec<f64>,
    /// Bundle adjustment RMS reprojection error (pixels) before/after.
    pub rms_before: f64,
    pub rms: f64,
    /// Horizontal and vertical field of view (degrees).
    pub fov: (f64, f64),
    /// Output focal length and the surface coordinates of the canvas's top-left corner: canvas
    /// pixel (x, y) (before boundary warp) shows surface point (x + 0.5 + origin.0, y + 0.5 + origin.1).
    pub focal: f64,
    pub origin: (f64, f64),
    pub color: FrameColor,
    pub metadata: lightcraft_meta::Metadata,
    pub raw: bool,
    pub baseline_exposure: f64,
}

/// Field of view (degrees) of the cameras' union, measured in the spherical projection.
fn fov(frames: &[Frame], cams: &[Option<Camera>]) -> (f64, f64) {
    let (mut tmin, mut tmax, mut pmin, mut pmax) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (f, c) in frames.iter().zip(cams) {
        let Some(c) = c else { continue };
        for (x, y) in border(f.width(), f.height(), 16) {
            if let Some((t, p)) = Projection::Spherical.forward(c.ray(x, y), 1.0) {
                tmin = tmin.min(t);
                tmax = tmax.max(t);
                pmin = pmin.min(p);
                pmax = pmax.max(p);
            }
        }
    }
    ((tmax - tmin).to_degrees(), (pmax - pmin).to_degrees())
}

/// Points along an image's border.
fn border(w: usize, h: usize, per_edge: usize) -> Vec<(f64, f64)> {
    let (w, h) = (w as f64, h as f64);
    let mut v = Vec::new();
    for k in 0..=per_edge {
        let t = k as f64 / per_edge as f64;
        v.extend([(t * w, 0.0), (t * w, h), (0.0, t * h), (w, t * h)]);
    }
    v
}

/// Surface bounding box of every used image in `proj` (output focal `f`).
fn bounds(frames: &[Frame], cams: &[Option<Camera>], proj: Projection, f: f64) -> Option<(f64, f64, f64, f64)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (fr, c) in frames.iter().zip(cams) {
        let Some(c) = c else { continue };
        for (x, y) in border(fr.width(), fr.height(), 24) {
            let (u, v) = proj.forward(c.ray(x, y), f)?;
            x0 = x0.min(u);
            y0 = y0.min(v);
            x1 = x1.max(u);
            y1 = y1.max(v);
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Auto projection: perspective when the field of view is moderate and the rectilinear canvas
/// stays compact; cylindrical for wide but not tall panoramas; spherical otherwise.
pub fn choose_projection(frames: &[Frame], cams: &[Option<Camera>]) -> Projection {
    let (h, v) = fov(frames, cams);
    let area: f64 = frames.iter().zip(cams).filter(|(_, c)| c.is_some()).map(|(f, _)| (f.width() * f.height()) as f64).sum();
    let fmed = median(cams.iter().flatten().map(|c| c.f).collect());
    if h < 110.0
        && v < 110.0
        && let Some((x0, y0, x1, y1)) = bounds(frames, cams, Projection::Perspective, fmed)
        && (x1 - x0) * (y1 - y0) < 2.5 * area
    {
        return Projection::Perspective;
    }
    if v < 90.0 { Projection::Cylindrical } else { Projection::Spherical }
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 1.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// One image resampled onto its canvas tile.
struct Tile {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
    /// Feather weight (0 = outside the image).
    wgt: Vec<f32>,
}

/// Register the frames: cameras for the connected set.
pub fn register(frames: &[Frame], progress: &Progress) -> Result<(Vec<Option<Camera>>, f64, f64)> {
    let n = frames.len();
    let work: Vec<(crate::features::Features, f64)> = frames
        .par_iter()
        .map(|f| {
            let med = crate::align::median_luma(&f.image, f.clip);
            let (p, s) = crate::align::work_image(&f.image, 0.18 / med.max(1e-6), 1000);
            (crate::features::detect(&p, 1200), s)
        })
        .collect();
    check(progress, 0.2, "Matching")?;
    let feats: Vec<_> = work.iter().map(|(f, _)| f.clone()).collect();
    let scales: Vec<f64> = work.iter().map(|(_, s)| *s).collect();
    let pairs = camera::match_pairs(&feats, &scales);
    let ids = camera::largest_component(n, &pairs);
    if ids.len() < 2 {
        return Err(MergeError::NoOverlap("the photos don't overlap enough to be stitched".into()));
    }
    check(progress, 0.3, "Aligning")?;
    // focal length: EXIF 35 mm equivalent, else the homographies, else ~50° horizontal
    let sizes: Vec<(usize, usize)> = frames.iter().map(|f| (f.width(), f.height())).collect();
    let mut fs = Vec::new();
    for p in &pairs {
        if !(ids.contains(&p.i) && ids.contains(&p.j)) {
            continue;
        }
        let ci = (sizes[p.i].0 as f64 / 2.0, sizes[p.i].1 as f64 / 2.0);
        let cj = (sizes[p.j].0 as f64 / 2.0, sizes[p.j].1 as f64 / 2.0);
        let ti = lightcraft_geom::Homography([1.0, 0.0, ci.0, 0.0, 1.0, ci.1, 0.0, 0.0, 1.0]);
        let tj = lightcraft_geom::Homography([1.0, 0.0, -cj.0, 0.0, 1.0, -cj.1, 0.0, 0.0, 1.0]);
        let hc = tj.mul(&p.h).mul(&ti);
        if let (Some(a), Some(b)) = camera::focals_from_homography(&hc) {
            fs.push((a * b).sqrt());
        }
    }
    let long = sizes[ids[0]].0.max(sizes[ids[0]].1) as f64;
    let exif_f = frames[ids[0]].metadata.focal_length_35mm.filter(|v| *v > 1.0).map(|f35| f35 / 36.0 * long);
    let hom_f = (!fs.is_empty()).then(|| median(fs.clone())).filter(|f| *f > 0.1 * long && *f < 20.0 * long);
    let f0 = exif_f.or(hom_f).unwrap_or(long * 1.07);
    let cams = camera::initial_cameras(&ids, &sizes, &pairs, f0);
    let anchor = ids.iter().copied().find(|&i| cams[i].is_some()).unwrap_or(ids[0]);
    check(progress, 0.35, "Aligning")?;
    let adj = camera::bundle_adjust(&cams, &pairs, anchor, 60);
    let mut cams = adj.cameras;
    camera::straighten(&mut cams);
    Ok((cams, adj.rms_before, adj.rms))
}

/// Stitch `frames` (oriented, linear) into a panorama.
pub fn stitch(frames: Vec<Frame>, opts: &PanoOptions, progress: &Progress) -> Result<PanoResult> {
    let n = frames.len();
    if n < 2 {
        return Err(MergeError::TooFew(n, 2));
    }
    check(progress, 0.01, "Finding features")?;
    let (cams, rms_before, rms) = register(&frames, progress)?;
    let used: Vec<usize> = (0..n).filter(|&i| cams[i].is_some()).collect();
    let fov = fov(&frames, &cams);
    let projection = match opts.projection {
        Projection::Auto => choose_projection(&frames, &cams),
        p => p,
    };
    let fmed = median(used.iter().filter_map(|&i| cams[i].map(|c| c.f)).collect());
    let Some((bx0, by0, bx1, by1)) = bounds(&frames, &cams, projection, fmed) else {
        return Err(MergeError::NoOverlap(format!("the field of view ({:.0}°) is too wide for a perspective projection", fov.0)));
    };
    let area: f64 = used.iter().map(|&i| (frames[i].width() * frames[i].height()) as f64).sum();
    if projection == Projection::Perspective && (bx1 - bx0) * (by1 - by0) > 12.0 * area {
        return Err(MergeError::NoOverlap(format!("the field of view ({:.0}°) is too wide for a perspective projection", fov.0)));
    }
    // output scale: native resolution, limited to max_megapixels
    let max_px = (opts.max_megapixels.max(0.1) * 1e6).min(400e6);
    let s = (max_px / ((bx1 - bx0) * (by1 - by0))).sqrt().min(1.0);
    let f_out = fmed * s;
    let (ox, oy) = (bx0 * s, by0 * s);
    let (cw, ch) = (((bx1 - bx0) * s).ceil() as usize + 1, ((by1 - by0) * s).ceil() as usize + 1);
    let levels = ((cw.min(ch) as f64).log2() - 3.0).floor().clamp(1.0, 7.0) as usize;
    let unit = 1usize << levels;
    let (pw, ph) = (cw.div_ceil(unit) * unit, ch.div_ceil(unit) * unit);
    check(progress, 0.45, "Projecting")?;

    // exposure from EXIF relative to the first used frame
    let ev0 = frames[used[0]].exposure;
    let mut gains: Vec<f64> = (0..n)
        .map(|i| match (frames[i].exposure, ev0) {
            (Some(e), Some(e0)) => 2f64.powf(e0 - e),
            _ => 1.0,
        })
        .collect();

    // resample every used frame onto its tile
    let tiles: Vec<Option<Tile>> = (0..n)
        .into_par_iter()
        .map(|i| {
            let c = cams[i]?;
            let fr = &frames[i];
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for (x, y) in border(fr.width(), fr.height(), 32) {
                let (u, v) = projection.forward(c.ray(x, y), f_out)?;
                x0 = x0.min(u - ox);
                y0 = y0.min(v - oy);
                x1 = x1.max(u - ox);
                y1 = y1.max(v - oy);
            }
            let pad = 2.0 * unit as f64;
            let tx0 = (((x0 - pad).max(0.0) as usize) / unit) * unit;
            let ty0 = (((y0 - pad).max(0.0) as usize) / unit) * unit;
            let tx1 = (((x1 + pad).max(0.0) as usize).min(pw)).div_ceil(unit) * unit;
            let ty1 = (((y1 + pad).max(0.0) as usize).min(ph)).div_ceil(unit) * unit;
            let (tw, th) = (tx1.min(pw).saturating_sub(tx0), ty1.min(ph).saturating_sub(ty0));
            if tw == 0 || th == 0 {
                return None;
            }
            let (fw, fh) = (fr.width() as f64, fr.height() as f64);
            let mut rgb = vec![[0f32; 3]; tw * th];
            let mut wgt = vec![0f32; tw * th];
            rgb.par_chunks_mut(tw).zip(wgt.par_chunks_mut(tw)).enumerate().for_each(|(ty, (row, wrow))| {
                let v = (ty0 + ty) as f64 + 0.5 + oy;
                for tx in 0..tw {
                    let u = (tx0 + tx) as f64 + 0.5 + ox;
                    let d = projection.inverse(u, v, f_out);
                    let Some((px, py)) = c.project(d) else { continue };
                    if px < 0.0 || py < 0.0 || px > fw || py > fh {
                        continue;
                    }
                    let wx = 1.0 - (2.0 * px / fw - 1.0).abs();
                    let wy = 1.0 - (2.0 * py / fh - 1.0).abs();
                    let w = (wx * wy).max(1e-6) as f32;
                    row[tx] = fr.image.sample_bilinear(px as f32, py as f32);
                    wrow[tx] = w;
                }
            });
            Some(Tile { x0: tx0, y0: ty0, w: tw, h: th, rgb, wgt })
        })
        .collect();
    check(progress, 0.6, "Exposure compensation")?;
    compensate_gains(&tiles, &mut gains, &frames);

    // seams: label = max feather weight
    let mut best = vec![0f32; pw * ph];
    let mut label = vec![u16::MAX; pw * ph];
    for (i, t) in tiles.iter().enumerate() {
        let Some(t) = t else { continue };
        for ty in 0..t.h {
            let row = (t.y0 + ty) * pw + t.x0;
            for tx in 0..t.w {
                let w = t.wgt[ty * t.w + tx];
                if w > best[row + tx] {
                    best[row + tx] = w;
                    label[row + tx] = i as u16;
                }
            }
        }
    }
    check(progress, 0.7, "Blending")?;
    let mut blender = Blender::new(pw, ph, levels);
    for (i, t) in tiles.into_iter().enumerate() {
        let Some(t) = t else { continue };
        let g = gains[i] as f32;
        let mut vals = Buf::new(t.w, t.h, 3);
        let mut mask = Buf::new(t.w, t.h, 1);
        for ty in 0..t.h {
            for tx in 0..t.w {
                let k = ty * t.w + tx;
                if t.wgt[k] > 0.0 {
                    for c in 0..3 {
                        vals.data[k * 3 + c] = (t.rgb[k][c] * g).max(1e-6).ln();
                    }
                }
                if label[(t.y0 + ty) * pw + t.x0 + tx] == i as u16 {
                    mask.data[k] = 1.0;
                }
            }
        }
        push_pull(&mut vals, &t.wgt);
        blender.add(t.x0, t.y0, vals, mask);
        check(progress, 0.7 + 0.2 * (i + 1) as f32 / n as f32, "Blending")?;
    }
    let out = blender.finish();
    let mut image = Rgb32f::new(cw, ch);
    let mut alpha = Plane::new(cw, ch);
    for y in 0..ch {
        for x in 0..cw {
            let k = y * pw + x;
            if label[k] != u16::MAX {
                image.data[y * cw + x] = [0, 1, 2].map(|c| out.data[k * 3 + c].exp());
                alpha.data[y * cw + x] = 1.0;
            }
        }
    }
    check(progress, 0.92, "Finishing")?;
    let (mut image, alpha) = finish::boundary_warp(&image, &alpha, opts.boundary_warp / 100.0);
    let crop = if opts.auto_crop {
        finish::auto_crop(&alpha).map(|(x, y, w, h)| [x as f64 / cw as f64, y as f64 / ch as f64, w as f64 / cw as f64, h as f64 / ch as f64])
    } else {
        None
    };
    if opts.fill_edges {
        image = finish::fill_edges(&image, &alpha);
    }
    let r = &frames[used[0]];
    Ok(PanoResult {
        image,
        alpha,
        crop,
        projection,
        used,
        cameras: cams,
        gains,
        rms_before,
        rms,
        fov,
        focal: f_out,
        origin: (ox, oy),
        color: r.color.clone(),
        metadata: r.metadata.clone(),
        raw: r.raw,
        baseline_exposure: r.baseline_exposure,
    })
}

/// Gain compensation (Brown & Lowe §6): minimise
/// Σᵢⱼ Nᵢⱼ [ (gᵢ Īᵢⱼ − gⱼ Īⱼᵢ)² / σN² + (1 − gᵢ/g⁰ᵢ)² / σg² ] over the overlaps, with intensities
/// relative to the mean overlap level (σN = 0.1; σg = 1, a weak prior that only fixes the overall
/// level) and `g⁰` the EXIF-based gains.
fn compensate_gains(tiles: &[Option<Tile>], gains: &mut [f64], frames: &[Frame]) {
    let n = tiles.len();
    let lum = |p: [f32; 3]| 0.25 * p[0] as f64 + 0.6 * p[1] as f64 + 0.15 * p[2] as f64;
    let mut nij = vec![0f64; n * n];
    let mut iij = vec![0f64; n * n];
    for i in 0..n {
        let Some(a) = &tiles[i] else { continue };
        for j in 0..n {
            let Some(b) = tiles[j].as_ref().filter(|_| i != j) else { continue };
            let (x0, y0) = (a.x0.max(b.x0), a.y0.max(b.y0));
            let (x1, y1) = ((a.x0 + a.w).min(b.x0 + b.w), (a.y0 + a.h).min(b.y0 + b.h));
            let (mut cnt, mut sum) = (0usize, 0f64);
            for y in (y0..y1).step_by(2) {
                for x in (x0..x1).step_by(2) {
                    let ka = (y - a.y0) * a.w + x - a.x0;
                    let kb = (y - b.y0) * b.w + x - b.x0;
                    if a.wgt[ka] > 0.0 && b.wgt[kb] > 0.0 {
                        let pa = a.rgb[ka];
                        if pa.iter().any(|v| *v >= frames[i].clip * 0.95) || b.rgb[kb].iter().any(|v| *v >= frames[j].clip * 0.95) {
                            continue;
                        }
                        cnt += 1;
                        sum += lum(pa);
                    }
                }
            }
            if cnt > 50 {
                nij[i * n + j] = cnt as f64;
                iij[i * n + j] = sum / cnt as f64;
            }
        }
    }
    let total: f64 = nij.iter().sum();
    if total <= 0.0 {
        return;
    }
    let mean = (0..n * n).map(|k| nij[k] * iij[k] * gains[k / n]).sum::<f64>() / total;
    if mean <= 0.0 {
        return;
    }
    let (sn, sg) = (0.1f64, 1.0f64);
    let g0: Vec<f64> = gains.to_vec();
    let mut a = vec![0f64; n * n];
    let mut b = vec![0f64; n];
    for i in 0..n {
        if tiles[i].is_none() {
            a[i * n + i] = 1.0;
            b[i] = g0[i];
            continue;
        }
        for j in 0..n {
            let nn = nij[i * n + j];
            if nn <= 0.0 || nij[j * n + i] <= 0.0 {
                continue;
            }
            let (ii, ij) = (iij[i * n + j] / mean, iij[j * n + i] / mean);
            // d/dgᵢ of Nᵢⱼ (gᵢ Iᵢⱼ − gⱼ Iⱼᵢ)²/σN² (both orderings of the pair are in the sum)
            a[i * n + i] += 2.0 * nn * ii * ii / (sn * sn);
            a[i * n + j] -= 2.0 * nn * ii * ij / (sn * sn);
            a[i * n + i] += 2.0 * nn / (sg * sg * g0[i] * g0[i]);
            b[i] += 2.0 * nn / (sg * sg * g0[i]);
        }
        if a[i * n + i] == 0.0 {
            a[i * n + i] = 1.0;
            b[i] = g0[i];
        }
    }
    if let Some(g) = linalg::solve(a, b, n)
        && g.iter().all(|v| v.is_finite() && *v > 0.0)
    {
        gains.copy_from_slice(&g);
    }
}
