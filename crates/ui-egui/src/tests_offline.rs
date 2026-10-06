//! Slow or offline drives never stall a frame (issue #104): with an injected file-system probe
//! that takes far longer than a frame (a sleeping NAS), the grid, the Info panel, the Missing
//! Photos view, the Rename dialog and an import keep drawing frames quickly, the probe never runs
//! on the UI thread, and the background answers still arrive.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_engine::{LibrarySource, Session};
use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};

/// What one file-system call takes on the "NAS".
const SLOW: Duration = Duration::from_millis(400);
/// A frame that waited for one such call would take at least this long.
const FRAME_LIMIT: Duration = Duration::from_millis(300);
const N: u64 = 12;

/// Threads the probe ran on, and how often.
#[derive(Clone, Default)]
struct Calls {
    threads: Arc<Mutex<Vec<ThreadId>>>,
    n: Arc<AtomicUsize>,
}

impl Calls {
    fn record(&self) {
        self.threads.lock().unwrap().push(std::thread::current().id());
        self.n.fetch_add(1, Ordering::SeqCst);
    }
    fn on(&self, t: ThreadId) -> usize {
        self.threads.lock().unwrap().iter().filter(|x| **x == t).count()
    }
}

/// `N` photos whose originals are on an unreachable share, a smart previews folder, and a probe
/// that sleeps like a NAS waking up before saying "not there".
fn offline_library(calls: &Calls) -> Session {
    let mut s = Session::new();
    for i in 0..N {
        let path = format!("/lightcraft-offline-nas/IMG_{i:03}.jpg");
        let p = Photo::new(PhotoId(i + 1), Source::File { path }, &format!("IMG_{i:03}.jpg"), "JPEG", 600, 400, "2026-01-01T00:00:00");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.media.smart_dir = Some(std::env::temp_dir().join("lightcraft-offline-smart-previews-none"));
    let c = calls.clone();
    s.media.availability.set_probe(Arc::new(move |_| {
        c.record();
        std::thread::sleep(SLOW);
        false
    }));
    s
}

fn max_frame(h: &mut Headless, frames: usize) -> Duration {
    let mut worst = Duration::ZERO;
    for _ in 0..frames {
        let t0 = Instant::now();
        h.step();
        worst = worst.max(t0.elapsed());
    }
    worst
}

#[test]
fn offline_originals_never_block_a_frame() {
    let calls = Calls::default();
    let app = LightcraftApp::new(offline_library(&calls), Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let ui_thread = std::thread::current().id();
    h.app.ui.view = ViewMode::PhotoGrid;
    h.step();
    assert!(h.app.session.media.availability.is_background(), "the app checks files off the UI thread");

    // the grid: every cell asks for its thumbnail (which asks whether the original is there)
    h.app.session.execute("library.select", &json!({"ids": [1]})).unwrap();
    let worst = max_frame(&mut h, 20);
    assert!(worst < FRAME_LIMIT, "grid frame took {worst:?}");

    // the Info panel of the active photo, in the loupe
    h.app.ui.view = ViewMode::Detail;
    h.app.ui.right = RightPanel::Info;
    let worst = max_frame(&mut h, 20);
    assert!(worst < FRAME_LIMIT, "Info panel frame took {worst:?}");

    // the Missing Photos view fills in as the background checks find the files gone
    h.app.ui.view = ViewMode::PhotoGrid;
    h.app.ui.right = RightPanel::None;
    h.app.session.execute("library.source", &json!({"kind": "missing"})).unwrap();
    assert_eq!(h.app.session.source, LibrarySource::Missing);
    let t0 = Instant::now();
    let mut worst = Duration::ZERO;
    while h.app.session.visible().len() < N as usize && t0.elapsed() < Duration::from_secs(60) {
        let f = Instant::now();
        h.step();
        worst = worst.max(f.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.app.session.visible().len(), N as usize, "every photo found missing in the end");
    assert!(worst < FRAME_LIMIT, "Missing Photos frame took {worst:?}");

    // the Rename dialog's preview (it checks which names are taken on disk)
    h.app.session.execute("library.select", &json!({"ids": [1, 2, 3]})).unwrap();
    h.app.ui.dialog = Some(crate::state::Dialog::Rename { template: "Trip-{seq:3}".into(), start: 1 });
    let worst = max_frame(&mut h, 20);
    assert!(worst < FRAME_LIMIT, "Rename dialog frame took {worst:?}");

    assert!(calls.n.load(Ordering::SeqCst) > 0, "the probe was asked");
    assert_eq!(calls.on(ui_thread), 0, "the slow probe never ran on the UI thread");
}

/// An import whose files take long to read (probe: decoding headers and hashing on a slow drive)
/// runs on a worker thread: frames stay quick, the probe never runs on the UI thread, and Cancel
/// stops it with what was read so far added.
#[test]
fn import_reads_files_off_the_ui_thread_and_can_be_cancelled() {
    let dir = std::env::temp_dir().join(format!("lc-ui-slow-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let n = 40;
    for i in 0..n {
        std::fs::write(dir.join(format!("IMG_{i:03}.jpg")), format!("not really a jpeg {i}")).unwrap();
    }
    let calls = Calls::default();
    let mut s = Session::new();
    let c = calls.clone();
    s.media.file_probe = Some(Arc::new(move |p: &str| {
        c.record();
        std::thread::sleep(Duration::from_millis(150));
        Ok(lightcraft_engine::media::ProbeInfo {
            width: 60,
            height: 40,
            format: "JPEG".into(),
            content_hash: Some(p.to_string()),
            ..Default::default()
        })
    }));
    let app = LightcraftApp::new(s, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let ui_thread = std::thread::current().id();
    h.step();
    let undo0 = h.app.session.undo.len();
    crate::import::start_paths(&mut h.app, vec![dir.to_string_lossy().to_string()]).unwrap();
    // some batches arrive; meanwhile every frame is quick
    let t0 = Instant::now();
    let mut worst = Duration::ZERO;
    while h.app.import.as_ref().is_some_and(|t| t.imported < 8) && t0.elapsed() < Duration::from_secs(60) {
        let f = Instant::now();
        h.step();
        worst = worst.max(f.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(worst < FRAME_LIMIT, "import frame took {worst:?}");
    assert!(h.app.import.is_some(), "still importing");
    let status = h.app.import.as_ref().map(crate::import::ImportTask::status).unwrap_or_default();
    assert!(status["total"].as_u64().unwrap_or(0) >= 8, "{status}");
    // Cancel: the files being read finish, nothing new starts
    let r = h.request("ui.clickWidget", json!({"id": "button:importCancel"}), Duration::from_secs(10));
    assert_eq!(r["ok"], true, "{r}");
    let t0 = Instant::now();
    while h.app.import.is_some() && t0.elapsed() < Duration::from_secs(60) {
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.app.import.is_none(), "stopped");
    let added = h.app.session.catalog.len();
    assert!((8..n).contains(&added), "cancelled part-way: {added} of {n} added");
    assert_eq!(h.app.session.undo.len(), undo0 + 1, "one undo step");
    assert_eq!(calls.on(ui_thread), 0, "files were never read on the UI thread");
    let _ = std::fs::remove_dir_all(&dir);
}
