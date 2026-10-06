//! The Masking panel: create masks, list them, edit components and local adjustments.

use egui::{Align2, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ControlSpec, LocalAdjustments, MaskShape, Section, Track};
use serde_json::json;

use super::edit::apply_slider_out;
use super::right::header;
use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register, slider, text_button};

const fn spec(id: &'static str, label: &'static str, min: f64, max: f64, step: f64, decimals: u8, track: Track) -> ControlSpec {
    ControlSpec { id, label, section: Section::Light, min, max, default: 0.0, step, decimals, track }
}

/// Local adjustment sliders (key = `LocalAdjustments` field name).
pub const LOCAL: &[ControlSpec] = &[
    spec("temp", "Temp", -100.0, 100.0, 1.0, 0, Track::Temp),
    spec("tint", "Tint", -100.0, 100.0, 1.0, 0, Track::Tint),
    spec("exposure", "Exposure", -4.0, 4.0, 0.01, 2, Track::Centered),
    spec("contrast", "Contrast", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("highlights", "Highlights", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("shadows", "Shadows", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("whites", "Whites", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("blacks", "Blacks", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("texture", "Texture", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("clarity", "Clarity", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("dehaze", "Dehaze", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("hue", "Hue", -100.0, 100.0, 1.0, 0, Track::Rainbow),
    spec("saturation", "Saturation", -100.0, 100.0, 1.0, 0, Track::Gradient { from: "#7a7a7a", to: "#e04a3a" }),
    spec("sharpness", "Sharpness", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("noise", "Noise", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("moire", "Moiré", -100.0, 100.0, 1.0, 0, Track::Centered),
    spec("defringe", "Defringe", -100.0, 100.0, 1.0, 0, Track::Centered),
];

pub fn local_get(a: &LocalAdjustments, key: &str) -> f64 {
    serde_json::to_value(a).ok().and_then(|v| v.get(key).and_then(|x| x.as_f64())).unwrap_or(0.0)
}

fn kind_label(s: &MaskShape) -> (&'static str, Icon) {
    match s {
        MaskShape::Brush { .. } => ("Brush", Icon::Brush),
        MaskShape::Linear { .. } => ("Linear Gradient", Icon::Linear),
        MaskShape::Radial { .. } => ("Radial Gradient", Icon::Radial),
        MaskShape::ColorRange { .. } => ("Color Range", Icon::Picker),
        MaskShape::LuminanceRange { .. } => ("Luminance Range", Icon::Sliders),
        MaskShape::DepthRange { .. } => ("Depth Range", Icon::Sliders),
        MaskShape::Subject => ("Subject", Icon::Subject),
        MaskShape::Sky => ("Sky", Icon::Sky),
        MaskShape::Background => ("Background", Icon::Subject),
        MaskShape::Object { .. } => ("Object", Icon::Subject),
        MaskShape::People { .. } => ("People", Icon::Subject),
        MaskShape::Landscape { .. } => ("Landscape", Icon::Sky),
    }
}

const TILE: f32 = 52.0;
const TILE_GAP: f32 = 6.0;

/// Columns and tile width of the Create New Mask grid in `width`: four tiles of up to 52 pt (at
/// least 48, so the labels fit), else three (narrowed if even those don't fit).
fn tile_layout(width: f32) -> (usize, f32) {
    let tile = |n: f32| ((width - (n - 1.0) * TILE_GAP) / n).floor().min(TILE);
    if tile(4.0) >= 48.0 { (4, tile(4.0)) } else { (3, tile(3.0).max(24.0)) }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Masking");
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Create New Mask")).color(t.text_dim));
        ui.add_space(6.0);
        let tiles: [(&str, &str, Icon); 8] = [
            ("subject", "Subject", Icon::Subject),
            ("sky", "Sky", Icon::Sky),
            ("background", "Background", Icon::Subject),
            ("brush", "Brush", Icon::Brush),
            ("linear", "Linear", Icon::Linear),
            ("radial", "Radial", Icon::Radial),
            ("luminanceRange", "Luminance", Icon::Sliders),
            ("colorRange", "Color", Icon::Picker),
        ];
        // four 52 pt tiles a row when they fit, else as many as fit (at least three, shrunk)
        let (cols, tile) = tile_layout(ui.available_width());
        egui::Grid::new("mask-tiles").spacing(vec2(TILE_GAP, TILE_GAP)).show(ui, |ui| {
            for (i, (kind, label, icon)) in tiles.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(tile, 52.0), Sense::click());
                register(ui.ctx(), format!("maskNew:{kind}"), r);
                ui.painter().rect_filled(r, 4.0, if resp.hovered() { t.hover } else { t.inset });
                paint(ui.painter(), Rect::from_center_size(r.center() - vec2(0.0, 7.0), vec2(20.0, 20.0)), *icon, t.text_label);
                ui.painter().text(pos2(r.center().x, r.bottom() - 9.0), Align2::CENTER_CENTER, *label, t.font(10.5), t.text_dim);
                if resp.clicked() {
                    match *kind {
                        "colorRange" => {
                            // an empty colour range; clicking the photo samples it
                            let _ = app.run("mask.add", json!({"kind": "colorRange"}));
                            app.ui.tool = "colorRange".into();
                            app.toast(ui.ctx(), "Click the photo to pick a colour · ⇧-click adds more");
                        }
                        "brush" | "linear" | "radial" => {
                            app.ui.tool = kind.to_string();
                            if *kind != "brush" {
                                let _ = app.run("mask.add", json!({"kind": kind}));
                            }
                        }
                        k => {
                            let _ = app.run("mask.add", json!({"kind": k}));
                        }
                    }
                }
                if i % cols == cols - 1 {
                    ui.end_row();
                }
            }
        });
    });
    divider(ui);
    // mask list
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Masks")).font(t.semibold(13.0)).color(t.text_label));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let on = app.ui.mask_overlay;
                if icon_button(ui, "maskOverlay", Icon::Eye, vec2(24.0, 24.0), on, true, "Show overlay (O)").clicked() {
                    app.ui.mask_overlay = !on;
                }
            });
        });
        if d.masks.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No masks yet. Choose a mask type above.")).color(t.text_dim));
        }
        let count = d.masks.len();
        for (index, m) in d.masks.iter().enumerate() {
            let sel = app.session.active_mask == Some(m.id);
            if let Some((rid, name)) = app.ui.renaming_mask.as_mut().filter(|(rid, _)| *rid == m.id) {
                // inline rename: Enter (or leaving the field) commits, Escape cancels
                let rid = *rid;
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width()).id_salt(("maskRename", rid)));
                register(ui.ctx(), format!("maskRename:{rid}"), r.rect);
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.ui.renaming_mask = None;
                } else if r.lost_focus() {
                    let name = name.trim().to_string();
                    app.ui.renaming_mask = None;
                    if !name.is_empty() && name != m.name {
                        let _ = app.run("mask.rename", json!({"id": rid, "name": name}));
                    }
                }
                continue;
            }
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
            register(ui.ctx(), format!("mask:{}", m.id), r);
            ui.painter().rect_filled(
                r,
                4.0,
                if sel {
                    t.tool_active
                } else if resp.hovered() {
                    t.hover.gamma_multiply(0.7)
                } else {
                    t.chrome
                },
            );
            let icon = m.components.first().map(|c| kind_label(&c.shape).1).unwrap_or(Icon::Mask);
            paint(ui.painter(), Rect::from_min_size(r.min + vec2(8.0, 7.0), vec2(16.0, 16.0)), icon, t.text_label);
            ui.painter().text(
                pos2(r.left() + 32.0, r.center().y),
                Align2::LEFT_CENTER,
                &m.name,
                t.font(13.0),
                if m.visible { t.text } else { t.text_disabled },
            );
            // show / hide on hover (and always while hidden)
            // (the pointer test, not `hovered`: over the eye, the row itself no longer counts as hovered)
            if ui.rect_contains_pointer(r) || !m.visible {
                let er = Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(22.0, 22.0));
                register(ui.ctx(), format!("maskVisible:{}", m.id), er);
                let eye = ui.interact(er, egui::Id::new(("maskVisible", m.id)), Sense::click());
                paint(
                    ui.painter(),
                    er.shrink(3.0),
                    if m.visible { Icon::Eye } else { Icon::EyeOff },
                    if eye.hovered() { t.text } else { t.text_dim },
                );
                if eye.clicked() {
                    let _ = app.run("mask.visible", json!({"id": m.id}));
                }
            }
            if resp.clicked() {
                let _ = app.run("mask.select", json!({"id": m.id}));
            }
            if resp.double_clicked() {
                app.ui.renaming_mask = Some((m.id, m.name.clone()));
            }
            resp.context_menu(|ui| mask_menu(app, ui, m.id, &m.name, m.visible, index, count));
        }
        if !d.masks.is_empty() {
            ui.add_space(6.0);
            overlay_options(app, ui);
        }
    });
    let Some(mid) = app.session.active_mask else { return };
    let Some(m) = d.masks.iter().find(|m| m.id == mid).cloned() else { return };
    divider(ui);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 8 }).show(ui, |ui| {
        for (i, c) in m.components.iter().enumerate() {
            let (kind, icon) = kind_label(&c.shape);
            let label = c.name.clone().unwrap_or_else(|| kind.to_string());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                paint(ui.painter(), r, icon, t.text_label);
                let op = match c.op {
                    lightcraft_develop::MaskOp::Add => "",
                    lightcraft_develop::MaskOp::Subtract => "− ",
                    lightcraft_develop::MaskOp::Intersect => "∩ ",
                };
                if let Some((_, _, name)) = app.ui.renaming_component.as_mut().filter(|(mid, k, _)| *mid == m.id && *k == i) {
                    // inline rename: Enter (or leaving the field) commits, Escape cancels
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(ui.available_width() - 30.0).id_salt(("compRename", m.id, i)));
                    register(ui.ctx(), format!("componentRename:{i}"), r.rect);
                    if !r.has_focus() && !r.lost_focus() {
                        r.request_focus();
                    }
                    if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                        app.ui.renaming_component = None;
                    } else if r.lost_focus() {
                        let name = name.trim().to_string();
                        app.ui.renaming_component = None;
                        if Some(&name) != c.name.as_ref() {
                            let _ = app.run("mask.component", json!({"id": m.id, "component": i, "action": "rename", "name": name}));
                        }
                    }
                    return;
                }
                // a long name is cut short (with …) before the options button, not past the panel
                let text = format!("{op}{label}{}", if c.invert { " (inverted)" } else { "" });
                let room = (ui.available_width() - 30.0).max(0.0);
                let resp = ui.scope(|ui| {
                    ui.set_max_width(room);
                    ui.add(egui::Label::new(text).truncate().sense(Sense::click()))
                });
                let resp = resp.inner;
                register(ui.ctx(), format!("component:{i}"), resp.rect);
                if resp.double_clicked() {
                    app.ui.renaming_component = Some((m.id, i, label.clone()));
                }
                resp.context_menu(|ui| component_row_menu(app, ui, m.id, i, &label, m.components.len()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let more = ui.small_button("…").on_hover_text(crate::i18n::tr("Component options"));
                    register(ui.ctx(), format!("button:componentMenu{i}"), more.rect);
                    egui::Popup::menu(&more).show(|ui| component_row_menu(app, ui, m.id, i, &label, m.components.len()));
                });
            });
            range_controls(app, ui, i, &c.shape);
        }
        ui.add_space(6.0);
        // wraps: the five actions are wider than a narrow panel
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            let add = text_button(ui, "maskAddComp", crate::i18n::tr("Add"), false);
            egui::Popup::menu(&add).show(|ui| component_menu(app, ui, "add"));
            let sub = text_button(ui, "maskSubComp", crate::i18n::tr("Subtract"), false);
            egui::Popup::menu(&sub).show(|ui| component_menu(app, ui, "subtract"));
            let int = text_button(ui, "maskIntComp", crate::i18n::tr("Intersect"), false);
            egui::Popup::menu(&int).show(|ui| component_menu(app, ui, "intersect"));
            if text_button(ui, "maskInvert", crate::i18n::tr("Invert"), m.invert).clicked() {
                let _ = app.run("mask.invert", json!({}));
            }
            if icon_button(ui, "maskDelete", Icon::Trash, vec2(26.0, 24.0), false, true, "Delete mask").clicked() {
                let _ = app.run("mask.delete", json!({}));
            }
        });
    });
    if app.ui.tool == "brush" {
        divider(ui);
        brush_settings(app, ui);
    }
    divider(ui);
    for s in LOCAL {
        let v = local_get(&m.adjust, s.id);
        let out = slider(ui, s, v, true, None);
        apply_slider_out(app, s, out, |app, v| app.run("mask.adjust", json!({"values": {s.id: v}})));
    }
    let amt = ControlSpec {
        id: "amount",
        label: "Amount",
        section: Section::Light,
        min: 0.0,
        max: 200.0,
        default: 100.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &amt, m.adjust.amount, true, None);
    apply_slider_out(app, &amt, out, |app, v| app.run("mask.adjust", json!({"values": {"amount": v}})));
    let refine = ControlSpec {
        id: "refine",
        label: "Refine Edges",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: 0.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &refine, m.refine, true, None);
    apply_slider_out(app, &refine, out, |app, v| app.run("mask.refine", json!({"value": v})));
    ui.add_space(30.0);
    let _ = Stroke::NONE;
}

