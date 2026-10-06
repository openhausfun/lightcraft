//! Issue #103: a damaged or unreadable settings file (prefs.json, presets.json, view.json) is
//! never silently replaced by the defaults: a damaged one is kept aside, an unreadable one is not
//! written this session, and the user is told.

use std::sync::{Arc, Mutex};

use lightcraft_catalog::{MemStore, Store};
use serde_json::json;

use crate::Session;
use crate::library::LibraryStores;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-settings-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn damaged_prefs_are_kept_aside_and_reported() {
    let lib = temp_dir("damaged");
    let bad = br#"{"exportPresets": [{"name": "Proof", "par"#;
    std::fs::write(lib.join("prefs.json"), bad).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let kept: Vec<_> = std::fs::read_dir(&lib)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("prefs.json.corrupt-"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(lib.join(&kept[0])).unwrap(), bad);
    let warnings = s.take_library_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("prefs.json is damaged") && warnings[0].contains(&kept[0]), "{warnings:?}");
    assert!(s.take_library_warnings().is_empty(), "each warning is handed out once");
    let info = s.execute("library.info", &json!({})).unwrap();
    assert_eq!(info["settingsWarnings"].as_array().map(Vec::len), Some(1), "{info}");
    // saving goes on (the damaged original is safe in its copy)
    s.execute("library.preferences", &json!({"cacheMb": 300})).unwrap();
    assert!(String::from_utf8(std::fs::read(lib.join("prefs.json")).unwrap()).unwrap().contains("300"));
    assert_eq!(std::fs::read(lib.join(&kept[0])).unwrap(), bad);
    drop(s);
    let _ = std::fs::remove_dir_all(&lib);
}

/// `presets.json` and `prefs.json` can't be read (locked by another program, an I/O error).
#[derive(Clone, Default)]
struct Unreadable {
    files: MemStore,
    unreadable: Arc<Mutex<Vec<String>>>,
}

impl Store for Unreadable {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        if self.unreadable.lock().unwrap().iter().any(|n| n == name) {
            return Err(std::io::Error::other("locked by another process"));
        }
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "unreadable".into()
    }
}

#[test]
fn unreadable_settings_are_not_overwritten() {
    let files = Unreadable::default();
    let presets = br#"{"user": [{"id": "user.mine", "name": "Mine", "group": "User Presets", "settings": {}}]}"#.to_vec();
    let prefs = br#"{"cacheMb": 777}"#.to_vec();
    files.files.set("presets.json", presets.clone());
    files.files.set("prefs.json", prefs.clone());
    *files.unreadable.lock().unwrap() = vec!["presets.json".into(), "prefs.json".into()];
    let mut s = Session::new();
    let stores = LibraryStores { dir: "unreadable".into(), catalog: Box::new(MemStore::new()), files: Box::new(files.clone()), on_disk: false };
    s.open_library_in(stores, true).unwrap();
    let warnings = s.take_library_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().all(|w| w.contains("couldn't be read") && w.contains("won't overwrite")), "{warnings:?}");

    // a preset change and a preference change: neither file is replaced by defaults
    s.execute("preset.create", &json!({"name": "New"})).unwrap();
    let e = s.execute("library.preferences", &json!({"cacheMb": 300}));
    assert!(e.is_err(), "saving preferences is refused, not done over the unread file");
    assert_eq!(files.files.get("presets.json").unwrap(), presets);
    assert_eq!(files.files.get("prefs.json").unwrap(), prefs);
}
