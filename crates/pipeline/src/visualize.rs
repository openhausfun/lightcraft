//! Diagnostic views drawn over a finished render ([`Overlay`]), shared by the CPU and GPU renderers
//! (they run on the 8-bit result, after the histogram is taken).
//!
//! - **Point Color range:** the photo is rendered without that sample's own adjustment; pixels
//!   inside the sample's range keep their colour, the rest turn grey (weighted by the range).
//! - **Visualize Spots** (Remove tool): a high-pass of the luminance thresholded to black/white,
//!   so dust spots and specks stand out as white dots. The scale is relative to the image (the
//!   same spots show at any preview size); the threshold slider (0..100) raises sensitivity.
//! - **Mask overlay** (Masking): the evaluated alpha of one mask, drawn as a colour tint, a colour
//!   tint on a black-and-white image, the image on black / white, or the alpha as white on black
//!   ([`MaskView`]). Both renderers hand the same alpha plane (the one the render used) to [`apply`].

use std::borrow::Cow;

use lightcraft_color::perceptual::{lab_to_lch, oklab_from_2020};
use lightcraft_color::transfer::srgb_to_linear;
use lightcraft_color::{REC2020, SRGB};
use lightcraft_develop::DevelopSettings;
use lightcraft_raster::{Plane, Rgba8};

use crate::colorops::PointK;
use crate::{Plan, for_rows};

/// A diagnostic view of a render.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Overlay {
    #[default]
    None,
    /// Point Color sample `i`: its range in colour, everything else grey.
    PointColorRange(u8),
    /// Visualize Spots with a threshold 0..100 (higher = more sensitive).
    Spots(u8),
    /// The evaluated alpha of mask `id` (a [`lightcraft_develop::Mask`] id), drawn as `view` in
    /// `color` at `opacity` (0..100; the colour views only).
    Mask { id: u16, view: MaskView, color: [u8; 3], opacity: u8 },
}

/// How [`Overlay::Mask`] draws the mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MaskView {
    /// The photo with the mask tinted in the overlay colour.
    #[default]
    Color,
    /// A black-and-white photo with the mask tinted in the overlay colour.
    ColorOnBw,
    /// The photo where the mask is, black elsewhere.
    ImageOnBlack,
    /// The photo where the mask is, white elsewhere.
    ImageOnWhite,
    /// The mask itself: white on black.
    WhiteOnBlack,
}

impl MaskView {
    pub const ALL: [MaskView; 5] = [MaskView::Color, MaskView::ColorOnBw, MaskView::ImageOnBlack, MaskView::ImageOnWhite, MaskView::WhiteOnBlack];

    /// Stable name (control channel, settings).
    pub fn name(self) -> &'static str {
        match self {
            MaskView::Color => "color",
            MaskView::ColorOnBw => "colorOnBw",
            MaskView::ImageOnBlack => "imageOnBlack",
            MaskView::ImageOnWhite => "imageOnWhite",
            MaskView::WhiteOnBlack => "whiteOnBlack",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MaskView::Color => "Color Overlay",
            MaskView::ColorOnBw => "Color Overlay on B&W",
            MaskView::ImageOnBlack => "Image on Black",
            MaskView::ImageOnWhite => "Image on White",
            MaskView::WhiteOnBlack => "White on Black",
        }
    }

    pub fn parse(s: &str) -> Option<MaskView> {
        MaskView::ALL.into_iter().find(|v| v.name() == s)
    }

    /// The next view (Shift+O cycles them).
    pub fn next(self) -> MaskView {
        let i = MaskView::ALL.iter().position(|v| *v == self).unwrap_or(0);
        MaskView::ALL[(i + 1) % MaskView::ALL.len()]
    }

    fn index(self) -> u64 {
        MaskView::ALL.iter().position(|v| *v == self).unwrap_or(0) as u64
    }
}

/// [`Overlay::Mask`]'s fields in 51 bits (exact in an `f64`): id 16, view 4, colour 24, opacity 7.
fn pack_mask(id: u16, view: MaskView, color: [u8; 3], opacity: u8) -> u64 {
    let rgb = (color[0] as u64) << 16 | (color[1] as u64) << 8 | color[2] as u64;
    id as u64 | view.index() << 16 | rgb << 20 | (opacity.min(100) as u64) << 44
}

