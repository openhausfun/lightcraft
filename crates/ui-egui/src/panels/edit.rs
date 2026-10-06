//! The Edit panel: histogram, Auto/B&W, profile, and the Light / Color / Effects / Detail / Optics
//! sections, built from the engine's control specs.

use egui::epaint::{Mesh, Vertex};
use egui::{Align2, Color32, Pos2, Rect, RichText, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{ControlSpec, DevelopSettings, Section, Track, WbMode, controls};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::render::Slot;
use crate::theme::Tokens;
use crate::widgets::{BAND_COLORS, SliderOut, divider, flyout_row, hex, register, section_header, slider, text_button};

/// Commit a slider interaction: begin → live updates → end, so a drag is one undo step.
pub fn apply_slider_out(
    app: &mut LightcraftApp,
    spec: &ControlSpec,
    out: SliderOut,
    mut set: impl FnMut(&mut LightcraftApp, f64) -> Result<Value, String>,
) {
    if out.drag_started && !out.reset {
        let _ = app.run("develop.beginInteraction", json!({"label": spec.label}));
        app.ui.dragging_control = Some(spec.id.to_string());
    }
    if let Some(v) = out.value {
        let _ = set(app, v);
    }
    if out.drag_stopped || out.reset {
        let _ = app.run("develop.endInteraction", json!({}));
        app.ui.dragging_control = None;
    }
}

pub(crate) fn control(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: &str, enabled: bool) {
    let Some(spec) = controls::find(id) else { return };
    let v = controls::get(d, id).unwrap_or(spec.default);
    let out = slider(ui, spec, v, enabled, None);
    apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": id, "value": v})));
}

/// Relative temperature scale for rendered (non-raw) files: −100..100 ↔ Kelvin via mired shift.
const REL_TEMP: ControlSpec = ControlSpec {
    id: "wb.tempRel",
    label: "Temp",
    section: Section::Color,
    min: -100.0,
    max: 100.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: Track::Temp,
};
const REL_TINT: ControlSpec = ControlSpec {
    id: "wb.tintRel",
    label: "Tint",
    section: Section::Color,
    min: -100.0,
    max: 100.0,
    default: 0.0,
    step: 1.0,
    decimals: 0,
    track: Track::Tint,
};

fn k_to_rel(k: f64) -> f64 {
    ((1e6 / 6500.0 - 1e6 / k) / 0.8).clamp(-100.0, 100.0)
}
fn rel_to_k(r: f64) -> f64 {
    1e6 / (1e6 / 6500.0 - r * 0.8)
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = app.session.develop_of(id).unwrap_or_default();
    // a raw shown from its embedded JPEG (preview only) gets the rendered-file white balance scale
    let raw = app.session.catalog.photo(id).is_some_and(|p| p.develops_raw() && !p.relative_wb());
    let mut d = d;
    if d.wb.mode == WbMode::AsShot && app.session.catalog.photo(id).is_some_and(|p| p.relative_wb()) {
        let wb = &mut std::sync::Arc::make_mut(&mut d).wb;
        wb.temp = 6500.0;
        wb.tint = 0.0;
    }
    let preview_only = app.session.catalog.photo(id).and_then(|p| p.preview_only.clone());

    if app.ui.histogram {
        histogram(app, ui, id);
    }
    if app.ui.soft_proof {
        soft_proofing(app, ui, id);
    }
    // header: Edit + Auto / B&W / HDR
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(pos2(hr.left() + 24.0, hr.bottom() - 10.0), Align2::LEFT_CENTER, crate::i18n::tr("Edit"), t.semibold(15.0), t.text);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 14 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if text_button(ui, "auto", crate::i18n::tr("Auto"), false).clicked() {
                let _ = app.run("develop.auto", json!({}));
                app.toast(ui.ctx(), "Auto settings applied");
            }
            let bw = crate::is_bw(&d);
            if text_button(ui, "bw", crate::i18n::tr("B&W"), bw).clicked() {
                let _ = app.run("develop.treatment", json!({}));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, "reset", crate::i18n::tr("Reset"), false).on_hover_text(crate::i18n::tr("Reset all edits (Cmd+Shift+R)")).clicked()
                {
                    let _ = app.run("develop.reset", json!({}));
                }
            });
        });
    });
    if let Some(why) = &preview_only {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 12 }).show(ui, |ui| {
            crate::widgets::preview_only_notice(ui, "edit", why);
        });
    }
    let n = app.session.selection.ids.len();
    if n > 1 && matches!(app.ui.view, crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid) {
        quick_develop(app, ui, n);
    }
    if app.session.auto_sync && n > 1 {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::tr_format!("Auto Sync: edits apply to {n} photos", n = n)).color(t.accent).size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if text_button(ui, "autoSyncOff", crate::i18n::tr("Turn Off"), false).clicked() {
                        let _ = app.run("develop.autoSync", json!({"on": false}));
                    }
                });
            });
        });
    }
    divider(ui);
    // profile row
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 14, bottom: 14 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Profile")).font(t.font(13.0)).color(t.text_dim));
            let name = lightcraft_engine::presets::profile(&d.profile.id).map(|p| p.name).unwrap_or("Color");
            let r =
                crate::widgets::dropdown(ui, "profile", if name == "Color" { crate::i18n::tr("Color") } else { name }, t.font(15.0), t.text_label);
            egui::Popup::menu(&r).show(|ui| profile_menu(app, ui, &d));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::widgets::icon_button(ui, "profileBrowser", Icon::ProfileGrid, vec2(28.0, 28.0), false, true, "Browse Profiles").clicked() {
                    let _ = app.run("panel.profiles", json!({}));
                }
            });
        });
    });
    if d.profile.id != "lc.color" {
        control(app, ui, &d, "profile.amount", true);
        ui.add_space(6.0);
    }
    divider(ui);

    section(app, ui, &d, "light", "Light", |app, ui, d| {
        for c in ["light.exposure", "light.contrast", "light.highlights", "light.shadows", "light.whites", "light.blacks"] {
            control(app, ui, d, c, true);
        }
        ui.add_space(6.0);
        let open = app.ui.flyout_open("curve");
        if flyout_row(ui, "curve", crate::i18n::tr("Curve"), Icon::Curve, open).clicked() {
            app.ui.toggle_flyout("curve");
        }
        if open {
            curve_editor(app, ui, id, d);
        }
        ui.add_space(8.0);
    });
    section(app, ui, &d, "color", "Color", |app, ui, d| {
        // White balance row
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 2 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::tr("White Balance")).font(t.font(13.0)).color(t.text_dim));
                let r = crate::widgets::dropdown(ui, "wbMode", crate::i18n::tr(d.wb.mode.label()), t.font(14.0), t.text_label);
                egui::Popup::menu(&r).show(|ui| {
                    for m in WbMode::ALL {
                        if m == WbMode::Custom {
                            continue;
                        }
                        if ui.selectable_label(d.wb.mode == m, m.label()).clicked() {
                            let mode = serde_json::to_value(m).unwrap_or_default();
                            let _ = app.run("develop.wb", json!({"mode": mode}));
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let active = app.ui.tool == "wbPicker";
                    if crate::widgets::icon_button(ui, "wbPicker", Icon::Picker, vec2(28.0, 28.0), active, true, "White Balance Selector (W)")
                        .clicked()
                    {
                        app.ui.tool = if active { String::new() } else { "wbPicker".into() };
                    }
                });
            });
        });
        if raw {
            control(app, ui, d, "wb.temp", true);
            control(app, ui, d, "wb.tint", true);
        } else {
            let out = slider(ui, &REL_TEMP, k_to_rel(d.wb.temp), true, None);
            apply_slider_out(app, &REL_TEMP, out, |app, v| app.run("develop.set", json!({"control": "wb.temp", "value": rel_to_k(v)})));
            let out = slider(ui, &REL_TINT, d.wb.tint.clamp(-100.0, 100.0), true, None);
            apply_slider_out(app, &REL_TINT, out, |app, v| app.run("develop.set", json!({"control": "wb.tint", "value": v})));
        }
        control(app, ui, d, "color.vibrance", true);
        control(app, ui, d, "color.saturation", true);
        ui.add_space(6.0);
        let open = app.ui.flyout_open("mixer");
        if flyout_row(ui, "mixer", if crate::is_bw(d) { "B&W Mixer" } else { "Color Mixer" }, Icon::Radial, open).clicked() {
            app.ui.toggle_flyout("mixer");
        }
        if open {
            mixer(app, ui, d);
        }
        if !crate::is_bw(d) {
            let open = app.ui.flyout_open("pointColor");
            if flyout_row(ui, "pointColor", crate::i18n::tr("Point Color"), Icon::Picker, open).clicked() {
                app.ui.toggle_flyout("pointColor");
            }
            if open {
                point_color(app, ui, d);
            }
        }
        let open = app.ui.flyout_open("grading");
        if flyout_row(ui, "grading", crate::i18n::tr("Color Grading"), Icon::Presets, open).clicked() {
            app.ui.toggle_flyout("grading");
        }
        if open {
            grading(app, ui, d);
        }
        ui.add_space(8.0);
    });
    section(app, ui, &d, "effects", "Effects", |app, ui, d| {
        for c in ["effects.texture", "effects.clarity", "effects.dehaze"] {
            control(app, ui, d, c, true);
        }
        sub_title(ui, crate::i18n::tr("Vignette"));
        {
            use lightcraft_develop::VignetteStyle as V;
            let styles = [
                (V::HighlightPriority, "Highlight", "highlightPriority"),
                (V::ColorPriority, "Color", "colorPriority"),
                (V::PaintOverlay, "Paint", "paintOverlay"),
            ];
            let items: Vec<(&str, &str)> = styles.iter().map(|(_, l, k)| (*l, *k)).collect();
            let active = styles.iter().position(|(v, _, _)| *v == d.vignette.style);
            let mut chosen = None;
            egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 4 }).show(ui, |ui| {
                chosen = crate::widgets::segmented(ui, "vignetteStyle", &items, active, 3);
            });
            if let Some(i) = chosen {
                let _ = app.run(
                    "develop.merge",
                    serde_json::json!({"settings": {"vignette": {"style": serde_json::to_value(styles[i].0).unwrap_or_default()}}}),
                );
            }
        }
        for c in ["vignette.amount", "vignette.midpoint", "vignette.feather", "vignette.roundness", "vignette.highlights"] {
            let enabled = c == "vignette.amount" || d.vignette.amount != 0.0;
            control(app, ui, d, c, enabled);
        }
        sub_title(ui, crate::i18n::tr("Grain"));
        for c in ["grain.amount", "grain.size", "grain.roughness"] {
            control(app, ui, d, c, c == "grain.amount" || d.grain.amount != 0.0);
        }
        ui.add_space(8.0);
    });
    section(app, ui, &d, "detail", "Detail", |app, ui, d| {
        for c in ["detail.sharpenAmount", "detail.sharpenRadius", "detail.sharpenDetail", "detail.sharpenMasking"] {
            control(app, ui, d, c, c == "detail.sharpenAmount" || d.detail.sharpen_amount > 0.0);
        }
        sub_title(ui, crate::i18n::tr("Noise Reduction"));
        for c in ["detail.nrLuminance", "detail.nrDetail", "detail.nrContrast", "detail.nrColor", "detail.nrColorDetail", "detail.nrColorSmoothness"]
        {
            control(app, ui, d, c, true);
        }
        ui.add_space(8.0);
    });
    section(app, ui, &d, "optics", "Optics", |app, ui, d| {
        let has_lens = app.session.catalog.photo(id).is_some_and(|p| p.embedded_lens.is_some());
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 4 }).show(ui, |ui| {
            let mut ca = d.optics.remove_ca;
            if ui.checkbox(&mut ca, crate::i18n::tr("Remove Chromatic Aberration")).changed() {
                let _ = app.run("develop.merge", json!({"settings": {"optics": {"remove_ca": ca}}, "label": "Remove CA"}));
            }
            let mut lp = d.optics.lens_profile;
            if ui.checkbox(&mut lp, crate::i18n::tr("Enable Lens Corrections")).changed() {
                let _ = app.run("develop.merge", json!({"settings": {"optics": {"lens_profile": lp}}, "label": "Lens Corrections"}));
            }
            if lp && !has_lens {
                ui.label(
                    egui::RichText::new(crate::i18n::tr("No lens data embedded in this file (DNG lens opcodes only)."))
                        .size(11.0)
                        .color(Tokens::get(ui.ctx()).text_dim),
                );
            }
        });
        if d.optics.lens_profile {
            for c in ["optics.profileDistortion", "optics.profileVignetting"] {
                control(app, ui, d, c, has_lens);
            }
        }
        sub_title(ui, crate::i18n::tr("Manual"));
        for c in ["optics.distortion", "optics.vignetting"] {
            control(app, ui, d, c, true);
        }
        control(app, ui, d, "optics.vignettingMidpoint", d.optics.vignetting != 0.0);
        ui.add_space(6.0);
        let open = app.ui.flyout_open("defringe");
        if flyout_row(ui, "defringe", crate::i18n::tr("Defringe"), Icon::Picker, open).clicked() {
            app.ui.toggle_flyout("defringe");
        }
        if open {
            control(app, ui, d, "optics.defringePurple", true);
            let on = d.optics.defringe_purple_amount > 0.0;
            control(app, ui, d, "optics.defringePurpleHueLo", on);
            control(app, ui, d, "optics.defringePurpleHueHi", on);
            control(app, ui, d, "optics.defringeGreen", true);
            let on = d.optics.defringe_green_amount > 0.0;
            control(app, ui, d, "optics.defringeGreenHueLo", on);
            control(app, ui, d, "optics.defringeGreenHueHi", on);
            sub_title(ui, crate::i18n::tr("Lateral Chromatic Aberration"));
            control(app, ui, d, "optics.caRed", true);
            control(app, ui, d, "optics.caBlue", true);
        }
        ui.add_space(8.0);
    });
    // Lightroom Classic's Calibration panel (the cloud app hides it): last, like there.
    section(app, ui, &d, "calibration", "Calibration", |app, ui, d| {
        sub_title(ui, crate::i18n::tr("Shadows"));
        control(app, ui, d, "calibration.shadowsTint", true);
        for (title, k) in [("Red Primary", "red"), ("Green Primary", "green"), ("Blue Primary", "blue")] {
            sub_title(ui, title);
            control(app, ui, d, &format!("calibration.{k}Hue"), true);
            control(app, ui, d, &format!("calibration.{k}Sat"), true);
        }
        ui.add_space(8.0);
    });
    ui.add_space(40.0);
}

