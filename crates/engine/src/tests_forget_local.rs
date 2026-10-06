//! Forgetting untouched Local browse records: by command and when the library opens; changed,
//! recent and visible records stay; a crash replays it; browsing again brings records back.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::{LibrarySource, Selection, Session};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-forget-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn png(p: &Path, seed: u8) {
    let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
    let b =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() }).unwrap();
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b).unwrap();
}

fn open(lib: &Path, clock: &Arc<Mutex<String>>) -> Session {
    let mut s = Session::new().with_fs();
    let c = clock.clone();
    s.clock = Box::new(move || c.lock().unwrap().clone());
    s.open_library(lib, false).unwrap();
    s
}

fn set(clock: &Arc<Mutex<String>>, t: &str) {
    *clock.lock().unwrap() = t.to_string();
}

fn local_paths(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s
        .catalog
        .photos()
        .filter(|p| p.local)
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

/// Leave the folder view (nothing browsed is visible or selected any more).
fn look_elsewhere(s: &mut Session) {
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    s.selection = Selection::default();
    s.previous_active = None;
    s.undo.clear();
}

#[test]
fn untouched_records_of_unbrowsed_folders_are_forgotten_and_come_back() {
    let root = temp_dir("cmd");
    let (a, b, lib) = (root.join("a"), root.join("b"), root.join("lib"));
    for (i, n) in ["a1.png", "a2.png", "a3.png"].iter().enumerate() {
        png(&a.join(n), i as u8);
    }
    png(&b.join("b1.png"), 7);
    let clock = Arc::new(Mutex::new("2026-08-01T10:00:00".to_string()));
    let mut s = open(&lib, &clock);
    s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    // the user rates a2 (a change: kept)
    let a2 = s.catalog.photos().find(|p| p.file_name == "a2.png").unwrap().id;
    s.selection = Selection::single(a2);
    s.execute("photo.rate", &json!({"rating": 3})).unwrap();
    set(&clock, "2026-08-25T10:00:00");
    s.execute("library.browse", &json!({"path": b.to_string_lossy()})).unwrap();
    look_elsewhere(&mut s);

    // 40 days after a was browsed, 16 after b
    set(&clock, "2026-09-10T10:00:00");
    let dry = s.execute("library.forgetLocal", &json!({"dryRun": true})).unwrap();
    assert_eq!(
        (dry["local"].as_u64(), dry["forgotten"].as_u64(), dry["keptTouched"].as_u64(), dry["keptRecent"].as_u64()),
        (Some(4), Some(2), Some(1), Some(1)),
        "{dry}"
    );
    assert_eq!(local_paths(&s).len(), 4, "a dry run changes nothing");
    let r = s.execute("library.forgetLocal", &json!({})).unwrap();
    assert_eq!(r["forgotten"], 2, "{r}");
    assert_eq!(local_paths(&s), vec!["a2.png", "b1.png"]);
    // files on disk are untouched
    assert!(["a1.png", "a2.png", "a3.png"].iter().all(|n| a.join(n).exists()));
    // not an undo step
    assert!(s.undo.is_empty());

    // a crash now: the log replays the same catalog
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s = open(&lib, &clock);
    assert_eq!(s.library.as_ref().unwrap().report.failed, 0);
    assert_eq!(s.catalog.to_snapshot(), expect, "nothing more to forget on open, same state after replay");

    // browsing a again brings the forgotten files back, fresh; the rated one is the same record
    let r = s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(3), Some(2)), "{r}");
    assert_eq!(s.catalog.photo(a2).unwrap().rating, 3);
    // while a is shown, nothing in it goes, whatever the time
    set(&clock, "2027-09-10T10:00:00");
    assert_eq!(s.execute("library.forgetLocal", &json!({"dryRun": true})).unwrap()["forgotten"], 1, "only b1 (not visible)");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn opening_the_library_forgets_per_the_preference() {
    let root = temp_dir("open");
    let (a, lib) = (root.join("a"), root.join("lib"));
    png(&a.join("x.png"), 1);
    png(&a.join("y.png"), 2);
    let clock = Arc::new(Mutex::new("2026-08-01T10:00:00".to_string()));
    let mut s = open(&lib, &clock);
    s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    look_elsewhere(&mut s);
    // 0 = never
    let r = s.execute("library.preferences", &json!({"forgetLocalDays": 0})).unwrap();
    assert_eq!(r["forgetLocalDays"], 0);
    s.close_library().unwrap();
    drop(s);
    set(&clock, "2027-01-01T10:00:00");
    let mut s = open(&lib, &clock);
    assert_eq!(local_paths(&s).len(), 2, "never");
    assert!(s.library.as_ref().unwrap().forgot_local.is_none());
    // back to 30 days: the next open forgets them
    s.execute("library.preferences", &json!({"forgetLocalDays": 30})).unwrap();
    s.source = LibrarySource::All;
    s.close_library().unwrap();
    drop(s);
    let s = open(&lib, &clock);
    assert!(local_paths(&s).is_empty());
    let info = {
        let mut s = s;
        s.execute("library.info", &json!({})).unwrap()
    };
    assert_eq!(info["forgotLocal"]["forgotten"], 2, "{info}");
    assert!(a.join("x.png").exists() && a.join("y.png").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// A file with an XMP sidecar comes in with the sidecar's rating and keywords: that is its
/// browse state, not a user change, so the record is forgotten like any other — and the sidecar
/// stays byte for byte, so browsing again reads the same values back.
#[test]
fn sidecar_values_are_not_user_changes_and_sidecars_stay() {
    let root = temp_dir("xmp");
    let (a, lib) = (root.join("a"), root.join("lib"));
    png(&a.join("x.png"), 1);
    let x = a.join("x.png").to_string_lossy().to_string();
    let mut p = lightcraft_catalog::Photo::new(
        lightcraft_catalog::PhotoId(1),
        lightcraft_catalog::Source::File { path: x.clone() },
        "x.png",
        "PNG",
        16,
        12,
        "2026-01-01T00:00:00",
    );
    p.rating = 4;
    p.meta.keywords = vec!["dunes".into()];
    let side = crate::sidecar::sidecar_path(&x, Default::default());
    std::fs::write(&side, crate::sidecar::sidecar_packet(&p, &lightcraft_catalog::Catalog::new())).unwrap();
    let packet = std::fs::read(&side).unwrap();

    let clock = Arc::new(Mutex::new("2026-08-01T10:00:00".to_string()));
    let mut s = open(&lib, &clock);
    s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    let rec = s.catalog.photos().find(|p| p.local).unwrap().clone();
    assert_eq!((rec.rating, rec.meta.keywords.clone()), (4, vec!["dunes".to_string()]), "read from the sidecar");
    look_elsewhere(&mut s);
    set(&clock, "2026-10-01T10:00:00");
    assert_eq!(s.execute("library.forgetLocal", &json!({})).unwrap()["forgotten"], 1);
    assert_eq!(std::fs::read(&side).unwrap(), packet, "the sidecar is untouched");
    s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    let back = s.catalog.photos().find(|p| p.local).unwrap();
    assert_ne!(back.id, rec.id, "a fresh record");
    assert_eq!((back.rating, back.meta.keywords.clone()), (4, vec!["dunes".to_string()]));
    let _ = std::fs::remove_dir_all(&root);
}

/// A catalog written before browse times existed: its Local records are kept on the first open
/// (their folders are stamped then), and forgotten only once the period has passed since.
#[test]
fn upgraded_catalogs_forget_nothing_at_first() {
    let root = temp_dir("upgrade");
    let (a, lib) = (root.join("a"), root.join("lib"));
    png(&a.join("x.png"), 1);
    let clock = Arc::new(Mutex::new("2026-08-01T10:00:00".to_string()));
    let mut s = open(&lib, &clock);
    s.execute("library.browse", &json!({"path": a.to_string_lossy()})).unwrap();
    look_elsewhere(&mut s);
    s.close_library().unwrap();
    drop(s);
    // strip the times and baselines, as an older version would have written the snapshot
    let snap = std::fs::read_to_string(lib.join("catalog.snap")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&snap).unwrap();
    v["catalog"].as_object_mut().unwrap().remove("browsed");
    for p in v["catalog"]["photos"].as_object_mut().unwrap().values_mut() {
        p.as_object_mut().unwrap().remove("local_baseline");
    }
    std::fs::write(lib.join("catalog.snap"), serde_json::to_vec(&v).unwrap()).unwrap();
    set(&clock, "2027-01-01T10:00:00");
    let s = open(&lib, &clock);
    assert_eq!(local_paths(&s).len(), 1, "kept on the first open after the upgrade");
    assert_eq!(s.catalog.last_browsed(&a.to_string_lossy()), Some("2027-01-01T10:00:00"));
    drop(s);
    set(&clock, "2027-02-15T10:00:00");
    let s = open(&lib, &clock);
    assert!(local_paths(&s).is_empty(), "the untouched legacy record goes once 30 days passed");
    let _ = std::fs::remove_dir_all(&root);
}
