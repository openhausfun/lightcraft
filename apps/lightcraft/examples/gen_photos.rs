//! Generate a folder of procedural test photos (JPEG) for import/startup benchmarks.
//!
//! `cargo run --release -p lightcraft --example gen_photos -- OUT_DIR [COUNT=200] [LONG_EDGE=2000]`
//!
//! Each photo is a `lightcraft-scenes` demo scene rendered through the pipeline with a varied
//! look (exposure, temperature, vibrance), so every file has distinct bytes. The output is our own
//! procedural content; keep it out of the repository (write to a scratch or gitignored folder).

use lightcraft_engine::Session;
use serde_json::json;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = std::path::PathBuf::from(args.next().expect("usage: gen_photos OUT_DIR [COUNT] [LONG_EDGE]"));
    let count: usize = args.next().and_then(|c| c.parse().ok()).unwrap_or(200);
    let edge: usize = args.next().and_then(|c| c.parse().ok()).unwrap_or(2000);
    std::fs::create_dir_all(&out).expect("create output folder");
    let mut s = Session::with_demo();
    let ids = s.visible_cloned();
    let t0 = std::time::Instant::now();
    for i in 0..count {
        let id = ids[i % ids.len()];
        let round = i / ids.len();
        s.execute("library.select", &json!({"ids": [id.0]})).expect("select");
        s.execute("develop.reset", &json!({})).expect("reset");
        let v = json!({
            "light.exposure": (round as f64 * 0.37).sin() * 0.8,
            "wb.temp": 6500.0 + (round as f64 * 1.3).cos() * 1500.0,
            "color.vibrance": (round * 13 % 50) as f64,
        });
        let _ = s.execute("develop.set", &json!({"values": v}));
        let img = s.render_now(id, edge, edge).expect("render");
        let bytes = lightcraft_codecs::encode_jpeg(
            &lightcraft_codecs::EncodeImage::rgba8(&img.image),
            88,
            lightcraft_codecs::ChromaSubsampling::S420,
            &lightcraft_codecs::EncodeMeta::default(),
        )
        .expect("encode");
        let dir = out.join(format!("roll-{:02}", i / 50 + 1));
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(format!("LCG{:05}.jpg", i + 1)), bytes).expect("write");
    }
    eprintln!("wrote {count} photos ({edge} px) to {} in {:.1} s", out.display(), t0.elapsed().as_secs_f64());
}
