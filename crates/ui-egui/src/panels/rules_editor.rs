//! The smart-album rule editor: match all / any / none of a list of rules (field, operator,
//! value), with nested groups. Field and operator lists come from `lightcraft_catalog::rules`.

use egui::RichText;
use lightcraft_catalog::rules::{FIELDS, Kind, field_kind, ops_for};
use lightcraft_catalog::{Match, Rule, RuleSet};
use serde_json::{Value, json};

use crate::theme::Tokens;
use crate::widgets::register;

/// A sensible starting value for `field` / `op`.
pub fn default_value(field: &str, op: &str) -> Value {
    match (field_kind(field), op) {
        (_, "isEmpty" | "isNotEmpty") => Value::Null,
        (Some(Kind::Date), "inLast" | "notInLast") => json!({"n": 30, "unit": "days"}),
        (Some(Kind::Date), "between") => json!(["", ""]),
        (Some(Kind::Number), "between") => json!([0, 0]),
        (Some(Kind::Number), _) => json!(if field == "rating" { 3 } else { 0 }),
        (Some(Kind::Choice(c)), _) => json!(c.first().copied().unwrap_or("")),
        (Some(Kind::Bool), _) => json!(true),
        _ => json!(""),
    }
}

/// A new rule (Rating ≥ 3).
pub fn new_rule() -> Rule {
    Rule::Field { field: "rating".into(), op: "gte".into(), value: json!(3) }
}

fn match_combo(ui: &mut egui::Ui, salt: &str, m: &mut Match) {
    let label = |m: Match| match m {
        Match::All => "all",
        Match::Any => "any",
        Match::None => "none",
    };
    egui::ComboBox::from_id_salt(format!("{salt}-match")).width(64.0).selected_text(label(*m)).show_ui(ui, |ui| {
        for x in [Match::All, Match::Any, Match::None] {
            ui.selectable_value(m, x, label(x));
        }
    });
}

fn text_value(ui: &mut egui::Ui, v: &mut Value, width: f32, hint: &str, salt: &str) {
    let mut s = match &*v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let r = ui.add(egui::TextEdit::singleline(&mut s).hint_text(hint).desired_width(width));
    register(ui.ctx(), format!("field:{salt}"), r.rect);
    if r.changed() {
        *v = json!(s);
    }
}

fn number_value(ui: &mut egui::Ui, v: &mut Value, field: &str) {
    let mut n = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())).unwrap_or(0.0);
    let (lo, hi, speed) = match field {
        "rating" => (0.0, 5.0, 0.05),
        "iso" => (0.0, 409_600.0, 10.0),
        "aperture" => (0.0, 64.0, 0.05),
        _ => (0.0, 100_000.0, 0.5),
    };
    let whole = matches!(field, "rating" | "iso" | "album");
    let dv = egui::DragValue::new(&mut n).range(lo..=hi).speed(speed);
    let dv = if whole { dv.fixed_decimals(0) } else { dv.max_decimals(2) };
    if ui.add(dv).changed() {
        *v = if whole { json!(n.round() as i64) } else { json!(n) };
    }
}

/// The value editor for one rule.
fn value_editor(ui: &mut egui::Ui, field: &str, op: &str, v: &mut Value, salt: &str) {
    match (field_kind(field), op) {
        (_, "isEmpty" | "isNotEmpty") => {}
        (Some(Kind::Date), "inLast" | "notInLast") => {
            if !v.is_object() {
                *v = default_value(field, op);
            }
            let mut n = v["n"].as_f64().unwrap_or(30.0);
            if ui.add(egui::DragValue::new(&mut n).range(1.0..=10_000.0).speed(0.2).fixed_decimals(0)).changed() {
                v["n"] = json!(n.round());
            }
            let unit = v["unit"].as_str().unwrap_or("days").to_string();
            egui::ComboBox::from_id_salt(format!("{salt}-unit")).width(70.0).selected_text(&unit).show_ui(ui, |ui| {
                for u in ["hours", "days", "weeks", "months", "years"] {
                    if ui.selectable_label(unit == u, u).clicked() {
                        v["unit"] = json!(u);
                    }
                }
            });
        }
        (Some(k), "between") => {
            if v.as_array().is_none_or(|a| a.len() != 2) {
                *v = default_value(field, op);
            }
            let Value::Array(a) = v else { return };
            if k == Kind::Number {
                number_value(ui, &mut a[0], field);
                ui.label(crate::i18n::tr("and"));
                number_value(ui, &mut a[1], field);
            } else {
                text_value(ui, &mut a[0], 86.0, "2026-01-01", &format!("{salt}-a"));
                ui.label(crate::i18n::tr("and"));
                text_value(ui, &mut a[1], 86.0, "2026-12", &format!("{salt}-b"));
            }
        }
        (Some(Kind::Number), _) => number_value(ui, v, field),
        (Some(Kind::Choice(c)), _) => {
            let cur = v.as_str().unwrap_or("").to_string();
            egui::ComboBox::from_id_salt(format!("{salt}-choice")).width(90.0).selected_text(&cur).show_ui(ui, |ui| {
                for x in c.iter() {
                    if ui.selectable_label(cur == *x, *x).clicked() {
                        *v = json!(x);
                    }
                }
            });
        }
        (Some(Kind::Bool), _) => {
            let mut b = v.as_bool().unwrap_or(true);
            egui::ComboBox::from_id_salt(format!("{salt}-bool")).width(60.0).selected_text(if b { "true" } else { "false" }).show_ui(ui, |ui| {
                ui.selectable_value(&mut b, true, "true");
                ui.selectable_value(&mut b, false, "false");
            });
            *v = json!(b);
        }
        (Some(Kind::Date), _) => text_value(ui, v, 120.0, "2026-04 or 2026-04-12", salt),
        _ => text_value(ui, v, 140.0, "", salt),
    }
}

