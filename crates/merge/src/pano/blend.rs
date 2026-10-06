//! Multi-band blending (P. J. Burt & E. H. Adelson, "A Multiresolution Spline With Application to
//! Image Mosaics", ACM ToG 1983), as used by Brown & Lowe: each image's Laplacian pyramid is
//! blended with the Gaussian pyramid of its (binary) seam mask, so low frequencies blend over wide
//! regions and high frequencies over narrow ones. Blending happens in log space, which turns
//! residual exposure differences into additive offsets the coarse bands smooth away.
//!
//! Images arrive as tiles placed on a canvas padded to a multiple of 2^levels; tile origins are
//! multiples of 2^levels so every pyramid level of a tile lines up with the canvas pyramid.

use rayon::prelude::*;

/// A planar-interleaved float buffer with `c` channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Buf {
    pub w: usize,
    pub h: usize,
    pub c: usize,
    pub data: Vec<f32>,
}

impl Buf {
    pub fn new(w: usize, h: usize, c: usize) -> Buf {
        Buf { w, h, c, data: vec![0.0; w * h * c] }
    }
    #[inline]
    fn at(&self, x: isize, y: isize, ch: usize) -> f32 {
        let x = x.clamp(0, self.w as isize - 1) as usize;
        let y = y.clamp(0, self.h as isize - 1) as usize;
        self.data[(y * self.w + x) * self.c + ch]
    }
}

const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];

/// Binomial 5-tap blur then decimate by 2 (output `⌈w/2⌉ × ⌈h/2⌉`).
pub fn down(b: &Buf) -> Buf {
    let (w2, h2) = (b.w.div_ceil(2), b.h.div_ceil(2));
    // horizontal pass at decimated x
    let mut tmp = Buf::new(w2, b.h, b.c);
    tmp.data.par_chunks_mut(w2 * b.c).enumerate().for_each(|(y, row)| {
        for x in 0..w2 {
            for ch in 0..b.c {
                let mut s = 0.0;
                for (k, wk) in K.iter().enumerate() {
                    s += wk * b.at(2 * x as isize + k as isize - 2, y as isize, ch);
                }
                row[x * b.c + ch] = s;
            }
        }
    });
    let mut out = Buf::new(w2, h2, b.c);
    out.data.par_chunks_mut(w2 * b.c).enumerate().for_each(|(y, row)| {
        for x in 0..w2 {
            for ch in 0..b.c {
                let mut s = 0.0;
                for (k, wk) in K.iter().enumerate() {
                    s += wk * tmp.at(x as isize, 2 * y as isize + k as isize - 2, ch);
                }
                row[x * b.c + ch] = s;
            }
        }
    });
    out
}

/// Expand to `w × h` (bilinear, pixel-centre aligned with [`down`]'s grid).
pub fn up(b: &Buf, w: usize, h: usize) -> Buf {
    let mut out = Buf::new(w, h, b.c);
    out.data.par_chunks_mut(w * b.c).enumerate().for_each(|(y, row)| {
        let fy = (y as f32 + 0.5) / 2.0 - 0.5;
        let y0 = fy.floor();
        let ty = fy - y0;
        let y0 = y0 as isize;
        for x in 0..w {
            let fx = (x as f32 + 0.5) / 2.0 - 0.5;
            let x0 = fx.floor();
            let tx = fx - x0;
            let x0 = x0 as isize;
            for ch in 0..b.c {
                let a = b.at(x0, y0, ch);
                let bb = b.at(x0 + 1, y0, ch);
                let c = b.at(x0, y0 + 1, ch);
                let d = b.at(x0 + 1, y0 + 1, ch);
                let top = a + (bb - a) * tx;
                let bot = c + (d - c) * tx;
                row[x * b.c + ch] = top + (bot - top) * ty;
            }
        }
    });
    out
}

/// Fill pixels with weight 0 from their neighbourhood (push–pull: weighted pyramid down, fill
/// from coarser levels up). `vals` has `c` channels; `wgt` one.
pub fn push_pull(vals: &mut Buf, wgt: &[f32]) {
    if wgt.iter().all(|&w| w > 0.0) || !wgt.iter().any(|&w| w > 0.0) {
        return;
    }
    let (w, h, c) = (vals.w, vals.h, vals.c);
    // premultiplied values + weight as the last channel
    let mut pm = Buf::new(w, h, c + 1);
    for i in 0..w * h {
        let a = wgt[i].clamp(0.0, 1.0);
        for ch in 0..c {
            pm.data[i * (c + 1) + ch] = vals.data[i * c + ch] * a;
        }
        pm.data[i * (c + 1) + c] = a;
    }
    // the finer levels, and the coarsest (1 × 1) one
    let (mut levels, mut l) = (Vec::new(), pm);
    while l.w > 1 || l.h > 1 {
        let d = down(&l);
        levels.push(std::mem::replace(&mut l, d));
    }
    // normalise the coarsest, then fill each finer level's holes from the coarser one
    let mut filled = {
        let mut f = Buf::new(l.w, l.h, c);
        for i in 0..l.w * l.h {
            let a = l.data[i * (c + 1) + c];
            for ch in 0..c {
                f.data[i * c + ch] = if a > 1e-12 { l.data[i * (c + 1) + ch] / a } else { 0.0 };
            }
        }
        f
    };
    for l in levels.iter().rev() {
        let coarse = up(&filled, l.w, l.h);
        let mut f = Buf::new(l.w, l.h, c);
        for i in 0..l.w * l.h {
            let a = l.data[i * (c + 1) + c].min(1.0);
            for ch in 0..c {
                let v = if a > 1e-12 { l.data[i * (c + 1) + ch] / l.data[i * (c + 1) + c] } else { 0.0 };
                f.data[i * c + ch] = v * a + coarse.data[i * c + ch] * (1.0 - a);
            }
        }
        filled = f;
    }
    for i in 0..w * h {
        if wgt[i] <= 0.0 {
            for ch in 0..c {
                vals.data[i * c + ch] = filled.data[i * c + ch];
            }
        }
    }
}

