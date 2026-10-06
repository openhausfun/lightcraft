//! The Detail (loupe) view: the developed photo, before/after, zoom & pan, the filmstrip, and the
//! on-canvas tools (crop, brush/gradient masks, remove spots, white-balance picker).

use egui::{Align2, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::PhotoId;
use lightcraft_develop::{DevelopSettings, MaskShape};
use lightcraft_geom::{Affine, Point};
use lightcraft_pipeline::geometry::Frame;
use serde_json::json;

use crate::LightcraftApp;
use crate::render::Slot;
use crate::state::{BeforeAfter, RightPanel, Zoom};
use crate::theme::Tokens;
use crate::widgets::register;

/// An in-progress on-canvas gesture.
#[derive(Clone, Debug)]
pub enum Gesture {
    Brush {
        points: Vec<Point>,
    },
    Spot {
        points: Vec<Point>,
    },
    /// Remove tool: dragging spot `spot`'s target (or its source).
    SpotMove {
        spot: usize,
        source: bool,
    },
    CropHandle {
        handle: u8,
        start: lightcraft_geom::Rect,
        angle: f64,
    },
    /// Straighten tool: a line drawn along something that should be level (or plumb).
    StraightenLine {
        a: Pos2,
    },
    CropRotate {
        start_angle: f64,
        a0: f32,
    },
    Pan {
        start: (f32, f32),
        at: Pos2,
    },
    /// Dragging component `comp` of `mask`: its pin (`handle` 0) or a linear gradient's start (1)
    /// or end (2).
    MaskHandle {
        mask: u32,
        comp: usize,
        handle: u8,
    },
    /// Guided Upright: a guide being drawn from `a` (normalized transformed coordinates).
    Guide {
        a: Point,
    },
    /// Red Eye: an ellipse being dragged from corner `a` (normalized).
    Eye {
        a: Point,
    },
    /// Targeted adjustment drag from `at` (normalized); `acc` = vertical travel not yet applied.
    Targeted {
        at: Point,
        acc: f32,
    },
}

/// Screen ↔ normalized-image mapping for the displayed frame.
#[derive(Clone, Copy)]
pub struct CanvasMap {
    rect: Rect,
    to_norm: Affine,
    from_norm: Affine,
    k: f32,
}

impl CanvasMap {
    fn new(frame: &Frame, rect: Rect) -> CanvasMap {
        let k = 8.0f32;
        let (w, h) = ((rect.width() * k).round().max(1.0) as usize, (rect.height() * k).round().max(1.0) as usize);
        let to_norm = frame.out_to_norm(w, h);
        CanvasMap { rect, to_norm, from_norm: to_norm.inverse().unwrap_or(Affine::IDENTITY), k }
    }
    pub fn norm(&self, p: Pos2) -> Point {
        self.to_norm.apply(Point::new(((p.x - self.rect.left()) * self.k) as f64, ((p.y - self.rect.top()) * self.k) as f64))
    }
    pub fn screen(&self, n: Point) -> Pos2 {
        let q = self.from_norm.apply(n);
        pos2(self.rect.left() + q.x as f32 / self.k, self.rect.top() + q.y as f32 / self.k)
    }
}

pub(crate) fn fit_rect(area: Rect, aspect: f32, zoom: Zoom, img_px: [usize; 2], ppp: f32, pan: (f32, f32)) -> Rect {
    let (aw, ah) = (area.width(), area.height());
    let (w, h) = match zoom {
        Zoom::Fit => {
            if aw / ah > aspect {
                (ah * aspect, ah)
            } else {
                (aw, aw / aspect)
            }
        }
        Zoom::Fill => {
            if aw / ah > aspect {
                (aw, aw / aspect)
            } else {
                (ah * aspect, ah)
            }
        }
        Zoom::Percent(p) => {
            // full-resolution pixels at p% (the photo's native width)
            let w = img_px[0] as f32 * p as f32 / 100.0 / ppp;
            (w, w / aspect)
        }
    };
    let c = if w <= aw && h <= ah {
        area.center()
    } else {
        // pan: which normalized point of the image sits at the area centre
        pos2(area.center().x - (pan.0 - 0.5) * w, area.center().y - (pan.1 - 0.5) * h)
    };
    Rect::from_center_size(c, vec2(w, h))
}

/// Ease the loupe rect toward `target` while a click-zoom animation runs; otherwise follow it exactly.
fn animated_rect(ctx: &egui::Context, anim: &mut bool, target: Rect) -> Rect {
    let t = if *anim { 0.22 } else { 0.0 };
    let id = egui::Id::new("loupe_anim");
    let v = |k: &str, x: f32| ctx.animate_value_with_time(id.with(k), x, t);
    let (c, s) = (target.center(), target.size());
    let r = Rect::from_center_size(pos2(v("cx", c.x), v("cy", c.y)), vec2(v("w", s.x), v("h", s.y)));
    if *anim && (r.center() - c).abs().max_elem() < 0.5 && (r.size() - s).abs().max_elem() < 0.5 {
        *anim = false;
    }
    r
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let full = ui.max_rect();
    let fullscreen = app.ui.fullscreen;
    let show_film = app.ui.filmstrip && !fullscreen;
    let film_h = if show_film { t.film_h } else { 0.0 };
    let canvas = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - film_h));
    app.canvas_rect = Some(canvas);
    if show_film {
        filmstrip(app, ui, Rect::from_min_max(pos2(full.left(), canvas.bottom()), full.max));
    }
    let Some(id) = app.session.active() else {
        super::empty_message(ui, canvas, "No photo selected", "Choose a photo in the grid or filmstrip");
        return;
    };
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return };
    let d = (*photo.develop).clone();
    // the full-screen preview shows the photo only: no tool overlays
    let right = if fullscreen { RightPanel::None } else { app.ui.right };
    let crop_tool = right == RightPanel::Crop;
    let frame = Frame::with_lens(photo.width.max(1) as usize, photo.height.max(1) as usize, &d, !crop_tool, photo.embedded_lens.as_ref());
    let aspect = frame.aspect() as f32;
    let ppp = ui.ctx().pixels_per_point();
    let area = canvas.shrink(if fullscreen {
        0.0
    } else if crop_tool {
        48.0
    } else {
        24.0
    });
    let max_edge = app.ui.settings.preview_edge.clamp(512, 8192) as f32;
    let native = [photo.width.max(1) as usize, photo.height.max(1) as usize];
    // two views (before, after): side by side or stacked
    let split = matches!(app.ui.before_after, BeforeAfter::SideBySide | BeforeAfter::TopBottom);
    let split_view = matches!(app.ui.before_after, BeforeAfter::Split | BeforeAfter::SplitTopBottom);
    let areas: Vec<Rect> = match app.ui.before_after {
        BeforeAfter::SideBySide => {
            let half = area.width() / 2.0 - 6.0;
            vec![
                Rect::from_min_size(area.min, vec2(half, area.height())),
                Rect::from_min_size(pos2(area.center().x + 6.0, area.top()), vec2(half, area.height())),
            ]
        }
        BeforeAfter::TopBottom => {
            let half = area.height() / 2.0 - 14.0;
            vec![
                Rect::from_min_size(area.min, vec2(area.width(), half)),
                Rect::from_min_size(pos2(area.left(), area.center().y + 14.0), vec2(area.width(), half)),
            ]
        }
        _ => vec![area],
    };
    let main_area = *areas.last().unwrap_or(&area);
    let target_rect = fit_rect(main_area, aspect, app.ui.zoom, native, ppp, app.ui.pan);
    let img_rect = animated_rect(ui.ctx(), &mut app.ui.zoom_anim, target_rect);
    app.image_rect = Some(img_rect);
    // request renders: the loupe at display resolution (drafts during drags)
    let interacting = app.session.interaction.is_some();
    let scale = if interacting { 0.6 } else { 1.0 };
    // render at the final size: a click-zoom animation only changes how the result is drawn
    let want = (target_rect.width().max(target_rect.height()) * ppp * scale).min(max_edge) as usize;
    let (rw, rh) = if aspect >= 1.0 { (want, (want as f32 / aspect) as usize) } else { ((want as f32 * aspect) as usize, want) };
    if let Some(job) = app.session.loupe_job(id, rw.max(8), rh.max(8), !crop_tool) {
        let job = if interacting { job.draft() } else { job };
        let job = job.with_overlay(view_overlay(app, &d)).with_proof(app.ui.soft_proof.then_some(app.ui.proof));
        app.renderer.request(Slot::Main, job, 100);
    }
    // hovering a preset or profile: the photo with that look, shown instead of the loupe render
    // once it is ready (nothing is committed)
    let hover_key = match app.hover_preview.clone() {
        Some(h) if !interacting => app.session.preview_job(id, rw.max(8), rh.max(8), !crop_tool, &h.settings).map(|job| {
            let key = job.key;
            app.renderer.request(Slot::Hover, job, 105);
            (key, h.label)
        }),
        _ => None,
    };
    // once this photo is on screen: prepare its neighbours in filmstrip order (source decoded and
    // kept, view render cached) so stepping to them is instant
    if !interacting && !app.renderer.is_pending(Slot::Main) && app.renderer.textures.get(&Slot::Main).is_some_and(|t| t.photo == id) {
        let ids = app.session.visible_cloned();
        if let Some(i) = ids.iter().position(|p| *p == id) {
            let next = ids.get(i + 1).copied();
            let prev = i.checked_sub(1).and_then(|j| ids.get(j)).copied();
            for (n, nid) in [next, prev].into_iter().enumerate() {
                let Some(nid) = nid else { continue };
                let Some(np) = app.session.catalog.photo(nid).cloned() else { continue };
                let nf = Frame::with_lens(np.width.max(1) as usize, np.height.max(1) as usize, &np.develop, !crop_tool, np.embedded_lens.as_ref());
                let na = nf.aspect() as f32;
                let nr = fit_rect(main_area, na, app.ui.zoom, [np.width.max(1) as usize, np.height.max(1) as usize], ppp, app.ui.pan);
                let nw = (nr.width().max(nr.height()) * ppp).min(max_edge) as usize;
                let (w, h) = if na >= 1.0 { (nw, (nw as f32 / na) as usize) } else { ((nw as f32 * na) as usize, nw) };
                if let Some(job) = app.session.loupe_job(nid, w.max(8), h.max(8), !crop_tool) {
                    app.renderer.prefetch(Slot::Prefetch(n as u8), job, PREFETCH_PRIORITY);
                }
            }
        }
    }
    // until the loupe has this photo: show its cached render / embedded preview / a thumbnail
    if app.renderer.textures.get(&Slot::Main).is_none_or(|t| t.photo != id)
        && let Some(q) = app.session.quick_view_job(id, want.max(8), !crop_tool)
    {
        app.renderer.request_quick(Slot::Preview, q, 110);
    }
    let show_before = app.ui.before_after == BeforeAfter::Original || ui.input(|i| i.key_down(egui::Key::Backslash));
    if (split || show_before || split_view)
        && let Some(job) = app.session.render_job(id, rw.max(8), rh.max(8), true, !crop_tool)
    {
        app.renderer.request(Slot::Before, job, 90);
    }
    let p = ui.painter_at(canvas);
    // what a view slot shows: its own render of this photo, else the stand-ins (no blank frame
    // between photos, and the full render replaces them in place)
    let draw = |slot: Slot, r: Rect| -> &'static str {
        let mine = |s: Slot| app.renderer.textures.get(&s).filter(|t| t.photo == id);
        let (tex, what) = if let Some(t) = mine(slot) {
            (t, "render")
        } else if let Some(t) = mine(Slot::Preview) {
            (t, t.quick.map(quick_name).unwrap_or("preview"))
        } else if let Some(t) = app.renderer.thumb(id) {
            (t, "thumb")
        } else {
            return "none";
        };
        p.image(tex.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        what
    };
    // soft proofing: a paper-white surround and the proof's name, as Lightroom shows it
    if app.ui.soft_proof && !fullscreen {
        p.rect_filled(canvas, 0.0, Color32::from_gray(238));
        let label = crate::i18n::tr_format!("Proof Preview · {}", app.ui.proof.space.label());
        p.text(pos2(canvas.right() - 16.0, canvas.top() + 14.0), Align2::RIGHT_CENTER, label, t.font(12.5), Color32::from_gray(60));
    }
    let shown;
    if split {
        let br = fit_rect(areas[0], aspect, app.ui.zoom, native, ppp, app.ui.pan);
        draw(Slot::Before, br);
        shown = draw(Slot::Main, img_rect);
        p.text(pos2(br.left(), br.bottom() + 14.0), Align2::LEFT_CENTER, crate::i18n::tr("Before"), t.font(12.0), t.text_dim);
        p.text(pos2(img_rect.left(), img_rect.bottom() + 14.0), Align2::LEFT_CENTER, crate::i18n::tr("After"), t.font(12.0), t.text_dim);
    } else if show_before {
        shown = draw(Slot::Before, img_rect);
        p.text(pos2(img_rect.left() + 8.0, img_rect.top() + 14.0), Align2::LEFT_CENTER, crate::i18n::tr("Before"), t.font(12.0), t.text);
    } else if let Some((_key, label)) = &hover_key
        && let Some(tex) = app.renderer.textures.get(&Slot::Hover).filter(|t| t.photo == id)
    {
        p.image(tex.tex.id(), img_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        let g = p.layout_no_wrap(label.clone(), t.font(12.0), Color32::WHITE);
        let bg = Rect::from_min_size(img_rect.left_top() + vec2(8.0, 8.0), g.size() + vec2(16.0, 8.0));
        p.rect_filled(bg, 4.0, Color32::from_black_alpha(160));
        p.galley(bg.min + vec2(8.0, 4.0), g, Color32::WHITE);
        shown = "hover";
    } else {
        shown = draw(Slot::Main, img_rect);
        if shown == "none" {
            match app.renderer.failure(Slot::Main) {
                Some(e) => {
                    p.text(
                        canvas.center() - vec2(0.0, 10.0),
                        Align2::CENTER_CENTER,
                        crate::i18n::tr("This photo can't be opened"),
                        t.semibold(14.0),
                        t.text,
                    );
                    p.text(canvas.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, e, t.font(12.0), t.text_dim);
                }
                None => {
                    p.text(canvas.center(), Align2::CENTER_CENTER, crate::i18n::tr("Rendering…"), t.font(13.0), t.text_dim);
                }
            }
        }
    }
    app.loupe_shown = Some((id, shown));
    if app.ui.before_after == BeforeAfter::Split {
        let mid = img_rect.center().x;
        if let Some(tex) = app.renderer.textures.get(&Slot::Before).filter(|t| t.photo == id) {
            let left = Rect::from_min_max(img_rect.min, pos2(mid, img_rect.bottom()));
            p.image(tex.tex.id(), left, Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 1.0)), Color32::WHITE);
        }
        p.line_segment([pos2(mid, img_rect.top()), pos2(mid, img_rect.bottom())], Stroke::new(1.5, Color32::WHITE));
    }
    if app.ui.before_after == BeforeAfter::SplitTopBottom {
        let mid = img_rect.center().y;
        if let Some(tex) = app.renderer.textures.get(&Slot::Before).filter(|t| t.photo == id) {
            let top = Rect::from_min_max(img_rect.min, pos2(img_rect.right(), mid));
            p.image(tex.tex.id(), top, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 0.5)), Color32::WHITE);
        }
        p.line_segment([pos2(img_rect.left(), mid), pos2(img_rect.right(), mid)], Stroke::new(1.5, Color32::WHITE));
    }
    if app.ui.show_clipping && !show_before && !fullscreen {
        clipping_overlay(app, &p, img_rect);
    }
    register(ui.ctx(), "canvas:image", img_rect);
    let map = CanvasMap::new(&frame, img_rect);
    let resp = ui.interact(canvas, egui::Id::new("loupe"), Sense::click_and_drag());
    info_overlay(app, &p, canvas, &photo);
    // a fine grid while a transform (geometry) slider is dragged, to judge verticals
    if app.ui.dragging_control.as_deref().is_some_and(|c| c.starts_with("geometry.")) {
        let n = 12;
        let stroke = Stroke::new(1.0, Color32::from_white_alpha(70));
        for i in 1..n {
            let fx = img_rect.left() + img_rect.width() * i as f32 / n as f32;
            p.line_segment([pos2(fx, img_rect.top()), pos2(fx, img_rect.bottom())], stroke);
        }
        let rows = ((n as f32) * img_rect.height() / img_rect.width()).round().max(2.0) as usize;
        for i in 1..rows {
            let fy = img_rect.top() + img_rect.height() * i as f32 / rows as f32;
            p.line_segment([pos2(img_rect.left(), fy), pos2(img_rect.right(), fy)], stroke);
        }
    }
    match right {
        RightPanel::Crop => crop_overlay(app, ui, &resp, &map, &frame, &d, id),
        RightPanel::Masking => mask_overlay(app, ui, &resp, &map, &d),
        RightPanel::Remove => remove_overlay(app, ui, &resp, &map, &d),
        RightPanel::RedEye => eye_overlay(app, ui, &resp, &map, &d),
        _ => general_interaction(app, ui, &resp, &map, img_rect, canvas, native, aspect),
    }
    // drawn and hit-tested above the loupe and its tools: clicks on it pan
    navigator(app, ui, canvas, img_rect, id);
    if let Some(why) = photo.preview_only.as_deref().filter(|_| !fullscreen) {
        preview_only_pill(ui, canvas, why);
    }
    resp.context_menu(|ui| {
        ui.menu_button(crate::i18n::tr("Zoom"), |ui| {
            for (label, cmd) in [("Fit", "view.zoomFit"), ("100%", "view.zoom100"), ("Zoom In", "view.zoomIn"), ("Zoom Out", "view.zoomOut")] {
                if ui.button(label).clicked() {
                    let _ = app.run(cmd, json!({}));
                }
            }
        });
        ui.separator();
        super::grid::context_menu(app, ui, id);
    });
}

