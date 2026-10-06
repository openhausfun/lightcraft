//! Background rendering: engine [`RenderJob`]s run on a [`JobPool`]; results become textures.
//!
//! Each visible thing that needs pixels is a *slot* (a grid thumbnail, the loupe, the "before"
//! image…). Requests are deduplicated per slot (a newer request replaces a queued older one) and
//! prioritised (loupe first, then on-screen thumbnails, then prefetch); thumbnails scrolled far
//! out of view are dropped from the queue. Thumbnail jobs hit the engine's preview cache (memory +
//! disk) before rendering. On wasm the jobs run inline, one per frame, unless the host installs a
//! [`RenderOffload`] (the browser build's Web Workers): then queued jobs are handed to it as it has
//! room, and its results are collected every frame.
//!
//! Stand-ins ([`QuickJob`]s, once per photo and settings): [`Slot::Preview`] holds what the loupe
//! shows until [`Slot::Main`] has the photo's render (cached view render, embedded camera JPEG or
//! a thumbnail render); [`Slot::ThumbQuick`] holds a raw's embedded preview in the grid until its
//! rendered thumbnail arrives (a cached thumbnail found by the quick job becomes the
//! [`Slot::Thumb`] texture directly).

use std::collections::HashMap;

use lightcraft_catalog::PhotoId;
use lightcraft_engine::Session;
use lightcraft_engine::media::{QuickJob, QuickSource, RenderJob, RenderResult};
use lightcraft_engine::pipeline::StageCache;
use lightcraft_preview::JobPool;
use lightcraft_raster::Histogram;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Thumb(PhotoId),
    /// A thumbnail's stand-in (embedded preview of an unedited raw).
    ThumbQuick(PhotoId),
    Main,
    /// The loupe's stand-in until `Main` has the photo.
    Preview,
    Before,
    Compare(u8),
    /// Background preparation of a neighbouring photo (no texture: its decoded source and view
    /// render are cached by the engine). 0 = next, 1 = previous.
    Prefetch(u8),
    /// A variant thumbnail (profile / preset browsers), by its job key ([`Session::variant_job`]).
    /// Kept in a bounded LRU ([`VARIANT_TEXTURES`]).
    Variant(u64),
    /// The loupe while hovering a preset or profile: the photo with the hovered look (nothing is
    /// committed).
    Hover,
    /// An import candidate's thumbnail in the import review dialog (by index).
    Import(u32),
    /// The second window's view of the active photo.
    Second,
}

pub struct Tex {
    pub key: u64,
    pub photo: PhotoId,
    pub tex: egui::TextureHandle,
    pub size: [usize; 2],
    pub histogram: Option<Histogram>,
    pub ms: f64,
    /// Set for stand-ins: where the image came from.
    pub quick: Option<QuickSource>,
    /// CPU copy of the pixels (only with [`Renderer::keep_pixels`]; for headless screenshots).
    pub pixels: Option<std::sync::Arc<egui::ColorImage>>,
}

/// Runs render jobs outside this thread's job pool (the browser build: Web Workers, each with its
/// own wasm instance). The [`Renderer`] keeps the queue (priorities, per-slot de-duplication) and
/// hands jobs over one at a time.
pub trait RenderOffload {
    /// Start `job` if an executor is free; otherwise give it back (`Some`: it stays queued).
    fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob>;
    /// Finished jobs since the last call: (slot, result, run time in ms).
    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)>;
}

struct Queued {
    slot: Slot,
    priority: u32,
    seq: u64,
    job: RenderJob,
}

/// Variant thumbnails kept as textures (LRU).
pub const VARIANT_TEXTURES: usize = 96;
/// Priority of variant thumbnail jobs: below on-screen grid thumbnails and the loupe.
const VARIANT_PRIORITY: u32 = 6;

