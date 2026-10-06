//! Culling views: **Compare** (the "select" and a "candidate" side by side, with synced zoom and
//! pan) and **Survey** (the selected photos tiled). Rating, flag and label keys act on the active
//! photo only here, and with Auto Advance on they move to the next candidate / photo.

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::{Flag, PhotoId};
use serde_json::{Value, json};

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::render::Slot;
use crate::state::{ViewMode, Zoom};
use crate::theme::Tokens;
use crate::widgets::register;

/// Most photos a survey shows.
pub const SURVEY_MAX: usize = 48;

/// Compare or Survey.
pub fn culling(app: &LightcraftApp) -> bool {
    matches!(app.ui.view, ViewMode::Compare | ViewMode::Survey)
}

/// The (select, candidate) pair, repaired or chosen if needed: the active photo against the next
/// selected photo, else against its neighbour in the view.
pub fn compare_pair(app: &mut LightcraftApp) -> Option<(PhotoId, PhotoId)> {
    let exists = |app: &LightcraftApp, id: PhotoId| app.session.catalog.photo(id).is_some_and(|p| !p.deleted);
    if let Some((a, b)) = app.ui.compare {
        let (a, b) = (PhotoId(a), PhotoId(b));
        if a != b && exists(app, a) && exists(app, b) {
            return Some((a, b));
        }
    }
    let vis = app.session.visible_cloned();
    let sel = app.session.active().or(vis.first().copied())?;
    let cand = app.session.selection.ids.iter().copied().find(|x| *x != sel).or_else(|| {
        let i = vis.iter().position(|x| *x == sel)?;
        vis.get(i + 1).or(i.checked_sub(1).and_then(|j| vis.get(j))).copied()
    })?;
    app.ui.compare = Some((sel.0, cand.0));
    Some((sel, cand))
}

/// Enter Compare with the current selection (the active photo becomes the select).
pub fn enter_compare(app: &mut LightcraftApp) -> Result<Value, String> {
    app.ui.compare = None;
    let (a, b) = compare_pair(app).ok_or("Compare needs at least two photos")?;
    app.ui.view = ViewMode::Compare;
    // the candidate is active: rating/flag keys judge it against the select
    select_pair(app, a, b, b);
    Ok(json!({"select": a.0, "candidate": b.0}))
}

fn select_pair(app: &mut LightcraftApp, a: PhotoId, b: PhotoId, active: PhotoId) {
    app.ui.compare = Some((a.0, b.0));
    let _ = app.session.execute("library.select", &json!({"ids": [a.0, b.0], "active": active.0}));
}

/// Move the candidate `d` photos through the view (skipping the select); it becomes active.
pub fn compare_step(app: &mut LightcraftApp, d: isize) -> Result<Value, String> {
    let (sel, cand) = compare_pair(app).ok_or("nothing to compare")?;
    let vis: Vec<PhotoId> = app.session.visible_cloned().into_iter().filter(|x| *x != sel).collect();
    let Some(i) = vis.iter().position(|x| *x == cand).or(if vis.is_empty() { None } else { Some(0) }) else { return Ok(Value::Null) };
    let j = (i as isize + d).clamp(0, vis.len() as isize - 1) as usize;
    select_pair(app, sel, vis[j], vis[j]);
    Ok(json!({"candidate": vis[j].0}))
}

pub fn swap(app: &mut LightcraftApp) -> Result<Value, String> {
    let (a, b) = compare_pair(app).ok_or("nothing to compare")?;
    let active = app.session.active().unwrap_or(b);
    select_pair(app, b, a, active);
    Ok(Value::Null)
}

/// The candidate becomes the select; the next photo becomes the candidate.
pub fn make_select(app: &mut LightcraftApp) -> Result<Value, String> {
    let (_, b) = compare_pair(app).ok_or("nothing to compare")?;
    let vis = app.session.visible_cloned();
    let i = vis.iter().position(|x| *x == b).unwrap_or(0);
    let next = vis.iter().skip(i + 1).chain(vis.iter().take(i).rev()).copied().find(|x| *x != b).ok_or("no next photo")?;
    select_pair(app, b, next, next);
    Ok(json!({"select": b.0, "candidate": next.0}))
}

