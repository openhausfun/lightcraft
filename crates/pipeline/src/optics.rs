//! Lens corrections (Optics) and the geometric warp.
//!
//! [`Warp`] is the complete inverse map from the *transformed* image (what the user sees before the crop:
//! after lens correction and perspective) back to the source pixels, evaluated once per output pixel inside the
//! single geometry resample:
//!
//! transformed px → (inverse perspective) → lens-corrected px → (manual distortion, embedded DNG warp,
//! per-channel lateral CA scale) → source px
//!
//! plus a radial vignetting gain evaluated at the source position. Every parameter is relative (normalized radii,
//! dimensionless scales), so a 400 px preview and a full-size export are warped identically.
//!
//! Also here: automatic lateral chromatic aberration estimation ([`estimate_lateral_ca`]) and [`defringe`].
//! Profile corrections use only lens data embedded in DNG files (`WarpRectilinear` / `FixVignetteRadial`).

use std::sync::Mutex;

use lightcraft_color::perceptual::{oklab_from_2020, oklab_to_2020};
use lightcraft_develop::{DevelopSettings, EmbeddedLens, EmbeddedVignette, EmbeddedWarp};
use lightcraft_geom::{Homography, Orientation, Point};
use lightcraft_raster::{Plane, Rgb32f};

use crate::for_rows;

/// Manual "Distortion" ±100 → radial coefficient (r normalized to the half diagonal).
const DISTORTION_K: f64 = 0.2;
/// Manual vignetting ±100 → stops of gain at the corners.
const VIGNETTE_STOPS: f64 = 1.5;
/// Manual CA ±100 → radial scale difference of a colour plane.
const CA_SCALE: f64 = 0.003;

/// The inverse map transformed → source, in pixels of the (user-)oriented source image `w × h`.
#[derive(Clone, Debug, PartialEq)]
pub struct Warp {
    pub w: f64,
    pub h: f64,
    /// Forward perspective (lens-corrected → transformed), in centred coordinates `(p − centre) / (long edge / 2)`.
    pub persp: Homography,
    /// Its inverse (transformed → lens-corrected).
    pub persp_inv: Homography,
    /// Manual radial distortion: source radius = r·(1 + k1·r²), r relative to the half diagonal.
    pub k1: f64,
    /// Manual vignetting: gain = 2^(amount·r^power) stops at source radius r (half-diagonal units).
    pub vig_stops: f64,
    pub vig_power: f64,
    /// Lateral CA: per-channel radial scale about the image centre (R, G, B).
    pub ca: [f64; 3],
    /// Embedded DNG lens corrections (already in this oriented frame) and their strengths (1 = as recorded).
    pub lens: Option<EmbeddedLens>,
    pub lens_dist: f64,
    pub lens_vig: f64,
}

impl Warp {
    pub fn identity(w: f64, h: f64) -> Warp {
        Warp {
            w,
            h,
            persp: Homography::IDENTITY,
            persp_inv: Homography::IDENTITY,
            k1: 0.0,
            vig_stops: 0.0,
            vig_power: 4.0,
            ca: [0.0; 3],
            lens: None,
            lens_dist: 0.0,
            lens_vig: 0.0,
        }
    }

    /// The lens part of the warp from the settings (optics section) and optional embedded lens data.
    pub fn from_settings(w: f64, h: f64, s: &DevelopSettings, lens: Option<&EmbeddedLens>) -> Warp {
        let mut wp = Warp::identity(w, h);
        if !s.section_enabled("optics") {
            return wp;
        }
        let o = &s.optics;
        wp.k1 = -DISTORTION_K * o.distortion / 100.0;
        wp.vig_stops = VIGNETTE_STOPS * o.vignetting / 100.0;
        wp.vig_power = 1.0 + 6.0 * (o.vignetting_midpoint / 100.0).clamp(0.0, 1.0);
        wp.ca = [CA_SCALE * o.ca_red / 100.0, 0.0, CA_SCALE * o.ca_blue / 100.0];
        if o.lens_profile
            && let Some(l) = lens
        {
            wp.lens = Some(*l);
            wp.lens_dist = (o.profile_distortion / 100.0).clamp(0.0, 2.0);
            wp.lens_vig = (o.profile_vignetting / 100.0).clamp(0.0, 2.0);
        }
        wp
    }

