//! Import: recursive folders, duplicate detection by content, add vs copy into the library.

use std::path::Path;

use serde_json::{Value, json};

use crate::Session;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-import-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A small procedural PNG (distinct per seed).
fn write_png(path: &Path, seed: u8) {
    let (w, h) = (48usize, 32usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn ids(v: &Value, key: &str) -> usize {
    v[key].as_array().map(Vec::len).unwrap_or(0)
}

#[test]
fn recursive_import_with_duplicates() {
    let src = temp_dir("src");
    write_png(&src.join("a.png"), 1);
    write_png(&src.join("trip/b.png"), 2);
    write_png(&src.join("trip/day2/c.PNG"), 3);
    std::fs::copy(src.join("a.png"), src.join("trip/a-copy.png")).unwrap(); // same bytes
    write_png(&src.join(".hidden/x.png"), 9);
    std::fs::write(src.join("notes.txt"), "not a photo").unwrap();
    std::fs::write(src.join("trip/broken.jpg"), "garbage").unwrap();

    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["scanned"], 5, "{r}"); // a, b, c, a-copy, broken (hidden + txt skipped)
    assert_eq!(ids(&r, "imported"), 3, "{r}");
    assert_eq!(ids(&r, "duplicates"), 1, "{r}");
    assert_eq!(r["duplicates"][0]["reason"], "content");
    assert_eq!(ids(&r, "failed"), 1, "{r}");
    assert_eq!(s.catalog.len(), 3);
    assert!(s.catalog.photos().all(|p| p.content_hash.as_ref().is_some_and(|h| h.len() == 32) && p.width == 48));

    // importing again: everything is a duplicate (by path), one undo step for the first import
    let r2 = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(ids(&r2, "imported"), 0);
    assert_eq!(r2["duplicates"].as_array().unwrap().iter().filter(|d| d["reason"] == "path").count(), 3);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 0);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn copy_into_library_and_persist() {
    let src = temp_dir("copysrc");
    let lib = temp_dir("copylib");
    write_png(&src.join("one.png"), 4);
    write_png(&src.join("sub/one.png"), 5); // same name, different content
    let mut s = Session::new().with_fs();
    assert!(s.execute("library.import", &json!({"paths": [src.to_string_lossy()], "mode": "copy"})).is_err(), "copy needs a library");
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()], "mode": "copy"})).unwrap();
    assert_eq!(ids(&r, "imported"), 2, "{r}");
    let paths: Vec<String> = s
        .catalog
        .photos()
        .map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => path.clone(),
            _ => panic!(),
        })
        .collect();
    for p in &paths {
        assert!(Path::new(p).starts_with(lib.join("Originals")), "{p}");
        assert!(Path::new(p).exists());
    }
    assert_ne!(paths[0], paths[1], "unique names");
    // originals deleted from the source: the library copies remain usable after a restart
    std::fs::remove_dir_all(&src).unwrap();
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert_eq!(s.catalog.len(), 2);
    let id = s.catalog.photos().next().unwrap().id;
    assert!(s.render_now(id, 32, 32).is_ok());
    // the library folder itself is never re-imported
    let r = s.execute("library.import", &json!({"paths": [lib.to_string_lossy()]})).unwrap();
    assert_eq!(r["scanned"], 0, "{r}");
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn civil_dates() {
    assert_eq!(crate::import::civil(0), "1970-01-01T00:00:00");
    assert_eq!(crate::import::civil(951_782_400), "2000-02-29T00:00:00");
    assert_eq!(crate::import::civil(1_790_000_000), "2026-09-21T14:13:20");
}