/// The photos a survey shows: the selection (in view order), or the active photo alone.
pub fn survey_photos(app: &mut LightcraftApp) -> Vec<PhotoId> {
    let vis = app.session.visible_cloned();
    let sel = &app.session.selection;
    let mut v: Vec<PhotoId> = vis.iter().copied().filter(|x| sel.contains(*x)).collect();
    // selected photos hidden in the view (e.g. inside a collapsed stack) come last
    let hidden: Vec<PhotoId> = sel.ids.iter().copied().filter(|x| !v.contains(x) && app.session.catalog.photo(*x).is_some()).collect();
    v.extend(hidden);
    if v.is_empty() {
        v.extend(app.session.active());
    }
    v.truncate(SURVEY_MAX);
    v
}

/// Move the active photo `d` steps within the survey.
pub fn survey_step(app: &mut LightcraftApp, d: isize) -> Result<Value, String> {
    let v = survey_photos(app);
    if v.is_empty() {
        return Ok(Value::Null);
    }
    let i = app.session.selection.active.and_then(|a| v.iter().position(|x| *x == a)).unwrap_or(0);
    let j = (i as isize + d).clamp(0, v.len() as isize - 1) as usize;
    app.session.selection.active = Some(v[j]);
    Ok(json!({"active": v[j].0}))
}

/// After rating/flagging with Auto Advance: next candidate (Compare), next photo in the survey,
/// else the next photo in the view.
pub fn advance(app: &mut LightcraftApp) {
    let _ = match app.ui.view {
        ViewMode::Compare => compare_step(app, 1),
        ViewMode::Survey => survey_step(app, 1),
        _ => app.session.execute("library.next", &json!({})).map_err(|e| e.to_string()),
    };
}

/// In the culling views, point a photo command at the active photo only (not the whole selection).
pub fn target_active(app: &LightcraftApp, params: &mut Value) {
    if culling(app)
        && params.get("ids").is_none()
        && let Some(a) = app.session.active()
    {
        params["ids"] = json!([a.0]);
    }
}

fn area_and_filmstrip(app: &mut LightcraftApp, ui: &mut egui::Ui) -> Rect {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let film_h = if app.ui.filmstrip { t.film_h } else { 0.0 };
    let canvas = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - film_h));
    app.canvas_rect = Some(canvas);
    if app.ui.filmstrip {
        super::detail::filmstrip(app, ui, Rect::from_min_max(pos2(full.left(), canvas.bottom()), full.max));
    }
    canvas
}

/// Draw one photo fitted into `area` (rendered at its display size into `slot`), with its caption
/// strip below. Returns the image rect and the click/drag response.
fn photo_tile(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, slot: Slot, area: Rect, label: &str, zoom: Zoom) -> (Rect, egui::Response) {
    let t = Tokens::get(ui.ctx());
    let ppp = ui.ctx().pixels_per_point();
    let resp = ui.interact(area, egui::Id::new(("cull-tile", slot_index(slot), id.0)), Sense::click_and_drag());
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return (area, resp) };
    let caption_h = 26.0;
    let img_area = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - caption_h));
    let frame = lightcraft_pipeline::geometry::Frame::with_lens(
        photo.width.max(1) as usize,
        photo.height.max(1) as usize,
        &photo.develop,
        true,
        photo.embedded_lens.as_ref(),
    );
    let aspect = frame.aspect() as f32;
    let native = [photo.width.max(1) as usize, photo.height.max(1) as usize];
    let img = super::detail::fit_rect(img_area, aspect, zoom, native, ppp, app.ui.pan);
    let want = (img.width().max(img.height()).min(img_area.width().max(img_area.height()) * 4.0) * ppp).min(2560.0) as usize;
    let (rw, rh) = if aspect >= 1.0 { (want, (want as f32 / aspect) as usize) } else { ((want as f32 * aspect) as usize, want) };
    if let Some(job) = app.session.render_job(id, rw.max(8), rh.max(8), false, true) {
        app.renderer.request(slot, job, 60);
    }
    if let Some(job) = app.session.thumb_job(id, 256) {
        app.renderer.request(Slot::Thumb(id), job, 30);
    }
    let p = ui.painter_at(img_area);
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    if let Some(tex) = app.renderer.textures.get(&slot).filter(|x| x.photo == id) {
        p.image(tex.tex.id(), img, uv, Color32::WHITE);
    } else if let Some(tex) = app.renderer.textures.get(&Slot::Thumb(id)) {
        p.image(tex.tex.id(), img, uv, Color32::WHITE);
    } else {
        p.rect_filled(img.intersect(img_area), 0.0, Color32::from_gray(38));
    }
    if photo.flag == Flag::Reject {
        p.rect_filled(img, 0.0, Color32::from_black_alpha(110));
    }
    let active = app.session.selection.active == Some(id);
    if active {
        p.rect_stroke(img.intersect(img_area.shrink(1.0)), 0.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
    }
    // caption: label · file name · stars · flag
    // caption right under the image (at the bottom when zoomed in)
    let cap_top = (img.bottom() + 2.0).min(img_area.bottom());
    let cap = Rect::from_min_max(pos2(img.left().max(area.left()), cap_top), pos2(area.right(), cap_top + caption_h));
    let pt = ui.painter();
    let mut x = cap.left() + 4.0;
    if !label.is_empty() {
        let g = pt.layout_no_wrap(label.to_string(), t.semibold(12.0), if active { t.text } else { t.text_label });
        pt.galley(pos2(x, cap.center().y - g.size().y / 2.0), g.clone(), t.text);
        x += g.size().x + 10.0;
    }
    let name = photo.copy_name.as_ref().map(|c| format!("{} ({c})", photo.file_name)).unwrap_or(photo.file_name.clone());
    let g = pt.layout_no_wrap(name, t.font(11.5), t.text_dim);
    let name_w = g.size().x;
    if x + name_w + 110.0 < cap.right() {
        pt.galley(pos2(x, cap.center().y - g.size().y / 2.0), g, t.text_dim);
        x += name_w + 10.0;
    }
    for i in 0..5u8 {
        let filled = i < photo.rating;
        let r = Rect::from_min_size(pos2(x + i as f32 * 13.0, cap.center().y - 6.0), vec2(12.0, 12.0));
        if r.right() < cap.right() {
            paint(pt, r, if filled { Icon::StarFilled } else { Icon::Star }, if filled { t.star } else { t.icon.gamma_multiply(0.6) });
        }
    }
    x += 5.0 * 13.0 + 4.0;
    let fr = Rect::from_min_size(pos2(x, cap.center().y - 7.0), vec2(14.0, 14.0));
    match photo.flag {
        Flag::Pick if fr.right() < cap.right() => paint(pt, fr, Icon::FlagPick, t.pick),
        Flag::Reject if fr.right() < cap.right() => paint(pt, fr, Icon::FlagReject, t.reject),
        _ => {}
    }
    register(ui.ctx(), format!("cull:{}", id.0), img);
    (img, resp)
}

