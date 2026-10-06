//! Crash-safe catalog persistence: an append-only op log plus periodic snapshots.
//!
//! Files (in a [`Store`]):
//! - `catalog.snap` — `{"format":"lightcraft-catalog","version":V,"seq":N,"catalog":{…}}`, the
//!   state after op `N` in catalog format `V` ([`VERSION`]); replaced atomically (temp + fsync +
//!   rename).
//! - `catalog.log` — JSON lines, one per op applied after the snapshot:
//!   `{"seq":N,"crc":C,"op":{…}}` where `C` is the CRC-32 of the op's JSON text. Appends are
//!   fsynced before [`Journal::append`] returns.
//!
//! Loading = snapshot + replay of the records with `seq > snapshot seq`. Recovery rules:
//! - a **torn final record** (crash mid-append: truncated line or bad CRC at the end) is dropped
//!   and the file is cut back to the last good record, so later appends start on a clean line;
//! - a **failed append** (disk full, a dropped network share) may leave part of its batch in the
//!   file: [`Journal::append`] cuts the log back to the last whole record before returning the
//!   error (or, if that fails too, before the next append), so the retry — the same ops with the
//!   same `seq`s — starts on a clean line. Logs written by builds without that cut-back hold the
//!   fragment glued to the first retried record on one line; the loader recognises the whole,
//!   CRC-valid record at the end of such a line and skips only the fragment;
//! - records already covered by the snapshot (crash between writing the snapshot and resetting
//!   the log) are skipped;
//! - a bad record **followed by good ones** is real damage: replay stops there, the log is kept
//!   as `catalog.log.damaged-<seq>`, and a fresh snapshot is written so the library stays usable.
//!
//! **Background compaction** ([`Journal::snapshot_in_background`], when the store offers a
//! [`Store::background_writer`]): a worker thread writes the snapshot of a copy of the catalog at
//! op `N` while appends go on (durably, as always) to the same log. Once the snapshot is durable,
//! [`Journal::poll`] atomically replaces the log with the records appended since the snapshot
//! started (`seq > N`, kept in memory). Every crash point is covered by the rules above:
//! - before the rename of `catalog.snap`: the old snapshot + the whole log (nothing was removed);
//! - between the rename and the log rewrite: the new snapshot + the whole log, whose records
//!   `<= N` are skipped as stale;
//! - after the log rewrite: the new snapshot + exactly the records `> N`.
//!
//! At most one snapshot is in flight; a synchronous [`Journal::snapshot`] and dropping the journal
//! first wait for it, so an older snapshot can never replace a newer one.
//!
//! # Format versions
//!
//! The snapshot's `version` is the catalog format, and it covers the log beside it: the log has
//! no header of its own, and is only ever appended to under a snapshot of this build's format.
//!
//! | Version | Written by | Adds |
//! |---|---|---|
//! | (none) | early builds: a log without a snapshot | |
//! | 1 | up to v0.2.0 | |
//! | 2 | after v0.2.0 | `Op::SetBrowsed`, `Catalog.browsed`, `Photo.local_baseline` |
//!
//! Rules:
//! - **Bump [`VERSION`]** (and add a row above) in the change that adds an [`Op`] variant or a
//!   serialized field to the catalog, a photo, an album, a stack or develop settings that an older
//!   build would misread or silently drop. New fields must deserialize from older data
//!   (`#[serde(default)]`), so newer builds keep reading every older format.
//! - **Opening an older format upgrades it**: right after loading, [`Journal::open`] writes a
//!   snapshot in this build's format (also for a new library), before anything is appended — so a
//!   library is never a mix, and older builds refuse it from then on instead of reading part of it.
//!   [`LoadReport::upgraded_from`] says when that happened.
//! - **A newer format is refused, untouched**: a snapshot whose `version` is above [`VERSION`],
//!   or a log record whose CRC matches (so it is exactly what was written) but whose op doesn't
//!   parse, fails [`Journal::open`] with [`CatalogError::Newer`] before any file is modified — it
//!   is never mistaken for a torn tail or damage. (Builds up to v0.2.0 refuse a v2 snapshot with
//!   "unsupported format".)

use crate::store::Store;
use crate::{Catalog, CatalogError, Op, Result};

