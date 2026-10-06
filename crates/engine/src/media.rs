//! Source proxies and render jobs.
//!
//! Sources are decoded (or generated) once per level and cached in memory LRUs: `Thumb` (≤ 512 px
//! long edge) for the grid and filmstrip, `Preview` (≤ 2560 px) for the loupe, `Full` (native
//! resolution, the last one only) for exports larger than a preview. A [`RenderJob`] is
//! self-contained and `Send`: frontends run it on a worker thread; if the source wasn't cached yet
//! the job loads it and hands it back in the [`RenderResult`] so the cache can keep it.
//!
//! Rendered thumbnails are cached too ([`lightcraft_preview::PreviewCache`]: memory LRU, plus a
//! disk cache in the library's `thumbs/` folder), keyed by the photo's content hash, the develop
//! settings hash, the size and [`RENDER_CACHE_VERSION`] — so reopening a library shows its grid
//! without decoding a single original, and an edit simply produces a new key.
//!
//! Opening a photo shows something at once ([`QuickJob`]): the loupe's last render for the current
//! settings (kept in the same cache under a size-independent "view" key), else the camera's
//! embedded JPEG for an unedited raw, else a thumbnail-level render — while the real render is
//! prepared. Grid thumbnails of unedited raws likewise start from the embedded preview.

use std::sync::Arc;

use lightcraft_catalog::{MediaKind, Photo, PhotoId, Source};
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{Quality, RenderRequest, Rendered, SourceInfo, StageCache};
use lightcraft_preview::{Hash128, Hasher128, Lru, PreviewCache};
use lightcraft_raster::{Histogram, Rgb32f, Rgba8};
use serde::{Deserialize, Serialize};

/// Bump when the pipeline's output changes, to invalidate cached thumbnails.
pub const RENDER_CACHE_VERSION: u64 = 7;

/// Thumbnails render at one of these long edges (so window/cell size changes reuse the cache).
pub const THUMB_SIZES: [usize; 4] = [128, 256, 384, 512];

/// Memory budgets.
const THUMB_SOURCE_BYTES: usize = 384 << 20;
const RENDERED_MEM_BYTES: usize = 128 << 20;
/// Disk budget for the thumbnail cache.
pub const DISK_CACHE_BYTES: u64 = 2 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLevel {
    Thumb,
    Preview,
    /// The original at its native resolution (exports larger than a preview).
    Full,
}

impl SourceLevel {
    pub fn max_edge(self) -> usize {
        match self {
            SourceLevel::Thumb => 512,
            SourceLevel::Preview => 2560,
            SourceLevel::Full => usize::MAX,
        }
    }
    /// Smallest level that can serve an output of `long_edge` pixels.
    pub fn for_size(long_edge: usize) -> SourceLevel {
        match long_edge {
            0..=520 => SourceLevel::Thumb,
            521..=2560 => SourceLevel::Preview,
            _ => SourceLevel::Full,
        }
    }
}

/// Decodes a file into a linear Rec.2020 image no larger than `max_edge` (set by the app).
pub type FileLoader = Arc<dyn Fn(&str, usize) -> Result<(Rgb32f, SourceInfo), String> + Send + Sync>;

/// A raw file's embedded (camera-rendered) preview as display sRGB, oriented, no larger than
/// `max_edge` (set by the app). `None`: no usable preview.
pub type PreviewLoader = Arc<dyn Fn(&str, usize) -> Option<Rgba8> + Send + Sync>;

/// Pixels and the source interpretation learned while decoding; they must be cached together.
#[derive(Clone)]
pub struct DecodedSource {
    pub image: Arc<Rgb32f>,
    pub info: Option<SourceInfo>,
    /// A smart preview's stored camera tone curve: the one decoder fact its pixels need that the
    /// catalog's header facts lack (used when `info` is `None`).
    pub camera_tone: Option<lightcraft_pipeline::tone::CameraTone>,
}

impl DecodedSource {
    pub fn new(image: Arc<Rgb32f>, info: Option<SourceInfo>) -> Self {
        DecodedSource { image, info, camera_tone: None }
    }

    /// What to render these pixels against: the decoder's facts, else `header` (the catalog's)
    /// with any stored camera tone curve.
    pub fn info_or(&self, header: SourceInfo) -> SourceInfo {
        self.info.unwrap_or(SourceInfo { camera_tone: self.camera_tone.or(header.camera_tone), ..header })
    }
}

#[derive(Clone)]
pub enum SourceRef {
    Loaded(Box<DecodedSource>),
    Demo {
        scene: Box<lightcraft_scenes::Scene>,
        max_edge: usize,
    },
    File {
        path: String,
        max_edge: usize,
        loader: Option<FileLoader>,
        /// The photo's smart preview, used when the original can't be read (offline drive). The
        /// UI thread never checks whether the original is there; the render worker finds out.
        fallback: Option<std::path::PathBuf>,
    },
    /// A smart preview standing in for a missing original.
    Smart {
        path: std::path::PathBuf,
    },
}

impl SourceRef {
    pub fn load(&self) -> Result<Arc<Rgb32f>, String> {
        self.load_source().map(|s| s.image)
    }

