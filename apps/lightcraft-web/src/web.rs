//! Browser host: opens the library in browser storage, starts the render workers and eframe on
//! `<canvas id="lightcraft_canvas">`, wires the file picker and drag-and-drop, and turns exports
//! into downloads.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use lightcraft_engine::Session;
use lightcraft_engine::catalog::Source;
use lightcraft_engine::library::LibraryStores;
use lightcraft_ui_egui::{LightcraftApp, Services, UiState};
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::backend::Backend;
use crate::backup::{ACTIVE_LIBRARY, DEFAULT_LIBRARY_DIR, valid_library_dir};
use crate::bench::Bench;
use crate::files::{Files, FlushOp, LIBRARY_FILES};
use crate::safety::{self, notice};
use crate::store::{Originals, content_hash, download_name, storage_key};
use crate::wire::ThumbIndex;
use crate::workers::{THUMB_INDEX, Workers};

const ACCEPT: &str = ".jpg,.jpeg,.png,.tif,.tiff,.webp,.dng,.cr2,.nef,.arw,.psd,.jxl,.gif,.bmp";

/// After a failed save, wait this long before trying again (ms).
const SAVE_RETRY_MS: f64 = 2000.0;

/// How long a notice stays on screen (s).
const NOTICE_SECS: f64 = 8.0;

/// How often view state, UI prefs and the thumbnail index are saved (ms).
const SAVE_EVERY_MS: f64 = 1000.0;

/// The page's window (`None` only outside a browser page, e.g. in a worker).
fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

/// Milliseconds since navigation start.
fn perf_now() -> f64 {
    window().and_then(|w| w.performance()).map(|p| p.now()).unwrap_or(0.0)
}

fn set_status(text: &str) {
    if let Some(el) = window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("lightcraft_status")) {
        el.set_text_content(Some(text));
    }
}

/// Storage key of library file `name` in library folder `dir`.
fn library_key(dir: &str, name: &str) -> String {
    format!("{dir}/{name}")
}

/// Repaint the app (notices from async tasks).
pub(crate) fn request_repaint() {
    CTX.with(|c| {
        if let Some(c) = c.borrow().as_ref() {
            c.request_repaint();
        }
    });
}

/// URL options (`?bench&store=idb&workers=2&reset`).
struct Options {
    bench: bool,
    /// "opfs" (default: OPFS, falling back to IndexedDB), "idb" or "memory".
    store: String,
    workers: Option<usize>,
    reset: bool,
}

impl Options {
    fn from_url() -> Options {
        let q = window().and_then(|w| w.location().search().ok()).unwrap_or_default();
        let params: Vec<(String, String)> = q
            .trim_start_matches('?')
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| match p.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (p.to_string(), String::new()),
            })
            .collect();
        let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        Options {
            bench: get("bench").is_some(),
            store: get("store").filter(|s| s == "idb" || s == "memory").unwrap_or_else(|| "opfs".into()),
            workers: get("workers").and_then(|v| v.parse().ok()),
            reset: get("reset").is_some(),
        }
    }
}

/// Store a picked/dropped file (bytes into browser storage, then queue its import).
async fn store_file(originals: Originals, backend: Option<Backend>, name: String, bytes: Vec<u8>, ctx: egui::Context) {
    let hash = content_hash(&bytes);
    if let Some(b) = &backend
        && let Err(e) = b.write(&storage_key(&hash), &bytes).await
    {
        // a photo whose bytes aren't stored would be gone after a reload: don't add it
        notice(format!("{name} was not added: browser storage refused it ({e}). Free up space or back up and clear old photos."));
        return;
    }
    originals.added(&name, &hash, Arc::from(bytes));
    ctx.request_repaint();
}

/// Read a browser `File` into storage.
async fn read_file(originals: Originals, backend: Option<Backend>, file: web_sys::File, ctx: egui::Context) {
    match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
        Ok(buf) => store_file(originals, backend, file.name(), js_sys::Uint8Array::new(&buf).to_vec(), ctx).await,
        Err(e) => log::warn!("reading {}: {e:?}", file.name()),
    }
}

