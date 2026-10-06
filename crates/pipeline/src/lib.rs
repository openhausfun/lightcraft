//! The LightCraft develop pipeline (CPU reference implementation).
//!
//! Input: a scene-referred, linear Rec.2020 source image (already EXIF-oriented) at any resolution
//! (full size or a proxy), plus [`DevelopSettings`]. Output: a display-encoded sRGB image at the
//! requested size, and its histogram.
//!
//! Stage order (see `docs/pipeline.md`):
//! 1. geometry — user orientation, lens corrections (distortion, CA, vignetting), perspective, crop +
//!    straighten, flips; one resample at output resolution; then defringe
//! 2. scene-linear — white balance, exposure, dehaze, local tone (highlights/shadows), texture,
//!    clarity, local adjustments (masks)
//! 3. tone map — contrast / whites / blacks filmic curve on luminance, highlight desaturation
//! 4. colour — vibrance, saturation, colour mixer, colour grading, B&W (OkLCh)
//! 5. display — gamut map to the output space (sRGB unless [`RenderRequest::space`] says otherwise), encode, tone curves (parametric + point), vignette, grain
//!
//! Spatial parameters are specified relative to the image's long edge, so a 400 px preview and a
//! 60 MP export look alike.
//!
//! Exposure is a gain, so the spatial stages run on the un-exposed image and the per-pixel stage
//! applies it (filters on log luminance are shift-equivariant: identical result). With
//! [`render_cached`] each stage's output is reused while its inputs are unchanged ([`StageCache`]):
//! dragging a tone, colour or exposure slider re-runs only the per-pixel stage.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod auto;
pub mod colorops;
pub mod cull;
pub mod dust;
pub mod finish;
pub mod geometry;
pub mod local;
pub mod lut;
pub mod masks;
pub mod optics;
pub mod output;
pub mod profiles;
pub mod redeye;
pub mod spots;
pub mod tone;
pub mod transform;
pub mod upright;
pub mod visualize;

pub use output::{DeepImage, DeepSamples, OutputDepth, OutputSpace, OutputTrc, Proof};
pub use visualize::{MaskView, Overlay};

use lightcraft_develop::{DevelopSettings, Treatment};
use std::any::Any;
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use lightcraft_raster::{Histogram, Plane, Rgb32f, Rgba8, par_rows};

pub use tone::ToneMap;

/// Facts about the source the settings are interpreted against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceInfo {
    /// Lens corrections embedded in the file (DNG opcodes), relative to the EXIF-oriented source.
    pub lens: Option<lightcraft_develop::EmbeddedLens>,
    /// Raw sources use absolute Kelvin white balance; rendered sources use a relative scale around
    /// their as-shot white.
    pub raw: bool,
    pub as_shot_temp: f64,
    pub as_shot_tint: f64,
    /// No measured camera illuminant: WB adjustments are relative to the camera's rendered look.
    pub relative_wb: bool,
    pub camera_tone: Option<tone::CameraTone>,
}

