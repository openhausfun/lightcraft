//! Headless tests of the Masking and Remove tools on the photo: overlay keys, pins, spot editing.

use std::time::Duration;

use lightcraft_develop::MaskShape;
use serde_json::json;

use crate::headless::Headless;
use crate::state::RightPanel;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn detail(panel: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("engine.execute", json!({"command": panel}), T);
    assert_eq!(r["ok"], true, "{r}");
    h
}

fn exec(h: &mut Headless, command: &str, params: serde_json::Value) -> serde_json::Value {
    let r = h.request("engine.execute", json!({"command": command, "params": params}), T);
    assert_eq!(r["ok"], true, "{command}: {r}");
    r["result"].clone()
}

fn pointer(h: &mut Headless, events: serde_json::Value) {
    let r = h.request("ui.pointer", json!({"events": events}), T);
    assert_eq!(r["ok"], true, "{r}");
}

fn develop(h: &Headless) -> lightcraft_develop::DevelopSettings {
    let id = h.app.session.active().expect("active photo");
    (*h.app.session.develop_of(id).unwrap_or_default()).clone()
}

#[test]
fn mask_overlay_keys_and_pins() {
    use lightcraft_pipeline::{MaskView, Overlay};
    let mut h = detail("panel.masking");
    assert_eq!(h.app.ui.right, RightPanel::Masking);
    exec(&mut h, "mask.add", json!({"kind": "radial", "center": [0.3, 0.4], "rx": 0.1, "ry": 0.1}));
    exec(&mut h, "mask.add", json!({"kind": "linear", "start": [0.7, 0.2], "end": [0.7, 0.6]}));
    assert_eq!(h.app.session.active_mask, Some(2));
    // the loupe asks the renderer for the selected mask's overlay
    let d = develop(&h);
    let o = crate::panels::detail::view_overlay(&h.app, &d);
    assert_eq!(o, Overlay::Mask { id: 2, view: MaskView::Color, color: [230, 30, 40], opacity: 50 });
    // O toggles it, Shift+O cycles the colour (and leaves the crop overlay alone)
    h.request("ui.key", json!({"key": "o"}), T);
    assert!(!h.app.ui.mask_overlay);
    assert_eq!(crate::panels::detail::view_overlay(&h.app, &d), Overlay::None);
    h.request("ui.key", json!({"key": "o"}), T);
    let crop = h.app.ui.crop_overlay;
    let colour = h.app.ui.mask_overlay_color;
    h.request("ui.key", json!({"key": "o", "shift": true}), T);
    assert_ne!(h.app.ui.mask_overlay_color, colour, "the next overlay colour");
    assert_eq!(h.app.ui.mask_overlay_mode, "color", "the mode stays");
    assert_eq!(h.app.ui.crop_overlay, crop);
    exec(&mut h, "view.maskOverlayMode", json!({"mode": "whiteOnBlack"}));
    exec(&mut h, "view.maskOverlayColor", json!({"color": "#2870f0", "opacity": 80}));
    assert_eq!((h.app.ui.mask_overlay_color, h.app.ui.mask_overlay_opacity), ([0x28, 0x70, 0xf0], 80.0));
    let r = h.request("engine.execute", json!({"command": "view.maskOverlayMode", "params": {"mode": "nope"}}), T);
    assert_eq!(r["ok"], false, "{r}");
    // clicking another mask's pin selects that mask
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "up", "x": 0.3, "y": 0.4}]));
    assert_eq!(h.app.session.active_mask, Some(1));
    // dragging a pin moves its component (one undo step)
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.4}, {"kind": "drag", "x": 0.35, "y": 0.45}, {"kind": "drag", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]),
    );
    let MaskShape::Radial { center, .. } = develop(&h).masks[0].components[0].shape.clone() else { panic!("radial") };
    assert!((center.x - 0.5).abs() < 0.02 && (center.y - 0.6).abs() < 0.02, "{center:?}");
    // dragging the linear gradient's pin (a non-selected mask) selects and moves it
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.7, "y": 0.4}, {"kind": "drag", "x": 0.72, "y": 0.4}, {"kind": "drag", "x": 0.8, "y": 0.4}, {"kind": "up", "x": 0.8, "y": 0.4}]),
    );
    assert_eq!(h.app.session.active_mask, Some(2));
    let MaskShape::Linear { start, end } = develop(&h).masks[1].components[0].shape.clone() else { panic!("linear") };
    assert!((start.x - 0.8).abs() < 0.02 && (end.x - 0.8).abs() < 0.02 && (start.y - 0.2).abs() < 0.02, "{start:?} {end:?}");
    // hidden pins can't be grabbed
    exec(&mut h, "view.maskPins", json!({"show": false}));
    pointer(&mut h, json!([{"kind": "down", "x": 0.5, "y": 0.6}, {"kind": "up", "x": 0.5, "y": 0.6}]));
    assert_eq!(h.app.session.active_mask, Some(2));
    h.settle(SETTLE);
}

