//! Nikon Huffman-compressed NEF image data (TIFF compression 34713): "lossless compressed" and "lossy compressed"
//! (type 1 and type 2) at 12 and 14 bits.
//!
//! **Clean-room.** No raw-decoder source code (dcraw, LibRaw, rawspeed, rawloader/rawler, darktable, RawTherapee,
//! libopenraw, ExifTool's Perl code, …) was read or consulted. What we used:
//!
//! - Prose format descriptions: Laurent Clévy, "Nikon Electronic File (NEF) format" (<http://lclevy.free.fr/nef/>),
//!   the maker-note tag table and the "0x96 (linearization table) tag format" table (version bytes, four `u16`
//!   predictor seeds at offset 2, curve size at 10, curve at 12, split value at 562 for lossy type 2, "lossy type
//!   2 reads an incomplete table and interpolation is required"); Bill Claff, "NEF Compression"
//!   (<https://www.photonstophotos.net/NikonInfo/NEF_Compression.htm>): a lossy encoding curve followed by a
//!   non-adaptive (fixed-table) Huffman stage, and the number of encoded values per camera (683, 769, 1025, 2753,
//!   3073, 4097 …), which confirmed our curve interpolation; the libopenraw format notes
//!   (<https://libopenraw.freedesktop.org/formats/nef/>, prose only); ExifTool's Nikon tag-name documentation
//!   (`0x0093` NEFCompression, `0x0096` NEFLinearizationTable); ITU-T T.81 Annex F/H for the "difference category
//!   + additional bits" coding and canonical (BITS/HUFFVAL) Huffman codes.
//! - Our own black-box analysis of CC0 samples from raw.pixls.us. Nikon's Huffman tables are not stored in the
//!   files and no prose source lists them, so they were recovered from the data: several cameras (D5100, D300,
//!   D700) were shot with the same scene in compressed and uncompressed modes; a beam search assigned code words
//!   to difference categories so that the decoded compressed image matched the uncompressed one (likelihood of
//!   the decoded values given the reference, minus the bits consumed, plus a description-length penalty per new
//!   code word). Each table was then verified on ~45 files from ~35 bodies (D40 … D850, Df, Z 6, Z 50, 1 J1): with
//!   the right table the whole strip decodes and ends within the last byte of the data, with every value inside
//!   the encoded range. The predictor and difference coding are the same as Pentax PEF (see `pef.rs`).
//!
//! Format, as established:
//! - One strip, MSB-first bit stream, no byte stuffing, continuous across rows. Per pixel a canonical Huffman code
//!   gives the category `n`, followed by `n` additional bits (T.81: a leading 0 means a negative difference).
//! - Each pixel is predicted from the same-colour pixel two to the left; the first two pixels of a row from the
//!   first two pixels of the previous row of the same parity, starting at the seeds of maker note `0x0096`.
//! - Lossless (`0x0096` version `0x46`): the decoded values are the samples. Lossy (version `0x44`): they index a
//!   curve. Type 1 (`0x44 0x10`) stores the full curve; type 2 (`0x44 0x20`, `0x44 0x40`) stores 257 points,
//!   one every `2^bits / 256` codes, linearly interpolated.
//! - Values in the maker note are in the maker note's byte order (newer bodies write little-endian notes).
//!
//! Not supported (returned as [`RawError::Unsupported`], the embedded preview is used instead):
//! - "Lossy after split" files (non-zero split row at `0x0096` offset 562): from that row on a different table
//!   is used which our analysis has not recovered yet.
//! - Other `0x0096` versions, bit depths other than 12/14.
//! - Code words that never occurred in any sample (the lossy 12-bit 8-bit code word `11111110`, and code words
//!   longer than 9 bits in the lossy tables) are rejected as corrupt; they cannot code a difference within the
//!   lossy value range anyway.

use super::pef::{Bits, Huffman, diff};
use crate::{MAX_SAMPLES, RawError, Result};
use lightcraft_tiff::ByteOrder;

/// A table code word whose symbol was never observed (see the module docs).
const UNSEEN: u8 = u8::MAX;

/// Canonical Huffman table: number of codes of length 1..=16 and the symbols (difference categories) in code order.
struct Table {
    counts: &'static [u8],
    symbols: &'static [u8],
}

