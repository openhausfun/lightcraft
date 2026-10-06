//! Things the user must not miss (issue #103): settings files that were damaged or unreadable when
//! the library opened (and host warnings such as a damaged `ui.json`), and quitting while changes
//! are only in memory.

use egui::{Align2, RichText, vec2};

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

/// Collect the library's settings-file warnings (each is shown once).
pub fn logic(app: &mut LightcraftApp) {
    let new = app.session.take_library_warnings();
    app.notices.extend(new);
}

/// The window may close now (`true`), or the quit prompt is shown (`false`): changes that
/// couldn't be written are retried first — the log, then a snapshot of everything in memory.
pub fn may_close(app: &mut LightcraftApp) -> bool {
    if app.quit_confirmed || app.session.unsaved().is_none() {
        return true;
    }
    let _ = app.session.persist();
    if app.session.unsaved().is_some()
        && let Err(e) = app.session.close_library()
    {
        log::error!("quit: library not saved: {e}");
    }
    match app.session.unsaved() {
        None => true,
        Some((n, e)) => {
            app.quit_prompt = Some(format!(
                "{} couldn't be written to disk: {e}\nIf you quit now, {} lost.",
                if n == 1 { "1 change".to_string() } else { format!("{n} changes") },
                if n == 1 { "it is" } else { "they are" }
            ));
            false
        }
    }
}

enum Choice {
    Retry,
    QuitAnyway,
    Cancel,
    Ok,
}

fn window(ctx: &egui::Context, id: &str, title: &str, text: &str, buttons: &[(&str, &str, Choice)]) -> Option<usize> {
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new((id, "dim"))).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.allocate_rect(screen, egui::Sense::click_and_drag());
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
    });
    let mut chosen = None;
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(egui::Margin::symmetric(18, 14));
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(440.0)
        .show(ctx, |ui| {
            // a fixed width: a wrapped label in an auto-sized, centred window would move it every frame
            ui.set_width(420.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add(egui::Label::new(RichText::new(text).color(t.text_label)).wrap());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (i, (bid, label, _)) in buttons.iter().enumerate() {
                    let r = ui.add(egui::Button::new(*label).min_size(vec2(0.0, 26.0)));
                    register(ui.ctx(), format!("button:{bid}"), r.rect);
                    if r.clicked() {
                        chosen = Some(i);
                    }
                }
            });
        });
    chosen
}

pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if let Some(text) = app.quit_prompt.clone() {
        let buttons = [
            ("quitRetry", "Try Saving Again", Choice::Retry),
            ("quitAnyway", "Quit Anyway", Choice::QuitAnyway),
            ("quitCancel", "Cancel", Choice::Cancel),
        ];
        let Some(i) = window(ctx, "quit-unsaved", "Quit with unsaved changes?", &text, &buttons) else { return };
        match buttons[i].2 {
            Choice::Retry => {
                app.quit_prompt = None;
                if may_close(app) {
                    app.ui.quit = true;
                }
            }
            Choice::QuitAnyway => {
                app.quit_prompt = None;
                app.quit_confirmed = true;
                app.ui.quit = true;
            }
            Choice::Cancel | Choice::Ok => app.quit_prompt = None,
        }
        return;
    }
    if let Some(text) = app.notices.first().cloned() {
        let buttons = [("noticeOk", "OK", Choice::Ok)];
        if window(ctx, "settings-notice", "Settings file problem", &text, &buttons).is_some() {
            app.notices.remove(0);
        }
    }
}