pub struct Renderer {
    pool: JobPool<Slot, RenderResult>,
    /// Jobs run elsewhere (see [`RenderOffload`]); `queue` holds the ones not started yet.
    offload: Option<Box<dyn RenderOffload>>,
    queue: Vec<Queued>,
    seq: u64,
    /// Slot → (key, priority) of the request in flight (queued or running).
    pending: HashMap<Slot, (u64, u32)>,
    pub textures: HashMap<Slot, Tex>,
    pub last_main_ms: f64,
    /// Jobs finished since start (for inspect/perf).
    pub completed: u64,
    /// Keep a CPU copy of every texture so the UI can be rasterized headlessly (set when the app
    /// is driven by the control channel).
    pub keep_pixels: bool,
    /// Per-view intermediate results (loupe and "before"), so slider drags redo only what changed.
    stages: HashMap<Slot, Arc<StageCache>>,
    /// Slot → key of the last quick job requested for it (each is tried once).
    quick_tried: HashMap<Slot, u64>,
    /// Jobs that failed (unreadable or missing file…), by slot and job key, with the error: not
    /// requested again until the key changes (an edit, another size) — the grid asks every frame.
    failed: HashMap<Slot, (u64, String, u64)>,
    /// The catalog revision seen at the last poll: a failure is retried once the catalog changes
    /// (a relinked or re-imported file).
    catalog_rev: u64,
    /// Prefetch slot → key of the last job submitted (each runs once).
    prefetched: HashMap<Slot, u64>,
    /// Variant key → frame it was last asked for (LRU of [`Slot::Variant`] textures).
    variant_used: HashMap<u64, u64>,
    /// Frames polled so far.
    frame: u64,
    /// Since when nothing has been pending, and whether the GPU pool was trimmed since.
    #[cfg(not(target_arch = "wasm32"))]
    idle: Option<(std::time::Instant, bool)>,
}

/// After this long without renders the GPU renderer's pool of recycled buffers is freed.
#[cfg(not(target_arch = "wasm32"))]
const IDLE_TRIM: std::time::Duration = std::time::Duration::from_secs(3);

impl Default for Renderer {
    fn default() -> Self {
        let threads = if cfg!(target_arch = "wasm32") { 0 } else { JobPool::<Slot, RenderResult>::default_threads().min(6) };
        Renderer {
            pool: JobPool::new(threads),
            offload: None,
            queue: Vec::new(),
            seq: 0,
            pending: HashMap::new(),
            textures: HashMap::new(),
            last_main_ms: 0.0,
            completed: 0,
            keep_pixels: false,
            stages: HashMap::new(),
            quick_tried: HashMap::new(),
            failed: HashMap::new(),
            catalog_rev: 0,
            prefetched: HashMap::new(),
            variant_used: HashMap::new(),
            frame: 0,
            #[cfg(not(target_arch = "wasm32"))]
            idle: None,
        }
    }
}

impl Renderer {
    /// Run jobs through `offload` from now on (instead of the job pool / inline).
    pub fn set_offload(&mut self, offload: Box<dyn RenderOffload>) {
        self.offload = Some(offload);
    }

    fn is_queued(&self, slot: Slot) -> bool {
        self.queue.iter().any(|q| q.slot == slot) || self.pool.is_queued(slot)
    }

    /// Is the slot's current texture (or pending request) already for `key`?
    /// Key of what `slot` shows or is rendering next (the pending request wins).
    pub fn wanted(&self, slot: Slot) -> Option<u64> {
        self.pending.get(&slot).map(|p| p.0).or_else(|| self.textures.get(&slot).map(|t| t.key))
    }

    pub fn is_current(&self, slot: Slot, key: u64) -> bool {
        self.textures.get(&slot).is_some_and(|t| t.key == key) || self.pending.get(&slot).is_some_and(|p| p.0 == key)
    }

    /// Request a render for `slot` (no-op if already current or pending at the same priority).
    pub fn request(&mut self, slot: Slot, job: RenderJob, priority: u32) {
        if self.textures.get(&slot).is_some_and(|t| t.key == job.key)
            || self.failed.get(&slot).is_some_and(|f| f.0 == job.key && f.2 == self.catalog_rev)
        {
            return;
        }
        if let Some(&(key, prio)) = self.pending.get(&slot)
            && key == job.key
            && (prio == priority || !self.is_queued(slot))
        {
            return;
        }
        self.pending.insert(slot, (job.key, priority));
        // (an offload keeps its own per-view stage caches; the flag tells it to)
        let job =
            if matches!(slot, Slot::Main | Slot::Before | Slot::Hover) { job.with_stages(self.stages.entry(slot).or_default().clone()) } else { job };
        if self.offload.is_some() {
            self.seq += 1;
            self.queue.retain(|q| q.slot != slot);
            self.queue.push(Queued { slot, priority, seq: self.seq, job });
            self.dispatch(); // an idle worker starts now rather than next frame
            return;
        }
        let key = job.key;
        let background = matches!(slot, Slot::Thumb(_) | Slot::ThumbQuick(_) | Slot::Prefetch(_));
        self.pool.submit(
            slot,
            key,
            priority,
            Box::new(move || if background { lightcraft_engine::memory::in_background(|| job.run()) } else { job.run() }),
        );
    }

