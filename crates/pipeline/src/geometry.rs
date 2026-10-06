//! Output framing: user orientation, lens corrections and perspective ([`Warp`]), crop + straighten,
//! flips — and the mapping between output pixels and normalized coordinates (used by masks, spots and
//! on-canvas tools).
//!
//! Normalized coordinates (masks, spots, crop) refer to the *transformed* image: the oriented source after
//! lens correction and perspective, at the source's size. Everything is sampled in one resample.

use lightcraft_develop::{DevelopSettings, EmbeddedLens};
use lightcraft_geom::{Affine, CropGeometry, Homography, Orientation, Point, Rect};
use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Rgb32f, par_rows};

use crate::optics::{Warp, reorient_lens};

/// Linear value of areas outside the source after a warp (a light neutral, like an empty canvas).
pub const BLANK: [f32; 3] = [0.55, 0.55, 0.55];

/// The geometric frame of a render.
#[derive(Clone, Debug)]
pub struct Frame {
    /// Source (EXIF-oriented) dimensions in pixels.
    pub src_w: usize,
    pub src_h: usize,
    /// User orientation (Rotate Left/Right, Flip) on top of the source.
    pub orient: Orientation,
    /// Oriented dimensions.
    pub ow: f64,
    pub oh: f64,
    pub crop: CropGeometry,
    pub flip_h: bool,
    pub flip_v: bool,
    /// Lens corrections / perspective (None = identity).
    pub warp: Option<Warp>,
}

impl Frame {
    pub fn new(src_w: usize, src_h: usize, s: &DevelopSettings, apply_crop: bool) -> Frame {
        Frame::with_lens(src_w, src_h, s, apply_crop, None)
    }

    /// Like [`Frame::new`], with the source's embedded lens corrections (relative to the EXIF-oriented source).
    pub fn with_lens(src_w: usize, src_h: usize, s: &DevelopSettings, apply_crop: bool, lens: Option<&EmbeddedLens>) -> Frame {
        let orient = s.orientation;
        let (ow, oh) = if orient.swaps_axes() { (src_h as f64, src_w as f64) } else { (src_w as f64, src_h as f64) };
        let crop = if apply_crop { s.crop.geometry } else { CropGeometry::default() };
        let lens = lens.map(|l| reorient_lens(l, orient, src_w as f64, src_h as f64));
        let mut warp = Warp::from_settings(ow, oh, s, lens.as_ref());
        let persp = perspective(s, ow, oh);
        if persp != Homography::IDENTITY
            && let Some(inv) = persp.inverse()
        {
            warp.persp = persp;
            warp.persp_inv = inv;
        }
        let mut f = Frame {
            src_w,
            src_h,
            orient,
            ow,
            oh,
            crop,
            flip_h: apply_crop && s.crop.flip_h,
            flip_v: apply_crop && s.crop.flip_v,
            warp: (!warp.is_identity()).then_some(warp),
        };
        if apply_crop && s.geometry.constrain_crop {
            f.constrain_to_image();
        }
        f
    }

    /// "Constrain Crop": shrink the crop about its centre until it contains no area outside the warped source.
    fn constrain_to_image(&mut self) {
        let Some(wp) = self.warp.as_ref().filter(|w| w.moves_pixels()) else { return };
        let a = self.crop.output_to_source(self.ow, self.oh, 1.0, 1.0);
        let (w, h) = (self.ow, self.oh);
        let fits = |t: f64| {
            (0..=48).all(|i| {
                let u = i as f64 / 48.0;
                [(u, 0.0), (u, 1.0), (0.0, u), (1.0, u)].iter().all(|&(x, y)| {
                    let p = Point::new(0.5 + t * (x - 0.5), 0.5 + t * (y - 0.5));
                    let s = wp.to_source(a.apply(p), 1);
                    s.x >= -1e-6 * w && s.y >= -1e-6 * h && s.x <= w * (1.0 + 1e-6) && s.y <= h * (1.0 + 1e-6)
                })
            })
        };
        if fits(1.0) {
            return;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let m = (lo + hi) / 2.0;
            if fits(m) { lo = m } else { hi = m }
        }
        let t = lo.max(0.02);
        let r = self.crop.rect;
        self.crop.rect = Rect::from_center(r.center(), r.width() * t, r.height() * t);
    }

