//! Panorama finishing: boundary warp, auto crop, fill edges.
//!
//! - **Boundary warp** (0–100): a separable stretch that maps, per column, the smoothed top and
//!   bottom edges of the valid area to the canvas edges, then per row the left and right edges;
//!   `amount` blends between the identity and that stretch. (A simple, documented alternative to
//!   content-preserving mesh warps.)
//! - **Auto crop**: the largest axis-aligned rectangle of fully valid pixels (maximal rectangle in a
//!   binary matrix via a per-row histogram and a monotone stack, O(w·h)).
//! - **Fill edges**: transparent areas are filled by push–pull diffusion of the surrounding pixels
//!   in log space (no patch synthesis).

use lightcraft_raster::{Plane, Rgb32f};
use rayon::prelude::*;

use super::blend::{Buf, push_pull};

fn edges(alpha: &Plane, vertical: bool) -> Option<(Vec<f64>, Vec<f64>)> {
    let (w, h) = (alpha.width, alpha.height);
    let (n, len) = if vertical { (w, h) } else { (h, w) };
    let at = |i: usize, k: usize| if vertical { alpha.get(i, k) } else { alpha.get(k, i) };
    let mut lo = vec![f64::NAN; n];
    let mut hi = vec![f64::NAN; n];
    for i in 0..n {
        if let Some(a) = (0..len).find(|&k| at(i, k) >= 0.5) {
            let b = (0..len).rev().find(|&k| at(i, k) >= 0.5).unwrap_or(a);
            lo[i] = a as f64;
            hi[i] = b as f64 + 1.0;
        }
    }
    let known: Vec<usize> = (0..n).filter(|&i| lo[i].is_finite()).collect();
    if known.len() < 2 {
        return None;
    }
    // fill gaps by linear interpolation / nearest at the ends
    for i in 0..n {
        if lo[i].is_finite() {
            continue;
        }
        let p = known.iter().rev().find(|&&k| k < i).copied();
        let q = known.iter().find(|&&k| k > i).copied();
        let (a, b) = match (p, q) {
            (Some(p), Some(q)) => {
                let t = (i - p) as f64 / (q - p) as f64;
                (lo[p] + (lo[q] - lo[p]) * t, hi[p] + (hi[q] - hi[p]) * t)
            }
            (Some(p), None) => (lo[p], hi[p]),
            (None, Some(q)) => (lo[q], hi[q]),
            _ => (0.0, len as f64),
        };
        lo[i] = a;
        hi[i] = b;
    }
    // smooth (box, radius n/40) so the stretch follows the overall shape, not every jag
    let r = (n / 40).max(1) as isize;
    let smooth = |v: &[f64]| -> Vec<f64> {
        (0..n as isize)
            .map(|i| {
                let (a, b) = ((i - r).max(0) as usize, ((i + r) as usize).min(n - 1));
                v[a..=b].iter().sum::<f64>() / (b - a + 1) as f64
            })
            .collect()
    };
    Some((smooth(&lo), smooth(&hi)))
}

fn stretch(img: &Rgb32f, alpha: &Plane, amount: f64, vertical: bool) -> (Rgb32f, Plane) {
    let Some((lo, hi)) = edges(alpha, vertical) else { return (img.clone(), alpha.clone()) };
    let (w, h) = (img.width, img.height);
    let len = if vertical { h } else { w } as f64;
    let mut out = Rgb32f::new(w, h);
    let mut oa = Plane::new(w, h);
    out.data.par_chunks_mut(w).zip(oa.data.par_chunks_mut(w)).enumerate().for_each(|(y, (row, arow))| {
        for x in 0..w {
            let (i, k) = if vertical { (x, y) } else { (y, x) };
            let t = (k as f64 + 0.5) / len;
            let src = lo[i] + t * (hi[i] - lo[i]);
            let s = (1.0 - amount) * (k as f64 + 0.5) + amount * src;
            let (sx, sy) = if vertical { (x as f32 + 0.5, s as f32) } else { (s as f32, y as f32 + 0.5) };
            row[x] = img.sample_bilinear(sx, sy);
            arow[x] = crate::features::bilinear(alpha, sx, sy);
        }
    });
    (out, oa)
}

/// Boundary warp by `amount` (0..1).
pub fn boundary_warp(img: &Rgb32f, alpha: &Plane, amount: f64) -> (Rgb32f, Plane) {
    if amount <= 0.0 {
        return (img.clone(), alpha.clone());
    }
    let amount = amount.min(1.0);
    let (i1, a1) = stretch(img, alpha, amount, true);
    stretch(&i1, &a1, amount, false)
}