/// The info overlay at the canvas' top left (`view.infoOverlay`): file name with the capture date
/// and size, or with the camera and exposure.
fn info_overlay(app: &LightcraftApp, p: &egui::Painter, canvas: Rect, photo: &lightcraft_catalog::Photo) {
    use crate::state::InfoOverlay;
    let lines: Vec<String> = match app.ui.info_overlay {
        InfoOverlay::Off => return,
        InfoOverlay::Basic => {
            let date = photo.captured.as_deref().unwrap_or(&photo.imported);
            vec![photo.file_name.clone(), format!("{} · {} × {}", pretty_date(date), photo.width, photo.height)]
        }
        InfoOverlay::Exposure => {
            let m = &photo.meta;
            let mut exp = Vec::new();
            if !m.shutter.is_empty() {
                exp.push(format!("{} s", m.shutter));
            }
            if let Some(a) = m.aperture {
                exp.push(format!("f/{a:.1}"));
            }
            if let Some(i) = m.iso {
                exp.push(format!("ISO {i}"));
            }
            if let Some(f) = m.focal_mm {
                exp.push(format!("{f:.0} mm"));
            }
            let camera = [m.camera.as_str(), m.lens.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" · ");
            let mut v = vec![photo.file_name.clone()];
            v.push(if exp.is_empty() { "No exposure information".into() } else { exp.join("  ") });
            if !camera.is_empty() {
                v.push(camera);
            }
            v
        }
    };
    let t = Tokens::get(p.ctx());
    let mut y = canvas.top() + 12.0;
    for (i, l) in lines.iter().enumerate() {
        let font = if i == 0 { t.semibold(15.0) } else { t.font(12.5) };
        let g = p.layout_no_wrap(l.clone(), font, Color32::WHITE);
        let at = pos2(canvas.left() + 14.0, y);
        // a soft shadow keeps the text readable on bright photos
        p.galley(at + vec2(1.0, 1.0), g.clone(), Color32::from_black_alpha(200));
        p.galley(at, g.clone(), Color32::WHITE);
        y += g.size().y + 3.0;
    }
    register(p.ctx(), "canvas:infoOverlay", Rect::from_min_max(canvas.min, pos2(canvas.left() + 320.0, y)));
}