    /// Add automatically estimated lateral CA (`[α_R, α_B]`, see [`crate::optics::estimate_lateral_ca`]).
    pub fn add_lateral_ca(&mut self, ca: [f64; 2]) {
        if ca == [0.0; 2] {
            return;
        }
        let w = self.warp.get_or_insert_with(|| Warp::identity(self.ow, self.oh));
        w.ca[0] += ca[0];
        w.ca[2] += ca[1];
    }

    /// Map a point of the transformed image (normalized) to the lens-corrected, pre-perspective image (normalized).
    pub fn transformed_to_corrected(&self, p: Point) -> Point {
        match &self.warp {
            Some(w) => {
                let q = w.to_corrected(Point::new(p.x * self.ow, p.y * self.oh));
                Point::new(q.x / self.ow, q.y / self.oh)
            }
            None => p,
        }
    }

    /// Inverse of [`Frame::transformed_to_corrected`].
    pub fn corrected_to_transformed(&self, p: Point) -> Point {
        match &self.warp {
            Some(w) => {
                let q = w.from_corrected(Point::new(p.x * self.ow, p.y * self.oh));
                Point::new(q.x / self.ow, q.y / self.oh)
            }
            None => p,
        }
    }

    /// Aspect ratio (w/h) of the output.
    pub fn aspect(&self) -> f64 {
        let r = self.crop.rect_px(self.ow, self.oh);
        r.width() / r.height()
    }

    /// Size of the (cropped) output at the source's own resolution, in pixels.
    pub fn native_size(&self) -> (f64, f64) {
        let r = self.crop.rect_px(self.ow, self.oh);
        (r.width(), r.height())
    }

    pub fn fit(&self, max_w: usize, max_h: usize) -> (usize, usize) {
        let a = self.aspect();
        let (mw, mh) = (max_w.max(1) as f64, max_h.max(1) as f64);
        let (w, h) = if mw / mh > a { (mh * a, mh) } else { (mw, mw / a) };
        ((w.round() as usize).max(1), (h.round() as usize).max(1))
    }

    /// Output pixels per unit of the oriented image's long edge.
    pub fn px_per_long(&self, out_w: usize) -> f64 {
        let r = self.crop.rect_px(self.ow, self.oh);
        out_w as f64 / r.width() * self.ow.max(self.oh)
    }

    /// Affine from output pixel coordinates to oriented-image pixel coordinates.
    pub fn out_to_oriented(&self, out_w: usize, out_h: usize) -> Affine {
        let (w, h) = (out_w as f64, out_h as f64);
        let mut flip = Affine::IDENTITY;
        if self.flip_h {
            flip = Affine([-1.0, 0.0, 0.0, 1.0, w, 0.0]) * flip;
        }
        if self.flip_v {
            flip = Affine([1.0, 0.0, 0.0, -1.0, 0.0, h]) * flip;
        }
        self.crop.output_to_source(self.ow, self.oh, w, h) * flip
    }

    /// Map an output pixel to normalized oriented-image coordinates (0..1).
    pub fn out_to_norm(&self, out_w: usize, out_h: usize) -> Affine {
        Affine::scale(1.0 / self.ow, 1.0 / self.oh) * self.out_to_oriented(out_w, out_h)
    }

    /// Normalized oriented coords → output pixel (inverse of `out_to_norm`).
    pub fn norm_to_out(&self, out_w: usize, out_h: usize) -> Affine {
        self.out_to_norm(out_w, out_h).inverse().unwrap_or(Affine::IDENTITY)
    }

    /// Convert normalized oriented coordinates to "long-edge units" (isotropic), used by mask shapes.
    pub fn norm_to_long(&self, p: Point) -> Point {
        let l = self.ow.max(self.oh);
        Point::new(p.x * self.ow / l, p.y * self.oh / l)
    }