pub const SNAPSHOT: &str = "catalog.snap";
pub const LOG: &str = "catalog.log";
const FORMAT: &str = "lightcraft-catalog";
/// The catalog format this build writes (and the newest it reads). See the module docs →
/// *Format versions*; bump it whenever an [`Op`] variant or a serialized field is added.
pub const VERSION: u32 = 2;

/// When [`Journal::wants_snapshot`] says it's time to compact the log.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotPolicy {
    pub max_records: u64,
    pub max_bytes: u64,
}

impl Default for SnapshotPolicy {
    fn default() -> Self {
        SnapshotPolicy { max_records: 2000, max_bytes: 16 << 20 }
    }
}

/// What happened while loading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    /// `seq` of the snapshot that was loaded (0 = none).
    pub snapshot_seq: u64,
    /// Log records applied on top of the snapshot.
    pub replayed: usize,
    /// Records skipped because the snapshot already contained them.
    pub stale: usize,
    /// Records whose op failed to apply (should never happen; reported, not fatal).
    pub failed: usize,
    /// Bytes of a torn final record that were dropped.
    pub torn_bytes: u64,
    /// Partial records left mid-log by a failed append that was retried (written by builds that
    /// didn't cut the log back after a failed write): skipped, the retried records were replayed.
    pub torn_fragments: usize,
    /// The damaged log was preserved under this name.
    pub damaged: Option<String>,
    /// No catalog files existed (a new library).
    pub created: bool,
    /// The library was in an older catalog format (`0` = before format versions: a log without a
    /// snapshot) and was rewritten in this version's format ([`VERSION`]).
    pub upgraded_from: Option<u32>,
}

/// Where persistence time goes (reported by `library.info` → `persistence`; printed to stderr
/// per write under `LIGHTCRAFT_PROFILE`). Times are wall-clock milliseconds on the calling
/// thread, i.e. how long the caller (the UI thread, for the app) was blocked.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistStats {
    /// [`Journal::append`] calls that wrote something.
    pub appends: u64,
    /// Encode + write + `sync_data` of the last / slowest append.
    pub last_append_ms: f64,
    pub max_append_ms: f64,
    /// Snapshots written (compactions, plus the ones on close / repair).
    pub snapshots: u64,
    /// The last snapshot, by stage.
    pub last_snapshot: SnapshotTiming,
    /// Total time of the slowest snapshot.
    pub max_snapshot_ms: f64,
    /// Longest time a snapshot blocked the caller (for background snapshots: copying the catalog
    /// and starting the worker, plus rewriting the log when it is done).
    pub max_blocking_ms: f64,
    /// A background snapshot is being written.
    pub snapshot_running: bool,
    /// Background snapshots that failed (the log was kept whole; retried later).
    pub failed_snapshots: u64,
}

/// One snapshot (compaction), by stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotTiming {
    /// Serialising the catalog to JSON, streamed into the temp file (buffered writes included).
    pub serialize_ms: f64,
    /// Flushing, `sync_all` and the atomic rename of `catalog.snap`.
    pub write_sync_ms: f64,
    /// Resetting `catalog.log` (an atomic rewrite to empty, or to the records appended while a
    /// background snapshot ran; fsynced).
    pub reset_ms: f64,
    /// Start to finish (for a background snapshot: until [`Journal::poll`] saw it done).
    pub total_ms: f64,
    /// How long the caller was blocked (equal to `total_ms` unless `background`).
    pub blocking_ms: f64,
    /// Written by a worker thread.
    pub background: bool,
    /// Size of `catalog.snap`.
    pub bytes: u64,
    /// Log records the snapshot compacted.
    pub records: u64,
}

/// `LIGHTCRAFT_PROFILE` is set: print persistence timings to stderr.
fn profiling() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGHTCRAFT_PROFILE").is_some())
}

