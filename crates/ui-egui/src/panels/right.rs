//! The right-hand panel next to the tool strip: Edit, Crop, Remove, Masking, Red Eye, Info,
//! Keywords, Versions, Activity.

use egui::{Align2, Rect, Sense, pos2, vec2};
use lightcraft_catalog::PhotoId;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{divider, register, slider, text_button};

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider));
    // the presets column and the left sidebar are laid out after this panel: leave them their room
    let reserve = if app.ui.presets { t.panel_w } else { 0.0 } + if app.ui.left_panel { crate::state::LEFT_WIDTH.min } else { 0.0 };
    let width = app.ui.right_width;
    let resized = super::resizable_side(ui, false, "right_panel", frame, width, crate::state::RIGHT_WIDTH, reserve, |ui| {
        let Some(id) = app.session.active() else {
            let r = ui.max_rect();
            super::empty_message(ui, r, "No photo selected", "Select a photo to edit");
            return;
        };
        egui::ScrollArea::vertical().id_salt("right-scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            match app.ui.right {
                RightPanel::Edit => super::edit::show(app, ui, id),
                RightPanel::Profiles => super::profiles::show(app, ui, id),
                RightPanel::Crop => crop(app, ui, id),
                RightPanel::Remove => remove(app, ui, id),
                RightPanel::Masking => super::masking::show(app, ui, id),
                RightPanel::RedEye => red_eye(app, ui, id),
                RightPanel::Info => info(app, ui, id),
                RightPanel::Keywords => keywords(app, ui, id),
                RightPanel::Versions => versions(app, ui, id),
                RightPanel::Activity => activity(app, ui, id),
                RightPanel::None => {}
            }
        });
    });
    if let Some(w) = resized {
        app.ui.right_width = w;
    }
}

pub fn header(ui: &mut egui::Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y + 2.0), Align2::LEFT_CENTER, title, t.semibold(15.0), t.text);
}

pub fn label_row(ui: &mut egui::Ui, label: &str, value: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    ui.painter().text(pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::tr(label), t.font(12.5), t.text_dim);
    ui.painter().text(pos2(r.left() + 110.0, r.center().y), Align2::LEFT_CENTER, value, t.font(12.5), t.text_label);
}

fn padded(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 24, right: 22, top: 6, bottom: 6 }).show(ui, add);
}

