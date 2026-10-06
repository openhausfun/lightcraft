//! Red eye and pet eye correction, in scene-linear light after white balance.
//!
//! The user draws an ellipse over the eye; inside it the pupil is found automatically on the
//! source image (resolution independent: a fixed grid of samples in normalized coordinates):
//!
//! - **red eye:** red-dominant pixels (`(R − max(G, B)) / (R + G + B)`) — flash light reflected by
//!   the retina;
//! - **pet eye:** the brightest pixels relative to the ellipse (the tapetum's glow, any colour).
//!
//! Their weighted centroid and area give the pupil disc, scaled by Pupil Size. Rendering (shared
//! by the CPU and the GPU kernel `redeye_k`): red eye neutralizes the red inside the disc (only
//! where it is red, so iris and catchlights stay) and darkens it; pet eye replaces the disc with a
//! dark pupil and can add a catchlight.

use lightcraft_develop::RedEye;
use lightcraft_geom::{Orientation, Point};
use lightcraft_raster::Rgb32f;

use crate::for_rows;
use crate::geometry::Frame;

/// `f32`s per eye in [`EyeK::words`] (the GPU parameter layout).
pub const EYE_WORDS: usize = 9;

/// Samples per axis of the detection grid over the ellipse's bounding box.
const GRID: usize = 40;

/// One eye resolved for rendering, in output pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeK {
    pub cx: f32,
    pub cy: f32,
    /// Pupil radius.
    pub r: f32,
    /// Darken 0..1.
    pub darken: f32,
    pub pet: bool,
    /// Catchlight disc (centre, radius), pet eye only.
    pub catchlight: Option<(f32, f32, f32)>,
}

impl EyeK {
    pub fn words(&self) -> [f32; EYE_WORDS] {
        let (lx, ly, lr) = self.catchlight.unwrap_or((0.0, 0.0, 0.0));
        [self.cx, self.cy, self.r, self.darken, self.pet as u8 as f32, self.catchlight.is_some() as u8 as f32, lx, ly, lr]
    }
}

/// A detected pupil: centre (normalized coordinates) and radius (long-edge units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pupil {
    pub center: Point,
    pub radius: f64,
    /// Whether anything was found (else: the ellipse centre and a default radius).
    pub found: bool,
}

/// Normalized point of the user-oriented image → normalized point of the source (inverse of
/// `Rgb32f::oriented`: undo the quarter turns, then the flip).
pub fn oriented_to_source(o: Orientation, p: Point) -> Point {
    let (flip, turns) = o.to_parts();
    let (mut u, mut v) = (p.x, p.y);
    for _ in 0..turns {
        // a clockwise turn maps source (x, y) to (1 − y, x)
        (u, v) = (v, 1.0 - u);
    }
    if flip {
        u = 1.0 - u;
    }
    Point::new(u, v)
}