#[test]
fn brush_strokes_carry_auto_mask() {
    let mut h = detail("panel.masking");
    let r = h.request("ui.set", json!({"brushAutoMask": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    exec(&mut h, "tool.brush", json!({}));
    assert_eq!(h.app.ui.tool, "brush");
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.5}, {"kind": "drag", "x": 0.4, "y": 0.5}, {"kind": "drag", "x": 0.5, "y": 0.5}, {"kind": "up", "x": 0.5, "y": 0.5}]),
    );
    let d = develop(&h);
    let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
    assert!(strokes[0].auto_mask && strokes[0].points.len() >= 2, "{strokes:?}");
    assert!(!strokes[0].erase);
    // holding ⌥ paints an erase stroke without switching the brush to Erase
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.35, "y": 0.5}, {"kind": "drag", "x": 0.45, "y": 0.5}, {"kind": "up", "x": 0.45, "y": 0.5}], "alt": true}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    let d = develop(&h);
    let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
    assert_eq!(strokes.len(), 2, "{strokes:?}");
    assert!(strokes[1].erase, "⌥ erases");
    assert!(!h.app.ui.brush_erase, "the brush mode is unchanged");
    h.settle(SETTLE);
}

#[test]
fn remove_spots_by_pointer_and_keyboard() {
    let mut h = detail("panel.remove");
    assert_eq!(h.app.ui.right, RightPanel::Remove);
    let spots = |h: &Headless| develop(h).spots;
    // paint a spot: it's added with the brush's size/feather/opacity and selected
    let r = h.request("ui.set", json!({"removeFeather": 30.0, "removeOpacity": 80.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "up", "x": 0.3, "y": 0.6}]));
    assert_eq!(spots(&h).len(), 1);
    assert_eq!((spots(&h)[0].feather, spots(&h)[0].opacity), (30.0, 80.0));
    assert_eq!(h.app.session.active_spot, Some(0));
    // [ / ] resize the selected spot, Shift+[ / Shift+] feather it
    let s0 = spots(&h)[0].size;
    h.request("ui.key", json!({"key": "]"}), T);
    assert!(spots(&h)[0].size > s0 * 1.1, "{} vs {s0}", spots(&h)[0].size);
    h.request("ui.key", json!({"key": "["}), T);
    h.request("ui.key", json!({"key": "["}), T);
    assert!(spots(&h)[0].size < s0 * 0.9);
    h.request("ui.key", json!({"key": "[", "shift": true}), T);
    assert_eq!(spots(&h)[0].feather, 20.0);
    h.request("ui.key", json!({"key": "]", "shift": true}), T);
    assert_eq!(spots(&h)[0].feather, 30.0);
    // / picks another source (and leaves the filmstrip alone)
    let (src, film) = (spots(&h)[0].source_offset, h.app.ui.filmstrip);
    h.request("ui.key", json!({"key": "/"}), T);
    assert_ne!(spots(&h)[0].source_offset, src);
    assert_eq!(h.app.ui.filmstrip, film);
    // a second spot elsewhere; clicking the first one's pin selects it
    pointer(&mut h, json!([{"kind": "down", "x": 0.7, "y": 0.3}, {"kind": "up", "x": 0.7, "y": 0.3}]));
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (2, Some(1)));
    pointer(&mut h, json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "up", "x": 0.3, "y": 0.6}]));
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (2, Some(0)));
    // drag its target: it moves, its source offset stays
    let src = spots(&h)[0].source_offset;
    pointer(
        &mut h,
        json!([{"kind": "down", "x": 0.3, "y": 0.6}, {"kind": "drag", "x": 0.32, "y": 0.6}, {"kind": "drag", "x": 0.4, "y": 0.6}, {"kind": "up", "x": 0.4, "y": 0.6}]),
    );
    let sp = spots(&h)[0].clone();
    assert!((sp.points[0].x - 0.4).abs() < 0.01 && (sp.points[0].y - 0.6).abs() < 0.01, "{:?}", sp.points);
    assert_eq!(sp.source_offset, src);
    // drag its source to a fixed place
    let o = sp.source_offset.unwrap();
    let (sx, sy) = (sp.points[0].x + o.x, sp.points[0].y + o.y);
    pointer(
        &mut h,
        json!([{"kind": "down", "x": sx, "y": sy}, {"kind": "drag", "x": sx + 0.01, "y": sy}, {"kind": "drag", "x": 0.5, "y": 0.8}, {"kind": "up", "x": 0.5, "y": 0.8}]),
    );
    let sp = spots(&h)[0].clone();
    let o = sp.source_offset.unwrap();
    assert!((sp.points[0].x + o.x - 0.5).abs() < 0.01 && (sp.points[0].y + o.y - 0.8).abs() < 0.01, "{o:?}");
    // ⌫ deletes the selected spot, not the photo
    let photos = h.app.session.visible_cloned().len();
    h.request("ui.key", json!({"key": "delete"}), T);
    assert_eq!((spots(&h).len(), h.app.session.active_spot), (1, None));
    assert_eq!(h.app.session.visible_cloned().len(), photos);
    h.request("ui.key", json!({"key": "delete"}), T);
    assert_eq!(spots(&h).len(), 1, "nothing selected: nothing deleted");
    h.settle(SETTLE);
}