/// A raw shown from its embedded JPEG (issue #10): a pill at the canvas' top centre saying so;
/// hovering it explains (registered as `notice:previewOnly:loupe`).
fn preview_only_pill(ui: &mut egui::Ui, canvas: Rect, reason: &str) {
    let t = Tokens::get(ui.ctx());
    let p = ui.painter_at(canvas);
    let text = crate::i18n::tr_format!("Preview only — editing the camera's embedded JPEG ({})", crate::widgets::preview_only_variant(reason));
    let g = p.layout_no_wrap(text, t.font(12.0), Color32::WHITE);
    let size = vec2(g.size().x + 40.0, 26.0);
    let r = Rect::from_min_size(pos2(canvas.center().x - size.x / 2.0, canvas.top() + 12.0), size);
    p.rect_filled(r, 13.0, Color32::from_black_alpha(185));
    crate::icons::paint(&p, Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(15.0, 15.0)), crate::icons::Icon::Info, t.caution);
    p.galley(pos2(r.left() + 29.0, r.center().y - g.size().y / 2.0), g, Color32::WHITE);
    register(ui.ctx(), "notice:previewOnly:loupe", r);
    ui.interact(r, egui::Id::new("preview-only-pill"), Sense::hover()).on_hover_text(crate::widgets::preview_only_explanation(reason));
}

/// `2026-09-30T12:00:00` → `2026-09-30 12:00`.
fn pretty_date(iso: &str) -> String {
    let d = iso.replacen('T', " ", 1);
    d.get(..16).map(str::to_string).unwrap_or(d)
}

/// Size of the Navigator mini map (points).
const NAV_W: f32 = 180.0;

/// The Navigator: while zoomed in, a mini map of the photo at the canvas' bottom right with the
/// visible region outlined; click or drag on it to pan.
fn navigator(app: &mut LightcraftApp, ui: &mut egui::Ui, canvas: Rect, img: Rect, id: PhotoId) {
    let zoomed = img.width() > canvas.width() + 1.0 || img.height() > canvas.height() + 1.0;
    if !app.ui.navigator || !zoomed || app.ui.fullscreen {
        return;
    }
    let tex = app.renderer.thumb(id).or_else(|| app.renderer.textures.get(&Slot::Main).filter(|t| t.photo == id));
    let aspect = img.width() / img.height().max(1.0);
    let (w, h) = if aspect >= 1.0 { (NAV_W, NAV_W / aspect) } else { (NAV_W * aspect, NAV_W) };
    let frame = Rect::from_min_size(pos2(canvas.right() - w - 16.0, canvas.bottom() - h - 16.0), vec2(w, h));
    let p = ui.painter_at(canvas);
    p.rect_filled(frame.expand(5.0), 4.0, Color32::from_black_alpha(190));
    match tex {
        Some(t) => p.image(t.tex.id(), frame, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE),
        None => p.rect_filled(frame, 0.0, Color32::from_gray(40)),
    };
    // the visible part of the photo, in normalized image coordinates
    let vis = canvas.intersect(img);
    let n = |q: Pos2| pos2((q.x - img.left()) / img.width(), (q.y - img.top()) / img.height());
    let (a, b) = (n(vis.min), n(vis.max));
    let to_nav = |q: Pos2| pos2(frame.left() + q.x * frame.width(), frame.top() + q.y * frame.height());
    let view = Rect::from_min_max(to_nav(a), to_nav(b));
    p.rect_stroke(view, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Inside);
    p.rect_stroke(view.expand(1.5), 0.0, Stroke::new(1.0, Color32::from_black_alpha(160)), StrokeKind::Outside);
    register(ui.ctx(), "canvas:navigator", frame);
    let resp = ui.interact(frame.expand(5.0), egui::Id::new("navigator"), Sense::click_and_drag());
    if (resp.clicked() || resp.dragged())
        && let Some(q) = resp.interact_pointer_pos()
    {
        // centre the view on the point under the pointer
        let u = ((q.x - frame.left()) / frame.width()).clamp(0.0, 1.0);
        let v = ((q.y - frame.top()) / frame.height()).clamp(0.0, 1.0);
        app.ui.pan = (u, v);
    }
}

/// Neighbour prefetch: below on-screen thumbnails, above background thumbnail refreshes.
const PREFETCH_PRIORITY: u32 = 4;

fn quick_name(q: lightcraft_engine::media::QuickSource) -> &'static str {
    use lightcraft_engine::media::QuickSource;
    match q {
        QuickSource::Cached => "cached",
        QuickSource::Embedded => "embedded",
        QuickSource::Small => "small",
    }
}