/// Show the browser's open dialog; picked files are imported asynchronously.
fn open_picker(originals: Originals, backend: Option<Backend>, ctx: egui::Context) {
    let Some(doc) = window().and_then(|w| w.document()) else { return };
    let Ok(input) = doc.create_element("input").map(|e| e.unchecked_into::<web_sys::HtmlInputElement>()) else { return };
    input.set_type("file");
    input.set_multiple(true);
    input.set_accept(ACCEPT);
    let inp = input.clone();
    let on_change = Closure::<dyn FnMut()>::new(move || {
        if let Some(files) = inp.files() {
            for i in 0..files.length() {
                if let Some(f) = files.get(i) {
                    wasm_bindgen_futures::spawn_local(read_file(originals.clone(), backend.clone(), f, ctx.clone()));
                }
            }
        }
    });
    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    on_change.forget();
    input.click();
}

/// Offer `bytes` as a download named after `path`.
fn download(path: &str, bytes: &[u8]) -> Result<(), String> {
    let e = |e: JsValue| format!("download failed: {e:?}");
    let arr = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let opts = web_sys::BlobPropertyBag::new();
    let name = download_name(path);
    opts.set_type(crate::store::mime_for(name));
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&arr, &opts).map_err(e)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(e)?;
    let doc = window().and_then(|w| w.document()).ok_or("no document")?;
    let a: web_sys::HtmlAnchorElement = doc.create_element("a").map_err(e)?.unchecked_into();
    a.set_href(&url);
    a.set_download(name);
    a.click();
    // revoke once the click has been handled
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    if let Some(w) = window() {
        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 5000);
    }
    log::info!("exported {name} ({} bytes)", bytes.len());
    Ok(())
}

fn services(originals: Originals, backend: Option<Backend>, files: Files, frozen: Rc<Cell<bool>>, ctx: egui::Context) -> Services {
    let (backup_backend, backup_files) = (backend.clone(), files);
    let restore_backend = backend.clone();
    Services {
        backup_library: Some(Box::new(move |session: &mut Session| {
            let Some(b) = backup_backend.clone() else { return Err("nothing is stored in this browser session (?store=memory)".into()) };
            // the photos' own names for the originals in the zip
            let names: std::collections::HashMap<String, String> = session
                .catalog
                .photos()
                .filter_map(|p| match &p.source {
                    Source::File { path } => crate::store::hash_of_path(path).map(|h| (h.to_string(), p.file_name.clone())),
                    _ => None,
                })
                .collect();
            let files = backup_files.clone();
            notice("Backing up the library… (the download starts when it's ready)");
            wasm_bindgen_futures::spawn_local(async move {
                match safety::backup(files, b, names).await {
                    Ok((n, bytes)) => notice(format!(
                        "Library backed up: {n} original{} ({:.1} MB). Keep the file somewhere safe.",
                        if n == 1 { "" } else { "s" },
                        bytes / 1e6
                    )),
                    Err(e) => notice(format!("Backup failed: {e}")),
                }
            });
            Ok(json!({"started": true}))
        })),
        restore_library: Some(Box::new(move |_session: &mut Session| {
            let Some(b) = restore_backend.clone() else { return Err("nothing is stored in this browser session (?store=memory)".into()) };
            let frozen = frozen.clone();
            // once the backup is in place, stop saving the old library (the page reloads)
            safety::pick_and_restore(b, move || frozen.set(true));
            Ok(json!({"started": true}))
        })),
        pick_files: Some(Box::new(move || {
            open_picker(originals.clone(), backend.clone(), ctx.clone());
            Vec::new() // files arrive asynchronously and are imported on a later frame
        })),
        // Preset files: browser pickers are asynchronous; not wired on the web yet.
        pick_preset_files: None,
        pick_tracklog: None,
        save_preset_file: None,
        pick_curve_preset_files: None,
        save_curve_preset_file: None,
        write: Some(Box::new(download)),
        // downloads happen on the main thread: exports run in the foreground on the web
        write_shared: None,
        png: Some(Box::new(|img: &lightcraft_raster::Rgba8| {
            lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
        })),
        reveal: None,
        open_with: None,
        open_url: Some(Box::new(|url: &str| {
            let w = web_sys::window().ok_or("no window")?;
            w.open_with_url_and_target(url, "_blank").map(|_| ()).map_err(|_| "the browser blocked the new tab".to_string())
        })),
        pick_folder: None,
    }
}

