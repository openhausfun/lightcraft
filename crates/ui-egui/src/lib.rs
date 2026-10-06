//! LightCraft's egui frontend: a Lightroom-style UI over `lightcraft-engine`.
//!
//! The UI is thin: every action goes through [`LightcraftApp::run`], which handles UI commands
//! (views, panels, zoom — see [`menus::UI_COMMANDS`]) and forwards everything else to the engine.
//! The same entry point serves menus, shortcuts, buttons and the control channel ([`control`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod control;
pub mod export_task;
pub mod headless;
pub mod i18n;
pub mod icons;
pub mod import;
pub mod links;
pub mod menubar;
pub mod menus;
pub mod merge;
pub mod panels;
pub mod render;
pub mod shortcuts;
pub mod softpaint;
pub mod state;
pub mod tasks;
pub mod theme;
pub mod widgets;

#[cfg(test)]
mod tests_curve;
#[cfg(test)]
mod tests_grid;
#[cfg(test)]
mod tests_library_problem;
#[cfg(test)]
mod tests_masking;
#[cfg(test)]
mod tests_offline;
#[cfg(test)]
mod tests_panels;
#[cfg(test)]
mod tests_quit_unsaved;
#[cfg(test)]
mod tests_scroll;
#[cfg(test)]
mod tests_unsaved;

use std::sync::mpsc::{Receiver, Sender};

use lightcraft_engine::Session;
use serde_json::Value;

pub use control::{ControlRequest, ControlResponse};
pub use state::UiState;

pub type PickFiles = Box<dyn FnMut() -> Vec<String>>;
/// A save dialog: suggested file name → chosen path (`None` = cancelled).
pub type SaveFile = Box<dyn FnMut(&str) -> Option<String>>;
pub type WriteFn = Box<dyn FnMut(&str, &[u8]) -> Result<(), String>>;
/// A writer other threads can use (background export).
pub type SharedWrite = std::sync::Arc<dyn Fn(&str, &[u8]) -> Result<(), String> + Send + Sync>;
pub type PngEncode = Box<dyn Fn(&lightcraft_raster::Rgba8) -> Vec<u8>>;
/// A folder chooser (`None` = cancelled).
pub type PickFolder = Box<dyn FnMut() -> Option<String>>;
/// Reveal a file in the system file manager (Finder / Explorer / the folder on Linux).
pub type RevealFn = Box<dyn FnMut(&str) -> Result<(), String>>;
/// Open a URL in the user's browser.
pub type OpenUrlFn = Box<dyn FnMut(&str) -> Result<(), String>>;
/// Open a file in an application (`app` = "" for the system's default one).
pub type OpenWithFn = Box<dyn FnMut(&str, &str) -> Result<(), String>>;
/// Something the host does with the session (web: back up or restore the library in browser
/// storage); the work may finish asynchronously.
pub type HostAction = Box<dyn FnMut(&mut Session) -> Result<Value, String>>;

