//! Journal (op log + snapshot) tests: replay(log) == state, torn tails, stale logs, damage.

use std::sync::Arc;

use lightcraft_develop::DevelopSettings;
use proptest::prelude::*;

use crate::journal::{LOG, SNAPSHOT, decode_record, encode_record};
use crate::*;

fn open(m: &MemStore) -> (Journal, Catalog, LoadReport) {
    Journal::open(Box::new(m.clone())).unwrap()
}

/// Interpret a random action against the live catalog as an op (may be invalid: then the live
/// apply fails and, like in the engine, nothing is logged).
fn op_for(c: &mut Catalog, kind: u8, a: u8, b: u8) -> Op {
    let photos: Vec<PhotoId> = c.photos().map(|p| p.id).collect();
    let albums: Vec<AlbumId> = c.albums().map(|a| a.id).collect();
    let pid = photos.get(a as usize % photos.len().max(1)).copied().unwrap_or(PhotoId(99));
    let aid = albums.get(a as usize % albums.len().max(1)).copied().unwrap_or(AlbumId(99));
    let rules = |b: u8| Filter {
        rating: b % 6,
        flag: [None, Some(Flag::Pick), Some(Flag::Reject)][b as usize % 3],
        date_from: (b % 4 == 1).then(|| "2026-01".to_string()),
        text: if b.is_multiple_of(5) { format!("{}", b % 10) } else { String::new() },
        ..Default::default()
    };
    match kind % 16 {
        0 | 1 => {
            let id = c.alloc_photo_id();
            let mut p = Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, &format!("{}.jpg", id.0), "JPEG", 60, 40, "2026-01-01");
            p.content_hash = Some(format!("{:032x}", id.0 * 7919));
            Op::AddPhoto { photo: Box::new(p) }
        }
        2 => Op::SetRating { id: pid, rating: b % 7 },
        3 => Op::SetFlag { id: pid, flag: [Flag::None, Flag::Pick, Flag::Reject][b as usize % 3] },
        4 => {
            let mut d = DevelopSettings::default();
            d.light.exposure = b as f64 / 37.0 - 2.0;
            d.light.contrast = -(a as f64) * 0.37;
            let settings = Arc::new(d);
            Op::Batch {
                ops: vec![
                    Op::SetDevelop { id: pid, settings: settings.clone(), label: "Exposure".into(), edited: Some(format!("t{b}")) },
                    Op::PushHistory { id: pid, step: HistoryStep { label: "Exposure".into(), settings } },
                ],
            }
        }
        5 => {
            let id = c.alloc_album_id();
            Op::AddAlbum {
                album: Album {
                    id,
                    name: format!("A{b}"),
                    parent: None,
                    folder: b.is_multiple_of(4),
                    photos: vec![],
                    cover: None,
                    smart: None,
                    quick: false,
                },
            }
        }
        6 => Op::SetAlbumPhotos { id: aid, photos: photos.iter().copied().filter(|p| (p.0 + b as u64).is_multiple_of(3)).collect() },
        7 => Op::RenameAlbum { id: aid, name: format!("Renamed {b} ✓") },
        8 => Op::SetDeleted { id: pid, deleted: b.is_multiple_of(2) },
        10 => {
            let id = c.alloc_album_id();
            Op::AddAlbum { album: Album { smart: Some(Box::new(rules(b))), ..Album::new(id, format!("Smart {b}")) } }
        }
        11 => Op::SetAlbumRules { id: aid, rules: Box::new(rules(b)) },
        12 => {
            let members: Vec<PhotoId> = photos.iter().copied().filter(|p| (p.0 + b as u64).is_multiple_of(4)).collect();
            c.group_ops(pid, &members, b.is_multiple_of(2)).unwrap_or(Op::Batch { ops: vec![] })
        }
        13 => Op::Batch { ops: c.remove_from_stacks_ops(&[pid]) },
        14 => c.set_top_ops(pid).unwrap_or(Op::Batch { ops: vec![] }),
        9 if b.is_multiple_of(5) => Op::SetLabelName { label: ColorLabel::ALL[a as usize % 5], name: (a % 3 != 1).then(|| format!("Label {b}")) },
        9 if b.is_multiple_of(2) => {
            Op::SetFile { id: pid, file_name: format!("Renamed_{b}.jpg"), source: Source::File { path: format!("/p/Renamed_{b}.jpg") } }
        }
        9 => Op::SetCaptured { id: pid, captured: (!b.is_multiple_of(3)).then(|| format!("2026-0{}-1{}T10:{:02}:00", 1 + b % 9, b % 10, a % 60)) },
        15 => match c.photo(pid).cloned() {
            // a virtual copy: same source, own id, copy_of/copy_name set
            Some(src) => {
                let id = c.alloc_photo_id();
                let mut v = (*src).clone();
                v.id = id;
                v.copy_of = Some(pid);
                v.copy_name = Some(format!("Copy {b}"));
                Op::AddPhoto { photo: Box::new(v) }
            }
            None => Op::Batch { ops: vec![] },
        },
        _ => c.delete_permanently_ops(pid),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// replay(snapshot + log) == live state, with snapshots at random points and reopening
    /// (resuming appends) in between.
    #[test]
    fn replay_equals_state(actions in proptest::collection::vec((0u8..20, 0u8..255, 0u8..255, 0u8..12), 1..80)) {
        let m = MemStore::new();
        let (mut j, mut live, r) = open(&m);
        prop_assert!(r.created);
        for (kind, a, b, ctl) in actions {
            let op = op_for(&mut live, kind, a, b);
            if live.apply(op.clone()).is_ok() {
                j.append(&[op]).unwrap();
            }
            match ctl {
                0 => j.snapshot(&live).unwrap(),
                1 => {
                    let (j2, c2, _) = open(&m);
                    prop_assert_eq!(c2.to_snapshot(), live.to_snapshot());
                    j = j2;
                }
                _ => {}
            }
        }
        let (_, loaded, r) = open(&m);
        prop_assert_eq!(r.failed, 0);
        prop_assert_eq!(loaded.to_snapshot(), live.to_snapshot());
        // allocation continues where it left off
        prop_assert_eq!(loaded.clone().alloc_photo_id(), live.clone().alloc_photo_id());
    }
}