/// The profile dropdown: Favorites, Recent, one submenu per group, then favourite toggle and
/// Browse….
fn profile_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    use lightcraft_engine::presets::{PROFILES, profile, profile_groups};
    let t = Tokens::get(ui.ctx());
    ui.set_min_width(200.0);
    let cur = d.profile.id.as_str();
    // (clicked, hovered)
    let mut pick: (Option<&'static str>, Option<&'static str>) = (None, None);
    let item = |ui: &mut egui::Ui,
                key: &str,
                p: &'static lightcraft_engine::presets::ProfileInfo,
                pick: &mut (Option<&'static str>, Option<&'static str>)| {
        let r = ui.selectable_label(p.id == cur, p.name);
        register(ui.ctx(), format!("profileMenu:{key}:{}", p.id), r.rect);
        if r.clicked() {
            pick.0 = Some(p.id);
        }
        if r.hovered() {
            pick.1 = Some(p.id);
        }
    };
    let heading = |ui: &mut egui::Ui, s: &str| {
        ui.label(egui::RichText::new(s).font(t.semibold(11.5)).color(t.text_dim));
    };
    for (title, key, ids) in [("Favorites", "fav", app.session.profile_favorites.clone()), ("Recent", "recent", app.session.profile_recent.clone())] {
        let list: Vec<_> = ids.iter().filter_map(|id| profile(id)).collect();
        if list.is_empty() {
            continue;
        }
        heading(ui, title);
        for p in list {
            item(ui, key, p, &mut pick);
        }
        ui.separator();
    }
    for g in profile_groups() {
        let r = ui.menu_button(g, |ui| {
            ui.set_min_width(170.0);
            for p in PROFILES.iter().filter(|p| p.group == g) {
                item(ui, "group", p, &mut pick);
            }
        });
        register(ui.ctx(), format!("profileMenu:groupMenu:{g}"), r.response.rect);
    }
    ui.separator();
    if let Some(p) = profile(cur) {
        let fav = app.session.profile_favorites.iter().any(|f| f == p.id);
        let label = if fav {
            crate::i18n::tr_format!("Remove “{}” from Favorites", p.name)
        } else {
            crate::i18n::tr_format!("Add “{}” to Favorites", p.name)
        };
        let r = ui.button(label);
        register(ui.ctx(), "profileMenu:toggleFavorite", r.rect);
        if r.clicked() {
            let _ = app.run("profile.favorite", json!({"id": p.id}));
        }
    }
    let r = ui.button(crate::i18n::tr("Browse…"));
    register(ui.ctx(), "profileMenu:browse", r.rect);
    if r.clicked() {
        let _ = app.run("panel.profiles", json!({}));
    }
    // resting on a profile previews it in the loupe
    if let Some(p) = pick.1.and_then(profile)
        && p.id != cur
    {
        let mut s = d.clone();
        s.profile.id = p.id.to_string();
        s.profile.amount = 100.0;
        app.hover_preview = Some(crate::HoverPreview { label: crate::i18n::tr_format!("Profile: {}", p.name), settings: s });
    }
    if let Some(id) = pick.0 {
        let _ = app.run("develop.profile", json!({"id": id}));
    }
}