fn ms_since(t: web_time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

pub struct Journal {
    store: Box<dyn Store>,
    /// `seq` of the last durable op.
    seq: u64,
    snapshot_seq: u64,
    log_records: u64,
    log_bytes: u64,
    pub policy: SnapshotPolicy,
    stats: PersistStats,
    /// The background snapshot in flight.
    pending: Option<Pending>,
    /// After a failed background snapshot: don't retry before the log has this many records.
    retry_at_records: u64,
    /// A failed append may have left part of its records after `log_bytes`, and cutting them off
    /// failed too: the next append cuts the log back to `log_bytes` first.
    tail_dirty: bool,
}

/// A background snapshot in flight.
struct Pending {
    /// The snapshot holds the state after this op.
    seq: u64,
    /// Log bytes / records appended since it started (all `> seq`): the log once it's done.
    tail: Vec<u8>,
    tail_records: u64,
    /// Log records it compacts.
    records: u64,
    started: web_time::Instant,
    /// Time the caller spent starting it.
    start_ms: f64,
    worker: std::thread::JoinHandle<std::io::Result<SnapshotTiming>>,
}

fn io(e: std::io::Error) -> CatalogError {
    CatalogError::Io(e.to_string())
}

/// One log line (without the newline) for op number `seq`.
pub fn encode_record(seq: u64, op: &Op) -> String {
    let body = serde_json::to_string(op).unwrap_or_default();
    let crc = crc32fast::hash(body.as_bytes());
    format!("{{\"seq\":{seq},\"crc\":{crc},\"op\":{body}}}")
}

/// Parse one log line (without the newline). `None` if it's malformed, the CRC doesn't match, or
/// the op is one this version doesn't know (see [`Journal::open`]).
pub fn decode_record(line: &str) -> Option<(u64, Op)> {
    match decode_line(line) {
        Line::Record(seq, op) => Some((seq, op)),
        Line::Newer(_) | Line::Bad => None,
    }
}

/// One log line, decoded.
enum Line {
    Record(u64, Op),
    /// The CRC matches — the line is exactly what was written — but the op doesn't parse: it
    /// was written by a newer LightCraft (an op or field this version doesn't know), not damaged.
    Newer(u64),
    /// Malformed or a CRC mismatch: torn or damaged.
    Bad,
}

fn decode_line(line: &str) -> Line {
    let parts = || {
        let rest = line.strip_prefix("{\"seq\":")?;
        let (seq, rest) = rest.split_once(",\"crc\":")?;
        let (crc, rest) = rest.split_once(",\"op\":")?;
        let body = rest.strip_suffix('}')?;
        Some((seq.parse::<u64>().ok()?, crc.parse::<u32>().ok()?, body))
    };
    let Some((seq, crc, body)) = parts() else { return Line::Bad };
    if crc32fast::hash(body.as_bytes()) != crc {
        return Line::Bad;
    }
    match serde_json::from_str(body) {
        Ok(op) => Line::Record(seq, op),
        Err(_) => Line::Newer(seq),
    }
}

/// The error for a library written by a newer LightCraft.
fn newer(what: String) -> CatalogError {
    CatalogError::Newer(format!("{what}; this version reads catalog format v{VERSION} and older"))
}

/// A line that doesn't decode but ends with a whole record: a fragment of a failed append followed
/// by the record that retried it (the retry re-encodes the same ops with the same `seq`s). Returns
/// the fragment's length and the record, if that record continues the log (`seq <= last + 1`).
/// The record's CRC must match, so damage is never mistaken for it.
fn resync(line: &[u8], last: u64) -> Option<(usize, (u64, Op))> {
    const START: &[u8] = b"{\"seq\":";
    (1..line.len().saturating_sub(START.len() - 1)).filter(|&i| line[i..].starts_with(START)).find_map(|i| {
        let rec = std::str::from_utf8(&line[i..]).ok().map(|l| l.trim_end_matches('\r')).and_then(decode_record)?;
        (rec.0 <= last + 1).then_some((i, rec))
    })
}

/// Write `catalog.snap` for the state after op `seq`, streaming the JSON into the store (no
/// whole-file string). The bytes are exactly
/// `{"format":"lightcraft-catalog","version":1,"seq":N,"catalog":<serde_json of the catalog>}\n`,
/// as before streaming. Fills in the serialise / write+sync times and the size.
fn write_snapshot(store: &mut dyn Store, seq: u64, catalog: &Catalog) -> std::io::Result<SnapshotTiming> {
    let t0 = web_time::Instant::now();
    let mut serialize_ms = 0.0;
    let bytes = store.write_atomic_with(SNAPSHOT, &mut |w| {
        use std::io::Write;
        let t = web_time::Instant::now();
        // serde_json emits many tiny writes: buffer them in a concrete writer (inlined), so only
        // 1 MiB chunks go through the store's `dyn Write`
        let mut b = std::io::BufWriter::with_capacity(1 << 20, w);
        write!(b, "{{\"format\":\"{FORMAT}\",\"version\":{VERSION},\"seq\":{seq},\"catalog\":")?;
        serde_json::to_writer(&mut b, catalog).map_err(std::io::Error::other)?;
        b.write_all(b"}\n")?;
        b.flush()?;
        serialize_ms = ms_since(t);
        Ok(())
    })?;
    Ok(SnapshotTiming { serialize_ms, write_sync_ms: (ms_since(t0) - serialize_ms).max(0.0), bytes, ..Default::default() })
}

/// Just the identification of `catalog.snap` (read before the catalog itself).
#[derive(serde::Deserialize)]
struct SnapHeader {
    format: String,
    version: u32,
}

/// The whole `catalog.snap` (after [`SnapHeader`] was checked; `format` and `version` are ignored).
#[derive(serde::Deserialize)]
struct SnapFile {
    seq: u64,
    catalog: Catalog,
}

impl Journal {
    /// Open (or create) the catalog in `store`: load the snapshot, replay the log, repair a torn
    /// tail. Fails only if the snapshot itself is unreadable (then nothing is modified).
    pub fn open(mut store: Box<dyn Store>) -> Result<(Journal, Catalog, LoadReport)> {
        let mut report = LoadReport::default();
        let snap = store.read(SNAPSHOT).map_err(io)?;
        let log = store.read(LOG).map_err(io)?;
        report.created = snap.is_none() && log.is_none();
        let (mut catalog, snapshot_seq, snapshot_version) = match snap {
            Some(bytes) => {
                // the header first: a newer snapshot may not parse as this version's catalog
                let h: SnapHeader = serde_json::from_slice(&bytes).map_err(|e| CatalogError::Corrupt(format!("{SNAPSHOT}: {e}")))?;
                if h.format != FORMAT {
                    return Err(CatalogError::Corrupt(format!("{SNAPSHOT}: not a LightCraft catalog (format {:?})", h.format)));
                }
                if h.version > VERSION {
                    return Err(newer(format!("{SNAPSHOT} is catalog format v{}", h.version)));
                }
                let s: SnapFile = serde_json::from_slice(&bytes).map_err(|e| CatalogError::Corrupt(format!("{SNAPSHOT}: {e}")))?;
                (s.catalog, s.seq, Some(h.version))
            }
            None => (Catalog::new(), 0, None),
        };
        report.snapshot_seq = snapshot_seq;
        let mut j = Journal {
            store,
            seq: snapshot_seq,
            snapshot_seq,
            log_records: 0,
            log_bytes: 0,
            policy: SnapshotPolicy::default(),
            stats: PersistStats::default(),
            pending: None,
            retry_at_records: 0,
            tail_dirty: false,
        };
        let log = log.unwrap_or_default();

        // Scan records; `good_end` is the byte offset just past the last good record.
        let mut pos = 0usize;
        let mut good_end = 0usize;
        let mut damaged_at: Option<usize> = None;
        let mut needs_newline = false;
        while pos < log.len() {
            let (line_end, next, has_nl) = match log[pos..].iter().position(|b| *b == b'\n') {
                Some(i) => (pos + i, pos + i + 1, true),
                None => (log.len(), log.len(), false),
            };
            let line = std::str::from_utf8(&log[pos..line_end]).ok().map(|l| l.trim_end_matches('\r'));
            if line.is_some_and(|l| l.trim().is_empty()) {
                pos = next;
                good_end = next;
                continue;
            }
            let mut record = line.and_then(decode_record);
            if record.is_none()
                && let Some((at, r)) = resync(&log[pos..line_end], j.seq)
            {
                // the torn start of a failed append, then the retried records (older builds
                // appended them right after the fragment)
                log::warn!("catalog log: skipped a partial record ({at} bytes) left by a failed write");
                report.torn_fragments += 1;
                record = Some(r);
            }
            if record.is_none()
                && let Some(Line::Newer(seq)) = line.map(decode_line)
            {
                return Err(newer(format!("{LOG} record {seq} holds a change this version doesn't know")));
            }
            match record {
                Some((seq, op)) if seq <= j.seq => {
                    // already in the snapshot
                    let _ = op;
                    report.stale += 1;
                }
                Some((seq, op)) if seq == j.seq + 1 => {
                    if catalog.apply(op).is_err() {
                        report.failed += 1;
                    } else {
                        report.replayed += 1;
                    }
                    j.seq = seq;
                }
                _ => {
                    // bad record (or a gap): torn tail if nothing good follows, else damage
                    let mut rest_has_good = false;
                    for l in log[next..].split(|b| *b == b'\n') {
                        match std::str::from_utf8(l).map(|l| decode_line(l.trim_end_matches('\r'))) {
                            Ok(Line::Record(..)) => rest_has_good = true,
                            // refused before anything is modified
                            Ok(Line::Newer(seq)) => return Err(newer(format!("{LOG} record {seq} holds a change this version doesn't know"))),
                            Ok(Line::Bad) | Err(_) => {}
                        }
                    }
                    if rest_has_good {
                        damaged_at = Some(pos);
                    }
                    break;
                }
            }
            j.log_records += 1;
            pos = next;
            good_end = next;
            needs_newline = !has_nl;
        }

        if let Some(at) = damaged_at {
            let name = format!("{LOG}.damaged-{}", j.seq);
            j.store.write_atomic(&name, &log).map_err(io)?;
            log::warn!("catalog log damaged at byte {at}; kept as {name}; state recovered up to op {}", j.seq);
            report.damaged = Some(name);
            j.snapshot(&catalog)?;
        } else {
            if good_end < log.len() {
                report.torn_bytes = (log.len() - good_end) as u64;
                log::warn!("catalog log: dropped a torn final record ({} bytes)", report.torn_bytes);
                j.store.truncate(LOG, good_end as u64).map_err(io)?;
                j.log_bytes = good_end as u64;
            } else if needs_newline {
                j.store.append(LOG, b"\n").map_err(io)?;
                j.log_bytes = good_end as u64 + 1;
            } else {
                j.log_bytes = good_end as u64;
            }
        }
        // Bring an older (or new, or snapshot-less) library to this version's format before
        // anything is appended: a snapshot in this format makes older builds refuse the library
        // instead of misreading ops or dropping fields they don't know.
        if report.created {
            // tiny: written directly, not counted as a compaction
            let empty = format!("{{\"format\":\"{FORMAT}\",\"version\":{VERSION},\"seq\":0,\"catalog\":{}}}\n", catalog.to_snapshot());
            if let Err(e) = j.store.write_atomic(SNAPSHOT, empty.as_bytes()) {
                log::warn!("catalog: can't write the new library's snapshot: {e}");
            }
        } else if report.damaged.is_none() && snapshot_version != Some(VERSION) {
            report.upgraded_from = Some(snapshot_version.unwrap_or(0));
            match j.snapshot(&catalog) {
                Ok(()) => {
                    if let Some(v) = report.upgraded_from {
                        log::info!("catalog: upgraded from format v{v} to v{VERSION}");
                    }
                }
                // nothing lost: the old files are intact and still load; retried on next open
                Err(e) => log::warn!("catalog: can't write the format v{VERSION} snapshot: {e}"),
            }
        }
        catalog.revision = 0;
        Ok((j, catalog, report))
    }

    /// Append ops (already applied to the live catalog) durably, in order.
    pub fn append(&mut self, ops: &[Op]) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let t0 = web_time::Instant::now();
        let mut buf = String::new();
        let mut seq = self.seq;
        for op in ops {
            seq += 1;
            buf.push_str(&encode_record(seq, op));
            buf.push('\n');
        }
        if self.tail_dirty {
            self.store.truncate(LOG, self.log_bytes).map_err(io)?;
            self.tail_dirty = false;
        }
        if let Err(e) = self.store.append(LOG, buf.as_bytes()) {
            // Part of the batch may be in the file. Cut it back to the last whole record, so the
            // retry (same seqs) starts on a clean line instead of after a fragment that would
            // read back as damage.
            if let Err(t) = self.store.truncate(LOG, self.log_bytes) {
                log::error!("catalog: can't cut the log back after a failed append: {t}");
                self.tail_dirty = true;
            }
            return Err(io(e));
        }
        if let Some(p) = self.pending.as_mut() {
            p.tail.extend_from_slice(buf.as_bytes());
            p.tail_records += ops.len() as u64;
        }
        self.seq = seq;
        self.log_records += ops.len() as u64;
        self.log_bytes += buf.len() as u64;
        let ms = ms_since(t0);
        self.stats.appends += 1;
        self.stats.last_append_ms = ms;
        self.stats.max_append_ms = self.stats.max_append_ms.max(ms);
        if profiling() {
            eprintln!("catalog: append {} op(s), {} B: {ms:.2} ms", ops.len(), buf.len());
        }
        Ok(())
    }

    /// Write a snapshot of `catalog` (which must reflect every appended op) and reset the log.
    pub fn snapshot(&mut self, catalog: &Catalog) -> Result<()> {
        self.snapshot_with_unlogged(catalog, 0)
    }

    /// [`Journal::snapshot`] of a catalog that also reflects `unlogged` ops applied after the
    /// last appended one but never written to the log (their append failed). The snapshot counts
    /// them (it holds the state after op `seq + unlogged`), so they are saved and must not be
    /// appended afterwards. Once the snapshot is durable [`Journal::seq`] includes them, even if
    /// resetting the log then fails (the error is still returned).
    pub fn snapshot_with_unlogged(&mut self, catalog: &Catalog, unlogged: u64) -> Result<()> {
        // never two snapshot writers; the one in flight is older, so it must land first
        if let Err(e) = self.wait() {
            log::warn!("catalog: background snapshot failed ({e}); writing one now");
        }
        let t0 = web_time::Instant::now();
        let seq = self.seq + unlogged;
        let mut timing = match write_snapshot(self.store.as_mut(), seq, catalog) {
            Ok(t) => t,
            Err(e) => {
                // nothing lost (the log is whole); don't retry on every append
                self.stats.failed_snapshots += 1;
                self.retry_at_records = self.log_records + self.policy.max_records;
                return Err(io(e));
            }
        };
        // the snapshot is durable: it holds every op up to `seq`
        self.seq = seq;
        self.snapshot_seq = seq;
        let t2 = web_time::Instant::now();
        // A crash here leaves old records in the log; they are skipped by seq on load.
        self.store.write_atomic(LOG, b"").map_err(io)?;
        timing.reset_ms = ms_since(t2);
        timing.total_ms = ms_since(t0);
        timing.blocking_ms = timing.total_ms;
        timing.records = self.log_records;
        self.record_snapshot(timing);
        self.log_records = 0;
        self.log_bytes = 0;
        self.retry_at_records = 0;
        self.tail_dirty = false;
        Ok(())
    }

    /// Compact like [`Journal::snapshot`], but write the snapshot on a worker thread from a copy
    /// of `catalog` (cheap: photos are shared) when the store supports it (else synchronously).
    /// `catalog` must reflect every appended op. Appends go on meanwhile; [`Journal::poll`]
    /// finishes the compaction. No-op while a snapshot is already in flight.
    pub fn snapshot_in_background(&mut self, catalog: &Catalog) -> Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let t0 = web_time::Instant::now();
        let Some(mut writer) = self.store.background_writer() else { return self.snapshot(catalog) };
        let copy = catalog.clone();
        let seq = self.seq;
        let spawned = std::thread::Builder::new().name("catalog-snapshot".into()).spawn(move || write_snapshot(writer.as_mut(), seq, &copy));
        let worker = match spawned {
            Ok(w) => w,
            Err(e) => {
                log::warn!("catalog: can't start a snapshot thread ({e}); writing it now");
                return self.snapshot(catalog);
            }
        };
        self.pending =
            Some(Pending { seq, tail: Vec::new(), tail_records: 0, records: self.log_records, started: t0, start_ms: ms_since(t0), worker });
        Ok(())
    }

    /// Finish a background snapshot if its worker is done (cheap otherwise). Returns whether one
    /// finished. An error means the snapshot failed: nothing was lost (the log is whole), and
    /// compaction is retried once the log has grown by another [`SnapshotPolicy::max_records`].
    pub fn poll(&mut self) -> Result<bool> {
        if !self.pending.as_ref().is_some_and(|p| p.worker.is_finished()) {
            return Ok(false);
        }
        self.finish().map(|()| true)
    }

    /// Wait for the background snapshot in flight, if any, and finish it.
    pub fn wait(&mut self) -> Result<()> {
        if self.pending.is_none() {
            return Ok(());
        }
        self.finish()
    }

    /// A background snapshot is in flight (until [`Journal::poll`] or [`Journal::wait`] finish it).
    pub fn snapshot_running(&self) -> bool {
        self.pending.is_some()
    }

    /// The background worker is done (its result not yet applied by [`Journal::poll`]).
    pub fn snapshot_written(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| p.worker.is_finished())
    }

    /// Join the worker; on success replace the log with the records appended meanwhile.
    fn finish(&mut self) -> Result<()> {
        let Some(p) = self.pending.take() else { return Ok(()) };
        let t0 = web_time::Instant::now();
        let written = p.worker.join().unwrap_or_else(|_| Err(std::io::Error::other("snapshot thread panicked")));
        let mut timing = match written {
            Ok(t) => t,
            Err(e) => {
                // the log still holds every record: nothing lost, retry later
                self.stats.failed_snapshots += 1;
                self.retry_at_records = self.log_records + self.policy.max_records;
                log::error!("catalog: background snapshot failed: {e}");
                return Err(io(e));
            }
        };
        // `catalog.snap` (op `p.seq`) is durable; the log's records `<= p.seq` are now stale.
        // Keep exactly the ones appended since (a crash before this rewrite skips the stale ones).
        let t1 = web_time::Instant::now();
        self.snapshot_seq = p.seq;
        if let Err(e) = self.store.write_atomic(LOG, &p.tail) {
            // the log stays whole (stale records are skipped on load); compact again later
            self.retry_at_records = self.log_records + self.policy.max_records;
            log::error!("catalog: can't trim the log after a snapshot: {e}");
            return Err(io(e));
        }
        timing.reset_ms = ms_since(t1);
        timing.total_ms = ms_since(p.started);
        timing.blocking_ms = p.start_ms + ms_since(t0);
        timing.background = true;
        timing.records = p.records;
        self.record_snapshot(timing);
        self.log_records = p.tail_records;
        self.log_bytes = p.tail.len() as u64;
        self.retry_at_records = 0;
        self.tail_dirty = false;
        Ok(())
    }

    fn record_snapshot(&mut self, t: SnapshotTiming) {
        self.stats.snapshots += 1;
        self.stats.last_snapshot = t;
        self.stats.max_snapshot_ms = self.stats.max_snapshot_ms.max(t.total_ms);
        self.stats.max_blocking_ms = self.stats.max_blocking_ms.max(t.blocking_ms);
        if profiling() {
            eprintln!(
                "catalog: {}snapshot of {} records, {} B: serialize {:.1} ms, write+sync {:.1} ms, log reset {:.1} ms, total {:.1} ms, blocking {:.1} ms",
                if t.background { "background " } else { "" },
                t.records,
                t.bytes,
                t.serialize_ms,
                t.write_sync_ms,
                t.reset_ms,
                t.total_ms,
                t.blocking_ms
            );
        }
    }

    /// Where persistence time went so far.
    pub fn stats(&self) -> PersistStats {
        PersistStats { snapshot_running: self.pending.is_some(), ..self.stats }
    }

    /// The log is long enough to be worth compacting.
    pub fn wants_snapshot(&self) -> bool {
        self.pending.is_none()
            && self.log_records >= self.retry_at_records
            && (self.log_records >= self.policy.max_records || self.log_bytes >= self.policy.max_bytes)
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }
    pub fn snapshot_seq(&self) -> u64 {
        self.snapshot_seq
    }
    pub fn log_records(&self) -> u64 {
        self.log_records
    }
    pub fn log_bytes(&self) -> u64 {
        self.log_bytes
    }
    pub fn describe(&self) -> String {
        self.store.describe()
    }
}

impl Drop for Journal {
    /// Wait for a background snapshot (never leave a writer behind that could replace a newer
    /// snapshot of a journal reopened on the same files). The log is left whole: the snapshot's
    /// records in it are skipped as stale on the next load.
    fn drop(&mut self) {
        if let Some(p) = self.pending.take()
            && let Ok(Err(e)) = p.worker.join()
        {
            log::error!("catalog: background snapshot failed: {e}");
        }
    }
}