fn crop(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Crop");
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Aspect Ratio"));
            let cur = d.crop.aspect.map(|(w, h)| format!("{} × {}", w as f64 / 100.0, h as f64 / 100.0)).unwrap_or_else(|| "Free".into());
            let r = ui.add(egui::Button::new(cur).frame(false));
            register(ui.ctx(), "button:cropAspect", r.rect);
            egui::Popup::menu(&r).show(|ui| {
                for (label, a) in [
                    ("Free", "free"),
                    ("Original", "original"),
                    ("1 × 1", "1x1"),
                    ("4 × 5 / 8 × 10", "4x5"),
                    ("8.5 × 11", "8.5x11"),
                    ("5 × 7", "5x7"),
                    ("2 × 3 / 4 × 6", "2x3"),
                    ("4 × 3", "4x3"),
                    ("16 × 9", "16x9"),
                    ("16 × 10", "16x10"),
                ] {
                    if ui.button(label).clicked() {
                        let _ = app.run("crop.aspect", json!({"aspect": a}));
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if text_button(ui, "cropRotateLeft", crate::i18n::tr("Rotate Left"), false).clicked() {
                let _ = app.run("photo.rotateLeft", json!({}));
            }
            if text_button(ui, "cropRotateRight", crate::i18n::tr("Rotate Right"), false).clicked() {
                let _ = app.run("photo.rotateRight", json!({}));
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if text_button(ui, "cropFlipH", crate::i18n::tr("Flip H"), d.crop.flip_h).clicked() {
                let _ = app.run("photo.flipHorizontal", json!({}));
            }
            if text_button(ui, "cropFlipV", crate::i18n::tr("Flip V"), d.crop.flip_v).clicked() {
                let _ = app.run("photo.flipVertical", json!({}));
            }
            if text_button(ui, "cropSwapAspect", crate::i18n::tr("Swap Aspect (X)"), false).clicked() {
                let _ = app.run("crop.rotateAspect", json!({}));
            }
            if text_button(ui, "cropReset", crate::i18n::tr("Reset"), false).clicked() {
                let _ = app.run("crop.reset", json!({}));
            }
        });
    });
    let ang = lightcraft_develop::controls::find("crop.angle").copied();
    if let Some(spec) = ang {
        let out = slider(ui, &spec, d.crop.geometry.angle, true, Some("Straighten"));
        super::edit::apply_slider_out(app, &spec, out, |app, v| app.run("crop.straighten", json!({"angle": v})));
    }
    padded(ui, |ui| {
        ui.horizontal(|ui| {
            let on = app.ui.tool == "straighten";
            if text_button(ui, "straightenTool", crate::i18n::tr("Straighten Tool"), on)
                .on_hover_text(crate::i18n::tr("Drag along the horizon; double-click for Auto"))
                .clicked()
            {
                app.ui.tool = if on { String::new() } else { "straighten".into() };
            }
            if text_button(ui, "straightenAuto", crate::i18n::tr("Auto"), false).clicked() {
                let _ = app.run("crop.autoStraighten", json!({}));
            }
        });
    });
    padded(ui, |ui| {
        ui.label(crate::i18n::tr("Overlay"));
        {
            use crate::state::CropOverlay as O;
            let opts = [
                (O::Thirds, "Thirds", "thirds"),
                (O::Grid, "Grid", "grid"),
                (O::Golden, "Golden", "golden"),
                (O::Diagonal, "Diagonal", "diagonal"),
                (O::Triangle, "Triangle", "triangle"),
                (O::Spiral, "Spiral", "spiral"),
                (O::None, "None", "none"),
            ];
            let items: Vec<(&str, &str)> = opts.iter().map(|(_, l, k)| (*l, *k)).collect();
            let active = opts.iter().position(|(o, _, _)| *o == app.ui.crop_overlay);
            if let Some(i) = crate::widgets::segmented(ui, "cropOverlay", &items, active, 4) {
                app.ui.crop_overlay = opts[i].0;
            }
            if matches!(app.ui.crop_overlay, O::Triangle | O::Spiral)
                && text_button(ui, "cropOverlayOrient", crate::i18n::tr("Flip Overlay (⇧O)"), false).clicked()
            {
                let _ = app.run("view.cropOverlayOrientation", json!({}));
            }
        }
    });
    divider(ui);
    header(ui, "Geometry");
    padded(ui, |ui| {
        ui.label(crate::i18n::tr("Upright"));
        {
            use lightcraft_develop::Upright;
            let modes = [
                ("Off", Upright::Off, "off"),
                ("Auto", Upright::Auto, "auto"),
                ("Guided", Upright::Guided, "guided"),
                ("Level", Upright::Level, "level"),
                ("Vertical", Upright::Vertical, "vertical"),
                ("Full", Upright::Full, "full"),
            ];
            let items: Vec<(&str, &str)> = modes.iter().map(|(l, _, k)| (*l, *k)).collect();
            let active = modes.iter().position(|(_, m, _)| *m == d.geometry.upright);
            if let Some(i) = crate::widgets::segmented(ui, "upright", &items, active, 3) {
                let (_, mode, key) = modes[i];
                let _ = app.run("geometry.upright", json!({"mode": key}));
                app.ui.tool = if mode == Upright::Guided { "guidedUpright".into() } else { String::new() };
            }
        }
        let guided = d.geometry.upright == lightcraft_develop::Upright::Guided;
        ui.horizontal_wrapped(|ui| {
            if !guided
                && d.geometry.upright != lightcraft_develop::Upright::Off
                && text_button(ui, "uprightUpdate", crate::i18n::tr("Update"), false).clicked()
            {
                let mode = serde_json::to_value(d.geometry.upright).unwrap_or_default();
                let _ = app.run("geometry.upright", json!({"mode": mode}));
            }
            if guided {
                let drawing = app.ui.tool == "guidedUpright";
                if text_button(ui, "uprightDraw", crate::i18n::tr("Draw Guides"), drawing).clicked() {
                    app.ui.tool = if drawing { String::new() } else { "guidedUpright".into() };
                }
                if !d.geometry.guides.is_empty() && text_button(ui, "uprightClear", crate::i18n::tr("Clear Guides"), false).clicked() {
                    let _ = app.run("geometry.guides", json!({"guides": []}));
                }
            }
        });
        if guided {
            ui.label(
                egui::RichText::new(crate::i18n::tr_format!(
                    "{} of 4 guides — drag along lines that should be vertical or horizontal.",
                    d.geometry.guides.len()
                ))
                .size(11.0)
                .color(Tokens::get(ui.ctx()).text_dim),
            );
        }
        ui.add_space(4.0);
        let mut c = d.geometry.constrain_crop;
        if ui.checkbox(&mut c, crate::i18n::tr("Constrain Crop")).changed() {
            let _ = app.run("develop.merge", json!({"settings": {"geometry": {"constrain_crop": c}}, "label": "Constrain Crop"}));
        }
    });
    for spec in lightcraft_develop::controls::in_section(lightcraft_develop::Section::Geometry).filter(|c| c.id != "crop.angle") {
        let v = lightcraft_develop::controls::get(&d, spec.id).unwrap_or(spec.default);
        let out = slider(ui, spec, v, true, None);
        super::edit::apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": spec.id, "value": v})));
    }
}

fn remove(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Remove");
    padded(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, tool) in [("Remove", "remove"), ("Heal", "heal"), ("Clone", "clone")] {
                if text_button(ui, &format!("removeMode-{tool}"), label, app.ui.tool == tool).clicked() {
                    app.ui.tool = tool.into();
                }
            }
        });
        ui.add_space(8.0);
        ui.label(crate::i18n::tr_format!("{} spot(s) on this photo", d.spots.len()));
        ui.add_space(4.0);
        if text_button(ui, "findDust", crate::i18n::tr("Find Dust Spots"), false)
            .on_hover_text(crate::i18n::tr("Heal the small, soft dark spots sensor dust leaves on smooth areas"))
            .clicked()
        {
            match app.run("spot.findDust", json!({})) {
                Ok(r) => {
                    let n = r["added"].as_u64().unwrap_or(0);
                    app.toast(
                        ui.ctx(),
                        if n == 0 {
                            "No dust spots found".to_string()
                        } else {
                            crate::i18n::tr_format!("Healed {n} dust spot{}", if n == 1 { "" } else { "s" }, n = n)
                        },
                    );
                }
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new(crate::i18n::tr("Paint over a distraction on the photo to remove it.")).color(Tokens::get(ui.ctx()).text_dim));
    });
    // brush settings; with a spot selected they edit that spot too
    let sel = app.session.active_spot.and_then(|i| d.spots.get(i).map(|sp| (i, sp.clone())));
    if let Some((i, sp)) = &sel {
        (app.ui.remove_size, app.ui.remove_feather, app.ui.remove_opacity) = (sp.size as f32, sp.feather as f32, sp.opacity as f32);
        divider(ui);
        let mode = match sp.mode {
            lightcraft_develop::SpotMode::Heal => "Heal",
            lightcraft_develop::SpotMode::Clone => "Clone",
            lightcraft_develop::SpotMode::Remove => "Remove",
        };
        super::edit::sub_title(ui, &crate::i18n::tr_format!("{mode} spot {} of {}", i + 1, d.spots.len(), mode = mode));
    }
    let plain = |id: &'static str, label: &'static str, min: f64, max: f64, default: f64| lightcraft_develop::ControlSpec {
        id,
        label,
        section: lightcraft_develop::Section::Detail,
        min,
        max,
        default,
        step: 1.0,
        decimals: 0,
        track: lightcraft_develop::Track::Plain,
    };
    let sliders = [
        (plain("ui.removeSize", "Size", 1.0, 250.0, 20.0), "size", (app.ui.remove_size * 1000.0) as f64),
        (plain("ui.removeFeather", "Feather", 0.0, 100.0, 50.0), "feather", app.ui.remove_feather as f64),
        (plain("ui.removeOpacity", "Opacity", 0.0, 100.0, 100.0), "opacity", app.ui.remove_opacity as f64),
    ];
    for (spec, key, v) in sliders {
        let out = slider(ui, &spec, v, true, None);
        let scale = if key == "size" { 1000.0 } else { 1.0 };
        if let Some(v) = out.value {
            match key {
                "size" => app.ui.remove_size = (v / scale) as f32,
                "feather" => app.ui.remove_feather = v as f32,
                _ => app.ui.remove_opacity = v as f32,
            }
        }
        if sel.is_some() {
            super::edit::apply_slider_out(app, &spec, out, |app, v| app.run("spot.update", json!({key: v / scale})));
        }
    }
    if sel.is_some() {
        padded(ui, |ui| {
            ui.horizontal(|ui| {
                if text_button(ui, "spotRefresh", crate::i18n::tr("Refresh Source (/)"), false).clicked() {
                    let _ = app.run("spot.refreshSource", json!({}));
                }
                if text_button(ui, "spotDelete", crate::i18n::tr("Delete (⌫)"), false).clicked() {
                    let _ = app.run("spot.delete", json!({}));
                }
            });
        });
    }
    // Visualize Spots (A): a black/white high-pass view that makes dust and specks stand out
    padded(ui, |ui| {
        let mut v = app.ui.visualize_spots;
        if ui.checkbox(&mut v, crate::i18n::tr("Visualize Spots (A)")).changed() {
            app.ui.visualize_spots = v;
        }
    });
    let spec = lightcraft_develop::ControlSpec {
        id: "ui.spotsThreshold",
        label: "Threshold",
        section: lightcraft_develop::Section::Detail,
        min: 0.0,
        max: 100.0,
        default: 50.0,
        step: 1.0,
        decimals: 0,
        track: lightcraft_develop::Track::Plain,
    };
    let out = slider(ui, &spec, app.ui.spots_threshold as f64, app.ui.visualize_spots, None);
    if let Some(v) = out.value {
        app.ui.spots_threshold = v as f32;
    }
    padded(ui, |ui| {
        if !d.spots.is_empty() && text_button(ui, "removeClear", crate::i18n::tr("Delete all spots"), false).clicked() {
            for i in (0..d.spots.len()).rev() {
                let _ = app.run("spot.delete", json!({"index": i}));
            }
        }
    });
}