    /// How [`Frame::sample`] resamples a (EXIF-oriented) source of `src_w × src_h` into `w × h`.
    pub fn sample_plan(&self, src_w: usize, src_h: usize, w: usize, h: usize) -> SamplePlan {
        let (ow, oh) = if self.orient.swaps_axes() { (src_h, src_w) } else { (src_w, src_h) };
        let crop_px = self.crop.rect_px(self.ow, self.oh);
        let k = crop_px.width() / w as f64;
        let prefilter = (k > 1.25).then(|| (((ow as f64 / k).round() as usize).max(1), ((oh as f64 / k).round() as usize).max(1)));
        let (bw, bh) = prefilter.unwrap_or((ow, oh));
        let (sx, sy) = (bw as f64 / ow as f64, bh as f64 / oh as f64);
        let mode = if self.warp.is_some() {
            SampleMode::Warp(self.out_to_oriented(w, h))
        } else if self.crop.is_identity() && !self.flip_h && !self.flip_v && bw == w && bh == h {
            SampleMode::Copy
        } else {
            SampleMode::Affine(Affine::scale(sx, sy) * self.out_to_oriented(w, h))
        };
        SamplePlan { ow, oh, prefilter, sx, sy, mode }
    }

    /// Sample the source into a `w × h` output buffer: orientation, crop, rotation, flips — one
    /// bilinear resample from a pre-filtered (area-downscaled) copy, so minification never aliases.
    pub fn sample(&self, src: &Rgb32f, w: usize, h: usize) -> Rgb32f {
        let oriented = if self.orient == Orientation::Normal { None } else { Some(src.oriented(self.orient)) };
        let o = oriented.as_ref().unwrap_or(src);
        let plan = self.sample_plan(src.width, src.height, w, h);
        let base = match plan.prefilter {
            Some((nw, nh)) => std::borrow::Cow::Owned(resize(o, nw, nh, Filter::Mitchell)),
            None => std::borrow::Cow::Borrowed(o),
        };
        let affine = |xf: Affine| {
            let mut out = Rgb32f::new(w, h);
            par_rows(&mut out.data, w, |y, row| {
                for (x, px) in row.iter_mut().enumerate() {
                    let p = xf.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
                    *px = base.sample_bilinear(p.x as f32, p.y as f32);
                }
            });
            out
        };
        match (plan.mode, self.warp.as_ref()) {
            (SampleMode::Copy, _) => base.into_owned(),
            (SampleMode::Warp(o2t), Some(warp)) => sample_warped(&base, plan.sx, plan.sy, warp, o2t, w, h),
            // `sample_plan` plans a warp only when there is one; without it the same mapping is affine
            (SampleMode::Warp(o2t), None) => affine(Affine::scale(plan.sx, plan.sy) * o2t),
            (SampleMode::Affine(xf), _) => affine(xf),
        }
    }

    /// The crop rectangle as a quad in normalized oriented coordinates (for overlays).
    pub fn crop_quad_norm(&self) -> [Point; 4] {
        let r = self.crop.rect_px(self.ow, self.oh);
        let back = Affine::rotate_about(-self.crop.angle.to_radians(), Point::new(self.ow / 2.0, self.oh / 2.0));
        r.corners().map(|p| {
            let q = back.apply(p);
            Point::new(q.x / self.ow, q.y / self.oh)
        })
    }
}

/// How a render resamples its source (see [`Frame::sample_plan`]).
#[derive(Clone, Debug)]
pub struct SamplePlan {
    /// Oriented source size.
    pub ow: usize,
    pub oh: usize,
    /// Size of the Mitchell prefilter (area downscale before the bilinear resample), if any.
    pub prefilter: Option<(usize, usize)>,
    /// Oriented-source px → prefiltered px.
    pub sx: f64,
    pub sy: f64,
    pub mode: SampleMode,
}

