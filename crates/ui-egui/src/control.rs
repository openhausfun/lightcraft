//! Programmatic control of the running app (agents, tests, MCP).
//!
//! Methods (JSON-lines over the host's transport):
//! - `engine.execute {command, params}` / `ui.menu.invoke`: run any engine or UI command
//! - `engine.commands`: engine + UI commands with enablement
//! - `ui.inspect`: UI state, panels, view, canvas/image rects, perf
//! - `ui.widgets {filter?}`: every registered widget id → rect (sliders, buttons, thumbnails…)
//! - `ui.clickWidget {id, count?}`: click a widget by automation id (real egui input)
//! - `ui.dragWidget {id, toX?, toY?, dx?, dy?}`: drag from a widget's centre
//! - `ui.move/click/drag {x, y, …}`: raw pointer input in screen points
//! - `ui.pointer {events: [{kind: down|drag|up, x, y}], space: "image"}`: gestures in normalized
//!   image coordinates (brush strokes, crop handles…) — mapped to screen and injected as real input
//! - `ui.key {key, shift?, alt?, cmd?}` / `ui.text {text}` / `ui.scroll {dx, dy}`
//! - `ui.set {view?, panel?, filmstrip?, leftPanel?, zoom?, …}` / `ui.dialog.confirm|cancel`
//! - `ui.screenshot {path?, headless?}` (headless: CPU-rendered, no compositor needed), `ui.resize {width, height}`, `ui.render {id?, size?, path?}`, `app.quit`

use std::sync::mpsc::Sender;

use serde_json::{Value, json};

use crate::LightcraftApp;

pub type ControlResponse = Value;

pub struct ControlRequest {
    pub method: String,
    pub params: Value,
    pub reply: Sender<ControlResponse>,
}

impl ControlRequest {
    pub fn new(method: impl Into<String>, params: Value) -> (Self, std::sync::mpsc::Receiver<ControlResponse>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (Self { method: method.into(), params, reply: tx }, rx)
    }
}

pub enum Outcome {
    Done(Value),
    Screenshot { path: Option<String>, headless: bool },
}

fn ok(v: Value) -> Outcome {
    Outcome::Done(json!({"ok": true, "result": v}))
}
fn err(e: impl std::fmt::Display) -> Outcome {
    Outcome::Done(json!({"ok": false, "error": e.to_string()}))
}
fn wrap(r: Result<Value, String>) -> Outcome {
    match r {
        Ok(v) => ok(v),
        Err(e) => err(e),
    }
}

pub fn all_commands(app: &LightcraftApp) -> Value {
    let mut v: Vec<Value> = app.session.commands().into_iter().map(|c| serde_json::to_value(c).unwrap_or_default()).collect();
    for (id, label, sc, menu) in crate::menus::UI_COMMANDS {
        v.push(json!({"id": id, "label": label, "shortcut": sc, "menu": [menu], "enabled": crate::menus::ui_enabled(app, id), "ui": true}));
    }
    Value::Array(v)
}

fn rect_json(r: egui::Rect) -> Value {
    json!([r.left(), r.top(), r.width(), r.height()])
}

