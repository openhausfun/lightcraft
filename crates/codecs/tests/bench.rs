//! Benchmark-ish, ignored by default. Run with:
//! `cargo test --release -p lightcraft-codecs --test bench -- --ignored --nocapture`

use lightcraft_codecs::*;
use std::time::Instant;

fn synthetic(w: usize, h: usize) -> Vec<u8> {
    // Smooth gradients + deterministic noise, roughly photo-like entropy.
    let mut s: u32 = 0x1234_5678;
    let mut v = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let n = (s & 15) as i32 - 8;
            let r = (x * 255 / w) as i32 + n;
            let g = (y * 255 / h) as i32 + n;
            let b = (((x / 37) ^ (y / 29)) & 0xFF) as i32 / 2 + 64 + n;
            v.extend_from_slice(&[r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]);
        }
    }
    v
}

fn time<T>(label: &str, mp: f64, reps: usize, mut f: impl FnMut() -> T) -> T {
    let mut out = f();
    let t = Instant::now();
    for _ in 0..reps {
        out = f();
    }
    let dt = t.elapsed().as_secs_f64() / reps as f64;
    if mp > 0.0 {
        println!("{label:<48} {:>8.2} ms  {:>7.1} MP/s", dt * 1e3, mp / dt);
    } else {
        println!("{label:<48} {:>8.2} ms", dt * 1e3);
    }
    out
}

#[test]
#[ignore]
fn bench_24mp_jpeg() {
    let (w, h) = (6000usize, 4000usize);
    let mp = (w * h) as f64 / 1e6;
    let px = synthetic(w, h);
    let img = EncodeImage::new(w as u32, h as u32, 3, Samples::U8(&px));
    let plain =
        time("encode JPEG q90 4:2:0 (jpeg-encoder)", mp, 1, || encode_jpeg(&img, 90, ChromaSubsampling::S420, &EncodeMeta::default()).unwrap());
    println!("  size {:.1} MB", plain.len() as f64 / 1e6);

    time("decode full → linear Rgb32f", mp, 3, || decode(&plain, DecodeOptions::default()).unwrap());
    time("decode fit 2048 (DCT 1/2 + resample)", mp, 3, || decode(&plain, DecodeOptions::fit(2048, 2048)).unwrap());
    time("decode_thumbnail 256 (DCT 1/8, no preview)", mp, 5, || decode_thumbnail(&plain, 256).unwrap());

    // Camera-style file: 320×213 EXIF thumbnail covers a 256 px request.
    let small: Vec<u8> = synthetic(320, 213);
    let thumb = encode_jpeg(&EncodeImage::new(320, 213, 3, Samples::U8(&small)), 80, ChromaSubsampling::S420, &EncodeMeta::default()).unwrap();
    let exif = exif_with_thumbnail(&thumb);
    let cam = encode_jpeg(&img, 90, ChromaSubsampling::S420, &EncodeMeta { exif: Some(&exif), ..Default::default() }).unwrap();
    let t = time("decode_thumbnail 256 (EXIF preview)", mp, 20, || decode_thumbnail(&cam, 256).unwrap());
    assert_eq!(t.source, ThumbnailSource::ExifThumbnail);

    let d = decode(&plain, DecodeOptions::default()).unwrap();
    time("to_working (sRGB → Rec.2020 matrix)", mp, 3, || to_working(&d));
    let rgba = d.to_srgb8();
    time("encode PNG 8-bit", mp, 1, || encode_png(&EncodeImage::rgba8(&rgba), &EncodeMeta::default()).unwrap());
    time("encode TIFF 8-bit deflate", mp, 1, || encode_tiff(&EncodeImage::rgba8(&rgba), TiffCompression::Deflate, &EncodeMeta::default()).unwrap());
}

fn exif_with_thumbnail(jpeg: &[u8]) -> Vec<u8> {
    let mut v = b"II*\0".to_vec();
    v.extend_from_slice(&8u32.to_le_bytes());
    v.extend_from_slice(&0u16.to_le_bytes());
    v.extend_from_slice(&14u32.to_le_bytes());
    // IFD1 at 14: 2 entries, data at 44
    v.extend_from_slice(&2u16.to_le_bytes());
    for (tag, val) in [(0x0201u16, 44u32), (0x0202, jpeg.len() as u32)] {
        v.extend_from_slice(&tag.to_le_bytes());
        v.extend_from_slice(&4u16.to_le_bytes());
        v.extend_from_slice(&1u32.to_le_bytes());
        v.extend_from_slice(&val.to_le_bytes());
    }
    v.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(v.len(), 44);
    v.extend_from_slice(jpeg);
    v
}