pub fn sub_title(ui: &mut egui::Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y + 4.0), Align2::LEFT_CENTER, crate::i18n::tr(title), t.semibold(13.0), t.text_label);
}

fn section(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    d: &DevelopSettings,
    id: &str,
    title: &str,
    body: impl FnOnce(&mut LightcraftApp, &mut egui::Ui, &DevelopSettings),
) {
    let open = app.ui.section_open(id);
    let (resp, toggled) = section_header(ui, id, title, open, Some(d.section_enabled(id)));
    if let Some(on) = toggled {
        let _ = app.run("develop.sectionEnabled", json!({"section": id, "enabled": on}));
    } else if resp.clicked() {
        app.ui.toggle_section(id);
    }
    if open {
        body(app, ui, d);
    }
    divider(ui);
}

// ------------------------------------------------------------------------------ histogram

fn histogram(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(w, 118.0), Sense::click());
    register(ui.ctx(), "histogram", r);
    let p = ui.painter();
    p.rect_filled(r, 0.0, t.button);
    let plot = Rect::from_min_max(r.min + vec2(0.0, 6.0), pos2(r.right(), r.bottom() - 26.0));
    // the loupe render of this photo, else its thumbnail (grid view before the loupe has rendered it)
    let hist = app
        .renderer
        .textures
        .get(&Slot::Main)
        .filter(|t| t.photo == id)
        .and_then(|t| t.histogram.clone())
        .or_else(|| app.renderer.textures.get(&Slot::Thumb(id)).and_then(|t| t.histogram.clone()));
    if let Some(h) = hist {
        let smooth = |v: &[u32]| -> Vec<f32> {
            (0..256)
                .map(|i| {
                    let (a, b) = (i.max(2) - 2, (i + 3).min(256));
                    v[a..b].iter().map(|x| *x as f32).sum::<f32>() / (b - a) as f32
                })
                .collect()
        };
        let chans = [(smooth(&h.r), hex("#df3939")), (smooth(&h.g), hex("#44b072")), (smooth(&h.b), hex("#3b6fe0"))];
        let peak = chans.iter().flat_map(|(v, _)| v.iter().skip(2).take(252)).fold(1.0f32, |a, b| a.max(*b));
        let to = |i: usize, v: f32| pos2(plot.left() + plot.width() * i as f32 / 255.0, plot.bottom() - plot.height() * (v / peak).sqrt().min(1.0));
        // luminance fill
        let lum = smooth(&h.luma);
        let mut mesh = Mesh::default();
        for i in 0..256 {
            let top = to(i, lum[i]);
            mesh.vertices.push(Vertex { pos: top, uv: Pos2::ZERO, color: Color32::from_gray(120).gamma_multiply(0.35) });
            mesh.vertices.push(Vertex { pos: pos2(top.x, plot.bottom()), uv: Pos2::ZERO, color: Color32::from_gray(120).gamma_multiply(0.35) });
            if i > 0 {
                let k = (i as u32) * 2;
                mesh.indices.extend([k - 2, k - 1, k, k - 1, k + 1, k]);
            }
        }
        p.add(mesh);
        // greyscale images: the three channels coincide — draw one neutral line
        let spread: f32 = (0..256).map(|i| (chans[0].0[i] - chans[1].0[i]).abs() + (chans[1].0[i] - chans[2].0[i]).abs()).sum();
        let total: f32 = chans[1].0.iter().sum::<f32>().max(1.0);
        if spread / total < 0.02 {
            let pts: Vec<Pos2> = (0..256).map(|i| to(i, chans[1].0[i])).collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.5, Color32::from_gray(200))));
        } else {
            for (v, c) in &chans {
                let pts: Vec<Pos2> = (0..256).map(|i| to(i, v[i])).collect();
                p.add(egui::Shape::line(pts, Stroke::new(1.5, *c)));
            }
        }
        let (lo, hi) = h.clipping();
        for (on, x, active) in [(lo > 0.003, plot.left() + 12.0, app.ui.show_clipping), (hi > 0.003, plot.right() - 12.0, app.ui.show_clipping)] {
            let tri = Rect::from_center_size(pos2(x, plot.top() + 10.0), vec2(14.0, 12.0));
            p.rect_filled(tri, 2.0, if active { t.hover } else { Color32::from_gray(60) });
            paint(p, tri.shrink(2.0), Icon::ChevronDown, if on { Color32::WHITE } else { Color32::from_gray(130) });
        }
    }
    if resp.clicked() {
        app.ui.show_clipping = !app.ui.show_clipping;
    }
    // EXIF strip
    if let Some(ph) = app.session.catalog.photo(id) {
        let m = &ph.meta;
        let items = [
            m.iso.map(|i| format!("ISO {i}")).unwrap_or_default(),
            m.focal_mm.map(|f| format!("{f:.0}mm")).unwrap_or_default(),
            m.aperture.map(|a| format!("f/{a:.1}")).unwrap_or_default(),
            m.shutter.clone(),
        ];
        let y = r.bottom() - 13.0;
        for (i, s) in items.iter().enumerate() {
            let x = r.left() + 18.0 + (r.width() - 36.0) * i as f32 / 3.0;
            let align = match i {
                0 => Align2::LEFT_CENTER,
                3 => Align2::RIGHT_CENTER,
                _ => Align2::CENTER_CENTER,
            };
            p.text(pos2(x, y), align, s, t.font(12.5), t.text_label);
        }
    }
}

