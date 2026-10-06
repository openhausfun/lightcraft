//! Standard image codecs for LightCraft (layer L1).
//!
//! - [`sniff`] detects the container format (and flags camera raws for `lightcraft-raw`).
//! - [`decode`] decodes JPEG, PNG, TIFF, WebP, GIF, BMP, PSD (merged composite) and JPEG XL into
//!   **linear-light** [`Rgb32f`] in the source's own primaries, described by [`SourceSpace`]; ICC
//!   profiles (matrix/TRC exactly, LUT/CMYK via the `moxcms` CMS), 16-bit and float precision are
//!   preserved. EXIF/XMP/ICC blobs are passed through untouched; orientation is reported, never applied.
//! - [`to_working`] converts a decode result to linear Rec.2020 D65 (the pipeline working space).
//! - [`decode_thumbnail`] is the fast path for grid thumbnails (EXIF thumbnail or DCT-scaled decode).
//! - [`encode`] writes JPEG, PNG, TIFF, lossless WebP and (native, feature `avif`) AVIF, embedding
//!   ICC/EXIF/XMP. [`icc::write_matrix_trc`] builds profiles for export.
//!
//! No decoder panics on malformed input (property-tested).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod convert;
pub mod encode;
pub mod exif;
pub mod icc;
mod jpeg;
pub mod jpeg_par;
#[cfg(feature = "jxl")]
mod jxl;
mod other;
mod png_codec;
mod psd;
mod sniff;
pub mod space;
mod tiff_codec;
mod webp;

pub use encode::{
    ChromaSubsampling, EncodeImage, EncodeMeta, Samples, TiffCompression, encode_avif, encode_jpeg, encode_png, encode_tiff, encode_webp_lossless,
};
pub use sniff::{Format, sniff};
pub use space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};

use lightcraft_raster::{Plane, Rgb32f, Rgba8};
use serde::{Deserialize, Serialize};

/// Codec errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unrecognised image format")]
    UnknownFormat,
    #[error("{0:?} decoding is not supported: {1}")]
    Unsupported(Format, &'static str),
    #[error("malformed {0:?} data: {1}")]
    Malformed(Format, String),
    #[error("image too large: {0}×{1}")]
    TooLarge(u64, u64),
    #[error("encode error: {0}")]
    Encode(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Decode options.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DecodeOptions {
    /// Fit the result within this box (never upscales). JPEG uses DCT-domain scaling (1/2, 1/4,
    /// 1/8) before a final linear-light resample; other formats decode fully then resample.
    pub max_size: Option<(u32, u32)>,
    /// Refuse images with more pixels than this (default 1 Gpx).
    pub max_pixels: u64,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        DecodeOptions { max_size: None, max_pixels: 1 << 30 }
    }
}

impl DecodeOptions {
    pub fn fit(w: u32, h: u32) -> Self {
        DecodeOptions { max_size: Some((w, h)), ..Default::default() }
    }
}

/// A decoded image.
#[derive(Clone, Debug)]
pub struct Decoded {
    pub format: Format,
    /// Linear light, in the primaries described by [`Decoded::space`].
    pub image: Rgb32f,
    /// Straight (unassociated) alpha 0..1, when the source has transparency.
    pub alpha: Option<Plane>,
    pub space: SourceSpace,
    /// Bits per sample in the file (8, 16, 32, …).
    pub bit_depth: u8,
    /// Sample format in the file was floating point.
    pub float: bool,
    pub has_alpha: bool,
    /// The source is grayscale (expanded to RGB in `image`).
    pub grayscale: bool,
    /// The source was CMYK (converted).
    pub cmyk: bool,
    /// EXIF orientation value 1..=8 (1 when absent). **Not applied** to `image`.
    pub orientation: u16,
    /// TIFF-structured EXIF bytes (starting at the `II`/`MM` byte-order mark), if embedded.
    /// For TIFF files the file itself is the TIFF structure and this is `None`.
    pub exif: Option<Vec<u8>>,
    pub xmp: Option<String>,
    pub icc: Option<Vec<u8>>,
    /// Dimensions of `image`.
    pub width: u32,
    pub height: u32,
    /// Full-resolution dimensions of the source (differs from `width`/`height` when scaled).
    pub source_width: u32,
    pub source_height: u32,
}

impl Decoded {
    /// Pixels converted to linear Rec.2020 D65.
    pub fn to_working(&self) -> Rgb32f {
        to_working(self)
    }

    /// Display-encoded 8-bit sRGB (gamut clipped), with alpha.
    pub fn to_srgb8(&self) -> Rgba8 {
        let m = self.space.to_space(&lightcraft_color::SRGB);
        let ident = m == lightcraft_color::Mat3::IDENTITY;
        let m = m.to_f32();
        let enc = lightcraft_color::transfer::encode_srgb8;
        let mut out = Rgba8::new(self.image.width, self.image.height);
        let alpha = self.alpha.as_ref();
        lightcraft_raster::par_rows(&mut out.data, self.image.width, |y, row| {
            let src = self.image.row(y);
            let a = alpha.map(|a| a.row(y));
            for (x, (o, p)) in row.iter_mut().zip(src).enumerate() {
                let q = if ident { *p } else { mat_apply(&m, *p) };
                let al = a.map(|a| (a[x].clamp(0.0, 1.0) * 255.0 + 0.5) as u8).unwrap_or(255);
                *o = [enc(q[0]), enc(q[1]), enc(q[2]), al];
            }
        });
        out
    }
}

#[inline]
pub(crate) fn mat_apply(m: &[[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2],
    ]
}

/// Decode any supported format (see [`Format::can_decode`]).
///
/// Malformed input yields an error, never a panic: on unwinding targets a panic inside a third-party
/// decoder is caught and reported as [`Error::Malformed`].
pub fn decode(bytes: &[u8], opts: DecodeOptions) -> Result<Decoded> {
    let format = sniff(bytes).ok_or(Error::UnknownFormat)?;
    std::panic::catch_unwind(|| decode_unguarded(bytes, format, &opts)).unwrap_or_else(|_| Err(Error::Malformed(format, "decoder panicked".into())))
}

/// [`decode`] without the panic guard (for fuzzing our own code paths).
#[doc(hidden)]
pub fn decode_unguarded(bytes: &[u8], format: Format, opts: &DecodeOptions) -> Result<Decoded> {
    let opts = *opts;
    match format {
        Format::Jpeg => jpeg::decode(bytes, &opts),
        Format::Png => png_codec::decode(bytes, &opts),
        Format::Tiff => tiff_codec::decode(bytes, &opts),
        Format::WebP => webp::decode(bytes, &opts),
        Format::Gif | Format::Bmp => other::decode(bytes, format, &opts),
        Format::Psd => psd::decode(bytes, &opts),
        #[cfg(feature = "jxl")]
        Format::Jxl => jxl::decode(bytes, &opts),
        #[cfg(not(feature = "jxl"))]
        Format::Jxl => Err(Error::Unsupported(format, "built without the `jxl` feature")),
        Format::Avif => Err(Error::Unsupported(format, "no pure-Rust, permissively licensed AV1 decoder yet")),
        Format::Heif => Err(Error::Unsupported(format, "no pure-Rust, permissively licensed HEVC decoder yet")),
        Format::RawTiffLike | Format::RawOther => Err(Error::Unsupported(format, "camera raw: decode with lightcraft-raw")),
    }
}

/// Convert a decode result to linear Rec.2020 D65 (Bradford-adapted), the develop working space.
pub fn to_working(d: &Decoded) -> Rgb32f {
    if d.space.is_working() {
        return d.image.clone();
    }
    let m = d.space.to_working().to_f32();
    d.image.map(|p| mat_apply(&m, p))
}

/// Where a thumbnail came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThumbnailSource {
    /// The JPEG thumbnail embedded in EXIF (IFD1).
    ExifThumbnail,
    /// An MPF (CIPA DC-007) preview image embedded in a JPEG.
    MpfPreview,
    /// A (DCT-)scaled decode of the main image.
    Scaled,
}

