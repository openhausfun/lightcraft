//! The library filter bar above the grid: rating (≥ / = / ≤), flags, colour labels, kind,
//! edited state and camera / lens / keyword pickers — every control drives `library.filter`.
//! "Save as Smart Album…" keeps the current view as a live album; "Clear" resets the filter.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, vec2};
use lightcraft_catalog::{ColorLabel, Flag, MediaKind, RatingOp};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::register;

pub const HEIGHT: f32 = 40.0;
/// Bar width (points) below which the controls take two rows.
const WIDE: f32 = 1240.0;

/// Colour-label swatches (UI colours, our own choice).
pub fn label_color(l: ColorLabel) -> Color32 {
    match l {
        ColorLabel::Red => Color32::from_rgb(222, 72, 72),
        ColorLabel::Yellow => Color32::from_rgb(232, 196, 58),
        ColorLabel::Green => Color32::from_rgb(88, 176, 92),
        ColorLabel::Blue => Color32::from_rgb(72, 130, 222),
        ColorLabel::Purple => Color32::from_rgb(158, 100, 210),
    }
}

fn filter(app: &mut LightcraftApp, patch: Value) {
    let _ = app.run("library.filter", patch);
}

/// A small label before a group of controls.
fn caption(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::tr(text)).font(t.font(12.0)).color(t.text_dim));
}