/// The right-click menu of a mask in the Masks list.
fn mask_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: u32, name: &str, visible: bool, index: usize, count: usize) {
    let mut run = |ui: &mut egui::Ui, label: &str, enabled: bool, cmd: &str, p: serde_json::Value| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            let _ = app.run(cmd, p);
            ui.close();
        }
    };
    run(ui, "Duplicate Mask", true, "mask.duplicate", json!({"id": id}));
    run(ui, "Duplicate and Invert Mask", true, "mask.duplicate", json!({"id": id, "invert": true}));
    run(ui, "Invert Mask", true, "mask.invert", json!({"id": id}));
    run(ui, if visible { "Hide Mask" } else { "Show Mask" }, true, "mask.visible", json!({"id": id}));
    ui.separator();
    run(ui, "Move Up", index > 0, "mask.move", json!({"id": id, "delta": -1}));
    run(ui, "Move Down", index + 1 < count, "mask.move", json!({"id": id, "delta": 1}));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_mask = Some((id, name.to_string()));
        ui.close();
    }
    if ui.button(crate::i18n::tr("Delete Mask")).clicked() {
        let _ = app.run("mask.delete", json!({"id": id}));
        ui.close();
    }
}

/// Overlay colours offered as swatches (the colour of the selected one is used for the tint).
pub const OVERLAY_COLORS: [[u8; 3]; 5] = [[230, 30, 40], [40, 200, 70], [40, 110, 240], [250, 210, 30], [255, 255, 255]];

