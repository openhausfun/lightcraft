//! Encoders: JPEG (`jpeg-encoder`), PNG (`png`), TIFF (`tiff`), lossless WebP (`image-webp`) and,
//! on native targets with the `avif` feature, AVIF (`ravif`/`rav1e`). Metadata (ICC/EXIF/XMP) is
//! embedded where the format allows.

use crate::{Error, Result};
use lightcraft_raster::Rgba8;
use serde::{Deserialize, Serialize};

/// Interleaved sample data. Integer samples are display-encoded (0..=MAX); float samples are
/// written as-is.
#[derive(Clone, Copy, Debug)]
pub enum Samples<'a> {
    U8(&'a [u8]),
    U16(&'a [u16]),
    F32(&'a [f32]),
}

/// An image to encode: `channels` = 1 (gray), 2 (gray + alpha), 3 (RGB) or 4 (RGBA), straight alpha.
#[derive(Clone, Copy, Debug)]
pub struct EncodeImage<'a> {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub samples: Samples<'a>,
}

impl<'a> EncodeImage<'a> {
    pub fn rgba8(img: &'a Rgba8) -> Self {
        EncodeImage { width: img.width as u32, height: img.height as u32, channels: 4, samples: Samples::U8(bytemuck::cast_slice(&img.data)) }
    }
    pub fn new(width: u32, height: u32, channels: u8, samples: Samples<'a>) -> Self {
        EncodeImage { width, height, channels, samples }
    }

    fn validate(&self) -> Result<()> {
        if !(1..=4).contains(&self.channels) {
            return Err(Error::Encode(format!("unsupported channel count {}", self.channels)));
        }
        if self.width == 0 || self.height == 0 {
            return Err(Error::Encode("zero dimension".into()));
        }
        let need = self.width as u64 * self.height as u64 * self.channels as u64;
        let have = match self.samples {
            Samples::U8(s) => s.len(),
            Samples::U16(s) => s.len(),
            Samples::F32(s) => s.len(),
        } as u64;
        if have < need {
            return Err(Error::Encode(format!("sample buffer too short: {have} < {need}")));
        }
        Ok(())
    }

    fn has_alpha(&self) -> bool {
        self.channels == 2 || self.channels == 4
    }
}

/// Metadata to embed.
#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeMeta<'a> {
    pub icc: Option<&'a [u8]>,
    /// TIFF-structured EXIF (starting at `II`/`MM`, no `Exif\0\0` prefix).
    pub exif: Option<&'a [u8]>,
    pub xmp: Option<&'a str>,
    /// Print resolution in pixels per inch (JPEG JFIF density, PNG `pHYs`, TIFF X/YResolution).
    pub ppi: Option<u16>,
}

/// JPEG chroma subsampling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChromaSubsampling {
    /// No subsampling (best quality).
    #[default]
    S444,
    S422,
    S420,
}