#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Find the pupil inside `eye`'s ellipse on `src` (the EXIF-oriented source; `o` = the user
/// orientation, `ow × oh` = the oriented image size in any unit).
pub fn detect(src: &Rgb32f, o: Orientation, ow: f64, oh: f64, eye: &RedEye) -> Pupil {
    let long = ow.max(oh);
    let (rx, ry) = (eye.rx.max(1e-4), eye.ry.max(1e-4));
    let fallback = Pupil { center: eye.center, radius: 0.5 * rx.min(ry), found: false };
    if src.width == 0 || src.height == 0 {
        return fallback;
    }
    // samples inside the ellipse: (normalized point, colour)
    let mut samples = Vec::with_capacity(GRID * GRID);
    for j in 0..GRID {
        for i in 0..GRID {
            let (dx, dy) = ((i as f64 + 0.5) / GRID as f64 * 2.0 - 1.0, (j as f64 + 0.5) / GRID as f64 * 2.0 - 1.0);
            if dx * dx + dy * dy > 1.0 {
                continue;
            }
            let n = Point::new(eye.center.x + dx * rx * long / ow, eye.center.y + dy * ry * long / oh);
            if !(0.0..1.0).contains(&n.x) || !(0.0..1.0).contains(&n.y) {
                continue;
            }
            let s = oriented_to_source(o, n);
            let (px, py) = ((s.x * src.width as f64) as usize, (s.y * src.height as f64) as usize);
            samples.push((n, src.data[py.min(src.height - 1) * src.width + px.min(src.width - 1)]));
        }
    }
    if samples.is_empty() {
        return fallback;
    }
    let lum = |c: [f32; 3]| lightcraft_color::luminance_2020(c);
    let weights: Vec<f32> = if eye.pet {
        let ymax = samples.iter().map(|(_, c)| lum(*c)).fold(0.0f32, f32::max);
        let mean = samples.iter().map(|(_, c)| lum(*c)).sum::<f32>() / samples.len() as f32;
        if ymax <= 1e-6 || ymax < 1.5 * mean {
            return fallback;
        }
        samples.iter().map(|(_, c)| smooth(0.5, 0.8, lum(*c) / ymax)).collect()
    } else {
        samples
            .iter()
            .map(|(_, c)| {
                let s = (c[0] - c[1].max(c[2])) / (c[0] + c[1] + c[2] + 1e-6);
                if c[0] > 1e-3 { smooth(0.22, 0.42, s) } else { 0.0 }
            })
            .collect()
    };
    let total: f32 = weights.iter().sum();
    if total < 3.0 {
        return fallback;
    }
    let (mut x, mut y) = (0.0, 0.0);
    for ((n, _), w) in samples.iter().zip(&weights) {
        x += n.x * *w as f64;
        y += n.y * *w as f64;
    }
    let frac = total as f64 / samples.len() as f64;
    Pupil { center: Point::new(x / total as f64, y / total as f64), radius: (frac * rx * ry).sqrt(), found: true }
}

/// Resolve the eyes for a `w × h` render of `frame` (`ppl` = output pixels per long-edge unit).
pub fn resolve(src: &Rgb32f, eyes: &[RedEye], orientation: Orientation, frame: &Frame, w: usize, h: usize, ppl: f64) -> Vec<EyeK> {
    if eyes.is_empty() {
        return Vec::new();
    }
    let to_out = frame.norm_to_out(w, h);
    eyes.iter()
        .map(|e| {
            let p = detect(src, orientation, frame.ow, frame.oh, e);
            let r = (p.radius * (0.5 + e.pupil_size.clamp(0.0, 100.0) / 100.0)).min(e.rx.max(e.ry) * 1.1);
            let c = to_out.apply(p.center);
            let r_px = (r * ppl) as f32;
            let catchlight =
                e.catchlight.filter(|_| e.pet).map(|off| ((c.x + off.x * r * ppl) as f32, (c.y + off.y * r * ppl) as f32, (0.22 * r_px).max(0.5)));
            EyeK { cx: c.x as f32, cy: c.y as f32, r: r_px.max(0.5), darken: (e.darken.clamp(0.0, 100.0) / 100.0) as f32, pet: e.pet, catchlight }
        })
        .collect()
}

/// One pixel (pixel centre `(x, y)` in output pixels) through every eye.
#[inline]
pub fn eye_pixel(c: [f32; 3], x: f32, y: f32, eyes: &[EyeK]) -> [f32; 3] {
    let mut c = c;
    for e in eyes {
        let d = ((x - e.cx) * (x - e.cx) + (y - e.cy) * (y - e.cy)).sqrt() / e.r;
        // red: generous disc (the red gate protects the iris); pet: the disc itself is replaced
        let m = if e.pet { 1.0 - smooth(0.95, 1.2, d) } else { 1.0 - smooth(1.0, 1.35, d) };
        if m > 0.0 {
            if e.pet {
                let k = lightcraft_color::luminance_2020(c).min(0.04) * (1.0 - 0.85 * e.darken);
                c = c.map(|v| v + (k - v) * m);
            } else {
                let red = (c[0] - c[1].max(c[2])) / c[0].max(1e-6);
                let a = m * smooth(0.25, 0.5, red);
                let n = 0.5 * (c[1] + c[2]) * (1.0 - 0.9 * e.darken);
                c = c.map(|v| v + (n - v) * a);
            }
        }
        if let Some((lx, ly, lr)) = e.catchlight {
            let dc = ((x - lx) * (x - lx) + (y - ly) * (y - ly)).sqrt() / lr;
            let mc = 1.0 - smooth(0.5, 1.0, dc);
            if mc > 0.0 {
                c = c.map(|v| v + (0.9 - v) * mc);
            }
        }
    }
    c
}