    pub fn load_source(&self) -> Result<DecodedSource, String> {
        let image = match self {
            SourceRef::Loaded(a) => return Ok((**a).clone()),
            SourceRef::Demo { scene, max_edge } => Arc::new(scene.render_fit(*max_edge)),
            SourceRef::File { path, max_edge, loader, fallback } => {
                let r = match loader {
                    Some(l) => l(path, *max_edge).map(|(image, info)| DecodedSource::new(Arc::new(image), Some(info))),
                    None => Err(format!("no decoder available for {path}")),
                };
                // An offline original renders from its smart preview (no decoder facts: header ones).
                return match (r, fallback) {
                    #[cfg(not(target_arch = "wasm32"))]
                    (Err(e), Some(sp)) if crate::smart::is_valid(sp) => crate::smart::load(sp).map_err(|e2| format!("{e}; smart preview: {e2}")),
                    (r, _) => r,
                };
            }
            #[cfg(not(target_arch = "wasm32"))]
            SourceRef::Smart { path } => return crate::smart::load(path),
            #[cfg(target_arch = "wasm32")]
            SourceRef::Smart { .. } => return Err("smart previews are not available here".into()),
        };
        Ok(DecodedSource::new(image, None))
    }
}

pub struct MediaCache {
    /// Decoded thumbnail-level sources (LRU by bytes).
    thumbs: Lru<PhotoId, DecodedSource>,
    /// Decoded preview-level sources with their last use ([`lightcraft_preview::next_tick`]).
    previews: Vec<(PhotoId, DecodedSource, u64)>,
    /// The last full-resolution original (exports; one at a time: ~300 MB at 24 MP).
    full: Option<(PhotoId, DecodedSource, u64)>,
    /// How many previews to keep (LRU).
    pub preview_capacity: usize,
    /// Bytes all decoded sources and rendered previews in memory may take together (the cache
    /// share of [`crate::memory::budget`]); the least recently used entry of any of them goes first.
    budget: usize,
    pub file_loader: Option<FileLoader>,
    pub file_probe: Option<FileProbe>,
    pub preview_loader: Option<PreviewLoader>,
    /// Reads a photo file's bytes (Photo Merge); `None` = the local file system.
    pub file_bytes: Option<crate::merge::ByteReader>,
    /// The library's smart previews folder (originals offline: render from the proxy).
    pub smart_dir: Option<std::path::PathBuf>,
    /// Whether originals (and smart previews) are on disk: cached and checked off the UI thread
    /// in the app (see [`crate::availability`]).
    pub availability: crate::availability::Availability,
    scenes: Vec<lightcraft_scenes::Scene>,
    /// Rendered thumbnails (memory, plus disk once a library is attached).
    pub rendered: Arc<PreviewCache>,
}

impl Default for MediaCache {
    fn default() -> Self {
        let budget = crate::memory::cache_share(crate::memory::budget());
        MediaCache {
            thumbs: Lru::new(THUMB_SOURCE_BYTES.min(budget)),
            previews: Vec::new(),
            full: None,
            preview_capacity: 0,
            budget,
            file_loader: None,
            file_probe: None,
            preview_loader: None,
            file_bytes: None,
            smart_dir: None,
            availability: Default::default(),
            scenes: Vec::new(),
            rendered: Arc::new(PreviewCache::memory(rendered_budget(budget))),
        }
    }
}

/// Memory for rendered previews out of the cache share.
fn rendered_budget(share: usize) -> usize {
    RENDERED_MEM_BYTES.min(share / 4)
}

fn source_bytes(img: &Rgb32f) -> usize {
    img.data.len() * 12 + 64 + std::mem::size_of::<SourceInfo>()
}

impl MediaCache {
    /// Keep rendered thumbnails on disk in `dir` as well.
    pub fn attach_disk_cache(&mut self, dir: &std::path::Path, disk_bytes: u64) {
        self.rendered = Arc::new(PreviewCache::with_disk(rendered_budget(self.budget), dir, disk_bytes));
    }

    /// Forget every decoded source (photo ids changed meaning, e.g. another library was opened).
    pub fn clear_sources(&mut self) {
        self.thumbs = Lru::new(THUMB_SOURCE_BYTES.min(self.budget));
        self.previews.clear();
        self.full = None;
    }

