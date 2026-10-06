//! Crash-safe writes of user-visible files: exports, renders, DNG conversions, smart previews and
//! XMP sidecars all go through here (the catalog's own files use [`crate::store::FsStore`]).
//!
//! **Write policy** (issue #134): what can't be recreated — originals, XMP sidecars, the catalog,
//! settings, DNGs that replace raws, smart previews (the only stand-in while an original is
//! offline), an edit copy that becomes a library photo — is written **durably**: synced to disk
//! before it counts as written. What can always be made again from the originals — exports,
//! renders, screenshots, merge previews — is written **atomically** only: a half-written file
//! never replaces a good one, but there is no per-file sync (on a USB drive or a NAS that sync
//! dominates a large export).
//!
//! - [`write_atomic`] replaces a file so that after a crash, a full disk or an unplugged drive
//!   either the old or the complete new content is there — never a truncated mix. The content
//!   goes to a new temp file in the same folder (unique name, `create_new`), is synced to disk and
//!   renamed over the target; then the folder is synced (POSIX). Any failure removes the temp
//!   file and leaves the target as it was.
//! - [`write_atomic_nosync`] is the same temp file + rename without the two syncs, for
//!   recreatable outputs only: a failed write still removes the temp file and leaves the target as
//!   it was, but after a power cut the new file may be lost (on some file systems left empty) —
//!   it is then simply exported again.
//! - [`write_new`] / [`write_new_unique`] write a file that must not exist yet: the same temp file
//!   and sync, plus a read-back check, and the file then appears under its final name **without
//!   replacing anything** (a hard link, which fails on a taken name; a check-then-rename where the
//!   volume has no hard links).
//! - [`same_file`] tells whether two paths name the same file on disk (device + inode on Unix,
//!   resolved paths elsewhere), for "is this target an original?" checks.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

thread_local! {
    static FAIL_AFTER: Cell<Option<u64>> = const { Cell::new(None) };
    static SYNCS: Cell<u64> = const { Cell::new(0) };
}

/// How many file and folder syncs this module made **on the calling thread** (for tests: a
/// durable write path syncs, a recreatable output doesn't).
#[doc(hidden)]
pub fn syncs_on_this_thread() -> u64 {
    SYNCS.with(Cell::get)
}

fn sync_file(f: &File) -> io::Result<()> {
    SYNCS.with(|c| c.set(c.get() + 1));
    f.sync_all()
}

/// Fault injection for tests (in any crate): while the returned guard lives, every write made by
/// this module **on the calling thread** fails once `bytes` bytes of a file were written — as a
/// full disk or an unplugged drive would.
#[doc(hidden)]
pub fn fail_writes_after(bytes: u64) -> FaultGuard {
    FAIL_AFTER.with(|c| c.set(Some(bytes)));
    FaultGuard(())
}

/// Ends a [`fail_writes_after`] when dropped.
#[doc(hidden)]
pub struct FaultGuard(());

impl Drop for FaultGuard {
    fn drop(&mut self) {
        FAIL_AFTER.with(|c| c.set(None));
    }
}

/// Counts what is written; fails past an injected limit.
struct Limited<W> {
    inner: W,
    n: u64,
    limit: Option<u64>,
}

impl<W: Write> Write for Limited<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let buf = match self.limit {
            Some(l) => {
                let room = usize::try_from(l.saturating_sub(self.n)).unwrap_or(usize::MAX);
                if room == 0 && !buf.is_empty() {
                    return Err(io::Error::other("write failed (injected fault)"));
                }
                buf.get(..room.min(buf.len())).unwrap_or(buf)
            }
            None => buf,
        };
        let k = self.inner.write(buf)?;
        self.n += k as u64;
        Ok(k)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn parent_of(path: &Path) -> &Path {
    path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// Make a rename or a new name in `dir` durable (POSIX). Opening a directory fails on Windows,
/// where NTFS journals the rename itself, so errors are ignored.
pub fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = sync_file(&d);
    }
}

