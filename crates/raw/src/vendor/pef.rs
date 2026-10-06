//! Pentax PEF — uncompressed and Pentax Huffman-compressed (compression 65535).
//!
//! Sources: TIFF 6.0 (container), ITU-T T.81 (JPEG) Annex H / F.1.2.1 for the lossless "difference category +
//! additional bits" coding, the ExifTool Pentax tag-name documentation (maker-note `0x0200` BlackPoint, `0x0201`
//! WhitePoint i.e. WB levels, `0x0220` HuffmanTable) and our own black-box analysis of CC0 samples from
//! raw.pixls.us (K10D, K-5 II s, K-3):
//!
//! - The Huffman table is stored in the file (maker note `0x0220`, in the maker note's byte order): a `u16` `d`,
//!   12 further bytes, then `n = d + 12` `u16` codes left-aligned in 12 bits, then `n` code lengths (bytes).
//!   Symbol `i` is the difference category `i` (T.81 Table H.2: the number of additional bits).
//! - The bit stream (MSB-first, no byte stuffing, continuous across rows) codes one difference per pixel; the
//!   additional bits follow T.81 F.1.2.1 (a leading 0 means a negative value).
//! - Prediction: each pixel is predicted from the previous same-colour pixel in its row (two to the left); the
//!   first two pixels of a row are predicted from the first two pixels of the previous row of the same parity
//!   (starting at 0). Found by trying candidate predictors and checking the decoded rows are continuous.
//! - Maker note `0x0038`/`0x0039`: the image area's left/top and width/height (verified against the data).

use super::{black_from_columns, white_from_data};

use crate::tiffraw::{Packing, read_image_in};
use crate::{BlackLevel, Cfa, ColorData, Mode, OpcodeLists, RawData, RawError, RawFormat, RawImage, Rect, Result};
use lightcraft_geom::Orientation;
use lightcraft_tiff::image::chunk_bytes;
use lightcraft_tiff::{ByteOrder, Tiff, makernote, tags as t};

const CROP_ORIGIN: u16 = 0x0038;
const CROP_SIZE: u16 = 0x0039;
const BLACK_POINT: u16 = 0x0200;
const WB_LEVELS: u16 = 0x0201;
const HUFFMAN: u16 = 0x0220;

/// A canonical-by-table Huffman decoder with a 12-bit lookup.
pub(crate) struct Huffman {
    /// Indexed by the next 12 bits: (symbol, length); length 0 = invalid.
    lut: Vec<(u8, u8)>,
}

impl Huffman {
    /// From `(left-aligned 12-bit code, length)` per symbol.
    pub fn new(codes: &[(u16, u8)]) -> Result<Huffman> {
        let mut lut = vec![(0u8, 0u8); 4096];
        for (sym, &(code, len)) in codes.iter().enumerate() {
            if len == 0 {
                continue;
            }
            if len > 12 || sym > 16 {
                return Err(RawError::Unsupported(format!("PEF Huffman code of length {len}")));
            }
            let first = (code >> (12 - len) << (12 - len)) as usize;
            for e in &mut lut[first..first + (1 << (12 - len))] {
                if e.1 == 0 {
                    *e = (sym as u8, len);
                }
            }
        }
        Ok(Huffman { lut })
    }
}

/// MSB-first bit reader that yields zeros past the end.
pub(crate) struct Bits<'a> {
    src: &'a [u8],
    pos: usize,
    acc: u64,
    n: u32,
}

impl<'a> Bits<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Bits { src, pos: 0, acc: 0, n: 0 }
    }
    #[inline]
    fn fill(&mut self) {
        while self.n <= 56 {
            let b = self.src.get(self.pos).copied().unwrap_or(0);
            self.pos += 1;
            self.acc |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }
    #[inline]
    pub fn peek(&mut self, k: u32) -> u32 {
        if self.n < k {
            self.fill();
        }
        (self.acc >> (64 - k)) as u32
    }
    #[inline]
    pub fn skip(&mut self, k: u32) {
        self.acc <<= k;
        self.n -= k;
    }
    #[inline]
    pub fn get(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        let v = self.peek(k);
        self.skip(k);
        v
    }
    /// Bits handed out so far (may exceed the source length: zeros are read past its end).
    pub fn consumed_bits(&self) -> usize {
        self.pos * 8 - self.n as usize
    }
    pub fn overrun(&self) -> bool {
        self.pos > self.src.len() + 8
    }
}

