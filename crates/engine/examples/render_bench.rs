//! End-to-end render benchmark (min of N runs, robust to a loaded machine):
//! `cargo run --release -p lightcraft-engine --example render_bench -- corpus/raw/arw-sony-a7m3-compressed.arw`
//! (`N=9` runs per scenario, `RAYON_NUM_THREADS=1` for algorithmic comparisons,
//! `LIGHTCRAFT_PROFILE=1` for per-stage timings).
//!
//! Without a file argument a procedural 6000×4000 source is used. Scenarios: a ~2.5 MP loupe render
//! of the 2560 px preview (cold, and with a warm stage cache while a slider is dragged), a draft,
//! and a full-size render + JPEG encode (export); `ONLY=batch` with several files times a full-size
//! export of each (decode + render + encode). Prints minimum wall-clock and minimum process CPU
//! time: on a shared machine the CPU time shows the work done, wall-clock also the wait for cores.
//! Each scenario runs on the CPU pipeline and, when a GPU adapter exists, on `lightcraft-gpu`
//! (second column; `LIGHTCRAFT_GPU=0` for the CPU only).
use std::sync::Arc;
use std::time::Instant;

use lightcraft_develop::DevelopSettings;
use lightcraft_engine::export::{ExportOptions, encode_image};
use lightcraft_engine::media::develop;
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;
use lightcraft_raster::resample::{Filter, fit};

/// Process CPU time in ms (all threads). The only `unsafe` is this libc clock read, in a dev-only example.
#[allow(unsafe_code)]
fn cpu_ms() -> f64 {
    #[cfg(unix)]
    {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `clock_gettime` only writes into the timespec we pass.
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
        ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 / 1e6
    }
    #[cfg(not(unix))]
    {
        0.0
    }
}

/// Minimum wall-clock and minimum CPU time (ms) over the runs.
struct T(f64, f64);

impl std::fmt::Display for T {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.1} ms wall, {:.1} ms cpu", self.0, self.1)
    }
}

fn best(n: usize, mut f: impl FnMut()) -> T {
    let mut b = T(f64::MAX, f64::MAX);
    for _ in 0..n {
        let (t, c) = (Instant::now(), cpu_ms());
        f();
        b.1 = b.1.min(cpu_ms() - c);
        b.0 = b.0.min(t.elapsed().as_secs_f64() * 1e3);
    }
    b
}

fn typical() -> DevelopSettings {
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.3;
    s.light.highlights = -40.0;
    s.light.shadows = 30.0;
    s.effects.clarity = 15.0;
    s.effects.texture = 10.0;
    s.effects.dehaze = 10.0;
    s.detail.nr_luminance = 30.0;
    s.detail.nr_color = 25.0;
    s.detail.sharpen_amount = 40.0;
    s
}

