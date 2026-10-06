//! TIFF decode via the `tiff` crate: 8/16/32-bit integer, 16/32/64-bit float, gray/RGB/CMYK/palette,
//! alpha (associated or not), ICC (34675), XMP (700), orientation (274). First image only.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;

const F: Format = Format::Tiff;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Malformed(F, e.to_string())
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut limits = Limits::default();
    limits.decoding_buffer_size = opts.max_pixels.saturating_mul(4 * 8).min(usize::MAX as u64) as usize;
    limits.ifd_value_size = 32 << 20;
    let mut d = Decoder::new(std::io::Cursor::new(bytes)).map_err(err)?.with_limits(limits);
    let (w, h) = d.dimensions().map_err(err)?;
    check_size(F, w as u64, h as u64, opts)?;
    let ct = d.colortype().map_err(err)?;

    let icc = tag_bytes(&mut d, Tag::IccProfile);
    let xmp = tag_bytes(&mut d, Tag::from_u16_exhaustive(700)).map(|v| String::from_utf8_lossy(&v).trim_end_matches('\0').to_string());
    let orientation = d.get_tag_u32(Tag::Orientation).ok().map(|v| v as u16);
    let photometric = d.get_tag_u32(Tag::PhotometricInterpretation).ok();
    let planar = d.get_tag_u32(Tag::PlanarConfiguration).ok() == Some(2);
    let extra = d.get_tag_u16_vec(Tag::ExtraSamples).ok().unwrap_or_default();
    let colormap = d.get_tag_u16_vec(Tag::ColorMap).ok();
    let compression = d.get_tag_u32(Tag::Compression).ok();

    let (model, n_color, n_samples, bits) = match ct {
        ColorType::Gray(b) => (Model::Gray, 1, 1, b),
        ColorType::GrayA(b) => (Model::Gray, 1, 2, b),
        ColorType::RGB(b) => (Model::Rgb, 3, 3, b),
        ColorType::RGBA(b) => (Model::Rgb, 3, 4, b),
        ColorType::CMYK(b) => (Model::Cmyk, 4, 4, b),
        ColorType::CMYKA(b) => (Model::Cmyk, 4, 5, b),
        ColorType::Palette(b) => (Model::Rgb, 1, 1, b),
        // JPEG-compressed YCbCr is converted to RGB by the JPEG decoder.
        ColorType::YCbCr(b) if compression == Some(7) => (Model::Rgb, 3, 3, b),
        ColorType::Multiband { bit_depth, num_samples } if num_samples >= 3 => (Model::Rgb, 3, num_samples as usize, bit_depth),
        ColorType::Multiband { bit_depth, num_samples } if num_samples >= 1 => (Model::Gray, 1, num_samples as usize, bit_depth),
        other => return Err(Error::Unsupported(F, unsupported_name(other))),
    };
    let has_alpha = n_samples > n_color && !matches!(ct, ColorType::Palette(_));
    let premultiplied = has_alpha && extra.first() == Some(&1);

    let res = d.read_image().map_err(err)?;
    let (wu, hu) = (w as usize, h as usize);
    let n = wu * hu;
    let mut buf = match res {
        DecodingResult::U8(v) => Buf::U8(v),
        DecodingResult::U16(v) => Buf::U16(v),
        // Rare integer widths: keep 16 significant bits (plenty for display-referred data).
        DecodingResult::U32(v) => Buf::U16(v.into_iter().map(|x| (x >> 16) as u16).collect()),
        DecodingResult::U64(v) => Buf::U16(v.into_iter().map(|x| (x >> 48) as u16).collect()),
        DecodingResult::I8(v) => Buf::U8(v.into_iter().map(|x| (x.max(0) as u8) << 1).collect()),
        DecodingResult::I16(v) => Buf::U16(v.into_iter().map(|x| (x.max(0) as u16) << 1).collect()),
        DecodingResult::I32(v) => Buf::U16(v.into_iter().map(|x| (x.max(0) >> 15) as u16).collect()),
        DecodingResult::I64(v) => Buf::U16(v.into_iter().map(|x| (x.max(0) >> 47) as u16).collect()),
        DecodingResult::F16(v) => Buf::F32(v.into_iter().map(|x| x.to_f32()).collect()),
        DecodingResult::F32(v) => Buf::F32(v),
        DecodingResult::F64(v) => Buf::F32(v.into_iter().map(|x| x as f32).collect()),
    };
    let bit_depth = bits;

    if planar && n_samples > 1 {
        buf = interleave(buf, n, n_samples);
    }

    // Palette → 16-bit RGB.
    if let ColorType::Palette(b) = ct {
        let map = colormap.ok_or_else(|| err("palette image without ColorMap"))?;
        let entries = 1usize << b.min(16);
        if map.len() < 3 * entries {
            return Err(err("short ColorMap"));
        }
        let idx: Vec<usize> = match &buf {
            Buf::U8(v) => v.iter().map(|&i| i as usize).collect(),
            Buf::U16(v) => v.iter().map(|&i| i as usize).collect(),
            Buf::F32(_) => return Err(err("float palette")),
        };
        let mut out = Vec::with_capacity(n * 3);
        for &i in idx.iter().take(n) {
            let i = i.min(entries - 1);
            out.extend_from_slice(&[map[i], map[entries + i], map[2 * entries + i]]);
        }
        buf = Buf::U16(out);
        let raw = Raw { width: wu, height: hu, model: Model::Rgb, alpha: false, premultiplied: false, buf, bit_depth };
        let meta = Meta { icc, xmp, orientation, ..Default::default() };
        return finish(F, raw, meta, (w, h), opts);
    }

    // WhiteIsZero → invert.
    if photometric == Some(0) {
        match &mut buf {
            Buf::U8(v) => v.iter_mut().for_each(|x| *x = 255 - *x),
            Buf::U16(v) => v.iter_mut().for_each(|x| *x = 65535 - *x),
            Buf::F32(v) => v.iter_mut().for_each(|x| *x = 1.0 - *x),
        }
    }

    // Drop extra samples beyond colour + one alpha.
    let keep = n_color + has_alpha as usize;
    if n_samples != keep {
        buf = select_channels(buf, n, n_samples, keep);
    }
    let raw = Raw { width: wu, height: hu, model, alpha: has_alpha, premultiplied, buf, bit_depth };
    let meta = Meta { icc, xmp, orientation, ..Default::default() };
    finish(F, raw, meta, (w, h), opts)
}