impl Default for SourceInfo {
    fn default() -> Self {
        Self { raw: false, as_shot_temp: 6500.0, as_shot_tint: 0.0, lens: None, relative_wb: false, camera_tone: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Quality {
    /// Interactive drags: skips the most expensive refinements.
    Draft,
    #[default]
    Full,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderRequest {
    /// Output must fit in `max_w × max_h` (aspect preserved).
    pub max_w: usize,
    pub max_h: usize,
    pub quality: Quality,
    /// Show the crop frame's content only (true) or the whole uncropped image (crop tool active).
    pub apply_crop: bool,
    /// A diagnostic view drawn over the result (e.g. Point Color's "visualize range").
    pub overlay: Overlay,
    /// Colour space of the result (exports; previews stay sRGB).
    pub space: OutputSpace,
    /// Sample format: 8-bit only, or also a 16-bit / linear float [`Rendered::deep`] (exports).
    pub depth: OutputDepth,
    /// Soft proofing (CPU only; see [`Proof`]).
    pub proof: Option<Proof>,
}

impl RenderRequest {
    pub fn fit(max_w: usize, max_h: usize) -> Self {
        Self {
            max_w,
            max_h,
            quality: Quality::Full,
            apply_crop: true,
            overlay: Overlay::None,
            space: OutputSpace::Srgb,
            depth: OutputDepth::U8,
            proof: None,
        }
    }
}

pub struct Rendered {
    /// The result, 8-bit (for deep renders: `deep` reduced to 8 bits, display-encoded).
    pub image: Rgba8,
    pub histogram: Histogram,
    /// The high-bit-depth result when [`RenderRequest::depth`] asks for one.
    pub deep: Option<DeepImage>,
}

/// Everything the per-pixel stage needs, precomputed at output resolution.
///
/// The image and planes are computed *before exposure* (so they can be reused while exposure is
/// dragged): the per-pixel stage multiplies the image by `gain` and shifts log planes by `ev`.
/// Spatial filters on log luminance are shift-equivariant, so this equals filtering after exposure.
pub(crate) struct Prepared {
    pub img: Arc<Rgb32f>,
    /// Log2 luminance relative to middle grey (before exposure: add `ev`).
    pub log_l: Arc<Plane>,
    pub base: Arc<Plane>,
    pub clarity_blur: Option<Arc<Plane>>,
    pub texture_blur: Option<Arc<Plane>>,
    pub dark: Option<Arc<Plane>>,
    /// Blurred chromaticity (`rgb / Y`) for local Moiré / Noise.
    pub chroma_blur: Option<Arc<Rgb32f>>,
    /// Airlight of `dark` (before exposure).
    pub air: f32,
    pub masks: Vec<masks::Evaluated>,
    /// Output pixels per unit of the source long edge.
    pub px_per_long: f64,
}

/// Output size for a source of `src_w × src_h` under `s`, fitting `max_w × max_h`.
pub fn output_size(src_w: usize, src_h: usize, s: &DevelopSettings, req: &RenderRequest) -> (usize, usize) {
    geometry::Frame::new(src_w, src_h, s, req.apply_crop).fit(req.max_w, req.max_h)
}

/// Size of the cropped output of a `src_w × src_h` source under `s` at the source's own resolution
/// (what a full-size export produces).
pub fn native_output_size(src_w: usize, src_h: usize, s: &DevelopSettings) -> (f64, f64) {
    geometry::Frame::new(src_w, src_h, s, true).native_size()
}

/// The geometric frame `render` uses for `src` (lens data, automatic CA estimate included).
pub fn frame_for(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, apply_crop: bool) -> geometry::Frame {
    let mut frame = geometry::Frame::with_lens(src.width, src.height, s, apply_crop, info.lens.as_ref());
    if s.optics.remove_ca && s.section_enabled("optics") {
        frame.add_lateral_ca(optics::estimate_lateral_ca(src));
    }
    frame
}

/// Intermediate results of recent renders of one view, reused by [`render_cached`].
///
/// Each stage is keyed by exactly the inputs it depends on: the resampled source by the source
/// buffer, framing and output size; the white-balanced, retouched, denoised image by those plus
/// white balance, spots and noise reduction; each spatial plane by that plus its own radius. So a
/// tone or colour slider drag (or exposure) re-runs only the per-pixel stage, a clarity drag skips
/// resampling and noise reduction, and so on. Holds the last few output sizes (a draft-size and a
/// full-size render of the same view both stay warm).
pub struct StageCache {
    entries: Mutex<Vec<CacheEntry>>,
    capacity: usize,
    /// Another renderer's per-view state (the GPU renderer keeps its device-resident stages here).
    ext: Mutex<Option<Arc<dyn Any + Send + Sync>>>,
}

impl Default for StageCache {
    fn default() -> Self {
        StageCache { entries: Mutex::new(Vec::new()), capacity: 2, ext: Mutex::new(None) }
    }
}

impl StageCache {
    /// Drop everything (e.g. when memory is needed).
    pub fn clear(&self) {
        self.lock().clear();
        *self.ext.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// Per-view state of type `T` kept alongside this cache (created on first use). Lets another
    /// renderer of the same view (e.g. `lightcraft-gpu`) keep its own stage cache with this one.
    pub fn extension<T: Any + Send + Sync + Default>(&self) -> Arc<T> {
        let mut g = self.ext.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = g.as_ref()
            && let Ok(t) = e.clone().downcast::<T>()
        {
            return t;
        }
        let t = Arc::new(T::default());
        *g = Some(t.clone());
        t
    }

    /// The per-view state of type `T` if there is one (unlike [`Self::extension`], never creates it).
    pub fn peek_extension<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.ext.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|e| e.clone().downcast::<T>().ok())
    }

    /// Bytes of the intermediate images held (each buffer counted once; the sources they were
    /// made from belong to their owner and are not counted).
    pub fn bytes(&self) -> usize {
        fn size<T>(i: &lightcraft_raster::Image<T>) -> usize {
            i.data.len() * std::mem::size_of::<T>()
        }
        let mut seen: Vec<usize> = Vec::new();
        let mut total = 0;
        let mut add = |p: usize, n: usize| {
            if !seen.contains(&p) {
                seen.push(p);
                total += n;
            }
        };
        for e in self.lock().iter() {
            add(Arc::as_ptr(&e.sampled) as usize, size(&e.sampled));
            if let Some((_, l)) = &e.lin {
                add(Arc::as_ptr(l) as usize, size(l));
            }
            let pl = &e.planes;
            let planes = pl.log_l.iter().chain(pl.base.iter().map(|x| &x.1)).chain(pl.clarity.iter().map(|x| &x.1));
            for p in planes.chain(pl.texture.iter().map(|x| &x.1)).chain(pl.dark.iter().map(|x| &x.1)) {
                add(Arc::as_ptr(p) as usize, size(p));
            }
        }
        total
    }

    /// Number of cached output sizes.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CacheEntry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn get(&self, src: &Arc<Rgb32f>, geo: u64) -> Option<CacheEntry> {
        self.lock().iter().find(|e| e.geo == geo && Arc::ptr_eq(&e.src, src)).cloned()
    }

    fn put(&self, e: CacheEntry) {
        let mut v = self.lock();
        v.retain(|o| !(o.geo == e.geo && Arc::ptr_eq(&o.src, &e.src)));
        v.push(e);
        while v.len() > self.capacity {
            v.remove(0);
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    src: Arc<Rgb32f>,
    geo: u64,
    sampled: Arc<Rgb32f>,
    lin: Option<(u64, Arc<Rgb32f>)>,
    planes: local::Planes,
}

fn hash_of(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(parts)
}

/// What a render resolves to before any pixel work: the effective settings (profile and Upright
/// applied), the geometric frame, the output size and the stage-cache keys. Shared by the CPU
/// renderer and the GPU renderer (`lightcraft-gpu`), so both key their caches identically.
pub struct Plan<'a> {
    pub settings: Cow<'a, DevelopSettings>,
    pub frame: geometry::Frame,
    pub w: usize,
    pub h: usize,
    /// Output pixels per unit of the oriented source's long edge.
    pub px_per_long: f64,
    /// Long edge of the source buffer (px).
    pub src_long: usize,
    /// Key of the resampled source (together with the source buffer's identity).
    pub geo: u64,
    /// Key of the white-balanced, retouched, denoised image (and of the planes computed from it).
    pub lin_key: u64,
    /// Red eye / pet eye corrections with their detected pupils, in output pixels.
    pub eyes: Vec<redeye::EyeK>,
}

/// Resolve `s` against `src` for `req` (see [`Plan`]).
pub fn plan<'a>(src: &Rgb32f, info: &SourceInfo, s: &'a DevelopSettings, req: &RenderRequest) -> Plan<'a> {
    let mut settings: Cow<'a, DevelopSettings> = match profiles::effective(s) {
        Cow::Borrowed(b) => upright::resolve(src, info, b),
        Cow::Owned(o) => Cow::Owned(upright::resolve(src, info, &o).into_owned()),
    };
    visualize::adjust_settings(req.overlay, &mut settings);
    let s = &*settings;
    let frame = frame_for(src, info, s, req.apply_crop);
    let (w, h) = frame.fit(req.max_w, req.max_h);
    let px_per_long = frame.px_per_long(w);
    let src_long = src.width.max(src.height);
    let geo = hash_of((format!("{frame:?}"), w, h));
    let (wb_t, wb_tint) = local::effective_wb(info, s);
    let eyes = redeye::resolve(src, &s.red_eye, s.orientation, &frame, w, h, px_per_long);
    let d = &s.detail;
    let lin_key = hash_of((
        geo,
        [wb_t, wb_tint, info.as_shot_temp, info.as_shot_tint].map(f64::to_bits),
        format!("{:?}", s.spots),
        format!("{eyes:?}"),
        // defringe runs in this stage
        format!("{:?}", s.optics),
        [d.nr_luminance, d.nr_detail, d.nr_color, d.nr_color_detail, d.nr_color_smoothness].map(f64::to_bits),
        src_long,
    ));
    Plan { settings, frame, w, h, px_per_long, src_long, geo, lin_key, eyes }
}

