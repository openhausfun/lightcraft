//! A CPU rasterizer for egui's tessellated output, so the exact UI can be drawn into an image
//! without a window or a GPU (headless screenshots, CI, agents running while the display sleeps).
//!
//! It mirrors what egui's GPU backends do (`egui-wgpu`'s `fs_main_gamma_framebuffer`):
//! - vertex colours are sRGB-encoded, premultiplied [`Color32`]s, interpolated in gamma space;
//! - textures are sampled in gamma space (bilinear on texel centres, clamp to edge — like the
//!   "predictable texture filtering" path), and multiplied with the vertex colour;
//! - blending is premultiplied `src + dst·(1 − src.a)` into an 8-bit gamma framebuffer;
//! - clip rects become integer scissor rects (`round(points · pixels_per_point)`).
//!
//! Triangles are rasterized at pixel centres with a consistent tie-break rule, so shared edges are
//! covered exactly once (no double-blended seams in anti-aliasing fringes). Paint callbacks are
//! skipped (the app draws everything — including photos — as textured meshes). Bands of rows are
//! rasterized in parallel; each pixel is touched by one thread in primitive order, so the output is
//! deterministic.

use std::collections::HashMap;
use std::sync::Arc;

use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, ColorImage, ImageData, TextureFilter, TextureId, TexturesDelta};

/// One texture's pixels (sRGB, premultiplied) and sampling filters.
#[derive(Clone)]
pub struct CpuTexture {
    pub image: Arc<ColorImage>,
    pub magnification: TextureFilter,
    pub minification: TextureFilter,
}

impl CpuTexture {
    pub fn linear(image: Arc<ColorImage>) -> Self {
        CpuTexture { image, magnification: TextureFilter::Linear, minification: TextureFilter::Linear }
    }
}

/// Looks up textures by id while painting.
pub trait TextureSource: Sync {
    fn texture(&self, id: TextureId) -> Option<&CpuTexture>;
}

/// CPU mirror of a context's textures, kept up to date from [`TexturesDelta`]s.
#[derive(Default)]
pub struct TextureStore {
    map: HashMap<TextureId, CpuTexture>,
    /// Frees from the last delta; applied before the next one (the frame using them may still be
    /// painted until then).
    deferred_free: Vec<TextureId>,
}

impl TextureStore {
    /// Apply a frame's texture updates (call once per frame, before painting that frame).
    pub fn apply(&mut self, mut delta: TexturesDelta) {
        for id in self.deferred_free.drain(..) {
            self.map.remove(&id);
        }
        for (id, d) in delta.set.iter().flat_map(|(id, ds)| ds.iter().map(move |d| (id, d))) {
            let ImageData::Color(img) = &d.image;
            match d.pos {
                None => {
                    self.map
                        .insert(*id, CpuTexture { image: img.clone(), magnification: d.options.magnification, minification: d.options.minification });
                }
                Some([x0, y0]) => {
                    let Some(t) = self.map.get_mut(id) else { continue };
                    let dst = Arc::make_mut(&mut t.image);
                    let [dw, dh] = dst.size;
                    let [w, h] = img.size;
                    for y in 0..h.min(dh.saturating_sub(y0)) {
                        let n = w.min(dw.saturating_sub(x0));
                        let s = &img.pixels[y * w..y * w + n];
                        dst.pixels[(y0 + y) * dw + x0..(y0 + y) * dw + x0 + n].copy_from_slice(s);
                    }
                    t.magnification = d.options.magnification;
                    t.minification = d.options.minification;
                }
            }
        }
        self.deferred_free.extend(delta.free.iter().copied());
        delta.clear();
    }

    pub fn insert(&mut self, id: TextureId, t: CpuTexture) {
        self.map.insert(id, t);
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl TextureSource for TextureStore {
    fn texture(&self, id: TextureId) -> Option<&CpuTexture> {
        self.map.get(&id)
    }
}

/// Textures from `over` take precedence over `base` (e.g. the app's photo textures over an
/// offscreen context's font atlas).
pub struct Layered<'a> {
    pub over: &'a HashMap<TextureId, CpuTexture>,
    pub base: &'a TextureStore,
}

impl TextureSource for Layered<'_> {
    fn texture(&self, id: TextureId) -> Option<&CpuTexture> {
        self.over.get(&id).or_else(|| self.base.texture(id))
    }
}

#[derive(Clone, Copy)]
struct PVert {
    x: f64,
    y: f64,
    c: [f32; 4],
    u: f32,
    v: f32,
}

struct Prepared<'a> {
    /// Scissor rect in pixels: [x0, y0, x1, y1) .
    clip: [usize; 4],
    tex: Option<&'a CpuTexture>,
    verts: Vec<PVert>,
    indices: &'a [u32],
}

