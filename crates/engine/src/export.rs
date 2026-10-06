//! Export: encode a rendered image to JPEG / PNG / TIFF / WebP / AVIF (or copy the original / write a
//! DNG, each with the edits in XMP) in sRGB, Display P3, Adobe RGB
//! (1998) compatible, ProPhoto RGB or Rec. 2020 with an embedded ICC profile we generate from the
//! published primaries and curves, optional output sharpening and a JPEG file-size limit. Pure (bytes in, bytes out) so the desktop
//! app, CLI, MCP and the web build share it; writing the file is the caller's job.

pub use lightcraft_codecs::TiffCompression;
use lightcraft_codecs::{ChromaSubsampling, EncodeImage, EncodeMeta, NamedSpace, Samples, encode, icc};
use lightcraft_meta::{DateTime, Gps, Metadata};
pub use lightcraft_pipeline::{DeepImage, DeepSamples, OutputDepth, OutputSpace};
use lightcraft_raster::Rgba8;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Png,
    Tiff,
    Webp,
    Avif,
    /// The original file, unchanged, with an XMP sidecar holding the edits.
    Original,
    /// A DNG of the raw data (raw photos only), the edits embedded as XMP.
    Dng,
}

impl ExportFormat {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "jpeg" | "jpg" => Self::Jpeg,
            "png" => Self::Png,
            "tiff" | "tif" => Self::Tiff,
            "webp" => Self::Webp,
            "avif" => Self::Avif,
            "original" => Self::Original,
            "dng" => Self::Dng,
            _ => return None,
        })
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Tiff => "tif",
            Self::Webp => "webp",
            Self::Avif => "avif",
            // the original keeps its own extension (see `ExportOptions::file_name_for`)
            Self::Original => "",
            Self::Dng => "dng",
        }
    }

    /// Formats that render pixels (everything but `Original` and `Dng`).
    pub fn is_rendered(self) -> bool {
        !matches!(self, Self::Original | Self::Dng)
    }
}

/// How the output size is chosen (see [`Resize`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResizeMode {
    /// `value` = long edge in pixels.
    #[default]
    LongEdge,
    /// `value` = short edge in pixels.
    ShortEdge,
    /// `value` = width in pixels.
    Width,
    /// `value` = height in pixels.
    Height,
    /// Fit inside `value × height` pixels, whichever way round the photo is (long edge ≤ the larger).
    Dimensions,
    /// `value` = megapixels.
    Megapixels,
    /// `value` = percent of the full size.
    Percent,
}

/// Output size: a mode, its value(s) and whether smaller photos are left at their own size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Resize {
    pub mode: ResizeMode,
    pub value: f32,
    /// Second dimension for [`ResizeMode::Dimensions`].
    pub height: u32,
    /// Never upscale past the photo's own (cropped) size.
    pub dont_enlarge: bool,
}

impl Default for Resize {
    fn default() -> Self {
        Self { mode: ResizeMode::LongEdge, value: 2048.0, height: 2048, dont_enlarge: true }
    }
}

impl Resize {
    pub fn long_edge(px: u32) -> Self {
        Self { value: px as f32, ..Default::default() }
    }

    /// Scale factor from a `w × h` (cropped, full-resolution) photo to the output.
    pub fn scale(&self, w: f64, h: f64) -> f64 {
        let (w, h) = (w.max(1.0), h.max(1.0));
        let (long, short) = (w.max(h), w.min(h));
        let v = (self.value as f64).max(0.0);
        let k = match self.mode {
            ResizeMode::LongEdge => v / long,
            ResizeMode::ShortEdge => v / short,
            ResizeMode::Width => v / w,
            ResizeMode::Height => v / h,
            ResizeMode::Dimensions => {
                let (a, b) = (v, self.height as f64);
                (a.max(b) / long).min(a.min(b) / short)
            }
            ResizeMode::Megapixels => (v * 1e6 / (w * h)).sqrt(),
            ResizeMode::Percent => v / 100.0,
        };
        let k = if self.dont_enlarge { k.min(1.0) } else { k };
        // at least one pixel, at most the encoders' 65535 limit
        k.clamp(1.0 / short, 65_535.0 / long)
    }

    /// Output size for a `w × h` (cropped, full-resolution) photo.
    pub fn apply(&self, w: f64, h: f64) -> (usize, usize) {
        let k = self.scale(w, h);
        (((w * k).round() as usize).max(1), ((h * k).round() as usize).max(1))
    }
}

/// What happens when an output file already exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Conflict {
    /// Add `-2`, `-3`, … to the name.
    #[default]
    Unique,
    Overwrite,
    /// Leave the existing file and skip the photo.
    Skip,
}

/// A named set of `app.export` params.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub params: serde_json::Value,
}

/// The built-in export presets (File → Export with Preset).
pub fn builtin_presets() -> Vec<ExportPreset> {
    let p = |name: &str, params: serde_json::Value| ExportPreset { name: name.into(), params };
    vec![
        p("JPEG (Small)", serde_json::json!({"format": "jpeg", "quality": 85, "longEdge": 2048, "colorSpace": "srgb"})),
        p("JPEG (Large)", serde_json::json!({"format": "jpeg", "quality": 92, "longEdge": 0, "colorSpace": "srgb"})),
        p("Original + Settings", serde_json::json!({"format": "original"})),
        p("DNG", serde_json::json!({"format": "dng"})),
    ]
}

/// Output sharpening target (applied after resizing, in display-encoded values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SharpenFor {
    #[default]
    None,
    Screen,
    Matte,
    Glossy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SharpenAmount {
    Low,
    #[default]
    Standard,
    High,
}

/// Where a watermark sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

/// A text watermark, sized relative to the image so every export size looks the same.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Watermark {
    pub text: String,
    /// Basic Japanese vertical lettering: upright glyphs in columns from right to left.
    pub vertical: bool,
    /// Text height as a fraction of the image's short edge.
    pub size: f32,
    /// 0..1.
    pub opacity: f32,
    pub anchor: Anchor,
    /// Margin as a fraction of the short edge.
    pub inset: f32,
    /// sRGB colour.
    pub color: [u8; 3],
    /// Soft dark drop shadow for legibility on bright areas.
    pub shadow: bool,
    /// A graphic (PNG with transparency, JPEG, …) drawn instead of the text (empty = text).
    pub image: String,
    /// The graphic's width as a fraction of the photo's width.
    pub image_width: f32,
    /// The colour space the photo is in (set by the encoder; the graphic is converted into it).
    #[serde(skip)]
    pub target: Option<OutputSpace>,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            vertical: false,
            size: 0.035,
            opacity: 0.7,
            anchor: Anchor::BottomRight,
            inset: 0.025,
            color: [255; 3],
            shadow: true,
            image: String::new(),
            image_width: 0.2,
            target: None,
        }
    }
}

/// Inter SemiBold (OFL, see assets/ATTRIBUTION.md).
static WATERMARK_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