/// Platform services injected by the host app (desktop or web).
#[derive(Default)]
pub struct Services {
    /// Show an open dialog for photos; returns paths.
    pub pick_files: Option<PickFiles>,
    /// Open dialog for preset files (`.lcpreset`, `.xmp`, `.lrtemplate`, `.zip`, `.dng`, Luminar `.lmp` / `.mplumpack`).
    pub pick_preset_files: Option<PickFiles>,
    /// Open dialog for a GPS track log (`.gpx`; Photo ▸ Auto-Tag from Tracklog…).
    pub pick_tracklog: Option<PickFiles>,
    /// Save dialog for an exported `.lcpreset` file.
    pub save_preset_file: Option<SaveFile>,
    /// Open dialog for point-curve preset files (`.lccurve`).
    pub pick_curve_preset_files: Option<PickFiles>,
    /// Save dialog for an exported `.lccurve` file.
    pub save_curve_preset_file: Option<SaveFile>,
    pub write: Option<WriteFn>,
    /// Thread-safe writer: with it, UI-started exports run in the background (desktop only).
    pub write_shared: Option<SharedWrite>,
    /// PNG encoder (the host links an image encoder; the UI crate stays codec-free).
    pub png: Option<PngEncode>,
    /// Show a file in the system file manager (desktop only).
    pub reveal: Option<RevealFn>,
    /// Choose a folder (Settings → General → Open Library…; desktop only).
    pub pick_folder: Option<PickFolder>,
    /// Open a web link in the browser (Help menu, About, Discord button).
    pub open_url: Option<OpenUrlFn>,
    /// Open a file in an external editor (Edit in External Editor; desktop only).
    pub open_with: Option<OpenWithFn>,
    /// File ▸ Back Up Library…: save the whole library (catalog and originals) as one file the
    /// user keeps (web only: there the library lives in browser storage, which the browser may
    /// clear; on the desktop it is a folder backed up like any other).
    pub backup_library: Option<HostAction>,
    /// File ▸ Restore Library from Backup… (web only; keeps the current library).
    pub restore_library: Option<HostAction>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Perf {
    /// Layout of the last frame ([`LightcraftApp::ui`], including commands run from widgets).
    pub frame_ms: f64,
    /// Per-frame logic before layout ([`LightcraftApp::logic`]: control channel, shortcuts,
    /// render polling, pending catalog persistence).
    pub logic_ms: f64,
    /// The whole update of the last frame: logic + layout.
    pub update_ms: f64,
    /// The slowest whole update since start.
    pub max_update_ms: f64,
    pub fps: f64,
}

pub struct LightcraftApp {
    /// Per-catalog-revision caches of library-wide results the panels show every frame
    /// (expensive on big libraries).
    pub caches: Caches,
    pub session: Session,
    pub ui: UiState,
    pub services: Services,
    pub renderer: render::Renderer,
    pub perf: Perf,
    /// macOS: the host draws the traffic lights over our top bar.
    pub integrated_titlebar: bool,
    /// The host installed a native menu bar (no in-window menus then).
    pub native_menu: bool,
    /// Shortcuts the native menu bar currently handles (`Cmd+Z`, `G`…): the egui shortcut handler
    /// leaves them alone so nothing fires twice.
    pub native_shortcuts: std::collections::HashSet<String>,
    /// The host is [`headless::Headless`] (it answers viewport screenshot commands itself).
    pub headless_host: bool,
    /// Warnings to show one at a time (damaged settings files…, issue #103).
    pub notices: Vec<String>,
    /// Quitting was stopped because changes couldn't be saved: the prompt's text.
    pub quit_prompt: Option<String>,
    /// Quit Anyway was chosen: the window may close with unsaved changes.
    pub quit_confirmed: bool,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_screenshots: Vec<PendingShot>,
    screenshot_token: u64,
    /// Offscreen context for headless screenshots of the windowed app.
    shadow: Option<headless::HeadlessView>,
    /// Synthetic input events (from the control channel) injected one step per frame.
    pub synthetic: Vec<egui::Event>,
    /// Modifiers announced for synthetic input (held from a button down to its release).
    synthetic_mods: egui::Modifiers,
    /// Clear `synthetic_mods` on the next frame.
    synthetic_mods_release: bool,
    styled: bool,
    fonts_ready: bool,
    last_time: f64,
    /// Rect of the photo canvas and the displayed image (screen points) from the last frame.
    pub canvas_rect: Option<egui::Rect>,
    pub image_rect: Option<egui::Rect>,
    /// Scroll offsets (points) of the photo grid (vertical) and the filmstrip (horizontal) as
    /// drawn last (`ui.inspect` → `scroll`).
    pub grid_scroll: Option<f32>,
    pub film_scroll: Option<f32>,
    /// Widget registry from the last frame (automation ids → rects).
    pub widgets: Vec<(String, egui::Rect)>,
    /// In-progress on-canvas gesture (brush stroke points, gradient drag…).
    pub gesture: Option<panels::detail::Gesture>,
    /// What the loupe drew last frame: photo and source ("render", "cached", "embedded", "small",
    /// "thumb", "none").
    pub loupe_shown: Option<(lightcraft_catalog::PhotoId, &'static str)>,
    /// Photo Merge dialog previews and background merges.
    pub merge: merge::MergeState,
    /// An import in progress (the import review dialog's batches).
    pub import: Option<import::ImportTask>,
    /// A folder scan in progress (feeds the import review).
    pub scan: Option<import::ScanTask>,
    /// A background export in progress.
    pub export: Option<export_task::ExportTask>,
    /// Background file-system work of other commands (Find Missing Photos, auto import…).
    pub tasks: tasks::Tasks,
    /// The files of the last finished background export (`ui.inspect` → `export.last`).
    pub last_export_result: Option<Value>,
    /// The look the loupe shows while the pointer rests on a preset or profile (set by the
    /// panels each frame; nothing is committed, no history entry).
    pub hover_preview: Option<HoverPreview>,
    /// The window is in full screen (as last reported by the host, or as last requested).
    pub window_is_fullscreen: bool,
    /// The GPU preference last applied (`app.gpu`), to apply Settings changes once.
    gpu_applied: Option<bool>,
    /// The memory budget setting last applied (MB, 0 = automatic).
    memory_applied: Option<u32>,
    /// The library failed to open at launch: the blocking window, then the temporary-session
    /// banner (issue #100). Cleared once a library opens.
    pub library_problem: Option<panels::library_problem::LibraryProblem>,
}

impl LightcraftApp {
    pub fn new(session: Session, services: Services) -> Self {
        Self {
            session,
            ui: UiState::default(),
            services,
            renderer: render::Renderer::default(),
            perf: Perf::default(),
            caches: Caches::default(),
            integrated_titlebar: false,
            native_menu: false,
            native_shortcuts: Default::default(),
            headless_host: false,
            notices: vec![],
            quit_prompt: None,
            quit_confirmed: false,
            control_rx: None,
            pending_screenshots: vec![],
            screenshot_token: 0,
            shadow: None,
            synthetic: vec![],
            synthetic_mods: egui::Modifiers::NONE,
            synthetic_mods_release: false,
            styled: false,
            fonts_ready: false,
            last_time: 0.0,
            canvas_rect: None,
            image_rect: None,
            grid_scroll: None,
            film_scroll: None,
            widgets: vec![],
            gesture: None,
            loupe_shown: None,
            merge: merge::MergeState::default(),
            import: None,
            scan: None,
            export: None,
            tasks: Default::default(),
            last_export_result: None,
            hover_preview: None,
            window_is_fullscreen: false,
            gpu_applied: None,
            memory_applied: None,
            library_problem: None,
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        // keep CPU copies of photo textures so `ui.screenshot {"headless": true}` can draw them
        self.renderer.keep_pixels = true;
        self
    }

    /// Run a UI or engine command by id. The single entry point for every frontend path.
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        let r = self.session.execute(id, &params).map_err(|e| e.to_string());
        if let Err(e) = &r {
            self.ui.status = e.clone();
        }
        r
    }