    fn half_long(&self) -> f64 {
        self.w.max(self.h) / 2.0
    }

    fn centre(&self) -> Point {
        Point::new(self.w / 2.0, self.h / 2.0)
    }

    /// Whether pixels move at all.
    pub fn moves_pixels(&self) -> bool {
        self.persp != Homography::IDENTITY
            || self.k1 != 0.0
            || self.ca.iter().any(|c| *c != 0.0)
            || (self.lens_dist != 0.0 && self.lens.is_some_and(|l| l.warp.is_some()))
    }

    /// Whether the colour planes are sampled at different positions.
    pub fn per_channel(&self) -> bool {
        self.ca[0] != self.ca[1]
            || self.ca[1] != self.ca[2]
            || (self.lens_dist != 0.0 && self.lens.and_then(|l| l.warp).is_some_and(|w| w.planes[0] != w.planes[1] || w.planes[1] != w.planes[2]))
    }

    pub fn has_gain(&self) -> bool {
        self.vig_stops != 0.0 || (self.lens_vig != 0.0 && self.lens.is_some_and(|l| l.vignette.is_some()))
    }

    pub fn is_identity(&self) -> bool {
        !self.moves_pixels() && !self.has_gain()
    }

    /// Transformed px → lens-corrected px (inverse perspective).
    pub fn to_corrected(&self, p: Point) -> Point {
        if self.persp == Homography::IDENTITY {
            return p;
        }
        let (c, l) = (self.centre(), self.half_long());
        let q = self.persp_inv.apply(Point::new((p.x - c.x) / l, (p.y - c.y) / l));
        Point::new(c.x + q.x * l, c.y + q.y * l)
    }

    /// Lens-corrected px → transformed px (forward perspective).
    pub fn from_corrected(&self, p: Point) -> Point {
        if self.persp == Homography::IDENTITY {
            return p;
        }
        let (c, l) = (self.centre(), self.half_long());
        let q = self.persp.apply(Point::new((p.x - c.x) / l, (p.y - c.y) / l));
        Point::new(c.x + q.x * l, c.y + q.y * l)
    }

    /// Lens-corrected px → source px for colour plane `ch` (0 = R, 1 = G, 2 = B).
    pub fn corrected_to_source(&self, p: Point, ch: usize) -> Point {
        let c = self.centre();
        let hd = (self.w.hypot(self.h) / 2.0).max(1e-9);
        let mut q = p;
        if self.k1 != 0.0 {
            let (dx, dy) = (q.x - c.x, q.y - c.y);
            let r2 = (dx * dx + dy * dy) / (hd * hd);
            let f = 1.0 + self.k1 * r2;
            q = Point::new(c.x + dx * f, c.y + dy * f);
        }
        if self.lens_dist != 0.0
            && let Some(wp) = self.lens.and_then(|l| l.warp)
        {
            let s = embedded_warp(&wp, q, ch, self.w, self.h);
            q = Point::new(q.x + (s.x - q.x) * self.lens_dist, q.y + (s.y - q.y) * self.lens_dist);
        }
        let k = self.ca[ch];
        if k != 0.0 {
            q = Point::new(c.x + (q.x - c.x) * (1.0 + k), c.y + (q.y - c.y) * (1.0 + k));
        }
        q
    }

    /// Transformed px → source px for plane `ch`.
    pub fn to_source(&self, p: Point, ch: usize) -> Point {
        self.corrected_to_source(self.to_corrected(p), ch)
    }

    /// Vignetting compensation gain at source position `src` (px).
    pub fn gain(&self, src: Point) -> f32 {
        let mut g = 1.0;
        if self.vig_stops != 0.0 {
            let hd = (self.w.hypot(self.h) / 2.0).max(1e-9);
            let r = (src.dist(self.centre()) / hd).min(1.5);
            g *= (self.vig_stops * r.powf(self.vig_power)).exp2();
        }
        if self.lens_vig != 0.0
            && let Some(v) = self.lens.and_then(|l| l.vignette)
        {
            g *= 1.0 + (embedded_vignette_gain(&v, src, self.w, self.h) - 1.0) * self.lens_vig;
        }
        g as f32
    }

