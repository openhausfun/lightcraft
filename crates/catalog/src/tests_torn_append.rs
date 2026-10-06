//! Issue #101: an append that fails part-way (disk full, a dropped network share) and is then
//! retried must not read back as log damage, and must lose nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::journal::LOG;
use crate::*;

/// A store whose next append writes only a prefix of its data, then fails.
#[derive(Clone, Default)]
struct Torn {
    files: MemStore,
    /// The next append writes this many bytes, then returns an error.
    torn_at: Arc<Mutex<Option<usize>>>,
    /// `truncate` fails (the cut-back after a failed append can't be done right away).
    truncate_fails: Arc<AtomicBool>,
}

impl Torn {
    fn tear_next_append(&self, bytes: usize) {
        *self.torn_at.lock().unwrap() = Some(bytes);
    }
}

impl Store for Torn {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if let Some(n) = self.torn_at.lock().unwrap().take() {
            self.files.append(name, &data[..n.min(data.len())])?;
            return Err(std::io::Error::other("disk full"));
        }
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        if self.truncate_fails.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("share unavailable"));
        }
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "torn".into()
    }
}

fn add_photo(c: &mut Catalog) -> Op {
    let id = c.alloc_photo_id();
    Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 4, 3, "2026-01-01")) }
}

/// Apply `ops` to the live catalog (like a command) and return them for the journal.
fn apply(c: &mut Catalog, ops: Vec<Op>) -> Vec<Op> {
    for op in &ops {
        c.apply(op.clone()).unwrap();
    }
    ops
}

/// Every tear position of a batch, then the retry (same ops, same seqs, like the engine's queue)
/// and more appends: reloading replays everything, reports no damage and no torn bytes.
#[test]
fn partial_append_then_retry_loads_cleanly() {
    let probe = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(probe.clone())).unwrap();
    let add = add_photo(&mut c);
    let first = apply(&mut c, vec![add]);
    j.append(&first).unwrap();
    let id = c.photos().next().unwrap().id;
    let batch = apply(&mut c, vec![Op::SetRating { id, rating: 3 }, Op::SetFlag { id, flag: Flag::Pick }, Op::SetRating { id, rating: 4 }]);
    let before = probe.get(LOG).unwrap().len();
    j.append(&batch).unwrap();
    let batch_len = probe.get(LOG).unwrap().len() - before;

    for tear in 1..batch_len {
        let t = Torn::default();
        let (mut j, mut c, _) = Journal::open(Box::new(t.clone())).unwrap();
        let add = add_photo(&mut c);
        let first = apply(&mut c, vec![add]);
        j.append(&first).unwrap();
        let good_len = t.files.get(LOG).unwrap().len();
        let batch = apply(&mut c, vec![Op::SetRating { id, rating: 3 }, Op::SetFlag { id, flag: Flag::Pick }, Op::SetRating { id, rating: 4 }]);
        t.tear_next_append(tear);
        assert!(j.append(&batch).is_err(), "tear at {tear}");
        // cut back to the last whole record
        assert_eq!(t.files.get(LOG).unwrap().len(), good_len, "tear at {tear}");
        j.append(&batch).unwrap();
        let more = apply(&mut c, vec![Op::SetLabel { id, label: Some(ColorLabel::ALL[0]) }]);
        j.append(&more).unwrap();
        drop(j);

        let (_, loaded, r) = Journal::open(Box::new(t.clone())).unwrap();
        assert!(r.damaged.is_none(), "tear at {tear}: {r:?}");
        assert_eq!((r.torn_bytes, r.torn_fragments, r.replayed), (0, 0, 5), "tear at {tear}");
        assert_eq!(loaded.to_snapshot(), c.to_snapshot(), "tear at {tear}");
    }
}

/// When cutting back fails too (the share is gone), the next append cuts back first; until that
/// works nothing more is appended after the fragment.
#[test]
fn failed_cut_back_is_done_before_the_next_append() {
    let t = Torn::default();
    let (mut j, mut c, _) = Journal::open(Box::new(t.clone())).unwrap();
    let add = add_photo(&mut c);
    let first = apply(&mut c, vec![add]);
    j.append(&first).unwrap();
    let good_len = t.files.get(LOG).unwrap().len();
    let id = c.photos().next().unwrap().id;
    let batch = apply(&mut c, vec![Op::SetRating { id, rating: 2 }, Op::SetRating { id, rating: 5 }]);
    t.truncate_fails.store(true, Ordering::SeqCst);
    t.tear_next_append(30);
    assert!(j.append(&batch).is_err());
    assert_eq!(t.files.get(LOG).unwrap().len(), good_len + 30, "the fragment is still there");
    // still unreachable: the retry fails without writing anything after the fragment
    assert!(j.append(&batch).is_err());
    assert_eq!(t.files.get(LOG).unwrap().len(), good_len + 30);
    t.truncate_fails.store(false, Ordering::SeqCst);
    j.append(&batch).unwrap();
    drop(j);
    let (_, loaded, r) = Journal::open(Box::new(t.clone())).unwrap();
    assert!(r.damaged.is_none(), "{r:?}");
    assert_eq!(r.replayed, 3);
    assert_eq!(loaded.to_snapshot(), c.to_snapshot());
}

