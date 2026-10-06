//! Background compaction (issue #37): a snapshot written by a worker while appends go on.
//! "Crashes" are simulated by copying the files at a given moment and loading the copy: every
//! acknowledged (appended) op must be there, in order, whatever the snapshot was doing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use proptest::prelude::*;

use crate::journal::{LOG, SNAPSHOT};
use crate::*;

/// A MemStore whose snapshot writes wait for a gate (to hold a background snapshot mid-flight)
/// and can be made to fail.
#[derive(Clone)]
struct Gated {
    files: MemStore,
    gate: Arc<(Mutex<bool>, Condvar)>,
    fail: Arc<AtomicBool>,
}

impl Gated {
    fn new() -> Gated {
        Gated { files: MemStore::new(), gate: Arc::new((Mutex::new(false), Condvar::new())), fail: Arc::new(AtomicBool::new(false)) }
    }
    fn set_gate(&self, open: bool) {
        *self.gate.0.lock().unwrap() = open;
        self.gate.1.notify_all();
    }
    /// The files as a crash at this moment would leave them.
    fn crash(&self) -> MemStore {
        let m = MemStore::new();
        *m.files.lock().unwrap() = self.files.files.lock().unwrap().clone();
        m
    }
    fn snapshot_seq_on_disk(&self) -> Option<u64> {
        let v: serde_json::Value = serde_json::from_slice(&self.files.get(SNAPSHOT)?).ok()?;
        v["seq"].as_u64()
    }
}

impl Store for Gated {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn write_atomic_with(&mut self, name: &str, fill: &mut dyn FnMut(&mut dyn std::io::Write) -> std::io::Result<()>) -> std::io::Result<u64> {
        if name == SNAPSHOT {
            let (lock, cv) = &*self.gate;
            let (open, timeout) = cv.wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(20), |open| !*open).unwrap();
            if !*open || timeout.timed_out() {
                return Err(std::io::Error::other("gate never opened"));
            }
        }
        let mut buf = Vec::new();
        fill(&mut buf)?;
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.files.write_atomic(name, &buf)?;
        Ok(buf.len() as u64)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "gated".into()
    }
    fn background_writer(&self) -> Option<Box<dyn Store>> {
        Some(Box::new(self.clone()))
    }
}

/// Apply and durably append one edit (two ops in one append every fourth time).
fn edit(j: &mut Journal, live: &mut Catalog, k: u64) {
    let one = |live: &mut Catalog, k: u64| {
        let op = if live.len() < 4 || k.is_multiple_of(3) {
            let id = live.alloc_photo_id();
            Op::AddPhoto { photo: Box::new(Photo::new(id, Source::File { path: format!("/p/{}.jpg", id.0) }, "p.jpg", "JPEG", 4, 3, "t")) }
        } else {
            Op::SetRating { id: PhotoId(1 + k % live.len() as u64), rating: (k % 6) as u8 }
        };
        live.apply(op.clone()).unwrap();
        op
    };
    let mut ops = vec![one(live, k)];
    if k.is_multiple_of(4) {
        ops.push(one(live, k / 4 + 1));
    }
    j.append(&ops).unwrap();
}

/// Load the files a crash now would leave: they must hold exactly the live state.
fn assert_recovers(g: &Gated, live: &Catalog) -> LoadReport {
    let (_, c, r) = Journal::open(Box::new(g.crash())).unwrap();
    assert_eq!(c.to_snapshot(), live.to_snapshot(), "{r:?}");
    assert_eq!((r.failed, r.torn_bytes, r.damaged.clone()), (0, 0, None));
    r
}