pub fn inspect(app: &LightcraftApp, ctx: &egui::Context) -> Value {
    let r = ctx.content_rect();
    json!({
        "ui": serde_json::to_value(&app.ui).unwrap_or_default(),
        "window": [r.width(), r.height()],
        "pixelsPerPoint": ctx.pixels_per_point(),
        "canvasRect": app.canvas_rect.map(rect_json),
        "imageRect": app.image_rect.map(rect_json),
        "scroll": {"grid": app.grid_scroll, "filmstrip": app.film_scroll},
        "active": app.session.active().map(|p| p.0),
        "selection": app.session.selection.ids.iter().map(|p| p.0).collect::<Vec<_>>(),
        "activeMask": app.session.active_mask,
        "widgetCount": app.widgets.len(),
        "perf": {"frameMs": app.perf.frame_ms, "logicMs": app.perf.logic_ms, "updateMs": app.perf.update_ms, "maxUpdateMs": app.perf.max_update_ms, "fps": app.perf.fps, "lastRenderMs": app.renderer.last_main_ms, "renderQueue": app.renderer.queued(), "rendersInFlight": app.renderer.in_flight(), "pendingSlots": app.renderer.pending_slots(), "mergeRunning": app.merge.busy(), "lastMerge": app.merge.last_result, "rendersDone": app.renderer.completed, "thumbTextures": app.renderer.thumb_textures(), "variantTextures": app.renderer.variant_textures(), "gpu": (lightcraft_engine::gpu::ready() && lightcraft_engine::gpu::available()).then(lightcraft_engine::gpu::adapter_name).flatten(), "gpuReason": lightcraft_engine::gpu::unavailable_reason(), "gpuFallback": lightcraft_engine::gpu::last_fallback()},
        "loupe": app.loupe_shown.map(|(p, src)| json!({"photo": p.0, "source": src, "pending": app.renderer.is_pending(crate::render::Slot::Main)})),
        "hoverPreview": app.hover_preview.as_ref().map(|h| h.label.clone()),
        "status": app.ui.status,
        "notices": app.notices,
        "quitPrompt": app.quit_prompt,
        "unsaved": app.session.unsaved().map(|(n, e)| json!({"ops": n, "error": e})),
        "libraryProblem": app.library_problem.as_ref().map(crate::panels::library_problem::LibraryProblem::to_json),
        "scan": app.scan.as_ref().map(crate::import::ScanTask::status),
        "export": {"running": app.export.as_ref().map(crate::export_task::ExportTask::status), "last": app.last_export_result},
        "import": app.import.as_ref().map(crate::import::ImportTask::status),
        "tasks": app.tasks.labels(),
        "memory": memory(app),
    })
}

/// Bytes held by the engine's caches and the renderer's (`ui.inspect` → `memory`).
pub fn memory(app: &LightcraftApp) -> Value {
    let mut v = serde_json::to_value(app.session.memory_report()).unwrap_or_default();
    if let (Some(o), Value::Object(r)) = (v.as_object_mut(), app.renderer.memory()) {
        o.extend(r);
    }
    v
}

fn modifiers(p: &Value) -> egui::Modifiers {
    let b = |n: &str| p.get(n).and_then(Value::as_bool).unwrap_or(false);
    egui::Modifiers { alt: b("alt"), ctrl: b("ctrl"), shift: b("shift"), mac_cmd: b("cmd") && cfg!(target_os = "macos"), command: b("cmd") }
}

fn key_from(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name).or(match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Some(egui::Key::Enter),
        "esc" | "escape" => Some(egui::Key::Escape),
        "delete" | "backspace" => Some(egui::Key::Backspace),
        "left" => Some(egui::Key::ArrowLeft),
        "right" => Some(egui::Key::ArrowRight),
        "up" => Some(egui::Key::ArrowUp),
        "down" => Some(egui::Key::ArrowDown),
        "space" => Some(egui::Key::Space),
        "tab" => Some(egui::Key::Tab),
        "\\" | "backslash" => Some(egui::Key::Backslash),
        "/" | "slash" => Some(egui::Key::Slash),
        "[" => Some(egui::Key::OpenBracket),
        "]" => Some(egui::Key::CloseBracket),
        _ => None,
    })
}

