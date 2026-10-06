//! Forgetting untouched Local browse records (see [`crate::local`]).

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;

use crate::*;

const OLD: &str = "2026-08-01T10:00:00";
const NOW: &str = "2026-10-05T10:00:00";

/// A Local record of `/pics/<folder>/IMG_<i>.JPG` as a browse catalogues it (with its baseline).
fn local(c: &mut Catalog, folder: &str, i: u32) -> PhotoId {
    let id = c.alloc_photo_id();
    let mut p = Photo::new(id, Source::File { path: format!("/pics/{folder}/IMG_{i}.JPG") }, &format!("IMG_{i}.JPG"), "JPEG", 60, 40, OLD);
    p.meta.camera = "Cam".into();
    p.meta.creator = "From EXIF".into(); // file metadata is part of the baseline, not a user change
    p.focal_and_wb_for_test();
    p.local = true;
    p.set_local_baseline();
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    id
}

impl Photo {
    fn focal_and_wb_for_test(&mut self) {
        self.meta.focal_mm = Some(35.3);
        self.as_shot_wb = Some((5123.4, 3.1));
    }
}

fn browsed(c: &mut Catalog, folder: &str, at: &str) {
    c.apply(Op::SetBrowsed { folder: format!("/pics/{folder}"), at: Some(at.into()) }).unwrap();
}

fn plan(c: &Catalog, days: u32) -> ForgetPlan {
    c.forget_local_plan(NOW, days, &|_| false)
}

fn run(c: &mut Catalog, days: u32) -> ForgetPlan {
    let p = plan(c, days);
    for op in p.ops() {
        c.apply(op).unwrap();
    }
    p
}

#[test]
fn untouched_records_of_old_folders_are_forgotten() {
    let mut c = Catalog::new();
    let old: Vec<_> = (0..5).map(|i| local(&mut c, "old", i)).collect();
    let recent: Vec<_> = (0..3).map(|i| local(&mut c, "recent", i)).collect();
    browsed(&mut c, "old", OLD);
    browsed(&mut c, "recent", "2026-10-01T09:00:00");
    // a library photo in the old folder is never a candidate
    let lib = c.alloc_photo_id();
    let mut p = Photo::new(lib, Source::File { path: "/pics/old/LIB.JPG".into() }, "LIB.JPG", "JPEG", 6, 4, OLD);
    p.set_local_baseline();
    c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();

    let dry = plan(&c, 30);
    assert_eq!((dry.local, dry.evict.len(), dry.kept_recent), (8, 5, 3), "{dry:?}");
    assert_eq!(dry.evict, old);
    assert_eq!(c.len(), 9, "a plan changes nothing");
    let done = run(&mut c, 30);
    assert_eq!(done.evict.len(), 5);
    assert!(old.iter().all(|id| c.photo(*id).is_none()));
    assert!(recent.iter().all(|id| c.photo(*id).is_some()));
    assert!(c.photo(lib).is_some());
    // the folder has no Local records left: its time goes
    assert_eq!(done.unstamp, vec!["/pics/old".to_string()]);
    assert_eq!(c.last_browsed("/pics/old"), None);
    assert_eq!(c.last_browsed("/pics/recent/"), Some("2026-10-01T09:00:00"));
    // 0 days = never
    browsed(&mut c, "recent", OLD);
    assert!(plan(&c, 0).evict.is_empty());
    assert_eq!(plan(&c, 30).evict.len(), 3);
}