    /// Bytes the caches may hold together.
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Change the bytes the caches may hold together, evicting at once if needed.
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
        self.thumbs.set_budget(THUMB_SOURCE_BYTES.min(bytes));
        self.rendered.set_mem_budget(rendered_budget(bytes));
        self.enforce_budget();
    }

    /// Bytes held by decoded sources and rendered previews in memory.
    pub fn held(&self) -> usize {
        let (t, p, f) = self.usage();
        t.bytes + p.bytes + f.bytes + self.rendered.mem_usage().1
    }

    /// Evict least recently used entries across the source caches and the rendered previews
    /// until they fit the budget. The newest preview-level source (the photo on screen, or the
    /// one just decoded) is never evicted here.
    pub fn enforce_budget(&mut self) {
        let mut held = self.held();
        while held > self.budget {
            let newest_preview = self.previews.iter().map(|e| e.2).max();
            let candidates = [
                self.thumbs.oldest_tick().map(|t| (t, 0)),
                self.previews.iter().filter(|e| Some(e.2) != newest_preview).map(|e| e.2).min().map(|t| (t, 1)),
                self.full.as_ref().map(|e| (e.2, 2)),
                self.rendered.oldest_tick().map(|t| (t, 3)),
            ];
            let Some((tick, which)) = candidates.into_iter().flatten().min() else { break };
            let freed = match which {
                0 => self.thumbs.pop_oldest(),
                1 => self.previews.iter().position(|e| e.2 == tick).map(|i| source_bytes(&self.previews.remove(i).1.image)),
                2 => self.full.take().map(|e| source_bytes(&e.1.image)),
                _ => self.rendered.evict_oldest(),
            };
            match freed {
                Some(b) if b > 0 => held = held.saturating_sub(b),
                _ => break,
            }
        }
    }

    pub fn get(&mut self, id: PhotoId, level: SourceLevel) -> Option<Arc<Rgb32f>> {
        self.get_source(id, level).map(|s| s.image)
    }

    fn get_source(&mut self, id: PhotoId, level: SourceLevel) -> Option<DecodedSource> {
        match level {
            SourceLevel::Thumb => self.thumbs.get(&id).cloned(),
            SourceLevel::Preview => self.previews.iter_mut().find(|e| e.0 == id).map(|e| {
                e.2 = lightcraft_preview::next_tick();
                e.1.clone()
            }),
            SourceLevel::Full => self.full.as_mut().filter(|e| e.0 == id).map(|e| {
                e.2 = lightcraft_preview::next_tick();
                e.1.clone()
            }),
        }
    }

    /// The facts of photo `id`'s cached sources: the decoder's when one has them, else `header`
    /// (with a smart preview's stored camera tone curve).
    fn source_facts(&self, id: PhotoId, header: SourceInfo) -> SourceInfo {
        let cached = || {
            self.thumbs
                .peek(&id)
                .into_iter()
                .chain(self.previews.iter().filter(|e| e.0 == id).map(|e| &e.1))
                .chain(self.full.iter().filter(|e| e.0 == id).map(|e| &e.1))
        };
        match cached().find(|s| s.info.is_some()).or_else(|| cached().next()) {
            Some(s) => s.info_or(header),
            None => header,
        }
    }

    pub fn insert(&mut self, id: PhotoId, level: SourceLevel, img: Arc<Rgb32f>) {
        self.insert_source(id, level, DecodedSource::new(img, None));
    }

    fn insert_source(&mut self, id: PhotoId, level: SourceLevel, img: DecodedSource) {
        let tick = lightcraft_preview::next_tick();
        match level {
            SourceLevel::Thumb => {
                let cost = source_bytes(&img.image);
                self.thumbs.insert(id, img, cost);
            }
            SourceLevel::Preview => {
                self.previews.retain(|e| e.0 != id);
                self.previews.push((id, img, tick));
                let cap = if self.preview_capacity == 0 { 4 } else { self.preview_capacity };
                while self.previews.len() > cap {
                    let oldest = self.previews.iter().enumerate().min_by_key(|(_, e)| e.2).map(|(i, _)| i).unwrap_or(0);
                    self.previews.remove(oldest);
                }
            }
            SourceLevel::Full => self.full = Some((id, img, tick)),
        }
        self.enforce_budget();
    }

    /// Forget a photo's decoded sources (e.g. after its file changed).
    pub fn forget(&mut self, id: PhotoId) {
        self.thumbs.remove(&id);
        self.previews.retain(|e| e.0 != id);
        if self.full.as_ref().is_some_and(|e| e.0 == id) {
            self.full = None;
        }
    }

    /// (decoded thumbnail sources, bytes).
    pub fn source_usage(&self) -> (usize, usize) {
        (self.thumbs.len(), self.thumbs.cost())
    }

    /// Decoded sources held: (thumbnail level, preview level, full size).
    pub fn usage(&self) -> (crate::memory::Usage, crate::memory::Usage, crate::memory::Usage) {
        use crate::memory::Usage;
        let previews = Usage::new(self.previews.len(), self.previews.iter().map(|e| source_bytes(&e.1.image)).sum());
        let full = self.full.as_ref().map(|e| Usage::new(1, source_bytes(&e.1.image))).unwrap_or_default();
        (Usage::new(self.thumbs.len(), self.thumbs.cost()), previews, full)
    }

    pub fn source_ref(&mut self, p: &Photo, level: SourceLevel) -> SourceRef {
        if let Some(a) = self.get_source(p.id, level) {
            return SourceRef::Loaded(Box::new(a));
        }
        // Procedural scenes have a nominal size: "full" is that size, not unbounded.
        let max_edge = level.max_edge().min(match (&p.source, level) {
            (Source::Demo { .. }, SourceLevel::Full) => p.width.max(p.height).max(1) as usize,
            _ => usize::MAX,
        });
        // the original is offline: its smart preview, when there is one. Only cached answers are
        // used here (no file-system call on the UI thread in the app); an original not known to
        // be offline is tried first and its smart preview is the render worker's fallback.
        #[cfg(not(target_arch = "wasm32"))]
        if let (Source::File { path }, Some(dir)) = (&p.source, &self.smart_dir) {
            let sp = dir.join(crate::smart::file_name(p));
            // (a proxy cut short fails to load, like the offline original; the worker's fallback
            // in `SourceRef::load` only uses a proxy that passes `smart::is_valid`)
            if self.availability.is_offline(path) && self.availability.exists(&sp.to_string_lossy()) == Some(true) {
                return SourceRef::Smart { path: sp };
            }
            let mut r = self.origin_ref(&p.source, max_edge);
            if let SourceRef::File { fallback, .. } = &mut r {
                *fallback = Some(sp);
            }
            return r;
        }
        self.origin_ref(&p.source, max_edge)
    }

    /// How to load `origin` at most `max_edge` pixels long, ignoring decoded sources in memory (a
    /// render worker in another wasm instance builds its sources from this).
    pub fn origin_ref(&mut self, origin: &Source, max_edge: usize) -> SourceRef {
        match origin {
            Source::Demo { scene } => {
                if self.scenes.is_empty() {
                    self.scenes = lightcraft_scenes::demo_library();
                }
                match self.scenes.iter().find(|s| s.id == *scene) {
                    Some(s) => SourceRef::Demo { scene: Box::new(s.clone()), max_edge },
                    None => SourceRef::File { path: format!("demo:{scene}"), max_edge, loader: None, fallback: None },
                }
            }
            Source::File { path } => SourceRef::File { path: path.clone(), max_edge, loader: self.file_loader.clone(), fallback: None },
        }
    }
}