fn slot_index(s: Slot) -> u8 {
    match s {
        Slot::Compare(i) => i,
        _ => 255,
    }
}

/// Drag-to-pan when zoomed in (shared by both compare panes: zoom and pan are synced).
fn pan(app: &mut LightcraftApp, resp: &egui::Response, img: Rect) {
    if app.ui.zoom != Zoom::Fit && resp.dragged() {
        let d = resp.drag_delta();
        let (px, py) = app.ui.pan;
        app.ui.pan = ((px - d.x / img.width().max(1.0)).clamp(0.0, 1.0), (py - d.y / img.height().max(1.0)).clamp(0.0, 1.0));
    }
}

pub fn show_compare(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let canvas = area_and_filmstrip(app, ui);
    let Some((sel, cand)) = compare_pair(app) else {
        super::empty_message(ui, canvas, "Nothing to compare", "Select two photos, then choose View → Compare (Shift+C)");
        return;
    };
    let area = canvas.shrink(18.0);
    let half = (area.width() - 16.0) / 2.0;
    let panes = [
        (sel, Rect::from_min_size(area.min, vec2(half, area.height())), "Select", 0u8),
        (cand, Rect::from_min_size(pos2(area.right() - half, area.top()), vec2(half, area.height())), "Candidate", 1u8),
    ];
    let zoom = app.ui.zoom;
    for (id, r, label, i) in panes {
        let (img, resp) = photo_tile(app, ui, id, Slot::Compare(i), r, label, zoom);
        if i == 1 {
            app.image_rect = Some(img);
        }
        pan(app, &resp, img);
        if resp.clicked() {
            select_pair(app, sel, cand, id);
        }
        if resp.double_clicked() {
            app.ui.zoom = if app.ui.zoom == Zoom::Fit { Zoom::Percent(100) } else { Zoom::Fit };
        }
        resp.context_menu(|ui| {
            if ui.button(crate::i18n::tr("Swap")).clicked() {
                let _ = swap(app);
            }
            if ui.button(crate::i18n::tr("Make Candidate the Select")).clicked() {
                let _ = make_select(app);
            }
            ui.separator();
            super::grid::context_menu(app, ui, id);
        });
    }
    let mid = area.center().x;
    ui.painter().line_segment([pos2(mid, area.top()), pos2(mid, area.bottom())], Stroke::new(1.0, Tokens::get(ui.ctx()).divider));
}

