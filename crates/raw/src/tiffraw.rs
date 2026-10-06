//! Reading the pixel data of a TIFF IFD (strips or tiles; uncompressed, lossless JPEG, Deflate), in parallel.

use crate::unpack::*;
use crate::{MAX_SAMPLES, RawData, RawError, Result, ljpeg};
use lightcraft_tiff::image::{Chunk, ImageInfo, chunk_bytes};
use lightcraft_tiff::{ByteOrder, tags::compression as comp};
use rayon::prelude::*;

/// Bit packing for uncompressed integer data with bit depths other than 8/16.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // vendor decoders use the other variants
pub enum Packing {
    /// TIFF standard: MSB-first, rows start on byte boundaries.
    Msb,
    /// LSB-first little-endian bit stream (some vendor raws), rows byte aligned.
    Lsb,
    /// Each sample stored in 16 bits in file byte order, regardless of `bits`.
    Word16,
}

enum ChunkPx {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

/// The checks [`read_image`] makes before decoding anything (sample format, size limits, data
/// present); returns the number of samples.
pub fn check_image(data: &[u8], info: &ImageInfo) -> Result<usize> {
    let (w, h) = (info.width as usize, info.height as usize);
    let cpp = info.samples_per_pixel as usize;
    let total = w.checked_mul(h).and_then(|v| v.checked_mul(cpp)).ok_or(RawError::Limit("image too large"))?;
    if total > MAX_SAMPLES || total == 0 {
        return Err(RawError::Limit("image too large"));
    }
    let bits = info.bits() as u32;
    let float = info.sample_format == 3;
    if float && !matches!(bits, 16 | 24 | 32) {
        return Err(RawError::Unsupported(format!("{bits}-bit floating point samples")));
    }
    if !float && !(1..=16).contains(&bits) {
        return Err(RawError::Unsupported(format!("{bits}-bit integer samples")));
    }
    let chunks = info.chunks(data.len() as u64);
    if chunks.is_empty() {
        return Err(RawError::Corrupt("no image data chunks".into()));
    }
    // Plausibility bound before allocating: no supported coding stores more than ~2000 samples per byte
    // (Deflate of constant data is the extreme), so tiny files cannot trigger huge allocations.
    let available: u64 = chunks.iter().map(|c| chunk_bytes(data, c).map_or(0, |s| s.len() as u64)).sum();
    if available.saturating_mul(2048) < total as u64 {
        return Err(RawError::Corrupt(format!("{available} bytes of image data cannot hold {total} samples")));
    }
    Ok(total)
}

/// Read the samples of `info` in `mode`: [`Mode::Header`] only checks them ([`check_image`]) and
/// returns no samples (floating-point data as an empty `F32`).
pub(crate) fn read_image_in(mode: crate::Mode, data: &[u8], info: &ImageInfo, order: ByteOrder, packing: Packing) -> Result<RawData> {
    match mode {
        crate::Mode::Full => read_image(data, info, order, packing),
        crate::Mode::Header => {
            check_image(data, info)?;
            // a lossless-JPEG layout the decoder rejects (subsampled components, e.g. Sony's
            // lossless M/S sizes) must fail here too, as the full decode will
            if info.compression == 7
                && let Some(src) = info.chunks(data.len() as u64).first().and_then(|c| chunk_bytes(data, c))
            {
                ljpeg::frame_info(src)?;
            }
            Ok(if info.sample_format == 3 { RawData::F32(Vec::new()) } else { RawData::U16(Vec::new()) })
        }
    }
}

/// Decode all chunks of `info` into one buffer of `width × height × cpp` samples.
pub fn read_image(data: &[u8], info: &ImageInfo, order: ByteOrder, packing: Packing) -> Result<RawData> {
    let (w, h) = (info.width as usize, info.height as usize);
    let total = check_image(data, info)?;
    let cpp = info.samples_per_pixel as usize;
    let bits = info.bits() as u32;
    let float = info.sample_format == 3;
    let chunks = info.chunks(data.len() as u64);
    let planar = info.planar == 2 && cpp > 1;
    let ccpp = if planar { 1 } else { cpp };
    let decoded: Vec<Result<(Chunk, ChunkPx)>> =
        chunks.par_iter().map(|c| decode_chunk(data, info, c, order, packing, ccpp, bits, float).map(|p| (*c, p))).collect();
    let mut out_u16 = if float { Vec::new() } else { vec![0u16; total] };
    let mut out_f32 = if float { vec![0f32; total] } else { Vec::new() };
    let mut ok = 0usize;
    let mut first_err = None;
    for r in decoded {
        let (c, px) = match r {
            Ok(v) => v,
            Err(e) => {
                first_err.get_or_insert(e);
                continue;
            }
        };
        ok += 1;
        let (cw, ch) = (c.width as usize, c.height as usize);
        let (x0, y0) = (c.x as usize, c.y as usize);
        if x0 >= w || y0 >= h {
            continue;
        }
        let copy_w = cw.min(w - x0);
        let copy_h = ch.min(h - y0);
        for y in 0..copy_h {
            for x in 0..copy_w {
                for s in 0..ccpp {
                    let src = (y * cw + x) * ccpp + s;
                    let dst_s = if planar { c.plane as usize } else { s };
                    if dst_s >= cpp {
                        continue;
                    }
                    let dst = ((y0 + y) * w + x0 + x) * cpp + dst_s;
                    match &px {
                        ChunkPx::U16(v) => {
                            if let (Some(o), Some(&s)) = (out_u16.get_mut(dst), v.get(src)) {
                                *o = s;
                            }
                        }
                        ChunkPx::F32(v) => {
                            if let (Some(o), Some(&s)) = (out_f32.get_mut(dst), v.get(src)) {
                                *o = s;
                            }
                        }
                    }
                }
            }
        }
    }
    if ok == 0 {
        return Err(first_err.unwrap_or_else(|| RawError::Corrupt("no decodable chunks".into())));
    }
    Ok(if float { RawData::F32(out_f32) } else { RawData::U16(out_u16) })
}

#[allow(clippy::too_many_arguments)]
fn decode_chunk(data: &[u8], info: &ImageInfo, c: &Chunk, order: ByteOrder, packing: Packing, cpp: usize, bits: u32, float: bool) -> Result<ChunkPx> {
    let src = chunk_bytes(data, c).ok_or_else(|| RawError::Corrupt("chunk offset past end of file".into()))?;
    let (cw, ch) = (c.width as usize, c.height as usize);
    let n = cw.checked_mul(ch).and_then(|v| v.checked_mul(cpp)).ok_or(RawError::Limit("chunk too large"))?;
    if n > MAX_SAMPLES {
        return Err(RawError::Limit("chunk too large"));
    }
    match info.compression {
        comp::NONE => unpack_chunk(src, order, packing, cw, ch, cpp, bits, float, 1),
        comp::JPEG => {
            let f = ljpeg::decode(src, n.saturating_mul(2).max(1 << 16))?;
            if f.data.len() < n {
                // Some writers encode edge tiles smaller than nominal: accept a frame that covers the rows it has.
                if f.data.is_empty() || f.data.len() % (cw * cpp) != 0 {
                    return Err(RawError::Corrupt(format!("lossless JPEG tile has {} samples, expected {n}", f.data.len())));
                }
            }
            let mut v = f.data;
            v.resize(n, 0);
            Ok(ChunkPx::U16(v))
        }
        comp::LOSSY_JPEG => {
            // lossy DNG: each chunk is a baseline (DCT) JPEG of 8-bit samples
            let (px, jw, jh, jc) = lossy_jpeg(src, cpp)?;
            let mut v = vec![0u16; n];
            for y in 0..ch.min(jh) {
                for x in 0..cw.min(jw) {
                    for k in 0..cpp {
                        v[(y * cw + x) * cpp + k] = px[(y * jw + x) * jc + k.min(jc - 1)] as u16;
                    }
                }
            }
            Ok(ChunkPx::U16(v))
        }
        comp::ADOBE_DEFLATE | comp::DEFLATE => {
            let bytes_per = if float { bits.div_ceil(8) as usize } else { 0 };
            let row_bytes = if float { cw * cpp * bytes_per } else { (cw * cpp * bits as usize).div_ceil(8) };
            let raw = inflate(src, row_bytes * ch)?;
            unpack_chunk(&raw, order, packing, cw, ch, cpp, bits, float, info.predictor)
        }
        other => Err(RawError::Unsupported(format!("TIFF compression {other}"))),
    }
}

/// Decode one baseline JPEG chunk to 8-bit samples: (samples, width, height, channels).
fn lossy_jpeg(src: &[u8], cpp: usize) -> Result<(Vec<u8>, usize, usize, usize)> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let cs = if cpp == 1 { ColorSpace::Luma } else { ColorSpace::RGB };
    let opts = DecoderOptions::default().set_max_width(1 << 16).set_max_height(1 << 16).set_strict_mode(false).jpeg_set_out_colorspace(cs);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(src), opts);
    let px = d.decode().map_err(|e| RawError::Corrupt(format!("lossy JPEG tile: {e}")))?;
    let info = d.info().ok_or_else(|| RawError::Corrupt("lossy JPEG tile without a header".into()))?;
    let (w, h) = (info.width as usize, info.height as usize);
    let c = if cpp == 1 { 1 } else { 3 };
    if w == 0 || h == 0 || px.len() < w * h * c {
        return Err(RawError::Corrupt("lossy JPEG tile: short output".into()));
    }
    Ok((px, w, h, c))
}