/// Whether the scene-linear stage needs work only the CPU does (defringe, spot removal).
pub fn lin_needs_cpu(s: &DevelopSettings) -> bool {
    let o = &s.optics;
    let defringe = s.section_enabled("optics") && (o.defringe_purple_amount > 0.0 || o.defringe_green_amount > 0.0);
    defringe || !s.spots.is_empty()
}

/// The white-balanced, defringed, retouched image (before noise reduction): the CPU part of the
/// scene-linear stage, in place.
pub fn lin_cpu(img: &mut Rgb32f, info: &SourceInfo, p: &Plan<'_>) {
    let s = &*p.settings;
    local::white_balance(img, info, s);
    optics::defringe(img, s, p.px_per_long / optics::DEFRINGE_REF_LONG);
    spots::apply(img, &s.spots, &p.frame, p.px_per_long);
    redeye::apply(img, &p.eyes);
}

/// Render `src` with settings `s`.
pub fn render(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest) -> Rendered {
    render_impl(Src::Borrowed(src), info, s, req, None)
}

/// [`render`], reusing (and refreshing) the intermediate results in `cache`. The output is
/// identical to [`render`]'s.
pub fn render_cached(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, cache: &StageCache) -> Rendered {
    render_impl(Src::Shared(src), info, s, req, Some(cache))
}