/// Targeted adjustment tool: dragging up/down on the photo raises/lowers the tone-curve region or
/// the colour-mixer bands under the press point (`develop.targeted`, one call per whole step, all
/// in one interaction = one undo step).
fn targeted_drag(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, target: &str) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    if resp.drag_started()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let _ = app.run("develop.beginInteraction", json!({"label": "Targeted Adjustment"}));
        app.gesture = Some(Gesture::Targeted { at: map.norm(q), acc: 0.0 });
    }
    if resp.dragged()
        && let Some(Gesture::Targeted { at, acc }) = &mut app.gesture
    {
        *acc -= resp.drag_delta().y * 0.5;
        let step = acc.trunc();
        if step != 0.0 {
            *acc -= step;
            let at = *at;
            let mut p = json!({"target": target, "x": at.x, "y": at.y, "delta": step});
            if target == "curve" && app.ui.curve_channel != "parametric" {
                p["channel"] = json!(app.ui.curve_channel);
            }
            let _ = app.run("develop.targeted", p);
        }
    }
    if resp.drag_stopped() && matches!(app.gesture, Some(Gesture::Targeted { .. })) {
        app.gesture = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
}

/// The diagnostic overlay the loupe shows (the selected mask, Point Color's visualized range,
/// Visualize Spots).
pub(crate) fn view_overlay(app: &LightcraftApp, d: &DevelopSettings) -> lightcraft_pipeline::Overlay {
    use lightcraft_pipeline::{MaskView, Overlay};
    if app.ui.fullscreen {
        return Overlay::None;
    }
    if app.ui.right == RightPanel::Masking
        && app.ui.mask_overlay
        && let Some(m) = app.session.active_mask.and_then(|id| d.masks.iter().find(|m| m.id == id))
        && !m.components.is_empty()
    {
        return Overlay::Mask {
            id: m.id.min(u16::MAX as u32) as u16,
            view: MaskView::parse(&app.ui.mask_overlay_mode).unwrap_or_default(),
            color: app.ui.mask_overlay_color,
            opacity: app.ui.mask_overlay_opacity.clamp(0.0, 100.0).round() as u8,
        };
    }
    if app.ui.right == RightPanel::Remove && app.ui.visualize_spots {
        return Overlay::Spots(app.ui.spots_threshold.clamp(0.0, 100.0).round() as u8);
    }
    let edit = app.ui.right == RightPanel::Edit;
    if edit && app.ui.point_color_visualize && app.ui.flyout_open("pointColor") && app.ui.point_color < d.point_colors.len() {
        return Overlay::PointColorRange(app.ui.point_color as u8);
    }
    Overlay::None
}

fn clipping_overlay(app: &LightcraftApp, p: &egui::Painter, r: Rect) {
    // Highlight clipped regions using the histogram's extremes isn't spatial; show a subtle frame hint.
    if let Some(h) = app.renderer.textures.get(&Slot::Main).and_then(|t| t.histogram.as_ref()) {
        let (lo, hi) = h.clipping();
        if hi > 0.003 {
            p.rect_stroke(r, 0.0, Stroke::new(2.0, Color32::from_rgb(255, 60, 60)), StrokeKind::Outside);
        }
        if lo > 0.003 {
            p.rect_stroke(r.expand(3.0), 0.0, Stroke::new(2.0, Color32::from_rgb(60, 120, 255)), StrokeKind::Outside);
        }
    }
}

fn general_interaction(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    resp: &egui::Response,
    map: &CanvasMap,
    img: Rect,
    canvas: Rect,
    native: [usize; 2],
    aspect: f32,
) {
    if app.ui.tool == "wbPicker" {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            let _ = app.run("develop.wbPick", json!({"x": n.x, "y": n.y}));
            app.ui.tool.clear();
        }
        return;
    }
    if let Some(target) = app.ui.tool.strip_prefix("tat:").map(str::to_string) {
        targeted_drag(app, ui, resp, map, &target);
        return;
    }
    if app.ui.tool == "colorRange" {
        // click: sample the colour for the selected mask's colour range; ⇧-click adds (up to 5)
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            let add = ui.input(|i| i.modifiers.shift);
            if let Err(e) = app.run("mask.sampleColor", json!({"x": n.x, "y": n.y, "add": add})) {
                app.toast(ui.ctx(), e);
            }
        }
        return;
    }
    if app.ui.tool == "pointColor" {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if resp.clicked()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let n = map.norm(q);
            if let Ok(r) = app.run("pointColor.pick", json!({"x": n.x, "y": n.y})) {
                app.ui.point_color = r["index"].as_u64().unwrap_or(0) as usize;
            }
            app.ui.tool.clear();
        }
        return;
    }
    // click toggles Fit ↔ the click-zoom ratio (2:1/3:1/5:1) at the clicked point; drag pans when zoomed
    let zoomed = img.width() > canvas.width() + 1.0 || img.height() > canvas.height() + 1.0;
    if resp.double_clicked() || (resp.clicked() && !zoomed) {
        if let Some(q) = resp.interact_pointer_pos() {
            let u = ((q.x - img.left()) / img.width()).clamp(0.0, 1.0);
            let v = ((q.y - img.top()) / img.height()).clamp(0.0, 1.0);
            app.ui.pan = (u, v);
        }
        app.ui.zoom = if matches!(app.ui.zoom, Zoom::Fit) { Zoom::Percent(app.ui.click_zoom) } else { Zoom::Fit };
        app.ui.zoom_anim = true;
    } else if resp.clicked() && zoomed {
        app.ui.zoom = Zoom::Fit;
        app.ui.zoom_anim = true;
    }
    if zoomed {
        // only over the photo: a panel drawn earlier (sliders) must keep its own cursor
        if resp.dragged() || resp.hovered() {
            ui.ctx().set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }
        if resp.dragged() {
            let dlt = resp.drag_delta();
            app.ui.pan.0 = (app.ui.pan.0 - dlt.x / img.width()).clamp(0.0, 1.0);
            app.ui.pan.1 = (app.ui.pan.1 - dlt.y / img.height()).clamp(0.0, 1.0);
        }
    }
    let _ = (native, aspect);
}

// ------------------------------------------------------------------------ crop