const XMP_NS: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Encode baseline JPEG (8-bit; alpha is dropped). `quality` 1..=100.
pub fn encode_jpeg(img: &EncodeImage, quality: u8, subsampling: ChromaSubsampling, meta: &EncodeMeta) -> Result<Vec<u8>> {
    img.validate()?;
    let Samples::U8(data) = img.samples else {
        return Err(Error::Encode("JPEG needs 8-bit samples".into()));
    };
    if img.width > u16::MAX as u32 || img.height > u16::MAX as u32 {
        return Err(Error::Encode("JPEG dimensions are limited to 65535".into()));
    }
    let n = img.width as usize * img.height as usize;
    let (data, ct): (std::borrow::Cow<[u8]>, jpeg_encoder::ColorType) = match img.channels {
        1 => (data[..n].into(), jpeg_encoder::ColorType::Luma),
        2 => (data.as_chunks::<2>().0.iter().take(n).map(|c| c[0]).collect::<Vec<_>>().into(), jpeg_encoder::ColorType::Luma),
        3 => (data[..n * 3].into(), jpeg_encoder::ColorType::Rgb),
        _ => (data[..n * 4].into(), jpeg_encoder::ColorType::Rgba),
    };
    if subsampling != ChromaSubsampling::S422 {
        // Parallel encoder (restart-interval bands on all cores).
        let mut segs: Vec<(u8, Vec<u8>)> = Vec::new();
        let ppi = meta.ppi.filter(|p| *p > 0).map_or([0u8, 0, 1, 0, 1], |p| {
            let [hi, lo] = p.to_be_bytes();
            [1, hi, lo, hi, lo]
        });
        segs.push((0xE0, [&b"JFIF\0\x01\x02"[..], &ppi, &[0, 0]].concat()));
        if let Some(exif) = meta.exif {
            if exif.len() > 65_527 {
                return Err(Error::Encode("EXIF larger than one APP1 segment (64 KiB)".into()));
            }
            segs.push((0xE1, [&b"Exif\0\0"[..], exif].concat()));
        }
        if let Some(xmp) = meta.xmp {
            let seg = [XMP_NS, xmp.as_bytes()].concat();
            if seg.len() > 65_533 {
                return Err(Error::Encode("XMP larger than one APP1 segment (extended XMP not supported)".into()));
            }
            segs.push((0xE1, seg));
        }
        if let Some(icc) = meta.icc {
            // ICC.1 Annex B.4: "ICC_PROFILE\0", 1-based chunk number, chunk count
            let chunks: Vec<&[u8]> = icc.chunks(65_519).collect();
            if chunks.len() > 255 {
                return Err(Error::Encode("ICC profile too large for JPEG".into()));
            }
            for (i, c) in chunks.iter().enumerate() {
                segs.push((0xE2, [&b"ICC_PROFILE\0"[..], &[i as u8 + 1, chunks.len() as u8], c].concat()));
            }
        }
        let ch = if img.channels == 2 { 1 } else { img.channels as usize };
        // `data` is already gray for 2-channel input (see above)
        return Ok(crate::jpeg_par::encode(&data, img.width as usize, img.height as usize, ch, quality, subsampling, &segs));
    }
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(match subsampling {
        ChromaSubsampling::S444 => jpeg_encoder::SamplingFactor::R_4_4_4,
        ChromaSubsampling::S422 => jpeg_encoder::SamplingFactor::R_4_2_2,
        ChromaSubsampling::S420 => jpeg_encoder::SamplingFactor::R_4_2_0,
    });
    // Standard tables: jpeg-encoder's optimized-table mode writes non-interleaved scans, which some
    // decoders (incl. zune-jpeg 0.5) mishandle. Interleaved baseline is universally supported.
    let e = |e: jpeg_encoder::EncodingError| Error::Encode(e.to_string());
    if let Some(ppi) = meta.ppi.filter(|p| *p > 0) {
        enc.set_density(jpeg_encoder::PixelDensity::dpi(ppi));
    }
    if let Some(exif) = meta.exif {
        if exif.len() > 65_527 {
            return Err(Error::Encode("EXIF larger than one APP1 segment (64 KiB)".into()));
        }
        enc.add_exif_metadata(exif).map_err(e)?;
    }
    if let Some(xmp) = meta.xmp {
        let mut seg = XMP_NS.to_vec();
        seg.extend_from_slice(xmp.as_bytes());
        if seg.len() > 65_533 {
            return Err(Error::Encode("XMP larger than one APP1 segment (extended XMP not supported)".into()));
        }
        enc.add_app_segment(1, seg).map_err(e)?;
    }
    if let Some(icc) = meta.icc {
        enc.add_icc_profile(icc).map_err(e)?;
    }
    enc.encode(&data, img.width as u16, img.height as u16, ct).map_err(e)?;
    Ok(out)
}

/// Encode PNG (8- or 16-bit; float input is rejected — quantize first).
pub fn encode_png(img: &EncodeImage, meta: &EncodeMeta) -> Result<Vec<u8>> {
    img.validate()?;
    let n = img.width as usize * img.height as usize * img.channels as usize;
    let (bytes, depth): (Vec<u8>, png::BitDepth) = match img.samples {
        Samples::U8(s) => (s[..n].to_vec(), png::BitDepth::Eight),
        Samples::U16(s) => (s[..n].iter().flat_map(|v| v.to_be_bytes()).collect(), png::BitDepth::Sixteen),
        Samples::F32(_) => return Err(Error::Encode("PNG needs 8- or 16-bit samples".into())),
    };
    let mut info = png::Info::with_size(img.width, img.height);
    info.color_type = match img.channels {
        1 => png::ColorType::Grayscale,
        2 => png::ColorType::GrayscaleAlpha,
        3 => png::ColorType::Rgb,
        _ => png::ColorType::Rgba,
    };
    info.bit_depth = depth;
    info.icc_profile = meta.icc.map(|b| b.to_vec().into());
    info.exif_metadata = meta.exif.map(|b| b.to_vec().into());
    info.pixel_dims = meta.ppi.filter(|p| *p > 0).map(|p| {
        let ppm = (p as f64 / 0.0254).round() as u32;
        png::PixelDimensions { xppu: ppm, yppu: ppm, unit: png::Unit::Meter }
    });
    let mut out = Vec::new();
    let e = |e: png::EncodingError| Error::Encode(e.to_string());
    {
        let mut enc = png::Encoder::with_info(&mut out, info).map_err(e)?;
        enc.set_compression(png::Compression::Fast);
        if let Some(xmp) = meta.xmp {
            enc.add_itxt_chunk("XML:com.adobe.xmp".into(), xmp.into()).map_err(e)?;
        }
        let mut w = enc.write_header().map_err(e)?;
        w.write_image_data(&bytes).map_err(e)?;
        w.finish().map_err(e)?;
    }
    Ok(out)
}

