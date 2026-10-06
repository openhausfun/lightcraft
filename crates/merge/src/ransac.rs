//! Robust 2-D motion models: normalised DLT homographies (Hartley & Zisserman, "Multiple View
//! Geometry", §4.4) and RANSAC (Fischler & Bolles 1981) with the adaptive iteration count.
//! Deterministic: the sampler is a fixed-seed xorshift, so results are reproducible.

use lightcraft_geom::{Homography, Point};

use crate::linalg::sym_eigen;

/// The kind of motion to fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    Translation,
    /// Rotation + uniform scale + translation (4 dof).
    Similarity,
    Homography,
}

impl Model {
    fn min_samples(self) -> usize {
        match self {
            Model::Translation => 1,
            Model::Similarity => 2,
            Model::Homography => 4,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fit {
    /// Maps points of the first set to the second.
    pub h: Homography,
    pub inliers: Vec<usize>,
    /// RMS transfer error of the inliers (pixels).
    pub rms: f64,
}

/// Small deterministic PRNG (xorshift64*).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Least-squares fit of `model` to all pairs (`a[i]` → `b[i]`).
pub fn fit_model(model: Model, a: &[Point], b: &[Point]) -> Option<Homography> {
    let n = a.len().min(b.len());
    if n < model.min_samples() {
        return None;
    }
    match model {
        Model::Translation => {
            let (mut dx, mut dy) = (0.0, 0.0);
            for i in 0..n {
                dx += b[i].x - a[i].x;
                dy += b[i].y - a[i].y;
            }
            Some(Homography([1.0, 0.0, dx / n as f64, 0.0, 1.0, dy / n as f64, 0.0, 0.0, 1.0]))
        }
        Model::Similarity => {
            // closed form (Umeyama without reflection): b ≈ s R a + t
            let ca = centroid(&a[..n]);
            let cb = centroid(&b[..n]);
            let (mut sxx, mut sxy, mut var) = (0.0, 0.0, 0.0);
            for i in 0..n {
                let (ax, ay) = (a[i].x - ca.x, a[i].y - ca.y);
                let (bx, by) = (b[i].x - cb.x, b[i].y - cb.y);
                sxx += ax * bx + ay * by;
                sxy += ax * by - ay * bx;
                var += ax * ax + ay * ay;
            }
            if var < 1e-12 {
                return None;
            }
            let (c, s) = (sxx / var, sxy / var);
            let tx = cb.x - (c * ca.x - s * ca.y);
            let ty = cb.y - (s * ca.x + c * ca.y);
            Some(Homography([c, -s, tx, s, c, ty, 0.0, 0.0, 1.0]))
        }
        Model::Homography => dlt(&a[..n], &b[..n]),
    }
}

fn centroid(p: &[Point]) -> Point {
    let n = p.len().max(1) as f64;
    Point::new(p.iter().map(|q| q.x).sum::<f64>() / n, p.iter().map(|q| q.y).sum::<f64>() / n)
}

/// Hartley normalisation: translate to the centroid, scale to mean distance √2.
fn normaliser(p: &[Point]) -> [f64; 3] {
    let c = centroid(p);
    let md = p.iter().map(|q| ((q.x - c.x).powi(2) + (q.y - c.y).powi(2)).sqrt()).sum::<f64>() / p.len().max(1) as f64;
    let s = if md > 1e-12 { std::f64::consts::SQRT_2 / md } else { 1.0 };
    [s, -s * c.x, -s * c.y]
}

/// Normalised direct linear transform (≥ 4 points).
pub fn dlt(a: &[Point], b: &[Point]) -> Option<Homography> {
    let n = a.len();
    if n < 4 {
        return None;
    }
    let ta = normaliser(a);
    let tb = normaliser(b);
    let mut ata = [0.0f64; 81];
    for i in 0..n {
        let (x, y) = (a[i].x * ta[0] + ta[1], a[i].y * ta[0] + ta[2]);
        let (u, v) = (b[i].x * tb[0] + tb[1], b[i].y * tb[0] + tb[2]);
        let r1 = [-x, -y, -1.0, 0.0, 0.0, 0.0, u * x, u * y, u];
        let r2 = [0.0, 0.0, 0.0, -x, -y, -1.0, v * x, v * y, v];
        for r in [r1, r2] {
            for p in 0..9 {
                if r[p] == 0.0 {
                    continue;
                }
                for q in 0..9 {
                    ata[p * 9 + q] += r[p] * r[q];
                }
            }
        }
    }
    let (_, vecs) = sym_eigen(&ata, 9);
    let h = &vecs[0];
    let hn = Homography([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8]]);
    // denormalise: H = Tb⁻¹ · Hn · Ta
    let ta_m = Homography([ta[0], 0.0, ta[1], 0.0, ta[0], ta[2], 0.0, 0.0, 1.0]);
    let tb_inv = Homography([1.0 / tb[0], 0.0, -tb[1] / tb[0], 0.0, 1.0 / tb[0], -tb[2] / tb[0], 0.0, 0.0, 1.0]);
    let h = tb_inv.mul(&hn).mul(&ta_m);
    let s = h.0[8];
    if s.abs() < 1e-14 || h.0.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(Homography(h.0.map(|v| v / s)))
}

pub fn transfer_error2(h: &Homography, a: Point, b: Point) -> f64 {
    let p = h.apply(a);
    (p.x - b.x).powi(2) + (p.y - b.y).powi(2)
}

/// RANSAC fit of `model` mapping `a` → `b`, inlier threshold `thresh` pixels. Refits on the
/// inliers (twice) and returns `None` when fewer than `min_inliers` agree.
pub fn ransac(model: Model, a: &[Point], b: &[Point], thresh: f64, min_inliers: usize, seed: u64) -> Option<Fit> {
    let n = a.len().min(b.len());
    let k = model.min_samples();
    if n < k.max(min_inliers) {
        return None;
    }
    let t2 = thresh * thresh;
    let mut rng = Rng::new(seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut best: Vec<usize> = Vec::new();
    let mut iters = 2000usize;
    let mut it = 0;
    let mut sa = Vec::with_capacity(k);
    let mut sb = Vec::with_capacity(k);
    while it < iters {
        it += 1;
        sa.clear();
        sb.clear();
        let mut idx = [usize::MAX; 4];
        let mut ok = true;
        for s in 0..k {
            let mut tries = 0;
            let i = loop {
                let i = rng.below(n);
                if !idx[..s].contains(&i) {
                    break i;
                }
                tries += 1;
                if tries > 20 {
                    ok = false;
                    break i;
                }
            };
            idx[s] = i;
            sa.push(a[i]);
            sb.push(b[i]);
        }
        if !ok {
            continue;
        }
        let h = match model {
            Model::Homography => {
                let src = [sa[0], sa[1], sa[2], sa[3]];
                let dst = [sb[0], sb[1], sb[2], sb[3]];
                if degenerate(&src) || degenerate(&dst) {
                    continue;
                }
                match Homography::from_quads(&src, &dst) {
                    Some(h) => h,
                    None => continue,
                }
            }
            _ => match fit_model(model, &sa, &sb) {
                Some(h) => h,
                None => continue,
            },
        };
        let inl: Vec<usize> = (0..n).filter(|&i| transfer_error2(&h, a[i], b[i]) < t2).collect();
        if inl.len() > best.len() {
            best = inl;
            let w = best.len() as f64 / n as f64;
            let p_fail = 1.0 - w.powi(k as i32);
            if p_fail <= 1e-12 {
                iters = it;
            } else {
                let need = ((1e-3f64).ln() / p_fail.ln()).ceil();
                iters = iters.min(need.max(16.0) as usize);
            }
        }
    }
    if best.len() < min_inliers.max(k) {
        return None;
    }
    let mut h = None;
    for _ in 0..3 {
        let ia: Vec<Point> = best.iter().map(|&i| a[i]).collect();
        let ib: Vec<Point> = best.iter().map(|&i| b[i]).collect();
        let Some(hh) = fit_model(model, &ia, &ib) else { break };
        let inl: Vec<usize> = (0..n).filter(|&i| transfer_error2(&hh, a[i], b[i]) < t2).collect();
        h = Some(hh);
        if inl.len() < best.len() || inl == best {
            break;
        }
        best = inl;
    }
    let h = h?;
    if best.len() < min_inliers.max(k) {
        return None;
    }
    let rms = (best.iter().map(|&i| transfer_error2(&h, a[i], b[i])).sum::<f64>() / best.len() as f64).sqrt();
    Some(Fit { h, inliers: best, rms })
}

/// Three (nearly) collinear points among the four.
fn degenerate(p: &[Point; 4]) -> bool {
    for (i, j, k) in [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)] {
        let area = ((p[j].x - p[i].x) * (p[k].y - p[i].y) - (p[j].y - p[i].y) * (p[k].x - p[i].x)).abs();
        let scale = p[i].dist(p[j]).max(p[i].dist(p[k])).max(1e-9);
        if area < 1e-3 * scale * scale {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ransac_recovers_a_homography_with_outliers() {
        let truth = Homography([1.02, 0.03, 12.0, -0.02, 0.99, -7.0, 1e-5, -2e-5, 1.0]);
        let mut rng = Rng::new(5);
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..200 {
            let p = Point::new(rng.unit() * 800.0, rng.unit() * 600.0);
            let q = if i % 4 == 0 { Point::new(rng.unit() * 800.0, rng.unit() * 600.0) } else { truth.apply(p) };
            let q = Point::new(q.x + (rng.unit() - 0.5) * 0.5, q.y + (rng.unit() - 0.5) * 0.5);
            a.push(p);
            b.push(q);
        }
        let f = ransac(Model::Homography, &a, &b, 2.0, 20, 1).unwrap();
        assert!(f.inliers.len() >= 145, "{}", f.inliers.len());
        assert!(f.rms < 0.5);
        for p in [Point::new(0.0, 0.0), Point::new(800.0, 600.0), Point::new(400.0, 100.0)] {
            assert!(f.h.apply(p).dist(truth.apply(p)) < 0.5);
        }
        let t = ransac(Model::Translation, &a, &a.iter().map(|p| Point::new(p.x + 3.0, p.y - 1.0)).collect::<Vec<_>>(), 1.0, 10, 2).unwrap();
        assert!((t.h.0[2] - 3.0).abs() < 1e-9 && (t.h.0[5] + 1.0).abs() < 1e-9);
        let s = fit_model(Model::Similarity, &a[..10], &a[..10].iter().map(|p| Point::new(-p.y * 2.0 + 5.0, p.x * 2.0)).collect::<Vec<_>>()).unwrap();
        assert!((s.0[0]).abs() < 1e-9 && (s.0[1] + 2.0).abs() < 1e-9 && (s.0[3] - 2.0).abs() < 1e-9);
    }
}
