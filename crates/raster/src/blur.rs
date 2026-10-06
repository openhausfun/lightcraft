//! Gaussian-like blur by three successive box filters (running sums, O(1) per pixel per pass).

use crate::resample::Pixel;
use crate::{Image, par_rows};

/// Box radii approximating a Gaussian of `sigma` with 3 passes (Kovesi / Wells).
pub fn box_radii(sigma: f32) -> [usize; 3] {
    let n = 3.0f32;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let m_ideal = (12.0 * sigma * sigma - n * (wl * wl) as f32 - 4.0 * n * wl as f32 - 3.0 * n) / (-4.0 * wl as f32 - 4.0);
    let m = m_ideal.round() as i32;
    std::array::from_fn(|i| (((if (i as i32) < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

/// One horizontal box pass over `row` (clamped edges), using `tmp` as scratch.
#[inline]
fn box_row<T: Pixel>(row: &mut [T], tmp: &mut Vec<T>, r: usize) {
    let w = row.len();
    if r == 0 || w == 0 {
        return;
    }
    tmp.clear();
    tmp.extend_from_slice(row);
    let s = &tmp[..];
    let last = w - 1;
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut acc = T::zero();
    for i in 0..=2 * r {
        acc = acc.madd(s[i.saturating_sub(r).min(last)], 1.0);
    }
    for x in 0..w {
        row[x] = T::zero().madd(acc, inv);
        acc = acc.madd(s[(x + r + 1).min(last)], 1.0).madd(s[x.saturating_sub(r)], -1.0);
    }
}

/// Rows per parallel band in the vertical pass (each band re-primes its running sums).
pub const BAND: usize = 32;

/// One vertical box pass `src → dst` (clamped edges): a running sum per column, vectorized across
/// the row, parallel over horizontal bands. Cache-friendly — no transposes.
fn box_v<T: Pixel>(src: &Image<T>, dst: &mut Image<T>, r: usize) {
    let (w, h) = (src.width, src.height);
    let inv = 1.0 / (2 * r + 1) as f32;
    let last = h - 1;
    let row = |y: usize| &src.data[y * w..(y + 1) * w];
    par_rows(&mut dst.data, w * BAND, |band, out| {
        let y0 = band * BAND;
        let mut acc = vec![T::zero(); w];
        for i in 0..=2 * r {
            let yy = (y0 + i).saturating_sub(r).min(last);
            for (a, v) in acc.iter_mut().zip(row(yy)) {
                *a = a.madd(*v, 1.0);
            }
        }
        for (k, o) in out.chunks_mut(w).enumerate() {
            let y = y0 + k;
            for (d, a) in o.iter_mut().zip(&acc) {
                *d = T::zero().madd(*a, inv);
            }
            let (add, sub) = (row((y + r + 1).min(last)), row(y.saturating_sub(r)));
            for ((a, p), m) in acc.iter_mut().zip(add).zip(sub) {
                *a = a.madd(*p, 1.0).madd(*m, -1.0);
            }
        }
    });
}

/// Blur with Gaussian `sigma` (pixels). `sigma <= 0.3` returns a copy.
pub fn gaussian<T: Pixel>(img: &Image<T>, sigma: f32) -> Image<T> {
    if sigma <= 0.3 || img.is_empty() {
        return img.clone();
    }
    let radii = box_radii(sigma);
    let mut a = img.clone();
    par_rows(&mut a.data, img.width, |_, row| {
        let mut tmp = Vec::with_capacity(row.len());
        for r in radii {
            box_row(row, &mut tmp, r);
        }
    });
    let mut b = Image::<T>::new(img.width, img.height);
    for r in radii {
        if r == 0 {
            continue;
        }
        box_v(&a, &mut b, r);
        std::mem::swap(&mut a, &mut b);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_constant_and_mass() {
        let img = Image::<f32>::filled(50, 30, 0.7);
        let b = gaussian(&img, 4.0);
        assert!(b.data.iter().all(|v| (v - 0.7).abs() < 1e-5));
        let mut imp = Image::<f32>::new(101, 101);
        imp.set(50, 50, 1.0);
        let b = gaussian(&imp, 5.0);
        let sum: f32 = b.data.iter().sum();
        assert!((sum - 1.0).abs() < 1e-3, "{sum}");
        // peak moved to the centre and symmetric
        assert!((b.get(45, 50) - b.get(55, 50)).abs() < 1e-6);
        assert!((b.get(50, 45) - b.get(50, 55)).abs() < 1e-6);
    }

    #[test]
    fn variance_close_to_sigma_squared() {
        let mut imp = Image::<f32>::new(201, 1);
        imp.set(100, 0, 1.0);
        let sigma = 8.0;
        let b = gaussian(&imp, sigma);
        let var: f32 = b.data.iter().enumerate().map(|(i, v)| v * ((i as f32 - 100.0).powi(2))).sum();
        assert!((var.sqrt() - sigma).abs() / sigma < 0.1, "{}", var.sqrt());
    }
}
