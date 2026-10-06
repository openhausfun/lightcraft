use std::sync::Arc;

use lightcraft_develop::DevelopSettings;

use super::*;

fn photo(c: &mut Catalog, name: &str, date: &str) -> PhotoId {
    let id = c.alloc_photo_id();
    let mut p = Photo::new(id, Source::Demo { scene: 1 }, name, "JPEG", 6000, 4000, "2026-09-30T10:00:00");
    p.captured = Some(date.to_string());
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

#[test]
fn apply_and_inverse_roundtrip() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01T10:00:00");
    let before = c.clone();
    let ops = vec![
        Op::SetRating { id: a, rating: 4 },
        Op::SetFlag { id: a, flag: Flag::Pick },
        Op::SetLabel { id: a, label: Some(ColorLabel::Red) },
        Op::SetDevelop {
            id: a,
            settings: Arc::new(DevelopSettings { treatment: lightcraft_develop::Treatment::Bw, ..Default::default() }),
            label: "B&W".into(),
            edited: Some("x".into()),
        },
    ];
    let mut invs = Vec::new();
    for op in ops {
        invs.push(c.apply(op).unwrap());
    }
    assert_eq!(c.photo(a).unwrap().rating, 4);
    for inv in invs.into_iter().rev() {
        c.apply(inv).unwrap();
    }
    assert_eq!(c.photos().collect::<Vec<_>>(), before.photos().collect::<Vec<_>>());
}

#[test]
fn invalid_ops_change_nothing() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01");
    let snap = c.to_snapshot();
    assert!(c.apply(Op::SetRating { id: a, rating: 9 }).is_err());
    assert!(c.apply(Op::SetRating { id: PhotoId(999), rating: 1 }).is_err());
    // batch with a failing op rolls back
    let r = c.apply(Op::Batch { ops: vec![Op::SetRating { id: a, rating: 2 }, Op::SetFlag { id: PhotoId(77), flag: Flag::Pick }] });
    assert!(r.is_err());
    assert_eq!(c.to_snapshot(), snap);
}

#[test]
fn albums_and_folders() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01");
    let f = c.alloc_album_id();
    c.apply(Op::AddAlbum {
        album: Album { id: f, name: "Trips".into(), parent: None, folder: true, photos: vec![], cover: None, smart: None, quick: false },
    })
    .unwrap();
    let al = c.alloc_album_id();
    c.apply(Op::AddAlbum {
        album: Album { id: al, name: "Alps".into(), parent: Some(f), folder: false, photos: vec![a], cover: None, smart: None, quick: false },
    })
    .unwrap();
    assert_eq!(c.albums_of(a), vec![al]);
    assert!(c.apply(Op::RemoveAlbum { id: f }).is_err(), "non-empty folder");
    assert!(c.apply(Op::SetAlbumPhotos { id: f, photos: vec![a] }).is_err(), "folders hold no photos");
    assert!(c.apply(Op::MoveAlbum { id: f, parent: Some(f) }).is_err());
    let del = c.delete_permanently_ops(a);
    let inv = c.apply(del).unwrap();
    assert!(c.photo(a).is_none() && c.album(al).unwrap().photos.is_empty());
    c.apply(inv).unwrap();
    assert!(c.photo(a).is_some() && c.album(al).unwrap().photos == vec![a]);
}