const LOSSLESS_12: Table = Table { counts: &[0, 1, 4, 2, 3, 1, 2], symbols: &[5, 4, 6, 3, 7, 2, 8, 1, 9, 0, 10, 11, 12] };
const LOSSLESS_14: Table = Table { counts: &[0, 1, 4, 2, 2, 3, 1, 2], symbols: &[7, 6, 8, 5, 9, 4, 10, 3, 11, 12, 2, 0, 1, 13, 14] };
const LOSSY_12: Table = Table { counts: &[0, 1, 5, 1, 1, 1, 1, 1, 1], symbols: &[5, 4, 3, 6, 2, 7, 1, 0, 8, 9, UNSEEN, 10] };
const LOSSY_14: Table = Table { counts: &[0, 1, 4, 3, 1, 1, 1, 1, 1], symbols: &[5, 6, 4, 7, 8, 3, 9, 2, 1, 0, 10, 11, 12] };

impl Table {
    /// `(left-aligned 12-bit code, length)` per symbol, for [`Huffman::new`].
    fn codes(&self) -> Vec<(u16, u8)> {
        let mut out = vec![(0u16, 0u8); 17];
        let mut code = 0u32;
        let mut symbols = self.symbols.iter();
        for (i, &n) in self.counts.iter().enumerate() {
            let len = i as u32 + 1;
            for _ in 0..n {
                if let Some(&s) = symbols.next()
                    && let Some(slot) = out.get_mut(s as usize)
                {
                    *slot = ((code << (12 - len)) as u16, len as u8);
                }
                code += 1;
            }
            code <<= 1;
        }
        out
    }
}

/// How the decoded values map to samples.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Encoding {
    /// The decoded values are the samples (`0..2^bits`).
    Lossless,
    /// The decoded values index this curve.
    Lossy(Vec<u16>),
}

/// Maker note `0x0096` (NEF linearization table), parsed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DecodeTable {
    pub encoding: Encoding,
    /// Predictor seeds `[row parity][column]`.
    pub seeds: [[i32; 2]; 2],
}

/// Parse maker note `0x0096` for a `bits`-bit image. `order` is the maker note's byte order.
pub(crate) fn parse_table(t: &[u8], order: ByteOrder, bits: u32) -> Result<DecodeTable> {
    let u16_at = |o: usize| t.get(o..o + 2).and_then(|s| <[u8; 2]>::try_from(s).ok()).map(|b| order.u16(b));
    let short = || RawError::Corrupt(format!("NEF: linearization table too short ({} bytes)", t.len()));
    if bits != 12 && bits != 14 {
        return Err(RawError::Unsupported(format!("Nikon compressed NEF with {bits}-bit samples")));
    }
    let (v0, v1) = (t.first().copied().ok_or_else(short)?, t.get(1).copied().ok_or_else(short)?);
    let mut seeds = [[0i32; 2]; 2];
    for (i, s) in seeds.iter_mut().flatten().enumerate() {
        *s = u16_at(2 + 2 * i).ok_or_else(short)? as i32;
    }
    let encoding = match (v0, v1) {
        (0x46, _) => Encoding::Lossless,
        (0x44, 0x10 | 0x20 | 0x40) => {
            let n = u16_at(10).ok_or_else(short)? as usize;
            let points: Vec<u16> = (0..n).map(|i| u16_at(12 + 2 * i)).collect::<Option<_>>().ok_or_else(short)?;
            if n < 2 {
                return Err(RawError::Corrupt(format!("NEF: linearization curve of {n} points")));
            }
            if v1 == 0x10 {
                Encoding::Lossy(points)
            } else {
                let split = u16_at(562).unwrap_or(0);
                if split != 0 {
                    return Err(RawError::Unsupported(format!(
                        "Nikon \"lossy after split\" compressed NEF (split at row {split}) is not decoded yet"
                    )));
                }
                // type 2: `n` points, one every 2^bits / (n - 1) codes
                let range = 1usize << bits;
                if n - 1 > range || !range.is_multiple_of(n - 1) {
                    return Err(RawError::Unsupported(format!("NEF: {n}-point lossy curve for {bits}-bit data")));
                }
                let step = range / (n - 1);
                let mut curve = Vec::with_capacity(range + 1);
                for pair in points.windows(2) {
                    let &[a, b] = pair else { continue };
                    let (a, b) = (a as u32, b as u32);
                    for k in 0..step as u32 {
                        let s = step as u32;
                        curve.push(((a * (s - k) + b * k) / s) as u16);
                    }
                }
                curve.extend(points.last().copied());
                Encoding::Lossy(curve)
            }
        }
        _ => return Err(RawError::Unsupported(format!("Nikon compressed NEF version {v0:#04x} {v1:#04x}"))),
    };
    Ok(DecodeTable { encoding, seeds })
}