fn red_eye(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let d = app.session.develop_of(id).unwrap_or_default();
    header(ui, "Red Eye");
    padded(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (label, pet) in [("Red Eye", false), ("Pet Eye", true)] {
                if text_button(ui, &format!("eyeMode-{}", if pet { "pet" } else { "red" }), label, app.ui.eye_pet == pet).clicked() {
                    app.ui.eye_pet = pet;
                }
            }
        });
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(crate::i18n::tr("Drag over an eye on the photo; the pupil inside is found automatically."))
                .color(Tokens::get(ui.ctx()).text_dim),
        );
    });
    let n = d.red_eye.len();
    if n == 0 {
        return;
    }
    if app.ui.eye >= n {
        app.ui.eye = n - 1;
    }
    let i = app.ui.eye;
    let eye = d.red_eye[i];
    divider(ui);
    super::edit::sub_title(ui, &format!("{} {} of {n}", if eye.pet { "Pet Eye" } else { "Red Eye" }, i + 1));
    for k in ["pupilSize", "darken"] {
        let cid = format!("redEye.{i}.{k}");
        let Some(spec) = lightcraft_develop::controls::find(&cid) else { continue };
        let v = lightcraft_develop::controls::get(&d, &cid).unwrap_or(spec.default);
        let out = slider(ui, spec, v, true, None);
        super::edit::apply_slider_out(app, spec, out, |app, v| app.run("develop.set", json!({"control": cid, "value": v})));
    }
    padded(ui, |ui| {
        if eye.pet {
            let mut on = eye.catchlight.is_some();
            if ui.checkbox(&mut on, crate::i18n::tr("Add Catchlight")).changed() {
                let _ = app.run("redeye.catchlight", json!({"index": i, "on": on}));
            }
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            if text_button(ui, "eyeDelete", crate::i18n::tr("Delete"), false).clicked() {
                let _ = app.run("redeye.delete", json!({"index": i}));
            }
            if n > 1 && text_button(ui, "eyeDeleteAll", crate::i18n::tr("Delete all"), false).clicked() {
                for k in (0..n).rev() {
                    let _ = app.run("redeye.delete", json!({"index": k}));
                }
            }
        });
    });
}

