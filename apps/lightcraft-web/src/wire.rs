//! Render jobs across the Web Worker boundary.
//!
//! A worker is a second wasm instance with its own memory, so an engine [`RenderJob`] (which holds
//! `Arc`s into the main instance) can't be shared. The main thread sends a [`WireJob`] instead:
//! where the pixels come from (the catalog [`Source`]), the develop settings and the request. The
//! worker keeps its own decoded sources ([`WorkerCore`]), reads originals and cached thumbnails
//! from browser storage, renders, and transfers the RGBA bytes back.
//!
//! Thumbnail disk cache on the web: workers read/write `thumbs/<key>.jpg` in browser storage; the
//! main thread keeps the index (sizes, recency) and prunes it to the native cache's budget
//! ([`ThumbIndex`]).

use std::collections::HashMap;
use std::sync::Arc;

use lightcraft_engine::catalog::Source;
use lightcraft_engine::develop::{DevelopSettings, EmbeddedLens};
use lightcraft_engine::media::{DecodedSource, MediaCache, RenderJob, SourceLevel, SourceRef};
use lightcraft_engine::pipeline::{Quality, RenderRequest, Rendered, SourceInfo, StageCache};
use lightcraft_preview::Lru;
use serde::{Deserialize, Serialize};

use crate::store::hash_of_path;

/// One render request for a worker.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WireJob {
    pub level: SourceLevel,
    pub origin: Source,
    /// Decode the source at most this long.
    pub max_edge: usize,
    pub raw: bool,
    #[serde(default)]
    pub relative_wb: bool,
    pub as_shot_temp: f64,
    pub as_shot_tint: f64,
    pub lens: Option<EmbeddedLens>,
    pub settings: Arc<DevelopSettings>,
    pub max_w: usize,
    pub max_h: usize,
    pub draft: bool,
    pub apply_crop: bool,
    /// Thumbnail cache key (hex): read the cached file first if `thumb_cached`, store the result.
    pub thumb: Option<String>,
    pub thumb_cached: bool,
    /// Reuse the worker's intermediate results for this view (the loupe), like the desktop's
    /// per-slot stage cache: slider drags then only redo what changed.
    pub stages: Option<String>,
    /// The request's diagnostic overlay (`Overlay::to_parts`).
    #[serde(default)]
    pub overlay: (u8, f64),
}

impl WireJob {
    /// `view`: the slot name, for jobs that keep a stage cache.
    pub fn from_job(job: &RenderJob, thumb_cached: bool, view: &str) -> WireJob {
        let max_edge = match &job.source {
            SourceRef::Demo { max_edge, .. } | SourceRef::File { max_edge, .. } => *max_edge,
            SourceRef::Loaded(img) => img.image.width.max(img.image.height),
            SourceRef::Smart { .. } => 2560,
        };
        WireJob {
            level: job.level,
            origin: job.origin.clone(),
            max_edge,
            raw: job.info.raw,
            relative_wb: job.info.relative_wb,
            as_shot_temp: job.info.as_shot_temp,
            as_shot_tint: job.info.as_shot_tint,
            lens: job.info.lens,
            settings: job.settings.clone(),
            max_w: job.request.max_w,
            max_h: job.request.max_h,
            draft: job.request.quality == Quality::Draft,
            apply_crop: job.request.apply_crop,
            thumb: job.cache.as_ref().map(|(_, k)| k.to_string()),
            thumb_cached,
            stages: job.stages.is_some().then(|| view.to_string()),
            overlay: job.request.overlay.to_parts(),
        }
    }

    pub fn info(&self) -> SourceInfo {
        SourceInfo {
            raw: self.raw,
            as_shot_temp: self.as_shot_temp,
            as_shot_tint: self.as_shot_tint,
            lens: self.lens,
            relative_wb: self.relative_wb,
            ..Default::default()
        }
    }

    pub fn request(&self) -> RenderRequest {
        RenderRequest {
            max_w: self.max_w,
            max_h: self.max_h,
            quality: if self.draft { Quality::Draft } else { Quality::Full },
            apply_crop: self.apply_crop,
            overlay: lightcraft_engine::pipeline::Overlay::from_parts(self.overlay.0, self.overlay.1),
            // workers render previews (exports run in-process)
            space: lightcraft_engine::pipeline::OutputSpace::Srgb,
            depth: lightcraft_engine::pipeline::OutputDepth::U8,
            proof: None,
        }
    }

