//! The profile browser (the profile menu's "Browse…", or the grid button next to it): every
//! profile as a live thumbnail of the current photo, grouped (Favorites first), with a favourite
//! star, the applied profile's amount, a temporary loupe preview while hovering and apply on click.

use egui::{Align2, Color32, Rect, Sense, Stroke, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::DevelopSettings;
use lightcraft_engine::presets::{PROFILES, ProfileInfo, profile, profile_groups};
use serde_json::json;

use super::presets::cover_uv;
use crate::icons::{Icon, paint};
use crate::state::RightPanel;
use crate::theme::Tokens;
use crate::widgets::{divider, icon_button, register};
use crate::{HoverPreview, LightcraftApp};

/// Long edge of the variant thumbnails (px).
const THUMB_EDGE: usize = 256;
/// Thumbnail columns.
const COLUMNS: usize = 2;

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    let t = Tokens::get(ui.ctx());
    let d = (*app.session.develop_of(id).unwrap_or_default()).clone();
    // header: back + title
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    let mut back = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(hr.min + vec2(12.0, 9.0), vec2(28.0, 28.0))));
    if icon_button(&mut back, "profilesBack", Icon::ChevronLeft, vec2(28.0, 28.0), false, true, "Back to Edit").clicked() {
        app.ui.right = RightPanel::Edit;
    }
    ui.painter().text(pos2(hr.left() + 46.0, hr.center().y + 2.0), Align2::LEFT_CENTER, crate::i18n::tr("Profiles"), t.semibold(15.0), t.text);
    divider(ui);
    // the applied profile and its amount
    let (cr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let name = profile(&d.profile.id).map(|p| p.name).unwrap_or("Color");
    ui.painter().text(pos2(cr.left() + 24.0, cr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Profile"), t.font(13.0), t.text_dim);
    ui.painter().text(pos2(cr.left() + 84.0, cr.center().y), Align2::LEFT_CENTER, name, t.font(14.0), t.text_label);
    if d.profile.id != "lc.color" {
        super::edit::control(app, ui, &d, "profile.amount", true);
        ui.add_space(6.0);
    }
    divider(ui);
    let mut sections: Vec<(&str, &str, Vec<&'static ProfileInfo>)> = Vec::new();
    let favs: Vec<&'static ProfileInfo> = app.session.profile_favorites.iter().filter_map(|f| profile(f)).collect();
    if !favs.is_empty() {
        sections.push(("Favorites", "fav", favs));
    }
    for g in profile_groups() {
        sections.push((g, "", PROFILES.iter().filter(|p| p.group == g).collect()));
    }
    for (title, tag, list) in sections {
        let open_id = egui::Id::new(("profile-group", title));
        let open: bool = ui.data(|m| m.get_temp(open_id)).unwrap_or(true);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
        register(ui.ctx(), format!("profileGroup:{title}"), r);
        paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.left() + 22.0, r.center().y), vec2(12.0, 12.0)),
            if open { Icon::ChevronDown } else { Icon::ChevronRight },
            t.text_label,
        );
        ui.painter().text(pos2(r.left() + 36.0, r.center().y), Align2::LEFT_CENTER, title, t.semibold(13.0), t.text_label);
        ui.painter().text(pos2(r.right() - 22.0, r.center().y), Align2::RIGHT_CENTER, list.len().to_string(), t.font(12.0), t.text_dim);
        if resp.clicked() {
            ui.data_mut(|m| m.insert_temp(open_id, !open));
        }
        if !open {
            continue;
        }
        let gap = 8.0;
        let cw = ((ui.available_width() - 44.0 - gap * (COLUMNS as f32 - 1.0)) / COLUMNS as f32).max(40.0);
        let th = (cw * 0.75).round();
        for row in list.chunks(COLUMNS) {
            let (rr, _) = ui.allocate_exact_size(vec2(ui.available_width(), th + 30.0), Sense::hover());
            for (i, p) in row.iter().enumerate() {
                let cell = Rect::from_min_size(pos2(rr.left() + 22.0 + i as f32 * (cw + gap), rr.top()), vec2(cw, th + 24.0));
                cell_ui(app, ui, &d, id, tag, p, cell, th);
            }
        }
        ui.add_space(6.0);
    }
    ui.add_space(30.0);
    let last_hover_id = egui::Id::new("last-profile-hover");
    let pointer_in_profiles = ui.ctx().input(|i| i.pointer.hover_pos()).is_some_and(|pos| ui.max_rect().contains(pos));
    if pointer_in_profiles {
        if app.hover_preview.is_none()
            && let Some((saved_id, saved_profile_id)) = ui.data(|m| m.get_temp::<(PhotoId, String)>(last_hover_id))
            && saved_id == id
            && let Some(p) = profile(&saved_profile_id)
        {
            let s = with_profile(&d, p);
            app.hover_preview = Some(HoverPreview { label: crate::i18n::tr_format!("Profile: {}", p.name), settings: s });
        }
    } else {
        ui.data_mut(|m| m.remove::<(PhotoId, String)>(last_hover_id));
    }
}

