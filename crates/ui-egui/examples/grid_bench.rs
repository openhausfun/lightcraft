//! Unchanged-frame cost of the photo grid (issue #35), on synthetic libraries:
//! `cargo run --release -p lightcraft-ui-egui --example grid_bench`
//! (`SIZES=200,10000,85000` photos, `FRAMES=60` measured frames per scenario).
//!
//! Each scenario builds a library of N photos (mixed aspect ratios, ~40 photos per capture day,
//! a three-photo stack every 100 photos), shows it in the Photo Grid (justified rows) or the
//! Square Grid, with All Photos or a recursive Local folder view as the source, settles (missing
//! files: thumbnails fail fast, so no decoding is timed), scrolls to the middle and then times
//! frames in which nothing changes. Prints per-frame wall-clock (min / median / p95 of the whole
//! app frame and of the grid panel alone), heap allocations per frame on the UI thread, and the
//! grid's own counters (cells visited, layout / view / index rebuilds).
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::time::{Duration, Instant};

use lightcraft_catalog::{Op, Photo, PhotoId, Source, Stack, StackId};
use lightcraft_engine::{Browse, LibrarySource, Session};
use lightcraft_ui_egui::headless::Headless;
use lightcraft_ui_egui::state::ViewMode;
use lightcraft_ui_egui::{LightcraftApp, Services};

/// Counts allocations made by the current thread (the UI thread is the one measured).
struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

fn count(size: usize) {
    let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
    let _ = BYTES.try_with(|n| n.set(n.get() + size as u64));
}

// SAFETY: every call is forwarded unchanged to the system allocator; the counters are plain
// thread-locals that never allocate. Dev-only example.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        count(l.size());
        // SAFETY: same contract as ours.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        count(l.size());
        // SAFETY: same contract as ours.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        count(new);
        // SAFETY: same contract as ours.
        unsafe { System.realloc(p, l, new) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same contract as ours.
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn allocs() -> (u64, u64) {
    (ALLOCS.with(Cell::get), BYTES.with(Cell::get))
}

const ROOT: &str = "/lightcraft-grid-bench/root";

fn library(n: usize, local: bool) -> Session {
    let mut s = Session::new();
    let shapes = [(6000, 4000), (4000, 6000), (4000, 4000), (6000, 3375), (3000, 4000)];
    for i in 0..n {
        let id = PhotoId(i as u64 + 1);
        let (w, h) = shapes[i % shapes.len()];
        let path = format!("{ROOT}/sub{}/IMG_{i:06}.jpg", i / 1000);
        let mut p = Photo::new(id, Source::File { path }, &format!("IMG_{i:06}.jpg"), "JPEG", w, h, "2026-01-01T00:00:00");
        // ~40 photos a day, going back in time
        let day = i / 40;
        let (y, m, d) = (2026 - (day / 336) as i32, 12 - (day / 28) % 12, 28 - day % 28);
        p.captured = Some(format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:00", 8 + (i % 40) / 4, i % 60));
        p.local = local;
        let _ = s.catalog.apply(Op::AddPhoto { photo: Box::new(p) });
    }
    for k in 0..n / 100 {
        let base = k as u64 * 100 + 1;
        let stack = Stack { id: StackId(k as u64 + 1), photos: vec![PhotoId(base), PhotoId(base + 1), PhotoId(base + 2)], collapsed: false };
        let _ = s.catalog.apply(Op::AddStack { stack });
    }
    if local {
        s.source = LibrarySource::Folder;
        s.browse = Some(Browse { path: ROOT.into(), subfolders: true });
    }
    s
}

struct Dist {
    min: f64,
    med: f64,
    p95: f64,
}

fn dist(mut v: Vec<f64>) -> Dist {
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v.get(((v.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    Dist { min: at(0.0), med: at(0.5), p95: at(0.95) }
}

fn scenario(n: usize, view: ViewMode, local: bool, frames: usize) {
    let t0 = Instant::now();
    let session = library(n, local);
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(session, services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    h.app.ui.view = view;
    h.app.ui.thumb_size = 160.0;
    h.settle(Duration::from_secs(120));
    // to the middle of the grid (and let the newly visible thumbnails fail / settle)
    if let Some(mid) = h.app.session.visible().get(n / 2).copied() {
        h.app.session.selection.active = Some(mid);
        h.app.session.selection.ids = vec![mid];
    }
    h.settle(Duration::from_secs(120));
    let setup = t0.elapsed();
    let stats0 = h.app.caches.grid_stats.clone();
    let mut frame_ms = Vec::with_capacity(frames);
    let mut grid_ms = Vec::with_capacity(frames);
    let (a0, b0) = allocs();
    for _ in 0..frames {
        let t = Instant::now();
        h.step();
        frame_ms.push(t.elapsed().as_secs_f64() * 1e3);
        grid_ms.push(h.app.caches.grid_stats.last_show.as_secs_f64() * 1e3);
    }
    let (a1, b1) = allocs();
    let st = &h.app.caches.grid_stats;
    let f = dist(frame_ms);
    let g = dist(grid_ms);
    let name = format!("{n:>6} {:<7} {:<5}", if view == ViewMode::SquareGrid { "square" } else { "photo" }, if local { "local" } else { "all" });
    println!(
        "{name} frame {:>7.2} / {:>7.2} / {:>7.2} ms  grid {:>7.2} / {:>7.2} / {:>7.2} ms  allocs/frame {:>7} ({:>9} B)  cells/frame {:>6}  rebuilds view {} layout {} stacks {}  (setup {:.1} s)",
        f.min,
        f.med,
        f.p95,
        g.min,
        g.med,
        g.p95,
        (a1 - a0) / frames as u64,
        (b1 - b0) / frames as u64,
        (st.cells_visited - stats0.cells_visited) / frames as u64,
        st.view_builds - stats0.view_builds,
        st.layout_builds - stats0.layout_builds,
        st.stack_index_builds - stats0.stack_index_builds,
        setup.as_secs_f64(),
    );
}

fn main() {
    let sizes: Vec<usize> = std::env::var("SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![200, 10_000, 85_000]);
    let frames = std::env::var("FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    println!("photos layout  source   frame min / median / p95        grid min / median / p95");
    for &n in &sizes {
        for view in [ViewMode::PhotoGrid, ViewMode::SquareGrid] {
            for local in [false, true] {
                scenario(n, view, local, frames);
            }
        }
    }
}