fn wait_written(j: &Journal) {
    for _ in 0..20_000 {
        if j.snapshot_written() {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("background snapshot never finished");
}

#[test]
fn background_snapshot_keeps_edits_made_while_it_runs() {
    let g = Gated::new();
    let (mut j, mut live, _) = Journal::open(Box::new(g.clone())).unwrap();
    for k in 0..30 {
        edit(&mut j, &mut live, k);
    }
    let n = j.seq();
    j.snapshot_in_background(&live).unwrap();
    assert!(j.snapshot_running() && !j.wants_snapshot() && j.stats().snapshot_running);

    // edits while the snapshot is being written are appended (durably) as usual
    let mut during = 0;
    for k in 30..50 {
        edit(&mut j, &mut live, k);
        during = j.seq() - n;
    }
    // crash before the snapshot landed: no snapshot yet, the whole log replays
    let r = assert_recovers(&g, &live);
    assert_eq!((r.snapshot_seq, r.replayed as u64), (0, j.seq()));

    // the snapshot lands (op n); crash before the log was trimmed: its records are stale
    g.set_gate(true);
    wait_written(&j);
    assert_eq!(g.snapshot_seq_on_disk(), Some(n));
    for k in 50..55 {
        edit(&mut j, &mut live, k);
        during = j.seq() - n;
    }
    let r = assert_recovers(&g, &live);
    assert_eq!((r.snapshot_seq, r.stale as u64, r.replayed as u64), (n, n, during));

    // finishing trims the log to exactly the records after the snapshot, in order
    assert!(j.poll().unwrap());
    assert!(!j.snapshot_running());
    assert_eq!((j.snapshot_seq(), j.log_records()), (n, during));
    let log = String::from_utf8(g.files.get(LOG).unwrap()).unwrap();
    let seqs: Vec<u64> = log.lines().map(|l| journal::decode_record(l).unwrap().0).collect();
    assert_eq!(seqs, ((n + 1)..=j.seq()).collect::<Vec<_>>());
    assert_eq!(j.log_bytes(), log.len() as u64);
    let r = assert_recovers(&g, &live);
    assert_eq!((r.snapshot_seq, r.stale, r.replayed as u64), (n, 0, during));
    let s = j.stats();
    assert!(s.last_snapshot.background && s.last_snapshot.records == n && !s.snapshot_running, "{s:?}");

    // and appending goes on after it
    edit(&mut j, &mut live, 99);
    assert_recovers(&g, &live);
}

#[test]
fn failed_background_snapshot_loses_nothing_and_retries_later() {
    let g = Gated::new();
    let (mut j, mut live, _) = Journal::open(Box::new(g.clone())).unwrap();
    j.policy = SnapshotPolicy { max_records: 10, max_bytes: u64::MAX };
    for k in 0..12 {
        edit(&mut j, &mut live, k);
    }
    assert!(j.wants_snapshot());
    g.fail.store(true, Ordering::SeqCst);
    g.set_gate(true);
    j.snapshot_in_background(&live).unwrap();
    edit(&mut j, &mut live, 12);
    wait_written(&j);
    assert!(j.poll().is_err());
    assert_eq!((j.snapshot_seq(), j.stats().failed_snapshots), (0, 1));
    // still the (empty) snapshot written when the library was created
    assert_eq!(g.snapshot_seq_on_disk(), Some(0));
    assert_recovers(&g, &live);
    // no retry storm: the next attempt waits for another `max_records` records
    assert!(!j.wants_snapshot());
    g.fail.store(false, Ordering::SeqCst);
    while !j.wants_snapshot() {
        let k = j.seq();
        edit(&mut j, &mut live, k);
    }
    j.snapshot_in_background(&live).unwrap();
    j.wait().unwrap();
    assert_eq!((j.snapshot_seq(), j.log_records()), (j.seq(), 0));
    let r = assert_recovers(&g, &live);
    assert_eq!((r.snapshot_seq, r.replayed), (j.seq(), 0));
}

/// A synchronous snapshot (close, "Optimize Library") waits for the background one, so the older
/// snapshot can't land after (and replace) the newer one.
#[test]
fn sync_snapshot_waits_for_the_background_one() {
    let g = Gated::new();
    let (mut j, mut live, _) = Journal::open(Box::new(g.clone())).unwrap();
    for k in 0..10 {
        edit(&mut j, &mut live, k);
    }
    j.snapshot_in_background(&live).unwrap();
    for k in 10..20 {
        edit(&mut j, &mut live, k);
    }
    let opener = {
        let g = g.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            g.set_gate(true);
        })
    };
    j.snapshot(&live).unwrap();
    opener.join().unwrap();
    assert!(!j.snapshot_running());
    assert_eq!(g.snapshot_seq_on_disk(), Some(j.seq()));
    assert_eq!((j.snapshot_seq(), j.log_records()), (j.seq(), 0));
    assert!(g.files.get(LOG).unwrap().is_empty());
    assert_eq!(j.stats().snapshots, 2);
    assert_recovers(&g, &live);
}