/// The watermark faces: Inter first, then the craft-fonts Japanese faces (Mincho first, the
/// watermark's serif look; then any other Japanese face). Without craft-fonts that is Inter alone,
/// and Japanese characters draw as Inter's missing-glyph box.
fn watermark_fonts(craft: &'static [crate::fonts::CraftFont]) -> Vec<ab_glyph::FontRef<'static>> {
    let mut japanese: Vec<_> = crate::fonts::japanese(craft).collect();
    japanese.sort_by_key(|f| !f.is_mincho());
    std::iter::once(WATERMARK_FONT)
        .chain(japanese.into_iter().map(|f| f.bytes))
        .filter_map(|bytes| ab_glyph::FontRef::try_from_slice(bytes).ok())
        .collect()
}

/// Draw `wm` onto `img` (straight alpha blending of the encoded values).
pub fn draw_watermark(img: &mut Rgba8, wm: &Watermark) {
    let width = img.width;
    watermark_coverage(img.width, img.height, wm, crate::fonts::CRAFT_FONTS, |x, y, k, col| {
        let p = &mut img.data[y * width + x];
        for c in 0..3 {
            p[c] = (p[c] as f32 + (col[c] as f32 - p[c] as f32) * k).round() as u8;
        }
    });
}

/// Draw `wm` onto a high-bit-depth image (`wm.color` is in the image's space, 8-bit encoded; float
/// images are linear and blend in linear light).
pub fn draw_watermark_deep(img: &mut DeepImage, wm: &Watermark) {
    let (width, trc) = (img.width, img.space.trc());
    match &mut img.samples {
        DeepSamples::U16(v) => watermark_coverage(img.width, img.height, wm, crate::fonts::CRAFT_FONTS, |x, y, k, col| {
            let i = (y * width + x) * 3;
            for c in 0..3 {
                let p = v[i + c] as f32;
                v[i + c] = (p + (col[c] as f32 * 257.0 - p) * k).round().clamp(0.0, 65535.0) as u16;
            }
        }),
        DeepSamples::F32(v) => watermark_coverage(img.width, img.height, wm, crate::fonts::CRAFT_FONTS, |x, y, k, col| {
            let i = (y * width + x) * 3;
            for c in 0..3 {
                let target = trc.decode(col[c] as f32 / 255.0);
                v[i + c] += (target - v[i + c]) * k;
            }
        }),
    }
}

/// Lay out `wm` on a `width × height` image and call `blend(x, y, coverage × opacity, colour)` for
/// every covered pixel (shadow pass first). `craft` is [`crate::fonts::CRAFT_FONTS`] (a parameter
/// so tests can render without it).
fn watermark_coverage(
    width: usize,
    height: usize,
    wm: &Watermark,
    craft: &'static [crate::fonts::CraftFont],
    mut blend_px: impl FnMut(usize, usize, f32, [u8; 3]),
) {
    use ab_glyph::{Font, PxScale, ScaleFont, point};
    if !wm.image.trim().is_empty() {
        logo_coverage(width, height, wm, blend_px);
        return;
    }
    let text = wm.text.trim();
    if text.is_empty() || width == 0 || height == 0 {
        return;
    }
    let fonts = watermark_fonts(craft);
    let Some(latin) = fonts.first() else { return };
    let short = width.min(height) as f32;
    let px = (wm.size.clamp(0.005, 0.5) * short).max(6.0);
    let mut glyphs = Vec::new();
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut tw = 0.0f32;
    let mut prev = None;
    let columns = text.split('\n').count();
    let mut column = 0usize;
    for original in text.chars() {
        if original == '\n' {
            if wm.vertical {
                column = column.saturating_add(1);
                tw = tw.max(y);
                y = 0.0;
            } else {
                tw = tw.max(x);
                x = 0.0;
                y += px;
            }
            prev = None;
            continue;
        }
        let vertical_form = match original {
            '、' => '︑',
            '。' => '︒',
            '（' => '︵',
            '）' => '︶',
            '「' => '﹁',
            '」' => '﹂',
            '『' => '﹃',
            '』' => '﹄',
            '【' => '︻',
            '】' => '︼',
            '…' => '︙',
            other => other,
        };
        let has = |font: &ab_glyph::FontRef<'_>, c: char| font.glyph_id(c).0 != 0;
        let ch = if wm.vertical && fonts.iter().skip(1).any(|f| has(f, vertical_form)) { vertical_form } else { original };
        // The first face with the glyph; Inter (its missing-glyph box) when none has it.
        let face = fonts.iter().position(|f| has(f, ch)).unwrap_or(0);
        let sf = fonts.get(face).unwrap_or(latin).as_scaled(PxScale::from(px));
        let id = sf.glyph_id(ch);
        if wm.vertical {
            let left = columns.saturating_sub(column.saturating_add(1)) as f32 * px;
            let pen = point(left + (px - sf.h_advance(id)) / 2.0, y + sf.ascent());
            let rotation = matches!(original, 'ー' | '—' | '–').then_some((left + px / 2.0, y + px / 2.0));
            glyphs.push((face, id.with_scale_and_position(px, pen), rotation));
            y += px;
        } else {
            if let Some((previous_face, previous)) = prev
                && previous_face == face
            {
                x += sf.kern(previous, id);
            }
            glyphs.push((face, id.with_scale_and_position(px, point(x, y + sf.ascent())), None));
            x += sf.h_advance(id);
            prev = Some((face, id));
        }
    }
    let (tw, th) = if wm.vertical { (columns as f32 * px, tw.max(y)) } else { (tw.max(x), y + px) };
    let inset = wm.inset.clamp(0.0, 0.4) * short;
    let (w, h) = (width as f32, height as f32);
    use Anchor::*;
    let ox = match wm.anchor {
        TopLeft | Left | BottomLeft => inset,
        Top | Center | Bottom => (w - tw) / 2.0,
        TopRight | Right | BottomRight => w - inset - tw,
    };
    let oy = match wm.anchor {
        TopLeft | Top | TopRight => inset,
        Left | Center | Right => (h - th) / 2.0,
        BottomLeft | Bottom | BottomRight => h - inset - th,
    };
    let alpha = wm.opacity.clamp(0.0, 1.0);
    let mut blend = |gx: i32, gy: i32, cov: f32, col: [u8; 3], a: f32| {
        if gx < 0 || gy < 0 || gx >= width as i32 || gy >= height as i32 {
            return;
        }
        blend_px(gx as usize, gy as usize, (cov * a).clamp(0.0, 1.0), col);
    };
    let passes: &[(f32, [u8; 3], f32)] =
        if wm.shadow { &[((px * 0.05).max(1.0), [0, 0, 0], 0.45), (0.0, wm.color, 1.0)] } else { &[(0.0, wm.color, 1.0)] };
    for &(off, col, a) in passes {
        for (face, g, rotation) in &glyphs {
            if let Some(o) = fonts.get(*face).and_then(|font| font.outline_glyph(g.clone())) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| {
                    let (sx, sy) = (b.min.x + gx as f32, b.min.y + gy as f32);
                    let (sx, sy) = match rotation {
                        Some((cx, cy)) => (cx - (sy - cy), cy + (sx - cx)),
                        None => (sx, sy),
                    };
                    blend((ox + off + sx) as i32, (oy + off + sy) as i32, cov, col, a * alpha);
                });
            }
        }
    }
}

/// A watermark graphic, decoded once per file (and again when the file changes).
fn watermark_logo(path: &str) -> Option<std::sync::Arc<Rgba8>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    static CACHE: Mutex<Option<HashMap<String, (std::time::SystemTime, Arc<Rgba8>)>>> = Mutex::new(None);
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let map = c.get_or_insert_with(HashMap::new);
    if let Some((m, img)) = map.get(path)
        && *m == modified
    {
        return Some(img.clone());
    }
    let bytes = std::fs::read(path).ok()?;
    let img = Arc::new(lightcraft_codecs::decode(&bytes, Default::default()).ok()?.to_srgb8());
    map.insert(path.to_string(), (modified, img.clone()));
    Some(img)
}

/// The graphic watermark: scaled (premultiplied, so transparent edges stay clean) to
/// `image_width` of the photo, placed like the text, blended with its alpha × opacity.
fn logo_coverage(width: usize, height: usize, wm: &Watermark, mut blend_px: impl FnMut(usize, usize, f32, [u8; 3])) {
    let Some(logo) = watermark_logo(wm.image.trim()) else {
        log::warn!("watermark: can't read {}", wm.image);
        return;
    };
    if width == 0 || height == 0 || logo.width == 0 || logo.height == 0 {
        return;
    }
    let lw = ((wm.image_width.clamp(0.01, 1.0) * width as f32).round() as usize).clamp(1, width);
    let lh = ((lw as f32 * logo.height as f32 / logo.width as f32).round() as usize).clamp(1, height);
    let pre: lightcraft_raster::Image<[f32; 4]> = lightcraft_raster::Image::from_fn(logo.width, logo.height, |x, y| {
        let p = logo.get(x, y);
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, a]
    });
    let scaled = lightcraft_raster::resample::resize(&pre, lw, lh, lightcraft_raster::resample::Filter::Mitchell);
    let inset = wm.inset.clamp(0.0, 0.4) * width.min(height) as f32;
    let (w, h, lwf, lhf) = (width as f32, height as f32, lw as f32, lh as f32);
    use Anchor::*;
    let ox = match wm.anchor {
        TopLeft | Left | BottomLeft => inset,
        Top | Center | Bottom => (w - lwf) / 2.0,
        TopRight | Right | BottomRight => w - inset - lwf,
    }
    .max(0.0) as usize;
    let oy = match wm.anchor {
        TopLeft | Top | TopRight => inset,
        Left | Center | Right => (h - lhf) / 2.0,
        BottomLeft | Bottom | BottomRight => h - inset - lhf,
    }
    .max(0.0) as usize;
    let space = wm.target.unwrap_or(OutputSpace::Srgb);
    let op = wm.opacity.clamp(0.0, 1.0);
    for y in 0..lh {
        for x in 0..lw {
            let [r, g, b, a] = scaled.get(x, y);
            let a = a.clamp(0.0, 1.0);
            let (px, py) = (ox + x, oy + y);
            if a <= 1e-4 || px >= width || py >= height {
                continue;
            }
            let c = [r, g, b].map(|v| (v / a).round().clamp(0.0, 255.0) as u8);
            blend_px(px, py, a * op, srgb8_in(space, c));
        }
    }
}

/// Which metadata is embedded in exported files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetadataPolicy {
    /// Everything we know: camera, capture settings, lens, location, title, caption, keywords, copyright.
    #[default]
    All,
    /// Everything except camera/lens make, model and capture settings.
    AllExceptCamera,
    /// Copyright and creator only.
    Copyright,
    /// Nothing (the sRGB profile is still embedded).
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// 1–100 (JPEG, AVIF).
    pub quality: u8,
    /// Output size (`None` = full size).
    pub resize: Option<Resize>,
    /// Print resolution written into the file (pixels per inch).
    pub ppi: u16,
    /// JPEG only: largest quality whose file fits in this many KB.
    pub limit_kb: Option<u32>,
    pub sharpen: SharpenFor,
    pub sharpen_amount: SharpenAmount,
    /// File name template with the batch-rename tokens ([`crate::rename::expand_tokens`]): `{name}`
    /// (original stem), `{seq}` (zero-padded to 3, counting from `start_number`; `{seq:N}` for N
    /// digits), `{date}` (capture date, YYYYMMDD; `{date:%Y-%m-%d}`…), `{num}`, `{folder}`,
    /// `{camera}`, `{lens}`, `{iso}`, `{rating}`, `{title}`, `{creator}`, `{ext}`; the output
    /// extension is appended.
    pub naming: String,
    /// First `{seq}` value.
    pub start_number: u32,
    /// Created inside the destination folder (empty = none).
    pub subfolder: String,
    /// When a file of the same name exists in the destination folder.
    pub conflict: Conflict,
    /// TIFF compression.
    pub tiff_compression: TiffCompression,
    /// DNG export: how the raw data is stored.
    pub dng_compression: DngCompression,
    pub metadata: MetadataPolicy,
    /// Strip GPS / location even when the policy would include it.
    pub remove_location: bool,
    /// Text watermark (none when absent or the text is empty).
    pub watermark: Option<Watermark>,
    /// Output colour space (AVIF is always sRGB: its muxer cannot embed a profile).
    pub color_space: OutputSpace,
    /// Bits per channel: 8, 16 (PNG, TIFF), 32 (TIFF: float, linear) or 10 (AVIF); `None` = the
    /// format's default (TIFF 16, everything else 8). See [`ExportOptions::effective_depth`].
    pub bit_depth: Option<u8>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 90,
            resize: None,
            ppi: 240,
            limit_kb: None,
            sharpen: SharpenFor::None,
            sharpen_amount: SharpenAmount::Standard,
            naming: "{name}".into(),
            start_number: 1,
            subfolder: String::new(),
            conflict: Conflict::Unique,
            tiff_compression: TiffCompression::Deflate,
            dng_compression: DngCompression::Lossless,
            metadata: MetadataPolicy::All,
            remove_location: false,
            watermark: None,
            color_space: OutputSpace::Srgb,
            bit_depth: None,
        }
    }
}

