use serde_json::{Value, json};

use crate::{Backend, Headless, PROTOCOL_VERSION, Server, call_tool, command_tool_name};

fn server() -> Server {
    Server::new(Box::new(Headless::demo()))
}

fn rpc(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let reply = s.handle_line(&line).expect("reply");
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["jsonrpc"], "2.0");
    assert_eq!(v["id"], id);
    v
}

#[test]
fn initialize_negotiates_version() {
    let mut s = server();
    let r = rpc(&mut s, 1, "initialize", json!({"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "lightcraft");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    let r = rpc(&mut s, 2, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert!(s.is_initialized());
}

#[test]
fn errors() {
    let mut s = server();
    let v: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(v["error"]["code"], -32700);
    assert_eq!(rpc(&mut s, 1, "nope", json!({}))["error"]["code"], -32601);
    assert_eq!(rpc(&mut s, 2, "tools/call", json!({}))["error"]["code"], -32602);
    assert!(s.handle_line("   ").is_none());
    // Unknown tools and failing commands are tool errors, not protocol errors.
    let r = rpc(&mut s, 3, "tools/call", json!({"name": "nope", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
    let r = rpc(&mut s, 4, "tools/call", json!({"name": "run_command", "arguments": {"command": "no.such"}}));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("unknown command"));
}

#[test]
fn tools_list_has_helpers_and_every_command() {
    let mut s = server();
    let r = rpc(&mut s, 1, "tools/list", json!({}));
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in ["list_commands", "run_command", "import", "set_develop", "render_photo", "export"] {
        assert!(names.contains(&n), "{n}");
    }
    // Headless: no UI tools.
    assert!(!names.contains(&"screenshot"));
    let hl = Headless::demo();
    for c in hl.session.commands() {
        assert!(!c.id.contains('_'), "command ids must not contain `_` ({})", c.id);
        let n = command_tool_name(c.id);
        assert!(names.contains(&n.as_str()), "{n}");
        assert!(n.len() <= 64 && n.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'), "{n}");
    }
    assert!(names.contains(&"cmd_app_export"));
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object");
    }
    // Compact mode keeps only the helpers.
    let mut s = Server::new(Box::new(Headless::demo())).with_command_tools(false);
    let r = rpc(&mut s, 1, "tools/list", json!({}));
    assert!(r["result"]["tools"].as_array().unwrap().iter().all(|t| !t["name"].as_str().unwrap().starts_with("cmd_")));
}

/// The `import` helper passes the copy options through: a folder template files the copy.
#[test]
fn import_tool_copies_with_a_folder_template() {
    let base = std::env::temp_dir().join(format!("lc-mcp-import-tpl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (src, dest) = (base.join("card"), base.join("out"));
    std::fs::create_dir_all(&src).unwrap();
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[20, 3, 9, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(src.join("a.png"), png).unwrap();
    let mut b = Headless::demo();
    b.session.clock = Box::new(|| "2026-01-14T05:58:48".to_string());
    let args =
        json!({"paths": [src.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "{date:%Y}/{date:%Y%m%d}"});
    let r = call_tool(&mut b, "import", &args);
    assert!(!r.is_error, "{r:?}");
    assert!(dest.join("2026").join("20260114").join("a.png").is_file());
    let _ = std::fs::remove_dir_all(&base);
}

/// Issue #93: `render_photo`, `ui.render` and `app.export` with an exact `path` must not replace
/// a photo's original.
#[test]
fn path_writes_never_replace_an_original() {
    let base = std::env::temp_dir().join(format!("lc-mcp-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 30, 9, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    let orig = base.join("a.png");
    std::fs::write(&orig, &png).unwrap();
    let mut b = Headless::demo();
    let r = call_tool(&mut b, "import", &json!({"paths": [orig.to_string_lossy()]}));
    assert!(!r.is_error, "{r:?}");
    let id = b.session.catalog.photos().find(|p| p.file_name == "a.png").unwrap().id.0;
    let path = orig.to_string_lossy().to_string();
    let r = call_tool(&mut b, "render_photo", &json!({"id": id, "path": path}));
    assert!(r.is_error, "{r:?}");
    assert!(b.call("ui.render", json!({"id": id, "path": path})).unwrap_err().contains("original"));
    let e = b.call("app.export", json!({"ids": [id], "path": path})).unwrap_err();
    assert!(e.contains("never writes over an original"), "{e}");
    assert_eq!(std::fs::read(&orig).unwrap(), png, "the original is untouched");
    // another path still works
    let r = call_tool(&mut b, "render_photo", &json!({"id": id, "path": base.join("render.png").to_string_lossy()}));
    assert!(!r.is_error, "{r:?}");
    let _ = std::fs::remove_dir_all(&base);
}

/// The `import` helper moves: renamed into the folder template, the source removed.
#[test]
fn import_tool_moves_with_a_folder_template() {
    let base = std::env::temp_dir().join(format!("lc-mcp-import-move-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (src, dest) = (base.join("card"), base.join("out"));
    std::fs::create_dir_all(&src).unwrap();
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[21, 3, 9, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(src.join("sample.png"), png).unwrap();
    let mut b = Headless::demo();
    b.session.clock = Box::new(|| "2026-01-14T05:58:48".to_string());
    let args = json!({"paths": [src.to_string_lossy()], "mode": "move", "destination": dest.to_string_lossy(),
        "organize": "{date:%Y}/{date:%Y%m%d}", "rename": "{date:%Y%m%d}_{seq:3}"});
    let r = call_tool(&mut b, "import", &args);
    assert!(!r.is_error, "{r:?}");
    assert!(dest.join("2026").join("20260114").join("20260114_001.png").is_file());
    assert!(!src.join("sample.png").exists(), "moved");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn command_tools_run_commands() {
    let mut b = Headless::demo();
    let first = b.session.visible_cloned()[0];
    let r = call_tool(&mut b, "cmd_library_select", &json!({"ids": [first.0]}));
    assert!(!r.is_error, "{r:?}");
    let r = call_tool(&mut b, "cmd_photo_rate", &json!({"rating": 4}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(b.session.catalog.photo(first).unwrap().rating, 4);
    let r = call_tool(&mut b, "cmd_edit_undo", &json!({}));
    assert!(!r.is_error);
    assert_eq!(b.session.catalog.photo(first).unwrap().rating, 0);
}

#[test]
fn set_develop_values_and_settings() {
    let mut b = Headless::demo();
    let first = b.session.visible_cloned()[0];
    let r = call_tool(&mut b, "set_develop", &json!({"id": first.0, "values": {"light.exposure": 1.25}}));
    assert!(!r.is_error, "{r:?}");
    assert_eq!(r.structured.as_ref().unwrap()["controls"][0]["value"], 1.25);
    assert_eq!(b.session.develop_of(first).unwrap().light.exposure, 1.25);
    let r = call_tool(&mut b, "set_develop", &json!({"settings": {"light": {"contrast": 30.0}}}));
    assert!(!r.is_error, "{r:?}");
    let d = b.session.develop_of(first).unwrap();
    assert_eq!((d.light.exposure, d.light.contrast), (1.25, 30.0));
    assert!(call_tool(&mut b, "set_develop", &json!({})).is_error);
    assert!(call_tool(&mut b, "set_develop", &json!({"values": {"no.such": 1}})).is_error);
}

#[test]
fn crop_needs_a_rect_angle_or_reset() {
    let mut b = Headless::demo();
    let first = b.session.visible_cloned()[0];
    // A guessed parameter must not succeed silently.
    let r = call_tool(&mut b, "crop", &json!({"id": first.0, "aspect": "1:1"}));
    assert!(r.is_error, "{r:?}");
    assert!(call_tool(&mut b, "crop", &json!({"id": first.0, "reset": false})).is_error);
    assert!(!call_tool(&mut b, "crop", &json!({"id": first.0, "rect": [0.1, 0.0, 0.9, 1.0]})).is_error);
    assert!(!call_tool(&mut b, "crop", &json!({"id": first.0, "reset": true})).is_error);
}

#[test]
fn ui_tools_need_the_app() {
    let mut b = Headless::demo();
    let r = call_tool(&mut b, "screenshot", &json!({}));
    assert!(r.is_error);
    assert!(r.content[0]["text"].as_str().unwrap().contains("--connect"));
}

#[test]
fn resources() {
    let mut s = server();
    let r = rpc(&mut s, 1, "resources/list", json!({}));
    let list = r["result"]["resources"].as_array().unwrap();
    assert_eq!(list.len(), 5);
    for (i, res) in list.iter().enumerate() {
        let uri = res["uri"].as_str().unwrap();
        let r = rpc(&mut s, 10 + i as u64, "resources/read", json!({"uri": uri}));
        let text = r["result"]["contents"][0]["text"].as_str().unwrap_or_else(|| panic!("{uri}: {r}"));
        serde_json::from_str::<Value>(text).unwrap();
    }
    assert_eq!(rpc(&mut s, 99, "resources/read", json!({"uri": "lightcraft://nope"}))["error"]["code"], -32002);
}