/// A BYTE/UNDEFINED tag as bytes (the decoder may surface unknown byte tags as unsigned values).
fn tag_bytes<R: std::io::Read + std::io::Seek>(d: &mut Decoder<R>, tag: Tag) -> Option<Vec<u8>> {
    use tiff::decoder::ifd::Value;
    fn byte(v: &Value) -> Option<u8> {
        match v {
            Value::Byte(b) => Some(*b),
            Value::Unsigned(u) => u8::try_from(*u).ok(),
            Value::Short(u) => u8::try_from(*u).ok(),
            Value::SignedByte(b) => Some(*b as u8),
            _ => None,
        }
    }
    match d.find_tag(tag).ok()?? {
        Value::List(v) => v.iter().map(byte).collect(),
        Value::Ascii(s) => Some(s.into_bytes()),
        v => byte(&v).map(|b| vec![b]),
    }
}

fn unsupported_name(ct: ColorType) -> &'static str {
    match ct {
        ColorType::Lab(_) => "CIE L*a*b* TIFF",
        ColorType::YCbCr(_) => "uncompressed YCbCr TIFF",
        _ => "colour type",
    }
}

fn interleave(buf: Buf, n: usize, s: usize) -> Buf {
    fn go<T: Copy + Default>(v: Vec<T>, n: usize, s: usize) -> Vec<T> {
        if v.len() < n * s {
            return v;
        }
        let mut out = vec![T::default(); n * s];
        for c in 0..s {
            for i in 0..n {
                out[i * s + c] = v[c * n + i];
            }
        }
        out
    }
    match buf {
        Buf::U8(v) => Buf::U8(go(v, n, s)),
        Buf::U16(v) => Buf::U16(go(v, n, s)),
        Buf::F32(v) => Buf::F32(go(v, n, s)),
    }
}

fn select_channels(buf: Buf, n: usize, s: usize, keep: usize) -> Buf {
    fn go<T: Copy>(v: Vec<T>, n: usize, s: usize, keep: usize) -> Vec<T> {
        if v.len() < n * s {
            return v;
        }
        let mut out = Vec::with_capacity(n * keep);
        for px in v.chunks_exact(s).take(n) {
            out.extend_from_slice(&px[..keep]);
        }
        out
    }
    match buf {
        Buf::U8(v) => Buf::U8(go(v, n, s, keep)),
        Buf::U16(v) => Buf::U16(go(v, n, s, keep)),
        Buf::F32(v) => Buf::F32(go(v, n, s, keep)),
    }
}