fn info(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    header(ui, "Info");
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        camera_card(ui, &p);
        ui.add_space(6.0);
        if let Some(why) = &p.preview_only {
            crate::widgets::preview_only_notice(ui, "info", why);
            ui.add_space(6.0);
        }
        if let Some(name) = &p.copy_name {
            let of = p.copy_of.and_then(|m| app.session.catalog.photo(m)).map(|m| m.file_name.clone()).unwrap_or_else(|| "a removed photo".into());
            ui.label(egui::RichText::new(format!("Virtual copy “{name}” of {of}")).color(t.text_label));
        }
        ui.add_space(8.0);
        if let Some(r) = ui.horizontal(|ui| crate::widgets::stars(ui, "info", p.rating, 20.0)).inner {
            let _ = app.run("photo.rate", json!({"rating": r}));
        }
        ui.add_space(6.0);
        // colour label: one swatch per label (hover shows its name); clicking the current one clears it
        ui.horizontal(|ui| {
            for l in lightcraft_catalog::ColorLabel::ALL {
                let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                let key = format!("{l:?}").to_lowercase();
                register(ui.ctx(), format!("label:{key}"), r);
                let on = p.label == Some(l);
                ui.painter().circle_filled(r.center(), if on || resp.hovered() { 8.0 } else { 6.5 }, crate::panels::grid::label_color(l));
                if on {
                    ui.painter().circle_stroke(r.center(), 10.0, egui::Stroke::new(1.5, t.text));
                }
                let resp = resp.on_hover_text(app.session.catalog.label_name(l));
                if resp.clicked() {
                    let _ = app.run("photo.label", json!({"label": if on { "none".to_string() } else { key }}));
                }
            }
            if let Some(l) = p.label {
                ui.label(egui::RichText::new(app.session.catalog.label_name(l)).color(t.text_label));
            }
        });
    });
    divider(ui);
    let m = p.meta.clone();
    padded(ui, |ui| {
        for (label, key, value, lines) in [
            ("Title", "title", &m.title, 1),
            ("Caption", "caption", &m.caption, 2),
            ("Alt Text", "altText", &m.alt_text, 2),
            ("Extended Description", "extendedDescription", &m.extended_description, 3),
            ("Copyright", "copyright", &m.copyright, 1),
        ] {
            meta_field(app, ui, label, key, value, lines);
        }
        copyright_status(app, ui, m.copyright_status);
        for (label, key, value, lines) in [
            ("Rights Usage Terms", "usageTerms", &m.usage_terms, 2),
            ("Copyright Info URL", "copyrightUrl", &m.copyright_url, 1),
            ("Creator", "creator", &m.creator, 1),
        ] {
            meta_field(app, ui, label, key, value, lines);
        }
        // file: name (rename), path (reveal), capture time (edit)
        let small = |ui: &mut egui::Ui, text: &str| ui.label(egui::RichText::new(text).size(11.5).color(t.text_dim));
        small(ui, "File Name");
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::icon_button(ui, "infoRename", Icon::Pencil, vec2(20.0, 20.0), false, true, "Rename…").clicked() {
                let _ = app.run("dialog.rename", json!({}));
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&p.file_name).color(t.text)).truncate());
            });
        });
        ui.add_space(4.0);
        small(ui, "File Path");
        ui.horizontal(|ui| {
            let path = match &p.source {
                lightcraft_catalog::Source::File { path } => path.clone(),
                lightcraft_catalog::Source::Demo { .. } => "Generated demo photo".into(),
            };
            ui.add(egui::Label::new(egui::RichText::new(path).size(12.0).color(t.text_label)).truncate());
            if crate::menus::ui_enabled(app, "app.showInFinder") {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::widgets::icon_button(ui, "infoReveal", Icon::Folder, vec2(20.0, 20.0), false, true, "Show in Finder").clicked() {
                        let _ = app.run("app.showInFinder", json!({}));
                    }
                });
            }
        });
        ui.add_space(4.0);
        small(ui, "Captured");
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 22.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::icon_button(ui, "editCaptureTime", Icon::Pencil, vec2(20.0, 20.0), false, true, "Edit Capture Time…").clicked() {
                let _ = app.run("dialog.captureTime", json!({}));
            }
            let cap = p.captured.as_deref().map(lightcraft_catalog::dates::display_time).unwrap_or_else(|| "—".into());
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(cap).size(12.5).color(t.text)).truncate());
            });
        });
        ui.add_space(6.0);
        for (label, key, value) in [
            ("Location", "location", &m.location),
            ("City", "city", &m.city),
            ("State / Province", "state", &m.state),
            ("Country", "country", &m.country),
        ] {
            meta_field(app, ui, label, key, value, 1);
        }
        // GPS: typed as "lat, lon" (or degrees / minutes / seconds); empty clears it
        let gps = m.gps.map(|(la, lo)| format!("{la:.6}, {lo:.6}")).unwrap_or_default();
        let gid = egui::Id::new("info-gps");
        ui.label(egui::RichText::new("GPS").size(11.5).color(t.text_dim));
        let mut text: String = ui.data(|d| d.get_temp(gid)).unwrap_or_else(|| gps.clone());
        let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text("latitude, longitude").desired_width(f32::INFINITY));
        register(ui.ctx(), "field:gps", r.rect);
        if r.has_focus() {
            ui.data_mut(|d| d.insert_temp(gid, text.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(gid));
        }
        if r.lost_focus()
            && text.trim() != gps
            && let Err(e) = app.run("photo.setMeta", json!({"gps": text.trim()}))
        {
            app.toast(ui.ctx(), e);
        }
        if let Some((la, lo)) = m.gps {
            let pretty = format!("{:.5}° {}, {:.5}° {}", la.abs(), if la >= 0.0 { "N" } else { "S" }, lo.abs(), if lo >= 0.0 { "E" } else { "W" });
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(pretty).size(11.0).color(t.text_dim));
                if text_button(ui, "showOnMap", crate::i18n::tr("Show on Map"), false)
                    .on_hover_text(crate::i18n::tr("Open the place in OpenStreetMap"))
                    .clicked()
                {
                    let url = format!("https://www.openstreetmap.org/?mlat={la:.6}&mlon={lo:.6}#map=15/{la:.6}/{lo:.6}");
                    if let Err(e) = crate::links::open(app, &url) {
                        app.toast(ui.ctx(), e);
                    }
                }
            });
        }
        if let Some(a) = p.analysis {
            let mut line = format!("Focus {:.0}", a.sharpness);
            if a.clipped > 0.02 {
                line.push_str(&format!(" · {:.0}% clipped", a.clipped * 100.0));
            }
            if a.group.is_some() {
                line.push_str(if a.best { " · best of its burst" } else { " · in a burst" });
            }
            ui.add_space(4.0);
            ui.label(egui::RichText::new(line).color(t.text_dim));
        }
        // offline originals / smart previews
        // (cached answers, checked off the UI thread: unknown counts as online / no smart preview)
        if let lightcraft_catalog::Source::File { path } = &p.source {
            let avail = &app.session.media.availability;
            let online = !avail.is_offline(path);
            let smart = app
                .session
                .media
                .smart_dir
                .as_ref()
                .is_some_and(|d| avail.exists(&d.join(lightcraft_engine::smart::file_name(&p)).to_string_lossy()) == Some(true));
            if !online || smart {
                let text = match (online, smart) {
                    (false, true) => "Original offline · editing the smart preview",
                    (false, false) => "Original offline · no smart preview",
                    _ => "Smart preview available",
                };
                ui.add_space(6.0);
                ui.label(egui::RichText::new(text).color(if online { t.text_dim } else { t.accent }));
            }
        }
        ui.add_space(10.0);
        if text_button(ui, "allMetadata", crate::i18n::tr("All Metadata…"), false)
            .on_hover_text(crate::i18n::tr("Every EXIF, GPS and XMP field in the file"))
            .clicked()
        {
            let _ = app.run("dialog.allMetadata", json!({}));
        }
    });
}

