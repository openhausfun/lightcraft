//! Clip-aware highlight handling on demosaiced camera RGB (before white balance, white level = 1.0).
//!
//! - [`clip_neutral`]: white-balance-aware clipping so that sensor-clipped areas render neutral instead of
//!   magenta/cyan (each channel limited to the smallest white-balanced clip level).
//! - [`reconstruct`]: where only some channels are clipped, rebuild them from the unclipped channels using the
//!   local chromaticity of nearby unclipped pixels (diffused into the clipped region with a coarse-to-fine
//!   normalised-convolution fill). Fully clipped pixels become neutral at the brightest plausible level.

use lightcraft_raster::Rgb32f;
use rayon::prelude::*;

/// Clip every channel of `wb ⊙ img` at `min_c(wb_c) · clip` — i.e. at the lowest channel's clip level after WB —
/// then divide the multipliers back out. `clip` is the sensor clip in normalised units (≈ 1.0).
pub fn clip_neutral(img: &mut Rgb32f, wb: [f32; 3], clip: f32) {
    let limit = wb.iter().cloned().fold(f32::MAX, f32::min) * clip;
    for p in &mut img.data {
        for c in 0..3 {
            p[c] = (p[c] * wb[c]).min(limit) / wb[c];
        }
    }
}

/// Fill `values` (per-pixel vectors) where `valid` is false from valid neighbours, coarse to fine.
fn fill_invalid(w: usize, h: usize, values: &mut [[f32; 3]], valid: &mut [bool]) {
    if w == 0 || h == 0 || valid.iter().all(|&v| v) || !valid.iter().any(|&v| v) {
        return;
    }
    // Build a pyramid of weighted means, fill each level from the next coarser one.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut cv = vec![[0f32; 3]; cw * ch];
    let mut cval = vec![false; cw * ch];
    {
        let (values, valid) = (&*values, &*valid);
        downsample(w, h, &mut cv, &mut cval, |i| valid[i].then(|| values[i]));
    }
    if cw * ch < w * h {
        fill_invalid(cw, ch, &mut cv, &mut cval);
    } else {
        // cannot shrink further (1×1): nothing valid anywhere handled above
        return;
    }
    values.par_chunks_mut(w).zip(valid.par_chunks_mut(w)).enumerate().for_each(|(y, (vrow, okrow))| {
        for x in 0..w {
            if !okrow[x] {
                vrow[x] = upsample(&cv, cw, ch, x, y);
                okrow[x] = true;
            }
        }
    });
}

/// One pyramid step: each `2 × 2` block of a `w × h` level becomes the mean of its valid
/// entries (`at(i)`: the value at index `i` when valid) in `cv`, `cval` (`⌈w/2⌉ × ⌈h/2⌉`).
fn downsample(w: usize, h: usize, cv: &mut [[f32; 3]], cval: &mut [bool], at: impl Fn(usize) -> Option<[f32; 3]> + Sync) {
    let cw = w.div_ceil(2);
    cv.par_chunks_mut(cw).zip(cval.par_chunks_mut(cw)).enumerate().for_each(|(y, (vrow, okrow))| {
        for x in 0..cw {
            let (mut s, mut n) = ([0f32; 3], 0);
            for dy in 0..2 {
                for dx in 0..2 {
                    let (sx, sy) = (2 * x + dx, 2 * y + dy);
                    if sx < w
                        && sy < h
                        && let Some(v) = at(sy * w + sx)
                    {
                        for c in 0..3 {
                            s[c] += v[c];
                        }
                        n += 1;
                    }
                }
            }
            if n > 0 {
                vrow[x] = s.map(|v| v / n as f32);
                okrow[x] = true;
            }
        }
    });
}

/// Bilinear sample of the `cw × ch` coarse level at fine pixel `(x, y)` (twice the resolution).
fn upsample(cv: &[[f32; 3]], cw: usize, ch: usize, x: usize, y: usize) -> [f32; 3] {
    let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
    let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch - 1) as f32);
    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let g = |xx: usize, yy: usize| cv[yy * cw + xx];
    let mut v = [0f32; 3];
    for c in 0..3 {
        let top = g(x0, y0)[c] + (g(x1, y0)[c] - g(x0, y0)[c]) * tx;
        let bot = g(x0, y1)[c] + (g(x1, y1)[c] - g(x0, y1)[c]) * tx;
        v[c] = top + (bot - top) * ty;
    }
    v
}