/// Accumulates tiles into the canvas Laplacian pyramid.
pub struct Blender {
    pub levels: usize,
    pub w: usize,
    pub h: usize,
    acc: Vec<Buf>,
    wsum: Vec<Buf>,
}

impl Blender {
    /// A blender for a `w × h` canvas (both multiples of 2^levels).
    pub fn new(w: usize, h: usize, levels: usize) -> Blender {
        let mut acc = Vec::new();
        let mut wsum = Vec::new();
        for l in 0..=levels {
            acc.push(Buf::new(w >> l, h >> l, 3));
            wsum.push(Buf::new(w >> l, h >> l, 1));
        }
        Blender { levels, w, h, acc, wsum }
    }

    /// Add a tile at canvas offset (`x0`, `y0`) (multiples of 2^levels; tile size too): `vals`
    /// (3 channels, holes already filled) and its seam mask (0..1).
    pub fn add(&mut self, x0: usize, y0: usize, vals: Buf, mask: Buf) {
        let mut g = vals;
        let mut m = mask;
        for l in 0..=self.levels {
            let (ox, oy) = (x0 >> l, y0 >> l);
            let (gn, mn) = if l < self.levels { (Some(down(&g)), Some(down(&m))) } else { (None, None) };
            let lap = match &gn {
                Some(gn) => {
                    let u = up(gn, g.w, g.h);
                    let mut lap = g.clone();
                    for (a, b) in lap.data.iter_mut().zip(&u.data) {
                        *a -= b;
                    }
                    lap
                }
                None => g.clone(),
            };
            let (acc, ws) = (&mut self.acc[l], &mut self.wsum[l]);
            let aw = acc.w;
            let tw = lap.w;
            acc.data.par_chunks_mut(aw * 3).zip(ws.data.par_chunks_mut(aw)).enumerate().for_each(|(y, (arow, wrow))| {
                if y < oy || y >= oy + lap.h {
                    return;
                }
                let ty = y - oy;
                for tx in 0..tw {
                    let x = ox + tx;
                    if x >= aw {
                        break;
                    }
                    let mk = m.data[ty * tw + tx];
                    if mk <= 0.0 {
                        continue;
                    }
                    for ch in 0..3 {
                        arow[x * 3 + ch] += lap.data[(ty * tw + tx) * 3 + ch] * mk;
                    }
                    wrow[x] += mk;
                }
            });
            if let (Some(gn), Some(mn)) = (gn, mn) {
                g = gn;
                m = mn;
            }
        }
    }

    /// Normalise and collapse the pyramid into the canvas.
    pub fn finish(self) -> Buf {
        let mut res: Option<Buf> = None;
        for l in (0..=self.levels).rev() {
            let mut b = self.acc[l].clone();
            let ws = &self.wsum[l];
            for i in 0..b.w * b.h {
                let s = ws.data[i];
                for ch in 0..3 {
                    b.data[i * 3 + ch] = if s > 1e-8 { b.data[i * 3 + ch] / s } else { 0.0 };
                }
            }
            if let Some(r) = res {
                let u = up(&r, b.w, b.h);
                for (a, v) in b.data.iter_mut().zip(&u.data) {
                    *a += v;
                }
            }
            res = Some(b);
        }
        // `0..=levels` always yields level 0
        res.unwrap_or_else(|| Buf::new(self.w, self.h, 3))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tile_reconstructs_exactly() {
        let (w, h) = (64, 48);
        let mut v = Buf::new(w, h, 3);
        for (i, x) in v.data.iter_mut().enumerate() {
            *x = ((i * 37) % 101) as f32 / 101.0;
        }
        let mut m = Buf::new(w, h, 1);
        m.data.fill(1.0);
        let mut b = Blender::new(w, h, 3);
        b.add(0, 0, v.clone(), m);
        let out = b.finish();
        for (a, b) in v.data.iter().zip(&out.data) {
            assert!((a - b).abs() < 1e-4);
        }
    }

    #[test]
    fn two_tiles_blend_without_a_step() {
        // left tile value 0, right tile value 1, seam in the middle: the result is a smooth ramp
        let (w, h) = (128, 32);
        let mut b = Blender::new(w, h, 4);
        for (val, side) in [(0.0f32, 0), (1.0, 1)] {
            let mut v = Buf::new(w, h, 3);
            v.data.fill(val);
            let mut m = Buf::new(w, h, 1);
            for y in 0..h {
                for x in 0..w {
                    m.data[y * w + x] = if (x >= w / 2) == (side == 1) { 1.0 } else { 0.0 };
                }
            }
            b.add(0, 0, v, m);
        }
        let out = b.finish();
        let row: Vec<f32> = (0..w).map(|x| out.data[(16 * w + x) * 3]).collect();
        let max_step = row.windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0, f32::max);
        assert!(max_step < 0.15, "max step {max_step}");
        assert!(row[2].abs() < 0.02 && (row[w - 3] - 1.0).abs() < 0.02);
    }

    #[test]
    fn push_pull_fills_holes() {
        let (w, h) = (20, 10);
        let mut v = Buf::new(w, h, 1);
        let mut wg = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w / 2 {
                v.data[y * w + x] = 0.7;
                wg[y * w + x] = 1.0;
            }
        }
        push_pull(&mut v, &wg);
        assert!(v.data.iter().all(|x| (x - 0.7).abs() < 1e-4));
    }
}