    /// Identifies the decoded source this job needs.
    fn source_key(&self) -> String {
        match &self.origin {
            Source::File { path } => format!("{}:{path}", self.max_edge),
            Source::Demo { scene } => format!("{}:demo:{scene}", self.max_edge),
        }
    }

    /// Storage key of the cached thumbnail.
    pub fn thumb_key(&self) -> Option<String> {
        self.thumb.as_ref().map(|h| thumb_storage_key(h))
    }
}

pub fn thumb_storage_key(hex: &str) -> String {
    format!("thumbs/{hex}.jpg")
}

/// Decoded sources per worker.
const WORKER_SOURCE_BYTES: usize = 192 << 20;

/// The synchronous part of a worker: decoded-source cache + rendering.
pub struct WorkerCore {
    sources: Lru<String, DecodedSource>,
    media: MediaCache,
    stages: HashMap<String, Arc<StageCache>>,
}

impl Default for WorkerCore {
    fn default() -> Self {
        WorkerCore { sources: Lru::new(WORKER_SOURCE_BYTES), media: MediaCache::default(), stages: HashMap::new() }
    }
}

impl WorkerCore {
    /// The content hash of the original to read from storage before [`WorkerCore::render`], if
    /// the job needs one that isn't decoded yet.
    pub fn needs_original(&mut self, job: &WireJob) -> Option<String> {
        if self.sources.get(&job.source_key()).is_some() {
            return None;
        }
        match &job.origin {
            Source::File { path } => hash_of_path(path).map(str::to_string),
            Source::Demo { .. } => None,
        }
    }

    /// Render `job`; `original` holds the file's bytes when [`WorkerCore::needs_original`] asked.
    pub fn render(&mut self, job: &WireJob, original: Option<&[u8]>) -> Result<Rendered, String> {
        let key = job.source_key();
        let src = match self.sources.get(&key).cloned() {
            Some(s) => s,
            None => {
                let src = match (&job.origin, original) {
                    (Source::File { .. }, Some(bytes)) => {
                        let (image, info) = lightcraft_engine::files::load_bytes(bytes, job.max_edge)?;
                        DecodedSource::new(Arc::new(image), Some(info))
                    }
                    (Source::File { path }, None) => return Err(format!("{path}: original not found in browser storage")),
                    (origin @ Source::Demo { .. }, _) => self.media.origin_ref(origin, job.max_edge).load_source()?,
                };
                let cost = src.image.width * src.image.height * 12 + 64;
                self.sources.insert(key, src.clone(), cost);
                src
            }
        };
        let info = src.info_or(job.info());
        Ok(match &job.stages {
            Some(view) => {
                let st = self.stages.entry(view.clone()).or_default().clone();
                lightcraft_engine::pipeline::render_cached(&src.image, &info, &job.settings, &job.request(), &st)
            }
            None => lightcraft_engine::pipeline::render(&src.image, &info, &job.settings, &job.request()),
        })
    }
}

/// The web thumbnail cache's index (main thread): size and last use of every stored thumbnail,
/// pruned like the native disk cache (least recently used first, down to 80 % of the budget).
#[derive(Default, Serialize, Deserialize)]
pub struct ThumbIndex {
    /// hex key → (bytes, last use).
    entries: HashMap<String, (u64, u64)>,
    clock: u64,
    #[serde(skip)]
    total: u64,
    #[serde(skip)]
    pub dirty: bool,
}