#[allow(clippy::too_many_arguments)]
fn unpack_chunk(
    src: &[u8],
    order: ByteOrder,
    packing: Packing,
    cw: usize,
    ch: usize,
    cpp: usize,
    bits: u32,
    float: bool,
    predictor: u16,
) -> Result<ChunkPx> {
    let row_n = cw * cpp;
    let (factor, fp) = match predictor {
        1 => (0, false),
        2 => (1, false),
        3 => (1, true),
        34892 => (2, false),
        34893 => (4, false),
        34894 => (2, true),
        34895 => (4, true),
        p => return Err(RawError::Unsupported(format!("predictor {p}"))),
    };
    if float {
        let bp = bits.div_ceil(8) as usize;
        let row_bytes = row_n * bp;
        let mut out = vec![0f32; row_n * ch];
        let mut rowbuf = vec![0u8; row_bytes];
        for y in 0..ch {
            let s = src.get(y * row_bytes..).unwrap_or(&[]);
            let len = s.len().min(row_bytes);
            rowbuf[..len].copy_from_slice(&s[..len]);
            rowbuf[len..].fill(0);
            let big = if fp {
                undo_float_predictor(&mut rowbuf, row_n, bp, cpp * factor);
                true
            } else {
                if factor > 0 {
                    return Err(RawError::Unsupported("integer predictor on float data".into()));
                }
                order == ByteOrder::Big
            };
            for (i, o) in out[y * row_n..(y + 1) * row_n].iter_mut().enumerate() {
                let b = &rowbuf[i * bp..(i + 1) * bp];
                *o = match (bp, big) {
                    (2, true) => f16_to_f32(u16::from_be_bytes([b[0], b[1]])),
                    (2, false) => f16_to_f32(u16::from_le_bytes([b[0], b[1]])),
                    (3, true) => f24_to_f32(u32::from_be_bytes([0, b[0], b[1], b[2]])),
                    (3, false) => f24_to_f32(u32::from_le_bytes([b[0], b[1], b[2], 0])),
                    (4, true) => f32::from_be_bytes([b[0], b[1], b[2], b[3]]),
                    _ => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                };
            }
        }
        return Ok(ChunkPx::F32(out));
    }
    if fp {
        return Err(RawError::Unsupported("floating-point predictor on integer data".into()));
    }
    let mut out = vec![0u16; row_n * ch];
    let (row_bytes, kind) = match (bits, packing) {
        (8, _) => (row_n, 0),
        (16, _) | (_, Packing::Word16) => (row_n * 2, 1),
        (_, Packing::Msb) => ((row_n * bits as usize).div_ceil(8), 2),
        (_, Packing::Lsb) => ((row_n * bits as usize).div_ceil(8), 3),
    };
    for y in 0..ch {
        let s = src.get(y * row_bytes..).unwrap_or(&[]);
        let s = &s[..s.len().min(row_bytes)];
        let row = &mut out[y * row_n..(y + 1) * row_n];
        match kind {
            0 => row.iter_mut().zip(s).for_each(|(o, &b)| *o = b as u16),
            1 => read_u16s(s, order, row),
            2 => unpack_msb(s, bits, row),
            _ => unpack_lsb(s, bits, row),
        }
        if factor > 0 {
            if bits == 8 {
                for i in cpp * factor..row.len() {
                    row[i] = (row[i] as u8).wrapping_add(row[i - cpp * factor] as u8) as u16;
                }
            } else {
                undo_diff_u16(row, cpp * factor);
            }
        }
    }
    Ok(ChunkPx::U16(out))
}