/// How the selected mask is shown: overlay mode, colour, opacity, pins.
fn overlay_options(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    use lightcraft_pipeline::MaskView;
    let t = Tokens::get(ui.ctx());
    let view = MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Overlay")).color(t.text_dim));
        let r = crate::widgets::dropdown(ui, "maskOverlayMode", view.label(), t.font(12.5), t.text_label);
        egui::Popup::menu(&r).show(|ui| {
            for v in MaskView::ALL {
                if ui.selectable_label(v == view, v.label()).clicked() {
                    let _ = app.run("view.maskOverlayMode", json!({"mode": v.name()}));
                }
            }
        });
    });
    let colored = matches!(view, MaskView::Color | MaskView::ColorOnBw);
    if colored {
        ui.horizontal(|ui| {
            for c in OVERLAY_COLORS {
                let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                register(ui.ctx(), format!("maskOverlayColor:{:02x}{:02x}{:02x}", c[0], c[1], c[2]), r);
                ui.painter().rect_filled(r.shrink(2.0), 3.0, egui::Color32::from_rgb(c[0], c[1], c[2]));
                if app.ui.mask_overlay_color == c {
                    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.5, t.text), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    let _ = app.run("view.maskOverlayColor", json!({"color": c}));
                }
            }
        });
    }
    let spec = ControlSpec {
        id: "ui.maskOverlayOpacity",
        label: "Opacity",
        section: Section::Light,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        step: 1.0,
        decimals: 0,
        track: Track::Plain,
    };
    let out = slider(ui, &spec, app.ui.mask_overlay_opacity as f64, colored && app.ui.mask_overlay, None);
    if let Some(v) = out.value {
        app.ui.mask_overlay_opacity = v as f32;
    }
    let mut pins = app.ui.mask_pins;
    if ui.checkbox(&mut pins, crate::i18n::tr("Show Pins")).changed() {
        let _ = app.run("view.maskPins", json!({"show": pins}));
    }
}

