//! Photo Grid (justified rows) and Square Grid. Virtualized: only visible cells request thumbnails.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_catalog::{Catalog, ColorLabel, DateRun, Flag, GroupBy, PhotoId, SortKey, StackId};
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::render::Slot;
use crate::state::ViewMode;
use crate::theme::Tokens;
use crate::widgets::register;

/// Thumbnail render size (pixels, long edge) for a cell of `pts` points.
fn thumb_px(pts: f32, ppp: f32) -> usize {
    let px = (pts * ppp).ceil() as usize;
    if px <= 256 {
        256
    } else if px <= 384 {
        384
    } else {
        512
    }
}

/// Counters of the grid's work, cumulative over frames (benchmarks and tests: an unchanged frame
/// must not rebuild the view, its layout or its indexes, and visits only the cells near the screen).
#[derive(Clone, Debug, Default)]
pub struct GridStats {
    /// Frames the grid was drawn.
    pub frames: u64,
    /// Cells looked at (tested against the viewport or drawn).
    pub cells_visited: u64,
    /// Layouts computed.
    pub layout_builds: u64,
    /// Per-view data (date runs, row index) built.
    pub view_builds: u64,
    /// Stack indexes built.
    pub stack_index_builds: u64,
    /// Wall-clock time of the last [`show`].
    pub last_show: web_time::Duration,
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t0 = web_time::Instant::now();
    show_inner(app, ui);
    app.caches.grid_stats.frames += 1;
    app.caches.grid_stats.last_show = t0.elapsed();
}

/// Photo → (stack, position in the stack).
type StackIndex = HashMap<PhotoId, (StackId, usize)>;

/// What the date runs depend on: the visible list's generation, the date the grid groups by
/// (`None`: no headers) and the grouping.
type RunsKey = (u64, Option<SortKey>, GroupBy);

/// What the grid derives from the visible photos, kept across frames. Everything is keyed by the
/// session's visible-list generation ([`lightcraft_engine::Session::visible_shared`]), which
/// changes whenever the list is recomputed (any catalog change: crops, orientation, dates, stacks;
/// or another source, filter or sort) — so an unchanged frame neither copies nor hashes the ids.
#[derive(Default)]
pub struct GridCache {
    runs: Option<(RunsKey, Arc<Vec<DateRun>>)>,
    /// Layout by (runs key, width, thumbnail size, square).
    layout: Option<((RunsKey, u32, u32, bool), Arc<GridLayout>)>,
    /// Stack index by generation.
    stacks: Option<(u64, Arc<StackIndex>)>,
    /// How many of the photos are browsed (Local) ones, by generation.
    local: Option<(u64, usize)>,
}

impl GridCache {
    fn runs(&mut self, stats: &mut GridStats, cat: &Catalog, ids: &[PhotoId], key: RunsKey) -> Arc<Vec<DateRun>> {
        match &self.runs {
            Some((k, r)) if *k == key => r.clone(),
            _ => {
                stats.view_builds += 1;
                let r = Arc::new(key.1.map(|sort| cat.date_runs(ids, sort, key.2)).unwrap_or_default());
                self.runs = Some((key, r.clone()));
                r
            }
        }
    }

    fn stacks(&mut self, stats: &mut GridStats, cat: &Catalog, generation: u64) -> Arc<StackIndex> {
        match &self.stacks {
            Some((g, s)) if *g == generation => s.clone(),
            _ => {
                stats.stack_index_builds += 1;
                let s = Arc::new(cat.stack_index());
                self.stacks = Some((generation, s.clone()));
                s
            }
        }
    }

    fn local(&mut self, cat: &Catalog, ids: &[PhotoId], generation: u64) -> usize {
        match self.local {
            Some((g, n)) if g == generation => n,
            _ => {
                let n = ids.iter().filter(|id| cat.photo(**id).is_some_and(|p| p.local)).count();
                self.local = Some((generation, n));
                n
            }
        }
    }
}

/// Width / height of each photo's cell: its cropped, oriented shape.
fn aspects(cat: &Catalog, ids: &[PhotoId]) -> Vec<f32> {
    ids.iter()
        .map(|id| {
            let p = cat.photo(*id);
            let (w, h) = p.map(|p| (p.width.max(1) as f32, p.height.max(1) as f32)).unwrap_or((3.0, 2.0));
            let swap = p.is_some_and(|p| p.develop.orientation.swaps_axes());
            let crop = p.map(|p| p.develop.crop.geometry.rect).unwrap_or(lightcraft_geom::Rect::UNIT);
            let (w, h) = if swap { (h, w) } else { (w, h) };
            (w * crop.width() as f32) / (h * crop.height() as f32).max(1e-3)
        })
        .collect()
}

/// The rows (of a layout's `rows`, top to bottom) that meet the y range `top..=bottom`, found by
/// binary search.
pub fn rows_between(rows: &[GridRow], top: f32, bottom: f32) -> &[GridRow] {
    let first = rows.partition_point(|r| r.bottom < top);
    let rest = rows.get(first..).unwrap_or(&[]);
    let n = rest.partition_point(|r| r.top <= bottom);
    rest.get(..n).unwrap_or(&[])
}