/// Write dirty library files to storage, in order, until none are left. A failure (quota,
/// storage cleared) is recorded in `files` — the catalog then reports changes as unsaved instead
/// of accepting them silently — and retried after [`SAVE_RETRY_MS`].
async fn flush(files: Files, backend: Backend, dir: String, flushing: Rc<Cell<bool>>, failed_at: Rc<Cell<f64>>) {
    'outer: loop {
        let ops = files.take_dirty();
        if ops.is_empty() {
            if files.error().is_some() {
                log::info!("lightcraft: saving works again");
            }
            files.set_error(None);
            break;
        }
        for (i, op) in ops.iter().enumerate() {
            let r = match op {
                FlushOp::Write { name, data } => backend.write(&library_key(&dir, name), data).await,
                FlushOp::Append { name, offset, data } => backend.write_at(&library_key(&dir, name), Some(*offset), data).await,
            };
            if let Err(e) = r {
                log::error!("saving {} failed: {e}", op.name());
                files.failed(&ops[i..]);
                files.set_error(Some(e));
                failed_at.set(perf_now());
                break 'outer;
            }
        }
    }
    flushing.set(false);
}

/// Everything loaded before the UI starts.
struct Boot {
    opts: Options,
    backend: Option<Backend>,
    files: Files,
    index: ThumbIndex,
    /// The library folder in storage (`library`, or a restored backup's).
    lib_dir: String,
}

async fn boot(opts: Options) -> Boot {
    let t0 = perf_now();
    let backend = if opts.store == "memory" {
        None
    } else {
        match Backend::open(opts.store == "idb").await {
            Ok(b) => Some(b),
            Err(e) => {
                log::error!("no browser storage ({e}); this session won't be saved");
                None
            }
        }
    };
    let files = Files::default();
    let mut index = ThumbIndex::default();
    let mut lib_dir = DEFAULT_LIBRARY_DIR.to_string();
    if let Some(b) = &backend {
        if opts.reset {
            // everything this site keeps in the browser, photos included: never without asking
            let ok = safety::confirm(
                "?reset deletes the LightCraft library stored in this browser, including every imported photo. \
                 This can't be undone. Delete it?",
            );
            if ok {
                match b.clear().await {
                    Ok(()) => log::info!("lightcraft: storage cleared (?reset)"),
                    Err(e) => log::error!("clearing storage: {e}"),
                }
            } else {
                log::info!("lightcraft: ?reset declined; storage kept");
            }
        }
        // a restored backup lives in its own folder (see `backup`)
        if let Ok(Some(d)) = b.read(ACTIVE_LIBRARY).await {
            let d = String::from_utf8_lossy(&d).trim().to_string();
            if valid_library_dir(&d) {
                lib_dir = d;
            }
        }
        for name in LIBRARY_FILES {
            match b.read(&library_key(&lib_dir, name)).await {
                Ok(Some(data)) => files.preload(name, data),
                Ok(None) => {}
                Err(e) => log::error!("reading {name}: {e}"),
            }
        }
        if let Ok(Some(j)) = b.read(THUMB_INDEX).await {
            index = ThumbIndex::from_json(&j);
        }
        crate::backend::request_persistence(|granted| {
            if !granted {
                notice(
                    "This browser may delete LightCraft's library when it runs short of space (persistent storage wasn't granted). \
                     Keep your original photos elsewhere and use File ▸ Back Up Library… now and then.",
                );
            }
        });
        // thumbnails stored without an index entry (index saved late, or lost): delete them
        let b2 = b.clone();
        let known: std::collections::HashSet<String> = b
            .list("thumbs")
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.ends_with(".jpg"))
            .map(|n| n.trim_end_matches(".jpg").to_string())
            .collect();
        let orphans: Vec<String> = known.into_iter().filter(|k| !index.contains(k)).collect();
        if !orphans.is_empty() {
            wasm_bindgen_futures::spawn_local(async move {
                for k in &orphans {
                    let _ = b2.remove(&crate::wire::thumb_storage_key(k)).await;
                }
                log::info!("thumbnail cache: removed {} unindexed files", orphans.len());
            });
        }
    }
    log::info!(
        "lightcraft: storage {} opened in {:.0} ms ({} thumbnails indexed, {:.1} MB)",
        backend.as_ref().map_or("memory", |b| b.kind()),
        perf_now() - t0,
        index.len(),
        index.total() as f64 / 1e6
    );
    Boot { opts, backend, files, index, lib_dir }
}

