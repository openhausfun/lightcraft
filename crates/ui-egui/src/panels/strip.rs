//! The vertical tool strip at the far right (Presets, Edit, Crop, Remove, Masking, Red Eye …).

use egui::vec2;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::Icon;
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::icon_button;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::right("tool_strip")
        .exact_size(t.strip_w)
        .resizable(false)
        .frame(
            egui::Frame::NONE
                .fill(t.chrome)
                .inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 8 })
                .stroke(egui::Stroke::new(1.0, t.divider)),
        )
        .show(ui, |ui| {
            let has_photo = app.session.active().is_some();
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                let sz = vec2(t.strip_w, 40.0);
                if icon_button(ui, "presets", Icon::Presets, sz, app.ui.presets, has_photo, "Presets (Shift+P)").clicked() {
                    let _ = app.run("panel.presets", json!({}));
                }
                for (id, icon, panel, tip) in [
                    ("edit", Icon::Sliders, RightPanel::Edit, "Edit (E)"),
                    ("crop", Icon::Crop, RightPanel::Crop, "Crop & Rotate (C)"),
                    ("remove", Icon::Eraser, RightPanel::Remove, "Remove (H)"),
                    ("masking", Icon::Mask, RightPanel::Masking, "Masking (M)"),
                    ("redeye", Icon::Eye, RightPanel::RedEye, "Red Eye"),
                ] {
                    let on = app.ui.right == panel || (panel == RightPanel::Edit && app.ui.right == RightPanel::Profiles);
                    if icon_button(ui, id, icon, sz, on, has_photo, tip).clicked() {
                        let _ = app.run(&format!("panel.{id}"), json!({}));
                    }
                    if id == "edit" {
                        separator(ui, &t);
                    }
                }
                separator(ui, &t);
                if icon_button(ui, "versions", Icon::Versions, sz, app.ui.right == RightPanel::Versions, has_photo, "Versions (Shift+V)").clicked() {
                    let _ = app.run("panel.versions", json!({}));
                }
                if icon_button(ui, "activity", Icon::Activity, sz, app.ui.right == RightPanel::Activity, true, "History & Activity (Y)").clicked() {
                    let _ = app.run("panel.activity", json!({}));
                }
                if icon_button(ui, "more", Icon::More, sz, false, true, "More").clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::About);
                }
            });
            // bottom icons
            let bottom = ui.max_rect().bottom();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(egui::pos2(ui.max_rect().left(), bottom - 96.0), ui.max_rect().right_bottom())),
            );
            child.vertical_centered(|ui| {
                let sz = vec2(t.strip_w, 40.0);
                if icon_button(ui, "keywords", Icon::Tag, sz, app.ui.right == RightPanel::Keywords, has_photo, "Keywords (K)").clicked() {
                    let _ = app.run("panel.keywords", json!({}));
                }
                if icon_button(ui, "info", Icon::Info, sz, app.ui.right == RightPanel::Info, has_photo, "Info (I)").clicked() {
                    let _ = app.run("panel.info", json!({}));
                }
            });
        });
}

fn separator(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(vec2(t.strip_w - 16.0, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.button_border.gamma_multiply(0.7));
}