/// Largest axis-aligned rectangle where `alpha ≥ 0.999`: (x, y, w, h) in pixels.
pub fn auto_crop(alpha: &Plane) -> Option<(usize, usize, usize, usize)> {
    let (w, h) = (alpha.width, alpha.height);
    if w == 0 || h == 0 {
        return None;
    }
    // work on a ≤ 1500 px grid with conservative (min) pooling
    let f = w.max(h).div_ceil(1500).max(1);
    let (gw, gh) = (w.div_ceil(f), h.div_ceil(f));
    let ok = |gx: usize, gy: usize| -> bool {
        for y in gy * f..((gy + 1) * f).min(h) {
            for x in gx * f..((gx + 1) * f).min(w) {
                if alpha.get(x, y) < 0.999 {
                    return false;
                }
            }
        }
        true
    };
    let grid: Vec<bool> = (0..gh).into_par_iter().flat_map_iter(|gy| (0..gw).map(move |gx| (gx, gy))).map(|(gx, gy)| ok(gx, gy)).collect();
    let mut heights = vec![0usize; gw];
    let mut best = (0usize, 0, 0, 0, 0); // area, x, y, w, h
    for gy in 0..gh {
        for gx in 0..gw {
            heights[gx] = if grid[gy * gw + gx] { heights[gx] + 1 } else { 0 };
        }
        let mut stack: Vec<usize> = Vec::new();
        for i in 0..=gw {
            let cur = if i < gw { heights[i] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= cur {
                    break;
                }
                stack.pop();
                let hgt = heights[top];
                let left = stack.last().map_or(0, |&s| s + 1);
                let width = i - left;
                let area = hgt * width;
                if area > best.0 {
                    best = (area, left, gy + 1 - hgt, width, hgt);
                }
            }
            stack.push(i);
        }
    }
    if best.0 == 0 {
        return None;
    }
    let (x, y) = (best.1 * f, best.2 * f);
    Some((x, y, (best.3 * f).min(w - x), (best.4 * f).min(h - y)))
}

/// Fill transparent pixels (alpha < 1) from their surroundings; returns the image with alpha 1.
pub fn fill_edges(img: &Rgb32f, alpha: &Plane) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let mut b = Buf::new(w, h, 3);
    for (i, p) in img.data.iter().enumerate() {
        for c in 0..3 {
            b.data[i * 3 + c] = (p[c].max(1e-6)).ln();
        }
    }
    let wgt: Vec<f32> = alpha.data.iter().map(|a| if *a >= 0.999 { 1.0 } else { 0.0 }).collect();
    push_pull(&mut b, &wgt);
    let mut out = img.clone();
    for (i, p) in out.data.iter_mut().enumerate() {
        let a = alpha.data[i].clamp(0.0, 1.0);
        if a < 0.999 {
            let f = [0, 1, 2].map(|c| b.data[i * 3 + c].exp());
            // the warped/blended edge pixels are premultiplied-like: un-premultiply then mix
            let v = if a > 1e-3 { p.map(|v| v / a) } else { [0.0; 3] };
            *p = [0, 1, 2].map(|c| v[c] * a + f[c] * (1.0 - a));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_finds_the_largest_rectangle() {
        let mut a = Plane::new(100, 60);
        for y in 10..50 {
            for x in 5..90 {
                a.set(x, y, 1.0);
            }
        }
        for y in 0..60 {
            a.set(40, y, 1.0); // a thin spike doesn't matter
        }
        assert_eq!(auto_crop(&a), Some((5, 10, 85, 40)));
    }

    #[test]
    fn warp_fills_a_curved_boundary() {
        let (w, h) = (200, 100);
        let mut a = Plane::new(w, h);
        let img = Rgb32f::from_fn(w, h, |x, _| [(x + 1) as f32 / w as f32; 3]);
        for x in 0..w {
            let dip = (10.0 * (1.0 - ((x as f32 - 100.0) / 100.0).powi(2))) as usize;
            for y in dip..h - dip {
                a.set(x, y, 1.0);
            }
        }
        let before = a.data.iter().filter(|v| **v >= 0.5).count();
        let (_, a2) = boundary_warp(&img, &a, 1.0);
        let after = a2.data.iter().filter(|v| **v >= 0.5).count();
        assert!(after as f64 > 0.97 * (w * h) as f64 && after > before, "{before} → {after}");
        let (_, a0) = boundary_warp(&img, &a, 0.0);
        assert_eq!(a0, a);
        let filled = fill_edges(&img, &a);
        assert!(filled.data.iter().all(|p| p[0] > 0.0));
    }
}