#[test]
fn browsing_a_folder_lists_its_photos_without_adding_them() {
    use crate::LibrarySource;
    let dir = std::env::temp_dir().join(format!("lc-browse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let png = |p: &std::path::Path, seed: u8| {
        let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
        let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() })
            .unwrap();
        std::fs::write(p, b).unwrap();
    };
    png(&dir.join("a.png"), 1);
    png(&dir.join("b.png"), 2);
    png(&dir.join("sub/c.png"), 3);
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.browse", &serde_json::json!({"path": dir.to_string_lossy()})).unwrap();
    assert_eq!(r["photos"], 2, "{r}");
    assert_eq!(s.source, LibrarySource::Folder);
    assert_eq!(s.visible_cloned().len(), 2);
    // not in the library
    s.execute("library.source", &serde_json::json!({"kind": "all"})).unwrap();
    assert!(s.visible_cloned().is_empty(), "browsed photos stay out of All Photos");
    assert_eq!(s.execute("catalog.stats", &serde_json::json!({})).unwrap()["photos"], 0, "nor in the counts");
    assert!(s.catalog.date_groups().is_empty());
    // subfolders; browsing again reuses the photos
    let r = s.execute("library.browse", &serde_json::json!({"path": dir.to_string_lossy(), "subfolders": true})).unwrap();
    assert_eq!((r["photos"].as_u64(), r["new"].as_u64()), (Some(3), Some(1)));
    // add one to the library
    let first = s.visible_cloned()[0];
    s.execute("photo.addToLibrary", &serde_json::json!({"ids": [first.0]})).unwrap();
    // importing the folder for real brings in the rest (no duplicates of the browsed ones)
    let r = s.execute("library.import", &serde_json::json!({"paths": [dir.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().map(Vec::len), Some(2), "{r}");
    s.execute("library.source", &serde_json::json!({"kind": "all"})).unwrap();
    assert_eq!(s.visible_cloned().len(), 3);
    assert_eq!(s.catalog.photos().count(), 3);
    assert!(s.execute("library.browse", &serde_json::json!({"path": dir.join("a.png").to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Photos a.png, b.png, c.png imported from `old`, then: a renamed into `moved`; b renamed, with
/// an impostor (same name and size, other bytes) under its old name; c gone, with two impostors.
fn renamed_and_ambiguous_scene(tag: &str) -> (std::path::PathBuf, Session) {
    let dir = temp_dir(tag);
    std::fs::create_dir_all(dir.join("old")).unwrap();
    std::fs::create_dir_all(dir.join("moved/real")).unwrap();
    for (n, seed) in [("a.png", 1), ("b.png", 2), ("c.png", 3)] {
        write_png(&dir.join("old").join(n), seed);
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.join("old").to_string_lossy()]})).unwrap();
    let impostor = |src: &Path, dst: &Path| {
        let mut b = std::fs::read(src).unwrap();
        let k = b.len() - 20;
        b[k] ^= 0xff;
        std::fs::write(dst, b).unwrap();
    };
    std::fs::rename(dir.join("old/a.png"), dir.join("moved/Trip-001.png")).unwrap();
    impostor(&dir.join("old/b.png"), &dir.join("moved/b.png"));
    std::fs::rename(dir.join("old/b.png"), dir.join("moved/real/Trip-002.png")).unwrap();
    impostor(&dir.join("old/c.png"), &dir.join("moved/c.png"));
    impostor(&dir.join("old/c.png"), &dir.join("moved/real/c.png"));
    std::fs::remove_file(dir.join("old/c.png")).unwrap();
    (dir, s)
}

/// Issue #105: Find Missing finds renamed files by content, prefers the content match over a
/// same-name same-size impostor, and skips (reports) photos it can't tell apart.
#[test]
fn find_missing_matches_renamed_files_by_content() {
    let (dir, mut s) = renamed_and_ambiguous_scene("missing-content");
    let r = s.execute("library.findMissing", &json!({"folder": dir.join("moved").to_string_lossy()})).unwrap();
    let found = r["found"].as_array().unwrap();
    assert_eq!(found.len(), 2, "{r}");
    let to = |name: &str| {
        found.iter().find(|f| f["from"].as_str().unwrap().ends_with(name)).map(|f| (f["to"].as_str().unwrap().to_string(), f["by"].clone()))
    };
    assert_eq!(to("a.png"), Some((dir.join("moved/Trip-001.png").to_string_lossy().to_string(), json!("content"))));
    assert_eq!(to("b.png"), Some((dir.join("moved/real/Trip-002.png").to_string_lossy().to_string(), json!("content"))), "not the impostor");
    assert_eq!(r["missing"], 1);
    assert_eq!(r["ambiguous"][0]["candidates"].as_array().map(Vec::len), Some(2), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #104: the app plans Find Missing on a worker thread (no session) and relinks the plan
/// afterwards; that gives exactly what the synchronous command gives — renamed files by content,
/// impostors and look-alikes skipped (`ambiguous`).
#[test]
fn find_missing_planned_on_a_worker_matches_the_command() {
    use crate::cmd::missing::{find_candidates, plan_find_missing};
    let (dir, mut s) = renamed_and_ambiguous_scene("missing-worker");
    let folder = dir.join("moved").to_string_lossy().to_string();
    let candidates = find_candidates(&s.catalog);
    let f = folder.clone();
    let plan = std::thread::spawn(move || plan_find_missing(&candidates, &f)).join().unwrap().unwrap();
    assert_eq!((plan.found.len(), plan.missing, plan.ambiguous.len()), (2, 1, 1), "{plan:?}");
    let applied = s.execute("library.findMissing", &plan.to_json()).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.execute("library.missing", &json!({})).unwrap().as_array().map(Vec::len), Some(3), "undo restores all three");
    let direct = s.execute("library.findMissing", &json!({"folder": folder})).unwrap();
    assert_eq!(applied, direct);
    assert_eq!(direct["found"].as_array().map(Vec::len), Some(2), "{direct}");
    assert_eq!(direct["ambiguous"].as_array().map(Vec::len), Some(1), "{direct}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #104: a plan made on a worker is re-checked when it is applied: a photo relinked
/// meanwhile is left alone, and a file that is now another photo's original isn't used.
#[test]
fn find_missing_plan_is_rechecked_when_applied() {
    use crate::cmd::missing::{find_candidates, plan_find_missing};
    let (dir, mut s) = renamed_and_ambiguous_scene("missing-recheck");
    let plan = plan_find_missing(&find_candidates(&s.catalog), &dir.join("moved").to_string_lossy()).unwrap();
    let to = |name: &str| plan.found.iter().find(|f| f.from.ends_with(name)).unwrap().clone();
    let (a, b) = (to("a.png"), to("b.png"));
    // meanwhile: a is relinked by hand (elsewhere), and the ambiguous c is pointed at b's file
    std::fs::copy(&a.to, dir.join("a-elsewhere.png")).unwrap();
    s.execute("photo.relink", &json!({"id": a.id.0, "path": dir.join("a-elsewhere.png").to_string_lossy()})).unwrap();
    let c = plan.ambiguous[0].id;
    s.execute("photo.relink", &json!({"id": c.0, "path": b.to})).unwrap();
    let r = s.execute("library.findMissing", &plan.to_json()).unwrap();
    assert_eq!(r["found"].as_array().map(Vec::len), Some(0), "{r}");
    assert_eq!(r["ambiguous"].as_array().map(Vec::len), Some(0), "c isn't missing any more: {r}");
    assert_eq!(r["missing"], 1, "b stays missing: its file is c's now: {r}");
    let path_of = |id: lightcraft_catalog::PhotoId| match &s.catalog.photo(id).unwrap().source {
        lightcraft_catalog::Source::File { path } => path.clone(),
        _ => String::new(),
    };
    assert_eq!(path_of(a.id), dir.join("a-elsewhere.png").to_string_lossy());
    assert_eq!(path_of(b.id), b.from);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_files_are_found_and_relinked() {
    let dir = std::env::temp_dir().join(format!("lc-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("old")).unwrap();
    std::fs::create_dir_all(dir.join("moved/deeper")).unwrap();
    let png = |p: &std::path::Path, seed: u8| {
        let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
        let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() })
            .unwrap();
        std::fs::write(p, b).unwrap();
    };
    png(&dir.join("old/a.png"), 1);
    png(&dir.join("old/b.png"), 2);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &serde_json::json!({"paths": [dir.join("old").to_string_lossy()]})).unwrap();
    assert_eq!(s.execute("library.missing", &serde_json::json!({})).unwrap().as_array().map(Vec::len), Some(0));
    // the files move away
    std::fs::rename(dir.join("old/a.png"), dir.join("moved/a.png")).unwrap();
    std::fs::rename(dir.join("old/b.png"), dir.join("moved/deeper/b.png")).unwrap();
    let m = s.execute("library.missing", &serde_json::json!({})).unwrap();
    assert_eq!(m.as_array().map(Vec::len), Some(2));
    s.execute("library.source", &serde_json::json!({"kind": "missing"})).unwrap();
    assert_eq!(s.visible_cloned().len(), 2, "the Missing Photos source lists them");
    s.execute("library.source", &serde_json::json!({"kind": "all"})).unwrap();
    // one by hand
    let id = m[0]["id"].as_u64().unwrap();
    let name = std::path::Path::new(m[0]["path"].as_str().unwrap()).file_name().unwrap().to_string_lossy().to_string();
    let new = if name == "a.png" { dir.join("moved/a.png") } else { dir.join("moved/deeper/b.png") };
    s.execute("photo.relink", &serde_json::json!({"id": id, "path": new.to_string_lossy()})).unwrap();
    assert_eq!(s.execute("library.missing", &serde_json::json!({})).unwrap().as_array().map(Vec::len), Some(1));
    assert!(s.execute("photo.relink", &serde_json::json!({"id": id, "path": dir.join("nope.png").to_string_lossy()})).is_err());
    // the rest by searching a folder
    let r = s.execute("library.findMissing", &serde_json::json!({"folder": dir.join("moved").to_string_lossy()})).unwrap();
    assert_eq!((r["found"].as_array().map(Vec::len), r["missing"].as_u64()), (Some(1), Some(0)), "{r}");
    // undo points the photo back at the old path but doesn't move any file
    s.execute("edit.undo", &serde_json::json!({})).unwrap();
    assert_eq!(s.execute("library.missing", &serde_json::json!({})).unwrap().as_array().map(Vec::len), Some(1));
    assert!(dir.join("moved/a.png").exists() && dir.join("moved/deeper/b.png").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recently_added_covers_recent_imports_newest_first() {
    let dir = std::env::temp_dir().join(format!("lc-recent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = |name: &str, seed: u8| {
        let img = lightcraft_raster::Rgba8::from_fn(16, 12, |x, y| [(x * 9) as u8, (y * 11) as u8, seed, 255]);
        let b = crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Png, ..Default::default() })
            .unwrap();
        std::fs::write(dir.join(name), b).unwrap();
        dir.join(name).to_string_lossy().to_string()
    };
    let mut s = Session::new().with_fs();
    // three imports: 60 days ago, 10 days ago, today
    for (when, name, seed) in [("2026-08-01T09:00:00", "old.png", 1), ("2026-09-20T09:00:00", "mid.png", 2), ("2026-09-30T09:00:00", "new.png", 3)] {
        let path = png(name, seed);
        let w = when.to_string();
        s.clock = Box::new(move || w.clone());
        s.execute("library.import", &serde_json::json!({"paths": [path]})).unwrap();
    }
    s.execute("library.source", &serde_json::json!({"kind": "recentlyAdded"})).unwrap();
    let names: Vec<String> = s.visible_cloned().iter().map(|id| s.catalog.photo(*id).unwrap().file_name.clone()).collect();
    assert_eq!(names, ["new.png", "mid.png"], "the last 30 days, newest import first");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Copy imports: a destination folder, flat / by-month folders, renamed copies (numbered in
/// import order), and a metadata preset on every photo.
#[test]
fn copy_with_destination_organize_rename_and_metadata_preset() {
    let src = temp_dir("orgsrc");
    let dest = temp_dir("orgdest");
    write_png(&src.join("a.png"), 1);
    write_png(&src.join("b.png"), 2);
    let mut s = Session::new().with_fs();
    s.execute("metadata.savePreset", &json!({"name": "Studio", "fields": {"copyright": "© Studio", "creator": "Sam"}})).unwrap();
    assert!(s.execute("library.import", &json!({"paths": [src.to_string_lossy()], "metadataPreset": "Nope"})).is_err());
    assert!(s.execute("library.import", &json!({"paths": [src.to_string_lossy()], "mode": "copy", "organize": "weekly"})).is_err());
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [src.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "flat",
                    "rename": "Shoot-{seq:3}", "renameStart": 7, "metadataPreset": "Studio"}),
        )
        .unwrap();
    assert_eq!(ids(&r, "imported"), 2, "{r}");
    let mut names: Vec<String> = s.catalog.photos().map(|p| p.file_name.clone()).collect();
    names.sort();
    assert_eq!(names, ["Shoot-007.png", "Shoot-008.png"], "catalogued under the new names");
    assert!(dest.join("Shoot-007.png").exists() && dest.join("Shoot-008.png").exists(), "flat: straight into the destination");
    assert!(s.catalog.photos().all(|p| p.meta.copyright == "© Studio" && p.meta.creator == "Sam"));
    // by month, no library needed when a destination is given
    let dest2 = temp_dir("orgdest2");
    write_png(&src.join("c.png"), 3);
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [src.join("c.png").to_string_lossy()], "mode": "copy", "destination": dest2.to_string_lossy(), "organize": "month"}),
        )
        .unwrap();
    assert_eq!(ids(&r, "imported"), 1, "{r}");
    let p = s.catalog.photos().find(|p| p.file_name == "c.png").unwrap();
    let lightcraft_catalog::Source::File { path } = &p.source else { panic!() };
    let rel = Path::new(path).strip_prefix(&dest2).unwrap();
    assert_eq!(rel.components().count(), 3, "YYYY/YYYY-MM/c.png: {rel:?}");
    assert_eq!(rel.parent().unwrap().file_name().unwrap().len(), 7);
    for d in [&src, &dest, &dest2] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Copy with a custom folder template: `{date:%Y}/{date:%Y%m%d}` files a photo under
/// `2026/20260114/` (an undated file by the import time); templates that would leave the
/// destination are refused.
#[test]
fn copy_with_a_folder_template() {
    let src = temp_dir("tplsrc");
    let dest = temp_dir("tpldest");
    write_png(&src.join("a.png"), 1);
    let mut s = Session::new().with_fs();
    s.clock = Box::new(|| "2026-01-14T05:58:48".to_string());
    for bad in ["../{date}", "/tmp/{date}", "{date:%Y}/../x", "C:/x"] {
        let r = s.execute(
            "library.import",
            &json!({"paths": [src.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": bad}),
        );
        assert!(r.is_err(), "{bad} should be refused");
    }
    assert_eq!(s.catalog.len(), 0);
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [src.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "{date:%Y}/{date:%Y%m%d}"}),
        )
        .unwrap();
    assert_eq!(ids(&r, "imported"), 1, "{r}");
    assert!(dest.join("2026").join("20260114").join("a.png").is_file());
    for d in [&src, &dest] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Auto import: files in the watched folder are added once their size held between two scans,
/// into the named album; non-photos are tried once; the selection stays put.
#[test]
fn auto_import_watched_folder() {
    let dir = temp_dir("watch");
    let mut s = Session::new().with_fs();
    assert!(s.execute("library.autoImport", &json!({"folder": dir.join("nope").to_string_lossy()})).is_err());
    s.execute("library.autoImport", &json!({"folder": dir.to_string_lossy(), "album": "Tethered"})).unwrap();
    assert_eq!(s.execute("library.autoImportScan", &json!({})).unwrap()["imported"], json!([]));
    write_png(&dir.join("one.png"), 1);
    std::fs::write(dir.join("notes.txt"), "not a photo").unwrap();
    // first sight: wait (it may still be copying)
    assert_eq!(s.execute("library.autoImportScan", &json!({})).unwrap()["imported"], json!([]));
    let r = s.execute("library.autoImportScan", &json!({})).unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 1, "{r}");
    let album = s.catalog.albums().find(|a| a.name == "Tethered").expect("album").id;
    assert_eq!(s.catalog.album_count(album), 1);
    assert!(s.selection.ids.is_empty(), "arrivals don't take the selection");
    // nothing new: nothing happens, and the text file isn't retried
    for _ in 0..2 {
        assert_eq!(s.execute("library.autoImportScan", &json!({})).unwrap()["imported"], json!([]));
    }
    write_png(&dir.join("two.png"), 2);
    s.execute("library.autoImportScan", &json!({})).unwrap();
    s.execute("library.autoImportScan", &json!({})).unwrap();
    assert_eq!(s.catalog.album_count(album), 2);
    s.execute("library.autoImport", &json!({"folder": null})).unwrap();
    assert_eq!(s.execute("library.autoImportScan", &json!({})).unwrap()["folder"], json!(null));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Smart previews: with the original offline the photo still renders (and edits apply) from
/// its proxy; without the proxy it can't be opened.
#[test]
fn smart_previews_stand_in_for_offline_originals() {
    let src = temp_dir("smartsrc");
    let lib = temp_dir("smartlib");
    write_png(&src.join("a.png"), 7);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let r = s.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!(r["built"], 1, "{r}");
    assert_eq!(s.execute("photo.smartPreview", &json!({})).unwrap(), json!({"smartPreview": true, "originalOnline": true}));
    let online = s.render_now(id, 48, 32).unwrap().image;
    // the drive goes away
    std::fs::rename(&src, src.with_extension("offline")).unwrap();
    s.media.forget(id);
    assert_eq!(s.execute("photo.smartPreview", &json!({})).unwrap()["originalOnline"], false);
    let offline = s.render_now(id, 48, 32).expect("renders from the smart preview").image;
    let diff: f64 = online.data.iter().zip(&offline.data).map(|(a, b)| (a[0] as f64 - b[0] as f64).abs()).sum::<f64>() / online.data.len() as f64;
    assert!(diff < 6.0, "the proxy looks like the original: {diff}");
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.media.forget(id);
    assert_ne!(s.render_now(id, 48, 32).unwrap().image.data, offline.data, "edits apply offline");
    // without the proxy it can't be opened
    s.execute("library.smartPreviews", &json!({"discard": true})).unwrap();
    s.media.forget(id);
    assert!(s.render_now(id, 48, 32).is_err());
    let _ = std::fs::remove_dir_all(src.with_extension("offline"));
    let _ = std::fs::remove_dir_all(&lib);
}

/// Issue #106: a smart preview cut short (crash, full drive) counted as built forever and failed
/// exactly when the original went offline. Writes are atomic now, and a damaged proxy is not
/// reported as present and is rebuilt.
#[test]
fn damaged_smart_previews_are_rebuilt_and_failed_writes_leave_none() {
    let src = temp_dir("smartdmg-src");
    let lib = temp_dir("smartdmg-lib");
    write_png(&src.join("a.png"), 5);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let dir = s.media.smart_dir.clone().unwrap();
    let file = dir.join(crate::smart::file_name(s.catalog.photo(id).unwrap()));
    {
        let _fault = lightcraft_catalog::safe_file::fail_writes_after(64);
        let r = s.execute("library.smartPreviews", &json!({})).unwrap();
        assert_eq!((r["built"].as_u64(), r["failed"].as_array().map(Vec::len)), (Some(0), Some(1)), "{r}");
    }
    assert!(!file.exists(), "no partial proxy");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no temp file left");
    assert_eq!(s.execute("photo.smartPreview", &json!({})).unwrap()["smartPreview"], false);

    let r = s.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!((r["built"].as_u64(), r["repaired"].as_u64()), (Some(1), Some(0)), "{r}");
    assert!(crate::smart::is_valid(&file));
    // cut short, as by a crash before the fix
    let full = std::fs::read(&file).unwrap();
    std::fs::write(&file, &full[..full.len() / 2]).unwrap();
    assert!(!crate::smart::is_valid(&file));
    assert_eq!(s.execute("photo.smartPreview", &json!({})).unwrap()["smartPreview"], false, "not counted as there");
    let r = s.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!((r["built"].as_u64(), r["repaired"].as_u64()), (Some(1), Some(1)), "{r}");
    assert_eq!(std::fs::read(&file).unwrap(), full, "rebuilt whole");
    // the app's background build (issue #104) checks and repairs the same way, on a worker thread
    std::fs::write(&file, &full[..full.len() / 2]).unwrap();
    let r = s.execute("library.smartPreviews", &json!({"background": true})).unwrap();
    assert_eq!(r["what"], "smart previews", "{r}");
    let t0 = std::time::Instant::now();
    let r = loop {
        let r = s.execute("library.previewProgress", &json!({})).unwrap();
        if r["running"] == false || t0.elapsed() > std::time::Duration::from_secs(60) {
            break r;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!((r["done"].as_u64(), r["failed"].as_u64(), r["repaired"].as_u64()), (Some(1), Some(0), Some(1)), "{r}");
    assert_eq!(std::fs::read(&file).unwrap(), full, "rebuilt whole in the background");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// LUT profiles: a .cube imports as a creative profile (into the library), renders differently
/// (Amount 0 = no change), is listed in the profile browser and comes back with the library.
#[test]
fn cube_luts_become_profiles() {
    let src = temp_dir("cubesrc");
    let lib = temp_dir("cubelib");
    // a warm look: red up, blue down
    let n = 9;
    let mut cube = String::from("TITLE \"Warm Test\"\nLUT_3D_SIZE 9\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let f = |v: usize| v as f32 / (n - 1) as f32;
                cube.push_str(&format!("{} {} {}\n", (f(r) * 1.15).min(1.0), f(g), f(b) * 0.8));
            }
        }
    }
    std::fs::create_dir_all(src.join("Film Looks")).unwrap();
    std::fs::write(src.join("Film Looks/warm.cube"), &cube).unwrap();
    std::fs::write(src.join("Film Looks/broken.cube"), "LUT_3D_SIZE 4\n0 0 0\n").unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, true).unwrap();
    let r = s.execute("profile.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!((r["imported"].as_array().unwrap().len(), r["failed"].as_array().unwrap().len()), (1, 1), "{r}");
    let id = r["imported"][0]["id"].as_str().unwrap().to_string();
    assert_eq!((r["imported"][0]["name"].as_str(), r["imported"][0]["group"].as_str()), (Some("Warm Test"), Some("Film Looks")));
    let menu = s.profile_menu();
    assert!(menu["groups"].as_array().unwrap().iter().any(|g| g["name"] == "Film Looks"), "{menu}");
    let photo = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [photo.0]})).unwrap();
    let base = s.render_now(photo, 48, 32).unwrap().image;
    s.execute("develop.profile", &json!({"id": id, "amount": 100})).unwrap();
    let warm = s.render_now(photo, 48, 32).unwrap().image;
    let mean = |img: &lightcraft_raster::Rgba8, k: usize| img.data.iter().map(|p| p[k] as f64).sum::<f64>() / img.data.len() as f64;
    assert!(
        mean(&warm, 0) > mean(&base, 0) && mean(&warm, 2) < mean(&base, 2),
        "warmer: R {} → {}, B {} → {}",
        mean(&base, 0),
        mean(&warm, 0),
        mean(&base, 2),
        mean(&warm, 2)
    );
    s.execute("develop.profile", &json!({"id": id, "amount": 0})).unwrap();
    assert_eq!(s.render_now(photo, 48, 32).unwrap().image.data, base.data, "Amount 0 = the photo as it was");
    // the library keeps it
    drop(s);
    lightcraft_pipeline::lut::unregister(&id);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert!(s.lut_profiles.iter().any(|p| p.id == id));
    assert!(lightcraft_pipeline::lut::get(&id).is_some(), "registered again on open");
    s.execute("profile.deleteImported", &json!({"id": id})).unwrap();
    assert!(lightcraft_pipeline::lut::get(&id).is_none());
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Folder rename / move on disk: files and sidecars go along, the photos are relinked and
/// still render; clashes and moving into itself are refused.
#[test]
fn folders_rename_and_move_with_their_photos() {
    let root = temp_dir("folders");
    let a = root.join("Trip");
    write_png(&a.join("one.png"), 3);
    std::fs::write(a.join("one.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
    std::fs::create_dir_all(root.join("Taken")).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [a.to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    assert!(s.execute("folder.rename", &json!({"path": a.to_string_lossy(), "name": "Taken"})).is_err(), "name taken");
    assert!(s.execute("folder.rename", &json!({"path": a.to_string_lossy(), "name": "a/b"})).is_err());
    let r = s.execute("folder.rename", &json!({"path": a.to_string_lossy(), "name": "Italy 2026"})).unwrap();
    assert_eq!(r["relinked"], 1);
    let b = root.join("Italy 2026");
    assert!(b.join("one.png").exists() && b.join("one.xmp").exists() && !a.exists());
    let path = |s: &Session| match &s.catalog.photo(id).unwrap().source {
        lightcraft_catalog::Source::File { path } => path.clone(),
        _ => panic!(),
    };
    assert_eq!(path(&s), b.join("one.png").to_string_lossy());
    s.media.forget(id);
    assert!(s.render_now(id, 16, 16).is_ok());
    assert!(s.execute("folder.move", &json!({"path": b.to_string_lossy(), "into": b.join("deeper").to_string_lossy()})).is_err(), "not into itself");
    let r = s.execute("folder.move", &json!({"path": b.to_string_lossy(), "into": root.join("Archive").to_string_lossy()})).unwrap();
    assert_eq!(r["relinked"], 1);
    assert_eq!(path(&s), root.join("Archive/Italy 2026/one.png").to_string_lossy());
    let _ = std::fs::remove_dir_all(&root);
}

/// Rename / Move Folder are undo steps (issue #97): undo renames the folder back on disk (files and
/// sidecars along) and restores the photos' paths, redo repeats it; a destination taken in the
/// meantime is never overwritten — the step is refused, reported and stays on the stack.
#[test]
fn folder_rename_and_move_undo_and_redo_on_disk() {
    let root = temp_dir("folders-undo");
    let a = root.join("Trip");
    write_png(&a.join("one.png"), 3);
    std::fs::write(a.join("one.xmp"), "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [a.to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let path = |s: &Session| match &s.catalog.photo(id).unwrap().source {
        lightcraft_catalog::Source::File { path } => path.clone(),
        _ => panic!(),
    };
    let undo0 = s.undo.len();
    let b = root.join("Italy");
    s.execute("folder.rename", &json!({"path": a.to_string_lossy(), "name": "Italy"})).unwrap();
    assert_eq!(s.undo.len(), undo0 + 1, "one undo step");
    assert!(s.undo.last().unwrap().label.contains("Rename Folder"));
    assert!(b.join("one.png").exists() && !a.exists());

    s.execute("edit.undo", &json!({})).unwrap();
    assert!(a.join("one.png").exists() && a.join("one.xmp").exists() && !b.exists(), "renamed back on disk");
    assert_eq!(path(&s), a.join("one.png").to_string_lossy());
    s.media.forget(id);
    assert!(s.render_now(id, 16, 16).is_ok());

    s.execute("edit.redo", &json!({})).unwrap();
    assert!(b.join("one.png").exists() && b.join("one.xmp").exists() && !a.exists());
    assert_eq!(path(&s), b.join("one.png").to_string_lossy());

    // the old name is taken now: undo is refused, nothing is overwritten, the step stays
    std::fs::create_dir_all(&a).unwrap();
    std::fs::write(a.join("keep.txt"), "mine").unwrap();
    let undo_n = s.undo.len();
    let e = s.execute("edit.undo", &json!({})).unwrap_err().to_string();
    assert!(e.contains("already exists"), "{e}");
    assert_eq!(s.undo.len(), undo_n, "the step stays on the stack");
    assert_eq!(std::fs::read_to_string(a.join("keep.txt")).unwrap(), "mine");
    assert!(b.join("one.png").exists());
    assert_eq!(path(&s), b.join("one.png").to_string_lossy());
    std::fs::remove_dir_all(&a).unwrap();

    // move into another folder, undo, redo
    let into = root.join("Archive");
    s.execute("folder.move", &json!({"path": b.to_string_lossy(), "into": into.to_string_lossy()})).unwrap();
    let moved = into.join("Italy");
    assert_eq!(path(&s), moved.join("one.png").to_string_lossy());
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(b.join("one.png").exists() && !moved.exists());
    assert_eq!(path(&s), b.join("one.png").to_string_lossy());
    // the redo target is taken (even by an empty folder): refused, nothing replaced
    std::fs::create_dir_all(&moved).unwrap();
    assert!(s.execute("edit.redo", &json!({})).is_err());
    assert!(b.join("one.png").exists());
    std::fs::remove_dir(&moved).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(moved.join("one.png").exists() && moved.join("one.xmp").exists() && !b.exists());
    assert_eq!(path(&s), moved.join("one.png").to_string_lossy());
    let _ = std::fs::remove_dir_all(&root);
}

/// Assisted culling: a burst (same scene, seconds apart) of a sharp and two soft shots, plus an
/// unrelated photo: the burst is grouped, the sharp one is its best, blurry shots can be rejected.
#[test]
fn assisted_culling_groups_bursts_and_scores_focus() {
    let dir = temp_dir("cull");
    let scene = lightcraft_scenes::demo_library()[0].render(480, 320);
    let soften = |img: &lightcraft_raster::Rgb32f, r: usize| {
        let (w, h) = (img.width, img.height);
        let mut out = img.clone();
        for y in 0..h {
            for x in 0..w {
                let (mut acc, mut n) = ([0f32; 3], 0.0);
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        let c = img.data[yy * w + xx];
                        acc = [acc[0] + c[0], acc[1] + c[1], acc[2] + c[2]];
                        n += 1.0;
                    }
                }
                out.data[y * w + x] = acc.map(|v| v / n);
            }
        }
        out
    };
    let save = |img: &lightcraft_raster::Rgb32f, name: &str| {
        let data: Vec<[u8; 4]> = img
            .data
            .iter()
            .map(|c| {
                let e = |v: f32| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
                [e(c[0]), e(c[1]), e(c[2]), 255]
            })
            .collect();
        let png = lightcraft_codecs::encode_png(
            &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: img.width, height: img.height, data }),
            &Default::default(),
        )
        .unwrap();
        std::fs::write(dir.join(name), png).unwrap();
    };
    save(&soften(&scene, 3), "a_soft.png");
    save(&scene, "b_sharp.png");
    save(&soften(&scene, 6), "c_softer.png");
    save(&lightcraft_scenes::demo_library()[7].render(480, 320), "d_other.png");
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    let ids: Vec<u64> = r["imported"].as_array().unwrap().iter().filter_map(Value::as_u64).collect();
    let by_name = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).unwrap().id;
    for (i, n) in ["a_soft.png", "b_sharp.png", "c_softer.png"].iter().enumerate() {
        let id = by_name(&s, n);
        s.commit("t", lightcraft_catalog::Op::SetCaptured { id, captured: Some(format!("2026-05-01T10:00:0{i}")) }).unwrap();
    }
    let other = by_name(&s, "d_other.png");
    s.commit("t", lightcraft_catalog::Op::SetCaptured { id: other, captured: Some("2026-05-01T10:00:04".into()) }).unwrap();
    let r = s.execute("photo.analyze", &json!({"ids": ids, "rejectBelow": 0.0, "pickBest": true})).unwrap();
    assert_eq!(r["groups"], 1, "{r}");
    let a = |s: &Session, n: &str| s.catalog.photo(by_name(s, n)).unwrap().analysis.unwrap();
    let (soft, sharp, softer, odd) = (a(&s, "a_soft.png"), a(&s, "b_sharp.png"), a(&s, "c_softer.png"), a(&s, "d_other.png"));
    eprintln!("sharpness: sharp {} soft {} softer {} other {}", sharp.sharpness, soft.sharpness, softer.sharpness, odd.sharpness);
    assert!(sharp.sharpness > soft.sharpness && soft.sharpness > softer.sharpness);
    assert!(sharp.best && !soft.best && sharp.group.is_some() && sharp.group == softer.group);
    assert!(odd.group.is_none(), "an unrelated photo isn't in the burst");
    assert_eq!(s.catalog.photo(by_name(&s, "b_sharp.png")).unwrap().flag, lightcraft_catalog::Flag::Pick);
    // reject the blurry ones (by score), as a rule too
    let cut = (soft.sharpness + sharp.sharpness) / 2.0;
    s.execute("photo.analyze", &json!({"ids": ids, "rejectBelow": cut})).unwrap();
    assert_eq!(s.catalog.photo(by_name(&s, "c_softer.png")).unwrap().flag, lightcraft_catalog::Flag::Reject);
    s.execute("library.filter", &json!({"ruleSet": {"rules": [{"field": "bestOfGroup", "op": "is", "value": true}]}})).unwrap();
    let vis = s.visible_cloned();
    assert!(vis.contains(&by_name(&s, "b_sharp.png")) && !vis.contains(&by_name(&s, "a_soft.png")) && vis.contains(&other));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Find Dust Spots: soft dark spots on a smooth sky are found and healed (one undo step).
#[test]
fn dust_spots_are_found_and_healed() {
    let dir = temp_dir("dust");
    let (w, h) = (900usize, 600usize);
    let spots = [(180.0f32, 140.0f32), (600.0, 300.0)];
    let data: Vec<[u8; 4]> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            let mut v = 0.5 + 0.3 * (y / h as f32);
            for (cx, cy) in spots {
                v -= 0.07 * (-((x - cx).powi(2) + (y - cy).powi(2)) / (2.0 * 25.0)).exp();
            }
            let b = (v * 255.0) as u8;
            [b, b, (b as f32 * 1.15).min(255.0) as u8, 255]
        })
        .collect();
    let png = lightcraft_codecs::encode_png(
        &lightcraft_codecs::EncodeImage::rgba8(&lightcraft_raster::Rgba8 { width: w, height: h, data }),
        &Default::default(),
    )
    .unwrap();
    std::fs::write(dir.join("sky.png"), png).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [dir.join("sky.png").to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let before = s.render_now(id, 900, 600).unwrap().image;
    let r = s.execute("spot.findDust", &json!({})).unwrap();
    assert_eq!(r["added"], 2, "{r}");
    let after = s.render_now(id, 900, 600).unwrap().image;
    let at = |img: &lightcraft_raster::Rgba8, x: usize, y: usize| img.data[y * 900 + x][0] as i32;
    for (cx, cy) in spots {
        assert!(at(&after, cx as usize, cy as usize) > at(&before, cx as usize, cy as usize) + 6, "the spot at {cx},{cy} is lifted");
    }
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.develop_of(id).unwrap().spots.is_empty(), "one undo step");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Smart previews can live outside the library: the folder is saved per library, used for
/// building, status and offline rendering, changing it needs an explicit choice for the
/// previews already built, and an unavailable folder is an error (never a silent fallback).
#[test]
fn smart_previews_folder_is_chosen_per_library() {
    let src = temp_dir("smartloc-src");
    let lib = temp_dir("smartloc-lib");
    let away = temp_dir("smartloc-away");
    write_png(&src.join("a.png"), 7);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("a.png").to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let loc = s.execute("library.smartPreviewsLocation", &json!({})).unwrap();
    assert_eq!(loc["custom"], false);
    assert_eq!(loc["path"], lib.join("Smart Previews").to_string_lossy().as_ref());
    s.execute("library.smartPreviews", &json!({})).unwrap();
    assert_eq!(crate::smart::stats(&lib.join("Smart Previews")).0, 1);

    // relative paths and missing drives are refused, and nothing is created on this drive
    assert!(s.execute("library.smartPreviewsLocation", &json!({"path": "rel/dir"})).is_err());
    let gone = away.join("unplugged/previews");
    let err = s.execute("library.smartPreviewsLocation", &json!({"path": gone.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("not available"), "{err}");
    assert!(!away.join("unplugged").exists());
    // the previews already built need an explicit choice
    let to = away.join("proxies");
    let err = s.execute("library.smartPreviewsLocation", &json!({"path": to.to_string_lossy()})).unwrap_err().to_string();
    assert!(err.contains("existing"), "{err}");
    assert_eq!(s.execute("library.smartPreviewsLocation", &json!({})).unwrap()["custom"], false);

    let r = s.execute("library.smartPreviewsLocation", &json!({"path": to.to_string_lossy(), "existing": "move"})).unwrap();
    assert_eq!((r["custom"].clone(), r["handled"].clone(), r["count"].clone()), (json!(true), json!(1), json!(1)), "{r}");
    assert_eq!(crate::smart::stats(&lib.join("Smart Previews")).0, 0);
    assert_eq!(s.execute("photo.smartPreview", &json!({})).unwrap()["smartPreview"], true);

    // the setting is saved with the library
    drop(s); // one session per library (issue #99)
    let mut again = Session::new().with_fs();
    again.open_library(&lib, false).unwrap();
    assert_eq!(again.execute("library.smartPreviewsLocation", &json!({})).unwrap()["path"], to.to_string_lossy().as_ref());

    // offline original: the render comes from the proxy in the chosen folder
    std::fs::rename(&src, src.with_extension("offline")).unwrap();
    again.media.forget(id);
    again.render_now(id, 48, 32).expect("renders from the smart preview in the chosen folder");

    // the chosen drive goes away: building says so and creates nothing on this drive
    let drive = away.join("drive");
    std::fs::create_dir_all(&drive).unwrap();
    let on_drive = drive.join("proxies");
    again.execute("library.smartPreviewsLocation", &json!({"path": on_drive.to_string_lossy(), "existing": "leave"})).unwrap();
    std::fs::remove_dir_all(&drive).unwrap();
    let loc = again.execute("library.smartPreviewsLocation", &json!({})).unwrap();
    assert_eq!(loc["available"], false, "{loc}");
    let err = again.execute("library.smartPreviews", &json!({"ids": [id.0]})).unwrap_err().to_string();
    assert!(err.contains("not available"), "{err}");
    assert!(!drive.exists());

    // back to the library's own folder, discarding the proxies left in the first chosen folder
    let r = again.execute("library.smartPreviewsLocation", &json!({"reset": true})).unwrap();
    assert_eq!(r["custom"], false, "{r}");
    assert_eq!(r["path"], lib.join("Smart Previews").to_string_lossy().as_ref());
}

/// Missing Photos covers library photos only: a small library next to many Local browse
/// records (and a deleted photo) checks — and lists, and counts — the library files alone.
#[test]
fn missing_photos_skip_local_browse_records() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    let gone = std::env::temp_dir().join(format!("lc-missing-scope-{}", std::process::id()));
    let mut s = Session::new();
    let mut ops = Vec::new();
    for i in 0..205u64 {
        let path = gone.join(format!("IMG_{i:04}.jpg")).to_string_lossy().to_string();
        let mut p = Photo::new(PhotoId(i + 1), Source::File { path }, &format!("IMG_{i:04}.jpg"), "JPEG", 60, 40, "2026-01-01T00:00:00");
        p.local = i >= 5; // 5 library photos, 200 seen while browsing
        p.deleted = i == 4;
        ops.push(Op::AddPhoto { photo: Box::new(p) });
    }
    s.commit("Add", Op::Batch { ops }).unwrap();
    let mut checked = 0;
    let lost = crate::cmd::missing::missing_with(&s.catalog, |_| {
        checked += 1;
        false
    });
    assert_eq!((checked, lost.len()), (4, 4), "only the non-deleted library files are checked");
    assert_eq!(crate::cmd::missing::candidates(&s.catalog).len(), 4, "the sidebar count checks the same files");
    assert_eq!(s.execute("library.missing", &json!({})).unwrap().as_array().map(Vec::len), Some(4));
    s.execute("library.source", &json!({"kind": "missing"})).unwrap();
    assert_eq!(s.visible_cloned().len(), 4, "the view lists what the count counts");
}