/// The camera card at the top of Info: camera, lens, size · file size and format, then the
/// capture settings.
fn camera_card(ui: &mut egui::Ui, p: &lightcraft_catalog::Photo) {
    let t = Tokens::get(ui.ctx());
    let m = &p.meta;
    egui::Frame::NONE.fill(t.canvas).corner_radius(6.0).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let dim = |s: &str| egui::RichText::new(s.to_string()).size(12.0).color(t.text_label);
        if !m.camera.is_empty() {
            ui.label(dim(&m.camera));
        }
        if !m.lens.is_empty() {
            ui.add(egui::Label::new(dim(&m.lens)).truncate());
        }
        ui.horizontal(|ui| {
            ui.label(dim(&format!("{} × {}  ·  {}", p.width, p.height, human_size(p.file_size))));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::Frame::NONE.fill(t.chrome).corner_radius(3.0).inner_margin(egui::Margin::symmetric(5, 1)).show(ui, |ui| {
                    ui.label(egui::RichText::new(p.format.to_uppercase()).size(10.5).color(t.text_label));
                });
            });
        });
        let rows = [
            ("Focal length", m.focal_mm.map(|f| format!("{f:.1} mm").replace(".0 mm", " mm"))),
            ("Shutter speed", (!m.shutter.is_empty()).then(|| format!("{} sec", m.shutter))),
            ("Aperture", m.aperture.map(|f| format!("f / {f:.1}").replace(".0", ""))),
            ("ISO", m.iso.map(|i| i.to_string())),
        ];
        if rows.iter().any(|r| r.1.is_some()) {
            ui.add_space(6.0);
            egui::Grid::new("info-capture").num_columns(2).spacing([24.0, 3.0]).show(ui, |ui| {
                for (k, v) in rows {
                    ui.label(dim(k));
                    ui.label(dim(&v.unwrap_or_else(|| "—".into())));
                    ui.end_row();
                }
            });
        }
    });
}

