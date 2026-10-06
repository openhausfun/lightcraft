//! Rendered thumbnails on disk: `<dir>/<2 hex>/<32 hex>.jpg`, bounded in bytes.
//!
//! The cache is disposable: files are written via temp + rename (readers never see partial
//! files) but not fsynced; unreadable files are deleted and re-rendered. When the total size
//! exceeds the budget, the least recently used files (by modification time, refreshed on read)
//! are removed down to 80 % of the budget.
//!
//! The cache only ever lists, counts or deletes files of its own name shape
//! (`<2 hex>/<32 hex>.jpg` and the `<32 hex>.tmp…` temp files of [`DiskCache::put`], see
//! [`is_cache_file`]): the directory may be shared with other files (e.g. a library opened on a
//! photo folder that already has a `thumbs/` folder), and those are never touched (issue #98).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use lightcraft_raster::Rgba8;

use crate::Hash128;

const QUALITY: u8 = 90;

pub struct DiskCache {
    dir: PathBuf,
    budget: u64,
    /// Bytes on disk (`None` until first scanned).
    total: Mutex<Option<u64>>,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub writes: AtomicU64,
}

impl DiskCache {
    pub fn new(dir: &Path, budget: u64) -> DiskCache {
        DiskCache {
            dir: dir.to_path_buf(),
            budget,
            total: Mutex::new(None),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            writes: AtomicU64::new(0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, key: Hash128) -> PathBuf {
        let hex = key.to_string();
        self.dir.join(&hex[..2]).join(format!("{hex}.jpg"))
    }

    pub fn get(&self, key: Hash128) -> Option<Rgba8> {
        let p = self.path(key);
        let Ok(bytes) = std::fs::read(&p) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        match decode_jpeg(&bytes) {
            Some(img) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                // refresh recency for pruning
                if let Ok(f) = std::fs::File::options().write(true).open(&p) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(img)
            }
            None => {
                log::warn!("preview cache: dropping unreadable {}", p.display());
                let _ = std::fs::remove_file(&p);
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn put(&self, key: Hash128, img: &Rgba8) {
        let Some(bytes) = encode_jpeg(img) else { return };
        let p = self.path(key);
        let Some(parent) = p.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        // know the current total before adding (the first scan must not count this file twice)
        let _ = self.size();
        let tmp = p.with_extension(format!("tmp{:?}", std::thread::current().id()).replace(['(', ')'], ""));
        if std::fs::write(&tmp, &bytes).is_err() || std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
            return;
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        let over = {
            let mut t = self.total.lock().unwrap_or_else(|e| e.into_inner());
            let total = t.get_or_insert(0);
            *total += bytes.len() as u64;
            *total > self.budget
        };
        if over {
            self.prune();
        }
    }

    /// All cache files (only names of the cache's own shape, see [`is_cache_file`]):
    /// (path, size, modified).
    fn scan(&self) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        for sub in rd.flatten() {
            let sub_name = sub.file_name();
            let Some(sub_name) = sub_name.to_str() else { continue };
            if !is_shard_name(sub_name) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(sub.path()) else { continue };
            for f in files.flatten() {
                let name = f.file_name();
                let Some(name) = name.to_str() else { continue };
                if !is_cache_file(sub_name, name) {
                    continue;
                }
                // `DirEntry::metadata` does not follow symlinks: a link is never a cache file
                if let Ok(m) = f.metadata()
                    && m.is_file()
                {
                    out.push((f.path(), m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH)));
                }
            }
        }
        out
    }

    /// Remove least recently used files until the cache is at most 80 % of its budget.
    pub fn prune(&self) {
        let mut files = self.scan();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let target = self.budget / 5 * 4;
        files.sort_by_key(|f| f.2);
        for (p, len, _) in files {
            if total <= target {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total -= len;
            }
        }
        *self.total.lock().unwrap_or_else(|e| e.into_inner()) = Some(total);
    }

    /// Bytes on disk (scans if not yet known).
    pub fn size(&self) -> u64 {
        let mut t = self.total.lock().unwrap_or_else(|e| e.into_inner());
        *t.get_or_insert_with(|| self.scan().iter().map(|f| f.1).sum())
    }

    pub fn clear(&self) {
        for (p, _, _) in self.scan() {
            let _ = std::fs::remove_file(p);
        }
        *self.total.lock().unwrap_or_else(|e| e.into_inner()) = Some(0);
    }
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A shard directory name: two lowercase hex digits.
fn is_shard_name(s: &str) -> bool {
    s.len() == 2 && is_lower_hex(s)
}

/// Whether `name` inside shard directory `shard` is a file this cache wrote: `<32 hex>.jpg`
/// (whose first two digits are the shard) or a `<32 hex>.tmp<alphanumeric>` temp file of
/// [`DiskCache::put`]. Anything else in the cache directory belongs to someone else.
pub fn is_cache_file(shard: &str, name: &str) -> bool {
    if !is_shard_name(shard) {
        return false;
    }
    let Some((stem, ext)) = name.split_once('.') else { return false };
    if stem.len() != 32 || !is_lower_hex(stem) || !stem.starts_with(shard) {
        return false;
    }
    ext == "jpg" || ext.strip_prefix("tmp").is_some_and(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// Encode a thumbnail as the cache stores it (JPEG, RGB, 4:2:0).
pub fn encode_jpeg(img: &Rgba8) -> Option<Vec<u8>> {
    if img.width == 0 || img.height == 0 || img.width > u16::MAX as usize || img.height > u16::MAX as usize {
        return None;
    }
    let mut rgb = Vec::with_capacity(img.width * img.height * 3);
    for p in &img.data {
        rgb.extend_from_slice(&p[..3]);
    }
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, QUALITY);
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_2_0);
    enc.encode(&rgb, img.width as u16, img.height as u16, jpeg_encoder::ColorType::Rgb).ok()?;
    Some(out)
}

/// Decode a cached thumbnail (see [`encode_jpeg`]).
pub fn decode_jpeg(bytes: &[u8]) -> Option<Rgba8> {
    use zune_core::bytestream::ZCursor;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), opts);
    let px = d.decode().ok()?;
    let info = d.info()?;
    let (w, h) = (info.width as usize, info.height as usize);
    if px.len() < w * h * 3 {
        return None;
    }
    let data = px.as_chunks::<3>().0.iter().take(w * h).map(|c| [c[0], c[1], c[2], 255]).collect();
    Some(Rgba8 { width: w, height: h, data })
}
