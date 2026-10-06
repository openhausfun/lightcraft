//! WebP decode via `image-webp` (lossy + lossless, alpha, ICCP/EXIF/XMP chunks; first frame of
//! animations).

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

const F: Format = Format::WebP;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Malformed(F, e.to_string())
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut d = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes)).map_err(err)?;
    let (w, h) = d.dimensions();
    check_size(F, w as u64, h as u64, opts)?;
    let alpha = d.has_alpha();
    let size = d.output_buffer_size().ok_or_else(|| err("buffer size overflow"))?;
    let mut buf = vec![0u8; size];
    d.read_image(&mut buf).map_err(err)?;
    let icc = d.icc_profile().ok().flatten();
    let exif = d.exif_metadata().ok().flatten().map(|e| if e.starts_with(b"Exif\0\0") { e[6..].to_vec() } else { e });
    let xmp = d.xmp_metadata().ok().flatten().map(|x| String::from_utf8_lossy(&x).trim_end_matches('\0').to_string());
    let raw = Raw { width: w as usize, height: h as usize, model: Model::Rgb, alpha, premultiplied: false, buf: Buf::U8(buf), bit_depth: 8 };
    finish(F, raw, Meta { icc, exif, xmp, ..Default::default() }, (w, h), opts)
}