fn human_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} B"),
    }
}

/// A labelled metadata text field: the typed text lives in egui memory while focused and is
/// saved (photo.setMeta `key`) when the field loses focus.
/// Copyright Status: Unknown / Copyrighted / Public Domain (`xmpRights:Marked`).
fn copyright_status(app: &mut LightcraftApp, ui: &mut egui::Ui, current: lightcraft_catalog::CopyrightStatus) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::tr("Copyright Status")).size(11.5).color(t.text_dim));
    let r = egui::ComboBox::from_id_salt("info-copyright-status").selected_text(crate::i18n::tr(current.label())).show_ui(ui, |ui| {
        for st in lightcraft_catalog::CopyrightStatus::ALL {
            if ui.selectable_label(st == current, crate::i18n::tr(st.label())).clicked() && st != current {
                let _ = app.run("photo.setMeta", json!({"copyrightStatus": st.id()}));
            }
        }
    });
    register(ui.ctx(), "field:copyrightStatus", r.response.rect);
    ui.add_space(6.0);
}

fn meta_field(app: &mut LightcraftApp, ui: &mut egui::Ui, label: &str, key: &str, value: &str, lines: usize) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(label).size(11.5).color(t.text_dim));
    let id = egui::Id::new(("info-field", key));
    let mut text: String = ui.data(|d| d.get_temp(id)).unwrap_or_else(|| value.to_string());
    let edit = if lines > 1 { egui::TextEdit::multiline(&mut text).desired_rows(lines) } else { egui::TextEdit::singleline(&mut text) };
    let r = ui.add(edit.desired_width(f32::INFINITY));
    register(ui.ctx(), format!("field:{key}"), r.rect);
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    if r.lost_focus() && text != value {
        let _ = app.run("photo.setMeta", json!({key: text}));
    }
    ui.add_space(6.0);
}

fn keywords(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    header(ui, "Keywords");
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        let kid = egui::Id::new("kw-input");
        let mut text = ui.data_mut(|d| d.get_temp::<String>(kid).unwrap_or_default());
        let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text(crate::i18n::tr("Add keyword")).desired_width(f32::INFINITY));
        register(ui.ctx(), "field:keyword", r.rect);
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
            let kws: Vec<String> = text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let _ = app.run("photo.setMeta", json!({"addKeywords": kws}));
            text.clear();
        }
        ui.data_mut(|d| d.insert_temp(kid, text));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for k in &p.meta.keywords {
                let r =
                    ui.add(egui::Button::new(egui::RichText::new(format!("{}  ×", k.replace('|', " › "))).color(t.text_label)).corner_radius(10.0));
                register(ui.ctx(), format!("keywordChip:{k}"), r.rect);
                if r.clicked() {
                    let _ = app.run("photo.setMeta", json!({"removeKeywords": [k]}));
                }
                r.context_menu(|ui| {
                    if ui.button(crate::i18n::tr("Remove from Photo")).clicked() {
                        let _ = app.run("photo.setMeta", json!({"removeKeywords": [k]}));
                    }
                    if ui.button(crate::i18n::tr("Show Photos with Keyword")).clicked() {
                        let _ = app.run("library.filter", json!({"keyword": k}));
                    }
                    ui.separator();
                    if ui.button(crate::i18n::tr("Rename Keyword…")).clicked() {
                        app.ui.dialog = Some(crate::state::Dialog::RenameKeyword { from: k.clone(), to: k.clone() });
                    }
                    if ui.button(crate::i18n::tr("Delete Keyword")).clicked() {
                        let _ = app.run("keyword.delete", json!({"keyword": k}));
                    }
                });
            }
        });
        ui.add_space(10.0);
        keyword_set(app, ui, &p.meta.keywords);
        ui.add_space(8.0);
        // the painter: click photos in the grid to give them (or take away) a keyword
        ui.horizontal(|ui| {
            let pid = egui::Id::new("kw-painter");
            let painting = app.ui.keyword_painter.clone();
            let mut k: String = ui.data(|d| d.get_temp(pid)).unwrap_or_else(|| painting.clone().unwrap_or_default());
            let r = ui.add_enabled(
                painting.is_none(),
                egui::TextEdit::singleline(&mut k).hint_text(crate::i18n::tr("Keyword to paint")).desired_width(130.0),
            );
            register(ui.ctx(), "field:keywordPainter", r.rect);
            ui.data_mut(|d| d.insert_temp(pid, k.clone()));
            let label = if painting.is_some() { "Stop" } else { "Paint" };
            if text_button(ui, "keywordPaint", label, painting.is_some())
                .on_hover_text(crate::i18n::tr("Click photos in the grid to toggle the keyword; Esc stops"))
                .clicked()
            {
                let _ = app.run("tool.keywordPainter", json!({"keyword": if painting.is_some() { serde_json::Value::Null } else { json!(k) }}));
            }
        });
        ui.add_space(10.0);
        // suggestions: completions of the typed text, else keywords used together with this
        // photo's keywords, else the most used ones
        let typed = ui.data(|d| d.get_temp::<String>(kid).unwrap_or_default());
        let last = typed.rsplit(',').next().unwrap_or("").trim().to_string();
        let suggestions = (*app.caches.suggestions(&app.session.catalog, &p.meta.keywords, &last, 12)).clone();
        if !suggestions.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("Suggestions")).color(t.text_dim));
            ui.horizontal_wrapped(|ui| {
                for k in suggestions {
                    let r = ui
                        .add(egui::Button::new(egui::RichText::new(format!("+ {}", k.replace('|', " › "))).color(t.text_label)).corner_radius(10.0));
                    register(ui.ctx(), format!("kwSuggest:{k}"), r.rect);
                    if r.clicked() {
                        let _ = app.run("photo.setMeta", json!({"addKeywords": [k]}));
                        if !last.is_empty() {
                            ui.data_mut(|d| d.insert_temp(kid, String::new()));
                        }
                    }
                }
            });
        }
    });
}