// ------------------------------------------------------------------------------ soft proofing

/// The Soft Proofing strip under the histogram: proof profile, gamut warnings, Create Proof Copy.
fn soft_proofing(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    use lightcraft_engine::pipeline::OutputSpace;
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Soft Proofing")).color(t.text).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut dest = app.ui.proof.dest_warning;
                let r = ui
                    .toggle_value(&mut dest, RichText::new("■").color(Color32::from_rgb(255, 40, 40)))
                    .on_hover_text(crate::i18n::tr("Show destination gamut warning"));
                register(ui.ctx(), "button:proofDestWarning", r.rect);
                let mut disp = app.ui.proof.display_warning;
                let r2 = ui
                    .toggle_value(&mut disp, RichText::new("■").color(Color32::from_rgb(40, 90, 255)))
                    .on_hover_text(crate::i18n::tr("Show display gamut warning"));
                register(ui.ctx(), "button:proofDisplayWarning", r2.rect);
                if dest != app.ui.proof.dest_warning || disp != app.ui.proof.display_warning {
                    let _ = app.run("view.softProof", json!({"destWarning": dest, "displayWarning": disp}));
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Profile:")).color(t.text_label));
            let cur = app.ui.proof.space;
            egui::ComboBox::from_id_salt("proof-space").width(170.0).selected_text(cur.label()).show_ui(ui, |ui| {
                for s in OutputSpace::ALL {
                    if ui.selectable_label(cur == s, s.label()).clicked() {
                        let _ = app.run("view.softProof", json!({"space": s}));
                    }
                }
            });
        });
        let r = ui.button(crate::i18n::tr("Create Proof Copy"));
        register(ui.ctx(), "button:createProofCopy", r.rect);
        if r.clicked() {
            let name = crate::i18n::tr_format!("Proof Copy ({})", app.ui.proof.space.label());
            if app.run("photo.virtualCopy", json!({"ids": [id.0], "name": name})).is_ok() {
                app.toast(ui.ctx(), "Proof copy created");
            }
        }
    });
    divider(ui);
}

