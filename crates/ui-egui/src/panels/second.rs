//! The second window (Window ▸ Second Window): the active photo, fitted, in its own window —
//! for a second display while the main window shows the grid or the tools.

use egui::{Color32, Rect, pos2, vec2};

use crate::LightcraftApp;
use crate::render::Slot;

pub fn show(app: &mut LightcraftApp, ctx: &egui::Context) {
    if !app.ui.second_window {
        return;
    }
    let vid = egui::ViewportId::from_hash_of("lightcraft-second-window");
    let builder = egui::ViewportBuilder::default().with_title("LightCraft — Second Window").with_inner_size([960.0, 640.0]);
    ctx.show_viewport_immediate(vid, builder, |ctx, class| {
        // (without native windows — web, headless — egui wraps this in a floating window)
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::BLACK)).show(ctx, |ui| body(app, ui));
        if class != egui::ViewportClass::EmbeddedWindow && ctx.input(|i| i.viewport().close_requested()) {
            app.ui.second_window = false;
        }
    });
}

fn body(app: &mut LightcraftApp, ui: &mut egui::Ui) {
    let area = ui.available_rect_before_wrap();
    crate::widgets::register(ui.ctx(), "view:secondWindow", area);
    ui.allocate_rect(area, egui::Sense::hover());
    let Some(id) = app.session.active() else {
        ui.painter().text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            crate::i18n::tr("No photo selected"),
            egui::FontId::proportional(14.0),
            Color32::GRAY,
        );
        return;
    };
    // a render sized for this window
    let ppp = ui.ctx().pixels_per_point();
    let (w, h) = (((area.width() * ppp) as usize).clamp(64, 4096), ((area.height() * ppp) as usize).clamp(64, 4096));
    if let Some(job) = app.session.loupe_job(id, w, h, true)
        && app.renderer.textures.get(&Slot::Second).is_none_or(|t| t.key != job.key)
        && !app.renderer.is_pending(Slot::Second)
    {
        app.renderer.request(Slot::Second, job, 95);
    }
    let mine = |s: Slot| app.renderer.textures.get(&s).filter(|t| t.photo == id);
    let tex = mine(Slot::Second).or_else(|| mine(Slot::Main)).or_else(|| app.renderer.thumb(id));
    if let Some(t) = tex {
        let [tw, th] = t.tex.size();
        let s = (area.width() / tw as f32).min(area.height() / th as f32);
        let r = Rect::from_center_size(area.center(), vec2(tw as f32 * s, th as f32 * s));
        ui.painter().image(t.tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}
