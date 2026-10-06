//! Sample unpacking: bit packing, byte orders, 16/24/32-bit floats, Deflate and TIFF / DNG predictors.
//!
//! Sources: TIFF 6.0 §14 (Differencing Predictor), Adobe Photoshop TIFF Technical Note 3 (floating-point
//! predictor), DNG 1.7 (predictors 34892–34895, bit packing, 16/24-bit floats), RFC 1950/1951 (zlib/deflate).

use crate::{RawError, Result};
use lightcraft_tiff::ByteOrder;

/// Unpack `out.len()` MSB-first packed samples of `bits` (1..=16) from `src` (TIFF bit order).
pub fn unpack_msb(src: &[u8], bits: u32, out: &mut [u16]) {
    debug_assert!((1..=16).contains(&bits));
    let mut acc: u64 = 0;
    let mut nacc: u32 = 0;
    let mut it = src.iter();
    let mask = (1u64 << bits) - 1;
    for o in out.iter_mut() {
        while nacc < bits {
            acc = (acc << 8) | *it.next().unwrap_or(&0) as u64;
            nacc += 8;
        }
        *o = ((acc >> (nacc - bits)) & mask) as u16;
        nacc -= bits;
    }
}

/// Unpack LSB-first packed samples (bits taken from the low end of little-endian words).
pub fn unpack_lsb(src: &[u8], bits: u32, out: &mut [u16]) {
    let mut acc: u64 = 0;
    let mut nacc: u32 = 0;
    let mut it = src.iter();
    let mask = (1u64 << bits) - 1;
    for o in out.iter_mut() {
        while nacc < bits {
            acc |= (*it.next().unwrap_or(&0) as u64) << nacc;
            nacc += 8;
        }
        *o = (acc & mask) as u16;
        acc >>= bits;
        nacc -= bits;
    }
}

pub fn read_u16s(src: &[u8], order: ByteOrder, out: &mut [u16]) {
    for (o, c) in out.iter_mut().zip(src.as_chunks::<2>().0) {
        *o = order.u16([c[0], c[1]]);
    }
}

/// IEEE 754 half → f32.
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign
        } else {
            // subnormal: normalise
            let mut e = 127 - 15 + 1;
            let mut m = man;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3ff) << 13)
        }
    } else if exp == 31 {
        sign | 0x7f80_0000 | (man << 13)
    } else {
        sign | ((exp + 127 - 15) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// DNG 24-bit float (1 sign, 7 exponent bias 63, 16 mantissa) → f32.
pub fn f24_to_f32(v: u32) -> f32 {
    let sign = (v >> 23) & 1;
    let exp = (v >> 16) & 0x7f;
    let man = v & 0xffff;
    let bits = if exp == 0 && man == 0 {
        sign << 31
    } else if exp == 0x7f {
        (sign << 31) | 0x7f80_0000 | (man << 7)
    } else if exp == 0 {
        // denormal: value = man * 2^(1-63-16)
        let v = man as f32 * 2f32.powi(1 - 63 - 16);
        return if sign == 1 { -v } else { v };
    } else {
        (sign << 31) | ((exp + 127 - 63) << 23) | (man << 7)
    };
    f32::from_bits(bits)
}

/// Inflate a zlib stream, refusing to produce more than `limit` bytes.
pub fn inflate(src: &[u8], limit: usize) -> Result<Vec<u8>> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(src, limit).map_err(|e| RawError::Corrupt(format!("deflate: {e:?}")))
}

/// Undo horizontal differencing on one row of integer samples with a sample `stride` (cpp × factor).
pub fn undo_diff_u16(row: &mut [u16], stride: usize) {
    for i in stride..row.len() {
        row[i] = row[i].wrapping_add(row[i - stride]);
    }
}

pub fn undo_diff_u8(row: &mut [u8], stride: usize) {
    for i in stride..row.len() {
        row[i] = row[i].wrapping_add(row[i - stride]);
    }
}

/// Undo the floating-point predictor on one row: `row` holds `n` samples of `bytes_per` bytes, byte-differenced
/// with stride `stride` samples, then stored as byte planes (most significant first). Returns the samples as
/// big-endian byte groups in place.
pub fn undo_float_predictor(row: &mut [u8], n: usize, bytes_per: usize, stride: usize) {
    undo_diff_u8(row, stride);
    let tmp = row.to_vec();
    for i in 0..n {
        for b in 0..bytes_per {
            if let (Some(dst), Some(&s)) = (row.get_mut(i * bytes_per + b), tmp.get(b * n + i)) {
                *dst = s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packing() {
        // 12-bit MSB: 0xABC, 0xDEF -> AB CD EF
        let mut o = [0u16; 2];
        unpack_msb(&[0xab, 0xcd, 0xef], 12, &mut o);
        assert_eq!(o, [0xabc, 0xdef]);
        unpack_lsb(&[0xbc, 0xfa, 0xde], 12, &mut o);
        assert_eq!(o, [0xabc, 0xdef]);
        let mut o = [0u16; 3];
        unpack_msb(&[0b1010_1100, 0b0100_0000], 3, &mut o);
        assert_eq!(o, [0b101, 0b011, 0b000]);
        let mut o = [0u16; 2];
        read_u16s(&[1, 2, 3, 4], ByteOrder::Big, &mut o);
        assert_eq!(o, [0x0102, 0x0304]);
        // short source: pads with zeros
        let mut o = [9u16; 4];
        unpack_msb(&[0xff], 14, &mut o);
        assert_eq!(o, [0x3fc0, 0, 0, 0]);
    }

    #[test]
    fn floats() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert!(f16_to_f32(0x7c00).is_infinite());
        // 24-bit: 1.0 = exp 63, mantissa 0
        assert_eq!(f24_to_f32(63 << 16), 1.0);
        assert_eq!(f24_to_f32((1 << 23) | (64 << 16) | 0x8000), -3.0);
        assert_eq!(f24_to_f32(0), 0.0);
    }

    #[test]
    fn predictors() {
        let mut r = [1u16, 1, 1, 1, 65535, 2];
        undo_diff_u16(&mut r, 2);
        assert_eq!(r, [1, 1, 2, 2, 1, 4]);
        // float predictor: two 32-bit samples 1.0 (3f800000), 2.0 (40000000), stride 1
        let planes: [u8; 8] = [0x3f, 0x40, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut row = planes;
        for i in (1..row.len()).rev() {
            row[i] = row[i].wrapping_sub(row[i - 1]);
        }
        undo_float_predictor(&mut row, 2, 4, 1);
        assert_eq!(f32::from_be_bytes([row[0], row[1], row[2], row[3]]), 1.0);
        assert_eq!(f32::from_be_bytes([row[4], row[5], row[6], row[7]]), 2.0);
    }

    #[test]
    fn inflate_limit() {
        let z = miniz_oxide::deflate::compress_to_vec_zlib(&[7u8; 1000], 6);
        assert_eq!(inflate(&z, 1000).unwrap(), vec![7u8; 1000]);
        assert!(inflate(&z, 10).is_err());
        assert!(inflate(&[1, 2, 3], 10).is_err());
    }
}