// ------------------------------------------------------------------------------ tone curve

fn curve_editor(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    // channel selector
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            for (ch, c) in [
                ("parametric", Color32::from_gray(200)),
                ("master", Color32::WHITE),
                ("red", hex("#dd3333")),
                ("green", hex("#33bb55")),
                ("blue", hex("#3377ee")),
            ] {
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                register(ui.ctx(), format!("curveChannel:{ch}"), r);
                let sel = app.ui.curve_channel == ch;
                if ch == "parametric" {
                    paint(ui.painter(), r.shrink(4.0), Icon::Curve, if sel { t.text } else { t.icon });
                } else {
                    ui.painter().circle_stroke(r.center(), 7.0, Stroke::new(2.0, c));
                    if sel {
                        ui.painter().circle_filled(r.center(), 3.5, c);
                    }
                }
                if sel {
                    ui.painter().line_segment([r.left_bottom() + vec2(4.0, 1.0), r.right_bottom() + vec2(-4.0, 1.0)], Stroke::new(2.0, t.text));
                }
                let name = match ch {
                    "parametric" => "Parametric curve",
                    "master" => "Point curve",
                    "red" => "Red channel",
                    "green" => "Green channel",
                    _ => "Blue channel",
                };
                let resp = resp.on_hover_text(crate::i18n::tr_format!("{name} — double-click to reset it", name = name));
                if resp.double_clicked() {
                    app.ui.curve_channel = ch.into();
                    let _ = app.run("curve.reset", json!({"channel": ch}));
                } else if resp.clicked() {
                    app.ui.curve_channel = ch.into();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                tat_button(app, ui, "tat:curve", "Targeted adjustment: drag up/down on the photo to adjust the curve there");
            });
        });
    });
    let ch = app.ui.curve_channel.clone();
    let w = ui.available_width();
    let side = (w - 48.0).max(80.0);
    let (outer, _) = ui.allocate_exact_size(vec2(w, side + 12.0), Sense::hover());
    let r = Rect::from_min_size(pos2(outer.left() + 24.0, outer.top() + 6.0), vec2(side, side));
    let resp = ui.interact(r, egui::Id::new(("curve", &ch)), Sense::click_and_drag());
    register(ui.ctx(), "curve", r);
    resp.context_menu(|ui| curve_reset_menu(app, ui, &ch));
    let p = ui.painter();
    p.rect_filled(r, 2.0, t.canvas);
    for i in 1..4 {
        let f = i as f32 / 4.0;
        p.line_segment(
            [pos2(r.left() + r.width() * f, r.top()), pos2(r.left() + r.width() * f, r.bottom())],
            Stroke::new(1.0, Color32::from_gray(52)),
        );
        p.line_segment(
            [pos2(r.left(), r.top() + r.height() * f), pos2(r.right(), r.top() + r.height() * f)],
            Stroke::new(1.0, Color32::from_gray(52)),
        );
    }
    p.line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.0, Color32::from_gray(70)));
    let to_screen = |x: f64, y: f64| pos2(r.left() + x as f32 * r.width(), r.bottom() - y as f32 * r.height());
    let from_screen = |q: Pos2| (((q.x - r.left()) / r.width()).clamp(0.0, 1.0) as f64, ((r.bottom() - q.y) / r.height()).clamp(0.0, 1.0) as f64);
    if ch == "parametric" {
        // draw the effective parametric curve by sampling the pipeline's region curve approximation
        let c = &d.curve;
        let regions = [
            (0.0, c.split_shadows / 100.0, c.shadows),
            (c.split_shadows / 100.0, c.split_mid / 100.0, c.darks),
            (c.split_mid / 100.0, c.split_highlights / 100.0, c.lights),
            (c.split_highlights / 100.0, 1.0, c.highlights),
        ];
        let pts: Vec<Pos2> = (0..=64)
            .map(|i| {
                let x = i as f64 / 64.0;
                let mut dd = 0.0;
                for (a, b, amt) in regions {
                    let (ctr, half) = ((a + b) / 2.0, (b - a) * 0.75 + 0.05);
                    let tt = ((x - ctr) / half).clamp(-1.0, 1.0);
                    dd += amt / 100.0 * 0.22 * (0.5 + 0.5 * (tt * std::f64::consts::PI).cos());
                }
                to_screen(x, (x + dd * 4.0 * x * (1.0 - x)).clamp(0.0, 1.0))
            })
            .collect();
        p.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::from_gray(220))));
        curve_footer(app, ui, d);
        for c in ["curve.highlights", "curve.lights", "curve.darks", "curve.shadows"] {
            control(app, ui, d, c, true);
        }
        control(app, ui, d, "curve.refineSaturation", true);
        return;
    }
    let pts_of = |d: &DevelopSettings| -> Vec<Point> {
        let v = match ch.as_str() {
            "red" => &d.curve.red,
            "green" => &d.curve.green,
            "blue" => &d.curve.blue,
            _ => &d.curve.master,
        };
        if v.is_empty() { vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)] } else { v.clone() }
    };
    let pts = pts_of(d);
    let curve = lightcraft_color::spline::MonotoneCurve::new(&pts.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>());
    let color = match ch.as_str() {
        "red" => hex("#dd3333"),
        "green" => hex("#33bb55"),
        "blue" => hex("#3377ee"),
        _ => Color32::from_gray(225),
    };
    let line: Vec<Pos2> = (0..=96).map(|i| to_screen(i as f64 / 96.0, curve.eval(i as f64 / 96.0).clamp(0.0, 1.0))).collect();
    p.add(egui::Shape::line(line, Stroke::new(2.0, color)));
    // interaction: drag a point (both axes, between its neighbours), drag empty space to add and
    // drag a new point, click empty space to add, double-click a point to delete it
    let drag_id = egui::Id::new(("curve-drag", &ch));
    // the point being dragged (stored as `Option<usize>` so a stale value can be cleared)
    let mut dragging: Option<usize> = ui.data(|dd| dd.get_temp::<Option<usize>>(drag_id)).flatten();
    let nearest = |q: Pos2| pts.iter().enumerate().map(|(i, p)| (i, to_screen(p.x, p.y).distance(q))).min_by(|a, b| a.1.total_cmp(&b.1));
    let hovered = resp.hover_pos().and_then(nearest).filter(|(_, dist)| *dist < 10.0).map(|(i, _)| i);
    if dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    for (i, q) in pts.iter().enumerate() {
        let c = to_screen(q.x, q.y);
        if dragging == Some(i) || (dragging.is_none() && hovered == Some(i)) {
            p.circle_filled(c, 5.5, color);
            p.circle_stroke(c, 5.5, Stroke::new(1.0, t.canvas));
        } else {
            p.circle_filled(c, 4.0, color);
        }
    }
    // input / output readout of the point being dragged (0–255 like the histogram)
    if let Some(q) = dragging.and_then(|i| pts.get(i)) {
        p.text(
            r.left_top() + vec2(6.0, 4.0),
            egui::Align2::LEFT_TOP,
            format!("{} / {}", (q.x * 255.0).round(), (q.y * 255.0).round()),
            t.font(11.0),
            t.text_dim,
        );
    }
    let mut new_pts = None;
    if resp.double_clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && let Some((i, dist)) = nearest(q)
        && dist < 10.0
        && i != 0
        && i != pts.len() - 1
    {
        let mut v = pts.clone();
        v.remove(i);
        new_pts = Some(v);
    } else if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        // pick the point under the press, not where the pointer is once the drag threshold passed
        let _ = app.run("develop.beginInteraction", json!({"label": "Tone Curve"}));
        match nearest(q) {
            Some((i, dist)) if dist < 10.0 => dragging = Some(i),
            _ => {
                let (x, y) = from_screen(q);
                let mut v = pts.clone();
                let i = v.iter().position(|p| p.x > x).unwrap_or(v.len());
                v.insert(i, Point::new(x, y));
                dragging = Some(i);
                new_pts = Some(v);
            }
        }
    } else if resp.dragged()
        && let (Some(i), Some(q)) = (dragging, resp.interact_pointer_pos())
    {
        let (mut x, y) = from_screen(q);
        let mut v = pts.clone();
        if i < v.len() {
            let lo = if i == 0 { 0.0 } else { v[i - 1].x + 0.01 };
            let hi = if i + 1 >= v.len() { 1.0 } else { v[i + 1].x - 0.01 };
            if i == 0 {
                x = x.min(hi);
            } else if i + 1 == v.len() {
                x = x.max(lo);
            } else {
                x = x.clamp(lo, hi);
            }
            v[i] = Point::new(x, y);
            new_pts = Some(v);
        }
    } else if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && nearest(q).is_none_or(|(_, dist)| dist >= 10.0)
    {
        let (x, y) = from_screen(q);
        let mut v = pts.clone();
        let i = v.iter().position(|p| p.x > x).unwrap_or(v.len());
        v.insert(i, Point::new(x, y));
        new_pts = Some(v);
    }
    if let Some(v) = new_pts {
        let arr: Vec<[f64; 2]> = v.iter().map(|p| [p.x, p.y]).collect();
        let _ = app.run("develop.curve", json!({"channel": ch, "points": arr}));
    }
    if resp.drag_stopped() {
        dragging = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
    ui.data_mut(|dd| dd.insert_temp(drag_id, dragging));
    curve_footer(app, ui, d);
    control(app, ui, d, "curve.refineSaturation", true);
    let _ = id;
}

