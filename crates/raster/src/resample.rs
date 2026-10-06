//! Separable resampling with precomputed filter weights.

use crate::{Image, par_rows};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Box,
    Bilinear,
    Mitchell,
    Lanczos3,
}

impl Filter {
    fn support(self) -> f32 {
        match self {
            Filter::Box => 0.5,
            Filter::Bilinear => 1.0,
            Filter::Mitchell => 2.0,
            Filter::Lanczos3 => 3.0,
        }
    }
    fn eval(self, x: f32) -> f32 {
        let x = x.abs();
        match self {
            Filter::Box => {
                if x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Filter::Bilinear => (1.0 - x).max(0.0),
            Filter::Mitchell => {
                let (b, c) = (1.0 / 3.0, 1.0 / 3.0);
                if x < 1.0 {
                    ((12.0 - 9.0 * b - 6.0 * c) * x * x * x + (-18.0 + 12.0 * b + 6.0 * c) * x * x + (6.0 - 2.0 * b)) / 6.0
                } else if x < 2.0 {
                    ((-b - 6.0 * c) * x * x * x + (6.0 * b + 30.0 * c) * x * x + (-12.0 * b - 48.0 * c) * x + (8.0 * b + 24.0 * c)) / 6.0
                } else {
                    0.0
                }
            }
            Filter::Lanczos3 => {
                if x < 1e-6 {
                    1.0
                } else if x < 3.0 {
                    let px = std::f32::consts::PI * x;
                    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
                } else {
                    0.0
                }
            }
        }
    }
}

/// Contributions for each output coordinate: (first source index, weights) — the exact taps
/// [`resize`] uses (GPU kernels upload these tables).
pub fn weights(src: usize, dst: usize, filter: Filter) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let fscale = scale.max(1.0);
    let support = filter.support() * fscale;
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale;
            let lo = ((center - support).floor() as isize).max(0) as usize;
            let hi = ((center + support).ceil() as usize).min(src);
            let mut w: Vec<f32> = (lo..hi).map(|j| filter.eval((j as f32 + 0.5 - center) / fscale)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > 1e-8 {
                w.iter_mut().for_each(|v| *v /= sum);
            } else if !w.is_empty() {
                let n = w.len() as f32;
                w.iter_mut().for_each(|v| *v = 1.0 / n);
            }
            (lo, w)
        })
        .collect()
}

/// Pixel types that can be resampled (linear combination).
pub trait Pixel: Copy + Default + Send + Sync {
    fn zero() -> Self;
    fn madd(self, o: Self, w: f32) -> Self;
}

impl Pixel for f32 {
    fn zero() -> Self {
        0.0
    }
    #[inline]
    fn madd(self, o: Self, w: f32) -> Self {
        self + o * w
    }
}

impl Pixel for [f32; 3] {
    fn zero() -> Self {
        [0.0; 3]
    }
    #[inline]
    fn madd(self, o: Self, w: f32) -> Self {
        [self[0] + o[0] * w, self[1] + o[1] * w, self[2] + o[2] * w]
    }
}

impl Pixel for [f32; 4] {
    fn zero() -> Self {
        [0.0; 4]
    }
    #[inline]
    fn madd(self, o: Self, w: f32) -> Self {
        [self[0] + o[0] * w, self[1] + o[1] * w, self[2] + o[2] * w, self[3] + o[3] * w]
    }
}

/// Resize to `w × h` (separable; horizontal pass then vertical pass).
pub fn resize<T: Pixel>(img: &Image<T>, w: usize, h: usize, filter: Filter) -> Image<T> {
    let (w, h) = (w.max(1), h.max(1));
    if img.width == w && img.height == h {
        return img.clone();
    }
    let wx = weights(img.width, w, filter);
    let mut tmp = Image::<T>::new(w, img.height);
    par_rows(&mut tmp.data, w, |y, row| {
        let src = img.row(y);
        for (x, o) in row.iter_mut().enumerate() {
            let (lo, ws) = &wx[x];
            let mut acc = T::zero();
            for (k, wt) in ws.iter().enumerate() {
                acc = acc.madd(src[lo + k], *wt);
            }
            *o = acc;
        }
    });
    let wy = weights(img.height, h, filter);
    let mut out = Image::<T>::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        let (lo, ws) = &wy[y];
        for (k, wt) in ws.iter().enumerate() {
            let src = tmp.row(lo + k);
            for (o, s) in row.iter_mut().zip(src) {
                *o = o.madd(*s, *wt);
            }
        }
    });
    out
}

/// Fit within `max_w × max_h` preserving aspect (never upscales).
pub fn fit<T: Pixel>(img: &Image<T>, max_w: usize, max_h: usize, filter: Filter) -> Image<T> {
    let s = (max_w as f64 / img.width as f64).min(max_h as f64 / img.height as f64).min(1.0);
    let w = ((img.width as f64 * s).round() as usize).max(1);
    let h = ((img.height as f64 * s).round() as usize).max(1);
    resize(img, w, h, filter)
}

/// 2× box downsample (pyramid level).
pub fn half<T: Pixel>(img: &Image<T>) -> Image<T> {
    let w = img.width.div_ceil(2).max(1);
    let h = img.height.div_ceil(2).max(1);
    let mut out = Image::<T>::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            let (sx, sy) = (2 * x as isize, 2 * y as isize);
            *o = T::zero()
                .madd(img.get_clamped(sx, sy), 0.25)
                .madd(img.get_clamped(sx + 1, sy), 0.25)
                .madd(img.get_clamped(sx, sy + 1), 0.25)
                .madd(img.get_clamped(sx + 1, sy + 1), 0.25);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_image_stays_constant() {
        let img = Image::<f32>::filled(37, 21, 0.42);
        for f in [Filter::Box, Filter::Bilinear, Filter::Mitchell, Filter::Lanczos3] {
            for (w, h) in [(10, 7), (80, 40), (37, 21), (1, 1)] {
                let r = resize(&img, w, h, f);
                assert!(r.data.iter().all(|v| (v - 0.42).abs() < 1e-5), "{f:?} {w}x{h}");
            }
        }
    }

    #[test]
    fn downscale_preserves_mean() {
        let img = Image::<f32>::from_fn(64, 64, |x, y| ((x * 7 + y * 3) % 11) as f32 / 10.0);
        let mean = img.data.iter().sum::<f32>() / img.len() as f32;
        let r = resize(&img, 16, 16, Filter::Lanczos3);
        let m2 = r.data.iter().sum::<f32>() / r.len() as f32;
        assert!((mean - m2).abs() < 0.01);
        let hh = half(&img);
        assert_eq!((hh.width, hh.height), (32, 32));
        let m3 = hh.data.iter().sum::<f32>() / hh.len() as f32;
        assert!((mean - m3).abs() < 1e-5);
    }

    #[test]
    fn fit_keeps_aspect() {
        let img = Image::<f32>::new(4000, 3000);
        let r = fit(&img, 400, 400, Filter::Box);
        assert_eq!((r.width, r.height), (400, 300));
        let small = fit(&Image::<f32>::new(10, 5), 400, 400, Filter::Box);
        assert_eq!((small.width, small.height), (10, 5));
    }
}