/// Decode a `w × h` Nikon Huffman-compressed strip `src` with `bits`-bit samples.
pub(crate) fn decode(src: &[u8], w: usize, h: usize, bits: u32, table: &DecodeTable) -> Result<Vec<u16>> {
    if bits != 12 && bits != 14 {
        return Err(RawError::Unsupported(format!("Nikon compressed NEF with {bits}-bit samples")));
    }
    let n = w.checked_mul(h).filter(|&n| n > 0 && n <= MAX_SAMPLES).ok_or(RawError::Limit("NEF image size"))?;
    // every code word is at least 2 bits long: a strip that can't hold the image is truncated (also bounds the
    // allocation by the file size)
    if n / 4 > src.len() {
        return Err(RawError::Corrupt(format!("NEF: {} bytes of compressed data for {w}x{h} pixels", src.len())));
    }
    let huff = Huffman::new(
        &match (&table.encoding, bits) {
            (Encoding::Lossless, 12) => LOSSLESS_12,
            (Encoding::Lossless, _) => LOSSLESS_14,
            (Encoding::Lossy(_), 12) => LOSSY_12,
            (Encoding::Lossy(_), _) => LOSSY_14,
        }
        .codes(),
    )?;
    let (lut, max): (&[u16], i32) = match &table.encoding {
        Encoding::Lossless => (&[], (1i32 << bits) - 1),
        Encoding::Lossy(curve) => (curve, curve.len() as i32 - 1),
    };
    let mut out = vec![0u16; n];
    let mut stream = Bits::new(src);
    let mut vpred = table.seeds;
    let mut clipped = 0usize;
    for (y, row) in out.chunks_exact_mut(w).enumerate() {
        let seeds = vpred.get_mut(y & 1).ok_or(RawError::Limit("NEF row"))?;
        let mut hpred = [0i32; 2];
        for (x, o) in row.iter_mut().enumerate() {
            let d = diff(&mut stream, &huff).ok_or_else(|| RawError::Corrupt(format!("NEF: invalid Huffman code at row {y}")))?;
            let p = if x < 2 {
                let s = seeds.get_mut(x).ok_or(RawError::Limit("NEF column"))?;
                *s += d;
                hpred[x & 1] = *s;
                *s
            } else {
                hpred[x & 1] += d;
                hpred[x & 1]
            };
            let v = p.clamp(0, max);
            clipped += (v != p) as usize;
            *o = if lut.is_empty() { v as u16 } else { lut.get(v as usize).copied().unwrap_or(0) };
        }
        // the camera pads the strip; reading more than a few bytes past its end means truncated or corrupt data
        if stream.consumed_bits() > src.len() * 8 + 64 {
            return Err(RawError::Corrupt(format!("NEF: compressed data ends at row {y} of {h}")));
        }
    }
    // a wrong table / corrupt stream drifts out of range quickly; real files stay inside (a handful of edge pixels)
    if clipped > n / 1000 + 16 {
        return Err(RawError::Corrupt(format!("NEF: {clipped} decoded values out of range")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit writer for the test encoder.
    #[derive(Default)]
    struct Writer {
        out: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl Writer {
        fn put(&mut self, v: u32, len: u32) {
            for i in (0..len).rev() {
                self.acc = (self.acc << 1) | ((v >> i) & 1) as u64;
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc as u8);
                    self.acc = 0;
                    self.n = 0;
                }
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push((self.acc << (8 - self.n)) as u8);
            }
            self.out.extend_from_slice(&[0; 4]);
            self.out
        }
    }

    fn category(d: i32) -> u32 {
        32 - d.unsigned_abs().leading_zeros()
    }

    /// Encode `values` (decoded domain: samples or curve indices) the way the camera does.
    fn encode(values: &[i32], w: usize, table: &Table, seeds: [[i32; 2]; 2]) -> Vec<u8> {
        let codes = table.codes();
        let mut wr = Writer::default();
        let mut vpred = seeds;
        for (y, row) in values.chunks(w).enumerate() {
            for (x, &v) in row.iter().enumerate() {
                let pred = if x < 2 { vpred[y & 1][x] } else { row[x - 2] };
                if x < 2 {
                    vpred[y & 1][x] = v;
                }
                let d = v - pred;
                let k = category(d);
                let (code, len) = codes[k as usize];
                assert!(len > 0, "no code for category {k}");
                wr.put(code as u32 >> (12 - len), len as u32);
                let extra = if d < 0 { d + (1 << k) - 1 } else { d };
                wr.put(extra as u32, k);
            }
        }
        wr.finish()
    }

    /// Deterministic test image: smooth gradient + texture + hard edges, within `0..=max`.
    fn image(w: usize, h: usize, max: i32, seed: u32) -> Vec<i32> {
        let mut s = seed;
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as i32, (i / w) as i32);
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (s >> 24) as i32 % 9 - 4;
                let base = (x * 37 + y * 11) % (max / 2) + if (x / 7 + y / 5) % 3 == 0 { max / 3 } else { 0 };
                (base + noise).clamp(0, max)
            })
            .collect()
    }

    fn lossless(bits: u32) -> DecodeTable {
        let s = 1 << (bits - 3);
        DecodeTable { encoding: Encoding::Lossless, seeds: [[s, s], [s, s]] }
    }

    #[test]
    fn tables_are_complete_prefix_codes() {
        for t in [LOSSLESS_12, LOSSLESS_14, LOSSY_12, LOSSY_14] {
            assert_eq!(t.counts.iter().map(|&c| c as usize).sum::<usize>(), t.symbols.len());
            let kraft: f64 = t.counts.iter().enumerate().map(|(i, &c)| c as f64 / (1u64 << (i + 1)) as f64).sum();
            assert!(kraft <= 1.0);
            assert!(Huffman::new(&t.codes()).is_ok());
        }
        assert_eq!(LOSSLESS_14.codes()[7], (0b00 << 10, 2));
        assert_eq!(LOSSLESS_14.codes()[0], (0b111110 << 6, 6));
        assert_eq!(LOSSY_12.codes()[10], (0b111111110 << 3, 9));
    }

    #[test]
    fn lossless_roundtrip_12_and_14() {
        for (bits, table) in [(12, LOSSLESS_12), (14, LOSSLESS_14)] {
            let (w, h) = (64, 9);
            let max = (1 << bits) - 1;
            let mut img = image(w, h, max, bits);
            // extreme jumps exercise the longest categories
            img[5] = 0;
            img[7] = max;
            img[w + 9] = max;
            let t = lossless(bits);
            let src = encode(&img, w, &table, t.seeds);
            let out = decode(&src, w, h, bits, &t).unwrap();
            assert_eq!(out, img.iter().map(|&v| v as u16).collect::<Vec<_>>(), "{bits}-bit");
        }
    }

    fn lossy_table(bits: u32, order: ByteOrder) -> Vec<u8> {
        // version 0x44 0x20, seeds, 257 points of a concave curve, padding up to the split value (0)
        let mut t = vec![0x44, 0x20];
        let put = |t: &mut Vec<u8>, v: u16| t.extend_from_slice(&if order == ByteOrder::Big { v.to_be_bytes() } else { v.to_le_bytes() });
        for _ in 0..4 {
            put(&mut t, 300);
        }
        put(&mut t, 257);
        let max = ((1u32 << bits) - 1) as f64;
        for i in 0..257u32 {
            put(&mut t, (max * (i as f64 / 64.0).min(1.0).sqrt()) as u16);
        }
        t.resize(624, 0);
        t
    }

    #[test]
    fn lossy_type2_curve_and_roundtrip() {
        for (bits, table) in [(12u32, LOSSY_12), (14, LOSSY_14)] {
            for order in [ByteOrder::Big, ByteOrder::Little] {
                let t = parse_table(&lossy_table(bits, order), order, bits).unwrap();
                assert_eq!(t.seeds, [[300, 300], [300, 300]]);
                let Encoding::Lossy(curve) = &t.encoding else { panic!() };
                let step = (1usize << bits) / 256;
                assert_eq!(curve.len(), (1 << bits) + 1);
                assert_eq!(curve[64 * step], (1 << bits) - 1);
                assert!(curve.windows(2).all(|p| p[0] <= p[1]));
                // the encoded range is the curve's rising part
                let (w, h) = (48, 7);
                let img = image(w, h, 64 * step as i32 - 1, 7);
                let src = encode(&img, w, &table, t.seeds);
                let out = decode(&src, w, h, bits, &t).unwrap();
                assert_eq!(out, img.iter().map(|&v| curve[v as usize]).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn lossy_type1_full_curve() {
        let mut t = vec![0x44, 0x10, 0, 10, 0, 10, 0, 10, 0, 10, 0, 5];
        for v in [0u16, 100, 400, 900, 1600] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        let parsed = parse_table(&t, ByteOrder::Big, 12).unwrap();
        assert_eq!(parsed.encoding, Encoding::Lossy(vec![0, 100, 400, 900, 1600]));
        let img = vec![0, 4, 1, 3, 2, 2, 4, 0];
        let src = encode(&img, 4, &LOSSY_12, parsed.seeds);
        assert_eq!(decode(&src, 4, 2, 12, &parsed).unwrap(), vec![0, 1600, 100, 900, 400, 400, 1600, 0]);
    }

    #[test]
    fn rejects_unsupported_and_malformed_tables() {
        let unsupported = |r: Result<DecodeTable>| matches!(r, Err(RawError::Unsupported(_)));
        let corrupt = |r: Result<DecodeTable>| matches!(r, Err(RawError::Corrupt(_)));
        let mut split = lossy_table(12, ByteOrder::Big);
        split[562..564].copy_from_slice(&345u16.to_be_bytes());
        assert!(unsupported(parse_table(&split, ByteOrder::Big, 12)));
        assert!(unsupported(parse_table(&[0x49, 0x30, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], ByteOrder::Big, 12)));
        assert!(unsupported(parse_table(&lossy_table(12, ByteOrder::Big), ByteOrder::Big, 16)));
        assert!(corrupt(parse_table(&[], ByteOrder::Big, 12)));
        assert!(corrupt(parse_table(&[0x46, 0x30, 8], ByteOrder::Big, 14)));
        assert!(corrupt(parse_table(&lossy_table(12, ByteOrder::Big)[..100], ByteOrder::Big, 12)));
        // 300 points don't divide the 12-bit range
        let mut odd = lossy_table(12, ByteOrder::Big);
        odd[10..12].copy_from_slice(&300u16.to_be_bytes());
        assert!(parse_table(&odd, ByteOrder::Big, 12).is_err());
        assert!(parse_table(&[0x44, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0], ByteOrder::Big, 12).is_err());
    }

    /// A minimal NEF: CFA SubIFD with one compressed strip, Exif maker note (`Nikon\0` v2 header + embedded TIFF in
    /// `mn_order`) holding the linearization table.
    fn nef_file(strip: Vec<u8>, w: u32, h: u32, bits: u16, table: Vec<u8>, mn_order: ByteOrder) -> Vec<u8> {
        use lightcraft_tiff::tags::{self as t, photometric};
        use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, Value};
        let mut mn = IfdBuilder::new();
        mn.set(0x0096, Value::Undefined(table));
        let mut note = b"Nikon\0\x02\x10\0\0".to_vec();
        note.extend(TiffWriter::new(mn_order, false).write(&[mn]).unwrap());
        let mut exif = IfdBuilder::new();
        exif.set(t::MAKER_NOTE, Value::Undefined(note));
        let mut raw = IfdBuilder::new();
        raw.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
        raw.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
        raw.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
        raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![bits]));
        raw.set(t::COMPRESSION, Value::Short(vec![t::compression::NIKON]));
        raw.set(t::PHOTOMETRIC, Value::Short(vec![photometric::CFA]));
        raw.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
        raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
        raw.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![strip] });
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::MAKE, Value::Ascii("NIKON CORPORATION".into()));
        ifd0.set(t::MODEL, Value::Ascii("NIKON TEST".into()));
        ifd0.set_child(t::EXIF_IFD, exif);
        ifd0.add_sub_ifd(raw);
        TiffWriter::new(ByteOrder::Big, false).write(&[ifd0]).unwrap()
    }

    #[test]
    fn decodes_through_the_nef_container() {
        // lossless 14-bit, big-endian maker note
        let (w, h) = (40usize, 6usize);
        let img = image(w, h, 16383, 11);
        let mut table = vec![0x46, 0x30];
        for _ in 0..4 {
            table.extend_from_slice(&2048u16.to_be_bytes());
        }
        table.resize(46, 0);
        let src = encode(&img, w, &LOSSLESS_14, [[2048; 2]; 2]);
        let bytes = nef_file(src, w as u32, h as u32, 14, table, ByteOrder::Big);
        let r = crate::decode(&bytes).unwrap();
        assert_eq!(r.data, crate::RawData::U16(img.iter().map(|&v| v as u16).collect()));
        assert_eq!(r.cfa.as_ref().unwrap().name(), "RGGB");
        assert_eq!(crate::probe_info(&bytes).unwrap(), r.info());
        // lossy 12-bit type 2, little-endian maker note (newer bodies)
        let table = lossy_table(12, ByteOrder::Little);
        let t = parse_table(&table, ByteOrder::Little, 12).unwrap();
        let Encoding::Lossy(curve) = &t.encoding else { panic!() };
        let img = image(w, h, 1023, 5);
        let src = encode(&img, w, &LOSSY_12, t.seeds);
        let r = crate::decode(&nef_file(src.clone(), w as u32, h as u32, 12, table.clone(), ByteOrder::Little)).unwrap();
        assert_eq!(r.data, crate::RawData::U16(img.iter().map(|&v| curve[v as usize]).collect()));
        // the split variant falls back to the preview
        let mut split = table;
        split[562..564].copy_from_slice(&3u16.to_le_bytes());
        let e = crate::decode(&nef_file(src, w as u32, h as u32, 12, split, ByteOrder::Little));
        assert!(matches!(e, Err(RawError::Unsupported(_))), "{e:?}");
    }

    #[test]
    fn truncated_and_corrupted_streams_error_never_panic() {
        let (w, h) = (64, 16);
        let t = lossless(14);
        let img = image(w, h, 16383, 3);
        let src = encode(&img, w, &LOSSLESS_14, t.seeds);
        // truncation: too short for the image, or ends early
        for cut in [0, 1, 10, src.len() / 2, src.len() - 40] {
            assert!(decode(&src[..cut], w, h, 14, &t).is_err(), "cut {cut}");
        }
        // garbage and bit flips: an error or (rarely) some image, never a panic
        let mut s = 12345u32;
        for i in 0..200 {
            let mut bad = src.clone();
            for _ in 0..1 + i % 8 {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let at = (s >> 8) as usize % bad.len();
                bad[at] ^= 1 << (s % 8);
            }
            let _ = decode(&bad, w, h, 14, &t);
            let noise: Vec<u8> = (0..src.len()).map(|k| ((k as u32).wrapping_mul(2_654_435_761) >> (i % 24)) as u8).collect();
            let _ = decode(&noise, w, h, 14, &t);
            let _ = decode(&noise, w, h, 12, &lossless(12));
        }
        // all-ones never forms a valid lossy 12-bit code
        assert!(decode(&[0xff; 64], 8, 8, 12, &parse_table(&lossy_table(12, ByteOrder::Big), ByteOrder::Big, 12).unwrap()).is_err());
        // absurd dimensions are limited before allocating
        assert!(decode(&src, usize::MAX, 2, 14, &t).is_err());
        assert!(decode(&src, 1 << 20, 1 << 20, 14, &t).is_err());
        assert!(decode(&src, 0, 2, 14, &t).is_err());
    }
}