/// Decode one difference: Huffman category then T.81 additional bits.
#[inline]
pub(crate) fn diff(bits: &mut Bits, h: &Huffman) -> Option<i32> {
    let (sym, len) = h.lut[bits.peek(12) as usize];
    if len == 0 {
        return None;
    }
    bits.skip(len as u32);
    let s = sym as u32;
    if s == 0 {
        return Some(0);
    }
    if s > 16 {
        return None;
    }
    let v = bits.get(s) as i32;
    Some(if v < 1 << (s - 1) { v - (1 << s) + 1 } else { v })
}

/// Parse the maker-note Huffman table.
pub(crate) fn table(b: &[u8], order: ByteOrder) -> Result<Huffman> {
    let u16_at = |i: usize| b.get(i..i + 2).map(|s| order.u16([s[0], s[1]]));
    let d = u16_at(0).ok_or_else(|| RawError::Corrupt("PEF Huffman table too short".into()))? as usize;
    let n = d + 12;
    if n > 17 || b.len() < 14 + 3 * n {
        return Err(RawError::Corrupt(format!("PEF Huffman table: {n} symbols in {} bytes", b.len())));
    }
    let codes: Vec<(u16, u8)> = (0..n).map(|i| (u16_at(14 + 2 * i).unwrap_or(0) & 0x0fff, b[14 + 2 * n + i])).collect();
    Huffman::new(&codes)
}

/// Decode a `w × h` Huffman-coded image.
pub(crate) fn decode_huffman(src: &[u8], h: &Huffman, w: usize, hgt: usize, bits_per: u32) -> Result<Vec<u16>> {
    let mut out = vec![0u16; w * hgt];
    let mut bits = Bits::new(src);
    let mut vpred = [[0i32; 2]; 2];
    let max = (1i32 << bits_per.min(16)) - 1;
    for y in 0..hgt {
        let mut hpred = [0i32; 2];
        let row = &mut out[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let d = diff(&mut bits, h).ok_or_else(|| RawError::Corrupt(format!("PEF: invalid Huffman code at row {y}")))?;
            let p = if x < 2 {
                vpred[y & 1][x] += d;
                hpred[x] = vpred[y & 1][x];
                hpred[x]
            } else {
                hpred[x & 1] += d;
                hpred[x & 1]
            };
            *o = p.clamp(0, max) as u16;
        }
        if bits.overrun() {
            return Err(RawError::Corrupt(format!("PEF: data ends at row {y} of {hgt}")));
        }
    }
    Ok(out)
}

/// The image area without all-dark leading/trailing columns (masked or empty), kept at even offsets.
fn dark_trimmed(d: &[u16], w: usize, h: usize) -> Rect {
    if w < 256 || h == 0 {
        return Rect::new(0, 0, w, h);
    }
    let step = (h / 256).max(1);
    let col_mean = |x: usize| (0..h).step_by(step).map(|y| d[y * w + x] as f64).sum::<f64>() / h.div_ceil(step) as f64;
    let interior = (w / 4..w * 3 / 4).step_by(w / 64).map(col_mean).sum::<f64>() / 32.0;
    let dark = interior * 0.05;
    let mut left = 0;
    while left < 64 && col_mean(left) < dark {
        left += 1;
    }
    let mut right = w;
    while right > w - 64 && col_mean(right - 1) < dark {
        right -= 1;
    }
    let left = left.next_multiple_of(2);
    Rect::new(left, 0, (right - left) & !1, h)
}

