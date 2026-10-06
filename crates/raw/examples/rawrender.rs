//! `cargo run --release -p lightcraft-raw --example rawrender -- IN OUT.tif [bilinear|ppg|ahd] [max_edge]`
//!
//! Quick visual check: develop → as-shot WB + camera matrix → sRGB, oriented, box-downscaled, 8-bit TIFF.

use lightcraft_color::{REC2020, SRGB, transfer::encode_srgb8};
use lightcraft_raw::{Method, color, decode};
use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, Value, tags};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).expect("read");
    let method = match a.get(2).map(String::as_str) {
        Some("bilinear") => Method::Bilinear,
        Some("ppg") => Method::Ppg,
        _ => Method::Ahd,
    };
    let max_edge: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1600);
    let raw = decode(&bytes).expect("decode");
    let mut img = raw.develop(method).expect("develop");
    let xy = color::as_shot_white_xy(&raw);
    let t = color::camera_transform(&raw, xy);
    let wb = t.wb;
    lightcraft_raw::highlight::reconstruct(&mut img, wb, 0.99);
    let to_srgb = REC2020.to_space(&SRGB).mul(&t.matrix);
    let gain = 2f32.powf(t.baseline_exposure as f32);
    let img = img.map(|p| {
        let q = to_srgb.apply_f32([p[0] * wb[0], p[1] * wb[1], p[2] * wb[2]]);
        // simple highlight roll-off so clipped areas are not harsh
        q.map(|v| {
            let v = (v * gain).max(0.0);
            if v < 0.8 { v } else { 0.8 + 0.2 * (1.0 - (-(v - 0.8) / 0.2).exp()) }
        })
    });
    let img = img.oriented(raw.orientation);
    let f = img.width.max(img.height).div_ceil(max_edge).max(1);
    let (w, h) = (img.width / f, img.height / f);
    let mut px = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let mut s = [0f32; 3];
            for dy in 0..f {
                for dx in 0..f {
                    let p = img.get(x * f + dx, y * f + dy);
                    for c in 0..3 {
                        s[c] += p[c];
                    }
                }
            }
            for c in 0..3 {
                px.push(encode_srgb8(s[c] / (f * f) as f32));
            }
        }
    }
    let mut ifd = IfdBuilder::new();
    ifd.set(tags::IMAGE_WIDTH, Value::Long(vec![w as u32]));
    ifd.set(tags::IMAGE_LENGTH, Value::Long(vec![h as u32]));
    ifd.set(tags::BITS_PER_SAMPLE, Value::Short(vec![8, 8, 8]));
    ifd.set(tags::SAMPLES_PER_PIXEL, Value::Short(vec![3]));
    ifd.set(tags::PHOTOMETRIC, Value::Short(vec![2]));
    ifd.set(tags::COMPRESSION, Value::Short(vec![1]));
    ifd.set_image(ImageData::Strips { rows_per_strip: h as u32, strips: vec![px] });
    std::fs::write(&a[1], TiffWriter::default().write(&[ifd]).unwrap()).expect("write");
    println!("{}x{} wb {:?} xy ({:.4},{:.4}) fallback {}", w, h, wb, xy.x, xy.y, t.matrix_is_fallback);
}
