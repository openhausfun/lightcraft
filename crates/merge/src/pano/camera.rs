//! Panorama registration: pairwise matching → camera rotations and focal lengths.
//!
//! After M. Brown & D. G. Lowe, "Automatic Panoramic Image Stitching using Invariant Features"
//! (IJCV 2007):
//! - pairwise feature matches verified with a RANSAC homography; a pair is accepted when the
//!   inliers explain enough of the matches (`nᵢ > 8 + 0.3·n_f`);
//! - the largest connected set of images is kept;
//! - focal length initialised from the homographies (Szeliski & Shum 1997 relations, derived here
//!   from the orthonormality of `K₁⁻¹ H K₀`), rotations by chaining the pairwise rotations along a
//!   maximum spanning tree (most inliers first);
//! - bundle adjustment of every camera's rotation and focal length (Levenberg–Marquardt on the
//!   symmetric reprojection error, Huber-weighted, with a numeric Jacobian per match);
//! - automatic straightening: the world "up" is the direction most perpendicular to every
//!   camera's x axis.

use lightcraft_geom::{Homography, Point};
use rayon::prelude::*;

use crate::features::{self, Features};
use crate::linalg::{self, M3};
use crate::ransac::{self, Model};

/// A pinhole camera: `u = f · (R d)ₓᵧ / (R d)_z + c` for a world direction `d`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Focal length (pixels of the image it belongs to).
    pub f: f64,
    /// World → camera rotation.
    pub r: M3,
    pub cx: f64,
    pub cy: f64,
}

impl Camera {
    /// World direction of image pixel `(x, y)`.
    #[inline]
    pub fn ray(&self, x: f64, y: f64) -> [f64; 3] {
        let d = [(x - self.cx) / self.f, (y - self.cy) / self.f, 1.0];
        linalg::apply3(&linalg::transpose3(&self.r), d)
    }
    /// Image pixel of world direction `d` (`None` behind the camera).
    #[inline]
    pub fn project(&self, d: [f64; 3]) -> Option<(f64, f64)> {
        let p = linalg::apply3(&self.r, d);
        (p[2] > 1e-9).then(|| (self.f * p[0] / p[2] + self.cx, self.f * p[1] / p[2] + self.cy))
    }
}

/// Verified matches between two images (full-resolution pixel coordinates).
#[derive(Clone, Debug)]
pub struct PairMatch {
    pub i: usize,
    pub j: usize,
    /// Maps image `i` pixels to image `j` pixels.
    pub h: Homography,
    pub pts: Vec<(Point, Point)>,
    /// Coarsest pyramid level of the two features of each match (finer = more precise).
    pub levels: Vec<u8>,
}

/// Match every pair of images. `feats[k]` are in work coordinates scaled by `scales[k]`.
pub fn match_pairs(feats: &[Features], scales: &[f64]) -> Vec<PairMatch> {
    let n = feats.len();
    let pairs: Vec<(usize, usize)> = (0..n).flat_map(|i| (i + 1..n).map(move |j| (i, j))).collect();
    let mut out: Vec<PairMatch> = pairs
        .par_iter()
        .filter_map(|&(i, j)| {
            let m = features::match_features(&feats[i], &feats[j], 0.8);
            if m.len() < 12 {
                return None;
            }
            let a: Vec<Point> = m.iter().map(|(p, _)| Point::new(feats[i].points[*p].x as f64, feats[i].points[*p].y as f64)).collect();
            let b: Vec<Point> = m.iter().map(|(_, q)| Point::new(feats[j].points[*q].x as f64, feats[j].points[*q].y as f64)).collect();
            let fit = ransac::ransac(Model::Homography, &a, &b, 3.0, 12, (i * 1000 + j) as u64)?;
            // Brown & Lowe's probabilistic verification
            if (fit.inliers.len() as f64) <= 8.0 + 0.3 * m.len() as f64 {
                return None;
            }
            let (si, sj) = (scales[i], scales[j]);
            let pts = fit.inliers.iter().map(|&k| (Point::new(a[k].x / si, a[k].y / si), Point::new(b[k].x / sj, b[k].y / sj))).collect();
            let levels = fit.inliers.iter().map(|&k| feats[i].points[m[k].0].level.max(feats[j].points[m[k].1].level)).collect();
            // conjugate to full resolution: H_full = S_j⁻¹ · H · S_i
            let s_i = Homography([si, 0.0, 0.0, 0.0, si, 0.0, 0.0, 0.0, 1.0]);
            let s_j_inv = Homography([1.0 / sj, 0.0, 0.0, 0.0, 1.0 / sj, 0.0, 0.0, 0.0, 1.0]);
            Some(PairMatch { i, j, h: s_j_inv.mul(&fit.h).mul(&s_i), pts, levels })
        })
        .collect();
    out.sort_by_key(|p| (p.i, p.j));
    out
}

