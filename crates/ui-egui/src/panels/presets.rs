//! The Presets column (opens to the left of the Edit panel): grouped presets with hover preview
//! (the loupe shows the photo with the preset while the pointer rests on it; nothing is
//! committed), optional live thumbnails, amount slider, create/favourite, import/export of preset
//! files.

use std::collections::BTreeMap;

use egui::{Align2, Color32, Rect, Sense, pos2, vec2};
use lightcraft_develop::{ControlSpec, Preset, Section, Track};
use serde_json::json;

use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register, slider};
use crate::{HoverPreview, LightcraftApp};

/// Long edge of preset thumbnails (px).
const THUMB_EDGE: usize = 128;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::right("presets_panel")
        .exact_size(t.panel_w)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
            ui.painter().text(pos2(hr.left() + 24.0, hr.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Presets"), t.semibold(15.0), t.text);
            let mut hdr = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(hr.right() - 80.0, hr.top()), hr.right_bottom()))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            if icon_button(&mut hdr, "presetCreate", Icon::Plus, vec2(28.0, 28.0), false, app.session.active().is_some(), "Create Preset…").clicked()
            {
                app.ui.dialog = Some(crate::state::Dialog::create_preset());
            }
            let more = icon_button(&mut hdr, "presetMore", Icon::More, vec2(28.0, 28.0), false, true, "More preset options");
            egui::Popup::menu(&more).show(|ui| {
                let r = ui.checkbox(&mut app.ui.preset_thumbs, crate::i18n::tr("Show Thumbnails"));
                register(ui.ctx(), "presetMenu:thumbnails", r.rect);
                ui.separator();
                if ui.button(crate::i18n::tr("Import Presets…")).clicked() {
                    let _ = app.run("file.importPresets", json!({}));
                }
                let any_user = app.session.presets.iter().any(|p| !p.builtin);
                if ui.add_enabled(any_user, egui::Button::new(crate::i18n::tr("Export User Presets…"))).clicked() {
                    let _ = app.run("file.exportPresets", json!({}));
                }
            });
            divider(ui);
            // amount slider (applies to the last applied preset)
            let amt_id = egui::Id::new("preset-amount");
            let last: Option<(String, f64)> = ui.data(|d| d.get_temp(amt_id));
            if let Some((pid, amount)) = last.clone() {
                let spec = ControlSpec {
                    id: "presetAmount",
                    label: "Amount",
                    section: Section::Profile,
                    min: 0.0,
                    max: 200.0,
                    default: 100.0,
                    step: 1.0,
                    decimals: 0,
                    track: Track::Plain,
                };
                let out = slider(ui, &spec, amount, true, None);
                if let Some(v) = out.value {
                    // re-apply from the pre-preset state: undo last preset step then apply with the new amount
                    let _ = app.run("edit.undo", json!({}));
                    let _ = app.run("preset.apply", json!({"id": pid, "amount": v}));
                    ui.data_mut(|d| d.insert_temp(amt_id, (pid, v)));
                }
                divider(ui);
            }
            let active = app.session.active();
            let current = active.and_then(|id| app.session.develop_of(id)).map(|d| (*d).clone());
            let thumbs = app.ui.preset_thumbs && active.is_some();
            let row_h = if thumbs { 54.0 } else { 26.0 };
            let scroll = egui::ScrollArea::vertical().id_salt("presets-scroll").auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut groups: BTreeMap<String, Vec<Preset>> = BTreeMap::new();
                for p in &app.session.presets {
                    groups.entry(p.group.clone()).or_default().push(p.clone());
                }
                for (g, items) in groups {
                    let open_id = egui::Id::new(("preset-group", &g));
                    let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    register(ui.ctx(), format!("presetGroup:{g}"), r);
                    paint(
                        ui.painter(),
                        Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(12.0, 12.0)),
                        if open { Icon::ChevronDown } else { Icon::ChevronRight },
                        t.text_label,
                    );
                    ui.painter().text(pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, &g, t.semibold(13.0), t.text_label);
                    if resp.clicked() {
                        ui.data_mut(|d| d.insert_temp(open_id, !open));
                    }
                    resp.context_menu(|ui| {
                        if ui.button(crate::i18n::tr("Export Group…")).clicked() {
                            let _ = app.run("file.exportPresets", json!({"group": g}));
                        }
                    });
                    if !open {
                        continue;
                    }
                    for pr in items {
                        let (pid, name, fav) = (pr.id.clone(), pr.name.clone(), pr.favorite);
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                        register(ui.ctx(), format!("preset:{pid}"), r);
                        let is_last = last.as_ref().is_some_and(|(l, _)| *l == pid);
                        if is_last {
                            ui.painter().rect_filled(r, 0.0, t.tool_active);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(r, 0.0, t.hover.gamma_multiply(0.7));
                        }
                        let look = if thumbs || resp.hovered() { current.as_ref().map(|d| pr.apply(d, 1.0)) } else { None };
                        let mut text_x = r.left() + 40.0;
                        if thumbs && let (Some(id), Some(s)) = (active, look.as_ref()) {
                            let tr = Rect::from_min_size(pos2(r.left() + 36.0, r.top() + 5.0), vec2(66.0, row_h - 10.0));
                            ui.painter().rect_filled(tr, 2.0, t.canvas);
                            if let Some(job) = app.session.variant_job(id, s, THUMB_EDGE)
                                && let Some(tex) = app.renderer.variant(job)
                            {
                                ui.painter().image(tex.tex.id(), tr, cover_uv(tr, tex.size), Color32::WHITE);
                            }
                            text_x = tr.right() + 10.0;
                        }
                        ui.painter().text(pos2(text_x, r.center().y), Align2::LEFT_CENTER, &name, t.font(13.0), t.text_label);
                        if fav {
                            paint(
                                ui.painter(),
                                Rect::from_center_size(pos2(r.right() - 20.0, r.center().y), vec2(12.0, 12.0)),
                                Icon::StarFilled,
                                t.star,
                            );
                        }
                        // resting on a preset previews it in the loupe (no history entry)
                        if resp.hovered()
                            && let Some(s) = look
                        {
                            let label = crate::i18n::tr_format!("Preset: {name}", name = name);
                            app.hover_preview = Some(HoverPreview { label: label.clone(), settings: s });
                            if let Some(id) = active {
                                ui.data_mut(|d| d.insert_temp(egui::Id::new("last-preset-hover"), (id, pr.clone(), label)));
                            }
                        }
                        if resp.clicked() {
                            let _ = app.run("preset.apply", json!({"id": pid, "amount": 100}));
                            ui.data_mut(|d| d.insert_temp(amt_id, (pid.clone(), 100.0)));
                            ui.data_mut(|d| d.remove::<(lightcraft_catalog::PhotoId, Preset, String)>(egui::Id::new("last-preset-hover")));
                            app.toast(ui.ctx(), crate::i18n::tr_format!("Preset: {name}", name = name));
                        }
                        let (builtin, group) = (pr.builtin, pr.group.clone());
                        resp.context_menu(|ui| {
                            if ui.button(if fav { "Remove from Favorites" } else { "Add to Favorites" }).clicked() {
                                let _ = app.run("preset.favorite", json!({"id": pid}));
                            }
                            if builtin {
                                return;
                            }
                            ui.separator();
                            if ui
                                .add_enabled(app.session.active().is_some(), egui::Button::new(crate::i18n::tr("Update with Current Settings")))
                                .clicked()
                            {
                                match app.run("preset.update", json!({"id": pid})) {
                                    Ok(_) => app.toast(ui.ctx(), format!("Updated “{name}”")),
                                    Err(e) => app.toast(ui.ctx(), e),
                                }
                            }
                            if ui.button(crate::i18n::tr("Rename…")).clicked() {
                                crate::panels::dialogs::prompt(
                                    app,
                                    "Rename Preset",
                                    "Preset name",
                                    &name,
                                    "preset.rename",
                                    json!({"id": pid}),
                                    "name",
                                );
                            }
                            ui.menu_button(crate::i18n::tr("Move to Group"), |ui| {
                                let mut groups: Vec<String> = app.session.presets.iter().filter(|p| !p.builtin).map(|p| p.group.clone()).collect();
                                groups.sort();
                                groups.dedup();
                                for g in groups.iter().filter(|g| **g != group) {
                                    if ui.button(g).clicked() {
                                        let _ = app.run("preset.move", json!({"id": pid, "group": g}));
                                    }
                                }
                                if ui.button(crate::i18n::tr("New Group…")).clicked() {
                                    crate::panels::dialogs::prompt(
                                        app,
                                        "Move Preset to New Group",
                                        "Group name",
                                        "",
                                        "preset.move",
                                        json!({"id": pid}),
                                        "group",
                                    );
                                }
                            });
                            ui.separator();
                            if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
                                let _ = app.run("preset.delete", json!({"id": pid}));
                            }
                        });
                    }
                }
                ui.add_space(30.0);
            });
            let last_hover_id = egui::Id::new("last-preset-hover");
            let pointer_in_scroll = ui.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|pos| scroll.inner_rect.contains(pos));
            if pointer_in_scroll {
                if app.hover_preview.is_none()
                    && let (Some(id), Some(curr)) = (active, current.as_ref())
                    && let Some((saved_id, saved_preset, saved_label)) =
                        ui.data(|d| d.get_temp::<(lightcraft_catalog::PhotoId, Preset, String)>(last_hover_id))
                    && saved_id == id
                {
                    app.hover_preview = Some(HoverPreview { label: saved_label, settings: saved_preset.apply(curr, 1.0) });
                }
            } else {
                ui.data_mut(|d| d.remove::<(lightcraft_catalog::PhotoId, Preset, String)>(last_hover_id));
            }
        });
}

/// The part of a `size` image that covers `r` (centre crop), as texture coordinates.
pub fn cover_uv(r: Rect, size: [usize; 2]) -> Rect {
    let a = size[0] as f32 / size[1].max(1) as f32;
    let ra = r.width() / r.height().max(1.0);
    if a > ra {
        let f = ra / a;
        Rect::from_min_max(pos2((1.0 - f) / 2.0, 0.0), pos2((1.0 + f) / 2.0, 1.0))
    } else {
        let f = a / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - f) / 2.0), pos2(1.0, (1.0 + f) / 2.0))
    }
}