/// A new, empty temp file next to `path` (hidden, unique in this process and folder).
fn create_temp(path: &Path) -> io::Result<(PathBuf, File)> {
    static N: AtomicU64 = AtomicU64::new(0);
    if path.file_name().is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a file path", path.display())));
    }
    let dir = parent_of(path);
    for _ in 0..1000 {
        let tmp = dir.join(format!(".lightcraft-{}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("no free temp file name in {}", dir.display())))
}

/// Write a temp file next to `path` with what `fill` writes, synced to disk when `sync`. On
/// failure the temp file is removed. Returns its path and length.
fn write_temp(path: &Path, sync: bool, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> io::Result<(PathBuf, u64)> {
    let (tmp, file) = create_temp(path)?;
    let r = (|| {
        let limit = FAIL_AFTER.with(Cell::get);
        let mut w = Limited { inner: io::BufWriter::with_capacity(1 << 20, file), n: 0, limit };
        fill(&mut w)?;
        w.flush()?;
        let n = w.n;
        let f = w.inner.into_inner().map_err(|e| e.into_error())?;
        if sync {
            sync_file(&f)?;
        }
        Ok(n)
    })();
    match r {
        Ok(n) => Ok((tmp, n)),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Replace (or create) `path` with `data` atomically and durably; see the module docs. The
/// folder must exist.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    write_atomic_with(path, &mut |w| w.write_all(data)).map(|_| ())
}

/// [`write_atomic`] with streamed content: `fill` writes it (an error aborts and leaves the old
/// file). Returns the length written.
pub fn write_atomic_with(path: &Path, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> io::Result<u64> {
    replace_with(path, true, fill)
}

/// Replace (or create) `path` with `data` atomically but **without syncing** it to disk — only for
/// outputs that can always be recreated (exports, renders, screenshots); see the module docs.
/// Anything that can't be made again goes through [`write_atomic`].
pub fn write_atomic_nosync(path: &Path, data: &[u8]) -> io::Result<()> {
    replace_with(path, false, &mut |w| w.write_all(data)).map(|_| ())
}

/// Temp file (synced when `sync`) renamed over `path`, then the folder synced when `sync`.
fn replace_with(path: &Path, sync: bool, fill: &mut dyn FnMut(&mut dyn Write) -> io::Result<()>) -> io::Result<u64> {
    let (tmp, n) = write_temp(path, sync, fill)?;
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if sync {
        sync_dir(parent_of(path));
    }
    Ok(n)
}

/// Write `data` as the new file `path`: never replaces an existing file (`AlreadyExists`), and
/// the file only appears once it is complete, synced and read back identical.
pub fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    write_new_unique(&mut std::iter::once(path.to_path_buf()), data).map(|_| ())
}

/// [`write_new`] under the first free name of `candidates` (all in the same folder, e.g.
/// `IMG_1.dng`, `IMG_1-2.dng`, …): the file is written once and then given the first name that
/// is not taken. Returns that name; `AlreadyExists` when every candidate is taken.
pub fn write_new_unique(candidates: &mut dyn Iterator<Item = PathBuf>, data: &[u8]) -> io::Result<PathBuf> {
    let mut candidates = candidates.peekable();
    let Some(first) = candidates.peek().cloned() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "no file name to write"));
    };
    let (tmp, _) = write_temp(&first, true, &mut |w| w.write_all(data))?;
    let r = (|| {
        // Kept (issue #134 point 4): these files replace raws (Convert / Copy as DNG) or become
        // library photos (merges). On a local disk the read usually comes from the cache, but on a
        // network volume it can go back to the server, and next to encoding a DNG it costs little.
        if !same_content(&tmp, data)? {
            return Err(io::Error::other("the written file reads back different (failing drive or connection?)"));
        }
        for path in candidates.by_ref().take(100_000) {
            match publish_new(&tmp, &path) {
                Ok(()) => {
                    sync_dir(parent_of(&path));
                    return Ok(path);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{}: no free file name", first.display())))
    })();
    // after a hard link the temp name is a second name of the new file; after a rename it's gone
    let _ = fs::remove_file(&tmp);
    r
}

/// Give the complete temp file `tmp` the name `path`, unless that is taken.
fn publish_new(tmp: &Path, path: &Path) -> io::Result<()> {
    match fs::hard_link(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        // no hard links on this volume (FAT/exFAT, some shares): check, then rename
        Err(_) => {
            if fs::symlink_metadata(path).is_ok() {
                return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("{} exists", path.display())));
            }
            fs::rename(tmp, path)
        }
    }
}

/// Does the file at `path` hold exactly `data`?
fn same_content(path: &Path, data: &[u8]) -> io::Result<bool> {
    use std::io::Read;
    let mut f = File::open(path)?;
    if f.metadata()?.len() != data.len() as u64 {
        return Ok(false);
    }
    let mut buf = vec![0u8; 1 << 20];
    let mut at = 0usize;
    loop {
        let k = match f.read(&mut buf) {
            Ok(0) => return Ok(at == data.len()),
            Ok(k) => k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if data.get(at..at + k) != buf.get(..k) {
            return Ok(false);
        }
        at += k;
    }
}

/// Do `a` and `b` name the same existing file (through links, `..`, case-insensitive names)?
pub fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (fs::metadata(a), fs::metadata(b)) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ma.dev() == mb.dev() && ma.ino() == mb.ino()
    }
    #[cfg(not(unix))]
    {
        if ma.len() != mb.len() {
            return false;
        }
        match (fs::canonicalize(a), fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y || x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-safe-file-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = temp("replace");
        let p = d.join("out.jpg");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two two");
        assert_eq!(names(&d), vec!["out.jpg"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failure_mid_write_keeps_the_old_file() {
        let d = temp("midwrite");
        let p = d.join("photo.jpg");
        fs::write(&p, b"the original bytes").unwrap();
        {
            let _f = fail_writes_after(4);
            let e = write_atomic(&p, &[7u8; 10_000]).unwrap_err();
            assert!(e.to_string().contains("injected"), "{e}");
            assert!(write_new(&d.join("new.dng"), &[1u8; 100]).is_err());
        }
        assert_eq!(fs::read(&p).unwrap(), b"the original bytes", "the target is untouched");
        assert_eq!(names(&d), vec!["photo.jpg"], "no temp file and no partial new file");
        // a fill error aborts the same way
        let r = write_atomic_with(&p, &mut |w| {
            w.write_all(b"half")?;
            Err(io::Error::other("encoder failed"))
        });
        assert!(r.is_err());
        assert_eq!(fs::read(&p).unwrap(), b"the original bytes");
        assert_eq!(names(&d), vec!["photo.jpg"]);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn recreatable_outputs_are_atomic_without_a_sync() {
        let d = temp("nosync");
        let p = d.join("export.jpg");
        let before = syncs_on_this_thread();
        write_atomic_nosync(&p, b"one").unwrap();
        write_atomic_nosync(&p, b"two two").unwrap();
        assert_eq!(syncs_on_this_thread(), before, "no file or folder sync");
        assert_eq!(fs::read(&p).unwrap(), b"two two");
        assert_eq!(names(&d), vec!["export.jpg"], "no temp file left");
        // still atomic: a failure part-way leaves the previous file and no temp file
        {
            let _f = fail_writes_after(3);
            assert!(write_atomic_nosync(&p, &[9u8; 10_000]).is_err());
        }
        assert_eq!(fs::read(&p).unwrap(), b"two two");
        assert_eq!(names(&d), vec!["export.jpg"]);
        // the durable writers do sync (the file; the folder too where it can be opened)
        let before = syncs_on_this_thread();
        write_atomic(&p, b"three").unwrap();
        let mid = syncs_on_this_thread();
        assert!(mid > before);
        write_new(&d.join("new.dng"), b"dng").unwrap();
        assert!(syncs_on_this_thread() > mid);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn new_files_never_replace_anything() {
        let d = temp("new");
        let p = d.join("a.dng");
        fs::write(&p, b"keep").unwrap();
        assert_eq!(write_new(&p, b"other").unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&p).unwrap(), b"keep");
        let mut c = (1..).map(|k| if k == 1 { d.join("a.dng") } else { d.join(format!("a-{k}.dng")) });
        let got = write_new_unique(&mut c, b"fresh").unwrap();
        assert_eq!(got, d.join("a-2.dng"));
        assert_eq!(fs::read(&got).unwrap(), b"fresh");
        assert_eq!(names(&d), vec!["a-2.dng", "a.dng"], "the temp file is gone");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn same_file_sees_through_other_spellings() {
        let d = temp("same");
        let p = d.join("x.jpg");
        fs::write(&p, b"x").unwrap();
        fs::write(d.join("y.jpg"), b"x").unwrap();
        fs::create_dir_all(d.join("sub")).unwrap();
        assert!(same_file(&p, &d.join("sub/../x.jpg")));
        assert!(!same_file(&p, &d.join("y.jpg")));
        assert!(!same_file(&p, &d.join("missing.jpg")));
        let _ = fs::remove_dir_all(&d);
    }
}
