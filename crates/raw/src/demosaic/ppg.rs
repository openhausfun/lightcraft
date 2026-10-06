//! Patterned Pixel Grouping (after Chuan-kai Lin's published description of the method).
//!
//! 1. Green at red/blue sites: four directional gradients (same-colour difference ×2 plus the green difference
//!    across the site); the estimate along the flattest direction is the nearer green weighted 3:1 with the far
//!    one plus a same-colour Laplacian correction, clamped to the two greens of that direction.
//! 2. Red/blue at green sites: colour-difference (C − G) average of the two same-colour neighbours.
//! 3. Blue at red sites (and red at blue): colour-difference average along the diagonal with the smaller
//!    gradient (both diagonals when equal).

use super::Mosaic;
use crate::Rgb32f;
use lightcraft_raster::par_rows;

/// Green plane with PPG estimates at non-green sites.
pub(crate) fn green_plane(m: &Mosaic) -> Vec<f32> {
    let (w, h) = (m.w, m.h);
    let mut g = vec![0f32; w * h];
    par_rows(&mut g, w, |y, row| {
        let y = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as isize;
            let c = m.get(x, y);
            if m.color(x, y) == 1 {
                *o = c;
                continue;
            }
            let (gn, gs, gw, ge) = (m.get(x, y - 1), m.get(x, y + 1), m.get(x - 1, y), m.get(x + 1, y));
            let (cn, cs, cw, ce) = (m.get(x, y - 2), m.get(x, y + 2), m.get(x - 2, y), m.get(x + 2, y));
            let dv = (gn - gs).abs();
            let dh = (gw - ge).abs();
            let grads = [(cn - c).abs() * 2.0 + dv, (cs - c).abs() * 2.0 + dv, (cw - c).abs() * 2.0 + dh, (ce - c).abs() * 2.0 + dh];
            let mut best = 0;
            for i in 1..4 {
                if grads[i] < grads[best] {
                    best = i;
                }
            }
            let (near, far, cfar) = match best {
                0 => (gn, gs, cn),
                1 => (gs, gn, cs),
                2 => (gw, ge, cw),
                _ => (ge, gw, ce),
            };
            let est = (near * 3.0 + far + c - cfar) * 0.25;
            *o = est.clamp(near.min(far), near.max(far));
        }
    });
    g
}

pub(crate) fn ppg(m: &Mosaic) -> Rgb32f {
    let (w, h) = (m.w, m.h);
    let g = green_plane(m);
    let gg = |x: isize, y: isize| g[super::reflect(y, h) * w + super::reflect(x, w)];
    let mut out = Rgb32f::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        let y = y as isize;
        for (x, px) in row.iter_mut().enumerate() {
            let x = x as isize;
            let own = m.color(x, y) as usize;
            let gv = gg(x, y);
            px[1] = gv;
            if own == 1 {
                // horizontal neighbours carry one colour, vertical the other
                let ch = m.color(x + 1, y) as usize;
                let cv = m.color(x, y + 1) as usize;
                let hval = gv + ((m.get(x - 1, y) - gg(x - 1, y)) + (m.get(x + 1, y) - gg(x + 1, y))) * 0.5;
                let vval = gv + ((m.get(x, y - 1) - gg(x, y - 1)) + (m.get(x, y + 1) - gg(x, y + 1))) * 0.5;
                px[ch] = hval;
                px[cv] = vval;
            } else {
                px[own] = m.get(x, y);
                let other = 2 - own;
                let d = |dx: isize, dy: isize| m.get(x + dx, y + dy) - gg(x + dx, y + dy);
                let ne = (m.get(x + 1, y - 1) - m.get(x - 1, y + 1)).abs() + (gg(x + 1, y - 1) - gv).abs() + (gg(x - 1, y + 1) - gv).abs();
                let nw = (m.get(x - 1, y - 1) - m.get(x + 1, y + 1)).abs() + (gg(x - 1, y - 1) - gv).abs() + (gg(x + 1, y + 1) - gv).abs();
                let diff = if ne < nw {
                    (d(1, -1) + d(-1, 1)) * 0.5
                } else if nw < ne {
                    (d(-1, -1) + d(1, 1)) * 0.5
                } else {
                    (d(1, -1) + d(-1, 1) + d(-1, -1) + d(1, 1)) * 0.25
                };
                px[other] = gv + diff;
            }
        }
    });
    out
}