struct WebApp {
    app: LightcraftApp,
    originals: Originals,
    files: Files,
    backend: Option<Backend>,
    workers: Option<Workers>,
    flushing: Rc<Cell<bool>>,
    /// When the last save to storage failed (ms; 0 = never).
    flush_failed_at: Rc<Cell<f64>>,
    /// Saving stopped (a restored backup is about to replace this library: the page reloads).
    frozen: Rc<Cell<bool>>,
    lib_dir: String,
    bench: Option<Bench>,
    first_frame_logged: bool,
    last_save: f64,
    ui_written: Vec<u8>,
}

impl WebApp {
    fn new(cc: &eframe::CreationContext<'_>, boot: Boot) -> Self {
        let Boot { opts, backend, files, index, lib_dir } = boot;
        let originals = Originals::default();
        let t = perf_now();
        let mut session = Session::new();
        originals.install(&mut session);
        let stores = LibraryStores {
            dir: format!("browser:{}/{lib_dir}", backend.as_ref().map_or("memory", |b| b.kind())).into(),
            catalog: Box::new(files.store()),
            files: Box::new(files.store()),
            on_disk: false,
        };
        let mut problem = None;
        match session.open_library_in(stores, true).cloned() {
            Ok(r) => log::info!(
                "lightcraft: library {} in {:.0} ms ({} photos; snapshot seq {}, {} ops replayed)",
                if r.created { "created" } else { "loaded" },
                perf_now() - t,
                session.catalog.len(),
                r.snapshot_seq,
                r.replayed
            ),
            Err(e) => {
                // never a silent demo session (issue #100): an empty one, and the window says why
                log::error!("opening the library failed: {e}");
                session = Session::new();
                originals.install(&mut session);
                problem = Some(lightcraft_ui_egui::panels::library_problem::LibraryProblem {
                    can_retry: false,
                    ..lightcraft_ui_egui::panels::library_problem::LibraryProblem::new(
                        "the browser's storage for this page",
                        format!("{e}. Reload the page to try again."),
                    )
                });
            }
        }
        if backend.is_some() {
            notice(
                "LightCraft in the browser is experimental. Your library is kept in this browser's storage: keep your original photos \
                 elsewhere and back up with File ▸ Back Up Library….",
            );
        }
        // previews are large (≤ 2560 px, f32): keep few in a 32-bit address space
        session.media.preview_capacity = 3;
        let frozen = Rc::new(Cell::new(false));
        let mut app = LightcraftApp::new(session, services(originals.clone(), backend.clone(), files.clone(), frozen.clone(), cc.egui_ctx.clone()));
        let ui_written = files.get("ui.json").unwrap_or_default();
        if let Ok(ui) = serde_json::from_slice::<UiState>(&ui_written) {
            app.ui = ui;
        }
        app.ui = app.ui.sanitized();
        app.library_problem = problem;
        let n = opts.workers.unwrap_or_else(|| {
            let cores = window().map_or(1, |w| w.navigator().hardware_concurrency() as usize);
            cores.saturating_sub(1).clamp(1, 4)
        });
        let workers =
            (n > 0).then(|| Workers::start(n, backend.as_ref().map_or("memory", |b| b.kind()), backend.clone(), index, cc.egui_ctx.clone()));
        if let Some(w) = &workers {
            app.renderer.set_offload(Box::new(w.clone()));
        }
        log::info!("lightcraft: {n} render workers");
        CTX.with(|c| *c.borrow_mut() = Some(cc.egui_ctx.clone()));
        let origin = lightcraft_ui_egui::now_ms() - perf_now();
        WebApp {
            app,
            originals,
            files,
            backend,
            workers,
            flushing: Rc::new(Cell::new(false)),
            flush_failed_at: Rc::new(Cell::new(0.0)),
            frozen,
            lib_dir,
            bench: opts.bench.then(|| Bench::new(origin)),
            first_frame_logged: false,
            last_save: 0.0,
            ui_written,
        }
    }