/// TIFF compression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TiffCompression {
    None,
    Lzw,
    #[default]
    Deflate,
    PackBits,
}

/// Encode TIFF: 8/16-bit integer or 32-bit float, gray/RGB with optional (unassociated) alpha.
/// ICC and XMP are embedded; EXIF is not (write it through the TIFF writer in `lightcraft-tiff`).
pub fn encode_tiff(img: &EncodeImage, compression: TiffCompression, meta: &EncodeMeta) -> Result<Vec<u8>> {
    use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype};
    use tiff::tags::Predictor;
    img.validate()?;
    let e = |e: tiff::TiffError| Error::Encode(e.to_string());
    let mut cur = std::io::Cursor::new(Vec::new());
    let comp = match compression {
        TiffCompression::None => Compression::Uncompressed,
        TiffCompression::Lzw => Compression::Lzw,
        TiffCompression::Deflate => Compression::Deflate(DeflateLevel::Fast),
        TiffCompression::PackBits => Compression::Packbits,
    };
    let float = matches!(img.samples, Samples::F32(_));
    let predictor =
        if !float && matches!(compression, TiffCompression::Lzw | TiffCompression::Deflate) { Predictor::Horizontal } else { Predictor::None };
    let mut enc = TiffEncoder::new(&mut cur).map_err(e)?.with_compression(comp).with_predictor(predictor);
    let (w, h) = (img.width, img.height);
    let n = w as usize * h as usize;
    // Gray + alpha is stored as RGBA (the encoder has no GrayA colour type).
    let ch = img.channels as usize;

    macro_rules! write {
        ($ct:ty, $data:expr, $alpha:expr) => {{
            let mut im = enc.new_image::<$ct>(w, h).map_err(e)?;
            if let Some(ppi) = meta.ppi.filter(|p| *p > 0) {
                im.resolution(tiff::tags::ResolutionUnit::Inch, tiff::encoder::Rational { n: ppi as u32, d: 1 });
            }
            if let Some(icc) = meta.icc {
                im.encoder().write_tag(tiff::tags::Tag::IccProfile, icc).map_err(e)?;
            }
            if let Some(xmp) = meta.xmp {
                im.encoder().write_tag(tiff::tags::Tag::from_u16_exhaustive(700), xmp.as_bytes()).map_err(e)?;
            }
            if $alpha {
                im.encoder().write_tag(tiff::tags::Tag::ExtraSamples, &[2u16][..]).map_err(e)?;
            }
            im.write_data($data).map_err(e)?;
        }};
    }
    fn expand_ga<T: Copy>(s: &[T], n: usize) -> Vec<T> {
        s.as_chunks::<2>().0.iter().take(n).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect()
    }
    match img.samples {
        Samples::U8(s) => match ch {
            1 => write!(colortype::Gray8, &s[..n], false),
            2 => write!(colortype::RGBA8, &expand_ga(s, n), true),
            3 => write!(colortype::RGB8, &s[..n * 3], false),
            _ => write!(colortype::RGBA8, &s[..n * 4], true),
        },
        Samples::U16(s) => match ch {
            1 => write!(colortype::Gray16, &s[..n], false),
            2 => write!(colortype::RGBA16, &expand_ga(s, n), true),
            3 => write!(colortype::RGB16, &s[..n * 3], false),
            _ => write!(colortype::RGBA16, &s[..n * 4], true),
        },
        Samples::F32(s) => match ch {
            1 => write!(colortype::Gray32Float, &s[..n], false),
            2 => write!(colortype::RGBA32Float, &expand_ga(s, n), true),
            3 => write!(colortype::RGB32Float, &s[..n * 3], false),
            _ => write!(colortype::RGBA32Float, &s[..n * 4], true),
        },
    }
    Ok(cur.into_inner())
}

/// Encode lossless WebP (8-bit). ICC/EXIF/XMP are embedded in the extended (VP8X) container.
pub fn encode_webp_lossless(img: &EncodeImage, meta: &EncodeMeta) -> Result<Vec<u8>> {
    img.validate()?;
    let Samples::U8(s) = img.samples else {
        return Err(Error::Encode("WebP needs 8-bit samples".into()));
    };
    let n = img.width as usize * img.height as usize * img.channels as usize;
    let ct = match img.channels {
        1 => image_webp::ColorType::L8,
        2 => image_webp::ColorType::La8,
        3 => image_webp::ColorType::Rgb8,
        _ => image_webp::ColorType::Rgba8,
    };
    let mut out = Vec::new();
    let mut enc = image_webp::WebPEncoder::new(&mut out);
    if let Some(icc) = meta.icc {
        enc.set_icc_profile(icc.to_vec());
    }
    if let Some(exif) = meta.exif {
        enc.set_exif_metadata(exif.to_vec());
    }
    if let Some(xmp) = meta.xmp {
        enc.set_xmp_metadata(xmp.as_bytes().to_vec());
    }
    enc.encode(&s[..n], img.width, img.height, ct).map_err(|e| Error::Encode(e.to_string()))?;
    Ok(out)
}

