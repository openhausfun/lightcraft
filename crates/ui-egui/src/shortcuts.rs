//! Keyboard shortcuts: parse `Cmd+Shift+X` style strings and dispatch UI and engine commands.

use egui::{Key, Modifiers};
use serde_json::json;

use crate::LightcraftApp;

/// Secondary key bindings for commands that already exist: `(shortcut, command id, params JSON)`.
/// They complement the primary shortcut declared on the command (Lightroom-desktop keys that our
/// primary keymap assigns elsewhere, see docs/parity.md → Shortcuts). Shown in Help → Keyboard Shortcuts.
pub const ALIASES: &[(&str, &str, &str)] = &[
    ("Cmd+D", "library.selectNone", "{}"),
    ("Shift+E", "dialog.export", "{}"),
    ("Cmd+E", "app.exportPrevious", "{}"),
    ("Space", "view.zoomToggle", "{}"),
    ("Shift+M", "version.create", "{}"),
    ("Shift+X", "photo.flag", r#"{"flag": "reject", "advance": true}"#),
    ("Shift+Z", "photo.flag", r#"{"flag": "pick", "advance": true}"#),
    ("Shift+U", "photo.flag", r#"{"flag": "none", "advance": true}"#),
    // Shift+[ / Shift+] arrive as { / } on most layouts
    ("Shift+{", "brush.featherLess", "{}"),
    ("Shift+}", "brush.featherMore", "{}"),
    // keyword set: ⌥1–⌥9 toggle the current set's keywords on the selection
    ("Alt+1", "keyword.toggleFromSet", r#"{"index": 1}"#),
    ("Alt+2", "keyword.toggleFromSet", r#"{"index": 2}"#),
    ("Alt+3", "keyword.toggleFromSet", r#"{"index": 3}"#),
    ("Alt+4", "keyword.toggleFromSet", r#"{"index": 4}"#),
    ("Alt+5", "keyword.toggleFromSet", r#"{"index": 5}"#),
    ("Alt+6", "keyword.toggleFromSet", r#"{"index": 6}"#),
    ("Alt+7", "keyword.toggleFromSet", r#"{"index": 7}"#),
    ("Alt+8", "keyword.toggleFromSet", r#"{"index": 8}"#),
    ("Alt+9", "keyword.toggleFromSet", r#"{"index": 9}"#),
];

pub fn parse(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part {
            "Cmd" => m.command = true,
            "Shift" => m.shift = true,
            "Alt" => m.alt = true,
            "Ctrl" => m.ctrl = true,
            // "Delete" means the key labelled ⌫ (egui's Backspace); forward-delete also matches, see `matches`.
            "Delete" => key = Some(Key::Backspace),
            k => {
                key = Key::from_name(k).or(match k {
                    "Right" => Some(Key::ArrowRight),
                    "Left" => Some(Key::ArrowLeft),
                    "Up" => Some(Key::ArrowUp),
                    "Down" => Some(Key::ArrowDown),
                    "\\" => Some(Key::Backslash),
                    "/" => Some(Key::Slash),
                    "=" => Some(Key::Equals),
                    "-" => Some(Key::Minus),
                    "[" => Some(Key::OpenBracket),
                    "]" => Some(Key::CloseBracket),
                    "'" => Some(Key::Quote),
                    "," => Some(Key::Comma),
                    _ => None,
                })
            }
        }
    }
    key.map(|k| (m, k))
}

fn matches(i: &egui::InputState, m: Modifiers, k: Key) -> bool {
    i.events.iter().any(|e| match e {
        egui::Event::Key { key, pressed: true, modifiers, .. } => {
            // `Ctrl` is the physical Control key (on macOS distinct from Cmd; elsewhere Cmd = Ctrl);
            // "Delete" (⌫ = Backspace) also matches forward-delete
            let ctrl_ok = if m.ctrl { modifiers.ctrl } else { !modifiers.ctrl || modifiers.command };
            let cmd_ok = m.ctrl || modifiers.command == m.command;
            (*key == k || (k == Key::Backspace && *key == Key::Delete)) && ctrl_ok && cmd_ok && modifiers.shift == m.shift && modifiers.alt == m.alt
        }
        _ => false,
    })
}

pub fn handle(app: &mut LightcraftApp, ctx: &egui::Context) {
    // don't steal keys from text fields
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    let mut fire: Vec<String> = Vec::new();
    // shortcuts the native menu bar handles (it consumes those key presses itself)
    let native = |sc: &str| app.native_shortcuts.contains(sc);
    let mut aliased: Vec<(&str, serde_json::Value)> = Vec::new();
    ctx.input(|i| {
        let mut ui_keys = Vec::new();
        for (id, _, sc, _) in crate::menus::UI_COMMANDS {
            if let Some((m, k)) = sc.and_then(parse) {
                ui_keys.push((m, k));
                if !native(sc.unwrap_or_default()) && matches(i, m, k) {
                    fire.push(id.to_string());
                }
            }
        }
        for c in lightcraft_engine::command_specs() {
            // A UI command bound to the same key wraps the engine command (e.g. `W` opens the
            // White Balance Selector tool rather than sampling without a point): the UI one wins.
            if let Some((m, k)) = c.shortcut.filter(|s| !native(s)).and_then(parse)
                && !ui_keys.contains(&(m, k))
                && matches(i, m, k)
            {
                fire.push(c.id.to_string());
            }
        }
        for (sc, id, params) in ALIASES {
            if let Some((m, k)) = parse(sc).filter(|_| !native(sc))
                && matches(i, m, k)
            {
                aliased.push((id, serde_json::from_str(params).unwrap_or_default()));
            }
        }
        // rating 0-5, colour labels 6-9 (with Shift: and advance)
        for (n, key) in [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5].iter().enumerate() {
            if matches(i, Modifiers::NONE, *key) && !native(&n.to_string()) {
                fire.push(format!("rate:{n}:0"));
            }
            if matches(i, Modifiers::SHIFT, *key) {
                fire.push(format!("rate:{n}:1"));
            }
        }
        for (label, key, sc) in [("red", Key::Num6, "6"), ("yellow", Key::Num7, "7"), ("green", Key::Num8, "8"), ("blue", Key::Num9, "9")] {
            if matches(i, Modifiers::NONE, key) && !native(sc) {
                fire.push(format!("label:{label}"));
            }
        }
    });
    use crate::panels::compare;
    // rating/flag/label keys: in Compare/Survey they act on the active photo only; Shift+key or
    // Auto Advance then moves on (next candidate in Compare, next photo elsewhere)
    let cull = |app: &mut LightcraftApp, id: &str, mut params: serde_json::Value, advance: bool| {
        compare::target_active(app, &mut params);
        let ok = app.run(id, params).is_ok();
        if ok && (advance || app.ui.auto_advance) {
            compare::advance(app);
        }
    };
    for (id, params) in aliased {
        // Space pauses / resumes a slideshow
        if id == "view.zoomToggle"
            && let Some((interval, _, paused)) = app.ui.slideshow
        {
            let now = ctx.input(|i| i.time);
            app.ui.slideshow = Some((interval, now + interval, !paused));
            app.toast(ctx, if paused { "Slideshow resumed" } else { "Slideshow paused" });
            continue;
        }
        // flag/rate aliases (Shift+X…) go through the culling path: active photo in Compare/Survey,
        // `advance` moves to the next candidate there
        if matches!(id, "photo.flag" | "photo.rate" | "photo.label") {
            let mut params = params;
            let advance = params.get("advance").and_then(serde_json::Value::as_bool).unwrap_or(false);
            if let Some(o) = params.as_object_mut() {
                o.remove("advance");
            }
            cull(app, id, params, advance);
        } else {
            let _ = app.run(id, params);
        }
    }
    for f in fire {
        if let Some(rest) = f.strip_prefix("rate:") {
            let (n, adv) = rest.split_once(':').unwrap_or(("0", "0"));
            cull(app, "photo.rate", json!({"rating": n.parse::<u8>().unwrap_or(0)}), adv == "1");
            let label =
                if n == "0" { "Rating cleared".to_string() } else { crate::i18n::tr_format!("Rated {}", "★".repeat(n.parse().unwrap_or(0))) };
            app.toast(ctx, label);
        } else if let Some(l) = f.strip_prefix("label:") {
            cull(app, "photo.label", json!({"label": l}), false);
        } else if f == "view.softProof" && matches!(app.ui.view, crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid) {
            // S in a grid: expand / collapse the stack (Lightroom's Library binding)
            let _ = app.run("stack.toggle", json!({}));
        } else if compare::culling(app) && (f == "library.next" || f == "library.previous") {
            let d = if f == "library.next" { 1 } else { -1 };
            let _ = if app.ui.view == crate::state::ViewMode::Compare { compare::compare_step(app, d) } else { compare::survey_step(app, d) };
        } else if matches!(f.as_str(), "photo.pick" | "photo.reject" | "photo.unflag") && app.ui.right != crate::state::RightPanel::Crop {
            cull(app, &f, json!({}), false);
            match f.as_str() {
                "photo.pick" => app.toast(ctx, "Flagged as Pick"),
                "photo.reject" => app.toast(ctx, "Flagged as Reject"),
                _ => app.toast(ctx, "Unflagged"),
            }
        } else {
            // in the full-screen preview (no panels) I cycles the info overlay instead
            if f == "panel.info" && app.ui.fullscreen {
                let _ = app.run("view.infoOverlay", json!({}));
                continue;
            }
            // Delete acts on what's being edited: the active mask in the Masking panel; never the
            // photo while retouching (spots are removed from their own panel).
            if f == "photo.delete" {
                use crate::state::RightPanel as R;
                match app.ui.right {
                    R::Masking => {
                        if app.session.active_mask.is_some() {
                            let _ = app.run("mask.delete", json!({}));
                        }
                        continue;
                    }
                    R::Remove => {
                        if app.session.active_spot.is_some() {
                            let _ = app.run("spot.delete", json!({}));
                        }
                        continue;
                    }
                    R::RedEye => continue,
                    _ => {}
                }
                if crate::menus::confirm_delete(app) {
                    continue;
                }
            }
            // B: the brush while editing; in the grids, add to the target album (Quick Collection)
            if f == "tool.brush" && matches!(app.ui.view, crate::state::ViewMode::PhotoGrid | crate::state::ViewMode::SquareGrid) {
                if let Ok(r) = app.run("album.toggleTarget", json!({})) {
                    let n = app.session.targets(&json!({})).len();
                    let what = if n == 1 { "photo".to_string() } else { crate::i18n::tr_format!("{n} photos", n = n) };
                    let name = r["name"].as_str().unwrap_or("Quick Collection").to_string();
                    app.toast(ctx, if r["added"] == true { format!("Added {what} to {name}") } else { format!("Removed {what} from {name}") });
                }
                continue;
            }
            // X is both reject (library) and swap crop aspect (crop tool)
            if f == "photo.reject" && app.ui.right == crate::state::RightPanel::Crop {
                let _ = app.run("crop.rotateAspect", json!({}));
                continue;
            }
            if f == "crop.rotateAspect" && app.ui.right != crate::state::RightPanel::Crop {
                continue;
            }
            // / refreshes the selected spot's source in the Remove tool (the filmstrip elsewhere)
            if f == "view.filmstrip" && app.ui.right == crate::state::RightPanel::Remove && app.session.active_spot.is_some() {
                let _ = app.run("spot.refreshSource", json!({}));
                continue;
            }
            // Shift+O cycles the mask overlay colour while masking (the crop overlay elsewhere)
            if f == "view.cropOverlay" && app.ui.right == crate::state::RightPanel::Masking {
                let _ = app.run("view.maskOverlayColor", json!({}));
                continue;
            }
            // while cropping: O cycles the guides, Shift+O their orientation, A locks the aspect
            if app.ui.right == crate::state::RightPanel::Crop {
                let crop_key = match f.as_str() {
                    "view.maskOverlay" => Some(("view.cropOverlay", json!({}))),
                    "view.cropOverlay" => Some(("view.cropOverlayOrientation", json!({}))),
                    "view.visualizeSpots" => Some(("crop.aspect", json!({"aspect": "toggle"}))),
                    _ => None,
                };
                if let Some((cmd, p)) = crop_key {
                    let _ = app.run(cmd, p);
                    continue;
                }
            }
            let _ = app.run(&f, json!({}));
            match f.as_str() {
                "photo.pick" => app.toast(ctx, "Flagged as Pick"),
                "photo.reject" => app.toast(ctx, "Flagged as Reject"),
                "photo.unflag" => app.toast(ctx, "Unflagged"),
                "edit.undo" => app.toast(ctx, "Undo"),
                "edit.redo" => app.toast(ctx, "Redo"),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn delete_means_the_backspace_key() {
        assert_eq!(parse("Delete"), Some((Modifiers::NONE, Key::Backspace)));
        assert_eq!(parse("Cmd+Delete"), Some((Modifiers::COMMAND, Key::Backspace)));
    }
    use super::*;

    #[test]
    fn parses_shortcuts() {
        let (m, k) = parse("Cmd+Shift+Z").unwrap();
        assert!(m.command && m.shift && !m.alt);
        assert_eq!(k, Key::Z);
        assert_eq!(parse("Right").unwrap().1, Key::ArrowRight);
        assert_eq!(parse("\\").unwrap().1, Key::Backslash);
        // every declared shortcut parses
        for (id, _, sc, _) in crate::menus::UI_COMMANDS {
            if let Some(sc) = sc {
                assert!(parse(sc).is_some(), "{id}: {sc}");
            }
        }
        for c in lightcraft_engine::command_specs() {
            if let Some(sc) = c.shortcut {
                assert!(parse(sc).is_some(), "{}: {sc}", c.id);
            }
        }
    }

    #[test]
    fn aliases_parse_and_target_existing_commands() {
        let ui: Vec<&str> = crate::menus::UI_COMMANDS.iter().map(|c| c.0).collect();
        for (sc, id, params) in ALIASES {
            assert!(parse(sc).is_some(), "{id}: {sc}");
            assert!(ui.contains(id) || lightcraft_engine::find_command(id).is_some(), "alias {sc} → unknown command {id}");
            assert!(serde_json::from_str::<serde_json::Value>(params).is_ok(), "alias {sc}: bad params");
        }
    }

    /// Engine commands that intentionally share a key and are disambiguated by context in [`handle`].
    const CONTEXTUAL: &[(&str, &str)] = &[("photo.reject", "crop.rotateAspect")];

    /// No key fires two different actions (a UI command may shadow the engine command it wraps).
    #[test]
    fn no_conflicting_bindings() {
        let mut ui: Vec<((Modifiers, Key), String)> = Vec::new();
        for (id, _, sc, _) in crate::menus::UI_COMMANDS {
            if let Some(k) = sc.and_then(parse) {
                ui.push((k, id.to_string()));
            }
        }
        for (sc, id, _) in ALIASES {
            ui.push((parse(sc).unwrap(), format!("alias {id}")));
        }
        let mut engine: Vec<((Modifiers, Key), &str)> = Vec::new();
        for c in lightcraft_engine::command_specs() {
            if let Some(k) = c.shortcut.and_then(parse) {
                engine.push((k, c.id));
            }
        }
        for (sc, id, _) in ALIASES {
            let k = parse(sc).unwrap();
            assert!(!engine.iter().any(|(k2, _)| *k2 == k), "alias {sc} ({id}) shadows an engine shortcut");
        }
        for (i, (k, a)) in ui.iter().enumerate() {
            for (k2, b) in &ui[i + 1..] {
                assert!(k != k2, "{a} and {b} share a key");
            }
        }
        for (i, (k, a)) in engine.iter().enumerate() {
            for (k2, b) in &engine[i + 1..] {
                let contextual = CONTEXTUAL.iter().any(|(x, y)| (x == a && y == b) || (x == b && y == a));
                assert!(k != k2 || contextual, "{a} and {b} share a key");
            }
        }
    }
}
