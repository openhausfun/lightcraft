//! Render workers: dedicated Web Workers, each running a second instance of this wasm module
//! (`worker.js` → [`worker_main`]). The main thread compiles the module once and posts the
//! compiled `WebAssembly.Module` to every worker, so a worker starts in a few milliseconds.
//!
//! This doesn't need wasm threads (shared memory, `+atomics`, a nightly `build-std`), only
//! module workers: the main thread posts a [`WireJob`] as JSON, the worker renders it and
//! transfers the RGBA bytes back (no copy across the boundary).
//!
//! Main side: [`Workers`] implements the UI's [`RenderOffload`]: one job per worker at a time,
//! with affinity (a photo goes back to the worker that already decoded it), the rendered-
//! thumbnail memory cache checked before dispatch, and the thumbnail storage index
//! ([`ThumbIndex`]) kept and pruned here. If no worker comes up, jobs run inline.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use js_sys::{Array, Object, Reflect, Uint8Array, Uint32Array};
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::media::{DISK_CACHE_BYTES, RenderJob, RenderResult, SourceLevel};
use lightcraft_engine::pipeline::Rendered;
use lightcraft_preview::{Hash128, PreviewCache};
use lightcraft_raster::{Histogram, Rgba8};
use lightcraft_ui_egui::render::{RenderOffload, Slot};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::backend::Backend;
use crate::wire::{ThumbIndex, WireJob, WorkerCore, thumb_storage_key};

/// Storage key of the thumbnail index.
pub const THUMB_INDEX: &str = "thumbs/index.json";

fn set(o: &Object, k: &str, v: &JsValue) {
    let _ = Reflect::set(o, &k.into(), v);
}

fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED)
}

fn now() -> f64 {
    let perf = get(&js_sys::global(), "performance");
    let f = get(&perf, "now");
    f.dyn_ref::<js_sys::Function>().and_then(|f| f.call0(&perf).ok()).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

// ---------------------------------------------------------------------------------------------
// worker side

thread_local! {
    static CORE: RefCell<WorkerCore> = RefCell::new(WorkerCore::default());
}

/// Entry point of a render worker (called by `worker.js` after instantiating the module).
/// `store` is the backend kind the page uses ("opfs", "idb" or "memory").
#[wasm_bindgen]
pub fn worker_main(store: String) {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    // The main thread sends jobs only after "ready", i.e. once storage is open, and one at a time.
    wasm_bindgen_futures::spawn_local(async move {
        let backend = match store.as_str() {
            "memory" => None,
            _ => Backend::open(store == "idb").await.map_err(|e| log::warn!("render worker: no storage ({e})")).ok(),
        };
        let s = scope.clone();
        let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
            let (s, b) = (s.clone(), backend.clone());
            wasm_bindgen_futures::spawn_local(async move { handle(&s, b, e.data()).await });
        });
        scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
        let ready = Object::new();
        set(&ready, "ready", &true.into());
        let _ = scope.post_message(&ready);
    });
}

