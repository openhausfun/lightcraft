//! Background export: the Export dialog, File → Export with Preset and Export with Previous hand
//! their batch to a worker thread so the window stays responsive. Photos are prepared on the UI
//! thread ([`lightcraft_engine::export::prepare_export`]: cheap, needs the session) and rendered,
//! encoded and written on the worker; a progress panel shows the count and a Cancel button.
//!
//! Needs [`crate::Services::write_shared`] (a thread-safe writer); without it (web) the batch runs
//! synchronously as before.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use egui::Align2;
use lightcraft_engine::export::{Destination, ExportOptions, PreparedExport, run_batch};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::theme::Tokens;

pub struct ExportTask {
    pub total: usize,
    /// Photos started so far and the file in progress.
    pub progress: Arc<Mutex<(usize, String)>>,
    pub cancel: Arc<AtomicBool>,
    rx: Receiver<Result<Vec<Value>, String>>,
}

impl ExportTask {
    /// `{total, done, current}` for `ui.inspect`.
    pub fn status(&self) -> Value {
        let (done, current) = self.progress.lock().map(|g| g.clone()).unwrap_or_default();
        json!({"total": self.total, "done": done, "current": current})
    }
}

/// Start exporting `items` in the background. Errors per photo are collected, not fatal.
pub fn start(app: &mut LightcraftApp, items: Vec<PreparedExport>, opts: ExportOptions, to: Destination) -> Result<Value, String> {
    if app.export.is_some() {
        return Err("an export is already running".into());
    }
    let write = app.services.write_shared.clone().ok_or("no background writer")?;
    let total = items.len();
    let progress = Arc::new(Mutex::new((0, String::new())));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = channel();
    let (p, c) = (progress.clone(), cancel.clone());
    let work = move || {
        let mut w = |path: &str, bytes: &[u8]| write(path, bytes);
        let r = run_batch(items, &opts, &to, &mut w, &|path| std::path::Path::new(path).exists(), false, &mut |done, name| {
            if let Ok(mut g) = p.lock() {
                *g = (done, name.to_string());
            }
            !c.load(Ordering::Relaxed)
        });
        let _ = tx.send(r);
    };
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::Builder::new().name("export".into()).spawn(work).map_err(|e| e.to_string())?;
    #[cfg(target_arch = "wasm32")]
    work();
    app.export = Some(ExportTask { total, progress, cancel, rx });
    Ok(json!({"background": true, "total": total}))
}

/// Per frame: draw the progress panel; when the batch finishes, report it.
pub fn poll(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(task) = &app.export else { return };
    match task.rx.try_recv() {
        Ok(r) => {
            let cancelled = task.cancel.load(Ordering::Relaxed);
            let total = task.total;
            app.export = None;
            let msg = match r {
                Ok(files) => {
                    let ok = files.iter().filter(|f| f.get("path").is_some()).count();
                    let failed: Vec<&Value> = files.iter().filter(|f| f.get("error").is_some()).collect();
                    let skipped = files.iter().filter(|f| f.get("skipped").is_some()).count();
                    let mut m =
                        crate::i18n::tr_format!("Exported {ok} of {total} photo{}", if total == 1 { "" } else { "s" }, ok = ok, total = total);
                    if skipped > 0 {
                        m += &crate::i18n::tr_format!(" · {skipped} skipped (file exists)", skipped = skipped);
                    }
                    if let Some(first) = failed.first() {
                        m += &crate::i18n::tr_format!(" · {} failed: {}", failed.len(), first["error"].as_str().unwrap_or("error"));
                    }
                    if cancelled {
                        m += " · cancelled";
                    }
                    app.last_export_result = Some(json!({"files": files, "cancelled": cancelled}));
                    m
                }
                Err(e) => e,
            };
            app.toast(ctx, msg);
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            let (done, current) = task.progress.lock().map(|g| g.clone()).unwrap_or_default();
            let (total, cancel) = (task.total, task.cancel.clone());
            let t = Tokens::get(ctx);
            egui::Window::new(crate::i18n::tr("Exporting"))
                .title_bar(false)
                .resizable(false)
                .anchor(Align2::LEFT_BOTTOM, [16.0, -56.0])
                .fixed_size([300.0, 64.0])
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(crate::i18n::tr_format!("Exporting {} of {total}", (done + 1).min(total), total = total))
                                .color(t.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let stopping = cancel.load(Ordering::Relaxed);
                            if crate::widgets::text_button(ui, "exportCancel", if stopping { "Stopping…" } else { "Cancel" }, false).clicked() {
                                cancel.store(true, Ordering::Relaxed);
                            }
                        });
                    });
                    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(280.0));
                    ui.label(egui::RichText::new(current).size(11.0).color(t.text_dim));
                });
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            app.export = None;
            app.toast(ctx, "Export stopped unexpectedly");
        }
    }
}