/// Apply the eyes to `img` (output resolution), in place.
pub fn apply(img: &mut Rgb32f, eyes: &[EyeK]) {
    if eyes.is_empty() {
        return;
    }
    let w = img.width;
    for_rows(&mut img.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            *px = eye_pixel(*px, x as f32 + 0.5, y as f32 + 0.5, eyes);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orientation_inverse_matches_raster() {
        let img = Rgb32f::from_fn(5, 3, |x, y| [x as f32, y as f32, 0.0]);
        use Orientation::*;
        for o in [Normal, Rotate90, Rotate180, Rotate270, FlipH, Transverse, FlipV, Transpose] {
            let r = img.oriented(o);
            for y in 0..r.height {
                for x in 0..r.width {
                    let n = Point::new((x as f64 + 0.5) / r.width as f64, (y as f64 + 0.5) / r.height as f64);
                    let s = oriented_to_source(o, n);
                    let (sx, sy) = ((s.x * 5.0) as usize, (s.y * 3.0) as usize);
                    assert_eq!(r.get(x, y), img.get(sx, sy), "{o:?} at {x},{y}");
                }
            }
        }
    }

    /// A skin-coloured patch with a red pupil of radius `pr` at (cx, cy) (pixels).
    fn eye_image(w: usize, h: usize, cx: f32, cy: f32, pr: f32, pupil: [f32; 3]) -> Rgb32f {
        Rgb32f::from_fn(w, h, |x, y| {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d < pr {
                pupil
            } else if d < pr * 2.0 {
                [0.12, 0.1, 0.08] // iris
            } else {
                [0.45, 0.3, 0.22]
            }
        })
    }

    #[test]
    fn detects_the_red_pupil_off_centre() {
        let img = eye_image(200, 100, 90.0, 52.0, 6.0, [0.5, 0.04, 0.04]);
        // ellipse centred off the pupil (long edge 200: rx 0.1 = 20 px)
        let eye = RedEye { center: Point::new(0.48, 0.5), rx: 0.1, ry: 0.08, pupil_size: 50.0, darken: 50.0, pet: false, catchlight: None };
        let p = detect(&img, Orientation::Normal, 200.0, 100.0, &eye);
        assert!(p.found);
        assert!((p.center.x - 0.45).abs() < 0.01 && (p.center.y - 0.52).abs() < 0.02, "{p:?}");
        assert!((p.radius * 200.0 - 6.0).abs() < 1.5, "{}", p.radius * 200.0);
        // nothing red: fallback
        let plain = eye_image(200, 100, 90.0, 52.0, 6.0, [0.03, 0.03, 0.03]);
        assert!(!detect(&plain, Orientation::Normal, 200.0, 100.0, &eye).found);
    }

    #[test]
    fn renders_fix_the_red_pupil_at_any_size() {
        use crate::{RenderRequest, SourceInfo, render};
        let src = eye_image(400, 200, 180.0, 104.0, 12.0, [0.5, 0.04, 0.04]);
        let mut s = lightcraft_develop::DevelopSettings::default();
        s.red_eye.push(RedEye { center: Point::new(0.47, 0.5), rx: 0.1, ry: 0.08, ..Default::default() });
        let info = SourceInfo::default();
        for size in [400, 160] {
            let img = render(&src, &info, &s, &RenderRequest::fit(size, size)).image;
            let (x, y) = ((0.45 * img.width as f64) as usize, (0.52 * img.height as f64) as usize);
            let p = img.data[y * img.width + x];
            assert!(p[0] < 90 && p[0].abs_diff(p[1]) < 12, "size {size}: pupil {p:?}");
            // skin outside the ellipse is untouched
            let q = img.data[(img.height / 2) * img.width + img.width / 10];
            let plain = render(&src, &info, &Default::default(), &RenderRequest::fit(size, size)).image;
            assert_eq!(q, plain.data[(img.height / 2) * img.width + img.width / 10]);
        }
    }

    #[test]
    fn pet_eye_finds_the_glow_darkens_it_and_adds_a_catchlight() {
        use crate::{RenderRequest, SourceInfo, render};
        // a green-yellow tapetum glow (any colour: pet eyes are found by brightness)
        let src = eye_image(400, 200, 210.0, 96.0, 14.0, [0.5, 0.7, 0.3]);
        let eye = RedEye { center: Point::new(0.52, 0.5), rx: 0.1, ry: 0.09, pet: true, ..Default::default() };
        let p = detect(&src, Orientation::Normal, 400.0, 200.0, &eye);
        assert!(p.found && (p.center.x - 0.525).abs() < 0.01 && (p.center.y - 0.48).abs() < 0.02, "{p:?}");
        assert!((p.radius * 400.0 - 14.0).abs() < 3.0, "{}", p.radius * 400.0);
        // a red pupil is not a glow candidate for red-eye detection when it's green
        assert!(!detect(&src, Orientation::Normal, 400.0, 200.0, &RedEye { pet: false, ..eye }).found);
        let info = SourceInfo::default();
        let mut s = lightcraft_develop::DevelopSettings::default();
        s.red_eye.push(eye);
        let img = render(&src, &info, &s, &RenderRequest::fit(400, 400)).image;
        let at = |img: &lightcraft_raster::Rgba8, x: f64, y: f64| img.data[(y * 200.0) as usize * 400 + (x * 400.0) as usize];
        let c = at(&img, 0.525, 0.48);
        assert!(c[1] < 70 && c[0].abs_diff(c[1]) < 4, "dark neutral pupil: {c:?}");
        // catchlight up-left of the centre
        s.red_eye[0].catchlight = Some(Point::new(-0.35, -0.35));
        let img = render(&src, &info, &s, &RenderRequest::fit(400, 400)).image;
        let r = 14.0 / 400.0;
        let l = at(&img, 0.525 - 0.35 * r, 0.48 - 0.35 * r * 2.0);
        assert!(l[0] > 200 && l[1] > 200 && l[2] > 200, "catchlight {l:?}");
        assert!(at(&img, 0.525 + 0.4 * r, 0.48 + 0.4 * r * 2.0)[1] < 70, "rest of the pupil stays dark");
    }

    #[test]
    fn red_pupil_is_neutralized_and_darkened_iris_kept() {
        let eyes = [EyeK { cx: 50.0, cy: 50.0, r: 10.0, darken: 0.5, pet: false, catchlight: None }];
        let red = eye_pixel([0.5, 0.04, 0.05], 50.0, 50.0, &eyes);
        assert!((red[0] - red[1]).abs() < 0.01 && red[0] < 0.05, "{red:?}");
        let iris = [0.12, 0.1, 0.08];
        let o = eye_pixel(iris, 52.0, 50.0, &eyes);
        assert!(o.iter().zip(&iris).all(|(a, b)| (a - b).abs() < 0.02), "not red: barely touched {o:?}");
        let far = eye_pixel([0.5, 0.04, 0.05], 80.0, 50.0, &eyes);
        assert_eq!(far, [0.5, 0.04, 0.05]);
        // darker with more Darken
        let dark = eye_pixel([0.5, 0.2, 0.2], 50.0, 50.0, &[EyeK { darken: 1.0, ..eyes[0] }]);
        let light = eye_pixel([0.5, 0.2, 0.2], 50.0, 50.0, &[EyeK { darken: 0.0, ..eyes[0] }]);
        assert!(dark[1] < light[1]);
    }
}