/// Everything needed to render one photo, detached from the session.
#[derive(Clone)]
pub struct RenderJob {
    pub photo: PhotoId,
    pub level: SourceLevel,
    pub source: SourceRef,
    /// Where the photo's pixels come from (for executors that can't share `source`, e.g. a web
    /// worker).
    pub origin: Source,
    pub info: SourceInfo,
    pub settings: Arc<DevelopSettings>,
    pub request: RenderRequest,
    /// Identifies the result for caching: hash of settings + request.
    pub key: u64,
    /// Rendered-thumbnail cache and this job's key in it.
    pub cache: Option<(Arc<PreviewCache>, Hash128)>,
    /// Intermediate results of this view's previous renders (set by the frontend for the loupe):
    /// slider drags then only redo the stages the changed setting feeds.
    pub stages: Option<Arc<StageCache>>,
    /// Keep a full-quality result here as the photo's view preview ([`crate::Session::loupe_job`]).
    pub view_cache: Option<(Arc<PreviewCache>, Hash128)>,
}

pub struct RenderResult {
    pub photo: PhotoId,
    pub level: SourceLevel,
    pub key: u64,
    pub rendered: Result<Rendered, String>,
    /// A source that was loaded by this job (to be inserted into the cache).
    pub loaded: Option<DecodedSource>,
    /// Set for a [`QuickJob`]'s stand-in: where the image came from.
    pub quick: Option<QuickSource>,
}

impl RenderJob {
    /// Render at draft quality (interactive drags). Gets its own result key.
    pub fn draft(mut self) -> Self {
        if self.request.quality != Quality::Draft {
            self.request.quality = Quality::Draft;
            self.key ^= 0x9e37_79b9_7f4a_7c15;
        }
        self
    }

    /// Draw a diagnostic overlay over the result (e.g. Point Color's visualized range). Gets its
    /// own result key.
    pub fn with_overlay(mut self, overlay: lightcraft_pipeline::Overlay) -> Self {
        if self.request.overlay != overlay {
            self.key ^= self.request.overlay.key().wrapping_mul(0x9e37_79b9_7f4a_7c15);
            self.request.overlay = overlay;
            self.key ^= overlay.key().wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
        // a diagnostic view (visualized range / spots) must never become the photo's cached preview
        if overlay != lightcraft_pipeline::Overlay::None {
            self.view_cache = None;
        }
        self
    }

    /// Soft-proof the result (see [`lightcraft_pipeline::Proof`]). Gets its own result key and is
    /// never kept as the photo's cached preview.
    pub fn with_proof(mut self, proof: Option<lightcraft_pipeline::Proof>) -> Self {
        if self.request.proof != proof {
            let k = |p: Option<lightcraft_pipeline::Proof>| p.map_or(0, |p| p.key()).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
            self.key ^= k(self.request.proof) ^ k(proof);
            self.request.proof = proof;
        }
        if proof.is_some() {
            self.view_cache = None;
        }
        self
    }

    /// Reuse `stages` across this view's renders.
    pub fn with_stages(mut self, stages: Arc<StageCache>) -> Self {
        self.stages = Some(stages);
        self
    }

    pub fn run(self) -> RenderResult {
        if let Some((cache, key)) = &self.cache
            && let Some(img) = cache.get(*key)
        {
            let image = Arc::unwrap_or_clone(img);
            let histogram = Histogram::of_srgb8(&image);
            return RenderResult {
                photo: self.photo,
                level: self.level,
                key: self.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                loaded: None,
                quick: None,
            };
        }
        let was_loaded = matches!(self.source, SourceRef::Loaded(_));
        match self.source.load_source() {
            Ok(source) => {
                let src = &source.image;
                let info = source.info_or(self.info);
                // Thumbnails (many small jobs side by side) stay on the CPU; views and exports use
                // the GPU when there is one.
                let gpu = self.cache.is_none();
                let rendered = develop(src, &info, &self.settings, &self.request, self.stages.as_deref(), gpu);
                if let Some((cache, key)) = &self.cache {
                    cache.put(*key, Arc::new(rendered.image.clone()));
                }
                if let Some((cache, key)) = &self.view_cache
                    && self.request.quality == Quality::Full
                {
                    cache.put_deferred(*key, Arc::new(rendered.image.clone()));
                }
                RenderResult {
                    photo: self.photo,
                    level: self.level,
                    key: self.key,
                    rendered: Ok(rendered),
                    loaded: (!was_loaded).then_some(source),
                    quick: None,
                }
            }
            Err(e) => RenderResult { photo: self.photo, level: self.level, key: self.key, rendered: Err(e), loaded: None, quick: None },
        }
    }
}

/// Render `src`: on the GPU when `gpu` is set and a GPU is available (`lightcraft_gpu`), else on
/// the CPU, reusing `stages` either way. Both produce the same image within 1–3 LSB (see
/// `docs/gpu-pipeline.md`); `LIGHTCRAFT_GPU=0` or [`lightcraft_gpu::set_enabled`] forces the CPU.
pub fn develop(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, stages: Option<&StageCache>, gpu: bool) -> Rendered {
    // LUT profiles have no GPU stage: they render on the CPU
    if gpu
        && !lightcraft_pipeline::lut::is_lut_profile(&s.profile.id)
        && !s.masks.iter().any(|m| m.visible && m.refine > 0.0)
        && req.proof.is_none()
        && let Some(r) = lightcraft_gpu::render(src, info, s, req, stages)
    {
        return r;
    }
    match stages {
        Some(st) => lightcraft_pipeline::render_cached(src, info, s, req, st),
        None => lightcraft_pipeline::render(src, info, s, req),
    }
}

/// Where a [`QuickJob`]'s stand-in image came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuickSource {
    /// The photo's own develop render for its current settings (cached view or thumbnail render).
    Cached,
    /// The camera's embedded JPEG (unedited raws).
    Embedded,
    /// A thumbnail-level develop render made for the occasion.
    Small,
}