/// A small display-ready image.
#[derive(Clone, Debug)]
pub struct Thumbnail {
    /// 8-bit sRGB with alpha, fitted within `max_edge` (orientation **not** applied).
    pub image: Rgba8,
    pub orientation: u16,
    pub source: ThumbnailSource,
    pub source_width: u32,
    pub source_height: u32,
}

/// Thumbnail request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThumbnailOptions {
    /// Fit the result within `max_edge × max_edge`.
    pub max_edge: u32,
    /// Accept an embedded preview whose long edge is at least this (it is then returned at its own,
    /// smaller size). Defaults to `max_edge`, i.e. only previews that need no upscaling. Lower it
    /// (e.g. 160 for typical EXIF thumbnails) for an instant placeholder.
    pub min_embedded_edge: u32,
}

impl ThumbnailOptions {
    pub fn new(max_edge: u32) -> Self {
        ThumbnailOptions { max_edge, min_embedded_edge: max_edge }
    }
}

/// Fast thumbnail: an embedded JPEG preview (EXIF IFD1 thumbnail or MPF preview) at least `max_edge`
/// on its long side if one exists, else a scaled decode. The result fits within `max_edge × max_edge`.
pub fn decode_thumbnail(bytes: &[u8], max_edge: u32) -> Result<Thumbnail> {
    decode_thumbnail_with(bytes, &ThumbnailOptions::new(max_edge))
}

/// [`decode_thumbnail`] with options.
pub fn decode_thumbnail_with(bytes: &[u8], opts: &ThumbnailOptions) -> Result<Thumbnail> {
    let max_edge = opts.max_edge.max(1);
    let min_edge = opts.min_embedded_edge.clamp(1, max_edge);
    if sniff(bytes) == Some(Format::Jpeg)
        && let Ok(Some(t)) = std::panic::catch_unwind(|| jpeg::embedded_thumbnail(bytes, max_edge, min_edge))
    {
        return Ok(t);
    }
    let d = decode(bytes, DecodeOptions::fit(max_edge, max_edge))?;
    Ok(Thumbnail {
        image: d.to_srgb8(),
        orientation: d.orientation,
        source: ThumbnailSource::Scaled,
        source_width: d.source_width,
        source_height: d.source_height,
    })
}