fn sample_catalog_with_log() -> (MemStore, Catalog, Catalog) {
    let m = MemStore::new();
    let (mut j, mut c, _) = open(&m);
    for k in 0..6u8 {
        let op = op_for(&mut c, k, k, k * 3);
        if c.apply(op.clone()).is_ok() {
            j.append(&[op]).unwrap();
        }
    }
    let before_last = c.clone();
    let op = Op::SetRating { id: c.photos().next().unwrap().id, rating: 5 };
    c.apply(op.clone()).unwrap();
    j.append(&[op]).unwrap();
    (m, before_last, c)
}

/// A crash mid-append: every possible truncation of the last record still loads (to the state
/// before that op), the torn bytes are cut off, and appending afterwards works.
#[test]
fn torn_last_record_is_tolerated() {
    let (m, before_last, full) = sample_catalog_with_log();
    let log = m.get(LOG).unwrap();
    let last_start = log[..log.len() - 1].iter().rposition(|b| *b == b'\n').unwrap() + 1;
    for cut in last_start..log.len() {
        let t = MemStore::new();
        t.set(LOG, log[..cut].to_vec());
        let (mut j, mut c, r) = open(&t);
        // the full record minus only its newline is complete and kept
        let expect = if cut == log.len() - 1 { &full } else { &before_last };
        assert_eq!(c.to_snapshot(), expect.to_snapshot(), "cut at {cut}");
        assert!(r.damaged.is_none());
        if cut < log.len() - 1 {
            assert_eq!(r.torn_bytes as usize, cut - last_start, "cut at {cut}");
        }
        // keeps working after recovery
        let id = c.photos().next().unwrap().id;
        let op = Op::SetFlag { id, flag: Flag::Reject };
        c.apply(op.clone()).unwrap();
        j.append(&[op]).unwrap();
        let (_, c2, r2) = open(&t);
        assert_eq!(c2.to_snapshot(), c.to_snapshot(), "cut at {cut}");
        assert_eq!(r2.torn_bytes, 0);
    }
    // garbage bytes (a partially flushed page) at the end are a torn tail too
    let t = MemStore::new();
    let mut g = log.clone();
    g.extend_from_slice(&[0, 0, 0, b'{', 0xff]);
    t.set(LOG, g);
    let (_, c, r) = open(&t);
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    assert_eq!(r.torn_bytes, 5);
}

/// Crash after the snapshot was written but before the log was reset: the stale records are
/// skipped by sequence number.
#[test]
fn stale_log_after_snapshot_is_skipped() {
    let (m, _, full) = sample_catalog_with_log();
    let old_log = m.get(LOG).unwrap();
    let (mut j, c, _) = open(&m);
    j.snapshot(&c).unwrap();
    assert!(m.get(LOG).unwrap().is_empty());
    m.set(LOG, old_log);
    let (_, c2, r) = open(&m);
    assert_eq!(c2.to_snapshot(), full.to_snapshot());
    assert!(r.stale > 0 && r.replayed == 0);
}

/// Damage in the middle of the log (not a torn tail): recover what precedes it, keep the
/// damaged file, and write a consistent snapshot.
#[test]
fn mid_log_damage_is_preserved_and_recovered() {
    let (m, _, _) = sample_catalog_with_log();
    let log = m.get(LOG).unwrap();
    let text = String::from_utf8(log).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    assert!(lines.len() > 4);
    lines[2] = lines[2].replacen("\"op\":{\"op\":\"", "\"op\":{\"op\":\"x", 1);
    let bad = format!("{}\n", lines.join("\n")).into_bytes();
    m.set(LOG, bad.clone());
    let (_, c, r) = open(&m);
    let name = r.damaged.clone().expect("damage reported");
    assert_eq!(m.get(&name).unwrap(), bad);
    assert_eq!(r.replayed, 2);
    // the recovered state was snapshotted: reopening is clean and identical
    let (_, c2, r2) = open(&m);
    assert_eq!(c2.to_snapshot(), c.to_snapshot());
    assert!(r2.damaged.is_none());
}