    /// Show a transient toast at the bottom of the canvas (like the reference app's HUD).
    /// Advance a running slideshow (wrapping around at the end).
    fn slideshow_tick(&mut self, ctx: &egui::Context) {
        let Some((interval, due, paused)) = self.ui.slideshow else { return };
        if !self.ui.fullscreen {
            self.ui.slideshow = None;
            return;
        }
        if paused {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now >= due {
            let vis = self.session.visible_cloned();
            if let Some(cur) = self.session.active()
                && !vis.is_empty()
            {
                let i = vis.iter().position(|x| *x == cur).map_or(0, |i| (i + 1) % vis.len());
                let _ = self.run("library.select", serde_json::json!({"ids": [vis[i].0]}));
            }
            self.ui.slideshow = Some((interval, now + interval, false));
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - now).clamp(0.05, interval)));
    }

    /// Announce the start and end of a Build Previews run.
    fn preview_build_status(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        let Some(b) = self.session.preview_build.clone() else { return };
        let key = std::sync::Arc::as_ptr(&b) as usize;
        if b.finished.load(Ordering::Relaxed) {
            if self.ui.preview_build_seen != Some((key, true)) {
                self.ui.preview_build_seen = Some((key, true));
                let (done, failed) = (b.done.load(Ordering::Relaxed), b.failed.load(Ordering::Relaxed));
                let plural = if done == 1 { "" } else { "s" };
                let mut msg = match (b.error(), b.what) {
                    (Some(e), what) => format!("{}: {e}", if what.is_empty() { "previews" } else { what }),
                    (None, "") => format!("Previews ready for {done} photo{plural}"),
                    (None, what) => format!("Done ({what}): {done} photo{plural}"),
                };
                if failed > 0 {
                    msg.push_str(&format!(" · {failed} couldn't be rendered"));
                }
                self.toast(ctx, msg);
            }
        } else {
            if self.ui.preview_build_seen != Some((key, false)) {
                self.ui.preview_build_seen = Some((key, false));
                let msg = match b.what {
                    "" => format!("Building previews for {} photos…", b.total),
                    what => format!("Working on {what} for {} photos…", b.total),
                };
                self.toast(ctx, msg);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Announce when saving the library starts failing (changes then live only in memory and are
    /// retried) and when it works again; the top bar's cloud icon shows the state meanwhile.
    fn save_status(&mut self, ctx: &egui::Context) {
        let unsaved = self.session.unsaved().map(|(n, e)| (n, e.to_string()));
        match (unsaved, self.ui.unsaved_seen) {
            (Some((n, e)), false) => {
                self.ui.unsaved_seen = true;
                let t = ctx.input(|i| i.time);
                let what = if n == 1 { "1 change".to_string() } else { format!("{n} changes") };
                self.ui.toast = Some((format!("{what} saved in memory but not written to disk: {e} — LightCraft will retry"), t + 6.0));
            }
            (None, true) => {
                self.ui.unsaved_seen = false;
                self.toast(ctx, "Library saved");
            }
            (Some(_), true) => ctx.request_repaint_after(std::time::Duration::from_secs(2)), // retry
            (None, false) => {}
        }
    }

    pub fn toast(&mut self, ctx: &egui::Context, text: impl Into<String>) {
        let t = ctx.input(|i| i.time);
        self.ui.toast = Some((text.into(), t + 1.4));
    }

    fn drain_control(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.control_rx.take() else { return };
        while let Ok(req) = rx.try_recv() {
            let reply = req.reply.clone();
            match control::handle(self, ctx, &req) {
                control::Outcome::Done(v) => {
                    let _ = reply.send(v);
                }
                control::Outcome::Screenshot { path, headless } => {
                    self.screenshot_token += 1;
                    let now = now_ms();
                    self.pending_screenshots.push(PendingShot {
                        token: self.screenshot_token,
                        path,
                        reply,
                        headless: headless && !self.headless_host,
                        not_before: now + 120.0,
                        deadline: now + SCREENSHOT_SETTLE_MS,
                        frames: 0,
                        sent_at: None,
                    });
                }
            }
        }
        self.control_rx = Some(rx);
    }

    /// Advance pending screenshots: wait (≥ 3 frames, ≥ 120 ms) until no renders are in flight
    /// (or a timeout), then capture — via the host's compositor, or headlessly
    /// ([`Self::headless_screenshot`]) when asked or when the compositor delivers nothing.
    fn issue_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_screenshots.is_empty() {
            return;
        }
        let now = now_ms();
        let busy = self.renderer.in_flight() > 0 || self.merge.busy();
        let mut shots = std::mem::take(&mut self.pending_screenshots);
        let mut shadow_ticked = false;
        shots.retain_mut(|s| {
            if let Some(sent) = s.sent_at {
                if now - sent < SCREENSHOT_FALLBACK_MS || self.headless_host {
                    return true;
                }
                // The compositor delivered nothing (display asleep, window occluded): go headless.
                log::warn!("ui.screenshot: no frame from the compositor after {SCREENSHOT_FALLBACK_MS} ms; rendering headlessly");
                s.sent_at = None;
                s.headless = true;
                s.deadline = now + SCREENSHOT_SETTLE_MS;
            }
            s.frames += 1;
            let ready = now >= s.not_before && s.frames >= 3 && (!busy || now > s.deadline);
            if s.headless {
                // The shadow frame requests the renders the UI needs, even while the window shows nothing.
                let img = if ready || !shadow_ticked { self.headless_screenshot(ctx, ready) } else { None };
                shadow_ticked = true;
                if let Some(img) = img {
                    let _ = s.reply.send(control::save_screenshot(self, &img, s.path.as_deref()));
                    return false;
                }
                true
            } else {
                if ready {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(s.token)));
                    s.sent_at = Some(now);
                }
                true
            }
        });
        shots.append(&mut self.pending_screenshots);
        self.pending_screenshots = shots;
        if !self.pending_screenshots.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// Draw the UI into an offscreen context at the window's size; with `capture`, rasterize it
    /// on the CPU (photo textures come from the renderer's CPU copies).
    pub fn headless_screenshot(&mut self, main: &egui::Context, capture: bool) -> Option<egui::ColorImage> {
        let mut view = self.shadow.take().unwrap_or_default();
        let size = main.input(|i| i.content_rect()).size();
        let size = if size.x >= 1.0 && size.y >= 1.0 { size } else { egui::vec2(1600.0, 1000.0) };
        let ppp = main.pixels_per_point();
        let time = main.input(|i| i.time);
        if view.frames() == 0 {
            // warm-up pass: activates our fonts (pending font definitions live in `Memory`, which
            // is replaced below)
            view.run(headless::HeadlessView::raw_input(size, ppp, time, vec![]), |_| {});
        }
        // same scroll offsets, open sections, style… as the window
        let memory = main.memory(|m| m.clone());
        view.ctx.memory_mut(|m| *m = memory);
        view.run(headless::HeadlessView::raw_input(size, ppp, time, vec![]), |ui| self.ui(ui));
        let img = capture.then(|| {
            let mut tex = self.renderer.cpu_textures();
            if let (Some((t, ..)), Some(px)) = (&self.merge.preview, &self.merge.preview_pixels) {
                tex.insert(t.id(), crate::softpaint::CpuTexture::linear(px.clone()));
            }
            view.paint(&tex)
        });
        self.shadow = Some(view);
        img
    }

    fn collect_screenshots(&mut self, ctx: &egui::Context) {
        if self.pending_screenshots.is_empty() {
            return;
        }
        let events: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { user_data, image, .. } => {
                        let token = user_data.data.as_ref().and_then(|d| d.downcast_ref::<u64>()).copied()?;
                        Some((token, image.clone()))
                    }
                    _ => None,
                })
                .collect()
        });
        for (token, image) in events {
            if let Some(i) = self.pending_screenshots.iter().position(|s| s.token == token) {
                let s = self.pending_screenshots.remove(i);
                let _ = s.reply.send(control::save_screenshot(self, &image, s.path.as_deref()));
            }
        }
    }

    /// Per-frame logic before layout (control channel, renders, shortcuts, drops).
    pub fn logic(&mut self, ctx: &egui::Context) {
        i18n::set_language(self.ui.language);
        let t0 = now_ms();
        panels::library_problem::logic(self);
        self.logic_inner(ctx);
        self.perf.logic_ms = now_ms() - t0;
    }

    fn logic_inner(&mut self, ctx: &egui::Context) {
        if !self.styled {
            theme::install_fonts(ctx);
            theme::apply(ctx);
            // File → Add from Device lists cards scanned in the background: show hot-plugs
            let repaint = ctx.clone();
            lightcraft_engine::devices::on_change(move || repaint.request_repaint());
            // "is the original there?" (grid, Info panel, Missing Photos) answers from a cache a
            // worker fills: a sleeping NAS or a dropped share never stalls a frame
            let repaint = ctx.clone();
            self.session.media.availability.run_in_background(std::sync::Arc::new(move || repaint.request_repaint()));
            self.styled = true;
        } else {
            self.fonts_ready = true;
        }
        panels::notices::logic(self);
        // closing the window (or Quit) with changes only in memory: retry, else ask first
        if ctx.input(|i| i.viewport().close_requested()) && !panels::notices::may_close(self) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let now = ctx.input(|i| i.time);
        let dt = now - self.last_time;
        if dt > 0.0 {
            self.perf.fps = self.perf.fps * 0.9 + (1.0 / dt).min(240.0) * 0.1;
        }
        self.last_time = now;
        self.drain_control(ctx);
        self.apply_settings(ctx);
        if !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
        self.renderer.poll(ctx, &mut self.session);
        merge::poll(self, ctx);
        import::poll_scan(self, ctx);
        import::tick(self, ctx);
        tasks::poll(self, ctx);
        self.preview_build_status(ctx);
        self.save_status(ctx);
        self.slideshow_tick(ctx);
        // back from an external editor: pick up the files it saved
        let focused = ctx.input(|i| i.focused);
        if focused && !self.ui.was_focused && !self.ui.external_edits.is_empty() {
            let ids = self.ui.external_edits.clone();
            if let Ok(r) = self.session.execute("photo.reload", &serde_json::json!({"ids": ids}))
                && r["reloaded"].as_array().is_some_and(|a| !a.is_empty())
            {
                self.toast(ctx, "Updated the edits saved in the external editor");
            }
        }
        self.ui.was_focused = focused;
        // auto import: list the watched folder every few seconds (on a worker thread: it may be on
        // a network share), then import what's new like any import (also on a worker thread)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(folder) = self.session.import_defaults.auto_folder.clone() {
            const LABEL: &str = "Auto Import";
            let now = ctx.input(|i| i.time);
            if now - self.ui.auto_import_at >= 3.0 && self.import.is_none() && !self.tasks.is_running(LABEL) {
                self.ui.auto_import_at = now;
                let work = move || lightcraft_engine::cmd::library::list_auto_import_folder(&folder);
                let done = |app: &mut LightcraftApp, _ctx: &egui::Context, listing: Result<Vec<(String, u64)>, String>| {
                    let listing = match listing {
                        Ok(l) => l,
                        Err(e) => return log::debug!("auto import: {e}"),
                    };
                    let p = serde_json::json!({"listing": listing, "start": false});
                    let Ok(r) = app.session.execute("library.autoImportScan", &p) else { return };
                    let Some(mut params) = r.get("import").cloned().filter(|_| app.import.is_none()) else { return };
                    let paths: Vec<String> =
                        params["paths"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
                    if let Some(o) = params.as_object_mut() {
                        o.remove("paths");
                    }
                    let undo0 = app.session.undo.len();
                    app.import = Some(import::ImportTask::new(paths, params, undo0, false).auto());
                };
                if let Err(e) = tasks::spawn(self, LABEL, work, done) {
                    log::warn!("{e}");
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(3));
        }
        self.session.persist_if_dirty();
        self.collect_screenshots(ctx);
        self.issue_screenshots(ctx);
        if self.fonts_ready {
            shortcuts::handle(self, ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let dropped: Vec<String> =
                ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_string_lossy().to_string()).filter(|p| !p.is_empty()).collect());
            // preset files import as presets, everything else as photos
            let (presets, photos): (Vec<String>, Vec<String>) = dropped.into_iter().partition(|p| is_preset_file(p));
            if !presets.is_empty() {
                let _ = self.run("file.importPresets", serde_json::json!({"paths": presets}));
            }
            // read and added on a worker thread (dropped folders can be large, or on a slow drive)
            if !photos.is_empty()
                && let Err(e) = import::start_paths(self, photos)
            {
                self.toast(ctx, e);
            }
        }
    }

    /// Apply settings that act outside the UI state: GPU rendering and window full screen.
    fn apply_settings(&mut self, ctx: &egui::Context) {
        if self.gpu_applied != Some(self.ui.settings.gpu) {
            self.gpu_applied = Some(self.ui.settings.gpu);
            let _ = self.session.execute("app.gpu", &serde_json::json!({"enabled": self.ui.settings.gpu}));
            // GPU device + kernels off the UI thread, once the window is up and only when GPU
            // rendering is on: a broken driver must not keep the window from appearing (issue #136)
            if self.ui.settings.gpu {
                lightcraft_engine::gpu::warm_up();
            }
        }
        let mb = self.ui.settings.memory_mb;
        // automatic at startup: leave the engine's default alone
        if self.memory_applied != Some(mb) && (mb > 0 || self.memory_applied.is_some()) {
            let mb = if mb == 0 { (lightcraft_engine::memory::default_budget() >> 20) as u32 } else { mb };
            let _ = self.session.execute("app.memoryBudget", &serde_json::json!({"mb": mb}));
        }
        self.memory_applied = Some(mb);
        if let Some(fs) = ctx.input(|i| i.viewport().fullscreen) {
            self.window_is_fullscreen = fs;
        }
        if let Some(on) = self.ui.window_fullscreen.take() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
            self.window_is_fullscreen = on;
        }
    }

    /// Inject synthetic events (pointer events one per frame; key sequences up to the release).
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        // the frame after a synthetic release / key: modifiers back up
        if std::mem::take(&mut self.synthetic_mods_release) && self.synthetic_mods != egui::Modifiers::NONE {
            self.synthetic_mods = egui::Modifiers::NONE;
            raw.events.push(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        }
        if self.synthetic.is_empty() {
            return;
        }
        let n = match self.synthetic[0] {
            egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. } => 1,
            _ => self.synthetic.iter().position(|e| matches!(e, egui::Event::Key { pressed: false, .. })).map_or(self.synthetic.len(), |i| i + 1),
        };
        if let Some(egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. }) = self.synthetic.first() {
            raw.events.push(egui::Event::PointerMoved(*p));
        }
        // egui's `input.modifiers` follow `ModifiersChanged` events, not the modifiers carried by
        // pointer/key events: announce the injected events' modifiers (held from button down to
        // the frame after up), so ⌥-drag, ⇧-click … work through the control channel
        let carried = self.synthetic[..n].iter().find_map(|e| match e {
            egui::Event::PointerButton { pressed, modifiers, .. } => Some((*modifiers, !pressed)),
            egui::Event::Key { modifiers, .. } => Some((*modifiers, true)),
            _ => None,
        });
        if let Some((m, ends)) = carried {
            if m != self.synthetic_mods {
                self.synthetic_mods = m;
                raw.events.push(egui::Event::ModifiersChanged(m));
            }
            self.synthetic_mods_release = ends;
        }
        raw.events.extend(self.synthetic.drain(..n));
    }

    /// Frame timings once layout is done (`t0`: when layout started).
    fn end_frame(&mut self, t0: f64) {
        self.perf.frame_ms = now_ms() - t0;
        self.perf.update_ms = self.perf.logic_ms + self.perf.frame_ms;
        self.perf.max_update_ms = self.perf.max_update_ms.max(self.perf.update_ms);
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        i18n::set_language(self.ui.language);
        let ctx = ui.ctx().clone();
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        let t0 = now_ms();
        if std::mem::take(&mut self.ui.quit) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // panels set it again this frame while the pointer rests on a preset or profile
        self.hover_preview = None;
        if self.ui.fullscreen {
            // full-screen preview: the photo alone on black
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(egui::Color32::BLACK)).show(ui, |ui| panels::detail::show(self, ui));
            panels::second::show(self, &ctx);
            panels::notices::show(self, &ctx);
            panels::dialogs::show(self, &ctx);
            panels::library_problem::show(self, &ctx);
            panels::toast(self, &ctx);
            self.widgets = widgets::take_registry(&ctx);
            self.end_frame(t0);
            return;
        }
        // Order matters: earlier panels take the full edge (top bar spans the window; the tool strip,
        // right panels and left panel run to the bottom; the bottom bar sits between them).
        panels::topbar::show(self, ui);
        panels::library_problem::banner(self, ui);
        panels::strip::show(self, ui);
        if self.ui.right != state::RightPanel::None {
            panels::right::show(self, ui);
        }
        if self.ui.presets {
            panels::presets::show(self, ui);
        }
        if self.ui.left_panel {
            panels::left::show(self, ui);
        }
        panels::bottombar::show(self, ui);
        let t = theme::Tokens::get(&ctx);
        let bg = if matches!(self.ui.view, state::ViewMode::Detail | state::ViewMode::Compare | state::ViewMode::Survey | state::ViewMode::Reference)
        {
            t.canvas
        } else {
            t.grid_bg
        };
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(bg)).show(ui, |ui| match self.ui.view {
            state::ViewMode::PhotoGrid | state::ViewMode::SquareGrid => panels::grid::show(self, ui),
            state::ViewMode::Detail => panels::detail::show(self, ui),
            state::ViewMode::Compare => panels::compare::show_compare(self, ui),
            state::ViewMode::Survey => panels::compare::show_survey(self, ui),
            state::ViewMode::Reference => panels::compare::show_reference(self, ui),
        });
        panels::second::show(self, &ctx);
        panels::notices::show(self, &ctx);
        panels::dialogs::show(self, &ctx);
        panels::library_problem::show(self, &ctx);
        import::progress(self, &ctx);
        import::scan_progress(self, &ctx);
        export_task::poll(self, &ctx);
        panels::grid::drag_feedback(self, &ctx);
        panels::toast(self, &ctx);
        self.widgets = widgets::take_registry(&ctx);
        self.end_frame(t0);
    }
}