    fn import_dropped(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let (originals, backend, ctx) = (self.originals.clone(), self.backend.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let name = f.path().to_string_lossy().to_string();
                match f.bytes_async().await {
                    Ok(bytes) => store_file(originals, backend, name, bytes, ctx).await,
                    Err(e) => log::warn!("reading dropped {name}: {e}"),
                }
            });
        }
        let paths = self.originals.take_pending();
        if !paths.is_empty() {
            let n = paths.len();
            match self.app.run("library.import", json!({"paths": paths})) {
                Ok(_) => self.app.toast(ctx, format!("Added {n} photo{}", if n == 1 { "" } else { "s" })),
                Err(e) => log::warn!("import: {e}"),
            }
        }
    }

    /// Load originals the main thread asked for (and the active photo's, ahead of time).
    fn load_originals(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.app.session.selection.active
            && let Some(Source::File { path }) = self.app.session.catalog.photo(id).map(|p| &p.source)
            && !self.originals.contains(path)
        {
            self.originals.prefetch(path);
        }
        let Some(b) = &self.backend else { return };
        for hash in self.originals.take_wanted() {
            let (b, originals, ctx) = (b.clone(), self.originals.clone(), ctx.clone());
            wasm_bindgen_futures::spawn_local(async move {
                match b.read(&storage_key(&hash)).await {
                    Ok(Some(bytes)) => {
                        originals.insert(&hash, Arc::from(bytes));
                        ctx.request_repaint();
                    }
                    Ok(None) => {
                        log::warn!("original {hash} is not in browser storage");
                        originals.load_failed(&hash);
                    }
                    Err(e) => {
                        log::warn!("loading original {hash}: {e}");
                        originals.load_failed(&hash);
                    }
                }
            });
        }
    }

    /// Save view state, UI prefs and the thumbnail index now and then; flush dirty files.
    fn save(&mut self) {
        if self.frozen.get() {
            return;
        }
        let now = perf_now();
        if now - self.last_save >= SAVE_EVERY_MS {
            self.last_save = now;
            self.app.session.save_view();
            if let Ok(ui) = serde_json::to_vec_pretty(&self.app.ui)
                && ui != self.ui_written
            {
                self.files.write("ui.json", &ui);
                self.ui_written = ui;
            }
            if let (Some(w), Some(b)) = (&self.workers, &self.backend)
                && let Some(index) = w.take_index_if_dirty()
            {
                let b = b.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = b.write(THUMB_INDEX, &index).await {
                        log::warn!("saving the thumbnail index: {e}");
                    }
                });
            }
        }
        // after a failure, retry every couple of seconds rather than every frame
        let backing_off = self.files.error().is_some() && now - self.flush_failed_at.get() < SAVE_RETRY_MS;
        if let Some(b) = &self.backend
            && self.files.is_dirty()
            && !self.flushing.get()
            && !backing_off
        {
            self.flushing.set(true);
            let (files, dir, flushing, failed_at) = (self.files.clone(), self.lib_dir.clone(), self.flushing.clone(), self.flush_failed_at.clone());
            wasm_bindgen_futures::spawn_local(flush(files, b.clone(), dir, flushing, failed_at));
        }
    }
}

type Reply = (js_sys::Function, js_sys::Function);

thread_local! {
    /// Commands from JavaScript ([`command`]), run on the next frame.
    static INBOX: std::cell::RefCell<Vec<(String, String, Reply)>> = const { std::cell::RefCell::new(Vec::new()) };
    static CTX: std::cell::RefCell<Option<egui::Context>> = const { std::cell::RefCell::new(None) };
}

/// Run a command by id from JavaScript (automation, tests): `await command("library.info", "{}")`
/// resolves to the result as JSON text, or rejects with the error. Besides every engine/UI
/// command, `web.stats` reports the browser host's state (storage, workers, originals in memory).
#[wasm_bindgen]
pub fn command(id: String, params: String) -> js_sys::Promise {
    js_sys::Promise::new(&mut |resolve, reject| {
        INBOX.with(|q| q.borrow_mut().push((id.clone(), params.clone(), (resolve, reject))));
        CTX.with(|c| c.borrow().as_ref().map(|c| c.request_repaint()));
    })
}

