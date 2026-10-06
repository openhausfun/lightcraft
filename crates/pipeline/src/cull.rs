//! Assisted culling measurements on a decoded source (any size; measured on a fixed-size copy so
//! scores compare across photos): focus, clipping, and a tiny signature for finding similar
//! shots (bursts). Classical image statistics — no learned models.

use lightcraft_raster::Rgb32f;

/// Long edge the measurements run at.
const EDGE: usize = 512;

/// Display-ish luminance (sRGB-encoded) of `img` resampled to fit `EDGE` (box filter).
fn luma(img: &Rgb32f) -> (Vec<f32>, usize, usize) {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let s = (EDGE as f64 / w.max(h) as f64).min(1.0);
    let (ow, oh) = (((w as f64 * s).round() as usize).max(8), ((h as f64 * s).round() as usize).max(8));
    let mut out = vec![0f32; ow * oh];
    for (y, row) in out.chunks_mut(ow).enumerate() {
        let (y0, y1) = (y * h / oh, ((y + 1) * h / oh).max(y * h / oh + 1).min(h));
        for (x, v) in row.iter_mut().enumerate() {
            let (x0, x1) = (x * w / ow, ((x + 1) * w / ow).max(x * w / ow + 1).min(w));
            let mut acc = 0.0f32;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let c = img.data[yy * w + xx];
                    acc += 0.2627 * c[0] + 0.678 * c[1] + 0.0593 * c[2];
                }
            }
            let l = acc / ((y1 - y0) * (x1 - x0)) as f32;
            *v = lightcraft_color::transfer::linear_to_srgb(l.clamp(0.0, 1.0));
        }
    }
    (out, ow, oh)
}

/// Focus 0..100: detail energy (Laplacian) of the sharpest areas — the 90th percentile of
/// 16 × 16-pixel blocks, so a sharp subject on a soft background still reads as sharp.
pub fn sharpness(img: &Rgb32f) -> f32 {
    let (l, w, h) = luma(img);
    let b = 16;
    let mut blocks = Vec::new();
    for by in (1..h.saturating_sub(1)).step_by(b) {
        for bx in (1..w.saturating_sub(1)).step_by(b) {
            let mut e = 0.0f32;
            let mut n = 0;
            for y in by..(by + b).min(h - 1) {
                for x in bx..(bx + b).min(w - 1) {
                    let c = l[y * w + x];
                    let lap = 4.0 * c - l[y * w + x - 1] - l[y * w + x + 1] - l[(y - 1) * w + x] - l[(y + 1) * w + x];
                    e += lap * lap;
                    n += 1;
                }
            }
            if n > 0 {
                blocks.push(e / n as f32);
            }
        }
    }
    if blocks.is_empty() {
        return 0.0;
    }
    let k = (blocks.len() as f32 * 0.9) as usize;
    let k = k.min(blocks.len() - 1);
    let (_, v, _) = blocks.select_nth_unstable_by(k, f32::total_cmp);
    // map energy to 0..100: a crisp, ordinary photo at 512 px reads ≈ 50–80 (calibrated on the
    // CC0 corpus), soft ones well below
    100.0 * (1.0 - (-*v / 0.012).exp())
}

/// Share of clipped pixels (crushed blacks + blown highlights), 0..1.
pub fn clipped(img: &Rgb32f) -> f32 {
    let n = img.data.len().max(1);
    let c = img.data.iter().filter(|p| p.iter().any(|v| *v >= 0.985) || p.iter().all(|v| *v <= 0.002)).count();
    c as f32 / n as f32
}

/// A tiny look-alike signature: 8 × 8 luminance, normalised to zero mean and unit norm.
pub fn signature(img: &Rgb32f) -> [f32; 64] {
    let (l, w, h) = luma(img);
    let mut s = [0f32; 64];
    for (i, v) in s.iter_mut().enumerate() {
        let (cx, cy) = (i % 8, i / 8);
        let (x0, x1) = (cx * w / 8, (cx + 1) * w / 8);
        let (y0, y1) = (cy * h / 8, (cy + 1) * h / 8);
        let mut acc = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                acc += l[y * w + x];
            }
        }
        *v = acc / ((x1 - x0) * (y1 - y0)).max(1) as f32;
    }
    let mean = s.iter().sum::<f32>() / 64.0;
    s.iter_mut().for_each(|v| *v -= mean);
    let norm = s.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
    s.iter_mut().for_each(|v| *v /= norm);
    s
}

/// Similarity of two signatures: 1 = the same picture, 0 = unrelated.
pub fn similarity(a: &[f32; 64], b: &[f32; 64]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>().clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blur(img: &Rgb32f, r: usize) -> Rgb32f {
        let (w, h) = (img.width, img.height);
        let mut out = img.clone();
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0f32; 3];
                let mut n = 0.0;
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        let c = img.data[yy * w + xx];
                        acc = [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]];
                        n += 1.0;
                    }
                }
                out.data[y * w + x] = acc.map(|v| v / n);
            }
        }
        out
    }

    #[test]
    fn blur_lowers_focus_and_signatures_match() {
        let img = lightcraft_scenes::demo_library()[0].render(480, 320);
        let soft = blur(&img, 4);
        let (a, b) = (sharpness(&img), sharpness(&soft));
        assert!(a > b + 10.0, "sharp {a} vs blurred {b}");
        assert!(similarity(&signature(&img), &signature(&soft)) > 0.95, "the same picture");
        let other = lightcraft_scenes::demo_library()[5].render(480, 320);
        assert!(similarity(&signature(&img), &signature(&other)) < 0.9);
        assert!(clipped(&img) < 0.5);
    }
}
