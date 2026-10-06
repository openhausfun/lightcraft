//! Issue #99: one process per library.

use std::path::PathBuf;

use crate::lock::{LOCK, OWNER};
use crate::*;

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-lock-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn second_opener_is_refused_until_the_first_lets_go() {
    let dir = temp_dir("twice");
    let first = LibraryLock::acquire(&dir, "LightCraft").unwrap();
    assert!(first.held());
    let owner: LockOwner = serde_json::from_slice(&std::fs::read(dir.join(OWNER)).unwrap()).unwrap();
    assert_eq!((owner.pid, owner.program.as_str()), (std::process::id(), "LightCraft"));

    let e = LibraryLock::acquire(&dir, "lightcraft-cli").unwrap_err();
    let LockError::InUse(Some(who)) = &e else { panic!("{e:?}") };
    assert_eq!(who.pid, std::process::id());
    let msg = e.to_string();
    assert!(msg.contains("already open in LightCraft") && msg.contains(&format!("process {}", std::process::id())), "{msg}");

    drop(first);
    assert!(!dir.join(OWNER).exists(), "the owner note goes with the lock");
    assert!(dir.join(LOCK).exists(), "the lock file itself may stay: only a held lock refuses");
    let again = LibraryLock::acquire(&dir, "lightcraft-cli").unwrap();
    assert!(again.held());
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Files left behind by a crash (lock file + owner note) don't lock anyone out.
#[test]
fn leftover_lock_files_are_not_a_lock() {
    let dir = temp_dir("stale");
    std::fs::write(dir.join(LOCK), b"").unwrap();
    std::fs::write(dir.join(OWNER), br#"{"pid":999999,"host":"elsewhere","program":"LightCraft","version":"0.2.0","since":1}"#).unwrap();
    let l = LibraryLock::acquire(&dir, "LightCraft").unwrap();
    assert!(l.held());
    let owner: LockOwner = serde_json::from_slice(&std::fs::read(dir.join(OWNER)).unwrap()).unwrap();
    assert_eq!(owner.pid, std::process::id(), "the note now names this process");
    drop(l);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Another process holds the lock; when it is killed (a crash), the OS releases it.
#[test]
fn lock_held_by_another_process_is_released_when_it_dies() {
    let dir = temp_dir("child");
    let exe = std::env::current_exe().unwrap();
    let mut child = std::process::Command::new(exe)
        .args(["--exact", "tests_lock::lock_holder_child", "--ignored", "--test-threads=1"])
        .env("LC_LOCK_CHILD_DIR", &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !dir.join("child-locked").exists() {
        assert!(child.try_wait().unwrap().is_none(), "child exited before locking");
        assert!(std::time::Instant::now() < deadline, "child never locked");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let e = LibraryLock::acquire(&dir, "LightCraft").unwrap_err();
    let LockError::InUse(Some(who)) = &e else { panic!("{e:?}") };
    assert_eq!(who.pid, child.id());
    assert!(e.to_string().contains("on this computer") || who.host.is_empty(), "{e}");

    child.kill().unwrap(); // SIGKILL: no clean-up runs, like a crash
    child.wait().unwrap();
    assert!(dir.join(OWNER).exists(), "the crashed holder's note is left behind");
    let l = LibraryLock::acquire(&dir, "LightCraft").expect("the OS released the dead process's lock");
    assert!(l.held());
    drop(l);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run by [`lock_held_by_another_process_is_released_when_it_dies`] in a child process: take the
/// lock, say so, and wait to be killed.
#[test]
#[ignore = "helper process for lock_held_by_another_process_is_released_when_it_dies"]
fn lock_holder_child() {
    let Some(dir) = std::env::var_os("LC_LOCK_CHILD_DIR") else { return };
    let _l = LibraryLock::acquire(std::path::Path::new(&dir), "lightcraft-cli").unwrap();
    std::fs::write(std::path::Path::new(&dir).join("child-locked"), b"").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(120));
}
