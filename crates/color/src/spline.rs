//! Curves for tone mapping: monotone cubic Hermite splines (Fritsch–Carlson) and lookup tables.
//!
//! Monotone interpolation never overshoots between control points, so a point curve can't invert
//! tones by accident (a common complaint with natural cubic splines).

use serde::{Deserialize, Serialize};

/// A curve through control points in `0..1 × 0..1` (x strictly increasing after normalization).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MonotoneCurve {
    xs: Vec<f64>,
    ys: Vec<f64>,
    ms: Vec<f64>,
}

impl MonotoneCurve {
    /// Build from points; sorts by x, merges duplicates. Fewer than two points → identity.
    pub fn new(points: &[(f64, f64)]) -> MonotoneCurve {
        let mut p: Vec<(f64, f64)> = points.iter().copied().filter(|(x, y)| x.is_finite() && y.is_finite()).collect();
        p.sort_by(|a, b| a.0.total_cmp(&b.0));
        p.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
        if p.len() < 2 {
            p = vec![(0.0, 0.0), (1.0, 1.0)];
        }
        let n = p.len();
        let xs: Vec<f64> = p.iter().map(|q| q.0).collect();
        let ys: Vec<f64> = p.iter().map(|q| q.1).collect();
        let d: Vec<f64> = (0..n - 1).map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i])).collect();
        let mut ms = vec![0.0; n];
        ms[0] = d[0];
        ms[n - 1] = d[n - 2];
        for i in 1..n - 1 {
            ms[i] = if d[i - 1] * d[i] <= 0.0 { 0.0 } else { (d[i - 1] + d[i]) / 2.0 };
        }
        // Fritsch–Carlson limiter where the data is monotone within a segment.
        for i in 0..n - 1 {
            if d[i] == 0.0 {
                ms[i] = 0.0;
                ms[i + 1] = 0.0;
                continue;
            }
            let a = ms[i] / d[i];
            let b = ms[i + 1] / d[i];
            let s = a * a + b * b;
            if s > 9.0 {
                let t = 3.0 / s.sqrt();
                ms[i] = t * a * d[i];
                ms[i + 1] = t * b * d[i];
            }
        }
        MonotoneCurve { xs, ys, ms }
    }

    pub fn identity() -> MonotoneCurve {
        MonotoneCurve::new(&[(0.0, 0.0), (1.0, 1.0)])
    }

    pub fn eval(&self, x: f64) -> f64 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0] + self.ms[0] * (x - self.xs[0]).min(0.0) * 0.0;
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1];
        }
        let i = match self.xs.binary_search_by(|v| v.total_cmp(&x)) {
            Ok(i) => return self.ys[i],
            Err(i) => i - 1,
        };
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.ms[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.ms[i + 1]
    }

    /// Sample into a LUT of `n` entries over `0..1`.
    pub fn to_lut(&self, n: usize) -> Lut1 {
        Lut1 { v: (0..n).map(|i| self.eval(i as f64 / (n - 1) as f64) as f32).collect() }
    }

    pub fn points(&self) -> Vec<(f64, f64)> {
        self.xs.iter().copied().zip(self.ys.iter().copied()).collect()
    }
}

/// A 1-D lookup table over `0..1` with linear interpolation; inputs outside clamp.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut1 {
    pub v: Vec<f32>,
}

impl Lut1 {
    pub fn identity(n: usize) -> Lut1 {
        Lut1 { v: (0..n).map(|i| i as f32 / (n - 1) as f32).collect() }
    }
    pub fn from_fn(n: usize, f: impl Fn(f32) -> f32) -> Lut1 {
        Lut1 { v: (0..n).map(|i| f(i as f32 / (n - 1) as f32)).collect() }
    }
    #[inline]
    pub fn eval(&self, x: f32) -> f32 {
        let n = self.v.len();
        let f = x.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (f as usize).min(n - 2);
        let t = f - i as f32;
        self.v[i] + (self.v[i + 1] - self.v[i]) * t
    }
    /// `self ∘ inner` (apply `inner` first).
    pub fn compose(&self, inner: &Lut1) -> Lut1 {
        Lut1 { v: inner.v.iter().map(|&x| self.eval(x)).collect() }
    }
    pub fn is_identity(&self) -> bool {
        let n = self.v.len();
        self.v.iter().enumerate().all(|(i, &y)| (y - i as f32 / (n - 1) as f32).abs() < 1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_curve() {
        let c = MonotoneCurve::identity();
        for i in 0..=10 {
            let x = i as f64 / 10.0;
            assert!((c.eval(x) - x).abs() < 1e-12);
        }
        assert!(c.to_lut(256).is_identity());
    }

    #[test]
    fn passes_through_points_and_is_monotone() {
        let pts = [(0.0, 0.0), (0.25, 0.15), (0.5, 0.5), (0.75, 0.88), (1.0, 1.0)];
        let c = MonotoneCurve::new(&pts);
        for (x, y) in pts {
            assert!((c.eval(x) - y).abs() < 1e-12);
        }
        let mut prev = -1.0;
        for i in 0..=1000 {
            let y = c.eval(i as f64 / 1000.0);
            assert!(y >= prev - 1e-12);
            prev = y;
        }
    }

    #[test]
    fn no_overshoot_on_flat_segment() {
        let c = MonotoneCurve::new(&[(0.0, 0.0), (0.4, 0.5), (0.6, 0.5), (1.0, 1.0)]);
        for i in 400..=600 {
            let y = c.eval(i as f64 / 1000.0);
            assert!((y - 0.5).abs() < 1e-9, "{y}");
        }
    }

    #[test]
    fn lut_compose() {
        let a = Lut1::from_fn(1024, |x| x * x);
        let b = Lut1::from_fn(1024, |x| x.sqrt());
        let c = a.compose(&b);
        assert!((c.eval(0.3) - 0.3).abs() < 2e-3);
    }

    proptest::proptest! {
        #[test]
        fn monotone_for_increasing_data(ys in proptest::collection::vec(0.0f64..1.0, 3..8)) {
            let mut ys = ys; ys.sort_by(|a, b| a.total_cmp(b));
            let n = ys.len();
            let pts: Vec<(f64, f64)> = ys.iter().enumerate().map(|(i, y)| (i as f64 / (n - 1) as f64, *y)).collect();
            let c = MonotoneCurve::new(&pts);
            let mut prev = f64::MIN;
            for i in 0..=200 { let y = c.eval(i as f64 / 200.0); proptest::prop_assert!(y >= prev - 1e-9); prev = y; }
        }
    }
}