impl ExportOptions {
    /// Read options from command params (`format`, `quality`, `limitKb`, `sharpen`, `sharpenAmount`,
    /// `naming`, `metadata`, `removeLocation`, `watermark`, `colorSpace`, `bitDepth`, `ppi`) and the
    /// size: one of `longEdge`, `shortEdge`, `width`, `height` (both = fit inside W × H),
    /// `megapixels`, `percent` (absent or 0 = full size), or a [`Resize`] object as `resize`; plus
    /// `dontEnlarge` (default true).
    pub fn from_json(p: &serde_json::Value) -> Self {
        use serde_json::Value;
        let d = Self::default();
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        let u = |k: &str| p.get(k).and_then(Value::as_u64);
        fn enm<T: serde::de::DeserializeOwned>(p: &Value, k: &str) -> Option<T> {
            p.get(k).cloned().and_then(|v| serde_json::from_value(v).ok())
        }
        Self {
            format: s("format").and_then(ExportFormat::parse).unwrap_or(d.format),
            quality: u("quality").map_or(d.quality, |q| q.clamp(1, 100) as u8),
            resize: Self::resize_from_json(p),
            ppi: u("ppi").filter(|v| *v > 0).map_or(d.ppi, |v| v.min(u16::MAX as u64) as u16),
            limit_kb: u("limitKb").filter(|v| *v > 0).map(|v| v.min(u32::MAX as u64) as u32),
            sharpen: enm(p, "sharpen").unwrap_or(d.sharpen),
            sharpen_amount: enm(p, "sharpenAmount").unwrap_or(d.sharpen_amount),
            naming: s("naming").map_or(d.naming, str::to_string),
            start_number: u("startNumber").map_or(d.start_number, |v| v.min(999_999) as u32),
            subfolder: s("subfolder").map_or(d.subfolder, |v| v.trim().replace(['\\', ':', '\0'], "_")),
            conflict: enm(p, "conflict").unwrap_or(d.conflict),
            tiff_compression: match s("tiffCompression").map(str::to_ascii_lowercase).as_deref() {
                Some("none") => TiffCompression::None,
                Some("lzw") => TiffCompression::Lzw,
                Some("zip" | "deflate") => TiffCompression::Deflate,
                _ => d.tiff_compression,
            },
            dng_compression: enm(p, "dngCompression").unwrap_or(d.dng_compression),
            metadata: enm(p, "metadata").unwrap_or(d.metadata),
            remove_location: p.get("removeLocation").and_then(Value::as_bool).unwrap_or(d.remove_location),
            watermark: match p.get("watermark") {
                Some(Value::String(t)) => Some(Watermark { text: t.clone(), ..Default::default() }),
                Some(v @ Value::Object(_)) => serde_json::from_value(v.clone()).ok(),
                _ => None,
            }
            .filter(|w: &Watermark| !w.text.trim().is_empty() || !w.image.trim().is_empty()),
            color_space: s("colorSpace").and_then(OutputSpace::parse).unwrap_or(d.color_space),
            bit_depth: u("bitDepth").filter(|b| matches!(b, 8 | 10 | 16 | 32)).map(|b| b as u8),
        }
    }

    fn resize_from_json(p: &serde_json::Value) -> Option<Resize> {
        use serde_json::Value;
        let f = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| *v > 0.0);
        let dont_enlarge = p.get("dontEnlarge").and_then(Value::as_bool).unwrap_or(true);
        if let Some(r) = p.get("resize").filter(|v| v.is_object()) {
            let r: Resize = serde_json::from_value(r.clone()).ok()?;
            return (r.value > 0.0).then_some(Resize { dont_enlarge: p.get("dontEnlarge").and_then(Value::as_bool).unwrap_or(r.dont_enlarge), ..r });
        }
        let r = |mode, value: f64| Resize { mode, value: value.min(65_535.0) as f32, height: 0, dont_enlarge };
        Some(match (f("width"), f("height")) {
            (Some(w), Some(h)) => Resize { height: h.min(65_535.0) as u32, ..r(ResizeMode::Dimensions, w) },
            (Some(w), None) => r(ResizeMode::Width, w),
            (None, Some(h)) => r(ResizeMode::Height, h),
            (None, None) => {
                if let Some(v) = f("longEdge") {
                    r(ResizeMode::LongEdge, v)
                } else if let Some(v) = f("shortEdge") {
                    r(ResizeMode::ShortEdge, v)
                } else if let Some(v) = f("megapixels") {
                    r(ResizeMode::Megapixels, v)
                } else {
                    let v = f("percent")?;
                    r(ResizeMode::Percent, v)
                }
            }
        })
    }

    /// These options as `app.export` params ([`ExportOptions::from_json`] reads them back
    /// unchanged; full size is written as `longEdge: 0`).
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("resize");
            if let Some(sz) = Self::resize_json(self.resize.as_ref()).as_object() {
                o.extend(sz.clone());
            }
            if self.resize.is_none() {
                o.insert("longEdge".into(), 0.into());
            }
            o.retain(|_, v| !v.is_null());
        }
        v
    }

    /// Whether `p` names an output size (any of the size params, even 0 = full size).
    pub fn has_size_param(p: &serde_json::Value) -> bool {
        ["resize", "longEdge", "shortEdge", "width", "height", "megapixels", "percent"].iter().any(|k| p.get(k).is_some())
    }

    /// The size params of [`ExportOptions::from_json`] for `resize` (`{}` = full size).
    pub fn resize_json(resize: Option<&Resize>) -> serde_json::Value {
        match resize {
            None => serde_json::json!({}),
            Some(r) => serde_json::json!({"resize": r, "dontEnlarge": r.dont_enlarge}),
        }
    }

    /// The sample format the file is written with (unsupported requests fall back to the closest).
    pub fn effective_depth(&self) -> OutputDepth {
        match (self.format, self.bit_depth) {
            (ExportFormat::Jpeg | ExportFormat::Webp | ExportFormat::Original | ExportFormat::Dng, _) => OutputDepth::U8,
            (ExportFormat::Png, Some(16 | 32)) => OutputDepth::U16,
            (ExportFormat::Png, _) => OutputDepth::U8,
            (ExportFormat::Tiff, Some(8)) => OutputDepth::U8,
            (ExportFormat::Tiff, Some(32)) => OutputDepth::F32Linear,
            (ExportFormat::Tiff, _) => OutputDepth::U16,
            (ExportFormat::Avif, Some(10 | 16 | 32)) => OutputDepth::U16,
            (ExportFormat::Avif, _) => OutputDepth::U8,
        }
    }

    /// The bit depths `format` offers (value, label); the first is its default.
    pub fn bit_depths(format: ExportFormat) -> &'static [(u8, &'static str)] {
        match format {
            ExportFormat::Jpeg | ExportFormat::Webp | ExportFormat::Original | ExportFormat::Dng => &[(8, "8-bit")],
            ExportFormat::Png => &[(8, "8-bit"), (16, "16-bit")],
            ExportFormat::Tiff => &[(16, "16-bit"), (8, "8-bit"), (32, "32-bit float")],
            ExportFormat::Avif => &[(8, "8-bit"), (10, "10-bit")],
        }
    }

    /// The colour space the file is actually written in (AVIF: sRGB).
    pub fn effective_space(&self) -> OutputSpace {
        if self.format == ExportFormat::Avif { OutputSpace::Srgb } else { self.color_space }
    }

    /// Output file name for photo `p` at 1-based position `seq` in a batch (the original's
    /// extension is kept for [`ExportFormat::Original`]).
    pub fn file_name_for(&self, p: &lightcraft_catalog::Photo, seq: usize) -> String {
        let base = if self.naming.trim().is_empty() { "{name}" } else { self.naming.as_str() };
        let n = seq + self.start_number.max(1) as usize - 1;
        let name = crate::rename::expand_tokens(base, p, n, 3);
        let mut name: String = name.chars().map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '_' } else { c }).collect();
        let (stem, ext) = p.file_name.rsplit_once('.').unwrap_or((&p.file_name, ""));
        if name.trim().is_empty() {
            // every token came out empty (no title, no capture time…): keep the original name
            name = stem.to_string();
        }
        if self.format == ExportFormat::Original {
            let base = name.trim_end_matches('.');
            if ext.is_empty() { base.to_string() } else { format!("{base}.{ext}") }
        } else {
            format!("{name}.{}", self.format.extension())
        }
    }
}

/// Unsharp mask on an 8-bit image (separable box-blur approximation of a small Gaussian).
pub fn output_sharpen(img: &mut Rgba8, target: SharpenFor, amount: SharpenAmount) {
    let src: Vec<[f32; 3]> = img.data.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();
    if let Some(out) = unsharp(img.width, img.height, &src, target, amount) {
        for (p, v) in img.data.iter_mut().zip(out) {
            for c in 0..3 {
                p[c] = v[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// [`output_sharpen`] on a high-bit-depth image (float images are sharpened in linear light).
pub fn output_sharpen_deep(img: &mut DeepImage, target: SharpenFor, amount: SharpenAmount) {
    let (w, h) = (img.width, img.height);
    match &mut img.samples {
        DeepSamples::U16(v) => {
            let src: Vec<[f32; 3]> = v.as_chunks::<3>().0.iter().map(|c| [c[0] as f32, c[1] as f32, c[2] as f32]).collect();
            if let Some(out) = unsharp(w, h, &src, target, amount) {
                for (d, s) in v.iter_mut().zip(out.as_flattened()) {
                    *d = s.round().clamp(0.0, 65535.0) as u16;
                }
            }
        }
        DeepSamples::F32(v) => {
            let src: Vec<[f32; 3]> = v.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
            if let Some(out) = unsharp(w, h, &src, target, amount) {
                for (d, s) in v.iter_mut().zip(out.as_flattened()) {
                    *d = s.max(0.0);
                }
            }
        }
    }
}

/// The sharpened values of `src` (`w × h`), or `None` when nothing is to be done.
fn unsharp(w: usize, h: usize, src: &[[f32; 3]], target: SharpenFor, amount: SharpenAmount) -> Option<Vec<[f32; 3]>> {
    let (radius, base) = match target {
        SharpenFor::None => return None,
        SharpenFor::Screen => (1usize, 0.35f32),
        SharpenFor::Matte => (2, 0.6),
        SharpenFor::Glossy => (1, 0.5),
    };
    let k = base
        * match amount {
            SharpenAmount::Low => 0.6,
            SharpenAmount::Standard => 1.0,
            SharpenAmount::High => 1.5,
        };
    if w < 3 || h < 3 {
        return None;
    }
    let r = radius as isize;
    let box_pass = |inp: &[[f32; 3]], horizontal: bool| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0; 3]; inp.len()];
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 3];
                for d in -r..=r {
                    let (sx, sy) = if horizontal {
                        ((x as isize + d).clamp(0, w as isize - 1) as usize, y)
                    } else {
                        (x, (y as isize + d).clamp(0, h as isize - 1) as usize)
                    };
                    let p = inp[sy * w + sx];
                    acc = [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]];
                }
                let n = (2 * r + 1) as f32;
                out[y * w + x] = acc.map(|v| v / n);
            }
        }
        out
    };
    let blur = box_pass(&box_pass(src, true), false);
    Some(src.iter().zip(&blur).map(|(s, b)| std::array::from_fn(|c| s[c] + (s[c] - b[c]) * k)).collect())
}

