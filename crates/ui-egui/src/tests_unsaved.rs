//! A failed save is visible: the command reports the error, a toast says the change is only in
//! memory, and the top bar shows a warning until a later save succeeds.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lightcraft_catalog::{MemStore, Store};
use lightcraft_engine::library::LibraryStores;
use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// Appends fail while `fail` is set.
#[derive(Clone, Default)]
struct Flaky {
    files: MemStore,
    fail: Arc<AtomicBool>,
}

impl Store for Flaky {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "flaky".into()
    }
}

#[test]
fn failed_save_is_reported_and_shown_until_a_save_succeeds() {
    let store = Flaky::default();
    let mut s = lightcraft_engine::Session::new();
    let stores = LibraryStores { dir: "flaky".into(), catalog: Box::new(store.clone()), files: Box::new(MemStore::new()), on_disk: false };
    s.open_library_in(stores, false).unwrap();
    let app = LightcraftApp::new(s, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.step();
    let has_indicator = |h: &Headless| h.app.widgets.iter().any(|(w, _)| w == "indicator:unsaved");
    assert!(!has_indicator(&h));

    store.fail.store(true, Ordering::SeqCst);
    let r = h.request("engine.execute", json!({"command": "album.create", "params": {"name": "Kept"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    assert!(
        r["error"].as_str().unwrap().contains("saved in memory but not written to disk: ") && r["error"].as_str().unwrap().contains("disk full"),
        "{r}"
    );
    assert_eq!(h.app.session.catalog.albums().count(), 1, "the change is applied");
    h.step();
    h.step();
    let toast = h.app.ui.toast.clone().map(|t| t.0).unwrap_or_default();
    assert!(toast.contains("not written to disk"), "{toast}");
    assert!(has_indicator(&h));
    let inspect = h.request("ui.inspect", json!({}), T);
    assert_eq!(inspect["result"]["unsaved"]["ops"], 1, "{inspect}");

    // the disk comes back: the next command writes the queue, the warning goes away
    store.fail.store(false, Ordering::SeqCst);
    let r = h.request("engine.execute", json!({"command": "library.info"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert!(!has_indicator(&h));
    assert_eq!(h.app.ui.toast.clone().map(|t| t.0).as_deref(), Some("Library saved"));
}
