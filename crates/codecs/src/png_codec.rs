//! PNG decode via the `png` crate (8/16-bit, palette, tRNS, iCCP, eXIf, iTXt XMP, sRGB/cICP/cHRM/gAMA).

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use lightcraft_color::{Mat3, RgbSpace, Xy};

const F: Format = Format::Png;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Malformed(F, e.to_string())
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND);
    dec.set_limits(png::Limits { bytes: opts.max_pixels.saturating_mul(8).min(usize::MAX as u64) as usize });
    let mut reader = dec.read_info().map_err(err)?;
    let (w, h) = {
        let i = reader.info();
        (i.width, i.height)
    };
    check_size(F, w as u64, h as u64, opts)?;
    let size = reader.output_buffer_size().ok_or_else(|| err("buffer size overflow"))?;
    let mut buf = vec![0u8; size];
    let out = reader.next_frame(&mut buf).map_err(err)?;
    // Text chunks after IDAT land in `info` once the stream is finished; ignore trailing errors.
    let _ = reader.finish();
    let info = reader.info();

    let (model, alpha) = match out.color_type {
        png::ColorType::Grayscale => (Model::Gray, false),
        png::ColorType::GrayscaleAlpha => (Model::Gray, true),
        png::ColorType::Rgb => (Model::Rgb, false),
        png::ColorType::Rgba => (Model::Rgb, true),
        png::ColorType::Indexed => return Err(err("palette not expanded")),
    };
    buf.truncate(out.buffer_size());
    let (samples, depth) = match out.bit_depth {
        png::BitDepth::Sixteen => (Buf::U16(buf.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect()), 16),
        _ => (Buf::U8(buf), info.bit_depth as u8),
    };
    let raw = Raw { width: out.width as usize, height: out.height as usize, model, alpha, premultiplied: false, buf: samples, bit_depth: depth };

    let xmp = info
        .utf8_text
        .iter()
        .find(|t| t.keyword == "XML:com.adobe.xmp")
        .and_then(|t| t.get_text().ok())
        .or_else(|| info.uncompressed_latin1_text.iter().find(|t| t.keyword == "XML:com.adobe.xmp").map(|t| t.text.clone()));
    let exif = info.exif_metadata.as_ref().map(|e| {
        let e = e.to_vec();
        // Some writers keep the JPEG "Exif\0\0" prefix.
        if e.starts_with(b"Exif\0\0") { e[6..].to_vec() } else { e }
    });
    let meta = Meta { icc: info.icc_profile.as_ref().map(|c| c.to_vec()), exif, xmp, hint: container_hint(info), ..Default::default() };
    finish(F, raw, meta, (w, h), opts)
}

fn container_hint(info: &png::Info) -> Option<SourceSpace> {
    if let Some(c) = &info.coding_independent_code_points {
        let named = match c.color_primaries {
            1 => Some(NamedSpace::Srgb),
            9 => Some(NamedSpace::Rec2020),
            12 => Some(NamedSpace::DisplayP3),
            _ => None,
        };
        let trc = match c.transfer_function {
            13 => Some(Trc::Srgb),
            1 | 6 | 14 | 15 => Some(Trc::Rec709),
            8 => Some(Trc::Linear),
            4 => Some(Trc::Gamma(2.2)),
            _ => None,
        };
        if let (Some(n), Some(t)) = (named, trc) {
            let mut s = SourceSpace::named(n, SpaceOrigin::Container);
            s.trc = Some([t.clone(), t.clone(), t]);
            return Some(s);
        }
    }
    if info.srgb.is_some() {
        return Some(SourceSpace::named(NamedSpace::Srgb, SpaceOrigin::Container));
    }
    let gamma = info.gama_chunk.or(info.source_gamma).map(|g| g.into_value()).filter(|g| *g > 0.0 && g.is_finite());
    let chrm = info.chrm_chunk.or(info.source_chromaticities);
    if gamma.is_none() && chrm.is_none() {
        return None;
    }
    let trc = match gamma {
        Some(g) if (g - 1.0).abs() < 1e-3 => Trc::Linear,
        Some(g) => Trc::Gamma(1.0 / g),
        None => Trc::Srgb,
    };
    let mut s = SourceSpace::named(NamedSpace::Srgb, SpaceOrigin::Container);
    if let Some(c) = chrm {
        let xy = |p: (png::ScaledFloat, png::ScaledFloat)| Xy::new(p.0.into_value() as f64, p.1.into_value() as f64);
        let space = RgbSpace { name: "PNG cHRM", r: xy(c.red), g: xy(c.green), b: xy(c.blue), white: xy(c.white) };
        let valid = [space.r, space.g, space.b, space.white].iter().all(|p| p.y > 1e-4 && p.x >= 0.0 && p.x + p.y <= 1.0 + 1e-6);
        let m = if valid { rgb_to_xyz_d50_checked(&space) } else { None };
        if let Some(m) = m {
            s.to_xyz_d50 = m;
            s.named = NamedSpace::recognize(&m);
        }
    }
    s.trc = Some([trc.clone(), trc.clone(), trc]);
    Some(s)
}

fn rgb_to_xyz_d50_checked(space: &RgbSpace) -> Option<Mat3> {
    let p = [space.r.to_xyz(), space.g.to_xyz(), space.b.to_xyz()];
    let m = Mat3([[p[0][0], p[1][0], p[2][0]], [p[0][1], p[1][1], p[2][1]], [p[0][2], p[1][2], p[2][2]]]);
    if m.determinant().abs() < 1e-9 {
        return None;
    }
    Some(crate::space::rgb_to_xyz_d50(space))
}