#[test]
fn filter_search_sort() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "beach.jpg", "2026-05-01T10:00:00");
    let b = photo(&mut c, "alps.jpg", "2026-04-01T10:00:00");
    let d = photo(&mut c, "city.jpg", "2025-12-24T10:00:00");
    c.apply(Op::SetRating { id: b, rating: 5 }).unwrap();
    c.apply(Op::SetFlag { id: d, flag: Flag::Reject }).unwrap();
    let mut m = c.photo(a).unwrap().meta.clone();
    m.keywords = vec!["ocean".into(), "summer".into()];
    m.iso = Some(1600);
    c.apply(Op::SetMeta { id: a, meta: Box::new(m) }).unwrap();

    let all = c.query(&Filter::default(), &Sort::default());
    assert_eq!(all, vec![a, b, d], "newest first");
    let asc = c.query(&Filter::default(), &Sort { key: SortKey::FileName, ascending: true, ..Default::default() });
    assert_eq!(asc, vec![b, a, d]);
    assert_eq!(c.query(&Filter { rating: 4, ..Default::default() }, &Sort::default()), vec![b]);
    assert_eq!(c.query(&Filter { flag: Some(Flag::Reject), ..Default::default() }, &Sort::default()), vec![d]);
    assert_eq!(c.query(&Filter { text: "summer".into(), ..Default::default() }, &Sort::default()), vec![a]);
    assert_eq!(c.query(&Filter { text: "iso:>800".into(), ..Default::default() }, &Sort::default()), vec![a]);
    assert_eq!(c.query(&Filter { text: "date:2025".into(), ..Default::default() }, &Sort::default()), vec![d]);
    assert_eq!(c.query(&Filter { date: Some("2026-04".into()), ..Default::default() }, &Sort::default()), vec![b]);
    c.apply(Op::SetDeleted { id: d, deleted: true }).unwrap();
    assert_eq!(c.query(&Filter::default(), &Sort::default()).len(), 2);
    assert_eq!(c.query(&Filter { deleted: true, ..Default::default() }, &Sort::default()), vec![d]);
    let g = c.date_groups();
    assert_eq!(g[0].year, "2026");
    assert_eq!(g[0].count, 2);
    assert_eq!(g[0].months.iter().map(|m| m.1).sum::<usize>(), 2);
    assert_eq!(g[0].days.iter().map(|d| d.1).sum::<usize>(), 2);
    assert!(g[0].days.iter().all(|(d, _)| d.len() == 10 && d.starts_with("2026")));
    assert!(g[0].days.windows(2).all(|w| w[0].0 > w[1].0), "newest first");
    assert_eq!(c.keywords(), vec![("ocean".to_string(), 1), ("summer".to_string(), 1)]);
}

#[test]
fn snapshot_and_log_replay() {
    let mut c = Catalog::new();
    let snap0 = c.to_snapshot();
    let mut log = String::new();
    let a = c.alloc_photo_id();
    let ops = vec![
        Op::AddPhoto { photo: Box::new(Photo::new(a, Source::File { path: "/x/a.jpg".into() }, "a.jpg", "JPEG", 10, 10, "t")) },
        Op::SetRating { id: a, rating: 3 },
        Op::SetFlag { id: a, flag: Flag::Pick },
    ];
    for op in ops {
        log.push_str(&Catalog::op_to_log_line(&op));
        c.apply(op).unwrap();
    }
    let mut r = Catalog::from_snapshot(&snap0).unwrap();
    // torn last line is tolerated
    let torn = format!("{log}{{\"op\":\"setRat");
    assert_eq!(r.replay(&torn).unwrap(), 3);
    assert_eq!(r.to_snapshot(), c.to_snapshot());
    assert!(Catalog::from_snapshot("{nope").is_err());
}

proptest::proptest! {
    #[test]
    fn random_ops_undo_to_start(ratings in proptest::collection::vec((0usize..3, 0u8..6, 0u8..3), 1..40)) {
        let mut c = Catalog::new();
        let ids: Vec<PhotoId> = (0..3).map(|i| photo(&mut c, &format!("p{i}.jpg"), "2026-01-01")).collect();
        let start = c.to_snapshot();
        let mut invs = Vec::new();
        for (i, r, f) in ratings {
            let op = if f == 0 { Op::SetRating { id: ids[i], rating: r } } else { Op::SetFlag { id: ids[i], flag: [Flag::None, Flag::Pick, Flag::Reject][f as usize] } };
            if let Ok(inv) = c.apply(op) { invs.push(inv); }
        }
        for inv in invs.into_iter().rev() { c.apply(inv).unwrap(); }
        proptest::prop_assert_eq!(c.to_snapshot(), start);
    }
}

