//! Headless tests of the point-curve editor: dragging points, resetting, presets.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

/// Detail view with the Edit panel's Curve flyout open on `channel`.
fn curve_open(channel: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.app.ui.right = crate::state::RightPanel::Edit;
    if !h.app.ui.section_open("light") {
        h.app.ui.toggle_section("light");
    }
    if !h.app.ui.flyout_open("curve") {
        h.app.ui.toggle_flyout("curve");
    }
    h.app.ui.curve_channel = channel.into();
    h.step();
    h.step();
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn develop(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().expect("active photo");
    (*h.app.session.develop_of(id).unwrap_or_default()).clone()
}

fn pts(v: &[lightcraft_geom::Point]) -> Vec<(f64, f64)> {
    v.iter().map(|p| (p.x, p.y)).collect()
}

/// Side of the (square) curve graph in screen points.
fn curve_side(h: &mut Headless) -> f64 {
    let r = h.request("ui.widgets", json!({"filter": "curve"}), T);
    let w = r["result"].as_array().unwrap().iter().find(|w| w["id"] == "curve").cloned().unwrap_or_else(|| panic!("curve widget on screen: {r}"));
    w["rect"][2].as_f64().unwrap()
}

#[test]
fn dragging_a_point_moves_it_in_both_axes_with_one_undo_step() {
    let mut h = curve_open("master");
    exec(&mut h, "develop.curve", json!({"channel": "master", "points": [[0.0, 0.0], [0.5, 0.5], [1.0, 1.0]]}));
    h.step();
    let side = curve_side(&mut h);
    let undo_before = h.app.session.undo.len();
    // grab the middle point and drag it up-left: x 0.5 → 0.4, y 0.5 → 0.7
    let r = h.request("ui.dragWidget", json!({"id": "curve", "fx": 0.5, "fy": 0.5, "dx": -0.1 * side, "dy": -0.2 * side, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let c = pts(&develop(&h).curve.master);
    assert_eq!(c.len(), 3, "dragging moves the point, it doesn't add one: {c:?}");
    assert!((c[1].0 - 0.4).abs() < 0.03, "x follows the pointer: {c:?}");
    assert!((c[1].1 - 0.7).abs() < 0.03, "y follows the pointer: {c:?}");
    assert_eq!(h.app.session.undo.len(), undo_before + 1, "one undo step per drag");
}

fn click_widget(h: &mut Headless, id: &str, count: u64) {
    let r = h.request("ui.clickWidget", json!({"id": id, "count": count}), T);
    assert_eq!(r["ok"], true, "{id}: {r}");
    h.step();
}

const S_CURVE: [[f64; 2]; 4] = [[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]];

#[test]
fn double_clicking_a_channel_resets_only_that_channel() {
    let mut h = curve_open("master");
    exec(&mut h, "develop.curve", json!({"channel": "master", "points": S_CURVE}));
    exec(&mut h, "develop.curve", json!({"channel": "green", "points": S_CURVE}));
    h.step();
    click_widget(&mut h, "curveChannel:green", 2);
    let c = develop(&h).curve;
    assert!(c.green.is_empty(), "green resets: {:?}", c.green);
    assert_eq!(c.master.len(), 4, "the other channels stay");
    assert_eq!(h.app.ui.curve_channel, "green");
}

#[test]
fn reset_button_resets_every_curve() {
    let mut h = curve_open("red");
    for ch in ["master", "red", "blue"] {
        exec(&mut h, "develop.curve", json!({"channel": ch, "points": S_CURVE}));
    }
    exec(&mut h, "develop.set", json!({"control": "curve.highlights", "value": -40}));
    h.step();
    click_widget(&mut h, "button:curveReset", 1);
    let c = develop(&h).curve;
    assert_eq!(c, lightcraft_develop::ToneCurve::default());
}

#[test]
fn point_curve_preset_dropdown_applies_and_saves() {
    let mut h = curve_open("master");
    click_widget(&mut h, "dropdown:curvePreset", 1);
    click_widget(&mut h, "curvePreset:Medium Contrast", 1);
    let c = develop(&h).curve;
    assert_eq!(c.master.len(), 5, "the preset's S-curve: {:?}", c.master);
    assert!(c.master[1].y < 0.25 && c.master[3].y > 0.75);
    // shape it further, then save it from the dropdown (name prompt)
    exec(&mut h, "develop.curve", json!({"channel": "blue", "points": [[0.0, 0.08], [1.0, 1.0]]}));
    h.step();
    click_widget(&mut h, "dropdown:curvePreset", 1);
    click_widget(&mut h, "curvePresetMenu:save", 1);
    let mut dlg = h.app.ui.dialog.take().expect("a name prompt opens");
    match &mut dlg {
        crate::state::Dialog::TextPrompt { command, value, .. } => {
            assert_eq!(command, "curve.savePreset");
            *value = "Cool Shadows".into();
        }
        _ => panic!("a name prompt opens"),
    }
    let r = crate::panels::dialogs::confirm_dialog(&mut h.app, &dlg);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(h.app.session.curve_presets.len(), 1);
    // Linear from the dropdown flattens every channel
    click_widget(&mut h, "dropdown:curvePreset", 1);
    click_widget(&mut h, "curvePreset:Linear", 1);
    let c = develop(&h).curve;
    assert!(c.master.is_empty() && c.blue.is_empty());
    click_widget(&mut h, "dropdown:curvePreset", 1);
    click_widget(&mut h, "curvePreset:Cool Shadows", 1);
    assert_eq!(develop(&h).curve.blue.len(), 2);
}

#[test]
fn dragging_a_point_past_its_neighbour_is_clamped() {
    let mut h = curve_open("red");
    exec(&mut h, "develop.curve", json!({"channel": "red", "points": [[0.0, 0.0], [0.3, 0.3], [0.6, 0.6], [1.0, 1.0]]}));
    h.step();
    let side = curve_side(&mut h);
    // drag the point at 0.3 far to the right (past 0.6)
    let r = h.request("ui.dragWidget", json!({"id": "curve", "fx": 0.3, "fy": 0.7, "dx": 0.5 * side, "dy": 0.0, "steps": 12}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    let c = pts(&develop(&h).curve.red);
    assert_eq!(c.len(), 4, "{c:?}");
    assert!(c[1].0 < c[2].0 && c[1].0 > 0.55, "clamped just left of its neighbour: {c:?}");
    assert_eq!(c[2], (0.6, 0.6), "the neighbour stays: {c:?}");
}
