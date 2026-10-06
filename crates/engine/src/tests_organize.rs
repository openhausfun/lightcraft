//! Organizing commands: smart albums, stacks, virtual copies — through `Session::execute`, with
//! undo and a persistent library (journal replay must reproduce the live state).

use lightcraft_catalog::Flag;
use serde_json::json;

use crate::{LibrarySource, Session};

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lc-organize-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn open(dir: &std::path::Path) -> Session {
    let mut s = Session::new();
    s.open_library(dir, true).unwrap();
    s
}

#[test]
fn smart_album_from_view_updates_live_and_persists() {
    let dir = temp_dir("smart");
    let mut s = open(&dir);
    s.execute("library.filter", &json!({"rating": 4})).unwrap();
    let want = s.visible_cloned();
    assert!(!want.is_empty());
    let r = s.execute("album.createSmart", &json!({"name": "Four plus"})).unwrap();
    let id = r["id"].as_u64().unwrap();
    assert_eq!(r["count"].as_u64().unwrap() as usize, want.len());
    s.execute("library.clearFilter", &json!({})).unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": id})).unwrap();
    let mut got = s.visible_cloned();
    got.sort();
    let mut w = want.clone();
    w.sort();
    assert_eq!(got, w);
    // live: rating another photo 5 adds it; rating a member 1 removes it
    let other = s.catalog.photos().find(|p| p.rating < 4 && !p.deleted).unwrap().id;
    s.execute("photo.rate", &json!({"ids": [other.0], "rating": 5})).unwrap();
    assert!(s.visible_cloned().contains(&other));
    s.execute("photo.rate", &json!({"ids": [want[0].0], "rating": 1})).unwrap();
    assert!(!s.visible_cloned().contains(&want[0]));
    // manual membership changes are refused
    assert!(s.execute("album.addPhotos", &json!({"id": id, "ids": [want[0].0]})).is_err());
    // edit rules (merge), rename; undo the rename
    s.execute("album.setRules", &json!({"id": id, "rules": {"flag": "pick"}})).unwrap();
    let rules = s.catalog.album(lightcraft_catalog::AlbumId(id)).unwrap().smart.clone().unwrap();
    assert_eq!((rules.rating, rules.flag), (4, Some(Flag::Pick)));
    s.execute("album.rename", &json!({"id": id, "name": "Best picks"})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.album(lightcraft_catalog::AlbumId(id)).unwrap().name, "Four plus");
    // rules via params: camera + lens + date range
    let r2 = s
        .execute("album.createSmart", &json!({"name": "Range", "rules": {"dateFrom": "2000-01-01", "dateTo": "2100", "lens": "", "edited": true}}))
        .unwrap();
    let edited = s.catalog.photos().filter(|p| !p.deleted && p.is_edited()).count();
    assert_eq!(r2["count"].as_u64().unwrap() as usize, edited);
    let listed = s.execute("albums.list", &json!({})).unwrap();
    assert!(listed.to_string().contains("\"rulesText\""), "{listed}");
    let expect = s.catalog.to_snapshot();
    drop(s); // crash: replay the log
    let mut s2 = open(&dir);
    assert_eq!(s2.catalog.to_snapshot(), expect);
    // deleting the smart album
    s2.execute("album.delete", &json!({"id": id})).unwrap();
    assert!(s2.catalog.album(lightcraft_catalog::AlbumId(id)).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn smart_album_from_smart_album_view_keeps_its_rules() {
    let mut s = Session::with_demo();
    let id = s.execute("album.createSmart", &json!({"name": "Picks", "rules": {"flag": "pick"}})).unwrap()["id"].as_u64().unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": id})).unwrap();
    s.execute("library.filter", &json!({"rating": 3})).unwrap();
    let r = s.view_rules();
    assert_eq!((r.flag, r.rating, r.album), (Some(Flag::Pick), 3, None));
    assert_eq!(s.source, LibrarySource::Album(lightcraft_catalog::AlbumId(id)));
}

#[test]
fn virtual_copies_are_independent_and_persist() {
    let dir = temp_dir("vc");
    let mut s = open(&dir);
    let src = s.visible_cloned()[1];
    let album = s.execute("album.create", &json!({"name": "Keep", "addSelected": false})).unwrap()["id"].as_u64().unwrap();
    s.execute("album.addPhotos", &json!({"id": album, "ids": [src.0]})).unwrap();
    s.execute("library.select", &json!({"ids": [src.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
    let undo_before = s.undo.len();
    let r = s.execute("photo.virtualCopy", &json!({})).unwrap();
    assert_eq!(s.undo.len(), undo_before + 1);
    let c1 = lightcraft_catalog::PhotoId(r["ids"][0].as_u64().unwrap());
    let r2 = s.execute("photo.virtualCopy", &json!({"ids": [src.0]})).unwrap();
    let c2 = lightcraft_catalog::PhotoId(r2["ids"][0].as_u64().unwrap());
    let (p, a, b) = (s.catalog.photo(src).unwrap().clone(), s.catalog.photo(c1).unwrap().clone(), s.catalog.photo(c2).unwrap().clone());
    assert_eq!((a.copy_of, a.copy_name.as_deref()), (Some(src), Some("Copy 1")));
    assert_eq!(b.copy_name.as_deref(), Some("Copy 2"));
    assert_eq!(a.source, p.source, "shares the original file");
    assert_eq!(a.develop.light.exposure, 0.5, "starts from the current settings");
    // a copy of a copy names itself after the master
    let r3 = s.execute("photo.virtualCopy", &json!({"ids": [c1.0]})).unwrap();
    let c3 = s.catalog.photo(lightcraft_catalog::PhotoId(r3["ids"][0].as_u64().unwrap())).unwrap().clone();
    assert_eq!((c3.copy_of, c3.copy_name.as_deref()), (Some(src), Some("Copy 3")));
    // independent settings
    s.execute("library.select", &json!({"ids": [c1.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": -1.0})).unwrap();
    assert_eq!(s.catalog.photo(src).unwrap().develop.light.exposure, 0.5);
    assert_eq!(s.catalog.photo(c1).unwrap().develop.light.exposure, -1.0);
    // same album, stacked (expanded) with the original, visible in the grid
    assert!(s.catalog.album(lightcraft_catalog::AlbumId(album)).unwrap().photos.starts_with(&[src, c2, c1, c3.id]));
    assert_eq!(s.catalog.stack_of(c1).unwrap().photos, vec![src, c2, c1, c3.id]);
    assert!(s.visible_cloned().contains(&c2));
    let found = s.execute("catalog.query", &json!({"filter": {"text": "copy:yes"}})).unwrap();
    assert_eq!(found["total"], 3);
    // undo removes a copy completely
    s.execute("edit.undo", &json!({})).unwrap(); // exposure
    s.execute("edit.undo", &json!({})).unwrap(); // third copy
    assert!(s.catalog.photo(c3.id).is_none());
    let expect = s.catalog.to_snapshot();
    drop(s);
    let s2 = open(&dir);
    assert_eq!(s2.catalog.to_snapshot(), expect);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stacks_group_collapse_and_replay() {
    let dir = temp_dir("stacks");
    let mut s = open(&dir);
    let vis = s.visible_cloned();
    let n = vis.len();
    let (a, b, c) = (vis[2], vis[3], vis[4]);
    s.execute("library.select", &json!({"ids": [a.0, b.0, c.0], "active": b.0})).unwrap();
    let r = s.execute("stack.group", &json!({})).unwrap();
    assert_eq!(r["count"], 3);
    // collapsed by default: the top (the active photo) stands for the stack
    let v = s.visible_cloned();
    assert_eq!(v.len(), n - 2);
    assert!(v.contains(&b) && !v.contains(&a) && !v.contains(&c));
    assert_eq!(s.selection.active, Some(b));
    let info = s.execute("photo.inspect", &json!({"id": a.0})).unwrap();
    assert_eq!(info["stack"]["photos"], json!([b.0, a.0, c.0]));
    // expand: members follow the top
    s.execute("stack.toggle", &json!({})).unwrap();
    let v = s.visible_cloned();
    let i = v.iter().position(|x| *x == b).unwrap();
    assert_eq!(&v[i..i + 3], &[b, a, c]);
    // set top, collapse with a member active → active moves to the visible top
    s.execute("library.select", &json!({"ids": [c.0]})).unwrap();
    s.execute("stack.setTop", &json!({})).unwrap();
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute("stack.toggle", &json!({})).unwrap();
    assert_eq!(s.selection.active, Some(c));
    // remove from stack; undo
    s.execute("stack.remove", &json!({"ids": [a.0]})).unwrap();
    assert_eq!(s.catalog.stack_of(c).unwrap().photos, vec![c, b]);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.stack_of(c).unwrap().photos.len(), 3);
    // auto-stack the rest of the view: a huge gap groups everything unstacked
    let pre = s.execute("stack.auto", &json!({"gap": 1e9, "preview": true})).unwrap();
    assert_eq!(pre["stacks"], 1);
    assert_eq!(s.catalog.stacks().count(), 1, "preview changes nothing");
    s.execute("stack.auto", &json!({"gap": 1e9})).unwrap();
    assert_eq!(s.catalog.stacks().count(), 2);
    assert_eq!(s.visible_cloned().len(), 2);
    s.execute("stack.expandAll", &json!({})).unwrap();
    assert_eq!(s.visible_cloned().len(), n);
    let expect = s.catalog.to_snapshot();
    drop(s);
    let mut s2 = open(&dir);
    assert_eq!(s2.catalog.to_snapshot(), expect);
    s2.execute("library.select", &json!({"ids": [b.0]})).unwrap();
    s2.execute("stack.ungroup", &json!({})).unwrap();
    assert_eq!(s2.catalog.stacks().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn album_source_reports_in_library_state_and_persists_the_view() {
    // An album source made `library.state` panic (serde can't put an integer in an internally
    // tagged enum) and `view.json` empty, so the view was lost on the next launch.
    let dir = temp_dir("album-view");
    let mut s = open(&dir);
    let id = s.execute("album.create", &json!({"name": "Trip"})).unwrap()["id"].as_u64().unwrap();
    s.execute("library.source", &json!({"kind": "album", "id": id})).unwrap();
    s.execute("library.sort", &json!({"key": "fileName"})).unwrap();
    let st = s.execute("library.state", &json!({})).unwrap();
    assert_eq!(st["source"], json!({"kind": "album", "id": id}));
    // the reported source is what `library.source` takes
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    s.execute("library.source", &st["source"]).unwrap();
    assert_eq!(s.source, LibrarySource::Album(lightcraft_catalog::AlbumId(id)));
    assert_eq!(s.execute("library.state", &json!({})).unwrap()["source"], json!({"kind": "album", "id": id}));
    s.close_library().unwrap();
    drop(s);
    let s2 = open(&dir);
    assert_eq!(s2.source, LibrarySource::Album(lightcraft_catalog::AlbumId(id)));
    assert_eq!(serde_json::to_value(s2.sort).unwrap()["key"], "fileName", "the rest of the view is restored too");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn filter_chips_clear_one_filter_and_total_ignores_the_filter() {
    let dir = temp_dir("chips");
    let mut s = open(&dir);
    let all = s.visible_cloned().len();
    assert!(all > 1);
    assert_eq!(s.source_total(), Some(all));
    s.execute("library.filter", &json!({"rating": 4, "date": "2026-01-16", "text": "x"})).unwrap();
    // the total is the source's, whatever the filter hides
    assert_eq!(s.source_total(), Some(all));
    let chips = crate::filter_chips(&s.filter, &s.catalog);
    let labels: Vec<&str> = chips.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Search: “x”", "Rating ≥ 4★", "Date: January 16, 2026"]);
    // clearing the date chip leaves the other two filters alone
    s.execute("library.filter", &chips[2].clear).unwrap();
    assert_eq!(s.filter.date, None);
    assert_eq!((s.filter.rating, s.filter.text.as_str()), (4, "x"));
    for c in crate::filter_chips(&s.filter, &s.catalog) {
        s.execute("library.filter", &c.clear).unwrap();
    }
    assert_eq!(s.filter, Default::default());
}