#[test]
fn record_codec() {
    let op = Op::RenameAlbum { id: AlbumId(3), name: "Line\nbreak \"quoted\" }".into() };
    let line = encode_record(42, &op);
    assert!(!line.contains('\n'));
    assert_eq!(decode_record(&line), Some((42, op)));
    assert_eq!(decode_record(&line.replace("Line", "Lime")), None, "crc mismatch");
    assert_eq!(decode_record(&line[..line.len() - 1]), None);
    // log lines stay plain JSON
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["op"]["op"], "renameAlbum");
}

#[test]
fn push_history_is_bounded_and_invertible() {
    let mut c = Catalog::new();
    let id = c.alloc_photo_id();
    c.apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::Demo { scene: 1 }, "a", "JPEG", 1, 1, "t")) }).unwrap();
    let step = |i: usize| HistoryStep { label: format!("s{i}"), settings: Arc::new(DevelopSettings::default()) };
    for i in 0..HISTORY_LIMIT + 5 {
        c.apply(Op::PushHistory { id, step: step(i) }).unwrap();
    }
    let h = &c.photo(id).unwrap().history;
    assert_eq!(h.len(), HISTORY_LIMIT);
    assert_eq!(h[0].label, "s5");
    let before = c.to_snapshot();
    let inv = c.apply(Op::PushHistory { id, step: step(999) }).unwrap();
    c.apply(inv).unwrap();
    assert_eq!(c.to_snapshot(), before);
}

#[test]
fn fs_store_roundtrip() {
    let dir = std::env::temp_dir().join(format!("lc-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut j, mut c, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert!(r.created);
    let id = c.alloc_photo_id();
    let add = Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: "/a.jpg".into() }, "a.jpg", "JPEG", 1, 1, "t")) };
    c.apply(add.clone()).unwrap();
    c.apply(Op::SetRating { id, rating: 4 }).unwrap();
    j.append(&[add, Op::SetRating { id, rating: 4 }]).unwrap();
    drop(j);
    let (mut j, c2, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert_eq!((r.replayed, r.created), (2, false));
    assert_eq!(c2.to_snapshot(), c.to_snapshot());
    j.snapshot(&c2).unwrap();
    assert_eq!(std::fs::metadata(dir.join(LOG)).unwrap().len(), 0);
    assert!(!dir.join(format!("{SNAPSHOT}.tmp")).exists());
    let (_, c3, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert_eq!((r.snapshot_seq, r.replayed), (2, 0));
    assert_eq!(c3.to_snapshot(), c.to_snapshot());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The streamed snapshot is byte-for-byte the format written before streaming (issue #37), so
/// existing libraries load and older builds can read new snapshots; FsStore and MemStore agree.
#[test]
fn streamed_snapshot_keeps_the_on_disk_format() {
    let (m, _, full) = sample_catalog_with_log();
    let (mut j, c, _) = open(&m);
    assert_eq!(c.to_snapshot(), full.to_snapshot());
    j.snapshot(&c).unwrap();
    let legacy = format!(
        "{{\"format\":\"lightcraft-catalog\",\"version\":{},\"seq\":{},\"catalog\":{}}}\n",
        crate::journal::VERSION,
        j.seq(),
        c.to_snapshot()
    );
    assert_eq!(String::from_utf8(m.get(SNAPSHOT).unwrap()).unwrap(), legacy);
    assert_eq!(j.stats().last_snapshot.bytes, legacy.len() as u64);

    let dir = std::env::temp_dir().join(format!("lc-journal-stream-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // an old-style snapshot (written by a build before streaming) loads, and is rewritten the same
    std::fs::write(dir.join(SNAPSHOT), legacy.as_bytes()).unwrap();
    let (mut fj, fc, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert_eq!((fc.to_snapshot(), r.snapshot_seq), (c.to_snapshot(), j.seq()));
    fj.snapshot(&fc).unwrap();
    assert_eq!(std::fs::read(dir.join(SNAPSHOT)).unwrap(), legacy.as_bytes());
    drop(fj);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A snapshot that fails while streaming (serialisation or I/O error) leaves the old file in
/// place and removes its temp file.
#[test]
fn failed_streamed_write_keeps_the_old_file() {
    let dir = std::env::temp_dir().join(format!("lc-journal-streamfail-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = FsStore::open(&dir).unwrap();
    s.write_atomic(SNAPSHOT, b"old").unwrap();
    let r = s.write_atomic_with(SNAPSHOT, &mut |w| {
        w.write_all(b"partial new content")?;
        Err(std::io::Error::other("serialisation failed"))
    });
    assert!(r.is_err());
    assert_eq!(std::fs::read(dir.join(SNAPSHOT)).unwrap(), b"old");
    assert!(!dir.join(format!("{SNAPSHOT}.tmp")).exists());
    assert_eq!(s.write_atomic_with(SNAPSHOT, &mut |w| w.write_all(b"new")).unwrap(), 3);
    assert_eq!(std::fs::read(dir.join(SNAPSHOT)).unwrap(), b"new");
    let _ = std::fs::remove_dir_all(&dir);
}