/// A square toggle with an icon or a colour swatch.
fn toggle(ui: &mut egui::Ui, id: &str, on: bool, tip: &str, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    register(ui.ctx(), format!("filter:{id}"), r);
    let p = ui.painter();
    if on {
        p.rect_filled(r, 5.0, t.tool_active);
        p.rect_stroke(r, 5.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(r, 5.0, t.hover);
    }
    draw(
        p,
        r.shrink(5.0),
        if on {
            t.text
        } else if resp.hovered() {
            t.text_label
        } else {
            t.icon
        },
    );
    resp.on_hover_text(crate::i18n::tr(tip))
}

/// A dropdown showing `current`; `items` are (label, filter patch, selected).
fn picker(app: &mut LightcraftApp, ui: &mut egui::Ui, id: &str, current: &str, active: bool, items: Vec<(String, Value, bool)>) {
    let t = Tokens::get(ui.ctx());
    let r = crate::widgets::dropdown(ui, &format!("filter-{id}"), crate::i18n::tr(current), t.font(12.5), if active { t.text } else { t.text_label });
    if active {
        ui.painter().rect_stroke(r.rect.expand(2.0), 4.0, Stroke::new(1.0, t.accent.gamma_multiply(0.7)), StrokeKind::Outside);
    }
    let mut chosen = None;
    egui::Popup::menu(&r).show(|ui| {
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (label, patch, sel) in &items {
                if ui.selectable_label(*sel, crate::i18n::tr(label)).clicked() {
                    chosen = Some(patch.clone());
                }
            }
        });
    });
    if let Some(p) = chosen {
        filter(app, p);
    }
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // one row when there is room, else the metadata pickers and actions go to a second row
    let two_rows = ui.available_width() < WIDE;
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), if two_rows { 2.0 * HEIGHT - 8.0 } else { HEIGHT }), Sense::hover());
    ui.painter().rect_filled(bar, 0.0, t.chrome);
    ui.painter().line_segment([bar.left_bottom(), bar.right_bottom()], Stroke::new(1.0, t.divider));
    register(ui.ctx(), "filterbar", bar);
    let f = app.session.filter.clone();
    let row = |ui: &mut egui::Ui, i: f32| {
        let r = Rect::from_min_size(bar.min + vec2(16.0, 4.0 + i * (HEIGHT - 8.0)), vec2(bar.width() - 32.0, HEIGHT - 8.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        c.spacing_mut().item_spacing.x = 4.0;
        c
    };
    let mut row1 = row(ui, 0.0);
    let mut row2 = row(ui, 1.0);
    let ui = &mut row1;

    // rating: operator + stars
    caption(ui, "Rating");
    let op_label = match f.rating_op {
        RatingOp::AtLeast => "≥",
        RatingOp::Exactly => "=",
        RatingOp::AtMost => "≤",
    };
    picker(
        app,
        ui,
        "ratingOp",
        op_label,
        false,
        [("≥  at least", "atLeast", RatingOp::AtLeast), ("=  exactly", "exactly", RatingOp::Exactly), ("≤  at most", "atMost", RatingOp::AtMost)]
            .into_iter()
            .map(|(l, k, op)| (l.to_string(), json!({"ratingOp": k}), f.rating_op == op))
            .collect(),
    );
    for i in 1..=5u8 {
        let (r, resp) = ui.allocate_exact_size(vec2(17.0, 26.0), Sense::click());
        register(ui.ctx(), format!("filter:star{i}"), r);
        let filled = i <= f.rating;
        let c = if filled {
            t.star
        } else if resp.hovered() {
            t.text_label
        } else {
            t.icon
        };
        paint(ui.painter(), Rect::from_center_size(r.center(), vec2(14.0, 14.0)), if filled { Icon::StarFilled } else { Icon::Star }, c);
        if resp.clicked() {
            filter(app, json!({"rating": if f.rating == i { 0 } else { i }}));
        }
    }
    ui.add_space(14.0);

    // flags (one at a time; click again to clear)
    caption(ui, "Flag");
    for (id, icon, flag, tip) in [
        ("pick", Icon::FlagPick, Flag::Pick, "Picked"),
        ("reject", Icon::FlagReject, Flag::Reject, "Rejected"),
        ("none", Icon::Circle, Flag::None, "Unflagged"),
    ] {
        let on = f.flag == Some(flag);
        let color = match flag {
            Flag::Pick if on => t.pick,
            Flag::Reject if on => t.reject,
            _ => Color32::TRANSPARENT,
        };
        let resp = toggle(ui, id, on, tip, |p, r, c| paint(p, r, icon, if color == Color32::TRANSPARENT { c } else { color }));
        if resp.clicked() {
            filter(app, json!({"flag": if on { Value::Null } else { json!(id) }}));
        }
    }
    ui.add_space(14.0);

    // colour labels
    caption(ui, "Label");
    // several labels can be on at once (any of them matches)
    let mut chosen: Vec<ColorLabel> = f.labels.clone();
    if let Some(l) = f.label
        && !chosen.contains(&l)
    {
        chosen.push(l);
    }
    for l in ColorLabel::ALL {
        let on = chosen.contains(&l);
        let name = format!("{l:?}").to_lowercase();
        let tip = format!("{} label (click more labels to show any of them)", app.session.catalog.label_name(l));
        let resp = toggle(ui, &format!("label-{name}"), on, &tip, |p, r, _| {
            p.circle_filled(r.center(), 5.5, label_color(l));
        });
        if resp.clicked() {
            let mut next = chosen.clone();
            if on {
                next.retain(|x| *x != l);
            } else {
                next.push(l);
            }
            next.sort();
            let names: Vec<String> = next.iter().map(|x| format!("{x:?}").to_lowercase()).collect();
            filter(app, json!({"label": Value::Null, "labels": names}));
        }
    }
    ui.add_space(14.0);

    // kind + edited
    let kind_label = match f.kind {
        None => "All types",
        Some(MediaKind::Image) => "Photos",
        Some(MediaKind::Raw) => "Raw",
        Some(MediaKind::Video) => "Videos",
    };
    let kind_label = match f.merged.as_deref() {
        Some("hdr") => "HDR",
        Some("panorama") => "Panoramas",
        Some("hdrPanorama") => "HDR Panoramas",
        Some(_) => "Merged",
        None => kind_label,
    };
    picker(
        app,
        ui,
        "kind",
        kind_label,
        f.kind.is_some() || f.merged.is_some(),
        [
            ("All types", Value::Null, None),
            ("Photos", json!("image"), Some(MediaKind::Image)),
            ("Raw", json!("raw"), Some(MediaKind::Raw)),
            ("Videos", json!("video"), Some(MediaKind::Video)),
        ]
        .into_iter()
        .map(|(l, v, k)| (l.to_string(), json!({"kind": v, "merged": null}), f.kind == k && f.merged.is_none()))
        .chain(
            [("HDR", "hdr"), ("Panoramas", "panorama"), ("HDR Panoramas", "hdrPanorama")]
                .into_iter()
                .map(|(l, m)| (l.to_string(), json!({"kind": null, "merged": m}), f.merged.as_deref() == Some(m))),
        )
        .collect(),
    );
    let edited_label = match f.edited {
        None => "Edited or not",
        Some(true) => "Edited",
        Some(false) => "Unedited",
    };
    picker(
        app,
        ui,
        "edited",
        edited_label,
        f.edited.is_some(),
        [("Edited or not", Value::Null, None), ("Edited", json!(true), Some(true)), ("Unedited", json!(false), Some(false))]
            .into_iter()
            .map(|(l, v, e)| (l.to_string(), json!({"edited": v}), f.edited == e))
            .collect(),
    );

    // metadata pickers
    let ui: &mut egui::Ui = if two_rows { &mut row2 } else { &mut row1 };
    let values = app.caches.filter_values(&app.session.catalog);
    let (cameras, lenses, keywords) = (values.cameras.clone(), values.lenses.clone(), values.keywords.clone());
    for (id, key, all_label, current, values) in [
        ("camera", "camera", "Camera", f.camera.clone(), cameras),
        ("lens", "lens", "Lens", f.lens.clone(), lenses),
        ("keyword", "keyword", "Keyword", f.keyword.clone(), keywords),
    ] {
        let mut items = vec![(format!("Any {}", all_label.to_lowercase()), json!({key: Value::Null}), current.is_none())];
        items.extend(values.into_iter().map(|v| (v.clone(), json!({key: v.clone()}), current.as_deref() == Some(v.as_str()))));
        let shown = current.clone().unwrap_or_else(|| all_label.to_string());
        let shown = if shown.chars().count() > 22 { format!("{}…", shown.chars().take(21).collect::<String>()) } else { shown };
        picker(app, ui, id, &shown, current.is_some(), items);
    }

    // save / clear
    ui.add_space(14.0);
    let filtering = app.session.filter != Default::default();
    let save = ui.add_enabled(filtering, egui::Button::new(egui::RichText::new(crate::i18n::tr("Save as Smart Album…")).font(t.font(12.5))));
    register(ui.ctx(), "button:filterSave", save.rect);
    if save.clicked() {
        let _ = app.run("dialog.newSmartAlbum", json!({}));
    }
    let clear = ui.add_enabled(filtering, egui::Button::new(egui::RichText::new(crate::i18n::tr("Clear")).font(t.font(12.5))));
    register(ui.ctx(), "button:filterClear", clear.rect);
    if clear.clicked() {
        app.ui.search.clear();
        let _ = app.run("library.clearFilter", json!({}));
    }
    // filter presets: apply one, save the current filter, delete
    let presets = ui.button(egui::RichText::new(crate::i18n::tr("Presets ▾")).font(t.font(12.5)));
    register(ui.ctx(), "button:filterPresets", presets.rect);
    egui::Popup::menu(&presets).show(|ui| {
        let names: Vec<String> = app.session.filter_presets.iter().map(|f| f.name.clone()).collect();
        for n in &names {
            let r = ui.button(n);
            if r.clicked() {
                let _ = app.run("filter.applyPreset", json!({"name": n}));
            }
            r.context_menu(|ui| {
                if ui.button(crate::i18n::tr("Delete Preset")).clicked() {
                    let _ = app.run("filter.deletePreset", json!({"name": n}));
                }
            });
        }
        if names.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No filter presets yet")).color(t.text_dim));
        }
        ui.separator();
        if ui.add_enabled(filtering, egui::Button::new(crate::i18n::tr("Save Current Filter as Preset…"))).clicked() {
            crate::panels::dialogs::prompt(app, "Save Filter Preset", "Preset name", "", "filter.savePreset", json!({}), "name");
        }
    });
}
