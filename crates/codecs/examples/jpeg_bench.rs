//! Compare the parallel JPEG encoder with `jpeg-encoder` (size, time, PSNR):
//! `cargo run --release -p lightcraft-codecs --example jpeg_bench [image]` (default: 6000×4000 synthetic).
use lightcraft_codecs::ChromaSubsampling;
use std::time::Instant;

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    10.0 * (255.0f64.powi(2) / mse.max(1e-9)).log10()
}

fn best<T>(n: usize, mut f: impl FnMut() -> T) -> (T, f64) {
    let mut out = None;
    let mut t = f64::MAX;
    for _ in 0..n {
        let s = Instant::now();
        let r = f();
        t = t.min(s.elapsed().as_secs_f64() * 1e3);
        out = Some(r);
    }
    (out.unwrap(), t)
}

fn main() {
    let (w, h, rgb) = match std::env::args().nth(1) {
        Some(p) => {
            let img = image::open(p).expect("open").to_rgb8();
            (img.width() as usize, img.height() as usize, img.into_raw())
        }
        None => {
            let (w, h) = (6000, 4000);
            let mut v = Vec::with_capacity(w * h * 3);
            for y in 0..h {
                for x in 0..w {
                    let t = (((x as f32 / 37.0).sin() * (y as f32 / 23.0).cos()) * 0.5 + 0.5) * 255.0;
                    v.extend_from_slice(&[(x * 255 / w) as u8, t as u8, ((x ^ y) & 0xff) as u8]);
                }
            }
            (w, h, v)
        }
    };
    for (sub, js) in
        [(ChromaSubsampling::S444, jpeg_encoder::SamplingFactor::R_4_4_4), (ChromaSubsampling::S420, jpeg_encoder::SamplingFactor::R_4_2_0)]
    {
        let (ours, t_ours) = best(5, || lightcraft_codecs::jpeg_par::encode(&rgb, w, h, 3, 90, sub, &[]));
        let (theirs, t_theirs) = best(3, || {
            let mut out = Vec::new();
            let mut e = jpeg_encoder::Encoder::new(&mut out, 90);
            e.set_sampling_factor(js);
            e.encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).unwrap();
            out
        });
        let dec = |b: &[u8]| jpeg_decoder::Decoder::new(b).decode().unwrap();
        println!(
            "{w}x{h} {sub:?} q90: parallel {t_ours:.0} ms, {} KB, PSNR {:.2} dB | jpeg-encoder {t_theirs:.0} ms, {} KB, PSNR {:.2} dB",
            ours.len() / 1024,
            psnr(&rgb, &dec(&ours)),
            theirs.len() / 1024,
            psnr(&rgb, &dec(&theirs))
        );
    }
}