/// The keyword set: pick a set, then nine buttons (⌥1–⌥9) that toggle its keywords on the
/// selected photos; "Save as Set…" keeps the current nine under a name.
fn keyword_set(app: &mut LightcraftApp, ui: &mut egui::Ui, have: &[String]) {
    let t = Tokens::get(ui.ctx());
    let sets = lightcraft_engine::cmd::keywords::keyword_sets_json(&app.session);
    let current = sets["current"].as_str().unwrap_or_default().to_string();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr("Keyword Set")).color(t.text_dim));
        egui::ComboBox::from_id_salt("kw-set").selected_text(&current).show_ui(ui, |ui| {
            for set in sets["sets"].as_array().into_iter().flatten() {
                let name = set["name"].as_str().unwrap_or_default();
                if ui.selectable_label(name == current, name).clicked() {
                    let _ = app.run("keyword.useSet", json!({"name": name}));
                }
            }
            ui.separator();
            if ui.button(crate::i18n::tr("Save Current Keywords as Set…")).clicked() {
                app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                    title: "Save Keyword Set".into(),
                    hint: "Set name".into(),
                    value: String::new(),
                    command: "keyword.saveSet".into(),
                    params: json!({}),
                    key: "name".into(),
                });
            }
            if current != lightcraft_engine::cmd::keywords::RECENT
                && ui.button(crate::i18n::tr_format!("Delete “{current}”", current = current)).clicked()
            {
                let _ = app.run("keyword.deleteSet", json!({"name": current}));
            }
        });
    });
    let kws: Vec<String> = sets["keywords"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect();
    if kws.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tr("Keywords you add appear here; ⌥1–⌥9 apply them.")).color(t.text_dim));
        return;
    }
    let bw = ((ui.available_width() - 8.0) / 3.0).floor().max(40.0);
    egui::Grid::new("kw-set-grid").num_columns(3).spacing([4.0, 4.0]).show(ui, |ui| {
        for (i, k) in kws.iter().enumerate() {
            let on = have.iter().any(|x| x.eq_ignore_ascii_case(k));
            let short = k.rsplit('|').next().unwrap_or(k);
            let r = ui
                .add_sized(
                    [bw, 22.0],
                    egui::Button::new(egui::RichText::new(short).size(11.5).color(if on { t.text } else { t.text_label })).selected(on).truncate(),
                )
                .on_hover_text(format!("{} — ⌥{}", k.replace('|', " › "), i + 1));
            register(ui.ctx(), format!("kwSet:{}", i + 1), r.rect);
            if r.clicked() {
                let _ = app.run("keyword.toggleFromSet", json!({"index": i + 1}));
            }
            if i % 3 == 2 {
                ui.end_row();
            }
        }
    });
}