/// Controls for a range component: the selected range (two handles over a dark → light bar),
/// Smoothness, and Show Luminance Map; colour ranges get Refine.
fn range_controls(app: &mut LightcraftApp, ui: &mut egui::Ui, comp: usize, shape: &MaskShape) {
    let t = Tokens::get(ui.ctx());
    let update = |app: &mut LightcraftApp, shape: MaskShape| {
        let _ = app.run("mask.update", json!({"component": comp, "shape": shape}));
    };
    match shape {
        MaskShape::LuminanceRange { lo, hi, lo_feather, hi_feather } => {
            ui.label(egui::RichText::new(crate::i18n::tr("Select Luminance Range")).color(t.text_dim).size(11.5));
            let (lo, hi) = (*lo, *hi);
            let w = ui.available_width().min(240.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::click_and_drag());
            register(ui.ctx(), format!("lumRange:{comp}"), rect);
            let p = ui.painter();
            // dark → light ramp, the selected span outlined
            let n = 24;
            for k in 0..n {
                let a = k as f32 / n as f32;
                let r = Rect::from_min_max(
                    pos2(rect.left() + a * w, rect.top() + 4.0),
                    pos2(rect.left() + (a + 1.0 / n as f32) * w + 0.5, rect.bottom() - 4.0),
                );
                let g = (a * 255.0) as u8;
                p.rect_filled(r, 0.0, egui::Color32::from_gray(g));
            }
            let x = |v: f64| rect.left() + v.clamp(0.0, 1.0) as f32 * w;
            p.rect_stroke(
                Rect::from_min_max(pos2(x(lo), rect.top() + 2.0), pos2(x(hi), rect.bottom() - 2.0)),
                2.0,
                Stroke::new(1.5, t.accent),
                egui::StrokeKind::Middle,
            );
            for v in [lo, hi] {
                p.circle_filled(pos2(x(v), rect.center().y), 5.0, egui::Color32::WHITE);
                p.circle_stroke(pos2(x(v), rect.center().y), 5.0, Stroke::new(1.0, egui::Color32::from_gray(40)));
            }
            // drag the nearer handle; one undo step per drag
            if resp.drag_started() {
                let _ = app.run("develop.beginInteraction", json!({"label": "Luminance Range"}));
            }
            if (resp.dragged() || resp.clicked())
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let v = (((pos.x - rect.left()) / w) as f64).clamp(0.0, 1.0);
                let (nlo, nhi) = if (v - lo).abs() <= (v - hi).abs() { (v.min(hi - 0.01), hi) } else { (lo, v.max(lo + 0.01)) };
                update(app, MaskShape::LuminanceRange { lo: nlo, hi: nhi, lo_feather: *lo_feather, hi_feather: *hi_feather });
            }
            if resp.drag_stopped() {
                let _ = app.run("develop.endInteraction", json!({}));
            }
            // Smoothness: both falloffs at once
            let smooth = ControlSpec {
                id: "smoothness",
                label: "Smoothness",
                section: Section::Light,
                min: 0.0,
                max: 100.0,
                default: 20.0,
                step: 1.0,
                decimals: 0,
                track: Track::Plain,
            };
            let cur = ((lo_feather + hi_feather) / 2.0 / 0.5 * 100.0).clamp(0.0, 100.0);
            let out = slider(ui, &smooth, cur, true, None);
            apply_slider_out(app, &smooth, out, |app, v| {
                let f = v / 100.0 * 0.5;
                app.run("mask.update", json!({"component": comp, "shape": MaskShape::LuminanceRange { lo, hi, lo_feather: f, hi_feather: f }}))
            });
            let mut map = app.ui.mask_overlay && app.ui.mask_overlay_mode == "colorOnBw";
            let r = ui.checkbox(&mut map, crate::i18n::tr("Show Luminance Map"));
            register(ui.ctx(), format!("check:lumMap{comp}"), r.rect);
            if r.changed() {
                // the luminance map: the photo in black & white with the selected range tinted
                if map {
                    app.ui.luminance_map_restore = Some((app.ui.mask_overlay, app.ui.mask_overlay_mode.clone()));
                    app.ui.mask_overlay = true;
                    app.ui.mask_overlay_mode = "colorOnBw".into();
                } else {
                    let (on, mode) = app.ui.luminance_map_restore.take().unwrap_or((false, "color".into()));
                    app.ui.mask_overlay = on;
                    app.ui.mask_overlay_mode = if mode == "colorOnBw" { "color".into() } else { mode };
                }
            }
        }
        MaskShape::ColorRange { samples, refine } => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{} sample{}", samples.len(), if samples.len() == 1 { "" } else { "s" }))
                        .color(t.text_dim)
                        .size(11.5),
                );
                let picking = app.ui.tool == "colorRange";
                if text_button(ui, &format!("colorPick{comp}"), crate::i18n::tr("Pick"), picking)
                    .on_hover_text(crate::i18n::tr("Click the photo to pick a colour; ⇧-click adds more (up to 5)"))
                    .clicked()
                {
                    app.ui.tool = if picking { String::new() } else { "colorRange".into() };
                }
            });
            let spec = ControlSpec {
                id: "refine",
                label: "Refine",
                section: Section::Color,
                min: 0.0,
                max: 100.0,
                default: 50.0,
                step: 1.0,
                decimals: 0,
                track: Track::Plain,
            };
            let out = slider(ui, &spec, *refine, true, None);
            let samples = samples.clone();
            apply_slider_out(app, &spec, out, |app, v| {
                app.run("mask.update", json!({"component": comp, "shape": MaskShape::ColorRange { samples: samples.clone(), refine: v }}))
            });
        }
        _ => {}
    }
}