    /// Whether a source position lies on the image (with a half-pixel margin).
    pub fn inside(&self, s: Point) -> bool {
        s.x >= -0.5 && s.y >= -0.5 && s.x <= self.w + 0.5 && s.y <= self.h + 0.5
    }
}

/// DNG `WarpRectilinear` source position for corrected position `p` (px) and plane `ch`.
pub fn embedded_warp(wp: &EmbeddedWarp, p: Point, ch: usize, w: f64, h: f64) -> Point {
    let (cx, cy) = (wp.center.x * w, wp.center.y * h);
    let m = (wp.radius * w.max(h)).max(1e-9);
    let [kr0, kr1, kr2, kr3, kt0, kt1] = wp.planes[ch.min(2)];
    let dx = (p.x - cx) / m;
    let dy = (p.y - cy) / m;
    let r2 = dx * dx + dy * dy;
    let f = kr0 + r2 * (kr1 + r2 * (kr2 + r2 * kr3));
    let sx = dx * f + kt0 * 2.0 * dx * dy + kt1 * (r2 + 2.0 * dx * dx);
    let sy = dy * f + kt1 * 2.0 * dx * dy + kt0 * (r2 + 2.0 * dy * dy);
    Point::new(cx + m * sx, cy + m * sy)
}

/// DNG `FixVignetteRadial` gain at source position `p` (px).
pub fn embedded_vignette_gain(v: &EmbeddedVignette, p: Point, w: f64, h: f64) -> f64 {
    let (cx, cy) = (v.center.x * w, v.center.y * h);
    let m = (v.radius * w.max(h)).max(1e-9);
    let r2 = ((p.x - cx).powi(2) + (p.y - cy).powi(2)) / (m * m);
    let mut g = 1.0;
    let mut rp = r2;
    for k in v.k {
        g += k * rp;
        rp *= r2;
    }
    g
}

/// Re-express embedded lens data (relative to a `w × h` image) for the image after orientation `o`.
pub fn reorient_lens(lens: &EmbeddedLens, o: Orientation, w: f64, h: f64) -> EmbeddedLens {
    if o == Orientation::Normal {
        return *lens;
    }
    let (ow, oh) = if o.swaps_axes() { (h, w) } else { (w, h) };
    let long_ratio = w.max(h) / ow.max(oh); // 1 (rotations keep the long edge)
    let map_pt = |c: Point| {
        let (x, y) = o.map(c.x * w, c.y * h, w, h);
        Point::new(x / ow, y / oh)
    };
    let warp = lens.warp.map(|wp| {
        let c = Point::new(wp.center.x * w, wp.center.y * h);
        let c2 = o.map(c.x, c.y, w, h);
        let planes = wp.planes.map(|[a, b, cc, d, kt0, kt1]| {
            // tangential coefficients form the vector (kt1, kt0), which rotates/reflects with the image
            let e = o.map(c.x + kt1, c.y + kt0, w, h);
            [a, b, cc, d, e.1 - c2.1, e.0 - c2.0]
        });
        EmbeddedWarp { planes, center: map_pt(wp.center), radius: wp.radius * long_ratio }
    });
    let vignette = lens.vignette.map(|v| EmbeddedVignette { k: v.k, center: map_pt(v.center), radius: v.radius * long_ratio });
    EmbeddedLens { warp, vignette }
}

// ------------------------------------------------------------------------------------------ lateral CA

/// Estimate lateral chromatic aberration: the radial magnification of the red and blue planes relative to green
/// (`[α_R, α_B]`, dimensionless; red edges sit at `r·(1 + α_R)`). Strong, radially oriented green edges are
/// located in each plane with sub-pixel 1-D matching of normalized edge profiles; a weighted, outlier-trimmed
/// least-squares fit of displacement = α·r gives the scale. Results are cached per image.
pub fn estimate_lateral_ca(img: &Rgb32f) -> [f64; 2] {
    static CACHE: Mutex<Vec<(u64, [f64; 2])>> = Mutex::new(Vec::new());
    let key = fingerprint(img);
    if let Ok(c) = CACHE.lock()
        && let Some((_, v)) = c.iter().find(|(k, _)| *k == key)
    {
        return *v;
    }
    let v = estimate_lateral_ca_uncached(img);
    if let Ok(mut c) = CACHE.lock() {
        c.push((key, v));
        if c.len() > 16 {
            c.remove(0);
        }
    }
    v
}

