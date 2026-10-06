//! JPEG: marker parsing (EXIF/XMP/ICC/Adobe/MPF), full decode via `zune-jpeg`, DCT-scaled and
//! CMYK/YCCK decode via `jpeg-decoder`, embedded-preview thumbnails.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result, Thumbnail, ThumbnailSource, exif};

const F: Format = Format::Jpeg;

/// What we learn from walking the marker segments before the first scan.
#[derive(Default, Debug)]
pub(crate) struct Markers {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    pub precision: u8,
    /// SOFn marker byte (0xC0 baseline, 0xC1 extended, 0xC2 progressive, …).
    pub sof: u8,
    /// Number of components in the first scan (< `components` for non-interleaved sequential files).
    pub first_scan_components: u8,
    pub exif: Option<Vec<u8>>,
    /// Offset of the EXIF TIFF header within the file.
    pub exif_offset: usize,
    pub xmp: Option<String>,
    pub icc: Option<Vec<u8>>,
    pub adobe_transform: Option<u8>,
    /// Offset of the MPF TIFF header within the file and its bytes.
    pub mpf: Option<(usize, Vec<u8>)>,
}

pub(crate) fn parse_markers(b: &[u8]) -> Option<Markers> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut m = Markers::default();
    let mut icc_chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut p = 2;
    while p + 4 <= b.len() {
        if b[p] != 0xFF {
            // Tolerate garbage between segments: resync on the next 0xFF.
            p += 1;
            continue;
        }
        let marker = b[p + 1];
        if marker == 0xFF {
            p += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            p += 2;
            continue;
        }
        if marker == 0xD9 {
            break;
        }
        if marker == 0xDA {
            m.first_scan_components = b.get(p + 4).copied().unwrap_or(0);
            break;
        }
        let len = u16::from_be_bytes([b[p + 2], b[p + 3]]) as usize;
        if len < 2 {
            return None;
        }
        let start = p + 4;
        let end = (p + 2 + len).min(b.len());
        let seg = &b[start..end];
        match marker {
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                if seg.len() >= 6 && m.components == 0 {
                    m.precision = seg[0];
                    m.sof = marker;
                    m.height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                    m.width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                    m.components = seg[5];
                }
            }
            0xE1 => {
                if seg.starts_with(b"Exif\0\0") && m.exif.is_none() {
                    m.exif = Some(seg[6..].to_vec());
                    m.exif_offset = start + 6;
                } else if let Some(x) = seg.strip_prefix(b"http://ns.adobe.com/xap/1.0/\0")
                    && m.xmp.is_none()
                {
                    m.xmp = Some(String::from_utf8_lossy(x).trim_end_matches('\0').to_string());
                }
            }
            0xE2 => {
                if seg.starts_with(b"ICC_PROFILE\0") && seg.len() >= 14 {
                    icc_chunks.push((seg[12], &seg[14..]));
                } else if seg.starts_with(b"MPF\0") && m.mpf.is_none() {
                    m.mpf = Some((start + 4, seg[4..].to_vec()));
                }
            }
            0xEE if seg.starts_with(b"Adobe") && seg.len() >= 12 => {
                m.adobe_transform = Some(seg[11]);
            }
            _ => {}
        }
        p += 2 + len;
    }
    if !icc_chunks.is_empty() {
        icc_chunks.sort_by_key(|c| c.0);
        m.icc = Some(icc_chunks.iter().flat_map(|c| c.1.iter().copied()).collect());
    }
    Some(m)
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let m = parse_markers(bytes).ok_or_else(|| Error::Malformed(F, "missing SOI".into()))?;
    if m.components == 0 {
        return Err(Error::Malformed(F, "no frame header".into()));
    }
    check_size(F, m.width as u64, m.height as u64, opts)?;
    let scale_to = opts.max_size.filter(|&(mw, mh)| mw > 0 && mh > 0 && (m.width >= 2 * mw || m.height >= 2 * mh));
    // zune-jpeg handles the common cases fastest; jpeg-decoder covers DCT scaling, CMYK/YCCK,
    // 12-bit and non-interleaved sequential scans (which zune-jpeg 0.5 mis-decodes with subsampling).
    let non_interleaved = matches!(m.sof, 0xC0 | 0xC1) && m.first_scan_components < m.components;
    let raw = if m.components == 4 || scale_to.is_some() || m.precision > 8 || non_interleaved {
        decode_jpeg_decoder(bytes, &m, scale_to)?
    } else {
        decode_zune(bytes, &m)?
    };
    let meta = Meta { icc: m.icc, exif: m.exif, xmp: m.xmp, ..Default::default() };
    finish(F, raw, meta, (m.width, m.height), opts)
}