/// Run one job message and post the result.
async fn handle(scope: &web_sys::DedicatedWorkerGlobalScope, backend: Option<Backend>, msg: JsValue) {
    let id = get(&msg, "id");
    let t0 = now();
    let out = Object::new();
    set(&out, "id", &id);
    let job: Result<WireJob, String> =
        get(&msg, "job").as_string().ok_or("no job".to_string()).and_then(|j| serde_json::from_str(&j).map_err(|e| e.to_string()));
    let mut transfer = Array::new();
    let result: Result<(Rendered, u32), String> = async {
        let job = job?;
        let mut cache_miss = false;
        // cached thumbnail?
        if job.thumb_cached
            && let (Some(b), Some(key)) = (&backend, job.thumb_key())
        {
            match b.read(&key).await.ok().flatten().and_then(|bytes| lightcraft_preview::decode_jpeg(&bytes)) {
                Some(image) => {
                    let histogram = Histogram::of_srgb8(&image);
                    return Ok((Rendered { image, histogram, deep: None }, 0));
                }
                None => cache_miss = true,
            }
        }
        set(&out, "cacheMiss", &cache_miss.into());
        let original = match CORE.with(|c| c.borrow_mut().needs_original(&job)) {
            Some(hash) => match &backend {
                Some(b) => b.read(&crate::store::storage_key(&hash)).await?,
                None => None,
            },
            None => None,
        };
        let rendered = CORE.with(|c| c.borrow_mut().render(&job, original.as_deref()))?;
        let mut stored = 0;
        if let (Some(b), Some(key)) = (&backend, job.thumb_key())
            && let Some(jpeg) = lightcraft_preview::encode_jpeg(&rendered.image)
        {
            match b.write(&key, &jpeg).await {
                Ok(()) => stored = jpeg.len() as u32,
                Err(e) => log::warn!("thumbnail cache write: {e}"),
            }
        }
        Ok((rendered, stored))
    }
    .await;
    match result {
        Ok((r, stored)) => {
            let rgba = Uint8Array::from(r.image.as_bytes().as_ref());
            let h = &r.histogram;
            let hist = Uint32Array::new_with_length(4 * 256 + 1);
            for (i, ch) in [&h.r, &h.g, &h.b, &h.luma].into_iter().enumerate() {
                hist.subarray((i * 256) as u32, ((i + 1) * 256) as u32).copy_from(ch);
            }
            hist.set_index(1024, h.total);
            set(&out, "ok", &true.into());
            set(&out, "w", &(r.image.width as u32).into());
            set(&out, "h", &(r.image.height as u32).into());
            set(&out, "rgba", &rgba);
            set(&out, "hist", &hist);
            set(&out, "stored", &stored.into());
            transfer = Array::of2(&rgba.buffer(), &hist.buffer());
        }
        Err(e) => {
            set(&out, "ok", &false.into());
            set(&out, "err", &e.into());
        }
    }
    set(&out, "ms", &(now() - t0).into());
    if let Err(e) = scope.post_message_with_transfer(&out, &transfer) {
        log::error!("render worker: post: {e:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// main side

struct Busy {
    id: u32,
    slot: Slot,
    photo: PhotoId,
    level: SourceLevel,
    key: u64,
    cache: Option<(Arc<PreviewCache>, Hash128)>,
}

struct W {
    worker: web_sys::Worker,
    ready: bool,
    dead: bool,
    busy: Option<Busy>,
}

struct Inner {
    workers: Vec<W>,
    done: Vec<(Slot, RenderResult, f64)>,
    next_id: u32,
    /// (photo, level) → worker that last decoded it.
    affinity: HashMap<(PhotoId, SourceLevel), usize>,
    index: ThumbIndex,
    backend: Option<Backend>,
    ctx: egui::Context,
    /// Jobs finished by workers / inline (for diagnostics).
    remote_done: u64,
    inline_done: u64,
    inline_this_frame: bool,
}

/// The main thread's handle on the render workers.
#[derive(Clone)]
pub struct Workers(Rc<RefCell<Inner>>);

impl Workers {
    /// Start `n` workers (none: every job runs inline). `store` is the backend kind for workers.
    pub fn start(n: usize, store: &str, backend: Option<Backend>, index: ThumbIndex, ctx: egui::Context) -> Workers {
        let w = Workers(Rc::new(RefCell::new(Inner {
            workers: Vec::new(),
            done: Vec::new(),
            next_id: 0,
            affinity: HashMap::new(),
            index,
            backend,
            ctx,
            remote_done: 0,
            inline_done: 0,
            inline_this_frame: false,
        })));
        let module = get(&js_sys::global(), "lightcraftModule");
        if module.is_undefined() {
            log::warn!("render workers: no compiled module on the page (window.lightcraftModule); rendering on the main thread");
            return w;
        }
        for i in 0..n {
            let opts = web_sys::WorkerOptions::new();
            opts.set_type(web_sys::WorkerType::Module);
            opts.set_name(&format!("lightcraft-render-{i}"));
            let worker = match web_sys::Worker::new_with_options("./worker.js", &opts) {
                Ok(x) => x,
                Err(e) => {
                    log::warn!("render worker {i}: {e:?}");
                    continue;
                }
            };
            let me = w.clone();
            let onmessage = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| me.on_message(i, e.data()));
            worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
            onmessage.forget();
            let me = w.clone();
            let onerror = Closure::<dyn FnMut(JsValue)>::new(move |e: JsValue| me.on_error(i, e));
            worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));
            onerror.forget();
            let init = Object::new();
            set(&init, "module", &module);
            set(&init, "store", &store.into());
            if let Err(e) = worker.post_message(&init) {
                log::warn!("render worker {i}: {e:?}");
                continue;
            }
            w.0.borrow_mut().workers.push(W { worker, ready: false, dead: false, busy: None });
        }
        w
    }

    fn on_error(&self, i: usize, e: JsValue) {
        let msg = get(&e, "message").as_string().unwrap_or_else(|| format!("{e:?}"));
        log::error!("render worker {i} failed: {msg}");
        let mut g = self.0.borrow_mut();
        if let Some(w) = g.workers.get_mut(i) {
            w.dead = true;
            w.worker.terminate();
            if let Some(b) = w.busy.take() {
                let r = RenderResult { photo: b.photo, level: b.level, key: b.key, rendered: Err(msg), loaded: None, quick: None };
                g.done.push((b.slot, r, 0.0));
            }
        }
        g.ctx.request_repaint();
    }

    fn on_message(&self, i: usize, data: JsValue) {
        let mut g = self.0.borrow_mut();
        let g = &mut *g;
        g.ctx.request_repaint();
        let Some(w) = g.workers.get_mut(i) else { return };
        if get(&data, "ready").is_truthy() {
            w.ready = true;
            return;
        }
        let id = get(&data, "id").as_f64().unwrap_or(-1.0) as u32;
        let Some(b) = w.busy.take_if(|b| b.id == id) else { return };
        let ms = get(&data, "ms").as_f64().unwrap_or(0.0);
        let rendered = if get(&data, "ok").is_truthy() {
            let (wd, ht) = (get(&data, "w").as_f64().unwrap_or(0.0) as usize, get(&data, "h").as_f64().unwrap_or(0.0) as usize);
            let rgba = Uint8Array::new(&get(&data, "rgba")).to_vec();
            let hist = Uint32Array::new(&get(&data, "hist")).to_vec();
            match Rgba8::from_bytes(wd, ht, &rgba) {
                Some(image) if hist.len() == 1025 => {
                    let ch = |k: usize| hist[k * 256..(k + 1) * 256].to_vec();
                    let histogram = Histogram { r: ch(0), g: ch(1), b: ch(2), luma: ch(3), total: hist[1024] };
                    Ok(Rendered { image, histogram, deep: None })
                }
                _ => Err("render worker: bad image".to_string()),
            }
        } else {
            Err(get(&data, "err").as_string().unwrap_or_else(|| "render worker failed".into()))
        };
        if let Ok(r) = &rendered
            && let Some((cache, key)) = &b.cache
        {
            cache.put(*key, Arc::new(r.image.clone()));
            let hex = key.to_string();
            let stored = get(&data, "stored").as_f64().unwrap_or(0.0) as u64;
            if get(&data, "cacheMiss").is_truthy() {
                g.index.remove(&hex);
            }
            if stored > 0 {
                g.index.insert(&hex, stored);
                let gone = g.index.prune(DISK_CACHE_BYTES);
                if !gone.is_empty()
                    && let Some(be) = g.backend.clone()
                {
                    log::info!("thumbnail cache: pruning {} files", gone.len());
                    wasm_bindgen_futures::spawn_local(async move {
                        for k in gone {
                            let _ = be.remove(&thumb_storage_key(&k)).await;
                        }
                    });
                }
            }
        }
        g.affinity.insert((b.photo, b.level), i);
        g.remote_done += 1;
        g.done.push((b.slot, RenderResult { photo: b.photo, level: b.level, key: b.key, rendered, loaded: None, quick: None }, ms));
    }

    /// The thumbnail index as JSON if it changed since the last call (the host saves it).
    pub fn take_index_if_dirty(&self) -> Option<Vec<u8>> {
        let mut g = self.0.borrow_mut();
        if !g.index.dirty {
            return None;
        }
        g.index.dirty = false;
        Some(g.index.to_json())
    }

    /// (workers alive, workers ready, jobs done remotely, jobs done inline).
    pub fn stats(&self) -> (usize, usize, u64, u64) {
        let g = self.0.borrow();
        let alive = g.workers.iter().filter(|w| !w.dead).count();
        let ready = g.workers.iter().filter(|w| w.ready && !w.dead).count();
        (alive, ready, g.remote_done, g.inline_done)
    }
}