/// `d` with profile `p` (at 100 % unless it is the applied one).
fn with_profile(d: &DevelopSettings, p: &ProfileInfo) -> DevelopSettings {
    let mut s = d.clone();
    if s.profile.id != p.id {
        s.profile.id = p.id.to_string();
        s.profile.amount = 100.0;
    }
    s
}

#[allow(clippy::too_many_arguments)]
fn cell_ui(app: &mut LightcraftApp, ui: &mut egui::Ui, d: &DevelopSettings, id: PhotoId, tag: &str, p: &'static ProfileInfo, cell: Rect, th: f32) {
    let t = Tokens::get(ui.ctx());
    let key = if tag.is_empty() { format!("profileCell:{}", p.id) } else { format!("profileCell:{tag}:{}", p.id) };
    let resp = ui.interact(cell, egui::Id::new(&key), Sense::click());
    register(ui.ctx(), key, cell);
    let thumb = Rect::from_min_size(cell.min, vec2(cell.width(), th));
    let s = with_profile(d, p);
    let painter = ui.painter();
    painter.rect_filled(thumb, 3.0, t.canvas);
    if let Some(job) = app.session.variant_job(id, &s, THUMB_EDGE)
        && let Some(tex) = app.renderer.variant(job)
    {
        painter.image(tex.tex.id(), thumb, cover_uv(thumb, tex.size), Color32::WHITE);
    }
    let applied = d.profile.id == p.id;
    if applied {
        painter.rect_stroke(thumb.expand(1.0), 4.0, Stroke::new(2.0, t.pick), egui::StrokeKind::Outside);
    } else if resp.hovered() {
        painter.rect_stroke(thumb, 3.0, Stroke::new(1.0, t.text_dim), egui::StrokeKind::Outside);
    }
    let name_color = if applied { t.text } else { t.text_label };
    painter.text(pos2(cell.left() + 2.0, thumb.bottom() + 12.0), Align2::LEFT_CENTER, p.name, t.font(12.0), name_color);
    // favourite star (always shown when set, on hover otherwise)
    let fav = app.session.profile_favorites.iter().any(|f| f == p.id);
    let star = Rect::from_center_size(pos2(thumb.right() - 13.0, thumb.top() + 13.0), vec2(20.0, 20.0));
    register(ui.ctx(), format!("profileStar:{}{}", if tag.is_empty() { "" } else { "fav:" }, p.id), star);
    let over_star = resp.hover_pos().is_some_and(|q| star.contains(q));
    if fav || resp.hovered() {
        painter.circle_filled(star.center(), 10.0, Color32::from_black_alpha(110));
        paint(painter, star.shrink(4.0), if fav { Icon::StarFilled } else { Icon::Star }, if fav || over_star { t.star } else { Color32::WHITE });
    }
    if resp.hovered() && !applied && !over_star {
        app.hover_preview = Some(HoverPreview { label: crate::i18n::tr_format!("Profile: {}", p.name), settings: s.clone() });
        ui.data_mut(|m| m.insert_temp(egui::Id::new("last-profile-hover"), (id, p.id.to_string())));
    }
    let resp = resp.on_hover_text(format!("{} ({})", p.name, p.group));
    if resp.clicked() {
        if resp.interact_pointer_pos().is_some_and(|q| star.contains(q)) {
            let _ = app.run("profile.favorite", json!({"id": p.id}));
        } else if !applied {
            let _ = app.run("develop.profile", json!({"id": p.id, "amount": 100}));
            ui.data_mut(|m| m.remove::<(PhotoId, String)>(egui::Id::new("last-profile-hover")));
        }
    }
}