/// The right-click / "…" menu of one component of the selected mask.
fn component_row_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, mask: u32, k: usize, label: &str, count: usize) {
    let mut run = |ui: &mut egui::Ui, text: &str, p: serde_json::Value| {
        if ui.button(text).clicked() {
            let mut p = p;
            p["id"] = json!(mask);
            p["component"] = json!(k);
            let _ = app.run("mask.component", p);
            ui.close();
        }
    };
    run(ui, "Invert", json!({"action": "invert"}));
    run(ui, &crate::i18n::tr_format!("Duplicate \"{label}\"", label = label), json!({"action": "duplicate"}));
    if k > 0 {
        ui.menu_button(crate::i18n::tr("Mode"), |ui| {
            for (op, text) in [("add", "Add"), ("subtract", "Subtract"), ("intersect", "Intersect")] {
                run(ui, text, json!({"action": "op", "op": op}));
            }
        });
    }
    ui.menu_button(crate::i18n::tr("Intersect with"), |ui| component_menu(app, ui, "intersect"));
    ui.menu_button(crate::i18n::tr("Subtract"), |ui| component_menu(app, ui, "subtract"));
    ui.separator();
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        app.ui.renaming_component = Some((mask, k, label.to_string()));
        ui.close();
    }
    let del = if count == 1 {
        crate::i18n::tr_format!("Delete \"{label}\" (and the mask)", label = label)
    } else {
        crate::i18n::tr_format!("Delete \"{label}\"", label = label)
    };
    if ui.button(del).clicked() {
        let _ = app.run("mask.component", json!({"id": mask, "component": k, "action": "delete"}));
        ui.close();
    }
}