const BAND: usize = 32;

/// Rasterize `primitives` into a `size` (pixels) image cleared to `clear`. The result is opaque if
/// `clear` is.
pub fn paint(primitives: &[ClippedPrimitive], textures: &dyn TextureSource, size: [usize; 2], pixels_per_point: f32, clear: Color32) -> ColorImage {
    let [w, h] = size;
    let ppp = pixels_per_point as f64;
    let prepared: Vec<Prepared<'_>> = primitives
        .iter()
        .filter_map(|cp| {
            let Primitive::Mesh(mesh) = &cp.primitive else { return None };
            if mesh.indices.is_empty() {
                return None;
            }
            let r = cp.clip_rect;
            let px = |v: f32, max: usize| ((v * pixels_per_point).round().max(0.0) as usize).min(max);
            let clip = [px(r.min.x, w), px(r.min.y, h), px(r.max.x, w), px(r.max.y, h)];
            if clip[0] >= clip[2] || clip[1] >= clip[3] {
                return None;
            }
            let verts = mesh
                .vertices
                .iter()
                .map(|v| {
                    let [r, g, b, a] = v.color.to_array();
                    PVert {
                        x: v.pos.x as f64 * ppp,
                        y: v.pos.y as f64 * ppp,
                        c: [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0],
                        u: v.uv.x,
                        v: v.uv.y,
                    }
                })
                .collect();
            Some(Prepared { clip, tex: textures.texture(mesh.texture_id), verts, indices: &mesh.indices })
        })
        .collect();

    let mut pixels = vec![clear; w * h];
    let bands: Vec<(usize, &mut [Color32])> = pixels.chunks_mut((BAND * w).max(1)).enumerate().map(|(i, c)| (i * BAND, c)).collect();
    let threads = thread_count().min(bands.len()).max(1);
    if threads <= 1 {
        for (y0, band) in bands {
            paint_band(&prepared, band, y0, w);
        }
    } else {
        let mut groups: Vec<Vec<(usize, &mut [Color32])>> = (0..threads).map(|_| Vec::new()).collect();
        for (i, b) in bands.into_iter().enumerate() {
            groups[i % threads].push(b);
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::scope(|s| {
            for g in groups {
                let prepared = &prepared;
                s.spawn(move || {
                    for (y0, band) in g {
                        paint_band(prepared, band, y0, w);
                    }
                });
            }
        });
        #[cfg(target_arch = "wasm32")]
        for g in groups {
            for (y0, band) in g {
                paint_band(&prepared, band, y0, w);
            }
        }
    }
    ColorImage::new(size, pixels)
}

fn thread_count() -> usize {
    if cfg!(target_arch = "wasm32") { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(16) }
}

fn paint_band(prepared: &[Prepared<'_>], band: &mut [Color32], y0: usize, w: usize) {
    if w == 0 {
        return;
    }
    let y1 = y0 + band.len() / w;
    for p in prepared {
        let cy0 = p.clip[1].max(y0);
        let cy1 = p.clip[3].min(y1);
        if cy0 >= cy1 {
            continue;
        }
        let clip = [p.clip[0], cy0, p.clip[2], cy1];
        for tri in p.indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (p.verts.get(tri[0] as usize), p.verts.get(tri[1] as usize), p.verts.get(tri[2] as usize)) else {
                continue;
            };
            raster_triangle(band, y0, w, clip, p.tex, *a, *b, *c);
        }
    }
}

/// Edge function `E(p) = A·x + B·y + C` of the directed edge a→b (positive on the inside of a
/// counter-clockwise-normalized triangle), plus whether this edge owns pixels exactly on it.
#[derive(Clone, Copy)]
struct Edge {
    a: f64,
    b: f64,
    c: f64,
    owns_ties: bool,
}

impl Edge {
    fn new(p: &PVert, q: &PVert) -> Self {
        let a = p.y - q.y;
        let b = q.x - p.x;
        let c = -(a * p.x + b * p.y);
        // Of two triangles sharing this edge, exactly one sees (A > 0) || (A == 0 && B > 0).
        Edge { a, b, c, owns_ties: a > 0.0 || (a == 0.0 && b > 0.0) }
    }
    #[inline]
    fn eval(&self, x: f64, y: f64) -> f64 {
        self.a * x + self.b * y + self.c
    }
    #[inline]
    fn inside(&self, w: f64) -> bool {
        w > 0.0 || (w == 0.0 && self.owns_ties)
    }
}

#[allow(clippy::too_many_arguments)]
fn raster_triangle(
    band: &mut [Color32],
    band_y0: usize,
    w: usize,
    clip: [usize; 4],
    tex: Option<&CpuTexture>,
    v0: PVert,
    mut v1: PVert,
    mut v2: PVert,
) {
    let area = (v1.x - v0.x) * (v2.y - v0.y) - (v1.y - v0.y) * (v2.x - v0.x);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    if area < 0.0 {
        std::mem::swap(&mut v1, &mut v2);
    }
    let area = area.abs();
    // Pixel-centre bounding box, clipped.
    let min_x = v0.x.min(v1.x).min(v2.x);
    let max_x = v0.x.max(v1.x).max(v2.x);
    let min_y = v0.y.min(v1.y).min(v2.y);
    let max_y = v0.y.max(v1.y).max(v2.y);
    let x0 = ((min_x - 0.5).ceil().max(clip[0] as f64)) as usize;
    let x1 = ((max_x - 0.5).floor() + 1.0).min(clip[2] as f64).max(0.0) as usize;
    let y0 = ((min_y - 0.5).ceil().max(clip[1] as f64)) as usize;
    let y1 = ((max_y - 0.5).floor() + 1.0).min(clip[3] as f64).max(0.0) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // Barycentric weight of v0 comes from the edge opposite it (v1→v2), etc.
    let e0 = Edge::new(&v1, &v2);
    let e1 = Edge::new(&v2, &v0);
    let e2 = Edge::new(&v0, &v1);
    let inv = 1.0 / area;

    let white = Color32::WHITE;
    let uniform_color = v0.c == v1.c && v1.c == v2.c;
    let uniform_uv = v0.u == v1.u && v1.u == v2.u && v0.v == v1.v && v1.v == v2.v;
    // Texture filter: magnify if the triangle covers more pixels than texels.
    let filter = tex.map(|t| {
        if t.magnification == t.minification {
            return t.magnification;
        }
        let [tw, th] = t.image.size;
        let uv_area = ((v1.u - v0.u) * (v2.v - v0.v) - (v1.v - v0.v) * (v2.u - v0.u)).abs() as f64 * tw as f64 * th as f64;
        if uv_area <= area { t.magnification } else { t.minification }
    });
    let sample = |u: f32, v: f32| -> [f32; 4] {
        match (tex, filter) {
            (Some(t), Some(f)) => sample_texture(&t.image, u, v, f),
            _ => to_f(white),
        }
    };
    let const_tex = if uniform_uv { Some(sample(v0.u, v0.v)) } else { None };
    let const_src = match (uniform_color, const_tex) {
        (true, Some(t)) => Some(mul(v0.c, t)),
        _ => None,
    };
    if let Some(s) = const_src
        && s == [0.0; 4]
    {
        return;
    }

    for y in y0..y1 {
        let py = y as f64 + 0.5;
        let row = &mut band[(y - band_y0) * w..(y - band_y0 + 1) * w];
        let px0 = x0 as f64 + 0.5;
        let mut w0 = e0.eval(px0, py);
        let mut w1 = e1.eval(px0, py);
        let mut w2 = e2.eval(px0, py);
        for dst in &mut row[x0..x1] {
            if e0.inside(w0) && e1.inside(w1) && e2.inside(w2) {
                let src = match const_src {
                    Some(s) => s,
                    None => {
                        let (l0, l1, l2) = ((w0 * inv) as f32, (w1 * inv) as f32, (w2 * inv) as f32);
                        let c = if uniform_color { v0.c } else { std::array::from_fn(|i| l0 * v0.c[i] + l1 * v1.c[i] + l2 * v2.c[i]) };
                        let t = match const_tex {
                            Some(t) => t,
                            None => sample(l0 * v0.u + l1 * v1.u + l2 * v2.u, l0 * v0.v + l1 * v1.v + l2 * v2.v),
                        };
                        mul(c, t)
                    }
                };
                *dst = blend(*dst, src);
            }
            w0 += e0.a;
            w1 += e1.a;
            w2 += e2.a;
        }
    }
}

#[inline]
fn to_f(c: Color32) -> [f32; 4] {
    let [r, g, b, a] = c.to_array();
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]
}