fn crop_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, frame: &Frame, d: &DevelopSettings, id: PhotoId) {
    if app.ui.tool == "guidedUpright" {
        guided_overlay(app, ui, resp, map, frame, d);
        return;
    }
    if app.ui.tool == "straighten" {
        straighten_overlay(app, ui, resp, false);
        return;
    }
    // hold ⌘ and drag: draw a straighten line without leaving the crop tool
    let cmd_drag = resp.drag_started() && ui.input(|i| i.modifiers.command);
    if cmd_drag || matches!(app.gesture, Some(Gesture::StraightenLine { .. })) {
        straighten_overlay(app, ui, resp, true);
        return;
    }
    if resp.hovered() && ui.input(|i| i.modifiers.command) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let quad = frame_crop_quad(d, frame);
    let pts: Vec<Pos2> = quad.iter().map(|q| map.screen(*q)).collect();
    let p = ui.painter();
    // darken outside the crop
    let img = map.rect;
    let shade = Color32::from_black_alpha(150);
    let mut mesh = egui::epaint::Mesh::default();
    let outer = [img.left_top(), img.right_top(), img.right_bottom(), img.left_bottom()];
    for i in 0..4 {
        let a = outer[i];
        let b = outer[(i + 1) % 4];
        let c = pts[(i + 1) % 4];
        let dd = pts[i];
        let base = mesh.vertices.len() as u32;
        for v in [a, b, c, dd] {
            mesh.vertices.push(egui::epaint::Vertex { pos: v, uv: Pos2::ZERO, color: shade });
        }
        mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    p.add(mesh);
    p.add(egui::Shape::closed_line(pts.clone(), Stroke::new(1.0, Color32::WHITE)));
    // overlay guides
    let lerp = |a: Pos2, b: Pos2, t: f32| a + (b - a) * t;
    let guide = Stroke::new(0.8, Color32::from_white_alpha(150));
    // (u, v) across / down the (rotated) crop frame
    let at = |(u, v): (f32, f32)| pts[0] + (pts[1] - pts[0]) * u + (pts[3] - pts[0]) * v;
    let (cw, ch) = (pts[0].distance(pts[1]), pts[0].distance(pts[3]));
    for line in super::crop_overlay::paths(app.ui.crop_overlay, app.ui.crop_overlay_orient, cw, ch) {
        p.add(egui::Shape::line(line.into_iter().map(at).collect(), guide));
    }
    // handles: 0..3 corners, 4..7 edges (top, right, bottom, left)
    let handles: Vec<Pos2> = (0..8).map(|i| if i < 4 { pts[i] } else { lerp(pts[i - 4], pts[(i - 3) % 4], 0.5) }).collect();
    for (i, h) in handles.iter().enumerate() {
        register(ui.ctx(), format!("cropHandle:{i}"), Rect::from_center_size(*h, vec2(14.0, 14.0)));
        let s = if i < 4 { 12.0 } else { 9.0 };
        p.rect_filled(Rect::from_center_size(*h, vec2(s, 3.0)), 0.0, Color32::WHITE);
        p.rect_filled(Rect::from_center_size(*h, vec2(3.0, s)), 0.0, Color32::WHITE);
    }
    let inside = |q: Pos2| {
        let n = map.norm(q);
        let s = to_straight(n, d.crop.geometry.angle, frame);
        d.crop.geometry.rect.contains(Point::new(s.x.clamp(-1.0, 2.0), s.y))
    };
    if let Some(hq) = resp.hover_pos() {
        let near = handles.iter().position(|h| h.distance(hq) < 12.0);
        ui.ctx().set_cursor_icon(match near {
            Some(0 | 2) => egui::CursorIcon::ResizeNwSe,
            Some(1 | 3) => egui::CursorIcon::ResizeNeSw,
            Some(4 | 6) => egui::CursorIcon::ResizeVertical,
            Some(_) => egui::CursorIcon::ResizeHorizontal,
            None if inside(hq) => egui::CursorIcon::Move,
            None => egui::CursorIcon::Alias,
        });
    }
    // double-click inside the crop box applies the crop (same as Return / Done)
    if resp.double_clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && inside(q)
        && !handles.iter().any(|h| h.distance(q) < 12.0)
    {
        let _ = app.run("tool.done", json!({}));
        return;
    }
    if resp.drag_started()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let _ = app.run("develop.beginInteraction", json!({"label": "Crop"}));
        app.gesture = Some(match handles.iter().position(|h| h.distance(q) < 12.0) {
            Some(h) => Gesture::CropHandle { handle: h as u8, start: d.crop.geometry.rect, angle: d.crop.geometry.angle },
            None if inside(q) => Gesture::CropHandle { handle: 8, start: d.crop.geometry.rect, angle: d.crop.geometry.angle },
            None => {
                let c = map.screen(Point::new(0.5, 0.5));
                Gesture::CropRotate { start_angle: d.crop.geometry.angle, a0: (q - c).angle() }
            }
        });
    }
    if resp.dragged()
        && let Some(q) = resp.interact_pointer_pos()
    {
        match app.gesture.clone() {
            Some(Gesture::CropHandle { handle, start, angle }) => {
                let n = to_straight(map.norm(q), angle, frame);
                let o = resp.interact_pointer_pos().map(|q0| q0 - resp.drag_delta()).unwrap_or(q);
                let _ = o;
                let mut r = start;
                let orig = ui.input(|i| i.pointer.press_origin()).map(|q0| to_straight(map.norm(q0), angle, frame)).unwrap_or(n);
                let (dx, dy) = (n.x - orig.x, n.y - orig.y);
                match handle {
                    0 => (r.x0, r.y0) = (start.x0 + dx, start.y0 + dy),
                    1 => (r.x1, r.y0) = (start.x1 + dx, start.y0 + dy),
                    2 => (r.x1, r.y1) = (start.x1 + dx, start.y1 + dy),
                    3 => (r.x0, r.y1) = (start.x0 + dx, start.y1 + dy),
                    4 => r.y0 = start.y0 + dy,
                    5 => r.x1 = start.x1 + dx,
                    6 => r.y1 = start.y1 + dy,
                    7 => r.x0 = start.x0 + dx,
                    _ => r = start.translate(lightcraft_geom::Vec2::new(dx, dy)),
                }
                // aspect lock
                if let Some((aw, ah)) = d.crop.aspect
                    && handle < 4
                {
                    let (iw, ih) = (frame.ow, frame.oh);
                    let a = if (iw >= ih) == (aw >= ah) { aw as f64 / ah as f64 } else { ah as f64 / aw as f64 };
                    let w_px = (r.x1 - r.x0).abs() * iw;
                    let h_px = w_px / a;
                    let hn = h_px / ih;
                    if handle == 0 || handle == 1 {
                        r.y0 = r.y1 - hn;
                    } else {
                        r.y1 = r.y0 + hn;
                    }
                }
                let rr = lightcraft_geom::Rect::new(r.x0.min(r.x1), r.y0.min(r.y1), r.x0.max(r.x1), r.y0.max(r.y1));
                if rr.width() > 0.02 && rr.height() > 0.02 {
                    let _ = app.run("crop.set", json!({"rect": [rr.x0, rr.y0, rr.x1, rr.y1]}));
                }
            }
            Some(Gesture::CropRotate { start_angle, a0 }) => {
                let c = map.screen(Point::new(0.5, 0.5));
                let a = (q - c).angle();
                let ang = (start_angle + (a - a0).to_degrees() as f64).clamp(-45.0, 45.0);
                let _ = app.run("crop.straighten", json!({"angle": (ang * 100.0).round() / 100.0}));
            }
            _ => {}
        }
    }
    if resp.drag_stopped() {
        app.gesture = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
    let _ = id;
}

/// Guided Upright: draw up to four guides along lines that should be vertical or horizontal. Guides are
/// stored in lens-corrected (pre-perspective) coordinates, so they stay attached to the image as it warps.
fn guided_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, frame: &Frame, d: &DevelopSettings) {
    let p = ui.painter_at(map.rect.expand(8.0));
    let col = Color32::from_rgb(255, 196, 40);
    let draw = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], Stroke::new(3.0, Color32::from_black_alpha(120)));
        p.line_segment([a, b], Stroke::new(1.5, col));
        for q in [a, b] {
            p.circle_filled(q, 4.0, col);
            p.circle_stroke(q, 4.0, Stroke::new(1.0, Color32::BLACK));
        }
    };
    for (i, (a, b)) in d.geometry.guides.iter().enumerate() {
        let (sa, sb) = (map.screen(frame.corrected_to_transformed(*a)), map.screen(frame.corrected_to_transformed(*b)));
        register(ui.ctx(), format!("uprightGuide:{i}"), Rect::from_two_pos(sa, sb));
        draw(sa, sb);
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    if resp.drag_started()
        && let Some(q) = resp.interact_pointer_pos()
    {
        app.gesture = Some(Gesture::Guide { a: map.norm(q) });
    }
    if let (Some(Gesture::Guide { a }), Some(q)) = (&app.gesture, resp.interact_pointer_pos()) {
        draw(map.screen(*a), q);
    }
    if resp.drag_stopped()
        && let (Some(Gesture::Guide { a }), Some(q)) = (app.gesture.clone(), resp.interact_pointer_pos())
    {
        app.gesture = None;
        let b = map.norm(q);
        if a.dist(b) > 0.02 {
            let (ca, cb) = (frame.transformed_to_corrected(a), frame.transformed_to_corrected(b));
            let _ = app.run("geometry.guides", json!({"guides": [[ca.x, ca.y, cb.x, cb.y]], "add": true}));
        }
    }
}

/// Normalized oriented coords → the straightened (rotated) frame the crop rect lives in.
fn to_straight(n: Point, angle: f64, frame: &Frame) -> Point {
    let (w, h) = (frame.ow, frame.oh);
    let px = Point::new(n.x * w, n.y * h);
    let r = Affine::rotate_about(angle.to_radians(), Point::new(w / 2.0, h / 2.0)).apply(px);
    Point::new(r.x / w, r.y / h)
}

fn frame_crop_quad(d: &DevelopSettings, frame: &Frame) -> [Point; 4] {
    let f = Frame { crop: d.crop.geometry, ..frame.clone() };
    f.crop_quad_norm()
}

// ------------------------------------------------------------------------ masks