/// Masks list: double-click renames in place, the hover eye hides a mask, the overlay colour
/// cycles through the swatches.
#[test]
fn mask_list_rename_hide_and_overlay_colour() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "linear"}));
    exec(&mut h, "mask.add", json!({"kind": "radial"}));
    h.settle(SETTLE);
    let first = develop(&h).masks[0].id;
    let r = h.request("ui.clickWidget", json!({"id": format!("mask:{first}"), "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(5));
    assert_eq!(h.app.ui.renaming_mask.as_ref().map(|r| r.0), Some(first), "double-click starts renaming");
    h.request("ui.key", json!({"key": "A", "cmd": true}), T);
    h.request("ui.text", json!({"text": "Sky"}), T);
    h.request("ui.key", json!({"key": "Enter"}), T);
    h.settle(Duration::from_secs(5));
    assert!(h.app.ui.renaming_mask.is_none());
    assert_eq!(develop(&h).masks[0].name, "Sky");
    // hover the row, then click its eye
    let r = h.request("ui.hoverWidget", json!({"id": format!("mask:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(Duration::from_secs(5));
    let r = h.request("ui.clickWidget", json!({"id": format!("maskVisible:{first}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!develop(&h).masks[0].visible);
    // overlay colour: no params = next swatch
    let before = h.app.ui.mask_overlay_color;
    exec(&mut h, "view.maskOverlayColor", json!({}));
    let all = crate::panels::masking::OVERLAY_COLORS;
    let i = all.iter().position(|c| *c == before).unwrap();
    assert_eq!(h.app.ui.mask_overlay_color, all[(i + 1) % all.len()]);
}

#[test]
fn command_drag_straightens_in_crop() {
    let mut h = detail("panel.crop");
    let tool = h.app.ui.tool.clone();
    let before = develop(&h).crop.geometry.angle;
    let r = h.request(
        "ui.pointer",
        json!({"events": [{"kind": "down", "x": 0.3, "y": 0.5}, {"kind": "drag", "x": 0.45, "y": 0.51}, {"kind": "drag", "x": 0.6, "y": 0.53}, {"kind": "up", "x": 0.6, "y": 0.53}], "cmd": true}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    let angle = develop(&h).crop.geometry.angle;
    assert!(angle != before && (1.0..15.0).contains(&angle.abs()), "a slightly tilted line straightens: {angle}");
    assert_eq!(h.app.ui.tool, tool, "the crop tool stays active");
    assert!(h.app.gesture.is_none());
    h.settle(SETTLE);
}

#[test]
fn double_click_in_crop_box_applies_the_crop() {
    let mut h = detail("panel.crop");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Crop);
    let c = h.app.image_rect.expect("image on screen").center();
    let r = h.request("ui.click", json!({"x": c.x, "y": c.y, "count": 2}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Edit, "double-click leaves the crop tool");
}

#[test]
fn option_digit_toggles_keyword_from_set() {
    let mut h = detail("panel.keywords");
    exec(&mut h, "photo.setMeta", json!({"addKeywords": ["alpha"]}));
    exec(&mut h, "photo.setMeta", json!({"removeKeywords": ["alpha"]}));
    let has = |h: &Headless| develop_photo_keywords(h).iter().any(|k| k == "alpha");
    assert!(!has(&h));
    let r = h.request("ui.key", json!({"key": "1", "alt": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(has(&h), "⌥1 adds the first recent keyword");
    let r = h.request("ui.clickWidget", json!({"id": "kwSet:1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!has(&h), "its button removes it again");
    h.settle(SETTLE);
}

fn develop_photo_keywords(h: &Headless) -> Vec<String> {
    let id = h.app.session.active().expect("active photo");
    h.app.session.catalog.photo(id).unwrap().meta.keywords.clone()
}

#[test]
fn smart_album_rule_editor_creates_and_edits() {
    let mut h = detail("panel.edit");
    exec(&mut h, "dialog.smartAlbum", json!({"name": "Keepers"}));
    // the editor starts with Rating ≥ 3; "+" adds a second rule
    let r = h.request("ui.clickWidget", json!({"id": "button:ruleAdd-rules-0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let Some(crate::state::Dialog::SmartRules { rules, .. }) = &mut h.app.ui.dialog else { panic!("no rule editor") };
    assert_eq!(rules.rules.len(), 2);
    rules.rules[1] = serde_json::from_value(json!({"field": "flag", "op": "isNot", "value": "reject"})).unwrap();
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    let a = h.app.session.catalog.albums().find(|a| a.name == "Keepers").expect("album").clone();
    let n = h.app.session.catalog.photos().filter(|p| !p.deleted && p.rating >= 3 && p.flag != lightcraft_catalog::Flag::Reject).count();
    assert_eq!(h.app.session.catalog.album_count(a.id), n);
    // edit: back to one rule
    exec(&mut h, "dialog.smartAlbum", json!({"id": a.id.0}));
    let r = h.request("ui.clickWidget", json!({"id": "button:ruleRemove-rules-1"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let r = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    let n3 = h.app.session.catalog.photos().filter(|p| !p.deleted && p.rating >= 3).count();
    assert_eq!(h.app.session.catalog.album_count(a.id), n3);
    h.settle(SETTLE);
}

#[test]
fn g_toggles_grids_and_shift_g_starts_guided_upright() {
    let mut h = detail("panel.edit");
    let key = |h: &mut Headless, shift: bool| {
        let r = h.request("ui.key", json!({"key": "g", "shift": shift}), T);
        assert_eq!(r["ok"], true, "{r}");
    };
    key(&mut h, false);
    assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid);
    key(&mut h, false);
    assert_eq!(h.app.ui.view, crate::state::ViewMode::SquareGrid);
    key(&mut h, false);
    assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid);
    key(&mut h, true);
    assert_eq!((h.app.ui.view, h.app.ui.right, h.app.ui.tool.as_str()), (crate::state::ViewMode::Detail, RightPanel::Crop, "guidedUpright"));
    assert_eq!(develop(&h).geometry.upright, lightcraft_develop::Upright::Guided);
    h.settle(SETTLE);
}

#[test]
fn luminance_range_controls_and_map() {
    let mut h = detail("panel.masking");
    exec(&mut h, "mask.add", json!({"kind": "luminanceRange", "lo": 0.6, "hi": 1.0}));
    h.settle(SETTLE);
    let lum = |h: &Headless| match &develop(h).masks[0].components[0].shape {
        MaskShape::LuminanceRange { lo, hi, lo_feather, .. } => (*lo, *hi, *lo_feather),
        s => panic!("{s:?}"),
    };
    let undo0 = h.app.session.undo.len();
    // drag the high handle from the right end to 80 %
    let r = h.request("ui.dragWidget", json!({"id": "lumRange:0", "fx": 1.0, "fy": 0.5, "dx": -48.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    let (lo, hi, _) = lum(&h);
    assert_eq!(lo, 0.6, "the nearer handle moves");
    assert!(hi < 0.9 && hi > 0.6, "{hi}");
    assert_eq!(h.app.session.undo.len(), undo0 + 1, "one undo step per drag");
    // Show Luminance Map: black-and-white overlay, and back
    let r = h.request("ui.clickWidget", json!({"id": "check:lumMap0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.mask_overlay && h.app.ui.mask_overlay_mode == "colorOnBw");
    let r = h.request("ui.clickWidget", json!({"id": "check:lumMap0"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_ne!(h.app.ui.mask_overlay_mode, "colorOnBw");
    h.settle(SETTLE);
}

#[test]
fn b_adds_to_quick_collection_in_the_grid_and_brushes_in_edit() {
    let mut h = detail("panel.edit");
    // in the loupe B is the masking brush
    let r = h.request("ui.key", json!({"key": "b"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.tool, "brush");
    assert!(h.app.session.catalog.quick_collection().is_none());
    // in the grid it adds the selection to the Quick Collection
    exec(&mut h, "view.photoGrid", json!({}));
    let r = h.request("ui.key", json!({"key": "b"}), T);
    assert_eq!(r["ok"], true, "{r}");
    let q = h.app.session.catalog.quick_collection().expect("quick collection");
    assert_eq!(h.app.session.catalog.album_count(q), 1);
    h.settle(SETTLE);
}

#[test]
fn local_folder_tree_expands_and_browses() {
    let base = std::env::temp_dir().join(format!("lc-ui-tree-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Trip/Day 1")).unwrap();
    std::fs::create_dir_all(base.join(".hidden")).unwrap();
    let mut h = detail("panel.edit");
    exec(&mut h, "view.leftPanel", json!({"show": true}));
    exec(&mut h, "library.browse", json!({"path": base.to_string_lossy()}));
    h.settle(SETTLE);
    let base_s = base.to_string_lossy().to_string();
    let r = h.request("ui.clickWidget", json!({"id": format!("folderToggle:{base_s}")}), T);
    assert_eq!(r["ok"], true, "{r}");
    for _ in 0..3 {
        h.step();
    }
    let trip = base.join("Trip").to_string_lossy().to_string();
    let r = h.request("ui.clickWidget", json!({"id": format!("source:local:{trip}")}), T);
    assert_eq!(r["ok"], true, "the subfolder is listed: {r}");
    assert_eq!(h.app.session.browse.as_ref().map(|b| b.path.clone()), Some(trip.clone()), "clicking it browses it");
    let hidden = base.join(".hidden").to_string_lossy().to_string();
    let r = h.request("ui.clickWidget", json!({"id": format!("source:local:{hidden}")}), T);
    assert_ne!(r["ok"], true, "hidden folders are not listed");
    let _ = std::fs::remove_dir_all(&base);
    h.settle(SETTLE);
}

#[test]
fn slideshow_advances_pauses_and_ends() {
    let mut h = detail("panel.edit");
    let first = h.app.session.active();
    exec(&mut h, "view.slideshow", json!({"interval": 0.5}));
    assert!(h.app.ui.fullscreen && h.app.ui.slideshow.is_some());
    // simulated time runs with the frames
    let mut moved = false;
    for _ in 0..200 {
        h.step();
        if h.app.session.active() != first {
            moved = true;
            break;
        }
    }
    assert!(moved, "the next photo comes up");
    // Space pauses: nothing moves
    let r = h.request("ui.key", json!({"key": "space"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.slideshow.is_some_and(|s| s.2), "paused");
    let held = h.app.session.active();
    for _ in 0..120 {
        h.step();
    }
    assert_eq!(h.app.session.active(), held);
    // Esc ends it
    let r = h.request("ui.key", json!({"key": "escape"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!h.app.ui.fullscreen && h.app.ui.slideshow.is_none());
    h.settle(SETTLE);
}

#[test]
fn geometry_slider_drag_marks_the_grid() {
    let mut h = detail("panel.crop");
    let spec = lightcraft_develop::controls::find("geometry.vertical").unwrap();
    let start = crate::widgets::SliderOut { value: None, drag_started: true, drag_stopped: false, reset: false };
    crate::panels::edit::apply_slider_out(&mut h.app, spec, start, |_, _| Ok(serde_json::Value::Null));
    assert_eq!(h.app.ui.dragging_control.as_deref(), Some("geometry.vertical"));
    h.step();
    let stop = crate::widgets::SliderOut { value: None, drag_started: false, drag_stopped: true, reset: false };
    crate::panels::edit::apply_slider_out(&mut h.app, spec, stop, |_, _| Ok(serde_json::Value::Null));
    assert!(h.app.ui.dragging_control.is_none());
    h.settle(SETTLE);
}

#[test]
fn edit_in_external_editor_opens_the_copy() {
    let dir = std::env::temp_dir().join(format!("lc-ui-ext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let opened: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> = Default::default();
    let o = opened.clone();
    let services = Services {
        png: None,
        open_with: Some(Box::new(move |path: &str, app: &str| {
            o.lock().unwrap().push((path.to_string(), app.to_string()));
            Ok(())
        })),
        ..Default::default()
    };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo().with_fs(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    h.app.ui.settings.external_editor = "PhotoCraft".into();
    let r = h.request("engine.execute", json!({"command": "photo.editInExternal", "params": {"dir": dir.to_string_lossy()}}), T);
    assert_eq!(r["ok"], true, "{r}");
    let calls = opened.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.ends_with("-Edit.tif") && std::path::Path::new(&calls[0].0).exists(), "{calls:?}");
    assert_eq!(calls[0].1, "PhotoCraft");
    let _ = std::fs::remove_dir_all(&dir);
    h.settle(SETTLE);
}

#[test]
fn second_window_shows_the_active_photo() {
    let mut h = detail("panel.edit");
    exec(&mut h, "view.photoGrid", json!({}));
    exec(&mut h, "view.secondWindow", json!({"show": true}));
    h.settle(SETTLE);
    // headless has no native windows: it's embedded, and renders the photo for its own slot
    let tex = h.app.renderer.textures.get(&crate::render::Slot::Second).map(|t| t.photo);
    assert_eq!(tex, h.app.session.active(), "the second window has its own render of the active photo");
    let r = h.request("ui.clickWidget", json!({"id": "view:secondWindow"}), T);
    assert_eq!(r["ok"], true, "on screen: {r}");
    exec(&mut h, "view.secondWindow", json!({}));
    assert!(!h.app.ui.second_window);
    h.settle(SETTLE);
}

/// Frame time of the photo grid on a 100k-photo library (ignored:
/// `cargo test --release -p lightcraft-ui-egui -- --ignored grid_frame_100k --nocapture`).
#[test]
#[ignore]
fn grid_frame_100k() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    let mut session = lightcraft_engine::Session::new();
    let ops = (0..100_000u64)
        .map(|i| {
            let mut p = Photo::new(
                PhotoId(i + 1),
                Source::Demo { scene: (i % 20) as u32 },
                &format!("IMG_{i:06}.jpg"),
                "JPEG",
                6000,
                4000 - (i % 3) as u32 * 1000,
                "2026-01-01T00:00:00",
            );
            p.captured = Some(format!("20{:02}-{:02}-{:02}T10:00:00", 10 + i % 16, 1 + i % 12, 1 + i % 28));
            p.meta.keywords = vec![format!("kw{}", i % 300)];
            Op::AddPhoto { photo: Box::new(p) }
        })
        .collect();
    session.commit("Add", Op::Batch { ops }).unwrap();
    let app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    let mut h = Headless::new(app, [1600.0, 1000.0], 1.0);
    h.app.ui.view = crate::state::ViewMode::PhotoGrid;
    h.app.ui.left_panel = true;
    for _ in 0..5 {
        h.step();
    }
    let t = std::time::Instant::now();
    let n = 30;
    for _ in 0..n {
        h.step();
    }
    let ms = t.elapsed().as_secs_f64() * 1e3 / n as f64;
    eprintln!("grid frame at 100k photos: {ms:.1} ms (UI thread, renders excluded)");
}

#[test]
fn keyword_painter_toggles_on_click() {
    let mut h = detail("panel.edit");
    exec(&mut h, "tool.keywordPainter", json!({"keyword": "harbour"}));
    assert_eq!(h.app.ui.view, crate::state::ViewMode::PhotoGrid);
    h.settle(SETTLE);
    let target = h.app.session.visible_cloned()[0];
    let has = |h: &Headless| h.app.session.catalog.photo(target).unwrap().meta.keywords.iter().any(|k| k == "harbour");
    let sel = h.app.session.selection.clone();
    // a click: press and release on separate frames, no movement
    let click = |h: &mut Headless| {
        let w = h.request("ui.widgets", json!({}), T);
        let r = w["result"]
            .as_array()
            .and_then(|a| a.iter().find(|x| x["id"] == format!("thumb:{}", target.0)))
            .map(|x| x["rect"].clone())
            .expect("thumb on screen");
        let (x, y) = (r[0].as_f64().unwrap() + r[2].as_f64().unwrap() / 2.0, r[1].as_f64().unwrap() + r[3].as_f64().unwrap() / 2.0);
        let r = h.request("ui.drag", json!({"x": x, "y": y, "toX": x, "toY": y, "steps": 2}), T);
        assert_eq!(r["ok"], true, "{r}");
    };
    click(&mut h);
    assert!(has(&h), "painted");
    assert_eq!(h.app.session.selection, sel, "painting doesn't select");
    click(&mut h);
    assert!(!has(&h), "a second click takes it away");
    h.request("ui.key", json!({"key": "escape"}), T);
    assert!(h.app.ui.keyword_painter.is_none());
    h.settle(SETTLE);
}

#[test]
fn grid_info_cycles_caption() {
    let mut h = detail("panel.edit");
    assert_eq!(exec(&mut h, "view.gridInfo", json!({}))["info"], "exposure");
    assert_eq!(exec(&mut h, "view.gridInfo", json!({}))["info"], "date");
    assert_eq!(exec(&mut h, "view.gridInfo", json!({"info": "filename"}))["info"], "filename");
    let r = h.request("engine.execute", json!({"command": "view.gridInfo", "params": {"info": "lens"}}), T);
    assert_ne!(r["ok"], true);
    exec(&mut h, "view.squareGrid", json!({}));
    exec(&mut h, "view.gridInfo", json!({"info": "exposure"}));
    h.settle(SETTLE);
}

#[test]
fn reference_view_pins_a_photo_beside_the_active_one() {
    let mut h = detail("panel.edit");
    let first = h.app.session.active().unwrap();
    exec(&mut h, "photo.setReference", json!({}));
    let r = h.request("ui.key", json!({"key": "r", "shift": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.view, crate::state::ViewMode::Reference);
    assert_eq!(h.app.ui.reference, Some(first.0));
    let active = h.app.session.active().unwrap();
    assert_ne!(active, first, "the next photo is the one being edited");
    // edits go to the active photo, the reference stays put
    exec(&mut h, "develop.set", json!({"control": "light.exposure", "value": 0.7}));
    assert_eq!(h.app.session.develop_of(active).unwrap().light.exposure, 0.7);
    assert_ne!(h.app.session.develop_of(first).unwrap().light.exposure, 0.7);
    h.settle(SETTLE);
    assert!(h.app.renderer.textures.get(&crate::render::Slot::Compare(0)).is_some_and(|t| t.photo == first), "the reference is drawn");
}

#[test]
fn soft_proofing_flags_out_of_gamut_colours_and_makes_proof_copies() {
    let mut h = detail("panel.edit");
    if h.app.ui.right != RightPanel::Edit {
        exec(&mut h, "panel.edit", json!({}));
    }
    assert_eq!(h.app.ui.right, RightPanel::Edit);
    h.app.renderer.keep_pixels = true;
    exec(&mut h, "develop.set", json!({"control": "color.saturation", "value": 100}));
    exec(&mut h, "develop.set", json!({"control": "color.vibrance", "value": 100}));
    let red = |h: &Headless| {
        let t = h.app.renderer.textures.get(&crate::render::Slot::Main).expect("loupe render");
        t.pixels.as_ref().expect("pixels kept").pixels.iter().filter(|c| c.r() == 255 && c.g() == 0 && c.b() == 0).count()
    };
    h.settle(SETTLE);
    let before = red(&h);
    // S in the loupe: soft proofing on; the warning is the proof's own setting
    let r = h.request("ui.key", json!({"key": "s"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.ui.soft_proof);
    let st = exec(&mut h, "view.softProof", json!({"space": "srgb", "destWarning": true}));
    assert_eq!(st, json!({"on": true, "space": "srgb", "destWarning": true, "displayWarning": false}));
    h.settle(SETTLE);
    assert!(red(&h) > before, "out-of-gamut colours are painted red");
    let r = h.request("engine.execute", json!({"command": "view.softProof", "params": {"space": "cmyk"}}), T);
    assert_ne!(r["ok"], true);
    // Create Proof Copy: a virtual copy named after the proof
    let n = h.app.session.catalog.photos().count();
    let r = h.request("ui.clickWidget", json!({"id": "button:createProofCopy"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.session.catalog.photos().count(), n + 1);
    let copy = (**h.app.session.catalog.photos().max_by_key(|p| p.id.0).unwrap()).clone();
    assert_eq!(copy.copy_name.as_deref(), Some("Proof Copy (sRGB)"));
    // S again turns it off; in a grid S is Expand/Collapse Stack (the copy made a stack)
    h.request("ui.key", json!({"key": "s"}), T);
    assert!(!h.app.ui.soft_proof);
    exec(&mut h, "view.photoGrid", json!({}));
    let stack = h.app.session.catalog.stack_of(copy.id).cloned().expect("stacked with its original");
    h.request("ui.key", json!({"key": "s"}), T);
    assert!(!h.app.ui.soft_proof);
    assert_ne!(h.app.session.catalog.stack_of(copy.id).unwrap().collapsed, stack.collapsed, "S toggled the stack");
}