/// Encode a rendered display-referred image according to `o` (its pixels must already be in
/// `o.effective_space()`). Resizing to `long_edge` is the
/// caller's job (render at that size); sharpening is applied here.
pub fn encode_image(img: &Rgba8, o: &ExportOptions) -> Result<Vec<u8>, String> {
    encode_with_metadata(img, o, None)
}

/// Like [`encode_image`], embedding `meta` (already filtered by the policy) as EXIF + XMP.
pub fn encode_with_metadata(img: &Rgba8, o: &ExportOptions, meta: Option<&Metadata>) -> Result<Vec<u8>, String> {
    let mut img = img.clone();
    output_sharpen(&mut img, o.sharpen, o.sharpen_amount);
    let space = o.effective_space();
    if let Some(wm) = &o.watermark {
        let wm = Watermark { color: srgb8_in(space, wm.color), target: Some(space), ..wm.clone() };
        draw_watermark(&mut img, &wm);
    }
    let profile = icc::write_named(named_space(space));
    let exif = meta.map(lightcraft_meta::write_exif);
    let xmp = meta.map(|m| lightcraft_meta::write_xmp(m, None));
    let meta = EncodeMeta { icc: Some(&profile), exif: exif.as_deref(), xmp: xmp.as_deref(), ppi: Some(o.ppi) };
    let e = EncodeImage::rgba8(&img);
    let r = match o.format {
        ExportFormat::Jpeg => {
            // 4:4:4 normally; 4:2:0 when targeting a file size (much smaller at equal visual quality).
            let sub = if o.limit_kb.is_some() { ChromaSubsampling::S420 } else { ChromaSubsampling::S444 };
            let jpeg = |q: u8| encode::encode_jpeg(&e, q, sub, &meta);
            match o.limit_kb {
                Some(kb) => {
                    let limit = kb as usize * 1024;
                    // Binary search for the highest quality that fits.
                    let (mut lo, mut hi) = (1u8, o.quality);
                    let mut best = None;
                    while lo <= hi {
                        let mid = lo + (hi - lo) / 2;
                        let b = jpeg(mid).map_err(|e| e.to_string())?;
                        if b.len() <= limit {
                            best = Some(b);
                            lo = mid + 1;
                        } else if mid == 1 {
                            break;
                        } else {
                            hi = mid - 1;
                        }
                    }
                    return best.ok_or_else(|| format!("cannot fit within {kb} KB"));
                }
                None => jpeg(o.quality),
            }
        }
        ExportFormat::Png => encode::encode_png(&e, &meta),
        ExportFormat::Tiff => encode::encode_tiff(&e, o.tiff_compression, &meta),
        ExportFormat::Webp => encode::encode_webp_lossless(&e, &meta),
        ExportFormat::Avif => encode::encode_avif(&e, o.quality, 8, &meta),
        f @ (ExportFormat::Original | ExportFormat::Dng) => return Err(format!("{f:?} export does not encode pixels")),
    };
    r.map_err(|e| e.to_string())
}

/// The codecs' name of an output space (for its ICC profile).
pub fn named_space(s: OutputSpace) -> NamedSpace {
    match s {
        OutputSpace::Srgb => NamedSpace::Srgb,
        OutputSpace::DisplayP3 => NamedSpace::DisplayP3,
        OutputSpace::AdobeRgb => NamedSpace::AdobeRgb,
        OutputSpace::ProPhoto => NamedSpace::ProPhoto,
        OutputSpace::Rec2020 => NamedSpace::Rec2020,
    }
}

/// An 8-bit sRGB colour expressed in `space` (encoded with its curve).
pub fn srgb8_in(space: OutputSpace, c: [u8; 3]) -> [u8; 3] {
    if space == OutputSpace::Srgb {
        return c;
    }
    let lin = c.map(lightcraft_color::transfer::decode_srgb8);
    let m = lightcraft_color::SRGB.to_space(&space.rgb_space()).to_f32();
    let t = space.trc();
    std::array::from_fn(|i| {
        let v = m[i][0] * lin[0] + m[i][1] * lin[1] + m[i][2] * lin[2];
        (t.encode(v) * 255.0 + 0.5) as u8
    })
}

/// Encode a render according to `o`: its high-bit-depth samples when it has them (16-bit PNG/TIFF,
/// 10-bit AVIF, 32-bit float linear TIFF), else its 8-bit image.
pub fn encode_rendered(r: &lightcraft_pipeline::Rendered, o: &ExportOptions, meta: Option<&Metadata>) -> Result<Vec<u8>, String> {
    match &r.deep {
        Some(d) if o.effective_depth() != OutputDepth::U8 => encode_deep(d, o, meta),
        _ => encode_with_metadata(&r.image, o, meta),
    }
}

/// Encode a high-bit-depth image (see [`encode_rendered`]).
pub fn encode_deep(img: &DeepImage, o: &ExportOptions, meta: Option<&Metadata>) -> Result<Vec<u8>, String> {
    let mut img = img.clone();
    output_sharpen_deep(&mut img, o.sharpen, o.sharpen_amount);
    if let Some(wm) = &o.watermark {
        let wm = Watermark { color: srgb8_in(img.space, wm.color), target: Some(img.space), ..wm.clone() };
        draw_watermark_deep(&mut img, &wm);
    }
    let profile = match img.samples {
        DeepSamples::F32(_) => icc::write_matrix_trc(&img.space.rgb_space(), &lightcraft_codecs::Trc::Linear),
        DeepSamples::U16(_) => icc::write_named(named_space(img.space)),
    };
    let exif = meta.map(lightcraft_meta::write_exif);
    let xmp = meta.map(|m| lightcraft_meta::write_xmp(m, None));
    let meta = EncodeMeta { icc: Some(&profile), exif: exif.as_deref(), xmp: xmp.as_deref(), ppi: Some(o.ppi) };
    let (w, h) = (img.width as u32, img.height as u32);
    let e = match &img.samples {
        DeepSamples::U16(v) => EncodeImage::new(w, h, 3, Samples::U16(v)),
        DeepSamples::F32(v) => EncodeImage::new(w, h, 3, Samples::F32(v)),
    };
    let r = match o.format {
        ExportFormat::Png => encode::encode_png(&e, &meta),
        ExportFormat::Tiff => encode::encode_tiff(&e, o.tiff_compression, &meta),
        ExportFormat::Avif => encode::encode_avif(&e, o.quality, 8, &meta),
        f => return Err(format!("{f:?} export is 8-bit only")),
    };
    r.map_err(|e| e.to_string())
}

/// Parse a shutter speed such as `1/250`, `0.5` or `2"` into seconds.
fn parse_shutter(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches(['s', '"']).trim();
    match s.split_once('/') {
        Some((n, d)) => Some(n.trim().parse::<f64>().ok()? / d.trim().parse::<f64>().ok()?),
        None => s.parse().ok(),
    }
    .filter(|v: &f64| v.is_finite() && *v > 0.0)
}

/// The metadata to embed for `photo` under `o.metadata` / `o.remove_location`. `None` = embed nothing.
pub fn export_metadata(photo: &lightcraft_catalog::Photo, o: &ExportOptions) -> Option<Metadata> {
    let m = &photo.meta;
    let text = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    // copyright info (also under "copyright only"): notice, creator, status, usage terms, info URL
    let mut out = Metadata {
        copyright: text(&m.copyright),
        copyright_marked: m.copyright_status.marked(),
        usage_terms: text(&m.usage_terms),
        copyright_url: text(&m.copyright_url),
        artist: text(&m.creator),
        software: Some("LightCraft".into()),
        ..Default::default()
    };
    match o.metadata {
        MetadataPolicy::None => return None,
        MetadataPolicy::Copyright => return Some(out),
        MetadataPolicy::All | MetadataPolicy::AllExceptCamera => {}
    }
    out.title = text(&m.title);
    out.caption = text(&m.caption);
    out.alt_text = text(&m.alt_text);
    out.extended_description = text(&m.extended_description);
    if !o.remove_location {
        out.sublocation = text(&m.location);
        out.city = text(&m.city);
        out.state = text(&m.state);
        out.country = text(&m.country);
    }
    out.keywords = m.keywords.clone();
    out.capture_time = photo.captured.as_deref().and_then(DateTime::parse_iso);
    out.rating = (photo.rating > 0).then_some(photo.rating as i8);
    // Pixels are exported upright: orientation is baked in.
    out.orientation = Some(lightcraft_meta::Orientation::Normal);
    if !o.remove_location {
        out.gps = m.gps.map(|(latitude, longitude)| Gps { latitude, longitude, altitude: None });
    }
    if o.metadata == MetadataPolicy::All {
        out.model = text(&m.camera);
        out.lens_model = text(&m.lens);
        out.focal_length = m.focal_mm.map(f64::from);
        out.f_number = m.aperture.map(f64::from);
        out.exposure_time = parse_shutter(&m.shutter);
        out.iso = m.iso;
    }
    Some(out)
}