#[derive(Clone, Debug)]
pub enum SampleMode {
    /// The (prefiltered) oriented source is the output.
    Copy,
    /// Bilinear through this affine (output px → prefiltered px).
    Affine(Affine),
    /// Through the warp; the affine maps output px → transformed px.
    Warp(Affine),
}

/// The perspective homography of the settings (Upright, then the manual Transform sliders), lens-corrected →
/// transformed, in centred coordinates.
pub fn perspective(s: &DevelopSettings, ow: f64, oh: f64) -> Homography {
    if !s.section_enabled("geometry") {
        return Homography::IDENTITY;
    }
    crate::transform::manual(&s.geometry).mul(&crate::upright::homography(&s.geometry, ow, oh))
}

/// One resample through the warp: `o2t` maps output px → transformed px, the warp maps those to source px
/// (scaled by `sx, sy` into `base`).
fn sample_warped(base: &Rgb32f, sx: f64, sy: f64, wp: &Warp, o2t: Affine, w: usize, h: usize) -> Rgb32f {
    let per_channel = wp.per_channel();
    let gain = wp.has_gain();
    let mut out = Rgb32f::new(w, h);
    par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let t = o2t.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
            let c = wp.to_corrected(t);
            let g = wp.corrected_to_source(c, 1);
            if !wp.inside(g) {
                *px = BLANK;
                continue;
            }
            let mut v = base.sample_bilinear((g.x * sx) as f32, (g.y * sy) as f32);
            if per_channel {
                for ch in [0usize, 2] {
                    let q = wp.corrected_to_source(c, ch);
                    v[ch] = base.sample_bilinear((q.x * sx) as f32, (q.y * sy) as f32)[ch];
                }
            }
            if gain {
                let k = wp.gain(g);
                v = v.map(|c| c * k);
            }
            *px = v;
        }
    });
    out
}

/// Whole-image rectangle in normalized coordinates.
pub const FULL: Rect = Rect::UNIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_is_identity() {
        let s = DevelopSettings::default();
        let f = Frame::new(300, 200, &s, true);
        assert_eq!(f.fit(3000, 3000), (3000, 2000));
        let m = f.out_to_norm(300, 200);
        let p = m.apply(Point::new(150.0, 100.0));
        assert!((p.x - 0.5).abs() < 1e-9 && (p.y - 0.5).abs() < 1e-9);
        assert!((f.px_per_long(300) - 300.0).abs() < 1e-9);
    }

    #[test]
    fn rotation_swaps_aspect() {
        let s = DevelopSettings { orientation: Orientation::Rotate90, ..Default::default() };
        let f = Frame::new(300, 200, &s, true);
        assert_eq!(f.fit(1000, 1000), (667, 1000));
    }

    #[test]
    fn crop_changes_aspect_and_mapping() {
        let mut s = DevelopSettings::default();
        s.crop.geometry.rect = Rect::new(0.5, 0.0, 1.0, 1.0);
        let f = Frame::new(400, 200, &s, true);
        assert_eq!(f.fit(1000, 1000), (1000, 1000));
        let p = f.out_to_norm(100, 100).apply(Point::new(0.0, 0.0));
        assert!((p.x - 0.5).abs() < 1e-9 && p.y.abs() < 1e-9);
        let back = f.norm_to_out(100, 100).apply(Point::new(0.75, 0.5));
        assert!((back.x - 50.0).abs() < 1e-6 && (back.y - 50.0).abs() < 1e-6);
    }

    #[test]
    fn flip_mirrors() {
        let mut s = DevelopSettings::default();
        s.crop.flip_h = true;
        let src = Rgb32f::from_fn(4, 1, |x, _| [x as f32, 0.0, 0.0]);
        let f = Frame::new(4, 1, &s, true);
        let out = f.sample(&src, 4, 1);
        assert!((out.get(0, 0)[0] - 3.0).abs() < 1e-5);
        assert!((out.get(3, 0)[0] - 0.0).abs() < 1e-5);
    }
}