pub(crate) fn decode(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    let tiff = Tiff::parse(bytes)?;
    let ifd0 = &tiff.ifds[0];
    let info = ifd0.image()?;
    let (w, hgt) = (info.width as usize, info.height as usize);
    let bits = info.bits() as u32;
    if w == 0 || hgt == 0 || w.saturating_mul(hgt) > crate::MAX_SAMPLES {
        return Err(RawError::Limit("image too large"));
    }
    let make = ifd0.string(t::MAKE).unwrap_or_default();
    let mn =
        tiff.exif().and_then(|e| e.get(t::MAKER_NOTE)).and_then(|e| makernote::parse_makernote(bytes, e.offset, e.count() as u64, tiff.order, &make));
    let pair = |tag: u16| mn.as_ref().and_then(|m| m.ifd.u64s(tag)).filter(|v| v.len() == 2).map(|v| (v[0] as usize, v[1] as usize));
    let tagged = match (pair(CROP_ORIGIN), pair(CROP_SIZE)) {
        (Some((x, y)), Some((cw, ch))) if cw > 0 && ch > 0 && x + cw <= w && y + ch <= hgt => Some(Rect::new(x, y, cw, ch)),
        _ => None,
    };
    // without crop tags the image area is found from the samples (dark borders)
    let read = if tagged.is_some() { mode } else { Mode::Full };
    let data = match info.compression {
        65535 => {
            let mn = mn.as_ref().ok_or_else(|| RawError::Corrupt("compressed PEF without maker note".into()))?;
            let tb = mn.ifd.bytes(HUFFMAN).ok_or_else(|| RawError::Unsupported("compressed PEF without a Huffman table".into()))?;
            let huff = table(tb, mn.order)?;
            let chunks = info.chunks(bytes.len() as u64);
            let first = chunks.first().ok_or_else(|| RawError::Corrupt("PEF without image data".into()))?;
            let mut c = *first;
            c.len = chunks.iter().map(|c| c.len).sum::<u64>().max(first.len);
            let src = chunk_bytes(bytes, &c)
                .or_else(|| bytes.get(first.offset as usize..))
                .ok_or_else(|| RawError::Corrupt("PEF data outside file".into()))?;
            if read == Mode::Full { RawData::U16(decode_huffman(src, &huff, w, hgt, bits)?) } else { RawData::U16(Vec::new()) }
        }
        1 => {
            let strip: u64 = info.chunks(bytes.len() as u64).iter().map(|c| c.len).sum();
            let packing = if strip >= (w * hgt * 2) as u64 { Packing::Word16 } else { Packing::Msb };
            read_image_in(read, bytes, &info, tiff.order, packing)?
        }
        c => return Err(RawError::Unsupported(format!("PEF compression {c}"))),
    };
    let RawData::U16(ref samples) = data else { return Err(RawError::Unsupported("float PEF".into())) };

    let active = match tagged {
        Some(r) => r,
        None => dark_trimmed(samples, w, hgt),
    };
    let cfa = match (ifd0.u64s(t::CFA_REPEAT_PATTERN_DIM).as_deref(), ifd0.bytes(t::CFA_PATTERN_EP)) {
        (Some([2, 2]), Some(p)) if p.len() == 4 && p.iter().all(|&c| c <= 2) => Cfa { width: 2, height: 2, pattern: p.to_vec() },
        _ => Cfa::bayer_static("BGGR"),
    };
    let black = match mn.as_ref().and_then(|m| m.ifd.f64s(BLACK_POINT)).as_deref() {
        Some(v @ [_, _, _, _]) => {
            // RGGB-ordered levels → per CFA position at the active area origin
            let a = cfa.shifted(active.x, active.y);
            let values = a.pattern.iter().map(|&c| [v[0], (v[1] + v[2]) / 2.0, v[3]][c as usize] as f32).collect();
            BlackLevel { repeat_rows: 2, repeat_cols: 2, values, ..Default::default() }
        }
        _ if active.x >= 4 => black_from_columns(samples, w, 0..active.x.saturating_sub(2), 0..hgt, active),
        _ => BlackLevel::uniform(0.0),
    };
    let wb = mn.as_ref().and_then(|m| m.ifd.f64s(WB_LEVELS)).filter(|v| v.len() == 4 && v.iter().all(|x| *x > 0.0)).map(|v| {
        let g = (v[1] + v[2]) / 2.0;
        [(v[0] / g) as f32, 1.0, (v[3] / g) as f32]
    });
    let white = white_from_data(samples, bits);
    let mut metadata = lightcraft_meta::from_tiff(&tiff);
    metadata.width = Some(active.width as u32);
    metadata.height = Some(active.height as u32);
    let img = RawImage {
        format: RawFormat::Pef,
        width: w,
        height: hgt,
        cpp: 1,
        data,
        cfa: Some(cfa),
        bits,
        black,
        white: vec![white],
        active_area: active,
        crop: Rect::new(0, 0, active.width, active.height),
        orientation: Orientation::from_exif(ifd0.u16(t::ORIENTATION).unwrap_or(1)),
        color: ColorData::default(),
        wb_multipliers: wb,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata,
    };
    img.validate_for(mode)?;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our own prefix code for categories 0..=14: lengths 2,2,3,3,4,5,..., codes assigned canonically.
    fn test_table() -> (Vec<u8>, Vec<(u16, u8)>) {
        let lens: [u8; 15] = [3, 2, 2, 3, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12];
        let mut order: Vec<usize> = (0..15).collect();
        order.sort_by_key(|&i| (lens[i], i));
        let mut codes = vec![(0u16, 0u8); 15];
        let (mut code, mut prev) = (0u32, lens[order[0]]);
        for (k, &i) in order.iter().enumerate() {
            if k > 0 {
                code = (code + 1) << (lens[i] - prev);
            }
            prev = lens[i];
            codes[i] = ((code << (12 - lens[i])) as u16, lens[i]);
        }
        let mut b = vec![0u8, 3];
        b.extend_from_slice(&[0; 12]);
        for c in &codes {
            b.extend_from_slice(&c.0.to_be_bytes());
        }
        b.extend(codes.iter().map(|c| c.1));
        (b, codes)
    }

    fn encode(px: &[u16], w: usize, codes: &[(u16, u8)]) -> Vec<u8> {
        let mut bits: Vec<bool> = Vec::new();
        let put = |v: u32, n: u8, bits: &mut Vec<bool>| (0..n).rev().for_each(|i| bits.push(v >> i & 1 == 1));
        let mut vpred = [[0i32; 2]; 2];
        for (y, row) in px.chunks(w).enumerate() {
            let mut hpred = [0i32; 2];
            for (x, &v) in row.iter().enumerate() {
                let pred = if x < 2 { vpred[y & 1][x] } else { hpred[x & 1] };
                let d = v as i32 - pred;
                if x < 2 {
                    vpred[y & 1][x] = v as i32;
                }
                hpred[x & 1] = v as i32;
                let s = if d == 0 { 0 } else { 32 - d.unsigned_abs().leading_zeros() } as u8;
                let (c, l) = codes[s as usize];
                put((c >> (12 - l)) as u32, l, &mut bits);
                if s > 0 {
                    let extra = if d < 0 { (d - 1) as u32 & ((1 << s) - 1) } else { d as u32 };
                    put(extra, s, &mut bits);
                }
            }
        }
        bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |a, (i, &b)| a | (b as u8) << (7 - i))).collect()
    }

    #[test]
    fn huffman_round_trip() {
        let (tb, codes) = test_table();
        let h = table(&tb, ByteOrder::Big).unwrap();
        let (w, hgt) = (37usize, 9usize);
        let px: Vec<u16> = (0..w * hgt).map(|i| ((i * 7919) % 16384) as u16).collect();
        let src = encode(&px, w, &codes);
        assert_eq!(decode_huffman(&src, &h, w, hgt, 14).unwrap(), px);
        // truncated data is reported, not a panic
        assert!(decode_huffman(&src[..src.len() / 3], &h, w, hgt, 14).is_err());
        assert!(table(&tb[..20], ByteOrder::Big).is_err());
    }
}
