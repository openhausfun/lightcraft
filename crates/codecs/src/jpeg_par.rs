//! Parallel baseline JPEG encoder (ITU-T T.81 / JFIF).
//!
//! The image is cut into bands of MCU rows separated by restart markers (`DRI` + `RSTn`): DC
//! prediction restarts at every marker, so bands are entropy-coded independently on all cores and
//! concatenated. Quantization uses the example tables of T.81 Annex K scaled by the usual 1–100
//! quality convention; Huffman coding uses the Annex K example tables (interleaved baseline, readable
//! by every decoder). Supports grayscale and YCbCr 4:4:4 / 4:2:0.

use crate::encode::ChromaSubsampling;

/// T.81 Annex K.1 / K.2 example quantization tables (natural order).
const Q_LUMA: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56,
    68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];
const Q_CHROMA: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

/// Zig-zag position k → natural index.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43,
    36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// T.81 Annex K.3 example Huffman tables: (code counts per length 1..16, symbols).
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// Huffman code table: symbol → (code, length).
struct Huff {
    code: [(u16, u8); 256],
}

impl Huff {
    /// Canonical codes from counts (T.81 Annex C).
    fn new(bits: &[u8; 16], vals: &[u8]) -> Huff {
        let mut code = [(0u16, 0u8); 256];
        let (mut c, mut k) = (0u16, 0usize);
        for (len, &n) in bits.iter().enumerate() {
            for _ in 0..n {
                code[vals[k] as usize] = (c, len as u8 + 1);
                c += 1;
                k += 1;
            }
            c <<= 1;
        }
        Huff { code }
    }
}

fn scaled_quant(base: &[u16; 64], quality: u8) -> [u16; 64] {
    let q = quality.clamp(1, 100) as u32;
    let scale = if q < 50 { 5000 / q } else { 200 - 2 * q };
    base.map(|b| ((b as u32 * scale + 50) / 100).clamp(1, 255) as u16)
}

/// Entropy-coded segment writer with 0xFF byte stuffing.
struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn new() -> Bits {
        Bits { out: Vec::new(), acc: 0, n: 0 }
    }
    #[inline]
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code & ((1 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xFF {
                self.out.push(0);
            }
            self.n -= 8;
        }
        self.acc &= (1 << self.n) - 1;
    }
    /// Pad with 1-bits to a byte boundary.
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

/// AAN output scale factors: `s[0] = 1`, `s[k] = cos(kπ/16)·√2`.
fn aan_scale() -> [f32; 8] {
    std::array::from_fn(|k| if k == 0 { 1.0 } else { ((k as f64 * std::f64::consts::PI / 16.0).cos() * std::f64::consts::SQRT_2) as f32 })
}

/// Quantization reciprocals with the AAN scaling folded in: coefficient = out / (8·s[u]·s[v]·q).
fn recip_table(q: &[u16; 64]) -> [f32; 64] {
    let s = aan_scale();
    std::array::from_fn(|i| 1.0 / (q[i] as f32 * s[i / 8] * s[i % 8] * 8.0))
}

/// One 8-point forward DCT (Arai–Agui–Nakajima flowgraph, 5 multiplies) on `d[o + k·stride]`.
#[inline(always)]
fn aan_1d(d: &mut [f32; 64], o: usize, stride: usize) {
    let g = |k: usize| o + k * stride;
    let (t0, t7) = (d[g(0)] + d[g(7)], d[g(0)] - d[g(7)]);
    let (t1, t6) = (d[g(1)] + d[g(6)], d[g(1)] - d[g(6)]);
    let (t2, t5) = (d[g(2)] + d[g(5)], d[g(2)] - d[g(5)]);
    let (t3, t4) = (d[g(3)] + d[g(4)], d[g(3)] - d[g(4)]);
    // even part
    let (t10, t13, t11, t12) = (t0 + t3, t0 - t3, t1 + t2, t1 - t2);
    d[g(0)] = t10 + t11;
    d[g(4)] = t10 - t11;
    let z1 = (t12 + t13) * 0.707_106_77;
    d[g(2)] = t13 + z1;
    d[g(6)] = t13 - z1;
    // odd part
    let (t10, t11, t12) = (t4 + t5, t5 + t6, t6 + t7);
    let z5 = (t10 - t12) * 0.382_683_43;
    let z2 = 0.541_196_1 * t10 + z5;
    let z4 = 1.306_563 * t12 + z5;
    let z3 = t11 * 0.707_106_77;
    let (z11, z13) = (t7 + z3, t7 - z3);
    d[g(5)] = z13 + z2;
    d[g(3)] = z13 - z2;
    d[g(1)] = z11 + z4;
    d[g(7)] = z11 - z4;
}