    /// Request a stand-in for `slot` (once per job key).
    pub fn request_quick(&mut self, slot: Slot, job: QuickJob, priority: u32) {
        if self.quick_tried.get(&slot) == Some(&job.key) {
            return;
        }
        self.quick_tried.insert(slot, job.key);
        self.pending.insert(slot, (job.key, priority));
        let key = job.key;
        self.pool.submit(slot, key, priority, Box::new(move || job.run()));
    }

    /// Prepare a photo in the background (a [`Slot::Prefetch`] job runs once per key; a newer one
    /// replaces it while queued). Memory stays bounded by the engine's source caches.
    pub fn prefetch(&mut self, slot: Slot, job: RenderJob, priority: u32) {
        if self.prefetched.get(&slot) == Some(&job.key) {
            return;
        }
        self.prefetched.insert(slot, job.key);
        self.pending.insert(slot, (job.key, priority));
        let key = job.key;
        self.pool.submit(slot, key, priority, Box::new(move || lightcraft_engine::memory::in_background(|| job.run())));
    }

    /// Is a request for `slot` queued or running?
    /// Drop every texture and cached stage (another library was opened: photo ids changed meaning).
    pub fn forget_all(&mut self) {
        self.textures.clear();
        self.stages.clear();
        self.quick_tried.clear();
        self.failed.clear();
        self.prefetched.clear();
        self.pending.clear();
        self.queue.clear();
    }

    pub fn is_pending(&self, slot: Slot) -> bool {
        self.pending.contains_key(&slot)
    }

    /// The texture to show for a grid/filmstrip thumbnail: the rendered one, else its stand-in.
    pub fn thumb(&self, id: PhotoId) -> Option<&Tex> {
        self.textures.get(&Slot::Thumb(id)).or_else(|| self.textures.get(&Slot::ThumbQuick(id)))
    }

    pub fn queued(&self) -> usize {
        self.pool.queued() + self.queue.len()
    }

    /// Hand queued jobs to the offload (best first) while it has room.
    fn dispatch(&mut self) {
        let Some(off) = self.offload.as_mut() else { return };
        while let Some(i) = self.queue.iter().enumerate().max_by_key(|(_, q)| (q.priority, q.seq)).map(|(i, _)| i) {
            let q = self.queue.swap_remove(i);
            if let Some(job) = off.try_start(q.slot, q.job) {
                self.queue.push(Queued { job, ..q });
                break;
            }
        }
    }

    /// Requests queued or running.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Why the last render for `slot` failed (until a render with another key succeeds).
    pub fn failure(&self, slot: Slot) -> Option<&str> {
        self.failed.get(&slot).map(|f| f.1.as_str())
    }

    /// The slots with a render pending (diagnostics: `ui.inspect`).
    pub fn pending_slots(&self) -> Vec<String> {
        self.pending.keys().map(|s| format!("{s:?}")).collect()
    }

    /// Thumbnail textures currently loaded.
    /// A variant thumbnail ([`Slot::Variant`]): its texture once rendered; until then the job is
    /// queued at low priority (below on-screen grid thumbnails). Call it every frame the variant
    /// is visible: variants not asked for during the last frames leave the queue, and textures
    /// beyond [`VARIANT_TEXTURES`] are evicted least recently used first.
    pub fn variant(&mut self, job: RenderJob) -> Option<&Tex> {
        let key = job.key;
        let slot = Slot::Variant(key);
        self.variant_used.insert(key, self.frame);
        if !self.textures.contains_key(&slot) {
            self.request(slot, job, VARIANT_PRIORITY);
        }
        self.textures.get(&slot)
    }

    /// Variant thumbnail textures currently loaded.
    pub fn variant_textures(&self) -> usize {
        self.textures.keys().filter(|s| matches!(s, Slot::Variant(_))).count()
    }