/// Columns for `n` tiles of aspect ~3:2 in `area`, maximizing the tile size.
pub fn survey_columns(n: usize, area: Rect) -> usize {
    let mut best = (1, 0.0f32);
    for cols in 1..=n.max(1) {
        let rows = n.div_ceil(cols);
        let w = area.width() / cols as f32;
        let h = area.height() / rows as f32;
        let s = w.min(h * 1.5);
        if s > best.1 {
            best = (cols, s);
        }
    }
    best.0
}

pub fn show_survey(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let canvas = area_and_filmstrip(app, ui);
    let photos = survey_photos(app);
    if photos.is_empty() {
        super::empty_message(ui, canvas, "Nothing to survey", "Select photos, then choose View → Survey (N)");
        return;
    }
    let area = canvas.shrink(14.0);
    let cols = survey_columns(photos.len(), area);
    let rows = photos.len().div_ceil(cols);
    let (cw, ch) = (area.width() / cols as f32, area.height() / rows as f32);
    for (i, id) in photos.iter().enumerate() {
        let (c, r) = (i % cols, i / cols);
        let cell = Rect::from_min_size(pos2(area.left() + c as f32 * cw, area.top() + r as f32 * ch), vec2(cw, ch)).shrink(6.0);
        let (img, resp) = photo_tile(app, ui, *id, Slot::Compare(i as u8), cell, "", Zoom::Fit);
        if app.session.selection.active == Some(*id) {
            app.image_rect = Some(img);
        }
        if resp.clicked() {
            app.session.selection.active = Some(*id);
        }
        if resp.double_clicked() {
            let _ = app.run("library.select", json!({"ids": [id.0]}));
            app.ui.view = ViewMode::Detail;
        }
        // remove from the survey (deselect) on hover
        if resp.hovered() && photos.len() > 1 {
            let xr = Rect::from_min_size(pos2(img.right() - 26.0, img.top() + 6.0), vec2(20.0, 20.0));
            let xresp = ui.interact(xr, egui::Id::new(("survey-x", id.0)), Sense::click()).on_hover_text(crate::i18n::tr("Remove from survey"));
            ui.painter().circle_filled(xr.center(), 10.0, Color32::from_black_alpha(if xresp.hovered() { 230 } else { 160 }));
            paint(ui.painter(), xr.shrink(4.0), Icon::Close, t.text);
            if xresp.clicked() {
                let _ = app.run("library.select", json!({"ids": [id.0], "mode": "toggle"}));
            }
        }
        resp.context_menu(|ui| super::grid::context_menu(app, ui, *id));
    }
    let n = photos.len();
    let msg = if app.session.selection.ids.len() > SURVEY_MAX {
        crate::i18n::tr_format!("Showing {n} of {}", app.session.selection.ids.len(), n = n)
    } else {
        String::new()
    };
    if !msg.is_empty() {
        ui.painter().text(pos2(canvas.right() - 16.0, canvas.top() + 10.0), Align2::RIGHT_TOP, msg, t.font(12.0), t.text_dim);
    }
}

/// Reference view: the reference photo (left, fixed) beside the active photo (right) — the one
/// the Edit panel works on, so a look can be matched by eye.
pub fn show_reference(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let canvas = area_and_filmstrip(app, ui);
    let reference = app.ui.reference.map(PhotoId).filter(|r| app.session.catalog.photo(*r).is_some());
    let (Some(r), Some(active)) = (reference, app.session.active()) else {
        super::empty_message(ui, canvas, "No reference photo", "Right-click a photo ▸ Set as Reference Photo, then View → Reference View (Shift+R)");
        return;
    };
    let area = canvas.shrink(18.0);
    let half = (area.width() - 16.0) / 2.0;
    let zoom = app.ui.zoom;
    let left = Rect::from_min_size(area.min, vec2(half, area.height()));
    let right = Rect::from_min_size(pos2(area.right() - half, area.top()), vec2(half, area.height()));
    let (_, lresp) = photo_tile(app, ui, r, Slot::Compare(0), left, "Reference", zoom);
    let (img, resp) = photo_tile(app, ui, active, Slot::Compare(1), right, "Active", zoom);
    app.image_rect = Some(img);
    pan(app, &resp, img);
    lresp.context_menu(|ui| {
        if ui.button(crate::i18n::tr("Clear Reference")).clicked() {
            app.ui.reference = None;
            app.ui.view = ViewMode::Detail;
        }
    });
    let mid = area.center().x;
    ui.painter().line_segment([pos2(mid, area.top()), pos2(mid, area.bottom())], Stroke::new(1.0, Tokens::get(ui.ctx()).divider));
}