/// Edit `rs` in place; `salt` keeps widget ids apart between groups.
pub fn edit(ui: &mut egui::Ui, rs: &mut RuleSet, salt: &str, depth: usize) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(crate::i18n::tr("Match")).color(t.text_label));
        match_combo(ui, salt, &mut rs.mode);
        ui.label(RichText::new(crate::i18n::tr("of the following rules:")).color(t.text_label));
    });
    let mut remove = None;
    let mut insert: Option<(usize, Rule)> = None;
    for (i, rule) in rs.rules.iter_mut().enumerate() {
        let rsalt = format!("{salt}-{i}");
        match rule {
            Rule::Group { group } => {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button("−").on_hover_text(crate::i18n::tr("Remove this group")).clicked() {
                            remove = Some(i);
                        }
                        ui.vertical(|ui| edit(ui, group, &rsalt, depth + 1));
                    });
                });
            }
            Rule::Field { field, op, value } => {
                ui.horizontal(|ui| {
                    let label = FIELDS.iter().find(|f| f.0 == field.as_str()).map_or(field.as_str(), |f| f.1).to_string();
                    egui::ComboBox::from_id_salt(format!("{rsalt}-field")).width(150.0).height(400.0).selected_text(label).show_ui(ui, |ui| {
                        for (id, l, k) in FIELDS {
                            if ui.selectable_label(field == id, *l).clicked() && field != id {
                                *field = id.to_string();
                                if !ops_for(*k).iter().any(|o| o.0 == op.as_str()) {
                                    *op = ops_for(*k)[0].0.to_string();
                                }
                                *value = default_value(field, op);
                            }
                        }
                    });
                    let kind = field_kind(field).unwrap_or(Kind::Text);
                    let op_label = ops_for(kind).iter().find(|o| o.0 == op.as_str()).map_or(op.as_str(), |o| o.1).to_string();
                    egui::ComboBox::from_id_salt(format!("{rsalt}-op")).width(110.0).selected_text(op_label).show_ui(ui, |ui| {
                        for (id, l) in ops_for(kind) {
                            if ui.selectable_label(op == id, *l).clicked() && op != id {
                                let was_special = matches!(op.as_str(), "between" | "inLast" | "notInLast" | "isEmpty" | "isNotEmpty");
                                *op = id.to_string();
                                if was_special || matches!(*id, "between" | "inLast" | "notInLast" | "isEmpty" | "isNotEmpty") {
                                    *value = default_value(field, op);
                                }
                            }
                        }
                    });
                    value_editor(ui, field, op, value, &rsalt);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let add = ui.small_button("+").on_hover_text(crate::i18n::tr("Add a rule (⌥-click: a group)"));
                        register(ui.ctx(), format!("button:ruleAdd-{rsalt}"), add.rect);
                        if add.clicked() {
                            let group = ui.input(|i| i.modifiers.alt) && depth < 3;
                            insert = Some((
                                i + 1,
                                if group { Rule::Group { group: RuleSet { mode: Match::Any, rules: vec![new_rule()] } } } else { new_rule() },
                            ));
                        }
                        let del = ui.small_button("−").on_hover_text(crate::i18n::tr("Remove this rule"));
                        register(ui.ctx(), format!("button:ruleRemove-{rsalt}"), del.rect);
                        if del.clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
        }
    }
    if let Some(i) = remove {
        rs.rules.remove(i);
    }
    if let Some((i, r)) = insert {
        rs.rules.insert(i.min(rs.rules.len()), r);
    }
    if rs.rules.is_empty() || depth == 0 {
        ui.horizontal(|ui| {
            let add = ui.small_button("+ Rule");
            register(ui.ctx(), format!("button:ruleAddEnd-{salt}"), add.rect);
            if add.clicked() {
                rs.rules.push(new_rule());
            }
            if depth < 3 && ui.small_button("+ Group").clicked() {
                rs.rules.push(Rule::Group { group: RuleSet { mode: Match::Any, rules: vec![new_rule()] } });
            }
        });
    }
}