fn unpack_mask(v: u64) -> Overlay {
    let rgb = (v >> 20) & 0xff_ffff;
    Overlay::Mask {
        id: (v & 0xffff) as u16,
        view: MaskView::ALL.get(((v >> 16) & 0xf) as usize).copied().unwrap_or_default(),
        color: [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8],
        opacity: ((v >> 44) & 0x7f).min(100) as u8,
    }
}

impl Overlay {
    /// A plain (kind, value) pair, e.g. for sending a request to a web worker.
    pub fn to_parts(self) -> (u8, f64) {
        match self {
            Overlay::None => (0, 0.0),
            Overlay::PointColorRange(i) => (1, i as f64),
            Overlay::Spots(t) => (2, t as f64),
            Overlay::Mask { id, view, color, opacity } => (3, pack_mask(id, view, color, opacity) as f64),
        }
    }

    /// Inverse of [`Overlay::to_parts`] (unknown kinds: no overlay).
    pub fn from_parts(kind: u8, v: f64) -> Overlay {
        match kind {
            1 => Overlay::PointColorRange(v as u8),
            2 => Overlay::Spots(v.clamp(0.0, 100.0) as u8),
            3 if v.is_finite() && v >= 0.0 => unpack_mask(v as u64),
            _ => Overlay::None,
        }
    }

    /// A stable value for render-cache keys (0 = no overlay).
    pub fn key(self) -> u64 {
        match self {
            Overlay::None => 0,
            Overlay::PointColorRange(i) => 0x1000 + i as u64,
            Overlay::Spots(t) => 0x2000 + t as u64,
            Overlay::Mask { id, view, color, opacity } => 3 << 60 | pack_mask(id, view, color, opacity),
        }
    }

    /// The mask an [`Overlay::Mask`] shows (if it exists in `s` and has components).
    pub fn mask(self, s: &DevelopSettings) -> Option<&lightcraft_develop::Mask> {
        match self {
            Overlay::Mask { id, .. } => s.masks.iter().find(|m| m.id == id as u32 && !m.components.is_empty()),
            _ => None,
        }
    }
}

/// Settings changes an overlay needs before rendering (e.g. the visualized sample's adjustment
/// is left out, so the range is shown on the colours it selects).
pub fn adjust_settings(o: Overlay, s: &mut Cow<'_, DevelopSettings>) {
    if let Overlay::PointColorRange(i) = o
        && let Some(p) = s.point_colors.get(i as usize)
        && !p.is_neutral()
    {
        let p = &mut s.to_mut().point_colors[i as usize];
        (p.hue_shift, p.sat_shift, p.lum_shift, p.variance) = (0.0, 0.0, 0.0, 0.0);
    }
}

/// Draw overlay `o` over `img` (rendered with `plan`). `mask` is the evaluated alpha of the mask
/// an [`Overlay::Mask`] shows, at the image's size (see [`Overlay::mask`]).
pub fn apply(img: &mut Rgba8, o: Overlay, plan: &Plan<'_>, mask: Option<&Plane>) {
    match o {
        Overlay::None => {}
        Overlay::PointColorRange(i) => {
            if let Some(p) = plan.settings.point_colors.get(i as usize) {
                point_color_range(img, &PointK::new(p));
            }
        }
        Overlay::Spots(t) => spots(img, t, plan.px_per_long),
        Overlay::Mask { view, color, opacity, .. } => {
            if let Some(a) = mask.filter(|a| (a.width, a.height) == (img.width, img.height)) {
                mask_view(img, a, view, color, opacity);
            }
        }
    }
}

/// Draw mask alpha `a` over `img` as `view`.
pub fn mask_view(img: &mut Rgba8, a: &Plane, view: MaskView, color: [u8; 3], opacity: u8) {
    let op = opacity.min(100) as f32 / 100.0;
    let col = color.map(|c| c as f32);
    let w = img.width;
    for_rows(&mut img.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let a = a.data[y * w + x].clamp(0.0, 1.0);
            let p = [px[0] as f32, px[1] as f32, px[2] as f32];
            let grey = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
            let out: [f32; 3] = match view {
                MaskView::Color => std::array::from_fn(|c| p[c] + (col[c] - p[c]) * a * op),
                MaskView::ColorOnBw => std::array::from_fn(|c| grey + (col[c] - grey) * a * op),
                MaskView::ImageOnBlack => p.map(|v| v * a),
                MaskView::ImageOnWhite => p.map(|v| v * a + 255.0 * (1.0 - a)),
                MaskView::WhiteOnBlack => [255.0 * a; 3],
            };
            let o = out.map(|v| v.round().clamp(0.0, 255.0) as u8);
            *px = [o[0], o[1], o[2], 255];
        }
    });
}