impl RenderOffload for Workers {
    fn try_start(&mut self, slot: Slot, job: RenderJob) -> Option<RenderJob> {
        let mut g = self.0.borrow_mut();
        let g = &mut *g;
        // rendered thumbnail in memory: no worker needed
        if let Some((cache, key)) = &job.cache
            && let Some(img) = cache.get(*key)
        {
            let image = Arc::unwrap_or_clone(img);
            let histogram = Histogram::of_srgb8(&image);
            let r = RenderResult {
                photo: job.photo,
                level: job.level,
                key: job.key,
                rendered: Ok(Rendered { image, histogram, deep: None }),
                loaded: None,
                quick: None,
            };
            g.done.push((slot, r, 0.0));
            return None;
        }
        if g.workers.iter().all(|w| w.dead) {
            // no workers: render here, one job per frame (as without workers)
            if g.inline_this_frame {
                return Some(job);
            }
            g.inline_this_frame = true;
            let t0 = now();
            let r = job.run();
            g.inline_done += 1;
            g.done.push((slot, r, now() - t0));
            return None;
        }
        let free = |w: &W| w.ready && !w.dead && w.busy.is_none();
        let pick = match g.affinity.get(&(job.photo, job.level)) {
            // a decoded preview-level source, or the loupe's stage cache, is worth waiting for; a
            // thumbnail isn't
            Some(&i) if g.workers.get(i).is_some_and(free) => Some(i),
            Some(&i) if (job.level == SourceLevel::Preview || job.stages.is_some()) && g.workers.get(i).is_some_and(|w| !w.dead) => None,
            _ => g.workers.iter().position(free),
        };
        let Some(i) = pick else { return Some(job) };
        // decided here, so the next job for this photo follows even before this one finishes
        g.affinity.insert((job.photo, job.level), i);
        let thumb_cached = job.cache.as_ref().is_some_and(|(_, k)| g.index.touch(&k.to_string()));
        let wire = WireJob::from_job(&job, thumb_cached, &format!("{slot:?}"));
        let Ok(text) = serde_json::to_string(&wire) else { return Some(job) };
        g.next_id = g.next_id.wrapping_add(1);
        let id = g.next_id;
        let msg = Object::new();
        set(&msg, "id", &id.into());
        set(&msg, "job", &text.into());
        let w = &mut g.workers[i];
        if let Err(e) = w.worker.post_message(&msg) {
            log::warn!("render worker {i}: {e:?}");
            return Some(job);
        }
        w.busy = Some(Busy { id, slot, photo: job.photo, level: job.level, key: job.key, cache: job.cache });
        None
    }

    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
        let mut g = self.0.borrow_mut();
        g.inline_this_frame = false;
        std::mem::take(&mut g.done)
    }
}