/// How long a screenshot waits for in-flight renders.
const SCREENSHOT_SETTLE_MS: f64 = 3000.0;
/// How long a windowed screenshot waits for the compositor before falling back to headless.
const SCREENSHOT_FALLBACK_MS: f64 = 2000.0;

/// A `ui.screenshot` request in progress.
struct PendingShot {
    token: u64,
    path: Option<String>,
    reply: Sender<ControlResponse>,
    headless: bool,
    not_before: f64,
    deadline: f64,
    frames: u32,
    /// When the viewport screenshot command went out.
    sent_at: Option<f64>,
}

/// Wall-clock milliseconds since the Unix epoch (`web-time` maps to `Date.now()` on the web).
pub fn now_ms() -> f64 {
    use web_time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

/// Whether the settings render in black & white.
/// A temporary look for the loupe (hovering a preset or profile).
#[derive(Clone, Debug, PartialEq)]
pub struct HoverPreview {
    /// What is previewed (e.g. "Preset: Warm Glow").
    pub label: String,
    /// The photo's settings with the look applied.
    pub settings: lightcraft_develop::DevelopSettings,
}

pub fn is_bw(d: &lightcraft_develop::DevelopSettings) -> bool {
    d.treatment == lightcraft_develop::Treatment::Bw || d.profile.id == "lc.mono" || d.profile.id.starts_with("lc.bw.")
}

/// Files dropped on the window that are presets rather than photos.
pub fn is_preset_file(path: &str) -> bool {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    ["lcpreset", "lrtemplate", "xmp", "zip", "cube", "lmp", "mplumpack"].contains(&ext.as_str())
}

#[cfg(test)]
mod drop_tests {
    #[test]
    fn dropped_presets_are_told_apart_from_photos() {
        for p in ["/a/Look.lrtemplate", "/a/b.XMP", "/a/pack.zip", "/a/x.lcpreset", "/a/Magic Hour.mplumpack", "/a/Pop.lmp", "/a/Bundle.LMP"] {
            assert!(super::is_preset_file(p), "{p}");
        }
        for p in ["/a/IMG_1.CR2", "/a/b.dng", "/a/c.jpg", "/a/folder"] {
            assert!(!super::is_preset_file(p), "{p}");
        }
    }
}

/// Results recomputed only when the catalog (or their inputs) change.
#[derive(Default)]
pub struct Caches {
    keyword_tree: Option<(u64, std::sync::Arc<Vec<lightcraft_catalog::KeywordNode>>)>,
    suggestions: Option<(u64, std::sync::Arc<Vec<String>>)>,
    counts: Option<(u64, LibraryCounts)>,
    date_groups: Option<(u64, std::sync::Arc<Vec<lightcraft_catalog::DateGroup>>)>,
    filter_values: Option<(u64, std::sync::Arc<FilterValues>)>,
    album_counts: Option<(u64, std::sync::Arc<std::collections::HashMap<lightcraft_catalog::AlbumId, usize>>)>,
    /// How often the album counts were recomputed (tests check that unchanged frames don't).
    pub album_count_scans: usize,
    /// The grid's date runs, layout and indexes (by the visible list's generation).
    pub grid: panels::grid::GridCache,
    /// What the grid did on its frames (benchmarks and tests check unchanged frames stay cheap).
    pub grid_stats: panels::grid::GridStats,
}

/// The left panel's counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct LibraryCounts {
    pub total: usize,
    pub picks: usize,
    pub deleted: usize,
}