#[inline]
fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
}

/// Premultiplied "over" in gamma space, rounded to 8 bits (like a `Rgba8Unorm` framebuffer).
#[inline]
fn blend(dst: Color32, src: [f32; 4]) -> Color32 {
    let k = 1.0 - src[3].clamp(0.0, 1.0);
    let d = dst.to_array();
    let ch = |i: usize| ((src[i] + d[i] as f32 / 255.0 * k).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Color32::from_rgba_premultiplied(ch(0), ch(1), ch(2), ch(3))
}

fn sample_texture(img: &ColorImage, u: f32, v: f32, filter: TextureFilter) -> [f32; 4] {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return [0.0; 4];
    }
    let at = |x: i64, y: i64| to_f(img.pixels[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize]);
    match filter {
        TextureFilter::Nearest => at((u * w as f32).floor() as i64, (v * h as f32).floor() as i64),
        TextureFilter::Linear => {
            let fx = u * w as f32 - 0.5;
            let fy = v * h as f32 - 0.5;
            let (x, y) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x, fy - y);
            let (x, y) = (x as i64, y as i64);
            let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
            std::array::from_fn(|i| {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bot = c[i] + (d[i] - c[i]) * tx;
                top + (bot - top) * ty
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::{Mesh, Vertex};
    use egui::{Rect, pos2};

    fn quad(rect: Rect, color: Color32) -> ClippedPrimitive {
        let mut m = Mesh::default();
        m.add_colored_rect(rect, color);
        ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) }
    }

    fn white_tex() -> TextureStore {
        let mut s = TextureStore::default();
        s.insert(TextureId::default(), CpuTexture::linear(Arc::new(ColorImage::new([1, 1], vec![Color32::WHITE]))));
        s
    }

    #[test]
    fn opaque_rect_fills_exactly_its_pixels() {
        let img = paint(&[quad(Rect::from_min_max(pos2(2.0, 3.0), pos2(6.0, 5.0)), Color32::RED)], &white_tex(), [10, 8], 1.0, Color32::BLACK);
        let n = img.pixels.iter().filter(|p| **p == Color32::RED).count();
        assert_eq!(n, 4 * 2);
        assert_eq!(img.pixels[3 * 10 + 2], Color32::RED);
        assert_eq!(img.pixels[2 * 10 + 2], Color32::BLACK);
    }

    #[test]
    fn shared_diagonal_is_not_blended_twice() {
        // half-transparent quad: every covered pixel gets exactly one blend
        let c = Color32::from_rgba_premultiplied(64, 64, 64, 128);
        let img = paint(&[quad(Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 8.0)), c)], &white_tex(), [8, 8], 1.0, Color32::BLACK);
        let first = img.pixels[0];
        assert!(img.pixels.iter().all(|p| *p == first), "seam on the diagonal");
        assert_eq!(first, Color32::from_rgba_premultiplied(64, 64, 64, 255));
    }

    #[test]
    fn clip_rect_and_scale_apply() {
        let mut p = quad(Rect::from_min_max(pos2(0.0, 0.0), pos2(10.0, 10.0)), Color32::GREEN);
        p.clip_rect = Rect::from_min_max(pos2(1.0, 1.0), pos2(3.0, 2.0));
        let img = paint(&[p], &white_tex(), [20, 20], 2.0, Color32::BLACK);
        assert_eq!(img.pixels.iter().filter(|p| **p == Color32::GREEN).count(), 4 * 2);
        assert_eq!(img.pixels[2 * 20 + 2], Color32::GREEN);
    }

    #[test]
    fn textured_triangle_samples_texels() {
        let mut s = TextureStore::default();
        let tex = ColorImage::new([2, 1], vec![Color32::RED, Color32::BLUE]);
        let id = TextureId::User(7);
        s.insert(id, CpuTexture { image: Arc::new(tex), magnification: TextureFilter::Nearest, minification: TextureFilter::Nearest });
        let mut m = Mesh::with_texture(id);
        m.add_rect_with_uv(Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 2.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let img = paint(&[ClippedPrimitive { clip_rect: Rect::EVERYTHING, primitive: Primitive::Mesh(m) }], &s, [4, 2], 1.0, Color32::BLACK);
        assert_eq!(img.pixels[0], Color32::RED);
        assert_eq!(img.pixels[3], Color32::BLUE);
        let _ = Vertex::default();
    }

    #[test]
    fn partial_texture_updates_patch_the_mirror() {
        let mut s = TextureStore::default();
        let id = TextureId::Managed(0);
        let mut d = TexturesDelta::default();
        d.push(id, egui::epaint::ImageDelta::full(ColorImage::new([4, 4], vec![Color32::BLACK; 16]), egui::TextureOptions::LINEAR));
        s.apply(d);
        let mut d = TexturesDelta::default();
        d.push(id, egui::epaint::ImageDelta::partial([1, 2], ColorImage::new([2, 1], vec![Color32::WHITE; 2]), egui::TextureOptions::LINEAR));
        s.apply(d);
        let t = s.texture(id).unwrap();
        assert_eq!(t.image.pixels[2 * 4 + 1], Color32::WHITE);
        assert_eq!(t.image.pixels[2 * 4 + 2], Color32::WHITE);
        assert_eq!(t.image.pixels[2 * 4 + 3], Color32::BLACK);
    }
}