/// Blur radius (Gaussian sigma) of the spot view, as a fraction of the long edge.
const SPOT_SIGMA: f64 = 0.006;

/// High-pass threshold (encoded luminance) for slider value `t` (0..100): 100 shows the faintest
/// specks, 0 only strong ones.
pub fn spot_threshold(t: u8) -> f32 {
    let k = 1.0 - t.min(100) as f32 / 100.0;
    0.01 + 0.2 * k * k
}

fn spots(img: &mut Rgba8, t: u8, ppl: f64) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }
    let l = lightcraft_raster::Plane::from_fn(w, h, |x, y| {
        let p = img.data[y * w + x];
        (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0
    });
    let b = lightcraft_raster::blur::gaussian(&l, (SPOT_SIGMA * ppl).max(0.8) as f32);
    let thr = spot_threshold(t);
    for (i, px) in img.data.iter_mut().enumerate() {
        let v = if (l.data[i] - b.data[i]).abs() > thr { 255 } else { 0 };
        *px = [v, v, v, 255];
    }
}

fn point_color_range(img: &mut Rgba8, k: &PointK) {
    let m = SRGB.to_space(&REC2020).to_f32();
    let lut: Vec<f32> = (0..256).map(|v| srgb_to_linear(v as f32 / 255.0)).collect();
    let w = img.width;
    for_rows(&mut img.data, w, |_, row| {
        for px in row.iter_mut() {
            let lin = [lut[px[0] as usize], lut[px[1] as usize], lut[px[2] as usize]];
            let c = std::array::from_fn(|r| m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2]);
            let [l, ch, h] = lab_to_lch(oklab_from_2020(c));
            let a = k.weight(l, ch, h);
            let grey = 0.2126 * px[0] as f32 + 0.7152 * px[1] as f32 + 0.0722 * px[2] as f32;
            for c in 0..3 {
                px[c] = (grey + (px[c] as f32 - grey) * a).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RenderRequest, SourceInfo, render};
    use lightcraft_develop::PointColor;
    use lightcraft_raster::Rgb32f;

    #[test]
    fn spots_view_marks_specks_at_any_size() {
        // a smooth gradient with two small dark specks
        let src = Rgb32f::from_fn(400, 300, |x, y| {
            let d = |cx: f32, cy: f32| ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let base = 0.1 + 0.3 * x as f32 / 400.0;
            if d(100.0, 100.0) < 2.5 || d(300.0, 200.0) < 2.5 { [base * 0.5; 3] } else { [base; 3] }
        });
        let info = SourceInfo::default();
        let s = DevelopSettings::default();
        for size in [400, 200] {
            let req = RenderRequest { overlay: Overlay::Spots(50), ..RenderRequest::fit(size, size) };
            let v = render(&src, &info, &s, &req).image;
            let at = |x: f64, y: f64| v.data[(y * v.height as f64) as usize * v.width + (x * v.width as f64) as usize];
            assert_eq!(at(0.25, 1.0 / 3.0), [255, 255, 255, 255], "speck at size {size}");
            assert_eq!(at(0.75, 2.0 / 3.0), [255, 255, 255, 255]);
            assert_eq!(at(0.5, 0.5), [0, 0, 0, 255], "smooth gradient is black");
            let white = v.data.iter().filter(|p| p[0] == 255).count() as f64 / v.data.len() as f64;
            assert!(white < 0.01, "{white}");
        }
        assert!(spot_threshold(0) > spot_threshold(50) && spot_threshold(50) > spot_threshold(100));
        let (k, v) = Overlay::Spots(37).to_parts();
        assert_eq!(Overlay::from_parts(k, v), Overlay::Spots(37));
    }

    #[test]
    fn point_color_range_keeps_the_selected_colour_only() {
        // left half orange, right half blue
        let src = Rgb32f::from_fn(32, 16, |x, _| if x < 16 { [0.4, 0.15, 0.04] } else { [0.03, 0.06, 0.35] });
        let info = SourceInfo::default();
        let plain = render(&src, &info, &DevelopSettings::default(), &RenderRequest::fit(32, 16)).image;
        // sample the orange as it renders
        let px = plain.data[8 * 32 + 4];
        let lin = [px[0], px[1], px[2]].map(|v| srgb_to_linear(v as f32 / 255.0));
        let c = SRGB.to_space(&REC2020).apply_f32(lin);
        let [l, ch, h] = lab_to_lch(oklab_from_2020(c));
        let mut s = DevelopSettings::default();
        s.point_colors.push(PointColor { lum: l as f64, chroma: ch as f64, hue: (h as f64).to_degrees(), hue_shift: 80.0, ..Default::default() });
        let req = RenderRequest { overlay: Overlay::PointColorRange(0), ..RenderRequest::fit(32, 16) };
        let v = render(&src, &info, &s, &req).image;
        let (o, b) = (v.data[8 * 32 + 4], v.data[8 * 32 + 28]);
        // the orange stays orange (and unshifted: the overlay shows the selection, not the edit)
        assert!(o[0].abs_diff(px[0]) <= 2 && o[2].abs_diff(px[2]) <= 2, "{o:?} vs {px:?}");
        // the blue turns grey
        assert!(b[0].abs_diff(b[2]) <= 1, "{b:?}");
        // without the overlay the edit shows
        let e = render(&src, &info, &s, &RenderRequest::fit(32, 16)).image.data[8 * 32 + 4];
        assert!(e[1].abs_diff(px[1]) > 10, "{e:?} vs {px:?}");
    }

    #[test]
    fn mask_overlay_shows_the_evaluated_alpha() {
        use lightcraft_develop::{Mask, MaskComponent, MaskOp, MaskShape};
        use lightcraft_geom::Point;
        let src = Rgb32f::from_fn(80, 40, |x, _| if x < 40 { [0.18, 0.18, 0.18] } else { [0.05, 0.1, 0.3] });
        let info = SourceInfo::default();
        let shape = MaskShape::Radial { center: Point::new(0.25, 0.5), rx: 0.15, ry: 0.15, angle: 0.0, feather: 10.0, invert: false };
        let mut s = DevelopSettings::default();
        s.masks.push(Mask { id: 7, components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }], ..Default::default() });
        let plain = render(&src, &info, &s, &RenderRequest::fit(80, 40)).image;
        let shot = |s: &DevelopSettings, view: MaskView| {
            let o = Overlay::Mask { id: 7, view, color: [255, 0, 0], opacity: 100 };
            render(&src, &info, s, &RenderRequest { overlay: o, ..RenderRequest::fit(80, 40) }).image
        };
        let (inside, outside) = (20 * 80 + 20, 20 * 80 + 70);
        let c = shot(&s, MaskView::Color);
        assert_eq!(c.data[inside], [255, 0, 0, 255], "full tint inside the mask");
        assert_eq!(c.data[outside], plain.data[outside], "untouched outside");
        let bw = shot(&s, MaskView::WhiteOnBlack);
        assert_eq!((bw.data[inside], bw.data[outside]), ([255; 4], [0, 0, 0, 255]));
        let ib = shot(&s, MaskView::ImageOnBlack);
        assert_eq!((ib.data[inside], ib.data[outside]), (plain.data[inside], [0, 0, 0, 255]));
        let iw = shot(&s, MaskView::ImageOnWhite);
        assert_eq!(iw.data[outside], [255; 4]);
        let g = shot(&s, MaskView::ColorOnBw).data[outside];
        assert!(g[0] == g[1] && g[1] == g[2], "grey outside: {g:?}");
        // a hidden mask still shows (it's the one being edited)
        s.masks[0].visible = false;
        assert_eq!(shot(&s, MaskView::WhiteOnBlack).data[inside], [255; 4]);
        // an unknown mask draws nothing
        let o = Overlay::Mask { id: 9, view: MaskView::WhiteOnBlack, color: [0; 3], opacity: 50 };
        assert_eq!(render(&src, &info, &s, &RenderRequest { overlay: o, ..RenderRequest::fit(80, 40) }).image, plain);
        // parts round trip (web worker requests) and distinct cache keys
        let o = Overlay::Mask { id: 513, view: MaskView::ImageOnWhite, color: [12, 200, 255], opacity: 65 };
        let (k, v) = o.to_parts();
        assert_eq!(Overlay::from_parts(k, v), o);
        assert_ne!(o.key(), Overlay::Mask { id: 513, view: MaskView::Color, color: [12, 200, 255], opacity: 65 }.key());
        assert_eq!(MaskView::WhiteOnBlack.next(), MaskView::Color);
        assert_eq!(MaskView::parse("colorOnBw"), Some(MaskView::ColorOnBw));
    }
}
