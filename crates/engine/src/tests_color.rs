//! Sampling commands: Point Color, the targeted adjustment tool, red eye.

use serde_json::json;

use crate::Session;

fn demo() -> Session {
    Session::with_demo()
}

fn active_dev(s: &Session) -> lightcraft_develop::DevelopSettings {
    (*s.develop_of(s.active().unwrap()).unwrap()).clone()
}

/// Spread of the channels (a saturation proxy) at normalized `(x, y)`.
fn chroma_at(img: &lightcraft_raster::Rgba8, x: f64, y: f64) -> i32 {
    let p = img.data[(img.height as f64 * y) as usize * img.width + (img.width as f64 * x) as usize];
    p[0].max(p[1]).max(p[2]) as i32 - p[0].min(p[1]).min(p[2]) as i32
}

#[test]
fn point_color_pick_and_adjust() {
    let mut s = demo();
    let id = s.active().unwrap();
    let before = s.render_now(id, 384, 384).unwrap().image;
    let r = s.execute("pointColor.pick", &json!({"x": 0.5, "y": 0.08})).unwrap();
    assert_eq!(r["index"], 0);
    let d = active_dev(&s);
    assert_eq!(d.point_colors.len(), 1);
    assert!(d.point_colors[0].chroma > 0.01, "the sky has colour: {r}");
    // the sample's sliders are develop controls (and listed as such)
    s.execute("develop.set", &json!({"control": "pointColor.0.satShift", "value": -100})).unwrap();
    assert_eq!(active_dev(&s).point_colors[0].sat_shift, -100.0);
    let ctl = s.execute("develop.controls", &json!({"section": "pointColor"})).unwrap();
    assert!(ctl.as_array().unwrap().iter().any(|c| c["id"] == "pointColor.0.satShift" && c["value"] == -100.0), "{ctl}");
    // the picked colour is desaturated in the render
    let after = s.render_now(id, 384, 384).unwrap().image;
    assert!(
        chroma_at(&after, 0.5, 0.08) + 8 < chroma_at(&before, 0.5, 0.08),
        "{} vs {}",
        chroma_at(&after, 0.5, 0.08),
        chroma_at(&before, 0.5, 0.08)
    );
    // at most 8 samples; delete
    for _ in 0..7 {
        s.execute("pointColor.pick", &json!({"x": 0.3, "y": 0.5})).unwrap();
    }
    assert!(s.execute("pointColor.pick", &json!({"x": 0.3, "y": 0.5})).is_err());
    s.execute("pointColor.delete", &json!({"index": 3})).unwrap();
    assert_eq!(active_dev(&s).point_colors.len(), 7);
    // survives the settings JSON (what the catalog and XMP sidecars store)
    let d = active_dev(&s);
    assert_eq!(lightcraft_develop::DevelopSettings::from_json(&d.to_json()).unwrap(), d);
}

#[test]
fn red_eye_commands_and_controls() {
    let mut s = demo();
    let r = s.execute("redeye.add", &json!({"center": [0.3, 0.4], "rx": 0.03, "ry": 0.02})).unwrap();
    assert_eq!(r["index"], 0);
    s.execute("redeye.add", &json!({"center": [0.6, 0.4], "rx": 0.03, "ry": 0.02, "darken": 80})).unwrap();
    let d = active_dev(&s);
    assert_eq!(d.red_eye.len(), 2);
    assert_eq!((d.red_eye[0].pupil_size, d.red_eye[0].darken, d.red_eye[1].darken), (50.0, 50.0, 80.0));
    // per-eye sliders are develop controls
    s.execute("develop.set", &json!({"values": {"redEye.1.pupilSize": 70, "redEye.0.darken": 140}})).unwrap();
    let d = active_dev(&s);
    assert_eq!((d.red_eye[1].pupil_size, d.red_eye[0].darken), (70.0, 100.0));
    let ctl = s.execute("develop.controls", &json!({"section": "redEye"})).unwrap();
    assert_eq!(ctl.as_array().unwrap().len(), 4, "{ctl}");
    // the render runs with eyes (detection falls back gracefully on a photo without red pupils)
    let id = s.active().unwrap();
    assert!(s.render_now(id, 200, 200).is_ok());
    s.execute("redeye.delete", &json!({"index": 0})).unwrap();
    assert_eq!(active_dev(&s).red_eye.len(), 1);
    assert!(s.execute("redeye.delete", &json!({"index": 5})).is_err());
    assert!(s.execute("redeye.add", &json!({"rx": 0.03})).is_err());
    let d = active_dev(&s);
    assert_eq!(lightcraft_develop::DevelopSettings::from_json(&d.to_json()).unwrap(), d);
}