/// Logs written before the fix: the fragment of the failed append is glued to the first retried
/// record on one line, followed by the rest of the retry. They load in full (the fragment is
/// skipped), instead of stopping at the fragment and setting the later records aside as damage.
#[test]
fn legacy_fragment_glued_to_the_retry_is_skipped() {
    let m = MemStore::new();
    let (mut j, mut c, _) = Journal::open(Box::new(m.clone())).unwrap();
    let add = add_photo(&mut c);
    let first = apply(&mut c, vec![add]);
    j.append(&first).unwrap();
    let good = m.get(LOG).unwrap();
    let id = c.photos().next().unwrap().id;
    let batch = apply(
        &mut c,
        vec![
            Op::SetRating { id, rating: 3 },
            Op::SetFlag { id, flag: Flag::Pick },
            Op::SetLabelName { label: ColorLabel::ALL[1], name: Some("Größe ✓".into()) },
        ],
    );
    j.append(&batch).unwrap();
    let full = m.get(LOG).unwrap();
    let batch_bytes = &full[good.len()..];
    let later = apply(&mut c, vec![Op::SetRating { id, rating: 1 }]);
    j.append(&later).unwrap();
    let later_bytes = m.get(LOG).unwrap()[full.len()..].to_vec();
    drop(j);

    // every tear (inside the first record, after it, inside a multi-byte character…)
    for tear in 1..batch_bytes.len() {
        if batch_bytes[tear - 1] == b'\n' {
            continue; // a tear on a record boundary leaves no fragment
        }
        let mut log = good.clone();
        log.extend_from_slice(&batch_bytes[..tear]);
        log.extend_from_slice(batch_bytes);
        log.extend_from_slice(&later_bytes);
        let t = MemStore::new();
        t.set(LOG, log);
        let (_, loaded, r) = Journal::open(Box::new(t)).unwrap();
        assert!(r.damaged.is_none(), "tear at {tear}: {r:?}");
        assert_eq!(r.torn_fragments, 1, "tear at {tear}");
        assert_eq!(loaded.to_snapshot(), c.to_snapshot(), "tear at {tear}");
    }

    // a damaged record whose tail happens to be a whole record from the future is still damage
    let mut log = good.clone();
    log.extend_from_slice(b"garbage");
    log.extend_from_slice(&later_bytes);
    log.extend_from_slice(&later_bytes);
    let t = MemStore::new();
    t.set(LOG, log);
    let (_, _, r) = Journal::open(Box::new(t)).unwrap();
    assert!(r.damaged.is_some(), "{r:?}");
}

/// A snapshot can absorb ops whose append failed: it records them as saved (`seq` moves past
/// them), and appending after it continues from there.
#[test]
fn snapshot_with_unlogged_ops_saves_them() {
    let t = Torn::default();
    let (mut j, mut c, _) = Journal::open(Box::new(t.clone())).unwrap();
    let add = add_photo(&mut c);
    let first = apply(&mut c, vec![add]);
    j.append(&first).unwrap();
    let id = c.photos().next().unwrap().id;
    let queued = apply(&mut c, vec![Op::SetRating { id, rating: 2 }, Op::SetFlag { id, flag: Flag::Reject }]);
    t.tear_next_append(10);
    assert!(j.append(&queued).is_err());
    j.snapshot_with_unlogged(&c, queued.len() as u64).unwrap();
    assert_eq!(j.seq(), 3);
    let after = apply(&mut c, vec![Op::SetRating { id, rating: 5 }]);
    j.append(&after).unwrap();
    drop(j);
    let (j, loaded, r) = Journal::open(Box::new(t.clone())).unwrap();
    assert_eq!((r.snapshot_seq, r.replayed), (3, 1));
    assert_eq!(j.seq(), 4);
    assert_eq!(loaded.to_snapshot(), c.to_snapshot());
}

/// `FsStore::truncate` on a file that was never created (an append that failed first) is a no-op.
#[test]
fn fs_store_truncates_a_missing_file_quietly() {
    let dir = std::env::temp_dir().join(format!("lc-torn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = FsStore::open(&dir).unwrap();
    s.truncate(LOG, 0).unwrap();
    assert!(!dir.join(LOG).exists());
    s.append(LOG, b"abc\n").unwrap();
    s.truncate(LOG, 0).unwrap();
    assert_eq!(std::fs::read(dir.join(LOG)).unwrap(), b"");
    let _ = std::fs::remove_dir_all(&dir);
}