/// Where a mask component's pin sits (normalized), for shapes that have a place on the photo.
pub(crate) fn component_pin(shape: &MaskShape) -> Option<Point> {
    match shape {
        MaskShape::Radial { center, .. } => Some(*center),
        MaskShape::Linear { start, end } => Some(Point::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0)),
        MaskShape::Brush { strokes } => strokes.iter().find(|s| !s.erase).and_then(|s| s.points.first()).copied(),
        _ => None,
    }
}

/// `shape` moved by `dn` (normalized); `handle` 1/2 = a linear gradient's start/end set to `at`.
fn moved_shape(shape: &MaskShape, handle: u8, dn: Point, at: Point) -> MaskShape {
    let mv = |p: Point| Point::new(p.x + dn.x, p.y + dn.y);
    match shape.clone() {
        MaskShape::Radial { center, rx, ry, angle, feather, invert } => MaskShape::Radial { center: mv(center), rx, ry, angle, feather, invert },
        MaskShape::Linear { start, end } => match handle {
            1 => MaskShape::Linear { start: at, end },
            2 => MaskShape::Linear { start, end: at },
            _ => MaskShape::Linear { start: mv(start), end: mv(end) },
        },
        MaskShape::Brush { mut strokes } => {
            for st in &mut strokes {
                st.points.iter_mut().for_each(|p| *p = mv(*p));
            }
            MaskShape::Brush { strokes }
        }
        s => s,
    }
}