/// One exported file.
pub struct Exported {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// Files written next to it, named like it with another extension: `(extension, bytes)` (the
    /// XMP sidecar of an `Original` export).
    pub sidecars: Vec<(&'static str, Vec<u8>)>,
}

/// Output size of photo `p` under `o` (its cropped full size when `o.resize` is `None`).
pub fn output_size(p: &lightcraft_catalog::Photo, o: &ExportOptions) -> (usize, usize) {
    let (w, h) = lightcraft_pipeline::native_output_size(p.width.max(1) as usize, p.height.max(1) as usize, &p.develop);
    match &o.resize {
        Some(r) => r.apply(w, h),
        None => ((w.round() as usize).max(1), (h.round() as usize).max(1)),
    }
}

/// Render photo `id` at the requested size and encode it (or copy / convert its original for
/// [`ExportFormat::Original`] / [`ExportFormat::Dng`]).
pub fn export_photo(session: &mut crate::Session, id: lightcraft_catalog::PhotoId, o: &ExportOptions, seq: usize) -> Result<Exported, String> {
    prepare_export(session, id, o, seq)?.run()
}

/// One photo's export, set up from the session ([`prepare_export`]). [`PreparedExport::run`] does
/// the heavy part (read, decode, render, encode) without the session, so it can run on another
/// thread.
pub struct PreparedExport {
    pub photo: lightcraft_catalog::PhotoId,
    pub file_name: String,
    work: Work,
    /// The library's originals, which [`run_batch`] never writes over (shared by a batch).
    guard: std::sync::Arc<crate::originals::OriginalGuard>,
}

struct RenderWork {
    job: crate::media::RenderJob,
    meta: Option<Metadata>,
    opts: ExportOptions,
}

enum Work {
    Render(Box<RenderWork>),
    /// [`ExportFormat::Original`] (the file's bytes + an XMP sidecar) or [`ExportFormat::Dng`] (the
    /// raw data re-encoded as a lossless DNG with the edits in its XMP).
    File {
        path: String,
        read: Option<crate::merge::ByteReader>,
        packet: String,
        dng: Option<DngCompression>,
        label: String,
        size: (usize, usize),
    },
}

/// Set up the export of photo `id` at 1-based position `seq` of a batch. (For a whole batch use
/// [`prepare_batch`]: it looks at the library's originals once.)
pub fn prepare_export(
    session: &mut crate::Session,
    id: lightcraft_catalog::PhotoId,
    o: &ExportOptions,
    seq: usize,
) -> Result<PreparedExport, String> {
    let guard = std::sync::Arc::new(session.original_guard());
    prepare_guarded(session, id, o, seq, guard)
}

/// [`prepare_export`] for each of `ids` in order (`{seq}` = position, from 1).
pub fn prepare_batch(session: &mut crate::Session, ids: &[lightcraft_catalog::PhotoId], o: &ExportOptions) -> Result<Vec<PreparedExport>, String> {
    let guard = std::sync::Arc::new(session.original_guard());
    ids.iter().enumerate().map(|(i, id)| prepare_guarded(session, *id, o, i + 1, guard.clone())).collect()
}

fn prepare_guarded(
    session: &mut crate::Session,
    id: lightcraft_catalog::PhotoId,
    o: &ExportOptions,
    seq: usize,
    guard: std::sync::Arc<crate::originals::OriginalGuard>,
) -> Result<PreparedExport, String> {
    let p = session.catalog.photo(id).ok_or("no such photo")?;
    let file_name = o.file_name_for(p, seq);
    let work = if o.format.is_rendered() {
        let (w, h) = output_size(p, o);
        let meta = export_metadata(p, o);
        let job = session.export_job(id, w, h, o.effective_space(), o.effective_depth())?;
        Work::Render(Box::new(RenderWork { job, meta, opts: o.clone() }))
    } else {
        let lightcraft_catalog::Source::File { path } = &p.source else {
            return Err(format!("{} is a generated demo photo: it has no original file to export", p.file_name));
        };
        Work::File {
            path: path.clone(),
            read: session.media.file_bytes.clone(),
            packet: crate::sidecar::sidecar_packet(p, &session.catalog),
            dng: (o.format == ExportFormat::Dng).then_some(o.dng_compression),
            label: p.file_name.clone(),
            size: (p.width as usize, p.height as usize),
        }
    };
    Ok(PreparedExport { photo: id, file_name, work, guard })
}

impl PreparedExport {
    pub fn run(self) -> Result<Exported, String> {
        let file_name = self.file_name;
        match self.work {
            Work::Render(w) => {
                let RenderWork { job, meta, opts } = *w;
                let r = job.run().rendered?;
                let bytes = encode_rendered(&r, &opts, meta.as_ref())?;
                Ok(Exported { file_name, bytes, width: r.image.width, height: r.image.height, sidecars: Vec::new() })
            }
            Work::File { path, read, packet, dng, label, size } => {
                let bytes = match &read {
                    Some(r) => r(&path)?,
                    None => std::fs::read(&path).map_err(|e| format!("{path}: {e}"))?,
                };
                let Some(dng) = dng else {
                    return Ok(Exported { file_name, bytes, width: size.0, height: size.1, sidecars: vec![("xmp", packet.into_bytes())] });
                };
                if lightcraft_raw::probe(&bytes).is_none() {
                    return Err(format!("{label}: DNG export needs a raw photo"));
                }
                let raw = lightcraft_raw::decode(&bytes).map_err(|e| format!("{label}: {e}"))?;
                drop(bytes);
                let compression = match dng {
                    DngCompression::Lossless => lightcraft_raw::DngCompression::Lj92 { tile: 256 },
                    DngCompression::Deflate => lightcraft_raw::DngCompression::Deflate { tile: 256, half: false },
                    DngCompression::Uncompressed => lightcraft_raw::DngCompression::Uncompressed,
                };
                let dng = lightcraft_raw::write_dng(&raw, &lightcraft_raw::DngWriteOptions { xmp: Some(packet), compression, ..Default::default() })
                    .map_err(|e| e.to_string())?;
                Ok(Exported { file_name, bytes: dng, width: raw.width, height: raw.height, sidecars: Vec::new() })
            }
        }
    }
}

/// How an exported DNG stores its raw data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DngCompression {
    /// Lossless JPEG (the usual DNG compression).
    #[default]
    Lossless,
    /// Deflate (zip) tiles with prediction.
    Deflate,
    Uncompressed,
}

/// The destination of a batch: a folder (with `ExportOptions::subfolder` and the conflict policy
/// applied to each file together with its sidecars), or one exact file path (single photo; an
/// ordinary file already there is replaced). Either way a catalogued original (or its sidecar)
/// is never written over: see [`crate::originals`].
#[derive(Clone, Debug, Default)]
pub struct Destination {
    pub dir: String,
    pub exact: Option<String>,
}

/// Export `ids` in order ([`prepare_batch`] + [`run_batch`]), stopping at the first error.
pub fn export_batch(
    session: &mut crate::Session,
    ids: &[lightcraft_catalog::PhotoId],
    o: &ExportOptions,
    to: &Destination,
    write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>,
    exists: &dyn Fn(&str) -> bool,
) -> Result<Vec<serde_json::Value>, String> {
    let items = prepare_batch(session, ids, o)?;
    run_batch(items, o, to, write, exists, true, &mut |_, _| true)
}

/// Write an exported or rendered file on disk: its folder is created if needed, and the file is
/// replaced atomically ([`lightcraft_catalog::safe_file::write_atomic_nosync`]: a temp file
/// renamed into place), so a failure part-way leaves any previous file intact and no truncated
/// one. Not synced to disk (issue #134): an export can always be made again from the original.
/// The native writer behind exports, renders and screenshots (app, CLI, MCP).
pub fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    write_with(path, bytes, false)
}

/// [`write_file`], synced to disk: for a written file that becomes a library photo (Edit a Copy)
/// and so can't simply be exported again once the catalog points at it.
pub fn write_file_durable(path: &str, bytes: &[u8]) -> Result<(), String> {
    write_with(path, bytes, true)
}

fn write_with(path: &str, bytes: &[u8], durable: bool) -> Result<(), String> {
    use lightcraft_catalog::safe_file::{write_atomic, write_atomic_nosync};
    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if durable { write_atomic(p, bytes) } else { write_atomic_nosync(p, bytes) }.map_err(|e| format!("{path}: {e}"))
}

/// The path of the sidecar with extension `ext` of the exported file `main`.
fn sidecar_path(main: &str, ext: &str) -> String {
    std::path::Path::new(main).with_extension(ext).to_string_lossy().to_string()
}