/// Cheap content fingerprint of an image (dimensions + 512 sampled pixels), for analysis caches.
pub(crate) fn fingerprint(img: &Rgb32f) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ ((img.width as u64) << 32 | img.height as u64);
    let n = img.data.len();
    for i in 0..512usize {
        let p = img.data[(i.wrapping_mul(2_654_435_761) ^ i) % n.max(1)];
        for c in p {
            h ^= c.to_bits() as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn estimate_lateral_ca_uncached(img: &Rgb32f) -> [f64; 2] {
    let (w, h) = (img.width, img.height);
    if w < 32 || h < 32 {
        return [0.0; 2];
    }
    let plane = |c: usize| -> Plane { img.map(|p| p[c]) };
    let planes = [plane(0), plane(1), plane(2)];
    let g = &planes[1];
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let hd = (w as f64).hypot(h as f64) / 2.0;
    // candidate edge points, best per grid cell (spread over the frame)
    const CELLS: usize = 24;
    const PER_CELL: usize = 12;
    let mut cells: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); CELLS * CELLS];
    let at = |x: usize, y: usize| g.data[y * w + x];
    for y in (6..h - 6).step_by(2) {
        for x in (6..w - 6).step_by(2) {
            let gx = at(x + 1, y) - at(x - 1, y);
            let gy = at(x, y + 1) - at(x, y - 1);
            let mag = gx.hypot(gy);
            let mean = (at(x + 1, y) + at(x - 1, y) + at(x, y + 1) + at(x, y - 1)) * 0.25 + 0.02;
            let rel = mag / mean;
            if rel < 0.2 {
                continue;
            }
            let (vx, vy) = (x as f64 - cx, y as f64 - cy);
            let rr = vx.hypot(vy);
            if rr < 0.2 * hd {
                continue;
            }
            let cos = ((gx as f64 * vx + gy as f64 * vy) / (mag as f64 * rr)).abs();
            if cos < 0.85 {
                continue;
            }
            let cell = (y * CELLS / h) * CELLS + x * CELLS / w;
            let v = &mut cells[cell];
            if v.len() < PER_CELL {
                v.push((rel, x, y));
            } else if let Some(min) = v.iter_mut().min_by(|a, b| a.0.total_cmp(&b.0))
                && min.0 < rel
            {
                *min = (rel, x, y);
            }
        }
    }
    let pts: Vec<(f32, usize, usize)> = cells.into_iter().flatten().collect();
    // sub-pixel displacement of R and B against G along the radial direction
    const HALF: i32 = 4; // profile half length (px)
    const SUB: i32 = 20; // samples per px
    const MAXD: i32 = 3; // max displacement (px)
    let sample = |p: &Plane, x: f64, y: f64| -> f32 {
        let fx = x.clamp(0.0, (w - 1) as f64);
        let fy = y.clamp(0.0, (h - 1) as f64);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = ((fx - x0 as f64) as f32, (fy - y0 as f64) as f32);
        let a = p.data[y0 * w + x0] + (p.data[y0 * w + x1] - p.data[y0 * w + x0]) * tx;
        let b = p.data[y1 * w + x0] + (p.data[y1 * w + x1] - p.data[y1 * w + x0]) * tx;
        a + (b - a) * ty
    };
    let znorm = |v: &mut [f32]| -> bool {
        let n = v.len() as f32;
        let m = v.iter().sum::<f32>() / n;
        let sd = (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / n).sqrt();
        if sd < 1e-5 {
            return false;
        }
        v.iter_mut().for_each(|x| *x = (*x - m) / sd);
        true
    };
    let mut obs: [Vec<(f64, f64, f64)>; 2] = [Vec::new(), Vec::new()];
    for &(wt, x, y) in &pts {
        let (vx, vy) = (x as f64 - cx, y as f64 - cy);
        let rr = vx.hypot(vy);
        let (ux, uy) = (vx / rr, vy / rr);
        let (px, py) = (x as f64, y as f64);
        let mut gp: Vec<f32> = (-HALF..=HALF).map(|t| sample(g, px + ux * t as f64, py + uy * t as f64)).collect();
        if !znorm(&mut gp) {
            continue;
        }
        for (k, ch) in [0usize, 2].into_iter().enumerate() {
            let reach = HALF + MAXD;
            let fine: Vec<f32> =
                (-reach * SUB..=reach * SUB).map(|i| sample(&planes[ch], px + ux * i as f64 / SUB as f64, py + uy * i as f64 / SUB as f64)).collect();
            let mut best = (f32::MAX, 0i32);
            for d in -MAXD * SUB..=MAXD * SUB {
                let mut prof: Vec<f32> = (-HALF..=HALF).map(|t| fine[((t + reach) * SUB + d) as usize]).collect();
                if !znorm(&mut prof) {
                    continue;
                }
                let ssd: f32 = prof.iter().zip(&gp).map(|(a, b)| (a - b) * (a - b)).sum();
                if ssd < best.0 {
                    best = (ssd, d);
                }
            }
            // normalized SSD = 2n(1 − corr): require a good match
            let n = (2 * HALF + 1) as f32;
            if best.0 < 0.3 * n && best.1.abs() < MAXD * SUB {
                obs[k].push((rr, best.1 as f64 / SUB as f64, wt as f64));
            }
        }
    }
    let fit = |o: &[(f64, f64, f64)]| -> f64 {
        if o.len() < 12 {
            return 0.0;
        }
        let solve = |o: &[&(f64, f64, f64)]| {
            let (num, den) = o.iter().fold((0.0, 0.0), |(n, d), (r, dd, wt)| (n + wt * dd * r, d + wt * r * r));
            if den > 0.0 { num / den } else { 0.0 }
        };
        let all: Vec<&(f64, f64, f64)> = o.iter().collect();
        let mut a = solve(&all);
        for _ in 0..2 {
            let mut res: Vec<f64> = o.iter().map(|(r, d, _)| (d - a * r).abs()).collect();
            res.sort_by(f64::total_cmp);
            let mad = res[res.len() / 2].max(0.05);
            let keep: Vec<&(f64, f64, f64)> = o.iter().filter(|(r, d, _)| (d - a * r).abs() <= 2.5 * mad).collect();
            if keep.len() < 8 {
                break;
            }
            a = solve(&keep);
        }
        a
    };
    [fit(&obs[0]), fit(&obs[1])]
}