#[cfg(test)]
mod lossy_tests {
    use super::*;
    use lightcraft_tiff::image::Layout;

    /// A lossy-DNG-style image: two 16×8 tiles, each a baseline JPEG, the right one cut short
    /// by the image edge.
    #[test]
    fn lossy_jpeg_tiles_decode() {
        let (tw, th) = (16usize, 8usize);
        let tile = |shade: u8| {
            let px: Vec<u8> = (0..tw * th).flat_map(|i| [shade, (i % tw * 15) as u8, 200]).collect();
            let mut out = Vec::new();
            jpeg_encoder::Encoder::new(&mut out, 100).encode(&px, tw as u16, th as u16, jpeg_encoder::ColorType::Rgb).unwrap();
            out
        };
        let (a, b) = (tile(40), tile(220));
        let mut file = vec![0u8; 8];
        let oa = file.len() as u64;
        file.extend_from_slice(&a);
        let ob = file.len() as u64;
        file.extend_from_slice(&b);
        let info = ImageInfo {
            width: 24,
            height: 8,
            bits_per_sample: vec![8, 8, 8],
            samples_per_pixel: 3,
            compression: comp::LOSSY_JPEG,
            photometric: 34892,
            planar: 1,
            predictor: 1,
            sample_format: 1,
            new_subfile_type: 0,
            layout: Layout::Tiles { tile_width: tw as u32, tile_height: th as u32 },
            offsets: vec![oa, ob],
            byte_counts: vec![a.len() as u64, b.len() as u64],
        };
        let RawData::U16(v) = read_image(&file, &info, ByteOrder::Little, Packing::Msb).unwrap() else { panic!("integer samples") };
        assert_eq!(v.len(), 24 * 8 * 3);
        let px = |x: usize, y: usize| &v[(y * 24 + x) * 3..(y * 24 + x) * 3 + 3];
        assert!((px(2, 3)[0] as i32 - 40).abs() <= 3 && (px(2, 3)[2] as i32 - 200).abs() <= 3, "{:?}", px(2, 3));
        assert!((px(20, 3)[0] as i32 - 220).abs() <= 3, "second tile: {:?}", px(20, 3));
        assert!((px(10, 5)[1] as i32 - 150).abs() <= 6, "green ramp: {:?}", px(10, 5));
    }
}