enum Src<'a> {
    Borrowed(&'a Rgb32f),
    Shared(&'a Arc<Rgb32f>),
}

fn render_impl(src: Src<'_>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, cache: Option<&StageCache>) -> Rendered {
    // `Instant::now()` panics on wasm32-unknown-unknown: only read the clock when profiling.
    let lap = |what: &str, t: &mut Option<std::time::Instant>| {
        if let Some(t) = t {
            eprintln!("  {what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            *t = std::time::Instant::now();
        }
    };
    let mut t = profiling().then(std::time::Instant::now);
    let src_img: &Rgb32f = match &src {
        Src::Borrowed(r) => r,
        Src::Shared(a) => a,
    };
    let plan = plan(src_img, info, s, req);
    let Plan { ref frame, w, h, px_per_long, src_long, geo, lin_key, .. } = plan;
    let s = &*plan.settings;

    let shared = match (&src, cache) {
        (Src::Shared(a), Some(c)) => Some((*a, c)),
        _ => None,
    };
    let cached = shared.and_then(|(a, c)| c.get(a, geo));
    let sampled = match &cached {
        Some(e) => e.sampled.clone(),
        None => Arc::new(frame.sample(src_img, w, h)),
    };
    lap("sample", &mut t);

    let lin = match cached.as_ref().and_then(|e| e.lin.clone()).filter(|(k, _)| *k == lin_key) {
        Some((_, img)) => img,
        None => {
            // Without a cache the resampled buffer is ours: work on it in place.
            let mut img = if shared.is_some() { (*sampled).clone() } else { Arc::unwrap_or_clone(sampled.clone()) };
            lin_cpu(&mut img, info, &plan);
            local::denoise(&mut img, s, src_long, w.max(h));
            Arc::new(img)
        }
    };
    lap("wb/spots/nr", &mut t);
    let mut planes = match cached.map(|e| e.planes) {
        Some(p) if p.key == lin_key => p,
        _ => local::Planes { key: lin_key, ..Default::default() },
    };
    let prep = local::prepare(lin.clone(), s, frame, px_per_long, req.quality, &mut planes);
    lap("prepare", &mut t);
    if let Some((a, c)) = shared {
        c.put(CacheEntry { src: a.clone(), geo, sampled, lin: Some((lin_key, lin)), planes });
    }
    if req.depth != OutputDepth::U8 {
        let deep = finish::finish_deep(&prep, s, frame, info, req.space, req.depth, req.proof);
        let image = deep.to_rgba8();
        let histogram = Histogram::of_srgb8(&image);
        lap("finish (deep)", &mut t);
        return Rendered { image, histogram, deep: Some(deep) };
    }
    let image = finish::finish(&prep, s, frame, info, req.space, req.proof);
    lap("finish", &mut t);
    let histogram = Histogram::of_srgb8(&image);
    lap("histogram", &mut t);
    let mut image = image;
    let mask = overlay_alpha(req.overlay, &plan, &prep);
    visualize::apply(&mut image, req.overlay, &plan, mask.as_ref());
    Rendered { image, histogram, deep: None }
}

/// The colour a colour-range mask would sample at normalized image point `p` (OkLab of the
/// exposed scene colours, as [`MaskShape::ColorRange`](lightcraft_develop::MaskShape) compares
/// them), averaged over 3 × 3 pixels of a render fitting `req`. `None` outside the image.
pub fn color_range_sample(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, p: lightcraft_geom::Point) -> Option<[f64; 3]> {
    let plan = plan(src, info, s, req);
    let mut img = plan.frame.sample(src, plan.w, plan.h);
    lin_cpu(&mut img, info, &plan);
    let o = plan.frame.norm_to_out(plan.w, plan.h).apply(p);
    let (cx, cy) = (o.x.floor() as i64, o.y.floor() as i64);
    if cx < 0 || cy < 0 || cx >= plan.w as i64 || cy >= plan.h as i64 {
        return None;
    }
    let gain = (s.light.exposure as f32).exp2();
    let mut acc = [0f64; 3];
    let mut n = 0.0;
    for y in (cy - 1).max(0)..=(cy + 1).min(plan.h as i64 - 1) {
        for x in (cx - 1).max(0)..=(cx + 1).min(plan.w as i64 - 1) {
            let c = img.data[y as usize * plan.w + x as usize].map(|v| v * gain);
            let lab = lightcraft_color::perceptual::oklab_from_2020(masks::tonemap_for_select(c));
            for k in 0..3 {
                acc[k] += lab[k] as f64;
            }
            n += 1.0;
        }
    }
    Some(acc.map(|v| v / n))
}

/// The alpha plane a mask overlay shows: the one the render evaluated, or (for a hidden mask) a
/// fresh evaluation.
fn overlay_alpha(o: Overlay, plan: &Plan<'_>, prep: &Prepared) -> Option<Plane> {
    let m = o.mask(&plan.settings)?;
    if let Some(e) = prep.masks.iter().find(|e| e.id == m.id) {
        return Some(e.alpha.clone());
    }
    let ev = plan.settings.light.exposure as f32;
    Some(masks::evaluate_one(m, &plan.frame, plan.w, plan.h, &prep.img, &prep.log_l, ev))
}

/// Convenience: render a before/after pair side by side is up to the UI; this renders "before"
/// (default look, keeping the crop so framing matches).
pub fn before_settings(s: &DevelopSettings) -> DevelopSettings {
    let mut b = DevelopSettings { crop: s.crop, orientation: s.orientation, ..DevelopSettings::default() };
    b.wb = lightcraft_develop::WhiteBalance { mode: lightcraft_develop::WbMode::AsShot, ..b.wb };
    b
}

pub(crate) fn is_bw(s: &DevelopSettings) -> bool {
    s.treatment == Treatment::Bw || s.profile.id == "lc.mono" || s.profile.id.starts_with("lc.bw.")
}

/// `LIGHTCRAFT_PROFILE` is set: print per-stage timings to stderr.
pub fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGHTCRAFT_PROFILE").is_some())
}

/// Run `f`, printing its duration under `LIGHTCRAFT_PROFILE`.
pub(crate) fn timed<R>(what: &str, f: impl FnOnce() -> R) -> R {
    if !profiling() {
        return f();
    }
    let t = std::time::Instant::now();
    let r = f();
    eprintln!("    {what}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    r
}

pub(crate) fn for_rows<T: Send>(data: &mut [T], w: usize, f: impl Fn(usize, &mut [T]) + Sync + Send) {
    par_rows(data, w, f)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_geometry;
#[cfg(test)]
mod tests_local;
