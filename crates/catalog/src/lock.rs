//! One process per library: an exclusive OS lock on `<library>/catalog.lock`, held for as long as
//! the library is open (issue #99).
//!
//! Two processes on the same library (the app and `lightcraft-cli --library`, two app instances,
//! two computers on a shared folder) would each keep their own catalog and `seq` counter, and
//! whichever wrote the last snapshot would silently drop the other's edits. So opening a library
//! takes the lock first, and a second opener gets [`LockError::InUse`] instead.
//!
//! - The lock is the OS's (`flock` on Unix, `LockFileEx` on Windows, via [`std::fs::File::try_lock`]):
//!   it is released when the [`LibraryLock`] is dropped and, after a crash or a kill, by the OS when
//!   the process ends. A `catalog.lock` file left behind is therefore never "stale": only a lock
//!   that's actually held refuses the open.
//! - Who holds it (process id, computer, program, version) is written to `catalog.lock.owner`, for
//!   the error message. It's a separate file because a Windows lock also blocks reading the
//!   locked file. It's only a hint: it may be missing or left over from a crash.
//! - File systems without lock support (some network shares): the library opens unlocked, with a
//!   warning — refusing it would lock the user out of their own library. Locks over SMB/NFS are
//!   advisory and depend on the server; the owner file is then the only hint.
//! - Browser storage (OPFS) has no file locks: see the web host for its own guard.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// The lock file in the library directory.
pub const LOCK: &str = "catalog.lock";
/// Who holds the lock (JSON, see [`LockOwner`]).
pub const OWNER: &str = "catalog.lock.owner";

/// Who has the library open (as written in [`OWNER`]).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LockOwner {
    pub pid: u32,
    /// Computer name, if known.
    pub host: String,
    /// The program (`LightCraft`, `lightcraft-cli`…) and its version.
    pub program: String,
    pub version: String,
    /// When it opened the library (seconds since 1970).
    pub since: u64,
}

impl LockOwner {
    /// This process, as `program`.
    pub fn this_process(program: &str) -> LockOwner {
        LockOwner {
            pid: std::process::id(),
            host: host_name(),
            program: program.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            since: web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        }
    }

    /// "LightCraft 0.3.0 (process 123 on studio-mac)".
    pub fn describe(&self) -> String {
        let program = if self.program.is_empty() { "another LightCraft" } else { self.program.as_str() };
        let version = if self.version.is_empty() { String::new() } else { format!(" {}", self.version) };
        let host = if self.host.is_empty() {
            String::new()
        } else if self.host == host_name() {
            " on this computer".to_string()
        } else {
            format!(" on {}", self.host)
        };
        format!("{program}{version} (process {}{host})", self.pid)
    }
}

/// The computer's name, best effort (empty if unknown).
fn host_name() -> String {
    let env = ["COMPUTERNAME", "HOSTNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|h| !h.trim().is_empty()));
    env.or_else(|| std::fs::read_to_string("/etc/hostname").ok()).map(|h| h.trim().to_string()).filter(|h| !h.is_empty()).unwrap_or_default()
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// Another process has the library open.
    #[error("This library is already open in {}. Close it there first.", .0.as_ref().map(LockOwner::describe).unwrap_or_else(|| "another LightCraft program".into()))]
    InUse(Option<LockOwner>),
}

/// The exclusive lock on a library directory; released on drop (and by the OS if the process
/// dies).
#[derive(Debug)]
pub struct LibraryLock {
    dir: PathBuf,
    /// `None` when the file system can't lock (the library is open unlocked).
    _file: Option<File>,
    held: bool,
}

impl LibraryLock {
    /// Lock `dir` (which must exist) for this process. Fails only when another process holds it.
    pub fn acquire(dir: &Path, program: &str) -> Result<LibraryLock, LockError> {
        let unlocked = |why: io::Error| {
            log::warn!("library {}: can't lock {LOCK} ({why}); opening it without protection against a second program", dir.display());
            LibraryLock { dir: dir.to_path_buf(), _file: None, held: false }
        };
        let file = match OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK)) {
            Ok(f) => f,
            Err(e) => return Ok(unlocked(e)),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                let owner = std::fs::read(dir.join(OWNER)).ok().and_then(|b| serde_json::from_slice::<LockOwner>(&b).ok());
                return Err(LockError::InUse(owner));
            }
            Err(TryLockError::Error(e)) => return Ok(unlocked(e)),
        }
        let owner = LockOwner::this_process(program);
        if let Ok(json) = serde_json::to_vec_pretty(&owner)
            && let Err(e) = std::fs::write(dir.join(OWNER), json)
        {
            log::warn!("library {}: can't write {OWNER}: {e}", dir.display());
        }
        Ok(LibraryLock { dir: dir.to_path_buf(), _file: Some(file), held: true })
    }

    /// The OS lock is held (`false`: the file system can't lock; opened unprotected).
    pub fn held(&self) -> bool {
        self.held
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for LibraryLock {
    /// Remove the owner note while still holding the lock (so it can't delete the next holder's),
    /// then the file handle closes and the OS releases the lock.
    fn drop(&mut self) {
        if self.held {
            let _ = std::fs::remove_file(self.dir.join(OWNER));
        }
    }
}