fn show_inner(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (generation, ids) = app.session.visible_shared();
    // header: source title + count
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    ui.painter().rect_filled(hr, 0.0, t.canvas);
    let sel_n = app.session.selection.ids.len();
    let chips = lightcraft_engine::filter_chips(&app.session.filter, &app.session.catalog);
    let total = app.session.source_total();
    let counted = super::chips::count_text(ids.len(), total, !chips.is_empty());
    let cnt = if sel_n > 1 { crate::i18n::tr_format!("{sel_n} selected · {counted}", counted = counted, sel_n = sel_n) } else { counted };
    match app.session.browse.clone().filter(|_| app.session.source == lightcraft_engine::LibrarySource::Folder) {
        Some(b) => {
            let local = app.caches.grid.local(&app.session.catalog, &ids, generation);
            folder_header(app, ui, hr, &b, &ids, local, &cnt)
        }
        None => {
            let title = app.session.source.label(&app.session.catalog);
            ui.painter().text(pos2(hr.left() + 20.0, hr.center().y), Align2::LEFT_CENTER, &title, t.semibold(17.0), t.text);
            ui.painter().text(pos2(hr.right() - 20.0, hr.center().y), Align2::RIGHT_CENTER, cnt, t.font(12.5), t.text_dim);
        }
    }
    super::chips::show(app, ui, &chips);
    if app.ui.filter_bar {
        super::filterbar::show(app, ui);
    }
    app.canvas_rect = Some(ui.max_rect());
    if ids.is_empty() {
        if !chips.is_empty() {
            let body = match total {
                Some(n) if n > 0 => {
                    crate::i18n::tr_format!("{n} photos in {} are hidden by the filters above", app.session.source.label(&app.session.catalog), n = n)
                }
                _ => "Remove a filter above, or choose Clear all".to_string(),
            };
            super::empty_message(ui, ui.max_rect(), "No photos match the active filters", &body);
        } else if app.session.filter != Default::default() {
            super::empty_message(
                ui,
                ui.max_rect(),
                "No matching photos",
                "A filter is hiding this view's photos: change it, or clear it (View → Clear Filters)",
            );
        } else if app.session.source == lightcraft_engine::LibrarySource::Folder {
            super::empty_message(ui, ui.max_rect(), "No photos in this folder", "Turn on Include subfolders, or pick another folder under Local");
        } else {
            super::empty_message(ui, ui.max_rect(), "No photos", "Import photos with File → Import Photos… (Cmd+Shift+I), or drop them here");
        }
        return;
    }
    let ppp = ui.ctx().pixels_per_point();
    let square = app.ui.view == ViewMode::SquareGrid;
    let target = app.ui.thumb_size;
    let avail_w = ui.available_width() - 8.0;
    let by = resolve_group(app.session.sort.group, target);
    let group_key = match app.session.source {
        lightcraft_engine::LibrarySource::RecentlyDeleted => None,
        lightcraft_engine::LibrarySource::RecentlyAdded => Some(SortKey::ImportDate),
        _ => Some(app.session.sort.key),
    };
    let runs_key = (generation, group_key, by);
    let stats = &mut app.caches.grid_stats;
    let runs = app.caches.grid.runs(stats, &app.session.catalog, &ids, runs_key);
    // the layout only changes with the photos (and their shapes), the grouping, the width and
    // the thumbnail size
    let lay_key = (runs_key, avail_w.to_bits(), target.to_bits(), square);
    let lay = match &app.caches.grid.layout {
        Some((k, l)) if *k == lay_key => l.clone(),
        _ => {
            stats.layout_builds += 1;
            let aspects = if square { vec![1.0; ids.len()] } else { aspects(&app.session.catalog, &ids) };
            let spans: Vec<(usize, usize)> = runs.iter().map(|r| (r.start, r.count)).collect();
            let l = Arc::new(layout(&aspects, &spans, avail_w, target, square));
            app.caches.grid.layout = Some((lay_key, l.clone()));
            l
        }
    };
    let stacks = app.caches.grid.stacks(stats, &app.session.catalog, generation);
    let total_h = lay.height + 12.0;
    let active = app.session.selection.active;
    // bring the active photo into view when it changes (keyboard, click, command) or the grid
    // comes back on screen; otherwise the scroll position is the user's
    let scroll_to: Option<Rect> = if follow_active(ui.ctx(), egui::Id::new("grid-follow-active"), active) {
        active.and_then(|a| ids.iter().position(|x| *x == a)).and_then(|i| lay.cells.get(i).copied())
    } else {
        None
    };
    let mut visible_ids = HashSet::new();
    let mut visited = 0u64;
    egui::ScrollArea::vertical().id_salt("grid-scroll").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total_h), Sense::hover());
        let origin = area.min;
        app.grid_scroll = Some(viewport.top());
        if let Some(r) = scroll_to {
            ui.scroll_to_rect(r.translate(origin.to_vec2()), None);
        }
        // only the rows on screen and a screen above and below it (their thumbnails are prefetched)
        let prefetch = viewport.expand2(vec2(0.0, viewport.height()));
        for row in rows_between(&lay.rows, prefetch.top(), prefetch.bottom()) {
            for i in row.start..row.end {
                let (Some(&id), Some(&rect)) = (ids.get(i), lay.cells.get(i)) else { continue };
                visited += 1;
                if !rect.intersects(prefetch) {
                    continue;
                }
                visible_ids.insert(id);
                let r = rect.translate(origin.to_vec2());
                let onscreen = rect.intersects(viewport);
                cell(app, ui, id, r, square, onscreen, ppp);
                if let Some((sid, pos)) = stacks.get(&id) {
                    stack_badge(app, ui, id, *sid, *pos, r, square);
                }
            }
        }
        // date headers (top to bottom); the current group's header sticks to the top while its
        // photos scroll by
        let first = lay.headers.partition_point(|h| h.bottom() < viewport.top());
        for (g, hr) in lay.headers.iter().enumerate().skip(first) {
            if hr.top() > viewport.bottom() {
                break;
            }
            if let Some(run) = runs.get(g) {
                group_header(app, ui, run, &ids, hr.translate(origin.to_vec2()), false);
            }
        }
        let sticky = lay.headers.partition_point(|h| h.top() <= viewport.top()).checked_sub(1);
        if let Some(g) = sticky
            && let Some(head) = lay.headers.get(g)
            && head.top() < viewport.top()
            && let Some(run) = runs.get(g)
        {
            // pushed up by the next header as it arrives
            let next_top = lay.headers.get(g + 1).map(|h| h.top()).unwrap_or(f32::INFINITY);
            let y = viewport.top().min(next_top - HEADER_H);
            let hr = Rect::from_min_size(pos2(0.0, y), vec2(area.width(), HEADER_H)).translate(origin.to_vec2());
            group_header(app, ui, run, &ids, hr, true);
        }
    });
    app.caches.grid_stats.cells_visited += visited;
    app.renderer.evict_thumbs(&visible_ids, 600);
    let _ = (Color32::BLACK, StrokeKind::Inside, Stroke::NONE);
}