fn push_drag(app: &mut LightcraftApp, a: egui::Pos2, b: egui::Pos2, steps: u64, m: egui::Modifiers) {
    app.synthetic.push(egui::Event::PointerMoved(a));
    app.synthetic.push(egui::Event::PointerButton { pos: a, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
    for i in 1..=steps.max(1) {
        let t = i as f32 / steps.max(1) as f32;
        app.synthetic.push(egui::Event::PointerMoved(a + (b - a) * t));
    }
    app.synthetic.push(egui::Event::PointerButton { pos: b, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
}

fn widget_rect(app: &LightcraftApp, id: &str) -> Option<egui::Rect> {
    app.widgets.iter().rev().find(|(w, _)| w == id).map(|(_, r)| *r)
}

pub fn handle(app: &mut LightcraftApp, ctx: &egui::Context, req: &ControlRequest) -> Outcome {
    let p = &req.params;
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    let f = |k: &str| p.get(k).and_then(Value::as_f64);
    match req.method.as_str() {
        "engine.execute" | "ui.menu.invoke" | "command" => {
            let Some(id) = s("command").or(s("id")) else { return err("missing `command`") };
            let params = p.get("params").cloned().filter(|v| !v.is_null()).unwrap_or(json!({}));
            let r = app.run(id, params);
            ctx.request_repaint();
            wrap(r)
        }
        "engine.commands" => ok(all_commands(app)),
        "ui.menu.list" => ok(serde_json::to_value(crate::menus::menu_entries(app)).unwrap_or_default()),
        "ui.menu.tree" => {
            ok(Value::Array(crate::menubar::menu_bar(app).into_iter().map(|(title, items)| json!({"label": title, "children": items})).collect()))
        }
        "ui.inspect" => ok(inspect(app, ctx)),
        "ui.widgets" => {
            let filter = s("filter").unwrap_or("");
            ok(Value::Array(
                app.widgets.iter().filter(|(id, _)| id.contains(filter)).map(|(id, r)| json!({"id": id, "rect": rect_json(*r)})).collect(),
            ))
        }
        "ui.clickWidget" | "ui.dragWidget" | "ui.hoverWidget" => {
            let Some(id) = s("id") else { return err("missing `id`") };
            let Some(r) = widget_rect(app, id) else { return err(format!("no widget `{id}` on screen (see ui.widgets)")) };
            let m = modifiers(p);
            // optional relative position inside the widget (0..1)
            let at = egui::pos2(r.left() + r.width() * f("fx").unwrap_or(0.5) as f32, r.top() + r.height() * f("fy").unwrap_or(0.5) as f32);
            if req.method == "ui.hoverWidget" {
                app.synthetic.push(egui::Event::PointerMoved(at));
            } else if req.method == "ui.clickWidget" {
                let n = p.get("count").and_then(Value::as_u64).unwrap_or(1);
                app.synthetic.push(egui::Event::PointerMoved(at));
                for _ in 0..n {
                    app.synthetic.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
                    app.synthetic.push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: m });
                }
            } else {
                let to = egui::pos2(
                    f("toX").map(|v| v as f32).unwrap_or(at.x + f("dx").unwrap_or(0.0) as f32),
                    f("toY").map(|v| v as f32).unwrap_or(at.y + f("dy").unwrap_or(0.0) as f32),
                );
                push_drag(app, at, to, p.get("steps").and_then(Value::as_u64).unwrap_or(10), m);
            }
            ctx.request_repaint();
            ok(json!({"rect": rect_json(r)}))
        }
        "ui.move" => {
            app.synthetic.push(egui::Event::PointerMoved(egui::pos2(f("x").unwrap_or(0.0) as f32, f("y").unwrap_or(0.0) as f32)));
            ctx.request_repaint();
            ok(Value::Null)
        }
        "ui.click" | "ui.drag" => {
            let a = egui::pos2(f("x").unwrap_or(0.0) as f32, f("y").unwrap_or(0.0) as f32);
            let m = modifiers(p);
            let button = if s("button") == Some("right") { egui::PointerButton::Secondary } else { egui::PointerButton::Primary };
            if req.method == "ui.drag" {
                let b = egui::pos2(f("toX").unwrap_or(0.0) as f32, f("toY").unwrap_or(0.0) as f32);
                push_drag(app, a, b, p.get("steps").and_then(Value::as_u64).unwrap_or(10), m);
            } else {
                app.synthetic.push(egui::Event::PointerMoved(a));
                for _ in 0..p.get("count").and_then(Value::as_u64).unwrap_or(1) {
                    app.synthetic.push(egui::Event::PointerButton { pos: a, button, pressed: true, modifiers: m });
                    app.synthetic.push(egui::Event::PointerButton { pos: a, button, pressed: false, modifiers: m });
                }
            }
            ctx.request_repaint();
            ok(Value::Null)
        }
        "ui.pointer" => {
            // events in normalized image coordinates of the displayed photo frame
            let Some(img) = app.image_rect else { return err("no image on screen (switch to Detail view)") };
            let Some(events) = p.get("events").and_then(Value::as_array) else { return err("missing `events`") };
            let m = modifiers(p);
            for e in events {
                let x = e.get("x").and_then(Value::as_f64).unwrap_or(0.5) as f32;
                let y = e.get("y").and_then(Value::as_f64).unwrap_or(0.5) as f32;
                let pos = egui::pos2(img.left() + x * img.width(), img.top() + y * img.height());
                match e.get("kind").and_then(Value::as_str).unwrap_or("move") {
                    "down" => {
                        app.synthetic.push(egui::Event::PointerMoved(pos));
                        app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: m });
                    }
                    "up" => {
                        app.synthetic.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: m })
                    }
                    _ => app.synthetic.push(egui::Event::PointerMoved(pos)),
                }
            }
            ctx.request_repaint();
            ok(json!({"events": events.len()}))
        }
        "ui.key" => {
            let Some(k) = s("key").and_then(key_from) else { return err("unknown or missing `key`") };
            let m = modifiers(p);
            app.synthetic.push(egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m });
            app.synthetic.push(egui::Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers: m });
            ctx.request_repaint();
            ok(Value::Null)
        }
        "ui.text" => {
            app.synthetic.push(egui::Event::Text(s("text").unwrap_or("").to_string()));
            ctx.request_repaint();
            ok(Value::Null)
        }
        "ui.scroll" => {
            app.synthetic.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(f("dx").unwrap_or(0.0) as f32, f("dy").unwrap_or(0.0) as f32),
                phase: egui::TouchPhase::Move,
                modifiers: modifiers(p),
            });
            ctx.request_repaint();
            ok(Value::Null)
        }
        "ui.set" => {
            let mut v = serde_json::to_value(&app.ui).unwrap_or_default();
            lightcraft_develop::presets::deep_merge(&mut v, p);
            match serde_json::from_value::<crate::UiState>(v) {
                Ok(mut u) => {
                    u.toast = app.ui.toast.clone();
                    u.dialog = app.ui.dialog.clone();
                    u.status = app.ui.status.clone();
                    app.ui = u;
                    ctx.request_repaint();
                    ok(serde_json::to_value(&app.ui).unwrap_or_default())
                }
                Err(e) => err(e),
            }
        }
        "ui.dialog.confirm" => match app.ui.dialog.take() {
            Some(d) => {
                let r = crate::panels::dialogs::confirm_dialog(app, &d);
                // the import review stays open on an error, as with its button
                if r.is_err() && matches!(d, crate::state::Dialog::Import { .. }) {
                    app.ui.dialog = Some(d);
                }
                wrap(r)
            }
            None => err("no dialog open"),
        },
        "ui.dialog.cancel" => {
            app.ui.dialog = None;
            ok(Value::Null)
        }
        "ui.resize" => {
            let w = f("width").unwrap_or(1600.0) as f32;
            let h = f("height").unwrap_or(1000.0) as f32;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
            ok(Value::Null)
        }
        "ui.screenshot" => {
            // never over a photo's original
            if let Some(Err(e)) = s("path").map(|path| app.session.check_write_target(path)) {
                return err(e);
            }
            ctx.request_repaint();
            Outcome::Screenshot { path: s("path").map(str::to_string), headless: p.get("headless").and_then(Value::as_bool).unwrap_or(false) }
        }
        "ui.render" => {
            let id = p.get("id").and_then(Value::as_u64).map(lightcraft_catalog::PhotoId).or(app.session.active());
            let Some(id) = id else { return err("no photo") };
            if let Some(Err(e)) = s("path").map(|path| app.session.check_write_target(path)) {
                return err(e);
            }
            let size = p.get("size").and_then(Value::as_u64).unwrap_or(1600) as usize;
            match app.session.render_now(id, size, size) {
                Ok(r) => match (s("path"), app.services.png.as_ref()) {
                    (Some(path), Some(png)) => {
                        let bytes = png(&r.image);
                        match app.services.write.as_mut() {
                            Some(w) => wrap(w(path, &bytes).map(|_| json!({"path": path, "width": r.image.width, "height": r.image.height}))),
                            None => err("no writer"),
                        }
                    }
                    _ => ok(json!({"width": r.image.width, "height": r.image.height, "histogram": r.histogram})),
                },
                Err(e) => err(e),
            }
        }
        "app.quit" => {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ok(Value::Null)
        }
        other => err(format!("unknown method `{other}`")),
    }
}