/// Something to show *now* for a photo while its real render is prepared. Tried in order: cached
/// renders, the embedded preview of an unedited raw, a thumbnail-level render.
#[derive(Clone)]
pub struct QuickJob {
    pub photo: PhotoId,
    pub key: u64,
    /// Cached renders to try first, in order: (cache, key).
    pub cached: Vec<(Arc<PreviewCache>, Hash128)>,
    /// Embedded preview of an unedited raw: (path, loader, max edge).
    pub embedded: Option<(String, PreviewLoader, usize)>,
    /// Last resort: run this thumbnail job.
    pub small: Option<Box<RenderJob>>,
}

impl QuickJob {
    pub fn run(self) -> RenderResult {
        let (photo, key) = (self.photo, self.key);
        let done = |rendered: Result<Rendered, String>, quick| RenderResult { photo, level: SourceLevel::Thumb, key, rendered, loaded: None, quick };
        let rendered = |image: Rgba8| {
            let histogram = Histogram::of_srgb8(&image);
            Ok(Rendered { image, histogram, deep: None })
        };
        for (cache, k) in &self.cached {
            if let Some(img) = cache.get(*k) {
                return done(rendered(Arc::unwrap_or_clone(img)), Some(QuickSource::Cached));
            }
        }
        if let Some((path, loader, edge)) = &self.embedded
            && let Some(image) = loader(path, *edge)
        {
            return done(rendered(image), Some(QuickSource::Embedded));
        }
        if let Some(job) = self.small {
            let r = job.run();
            return RenderResult { key, quick: Some(QuickSource::Small), ..r };
        }
        done(Err("no quick preview".into()), None)
    }
}

/// What identifies a photo's pixels for caching: its content hash, else its source.
pub fn content_key(p: &Photo) -> String {
    match (&p.content_hash, &p.source) {
        (Some(h), _) => h.clone(),
        (None, Source::Demo { scene }) => format!("demo:{scene}"),
        (None, Source::File { path }) => format!("file:{path}:{}", p.file_size),
    }
}

pub fn source_info(p: &Photo) -> SourceInfo {
    // Procedural demo scenes are scene-referred HDR (like raw files): use the filmic tone map.
    if matches!(p.source, Source::Demo { .. }) {
        return SourceInfo { raw: true, ..Default::default() };
    }
    // A raw shown from its embedded preview is a rendered (display-referred) JPEG: relative white
    // balance and the display tone curve, like any other rendered file.
    if p.develops_raw() {
        let (temp, tint) = if p.relative_wb() { (6500.0, 0.0) } else { p.as_shot_wb.unwrap_or((5500.0, 0.0)) };
        SourceInfo { raw: true, as_shot_temp: temp, as_shot_tint: tint, lens: p.embedded_lens, relative_wb: p.relative_wb(), ..Default::default() }
    } else {
        SourceInfo::default()
    }
}