// ------------------------------------------------------------------------------------------ defringe

/// OkLab hue (degrees) spanned by the purple hue slider (0..100) and the green one.
const PURPLE_HUES: (f32, f32) = (240.0, 360.0);
const GREEN_HUES: (f32, f32) = (90.0, 210.0);

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Membership of hue `h` (deg) in `[lo, hi]` (deg) with soft shoulders.
fn hue_weight(h: f32, lo: f32, hi: f32) -> f32 {
    let feather = 8.0;
    let mid = (lo + hi) / 2.0;
    let half = (hi - lo) / 2.0;
    let mut d = (h - mid).abs() % 360.0;
    if d > 180.0 {
        d = 360.0 - d;
    }
    1.0 - smoothstep(half, half + feather, d)
}

/// Long edge (px) at which defringe widths are specified: fringes are 1–7 px wide on a ~4000 px image.
pub const DEFRINGE_REF_LONG: f64 = 4000.0;

/// Defringe: desaturate purple / green colour fringes that sit next to high-contrast edges. `scale` is output
/// pixels per reference pixel (output long edge / [`DEFRINGE_REF_LONG`]) so the fringe width is the same
/// fraction of the image at any size.
pub fn defringe(img: &mut Rgb32f, s: &DevelopSettings, scale: f64) {
    let o = &s.optics;
    if !s.section_enabled("optics") || (o.defringe_purple_amount <= 0.0 && o.defringe_green_amount <= 0.0) {
        return;
    }
    let (w, h) = (img.width, img.height);
    let max_amt = o.defringe_purple_amount.max(o.defringe_green_amount);
    // fringe width in source px grows with the amount (1..7 px), in output px at least one
    let radius = ((1.0 + max_amt * 0.3) * scale).round().max(1.0) as usize;
    // perceptual lightness and its local range (dilated contrast)
    let lab: Vec<[f32; 3]> = img.data.iter().map(|p| oklab_from_2020([p[0].max(0.0), p[1].max(0.0), p[2].max(0.0)])).collect();
    let l = Plane { width: w, height: h, data: lab.iter().map(|p| p[0]).collect() };
    let (lo, hi) = (min_filter(&l, radius, false), min_filter(&l, radius, true));
    let range = |lo: f64, hi: f64, span: (f32, f32)| {
        let a = span.0 + (span.1 - span.0) * (lo / 100.0) as f32;
        let b = span.0 + (span.1 - span.0) * (hi / 100.0) as f32;
        (a.min(b), a.max(b))
    };
    let pr = range(o.defringe_purple_hue_lo, o.defringe_purple_hue_hi, PURPLE_HUES);
    let gr = range(o.defringe_green_hue_lo, o.defringe_green_hue_hi, GREEN_HUES);
    let ps = (o.defringe_purple_amount / 8.0).min(1.0) as f32;
    let gs = (o.defringe_green_amount / 8.0).min(1.0) as f32;
    for_rows(&mut img.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let edge = smoothstep(0.08, 0.25, hi.data[i] - lo.data[i]);
            if edge <= 0.0 {
                continue;
            }
            let [ll, a, b] = lab[i];
            let c = a.hypot(b);
            let chroma_w = smoothstep(0.015, 0.05, c);
            if chroma_w <= 0.0 {
                continue;
            }
            let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
            let k = (ps * hue_weight(hue, pr.0, pr.1)).max(gs * hue_weight(hue, gr.0, gr.1)) * edge * chroma_w;
            if k <= 0.0 {
                continue;
            }
            let f = 1.0 - k.min(1.0);
            *px = oklab_to_2020([ll, a * f, b * f]).map(|v| v.max(0.0));
        }
    });
}

