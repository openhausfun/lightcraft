//! A prioritized job pool with per-slot de-duplication.
//!
//! Each job targets a *slot* (a grid cell, the loupe…). Submitting for a slot replaces a job for
//! that slot that hasn't started yet, so scrolling or dragging never builds a backlog. Workers
//! take the highest priority first (newest first among equals). Frontends re-prioritize or drop
//! queued jobs when the view changes (e.g. thumbnails scrolled out of view).
//!
//! Native: a fixed set of worker threads, started on first use. wasm32 (no threads): the host
//! calls [`JobPool::run_inline`] once per frame.

use std::hash::Hash;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

pub type Job<R> = Box<dyn FnOnce() -> R + Send>;

struct Entry<S, R> {
    slot: S,
    key: u64,
    priority: u32,
    seq: u64,
    job: Job<R>,
}

struct Shared<S, R> {
    queue: Mutex<Vec<Entry<S, R>>>,
    cv: Condvar,
    shutdown: AtomicBool,
}

/// A finished job.
pub struct Done<S, R> {
    pub slot: S,
    /// The key the job was submitted with (to drop stale results).
    pub key: u64,
    pub result: R,
    /// Run time in milliseconds (0 on wasm).
    pub ms: f64,
}

pub struct JobPool<S, R> {
    shared: Arc<Shared<S, R>>,
    tx: Sender<Done<S, R>>,
    rx: Receiver<Done<S, R>>,
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    threads: usize,
    started: bool,
    seq: u64,
}

impl<S: Copy + Eq + Hash + Send + 'static, R: Send + 'static> JobPool<S, R> {
    /// A pool with `threads` workers (none on wasm; `0` = jobs only run via [`JobPool::run_inline`]).
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = channel();
        JobPool {
            shared: Arc::new(Shared { queue: Mutex::new(Vec::new()), cv: Condvar::new(), shutdown: AtomicBool::new(false) }),
            tx,
            rx,
            threads,
            started: false,
            seq: 0,
        }
    }

    /// Workers for this machine: cores − 1, between 2 and 8.
    pub fn default_threads() -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::thread::available_parallelism().map(|n| n.get().saturating_sub(1)).unwrap_or(4).clamp(2, 8)
        }
        #[cfg(target_arch = "wasm32")]
        {
            1
        }
    }

    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        #[cfg(not(target_arch = "wasm32"))]
        for i in 0..self.threads {
            let shared = self.shared.clone();
            let tx = self.tx.clone();
            let _ = std::thread::Builder::new().name(format!("lc-job-{i}")).spawn(move || {
                while let Some(e) = take(&shared, true) {
                    let t0 = std::time::Instant::now();
                    let result = (e.job)();
                    let done = Done { slot: e.slot, key: e.key, result, ms: t0.elapsed().as_secs_f64() * 1000.0 };
                    if tx.send(done).is_err() {
                        break;
                    }
                }
            });
        }
    }

    /// Queue `job` for `slot`, replacing a queued (not yet running) job for the same slot.
    pub fn submit(&mut self, slot: S, key: u64, priority: u32, job: Job<R>) {
        self.start();
        self.seq += 1;
        let (lock, cv) = (&self.shared.queue, &self.shared.cv);
        let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|e| e.slot != slot);
        q.push(Entry { slot, key, priority, seq: self.seq, job });
        cv.notify_one();
    }

    /// Change queued jobs' priorities: `f(slot, priority)` returns the new priority, or `None` to
    /// drop the job. Returns the dropped slots.
    pub fn reprioritize(&self, mut f: impl FnMut(&S, u32) -> Option<u32>) -> Vec<S> {
        let mut dropped = Vec::new();
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.retain_mut(|e| match f(&e.slot, e.priority) {
            Some(p) => {
                e.priority = p;
                true
            }
            None => {
                dropped.push(e.slot);
                false
            }
        });
        dropped
    }

    /// Queued (not started) jobs.
    pub fn queued(&self) -> usize {
        self.shared.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Is a job for `slot` queued?
    pub fn is_queued(&self, slot: S) -> bool {
        self.shared.queue.lock().map(|q| q.iter().any(|e| e.slot == slot)).unwrap_or(false)
    }

    /// A finished job, if any.
    pub fn try_recv(&self) -> Option<Done<S, R>> {
        self.rx.try_recv().ok()
    }

    /// Run up to `n` jobs on the calling thread (wasm, tests). Returns how many ran.
    pub fn run_inline(&mut self, n: usize) -> usize {
        let mut ran = 0;
        while ran < n {
            let Some(e) = take(&self.shared, false) else { break };
            let result = (e.job)();
            let _ = self.tx.send(Done { slot: e.slot, key: e.key, result, ms: 0.0 });
            ran += 1;
        }
        ran
    }
}

/// Pop the best job (highest priority, newest first). Blocks when `wait` until one is queued or
/// the pool shuts down.
fn take<S, R>(shared: &Shared<S, R>, wait: bool) -> Option<Entry<S, R>> {
    let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if shared.shutdown.load(Ordering::Relaxed) {
            return None;
        }
        if let Some(i) = q.iter().enumerate().max_by_key(|(_, e)| (e.priority, e.seq)).map(|(i, _)| i) {
            return Some(q.swap_remove(i));
        }
        if !wait {
            return None;
        }
        q = shared.cv.wait(q).unwrap_or_else(|e| e.into_inner());
    }
}

impl<S, R> Drop for JobPool<S, R> {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
        self.shared.cv.notify_all();
    }
}