/// Encode AVIF (sRGB, 4:4:4, 10-bit AV1) with `ravif` (rav1e). 8-bit samples are widened by the
/// encoder; 16-bit samples keep 10 bits of precision (BT.601 full-range YCbCr computed here).
/// `quality` 1..=100, `speed` 1 (slow) ..= 10. EXIF is embedded; ICC is not supported by the muxer
/// (the file is tagged sRGB via `nclx`). Returns [`Error::Encode`] for float samples, on wasm32 or
/// when built without the `avif` feature.
pub fn encode_avif(img: &EncodeImage, quality: u8, speed: u8, meta: &EncodeMeta) -> Result<Vec<u8>> {
    img.validate()?;
    #[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
    {
        let mut enc = ravif::Encoder::new()
            .with_quality(quality.clamp(1, 100) as f32)
            .with_alpha_quality(quality.clamp(1, 100) as f32)
            .with_speed(speed.clamp(1, 10));
        if let Some(exif) = meta.exif {
            enc = enc.with_exif(exif.to_vec());
        }
        let s = match img.samples {
            Samples::U8(s) => s,
            Samples::U16(s) => return avif_10bit(&enc, img, s),
            Samples::F32(_) => return Err(Error::Encode("AVIF encoder needs 8- or 16-bit samples".into())),
        };
        let n = img.width as usize * img.height as usize;
        let ch = img.channels as usize;
        let px: Vec<ravif::RGBA8> = s
            .chunks_exact(ch)
            .take(n)
            .map(|c| match ch {
                1 => ravif::RGBA8::new(c[0], c[0], c[0], 255),
                2 => ravif::RGBA8::new(c[0], c[0], c[0], c[1]),
                3 => ravif::RGBA8::new(c[0], c[1], c[2], 255),
                _ => ravif::RGBA8::new(c[0], c[1], c[2], c[3]),
            })
            .collect();
        let r = enc.encode_rgba(ravif::Img::new(&px[..], img.width as usize, img.height as usize)).map_err(|e| Error::Encode(e.to_string()))?;
        let _ = img.has_alpha();
        Ok(r.avif_file)
    }
    #[cfg(not(all(feature = "avif", not(target_arch = "wasm32"))))]
    {
        let _ = (quality, speed, meta, img.has_alpha());
        Err(Error::Encode("AVIF encoding is not available in this build".into()))
    }
}

/// 16-bit RGB(A) → 10-bit BT.601 full-range YCbCr planes (the matrix `ravif` uses for 8-bit input).
#[cfg(all(feature = "avif", not(target_arch = "wasm32")))]
fn avif_10bit(enc: &ravif::Encoder, img: &EncodeImage, s: &[u16]) -> Result<Vec<u8>> {
    const K: [f32; 3] = [0.299, 0.587, 0.114];
    let n = img.width as usize * img.height as usize;
    let ch = img.channels as usize;
    let rgb = |c: &[u16]| -> [f32; 3] {
        let v = |x: u16| x as f32 * (1023.0 / 65535.0);
        match ch {
            1 | 2 => [v(c[0]); 3],
            _ => [v(c[0]), v(c[1]), v(c[2])],
        }
    };
    let planes = s.chunks_exact(ch).take(n).map(|c| {
        let [r, g, b] = rgb(c);
        let y = K[0] * r + K[1] * g + K[2] * b;
        let cb = (b - y) * (0.5 / (1.0 - K[2])) + 512.0;
        let cr = (r - y) * (0.5 / (1.0 - K[0])) + 512.0;
        [y, cb, cr].map(|v| v.round().clamp(0.0, 1023.0) as u16)
    });
    let alpha_at = |c: &[u16]| if img.has_alpha() { c[ch - 1] } else { u16::MAX };
    let has_alpha = s.chunks_exact(ch).take(n).any(|c| alpha_at(c) != u16::MAX);
    let alpha = has_alpha.then(|| s.chunks_exact(ch).take(n).map(move |c| ((alpha_at(c) as u32 * 1023 + 32767) / 65535) as u16));
    let r = enc
        .encode_raw_planes_10_bit(img.width as usize, img.height as usize, planes, alpha, ravif::PixelRange::Full, ravif::MatrixCoefficients::BT601)
        .map_err(|e| Error::Encode(e.to_string()))?;
    Ok(r.avif_file)
}