/// Should a scrolling photo strip (`key`: the grid, the filmstrip) bring the active photo into
/// view on this pass? Yes when the active photo changed since the strip was last drawn, or when
/// the strip was not drawn on the previous pass (view switch, panel shown again). Otherwise the
/// scroll position is the user's: time passing, finished renders, edits to the photo or a released
/// scroll bar never move it (issue #11).
pub(crate) fn follow_active(ctx: &egui::Context, key: egui::Id, active: Option<PhotoId>) -> bool {
    let pass = ctx.cumulative_pass_nr();
    let last: Option<(Option<PhotoId>, u64)> = ctx.data(|d| d.get_temp(key));
    ctx.data_mut(|d| d.insert_temp(key, (active, pass)));
    match last {
        Some((was, drawn)) => was != active || drawn + 1 < pass,
        None => true,
    }
}

/// Height of a date header row (points).
pub const HEADER_H: f32 = 40.0;

/// Cell rectangles (one per photo, in order), date-header rectangles (one per group, top to
/// bottom), the rows of cells (top to bottom, for finding the visible ones by binary search) and
/// the total height of the grid content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GridLayout {
    pub cells: Vec<Rect>,
    pub headers: Vec<Rect>,
    pub rows: Vec<GridRow>,
    pub height: f32,
}

/// One row of cells: `cells[start..end]`, between `top` and `bottom`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridRow {
    pub top: f32,
    pub bottom: f32,
    pub start: usize,
    pub end: usize,
}

/// The grouping actually shown: `Auto` is days with large thumbnails, months with medium ones and
/// years when zoomed far out.
pub fn resolve_group(by: GroupBy, thumb_size: f32) -> GroupBy {
    match by {
        GroupBy::Auto if thumb_size >= 150.0 => GroupBy::Day,
        GroupBy::Auto if thumb_size >= 110.0 => GroupBy::Month,
        GroupBy::Auto => GroupBy::Year,
        other => other,
    }
}

/// Lay out the grid: `aspects` (width / height per photo; ignored for the square grid) in
/// `groups` (`(start, count)` runs; empty = one group without a header), `avail_w` wide, rows
/// about `target` high (square cells about `target` wide). Every group starts on a new row
/// below its header. Justified rows fill the width exactly except a group's last row, which keeps
/// the target height.
pub fn layout(aspects: &[f32], groups: &[(usize, usize)], avail_w: f32, target: f32, square: bool) -> GridLayout {
    let n = aspects.len();
    let whole = [(0, n)];
    let (groups, headed) = if groups.is_empty() { (&whole[..], false) } else { (groups, true) };
    let gap = if square { 1.0 } else { 6.0 };
    let mut out = GridLayout { cells: vec![Rect::NOTHING; n], headers: Vec::with_capacity(groups.len()), rows: Vec::new(), height: 0.0 };
    let mut y = 4.0f32;
    for &(start, count) in groups {
        let end = (start + count).min(n);
        if headed {
            out.headers.push(Rect::from_min_size(pos2(0.0, y), vec2(avail_w + 8.0, HEADER_H)));
            y += HEADER_H;
        }
        if start >= end {
            continue;
        }
        if square {
            let cols = ((avail_w + gap) / (target + gap)).floor().max(1.0);
            let cw = (avail_w - gap * (cols - 1.0)) / cols;
            for i in start..end {
                let k = (i - start) as f32;
                let (c, r) = (k % cols, (k / cols).floor());
                out.cells[i] = Rect::from_min_size(pos2(4.0 + c * (cw + gap), y + r * (cw + gap)), vec2(cw, cw));
            }
            let per_row = (cols as usize).max(1);
            for (r, row_start) in (start..end).step_by(per_row).enumerate() {
                let top = y + r as f32 * (cw + gap);
                out.rows.push(GridRow { top, bottom: top + cw, start: row_start, end: (row_start + per_row).min(end) });
            }
            let rows = ((end - start) as f32 / cols).ceil();
            y += rows * (cw + gap);
        } else {
            // justified rows: accumulate aspect ratios until the row is full
            let mut i = start;
            while i < end {
                let mut sum = 0.0;
                let mut j = i;
                while j < end {
                    sum += aspects[j].max(0.05);
                    let h = (avail_w - gap * (j - i) as f32) / sum;
                    j += 1;
                    if h <= target {
                        break;
                    }
                }
                let row_h = (avail_w - gap * (j - i - 1) as f32) / sum;
                let h = if j < end || row_h <= target { row_h } else { target };
                let mut x = 4.0;
                for k in i..j {
                    let w = aspects[k].max(0.05) * h;
                    out.cells[k] = Rect::from_min_size(pos2(x, y), vec2(w, h));
                    x += w + gap;
                }
                out.rows.push(GridRow { top: y, bottom: y + h, start: i, end: j });
                y += h + gap;
                i = j;
            }
        }
        if headed {
            y += 8.0;
        }
    }
    out.height = y;
    out
}

/// A date header: "Wednesday, 30 September 2026 · 12 photos". Clicking it selects the group
/// (Cmd/Shift: adds to the selection).
fn group_header(app: &mut LightcraftApp, ui: &mut egui::Ui, run: &DateRun, ids: &[PhotoId], r: Rect, pinned: bool) {
    let t = Tokens::get(ui.ctx());
    let resp = ui.interact(r, egui::Id::new(("group-header", run.start, pinned)), Sense::click());
    if !pinned {
        register(ui.ctx(), format!("group:{}", if run.key.is_empty() { "unknown" } else { &run.key }), r);
    }
    let p = ui.painter();
    if pinned {
        p.rect_filled(r, 0.0, t.canvas);
        p.hline(r.x_range(), r.bottom(), Stroke::new(1.0, t.divider));
    }
    let g = p.layout_no_wrap(run.label.clone(), t.semibold(15.0), if resp.hovered() { t.text } else { t.text_label });
    let x = r.left() + 8.0;
    let gw = g.size().x;
    p.galley(pos2(x, r.center().y - g.size().y / 2.0), g, t.text);
    let n = run.count;
    p.text(
        pos2(x + gw + 10.0, r.center().y),
        Align2::LEFT_CENTER,
        crate::i18n::tr_format!("· {n} photo{}", if n == 1 { "" } else { "s" }, n = n),
        t.font(12.5),
        t.text_dim,
    );
    if resp.clicked() {
        let group: Vec<u64> = ids[run.start..(run.start + run.count).min(ids.len())].iter().map(|p| p.0).collect();
        let m = ui.input(|i| i.modifiers);
        let mode = if m.command || m.shift { "add" } else { "replace" };
        let _ = app.run("library.select", json!({"ids": group, "mode": mode}));
    }
}

