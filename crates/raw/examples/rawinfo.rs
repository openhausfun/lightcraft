//! `cargo run --release -p lightcraft-raw --example rawinfo -- [--tree] FILE...`
//!
//! Prints the TIFF structure (with `--tree`), decode result, timings (MP/s) and basic statistics.

use lightcraft_raw::{Method, RawData, decode, embedded_preview, probe};
use lightcraft_tiff::{Ifd, Tiff, Value};
use std::time::Instant;

fn short(v: &Value) -> String {
    let s = match v {
        Value::Ascii(s) => format!("{s:?}"),
        Value::Byte(b) | Value::Undefined(b) => format!("{} bytes {:02x?}", b.len(), &b[..b.len().min(12)]),
        other => {
            let f = other.to_f64_vec();
            format!("{:?}{}", &f[..f.len().min(12)], if f.len() > 12 { format!(" …({})", f.len()) } else { String::new() })
        }
    };
    s.chars().take(160).collect()
}

fn dump(ifd: &Ifd, depth: usize, label: &str) {
    let pad = "  ".repeat(depth);
    println!("{pad}{label} @{} ({} entries)", ifd.offset, ifd.entries.len());
    for e in &ifd.entries {
        println!("{pad}  {:5} 0x{:04x} {:?}: {}", e.tag, e.tag, e.field_type(), short(&e.value));
    }
    for (i, s) in ifd.sub_ifds.iter().enumerate() {
        dump(s, depth + 1, &format!("SubIFD[{i}]"));
    }
    for (n, c) in [("Exif", &ifd.exif), ("GPS", &ifd.gps), ("Interop", &ifd.interop)] {
        if let Some(c) = c {
            dump(c, depth + 1, n);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tree = args.iter().any(|a| a == "--tree");
    for path in args.iter().filter(|a| !a.starts_with("--")) {
        let bytes = std::fs::read(path).expect("read");
        println!("== {path} ({:.1} MB) format {:?}", bytes.len() as f64 / 1e6, probe(&bytes));
        if tree && let Ok(t) = Tiff::parse(&bytes) {
            for (i, ifd) in t.ifds.iter().enumerate() {
                dump(ifd, 0, &format!("IFD{i}"));
            }
        }
        let t0 = Instant::now();
        match decode(&bytes) {
            Ok(r) => {
                let dt = t0.elapsed().as_secs_f64();
                let mp = (r.width * r.height) as f64 / 1e6;
                let (mn, mx, mean) = match &r.data {
                    RawData::U16(v) => {
                        let mn = *v.iter().min().unwrap_or(&0) as f64;
                        let mx = *v.iter().max().unwrap_or(&0) as f64;
                        (mn, mx, v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64)
                    }
                    RawData::F32(v) => (
                        v.iter().cloned().fold(f32::MAX, f32::min) as f64,
                        v.iter().cloned().fold(f32::MIN, f32::max) as f64,
                        v.iter().map(|&x| x as f64).sum::<f64>() / v.len() as f64,
                    ),
                };
                println!(
                    "  {}x{} cpp {} bits {} cfa {:?} black {:.1} white {:?} active {:?} crop {:?} orient {:?}",
                    r.width,
                    r.height,
                    r.cpp,
                    r.bits,
                    r.cfa.as_ref().map(|c| c.name()),
                    r.black.mean(),
                    r.white,
                    r.active_area,
                    r.crop,
                    r.orientation
                );
                println!(
                    "  data min {mn} max {mx} mean {mean:.1}; wb {:?}; opcodes {}/{}/{}",
                    r.wb_multipliers,
                    r.opcodes.list1.len(),
                    r.opcodes.list2.len(),
                    r.opcodes.list3.len()
                );
                println!("  decode {:.0} ms ({:.0} MP/s)", dt * 1e3, mp / dt);
                println!(
                    "  meta: {:?} {:?} lens {:?} iso {:?} f {:?} t {:?} date {:?}",
                    r.metadata.make,
                    r.metadata.model,
                    r.metadata.lens_model,
                    r.metadata.iso,
                    r.metadata.f_number,
                    r.metadata.exposure_display(),
                    r.metadata.capture_time.map(|d| d.to_iso())
                );
                let xy = lightcraft_raw::color::as_shot_white_xy(&r);
                let ct = lightcraft_raw::color::camera_transform(&r, xy);
                println!("  as-shot xy ({:.4}, {:.4}) wb {:?} fallback {}", xy.x, xy.y, ct.wb, ct.matrix_is_fallback);
                for m in [Method::Bilinear, Method::Ppg, Method::Ahd] {
                    let t1 = Instant::now();
                    match r.develop(m) {
                        Ok(img) => {
                            let dt = t1.elapsed().as_secs_f64();
                            let mut s = [0f64; 3];
                            for p in &img.data {
                                for c in 0..3 {
                                    s[c] += p[c] as f64;
                                }
                            }
                            let n = img.data.len() as f64;
                            println!(
                                "  develop {m:?}: {}x{} in {:.0} ms ({:.0} MP/s), mean rgb {:.3} {:.3} {:.3}",
                                img.width,
                                img.height,
                                dt * 1e3,
                                mp / dt,
                                s[0] / n,
                                s[1] / n,
                                s[2] / n
                            );
                        }
                        Err(e) => println!("  develop {m:?}: {e}"),
                    }
                }
            }
            Err(e) => println!("  decode error: {e}"),
        }
        match embedded_preview(&bytes) {
            Some(p) => println!("  preview: {} bytes", p.len()),
            None => println!("  preview: none"),
        }
    }
}