/// The values the filter bar's pickers offer.
#[derive(Clone, Debug, Default)]
pub struct FilterValues {
    pub cameras: Vec<String>,
    pub lenses: Vec<String>,
    pub keywords: Vec<String>,
}

pub(crate) fn key_of(parts: impl std::hash::Hash) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one(parts)
}

impl Caches {
    /// The library's keyword tree.
    pub fn keyword_tree(&mut self, cat: &lightcraft_catalog::Catalog) -> std::sync::Arc<Vec<lightcraft_catalog::KeywordNode>> {
        match &self.keyword_tree {
            Some((r, t)) if *r == cat.revision => t.clone(),
            _ => {
                let t = std::sync::Arc::new(cat.keyword_tree());
                self.keyword_tree = Some((cat.revision, t.clone()));
                t
            }
        }
    }
    /// All Photos / Picks / Recently Deleted counts.
    pub fn counts(&mut self, cat: &lightcraft_catalog::Catalog) -> LibraryCounts {
        if let Some((r, c)) = self.counts
            && r == cat.revision
        {
            return c;
        }
        let mut c = LibraryCounts::default();
        for p in cat.photos() {
            if p.in_library() {
                c.total += 1;
                if p.flag == lightcraft_catalog::Flag::Pick {
                    c.picks += 1;
                }
            } else if p.deleted && !p.local {
                c.deleted += 1;
            }
        }
        self.counts = Some((cat.revision, c));
        c
    }
    /// The By Date tree.
    pub fn date_groups(&mut self, cat: &lightcraft_catalog::Catalog) -> std::sync::Arc<Vec<lightcraft_catalog::DateGroup>> {
        match &self.date_groups {
            Some((r, g)) if *r == cat.revision => g.clone(),
            _ => {
                let g = std::sync::Arc::new(cat.date_groups());
                self.date_groups = Some((cat.revision, g.clone()));
                g
            }
        }
    }
    /// Cameras, lenses and keywords in the library (filter bar pickers).
    pub fn filter_values(&mut self, cat: &lightcraft_catalog::Catalog) -> std::sync::Arc<FilterValues> {
        match &self.filter_values {
            Some((r, v)) if *r == cat.revision => v.clone(),
            _ => {
                let distinct = |mut v: Vec<String>| {
                    v.retain(|s| !s.trim().is_empty());
                    v.sort_by_key(|s| s.to_lowercase());
                    v.dedup();
                    v
                };
                let lib = || cat.photos().filter(|p| p.in_library());
                let v = std::sync::Arc::new(FilterValues {
                    cameras: distinct(lib().map(|p| p.meta.camera.clone()).collect()),
                    lenses: distinct(lib().map(|p| p.meta.lens.clone()).collect()),
                    keywords: cat.keywords().into_iter().map(|(k, _)| k).collect(),
                });
                self.filter_values = Some((cat.revision, v.clone()));
                v
            }
        }
    }
    /// Keyword suggestions for a photo with `current` keywords and the typed `prefix`.
    pub fn suggestions(&mut self, cat: &lightcraft_catalog::Catalog, current: &[String], prefix: &str, n: usize) -> std::sync::Arc<Vec<String>> {
        let k = key_of((cat.revision, current, prefix, n));
        match &self.suggestions {
            Some((h, v)) if *h == k => v.clone(),
            _ => {
                let v = std::sync::Arc::new(cat.keyword_suggestions(current, prefix, n));
                self.suggestions = Some((k, v.clone()));
                v
            }
        }
    }
    /// Every album's photo count for the sidebar (a smart album evaluates its rules, which
    /// scans the catalog). Recomputed when the catalog changes (photos, metadata, album rules
    /// all bump its revision) and — only while some smart album has an "in the last…" rule —
    /// when `now` (the session clock, ISO) enters a new minute, so such counts follow the clock
    /// within a minute without rescanning every frame.
    pub fn album_counts(
        &mut self,
        cat: &lightcraft_catalog::Catalog,
        now: &str,
    ) -> std::sync::Arc<std::collections::HashMap<lightcraft_catalog::AlbumId, usize>> {
        let relative = cat.albums().any(|a| a.smart.as_ref().is_some_and(|f| f.depends_on_now()));
        let minute = if relative { now.get(..16).unwrap_or(now) } else { "" };
        let k = key_of((cat.revision, minute));
        match &self.album_counts {
            Some((h, v)) if *h == k => v.clone(),
            _ => {
                if relative {
                    // "in the last N days" counts back from the session clock, as in the grid
                    lightcraft_catalog::rules::set_now(Some(now.to_string()));
                }
                let v: std::sync::Arc<std::collections::HashMap<_, _>> =
                    std::sync::Arc::new(cat.albums().map(|a| (a.id, cat.album_count(a.id))).collect());
                self.album_count_scans += 1;
                self.album_counts = Some((k, v.clone()));
                v
            }
        }
    }
}