/// Request a grid/filmstrip thumbnail at `priority`. An unedited raw without a thumbnail texture
/// first gets a stand-in (its cached thumbnail, else its embedded camera preview), and its real
/// render then follows in the background.
pub fn request_thumb(app: &mut LightcraftApp, id: PhotoId, size: usize, priority: u32) {
    let Some(job) = app.session.thumb_job(id, size) else { return };
    let quick = if app.renderer.textures.contains_key(&Slot::Thumb(id)) { None } else { app.session.quick_thumb_job(&job) };
    match quick {
        Some(q) => {
            app.renderer.request_quick(Slot::ThumbQuick(id), q, priority + 1);
            if !app.renderer.is_pending(Slot::ThumbQuick(id)) {
                app.renderer.request(Slot::Thumb(id), job, BACKGROUND_THUMB_PRIORITY);
            }
        }
        None => app.renderer.request(Slot::Thumb(id), job, priority),
    }
}

/// Rendered thumbnails replacing embedded previews: after everything on screen.
pub const BACKGROUND_THUMB_PRIORITY: u32 = 3;

fn cell(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, r: Rect, square: bool, onscreen: bool, ppp: f32) {
    let t = Tokens::get(ui.ctx());
    let Some(photo) = app.session.catalog.photo(id).cloned() else { return };
    let resp = ui.interact(r, egui::Id::new(("cell", id.0)), Sense::click_and_drag());
    register(ui.ctx(), format!("thumb:{}", id.0), r);
    let selected = app.session.selection.contains(id);
    // screen readers: the file, then rating / flag / label
    let mut spoken = photo.file_name.clone();
    if photo.rating > 0 {
        spoken.push_str(&format!(", {} star{}", photo.rating, if photo.rating == 1 { "" } else { "s" }));
    }
    match photo.flag {
        lightcraft_catalog::Flag::Pick => spoken.push_str(", picked"),
        lightcraft_catalog::Flag::Reject => spoken.push_str(", rejected"),
        lightcraft_catalog::Flag::None => {}
    }
    if let Some(l) = photo.label {
        spoken.push_str(&format!(", {} label", app.session.catalog.label_name(l)));
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &spoken));
    let active = app.session.selection.active == Some(id);
    let p = ui.painter();
    let img_rect = if square {
        p.rect_filled(r, 0.0, if selected { t.cell_selected } else { t.cell });
        Rect::from_min_max(r.min + vec2(10.0, 24.0), r.max - vec2(10.0, 10.0))
    } else {
        r
    };
    // thumbnail
    let size = thumb_px(img_rect.width().max(img_rect.height()), ppp);
    request_thumb(app, id, size, if onscreen { 10 } else { 5 });
    if let Some(tex) = app.renderer.thumb(id) {
        let [tw, th] = tex.size;
        let fit = if square {
            let s = (img_rect.width() / tw as f32).min(img_rect.height() / th as f32);
            Rect::from_center_size(img_rect.center(), vec2(tw as f32 * s, th as f32 * s))
        } else {
            img_rect
        };
        p.image(tex.tex.id(), fit, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        if active {
            p.rect_stroke(fit.expand(if square { 2.0 } else { 0.0 }), 0.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
        } else if selected {
            p.rect_stroke(fit, 0.0, Stroke::new(2.0, Color32::from_gray(170)), StrokeKind::Outside);
        }
    } else {
        let ph = img_rect.shrink(if square { 20.0 } else { 0.0 });
        p.rect_filled(ph, 0.0, Color32::from_gray(38));
        if app.renderer.failure(Slot::Thumb(id)).is_some() {
            // unreadable / missing file
            p.text(ph.center(), Align2::CENTER_CENTER, "!", t.semibold(18.0), t.text_dim);
        }
    }
    // labels and badges
    if square && app.ui.show_filenames {
        let m = &photo.meta;
        let name = match app.ui.grid_info.as_str() {
            "exposure" => {
                let parts: Vec<String> = [
                    // whole seconds read "8 s"; fractions as written ("1/250")
                    (!m.shutter.is_empty())
                        .then(|| if m.shutter.contains('/') || m.shutter.ends_with('s') { m.shutter.clone() } else { format!("{} s", m.shutter) }),
                    m.aperture.map(|a| format!("f/{a:.1}").replace(".0", "")),
                    m.iso.map(|i| format!("ISO {i}")),
                    m.focal_mm.map(|f| format!("{f:.0} mm")),
                ]
                .into_iter()
                .flatten()
                .collect();
                if parts.is_empty() { "—".to_string() } else { parts.join(" · ") }
            }
            "date" => photo.captured.as_deref().map(lightcraft_catalog::dates::display_time).unwrap_or_else(|| "No date".into()),
            _ => photo.file_name.rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or(photo.file_name.clone()),
        };
        p.text(pos2(r.left() + 8.0, r.top() + 12.0), Align2::LEFT_CENTER, name, t.font(10.5), t.text_dim);
        let fmt = photo.format.clone();
        let g = p.layout_no_wrap(fmt, t.semibold(9.0), t.text_label);
        let br = Rect::from_min_size(pos2(r.right() - g.size().x - 14.0, r.top() + 5.0), g.size() + vec2(8.0, 3.0));
        p.rect_filled(br, 2.0, Color32::from_gray(26));
        p.galley(br.min + vec2(4.0, 1.5), g, t.text_label);
    }
    let show_badges = match app.ui.settings.grid_badges {
        crate::state::GridBadges::Auto => resp.hovered() || selected || photo.rating > 0 || photo.flag != Flag::None || photo.label.is_some(),
        crate::state::GridBadges::Always => true,
        crate::state::GridBadges::Never => false,
    };
    if show_badges {
        let bar = Rect::from_min_max(pos2(img_rect.left(), img_rect.bottom() - 24.0), img_rect.right_bottom());
        if resp.hovered() || selected {
            p.rect_filled(bar, 0.0, Color32::from_black_alpha(120));
        }
        let mut x = bar.left() + 6.0;
        for i in 0..photo.rating {
            paint(p, Rect::from_min_size(pos2(x + i as f32 * 13.0, bar.center().y - 6.0), vec2(12.0, 12.0)), Icon::StarFilled, t.star);
        }
        x += photo.rating as f32 * 13.0 + 4.0;
        match photo.flag {
            Flag::Pick => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagPick, t.pick),
            Flag::Reject => paint(p, Rect::from_min_size(pos2(x, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::FlagReject, t.reject),
            Flag::None => {}
        }
        let mut right = bar.right();
        if photo.is_edited() {
            paint(p, Rect::from_min_size(pos2(right - 20.0, bar.center().y - 7.0), vec2(14.0, 14.0)), Icon::Sliders, t.text_label);
            right -= 20.0;
        }
        if let Some(l) = photo.label {
            p.circle(pos2(right - 12.0, bar.center().y), 5.0, label_color(l), Stroke::new(1.0, Color32::from_black_alpha(120)));
        }
    }
    if let Some(name) = &photo.copy_name {
        // virtual copy: a folded-corner tag at the top right
        let g = p.layout_no_wrap(name.clone(), t.semibold(10.5), Color32::WHITE);
        let tr = if square { img_rect.right_top() + vec2(-6.0, 6.0) } else { img_rect.right_top() + vec2(-5.0, 5.0) };
        let br = Rect::from_min_max(pos2(tr.x - g.size().x - 26.0, tr.y), pos2(tr.x, tr.y + 20.0));
        p.rect_filled(br, 10.0, Color32::from_black_alpha(165));
        let c = pos2(br.left() + 13.0, br.center().y);
        let s = 5.0;
        p.add(egui::Shape::convex_polygon(
            vec![c + vec2(-s, -s), c + vec2(s * 0.3, -s), c + vec2(s, -s * 0.3), c + vec2(s, s), c + vec2(-s, s)],
            Color32::TRANSPARENT,
            Stroke::new(1.2, Color32::WHITE),
        ));
        p.line_segment([c + vec2(s * 0.3, -s), c + vec2(s * 0.3, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.line_segment([c + vec2(s * 0.3, -s * 0.3), c + vec2(s, -s * 0.3)], Stroke::new(1.2, Color32::WHITE));
        p.galley(pos2(br.left() + 22.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    }
    if let Some(why) = &photo.preview_only {
        // a raw shown from its embedded JPEG: a small amber "Preview" pill at the image's bottom
        // left, above the badge bar (always shown: it changes what editing does)
        let g = p.layout_no_wrap(crate::i18n::tr("Preview").into(), t.semibold(9.5), Color32::WHITE);
        let at = pos2(img_rect.left() + 6.0, img_rect.bottom() - 30.0 - 16.0);
        let br = Rect::from_min_size(at, vec2(g.size().x + 24.0, 16.0));
        p.rect_filled(br, 8.0, Color32::from_black_alpha(170));
        paint(p, Rect::from_center_size(pos2(br.left() + 9.0, br.center().y), vec2(12.0, 12.0)), Icon::Info, t.caution);
        p.galley(pos2(br.left() + 17.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
        register(ui.ctx(), format!("badge:previewOnly:{}", id.0), br);
        ui.interact(br, egui::Id::new(("preview-only-badge", id.0)), Sense::hover())
            .on_hover_text(crate::i18n::tr_format!("Preview only — {}", crate::widgets::preview_only_explanation(why)));
    }
    if photo.flag == Flag::Reject {
        p.rect_filled(img_rect, 0.0, Color32::from_black_alpha(110));
    }
    // interaction
    if let Some(k) = app.ui.keyword_painter.clone() {
        // painting: a click toggles the keyword on this photo
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
        if resp.clicked() {
            let has = photo.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k));
            let key = if has { "removeKeywords" } else { "addKeywords" };
            let _ = app.run("photo.setMeta", json!({"ids": [id.0], key: [k]}));
        }
        return;
    }
    if resp.clicked() {
        let m = ui.input(|i| i.modifiers);
        let mode = if m.shift {
            "range"
        } else if m.command {
            "toggle"
        } else {
            "replace"
        };
        let _ = app.run("library.select", json!({"ids": [id.0], "mode": mode}));
    }
    if resp.double_clicked() {
        let _ = app.run("library.select", json!({"ids": [id.0]}));
        let _ = app.run("view.detail", json!({}));
    }
    // drag the selection (this photo joins it, or replaces it when it wasn't selected)
    if resp.drag_started() {
        if !app.session.selection.contains(id) {
            let _ = app.run("library.select", json!({"ids": [id.0]}));
        }
        app.ui.dragging_photos = Some(app.session.selection.ids.iter().map(|p| p.0).collect());
    }
    resp.context_menu(|ui| context_menu(app, ui, id));
}

/// The header of a Local folder view: the path as a breadcrumb (each part opens that folder),
/// Include subfolders, Add to My Photos.
fn folder_header(app: &mut LightcraftApp, ui: &mut egui::Ui, hr: Rect, b: &lightcraft_engine::Browse, ids: &[PhotoId], local_n: usize, cnt: &str) {
    let t = Tokens::get(ui.ctx());
    let mut child =
        ui.new_child(egui::UiBuilder::new().max_rect(hr.shrink2(vec2(16.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 4.0;
    let parts: Vec<&str> = b.path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    // the last few parts (the root side is elided)
    let skip = parts.len().saturating_sub(4);
    if skip > 0 {
        child.label(egui::RichText::new("…  ›").color(t.text_dim));
    }
    for (i, part) in parts.iter().enumerate().skip(skip) {
        let last = i + 1 == parts.len();
        let text = egui::RichText::new(*part).size(13.0).color(if last { t.text } else { t.text_label });
        let r = child.add(egui::Label::new(if last { text.strong() } else { text }).sense(Sense::click()));
        register(child.ctx(), format!("crumb:{i}"), r.rect);
        if !last {
            if r.hovered() {
                child.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked() {
                let prefix = if b.path.starts_with('/') { format!("/{}", parts[..=i].join("/")) } else { parts[..=i].join("/") };
                let _ = app.run("library.browse", json!({"path": prefix}));
            }
            child.label(egui::RichText::new("›").color(t.text_dim));
        }
    }
    child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(egui::RichText::new(cnt).size(12.5).color(t.text_dim));
        ui.add_space(10.0);
        // counted once per view (cached); the ids are only gathered on a click
        if local_n > 0 {
            let label = crate::i18n::tr_format!("Add {local_n} to My Photos", local_n = local_n);
            if crate::widgets::text_button(ui, "addToLibrary", &label, false).clicked() {
                let local: Vec<u64> = ids.iter().filter(|id| app.session.catalog.photo(**id).is_some_and(|p| p.local)).map(|id| id.0).collect();
                let n = local.len();
                match app.run("photo.addToLibrary", json!({"ids": local})) {
                    Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Added {n} photo{} to My Photos", if n == 1 { "" } else { "s" }, n = n)),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        let mut sub = b.subfolders;
        let c = ui.checkbox(&mut sub, crate::i18n::tr("Include subfolders"));
        register(ui.ctx(), "check:includeSubfolders", c.rect);
        if c.changed() {
            let _ = app.run("library.browse", json!({"path": b.path, "subfolders": sub}));
        }
    });
}

/// While photos are dragged: a badge at the pointer; the drag ends when the button is up
/// (drop targets act on the release frame, before this runs).
pub fn drag_feedback(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(ids) = &app.ui.dragging_photos else { return };
    let (released, down, pos) = ctx.input(|i| (i.pointer.any_released(), i.pointer.any_down(), i.pointer.latest_pos()));
    if released || !down {
        app.ui.dragging_photos = None;
        return;
    }
    let Some(pos) = pos else { return };
    let t = Tokens::get(ctx);
    let n = ids.len();
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    egui::Area::new(egui::Id::new("drag-photos")).order(egui::Order::Tooltip).interactable(false).fixed_pos(pos + vec2(14.0, 10.0)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.accent).corner_radius(10.0).inner_margin(egui::Margin::symmetric(9, 3)).show(ui, |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::tr_format!("{n} photo{}", if n == 1 { "" } else { "s" }, n = n))
                    .color(Color32::WHITE)
                    .font(t.semibold(12.0)),
            );
        });
    });
}

pub use super::filterbar::label_color;

/// "Set Color Label" items (coloured dot + the label's name), shared by context menus.
pub fn label_menu(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let current = app.session.active().and_then(|id| app.session.catalog.photo(id)).and_then(|p| p.label);
    for l in ColorLabel::ALL {
        let name = app.session.catalog.label_name(l);
        let resp = ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 5.0, label_color(l));
            ui.selectable_label(current == Some(l), name)
        });
        if resp.inner.clicked() {
            let _ = app.run("photo.label", json!({"label": format!("{l:?}").to_lowercase()}));
            ui.close();
        }
    }
    if ui.selectable_label(current.is_none(), crate::i18n::tr("None")).clicked() {
        let _ = app.run("photo.label", json!({"label": "none"}));
    }
    ui.separator();
    if ui.button(crate::i18n::tr("Edit Label Names…")).clicked() {
        let _ = app.run("dialog.labelNames", json!({}));
    }
}

/// Stack badge at the cell's top-left: the photo count on a collapsed stack's top, `i/n` on the
/// members of an expanded stack. Clicking it expands/collapses the stack.
pub fn stack_badge(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId, sid: lightcraft_catalog::StackId, pos: usize, r: Rect, square: bool) {
    let t = Tokens::get(ui.ctx());
    let Some(st) = app.session.catalog.stack(sid) else { return };
    let n = st.photos.len();
    let collapsed = st.collapsed;
    let text = if collapsed { n.to_string() } else { format!("{}/{n}", pos + 1) };
    let p = ui.painter();
    let g = p.layout_no_wrap(text, t.semibold(11.0), Color32::WHITE);
    let origin = if square { r.min + vec2(10.0, 26.0) } else { r.min + vec2(5.0, 5.0) };
    let br = Rect::from_min_size(origin, vec2(g.size().x + 30.0, 20.0));
    let resp = ui.interact(br, egui::Id::new(("stack-badge", id.0)), Sense::click()).on_hover_text(if collapsed {
        "Stack — click to expand (S)"
    } else {
        "Stack — click to collapse (S)"
    });
    register(ui.ctx(), format!("stack:{}", id.0), br);
    let fill = if resp.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(165) };
    p.rect_filled(br, 10.0, fill);
    if pos == 0 && !collapsed {
        p.rect_stroke(br, 10.0, Stroke::new(1.0, t.accent), StrokeKind::Inside);
    }
    paint(p, Rect::from_min_size(br.min + vec2(6.0, 3.0), vec2(14.0, 14.0)), Icon::Stack, Color32::WHITE);
    p.galley(pos2(br.min.x + 24.0, br.center().y - g.size().y / 2.0), g, Color32::WHITE);
    if resp.clicked() {
        let _ = app.run("stack.toggle", json!({"ids": [id.0]}));
    }
}

pub fn context_menu(app: &mut LightcraftApp, ui: &mut egui::Ui, id: PhotoId) {
    if !app.session.selection.contains(id) {
        let _ = app.run("library.select", json!({"ids": [id.0]}));
    }
    if ui.button(crate::i18n::tr("Open in Detail")).clicked() {
        let _ = app.run("view.detail", json!({}));
    }
    if ui.button(crate::i18n::tr("Find Similar Photos")).clicked() {
        match app.run("library.findSimilar", json!({"id": id.0})) {
            Ok(r) => {
                let n = r["photos"].as_array().map_or(0, Vec::len);
                app.ui.view = crate::state::ViewMode::PhotoGrid;
                app.toast(ui.ctx(), format!("{n} similar photo{} · View ▸ Clear Filters to see all", if n == 1 { "" } else { "s" }));
            }
            Err(e) => app.toast(ui.ctx(), e),
        }
    }
    if ui.button(crate::i18n::tr("Set as Reference Photo")).clicked() {
        let _ = app.run("photo.setReference", json!({"id": id.0}));
    }
    ui.separator();
    ui.menu_button(crate::i18n::tr("Set Rating"), |ui| {
        for r in 0..=5 {
            if ui.button(if r == 0 { "No Stars".to_string() } else { "★".repeat(r) }).clicked() {
                let _ = app.run("photo.rate", json!({"rating": r}));
            }
        }
    });
    ui.menu_button(crate::i18n::tr("Set Flag"), |ui| {
        for (l, f) in [("Pick", "pick"), ("Reject", "reject"), ("Unflagged", "none")] {
            if ui.button(l).clicked() {
                let _ = app.run("photo.flag", json!({"flag": f}));
            }
        }
    });
    ui.menu_button(crate::i18n::tr("Set Color Label"), |ui| label_menu(app, ui));
    ui.menu_button(crate::i18n::tr("Add to Album"), |ui| {
        let albums: Vec<_> = app.session.catalog.albums().filter(|a| !a.folder && !a.is_smart()).map(|a| (a.id.0, a.name.clone())).collect();
        for (aid, name) in albums {
            if ui.button(name).clicked() {
                let _ = app.run("album.addPhotos", json!({"id": aid}));
            }
        }
    });
    // in a (non-smart) album: take the selection out of it, or make this photo its cover
    if let lightcraft_engine::LibrarySource::Album(aid) = app.session.source
        && app.session.catalog.album(aid).is_some_and(|a| !a.folder && !a.is_smart())
    {
        if ui.button(crate::i18n::tr("Remove from Album")).clicked() {
            let _ = app.run("album.removePhotos", json!({"id": aid.0}));
        }
        if ui.button(crate::i18n::tr("Set as Album Cover")).clicked() {
            let _ = app.run("album.setCover", json!({"id": aid.0, "photo": id.0}));
        }
    }
    if ui.button(crate::i18n::tr("Rename…")).clicked() {
        let _ = app.run("dialog.rename", json!({}));
    }
    if ui.button(crate::i18n::tr("Create Virtual Copy")).clicked() {
        let _ = app.run("photo.virtualCopy", json!({}));
    }
    if ui.button(crate::i18n::tr("Create Version")).clicked() {
        let _ = app.run("version.create", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Stack"), |ui| {
        let stacked = app.session.catalog.stack_of(id).is_some();
        let several = app.session.selection.ids.len() > 1;
        for (label, cmd, on) in [
            ("Group into Stack", "stack.group", several),
            ("Ungroup Stack", "stack.ungroup", stacked),
            ("Remove from Stack", "stack.remove", stacked),
            ("Set as Top of Stack", "stack.setTop", stacked),
            ("Move Up in Stack", "stack.moveUp", stacked),
            ("Move Down in Stack", "stack.moveDown", stacked),
            ("Split Stack", "stack.split", stacked),
            ("Expand/Collapse Stack", "stack.toggle", stacked),
        ] {
            if ui.add_enabled(on, egui::Button::new(label)).clicked() {
                let _ = app.run(cmd, json!({}));
            }
        }
        ui.separator();
        if ui.button(crate::i18n::tr("Auto-Stack by Capture Time…")).clicked() {
            let _ = app.run("dialog.autoStack", json!({}));
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Copy Edit Settings")).clicked() {
        let _ = app.run("develop.copy", json!({}));
    }
    if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Edit Settings"))).clicked() {
        let _ = app.run("develop.paste", json!({}));
    }
    if ui.add_enabled(app.session.clipboard.is_some(), egui::Button::new(crate::i18n::tr("Paste Selected Settings…"))).clicked() {
        let _ = app.run("dialog.pasteSettings", json!({}));
    }
    if ui.button(crate::i18n::tr("Reset Edits")).clicked() {
        let _ = app.run("develop.reset", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Photo Merge"), |ui| {
        let n = app.session.targets(&json!({})).len();
        for (id, label) in [("dialog.mergeHdr", "HDR…"), ("dialog.mergePanorama", "Panorama…"), ("dialog.mergeHdrPanorama", "HDR Panorama…")] {
            if ui.add_enabled(n >= 2, egui::Button::new(label)).clicked() {
                let _ = app.run(id, json!({}));
            }
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Rotate Left")).clicked() {
        let _ = app.run("photo.rotateLeft", json!({}));
    }
    if ui.button(crate::i18n::tr("Rotate Right")).clicked() {
        let _ = app.run("photo.rotateRight", json!({}));
    }
    ui.separator();
    // the original moved or its drive is gone: point the photo at the file again
    // (a cached answer, checked off the UI thread)
    let missing = matches!(&app.session.catalog.photo(id).map(|p| p.source.clone()), Some(lightcraft_catalog::Source::File { path }) if app.session.media.availability.is_offline(path));
    if missing && ui.button(crate::i18n::tr("Locate Missing File…")).clicked() {
        let _ = app.run("photo.locate", json!({}));
    }
    if ui.add_enabled(crate::menus::ui_enabled(app, "app.showInFinder"), egui::Button::new(crate::i18n::tr("Show in Finder"))).clicked() {
        let _ = app.run("app.showInFinder", json!({}));
    }
    if ui.button(crate::i18n::tr("Export…")).clicked() {
        let _ = app.run("dialog.export", json!({}));
    }
    ui.menu_button(crate::i18n::tr("Export with Preset"), |ui| {
        for (p, _) in app.session.all_export_presets() {
            if ui.button(&p.name).clicked() {
                match app.run("app.export", json!({"preset": p.name, "background": true})) {
                    Ok(r) if r.get("background").is_some() => {}
                    Ok(r) => {
                        let n = r["files"].as_array().map_or(0, Vec::len);
                        app.toast(ui.ctx(), crate::i18n::tr_format!("Exported {n} photo{}", if n == 1 { "" } else { "s" }, n = n));
                    }
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
    });
    ui.separator();
    if ui.button(crate::i18n::tr("Delete Photo")).clicked() {
        let _ = app.run("photo.delete", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ungrouped_justified_rows_fill_the_width() {
        let aspects = [1.5f32, 1.5, 0.66, 1.0, 1.5, 1.5, 1.5];
        let l = layout(&aspects, &[], 600.0, 150.0, false);
        assert!(l.headers.is_empty());
        assert_eq!(l.cells.len(), aspects.len());
        // full rows end at the right edge; rows are at most the target height
        let first_row: Vec<&Rect> = l.cells.iter().filter(|c| (c.top() - l.cells[0].top()).abs() < 0.01).collect();
        assert!(first_row.len() >= 2);
        assert!((first_row.last().unwrap().right() - 604.0).abs() < 0.5, "{first_row:?}");
        assert!(first_row.iter().all(|c| c.height() <= 150.0 + 0.01));
        let last = l.cells.last().unwrap();
        assert!(last.right() <= 604.0 + 0.5);
        assert!((l.height - (last.bottom() + 6.0)).abs() < 0.01);
    }

    #[test]
    fn groups_start_new_rows_below_their_headers() {
        let aspects = [1.5f32; 7];
        let l = layout(&aspects, &[(0, 2), (2, 5)], 1000.0, 200.0, false);
        assert_eq!(l.headers.len(), 2);
        assert_eq!(l.headers[0].top(), 4.0);
        assert_eq!(l.headers[0].height(), HEADER_H);
        // group 1: two photos in a short row below its header (not stretched)
        assert_eq!(l.cells[0].top(), l.headers[0].bottom());
        assert!((l.cells[0].height() - 200.0).abs() < 0.01);
        assert_eq!(l.cells[1].top(), l.cells[0].top());
        // group 2 starts on a new row below its own header, after group 1's photos
        assert!(l.headers[1].top() >= l.cells[1].bottom());
        assert_eq!(l.cells[2].top(), l.headers[1].bottom());
        assert!((l.cells[2].left() - 4.0).abs() < 0.01, "a group's first photo starts the row");
        for c in &l.cells[2..] {
            assert!(c.top() >= l.headers[1].bottom());
        }
        // every photo has a cell; nothing overlaps
        for (i, a) in l.cells.iter().enumerate() {
            assert!(a.width() > 0.0);
            for b in &l.cells[i + 1..] {
                assert!(!a.shrink(0.5).intersects(b.shrink(0.5)), "{a:?} {b:?}");
            }
            for h in &l.headers {
                assert!(!a.shrink(0.5).intersects(*h));
            }
        }
    }

    /// The row index covers every cell once, in order and top to bottom, so the visible cells
    /// are found by binary search.
    #[test]
    fn rows_index_every_cell_top_to_bottom() {
        let aspects: Vec<f32> = (0..500).map(|i| [1.5f32, 0.66, 1.0, 1.78][i % 4]).collect();
        let groups = [(0, 7), (7, 200), (207, 1), (208, 292)];
        for square in [false, true] {
            let l = layout(&aspects, &groups, 900.0, 140.0, square);
            let mut next = 0;
            for (k, row) in l.rows.iter().enumerate() {
                assert_eq!(row.start, next, "rows are contiguous");
                assert!(row.end > row.start);
                for c in &l.cells[row.start..row.end] {
                    assert!((c.top() - row.top).abs() < 0.01 && (c.bottom() - row.bottom).abs() < 0.01, "{c:?} {row:?}");
                }
                if let Some(prev) = k.checked_sub(1).map(|p| l.rows[p]) {
                    assert!(row.top > prev.bottom, "{prev:?} {row:?}");
                }
                next = row.end;
            }
            assert_eq!(next, aspects.len());
            // the binary search finds exactly the cells a linear scan does
            for (top, bottom) in [(0.0, 300.0), (1234.0, 2100.0), (l.height - 50.0, l.height + 900.0), (-500.0, -1.0)] {
                let band = Rect::from_min_max(pos2(0.0, top), pos2(2000.0, bottom));
                let want: Vec<usize> = (0..aspects.len()).filter(|i| l.cells[*i].intersects(band)).collect();
                let got: Vec<usize> = rows_between(&l.rows, top, bottom).iter().flat_map(|r| r.start..r.end).collect();
                assert_eq!(got, want, "square {square} {top}..{bottom}");
            }
        }
    }

    #[test]
    fn square_grid_groups_and_auto_levels() {
        let l = layout(&[1.0; 5], &[(0, 3), (3, 2)], 400.0, 100.0, true);
        // 4 columns: group 1 fills 3 cells of a row, group 2 starts a new row
        assert_eq!(l.cells[0].top(), l.cells[2].top());
        assert_eq!(l.cells[3].left(), 4.0);
        assert!(l.cells[3].top() > l.cells[0].bottom() + HEADER_H - 1.0);
        assert!((l.cells[0].width() - l.cells[0].height()).abs() < 0.01);
        assert_eq!(resolve_group(GroupBy::Auto, 220.0), GroupBy::Day);
        assert_eq!(resolve_group(GroupBy::Auto, 120.0), GroupBy::Month);
        assert_eq!(resolve_group(GroupBy::Auto, 90.0), GroupBy::Year);
        assert_eq!(resolve_group(GroupBy::None, 220.0), GroupBy::None);
        assert_eq!(resolve_group(GroupBy::Year, 400.0), GroupBy::Year);
    }
}