/// The row under the curve graph: point-curve presets and reset every curve.
fn curve_footer(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    use lightcraft_engine::cmd::curves::{all_presets, matching_preset};
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Point Curve")).font(t.font(12.0)).color(t.text_label));
            let current = matching_preset(&app.session, &d.curve);
            let r = crate::widgets::dropdown(ui, "curvePreset", current.as_deref().unwrap_or("Custom"), t.font(12.0), t.text);
            egui::Popup::menu(&r).show(|ui| {
                ui.set_min_width(180.0);
                let presets = all_presets(&app.session);
                let mut user_seen = false;
                for p in &presets {
                    if !p.builtin && !user_seen {
                        user_seen = true;
                        ui.separator();
                    }
                    let r = ui.selectable_label(current.as_deref() == Some(p.name.as_str()), &p.name);
                    register(ui.ctx(), format!("curvePreset:{}", p.name), r.rect);
                    if r.clicked() {
                        let _ = app.run("curve.applyPreset", json!({"name": p.name}));
                        ui.close();
                    }
                    if !p.builtin {
                        r.context_menu(|ui| {
                            if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
                                let _ = app.run("curve.deletePreset", json!({"name": p.name}));
                                ui.close();
                            }
                        });
                    }
                }
                ui.separator();
                let shaped = current.as_deref() != Some("Linear");
                let r = ui.add_enabled(shaped, egui::Button::new(crate::i18n::tr("Save Point Curve…")));
                register(ui.ctx(), "curvePresetMenu:save", r.rect);
                if r.clicked() {
                    crate::panels::dialogs::prompt(app, "Save Point Curve Preset", "Preset name", "", "curve.savePreset", json!({}), "name");
                    ui.close();
                }
                if ui.add_enabled(app.services.pick_curve_preset_files.is_some(), egui::Button::new(crate::i18n::tr("Import Presets…"))).clicked() {
                    let _ = app.run("file.importCurvePresets", json!({}));
                    ui.close();
                }
                let can_export = app.services.save_curve_preset_file.is_some() && !app.session.curve_presets.is_empty();
                if ui.add_enabled(can_export, egui::Button::new(crate::i18n::tr("Export Presets…"))).clicked() {
                    let _ = app.run("file.exportCurvePresets", json!({}));
                    ui.close();
                }
                if presets.iter().any(|p| !p.builtin) {
                    ui.label(RichText::new(crate::i18n::tr("Right-click a preset to delete it")).font(t.font(11.0)).color(t.text_dim));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let r = text_button(ui, "curveReset", crate::i18n::tr("Reset"), false).on_hover_text(crate::i18n::tr(
                    "Reset all curves: point curves (all channels) and parametric (double-click a channel to reset only that one)",
                ));
                if r.clicked() {
                    let _ = app.run("curve.reset", json!({"channel": "all"}));
                }
            });
        });
    });
}

