//! The library couldn't be opened at launch (issue #100): never a silent in-memory session. A
//! blocking window says which library and why, and offers Try Again, Choose Another Library…,
//! Continue Without Saving and Quit. Continuing shows a banner under the top bar for as long as the
//! session is temporary.

use egui::{Align2, RichText, vec2};
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// A library that failed to open (set by the host at launch).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryProblem {
    /// The library folder ("" when there was none to try, e.g. no home folder).
    pub path: String,
    /// Why it didn't open.
    pub error: String,
    /// Continue Without Saving was chosen: the banner shows instead of the window.
    pub dismissed: bool,
    /// Files from the command line, imported once a library opens.
    pub pending_import: Vec<String>,
    /// Try Again reopens `path` as a library folder (`false` in the browser, whose storage isn't
    /// a folder: reloading the page tries again).
    pub can_retry: bool,
}

impl LibraryProblem {
    pub fn new(path: impl Into<String>, error: impl Into<String>) -> LibraryProblem {
        LibraryProblem { path: path.into(), error: error.into(), can_retry: true, ..Default::default() }
    }

    /// `ui.inspect` → `libraryProblem`.
    pub fn to_json(&self) -> serde_json::Value {
        json!({"path": self.path, "error": self.error, "temporarySession": self.dismissed, "pendingImport": self.pending_import.len()})
    }
}

/// What the user chose.
enum Choice {
    Retry,
    Choose,
    Temporary,
    Quit,
}

/// Open `path` (`None`: ask for a folder) through `app.openLibrary`; on success the problem is
/// over (and command-line files are imported), else it shows the new error.
fn open(app: &mut LightcraftApp, path: Option<String>) {
    let params = match &path {
        Some(p) => json!({"path": p}),
        None => json!({}),
    };
    match app.run("app.openLibrary", params) {
        Ok(serde_json::Value::Null) => {} // folder dialog cancelled
        Ok(_) => {
            let files = app.library_problem.take().map(|p| p.pending_import).unwrap_or_default();
            if !files.is_empty() {
                let _ = app.run("library.import", json!({"paths": files}));
            }
        }
        Err(e) => {
            if let Some(p) = app.library_problem.as_mut() {
                if let Some(path) = path {
                    p.path = path;
                }
                p.error = e;
                p.dismissed = false;
            }
        }
    }
}

/// Clear the problem once a library is open (e.g. through Settings → Open Library…).
pub fn logic(app: &mut LightcraftApp) {
    if app.library_problem.is_some() && app.session.library.is_some() {
        app.library_problem = None;
    }
}

/// The blocking window (until a choice is made).
pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    let Some(problem) = app.library_problem.clone() else { return };
    if problem.dismissed {
        return;
    }
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("library-problem-dim")).order(egui::Order::Middle).fixed_pos(screen.min).interactable(true).show(ctx, |ui| {
        // swallows clicks: nothing behind the window can be used meanwhile
        ui.allocate_rect(screen, egui::Sense::click_and_drag());
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
    });
    let mut choice = None;
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(18, 14));
    egui::Window::new(crate::i18n::tr("Your library couldn't be opened"))
        .id(egui::Id::new("library-problem"))
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(460.0)
        .show(ctx, |ui| {
            // a fixed width: a wrapped label in an auto-sized, centred window would move it every frame
            ui.set_width(440.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            if problem.path.is_empty() {
                ui.label(RichText::new(crate::i18n::tr("LightCraft couldn't find where to keep your library.")).color(t.text));
            } else {
                ui.label(RichText::new(crate::i18n::tr("LightCraft couldn't open the library at")).color(t.text_label));
                ui.label(RichText::new(&problem.path).font(t.semibold(12.5)).color(t.text));
            }
            ui.add(egui::Label::new(RichText::new(&problem.error).color(t.caution)).wrap());
            ui.label(
                RichText::new(
                    "Your photos and edits in it are untouched. You can try again (e.g. after closing another program that has it open \
                     or reconnecting its drive), open a different library or create a new one, or continue without a library — \
                     then nothing you do is saved.",
                )
                .color(t.text_dim),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut button = |ui: &mut egui::Ui, id: &str, label: &str, enabled: bool, c: Choice| {
                    let r = ui.add_enabled(enabled, egui::Button::new(crate::i18n::tr(label)).min_size(vec2(0.0, 26.0)));
                    register(ui.ctx(), format!("button:{id}"), r.rect);
                    if r.clicked() {
                        choice = Some(c);
                    }
                };
                button(ui, "libraryRetry", "Try Again", problem.can_retry && !problem.path.is_empty(), Choice::Retry);
                button(ui, "libraryChoose", "Choose Another Library…", app.services.pick_folder.is_some(), Choice::Choose);
                button(ui, "libraryTemporary", "Continue Without Saving", true, Choice::Temporary);
                button(ui, "libraryQuit", "Quit", true, Choice::Quit);
            });
        });
    match choice {
        Some(Choice::Retry) => open(app, Some(problem.path)),
        Some(Choice::Choose) => open(app, None),
        Some(Choice::Temporary) => {
            if let Some(p) = app.library_problem.as_mut() {
                p.dismissed = true;
            }
            let files = app.library_problem.as_mut().map(|p| std::mem::take(&mut p.pending_import)).unwrap_or_default();
            if !files.is_empty() {
                let _ = app.run("library.import", json!({"paths": files}));
            }
        }
        Some(Choice::Quit) => app.ui.quit = true,
        None => {}
    }
}

/// The banner under the top bar while the session is temporary (Continue Without Saving).
pub fn banner(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    if !app.library_problem.as_ref().is_some_and(|p| p.dismissed) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let mut reopen = false;
    egui::Panel::top("library-problem-banner")
        .exact_size(30.0)
        .frame(egui::Frame::NONE.fill(t.caution).inner_margin(egui::Margin::symmetric(12, 0)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let r = ui.label(
                    RichText::new(crate::i18n::tr("Temporary session — your library isn't open, and nothing you do here is saved."))
                        .font(t.semibold(12.5))
                        .color(t.canvas),
                );
                register(ui.ctx(), "indicator:temporarySession", r.rect);
                let b = ui.add(
                    egui::Button::new(RichText::new(crate::i18n::tr("Open Library…")).color(t.canvas)).fill(egui::Color32::from_black_alpha(40)),
                );
                register(ui.ctx(), "button:libraryReopen", b.rect);
                reopen = b.clicked();
            });
        });
    if reopen && let Some(p) = app.library_problem.as_mut() {
        p.dismissed = false;
    }
}
