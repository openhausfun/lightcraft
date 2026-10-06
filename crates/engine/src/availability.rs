//! Is a file there? — answered without blocking the caller.
//!
//! On a sleeping NAS, a dropped share or a USB drive spinning up, one `stat` can take as long as
//! the OS or SMB timeout. The UI asks about files every frame (grid cells, the Info panel, the
//! Missing Photos view), so in the app the answers come from a cache that a worker thread fills
//! ([`Availability::run_in_background`]): an unknown file is reported as `None` ("assume it is
//! there"; the render worker finds out if it isn't), known answers are rechecked after
//! [`RECHECK`]. Everywhere else (CLI, MCP, tests) the default is synchronous: every question is a
//! fresh check, as before.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Checks whether a path exists (injectable: tests use a slow one).
pub type ExistsProbe = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// Called (from the worker) when an answer changed, e.g. to repaint the UI.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How long a cached answer is used before it is checked again (it is still returned meanwhile).
pub const RECHECK: Duration = Duration::from_secs(15);

fn fs_exists(path: &str) -> bool {
    // the browser build has no file system: its originals are always "there"
    cfg!(target_arch = "wasm32") || std::path::Path::new(path).exists()
}

#[derive(Default)]
struct State {
    /// path → (exists, when checked)
    known: HashMap<String, (bool, std::time::Instant)>,
    queue: VecDeque<String>,
    queued: HashSet<String>,
    worker: bool,
    /// Bumped whenever an answer changes (callers recompute what depends on it).
    generation: u64,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// File availability, synchronous or cached + background-checked. Cloning shares the cache.
#[derive(Clone)]
pub struct Availability {
    probe: ExistsProbe,
    background: Option<Waker>,
    state: Arc<Mutex<State>>,
}

impl Default for Availability {
    fn default() -> Self {
        Self { probe: Arc::new(fs_exists), background: None, state: Arc::default() }
    }
}

impl std::fmt::Debug for Availability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Availability").field("background", &self.background.is_some()).finish()
    }
}

impl Availability {
    /// Replace the existence check (tests: a slow or counting probe). Forgets cached answers.
    pub fn set_probe(&mut self, probe: ExistsProbe) {
        self.probe = probe;
        let mut st = lock(&self.state);
        st.known.clear();
        st.generation += 1;
    }

    /// Answer from the cache and check on a worker thread from now on; `wake` runs when an
    /// answer changed. No effect in the browser build (no threads, no file system).
    pub fn run_in_background(&mut self, wake: Waker) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        self.background = Some(wake);
    }

    /// The existence check itself (blocking: for worker threads, e.g. the Rename preview's).
    pub fn probe(&self) -> ExistsProbe {
        self.probe.clone()
    }

    pub fn is_background(&self) -> bool {
        self.background.is_some()
    }

    /// Does `path` exist? Synchronous mode: checked now. Background mode: the cached answer
    /// (`None` while unknown); unknown or stale answers are queued for the worker.
    pub fn exists(&self, path: &str) -> Option<bool> {
        let Some(wake) = &self.background else {
            return Some((self.probe)(path));
        };
        let mut st = lock(&self.state);
        let now = std::time::Instant::now();
        let cached = st.known.get(path).copied();
        if cached.is_none_or(|(_, at)| now.saturating_duration_since(at) >= RECHECK) && !st.queued.contains(path) {
            st.queued.insert(path.to_string());
            st.queue.push_back(path.to_string());
            if !st.worker {
                st.worker = true;
                drop(st);
                self.spawn_worker(wake.clone());
                return cached.map(|c| c.0);
            }
        }
        cached.map(|c| c.0)
    }

    /// Known to be missing (unknown counts as present).
    pub fn is_offline(&self, path: &str) -> bool {
        self.exists(path) == Some(false)
    }

    /// Changes whenever a cached answer changes.
    pub fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    /// Paths waiting for the worker.
    pub fn pending(&self) -> usize {
        lock(&self.state).queue.len()
    }

    fn spawn_worker(&self, wake: Waker) {
        let (state, probe) = (self.state.clone(), self.probe.clone());
        let started = std::thread::Builder::new().name("lc-availability".into()).spawn(move || {
            loop {
                let path = {
                    let mut st = lock(&state);
                    let Some(p) = st.queue.pop_front() else {
                        st.worker = false;
                        return;
                    };
                    p
                };
                let ok = probe(&path);
                let changed = {
                    let mut st = lock(&state);
                    st.queued.remove(&path);
                    let prev = st.known.insert(path, (ok, std::time::Instant::now()));
                    let changed = prev.map(|p| p.0) != Some(ok);
                    if changed {
                        st.generation += 1;
                    }
                    changed
                };
                if changed {
                    wake();
                }
            }
        });
        if started.is_err() {
            // try again on the next question
            lock(&self.state).worker = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn synchronous_by_default() {
        let mut a = Availability::default();
        let n = Arc::new(AtomicUsize::new(0));
        let c = n.clone();
        a.set_probe(Arc::new(move |p| {
            c.fetch_add(1, Ordering::SeqCst);
            p == "here"
        }));
        assert_eq!(a.exists("here"), Some(true));
        assert_eq!(a.exists("gone"), Some(false));
        assert_eq!(a.exists("gone"), Some(false));
        assert_eq!(n.load(Ordering::SeqCst), 3, "every question is a fresh check");
    }

    #[test]
    fn background_answers_from_the_cache_without_blocking() {
        let mut a = Availability::default();
        let caller = std::thread::current().id();
        let on_caller = Arc::new(AtomicUsize::new(0));
        let c = on_caller.clone();
        a.set_probe(Arc::new(move |p| {
            if std::thread::current().id() == caller {
                c.fetch_add(1, Ordering::SeqCst);
            }
            std::thread::sleep(Duration::from_millis(200)); // a sleeping NAS
            p == "here"
        }));
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        a.run_in_background(Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let t0 = std::time::Instant::now();
        assert_eq!(a.exists("here"), None, "unknown at first");
        assert_eq!(a.exists("gone"), None);
        assert!(t0.elapsed() < Duration::from_millis(150), "the caller never waits for the probe");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while (a.exists("here").is_none() || a.exists("gone").is_none()) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(a.exists("here"), Some(true));
        assert_eq!(a.exists("gone"), Some(false));
        assert!(a.is_offline("gone") && !a.is_offline("here") && !a.is_offline("unknown"));
        assert_eq!(on_caller.load(Ordering::SeqCst), 0, "never probed on the calling thread");
        assert!(woke.load(Ordering::SeqCst) >= 2);
        assert!(a.generation() >= 2);
    }
}