impl crate::Session {
    /// Build a render job for `id` fitting `max_w × max_h`. `before` renders the unedited look.
    pub fn render_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, before: bool, apply_crop: bool) -> Option<RenderJob> {
        self.build_job(id, max_w, max_h, before, apply_crop, None)
    }

    /// A grid/filmstrip thumbnail job: the long edge is rounded up to one of [`THUMB_SIZES`]
    /// (≥ `long_edge`) and the result comes from / goes to the thumbnail cache (memory + disk).
    pub fn thumb_job(&mut self, id: PhotoId, long_edge: usize) -> Option<RenderJob> {
        let b = THUMB_SIZES.iter().copied().find(|s| *s >= long_edge).unwrap_or(THUMB_SIZES[THUMB_SIZES.len() - 1]);
        self.build_job(id, b, b, false, true, Some(b))
    }

    fn build_job(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        before: bool,
        apply_crop: bool,
        thumb_bucket: Option<usize>,
    ) -> Option<RenderJob> {
        let p = self.catalog.photo(id)?.clone();
        let level = SourceLevel::for_size(max_w.max(max_h));
        let source = self.media.source_ref(&p, level);
        let settings = if before { Arc::new(self.before_settings(&p)) } else { p.develop.clone() };
        let request = RenderRequest { apply_crop, ..RenderRequest::fit(max_w, max_h) };
        // the photo id is part of the key: two photos with the same settings and size must not
        // share a result (a view slot showing photo A would otherwise look current for photo B)
        // …and so is the file's content: a file changed on disk (Reload) renders afresh
        let content = Hasher128::new().str(&content_key(&p)).finish().0 as u64;
        let key = settings.hash64()
            ^ ((max_w as u64) << 40)
            ^ ((max_h as u64) << 20)
            ^ (apply_crop as u64)
            ^ (level as u64) << 60
            ^ id.0.wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ content.rotate_left(17);
        let cache = thumb_bucket.map(|b| {
            let k = Hasher128::new().str(&content_key(&p)).u64(settings.hash64()).u64(b as u64).u64(RENDER_CACHE_VERSION).finish();
            (self.media.rendered.clone(), k)
        });
        Some(RenderJob {
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: source_info(&p),
            settings,
            request,
            key,
            cache,
            stages: None,
            view_cache: None,
        })
    }

    /// A render of `id` with `settings` in place of its own, fitting `max_w × max_h` (the loupe's
    /// temporary preview while hovering a preset or profile; nothing is committed or cached).
    pub fn preview_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, apply_crop: bool, settings: &DevelopSettings) -> Option<RenderJob> {
        let mut job = self.render_job(id, max_w, max_h, false, apply_crop)?;
        job.settings = Arc::new(settings.clone());
        job.key = settings.hash64() ^ ((max_w as u64) << 40) ^ ((max_h as u64) << 20) ^ (apply_crop as u64) ^ (job.level as u64) << 60;
        Some(job)
    }

    /// Cache key of a variant thumbnail ([`Self::variant_job`]).
    pub fn variant_key(p: &Photo, settings: &DevelopSettings, edge: usize) -> Hash128 {
        Hasher128::new().str(&content_key(p)).str("variant").u64(settings.hash64()).u64(edge as u64).u64(RENDER_CACHE_VERSION).finish()
    }

    /// A thumbnail of `id` rendered with `settings` instead of its own (profile and preset
    /// browsers): from the thumbnail-level source, long edge `edge` (≤ 512), cropped. The result
    /// is cached (memory + the library's disk cache) under the photo's content and the settings
    /// hash, so a variant renders once; `key` is derived from the same hash, so a frontend can
    /// keep one texture per variant.
    pub fn variant_job(&mut self, id: PhotoId, settings: &DevelopSettings, edge: usize) -> Option<RenderJob> {
        let p = self.catalog.photo(id)?.clone();
        let edge = edge.clamp(16, SourceLevel::Thumb.max_edge());
        let level = SourceLevel::Thumb;
        let source = self.media.source_ref(&p, level);
        let ck = Self::variant_key(&p, settings, edge);
        Some(RenderJob {
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: source_info(&p),
            settings: Arc::new(settings.clone()),
            request: RenderRequest { apply_crop: true, ..RenderRequest::fit(edge, edge) },
            key: (ck.0 as u64) ^ ((ck.0 >> 64) as u64),
            cache: Some((self.media.rendered.clone(), ck)),
            stages: None,
            view_cache: None,
        })
    }

    /// Size-independent cache key of a photo's view render (loupe) for its current settings.
    fn view_key(p: &Photo, apply_crop: bool) -> Hash128 {
        Hasher128::new().str(&content_key(p)).str("view").u64(p.develop.hash64()).u64(apply_crop as u64).u64(RENDER_CACHE_VERSION).finish()
    }

    /// The loupe's render job: like [`Self::render_job`], and a full-quality result is kept as the
    /// photo's view preview (memory, and disk with a library) for [`Self::quick_view_job`].
    pub fn loupe_job(&mut self, id: PhotoId, max_w: usize, max_h: usize, apply_crop: bool) -> Option<RenderJob> {
        let mut job = self.render_job(id, max_w, max_h, false, apply_crop)?;
        let p = self.catalog.photo(id)?;
        job.view_cache = Some((self.media.rendered.clone(), Self::view_key(p, apply_crop)));
        Some(job)
    }

    /// The embedded preview of an unedited raw (path, loader), when the app installed a loader.
    fn embedded_of(&self, p: &Photo) -> Option<(String, PreviewLoader)> {
        match (&p.source, &self.media.preview_loader) {
            (Source::File { path }, Some(l)) if p.kind == MediaKind::Raw && crate::import::has_import_look(p) => Some((path.clone(), l.clone())),
            _ => None,
        }
    }

    /// Something to show in the loupe right away for `id` (see [`QuickJob`]): its cached view
    /// render, else the embedded preview of an unedited raw, else a cached or fresh thumbnail.
    pub fn quick_view_job(&mut self, id: PhotoId, max_edge: usize, apply_crop: bool) -> Option<QuickJob> {
        let p = self.catalog.photo(id)?.clone();
        let small = self.thumb_job(id, THUMB_SIZES[THUMB_SIZES.len() - 1])?;
        // (the thumbnail job itself starts with the cached thumbnail, after the sharper embedded preview)
        let cached = vec![(self.media.rendered.clone(), Self::view_key(&p, apply_crop))];
        let h = Hasher128::new().str(&content_key(&p)).str("quick").u64(p.develop.hash64()).u64(apply_crop as u64).finish();
        let key = h.0 as u64;
        let embedded = self.embedded_of(&p).map(|(path, l)| (path, l, max_edge.clamp(1, SourceLevel::Preview.max_edge())));
        Some(QuickJob { photo: id, key, cached, embedded, small: apply_crop.then(|| Box::new(small)) })
    }

    /// A first grid thumbnail for an unedited raw: the cached render for `job` (the real
    /// thumbnail job, from [`Self::thumb_job`]) if there is one — then the result carries
    /// [`QuickSource::Cached`] and `job.key` and is final — else its embedded preview. `None` for
    /// other photos (render them directly).
    pub fn quick_thumb_job(&mut self, job: &RenderJob) -> Option<QuickJob> {
        let p = self.catalog.photo(job.photo)?.clone();
        let (path, l) = self.embedded_of(&p)?;
        let edge = job.request.max_w.max(job.request.max_h);
        Some(QuickJob {
            photo: job.photo,
            key: job.key,
            cached: job.cache.clone().into_iter().collect(),
            embedded: Some((path, l, edge)),
            small: None,
        })
    }

    /// Accept a finished job's loaded source into the cache.
    pub fn accept(&mut self, r: &RenderResult) {
        if let Some(src) = &r.loaded {
            self.media.insert_source(r.photo, r.level, src.clone());
        }
    }

    /// Synchronous render (CLI, MCP, tests).
    pub fn render_now(&mut self, id: PhotoId, max_w: usize, max_h: usize) -> Result<Rendered, String> {
        let job = self.render_job(id, max_w, max_h, false, true).ok_or("no such photo")?;
        let r = job.run();
        self.accept(&r);
        r.rendered
    }

    /// Synchronous render for export: like [`Self::render_now`], into colour space `space` with
    /// sample format `depth`.
    pub fn render_export(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        space: lightcraft_pipeline::OutputSpace,
        depth: lightcraft_pipeline::OutputDepth,
    ) -> Result<Rendered, String> {
        let r = self.export_job(id, max_w, max_h, space, depth)?.run();
        self.accept(&r);
        r.rendered
    }

    /// The job [`Self::render_export`] runs, to run elsewhere (e.g. a background export thread).
    pub fn export_job(
        &mut self,
        id: PhotoId,
        max_w: usize,
        max_h: usize,
        space: lightcraft_pipeline::OutputSpace,
        depth: lightcraft_pipeline::OutputDepth,
    ) -> Result<RenderJob, String> {
        let mut job = self.render_job(id, max_w, max_h, false, true).ok_or("no such photo")?;
        job.request.space = space;
        job.request.depth = depth;
        job.key ^= (space as u64 + 1).wrapping_mul(0xa076_1d64_78bd_642f) ^ (depth as u64 + 1).wrapping_mul(0xe703_7ed1_a0b4_28db);
        Ok(job)
    }

    /// Prefer decoder facts to header-only metadata for pixel-statistics commands.
    pub fn source_info(&self, id: PhotoId) -> SourceInfo {
        let header = self.catalog.photo(id).map(|p| source_info(p)).unwrap_or_default();
        self.media.source_facts(id, header)
    }

    /// The source proxy for pixel-statistics commands (auto tone/WB), loading synchronously.
    pub fn source_now(&mut self, id: PhotoId, level: SourceLevel) -> Result<Arc<Rgb32f>, String> {
        let p = self.catalog.photo(id).ok_or("no such photo")?.clone();
        let r = self.media.source_ref(&p, level).load_source()?;
        let image = r.image.clone();
        self.media.insert_source(id, level, r);
        Ok(image)
    }
}

