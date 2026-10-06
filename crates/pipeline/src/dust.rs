//! Dust-spot detection: sensor dust shows as small, soft, darker-than-surroundings round spots
//! on smooth areas (sky, walls). Classical: a blurred copy gives the surroundings, the darkening
//! is compared with the local texture so detail doesn't trigger it, and each candidate blob
//! must be small and roughly round.

use lightcraft_raster::{Plane, Rgba8, blur::gaussian};

/// A found spot: centre (0..1 of the image's width / height) and radius (fraction of the long
/// edge), with how much darker it is (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DustSpot {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    pub strength: f32,
}

/// Dust spots in a rendered image; `sensitivity` 0..100 (higher finds fainter spots).
pub fn detect(img: &Rgba8, sensitivity: f32) -> Vec<DustSpot> {
    let (w, h) = (img.width, img.height);
    if w < 32 || h < 32 {
        return Vec::new();
    }
    let long = w.max(h) as f32;
    let l = Plane::from_fn(w, h, |x, y| {
        let p = img.data[y * w + x];
        (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
    });
    let around = gaussian(&l, (0.012 * long).max(2.0));
    // texture: local mean of the fine detail (what isn't explained by the surroundings)
    let detail = Plane::from_fn(w, h, |x, y| (l.data[y * w + x] - around.data[y * w + x]).abs());
    let texture = gaussian(&detail, (0.02 * long).max(3.0));
    let min_dark = 0.03 - 0.022 * (sensitivity / 100.0).clamp(0.0, 1.0);
    let dark: Vec<f32> = (0..w * h).map(|i| (around.data[i] - l.data[i]).max(0.0)).collect();
    let candidate = |i: usize| dark[i] > min_dark && dark[i] > 3.0 * texture.data[i] && l.data[i] > 0.08;
    // blobs of candidate pixels (4-connected)
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let (rmin, rmax) = (0.0015 * long, 0.02 * long);
    for start in 0..w * h {
        if seen[start] || !candidate(start) {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let (mut n, mut sx, mut sy, mut peak) = (0usize, 0.0f64, 0.0f64, 0.0f32);
        let (mut x0, mut x1, mut y0, mut y1) = (w, 0, h, 0);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            n += 1;
            sx += x as f64;
            sy += y as f64;
            peak = peak.max(dark[i]);
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            if n > (rmax * rmax * 4.0) as usize {
                break; // too big to be dust: stop growing
            }
            for j in [i.wrapping_sub(1), i + 1, i.wrapping_sub(w), i + w] {
                let ok = j < w * h && !seen[j] && ((j % w).abs_diff(x) <= 1);
                if ok && candidate(j) {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        let r = (bw.max(bh)) / 2.0;
        let round = bw.max(bh) / bw.min(bh) <= 1.8 && n as f32 >= 0.45 * bw * bh;
        if r >= rmin && r <= rmax && round {
            out.push(DustSpot {
                x: (sx / n as f64 + 0.5) / w as f64,
                y: (sy / n as f64 + 0.5) / h as f64,
                radius: (r / long) as f64 * 1.6,
                strength: peak,
            });
        }
    }
    out.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    out.truncate(60);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A smooth sky gradient with three soft dark spots: all found, nothing else.
    #[test]
    fn finds_spots_on_smooth_areas() {
        let (w, h) = (600usize, 400usize);
        let spots = [(120.0f32, 90.0f32, 4.0f32), (400.0, 200.0, 6.0), (500.0, 330.0, 3.0)];
        let data: Vec<[u8; 4]> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let mut v = 0.55 + 0.25 * (y / h as f32) + 0.05 * (x / w as f32);
                for (cx, cy, r) in spots {
                    let d2 = (x - cx).powi(2) + (y - cy).powi(2);
                    v -= 0.06 * (-d2 / (2.0 * r * r)).exp();
                }
                let b = (v * 255.0) as u8;
                [b, b, (b as f32 * 1.1).min(255.0) as u8, 255]
            })
            .collect();
        let found = detect(&Rgba8 { width: w, height: h, data }, 50.0);
        assert_eq!(found.len(), 3, "{found:?}");
        for (cx, cy, _) in spots {
            assert!(
                found.iter().any(|s| (s.x * w as f64 - cx as f64).abs() < 3.0 && (s.y * h as f64 - cy as f64).abs() < 3.0),
                "{cx},{cy}: {found:?}"
            );
        }
    }

    #[test]
    fn textured_scenes_give_few_false_spots() {
        let img = lightcraft_scenes::demo_library()[2].render(600, 400);
        let data = img
            .data
            .iter()
            .map(|c| {
                let e = |v: f32| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0) as u8;
                [e(c[0]), e(c[1]), e(c[2]), 255]
            })
            .collect();
        let found = detect(&Rgba8 { width: 600, height: 400, data }, 50.0);
        assert!(found.len() <= 3, "{} false spots", found.len());
    }
}