/// Right-click menu of the curve graph.
fn curve_reset_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, ch: &str) {
    let label =
        if ch == "parametric" { "Reset Parametric Curve".to_string() } else { crate::i18n::tr_format!("Reset {} Channel", channel_label(ch)) };
    let r = ui.button(label);
    register(ui.ctx(), "curveMenu:resetChannel", r.rect);
    if r.clicked() {
        let _ = app.run("curve.reset", json!({"channel": ch}));
        ui.close();
    }
    let r = ui.button(crate::i18n::tr("Reset All Curves"));
    register(ui.ctx(), "curveMenu:resetAll", r.rect);
    if r.clicked() {
        let _ = app.run("curve.reset", json!({"channel": "all"}));
        ui.close();
    }
}

fn channel_label(ch: &str) -> &'static str {
    match ch {
        "red" => "Red",
        "green" => "Green",
        "blue" => "Blue",
        _ => "RGB",
    }
}

/// Toggle for a targeted-adjustment tool (`tool` = `tat:<target>`).
fn tat_button(app: &mut LightcraftApp, ui: &mut egui::Ui, tool: &str, tip: &str) {
    let active = app.ui.tool == tool;
    let id = tool.replace(':', "-");
    if crate::widgets::icon_button(ui, &id, Icon::Target, vec2(26.0, 26.0), active, true, tip).clicked() {
        app.ui.tool = if active { String::new() } else { tool.to_string() };
    }
}

// ------------------------------------------------------------------------------ colour mixer

fn mixer(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let bands = lightcraft_develop::MIXER_BANDS;
    let sel = bands.iter().position(|b| *b == app.ui.mixer_mode).unwrap_or(0);
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            for (i, b) in bands.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                register(ui.ctx(), format!("mixerBand:{b}"), r);
                ui.painter().circle_filled(r.center(), 9.0, hex(BAND_COLORS[i]));
                if i == sel {
                    ui.painter().circle_stroke(r.center(), 10.5, Stroke::new(1.5, t.text));
                    ui.painter().circle_filled(r.center(), 2.0, Color32::WHITE);
                }
                if resp.clicked() {
                    app.ui.mixer_mode = b.to_string();
                }
            }
        });
    });
    // targeted adjustment: pick the attribute, then drag on the photo
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 2 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            paint(ui.painter(), r, Icon::Target, t.icon);
            let items: &[(&str, &str)] =
                if crate::is_bw(d) { &[("lum", "B&W Mix")] } else { &[("hue", "Hue"), ("sat", "Saturation"), ("lum", "Luminance")] };
            for (k, label) in items {
                let tool = format!("tat:{k}");
                let active = app.ui.tool == tool;
                if text_button(ui, &format!("tatMixer-{k}"), label, active)
                    .on_hover_text(crate::i18n::tr("Targeted adjustment: drag up/down on the photo to adjust the colours there"))
                    .clicked()
                {
                    app.ui.tool = if active { String::new() } else { tool };
                }
            }
        });
    });
    let b = bands[sel];
    if crate::is_bw(d) {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            if crate::widgets::text_button(ui, "bwAuto", crate::i18n::tr("Auto"), false)
                .on_hover_text(crate::i18n::tr("Set the mix from the photo's colours"))
                .clicked()
            {
                let _ = app.run("develop.autoBwMix", json!({}));
            }
        });
        control(app, ui, d, &format!("bw.{b}"), true);
    } else {
        for k in ["hue", "sat", "lum"] {
            control(app, ui, d, &format!("mixer.{b}.{k}"), true);
        }
    }
}

// ------------------------------------------------------------------------------ point color

/// Display colour of an OkLCh sample (lightness, chroma, hue in degrees).
fn oklch_color(l: f64, c: f64, h_deg: f64) -> Color32 {
    use lightcraft_color::perceptual::{lch_to_lab, oklab_to_2020};
    let lin = oklab_to_2020(lch_to_lab([l as f32, c as f32, (h_deg as f32).to_radians()]));
    let s = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB).apply_f32(lin);
    let e = s.map(|v| (lightcraft_color::transfer::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
    Color32::from_rgb(e[0], e[1], e[2])
}

/// Point Color: swatches of the samples (+ the eyedropper), the selected sample's shifts and range,
/// and "Visualize range".
fn point_color(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let n = d.point_colors.len();
    if app.ui.point_color >= n && n > 0 {
        app.ui.point_color = n - 1;
    }
    let sel = app.ui.point_color;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 8, bottom: 4 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let active = app.ui.tool == "pointColor";
            let full = n >= lightcraft_develop::MAX_POINT_COLORS;
            if crate::widgets::icon_button(ui, "pointColorPicker", Icon::Picker, vec2(26.0, 26.0), active, !full, "Sample a colour on the photo")
                .clicked()
            {
                app.ui.tool = if active { String::new() } else { "pointColor".into() };
            }
            for (i, p) in d.point_colors.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                register(ui.ctx(), format!("pointColor:{i}"), r);
                ui.painter().rect_filled(r.shrink(2.0), 3.0, oklch_color(p.lum, p.chroma, p.hue));
                if i == sel {
                    ui.painter().rect_stroke(r, 4.0, Stroke::new(1.5, t.text), egui::StrokeKind::Inside);
                }
                if resp.clicked() {
                    app.ui.point_color = i;
                }
            }
        });
    });
    if n == 0 {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 2, bottom: 8 }).show(ui, |ui| {
            ui.label(egui::RichText::new(crate::i18n::tr("Pick a colour on the photo with the eyedropper.")).size(12.0).color(t.text_dim));
        });
        return;
    }
    for k in ["hueShift", "satShift", "lumShift", "variance"] {
        control(app, ui, d, &format!("pointColor.{sel}.{k}"), true);
    }
    sub_title(ui, crate::i18n::tr("Range"));
    for k in ["range", "hueRange", "satRange", "lumRange"] {
        control(app, ui, d, &format!("pointColor.{sel}.{k}"), true);
    }
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 4, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let mut v = app.ui.point_color_visualize;
            if ui.checkbox(&mut v, crate::i18n::tr("Visualize range")).changed() {
                app.ui.point_color_visualize = v;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if text_button(ui, "pointColorDelete", crate::i18n::tr("Delete"), false).clicked() {
                    let _ = app.run("pointColor.delete", json!({"index": sel}));
                }
            });
        });
    });
}