    /// Drop variant jobs nobody asked for in the last frames, and the least recently used
    /// variant textures beyond the budget.
    fn evict_variants(&mut self) {
        let now = self.frame;
        let stale = |used: Option<&u64>| used.is_none_or(|f| now.saturating_sub(*f) > 2);
        let used = &self.variant_used;
        let dropped = self.pool.reprioritize(|s, p| match s {
            Slot::Variant(k) if stale(used.get(k)) => None,
            _ => Some(p),
        });
        for s in dropped {
            self.pending.remove(&s);
        }
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Variant(k) if stale(used.get(&k)) => {
                pending.remove(&q.slot);
                false
            }
            _ => true,
        });
        let n = self.variant_textures();
        if n > VARIANT_TEXTURES {
            let mut have: Vec<(u64, u64)> = self
                .textures
                .keys()
                .filter_map(|s| if let Slot::Variant(k) = s { Some((self.variant_used.get(k).copied().unwrap_or(0), *k)) } else { None })
                .collect();
            have.sort_unstable();
            for (_, k) in have.into_iter().take(n - VARIANT_TEXTURES) {
                self.textures.remove(&Slot::Variant(k));
                self.variant_used.remove(&k);
            }
        }
        if self.variant_used.len() > 4 * VARIANT_TEXTURES {
            let textures = &self.textures;
            let pending = &self.pending;
            self.variant_used.retain(|k, _| textures.contains_key(&Slot::Variant(*k)) || pending.contains_key(&Slot::Variant(*k)));
        }
    }

    pub fn thumb_textures(&self) -> usize {
        self.textures.keys().filter(|s| matches!(s, Slot::Thumb(_) | Slot::ThumbQuick(_))).count()
    }

    /// Collect finished jobs into textures. Returns true if anything changed.
    pub fn poll(&mut self, ctx: &egui::Context, session: &mut Session) -> bool {
        self.catalog_rev = session.catalog.revision;
        self.frame += 1;
        // wasm: run one job per frame on this thread, timed with the host clock
        #[cfg(target_arch = "wasm32")]
        let inline_ms = {
            let t0 = crate::now_ms();
            self.pool.run_inline(1);
            crate::now_ms() - t0
        };
        #[cfg(not(target_arch = "wasm32"))]
        let inline_ms = 0.0;
        let mut finished = Vec::new();
        while let Some(done) = self.pool.try_recv() {
            finished.push((done.slot, done.result, if done.ms > 0.0 { done.ms } else { inline_ms }));
        }
        if let Some(off) = self.offload.as_mut() {
            finished.extend(off.finished());
        }
        self.dispatch();
        let mut changed = false;
        for (mut slot, r, ms) in finished {
            session.accept(&r);
            self.completed += 1;
            if self.pending.get(&slot).is_some_and(|p| p.0 == r.key) {
                self.pending.remove(&slot);
            }
            let rendered = match r.rendered {
                Ok(x) => {
                    self.failed.remove(&slot);
                    x
                }
                Err(e) => {
                    if !matches!(slot, Slot::Prefetch(_)) {
                        log::warn!("render {slot:?}: {e}");
                        self.failed.insert(slot, (r.key, e, self.catalog_rev));
                    }
                    continue;
                }
            };
            if matches!(slot, Slot::Prefetch(_)) {
                continue;
            }
            if let Slot::ThumbQuick(id) = slot {
                if self.textures.contains_key(&Slot::Thumb(id)) {
                    continue; // the real thumbnail won the race
                }
                if r.quick == Some(QuickSource::Cached) {
                    // the photo's own cached thumbnail: final
                    slot = Slot::Thumb(id);
                }
            }
            if let Slot::Thumb(id) = slot {
                self.textures.remove(&Slot::ThumbQuick(id));
            }
            let img = &rendered.image;
            let color = std::sync::Arc::new(egui::ColorImage::from_rgba_unmultiplied([img.width, img.height], &img.as_bytes()));
            let pixels = self.keep_pixels.then(|| color.clone());
            let name = format!("{slot:?}");
            match self.textures.get_mut(&slot) {
                Some(t) => {
                    t.tex.set(color, egui::TextureOptions::LINEAR);
                    t.pixels = pixels;
                    t.key = r.key;
                    t.photo = r.photo;
                    t.size = [img.width, img.height];
                    t.histogram = Some(rendered.histogram);
                    t.ms = ms;
                    t.quick = r.quick;
                }
                None => {
                    let tex = ctx.load_texture(name, color, egui::TextureOptions::LINEAR);
                    self.textures.insert(
                        slot,
                        Tex {
                            key: r.key,
                            photo: r.photo,
                            tex,
                            size: [img.width, img.height],
                            histogram: Some(rendered.histogram),
                            ms,
                            quick: r.quick,
                            pixels,
                        },
                    );
                }
            }
            if slot == Slot::Main {
                self.last_main_ms = ms;
            }
            changed = true;
        }
        self.evict_variants();
        #[cfg(not(target_arch = "wasm32"))]
        self.trim_when_idle(ctx);
        if !self.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        changed
    }

    /// Free the GPU renderer's recycled buffers once nothing has rendered for [`IDLE_TRIM`] (they
    /// are reallocated by the next render; while working, renders reuse them).
    #[cfg(not(target_arch = "wasm32"))]
    fn trim_when_idle(&mut self, ctx: &egui::Context) {
        if !self.pending.is_empty() {
            self.idle = None;
            return;
        }
        let now = std::time::Instant::now();
        let (since, trimmed) = *self.idle.get_or_insert((now, false));
        if trimmed {
            return;
        }
        let waited = now.duration_since(since);
        if waited >= IDLE_TRIM {
            lightcraft_engine::gpu::trim_pool(0);
            lightcraft_engine::memory::release();
            self.idle = Some((since, true));
        } else {
            ctx.request_repaint_after(IDLE_TRIM - waited);
        }
    }

    /// What the renderer holds: per-view stage caches (CPU images and GPU buffers) and textures.
    pub fn memory(&self) -> serde_json::Value {
        let cpu: usize = self.stages.values().map(|s| s.bytes()).sum();
        let gpu: usize = self.stages.values().map(|s| lightcraft_engine::gpu::stage_bytes(s)).sum();
        let tex: usize = self.textures.values().map(|t| t.size[0] * t.size[1] * 4).sum();
        let copies: usize = self.textures.values().filter_map(|t| t.pixels.as_ref()).map(|p| p.pixels.len() * 4).sum();
        serde_json::json!({
            "stageCaches": {"count": self.stages.len(), "cpuBytes": cpu, "gpuBytes": gpu},
            "textures": {"count": self.textures.len(), "bytes": tex, "cpuCopyBytes": copies},
        })
    }

    /// CPU copies of the current textures by id (see [`Self::keep_pixels`]).
    pub fn cpu_textures(&self) -> HashMap<egui::TextureId, crate::softpaint::CpuTexture> {
        self.textures.values().filter_map(|t| Some((t.tex.id(), crate::softpaint::CpuTexture::linear(t.pixels.clone()?)))).collect()
    }

    /// Drop the import review thumbnails (a new review, or the dialog closed).
    pub fn forget_imports(&mut self) {
        self.textures.retain(|s, _| !matches!(s, Slot::Import(_)));
        self.quick_tried.retain(|s, _| !matches!(s, Slot::Import(_)));
    }

    /// Drop thumbnails that aren't in `keep` (bounded memory for huge libraries), and queued
    /// thumbnail jobs for photos that scrolled out of `keep`.
    pub fn evict_thumbs(&mut self, keep: &std::collections::HashSet<PhotoId>, max: usize) {
        let dropped = self.pool.reprioritize(|s, p| match s {
            Slot::Thumb(id) | Slot::ThumbQuick(id) if !keep.contains(id) && p <= 11 => None,
            _ => Some(p),
        });
        for s in dropped {
            self.pending.remove(&s);
            self.quick_tried.remove(&s);
        }
        let pending = &mut self.pending;
        self.queue.retain(|q| match q.slot {
            Slot::Thumb(id) if !keep.contains(&id) && q.priority <= 10 => {
                pending.remove(&q.slot);
                false
            }
            _ => true,
        });
        let thumbs = self.thumb_textures();
        if thumbs <= max {
            return;
        }
        self.textures.retain(|s, _| !matches!(s, Slot::Thumb(id) | Slot::ThumbQuick(id) if !keep.contains(id)));
        self.quick_tried.retain(|s, _| !matches!(s, Slot::ThumbQuick(id) if !keep.contains(id)));
    }
}