fn component_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, op: &str) {
    for (kind, label) in [
        ("brush", "Brush"),
        ("linear", "Linear Gradient"),
        ("radial", "Radial Gradient"),
        ("sky", "Sky"),
        ("subject", "Subject"),
        ("luminanceRange", "Luminance Range"),
    ] {
        // painting only adds or erases
        if kind == "brush" && op == "intersect" {
            continue;
        }
        if ui.button(label).clicked() {
            if kind == "brush" {
                app.ui.tool = "brush".into();
                app.ui.brush_erase = op == "subtract";
            } else {
                let _ = app.run("mask.addComponent", json!({"op": op, "kind": kind}));
            }
        }
    }
}

fn brush_settings(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 0 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Brush")).font(t.semibold(13.0)));
            if text_button(ui, "brushAdd", crate::i18n::tr("Add"), !app.ui.brush_erase).clicked() {
                app.ui.brush_erase = false;
            }
            if text_button(ui, "brushErase", crate::i18n::tr("Erase"), app.ui.brush_erase).clicked() {
                app.ui.brush_erase = true;
            }
        });
        let mut auto = app.ui.brush_auto_mask;
        if ui
            .checkbox(&mut auto, crate::i18n::tr("Auto Mask"))
            .on_hover_text(crate::i18n::tr("Paint only areas like the one under the brush"))
            .changed()
        {
            app.ui.brush_auto_mask = auto;
        }
    });
    for (id, label, min, max, get) in [
        ("ui.brushSize", "Size", 1.0, 100.0, (app.ui.brush_size * 400.0) as f64),
        ("ui.brushFeather", "Feather", 0.0, 100.0, app.ui.brush_feather as f64),
        ("ui.brushFlow", "Flow", 1.0, 100.0, app.ui.brush_flow as f64),
    ] {
        let s = ControlSpec { id, label, section: Section::Light, min, max, default: min, step: 1.0, decimals: 0, track: Track::Plain };
        let out = slider(ui, &s, get, true, None);
        if let Some(v) = out.value {
            match id {
                "ui.brushSize" => app.ui.brush_size = (v / 400.0) as f32,
                "ui.brushFeather" => app.ui.brush_feather = v as f32,
                _ => app.ui.brush_flow = v as f32,
            }
        }
    }
}
