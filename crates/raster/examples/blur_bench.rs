//! Micro-benchmark for the blur/map primitives (min of N runs, wall-clock and process CPU time, so
//! it stays meaningful on a loaded machine):
//! `RAYON_NUM_THREADS=1 cargo run --release -p lightcraft-raster --example blur_bench`.
use lightcraft_raster::{Plane, Rgb32f, blur::gaussian};
use std::time::Instant;

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

fn best(n: usize, mut f: impl FnMut()) -> String {
    let (mut wall, mut cpu) = (f64::MAX, f64::MAX);
    for _ in 0..n {
        let (t, c) = (Instant::now(), cpu_ms());
        f();
        cpu = cpu.min(cpu_ms() - c);
        wall = wall.min(t.elapsed().as_secs_f64() * 1e3);
    }
    format!("{wall:.1} ms wall, {cpu:.1} ms cpu")
}

fn main() {
    let (w, h) = (3000, 2400);
    let p = Plane::from_fn(w, h, |x, y| ((x * 7 + y * 13) % 255) as f32 / 255.0);
    let c = Rgb32f::from_fn(w, h, |x, y| [x as f32 / w as f32, y as f32 / h as f32, 0.5]);
    for sigma in [1.0f32, 4.0, 24.0] {
        println!("plane gaussian σ={sigma}: {}", best(9, || drop(gaussian(&p, sigma))));
    }
    println!("rgb gaussian σ=4: {}", best(9, || drop(gaussian(&c, 4.0))));
}