fn decode_zune(bytes: &[u8], m: &Markers) -> Result<Raw> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let gray = m.components == 1;
    let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false).jpeg_set_out_colorspace(if gray {
        ColorSpace::Luma
    } else {
        ColorSpace::RGB
    });
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let px = d.decode().map_err(|e| Error::Malformed(F, e.to_string()))?;
    let info = d.info().ok_or_else(|| Error::Malformed(F, "no info".into()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let model = if gray { Model::Gray } else { Model::Rgb };
    if px.len() < w * h * model.channels() {
        return Err(Error::Malformed(F, "short output".into()));
    }
    Ok(Raw { width: w, height: h, model, alpha: false, premultiplied: false, buf: Buf::U8(px), bit_depth: 8 })
}

fn decode_jpeg_decoder(bytes: &[u8], m: &Markers, scale_to: Option<(u32, u32)>) -> Result<Raw> {
    use jpeg_decoder::PixelFormat;
    let mut d = jpeg_decoder::Decoder::new(bytes);
    d.read_info().map_err(|e| Error::Malformed(F, e.to_string()))?;
    if let Some((mw, mh)) = scale_to {
        // Ask for the smallest DCT scale still covering the target box.
        let (sw, sh) = scaled_request(m.width, m.height, mw, mh);
        d.scale(sw, sh).map_err(|e| Error::Malformed(F, e.to_string()))?;
    }
    let px = d.decode().map_err(|e| Error::Malformed(F, e.to_string()))?;
    let info = d.info().ok_or_else(|| Error::Malformed(F, "no info".into()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let (model, buf, depth) = match info.pixel_format {
        PixelFormat::L8 => (Model::Gray, Buf::U8(px), 8),
        PixelFormat::L16 => {
            let v = px.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            (Model::Gray, Buf::U16(v), 16)
        }
        PixelFormat::RGB24 => (Model::Rgb, Buf::U8(px), 8),
        PixelFormat::CMYK32 => (Model::Cmyk, Buf::U8(px), 8),
    };
    Ok(Raw { width: w, height: h, model, alpha: false, premultiplied: false, buf, bit_depth: depth })
}

/// The box to request from `jpeg-decoder::scale` so the decoded image still covers `mw × mh` once
/// fitted with the source aspect ratio.
fn scaled_request(w: u32, h: u32, mw: u32, mh: u32) -> (u16, u16) {
    let s = (mw as f64 / w as f64).min(mh as f64 / h as f64).min(1.0);
    let tw = ((w as f64 * s).ceil() as u32).clamp(1, u16::MAX as u32);
    let th = ((h as f64 * s).ceil() as u32).clamp(1, u16::MAX as u32);
    (tw as u16, th as u16)
}

/// Fast path: an embedded JPEG preview (EXIF IFD1 thumbnail, or the largest MPF preview) whose long
/// edge is ≥ `max_edge`, decoded with DCT scaling and fitted.
pub(crate) fn embedded_thumbnail(bytes: &[u8], max_edge: u32, min_edge: u32) -> Option<Thumbnail> {
    let m = parse_markers(bytes)?;
    let summary = m.exif.as_deref().map(exif::summarize).unwrap_or_default();
    let orientation = summary.orientation.unwrap_or(1);
    let mut candidates: Vec<(&[u8], ThumbnailSource)> = Vec::new();
    if let (Some(ex), Some((o, l))) = (m.exif.as_deref(), summary.thumbnail) {
        candidates.push((&ex[o..o + l], ThumbnailSource::ExifThumbnail));
    }
    for (o, l) in mpf_images(&m) {
        if let Some(s) = bytes.get(o..o.saturating_add(l)) {
            candidates.push((s, ThumbnailSource::MpfPreview));
        }
    }
    // Smallest adequate preview wins.
    let mut best: Option<(u32, &[u8], ThumbnailSource)> = None;
    for (data, src) in candidates {
        let Some(pm) = parse_markers(data) else { continue };
        let long = pm.width.max(pm.height);
        // Previews must have the main image's aspect ratio (within 2%), else they are letterboxed/cropped.
        let aspect_ok = m.width > 0
            && m.height > 0
            && pm.height > 0
            && ((pm.width as f64 / pm.height as f64) / (m.width as f64 / m.height as f64) - 1.0).abs() < 0.02;
        // Prefer the smallest preview covering `max_edge`; otherwise the largest one ≥ `min_edge`.
        let better = match &best {
            None => true,
            Some(b) if b.0 >= max_edge => long >= max_edge && long < b.0,
            Some(b) => long > b.0,
        };
        if long >= min_edge && aspect_ok && better {
            best = Some((long, data, src));
        }
    }
    let (_, data, source) = best?;
    let d = decode(data, &DecodeOptions::fit(max_edge, max_edge)).ok()?;
    Some(Thumbnail { image: d.to_srgb8(), orientation, source, source_width: m.width, source_height: m.height })
}

/// (file offset, length) of MPF-listed images other than the primary.
fn mpf_images(m: &Markers) -> Vec<(usize, usize)> {
    let Some((base, data)) = &m.mpf else { return vec![] };
    let Some(t) = exif::Tiff::new(data) else { return vec![] };
    let Some(ifd) = t.first_ifd() else { return vec![] };
    let Some((entries, _)) = t.ifd(ifd) else { return vec![] };
    let Some(e) = entries.iter().find(|e| e.tag == 0xB002) else { return vec![] };
    let Some(list) = t.bytes(e) else { return vec![] };
    let mut out = Vec::new();
    for (i, rec) in list.as_chunks::<16>().0.iter().enumerate().take(16) {
        if i == 0 {
            continue;
        }
        let rd = |o: usize| {
            let a = [rec[o], rec[o + 1], rec[o + 2], rec[o + 3]];
            if data.starts_with(b"II") { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) }
        };
        let (size, off) = (rd(4) as usize, rd(8) as usize);
        if size > 0 && off > 0 {
            out.push((base + off, size));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_request_covers() {
        assert_eq!(scaled_request(6000, 4000, 256, 256), (256, 171));
        assert_eq!(scaled_request(100, 100, 256, 256), (100, 100));
    }

    #[test]
    fn markers_on_garbage() {
        assert!(parse_markers(b"").is_none());
        assert!(parse_markers(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 1]).is_none());
        let m = parse_markers(&[0xFF, 0xD8, 0xFF, 0xE1, 0, 200, 1, 2]).unwrap();
        assert!(m.exif.is_none());
    }
}