/// Forward DCT of a level-shifted 8×8 block and quantization.
#[inline]
fn fdct_quant(block: &[f32; 64], recip: &[f32; 64], out: &mut [i16; 64]) {
    let mut d = *block;
    for r in 0..8 {
        aan_1d(&mut d, r * 8, 1);
    }
    for c in 0..8 {
        aan_1d(&mut d, c, 8);
    }
    for i in 0..64 {
        out[i] = (d[i] * recip[i]).round() as i16;
    }
}

#[inline]
fn magnitude(v: i32) -> (u32, u32) {
    let a = v.unsigned_abs();
    let size = 32 - a.leading_zeros();
    let bits = if v < 0 { (v - 1) as u32 } else { v as u32 };
    (size, bits & ((1u32 << size) - 1))
}

fn encode_block(w: &mut Bits, q: &[i16; 64], pred: &mut i32, dc: &Huff, ac: &Huff) {
    let d = q[0] as i32 - *pred;
    *pred = q[0] as i32;
    let (s, b) = magnitude(d);
    let (c, l) = dc.code[s as usize];
    w.put(c as u32, l as u32);
    w.put(b, s);
    let mut run = 0u32;
    for &zi in &ZIGZAG[1..] {
        let v = q[zi] as i32;
        if v == 0 {
            run += 1;
            continue;
        }
        while run >= 16 {
            let (c, l) = ac.code[0xF0];
            w.put(c as u32, l as u32);
            run -= 16;
        }
        let (s, b) = magnitude(v);
        let (c, l) = ac.code[((run << 4) | s) as usize];
        w.put(c as u32, l as u32);
        w.put(b, s);
        run = 0;
    }
    if run > 0 {
        let (c, l) = ac.code[0x00];
        w.put(c as u32, l as u32);
    }
}

/// Planar component samples (level-shifted to −128..127) with edge replication on read.
struct Plane {
    w: usize,
    h: usize,
    data: Vec<f32>,
}

impl Plane {
    #[inline]
    fn block(&self, bx: usize, by: usize, out: &mut [f32; 64]) {
        let (x0, y0) = (bx * 8, by * 8);
        if x0 + 8 <= self.w && y0 + 8 <= self.h {
            for y in 0..8 {
                let i = (y0 + y) * self.w + x0;
                out[y * 8..y * 8 + 8].copy_from_slice(&self.data[i..i + 8]);
            }
            return;
        }
        for y in 0..8 {
            let sy = (by * 8 + y).min(self.h - 1);
            let row = &self.data[sy * self.w..(sy + 1) * self.w];
            for x in 0..8 {
                out[y * 8 + x] = row[(bx * 8 + x).min(self.w - 1)];
            }
        }
    }
}

