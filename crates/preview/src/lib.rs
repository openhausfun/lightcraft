//! Previews: everything that makes a big library show up instantly.
//!
//! - [`hash`]: 128-bit content hashes (file bytes → duplicate detection and cache keys).
//! - [`Lru`]: a cost-bounded memory LRU (decoded sources, rendered thumbnails).
//! - [`DiskCache`]: rendered thumbnails as JPEG files keyed by `(content hash, settings hash,
//!   size, renderer version)`, bounded in bytes (least recently used files are pruned).
//! - [`PreviewCache`]: memory LRU in front of the disk cache; shared by render workers.
//! - [`JobPool`]: worker threads with per-slot de-duplication and priorities (visible thumbnails
//!   before off-screen prefetch); runs jobs inline on wasm.
//!
//! No UI dependencies (L3): the egui frontend, the CLI and the MCP server share it.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod disk;
pub mod hash;
pub mod lru;
pub mod pool;

use std::path::Path;
use std::sync::{Arc, Mutex};

pub use disk::{DiskCache, decode_jpeg, encode_jpeg};
pub use hash::{Hash128, Hasher128, hash_bytes};
use lightcraft_raster::Rgba8;
pub use lru::{Lru, next_tick};
pub use pool::JobPool;

/// Rendered-thumbnail cache: memory LRU over an optional disk cache.
pub struct PreviewCache {
    mem: Mutex<Lru<Hash128, Arc<Rgba8>>>,
    disk: Option<DiskCache>,
}

impl PreviewCache {
    /// Memory-only cache of at most `mem_bytes`.
    pub fn memory(mem_bytes: usize) -> PreviewCache {
        PreviewCache { mem: Mutex::new(Lru::new(mem_bytes)), disk: None }
    }

    /// Memory LRU + disk cache in `dir` (at most `disk_bytes`).
    pub fn with_disk(mem_bytes: usize, dir: &Path, disk_bytes: u64) -> PreviewCache {
        PreviewCache { mem: Mutex::new(Lru::new(mem_bytes)), disk: Some(DiskCache::new(dir, disk_bytes)) }
    }

    pub fn disk(&self) -> Option<&DiskCache> {
        self.disk.as_ref()
    }

    pub fn get(&self, key: Hash128) -> Option<Arc<Rgba8>> {
        if let Some(v) = self.mem.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Some(v.clone());
        }
        let img = Arc::new(self.disk.as_ref()?.get(key)?);
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).insert(key, img.clone(), cost(&img));
        Some(img)
    }

    pub fn put(&self, key: Hash128, img: Arc<Rgba8>) {
        if let Some(d) = &self.disk {
            d.put(key, &img);
        }
        let c = cost(&img);
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).insert(key, img, c);
    }

    /// Like [`Self::put`], but the disk write (a JPEG encode) happens on a background thread, so
    /// the caller (a render about to hand its result to the screen) isn't held up.
    pub fn put_deferred(self: &Arc<Self>, key: Hash128, img: Arc<Rgba8>) {
        let c = cost(&img);
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).insert(key, img.clone(), c);
        if self.disk.is_none() {
            return;
        }
        let me = self.clone();
        let write = move || {
            if let Some(d) = &me.disk {
                d.put(key, &img);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("lc-preview-write".into()).spawn(write).is_err() {
            log::warn!("preview cache: could not start a writer thread");
        }
        #[cfg(target_arch = "wasm32")]
        write();
    }

    /// Drop everything (memory and disk).
    pub fn clear(&self) {
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).clear();
        if let Some(d) = &self.disk {
            d.clear();
        }
    }

    /// Use tick ([`next_tick`]) of the least recently used image in memory.
    pub fn oldest_tick(&self) -> Option<u64> {
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).oldest_tick()
    }

    /// Drop the least recently used image from memory (it stays on disk); returns its bytes.
    pub fn evict_oldest(&self) -> Option<usize> {
        self.mem.lock().unwrap_or_else(|e| e.into_inner()).pop_oldest()
    }

    /// Change the memory budget (bytes), evicting at once if needed.
    pub fn set_mem_budget(&self, bytes: usize) {
        let mut m = self.mem.lock().unwrap_or_else(|e| e.into_inner());
        m.set_budget(bytes);
        while m.cost() > bytes && m.len() > 1 {
            m.pop_oldest();
        }
    }

    /// (entries, bytes) held in memory.
    pub fn mem_usage(&self) -> (usize, usize) {
        let m = self.mem.lock().unwrap_or_else(|e| e.into_inner());
        (m.len(), m.cost())
    }
}

fn cost(img: &Rgba8) -> usize {
    img.width * img.height * 4 + 64
}

#[cfg(test)]
mod tests;