/// White-balanced chromaticity of an unclipped pixel (`None`: clipped or too dark to tell).
#[inline]
fn chroma_of(p: &[f32; 3], wb: [f32; 3], clip: f32) -> Option<[f32; 3]> {
    if p[0] >= clip || p[1] >= clip || p[2] >= clip {
        return None;
    }
    let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
    let s = q[0] + q[1] + q[2];
    (s > 1e-4).then(|| q.map(|v| v.max(0.0) / s))
}

/// Reconstruct partially clipped channels. `clip` is the sensor clip level in normalised units (use slightly
/// below 1.0, e.g. 0.99), `wb` the white-balance multipliers that will be applied afterwards.
/// Returns the number of pixels that had at least one clipped channel.
///
/// The chromaticity field is diffused coarse to fine from the unclipped pixels. A clipped pixel is
/// never valid at full resolution, so its chromaticity is always the bilinear sample of the
/// half-resolution level: we start the pyramid there, straight from the image, and never build
/// the full-resolution field (same result, a quarter of the memory and far less work).
pub fn reconstruct(img: &mut Rgb32f, wb: [f32; 3], clip: f32) -> usize {
    let (w, h) = (img.width, img.height);
    let is_clipped = |p: &[f32; 3]| p[0] >= clip || p[1] >= clip || p[2] >= clip;
    let count = img.data.par_iter().filter(|p| is_clipped(p)).count();
    if count == 0 {
        return 0;
    }
    // chromaticity (white-balanced ratios to the channel sum) of unclipped pixels, at half resolution
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut cv = vec![[1f32 / 3.0; 3]; cw * ch];
    let mut cval = vec![false; cw * ch];
    {
        let data = &img.data;
        downsample(w, h, &mut cv, &mut cval, |i| chroma_of(&data[i], wb, clip));
    }
    if cw * ch < w * h {
        fill_invalid(cw, ch, &mut cv, &mut cval);
    }
    let any_valid = cw * ch < w * h && cval.iter().any(|&v| v);
    let max_level = wb.iter().cloned().fold(0.0f32, f32::max) * clip;
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, px) in row.iter_mut().enumerate() {
            let p = *px;
            let cl = [p[0] >= clip, p[1] >= clip, p[2] >= clip];
            if !cl.iter().any(|&b| b) {
                continue;
            }
            let r = if any_valid { upsample(&cv, cw, ch, x, y) } else { [1f32 / 3.0; 3] };
            let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            // estimate the white-balanced channel sum from the unclipped channels
            let (mut sum, mut k) = (0f32, 0usize);
            for c in 0..3 {
                if !cl[c] && r[c] > 1e-3 {
                    sum += q[c] / r[c];
                    k += 1;
                }
            }
            let mut out = q;
            if k == 0 {
                let v = q.iter().cloned().fold(0.0f32, f32::max).max(max_level);
                out = [v; 3];
            } else {
                let sum = sum / k as f32;
                for c in 0..3 {
                    if cl[c] {
                        out[c] = q[c].max(sum * r[c]);
                    }
                }
            }
            *px = [out[0] / wb[0], out[1] / wb[1], out[2] / wb[2]];
        }
    });
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    // The previous full-resolution implementation, kept as the reference for `reconstruct`.
    /// Fill `values` (per-pixel vectors) where `valid` is false from valid neighbours, coarse to fine.
    fn fill_invalid_ref(w: usize, h: usize, values: &mut [[f32; 3]], valid: &mut [bool]) {
        if w == 0 || h == 0 || valid.iter().all(|&v| v) || !valid.iter().any(|&v| v) {
            return;
        }
        // Build a pyramid of weighted means, fill each level from the next coarser one.
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut cv = vec![[0f32; 3]; cw * ch];
        let mut cval = vec![false; cw * ch];
        {
            let (values, valid) = (&*values, &*valid);
            cv.par_chunks_mut(cw).zip(cval.par_chunks_mut(cw)).enumerate().for_each(|(y, (vrow, okrow))| {
                for x in 0..cw {
                    let (mut s, mut n) = ([0f32; 3], 0);
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let (sx, sy) = (2 * x + dx, 2 * y + dy);
                            if sx < w && sy < h && valid[sy * w + sx] {
                                for c in 0..3 {
                                    s[c] += values[sy * w + sx][c];
                                }
                                n += 1;
                            }
                        }
                    }
                    if n > 0 {
                        vrow[x] = s.map(|v| v / n as f32);
                        okrow[x] = true;
                    }
                }
            });
        }
        if cw * ch < w * h {
            fill_invalid_ref(cw, ch, &mut cv, &mut cval);
        } else {
            // cannot shrink further (1×1): nothing valid anywhere handled above
            return;
        }
        values.par_chunks_mut(w).zip(valid.par_chunks_mut(w)).enumerate().for_each(|(y, (vrow, okrow))| {
            for x in 0..w {
                if !okrow[x] {
                    // bilinear upsample of the coarse level
                    let fx = ((x as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (cw - 1) as f32);
                    let fy = ((y as f32 + 0.5) / 2.0 - 0.5).clamp(0.0, (ch - 1) as f32);
                    let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
                    let (x1, y1) = ((x0 + 1).min(cw - 1), (y0 + 1).min(ch - 1));
                    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                    let g = |xx: usize, yy: usize| cv[yy * cw + xx];
                    let mut v = [0f32; 3];
                    for c in 0..3 {
                        let top = g(x0, y0)[c] + (g(x1, y0)[c] - g(x0, y0)[c]) * tx;
                        let bot = g(x0, y1)[c] + (g(x1, y1)[c] - g(x0, y1)[c]) * tx;
                        v[c] = top + (bot - top) * ty;
                    }
                    vrow[x] = v;
                    okrow[x] = true;
                }
            }
        });
    }

    /// Reconstruct partially clipped channels. `clip` is the sensor clip level in normalised units (use slightly
    /// below 1.0, e.g. 0.99), `wb` the white-balance multipliers that will be applied afterwards.
    /// Returns the number of pixels that had at least one clipped channel.
    fn reconstruct_ref(img: &mut Rgb32f, wb: [f32; 3], clip: f32) -> usize {
        let (w, h) = (img.width, img.height);
        let n = w * h;
        let is_clipped = |p: &[f32; 3]| p[0] >= clip || p[1] >= clip || p[2] >= clip;
        let count = img.data.par_iter().filter(|p| is_clipped(p)).count();
        if count == 0 {
            return 0;
        }
        // chromaticity (white-balanced ratios to the channel mean) of unclipped pixels
        let mut chroma = vec![[1f32 / 3.0; 3]; n];
        let mut valid = vec![false; n];
        chroma.par_iter_mut().zip(valid.par_iter_mut()).zip(img.data.par_iter()).for_each(|((r, v), p)| {
            if is_clipped(p) {
                return;
            }
            let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            let s = q[0] + q[1] + q[2];
            if s > 1e-4 {
                *r = q.map(|v| v.max(0.0) / s);
                *v = true;
            }
        });
        fill_invalid_ref(w, h, &mut chroma, &mut valid);
        let max_level = wb.iter().cloned().fold(0.0f32, f32::max) * clip;
        img.data.par_iter_mut().zip(chroma.par_iter()).for_each(|(px, r)| {
            let p = *px;
            let cl = [p[0] >= clip, p[1] >= clip, p[2] >= clip];
            if !cl.iter().any(|&b| b) {
                return;
            }
            let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
            // estimate the white-balanced channel sum from the unclipped channels
            let (mut sum, mut k) = (0f32, 0usize);
            for c in 0..3 {
                if !cl[c] && r[c] > 1e-3 {
                    sum += q[c] / r[c];
                    k += 1;
                }
            }
            let mut out = q;
            if k == 0 {
                let v = q.iter().cloned().fold(0.0f32, f32::max).max(max_level);
                out = [v; 3];
            } else {
                let sum = sum / k as f32;
                for c in 0..3 {
                    if cl[c] {
                        out[c] = q[c].max(sum * r[c]);
                    }
                }
            }
            *px = [out[0] / wb[0], out[1] / wb[1], out[2] / wb[2]];
        });
        count
    }

    /// `cargo test --release -p lightcraft-raw --lib -- --ignored bench_reconstruct --nocapture`
    #[test]
    #[ignore]
    fn bench_reconstruct() {
        let img = Rgb32f::from_fn(6000, 4000, |x, y| {
            let v = 0.4 + 0.8 * ((x as f32 * 0.003).sin() * (y as f32 * 0.002).cos()).abs();
            [v.min(1.0), (v * 0.7).min(1.0), (v * 0.5).min(1.0)]
        });
        let wb = [2.0, 1.0, 1.5];
        let time = |f: &dyn Fn(&mut Rgb32f) -> usize| {
            (0..3)
                .map(|_| {
                    let mut i = img.clone();
                    let t = std::time::Instant::now();
                    f(&mut i);
                    t.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::MAX, f64::min)
        };
        let new = time(&|i| reconstruct(i, wb, 0.99));
        let old = time(&|i| reconstruct_ref(i, wb, 0.99));
        eprintln!("reconstruct 24 MP: {new:.0} ms (full-resolution reference: {old:.0} ms)");
    }

    /// The half-resolution start gives exactly the full-resolution algorithm's result.
    #[test]
    fn matches_the_full_resolution_reference() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        for (w, h) in [(1, 1), (1, 2), (3, 1), (7, 5), (64, 48), (129, 77)] {
            for density in [0.02f32, 0.3, 0.97] {
                // smooth colour with clipped blobs, some dark pixels, some fully clipped ones
                let noise: Vec<f32> = (0..w * h).map(|_| rnd()).collect();
                let img = Rgb32f::from_fn(w, h, |x, y| {
                    let t = ((x as f32 * 0.21).sin() + (y as f32 * 0.17).cos()) * 0.5;
                    let base = [0.5 + 0.4 * t, 0.45 - 0.2 * t, 0.3 + 0.1 * t];
                    let r = noise[y * w + x];
                    if r < density * 0.3 {
                        [1.0, 1.0, 1.0]
                    } else if r < density {
                        [1.0, base[1] * 1.6, base[2]]
                    } else if r > 0.995 {
                        [0.0, 0.0, 0.00001]
                    } else {
                        base
                    }
                });
                let wb = [2.1, 1.0, 1.6];
                let (mut a, mut b) = (img.clone(), img.clone());
                assert_eq!(reconstruct(&mut a, wb, 0.99), reconstruct_ref(&mut b, wb, 0.99));
                assert!(a.data == b.data, "{w}×{h} at {density}");
            }
        }
    }

    #[test]
    fn clip_neutral_makes_clipped_white_neutral() {
        let wb = [2.0, 1.0, 1.5];
        let mut img = Rgb32f::filled(2, 1, [1.0, 1.0, 1.0]);
        img.data[1] = [0.2, 0.3, 0.4];
        clip_neutral(&mut img, wb, 1.0);
        let p = img.data[0];
        let q = [p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]];
        assert!((q[0] - q[1]).abs() < 1e-6 && (q[1] - q[2]).abs() < 1e-6);
        assert_eq!(img.data[1], [0.2, 0.3, 0.4]);
    }

    #[test]
    fn reconstruct_restores_clipped_channel() {
        // a warm gradient whose red channel clips in the bright half
        let truth = Rgb32f::from_fn(64, 16, |x, _| {
            let v = 0.3 + 1.2 * x as f32 / 63.0;
            [v * 1.0, v * 0.6, v * 0.35]
        });
        let mut img = truth.map(|p| p.map(|v| v.min(1.0)));
        let before: f32 = img.data.iter().zip(&truth.data).map(|(a, b)| (a[0] - b[0]).abs()).sum();
        let n = reconstruct(&mut img, [1.0, 1.0, 1.0], 0.999);
        assert!(n > 0);
        let after: f32 = img.data.iter().zip(&truth.data).map(|(a, b)| (a[0] - b[0]).abs()).sum();
        assert!(after < before * 0.2, "before {before} after {after}");
        // unclipped pixels untouched
        assert_eq!(img.get(0, 0), truth.get(0, 0));
    }

    #[test]
    fn fully_clipped_and_no_clipping() {
        let mut img = Rgb32f::filled(4, 4, [1.0; 3]);
        assert_eq!(reconstruct(&mut img, [2.0, 1.0, 1.5], 0.99), 16);
        let p = img.data[0];
        assert!((p[0] * 2.0 - p[1]).abs() < 1e-5 && (p[2] * 1.5 - p[1]).abs() < 1e-5);
        let mut img = Rgb32f::filled(4, 4, [0.5; 3]);
        assert_eq!(reconstruct(&mut img, [2.0, 1.0, 1.5], 0.99), 0);
        let mut empty = Rgb32f::new(0, 0);
        assert_eq!(reconstruct(&mut empty, [1.0; 3], 0.99), 0);
    }
}
