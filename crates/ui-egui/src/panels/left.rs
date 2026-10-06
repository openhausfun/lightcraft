//! The left "My Photos" panel: library sources, albums tree, and date groups.

use egui::{Align2, Rect, Sense, pos2, vec2};
use lightcraft_catalog::{Album, AlbumId, KeywordNode};
use lightcraft_engine::LibrarySource;
use serde_json::json;

use crate::LightcraftApp;
use crate::icons::{Icon, paint};
use crate::theme::Tokens;
use crate::widgets::{icon_button, register};

fn row(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    id: &str,
    icon: Icon,
    label: &str,
    count: Option<usize>,
    selected: bool,
    indent: f32,
) -> egui::Response {
    let label = if matches!(id, "all" | "recentlyAdded" | "picks" | "missing" | "recentlyDeleted") { crate::i18n::tr(label) } else { label };
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 29.0), Sense::click());
    register(ui.ctx(), format!("source:{id}"), r);
    let name = match count {
        Some(n) => crate::i18n::tr_format!("{label}, {n} photos", label = label, n = n),
        None => label.to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
    let inner = r.shrink2(vec2(8.0, 0.0));
    if selected {
        ui.painter().rect_filled(inner, 4.0, t.canvas);
    } else if resp.hovered() {
        ui.painter().rect_filled(inner, 4.0, t.hover.gamma_multiply(0.6));
    }
    paint(
        ui.painter(),
        Rect::from_min_size(pos2(r.left() + 18.0 + indent, r.center().y - 8.0), vec2(16.0, 16.0)),
        icon,
        if selected { t.text } else { t.icon },
    );
    ui.painter().text(
        pos2(r.left() + 42.0 + indent, r.center().y),
        Align2::LEFT_CENTER,
        label,
        t.font(13.5),
        if selected { t.text } else { t.text_label },
    );
    if let Some(n) = count.filter(|_| app.ui.show_counts) {
        ui.painter().text(pos2(r.right() - 18.0, r.center().y), Align2::RIGHT_CENTER, n.to_string(), t.font(12.5), t.text_dim);
    }
    let _ = app;
    resp
}

pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.chrome).stroke(egui::Stroke::new(1.0, t.divider));
    let width = app.ui.left_width;
    let resized = super::resizable_side(ui, true, "left_panel", frame, width, crate::state::LEFT_WIDTH, 0.0, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        ui.painter().text(pos2(hr.left() + 18.0, hr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("My Photos"), t.semibold(15.0), t.text);
        let counts = app.caches.counts(&app.session.catalog);
        let (total, picks, deleted) = (counts.total, counts.picks, counts.deleted);
        egui::ScrollArea::vertical().id_salt("left-scroll").auto_shrink([false, false]).show(ui, |ui| {
            let src = app.session.source;
            for (id, icon, label, count, s) in [
                ("all", Icon::Photos, "All Photos", Some(total), LibrarySource::All),
                ("recentlyAdded", Icon::Clock, "Recently Added", None, LibrarySource::RecentlyAdded),
                ("picks", Icon::FlagPick, "Picks", Some(picks), LibrarySource::Picks),
            ] {
                if row(app, ui, id, icon, label, count, src == s, 0.0).clicked() {
                    let _ = app.run("library.source", json!({"kind": id}));
                }
            }
            // photos whose files can't be found (checked every few seconds, not every frame)
            let missing = missing_count(app, ui);
            if (missing > 0 || src == LibrarySource::Missing)
                && row(app, ui, "missing", Icon::Folder, "Missing Photos", Some(missing), src == LibrarySource::Missing, 0.0).clicked()
            {
                let _ = app.run("library.source", json!({"kind": "missing"}));
            }
            ui.add_space(10.0);
            // Albums header
            let (ar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
            ui.painter().text(pos2(ar.left() + 18.0, ar.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Albums"), t.semibold(13.5), t.text_label);
            let mut hdr = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(Rect::from_min_max(pos2(ar.right() - 50.0, ar.top()), ar.right_bottom()))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            let plus = icon_button(&mut hdr, "albumNew", Icon::Plus, vec2(26.0, 26.0), false, true, "Create Album");
            egui::Popup::menu(&plus).show(|ui| {
                if ui.button(crate::i18n::tr("Create Album…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewAlbum { name: String::new(), folder: false });
                }
                if ui.button(crate::i18n::tr("Create Smart Album…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::SmartRules {
                        id: None,
                        name: String::new(),
                        rules: lightcraft_catalog::RuleSet { rules: vec![crate::panels::rules_editor::new_rule()], ..Default::default() },
                    });
                }
                if ui.button(crate::i18n::tr("Create Smart Album from Filter…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewSmartAlbum { name: String::new() });
                }
                if ui.button(crate::i18n::tr("Create Folder…")).clicked() {
                    app.ui.dialog = Some(crate::state::Dialog::NewAlbum { name: String::new(), folder: true });
                }
            });
            let albums: Vec<Album> = app.session.catalog.albums().cloned().collect();
            albums_tree(app, ui, &albums, None, 0.0);
            ui.add_space(10.0);
            local_section(app, ui);
            // By date
            let (dr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
            ui.painter().text(pos2(dr.left() + 18.0, dr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("By Date"), t.semibold(13.5), t.text_label);
            for g in app.caches.date_groups(&app.session.catalog).iter() {
                // year → month → day; a click filters by that prefix, the triangle opens a level
                if date_row(app, ui, &g.year, &g.year, g.count, 0.0) {
                    for (m, n) in &g.months {
                        let label = lightcraft_catalog::dates::group_label(m).split(' ').next().unwrap_or(m).to_string();
                        if date_row(app, ui, m, &label, *n, 16.0) {
                            for (d, n) in g.days.iter().filter(|(d, _)| d.starts_with(m.as_str())) {
                                let label = lightcraft_catalog::dates::group_label(d);
                                // "Sunday, 20 September 2026" → "Sunday, 20"
                                let label = label.rsplitn(3, ' ').nth(2).unwrap_or(&label).to_string();
                                date_row(app, ui, d, &label, *n, 32.0);
                            }
                        }
                    }
                }
            }
            keywords_section(app, ui);
            ui.add_space(10.0);
            if row(app, ui, "recentlyDeleted", Icon::Trash, "Recently Deleted", Some(deleted), src == LibrarySource::RecentlyDeleted, 0.0).clicked() {
                let _ = app.run("library.source", json!({"kind": "recentlyDeleted"}));
            }
        });
    });
    if let Some(w) = resized {
        app.ui.left_width = w;
    }
}

/// How many library photos have no file (Local browse records are not checked; see
/// `cmd::missing::checked_path`). Checking stats every file, which on a network share takes
/// seconds, so it runs on a worker thread: the count shown is the last finished one, refreshed
/// at most every 5 s, and at once (after the running check) when the catalog changed.
fn missing_count(app: &mut LightcraftApp, ui: &mut egui::Ui) -> usize {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    #[derive(Clone, Default)]
    struct Job {
        n: std::sync::Arc<AtomicUsize>,
        running: std::sync::Arc<AtomicBool>,
        at: f64,
        rev: u64,
    }
    let id = egui::Id::new("missing-count");
    let now = ui.input(|i| i.time);
    let rev = app.session.catalog.revision;
    let mut job: Job = ui.data(|d| d.get_temp(id)).unwrap_or(Job { rev: u64::MAX, at: f64::MIN, ..Default::default() });
    let fresh = now - job.at < 5.0 && job.rev == rev;
    if !fresh && !job.running.load(Relaxed) {
        // the same scope as the Missing Photos view: library photos only, never Local browse records
        let paths = lightcraft_engine::cmd::missing::candidates(&app.session.catalog);
        job.running.store(true, Relaxed);
        job.at = now;
        job.rev = rev;
        let (n, running, ctx) = (job.n.clone(), job.running.clone(), ui.ctx().clone());
        let work = move || {
            let missing = if cfg!(target_arch = "wasm32") { 0 } else { paths.iter().filter(|p| !std::path::Path::new(p).exists()).count() };
            n.store(missing, Relaxed);
            running.store(false, Relaxed);
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(work);
        #[cfg(target_arch = "wasm32")]
        work();
        ui.data_mut(|d| d.insert_temp(id, job.clone()));
    }
    job.n.load(Relaxed)
}

/// Folders on this computer to browse without adding (Lightroom's Local): Pictures, Desktop,
/// Downloads, the home folder, the folder being browsed, and Browse Folder….
fn local_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let (lr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    ui.painter().text(pos2(lr.left() + 18.0, lr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Local"), t.semibold(13.5), t.text_label);
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let mut builtin: Vec<(String, String)> = Vec::new();
    if !home.is_empty() {
        for (name, sub) in [("Pictures", "Pictures"), ("Desktop", "Desktop"), ("Downloads", "Downloads"), ("Home", "")] {
            // joined with the platform's separator, like the paths browsing gives back
            let p = if sub.is_empty() { home.clone() } else { std::path::Path::new(&home).join(sub).to_string_lossy().to_string() };
            // (checked off the UI thread: a home folder can be on a network share)
            if fs_cached(ui, "is-dir", &p, 5.0, |p| std::path::Path::new(p).is_dir()) == Some(true) {
                builtin.push((name.to_string(), p));
            }
        }
    }
    let browsing = app.session.browse.clone().filter(|_| app.session.source == LibrarySource::Folder);
    let current = browsing.as_ref().map(|b| b.path.clone());
    let local = local_places(builtin, &app.ui.local_roots, current.as_deref(), app.ui.local_browse_root.as_deref(), &app.ui.hidden_locations);
    if current.is_some() {
        app.ui.local_browse_root = local.browse_root.clone();
    }
    for (i, (name, path)) in local.places.iter().enumerate() {
        let transient = local.browse_root.as_deref() == Some(path.as_str());
        let reveal = local.owner == Some(i);
        folder_tree(app, ui, name, path, 0.0, current.as_deref(), reveal, transient);
    }
    if app.services.pick_folder.is_some() && row(app, ui, "local:browse", Icon::Plus, "Browse Folder…", None, false, 0.0).clicked() {
        let picked = app.services.pick_folder.as_mut().and_then(|f| f());
        if let Some(path) = picked {
            match app.run("library.browse", json!({"path": path})) {
                // the picked folder stays in Local (and comes back if it was hidden)
                Ok(r) => {
                    let dir = r["path"].as_str().unwrap_or(&path).to_string();
                    let _ = app.run("local.addRoot", json!({"path": dir}));
                }
                Err(e) => app.toast(ui.ctx(), e),
            }
        }
    }
    let hidden = app.ui.hidden_locations.len();
    if hidden > 0 {
        let label = crate::i18n::tr_format!("Show {hidden} hidden location{}", if hidden == 1 { "" } else { "s" }, hidden = hidden);
        if row(app, ui, "local:restoreHidden", Icon::Folder, &label, None, false, 0.0)
            .on_hover_text(crate::i18n::tr("Put the locations you removed from Local back (no files change)"))
            .clicked()
        {
            let _ = app.run("local.restoreHidden", json!({}));
        }
    }
    ui.add_space(10.0);
}

/// Whether two paths name the same folder, however they are spelled (separators, trailing
/// slash, `.`/`..`, drive-letter case; see `lightcraft_catalog::query::folder_key`).
pub(crate) fn same_folder(a: &str, b: &str) -> bool {
    a == b || lightcraft_catalog::query::folder_key(a) == lightcraft_catalog::query::folder_key(b)
}

/// Local's top-level folders and how the folder being browsed sits among them.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct LocalPlaces {
    /// (label, path) of each top-level folder, in order.
    pub places: Vec<(String, String)>,
    /// The top-level folder the browsed folder lies in (the innermost one): its tree opens on
    /// the way down to it.
    pub owner: Option<usize>,
    /// A folder listed only for this session because the browsed folder is in no saved
    /// location (browsed from a breadcrumb, the CLI…); it stays while browsing below it.
    pub browse_root: Option<String>,
}

fn folder_label(path: &str) -> String {
    std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string())
}

/// Local's top-level folders, in order: the built-in places, the folders kept with Browse
/// Folder… / Keep in Local (`saved`), then — only when the browsed folder lies in none of those —
/// a session root for it: the previous one (`browse_root`) while browsing stays below it, else
/// the browsed folder itself. A folder inside a listed one is shown inside that one's tree (the
/// root stays; its siblings stay reachable), never as a root of its own. Each folder is listed
/// once however its path is spelled (the first spelling wins), and hidden ones are left out —
/// they stay reachable through breadcrumbs and Browse Folder….
pub(crate) fn local_places(
    builtin: Vec<(String, String)>,
    saved: &[String],
    browsing: Option<&str>,
    browse_root: Option<&str>,
    hidden: &[String],
) -> LocalPlaces {
    use lightcraft_catalog::query::{folder_key, folder_within};
    let is_hidden = |p: &str| hidden.iter().any(|h| same_folder(h, p));
    let mut places = builtin;
    for path in saved {
        if !places.iter().any(|(_, p)| same_folder(p, path)) {
            places.push((folder_label(path), path.clone()));
        }
    }
    places.retain(|(_, p)| !is_hidden(p));
    let mut out = LocalPlaces::default();
    if let Some(c) = browsing
        && !places.iter().any(|(_, p)| folder_within(c, p))
    {
        let root = browse_root.filter(|r| folder_within(c, r)).unwrap_or(c);
        if !is_hidden(root) {
            places.push((folder_label(root), root.to_string()));
            out.browse_root = Some(root.to_string());
        }
    }
    if let Some(c) = browsing {
        out.owner = places.iter().enumerate().filter(|(_, (_, p))| folder_within(c, p)).max_by_key(|(_, (_, p))| folder_key(p).len()).map(|(i, _)| i);
    }
    out.places = places;
    out
}

/// The subfolders of `path` (not hidden ones), sorted; listed on a worker thread at most every
/// 2 s per folder (empty until the first listing).
fn subfolders(ui: &egui::Ui, path: &str) -> Vec<(String, String)> {
    fs_cached(ui, "subfolders", path, 2.0, list_subfolders).unwrap_or_default()
}

fn list_subfolders(path: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    (!name.starts_with('.')).then(|| (name, e.path().to_string_lossy().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// A file-system answer for `path` (`f(path)`), kept per `kind` and path and refreshed on a worker
/// thread at most every `every` seconds: a folder on a sleeping NAS, a dropped share or a
/// spinning-up drive never blocks a frame. `None` until the first answer arrives.
pub(crate) fn fs_cached<T: Clone + Send + 'static>(ui: &egui::Ui, kind: &'static str, path: &str, every: f64, f: fn(&str) -> T) -> Option<T> {
    struct Entry<T> {
        value: Option<T>,
        at: Option<f64>,
        running: bool,
    }
    type Cell<T> = std::sync::Arc<std::sync::Mutex<Entry<T>>>;
    let id = egui::Id::new(("fs-cached", kind, path.to_string()));
    let now = ui.input(|i| i.time);
    let cell: Cell<T> = match ui.data(|d| d.get_temp::<Cell<T>>(id)) {
        Some(c) => c,
        None => {
            let c: Cell<T> = std::sync::Arc::new(std::sync::Mutex::new(Entry { value: None, at: None, running: false }));
            ui.data_mut(|d| d.insert_temp(id, c.clone()));
            c
        }
    };
    let lock = |c: &Cell<T>| c.lock().unwrap_or_else(std::sync::PoisonError::into_inner).value.clone();
    let start = {
        let mut e = cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let due = !e.running && e.at.is_none_or(|at| now - at >= every || now < at);
        if due {
            e.running = true;
            e.at = Some(now);
        }
        due
    };
    if start {
        let (out, path, repaint) = (cell.clone(), path.to_string(), ui.ctx().clone());
        let work = move || {
            let v = f(&path);
            let mut e = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            e.value = Some(v);
            e.running = false;
            drop(e);
            repaint.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("lc-fs-list".into()).spawn(work).is_err() {
            cell.lock().unwrap_or_else(std::sync::PoisonError::into_inner).running = false;
        }
        #[cfg(target_arch = "wasm32")]
        work();
    }
    lock(&cell)
}

/// A folder on disk with a disclosure triangle: click browses it, the triangle lists its
/// subfolders. In the tree that holds the folder being browsed (`reveal`), the folders on the
/// way down to it open whenever the browsed folder changes, so it shows highlighted in place.
/// `transient`: a top-level row listed for this session only (it offers Keep in Local).
#[allow(clippy::too_many_arguments)]
fn folder_tree(
    app: &mut LightcraftApp,
    ui: &mut egui::Ui,
    name: &str,
    path: &str,
    indent: f32,
    current: Option<&str>,
    reveal: bool,
    transient: bool,
) {
    let t = Tokens::get(ui.ctx());
    let open_id = egui::Id::new(("folder-open", path.to_string()));
    let sel = current.is_some_and(|c| same_folder(c, path));
    let on_the_way = !sel && current.is_some_and(|c| lightcraft_catalog::query::folder_within(c, path));
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    if reveal && on_the_way {
        // opened once per browsed folder: collapsing it again afterwards sticks
        let revealed_id = egui::Id::new(("folder-revealed", path.to_string()));
        let target = current.map(str::to_string);
        if ui.data(|d| d.get_temp::<Option<String>>(revealed_id)) != Some(target.clone()) {
            open = true;
            ui.data_mut(|d| {
                d.insert_temp(open_id, true);
                d.insert_temp(revealed_id, target);
            });
        }
    }
    let resp = row(app, ui, &format!("local:{path}"), Icon::Folder, name, None, sel, indent + 12.0).on_hover_text(path);
    let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
    let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
    let tr = ui.interact(tri, egui::Id::new(("folder-tri", path.to_string())), Sense::click());
    register(ui.ctx(), format!("folderToggle:{path}"), tri);
    let col = if tr.hovered() { t.text } else { t.text_dim };
    let pts = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
    if tr.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_temp(open_id, open));
    } else if resp.clicked()
        && let Err(e) = app.run("library.browse", json!({"path": path}))
    {
        app.toast(ui.ctx(), e);
    }
    resp.context_menu(|ui| {
        if (transient || indent > 0.0)
            && ui
                .button(if transient { "Keep in Local" } else { "Add to Local" })
                .on_hover_text(crate::i18n::tr("List this folder in Local from now on"))
                .clicked()
        {
            let _ = app.run("local.addRoot", json!({"path": path}));
            ui.close();
        }
        if indent == 0.0
            && ui
                .button(crate::i18n::tr("Remove from Local"))
                .on_hover_text(crate::i18n::tr("Hides this shortcut only; the folder and its photos stay as they are"))
                .clicked()
        {
            let _ = app.run("local.hide", json!({"path": path}));
            ui.close();
        }
        if ui.button(crate::i18n::tr("Rename Folder…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::TextPrompt {
                title: crate::i18n::tr_format!("Rename “{name}”", name = name),
                hint: "Folder name (renamed on disk; its photos follow)".into(),
                value: name.to_string(),
                command: "folder.rename".into(),
                params: json!({"path": path}),
                key: "name".into(),
            });
        }
        if app.services.pick_folder.is_some() && ui.button(crate::i18n::tr("Move Folder To…")).clicked() {
            let into = app.services.pick_folder.as_mut().and_then(|f| f());
            if let Some(into) = into {
                match app.run("folder.move", json!({"path": path, "into": into})) {
                    Ok(r) => app.toast(ui.ctx(), format!("Moved; {} photo(s) relinked", r["relinked"])),
                    Err(e) => app.toast(ui.ctx(), e),
                }
            }
        }
        if app.services.reveal.is_some()
            && ui.button(crate::i18n::tr("Show in Finder")).clicked()
            && let Some(f) = app.services.reveal.as_mut()
        {
            let _ = f(path);
        }
    });
    if open && indent < 12.0 * 8.0 {
        for (n, p) in subfolders(ui, path) {
            folder_tree(app, ui, &n, &p, indent + 12.0, current, reveal, false);
        }
    }
}

/// One By Date row (`key`: `YYYY`, `YYYY-MM` or `YYYY-MM-DD`); returns whether it is open.
fn date_row(app: &mut LightcraftApp, ui: &mut egui::Ui, key: &str, label: &str, count: usize, indent: f32) -> bool {
    let t = Tokens::get(ui.ctx());
    let open_id = egui::Id::new(("date-open", key.to_string()));
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    let sel = app.session.filter.date.as_deref() == Some(key);
    let resp = row(app, ui, &format!("date:{key}"), Icon::Clock, label, Some(count), sel, indent);
    if key.len() < 10 {
        let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
        let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
        let tr = ui.interact(tri, egui::Id::new(("date-tri", key.to_string())), Sense::click());
        register(ui.ctx(), format!("dateToggle:{key}"), tri);
        let col = if tr.hovered() { t.text } else { t.text_dim };
        let pts = if open {
            vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
        } else {
            vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
        };
        ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
        if tr.clicked() {
            open = !open;
            ui.data_mut(|d| d.insert_temp(open_id, open));
        }
    }
    if resp.clicked() {
        let v = if sel { serde_json::Value::Null } else { json!(key) };
        let _ = app.run("library.filter", json!({"date": v}));
    }
    open
}

fn albums_tree(app: &mut LightcraftApp, ui: &mut egui::Ui, all: &[Album], parent: Option<AlbumId>, indent: f32) {
    let mut kids: Vec<&Album> = all.iter().filter(|a| a.parent == parent).collect();
    kids.sort_by_key(|a| (!a.folder, a.name.to_lowercase()));
    for a in kids {
        if a.folder {
            let open_id = egui::Id::new(("folder-open", a.id.0));
            let open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(true);
            let resp = row(app, ui, &format!("folder:{}", a.id.0), Icon::Folder, &a.name, None, false, indent);
            if resp.clicked() {
                ui.data_mut(|d| d.insert_temp(open_id, !open));
            }
            folder_menu(app, &resp, a);
            if open {
                albums_tree(app, ui, all, Some(a.id), indent + 16.0);
            }
        } else {
            let sel = app.session.source == LibrarySource::Album(a.id);
            let icon = if a.is_smart() { Icon::SmartAlbum } else { Icon::Album };
            // cached: a smart album's count scans the catalog
            let now = (app.session.clock)();
            let n = app.caches.album_counts(&app.session.catalog, &now).get(&a.id).copied().unwrap_or(0);
            // the album B adds to is marked "+"
            let target =
                app.session.target_album.filter(|t| app.session.catalog.album(*t).is_some()).or_else(|| app.session.catalog.quick_collection());
            let label = if target == Some(a.id) { format!("{} +", a.name) } else { a.name.clone() };
            let mut resp = row(app, ui, &format!("album:{}", a.id.0), icon, &label, Some(n), sel, indent);
            if !a.is_smart() {
                drop_target(app, ui, &resp, a);
            }
            if let Some(rules) = &a.smart {
                resp = resp.on_hover_text(crate::i18n::tr_format!("Smart album: {}", rules.describe()));
            }
            if resp.clicked() {
                let _ = app.run("library.source", json!({"kind": "album", "id": a.id.0}));
            }
            folder_menu(app, &resp, a);
        }
    }
}

/// An album row while photos are dragged from the grid: highlighted under the pointer; a
/// release there adds them.
fn drop_target(app: &mut LightcraftApp, ui: &mut egui::Ui, resp: &egui::Response, a: &Album) {
    let Some(ids) = app.ui.dragging_photos.clone() else { return };
    let over = ui.input(|i| i.pointer.latest_pos()).is_some_and(|p| resp.rect.contains(p));
    if !over {
        return;
    }
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_stroke(resp.rect.shrink2(vec2(8.0, 1.0)), 4.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    if ui.input(|i| i.pointer.any_released()) {
        let n = ids.len();
        match app.run("album.addPhotos", json!({"id": a.id.0, "ids": ids})) {
            Ok(_) => app.toast(ui.ctx(), crate::i18n::tr_format!("Added {n} photo{} to “{}”", if n == 1 { "" } else { "s" }, a.name, n = n)),
            Err(e) => app.toast(ui.ctx(), e),
        }
        app.ui.dragging_photos = None;
    }
}

fn folder_menu(app: &mut LightcraftApp, resp: &egui::Response, a: &Album) {
    resp.context_menu(|ui| {
        if !a.folder && !a.is_smart() && ui.button(crate::i18n::tr("Add Selected Photos")).clicked() {
            let _ = app.run("album.addPhotos", json!({"id": a.id.0}));
        }
        if !a.folder && !a.is_smart() {
            let is_target = app.session.target_album == Some(a.id) || (app.session.target_album.is_none() && a.quick);
            if !is_target && ui.button(crate::i18n::tr("Set as Target Album (B adds to it)")).clicked() {
                let _ = app.run("album.setTarget", json!({"id": if a.quick { serde_json::Value::Null } else { json!(a.id.0) }}));
            }
            if is_target && !a.quick && ui.button(crate::i18n::tr("Stop Using as Target Album")).clicked() {
                let _ = app.run("album.setTarget", json!({"id": null}));
            }
        }
        if a.quick && ui.button(crate::i18n::tr("Clear Quick Collection")).clicked() {
            let _ = app.run("album.clearQuick", json!({}));
        }
        if a.is_smart() && ui.button(crate::i18n::tr("Edit Smart Album…")).clicked() {
            // older smart albums keep their filter fields; the editor works on the rule set
            let rules = a.smart.as_ref().and_then(|f| f.rule_set.clone()).unwrap_or_default();
            app.ui.dialog = Some(crate::state::Dialog::SmartRules { id: Some(a.id.0), name: a.name.clone(), rules });
        }
        if a.is_smart() && ui.button(crate::i18n::tr("Update Rules from Current Filter")).clicked() {
            let _ = app.run("album.setRules", json!({"id": a.id.0, "fromView": true}));
        }
        if !a.folder {
            // export: show the album, select its photos, then the dialog / a preset
            let show_all = |app: &mut LightcraftApp| {
                let _ = app.run("library.source", json!({"kind": "album", "id": a.id.0}));
                let _ = app.run("library.selectAll", json!({}));
            };
            let has_photos = app.session.catalog.album_count(a.id) > 0;
            if ui.add_enabled(has_photos, egui::Button::new(crate::i18n::tr("Export Album…"))).clicked() {
                show_all(app);
                let _ = app.run("dialog.export", json!({}));
            }
            ui.add_enabled_ui(has_photos, |ui| {
                ui.menu_button(crate::i18n::tr("Export Album with Preset"), |ui| {
                    for (p, _) in app.session.all_export_presets() {
                        if ui.button(&p.name).clicked() {
                            show_all(app);
                            if let Err(e) = app.run("app.export", json!({"preset": p.name, "background": true})) {
                                app.toast(ui.ctx(), e);
                            }
                        }
                    }
                });
            });
            ui.separator();
        }
        // move into another folder (not into itself or one of its own subfolders)
        let mut folders: Vec<(u64, String)> =
            app.session.catalog.albums().filter(|f| f.folder && !is_within(app, f.id, a.id)).map(|f| (f.id.0, f.name.clone())).collect();
        folders.sort_by_key(|(_, n)| n.to_lowercase());
        ui.menu_button(crate::i18n::tr("Move to"), |ui| {
            if ui.add_enabled(a.parent.is_some(), egui::Button::new(crate::i18n::tr("Top Level"))).clicked() {
                let _ = app.run("album.move", json!({"id": a.id.0, "parent": null}));
            }
            for (fid, name) in &folders {
                if ui.add_enabled(a.parent.map(|p| p.0) != Some(*fid), egui::Button::new(name)).clicked() {
                    let _ = app.run("album.move", json!({"id": a.id.0, "parent": fid}));
                }
            }
        });
        if ui.button(crate::i18n::tr("Rename…")).clicked() {
            app.ui.dialog = Some(crate::state::Dialog::RenameAlbum { id: a.id.0, name: a.name.clone() });
        }
        if ui.button(crate::i18n::tr("Delete")).clicked() {
            let _ = app.run("album.delete", json!({"id": a.id.0}));
        }
    });
}

/// Whether `id` is `ancestor` or lies inside it.
fn is_within(app: &LightcraftApp, id: lightcraft_catalog::AlbumId, ancestor: lightcraft_catalog::AlbumId) -> bool {
    let mut cur = Some(id);
    let mut guard = 0;
    while let Some(c) = cur {
        if c == ancestor {
            return true;
        }
        cur = app.session.catalog.album(c).and_then(|x| x.parent);
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    false
}

/// "Keywords": the library's keyword tree with photo counts (`a|b|c` keywords nest). A click
/// filters the grid by the keyword (children included), the triangle opens a level, and the
/// context menu renames, merges or deletes the keyword across the library.
fn keywords_section(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let tree = app.caches.keyword_tree(&app.session.catalog);
    if tree.is_empty() {
        return;
    }
    ui.add_space(10.0);
    let (kr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    ui.painter().text(pos2(kr.left() + 18.0, kr.center().y), Align2::LEFT_CENTER, crate::i18n::tr("Keywords"), t.semibold(13.5), t.text_label);
    keyword_rows(app, ui, &tree, 0.0);
}

fn keyword_rows(app: &mut LightcraftApp, ui: &mut egui::Ui, nodes: &[KeywordNode], indent: f32) {
    let t = Tokens::get(ui.ctx());
    for n in nodes {
        let open_id = egui::Id::new(("kw-open", n.path.to_lowercase()));
        let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
        let sel = app.session.filter.keyword.as_deref().is_some_and(|k| k.eq_ignore_ascii_case(&n.path));
        let resp = row(app, ui, &format!("keyword:{}", n.path), Icon::Tag, &n.name, Some(n.count), sel, indent);
        if !n.children.is_empty() {
            // disclosure triangle left of the icon
            let c = pos2(resp.rect.left() + 10.0 + indent, resp.rect.center().y);
            let tri = Rect::from_center_size(c, vec2(14.0, 14.0));
            let tr = ui.interact(tri, egui::Id::new(("kw-tri", n.path.to_lowercase())), Sense::click());
            register(ui.ctx(), format!("keywordToggle:{}", n.path), tri);
            let col = if tr.hovered() { t.text } else { t.text_dim };
            let pts = if open {
                vec![c + vec2(-4.0, -2.0), c + vec2(4.0, -2.0), c + vec2(0.0, 3.0)]
            } else {
                vec![c + vec2(-2.0, -4.0), c + vec2(3.0, 0.0), c + vec2(-2.0, 4.0)]
            };
            ui.painter().add(egui::Shape::convex_polygon(pts, col, egui::Stroke::NONE));
            if tr.clicked() {
                open = !open;
                ui.data_mut(|d| d.insert_temp(open_id, open));
            }
        }
        let resp = resp.on_hover_text(if n.children.is_empty() { n.path.clone() } else { format!("{} (includes the keywords below it)", n.path) });
        if resp.clicked() {
            let v = if sel { serde_json::Value::Null } else { json!(n.path) };
            let _ = app.run("library.filter", json!({"keyword": v}));
        }
        resp.context_menu(|ui| {
            let has_sel = app.session.active().is_some();
            if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Add to Selected Photos"))).clicked() {
                let _ = app.run("photo.setMeta", json!({"addKeywords": [n.path]}));
            }
            if ui.add_enabled(has_sel, egui::Button::new(crate::i18n::tr("Remove from Selected Photos"))).clicked() {
                let _ = app.run("photo.setMeta", json!({"removeKeywords": [n.path]}));
            }
            ui.separator();
            if ui.button(crate::i18n::tr("Rename Keyword…")).clicked() {
                app.ui.dialog = Some(crate::state::Dialog::RenameKeyword { from: n.path.clone(), to: n.path.clone() });
            }
            if ui.button(crate::i18n::tr("Merge into…")).clicked() {
                app.ui.dialog = Some(crate::state::Dialog::MergeKeywords { from: vec![n.path.clone()], into: String::new() });
            }
            if ui.button(crate::i18n::tr("Delete Keyword")).clicked() {
                let _ = app.run("keyword.delete", json!({"keyword": n.path}));
            }
        });
        if open && !n.children.is_empty() {
            keyword_rows(app, ui, &n.children, indent + 16.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::local_places;

    fn names(v: &[(String, String)]) -> Vec<&str> {
        v.iter().map(|(n, _)| n.as_str()).collect()
    }

    /// A built-in location browsed (or picked) under another spelling of its path is the same
    /// row, not a second root — for every built-in, not just named ones.
    #[test]
    fn equivalent_paths_are_one_local_location() {
        for (home, sep) in [("D:\\Example", '\\'), ("D:/Example", '/'), ("/home/example", '/')] {
            let builtin: Vec<(String, String)> =
                ["Pictures", "Desktop", "Downloads"].iter().map(|s| (s.to_string(), format!("{home}{sep}{s}"))).collect();
            for (_, p) in builtin.clone() {
                // the browsed spelling: other separators, a trailing one, a different drive-letter case
                let flipped = p.replace(['/', '\\'], if sep == '/' { "\\" } else { "/" });
                for browsing in [flipped.clone(), format!("{p}{sep}"), p.replacen("D:", "d:", 1)] {
                    let l = local_places(builtin.clone(), std::slice::from_ref(&browsing), Some(&browsing), None, &[]);
                    assert_eq!(names(&l.places), ["Pictures", "Desktop", "Downloads"], "{p} browsed as {browsing}");
                    assert_eq!(l.places[l.owner.unwrap()].1, p, "the built-in row is the one highlighted");
                }
                // hiding under one spelling hides the other
                let l = local_places(builtin.clone(), &[], None, None, std::slice::from_ref(&flipped));
                assert_eq!(l.places.len(), 2, "hidden {flipped}");
            }
        }
        // a different folder is still added
        let l = local_places(vec![("Pictures".into(), "/home/example/Pictures".into())], &[], Some("/home/example/Pictures2"), None, &[]);
        assert_eq!(names(&l.places), ["Pictures", "Pictures2"]);
    }

    /// A folder inside a kept root is shown inside that root's tree: the root stays (its other
    /// subfolders stay reachable) and no second root appears; other kept roots stay too.
    #[test]
    fn browsing_below_a_kept_root_keeps_the_root() {
        let photos = "/data/Photos".to_string();
        let other = "/data/Scans".to_string();
        let saved = [photos.clone(), other.clone()];
        let builtin = || vec![("Home".to_string(), "/home/example".to_string())];
        for browsing in ["/data/Photos/2026/20260101", "/data/Photos/2026", "/data/Photos", "D:\\x"] {
            let l = local_places(builtin(), &saved, Some(browsing), None, &[]);
            let roots: Vec<&str> = l.places.iter().map(|(_, p)| p.as_str()).collect();
            if browsing.starts_with("/data") {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans"], "{browsing}");
                assert_eq!(l.owner, Some(1), "the Photos tree opens down to {browsing}");
                assert_eq!(l.browse_root, None);
            } else {
                assert_eq!(roots, ["/home/example", "/data/Photos", "/data/Scans", "D:\\x"], "a folder outside them gets a row");
            }
        }
        // the innermost containing root is the one that opens
        let l = local_places(builtin(), &["/home/example/Pictures".into()], Some("/home/example/Pictures/Trip"), None, &[]);
        assert_eq!(l.owner, Some(1));
    }

    /// A browsed folder outside every kept root gets a session row, which stays while browsing
    /// below it and gives way when browsing moves elsewhere.
    #[test]
    fn session_root_stays_while_browsing_below_it() {
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/t/base"));
        let l = local_places(Vec::new(), &[], Some("/t/base/Trip/Day 1"), l.browse_root.as_deref(), &[]);
        assert_eq!((l.places.len(), l.browse_root.as_deref(), l.owner), (1, Some("/t/base"), Some(0)), "{l:?}");
        let l = local_places(Vec::new(), &[], Some("/elsewhere"), l.browse_root.as_deref(), &[]);
        assert_eq!(l.browse_root.as_deref(), Some("/elsewhere"));
        // hidden: no row at all
        let l = local_places(Vec::new(), &[], Some("/t/base"), None, &["/t/base/".into()]);
        assert!(l.places.is_empty() && l.browse_root.is_none());
    }
}