fn segment(out: &mut Vec<u8>, marker: u8, payload: &[u8]) {
    out.extend_from_slice(&[0xFF, marker]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(payload);
}

fn dht(out: &mut Vec<u8>, class_id: u8, bits: &[u8; 16], vals: &[u8]) {
    let mut p = vec![class_id];
    p.extend_from_slice(bits);
    p.extend_from_slice(vals);
    segment(out, 0xC4, &p);
}

/// Encode interleaved 8-bit samples (`channels` 1, 3 or 4; alpha is dropped).
pub fn encode(
    data: &[u8],
    width: usize,
    height: usize,
    channels: usize,
    quality: u8,
    sub: ChromaSubsampling,
    app_segments: &[(u8, Vec<u8>)],
) -> Vec<u8> {
    let gray = channels == 1;
    let (hs, vs) = match (gray, sub) {
        (true, _) | (false, ChromaSubsampling::S444) => (1usize, 1usize),
        (false, _) => (2, 2),
    };
    // colour conversion (JFIF YCbCr, full range, level-shifted) in one pass; chroma averaged 2×2 for 4:2:0
    let n = width * height;
    #[cfg(feature = "parallel")]
    use rayon::prelude::*;
    // 4:4:4: Y, Cb, Cr at full resolution in one pass; 4:2:0: Y here, chroma below straight from 2×2 RGB means
    let ncomp = if gray || hs == 2 { 1 } else { 3 };
    let mut full: Vec<Vec<f32>> = (0..ncomp).map(|_| vec![0f32; n]).collect();
    {
        let rows: Vec<(usize, Vec<&mut [f32]>)> = {
            let mut its: Vec<_> = full.iter_mut().map(|p| p.chunks_mut(width)).collect();
            (0..height).map_while(|y| Some((y, its.iter_mut().map(|it| it.next()).collect::<Option<_>>()?))).collect()
        };
        let conv = |(y, mut out): (usize, Vec<&mut [f32]>)| {
            let src = &data[y * width * channels..(y + 1) * width * channels];
            if gray {
                for (o, p) in out[0].iter_mut().zip(src.chunks_exact(channels)) {
                    *o = p[0] as f32 - 128.0;
                }
                return;
            }
            if out.len() == 1 {
                for (o, p) in out[0].iter_mut().zip(src.chunks_exact(channels)) {
                    *o = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32 - 128.0;
                }
                return;
            }
            for (x, p) in src.chunks_exact(channels).enumerate() {
                let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                out[0][x] = 0.299 * r + 0.587 * g + 0.114 * b - 128.0;
                out[1][x] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                out[2][x] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
            }
        };
        #[cfg(feature = "parallel")]
        rows.into_par_iter().for_each(conv);
        #[cfg(not(feature = "parallel"))]
        rows.into_iter().for_each(conv);
    }
    let mut it = full.into_iter();
    let mut planes = vec![Plane { w: width, h: height, data: it.next().unwrap_or_default() }];
    planes.extend(it.map(|c| Plane { w: width, h: height, data: c }));
    if !gray && hs == 2 {
        // conversion is linear: average RGB over 2×2 first, then convert once
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        let (mut cb, mut cr) = (vec![0f32; cw * ch], vec![0f32; cw * ch]);
        let down = |(cy, (ob, or)): (usize, (&mut [f32], &mut [f32]))| {
            let (y0, y1) = (2 * cy, (2 * cy + 1).min(height - 1));
            let (r0, r1) = (&data[y0 * width * channels..], &data[y1 * width * channels..]);
            for cx in 0..cw {
                let (x0, x1) = (2 * cx * channels, (2 * cx + 1).min(width - 1) * channels);
                let s = |k: usize| (r0[x0 + k] as f32 + r0[x1 + k] as f32 + r1[x0 + k] as f32 + r1[x1 + k] as f32) * 0.25;
                let (r, g, b) = (s(0), s(1), s(2));
                ob[cx] = -0.168_736 * r - 0.331_264 * g + 0.5 * b;
                or[cx] = 0.5 * r - 0.418_688 * g - 0.081_312 * b;
            }
        };
        #[cfg(feature = "parallel")]
        cb.par_chunks_mut(cw).zip(cr.par_chunks_mut(cw)).enumerate().for_each(down);
        #[cfg(not(feature = "parallel"))]
        cb.chunks_mut(cw).zip(cr.chunks_mut(cw)).enumerate().for_each(down);
        planes.push(Plane { w: cw, h: ch, data: cb });
        planes.push(Plane { w: cw, h: ch, data: cr });
    }

    let ql = scaled_quant(&Q_LUMA, quality);
    let qc = scaled_quant(&Q_CHROMA, quality);
    let (rl, rc) = (recip_table(&ql), recip_table(&qc));
    let (dcl, acl) = (Huff::new(&DC_LUMA_BITS, &DC_VALS), Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS));
    let (dcc, acc) = (Huff::new(&DC_CHROMA_BITS, &DC_VALS), Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS));

    let (mcu_w, mcu_h) = (8 * hs, 8 * vs);
    let (mx, my) = (width.div_ceil(mcu_w), height.div_ceil(mcu_h));
    // restart interval: whole MCU rows per band, ≤ 65535 MCUs, enough bands to keep all cores busy
    let rows_per_band = (my / 64).clamp(1, (65_535 / mx.max(1)).max(1));
    let bands: Vec<usize> = (0..my.div_ceil(rows_per_band)).collect();
    let encode_band = |band: &usize| -> Vec<u8> {
        let mut w = Bits::new();
        let mut pred = [0i32; 3];
        let (mut blk, mut q) = ([0f32; 64], [0i16; 64]);
        for mcu_y in band * rows_per_band..((band + 1) * rows_per_band).min(my) {
            for mcu_x in 0..mx {
                for vy in 0..vs {
                    for hx in 0..hs {
                        planes[0].block(mcu_x * hs + hx, mcu_y * vs + vy, &mut blk);
                        fdct_quant(&blk, &rl, &mut q);
                        encode_block(&mut w, &q, &mut pred[0], &dcl, &acl);
                    }
                }
                if !gray {
                    for k in 1..3 {
                        planes[k].block(mcu_x, mcu_y, &mut blk);
                        fdct_quant(&blk, &rc, &mut q);
                        encode_block(&mut w, &q, &mut pred[k], &dcc, &acc);
                    }
                }
            }
        }
        w.finish()
    };
    #[cfg(feature = "parallel")]
    let coded: Vec<Vec<u8>> = bands.par_iter().map(encode_band).collect();
    #[cfg(not(feature = "parallel"))]
    let coded: Vec<Vec<u8>> = bands.iter().map(encode_band).collect();

    // headers
    let mut out = Vec::with_capacity(coded.iter().map(Vec::len).sum::<usize>() + 4096);
    out.extend_from_slice(&[0xFF, 0xD8]);
    // JFIF APP0 first (callers may pass their own, e.g. with a print density)
    if !app_segments.iter().any(|(m, _)| *m == 0xE0) {
        segment(&mut out, 0xE0, &[b'J', b'F', b'I', b'F', 0, 1, 2, 0, 0, 1, 0, 1, 0, 0]);
    }
    for (marker, payload) in app_segments {
        segment(&mut out, *marker, payload);
    }
    let dqt = |id: u8, q: &[u16; 64]| -> Vec<u8> {
        let mut p = vec![id];
        p.extend(ZIGZAG.iter().map(|&i| q[i] as u8));
        p
    };
    segment(&mut out, 0xDB, &dqt(0, &ql));
    if !gray {
        segment(&mut out, 0xDB, &dqt(1, &qc));
    }
    let mut sof = vec![8];
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    if gray {
        sof.extend_from_slice(&[1, 1, 0x11, 0]);
    } else {
        sof.extend_from_slice(&[3, 1, ((hs as u8) << 4) | vs as u8, 0, 2, 0x11, 1, 3, 0x11, 1]);
    }
    segment(&mut out, 0xC0, &sof);
    dht(&mut out, 0x00, &DC_LUMA_BITS, &DC_VALS);
    dht(&mut out, 0x10, &AC_LUMA_BITS, &AC_LUMA_VALS);
    if !gray {
        dht(&mut out, 0x01, &DC_CHROMA_BITS, &DC_VALS);
        dht(&mut out, 0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALS);
    }
    let interval = (rows_per_band * mx) as u16;
    if coded.len() > 1 {
        segment(&mut out, 0xDD, &interval.to_be_bytes());
    }
    if gray {
        segment(&mut out, 0xDA, &[1, 1, 0x00, 0, 63, 0]);
    } else {
        segment(&mut out, 0xDA, &[3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
    }
    let last = coded.len().saturating_sub(1);
    for (i, c) in coded.iter().enumerate() {
        out.extend_from_slice(c);
        if i < last {
            out.extend_from_slice(&[0xFF, 0xD0 + (i % 8) as u8]);
        }
    }
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rgb(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let t = ((x as f32 / 13.0).sin() * 0.5 + 0.5) * 255.0;
                v.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, t as u8]);
            }
        }
        v
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
        10.0 * (255.0f64.powi(2) / mse.max(1e-9)).log10()
    }

    /// The AAN path matches the direct DCT-II definition.
    #[test]
    fn aan_matches_reference_dct() {
        let block: [f32; 64] = std::array::from_fn(|i| ((i * 37 % 255) as f32) - 128.0);
        let q = [1u16; 64];
        let mut out = [0i16; 64];
        fdct_quant(&block, &recip_table(&q), &mut out);
        for v in 0..8 {
            for u in 0..8 {
                let c = |k: usize| if k == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                let mut s = 0.0;
                for y in 0..8 {
                    for x in 0..8 {
                        s += block[y * 8 + x] as f64
                            * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                            * (((2 * y + 1) * v) as f64 * std::f64::consts::PI / 16.0).cos();
                    }
                }
                let f = 0.25 * c(u) * c(v) * s;
                assert!((out[v * 8 + u] as f64 - f).abs() <= 0.51, "({u},{v}): {} vs {f}", out[v * 8 + u]);
            }
        }
    }

    #[test]
    fn huffman_tables_are_consistent() {
        for (bits, n) in [(&AC_LUMA_BITS, AC_LUMA_VALS.len()), (&AC_CHROMA_BITS, AC_CHROMA_VALS.len()), (&DC_LUMA_BITS, 12), (&DC_CHROMA_BITS, 12)] {
            assert_eq!(bits.iter().map(|&b| b as usize).sum::<usize>(), n);
        }
    }

    #[test]
    fn decodes_with_independent_decoders_at_odd_sizes() {
        for (w, h, sub) in [
            (1, 1, ChromaSubsampling::S444),
            (37, 23, ChromaSubsampling::S420),
            (640, 1031, ChromaSubsampling::S420),
            (803, 600, ChromaSubsampling::S444),
        ] {
            let src = test_rgb(w, h);
            let jpg = encode(&src, w, h, 3, 92, sub, &[]);
            let mut d = jpeg_decoder::Decoder::new(&jpg[..]);
            let px = d.decode().expect("jpeg-decoder");
            let info = d.info().unwrap();
            assert_eq!((info.width as usize, info.height as usize), (w, h));
            if w * h > 64 {
                assert!(psnr(&src, &px) > 30.0, "{w}x{h} {sub:?}: psnr {}", psnr(&src, &px));
            }
            let z = zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(&jpg[..])).decode().expect("zune-jpeg");
            assert_eq!(z.len(), px.len());
        }
    }

    #[test]
    fn grayscale_and_quality_monotone() {
        let (w, h) = (300, 200);
        let src: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8 ^ ((i / w) as u8 & 0x10)).collect();
        let lo = encode(&src, w, h, 1, 30, ChromaSubsampling::S444, &[]);
        let hi = encode(&src, w, h, 1, 95, ChromaSubsampling::S444, &[]);
        assert!(lo.len() < hi.len());
        let px = jpeg_decoder::Decoder::new(&hi[..]).decode().unwrap();
        assert!(psnr(&src, &px) > 35.0);
    }
}