#[test]
fn smart_albums_update_live() {
    let mut c = Catalog::new();
    let a = photo(&mut c, "a.jpg", "2026-04-01T10:00:00");
    let b = photo(&mut c, "b.jpg", "2026-05-02T10:00:00");
    let z = photo(&mut c, "z.jpg", "2025-12-31T10:00:00");
    let id = c.alloc_album_id();
    let rules = Filter { rating: 3, date_from: Some("2026-01-01".into()), date_to: Some("2026-04".into()), ..Default::default() };
    c.apply(Op::AddAlbum { album: Album { smart: Some(Box::new(rules)), ..Album::new(id, "Good spring") } }).unwrap();
    assert!(c.album_photos(id).is_empty());
    for p in [a, b, z] {
        c.apply(Op::SetRating { id: p, rating: 4 }).unwrap();
    }
    // b is after the range, z before it
    assert_eq!(c.album_photos(id), vec![a]);
    assert_eq!(c.album_count(id), 1);
    assert_eq!(c.albums_of(a), vec![id]);
    // via the album filter (what the grid uses)
    assert_eq!(c.query(&Filter { album: Some(id), ..Default::default() }, &Sort::default()), vec![a]);
    // deleted photos drop out
    c.apply(Op::SetDeleted { id: a, deleted: true }).unwrap();
    assert!(c.album_photos(id).is_empty());
    c.apply(Op::SetDeleted { id: a, deleted: false }).unwrap();
    // change the rules; inverse restores
    let inv = c.apply(Op::SetAlbumRules { id, rules: Box::new(Filter { rating: 4, rating_op: RatingOp::Exactly, ..Default::default() }) }).unwrap();
    assert_eq!(c.album_photos(id), vec![a, b, z]);
    c.apply(inv).unwrap();
    assert_eq!(c.album_photos(id), vec![a]);
    // smart albums hold no photos and can't nest smart rules
    assert!(c.apply(Op::SetAlbumPhotos { id, photos: vec![b] }).is_err());
    let id2 = c.alloc_album_id();
    let bad = Filter { album: Some(id), ..Default::default() };
    assert!(c.apply(Op::AddAlbum { album: Album { smart: Some(Box::new(bad)), ..Album::new(id2, "x") } }).is_err());
    assert!(c.apply(Op::SetAlbumRules { id, rules: Box::new(Filter { deleted: true, ..Default::default() }) }).is_err());
    // rules on a manual album are rejected
    let manual = c.alloc_album_id();
    c.apply(Op::AddAlbum { album: Album::new(manual, "m") }).unwrap();
    assert!(c.apply(Op::SetAlbumRules { id: manual, rules: Box::new(Filter::default()) }).is_err());
    // a smart album may narrow a manual album
    c.apply(Op::SetAlbumPhotos { id: manual, photos: vec![b, z] }).unwrap();
    c.apply(Op::SetAlbumRules { id, rules: Box::new(Filter { album: Some(manual), lens: Some(String::new()), ..Default::default() }) }).unwrap();
    assert_eq!(c.album_photos(id), vec![b, z]);
    // persisted in snapshots
    let back = Catalog::from_snapshot(&c.to_snapshot()).unwrap();
    assert_eq!(back.album_photos(id), vec![b, z]);
    assert!(Filter { rating: 3, keyword: Some("sea".into()), ..Default::default() }.describe().contains("rating ≥ 3, keyword sea"));
}

#[test]
fn capture_times_display_like_the_info_panel() {
    use crate::dates::display_time;
    assert_eq!(display_time("2022-03-30T10:11:11"), "March 30, 2022 at 10:11:11 AM");
    assert_eq!(display_time("2022-03-30T22:05:01.5-04:00"), "March 30, 2022 at 10:05:01 PM");
    assert_eq!(display_time("2022-01-02T00:00:00"), "January 2, 2022 at 12:00:00 AM");
    assert_eq!(display_time("2022-07-04"), "July 4, 2022");
    assert_eq!(display_time("someday"), "someday");
}

#[test]
fn folder_paths() {
    use crate::query::in_folder;
    assert!(in_folder("/a/b/c.jpg", "/a/b", false));
    assert!(in_folder("/a/b/c.jpg", "/a/b/", false));
    assert!(!in_folder("/a/b/x/c.jpg", "/a/b", false));
    assert!(in_folder("/a/b/x/c.jpg", "/a/b", true));
    assert!(!in_folder("/a/bc/d.jpg", "/a/b", true), "a sibling with the same prefix");
    assert!(in_folder("C:\\Pics\\a.jpg", "C:\\Pics", false));
}

#[test]
fn merge_results_are_recognised() {
    use crate::query::merged_kind;
    assert_eq!(merged_kind("IMG_1-HDR.dng"), Some("hdr"));
    assert_eq!(merged_kind("IMG_1-Pano.dng"), Some("panorama"));
    assert_eq!(merged_kind("IMG_1-HDR-Pano.dng"), Some("hdrPanorama"));
    assert_eq!(merged_kind("IMG_1-HDR-2.dng"), Some("hdr"), "a second merge of the same photo");
    assert_eq!(merged_kind("hdr.jpg"), None);
    assert_eq!(merged_kind("panorama.jpg"), None);
}