/// Separable square min (or max) filter of radius `r`.
fn min_filter(p: &Plane, r: usize, max: bool) -> Plane {
    let (w, h) = (p.width, p.height);
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let mut tmp = Plane::new(w, h);
    for_rows(&mut tmp.data, w, |y, row| {
        let src = &p.data[y * w..(y + 1) * w];
        for (x, v) in row.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            *v = src[a..=b].iter().copied().fold(src[x], pick);
        }
    });
    let mut out = Plane::new(w, h);
    for_rows(&mut out.data, w, |y, row| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for (x, v) in row.iter_mut().enumerate() {
            let mut m = tmp.data[y * w + x];
            for yy in a..=b {
                m = pick(m, tmp.data[yy * w + x]);
            }
            *v = m;
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of dark lines on white with lateral CA: red magnified by `ar`, blue by `ab` (about the centre).
    fn ca_grid(w: usize, h: usize, ar: f64, ab: f64) -> Rgb32f {
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let val = |x: f64, y: f64| -> f32 {
            // smooth-edged discs on a grid (edges in every direction)
            let cell = w as f64 / 10.0;
            let (fx, fy) = ((x / cell).fract() - 0.5, (y / cell).fract() - 0.5);
            let d = fx.hypot(fy) * cell - cell * 0.28;
            (0.05 + 0.85 * (d / 1.2).clamp(-0.5, 0.5) + 0.425) as f32
        };
        Rgb32f::from_fn(w, h, |x, y| {
            let (x, y) = (x as f64 + 0.5, y as f64 + 0.5);
            let at = |a: f64| val(cx + (x - cx) / (1.0 + a), cy + (y - cy) / (1.0 + a));
            [at(ar), at(0.0), at(ab)]
        })
    }

    #[test]
    fn ca_estimate_recovers_scale() {
        let img = ca_grid(600, 400, 0.004, -0.003);
        let [r, b] = estimate_lateral_ca_uncached(&img);
        assert!((r - 0.004).abs() < 0.0008, "red {r}");
        assert!((b + 0.003).abs() < 0.0008, "blue {b}");
        let clean = ca_grid(600, 400, 0.0, 0.0);
        let [r, b] = estimate_lateral_ca_uncached(&clean);
        assert!(r.abs() < 0.0005 && b.abs() < 0.0005, "{r} {b}");
    }

    #[test]
    fn warp_identity_and_distortion_direction() {
        let s = DevelopSettings::default();
        let wp = Warp::from_settings(300.0, 200.0, &s, None);
        assert!(wp.is_identity());
        let mut s = s;
        s.optics.distortion = 50.0; // corrects barrel: corners sample from further in
        let wp = Warp::from_settings(300.0, 200.0, &s, None);
        let q = wp.to_source(Point::new(0.0, 0.0), 1);
        assert!(q.x > 0.0 && q.y > 0.0);
        let c = wp.to_source(Point::new(150.0, 100.0), 1);
        assert!(c.dist(Point::new(150.0, 100.0)) < 1e-9);
    }

    #[test]
    fn manual_vignette_brightens_corners() {
        let mut s = DevelopSettings::default();
        s.optics.vignetting = 100.0;
        let wp = Warp::from_settings(300.0, 200.0, &s, None);
        assert!((wp.gain(Point::new(150.0, 100.0)) - 1.0).abs() < 1e-6);
        assert!(wp.gain(Point::new(0.0, 0.0)) > 2.0);
    }

    #[test]
    fn reorient_keeps_centre_mapping() {
        let lens = EmbeddedLens {
            warp: Some(EmbeddedWarp { planes: [[1.0, 0.01, 0.0, 0.0, 0.001, 0.002]; 3], center: Point::new(0.4, 0.5), radius: 0.6 }),
            vignette: Some(EmbeddedVignette { k: [0.2, 0.0, 0.0, 0.0, 0.0], center: Point::new(0.4, 0.5), radius: 0.6 }),
        };
        let r = reorient_lens(&lens, Orientation::Rotate90, 300.0, 200.0);
        // rotate 90° cw: (x, y) → (h − y, x)
        let c = r.warp.unwrap().center;
        assert!((c.x - 0.5).abs() < 1e-9 && (c.y - 0.4).abs() < 1e-9, "{c:?}");
        // a point's warped position transforms like the point itself
        let p = Point::new(50.0, 30.0);
        let s0 = embedded_warp(&lens.warp.unwrap(), p, 0, 300.0, 200.0);
        let m = |q: Point| {
            let (x, y) = Orientation::Rotate90.map(q.x, q.y, 300.0, 200.0);
            Point::new(x, y)
        };
        let s1 = embedded_warp(&r.warp.unwrap(), m(p), 0, 200.0, 300.0);
        assert!(s1.dist(m(s0)) < 1e-6, "{s1:?} vs {:?}", m(s0));
    }

    #[test]
    fn defringe_removes_purple_at_edges_only() {
        let mut s = DevelopSettings::default();
        s.optics.defringe_purple_amount = 10.0;
        // white | black edge with a purple column next to it, and a purple patch far from edges
        let purple = [0.25f32, 0.05, 0.45];
        let mut img = Rgb32f::from_fn(40, 20, |x, _| if x < 20 { [0.9; 3] } else { [0.02; 3] });
        for y in 0..20 {
            img.set(20, y, purple);
        }
        let flat = Rgb32f::filled(40, 20, purple);
        let mut flat2 = flat.clone();
        defringe(&mut img, &s, 1.0);
        defringe(&mut flat2, &s, 1.0);
        let chroma = |p: [f32; 3]| {
            let l = oklab_from_2020(p);
            l[1].hypot(l[2])
        };
        assert!(chroma(img.get(20, 10)) < chroma(purple) * 0.3);
        assert!((chroma(flat2.get(20, 10)) - chroma(purple)).abs() < 1e-4);
    }
}