/// Default export folder: `~/Pictures/LightCraft Exports` (falls back to the working directory).
pub fn default_export_dir() -> String {
    std::env::var("HOME").map(|h| format!("{h}/Pictures/LightCraft Exports")).unwrap_or_default()
}

/// Export the selected photos (UI command `app.export`). Params: see
/// [`lightcraft_engine::export::ExportOptions::from_json`], plus `dir` (output folder) or `path`
/// (exact output file, single photo), `ids` (default: the selection, else the active photo).
pub fn export_active(app: &mut LightcraftApp, p: &Value) -> Result<Value, String> {
    use lightcraft_engine::export::{Destination, ExportOptions, export_batch};
    let p = &app.session.export_params(p)?;
    let mut opts = ExportOptions::from_json(p);
    if let (Some(path), None) = (p.get("path").and_then(Value::as_str), p.get("format")) {
        let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
        opts.format = lightcraft_engine::export::ExportFormat::parse(ext).unwrap_or(opts.format);
    }
    let ids: Vec<_> = match p.get("ids").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect(),
        None if !app.session.selection.ids.is_empty() => {
            // Batch order (and `{seq}`) follows the grid order.
            let sel: std::collections::HashSet<_> = app.session.selection.ids.iter().copied().collect();
            let mut v: Vec<_> = app.session.visible().iter().copied().filter(|id| sel.contains(id)).collect();
            let shown: std::collections::HashSet<_> = v.iter().copied().collect();
            v.extend(app.session.selection.ids.iter().filter(|id| !shown.contains(id)));
            v
        }
        None => app.session.active().into_iter().collect(),
    };
    if ids.is_empty() {
        return Err("no photo selected".into());
    }
    // no folder given (e.g. File → Export with Preset): the last export's, else the default
    let last_dir = app.session.last_export.as_ref().and_then(|l| l.get("dir")).and_then(Value::as_str).map(str::to_string);
    let dir = p.get("dir").and_then(Value::as_str).map(str::to_string).or(last_dir).filter(|d| !d.is_empty()).unwrap_or_else(default_export_dir);
    let to = Destination { dir: dir.clone(), exact: p.get("path").and_then(Value::as_str).map(str::to_string) };
    let background = p.get("background").and_then(Value::as_bool).unwrap_or(false) && app.services.write_shared.is_some();
    let out = if background {
        let items = lightcraft_engine::export::prepare_batch(&mut app.session, &ids, &opts)?;
        crate::export_task::start(app, items, opts, to)?
    } else {
        let w = app.services.write.as_mut().ok_or("no writer")?;
        json!({"files": export_batch(&mut app.session, &ids, &opts, &to, &mut |path, bytes| w(path, bytes), &|path| std::path::Path::new(path).exists())?})
    };
    // remember for Export with Previous (and to prefill the dialog)
    let mut last = p.clone();
    if let Some(o) = last.as_object_mut() {
        o.remove("ids");
        o.remove("path");
        o.insert("dir".into(), json!(dir));
    }
    app.session.last_export = Some(last);
    let _ = app.session.save_prefs();
    Ok(out)
}

pub fn save_screenshot(app: &mut LightcraftApp, image: &egui::ColorImage, path: Option<&str>) -> Value {
    let [w, h] = image.size;
    let Some(path) = path else {
        return json!({"ok": true, "result": {"width": w, "height": h}});
    };
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    let Some(img) = lightcraft_raster::Rgba8::from_bytes(w, h, &rgba) else {
        return json!({"ok": false, "error": "bad screenshot buffer"});
    };
    let Some(png) = app.services.png.as_ref() else { return json!({"ok": false, "error": "no encoder"}) };
    let bytes = png(&img);
    match app.services.write.as_mut() {
        Some(wr) => match wr(path, &bytes) {
            Ok(()) => json!({"ok": true, "result": {"path": path, "width": w, "height": h}}),
            Err(e) => json!({"ok": false, "error": e}),
        },
        None => json!({"ok": false, "error": "no writer configured"}),
    }
}
