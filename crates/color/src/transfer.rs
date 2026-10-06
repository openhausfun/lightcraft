//! Transfer functions (OETF/EOTF) and fast 8-bit display encoding.

use std::sync::OnceLock;

#[inline]
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

#[inline]
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

pub fn gamma_to_linear(v: f32, g: f32) -> f32 {
    v.max(0.0).powf(g)
}

pub fn linear_to_gamma(v: f32, g: f32) -> f32 {
    v.max(0.0).powf(1.0 / g)
}

const PQ_M1: f64 = 2610.0 / 16384.0;
const PQ_M2: f64 = 2523.0 / 4096.0 * 128.0;
const PQ_C1: f64 = 3424.0 / 4096.0;
const PQ_C2: f64 = 2413.0 / 4096.0 * 32.0;
const PQ_C3: f64 = 2392.0 / 4096.0 * 32.0;

/// SMPTE ST 2084 inverse EOTF: absolute luminance / 10000 cd/m² → code value 0..1.
pub fn pq_encode(y: f64) -> f64 {
    let y = y.clamp(0.0, 1.0).powf(PQ_M1);
    ((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y)).powf(PQ_M2)
}

/// SMPTE ST 2084 EOTF: code value → luminance / 10000 cd/m².
pub fn pq_decode(e: f64) -> f64 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / PQ_M2);
    ((p - PQ_C1).max(0.0) / (PQ_C2 - PQ_C3 * p)).powf(1.0 / PQ_M1)
}

const HLG_A: f64 = 0.178_832_77;
const HLG_B: f64 = 0.284_668_92;
const HLG_C: f64 = 0.559_910_73;

/// ITU-R BT.2100 HLG OETF (scene linear 0..1 → signal).
pub fn hlg_encode(e: f64) -> f64 {
    let e = e.max(0.0);
    if e <= 1.0 / 12.0 { (3.0 * e).sqrt() } else { HLG_A * (12.0 * e - HLG_B).ln() + HLG_C }
}

pub fn hlg_decode(s: f64) -> f64 {
    if s <= 0.5 { s * s / 3.0 } else { (((s - HLG_C) / HLG_A).exp() + HLG_B) / 12.0 }
}

const ENC_BITS: usize = 14;
const ENC_N: usize = 1 << ENC_BITS;

fn enc_lut() -> &'static [u8] {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..ENC_N)
            .map(|i| {
                // index = sqrt-spaced domain for precision in the shadows
                let t = i as f32 / (ENC_N - 1) as f32;
                let lin = t * t;
                (linear_to_srgb(lin) * 255.0 + 0.5) as u8
            })
            .collect()
    })
}

/// Fast linear (0..1, clamped) → sRGB 8-bit using a sqrt-domain LUT (max error ≤ 1 code value).
#[inline]
pub fn encode_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let i = (v.sqrt() * (ENC_N - 1) as f32 + 0.5) as usize;
    enc_lut()[i.min(ENC_N - 1)]
}

fn dec_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// sRGB 8-bit → linear.
#[inline]
pub fn decode_srgb8(v: u8) -> f32 {
    dec_lut()[v as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_roundtrip() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn encode8_matches_exact() {
        for i in 0..=4096 {
            let lin = i as f32 / 4096.0;
            let exact = (linear_to_srgb(lin) * 255.0).round() as i32;
            assert!((encode_srgb8(lin) as i32 - exact).abs() <= 1, "{lin}");
        }
        for v in 0..=255u8 {
            assert_eq!(encode_srgb8(decode_srgb8(v)), v);
        }
    }

    #[test]
    fn pq_hlg_roundtrip() {
        for i in 0..=50 {
            let y = i as f64 / 50.0;
            assert!((pq_decode(pq_encode(y)) - y).abs() < 1e-9);
            assert!((hlg_decode(hlg_encode(y)) - y).abs() < 1e-9);
        }
        // 100 cd/m² ≈ PQ 0.508
        assert!((pq_encode(0.01) - 0.508).abs() < 0.002);
    }
}
