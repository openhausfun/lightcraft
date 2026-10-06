//! Bilinear demosaicing for any CFA pattern: each missing colour is the mean of the same-colour samples in the
//! 3×3 neighbourhood (5×5 when the 3×3 has none, as happens in some non-Bayer layouts).

use super::Mosaic;
use crate::Rgb32f;
use lightcraft_raster::par_rows;

/// Offsets (dx, dy) of same-colour neighbours for each pattern position and colour.
pub(crate) fn neighbour_table(m: &Mosaic) -> Vec<[Vec<(isize, isize)>; 3]> {
    let cfa = m.cfa;
    let mut table = Vec::with_capacity(cfa.width * cfa.height);
    for py in 0..cfa.height {
        for px in 0..cfa.width {
            let mut per: [Vec<(isize, isize)>; 3] = Default::default();
            for (c, list) in per.iter_mut().enumerate() {
                for r in [1isize, 2] {
                    for dy in -r..=r {
                        for dx in -r..=r {
                            let (x, y) = (px as isize + dx + 6 * cfa.width as isize, py as isize + dy + 6 * cfa.height as isize);
                            if cfa.color_at(x as usize, y as usize) as usize == c {
                                list.push((dx, dy));
                            }
                        }
                    }
                    if !list.is_empty() {
                        break;
                    }
                }
            }
            table.push(per);
        }
    }
    table
}

pub(crate) fn bilinear(m: &Mosaic) -> Rgb32f {
    let (w, h) = (m.w, m.h);
    let table = neighbour_table(m);
    let cfa = m.cfa;
    let mut out = Rgb32f::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let own = cfa.color_at(x, y) as usize;
            let interior = x >= 2 && y >= 2 && x + 2 < w && y + 2 < h;
            let entry = &table[(y % cfa.height) * cfa.width + (x % cfa.width)];
            for c in 0..3 {
                if c == own {
                    px[c] = m.data[y * w + x];
                    continue;
                }
                let (mut s, mut n) = (0.0f32, 0u32);
                if interior {
                    for &(dx, dy) in &entry[c] {
                        s += m.data[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
                        n += 1;
                    }
                } else {
                    for r in [1isize, 2] {
                        for dy in -r..=r {
                            for dx in -r..=r {
                                let (nx, ny) = (x as isize + dx, y as isize + dy);
                                if m.color(nx, ny) as usize == c {
                                    s += m.get(nx, ny);
                                    n += 1;
                                }
                            }
                        }
                        if n > 0 {
                            break;
                        }
                    }
                }
                px[c] = if n > 0 { s / n as f32 } else { m.data[y * w + x] };
            }
        }
    });
    out
}