impl ThumbIndex {
    pub fn from_json(bytes: &[u8]) -> ThumbIndex {
        let mut i: ThumbIndex = serde_json::from_slice(bytes).unwrap_or_default();
        i.total = i.entries.values().map(|e| e.0).sum();
        i
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Is `hex` stored? Counts as a use.
    pub fn touch(&mut self, hex: &str) -> bool {
        self.clock += 1;
        let clock = self.clock;
        match self.entries.get_mut(hex) {
            Some(e) => {
                e.1 = clock;
                self.dirty = true;
                true
            }
            None => false,
        }
    }

    pub fn contains(&self, hex: &str) -> bool {
        self.entries.contains_key(hex)
    }

    pub fn insert(&mut self, hex: &str, bytes: u64) {
        self.clock += 1;
        if let Some(old) = self.entries.insert(hex.to_string(), (bytes, self.clock)) {
            self.total -= old.0;
        }
        self.total += bytes;
        self.dirty = true;
    }

    pub fn remove(&mut self, hex: &str) {
        if let Some(old) = self.entries.remove(hex) {
            self.total -= old.0;
            self.dirty = true;
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Over `budget`: drop the least recently used entries down to 80 % of it. Returns the keys
    /// whose files should be deleted.
    pub fn prune(&mut self, budget: u64) -> Vec<String> {
        if self.total <= budget {
            return Vec::new();
        }
        let target = budget / 5 * 4;
        let mut by_age: Vec<(String, u64, u64)> = self.entries.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect();
        by_age.sort_by_key(|e| e.2);
        let mut out = Vec::new();
        for (k, _, _) in by_age {
            if self.total <= target {
                break;
            }
            self.remove(&k);
            out.push(k);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_engine::Session;

    #[test]
    fn wire_job_renders_like_the_engine() {
        let mut s = Session::with_demo();
        let id = s.visible_cloned()[0];
        s.execute("library.select", &serde_json::json!({"ids": [id.0]})).unwrap();
        s.execute("develop.set", &serde_json::json!({"control": "light.exposure", "value": 0.7})).unwrap();
        let job = s.thumb_job(id, 128).unwrap();
        let wire = WireJob::from_job(&job, false, "Thumb");
        // through JSON, as it crosses the worker boundary
        let wire: WireJob = serde_json::from_str(&serde_json::to_string(&wire).unwrap()).unwrap();
        assert!(wire.thumb.is_some());
        let mut core = WorkerCore::default();
        assert_eq!(core.needs_original(&wire), None, "demo scenes are generated");
        let a = core.render(&wire, None).unwrap();
        let b = job.run().rendered.unwrap();
        assert_eq!((a.image.width, a.image.height), (b.image.width, b.image.height));
        assert_eq!(a.image.data, b.image.data);
        assert_eq!(a.histogram, b.histogram);
    }

    #[test]
    fn worker_reads_originals_once() {
        let bytes = crate::store::png_bytes(64, 48);
        let hash = crate::store::content_hash(&bytes);
        let origin = Source::File { path: crate::store::original_path(&hash, "a.png") };
        let job = WireJob {
            level: SourceLevel::Thumb,
            origin,
            max_edge: 512,
            raw: false,
            relative_wb: false,
            as_shot_temp: 6500.0,
            as_shot_tint: 0.0,
            lens: None,
            settings: Arc::new(DevelopSettings::default()),
            max_w: 32,
            max_h: 32,
            draft: false,
            apply_crop: true,
            thumb: None,
            thumb_cached: false,
            stages: Some("Main".into()),
            overlay: (0, 0.0),
        };
        let mut core = WorkerCore::default();
        assert_eq!(core.needs_original(&job).as_deref(), Some(hash.as_str()));
        assert!(core.render(&job, None).is_err());
        let r = core.render(&job, Some(&bytes)).unwrap();
        assert_eq!(r.image.width, 32);
        assert_eq!(core.needs_original(&job), None, "decoded source is kept");
    }

    #[test]
    fn thumb_index_prunes_lru() {
        let mut i = ThumbIndex::default();
        for k in ["a", "b", "c", "d"] {
            i.insert(k, 100);
        }
        assert!(i.touch("a"));
        assert!(!i.touch("zz"));
        assert!(i.prune(1000).is_empty());
        let gone = i.prune(300); // 400 > 300 → down to 240
        assert_eq!(gone, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(i.total(), 200);
        let j = ThumbIndex::from_json(&i.to_json());
        assert_eq!((j.len(), j.total()), (2, 200));
        assert!(j.contains("a") && j.contains("d"));
    }
}