/// Preview-only raws (an undecodable raw variant shown from its embedded JPEG): the reason is
/// stored with the photo, old catalogs without it still load, and Reload's op sets / clears it
/// with an exact inverse that survives serialisation (the journal).
#[test]
fn preview_only_survives_serde_and_set_content_undo() {
    let mut c = Catalog::new();
    let a = c.alloc_photo_id();
    let mut p = Photo::new(a, Source::File { path: "/x/DSC_0001.NEF".into() }, "DSC_0001.NEF", "NEF", 6000, 4000, "2026-10-01T00:00:00");
    p.kind = MediaKind::Raw;
    p.preview_only = Some("Nikon Huffman-compressed NEF".into());
    assert!(!p.develops_raw());
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    // the catalog round-trips it
    let back: Catalog = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(back.photo(a).unwrap().preview_only.as_deref(), Some("Nikon Huffman-compressed NEF"));
    // an older catalog (no field) loads as decodable
    let mut v = serde_json::to_value(&c).unwrap();
    let photos = v["photos"].as_object_mut().unwrap();
    let ph = photos.values_mut().next().unwrap();
    ph.as_object_mut().unwrap().remove("preview_only").expect("serialised when set");
    let old: Catalog = serde_json::from_value(v).unwrap();
    assert_eq!(old.photo(a).unwrap().preview_only, None);
    assert!(old.photo(a).unwrap().develops_raw());
    // Reload: the file decodes now → cleared; undo (through the journal's JSON) restores it
    let op = Op::SetContent { id: a, width: 6016, height: 4000, file_size: 1, content_hash: None, preview_only: None };
    let inv = c.apply(op).unwrap();
    assert!(c.photo(a).unwrap().develops_raw());
    let inv: Op = serde_json::from_str(&serde_json::to_string(&inv).unwrap()).unwrap();
    c.apply(inv).unwrap();
    assert_eq!(c.photo(a).unwrap().preview_only.as_deref(), Some("Nikon Huffman-compressed NEF"));
    assert_eq!(c.photo(a).unwrap().width, 6000);
}

/// Equivalent spellings of one folder (separators, trailing or doubled separators, `.`/`..`,
/// drive-letter case, verbatim and UNC prefixes) share an identity; different folders don't.
#[test]
fn folder_identity_ignores_spelling() {
    use crate::query::{folder_key, folder_within};
    let same = |a: &str, b: &str| assert_eq!(folder_key(a), folder_key(b), "{a} vs {b}");
    let differ = |a: &str, b: &str| assert_ne!(folder_key(a), folder_key(b), "{a} vs {b}");
    for (a, b) in [
        ("D:/Example/Photos", "D:\\Example\\Photos"),
        ("D:/Example/Photos", "d:\\Example\\Photos\\"),
        ("D:\\Example/Photos", "D:/Example//Photos/."),
        ("D:/Example/Photos", "D:/Example/Other/../Photos"),
        ("D:/Example/Photos", "\\\\?\\D:\\Example\\Photos"),
        ("\\\\server\\share\\Photos", "//server/share/Photos/"),
        ("\\\\?\\UNC\\server\\share\\Photos", "//server/share/Photos"),
        ("/home/me/Photos", "/home/me/Photos/"),
        ("/home/me/Photos", "/home//me/./Photos"),
        ("/", "//"),
        ("C:\\", "c:/"),
    ] {
        same(a, b);
    }
    differ("D:/Example/Photos", "D:/Example/Photos2");
    differ("D:/Example/Photos", "E:/Example/Photos");
    differ("/home/me/Photos", "/home/me");
    assert_eq!(folder_key("/a/../../b"), "/b", "`..` stops at the root");
    assert!(folder_within("D:\\Example\\Photos\\2026", "D:/Example/Photos/"));
    assert!(folder_within("D:/Example/Photos", "d:\\Example\\Photos"));
    assert!(!folder_within("D:/Example/Photos2", "D:/Example/Photos"));
    assert!(folder_within("/a/b", "/"));
    assert!(folder_within("C:\\x", "c:\\"));
}