#[test]
fn pet_eye_catchlight_command() {
    let mut s = demo();
    s.execute("redeye.add", &json!({"center": [0.3, 0.4], "rx": 0.03, "ry": 0.03})).unwrap();
    s.execute("redeye.add", &json!({"center": [0.6, 0.4], "rx": 0.03, "ry": 0.03, "pet": true})).unwrap();
    assert!(s.execute("redeye.catchlight", &json!({"index": 0})).is_err(), "red eyes have no catchlight");
    s.execute("redeye.catchlight", &json!({"index": 1})).unwrap();
    assert_eq!(active_dev(&s).red_eye[1].catchlight, Some(lightcraft_geom::Point::new(-0.35, -0.35)));
    s.execute("redeye.catchlight", &json!({"index": 1, "offset": [0.2, -3.0]})).unwrap();
    assert_eq!(active_dev(&s).red_eye[1].catchlight, Some(lightcraft_geom::Point::new(0.2, -1.0)));
    s.execute("redeye.catchlight", &json!({"index": 1, "on": false})).unwrap();
    assert_eq!(active_dev(&s).red_eye[1].catchlight, None);
}

#[test]
fn targeted_adjustment_on_curve_and_mixer() {
    let mut s = demo();
    // the bright sky lies in the upper regions of the parametric curve
    let r = s.execute("develop.targeted", &json!({"target": "curve", "x": 0.5, "y": 0.35, "delta": 20})).unwrap();
    let (k, v) = r.as_object().unwrap().iter().next().map(|(k, v)| (k.clone(), v.clone())).unwrap();
    assert!(k == "curve.lights" || k == "curve.highlights", "{r}");
    assert_eq!(v, 20.0);
    assert_eq!(lightcraft_develop::controls::get(&active_dev(&s), &k), Some(20.0));
    // the dark trees in the lower ones
    let r = s.execute("develop.targeted", &json!({"target": "curve", "x": 0.4, "y": 0.62, "delta": -10})).unwrap();
    assert!(r.get("curve.shadows").is_some() || r.get("curve.darks").is_some(), "{r}");
    // point curve: a point appears at the sampled input, raised by delta levels
    s.execute("develop.targeted", &json!({"target": "curve", "channel": "master", "x": 0.5, "y": 0.35, "delta": 12.75})).unwrap();
    let m = active_dev(&s).curve.master;
    assert_eq!(m.len(), 3);
    assert!((m[1].y - m[1].x - 0.05).abs() < 1e-6, "{m:?}");
    // a second step moves the same point
    s.execute("develop.targeted", &json!({"target": "curve", "channel": "master", "x": 0.5, "y": 0.35, "delta": 12.75})).unwrap();
    assert_eq!(active_dev(&s).curve.master.len(), 3);
    // colour mixer: the purple sky's bands lose saturation, the dominant one by the full delta
    let r = s.execute("develop.targeted", &json!({"target": "sat", "x": 0.5, "y": 0.1, "delta": -30})).unwrap();
    let o = r.as_object().unwrap();
    assert!(!o.is_empty() && o.keys().all(|k| k.starts_with("mixer.") && k.ends_with(".sat")), "{r}");
    assert!(o.values().any(|v| v.as_f64() == Some(-30.0)), "{r}");
    assert!(o.values().all(|v| v.as_f64().unwrap() < 0.0));
    assert!(s.execute("develop.targeted", &json!({"target": "nope", "x": 0.5, "y": 0.5, "delta": 1})).is_err());
}
