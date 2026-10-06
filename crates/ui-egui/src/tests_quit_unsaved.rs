//! Issue #103: quitting with changes that couldn't be saved asks first (after one more try), and
//! settings-file problems found when the library opened are shown, not swallowed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lightcraft_catalog::{MemStore, Store};
use lightcraft_engine::library::LibraryStores;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// Every write fails while `fail` is set (an unplugged drive).
#[derive(Clone, Default)]
struct Unplugged {
    files: MemStore,
    fail: Arc<AtomicBool>,
}

impl Unplugged {
    fn check(&self) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) { Err(std::io::Error::other("drive unplugged")) } else { Ok(()) }
    }
}

impl Store for Unplugged {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.check()?;
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.check()?;
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "unplugged".into()
    }
}

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

/// Click a widget and let the synthetic events (move, press, release) play out.
fn click(h: &mut Headless, id: &str) {
    // a window that just appeared is placed on its second frame
    h.step();
    h.step();
    assert_eq!(h.request("ui.clickWidget", json!({"id": id}), T)["ok"], true, "{id}");
    for _ in 0..4 {
        h.step();
    }
}

#[test]
fn quitting_with_unsaved_changes_asks_first() {
    let store = Unplugged::default();
    let mut s = lightcraft_engine::Session::new();
    let stores = LibraryStores { dir: "unplugged".into(), catalog: Box::new(store.clone()), files: Box::new(MemStore::new()), on_disk: false };
    s.open_library_in(stores, false).unwrap();
    let app = LightcraftApp::new(s, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    // nothing unsaved: closing is fine
    assert!(crate::panels::notices::may_close(&mut h.app));

    store.fail.store(true, Ordering::SeqCst);
    let r = h.request("engine.execute", json!({"command": "album.create", "params": {"name": "Kept"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    assert!(!crate::panels::notices::may_close(&mut h.app), "retried, still failing: ask");
    h.step();
    let prompt = h.app.quit_prompt.clone().unwrap();
    assert!(prompt.contains("1 change couldn't be written to disk") && prompt.contains("drive unplugged"), "{prompt}");
    for b in ["button:quitRetry", "button:quitAnyway", "button:quitCancel"] {
        assert!(has(&h, b), "{b}");
    }
    assert_eq!(h.request("ui.inspect", json!({}), T)["result"]["quitPrompt"], json!(prompt));
    // Cancel: back to work, nothing quit
    click(&mut h, "button:quitCancel");
    assert!(h.app.quit_prompt.is_none() && !h.app.quit_confirmed);

    // the drive is back: Try Saving Again saves and quits
    assert!(!crate::panels::notices::may_close(&mut h.app));
    h.step();
    store.fail.store(false, Ordering::SeqCst);
    click(&mut h, "button:quitRetry");
    assert!(h.app.session.unsaved().is_none(), "saved");
    assert!(h.app.quit_prompt.is_none() && !h.app.quit_confirmed);

    // Quit Anyway is a deliberate choice
    store.fail.store(true, Ordering::SeqCst);
    let _ = h.request("engine.execute", json!({"command": "album.create", "params": {"name": "Lost"}}), T);
    assert!(!crate::panels::notices::may_close(&mut h.app));
    h.step();
    click(&mut h, "button:quitAnyway");
    assert!(h.app.quit_confirmed && crate::panels::notices::may_close(&mut h.app));
}

#[test]
fn damaged_settings_file_is_shown() {
    let files = MemStore::new();
    files.set("presets.json", b"{\"user\": [ {\"id\": ".to_vec());
    let mut s = lightcraft_engine::Session::new();
    let stores = LibraryStores { dir: "lib".into(), catalog: Box::new(MemStore::new()), files: Box::new(files.clone()), on_disk: false };
    s.open_library_in(stores, false).unwrap();
    let app = LightcraftApp::new(s, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    h.step();
    assert_eq!(h.app.notices.len(), 1);
    assert!(h.app.notices[0].contains("presets.json is damaged"), "{:?}", h.app.notices);
    assert!(has(&h, "button:noticeOk"));
    assert!(files.files.lock().unwrap().keys().any(|k| k.starts_with("presets.json.corrupt-")));
    click(&mut h, "button:noticeOk");
    assert!(h.app.notices.is_empty(), "{:?}", h.app.notices);
    h.step();
    assert!(!has(&h, "button:noticeOk"));
}