#[test]
fn any_user_change_keeps_a_record() {
    type Change = Box<dyn Fn(&mut Catalog, PhotoId)>;
    let set = |f: fn(&mut Photo)| -> Change {
        Box::new(move |c: &mut Catalog, id: PhotoId| {
            let mut p = (**c.photo(id).unwrap()).clone();
            f(&mut p);
            c.apply(Op::RemovePhoto { id }).unwrap();
            c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        })
    };
    let op = |f: fn(PhotoId) -> Op| -> Change { Box::new(move |c: &mut Catalog, id: PhotoId| drop(c.apply(f(id)).unwrap())) };
    let edited = || {
        let mut d = DevelopSettings::default();
        d.light.exposure = 0.3;
        Arc::new(d)
    };
    let changes: Vec<(&str, Change)> = vec![
        ("rating", op(|id| Op::SetRating { id, rating: 3 })),
        ("flag", op(|id| Op::SetFlag { id, flag: Flag::Reject })),
        ("label", op(|id| Op::SetLabel { id, label: Some(ColorLabel::Red) })),
        ("develop", Box::new(move |c, id| drop(c.apply(Op::SetDevelop { id, settings: edited(), label: "Exposure".into(), edited: None }).unwrap()))),
        ("crop", set(|p| Arc::make_mut(&mut p.develop).crop.flip_h = true)),
        (
            "history only",
            Box::new(|c, id| {
                let step = HistoryStep { label: "Reset".into(), settings: Arc::new(DevelopSettings::default()) };
                drop(c.apply(Op::PushHistory { id, step }).unwrap())
            }),
        ),
        ("versions", set(|p| p.versions.push(Version { name: "v".into(), created: NOW.into(), settings: p.develop.clone(), auto: false }))),
        ("title", set(|p| p.meta.title = "T".into())),
        ("caption", set(|p| p.meta.caption = "C".into())),
        ("keywords", set(|p| p.meta.keywords.push("k".into()))),
        ("location", set(|p| p.meta.city = "Oslo".into())),
        ("gps", set(|p| p.meta.gps = Some((1.0, 2.0)))),
        ("copyright", set(|p| p.meta.copyright = "© me".into())),
        ("creator edit", set(|p| p.meta.creator = "Me".into())),
        ("capture time", op(|id| Op::SetCaptured { id, captured: Some("2020-01-01T00:00:00".into()) })),
        ("rename", set(|p| p.file_name = "renamed.JPG".into())),
        ("culling", op(|id| Op::SetAnalysis { id, analysis: Some(Analysis::default()) })),
        ("deleted", op(|id| Op::SetDeleted { id, deleted: true })),
        ("pick", op(|id| Op::SetFlag { id, flag: Flag::Pick })),
        (
            "album",
            Box::new(|c, id| {
                let a = c.alloc_album_id();
                let mut al = Album::new(a, "A");
                al.photos.push(id);
                drop(c.apply(Op::AddAlbum { album: al }).unwrap())
            }),
        ),
        (
            "stack",
            Box::new(|c, id| {
                let other = local(c, "elsewhere", 99);
                let s = c.alloc_stack_id();
                drop(c.apply(Op::AddStack { stack: Stack { id: s, photos: vec![id, other], collapsed: false } }).unwrap())
            }),
        ),
        (
            "virtual copy",
            Box::new(|c, id| {
                let mut v = (**c.photo(id).unwrap()).clone();
                v.id = c.alloc_photo_id();
                v.copy_of = Some(id);
                v.copy_name = Some("Copy 1".into());
                v.local = false;
                drop(c.apply(Op::AddPhoto { photo: Box::new(v) }).unwrap())
            }),
        ),
    ];
    for (what, change) in changes {
        let mut c = Catalog::new();
        let id = local(&mut c, "old", 1);
        let control = local(&mut c, "old", 2);
        browsed(&mut c, "old", OLD);
        change(&mut c, id);
        let p = plan(&c, 30);
        assert_eq!(p.evict, vec![control], "{what}: {p:?}");
        assert_eq!(p.kept_touched + p.kept_in_use, 1, "{what}: {p:?}");
        // the stamp stays: a record in the folder remains
        assert!(p.unstamp.is_empty(), "{what}");
    }
    // a change undone (back to the browse state) is untouched again
    let mut c = Catalog::new();
    let id = local(&mut c, "old", 1);
    browsed(&mut c, "old", OLD);
    let inv = c.apply(Op::SetRating { id, rating: 2 }).unwrap();
    assert!(plan(&c, 30).evict.is_empty());
    c.apply(inv).unwrap();
    assert_eq!(plan(&c, 30).evict, vec![id]);
}