/// The Masking tool on the photo: outlines and handles of the selected mask, a pin per mask
/// component (click selects its mask, drag moves the component), and brush painting. The mask
/// itself shows as a rendered overlay ([`view_overlay`]).
fn mask_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    let tint = {
        let [r, g, b] = app.ui.mask_overlay_color;
        Color32::from_rgba_unmultiplied(r, g, b, 110)
    };
    let long = (map.rect.width().max(map.rect.height())) as f64;
    let active = app.session.active_mask;
    let clip = app.canvas_rect.unwrap_or(map.rect);
    let p = &ui.painter_at(clip);
    // (mask, component, handle, screen position) of everything that can be grabbed
    let mut grips: Vec<(u32, usize, u8, Pos2)> = Vec::new();
    for m in &d.masks {
        let sel = Some(m.id) == active;
        for (ci, c) in m.components.iter().enumerate() {
            match &c.shape {
                MaskShape::Radial { center, rx, ry, angle, .. } if sel => {
                    let pts: Vec<Pos2> = (0..64)
                        .map(|i| {
                            let a = i as f64 / 64.0 * std::f64::consts::TAU;
                            let (s, co) = angle.to_radians().sin_cos();
                            let (x, y) = (rx * a.cos(), ry * a.sin());
                            let l = frame_long_norm(map);
                            map.screen(Point::new(center.x + (x * co - y * s) * l.0, center.y + (x * s + y * co) * l.1))
                        })
                        .collect();
                    p.add(egui::Shape::closed_line(pts, Stroke::new(1.5, Color32::from_white_alpha(230))));
                }
                MaskShape::Linear { start, end } if sel => {
                    let (a, b) = (map.screen(*start), map.screen(*end));
                    let dir = (b - a).normalized();
                    let perp = vec2(-dir.y, dir.x) * 3000.0;
                    for (q, alpha) in [(a, 230), (b, 110)] {
                        p.line_segment([q - perp, q + perp], Stroke::new(1.0, Color32::from_white_alpha(alpha)));
                    }
                    for (i, q) in [a, b].iter().enumerate() {
                        p.circle_stroke(*q, 5.0, Stroke::new(1.5, Color32::WHITE));
                        register(ui.ctx(), format!("maskHandle:{}:{ci}:{i}", m.id), Rect::from_center_size(*q, vec2(14.0, 14.0)));
                        grips.push((m.id, ci, i as u8 + 1, *q));
                    }
                }
                _ => {}
            }
            if app.ui.mask_pins
                && let Some(at) = component_pin(&c.shape)
            {
                let q = map.screen(at);
                pin(p, q, sel);
                register(ui.ctx(), format!("maskPin:{}:{ci}", m.id), Rect::from_center_size(q, vec2(14.0, 14.0)));
                grips.push((m.id, ci, 0, q));
            }
        }
    }
    // brush tool
    if app.ui.tool == "brush" {
        let r = (app.ui.brush_size as f64 * long) as f32;
        if let Some(h) = resp.hover_pos() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            p.circle_stroke(h, r, Stroke::new(1.0, Color32::WHITE));
            p.circle_stroke(h, r * (1.0 - app.ui.brush_feather / 100.0).max(0.05), Stroke::new(1.0, Color32::from_white_alpha(120)));
        }
        if let Some(q) = resp.interact_pointer_pos()
            && (resp.drag_started() || resp.dragged() || resp.clicked())
        {
            let n = map.norm(q);
            match &mut app.gesture {
                Some(Gesture::Brush { points }) => points.push(n),
                _ => app.gesture = Some(Gesture::Brush { points: vec![n] }),
            }
        }
        // the stroke being painted (the render shows it once it's committed)
        if let Some(Gesture::Brush { points }) = &app.gesture {
            for q in points {
                p.circle_filled(map.screen(*q), r, tint);
            }
        }
        if (resp.drag_stopped() || resp.clicked())
            && let Some(Gesture::Brush { points }) = app.gesture.take()
        {
            let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
            // holding ⌥ (Alt) paints with the other mode: Erase while adding, Add while erasing
            let erase = app.ui.brush_erase != resp.ctx.input(|i| i.modifiers.alt);
            let _ = app.run(
                "mask.brushStroke",
                json!({"points": pts, "size": app.ui.brush_size, "feather": app.ui.brush_feather, "flow": app.ui.brush_flow,
                       "erase": erase, "autoMask": app.ui.brush_auto_mask}),
            );
        }
        return;
    }
    // the grip under the pointer: the selected mask's first, then the nearest
    let hit = |q: Pos2| {
        grips
            .iter()
            .filter(|g| g.3.distance(q) < 10.0)
            .min_by(|a, b| {
                (Some(a.0) != active, a.3.distance(q)).partial_cmp(&(Some(b.0) != active, b.3.distance(q))).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    };
    if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && let Some((mid, ..)) = hit(q)
        && Some(mid) != active
    {
        let _ = app.run("mask.select", json!({"id": mid}));
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        let grip = hit(q).map(|(m, c, h, _)| (m, c, h)).or_else(|| {
            // anywhere on the photo drags the selected linear gradient
            let m = d.masks.iter().find(|m| Some(m.id) == active)?;
            matches!(m.components.first()?.shape, MaskShape::Linear { .. }).then_some((m.id, 0, 0))
        });
        if let Some((mask, comp, handle)) = grip {
            if Some(mask) != active {
                let _ = app.run("mask.select", json!({"id": mask}));
            }
            let _ = app.run("develop.beginInteraction", json!({"label": "Edit Mask"}));
            app.gesture = Some(Gesture::MaskHandle { mask, comp, handle });
        }
    }
    if resp.dragged()
        && let (Some(Gesture::MaskHandle { mask, comp, handle }), Some(q)) = (app.gesture.clone(), resp.interact_pointer_pos())
        && let Some(shape) = d.masks.iter().find(|m| m.id == mask).and_then(|m| m.components.get(comp)).map(|c| &c.shape)
    {
        let n = map.norm(q);
        let n0 = map.norm(q - resp.drag_delta());
        let new_shape = moved_shape(shape, handle, Point::new(n.x - n0.x, n.y - n0.y), n);
        let _ = app.run("mask.update", json!({"id": mask, "component": comp, "shape": new_shape}));
    }
    if resp.drag_stopped() && matches!(app.gesture, Some(Gesture::MaskHandle { .. })) {
        app.gesture = None;
        let _ = app.run("develop.endInteraction", json!({}));
    }
}

/// Long-edge units → normalized units for x and y.
fn frame_long_norm(map: &CanvasMap) -> (f64, f64) {
    // derive from the mapping: 1 normalized unit in x and y in screen points
    let o = map.screen(Point::new(0.0, 0.0));
    let x = map.screen(Point::new(1.0, 0.0)).distance(o) as f64;
    let y = map.screen(Point::new(0.0, 1.0)).distance(o) as f64;
    let l = x.max(y);
    (l / x.max(1e-9), l / y.max(1e-9))
}

fn pin(p: &egui::Painter, c: Pos2, sel: bool) {
    p.circle_filled(c, 6.0, if sel { Color32::from_rgb(1, 101, 221) } else { Color32::from_gray(200) });
    p.circle_stroke(c, 6.0, Stroke::new(1.5, Color32::WHITE));
}

// ------------------------------------------------------------------------ remove

/// The Remove tool on the photo: every spot's outline and pin (the selected one with its source),
/// click a pin to select its spot, drag a target or source to move it, paint elsewhere to add one.
fn remove_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    let long = (map.rect.width().max(map.rect.height())) as f64;
    let p = ui.painter_at(app.canvas_rect.unwrap_or(map.rect));
    let active = app.session.active_spot.filter(|i| *i < d.spots.len());
    // (spot, is source, screen centre, radius) of everything that can be grabbed
    let mut grips: Vec<(usize, bool, Pos2, f32)> = Vec::new();
    for (i, s) in d.spots.iter().enumerate() {
        let r = (s.size * long) as f32;
        let sel = Some(i) == active;
        let col = Color32::from_white_alpha(if sel { 230 } else { 150 });
        for q in &s.points {
            p.circle_stroke(map.screen(*q), r, Stroke::new(if sel { 1.5 } else { 1.0 }, col));
        }
        let Some(&t) = s.points.first() else { continue };
        let tq = map.screen(t);
        if let Some(o) = s.source_offset.filter(|_| sel) {
            let sq = map.screen(Point::new(t.x + o.x, t.y + o.y));
            for q in &s.points {
                p.circle_stroke(map.screen(Point::new(q.x + o.x, q.y + o.y)), r, Stroke::new(1.0, Color32::from_white_alpha(170)));
            }
            let dir = (tq - sq).normalized();
            p.arrow(sq + dir * r, (tq - sq) - dir * (2.0 * r).min((tq - sq).length()), Stroke::new(1.0, Color32::from_white_alpha(200)));
            register(ui.ctx(), format!("spotSource:{i}"), Rect::from_center_size(sq, vec2(14.0, 14.0)));
            grips.push((i, true, sq, r));
        }
        pin(&p, tq, sel);
        register(ui.ctx(), format!("spotPin:{i}"), Rect::from_center_size(tq, vec2(14.0, 14.0)));
        grips.push((i, false, tq, r));
    }
    let r = (app.ui.remove_size as f64 * long) as f32;
    // a target or source under `q`: the selected spot's first, then the nearest pin
    let hit = |q: Pos2| {
        grips
            .iter()
            .filter(|g| g.2.distance(q) < g.3.max(8.0))
            .min_by(|a, b| {
                (Some(a.0) != active, a.2.distance(q)).partial_cmp(&(Some(b.0) != active, b.2.distance(q))).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
    };
    let hover = resp.hover_pos().and_then(hit);
    if let Some(h) = resp.hover_pos() {
        if hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        } else {
            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            p.circle_stroke(h, r, Stroke::new(1.0, Color32::WHITE));
            p.circle_stroke(h, r * (1.0 - app.ui.remove_feather / 100.0).max(0.05), Stroke::new(1.0, Color32::from_white_alpha(110)));
        }
    }
    let press = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
    if resp.drag_started()
        && let Some((spot, source, ..)) = press.and_then(hit)
    {
        if Some(spot) != active {
            let _ = app.run("spot.select", json!({"index": spot}));
        }
        let _ = app.run("develop.beginInteraction", json!({"label": "Edit Spot"}));
        app.gesture = Some(Gesture::SpotMove { spot, source });
    }
    if let Some(Gesture::SpotMove { spot, source }) = app.gesture.clone() {
        if resp.dragged()
            && let Some(q) = resp.interact_pointer_pos()
        {
            let (n, n0) = (map.norm(q), map.norm(q - resp.drag_delta()));
            let dn = [n.x - n0.x, n.y - n0.y];
            if dn != [0.0, 0.0] {
                let _ = app.run("spot.update", json!({"index": spot, if source { "moveSource" } else { "move" }: dn}));
            }
        }
        if resp.drag_stopped() {
            app.gesture = None;
            let _ = app.run("develop.endInteraction", json!({}));
        }
        return;
    }
    if resp.clicked()
        && let Some((spot, ..)) = press.and_then(hit)
    {
        let _ = app.run("spot.select", json!({"index": spot}));
        return;
    }
    if let Some(q) = resp.interact_pointer_pos()
        && (resp.drag_started() || resp.dragged() || resp.clicked())
    {
        let n = map.norm(q);
        match &mut app.gesture {
            Some(Gesture::Spot { points }) => points.push(n),
            _ => app.gesture = Some(Gesture::Spot { points: vec![n] }),
        }
    }
    if let Some(Gesture::Spot { points }) = &app.gesture {
        for q in points {
            p.circle_filled(map.screen(*q), r, Color32::from_white_alpha(60));
        }
    }
    if (resp.drag_stopped() || resp.clicked())
        && let Some(Gesture::Spot { points }) = app.gesture.take()
    {
        let mode = match app.ui.tool.as_str() {
            "heal" => "heal",
            "clone" => "clone",
            _ => "remove",
        };
        let pts: Vec<[f64; 2]> = points.iter().map(|q| [q.x, q.y]).collect();
        let _ = app.run(
            "spot.add",
            json!({"mode": mode, "points": pts, "size": app.ui.remove_size, "feather": app.ui.remove_feather, "opacity": app.ui.remove_opacity}),
        );
    }
}

// ------------------------------------------------------------------------ red eye

/// Red Eye tool: drag an ellipse over an eye (a new correction), click one to select it.
fn eye_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, map: &CanvasMap, d: &DevelopSettings) {
    // screen points per long-edge unit
    let o = map.screen(Point::new(0.0, 0.0));
    let (sx, sy) = (map.screen(Point::new(1.0, 0.0)).distance(o), map.screen(Point::new(0.0, 1.0)).distance(o));
    let per_long = sx.max(sy);
    let p = ui.painter();
    for (i, e) in d.red_eye.iter().enumerate() {
        let c = map.screen(e.center);
        let r = vec2(e.rx as f32 * per_long, e.ry as f32 * per_long);
        let sel = i == app.ui.eye;
        let col = if sel { Color32::WHITE } else { Color32::from_white_alpha(150) };
        p.add(egui::Shape::ellipse_stroke(c, r, Stroke::new(if sel { 1.5 } else { 1.0 }, col)));
        p.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], Stroke::new(1.0, col));
        p.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], Stroke::new(1.0, col));
    }
    if resp.hover_pos().is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        // from where the button went down (a drag is recognized only after some movement)
        app.gesture = Some(Gesture::Eye { a: map.norm(q) });
    }
    if let Some(Gesture::Eye { a }) = app.gesture
        && let Some(q) = resp.interact_pointer_pos()
    {
        let (a, b) = (map.screen(a), q);
        p.add(egui::Shape::ellipse_stroke(a.lerp(b, 0.5), ((b - a) / 2.0).abs(), Stroke::new(1.0, Color32::WHITE)));
    }
    if resp.drag_stopped()
        && let Some(Gesture::Eye { a }) = app.gesture.take()
        && let Some(q) = resp.interact_pointer_pos()
    {
        let (sa, sb) = (map.screen(a), q);
        let half = ((sb - sa) / 2.0).abs() / per_long;
        if half.x > 0.002 && half.y > 0.002 {
            let c = map.norm(sa.lerp(sb, 0.5));
            let r = app.run("redeye.add", json!({"center": [c.x, c.y], "rx": half.x, "ry": half.y, "pet": app.ui.eye_pet}));
            if let Ok(v) = r {
                app.ui.eye = v["index"].as_u64().unwrap_or(0) as usize;
            }
        }
    } else if resp.clicked()
        && let Some(q) = resp.interact_pointer_pos()
        && let Some(i) = d.red_eye.iter().position(|e| {
            let c = map.screen(e.center);
            let (dx, dy) = ((q.x - c.x) / (e.rx as f32 * per_long).max(4.0), (q.y - c.y) / (e.ry as f32 * per_long).max(4.0));
            dx * dx + dy * dy <= 1.0
        })
    {
        app.ui.eye = i;
    }
}