// ------------------------------------------------------------------------------ colour grading

fn grading(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings) {
    let t = Tokens::get(ui.ctx());
    let w = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(w, 250.0), Sense::hover());
    let big = 44.0;
    let small = 38.0;
    let wheels = [
        ("midtones", "Midtones", pos2(area.center().x, area.top() + 60.0), big),
        ("shadows", "Shadows", pos2(area.left() + w * 0.28, area.top() + 170.0), small),
        ("highlights", "Highlights", pos2(area.left() + w * 0.72, area.top() + 170.0), small),
    ];
    for (key, label, c, rad) in wheels {
        let wh = match key {
            "shadows" => d.grading.shadows,
            "midtones" => d.grading.midtones,
            _ => d.grading.highlights,
        };
        let r = Rect::from_center_size(c, vec2(rad * 2.0, rad * 2.0));
        let resp = ui.interact(r, egui::Id::new(("wheel", key)), Sense::click_and_drag());
        register(ui.ctx(), format!("wheel:{key}"), r);
        paint_wheel(ui.painter(), c, rad);
        let a = (wh.hue as f32).to_radians();
        let dot = c + vec2(a.cos(), -a.sin()) * rad * (wh.sat as f32 / 100.0);
        ui.painter().circle_filled(dot, 5.0, Color32::WHITE);
        ui.painter().circle_stroke(dot, 5.0, Stroke::new(1.0, Color32::BLACK));
        ui.painter().text(pos2(c.x, c.y + rad + 12.0), Align2::CENTER_CENTER, crate::i18n::tr(label), t.font(12.0), t.text_label);
        if resp.drag_started() {
            let _ = app.run("develop.beginInteraction", json!({"label": crate::i18n::tr_format!("{label} Grading", label = label)}));
        }
        if (resp.dragged() || resp.clicked())
            && let Some(q) = resp.interact_pointer_pos()
        {
            let v = q - c;
            let hue = ((-v.y).atan2(v.x).to_degrees() + 360.0) % 360.0;
            let sat = (v.length() / rad * 100.0).min(100.0);
            let _ = app.run("develop.set", json!({"values": {format!("grading.{key}.hue"): hue.round(), format!("grading.{key}.sat"): sat.round()}}));
        }
        if resp.drag_stopped() || resp.clicked() {
            let _ = app.run("develop.endInteraction", json!({}));
        }
        if resp.double_clicked() {
            let _ = app.run("develop.set", json!({"values": {format!("grading.{key}.hue"): 0, format!("grading.{key}.sat"): 0}}));
        }
    }
    for c in ["grading.shadows.lum", "grading.midtones.lum", "grading.highlights.lum", "grading.blending", "grading.balance"] {
        control(app, ui, d, c, true);
    }
}

fn paint_wheel(p: &egui::Painter, c: Pos2, rad: f32) {
    let mut mesh = Mesh::default();
    let n = 48;
    mesh.vertices.push(Vertex { pos: c, uv: Pos2::ZERO, color: Color32::from_gray(128) });
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let rgb = lightcraft_color::perceptual::hsv_to_rgb(a.to_degrees(), 0.75, 0.8);
        let col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
        mesh.vertices.push(Vertex { pos: c + vec2(a.cos(), -a.sin()) * rad, uv: Pos2::ZERO, color: col });
        if i > 0 {
            mesh.indices.extend([0, i as u32, i as u32 + 1]);
        }
    }
    p.add(mesh);
    p.circle_stroke(c, rad, Stroke::new(1.0, Color32::from_gray(40)));
}

/// Quick Develop (grid with several photos selected): relative steps applied to every selected
/// photo from its own value.
fn quick_develop(app: &mut LightcraftApp, ui: &mut egui::Ui, n: usize) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 0, bottom: 10 }).show(ui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::tr_format!("Quick Develop · {n} photos", n = n)).color(t.text_label).size(12.5));
        ui.add_space(4.0);
        let rows: [(&str, &str, f64, f64); 9] = [
            ("Exposure", "light.exposure", 1.0 / 3.0, 1.0),
            ("Contrast", "light.contrast", 5.0, 20.0),
            ("Highlights", "light.highlights", 5.0, 20.0),
            ("Shadows", "light.shadows", 5.0, 20.0),
            ("Whites", "light.whites", 5.0, 20.0),
            ("Blacks", "light.blacks", 5.0, 20.0),
            ("Clarity", "effects.clarity", 5.0, 20.0),
            ("Vibrance", "color.vibrance", 5.0, 20.0),
            ("Temp", "wb.temp", 100.0, 500.0),
        ];
        egui::Grid::new("quick-develop").num_columns(5).spacing([4.0, 3.0]).show(ui, |ui| {
            for (label, ctl, small, big) in rows {
                ui.label(egui::RichText::new(crate::i18n::tr(label)).size(11.5).color(t.text_dim));
                for (txt, d) in [("◀◀", -big), ("◀", -small), ("▶", small), ("▶▶", big)] {
                    let r = ui.add(egui::Button::new(egui::RichText::new(txt).size(10.0)).min_size(egui::vec2(28.0, 18.0)));
                    crate::widgets::register(ui.ctx(), format!("button:quick-{ctl}-{txt}"), r.rect);
                    if r.on_hover_text(crate::i18n::tr_format!("{label} {d:+} on every selected photo", d = d, label = label)).clicked() {
                        let _ = app.run("develop.quickAdjust", json!({"control": ctl, "delta": d}));
                    }
                }
                ui.end_row();
            }
        });
    });
}
