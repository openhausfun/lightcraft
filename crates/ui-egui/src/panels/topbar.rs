//! The top bar: sidebar toggle, back/forward, search, filter, and the right-hand icons.

use egui::{Align2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let left = if app.integrated_titlebar { 78 } else { 10 };
    egui::Panel::top("top_bar")
        .exact_size(t.top_bar_h)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin { left, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let mut sw = 640.0f32.min(full.width() - 460.0).max(200.0);
            if !app.native_menu {
                // leave room for the in-window menus left of the (centred) search field
                let menus_right = full.left() + 140.0 + crate::menubar::bar_width(ui) + 24.0;
                sw = sw.min(2.0 * (full.center().x - menus_right)).max(200.0);
            }
            let search_left = full.center().x - sw / 2.0;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icon_button(ui, "sidebar", Icon::Sidebar, vec2(30.0, 30.0), app.ui.left_panel, true, "Show/hide My Photos panel").clicked() {
                    let _ = app.run("view.leftPanel", json!({}));
                }
                ui.add_space(8.0);
                if icon_button(ui, "back", Icon::Back, vec2(30.0, 30.0), false, true, "Back").clicked() {
                    let _ = app.run("view.back", json!({}));
                }
                icon_button(ui, "forward", Icon::Forward, vec2(30.0, 30.0), false, false, "Forward");
                if !app.native_menu {
                    // no native menu bar (web, Windows, Linux): menus in the top bar
                    ui.add_space(10.0);
                    let room = search_left - ui.cursor().left() - 16.0;
                    crate::menubar::show_in_window(app, ui, room);
                }
            });
            // search field (centred on the window)
            let sr = Rect::from_center_size(pos2(full.center().x, full.center().y), vec2(sw, 28.0));
            let id = egui::Id::new("search-field");
            if std::mem::take(&mut app.ui.focus_search) {
                ui.memory_mut(|m| m.request_focus(id));
            }
            let focused = ui.memory(|m| m.has_focus(id));
            ui.painter().rect(
                sr,
                4.0,
                if focused { t.canvas } else { t.field },
                Stroke::new(1.0, if focused { t.accent } else { t.field_border }),
                StrokeKind::Inside,
            );
            register(ui.ctx(), "field:search", sr);
            let mut child =
                ui.new_child(egui::UiBuilder::new().max_rect(sr.shrink2(vec2(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
            let empty = app.ui.search.is_empty();
            if empty && !focused {
                let g = child.painter().layout_no_wrap(crate::i18n::tr("Search Photos").into(), t.font(13.5), t.text_dim);
                let w = g.size().x + 24.0;
                let x0 = sr.center().x - w / 2.0;
                paint(child.painter(), Rect::from_min_size(pos2(x0, sr.center().y - 8.0), vec2(16.0, 16.0)), Icon::Search, t.text_dim);
                child.painter().galley(pos2(x0 + 24.0, sr.center().y - g.size().y / 2.0), g, t.text_dim);
            }
            let resp = child.add(
                egui::TextEdit::singleline(&mut app.ui.search)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .desired_width(sr.width() - 20.0)
                    .font(t.font(13.5))
                    .text_color(t.text),
            );
            if resp.changed() {
                let q = app.ui.search.clone();
                let _ = app.run("library.filter", json!({"text": q}));
            }
            // filter icon right of the search field
            let fr = Rect::from_center_size(pos2(sr.right() + 22.0, sr.center().y), vec2(28.0, 28.0));
            let fresp = ui.interact(fr, egui::Id::new("filter-btn"), Sense::click());
            register(ui.ctx(), "icon:filter", fr);
            let filtering = app.session.filter != Default::default() || app.ui.filter_bar;
            paint(
                ui.painter(),
                fr.shrink(6.0),
                Icon::Filter,
                if filtering {
                    t.accent
                } else if fresp.hovered() {
                    t.text
                } else {
                    t.icon
                },
            );
            // badge: how many filters are on, even with the filter bar closed
            let active = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog).len();
            if active > 0 {
                let c = fr.right_top() + vec2(-3.0, 8.0);
                ui.painter().circle_filled(c, 7.0, t.accent);
                ui.painter().text(c, Align2::CENTER_CENTER, active.to_string(), t.semibold(9.5), t.canvas);
            }
            let fresp = fresp.on_hover_text(if active > 0 {
                crate::i18n::tr_format!("Filter bar — {active} active filter{}", if active == 1 { "" } else { "s" }, active = active)
            } else {
                "Filter bar".into()
            });
            if fresp.clicked() {
                let _ = app.run("view.filterBar", json!({}));
            }
            // right icons
            let mut x = full.right() - 18.0;
            // saving is failing: the cloud icon turns into a warning until a save succeeds
            let unsaved = app.session.unsaved().map(|(n, e)| {
                crate::i18n::tr_format!(
                    "{n} change{} saved in memory but not written to disk: {e}\nLightCraft retries automatically; quitting now would lose {}.",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "it" } else { "them" },
                    e = e,
                    n = n
                )
            });
            let cloud_tip = unsaved.as_deref().unwrap_or("Local library — no cloud account needed");
            for (id, icon, tip, cmd) in [
                ("discord", Icon::Chat, "Join the ArtCraft community on Discord", "app.discord"),
                ("cloud", Icon::Cloud, cloud_tip, ""),
                ("help", Icon::Help, "Keyboard shortcuts", "app.shortcuts"),
                ("share", Icon::Share, "Export", "dialog.export"),
                ("bell", Icon::Bell, "Activity", "panel.activity"),
            ] {
                let r = Rect::from_center_size(pos2(x, full.center().y), vec2(28.0, 28.0));
                let resp = ui.interact(r, egui::Id::new(("top", id)), Sense::click()).on_hover_text(tip);
                register(ui.ctx(), format!("icon:{id}"), r);
                let warn = id == "cloud" && unsaved.is_some();
                let colour = if warn {
                    t.caution
                } else if resp.hovered() {
                    t.text
                } else {
                    t.icon
                };
                paint(ui.painter(), r.shrink(5.0), icon, colour);
                if warn {
                    let c = r.right_top() + vec2(-5.0, 6.0);
                    ui.painter().circle_filled(c, 6.0, t.reject);
                    ui.painter().text(c, Align2::CENTER_CENTER, "!", t.semibold(9.5), t.canvas);
                    register(ui.ctx(), "indicator:unsaved", r);
                }
                if resp.clicked() && !cmd.is_empty() {
                    let _ = app.run(cmd, json!({}));
                }
                x -= 40.0;
            }
            let _ = Align2::CENTER_CENTER;
        });
}
