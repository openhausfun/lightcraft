//! Demosaicing for non-Bayer patterns (Fujifilm X-Trans and others): our own edge-weighted colour-difference
//! interpolation.
//!
//! 1. Green at non-green sites: robust weighted mean of the green samples in the 3×3 neighbourhood (5×5 if
//!    none), each weighted by `1 / (ε + |g − median|)` so samples across an edge contribute little.
//! 2. Red and blue everywhere: `G(p) + Σ w_q (C(q) − G(q)) / Σ w_q` over same-colour samples `q` in the 5×5
//!    neighbourhood, with `w_q = 1 / (d²(p, q) · (ε + |G(q) − G(p)|))` — spatially close samples on the same
//!    side of an edge dominate.

use super::Mosaic;
use crate::Rgb32f;
use lightcraft_raster::par_rows;

const EPS: f32 = 1e-3;

/// Same-colour neighbour offsets for every pattern phase, in scan order (so sums run in the same
/// order as a plain 2-D loop and interior pixels match border pixels bit for bit).
struct Offsets {
    pw: usize,
    ph: usize,
    /// Per phase: green offsets at radius 1, then those at radius 2 (ring order as the loop below).
    green: Vec<(Vec<(isize, isize)>, Vec<(isize, isize)>)>,
    /// Per phase and colour (0 = red, 1 = blue): 5×5 offsets of that colour.
    rb: Vec<[Vec<(isize, isize)>; 2]>,
}

impl Offsets {
    fn new(m: &Mosaic) -> Offsets {
        let (pw, ph) = (m.cfa.width.max(1), m.cfa.height.max(1));
        // phases are evaluated far from the borders (pattern coordinates wrap)
        let col = |x: isize, y: isize| m.cfa.color_at(x.rem_euclid(pw as isize) as usize, y.rem_euclid(ph as isize) as usize);
        let mut green = Vec::new();
        let mut rb = Vec::new();
        for py in 0..ph as isize {
            for px in 0..pw as isize {
                let ring = |r: isize| {
                    let mut v = Vec::new();
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if (dx != 0 || dy != 0) && col(px + dx, py + dy) == 1 {
                                v.push((dx, dy));
                            }
                        }
                    }
                    v
                };
                green.push((ring(1), ring(2)));
                let of = |c: u8| {
                    let mut v = Vec::new();
                    for dy in -2isize..=2 {
                        for dx in -2isize..=2 {
                            if col(px + dx, py + dy) == c {
                                v.push((dx, dy));
                            }
                        }
                    }
                    v
                };
                rb.push([of(0), of(2)]);
            }
        }
        Offsets { pw, ph, green, rb }
    }
    #[inline]
    fn phase(&self, x: usize, y: usize) -> usize {
        (y % self.ph) * self.pw + x % self.pw
    }
}

/// Robust weighted mean of `vals` around their median.
#[inline]
fn robust_mean(vals: &[f32]) -> f32 {
    let mut sorted = [0f32; 24];
    let sorted = &mut sorted[..vals.len()];
    sorted.copy_from_slice(vals);
    sorted.sort_unstable_by(|a, b| a.total_cmp(b));
    let med = sorted[sorted.len() / 2];
    let (mut s, mut ws) = (0.0f32, 0.0f32);
    for &v in vals {
        let wt = 1.0 / (EPS + (v - med).abs());
        s += wt * v;
        ws += wt;
    }
    s / ws
}

fn green(m: &Mosaic, off: &Offsets) -> Vec<f32> {
    let (w, h) = (m.w, m.h);
    let mut g = vec![0f32; w * h];
    par_rows(&mut g, w, |y, row| {
        let interior_y = y >= 2 && y + 2 < h;
        let yi = y as isize;
        let mut vals: Vec<f32> = Vec::with_capacity(24);
        for (x, o) in row.iter_mut().enumerate() {
            if interior_y && x >= 2 && x + 2 < w {
                // fast path: precomputed offsets, no reflection
                let ph = off.phase(x, y);
                let i = y * w + x;
                if m.cfa.color_at(x, y) == 1 {
                    *o = m.data[i];
                    continue;
                }
                let (r1, r2) = &off.green[ph];
                let ring = if r1.is_empty() { r2 } else { r1 };
                if ring.is_empty() {
                    *o = m.data[i];
                    continue;
                }
                vals.clear();
                vals.extend(ring.iter().map(|&(dx, dy)| m.data[(i as isize + dy * w as isize + dx) as usize]));
                *o = robust_mean(&vals);
                continue;
            }
            let x = x as isize;
            if m.color(x, yi) == 1 {
                *o = m.get(x, yi);
                continue;
            }
            vals.clear();
            for r in [1isize, 2] {
                for dy in -r..=r {
                    for dx in -r..=r {
                        if (dx != 0 || dy != 0) && m.color(x + dx, yi + dy) == 1 {
                            vals.push(m.get(x + dx, yi + dy));
                        }
                    }
                }
                if !vals.is_empty() {
                    break;
                }
            }
            if vals.is_empty() {
                *o = m.get(x, yi);
                continue;
            }
            *o = robust_mean(&vals);
        }
    });
    g
}