fn versions(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    header(ui, "Versions");
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        if text_button(ui, "versionCreate", crate::i18n::tr("Create Version"), false).clicked() {
            let _ = app.run("version.create", json!({}));
        }
        ui.add_space(8.0);
        // Named (made by you) and Auto (made by LightCraft) versions
        let tab_id = egui::Id::new("versions-tab");
        let mut auto: bool = ui.data(|d| d.get_temp(tab_id)).unwrap_or(false);
        let named_n = p.versions.iter().filter(|v| !v.auto).count();
        let items = [("Named", "named"), ("Auto", "auto")];
        let labels = [crate::i18n::tr_format!("Named ({named_n})", named_n = named_n), format!("Auto ({})", p.versions.len() - named_n)];
        let items: Vec<(&str, &str)> = items.iter().zip(&labels).map(|((_, k), l)| (l.as_str(), *k)).collect();
        if let Some(i) = crate::widgets::segmented(ui, "versionsTab", &items, Some(auto as usize), 2) {
            auto = i == 1;
            ui.data_mut(|d| d.insert_temp(tab_id, auto));
        }
        ui.add_space(6.0);
        let shown: Vec<(usize, &lightcraft_catalog::Version)> = p.versions.iter().enumerate().filter(|(_, v)| v.auto == auto).collect();
        if shown.is_empty() {
            let msg = if auto { "No automatic versions." } else { "No versions yet. Create one to keep this look." };
            ui.label(egui::RichText::new(msg).color(t.text_dim));
        }
        // newest first
        for (i, v) in shown.into_iter().rev() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
            register(ui.ctx(), format!("version:{i}"), r);
            let current = *v.settings == *p.develop;
            if resp.hovered() {
                ui.painter().rect_filled(r, 4.0, t.hover.gamma_multiply(0.7));
            }
            // thumbnail of the version's look
            let tr = Rect::from_min_size(r.min + vec2(4.0, 5.0), vec2(72.0, 48.0));
            ui.painter().rect_filled(tr, 2.0, t.canvas);
            if let Some(job) = app.session.variant_job(id, &v.settings, 160)
                && let Some(tex) = app.renderer.variant(job)
            {
                ui.painter().image(tex.tex.id(), tr, crate::panels::presets::cover_uv(tr, tex.size), egui::Color32::WHITE);
            }
            let x = tr.right() + 10.0;
            ui.painter().with_clip_rect(r.with_max_x(r.right() - 24.0)).text(
                pos2(x, r.top() + 18.0),
                Align2::LEFT_CENTER,
                &v.name,
                t.semibold(13.0),
                t.text,
            );
            let painter = ui.painter().with_clip_rect(r);
            painter.text(pos2(x, r.top() + 38.0), Align2::LEFT_CENTER, short_time(&v.created), t.font(11.0), t.text_dim);
            if current {
                paint(ui.painter(), Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), vec2(14.0, 14.0)), Icon::Check, t.accent);
            }
            // resting on a version shows it in the loupe (nothing is changed)
            if resp.hovered() && !current {
                app.hover_preview =
                    Some(crate::HoverPreview { label: crate::i18n::tr_format!("Version: {}", v.name), settings: (*v.settings).clone() });
            }
            if resp.double_clicked() {
                crate::panels::dialogs::prompt(app, "Rename Version", "Version name", &v.name, "version.rename", json!({"index": i}), "name");
            } else if resp.clicked() && !current {
                let _ = app.run("version.restore", json!({"index": i}));
            }
            resp.on_hover_text(if current { "The photo has these settings" } else { "Click to restore · double-click to rename" }).context_menu(
                |ui| {
                    if ui.button(crate::i18n::tr("Restore")).clicked() {
                        let _ = app.run("version.restore", json!({"index": i}));
                    }
                    if ui.button(crate::i18n::tr("Update with Current Settings")).clicked() {
                        let _ = app.run("version.update", json!({"index": i}));
                    }
                    if ui.button(crate::i18n::tr("Rename…")).clicked() {
                        crate::panels::dialogs::prompt(app, "Rename Version", "Version name", &v.name, "version.rename", json!({"index": i}), "name");
                    }
                    if ui.button(crate::i18n::tr("Set as Before")).clicked() {
                        let _ = app.run("beforeAfter.setBefore", json!({"source": "version", "name": v.name}));
                    }
                    ui.separator();
                    if ui.button(crate::i18n::tr("Delete")).clicked() {
                        let _ = app.run("version.delete", json!({"index": i}));
                    }
                },
            );
        }
    });
}

/// "Sep 30, 2026, 12:00 PM" from an ISO time.
fn short_time(iso: &str) -> String {
    let long = lightcraft_catalog::dates::display_time(iso);
    // "September 30, 2026 at 12:00:00 PM" → month abbreviated, seconds dropped
    let (date, time) = long.split_once(" at ").map_or((long.as_str(), None), |(d, t)| (d, Some(t)));
    let date = match date.split_once(' ') {
        Some((m, rest)) => format!("{} {rest}", m.chars().take(3).collect::<String>()),
        None => date.to_string(),
    };
    match time.and_then(|t| t.rsplit_once(' ')) {
        Some((hms, ampm)) => format!("{date}, {} {ampm}", hms.rsplit_once(':').map_or(hms, |x| x.0)),
        None => date,
    }
}

fn activity(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let Some(p) = app.session.catalog.photo(id).cloned() else { return };
    header(ui, "History");
    let t = Tokens::get(ui.ctx());
    padded(ui, |ui| {
        if p.history.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::tr("No edits yet.")).color(t.text_dim));
        }
        for (i, h) in p.history.iter().enumerate().rev() {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
            register(ui.ctx(), format!("history:{i}"), r);
            if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, t.hover);
            }
            paint(ui.painter(), Rect::from_min_size(r.min + vec2(0.0, 4.0), vec2(16.0, 16.0)), Icon::Clock, t.icon);
            ui.painter().text(pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &h.label, t.font(12.5), t.text_label);
            if resp.clicked() {
                let _ = app.run("history.restore", json!({"index": i}));
            }
            resp.context_menu(|ui| {
                if ui.button(crate::i18n::tr("Copy History Step Settings to Before")).clicked() {
                    let _ = app.run("beforeAfter.setBefore", json!({"source": "history", "index": i}));
                }
                if ui.button(crate::i18n::tr("Create Version from Step")).clicked() {
                    let _ = app.run("history.restore", json!({"index": i}));
                    let _ = app.run("version.create", json!({"name": h.label}));
                }
                ui.separator();
                if ui.button(crate::i18n::tr("Clear History")).clicked() {
                    let _ = app.run("history.clear", json!({}));
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_times() {
        assert_eq!(super::short_time("2026-09-30T12:00:05"), "Sep 30, 2026, 12:00 PM");
        assert_eq!(super::short_time("2026-01-02"), "Jan 2, 2026");
    }
}