/// Catalogs from before last-browsed times (and baselines): nothing is forgotten on the first
/// run — the folders are stamped now — and baseline-less records are judged by their data.
#[test]
fn upgrade_without_times_forgets_nothing_then_counts_from_then() {
    let mut c = Catalog::new();
    let a = local(&mut c, "a", 1);
    // legacy records: no baseline
    let legacy = |c: &mut Catalog, i: u32, f: fn(&mut Photo)| {
        let id = c.alloc_photo_id();
        let mut p = Photo::new(id, Source::File { path: format!("/pics/a/L{i}.JPG") }, "L.JPG", "JPEG", 6, 4, OLD);
        p.meta.camera = "Cam".into();
        p.local = true;
        f(&mut p);
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        id
    };
    let clean = legacy(&mut c, 1, |_| {});
    let rated = legacy(&mut c, 2, |p| p.rating = 1);
    let with_creator = legacy(&mut c, 3, |p| p.meta.creator = "maybe the user".into());
    let p = run(&mut c, 30);
    assert!(p.evict.is_empty(), "{p:?}");
    assert_eq!(p.stamp, vec!["/pics/a".to_string()]);
    assert_eq!(c.last_browsed("/pics/a"), Some(NOW));
    // 30+ days later
    let later = c.forget_local_plan("2026-11-06T10:00:00", 30, &|_| false);
    let mut evict = later.evict.clone();
    evict.sort();
    assert_eq!(evict, vec![a, clean]);
    assert_eq!(later.kept_touched, 2, "{rated:?} {with_creator:?}");
}

#[test]
fn in_use_records_are_kept() {
    let mut c = Catalog::new();
    let a = local(&mut c, "old", 1);
    let b = local(&mut c, "old", 2);
    browsed(&mut c, "old", OLD);
    let p = c.forget_local_plan(NOW, 30, &|p| p.id == a);
    assert_eq!((p.evict.clone(), p.kept_in_use), (vec![b], 1));
}

/// The ops are journaled: a crash right after forgetting reloads the same catalog, and a
/// fingerprint survives the snapshot round trip (a reloaded record is still untouched).
#[test]
fn forgetting_is_journaled_and_replays() {
    let m = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let mut ops = Vec::new();
    for i in 0..10 {
        local(&mut c, if i % 2 == 0 { "old" } else { "new" }, i);
    }
    // log what was built so far: the ops are the records as added
    for p in c.photos() {
        ops.push(Op::AddPhoto { photo: Box::new((**p).clone()) });
    }
    ops.push(Op::SetBrowsed { folder: "/pics/old".into(), at: Some(OLD.into()) });
    ops.push(Op::SetBrowsed { folder: "/pics/new".into(), at: Some(NOW.into()) });
    browsed(&mut c, "old", OLD);
    browsed(&mut c, "new", NOW);
    j.append(&ops).unwrap();
    j.snapshot(&c).unwrap();
    drop(j);
    // reload from the snapshot: baselines still match
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let p = c.forget_local_plan(NOW, 30, &|_| false);
    assert_eq!(p.evict.len(), 5, "{p:?}");
    let ops = p.ops();
    for op in &ops {
        c.apply(op.clone()).unwrap();
    }
    j.append(&ops).unwrap();
    let expect = c.to_snapshot();
    drop(j); // crash: replay the log
    let (_, c2, r) = Journal::open(Box::new(m)).unwrap();
    assert_eq!(r.failed, 0);
    assert_eq!(c2.to_snapshot(), expect);
    assert_eq!(c2.len(), 5);
    // nothing left to forget
    assert!(c2.forget_local_plan(NOW, 30, &|_| false).evict.is_empty());
}

#[test]
fn op_photo_ids_cover_nested_ops() {
    let op = Op::Batch {
        ops: vec![
            Op::SetRating { id: PhotoId(1), rating: 1 },
            Op::SetAlbumPhotos { id: AlbumId(1), photos: vec![PhotoId(2), PhotoId(3)] },
            Op::Batch { ops: vec![Op::RemovePhoto { id: PhotoId(4) }] },
        ],
    };
    let mut ids = Vec::new();
    op.photo_ids(&mut |id| ids.push(id.0));
    assert_eq!(ids, vec![1, 2, 3, 4]);
}
