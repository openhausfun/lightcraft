//! Headless tests of the photo grid's and the filmstrip's scrolling (issue #11): the user's
//! scroll position stays put until the active photo changes; the mouse wheel scrolls the
//! filmstrip sideways.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo(view: &str) -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    // big thumbnails: the demo library is several screens tall
    let r = h.request("ui.set", json!({"view": view, "thumbSize": 480.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    let first = h.app.session.visible_cloned()[0].0;
    let r = h.request("engine.execute", json!({"command": "library.select", "params": {"ids": [first]}}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

/// Run `n` frames (plus a settle: thumbnails and renders finishing must not move anything).
fn idle(h: &mut Headless, n: usize) {
    h.settle(SETTLE);
    for _ in 0..n {
        h.step();
    }
}

fn grid_y(h: &Headless) -> f32 {
    h.app.grid_scroll.expect("grid drawn")
}

fn film_x(h: &Headless) -> f32 {
    h.app.film_scroll.expect("filmstrip drawn")
}

fn widget(h: &Headless, id: &str) -> egui::Rect {
    h.app.widgets.iter().find(|(w, _)| w == id).map(|(_, r)| *r).unwrap_or_else(|| panic!("no widget {id}"))
}

#[test]
fn grid_keeps_the_users_scroll_position() {
    let mut h = demo("photoGrid");
    assert_eq!(grid_y(&h), 0.0);
    let canvas = h.app.canvas_rect.expect("grid canvas");
    // the mouse wheel scrolls the grid away from the active (first) photo…
    h.request("ui.move", json!({"x": canvas.center().x, "y": canvas.center().y}), T);
    for _ in 0..4 {
        h.request("ui.scroll", json!({"dy": -300.0}), T);
    }
    idle(&mut h, 120);
    let y = grid_y(&h);
    assert!(y > 600.0, "the wheel scrolled the grid ({y})");
    // …and it stays there: no snap back after a second and more of frames and finished renders
    idle(&mut h, 120);
    assert_eq!(grid_y(&h), y, "the grid snapped back");
    // dragging the scroll bar back up a bit, then releasing it
    let x = canvas.right() - 3.0;
    let r = h.request("ui.drag", json!({"x": x, "y": canvas.top() + 300.0, "toX": x, "toY": canvas.top() + 200.0}), T);
    assert_eq!(r["ok"], true, "{r}");
    idle(&mut h, 120);
    let y2 = grid_y(&h);
    assert!(y2 > 0.0 && (y2 - y).abs() > 1.0, "the scroll bar moved the grid ({y} → {y2})");
    // editing the active photo does not yank the grid back either
    let r = h.request("engine.execute", json!({"command": "develop.set", "params": {"control": "light.exposure", "value": 0.5}}), T);
    assert_eq!(r["ok"], true, "{r}");
    idle(&mut h, 60);
    assert_eq!(grid_y(&h), y2, "an edit snapped the grid back");
    // a new active photo (keyboard) brings it into view
    h.request("ui.key", json!({"key": "right"}), T);
    idle(&mut h, 60);
    let second = h.app.session.visible_cloned()[1].0;
    assert_eq!(h.app.session.active().map(|p| p.0), Some(second));
    let cell = widget(&h, &format!("thumb:{second}"));
    assert!(canvas.contains_rect(cell) || cell.intersects(canvas), "the new active photo is in view ({cell:?} vs {canvas:?})");
    assert!(grid_y(&h) < y2, "scrolled up to the second photo");
}

#[test]
fn filmstrip_wheel_scrolls_and_keeps_its_position() {
    let mut h = demo("detail");
    let first = h.app.session.visible_cloned()[0].0;
    let x0 = film_x(&h);
    let film = widget(&h, &format!("film:{first}"));
    // a vertical wheel over the filmstrip scrolls it sideways
    h.request("ui.move", json!({"x": 600.0, "y": film.center().y}), T);
    for _ in 0..3 {
        h.request("ui.scroll", json!({"dy": -200.0}), T);
    }
    idle(&mut h, 60);
    let x = film_x(&h);
    assert!(x > x0 + 300.0, "the wheel scrolled the filmstrip ({x0} → {x})");
    // a horizontal (trackpad) scroll still works
    h.request("ui.scroll", json!({"dx": 100.0}), T);
    idle(&mut h, 60);
    let x1 = film_x(&h);
    assert!(x1 < x - 50.0, "a horizontal scroll moved it back ({x} → {x1})");
    // it stays there
    idle(&mut h, 120);
    assert_eq!(film_x(&h), x1, "the filmstrip snapped back");
    // the next photo by keyboard brings the filmstrip to it
    h.request("ui.key", json!({"key": "right"}), T);
    idle(&mut h, 60);
    let second = h.app.session.visible_cloned()[1].0;
    assert_eq!(h.app.session.active().map(|p| p.0), Some(second));
    assert!(film_x(&h) < x1, "scrolled back to the active photo");
    let cell = widget(&h, &format!("film:{second}"));
    assert!(cell.left() >= 0.0 && cell.right() <= 1200.0, "the new active photo is in view ({cell:?})");
}
