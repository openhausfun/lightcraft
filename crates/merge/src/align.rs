//! Frame alignment for HDR brackets (handheld, small motion): features on exposure-normalised,
//! gamma-encoded luminance at ≤ 1200 px, matched to the reference frame, then a RANSAC homography
//! (falling back to similarity, then translation, then identity when the scene gives too little).

use lightcraft_geom::{Homography, Point};
use lightcraft_raster::resample::{Filter, fit};
use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

use crate::features::{self, Features};
use crate::ransac::{self, Model};

/// Long edge of the images features are detected on.
pub const WORK_EDGE: usize = 1200;

/// A grey, perceptually encoded work image: `(luminance · gain)^(1/2.2)`, clamped to 0..1, and
/// the scale from full-resolution pixels to work pixels.
pub fn work_image(img: &Rgb32f, gain: f32, max_edge: usize) -> (Plane, f64) {
    let small = if img.width.max(img.height) > max_edge { fit(img, max_edge, max_edge, Filter::Box) } else { img.clone() };
    let scale = small.width as f64 / img.width as f64;
    let p = small.map(|c| {
        let y = (0.25 * c[0] + 0.6 * c[1] + 0.15 * c[2]) * gain;
        y.clamp(0.0, 1.0).powf(1.0 / 2.2)
    });
    (p, scale)
}

/// Median luminance of the (unsaturated, non-black) pixels: a brightness estimate for frames
/// without EXIF.
pub fn median_luma(img: &Rgb32f, clip: f32) -> f32 {
    let step = (img.data.len() / 100_000).max(1);
    let mut v: Vec<f32> =
        img.data.iter().step_by(step).filter(|p| p[0].max(p[1]).max(p[2]) < clip * 0.98).map(|c| 0.25 * c[0] + 0.6 * c[1] + 0.15 * c[2]).collect();
    if v.is_empty() {
        return clip;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2].max(1e-6)
}

#[derive(Clone, Debug)]
pub struct Alignment {
    /// Maps reference pixel coordinates to this frame's coordinates (full resolution).
    pub h: Homography,
    pub model: &'static str,
    pub inliers: usize,
    pub rms: f64,
}

impl Alignment {
    pub fn identity() -> Alignment {
        Alignment { h: Homography::IDENTITY, model: "identity", inliers: 0, rms: 0.0 }
    }
}

/// Conjugate a work-resolution homography to full resolution: S⁻¹ · H · S with S = diag(s, s, 1).
pub fn scale_homography(h: &Homography, s: f64) -> Homography {
    let m = h.0;
    Homography([m[0], m[1], m[2] / s, m[3], m[4], m[5] / s, m[6] * s, m[7] * s, m[8]])
}

/// Whether `h` is a plausible handheld motion for a `w × h` frame: corners move less than
/// `max_frac` of the diagonal and the area changes by < 20 %.
pub fn plausible(h: &Homography, w: f64, ht: f64, max_frac: f64) -> bool {
    let diag = (w * w + ht * ht).sqrt();
    let corners = [Point::new(0.0, 0.0), Point::new(w, 0.0), Point::new(w, ht), Point::new(0.0, ht)];
    let moved: Vec<Point> = corners.iter().map(|&c| h.apply(c)).collect();
    if corners.iter().zip(&moved).any(|(a, b)| a.dist(*b) > max_frac * diag || !b.x.is_finite() || !b.y.is_finite()) {
        return false;
    }
    let area = |p: &[Point]| {
        let mut s = 0.0;
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            s += a.x * b.y - b.x * a.y;
        }
        s.abs() / 2.0
    };
    let r = area(&moved) / (w * ht);
    (0.8..1.25).contains(&r)
}

/// Align `frame` to `reference` (both as work images with features already detected).
pub fn align_pair(ref_feat: &Features, feat: &Features, scale: f64, full_w: usize, full_h: usize, seed: u64) -> Alignment {
    let m = features::match_features(ref_feat, feat, 0.8);
    let a: Vec<Point> = m.iter().map(|(i, _)| Point::new(ref_feat.points[*i].x as f64, ref_feat.points[*i].y as f64)).collect();
    let b: Vec<Point> = m.iter().map(|(_, j)| Point::new(feat.points[*j].x as f64, feat.points[*j].y as f64)).collect();
    let (ww, wh) = (full_w as f64 * scale, full_h as f64 * scale);
    // Fit every model; keep the simplest one that explains (almost) as many matches as the most
    // general one — extra degrees of freedom fitted to clustered points extrapolate badly.
    let mut fits: Vec<(&'static str, ransac::Fit)> = Vec::new();
    for (model, name, min) in [(Model::Translation, "translation", 5), (Model::Similarity, "similarity", 8), (Model::Homography, "homography", 16)] {
        if let Some(f) = ransac::ransac(model, &a, &b, 1.5, min, seed)
            && plausible(&f.h, ww, wh, 0.12)
        {
            fits.push((name, f));
        }
    }
    let Some(most) = fits.iter().map(|(_, f)| f.inliers.len()).max() else { return Alignment::identity() };
    // the fit with `most` inliers always qualifies
    let Some((name, f)) = fits.into_iter().find(|(_, f)| f.inliers.len() as f64 >= most as f64 * 0.97) else { return Alignment::identity() };
    Alignment { h: scale_homography(&f.h, scale), model: name, inliers: f.inliers.len(), rms: f.rms / scale }
}

/// Resample `img` into the reference geometry: `out(p) = img(h(p))`. Returns the warped image and
/// a validity mask (false where `h(p)` falls outside `img`).
pub fn warp(img: &Rgb32f, h: &Homography) -> (Rgb32f, Vec<bool>) {
    let (w, ht) = (img.width, img.height);
    if *h == Homography::IDENTITY {
        return (img.clone(), vec![true; w * ht]);
    }
    let mut out = Rgb32f::new(w, ht);
    let mut valid = vec![false; w * ht];
    out.data.par_chunks_mut(w).zip(valid.par_chunks_mut(w)).enumerate().for_each(|(y, (row, vrow))| {
        for x in 0..w {
            let p = h.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
            let (sx, sy) = (p.x as f32, p.y as f32);
            if sx >= 0.0 && sy >= 0.0 && sx <= w as f32 && sy <= ht as f32 {
                row[x] = img.sample_bilinear(sx, sy);
                vrow[x] = true;
            }
        }
    });
    (out, valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homography_scaling_conjugates() {
        let h = Homography([1.01, 0.02, 5.0, -0.01, 0.99, -3.0, 1e-5, 2e-5, 1.0]);
        let s = 0.25;
        let full = scale_homography(&h, s);
        let p = Point::new(400.0, 300.0);
        let w = h.apply(Point::new(p.x * s, p.y * s));
        let f = full.apply(p);
        assert!((f.x * s - w.x).abs() < 1e-9 && (f.y * s - w.y).abs() < 1e-9);
        assert!(plausible(&Homography::IDENTITY, 100.0, 100.0, 0.1));
        assert!(!plausible(&Homography([1.0, 0.0, 50.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]), 100.0, 100.0, 0.1));
    }
}