/// Run prepared exports in order: pick each one's path, and hand the bytes (and sidecars) to
/// `write`. `exists` tells whether a path is taken. The conflict policy applies to a file and its
/// sidecars as one: with Unique both get the same free name, with Skip the photo is skipped when
/// either is taken. A path that is a catalogued original (or its sidecar) is refused whatever the
/// policy. `progress(done, next file)` is called before each photo; returning false cancels the
/// rest. Returns one JSON object per photo: `{path, width, height, bytes, sidecars}`,
/// `{skipped: path}` or (unless `stop_on_error`) `{photo, file, error}`.
pub fn run_batch(
    items: Vec<PreparedExport>,
    o: &ExportOptions,
    to: &Destination,
    write: &mut dyn FnMut(&str, &[u8]) -> Result<(), String>,
    exists: &dyn Fn(&str) -> bool,
    stop_on_error: bool,
    progress: &mut dyn FnMut(usize, &str) -> bool,
) -> Result<Vec<serde_json::Value>, String> {
    use serde_json::json;
    let join = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { format!("{}/{b}", a.trim_end_matches('/')) };
    let dir = if o.subfolder.is_empty() { to.dir.clone() } else { join(&to.dir, &o.subfolder) };
    let single = items.len() == 1;
    let mut taken = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (i, item) in items.into_iter().enumerate() {
        if !progress(i, &item.file_name) {
            break;
        }
        let (photo, name, guard) = (item.photo, item.file_name.clone(), item.guard.clone());
        let e = match item.run() {
            Ok(e) => e,
            Err(err) if stop_on_error => return Err(err),
            Err(err) => {
                out.push(json!({"photo": photo.0, "file": name, "error": err}));
                continue;
            }
        };
        // the exported file and its sidecars
        let group = |main: &str| std::iter::once(main.to_string()).chain(e.sidecars.iter().map(|(x, _)| sidecar_path(main, x))).collect::<Vec<_>>();
        let path = match to.exact.as_deref().filter(|_| single) {
            Some(p) => Ok(p.to_string()),
            None => {
                let path = join(&dir, &e.file_name);
                let busy = |main: &str| group(main).iter().any(|p| taken.contains(p) || exists(p));
                if !busy(&path) {
                    Ok(path)
                } else {
                    match o.conflict {
                        Conflict::Overwrite => Ok(path),
                        Conflict::Skip => {
                            out.push(json!({"skipped": path}));
                            continue;
                        }
                        Conflict::Unique => {
                            let (stem, ext) = e.file_name.rsplit_once('.').map_or((e.file_name.as_str(), None), |(a, b)| (a, Some(b)));
                            let name = |n: usize| join(&dir, &ext.map_or(format!("{stem}-{n}"), |x| format!("{stem}-{n}.{x}")));
                            (2..1_000_000).map(name).find(|p| !busy(p)).ok_or_else(|| format!("{path}: no free file name"))
                        }
                    }
                }
            }
        };
        let file = path.clone().unwrap_or_else(|_| name.clone());
        let written = path.and_then(|path| {
            let files = group(&path);
            // never over an original, whatever the conflict policy or the exact path said
            for f in &files {
                guard.check(std::path::Path::new(f))?;
            }
            write(&path, &e.bytes)?;
            let mut sidecars = Vec::new();
            for ((_, bytes), sc) in e.sidecars.iter().zip(files.iter().skip(1)) {
                write(sc, bytes)?;
                sidecars.push(sc.clone());
            }
            Ok((path, files, sidecars))
        });
        match written {
            Ok((path, files, sidecars)) => {
                taken.extend(files);
                out.push(json!({"path": path, "width": e.width, "height": e.height, "bytes": e.bytes.len(), "sidecars": sidecars}));
            }
            Err(err) if stop_on_error => return Err(err),
            Err(err) => out.push(json!({"photo": photo.0, "file": file, "error": err})),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_image() -> Rgba8 {
        let mut img = Rgba8::new(96, 64);
        for (i, p) in img.data.iter_mut().enumerate() {
            let (x, y) = (i % 96, i / 96);
            *p = [(x * 2) as u8, (y * 3) as u8, ((x * y) % 255) as u8, 255];
        }
        img
    }

    #[test]
    fn formats_have_magic() {
        let img = test_image();
        for (f, magic) in
            [(ExportFormat::Jpeg, &b"\xFF\xD8"[..]), (ExportFormat::Png, b"\x89PNG"), (ExportFormat::Tiff, b"II"), (ExportFormat::Webp, b"RIFF")]
        {
            let b = encode_image(&img, &ExportOptions { format: f, ..Default::default() }).unwrap();
            assert!(b.starts_with(magic), "{f:?}");
        }
    }

    #[test]
    fn size_limit_respected() {
        let img = test_image();
        let full = encode_image(&img, &ExportOptions { quality: 100, ..Default::default() }).unwrap();
        let kb = (full.len() / 1024 / 2).max(2) as u32;
        let b = encode_image(&img, &ExportOptions { quality: 100, limit_kb: Some(kb), ..Default::default() }).unwrap();
        assert!(b.len() <= kb as usize * 1024);
    }

    #[test]
    fn sharpen_increases_edge_contrast() {
        let mut img = Rgba8::new(8, 8);
        for (i, p) in img.data.iter_mut().enumerate() {
            let v = if i % 8 < 4 { 80 } else { 170 };
            *p = [v, v, v, 255];
        }
        output_sharpen(&mut img, SharpenFor::Matte, SharpenAmount::High);
        assert!(img.data[3][0] < 80 && img.data[4][0] > 170);
    }

    #[test]
    fn metadata_policies() {
        use lightcraft_catalog::{Photo, PhotoId, Source};
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "a.jpg", "jpeg", 10, 10, "2026-09-30T00:00:00");
        p.meta.camera = "Synthetic X2".into();
        p.meta.copyright = "(c) Me".into();
        p.meta.copyright_status = lightcraft_catalog::CopyrightStatus::Copyrighted;
        p.meta.usage_terms = "Editorial use only".into();
        p.meta.copyright_url = "https://example.com/rights".into();
        p.meta.shutter = "1/250".into();
        p.meta.gps = Some((43.0, -110.0));
        let all = export_metadata(&p, &ExportOptions::default()).unwrap();
        assert_eq!(all.model.as_deref(), Some("Synthetic X2"));
        assert!((all.exposure_time.unwrap() - 0.004).abs() < 1e-9);
        assert!(all.gps.is_some());
        let o = ExportOptions { metadata: MetadataPolicy::AllExceptCamera, remove_location: true, ..Default::default() };
        let m = export_metadata(&p, &o).unwrap();
        assert!(m.model.is_none() && m.gps.is_none() && m.copyright.is_some());
        let c = export_metadata(&p, &ExportOptions { metadata: MetadataPolicy::Copyright, ..Default::default() }).unwrap();
        assert!(c.model.is_none() && c.gps.is_none() && c.copyright.as_deref() == Some("(c) Me"));
        // "copyright only" keeps all the copyright info: status, usage terms, info URL
        assert_eq!(
            (c.copyright_marked, c.usage_terms.as_deref(), c.copyright_url.as_deref()),
            (Some(true), Some("Editorial use only"), Some("https://example.com/rights"))
        );
        assert!(export_metadata(&p, &ExportOptions { metadata: MetadataPolicy::None, ..Default::default() }).is_none());
        // embedded and readable back from the JPEG
        let jpg = encode_with_metadata(&test_image(), &ExportOptions::default(), Some(&all)).unwrap();
        let back = lightcraft_meta::extract(&jpg);
        assert_eq!(back.model.as_deref(), Some("Synthetic X2"));
        assert_eq!(back.copyright.as_deref(), Some("(c) Me"));
        assert_eq!((back.copyright_marked, back.usage_terms.as_deref()), (Some(true), Some("Editorial use only")));
        assert_eq!(back.copyright_url.as_deref(), Some("https://example.com/rights"));
        assert!(back.gps.is_some());
    }

    #[test]
    fn watermark_draws_in_the_anchored_corner_only() {
        let mut img = Rgba8::new(400, 300);
        for p in img.data.iter_mut() {
            *p = [0, 0, 0, 255];
        }
        let o = ExportOptions::from_json(&serde_json::json!({"watermark": {"text": "LightCraft", "size": 0.08, "opacity": 1.0, "shadow": false}}));
        let wm = o.watermark.clone().unwrap();
        draw_watermark(&mut img, &wm);
        let lit = |x0: usize, x1: usize, y0: usize, y1: usize| {
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|&(x, y)| img.get(x, y)[0] > 128).count()
        };
        assert!(lit(200, 400, 225, 300) > 100, "bottom-right has text");
        assert!(lit(0, 200, 0, 150) == 0, "top-left untouched");
        // string shorthand
        assert_eq!(ExportOptions::from_json(&serde_json::json!({"watermark": "© Me"})).watermark.unwrap().text, "© Me");
        assert!(ExportOptions::from_json(&serde_json::json!({"watermark": ""})).watermark.is_none());
    }

    fn vertical_japanese() -> Watermark {
        let options =
            ExportOptions::from_json(&json!({"watermark": {"text": "日本語", "vertical": true, "size": 0.1, "shadow": false, "opacity": 1.0}}));
        options.watermark.unwrap()
    }

    #[test]
    fn japanese_watermark_options_and_legacy_defaults() {
        let old = ExportOptions::from_json(&json!({"watermark": {"text": "日本語"}}));
        assert!(!old.watermark.unwrap().vertical);
        let wm = vertical_japanese();
        assert!(wm.vertical);
        let round: Watermark = serde_json::from_value(serde_json::to_value(&wm).unwrap()).unwrap();
        assert!(round.vertical);
    }

    /// Built without craft-fonts, a Japanese watermark still lays out and draws (Inter's
    /// missing-glyph boxes), and Latin text is unaffected.
    #[test]
    fn watermarks_work_without_craft_fonts() {
        assert_eq!(watermark_fonts(&[]).len(), 1, "Inter only");
        for wm in [vertical_japanese(), Watermark { text: "LightCraft 日本語".into(), ..Watermark::default() }] {
            let mut covered = 0usize;
            watermark_coverage(400, 300, &wm, &[], |_, _, k, _| covered += usize::from(k > 0.0));
            assert!(covered > 0, "{:?} draws something", wm.text);
        }
    }

    #[test]
    fn japanese_watermarks_support_vertical_columns() {
        use ab_glyph::Font;
        let fonts = watermark_fonts(crate::fonts::CRAFT_FONTS);
        if fonts.len() < 2 {
            eprintln!("skipped: built without CRAFT_FONTS_DIR, so there is no Japanese watermark face");
            return;
        }
        for c in "日本語の文字".chars() {
            assert!(fonts[1..].iter().any(|f| f.glyph_id(c).0 != 0), "a craft-fonts face has {c}");
        }
        let wm = vertical_japanese();
        let mut img = Rgba8::new(400, 300);
        img.data.fill([0, 0, 0, 255]);
        draw_watermark(&mut img, &wm);
        let lit: Vec<_> = (0..300).flat_map(|y| (0..400).map(move |x| (x, y))).filter(|&(x, y)| img.get(x, y)[0] > 64).collect();
        assert!(lit.len() > 100);
        let w = lit.iter().map(|p| p.0).max().unwrap() - lit.iter().map(|p| p.0).min().unwrap();
        let h = lit.iter().map(|p| p.1).max().unwrap() - lit.iter().map(|p| p.1).min().unwrap();
        assert!(h > w * 2, "vertical Japanese must extend down the column");
        let two = Watermark { text: "日\n本".into(), anchor: Anchor::TopLeft, inset: 0.0, ..wm };
        img.data.fill([0, 0, 0, 255]);
        draw_watermark(&mut img, &two);
        for (text, dx) in [("本", 0), ("日", 30)] {
            let mut single = Rgba8::new(400, 300);
            single.data.fill([0, 0, 0, 255]);
            draw_watermark(&mut single, &Watermark { text: text.into(), ..two.clone() });
            for y in 0..30 {
                for x in 0..30 {
                    assert_eq!(img.get(x + dx, y), single.get(x, y), "newlines move columns left");
                }
            }
        }
    }

    #[test]
    fn graphic_watermark_is_placed_scaled_and_blended() {
        let dir = std::env::temp_dir().join(format!("lc-wm-logo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logo_path = dir.join("logo.png");
        // 40×20: left half opaque red, right half fully transparent
        let logo = Rgba8::from_fn(40, 20, |x, _| if x < 20 { [255, 0, 0, 255] } else { [0, 255, 0, 0] });
        let png = lightcraft_codecs::encode_png(&EncodeImage::rgba8(&logo), &EncodeMeta::default()).unwrap();
        std::fs::write(&logo_path, png).unwrap();
        let mut img = Rgba8::new(400, 300);
        img.data.iter_mut().for_each(|p| *p = [0, 0, 0, 255]);
        let wm = Watermark { image: logo_path.to_string_lossy().into(), image_width: 0.25, opacity: 1.0, inset: 0.0, ..Default::default() };
        draw_watermark(&mut img, &wm);
        // 100 px wide × 50 px high in the bottom-right corner
        assert!(img.get(310, 280)[0] > 200, "opaque half: {:?}", img.get(310, 280));
        assert_eq!(img.get(390, 280), [0, 0, 0, 255], "transparent half leaves the photo");
        assert_eq!(img.get(310, 240), [0, 0, 0, 255], "above the logo");
        assert_eq!(img.get(10, 10), [0, 0, 0, 255]);
        // an image-only watermark survives the params (no text needed)
        let o = ExportOptions::from_json(&json!({"watermark": {"image": logo_path.to_string_lossy(), "imageWidth": 0.1}}));
        assert_eq!(o.watermark.as_ref().map(|w| w.image_width), Some(0.1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn naming_and_params() {
        let o = ExportOptions::from_json(&serde_json::json!({"format": "jpg", "quality": 150, "longEdge": 2048, "naming": "{name}-{seq}"}));
        assert_eq!(o.format, ExportFormat::Jpeg);
        assert_eq!(o.quality, 100);
        assert_eq!(o.resize, Some(Resize { dont_enlarge: true, height: 0, ..Resize::long_edge(2048) }));
        assert_eq!(o.file_name_for(&named("IMG/1.png"), 7), "IMG_1-007.jpg");
        assert_eq!(o.ppi, 240);
        let orig = ExportOptions { format: ExportFormat::Original, naming: "{name}-{seq}".into(), ..Default::default() };
        assert_eq!(orig.file_name_for(&named("DSC_1.NEF"), 2), "DSC_1-002.NEF");
        assert_eq!(orig.file_name_for(&named("noext"), 1), "noext-001");
        assert_eq!(ExportOptions { format: ExportFormat::Dng, ..Default::default() }.file_name_for(&named("a.cr2"), 1), "a.dng");
    }

    fn named(file_name: &str) -> lightcraft_catalog::Photo {
        use lightcraft_catalog::{Photo, PhotoId, Source};
        Photo::new(PhotoId(1), Source::Demo { scene: 0 }, file_name, "", 1, 1, "2026-01-01T00:00:00")
    }

    /// Export naming takes the same tokens as Rename Photos and import renaming.
    #[test]
    fn naming_uses_the_rename_tokens() {
        let mut p = named("DSC_0815.NEF");
        p.captured = Some("2026-09-30T14:05:09".into());
        p.meta.camera = "Model X/2".into();
        p.meta.title = "Harbour".into();
        p.rating = 5;
        let o = |naming: &str| ExportOptions { naming: naming.into(), start_number: 9, ..Default::default() };
        assert_eq!(o("{date:%Y-%m-%d}_{title}_{seq:2}").file_name_for(&p, 1), "2026-09-30_Harbour_09.jpg");
        assert_eq!(o("{camera}-{num}-{rating}").file_name_for(&p, 1), "Model X_2-0815-5.jpg", "path separators become _");
        assert_eq!(o("{name}-{seq}").file_name_for(&p, 3), "DSC_0815-011.jpg", "a bare {{seq}} keeps its 3 digits");
        assert_eq!(o("{ext}_{name}").file_name_for(&p, 1), "NEF_DSC_0815.jpg");
        // nothing left after expanding: the original name
        p.meta.title.clear();
        assert_eq!(o("{title}").file_name_for(&p, 1), "DSC_0815.jpg");
    }

    #[test]
    fn params_round_trip() {
        let o = ExportOptions {
            format: ExportFormat::Tiff,
            quality: 70,
            resize: Some(Resize { mode: ResizeMode::Megapixels, value: 12.5, height: 0, dont_enlarge: false }),
            ppi: 300,
            limit_kb: Some(500),
            sharpen: SharpenFor::Glossy,
            naming: "{date}-{name}-{seq}".into(),
            start_number: 42,
            subfolder: "Client".into(),
            conflict: Conflict::Skip,
            tiff_compression: TiffCompression::Lzw,
            metadata: MetadataPolicy::Copyright,
            remove_location: true,
            watermark: Some(Watermark { text: "© LC".into(), ..Default::default() }),
            color_space: OutputSpace::ProPhoto,
            bit_depth: Some(16),
            ..Default::default()
        };
        assert_eq!(ExportOptions::from_json(&o.to_json()), o);
        let full = ExportOptions::default();
        assert_eq!(ExportOptions::from_json(&full.to_json()), full);
        assert!(ExportOptions::has_size_param(&full.to_json()), "full size is explicit");
        let mut p = named("IMG.png");
        p.captured = Some("2026-09-30T10:00:00".into());
        assert_eq!(o.file_name_for(&p, 2), "20260930-IMG-043.tif");
    }

    #[test]
    fn resize_modes() {
        let size = |p: serde_json::Value, w: f64, h: f64| ExportOptions::from_json(&p).resize.map(|r| r.apply(w, h));
        // a 6000 × 4000 landscape and a 4000 × 6000 portrait
        assert_eq!(size(json!({"longEdge": 1500}), 6000.0, 4000.0), Some((1500, 1000)));
        assert_eq!(size(json!({"longEdge": 1500}), 4000.0, 6000.0), Some((1000, 1500)));
        assert_eq!(size(json!({"shortEdge": 1080}), 6000.0, 4000.0), Some((1620, 1080)));
        assert_eq!(size(json!({"width": 3000}), 4000.0, 6000.0), Some((3000, 4500)));
        assert_eq!(size(json!({"height": 1000}), 6000.0, 4000.0), Some((1500, 1000)));
        // W × H fits either orientation: the long edge gets the larger number
        assert_eq!(size(json!({"width": 1000, "height": 800}), 6000.0, 4000.0), Some((1000, 667)));
        assert_eq!(size(json!({"width": 1000, "height": 800}), 4000.0, 6000.0), Some((667, 1000)));
        assert_eq!(size(json!({"width": 1000, "height": 500}), 6000.0, 4000.0), Some((750, 500)));
        let (w, h) = size(json!({"megapixels": 6}), 6000.0, 4000.0).unwrap();
        assert!(((w * h) as f64 - 6e6).abs() < 6e6 * 0.002, "{w}×{h}");
        assert_eq!(size(json!({"percent": 25}), 6000.0, 4000.0), Some((1500, 1000)));
        // don't enlarge (default) vs enlarge
        assert_eq!(size(json!({"longEdge": 8000}), 6000.0, 4000.0), Some((6000, 4000)));
        assert_eq!(size(json!({"longEdge": 9000, "dontEnlarge": false}), 6000.0, 4000.0), Some((9000, 6000)));
        // full size / absent
        assert_eq!(size(json!({"longEdge": 0}), 6000.0, 4000.0), None);
        assert_eq!(size(json!({}), 6000.0, 4000.0), None);
        assert!(ExportOptions::has_size_param(&json!({"longEdge": 0})) && !ExportOptions::has_size_param(&json!({"quality": 3})));
        // the `resize` object form round-trips (what the dialog and prefs store)
        let r = Resize { mode: ResizeMode::ShortEdge, value: 720.0, height: 0, dont_enlarge: false };
        let back = ExportOptions::from_json(&ExportOptions::resize_json(Some(&r))).resize;
        assert_eq!(back, Some(r));
        assert_eq!(ExportOptions::from_json(&ExportOptions::resize_json(None)).resize, None);
        assert_eq!(ExportOptions::from_json(&json!({"ppi": 300})).ppi, 300);
        // never larger than the encoders' limit, never empty
        assert_eq!(size(json!({"percent": 10000, "dontEnlarge": false}), 6000.0, 4000.0), Some((65535, 43690)));
        assert_eq!(size(json!({"width": 1}), 6000.0, 40.0).map(|s| s.1), Some(1));
    }

    /// A flat field of a saturated green inside Display P3 but outside sRGB, rendered into `space`.
    fn p3_green(space: OutputSpace) -> Rgba8 {
        use lightcraft_color::{DISPLAY_P3, REC2020};
        let c = DISPLAY_P3.to_space(&REC2020).apply_f32([0.04, 0.45, 0.04]);
        let src = lightcraft_raster::Rgb32f::filled(16, 16, c);
        let req = lightcraft_pipeline::RenderRequest { space, ..lightcraft_pipeline::RenderRequest::fit(16, 16) };
        lightcraft_pipeline::render(&src, &Default::default(), &Default::default(), &req).image
    }

    /// Decode `bytes` and return the centre pixel in linear sRGB primaries (unclamped), plus the
    /// recognised space of the embedded profile.
    fn decoded_in_srgb(bytes: &[u8]) -> ([f32; 3], Option<NamedSpace>) {
        let d = lightcraft_codecs::decode(bytes, Default::default()).expect("decodes");
        let m = d.space.to_space(&lightcraft_color::SRGB);
        (m.apply_f32(d.image.get(8, 8)), d.space.named)
    }

    #[test]
    fn p3_colour_survives_a_p3_export_and_is_clipped_in_srgb() {
        let o = |space| ExportOptions { format: ExportFormat::Png, color_space: space, ..Default::default() };
        let (p3, named) = decoded_in_srgb(&encode_image(&p3_green(OutputSpace::DisplayP3), &o(OutputSpace::DisplayP3)).unwrap());
        assert_eq!(named, Some(NamedSpace::DisplayP3));
        assert!(p3[0] < -0.03 || p3[2] < -0.03, "outside sRGB after a P3 round trip: {p3:?}");
        let (s, named) = decoded_in_srgb(&encode_image(&p3_green(OutputSpace::Srgb), &o(OutputSpace::Srgb)).unwrap());
        assert_eq!(named, Some(NamedSpace::Srgb));
        assert!(s.iter().all(|v| *v > -0.002), "{s:?}");
        // the green itself is about the same brightness either way
        assert!((p3[1] - s[1]).abs() < 0.15, "{p3:?} {s:?}");
    }

    #[test]
    fn every_space_embeds_its_own_profile_and_round_trips() {
        let grey = {
            let src = lightcraft_raster::Rgb32f::filled(16, 16, [0.18; 3]);
            let base = lightcraft_pipeline::RenderRequest::fit(16, 16);
            move |space| {
                lightcraft_pipeline::render(&src, &Default::default(), &Default::default(), &lightcraft_pipeline::RenderRequest { space, ..base })
                    .image
            }
        };
        let reference =
            decoded_in_srgb(&encode_image(&grey(OutputSpace::Srgb), &ExportOptions { format: ExportFormat::Tiff, ..Default::default() }).unwrap()).0;
        for space in OutputSpace::ALL {
            for format in [ExportFormat::Jpeg, ExportFormat::Png, ExportFormat::Tiff, ExportFormat::Webp] {
                let o = ExportOptions { format, color_space: space, ..Default::default() };
                let bytes = encode_image(&grey(space), &o).unwrap();
                let (px, named) = decoded_in_srgb(&bytes);
                assert_eq!(named, Some(named_space(space)), "{space:?} {format:?}");
                // a neutral grey decodes to the same linear value whatever the output space
                for c in 0..3 {
                    assert!((px[c] - reference[c]).abs() < 0.01, "{space:?} {format:?}: {px:?} vs {reference:?}");
                }
            }
            let icc = icc::write_named(named_space(space));
            let info = icc::parse(&icc).expect("our profile parses");
            assert_eq!(info.named, Some(named_space(space)));
            assert!(info.description.as_deref().is_some_and(|d| !d.is_empty()));
        }
        let o = ExportOptions { format: ExportFormat::Avif, color_space: OutputSpace::ProPhoto, ..Default::default() };
        assert_eq!(o.effective_space(), OutputSpace::Srgb);
        let o = ExportOptions::from_json(&serde_json::json!({"colorSpace": "displayP3"}));
        assert_eq!(o.color_space, OutputSpace::DisplayP3);
        assert_eq!(ExportOptions::from_json(&serde_json::json!({"colorSpace": "Adobe RGB (1998) compatible"})).color_space, OutputSpace::AdobeRgb);
    }

    #[test]
    fn srgb_watermark_colour_is_converted() {
        assert_eq!(srgb8_in(OutputSpace::Srgb, [10, 200, 30]), [10, 200, 30]);
        for s in OutputSpace::ALL {
            assert_eq!(srgb8_in(s, [255, 255, 255]), [255, 255, 255], "{s:?}");
            assert_eq!(srgb8_in(s, [0, 0, 0]), [0, 0, 0], "{s:?}");
        }
        let p3 = srgb8_in(OutputSpace::DisplayP3, [255, 0, 0]);
        assert!(p3[0] < 255 && p3[1] > 0, "{p3:?}");
    }

    /// A shallow horizontal grey ramp (few 8-bit levels), rendered at `depth`.
    fn ramp(depth: OutputDepth, space: OutputSpace) -> lightcraft_pipeline::Rendered {
        let src = lightcraft_raster::Rgb32f::from_fn(1024, 4, |x, _| [0.10 + 0.03 * x as f32 / 1023.0; 3]);
        let req = lightcraft_pipeline::RenderRequest { depth, space, ..lightcraft_pipeline::RenderRequest::fit(1024, 4) };
        lightcraft_pipeline::render(&src, &Default::default(), &Default::default(), &req)
    }

    fn distinct_levels(bytes: &[u8]) -> (usize, lightcraft_codecs::Decoded) {
        let d = lightcraft_codecs::decode(bytes, Default::default()).expect("decodes");
        let mut v: Vec<u32> = (0..d.image.width).map(|x| d.image.get(x, 1)[1].to_bits()).collect();
        v.dedup();
        (v.len(), d)
    }

    #[test]
    fn sixteen_bit_tiff_and_png_have_no_banding() {
        let tiff = |bit_depth| ExportOptions { format: ExportFormat::Tiff, bit_depth, ..Default::default() };
        assert_eq!(tiff(None).effective_depth(), OutputDepth::U16, "TIFF defaults to 16-bit");
        let r16 = ramp(OutputDepth::U16, OutputSpace::Srgb);
        assert!(matches!(r16.deep.as_ref().unwrap().samples, DeepSamples::U16(_)));
        let (levels16, d16) = distinct_levels(&encode_rendered(&r16, &tiff(None), None).unwrap());
        let r8 = ramp(OutputDepth::U8, OutputSpace::Srgb);
        let (levels8, _) = distinct_levels(&encode_rendered(&r8, &tiff(Some(8)), None).unwrap());
        eprintln!("gradient levels: 16-bit TIFF {levels16}, 8-bit TIFF {levels8}");
        assert!(levels8 < 40, "{levels8}");
        assert!(levels16 > 10 * levels8, "16-bit: {levels16} levels vs 8-bit: {levels8}");
        assert_eq!(d16.space.named, Some(NamedSpace::Srgb));
        // the 8-bit preview of a deep render matches the 8-bit render
        for (a, b) in r16.image.data.iter().zip(&r8.image.data) {
            assert!(a[1].abs_diff(b[1]) <= 1, "{a:?} {b:?}");
        }
        let png = ExportOptions { format: ExportFormat::Png, bit_depth: Some(16), color_space: OutputSpace::DisplayP3, ..Default::default() };
        let rp = ramp(OutputDepth::U16, OutputSpace::DisplayP3);
        let bytes = encode_rendered(&rp, &png, None).unwrap();
        assert_eq!(bytes[24], 16, "PNG IHDR bit depth");
        let (levels, d) = distinct_levels(&bytes);
        assert!(levels > 10 * levels8, "{levels}");
        assert_eq!(d.space.named, Some(NamedSpace::DisplayP3));
        // sharpening and watermarking work on deep images too
        let o = ExportOptions {
            sharpen: SharpenFor::Matte,
            watermark: Some(Watermark { text: "LC".into(), size: 0.5, ..Default::default() }),
            ..tiff(None)
        };
        assert!(encode_rendered(&r16, &o, None).is_ok());
    }

    #[test]
    fn float_tiff_is_linear_with_a_linear_profile() {
        let o = ExportOptions { format: ExportFormat::Tiff, bit_depth: Some(32), color_space: OutputSpace::ProPhoto, ..Default::default() };
        assert_eq!(o.effective_depth(), OutputDepth::F32Linear);
        let rf = ramp(OutputDepth::F32Linear, OutputSpace::ProPhoto);
        let (levels, df) = distinct_levels(&encode_rendered(&rf, &o, None).unwrap());
        assert!(levels > 500, "{levels}");
        assert_eq!(df.space.named, Some(NamedSpace::ProPhoto));
        assert!(df.space.trc.as_ref().is_some_and(|t| t[0].is_linear()), "{:?}", df.space.trc);
        // decodes to the same light as the 16-bit gamma-encoded export
        let o16 = ExportOptions { bit_depth: Some(16), ..o.clone() };
        let (_, d16) = distinct_levels(&encode_rendered(&ramp(OutputDepth::U16, OutputSpace::ProPhoto), &o16, None).unwrap());
        for x in [0, 300, 700, 1023] {
            let (a, b) = (df.image.get(x, 1)[1], d16.image.get(x, 1)[1]);
            assert!((a - b).abs() < 2e-4, "{x}: {a} vs {b}");
        }
    }

    #[test]
    fn bit_depth_options_per_format() {
        let d = |format, bit_depth| ExportOptions { format, bit_depth, ..Default::default() }.effective_depth();
        assert_eq!(d(ExportFormat::Jpeg, Some(16)), OutputDepth::U8);
        assert_eq!(d(ExportFormat::Webp, None), OutputDepth::U8);
        assert_eq!(d(ExportFormat::Png, None), OutputDepth::U8);
        assert_eq!(d(ExportFormat::Png, Some(16)), OutputDepth::U16);
        assert_eq!(d(ExportFormat::Avif, Some(10)), OutputDepth::U16);
        assert_eq!(ExportOptions::from_json(&serde_json::json!({"bitDepth": 16})).bit_depth, Some(16));
        assert_eq!(ExportOptions::from_json(&serde_json::json!({"bitDepth": 12})).bit_depth, None);
        for f in [ExportFormat::Jpeg, ExportFormat::Png, ExportFormat::Tiff, ExportFormat::Webp, ExportFormat::Avif] {
            let first = ExportOptions::bit_depths(f)[0].0;
            let def = d(f, None);
            assert_eq!(d(f, Some(first)), def, "{f:?}: the first choice is the default");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn avif_ten_bit() {
        let o = ExportOptions { format: ExportFormat::Avif, bit_depth: Some(10), quality: 60, ..Default::default() };
        let r = ramp(OutputDepth::U16, OutputSpace::Srgb);
        match encode_rendered(&r, &o, None) {
            Ok(b) => assert_eq!(&b[4..8], b"ftyp"),
            Err(e) => assert!(e.contains("not available"), "{e}"),
        }
    }
}