impl WebApp {
    fn web_stats(&self) -> serde_json::Value {
        let (alive, ready, remote, inline) = self.workers.as_ref().map_or((0, 0, 0, 0), Workers::stats);
        let (orig_n, orig_bytes) = self.originals.usage();
        json!({
            "storage": self.backend.as_ref().map_or("memory", |b| b.kind()),
            "library": self.lib_dir,
            "dirty": self.files.is_dirty(),
            "flushing": self.flushing.get(),
            "saveError": self.files.error(),
            "workers": {"alive": alive, "ready": ready, "remoteJobs": remote, "inlineJobs": inline},
            "originalsInMemory": orig_n,
            "originalBytesInMemory": orig_bytes,
            "renderQueue": self.app.renderer.queued(),
            "rendersInFlight": self.app.renderer.in_flight(),
            "thumbTextures": self.app.renderer.thumb_textures(),
        })
    }

    fn run_inbox(&mut self) {
        let inbox = INBOX.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for (id, params, (resolve, reject)) in inbox {
            let params: serde_json::Value = serde_json::from_str(&params).unwrap_or(json!({}));
            let r = if id == "web.stats" { Ok(self.web_stats()) } else { self.app.run(&id, params) };
            let _ = match r {
                Ok(v) => resolve.call1(&JsValue::NULL, &v.to_string().into()),
                Err(e) => reject.call1(&JsValue::NULL, &e.into()),
            };
        }
    }
}

impl eframe::App for WebApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        for text in safety::take_notices() {
            let now = ctx.input(|i| i.time);
            self.app.ui.toast = Some((text, now + NOTICE_SECS));
        }
        self.run_inbox();
        self.import_dropped(ctx);
        self.load_originals(ctx);
        self.app.logic(ctx);
        self.save();
        if let Some(b) = self.bench.as_mut()
            && let Some(report) = b.step(&mut self.app)
        {
            log::info!("lightcraft-bench {report}");
            if let Some((alive, ready, remote, inline)) = self.workers.as_ref().map(Workers::stats) {
                log::info!("lightcraft-workers {{\"alive\":{alive},\"ready\":{ready},\"remote_jobs\":{remote},\"inline_jobs\":{inline}}}");
            }
            set_status(&format!("bench: {report}"));
            self.bench = None;
        }
        if self.bench.is_some() {
            ctx.request_repaint();
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        self.app.raw_input_hook(raw);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.app.ui(ui);
        if !self.first_frame_logged && !self.app.widgets.is_empty() {
            self.first_frame_logged = true;
            log::info!("lightcraft first UI frame at {:.0} ms after navigation start", perf_now());
        }
    }
}

/// Start the app (called by `index.html` after instantiating the module).
#[wasm_bindgen]
pub fn start() {
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    safety::install_panic_hook();
    log::info!("lightcraft: wasm instantiated at {:.0} ms", perf_now());
    let opts = Options::from_url();
    wasm_bindgen_futures::spawn_local(async move {
        // one tab per library: two would each keep their own copy and overwrite each other's saves
        if opts.store != "memory" && safety::acquire_tab_lock().await == Some(false) {
            safety::show_blocking(
                "LightCraft is already open in another tab or window of this browser.\n\n\
                 Switch to that tab, or close it and reload this page. (Two tabs would overwrite each other's changes.)",
            );
            return;
        }
        let Some(canvas) = window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("lightcraft_canvas"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        else {
            log::error!("missing <canvas id=\"lightcraft_canvas\">");
            return;
        };
        let boot = boot(opts).await;
        let runner = eframe::WebRunner::new();
        let r = runner.start(canvas, eframe::WebOptions::default(), Box::new(move |cc| Ok(Box::new(WebApp::new(cc, boot))))).await;
        match r {
            Ok(()) => {
                if let Some(el) = window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("lightcraft_loading")) {
                    el.remove();
                }
            }
            Err(e) => {
                log::error!("LightCraft failed to start: {e:?}");
                set_status(&format!("LightCraft failed to start: {e:?}"));
            }
        }
    });
}