/// The largest connected component of the match graph (sorted image indices).
pub fn largest_component(n: usize, pairs: &[PairMatch]) -> Vec<usize> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let nx = p[c];
            p[c] = r;
            c = nx;
        }
        r
    }
    for m in pairs {
        let (a, b) = (find(&mut parent, m.i), find(&mut parent, m.j));
        if a != b {
            parent[a] = b;
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    groups.into_values().max_by_key(|g| g.len()).unwrap_or_default()
}

/// Focal length candidates (f₀, f₁) from a homography between centred image coordinates.
pub fn focals_from_homography(h: &Homography) -> (Option<f64>, Option<f64>) {
    let m = h.0;
    let (h00, h01, h02, h10, h11, h12, h20, h21) = (m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7]);
    let pick = |n1: f64, d1: f64, n2: f64, d2: f64| -> Option<f64> {
        let a = (d1.abs() > 1e-12).then(|| n1 / d1).filter(|v| *v > 0.0);
        let b = (d2.abs() > 1e-12).then(|| n2 / d2).filter(|v| *v > 0.0);
        let v = match (a, b) {
            (Some(a), Some(b)) => {
                if d1.abs() > d2.abs() {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            _ => return None,
        };
        Some(v.sqrt())
    };
    // rows of K₁⁻¹HK₀ orthogonal / equal length → f₀; columns → f₁
    let f0 = pick(-h02 * h12, h00 * h10 + h01 * h11, h12 * h12 - h02 * h02, h00 * h00 + h01 * h01 - h10 * h10 - h11 * h11);
    let f1 = pick(-(h00 * h01 + h10 * h11), h20 * h21, h01 * h01 + h11 * h11 - h00 * h00 - h10 * h10, h20 * h20 - h21 * h21);
    (f0, f1)
}

fn centred(h: &Homography, ci: (f64, f64), cj: (f64, f64)) -> Homography {
    let ti = Homography([1.0, 0.0, ci.0, 0.0, 1.0, ci.1, 0.0, 0.0, 1.0]);
    let tj_inv = Homography([1.0, 0.0, -cj.0, 0.0, 1.0, -cj.1, 0.0, 0.0, 1.0]);
    tj_inv.mul(h).mul(&ti)
}

/// Initial cameras for `ids` (a connected set): shared focal `f0`, rotations chained along a
/// maximum spanning tree rooted at the best-connected image.
pub fn initial_cameras(ids: &[usize], sizes: &[(usize, usize)], pairs: &[PairMatch], f0: f64) -> Vec<Option<Camera>> {
    let n = sizes.len();
    let mut cams: Vec<Option<Camera>> = vec![None; n];
    let deg = |k: usize| pairs.iter().filter(|p| p.i == k || p.j == k).map(|p| p.pts.len()).sum::<usize>();
    let Some(&root) = ids.iter().max_by_key(|&&k| (deg(k), usize::MAX - k)) else { return cams };
    let c = |k: usize| (sizes[k].0 as f64 / 2.0, sizes[k].1 as f64 / 2.0);
    cams[root] = Some(Camera { f: f0, r: linalg::I3, cx: c(root).0, cy: c(root).1 });
    let mut edges: Vec<&PairMatch> = pairs.iter().filter(|p| ids.contains(&p.i) && ids.contains(&p.j)).collect();
    edges.sort_by_key(|p| std::cmp::Reverse(p.pts.len()));
    loop {
        let mut grew = false;
        for e in &edges {
            let (known, new, h) = match (cams[e.i].is_some(), cams[e.j].is_some()) {
                (true, false) => (e.i, e.j, e.h),
                (false, true) => (
                    e.j,
                    e.i,
                    match e.h.inverse() {
                        Some(h) => h,
                        None => continue,
                    },
                ),
                _ => continue,
            };
            // h maps `known` → `new`; R_new R_knownᵀ ≈ K_new⁻¹ H K_known
            let hc = centred(&h, c(known), c(new));
            let k = [[f0, 0.0, 0.0], [0.0, f0, 0.0], [0.0, 0.0, 1.0]];
            let kinv = [[1.0 / f0, 0.0, 0.0], [0.0, 1.0 / f0, 0.0], [0.0, 0.0, 1.0]];
            let hm = [[hc.0[0], hc.0[1], hc.0[2]], [hc.0[3], hc.0[4], hc.0[5]], [hc.0[6], hc.0[7], hc.0[8]]];
            let rel = linalg::mul3(&linalg::mul3(&kinv, &hm), &k);
            let d = linalg::det3(&rel);
            if !d.is_finite() || d.abs() < 1e-12 {
                continue;
            }
            let s = d.signum() * d.abs().cbrt();
            let rel = rel.map(|r| r.map(|v| v / s));
            let rel = linalg::orthonormalize(&rel);
            let Some(rk) = cams[known].map(|c| c.r) else { continue };
            cams[new] = Some(Camera { f: f0, r: linalg::mul3(&rel, &rk), cx: c(new).0, cy: c(new).1 });
            grew = true;
        }
        if !grew {
            break;
        }
    }
    cams
}

/// Bundle adjustment result.
#[derive(Clone, Debug)]
pub struct Adjusted {
    pub cameras: Vec<Option<Camera>>,
    /// RMS reprojection error (pixels) before and after.
    pub rms_before: f64,
    pub rms: f64,
    pub iterations: usize,
}

struct Obs {
    i: usize,
    j: usize,
    a: Point,
    b: Point,
}

/// Residuals of one match: image-j point projected into i, and image-i point into j.
fn residual(ci: &Camera, cj: &Camera, a: Point, b: Point) -> [f64; 4] {
    let big = 1e3;
    let p = ci.project(cj.ray(b.x, b.y));
    let q = cj.project(ci.ray(a.x, a.y));
    let (r0, r1) = p.map_or((big, big), |(x, y)| (x - a.x, y - a.y));
    let (r2, r3) = q.map_or((big, big), |(x, y)| (x - b.x, y - b.y));
    [r0, r1, r2, r3]
}

fn perturb(c: &Camera, k: usize, eps: f64) -> Camera {
    let mut c = *c;
    if k < 3 {
        let mut w = [0.0; 3];
        w[k] = eps;
        c.r = linalg::mul3(&linalg::rodrigues(w), &c.r);
    } else {
        c.f += eps;
    }
    c
}

/// Levenberg–Marquardt over (rotation increment, focal) of every camera except the anchor's
/// rotation (gauge freedom). At most `max_per_pair` matches per pair are used.
pub fn bundle_adjust(cams: &[Option<Camera>], pairs: &[PairMatch], anchor: usize, max_per_pair: usize) -> Adjusted {
    let n = cams.len();
    let idx: Vec<Option<usize>> = {
        let mut k = 0;
        cams.iter()
            .map(|c| {
                c.map(|_| {
                    k += 1;
                    k - 1
                })
            })
            .collect()
    };
    let m = idx.iter().flatten().count();
    let np = 4 * m;
    let mut obs = Vec::new();
    for p in pairs {
        if cams[p.i].is_none() || cams[p.j].is_none() {
            continue;
        }
        // prefer the precisely located (fine-level) matches when there are enough of them
        let fine: Vec<usize> = (0..p.pts.len()).filter(|&k| p.levels.get(k).is_none_or(|l| *l <= 1)).collect();
        let use_idx: Vec<usize> = if fine.len() >= 16 { fine } else { (0..p.pts.len()).collect() };
        let step = (use_idx.len() / max_per_pair.max(1)).max(1);
        for &k in use_idx.iter().step_by(step) {
            let (a, b) = p.pts[k];
            obs.push(Obs { i: p.i, j: p.j, a, b });
        }
    }
    let mut cur: Vec<Option<Camera>> = cams.to_vec();
    let sigma = 2.0f64;
    let cost = |cs: &[Option<Camera>]| -> (f64, f64) {
        let (mut c, mut sq) = (0.0, 0.0);
        for o in &obs {
            // every observation joins two posed cameras (see `obs` above)
            let (Some(ci), Some(cj)) = (cs[o.i].as_ref(), cs[o.j].as_ref()) else { continue };
            let r = residual(ci, cj, o.a, o.b);
            for v in r {
                let a = v.abs();
                sq += v * v;
                c += if a <= sigma { 0.5 * v * v } else { sigma * (a - 0.5 * sigma) };
            }
        }
        (c, (sq / (obs.len().max(1) * 4) as f64).sqrt())
    };
    let (mut c0, rms_before) = cost(&cur);
    let mut lambda = 1e-3;
    let mut iterations = 0;
    let _ = n;
    for _ in 0..100 {
        iterations += 1;
        // normal equations (Huber IRLS weights)
        let (jtj, jtr) = obs
            .par_iter()
            .fold(
                || (vec![0.0f64; np * np], vec![0.0f64; np]),
                |(mut jtj, mut jtr), o| {
                    let (Some(ci), Some(cj)) = (cur[o.i].as_ref(), cur[o.j].as_ref()) else { return (jtj, jtr) };
                    let r = residual(ci, cj, o.a, o.b);
                    let mut cols: Vec<(usize, [f64; 4])> = Vec::with_capacity(8);
                    for (cam, which) in [(o.i, 0), (o.j, 1)] {
                        let Some(i) = idx[cam] else { continue };
                        let base = 4 * i;
                        for k in 0..4 {
                            if k < 3 && cam == anchor {
                                continue;
                            }
                            let c = if which == 0 { ci } else { cj };
                            let eps = if k < 3 { 1e-6 } else { 1e-4 * c.f };
                            let pc = perturb(c, k, eps);
                            let rr = if which == 0 { residual(&pc, cj, o.a, o.b) } else { residual(ci, &pc, o.a, o.b) };
                            let mut d = [0.0; 4];
                            for t in 0..4 {
                                d[t] = (rr[t] - r[t]) / eps;
                            }
                            cols.push((base + k, d));
                        }
                    }
                    for t in 0..4 {
                        let a = r[t].abs();
                        let w = if a <= sigma { 1.0 } else { sigma / a };
                        for &(p, dp) in &cols {
                            jtr[p] += w * dp[t] * r[t];
                            for &(q, dq) in &cols {
                                jtj[p * np + q] += w * dp[t] * dq[t];
                            }
                        }
                    }
                    (jtj, jtr)
                },
            )
            .reduce(
                || (vec![0.0f64; np * np], vec![0.0f64; np]),
                |(mut a, mut b), (c, d)| {
                    for (x, y) in a.iter_mut().zip(&c) {
                        *x += y;
                    }
                    for (x, y) in b.iter_mut().zip(&d) {
                        *x += y;
                    }
                    (a, b)
                },
            );
        let mut improved = false;
        for _ in 0..10 {
            let mut a = jtj.clone();
            for p in 0..np {
                let d = a[p * np + p];
                a[p * np + p] = d + lambda * d.max(1e-9) + 1e-12;
            }
            let Some(delta) = linalg::solve(a, jtr.iter().map(|v| -v).collect(), np) else {
                lambda *= 10.0;
                continue;
            };
            let mut trial = cur.clone();
            for (cam, c) in trial.iter_mut().enumerate() {
                let (Some(c), Some(b)) = (c.as_mut(), idx[cam]) else { continue };
                let b = 4 * b;
                if cam != anchor {
                    c.r = linalg::orthonormalize(&linalg::mul3(&linalg::rodrigues([delta[b], delta[b + 1], delta[b + 2]]), &c.r));
                }
                c.f = (c.f + delta[b + 3]).max(1.0);
            }
            let (c1, _) = cost(&trial);
            if c1 < c0 {
                let rel = (c0 - c1) / c0.max(1e-300);
                cur = trial;
                c0 = c1;
                lambda = (lambda * 0.3).max(1e-9);
                improved = rel > 1e-9;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    let (_, rms) = cost(&cur);
    Adjusted { cameras: cur, rms_before, rms, iterations }
}

/// Rotate the world so that it is level: "down" (+y) is the direction most perpendicular to every
/// camera's x axis, and "forward" (+z) the mean viewing direction.
pub fn straighten(cams: &mut [Option<Camera>]) {
    let rs: Vec<M3> = cams.iter().flatten().map(|c| c.r).collect();
    if rs.is_empty() {
        return;
    }
    let mut cov = [0.0f64; 9];
    let (mut ysum, mut zsum, mut xsum) = ([0.0; 3], [0.0; 3], [0.0; 3]);
    for r in &rs {
        let (x, y, z) = (r[0], r[1], r[2]);
        for a in 0..3 {
            for b in 0..3 {
                cov[a * 3 + b] += x[a] * x[b];
            }
            ysum[a] += y[a];
            zsum[a] += z[a];
            xsum[a] += x[a];
        }
    }
    let (vals, vecs) = linalg::sym_eigen(&cov, 3);
    let total = vals.iter().sum::<f64>().max(1e-12);
    let mut down = [vecs[0][0], vecs[0][1], vecs[0][2]];
    if vals[1] < 1e-3 * total {
        // all x axes (nearly) parallel (a vertical sweep): use the mean camera "down" made
        // perpendicular to the common x axis
        let xa = linalg::normalize(xsum);
        let y = linalg::normalize(ysum);
        let k = linalg::dot(y, xa);
        down = linalg::normalize([y[0] - k * xa[0], y[1] - k * xa[1], y[2] - k * xa[2]]);
    }
    if linalg::dot(down, ysum) < 0.0 {
        down = down.map(|v| -v);
    }
    let z = linalg::normalize(zsum);
    let k = linalg::dot(z, down);
    let fwd = linalg::normalize([z[0] - k * down[0], z[1] - k * down[1], z[2] - k * down[2]]);
    let right = linalg::cross(down, fwd);
    let w = [right, down, fwd];
    let wt = linalg::transpose3(&w);
    for c in cams.iter_mut().flatten() {
        c.r = linalg::mul3(&c.r, &wt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focal_from_a_rotation_homography() {
        let f = 900.0;
        let r = linalg::rodrigues([0.05, 0.3, 0.02]);
        let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let kinv = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
        let h = linalg::mul3(&linalg::mul3(&k, &r), &kinv);
        let hh = Homography([h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2]]);
        let (a, b) = focals_from_homography(&hh);
        assert!((a.unwrap() - f).abs() < 1.0, "{a:?}");
        assert!((b.unwrap() - f).abs() < 1.0, "{b:?}");
    }
}