#[cfg(test)]
mod cache_tests {
    use lightcraft_catalog::{Album, AlbumId, Catalog, Filter, Op, Photo, PhotoId, RuleSet, Source};

    fn photo(id: u64, captured: &str, rating: u8) -> Op {
        let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("p{id}.jpg"), "JPEG", 60, 40, "2026-01-01T00:00:00");
        p.captured = Some(captured.into());
        p.rating = rating;
        Op::AddPhoto { photo: Box::new(p) }
    }

    fn smart(id: u64, rules: serde_json::Value) -> Op {
        let mut a = Album::new(AlbumId(id), format!("smart {id}"));
        let rules: RuleSet = serde_json::from_value(rules).unwrap();
        a.smart = Some(Box::new(Filter { rule_set: Some(rules), ..Default::default() }));
        Op::AddAlbum { album: a }
    }

    /// Smart-album counts: unchanged frames reuse them; metadata and rule changes and (for
    /// "in the last…" rules) the clock crossing into a new minute recompute them.
    #[test]
    fn smart_album_counts_are_cached_until_something_changes() {
        let mut cat = Catalog::default();
        let mut c = super::Caches::default();
        for (i, d) in ["2026-09-30T11:59:30", "2026-09-29T08:00:00", "2026-01-01T08:00:00"].iter().enumerate() {
            cat.apply(photo(i as u64 + 1, d, if i == 0 { 5 } else { 1 })).unwrap();
        }
        cat.apply(smart(10, serde_json::json!({"rules": [{"field": "rating", "op": "gte", "value": 3}]}))).unwrap();
        cat.apply(smart(11, serde_json::json!({"rules": [{"field": "captureDate", "op": "inLast", "value": {"n": 1, "unit": "hours"}}]}))).unwrap();
        let now = "2026-09-30T12:00:00";
        let n = c.album_counts(&cat, now);
        assert_eq!((n[&AlbumId(10)], n[&AlbumId(11)]), (1, 1));
        assert_eq!(c.album_count_scans, 1);
        for _ in 0..10 {
            c.album_counts(&cat, "2026-09-30T12:00:40");
        }
        assert_eq!(c.album_count_scans, 1, "unchanged frames in the same minute don't rescan");
        // metadata change
        cat.apply(Op::SetRating { id: PhotoId(2), rating: 4 }).unwrap();
        assert_eq!(c.album_counts(&cat, now)[&AlbumId(10)], 2);
        // rule change
        let rs: RuleSet = serde_json::from_value(serde_json::json!({"rules": [{"field": "rating", "op": "gte", "value": 5}]})).unwrap();
        cat.apply(Op::SetAlbumRules { id: AlbumId(10), rules: Box::new(Filter { rule_set: Some(rs), ..Default::default() }) }).unwrap();
        assert_eq!(c.album_counts(&cat, now)[&AlbumId(10)], 1);
        // an hour later the 11:59:30 photo has left "in the last hour", with no catalog change
        assert_eq!(c.album_counts(&cat, "2026-09-30T13:00:10")[&AlbumId(11)], 0);
        assert_eq!(c.album_count_scans, 4);
        lightcraft_catalog::rules::set_now(None);
    }

    /// Without "in the last…" rules the clock never causes a rescan.
    #[test]
    fn absolute_rules_ignore_the_clock() {
        let mut cat = Catalog::default();
        let mut c = super::Caches::default();
        cat.apply(photo(1, "2026-09-30T11:59:30", 5)).unwrap();
        cat.apply(smart(10, serde_json::json!({"rules": [{"field": "rating", "op": "gte", "value": 3}]}))).unwrap();
        c.album_counts(&cat, "2026-09-30T12:00:00");
        c.album_counts(&cat, "2027-01-01T00:00:00");
        assert_eq!(c.album_count_scans, 1);
    }
}