pub(crate) fn directional(m: &Mosaic) -> Rgb32f {
    let (w, h) = (m.w, m.h);
    let off = Offsets::new(m);
    let g = green(m, &off);
    let gg = |x: isize, y: isize| g[super::reflect(y, h) * w + super::reflect(x, w)];
    let mut out = Rgb32f::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        let interior_y = y >= 2 && y + 2 < h;
        let yi = y as isize;
        for (x, px) in row.iter_mut().enumerate() {
            if interior_y && x >= 2 && x + 2 < w {
                let i = y * w + x;
                let own = m.cfa.color_at(x, y) as usize;
                let gp = g[i];
                px[1] = gp;
                let lists = &off.rb[off.phase(x, y)];
                for (li, c) in [0usize, 2].into_iter().enumerate() {
                    if c == own {
                        px[c] = m.data[i];
                        continue;
                    }
                    let (mut s, mut ws) = (0.0f32, 0.0f32);
                    for &(dx, dy) in &lists[li] {
                        let j = (i as isize + dy * w as isize + dx) as usize;
                        let gq = g[j];
                        let d2 = (dx * dx + dy * dy) as f32;
                        let wt = 1.0 / (d2 * (EPS + (gq - gp).abs()));
                        s += wt * (m.data[j] - gq);
                        ws += wt;
                    }
                    px[c] = if ws > 0.0 { gp + s / ws } else { gp };
                }
                continue;
            }
            let x = x as isize;
            let own = m.color(x, yi) as usize;
            let gp = gg(x, yi);
            px[1] = gp;
            for c in [0usize, 2] {
                if c == own {
                    px[c] = m.get(x, yi);
                    continue;
                }
                let (mut s, mut ws) = (0.0f32, 0.0f32);
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        if m.color(x + dx, yi + dy) as usize != c {
                            continue;
                        }
                        let gq = gg(x + dx, yi + dy);
                        let d2 = (dx * dx + dy * dy) as f32;
                        let wt = 1.0 / (d2 * (EPS + (gq - gp).abs()));
                        s += wt * (m.get(x + dx, yi + dy) - gq);
                        ws += wt;
                    }
                }
                px[c] = if ws > 0.0 { gp + s / ws } else { gp };
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cfa;

    /// The straightforward per-pixel formulation (reflection at every access): the precomputed
    /// interior path must match it bit for bit.
    fn reference(m: &Mosaic) -> Rgb32f {
        let (w, h) = (m.w, m.h);
        let mut g = vec![0f32; w * h];
        for y in 0..h as isize {
            for x in 0..w as isize {
                let o = &mut g[y as usize * w + x as usize];
                if m.color(x, y) == 1 {
                    *o = m.get(x, y);
                    continue;
                }
                let mut vals = Vec::new();
                for r in [1isize, 2] {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            if (dx != 0 || dy != 0) && m.color(x + dx, y + dy) == 1 {
                                vals.push(m.get(x + dx, y + dy));
                            }
                        }
                    }
                    if !vals.is_empty() {
                        break;
                    }
                }
                *o = if vals.is_empty() { m.get(x, y) } else { robust_mean(&vals) };
            }
        }
        let gg = |x: isize, y: isize| g[super::super::reflect(y, h) * w + super::super::reflect(x, w)];
        Rgb32f::from_fn(w, h, |x, y| {
            let (x, y) = (x as isize, y as isize);
            let own = m.color(x, y) as usize;
            let gp = gg(x, y);
            let mut px = [0f32, gp, 0.0];
            for c in [0usize, 2] {
                if c == own {
                    px[c] = m.get(x, y);
                    continue;
                }
                let (mut s, mut ws) = (0.0f32, 0.0f32);
                for dy in -2isize..=2 {
                    for dx in -2isize..=2 {
                        if m.color(x + dx, y + dy) as usize == c {
                            let gq = gg(x + dx, y + dy);
                            let wt = 1.0 / ((dx * dx + dy * dy) as f32 * (EPS + (gq - gp).abs()));
                            s += wt * (m.get(x + dx, y + dy) - gq);
                            ws += wt;
                        }
                    }
                }
                px[c] = if ws > 0.0 { gp + s / ws } else { gp };
            }
            px
        })
    }

    #[test]
    fn fast_path_is_bit_exact() {
        let cfa = Cfa::xtrans().shifted(1, 2);
        let (w, h) = (37, 29);
        let mut seed = 12345u32;
        let data: Vec<f32> = (0..w * h)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32
            })
            .collect();
        let m = Mosaic { w, h, data: &data, cfa: &cfa };
        let (a, b) = (directional(&m), reference(&m));
        assert!(a.data.iter().zip(&b.data).all(|(p, q)| p.map(f32::to_bits) == q.map(f32::to_bits)));
    }
}