/// Dropping the journal (e.g. reopening the library) waits for the snapshot in flight and leaves
/// the log whole: reopening finds every op.
#[test]
fn dropping_the_journal_waits_for_its_snapshot() {
    let g = Gated::new();
    let (mut j, mut live, _) = Journal::open(Box::new(g.clone())).unwrap();
    for k in 0..10 {
        edit(&mut j, &mut live, k);
    }
    let n = j.seq();
    j.snapshot_in_background(&live).unwrap();
    for k in 10..15 {
        edit(&mut j, &mut live, k);
    }
    let opener = {
        let g = g.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            g.set_gate(true);
        })
    };
    let total = j.seq();
    drop(j);
    opener.join().unwrap();
    assert_eq!(g.snapshot_seq_on_disk(), Some(n));
    let (_, c, r) = Journal::open(Box::new(g.files.clone())).unwrap();
    assert_eq!(c.to_snapshot(), live.to_snapshot());
    assert_eq!((r.snapshot_seq, r.stale as u64, r.replayed as u64), (n, n, total - n));
}

/// On disk, with a real worker thread: a crash mid-write leaves only a temp file, which loading
/// ignores; edits during the snapshot survive; reopening resumes appends.
#[test]
fn fs_store_background_snapshot() {
    let dir = std::env::temp_dir().join(format!("lc-journal-bg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut j, mut live, _) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    for k in 0..200 {
        edit(&mut j, &mut live, k);
    }
    // a previous crash mid-snapshot left a partial temp file
    std::fs::write(dir.join(format!("{SNAPSHOT}.tmp")), b"{\"format\":\"lightcraft-catal").unwrap();
    let (mut j2, c2, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert_eq!((c2.to_snapshot(), r.snapshot_seq), (live.to_snapshot(), 0));
    drop(j);
    let n = j2.seq();
    j2.snapshot_in_background(&live).unwrap();
    for k in 200..260 {
        edit(&mut j2, &mut live, k);
    }
    j2.wait().unwrap();
    assert_eq!((j2.snapshot_seq(), j2.log_records()), (n, j2.seq() - n));
    assert!(!dir.join(format!("{SNAPSHOT}.tmp")).exists());
    edit(&mut j2, &mut live, 999);
    drop(j2);
    let (_, c3, r) = Journal::open(Box::new(FsStore::open(&dir).unwrap())).unwrap();
    assert_eq!(c3.to_snapshot(), live.to_snapshot());
    assert_eq!((r.snapshot_seq, r.stale), (n, 0));
    let _ = std::fs::remove_dir_all(&dir);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Any interleaving of edits, background snapshots (started, landed, finished), synchronous
    /// snapshots and crashes: a crash at any point recovers exactly the acknowledged ops.
    #[test]
    fn background_compaction_never_loses_acknowledged_ops(actions in proptest::collection::vec((0u8..8, 0u64..1000), 1..60)) {
        let g = Gated::new();
        let (mut j, mut live, _) = Journal::open(Box::new(g.clone())).unwrap();
        for (ctl, k) in actions {
            match ctl {
                0..=2 => edit(&mut j, &mut live, k),
                3 => j.snapshot_in_background(&live).unwrap(),
                4 | 5 => {
                    // the worker writes its snapshot; finish it now or later
                    if j.snapshot_running() {
                        g.set_gate(true);
                        wait_written(&j);
                        g.set_gate(false);
                        if ctl == 4 {
                            prop_assert!(j.poll().unwrap());
                        }
                    }
                }
                6 => {
                    g.set_gate(true);
                    j.snapshot(&live).unwrap();
                    g.set_gate(false);
                    prop_assert_eq!(g.snapshot_seq_on_disk(), Some(j.seq()));
                }
                _ => {
                    // a crash now
                    let (_, c, r) = Journal::open(Box::new(g.crash())).unwrap();
                    prop_assert_eq!(c.to_snapshot(), live.to_snapshot());
                    prop_assert_eq!(r.failed, 0);
                    prop_assert!(r.damaged.is_none());
                }
            }
            // the snapshot on disk never covers more than what was appended
            prop_assert!(g.snapshot_seq_on_disk().unwrap_or(0) <= j.seq());
        }
        g.set_gate(true);
        let (_, c, _) = Journal::open(Box::new(g.crash())).unwrap();
        prop_assert_eq!(c.to_snapshot(), live.to_snapshot());
        drop(j);
        let (_, c, _) = Journal::open(Box::new(g.files.clone())).unwrap();
        prop_assert_eq!(c.to_snapshot(), live.to_snapshot());
    }
}