/// Max and mean |Δ| (8-bit, RGB) between two renders.
fn diff(a: &lightcraft_raster::Rgba8, b: &lightcraft_raster::Rgba8) -> (u8, f64) {
    let (mut max, mut sum) = (0u8, 0u64);
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            max = max.max(d);
            sum += d as u64;
        }
    }
    (max, sum as f64 / (a.data.len() * 3).max(1) as f64)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = std::env::var("N").ok().and_then(|v| v.parse().ok()).unwrap_or(5);
    let only = std::env::var("ONLY").unwrap_or_default();
    let run = |name: &str| only.is_empty() || only.split(',').any(|o| name.contains(o));
    if only == "source" {
        // opening a photo: where the time goes before the first sharp loupe (per file)
        for path in &args {
            let bytes = std::fs::read(path).expect("read");
            println!("{path}");
            let raw = lightcraft_raw::decode(&bytes).expect("decode");
            println!("  {}×{} cpp {} cfa {:?}", raw.width, raw.height, raw.cpp, raw.cfa.as_ref().map(|c| c.name()));
            println!("  {:<36} {}", "raw decode:", best(n, || drop(lightcraft_raw::decode(&bytes).expect("decode"))));
            println!("  {:<36} {}", "normalize:", best(n, || drop(raw.normalized().expect("norm"))));
            let norm = raw.normalized().expect("norm");
            println!("  {:<36} {}", "demosaic AHD:", best(n, || drop(lightcraft_raw::demosaic(&norm, lightcraft_raw::Method::Ahd))));
            if let Some(k) = lightcraft_engine::files::bin_factor(&raw, 2560) {
                println!("  {:<36} {}", format!("binned ×{k} (2560 preview):"), best(n, || drop(raw.develop_binned(k, 0.99))));
                let img = raw.develop_binned(k, 0.99).expect("bin").expect("binnable");
                let t = lightcraft_raw::color::camera_transform(&raw, lightcraft_raw::color::as_shot_white_xy(&raw));
                println!(
                    "  {:<36} {}",
                    "highlight reconstruct (binned):",
                    best(n, || {
                        let mut i = img.clone();
                        lightcraft_raw::highlight::reconstruct(&mut i, t.wb, 0.99);
                    })
                );
                println!("  {:<36} {}", "clone (binned):", best(n, || drop(img.clone())));
                println!("  {:<36} {}", "fit 2560 (binned):", best(n, || drop(fit(&img, 2560, 2560, Filter::Box))));
            }
            println!("  {:<36} {}", "embedded preview (2560):", best(n, || drop(lightcraft_engine::files::load_embedded_preview(&bytes, 2560))));
            for edge in [512usize, 2560, usize::MAX] {
                println!(
                    "  {:<36} {}",
                    format!("load_bytes({edge}):"),
                    best(n, || drop(lightcraft_engine::files::load_bytes(&bytes, edge).expect("load")))
                );
            }
        }
        return;
    }
    let (full, info) = match args.first() {
        Some(path) => {
            let bytes = std::fs::read(path).expect("read");
            let t = Instant::now();
            let r = lightcraft_engine::files::load_bytes(&bytes, usize::MAX).expect("decode");
            println!("decode full: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
            r
        }
        None => (
            Rgb32f::from_fn(6000, 4000, |x, y| {
                let v = ((x as f32 * 0.011).sin() * (y as f32 * 0.007).cos() * 0.5 + 0.5) * 0.4 + ((x * 31 + y * 17) % 13) as f32 * 0.002;
                [v, v * 0.9, v * 0.7]
            }),
            SourceInfo { raw: true, ..Default::default() },
        ),
    };
    let full = Arc::new(full);
    println!("source {}×{} ({:.1} MP), {n} runs each", full.width, full.height, (full.width * full.height) as f64 / 1e6);
    // GPU column: `LIGHTCRAFT_GPU=0` (or no adapter) prints only the CPU column.
    let t = Instant::now();
    let gpu = lightcraft_engine::gpu::available();
    match lightcraft_engine::gpu::adapter_name().filter(|_| gpu) {
        Some(a) => println!("gpu: {a} (device + kernels: {:.0} ms)", t.elapsed().as_secs_f64() * 1e3),
        None => println!("gpu: none"),
    }
    let backends: &[bool] = if gpu { &[false, true] } else { &[false] };
    let row = |name: &str, f: &mut dyn FnMut(bool)| {
        let cols: Vec<String> = backends.iter().map(|&g| format!("{}", best(n, || f(g)))).collect();
        println!("{name:<38} cpu: {}{}", cols[0], cols.get(1).map(|g| format!("  |  gpu: {g}")).unwrap_or_default());
    };
    let preview = Arc::new(fit(&full, 2560, 2560, Filter::Box));
    let s = typical();
    let view = RenderRequest::fit(1920, 1280);
    let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(1152, 768) };
    if gpu {
        let c = develop(&preview, &info, &s, &view, None, false).image;
        let g = develop(&preview, &info, &s, &view, None, true).image;
        let (max, mean) = diff(&c, &g);
        println!("gpu vs cpu, loupe 1920×1280 typical: max {max} LSB, mean {mean:.4} LSB");
    }

    if run("cold") {
        // a new photo: no cached stages (the GPU also uploads the preview)
        row("loupe 1920×1280 cold:", &mut |g| drop(develop(&preview, &info, &s, &view, None, g)));
        row("loupe draft 1152×768 cold:", &mut |g| drop(develop(&preview, &info, &s, &draft, None, g)));
    }
    let caches = [StageCache::default(), StageCache::default()];
    let mut k = 0.0;
    if run("drag") {
        for g in backends {
            drop(develop(&preview, &info, &s, &view, Some(&caches[*g as usize]), *g));
        }
        row("loupe 1920×1280 exposure drag (warm):", &mut |g| {
            let mut t = s.clone();
            k += 1.0;
            t.light.contrast = k % 50.0;
            t.light.exposure = 0.3 + k * 0.01;
            drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
        });
        for g in backends {
            drop(develop(&preview, &info, &s, &draft, Some(&caches[*g as usize]), *g));
        }
        row("loupe draft highlights drag (warm):", &mut |g| {
            let mut t = s.clone();
            k += 1.0;
            t.light.highlights = -40.0 + k % 50.0;
            drop(develop(&preview, &info, &t, &draft, Some(&caches[g as usize]), g));
        });
        row("loupe draft clarity drag (warm):", &mut |g| {
            let mut t = s.clone();
            k += 1.0;
            t.effects.clarity = 15.0 + k % 50.0;
            drop(develop(&preview, &info, &t, &draft, Some(&caches[g as usize]), g));
        });
        row("loupe draft NR drag (warm):", &mut |g| {
            let mut t = s.clone();
            k += 1.0;
            t.detail.nr_luminance = 30.0 + k % 50.0;
            drop(develop(&preview, &info, &t, &draft, Some(&caches[g as usize]), g));
        });
        row("loupe 1920×1280 NR drag (warm):", &mut |g| {
            let mut t = s.clone();
            k += 1.0;
            t.detail.nr_luminance = 30.0 + k % 50.0;
            drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
        });
    }
    if run("finish") {
        // per-pixel stage cost by feature (warm cache: exposure changes only)
        let variants: [(&str, fn(&mut DevelopSettings)); 6] = [
            ("typical", |_| {}),
            ("- dehaze", |t| t.effects.dehaze = 0.0),
            ("- clarity", |t| t.effects.clarity = 0.0),
            ("- texture/sharpen", |t| {
                t.effects.texture = 0.0;
                t.detail.sharpen_amount = 0.0;
            }),
            ("- highlights/shadows", |t| {
                t.light.highlights = 0.0;
                t.light.shadows = 0.0;
            }),
            ("+ vibrance/saturation", |t| {
                t.color.vibrance = 20.0;
                t.color.saturation = 10.0;
            }),
        ];
        for (name, f) in variants {
            let mut t = s.clone();
            f(&mut t);
            for g in backends {
                drop(develop(&preview, &info, &t, &view, Some(&caches[*g as usize]), *g));
            }
            row(&format!("finish 1920×1280 {name}"), &mut |g| {
                k += 1.0;
                t.light.exposure = 0.3 + k * 0.001;
                drop(develop(&preview, &info, &t, &view, Some(&caches[g as usize]), g));
            });
        }
    }
    if run("batch") && args.len() > 1 {
        // full-size JPEG export of all the files given (decode + render + encode)
        use lightcraft_engine::export::export_photo;
        let mut session = lightcraft_engine::Session::new().with_fs();
        let r = session.execute("library.import", &serde_json::json!({"paths": args})).expect("import");
        let ids: Vec<_> = r["imported"].as_array().expect("ids").iter().filter_map(|v| v.as_u64()).map(lightcraft_engine::catalog::PhotoId).collect();
        let items: Vec<_> = ids.iter().enumerate().map(|(i, id)| (*id, i + 1)).collect();
        let o = ExportOptions::default();
        row(&format!("batch export of {} files (decode+render+JPEG):", items.len()), &mut |g| {
            lightcraft_engine::gpu::set_enabled(g);
            for &(id, seq) in &items {
                drop(export_photo(&mut session, id, &o, seq).expect("export"));
                session.media.forget(id); // as in a batch: every original is decoded once
            }
        });
        lightcraft_engine::gpu::set_enabled(true);
        return;
    }
    if run("export") {
        let big = RenderRequest::fit(full.width, full.height);
        let ne = n.min(3);
        let mut img = None;
        let cols: Vec<String> = backends
            .iter()
            .map(|&g| {
                let ms = best(ne, || img = Some(develop(&full, &info, &s, &big, None, g).image));
                format!("{ms}")
            })
            .collect();
        println!(
            "{:<38} cpu: {}{}",
            format!("export render {}×{}:", full.width, full.height),
            cols[0],
            cols.get(1).map(|g| format!("  |  gpu: {g}")).unwrap_or_default()
        );
        if gpu {
            let c = develop(&full, &info, &s, &big, None, false).image;
            let (max, mean) = diff(&c, img.as_ref().expect("rendered"));
            println!("gpu vs cpu, export {}×{} typical: max {max} LSB, mean {mean:.4} LSB", full.width, full.height);
        }
        let img = img.expect("rendered");
        println!("export JPEG encode:                    {}", best(ne, || drop(encode_image(&img, &ExportOptions::default()))));
    }
}