/// What an import learns from a file header (set by the app from `lightcraft-codecs`/`-raw`).
#[derive(Clone, Debug, Default)]
pub struct ProbeInfo {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub kind: MediaKind,
    pub file_size: u64,
    pub captured: Option<String>,
    pub meta: lightcraft_catalog::Meta,
    pub as_shot_wb: Option<(f64, f64)>,
    /// Hash of the file's bytes (hex), for duplicate detection. When it is a 32-digit
    /// [`lightcraft_preview::hash_bytes`] of the whole file (as the native probe computes it),
    /// Import → Copy verifies each copy against it instead of reading the source again.
    pub content_hash: Option<String>,
    /// Lens corrections embedded in the file (DNG opcodes).
    pub embedded_lens: Option<lightcraft_develop::EmbeddedLens>,
    /// The file's embedded XMP packet (raw/DNG files), for develop settings stored inside the file.
    pub xmp: Option<String>,
    /// A raw variant that can't be decoded yet: why. The file is described (and will be shown and
    /// edited) from its embedded preview; see [`lightcraft_catalog::Photo::preview_only`].
    pub preview_only: Option<String>,
}

pub type FileProbe = Arc<dyn Fn(&str) -> Result<ProbeInfo, String> + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_info_survives_render_jobs_cache_and_eviction() {
        let tone = lightcraft_pipeline::tone::CameraTone::new(std::array::from_fn(|i| {
            let x = 0.01 * (i + 1) as f32;
            [x, (x * 2.0).min(0.9)]
        }))
        .unwrap();
        let info = SourceInfo { raw: true, relative_wb: true, camera_tone: Some(tone), ..Default::default() };
        let mut s = crate::Session::with_demo();
        let id = s.active().unwrap();
        let mut job = s.render_job(id, 64, 64, false, true).unwrap();
        job.source = SourceRef::File {
            path: "synthetic.arw".into(),
            max_edge: 64,
            loader: Some(Arc::new(move |_, _| {
                let mut image = Rgb32f::new(64, 64);
                image.data.fill([0.1; 3]);
                Ok((image, info))
            })),
            fallback: None,
        };
        job.info = SourceInfo::default(); // Header facts cannot override decoder facts.
        job.settings = Arc::new(DevelopSettings::default());
        let r = job.clone().run();
        assert_eq!(r.loaded.as_ref().unwrap().info, Some(info));
        let expected = lightcraft_pipeline::render(&r.loaded.as_ref().unwrap().image, &info, &job.settings, &job.request);
        assert_eq!(r.rendered.as_ref().unwrap().image.data, expected.image.data);
        s.accept(&r);
        job.source = s.media.source_ref(s.catalog.photo(id).unwrap(), job.level);
        let again = job.run();
        assert!(again.loaded.is_none());
        assert_eq!(r.rendered.unwrap().image.data, again.rendered.unwrap().image.data);
        s.media.forget(id);
        assert!(s.media.get_source(id, r.level).is_none());
    }

    #[test]
    fn budget_evicts_least_recently_used_across_caches() {
        let img = |w: usize| Arc::new(Rgb32f::new(w, w));
        let mut m = MediaCache::default();
        let mb = 1 << 20;
        m.set_budget(30 * mb);
        // 3 previews of 12 MB (1000² × 12 B) don't fit 30 MB: the least recently used goes
        m.insert(PhotoId(1), SourceLevel::Preview, img(1000));
        m.insert(PhotoId(2), SourceLevel::Preview, img(1000));
        assert!(m.get(PhotoId(1), SourceLevel::Preview).is_some()); // 1 is now more recent than 2
        m.insert(PhotoId(3), SourceLevel::Preview, img(1000));
        assert!(m.get(PhotoId(2), SourceLevel::Preview).is_none());
        assert!(m.get(PhotoId(1), SourceLevel::Preview).is_some() && m.get(PhotoId(3), SourceLevel::Preview).is_some());
        // a thumbnail source and a rendered preview compete in the same budget
        m.insert(PhotoId(4), SourceLevel::Thumb, img(500));
        m.rendered.put(Hash128(7), Arc::new(lightcraft_raster::Rgba8::new(1000, 1000)));
        assert!(m.held() <= 30 * mb, "{}", m.held());
        // the newest preview (the one on screen) is never evicted, even alone over budget
        m.set_budget(mb);
        assert!(m.get(PhotoId(3), SourceLevel::Preview).is_some());
        assert_eq!(m.usage().1.count, 1);
        assert_eq!(m.usage().0.count, 0);
        assert_eq!(m.rendered.mem_usage().0, 0);
    }

    #[test]
    fn work_gate_waits_for_room() {
        use crate::memory::WorkGate;
        let g = Arc::new(WorkGate::new(100));
        let a = g.acquire(80);
        // more than fits waits until `a` is released; urgent work never waits
        let urgent = g.acquire_urgent(500);
        assert_eq!(g.usage().0, 580);
        drop(urgent);
        let g2 = g.clone();
        let t = std::thread::spawn(move || {
            let _b = g2.acquire(50);
            g2.usage().0
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(g.usage().0, 80, "the second holder is still waiting");
        drop(a);
        assert_eq!(t.join().unwrap(), 50);
        assert_eq!(g.usage().0, 0);
        // a single holder may exceed the limit
        drop(g.acquire(1000));
    }

    #[test]
    fn levels_cover_sizes_and_large_exports_use_the_original() {
        assert_eq!(SourceLevel::for_size(256), SourceLevel::Thumb);
        assert_eq!(SourceLevel::for_size(2560), SourceLevel::Preview);
        assert_eq!(SourceLevel::for_size(2561), SourceLevel::Full);
        let mut s = crate::Session::with_demo();
        let p = s.catalog.photos().next().unwrap().clone();
        let long = p.width.max(p.height) as usize;
        assert!(long > 2560, "demo photos are camera-sized");
        // a full-size export reads the source at its native size, not an upscaled preview
        let job = s.render_job(p.id, long, long, false, true).unwrap();
        assert_eq!(job.level, SourceLevel::Full);
        match &job.source {
            SourceRef::Demo { max_edge, .. } => assert_eq!(*max_edge, long),
            _ => panic!("demo source expected"),
        }
        let loupe = s.render_job(p.id, 1600, 1600, false, true).unwrap();
        assert_eq!(loupe.level, SourceLevel::Preview);
        assert_ne!(loupe.key, s.render_job(p.id, 1600, 1600, false, true).unwrap().draft().key);
        // two photos with identical settings and size never share a result key
        let plain: Vec<_> = s.catalog.photos().filter(|p| !p.is_edited()).map(|p| p.id).take(2).collect();
        let (a, b) = (s.render_job(plain[0], 1600, 1600, false, true).unwrap(), s.render_job(plain[1], 1600, 1600, false, true).unwrap());
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn variant_jobs_have_their_own_keys_and_hit_the_cache() {
        let mut s = crate::Session::with_demo();
        let id = s.active().unwrap();
        let mine = (*s.develop_of(id).unwrap()).clone();
        let with = |pid: &str| {
            let mut d = mine.clone();
            d.profile.id = pid.into();
            d
        };
        let (a, b) = (with("lc.vivid"), with("lc.bw.sepia"));
        let ja = s.variant_job(id, &a, 128).unwrap();
        let jb = s.variant_job(id, &b, 128).unwrap();
        assert_ne!(ja.key, jb.key, "keys differ per settings");
        assert_eq!(ja.key, s.variant_job(id, &a, 128).unwrap().key, "stable");
        assert_ne!(ja.key, s.variant_job(id, &a, 256).unwrap().key, "and per size");
        assert_ne!(ja.key, s.thumb_job(id, 128).unwrap().key, "not the photo's own thumbnail");
        assert_eq!(ja.level, SourceLevel::Thumb);
        let (cache, ck) = ja.cache.clone().unwrap();
        assert!(cache.get(ck).is_none());
        let r = ja.run();
        s.accept(&r);
        let first = r.rendered.unwrap().image;
        assert!(first.width.max(first.height) <= 128);
        assert!(cache.get(ck).is_some(), "rendered variants are cached");
        assert!(cache.get(jb.cache.as_ref().unwrap().1).is_none());
        // a second job for the same variant is served from the cache, pixel for pixel
        let again = s.variant_job(id, &a, 128).unwrap().run();
        assert!(again.loaded.is_none());
        assert_eq!(again.rendered.unwrap().image.as_bytes(), first.as_bytes());
        let other = jb.run().rendered.unwrap().image;
        assert_ne!(other.as_bytes(), first.as_bytes());
        // the photo's own settings never changed
        assert_eq!(*s.develop_of(id).unwrap(), mine);
        // hover previews: a loupe-sized job with other settings, its own key
        let pv = s.preview_job(id, 800, 600, true, &a).unwrap();
        assert_ne!(pv.key, s.render_job(id, 800, 600, false, true).unwrap().key);
        assert_eq!(*pv.settings, a);
    }
}