// ------------------------------------------------------------------------ filmstrip

pub(crate) fn filmstrip(app: &mut LightcraftApp, ui: &mut egui::Ui, r: Rect) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(r, 0.0, t.canvas);
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 4.0)), 0.0, Color32::from_gray(0x20));
    let ids = app.session.visible_cloned();
    let active = app.session.selection.active;
    let cell_w = 120.0;
    let ppp = ui.ctx().pixels_per_point();
    if ids.is_empty() {
        let why = if app.session.filter != Default::default() { "No photos match the filters (View → Clear Filters)" } else { "No photos" };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, why, t.font(12.5), t.text_dim);
        return;
    }
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(0.0, 4.0))));
    // a vertical mouse wheel scrolls the strip sideways (horizontal trackpad scrolls still work)
    child.style_mut().always_scroll_the_only_direction = true;
    // only a new active photo (or the strip appearing) scrolls it; see `grid::follow_active`
    let follow = super::grid::follow_active(child.ctx(), egui::Id::new("film-follow-active"), active);
    egui::ScrollArea::horizontal().id_salt("filmstrip").auto_shrink([false, false]).show_viewport(&mut child, |ui, vp| {
        let (area, _) = ui.allocate_exact_size(vec2(ids.len() as f32 * cell_w, r.height() - 16.0), Sense::hover());
        app.film_scroll = Some(vp.left());
        for (i, id) in ids.iter().enumerate() {
            let cr = Rect::from_min_size(pos2(area.left() + i as f32 * cell_w, area.top()), vec2(cell_w, area.height()));
            let local = Rect::from_min_size(pos2(i as f32 * cell_w, 0.0), cr.size());
            // centred when it is (partly) off screen; a click on a visible thumbnail leaves the strip alone
            if follow && Some(*id) == active && !(local.left() >= vp.left() && local.right() <= vp.right()) {
                ui.scroll_to_rect(cr, Some(egui::Align::Center));
            }
            if !local.intersects(vp.expand2(vec2(cell_w * 4.0, 0.0))) {
                continue;
            }
            let resp = ui.interact(cr, egui::Id::new(("film", id.0)), Sense::click());
            register(ui.ctx(), format!("film:{}", id.0), cr);
            let sel = Some(*id) == active;
            let p = ui.painter();
            if sel {
                p.rect_filled(cr, 0.0, t.cell_selected);
            } else if resp.hovered() {
                p.rect_filled(cr, 0.0, t.cell_selected.gamma_multiply(0.6));
            }
            let names = app.ui.settings.film_names;
            if let Some(ph) = app.session.catalog.photo(*id).filter(|_| names) {
                let name = ph.file_name.rsplit_once('.').map(|(n, _)| n).unwrap_or(&ph.file_name);
                let short: String = if name.len() > 14 { format!("{}…", &name[..13]) } else { name.to_string() };
                p.text(pos2(cr.left() + 8.0, cr.top() + 10.0), Align2::LEFT_CENTER, short, t.font(10.0), t.text_dim);
                p.text(pos2(cr.right() - 8.0, cr.top() + 10.0), Align2::RIGHT_CENTER, &ph.format, t.semibold(8.5), t.text_dim);
            }
            let img_area = Rect::from_min_max(cr.min + vec2(10.0, 22.0), cr.max - vec2(10.0, 8.0));
            super::grid::request_thumb(app, *id, (256.0 * ppp.min(2.0) / 2.0) as usize * 2, 8);
            if let Some(tex) = app.renderer.thumb(*id) {
                let [tw, th] = tex.size;
                let s = (img_area.width() / tw as f32).min(img_area.height() / th as f32);
                let fr = Rect::from_center_size(img_area.center(), vec2(tw as f32 * s, th as f32 * s));
                p.image(tex.tex.id(), fr, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                if sel {
                    p.rect_stroke(fr, 0.0, Stroke::new(1.5, Color32::WHITE), StrokeKind::Outside);
                }
                if app.ui.settings.film_badges
                    && let Some(ph) = app.session.catalog.photo(*id)
                {
                    film_badges(p, &t, fr, ph);
                }
                if let Some(st) = app.session.catalog.stack_of(*id) {
                    let text =
                        if st.collapsed { st.photos.len().to_string() } else { format!("{}/{}", st.position(*id).unwrap_or(0) + 1, st.photos.len()) };
                    let g = p.layout_no_wrap(text, t.semibold(9.5), Color32::WHITE);
                    let br = Rect::from_min_size(fr.min + vec2(3.0, 3.0), vec2(g.size().x + 22.0, 15.0));
                    p.rect_filled(br, 7.5, Color32::from_black_alpha(170));
                    crate::icons::paint(p, Rect::from_min_size(br.min + vec2(4.0, 2.0), vec2(11.0, 11.0)), crate::icons::Icon::Stack, Color32::WHITE);
                    p.galley(pos2(br.min.x + 17.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
                }
            }
            if resp.clicked() {
                let m = ui.input(|i| i.modifiers);
                let mode = if m.command {
                    "toggle"
                } else if m.shift {
                    "range"
                } else {
                    "replace"
                };
                let _ = app.run("library.select", json!({"ids": [id.0], "mode": mode}));
            }
        }
    });
}

/// Rating, flag and edited badges along a filmstrip thumbnail's bottom edge (Settings → Interface).
fn film_badges(p: &egui::Painter, t: &Tokens, fr: Rect, ph: &lightcraft_catalog::Photo) {
    use crate::icons::{Icon, paint};
    use lightcraft_catalog::Flag;
    let edited = ph.is_edited();
    if ph.rating == 0 && ph.flag == Flag::None && !edited {
        return;
    }
    let bar = Rect::from_min_max(pos2(fr.left(), fr.bottom() - 15.0), fr.right_bottom());
    p.rect_filled(bar, 0.0, Color32::from_black_alpha(130));
    let y = bar.center().y;
    let mut x = bar.left() + 3.0;
    for i in 0..ph.rating {
        paint(p, Rect::from_min_size(pos2(x + i as f32 * 9.0, y - 4.0), vec2(8.0, 8.0)), Icon::StarFilled, t.star);
    }
    x += ph.rating as f32 * 9.0 + 2.0;
    match ph.flag {
        Flag::Pick => paint(p, Rect::from_min_size(pos2(x, y - 5.0), vec2(10.0, 10.0)), Icon::FlagPick, t.pick),
        Flag::Reject => paint(p, Rect::from_min_size(pos2(x, y - 5.0), vec2(10.0, 10.0)), Icon::FlagReject, t.reject),
        Flag::None => {}
    }
    if edited {
        paint(p, Rect::from_min_size(pos2(bar.right() - 13.0, y - 5.0), vec2(10.0, 10.0)), Icon::Sliders, t.text_label);
    }
}

/// Straighten tool: drag along a horizon (or a vertical) to set the crop angle; double-click = Auto.
/// The image is shown unrotated in the crop view, so the line's on-screen angle is its image angle.
/// `held`: drawn with ⌘ held in the crop tool, which stays active afterwards.
fn straighten_overlay(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, held: bool) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    if !held && resp.double_clicked() {
        let _ = app.run("crop.autoStraighten", json!({}));
        app.ui.tool.clear();
        app.gesture = None;
        return;
    }
    if resp.drag_started()
        && let Some(q) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        app.gesture = Some(Gesture::StraightenLine { a: q });
    }
    let Some(Gesture::StraightenLine { a }) = app.gesture.clone() else { return };
    let Some(b) = resp.interact_pointer_pos().or(resp.hover_pos()) else { return };
    let p = ui.painter();
    p.line_segment([a, b], Stroke::new(3.0, Color32::from_black_alpha(140)));
    p.line_segment([a, b], Stroke::new(1.5, Color32::WHITE));
    if resp.drag_stopped() {
        app.gesture = None;
        let v = b - a;
        if v.length() > 8.0 {
            let mut deg = v.y.atan2(v.x).to_degrees() as f64;
            // fold to the nearest axis: a line closer to vertical straightens plumb lines
            while deg > 45.0 {
                deg -= 90.0;
            }
            while deg < -45.0 {
                deg += 90.0;
            }
            let _ = app.run("crop.straighten", json!({"angle": (-deg * 100.0).round() / 100.0}));
        }
        if !held {
            app.ui.tool.clear();
        }
    }
}
