//! Design tokens (colours, sizes, fonts). Values measured from black-box observation of the
//! reference app's dark theme (see plan/lightroom/10-observed-ui.md); everything is our own code.

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

pub const FONT_SEMIBOLD: &str = "semibold";

#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    /// Top bar, side panels, bottom bar, tool strip.
    pub chrome: Color32,
    /// Photo canvas (detail view) and filmstrip.
    pub canvas: Color32,
    /// Grid background behind the cells.
    pub grid_bg: Color32,
    pub cell: Color32,
    pub cell_selected: Color32,
    pub divider: Color32,
    pub inset: Color32,
    pub field: Color32,
    pub field_border: Color32,
    pub button: Color32,
    pub button_border: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub tool_active: Color32,
    pub text: Color32,
    pub text_label: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    /// Icon glyphs in the chrome.
    pub icon: Color32,
    pub track: Color32,
    pub thumb: Color32,
    pub thumb_hover: Color32,
    pub accent: Color32,
    pub star: Color32,
    pub pick: Color32,
    pub reject: Color32,
    /// Cautionary notices (e.g. a raw shown from its embedded preview): a muted amber.
    pub caution: Color32,
    pub mask_overlay: Color32,
    // metrics (points)
    pub top_bar_h: f32,
    pub bottom_bar_h: f32,
    pub panel_w: f32,
    pub strip_w: f32,
    pub slider_row_h: f32,
    pub section_h: f32,
    pub film_h: f32,
}

impl Default for Tokens {
    fn default() -> Self {
        Tokens {
            chrome: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            canvas: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            grid_bg: Color32::from_rgb(0x0f, 0x0f, 0x0f),
            cell: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            cell_selected: Color32::from_rgb(0x2d, 0x2d, 0x2d),
            divider: Color32::from_rgb(0x1c, 0x1c, 0x1c),
            inset: Color32::from_rgb(0x23, 0x23, 0x23),
            field: Color32::from_rgb(0x22, 0x22, 0x22),
            field_border: Color32::from_rgb(0x3c, 0x3c, 0x3c),
            button: Color32::from_rgb(0x24, 0x24, 0x24),
            button_border: Color32::from_rgb(0x4c, 0x4c, 0x4c),
            hover: Color32::from_rgb(0x3a, 0x3a, 0x3a),
            pressed: Color32::from_rgb(0x3f, 0x3f, 0x3f),
            tool_active: Color32::from_rgb(0x3f, 0x3f, 0x3f),
            text: Color32::from_rgb(0xe2, 0xe2, 0xe2),
            text_label: Color32::from_rgb(0xbc, 0xbc, 0xbc),
            text_dim: Color32::from_rgb(0x8e, 0x8e, 0x8e),
            text_disabled: Color32::from_rgb(0x5c, 0x5c, 0x5c),
            icon: Color32::from_rgb(0x9a, 0x9a, 0x9a),
            track: Color32::from_rgb(0x5a, 0x5a, 0x5a),
            thumb: Color32::from_rgb(0xa0, 0xa0, 0xa0),
            thumb_hover: Color32::from_rgb(0xe0, 0xe0, 0xe0),
            accent: Color32::from_rgb(0x01, 0x65, 0xdd),
            star: Color32::from_rgb(0xd8, 0xd8, 0xd8),
            pick: Color32::from_rgb(0xf0, 0xf0, 0xf0),
            reject: Color32::from_rgb(0xe0, 0x4a, 0x4a),
            caution: Color32::from_rgb(0xe3, 0xa8, 0x3c),
            mask_overlay: Color32::from_rgba_unmultiplied(0xe0, 0x20, 0x30, 110),
            top_bar_h: 42.0,
            bottom_bar_h: 48.0,
            panel_w: 270.0,
            strip_w: 48.0,
            slider_row_h: 45.0,
            section_h: 57.0,
            film_h: 142.0,
        }
    }
}

impl Tokens {
    pub fn get(ctx: &egui::Context) -> Tokens {
        ctx.data(|d| d.get_temp::<Tokens>(egui::Id::NULL)).unwrap_or_default()
    }
    pub fn font(&self, size: f32) -> FontId {
        FontId::proportional(size)
    }
    pub fn semibold(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(FONT_SEMIBOLD.into()))
    }
}

pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions(lightcraft_engine::CRAFT_FONTS));
}

/// Inter (bundled) for Latin text, egui's default fonts, then the craft-fonts Japanese faces as
/// the last fallback of every family (BIZ UDPGothic first; Bold first for the semibold family).
/// Without craft-fonts (`craft` empty) Japanese text has no glyphs and shows as boxes.
pub fn font_definitions(craft: &'static [lightcraft_engine::CraftFont]) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("Inter".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-Regular.ttf"))));
    fonts.font_data.insert("Inter-SemiBold".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"))));
    let japanese: Vec<_> = lightcraft_engine::fonts::japanese(craft).collect();
    for f in &japanese {
        fonts.font_data.insert(craft_font_name(f), Arc::new(FontData::from_static(f.bytes)));
    }
    // Craft-fonts faces in preference order for a family drawn in `style`.
    let fallback = |style: &str| {
        let mut faces = japanese.clone();
        faces.sort_by_key(|f| (f.family != "BIZ UDPGothic", f.style != style, f.family.contains("Mincho")));
        faces.into_iter().map(craft_font_name).collect::<Vec<_>>()
    };
    let defaults: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["Inter".to_string()];
    prop.extend(defaults.iter().cloned());
    prop.extend(fallback("Regular"));
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut semi = vec!["Inter-SemiBold".to_string()];
    semi.extend(defaults);
    semi.extend(fallback("Bold"));
    fonts.families.insert(FontFamily::Name(FONT_SEMIBOLD.into()), semi);
    fonts.families.entry(FontFamily::Monospace).or_default().extend(fallback("Regular"));
    fonts
}

/// The embedded font families for the About box: Inter, plus the craft-fonts families when built
/// with them.
pub fn font_credits() -> String {
    let mut families = vec!["Inter"];
    for f in lightcraft_engine::CRAFT_FONTS {
        if !families.contains(&f.family) {
            families.push(f.family);
        }
    }
    families.join(" / ")
}

fn craft_font_name(f: &lightcraft_engine::CraftFont) -> String {
    format!("craft-fonts {} {}", f.family, f.style)
}

pub fn apply(ctx: &egui::Context) {
    let t = Tokens::default();
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, t));
    let mut v = Visuals::dark();
    v.panel_fill = t.chrome;
    v.window_fill = t.chrome;
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.inset;
    v.window_stroke = Stroke::new(1.0, t.button_border);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = t.accent.gamma_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.override_text_color = Some(t.text_label);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(140) };
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(160) };
    let w = &mut v.widgets;
    for (wv, fill) in [
        (&mut w.noninteractive, t.chrome),
        (&mut w.inactive, t.button),
        (&mut w.hovered, t.hover),
        (&mut w.active, t.pressed),
        (&mut w.open, t.hover),
    ] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.corner_radius = CornerRadius::same(4);
        wv.fg_stroke = Stroke::new(1.0, t.text_label);
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    w.inactive.bg_stroke = Stroke::new(1.0, t.button_border);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size.y = 22.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(16.0, FontFamily::Name(FONT_SEMIBOLD.into())));
        s.animation_time = 0.08;
    });
}
