//! Active-filter chips under the grid header: one removable chip per constraint (search, rating,
//! date, keyword…), "+N more" for what does not fit, and "Clear all". The count next to them says
//! how many of the source's photos match, so an empty grid never looks like an empty folder.

use egui::{Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use lightcraft_engine::FilterChip;
use serde_json::json;

use crate::LightcraftApp;
use crate::theme::Tokens;
use crate::widgets::register;

pub const HEIGHT: f32 = 32.0;
const GAP: f32 = 6.0;
/// Room kept for "+N more" and "Clear all".
const TAIL: f32 = 170.0;

/// "14 of 120 photos" when a filter narrows the source, else "14 photos".
pub fn count_text(matching: usize, total: Option<usize>, filtering: bool) -> String {
    match total {
        Some(t) if filtering && t != matching => crate::i18n::tr_format!("{matching} of {t} photos", matching = matching, t = t),
        _ => crate::i18n::tr_format!("{matching} photos", matching = matching),
    }
}

/// Width of a chip with `label` (text + the × button).
fn chip_width(ui: &egui::Ui, label: &str, font: egui::FontId) -> f32 {
    ui.painter().layout_no_wrap(label.to_string(), font, egui::Color32::WHITE).size().x + 12.0 + 22.0
}

/// Draws the strip; does nothing without chips.
pub fn show(app: &mut LightcraftApp, ui: &mut egui::Ui, chips: &[FilterChip]) {
    if chips.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    ui.painter().rect_filled(bar, 0.0, t.canvas);
    register(ui.ctx(), "filterchips", bar);
    let font = t.font(12.0);
    let right = bar.right() - 16.0;
    let mut x = bar.left() + 20.0;
    let mut shown = 0;
    for (i, c) in chips.iter().enumerate() {
        let w = chip_width(ui, &c.label, font.clone());
        // always show the first chip; keep room for the tail unless this is the last one that fits
        let room = right - x - if i + 1 == chips.len() { 90.0 } else { TAIL };
        if i > 0 && w > room {
            break;
        }
        let r = Rect::from_min_size(pos2(x, bar.center().y - 11.0), vec2(w.min(right - x), 22.0));
        ui.painter().rect(r, 11.0, t.field, Stroke::new(1.0, t.accent.gamma_multiply(0.6)), StrokeKind::Inside);
        let text_r = Rect::from_min_max(r.min, pos2(r.right() - 22.0, r.max.y));
        ui.painter().with_clip_rect(text_r).text(pos2(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, &c.label, font.clone(), t.text);
        let xr = Rect::from_center_size(pos2(r.right() - 12.0, r.center().y), vec2(18.0, 18.0));
        let resp = ui.interact(xr, egui::Id::new(("filter-chip-x", i)), Sense::click()).on_hover_text(crate::i18n::tr("Remove this filter"));
        register(ui.ctx(), format!("chip:{i}"), xr);
        let col = if resp.hovered() { t.text } else { t.text_dim };
        let m = 3.5;
        let (a, b) = (xr.center() - vec2(m, m), xr.center() + vec2(m, m));
        ui.painter().line_segment([a, b], Stroke::new(1.3, col));
        ui.painter().line_segment([pos2(a.x, b.y), pos2(b.x, a.y)], Stroke::new(1.3, col));
        if resp.clicked() {
            let _ = app.run("library.filter", c.clear.clone());
            if c.clear.get("text").is_some() {
                app.ui.search.clear();
            }
        }
        x = r.right() + GAP;
        shown = i + 1;
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(x, bar.top() + 4.0), pos2(right, bar.bottom() - 4.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    let hidden = &chips[shown..];
    if !hidden.is_empty() {
        let more = crate::widgets::text_button(&mut child, "chipsMore", &format!("+{} more", hidden.len()), false);
        egui::Popup::menu(&more).show(|ui| {
            for c in hidden {
                if ui.button(format!("{}  ✕", c.label)).on_hover_text(crate::i18n::tr("Remove this filter")).clicked() {
                    let _ = app.run("library.filter", c.clear.clone());
                    if c.clear.get("text").is_some() {
                        app.ui.search.clear();
                    }
                }
            }
        });
    }
    if crate::widgets::text_button(&mut child, "chipsClearAll", crate::i18n::tr("Clear all"), false).clicked() {
        app.ui.search.clear();
        let _ = app.run("library.clearFilter", json!({}));
    }
}

#[cfg(test)]
mod tests {
    use super::count_text;

    #[test]
    fn count_says_matching_of_total_only_when_a_filter_narrows() {
        assert_eq!(count_text(0, Some(14), true), "0 of 14 photos");
        assert_eq!(count_text(14, Some(14), true), "14 photos");
        assert_eq!(count_text(5, Some(14), false), "5 photos");
        assert_eq!(count_text(3, None, true), "3 photos");
    }
}
