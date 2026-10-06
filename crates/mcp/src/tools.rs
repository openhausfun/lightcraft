//! MCP tools: hand-written helpers (import, develop, render, screenshot, UI input…) plus one tool
//! per command in the engine's command registry (`cmd_<id with . → _>`), all forwarded to a
//! [`Backend`] as control-channel methods.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::backend::Backend;
use crate::base64::base64_encode;
use crate::headless::{encode_image, expand_paths};

/// Prefix of the tools generated from the command registry.
pub const COMMAND_TOOL_PREFIX: &str = "cmd_";

/// The result of `tools/call`: MCP content blocks plus the error flag.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
    pub structured: Option<Value>,
}

impl ToolResult {
    pub fn text(t: impl Into<String>) -> Self {
        Self { content: vec![json!({"type": "text", "text": t.into()})], ..Default::default() }
    }

    /// A JSON result as pretty text plus `structuredContent` when it is an object.
    pub fn json(v: Value) -> Self {
        let text = if v.is_null() { "ok".to_string() } else { serde_json::to_string_pretty(&v).unwrap_or_default() };
        let mut r = Self::text(text);
        if v.is_object() {
            r.structured = Some(v);
        }
        r
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { is_error: true, ..Self::text(msg) }
    }

    fn from(r: Result<Value, String>) -> Self {
        match r {
            Ok(v) => Self::json(v),
            Err(e) => Self::error(e),
        }
    }

    pub fn to_value(&self) -> Value {
        let mut v = json!({"content": self.content, "isError": self.is_error});
        if let Some(s) = &self.structured {
            v["structuredContent"] = s.clone();
        }
        v
    }

    /// The first image block's base64 data and mime type.
    pub fn image(&self) -> Option<(&str, &str)> {
        self.content.iter().find(|c| c["type"] == "image").and_then(|c| Some((c["data"].as_str()?, c["mimeType"].as_str()?)))
    }
}

fn tool(name: &str, title: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required},
    })
}

fn ids_schema(what: &str) -> Value {
    json!({"type": "array", "items": {"type": "integer"}, "description": what})
}

/// The hand-written helper tools. UI tools are included only when a desktop app is attached.
pub fn helper_tools(has_ui: bool) -> Vec<Value> {
    let photo_id = json!({"type": "integer", "description": "Photo id (default: the active photo). Makes that photo active first."});
    let mut v = vec![
        tool(
            "list_commands",
            "List commands",
            "Every command (engine + UI when connected) with id, label, menu, shortcut, parameter doc and whether it is enabled now. Each id is also callable as tool `cmd_<id with . replaced by _>` or via run_command.",
            json!({"filter": {"type": "string", "description": "Only commands whose id or label contains this text (case-insensitive)"}}),
            &[],
        ),
        tool(
            "run_command",
            "Run command",
            "Run any LightCraft command by id with JSON params (see list_commands for ids and parameter docs), e.g. {command: \"photo.rate\", params: {rating: 4}}.",
            json!({"command": {"type": "string"}, "params": {"type": "object", "description": "Command parameters", "additionalProperties": true}}),
            &["command"],
        ),
        tool(
            "import",
            "Import photos",
            "Add image/raw files to the library. Folders are scanned recursively for photos; relative paths resolve against the server's working directory. The first imported photo becomes active. Returns the new photo ids.",
            json!({
                "paths": {"type": "array", "items": {"type": "string"}, "description": "Files and/or folders"},
                "album": {"type": "integer", "description": "Also add to this album id"},
                "mode": {"type": "string", "enum": ["add", "copy", "move"], "description": "add (default) = reference the files in place; copy = copy them into `destination` (default: the library's Originals/); move = as copy, then each original and its XMP sidecars are removed from the source once the copy is verified and catalogued (duplicates and failures keep their sources; `kept` lists sources left in place and why; undo leaves the moved files at the destination)"},
                "destination": {"type": "string", "description": "Copy or move: destination folder"},
                "organize": {"type": "string", "description": "Copy or move: folders inside the destination — date (YYYY/YYYY-MM-DD, default), month (YYYY/YYYY-MM), flat, or a folder template such as {date:%Y}/{date:%Y%m%d} (→ 2026/20260114; the template's / make the levels, tokens as for rename; relative, no ..). Dated by capture time, else the import time"},
                "rename": {"type": "string", "description": "Copy or move: file-name template, e.g. {date:%Y%m%d_%H%M%S}_{seq:3} (run_command photo.renameTokens lists the tags); the extension is kept"}
            }),
            &["paths"],
        ),
        tool(
            "query_photos",
            "Query photos",
            "List photos (id, file name, size, rating, flag, edited…). Without filter/sort: the current library view.",
            json!({
                "filter": {"type": "object", "description": "Catalog Filter JSON (e.g. {\"minRating\": 3})", "additionalProperties": true},
                "sort": {"type": "object", "additionalProperties": true},
                "offset": {"type": "integer"},
                "limit": {"type": "integer", "description": "Default 200"}
            }),
            &[],
        ),
        tool(
            "select_photos",
            "Select photos",
            "Set the selection; the active photo (the one develop tools edit) is `active` or the first id.",
            json!({"ids": ids_schema("Photo ids"), "active": {"type": "integer"}, "mode": {"type": "string", "enum": ["replace", "add", "toggle", "range"]}}),
            &["ids"],
        ),
        tool(
            "list_controls",
            "List develop controls",
            "Every develop slider (id like `light.exposure`, label, section, min, max, default, current value) for the active photo.",
            json!({"section": {"type": "string", "description": "Only this section (e.g. light, color, effects, detail)"}}),
            &[],
        ),
        tool(
            "get_develop",
            "Get develop settings",
            "The full develop settings JSON of a photo (white balance, light, color, curves, mixer, grading, effects, crop, masks, spots…).",
            json!({"id": photo_id}),
            &[],
        ),
        tool(
            "set_develop",
            "Set develop settings",
            "Edit a photo (one undo step each for values and settings). `values` sets sliders by control id (see list_controls), e.g. {\"light.exposure\": 0.7, \"light.contrast\": 20}. `settings` deep-merges a partial develop-settings JSON (as returned by get_develop).",
            json!({
                "id": photo_id,
                "values": {"type": "object", "additionalProperties": {"type": "number"}, "description": "controlId → value"},
                "settings": {"type": "object", "additionalProperties": true, "description": "Partial DevelopSettings JSON to merge"},
                "label": {"type": "string", "description": "History label for `settings`"}
            }),
            &[],
        ),
        tool(
            "apply_preset",
            "Apply preset",
            "Apply a develop preset (ids from cmd_presets_list) to the selection or `ids`, at `amount` percent (0..200, default 100).",
            json!({"preset": {"type": "string", "description": "Preset id"}, "amount": {"type": "number"}, "ids": ids_schema("Photo ids (default: selection)")}),
            &["preset"],
        ),
        tool(
            "crop",
            "Crop / straighten",
            "Set the crop of the active photo: `rect` [x0,y0,x1,y1] in normalized coordinates (0..1, origin top-left) of the straightened frame, `angle` in degrees. `reset: true` clears it. Give at least one of them.",
            json!({"id": photo_id, "rect": {"type": "array", "items": {"type": "number"}, "minItems": 4, "maxItems": 4}, "angle": {"type": "number"}, "reset": {"type": "boolean"}}),
            &[],
        ),
        tool(
            "render_photo",
            "Render photo",
            "Render a photo with its current develop settings (cropped) and return it as an image so you can look at the result.",
            json!({
                "id": photo_id,
                "size": {"type": "integer", "description": "Long edge in pixels (default 1024, max 4096)"},
                "format": {"type": "string", "enum": ["png", "jpeg"], "description": "Image encoding (default png; jpeg is smaller)"},
                "path": {"type": "string", "description": "Also save the rendered file here"}
            }),
            &[],
        ),
        tool(
            "export",
            "Export photo",
            "Render and write photos. Give `path` (one photo; format from the extension unless `format` is set) or `dir` (batch, file names from `naming`).",
            json!({
                "id": photo_id,
                "ids": {"type": "array", "items": {"type": "integer"}, "description": "Photos to export (default: the given/active photo)"},
                "path": {"type": "string"},
                "dir": {"type": "string"},
                "format": {"type": "string", "enum": ["jpeg", "png", "tiff", "webp", "avif", "original", "dng"], "description": "original = the file as is + an XMP sidecar with the edits; dng = raw photos as DNG with the edits embedded"},
                "longEdge": {"type": "integer", "description": "Output long edge in px. When no size param is given: 3000; 0 = full size (cropped, native resolution)"},
                "shortEdge": {"type": "integer", "description": "Output short edge in px"},
                "width": {"type": "integer", "description": "Output width in px (with height: fit inside width × height, either orientation)"},
                "height": {"type": "integer", "description": "Output height in px"},
                "megapixels": {"type": "number", "description": "Output size in megapixels"},
                "percent": {"type": "number", "description": "Output size as a percentage of full size"},
                "dontEnlarge": {"type": "boolean", "description": "Never upscale past the photo's own size (default true)"},
                "ppi": {"type": "integer", "description": "Print resolution written into the file (default 240)"},
                "quality": {"type": "integer", "description": "JPEG/AVIF quality 1..100"},
                "limitKb": {"type": "integer", "description": "JPEG: largest quality that fits this many KB"},
                "sharpen": {"type": "string", "enum": ["none", "screen", "matte", "glossy"]},
                "sharpenAmount": {"type": "string", "enum": ["low", "standard", "high"]},
                "naming": {"type": "string", "description": "File-name template, e.g. {name}-{seq} or {date:%Y-%m-%d}_{title}_{seq:2} (tokens: {name} {num} {seq} {seq:N} {date} {date:%…} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext})"},
                "startNumber": {"type": "integer", "description": "First {seq} value (default 1)"},
                "metadata": {"type": "string", "enum": ["all", "allExceptCamera", "copyright", "none"]},
                "removeLocation": {"type": "boolean"},
                "colorSpace": {"type": "string", "enum": ["srgb", "displayP3", "adobeRgb", "proPhoto", "rec2020"], "description": "Output colour space (default sRGB; AVIF is always sRGB). adobeRgb = Adobe RGB (1998) compatible; the embedded ICC profile is generated from the published primaries"},
                "bitDepth": {"type": "integer", "enum": [8, 10, 16, 32], "description": "Bits per channel: PNG 8|16 (default 8), TIFF 8|16|32 (default 16; 32 = linear float with a linear profile), AVIF 8|10; JPEG/WebP are 8-bit"},
                "watermark": {"description": "Text, or {text, vertical (boolean; defaults to false), size (fraction of short edge), opacity, anchor (topLeft|top|topRight|left|center|right|bottomLeft|bottom|bottomRight), inset, color [r,g,b], shadow}"}
            }),
            &[],
        ),
    ];
    if has_ui {
        v.extend([
            tool(
                "screenshot",
                "Screenshot",
                "Capture the LightCraft window (after pending renders finish) and return it as an image.",
                json!({"maxSize": {"type": "integer", "description": "Downscale so the long edge is at most this (default 1600)"}, "format": {"type": "string", "enum": ["png", "jpeg"]}, "path": {"type": "string", "description": "Also save the screenshot here"}}),
                &[],
            ),
            tool("inspect_ui", "Inspect UI", "UI state: view, open panel, window and image rects, active photo, selection, active mask, status.", json!({}), &[]),
            tool(
                "set_ui",
                "Set UI state",
                "Merge UI state, e.g. {\"view\": \"detail\"} (photoGrid|squareGrid|detail|compare) (see inspect_ui for fields).",
                json!({"state": {"type": "object", "additionalProperties": true}}),
                &["state"],
            ),
            tool(
                "list_widgets",
                "List widgets",
                "On-screen widgets by automation id with rects [x, y, w, h] in screen points.",
                json!({"filter": {"type": "string"}}),
                &[],
            ),
            tool(
                "click",
                "Click",
                "Click a widget by automation id (`widget`, see list_widgets) or a screen point (`x`, `y`). `count: 2` double-clicks.",
                json!({"widget": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"}, "count": {"type": "integer"}, "button": {"type": "string", "enum": ["left", "right"]}}),
                &[],
            ),
            tool(
                "press_key",
                "Press key",
                "Press a key with optional modifiers (shortcuts), e.g. {key: \"Z\", cmd: true}. Names: A–Z, 0–9, Enter, Escape, Delete, Left/Right/Up/Down, Space, Tab, F1… .",
                json!({"key": {"type": "string"}, "cmd": {"type": "boolean"}, "shift": {"type": "boolean"}, "alt": {"type": "boolean"}, "ctrl": {"type": "boolean"}}),
                &["key"],
            ),
            tool("type_text", "Type text", "Type text into the focused field.", json!({"text": {"type": "string"}}), &["text"]),
            tool(
                "pointer_gesture",
                "Pointer gesture",
                "Replay a pointer gesture on the photo in normalized image coordinates (0..1, origin top-left): brush strokes, gradient drags, crop handles, heal spots. Needs the Detail view.",
                json!({
                    "events": {"type": "array", "items": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["down", "drag", "move", "up"]}, "x": {"type": "number"}, "y": {"type": "number"}}, "required": ["kind", "x", "y"]}},
                    "alt": {"type": "boolean"}, "shift": {"type": "boolean"}, "cmd": {"type": "boolean"}
                }),
                &["events"],
            ),
        ]);
    }
    v
}

/// MCP tool name of a command id (`develop.set` → `cmd_develop_set`).
pub fn command_tool_name(id: &str) -> String {
    format!("{COMMAND_TOOL_PREFIX}{}", id.replace(['.', ' '], "_"))
}

/// One tool per command reported by the backend's `engine.commands`.
pub fn command_tools(backend: &mut dyn Backend) -> Vec<Value> {
    let Ok(Value::Array(cmds)) = backend.call("engine.commands", json!({})) else { return Vec::new() };
    cmds.iter()
        .filter_map(|c| {
            let id = c["id"].as_str()?;
            let label = c["label"].as_str().unwrap_or(id);
            let mut d = format!("{label} (command `{id}`).");
            if let Some(p) = c["params"].as_str().filter(|p| !p.is_empty()) {
                d.push_str(&format!(" Params: {p}."));
            }
            if let Some(m) = c["menu"].as_array().filter(|m| m.iter().any(|x| x.as_str().is_some_and(|s| !s.is_empty()))) {
                let m: Vec<&str> = m.iter().filter_map(Value::as_str).collect();
                d.push_str(&format!(" Menu: {}.", m.join(" › ")));
            }
            if let Some(s) = c["shortcut"].as_str() {
                d.push_str(&format!(" Shortcut: {s}."));
            }
            Some(json!({
                "name": command_tool_name(id),
                "title": label,
                "description": d,
                "inputSchema": {"type": "object", "additionalProperties": true},
            }))
        })
        .collect()
}

/// All tools: helpers, plus the generated command tools when `with_commands`.
pub fn tool_definitions(backend: &mut dyn Backend, with_commands: bool) -> Vec<Value> {
    let mut v = helper_tools(backend.has_ui());
    if with_commands {
        v.extend(command_tools(backend));
    }
    v
}

fn exec(b: &mut dyn Backend, command: &str, params: Value) -> Result<Value, String> {
    b.call("engine.execute", json!({"command": command, "params": params}))
}

/// Make `args.id` (if any) the active photo.
fn activate(b: &mut dyn Backend, args: &Value) -> Result<(), String> {
    if let Some(id) = args.get("id").and_then(Value::as_u64) {
        exec(b, "library.select", json!({"ids": [id], "active": id}))?;
    }
    Ok(())
}

fn temp_path(tag: &str) -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("lightcraft-mcp-{}-{}-{tag}.png", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// Read a PNG the backend wrote, optionally downscale / re-encode as JPEG, and wrap it as image
/// content (+ a text block with the metadata).
fn image_result(file: &std::path::Path, max: Option<u32>, format: &str, save_to: Option<&str>, meta: Value) -> ToolResult {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) => return ToolResult::error(format!("cannot read rendered image {}: {e}", file.display())),
    };
    let _ = std::fs::remove_file(file);
    let jpeg = matches!(format, "jpeg" | "jpg");
    let (w, h) = (meta["width"].as_u64().unwrap_or(0) as u32, meta["height"].as_u64().unwrap_or(0) as u32);
    let too_big = max.is_some_and(|m| w.max(h) > m);
    let (bytes, w, h) = if jpeg || too_big {
        let opts = match max {
            Some(m) => lightcraft_codecs::DecodeOptions::fit(m, m),
            None => lightcraft_codecs::DecodeOptions::default(),
        };
        let d = match lightcraft_codecs::decode(&bytes, opts) {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("cannot decode rendered image: {e}")),
        };
        let img = d.to_srgb8();
        match encode_image(if jpeg { "jpg" } else { "png" }, &img, 88) {
            Ok(b) => (b, img.width as u32, img.height as u32),
            Err(e) => return ToolResult::error(e),
        }
    } else {
        (bytes, w, h)
    };
    if let Some(p) = save_to
        && let Err(e) = lightcraft_engine::export::write_file(p, &bytes)
    {
        return ToolResult::error(e);
    }
    let mime = if jpeg { "image/jpeg" } else { "image/png" };
    let mut info = meta;
    info["width"] = json!(w);
    info["height"] = json!(h);
    if let Some(p) = save_to {
        info["path"] = json!(p);
    } else if let Some(o) = info.as_object_mut() {
        o.remove("path");
    }
    ToolResult {
        content: vec![json!({"type": "image", "data": base64_encode(&bytes), "mimeType": mime}), json!({"type": "text", "text": info.to_string()})],
        is_error: false,
        structured: None,
    }
}

/// A user-given `path` to save to must not be a photo's original (or its sidecar).
fn check_save_path(b: &mut dyn Backend, args: &Value) -> Result<(), String> {
    match args.get("path").and_then(Value::as_str) {
        Some(p) => exec(b, "export.checkTarget", json!({"path": p})).map(|_| ()),
        None => Ok(()),
    }
}

fn render_photo(b: &mut dyn Backend, args: &Value) -> ToolResult {
    if let Err(e) = check_save_path(b, args) {
        return ToolResult::error(e);
    }
    let size = args.get("size").and_then(Value::as_u64).unwrap_or(1024).clamp(16, 4096);
    let id = match args.get("id").and_then(Value::as_u64) {
        Some(id) => Some(id),
        None => match exec(b, "library.state", json!({})) {
            Ok(s) => s["selection"]["active"].as_u64().or(s["selection"]["ids"][0].as_u64()),
            Err(e) => return ToolResult::error(e),
        },
    };
    let Some(id) = id else { return ToolResult::error("no active photo: import or select one, or pass `id`") };
    let file = temp_path("render");
    let path = file.to_string_lossy().to_string();
    let meta = match b.call("ui.render", json!({"id": id, "size": size, "path": path})) {
        Ok(m) => m,
        Err(e) => return ToolResult::error(e),
    };
    let mut meta = meta;
    meta["id"] = json!(id);
    image_result(&file, None, args.get("format").and_then(Value::as_str).unwrap_or("png"), args.get("path").and_then(Value::as_str), meta)
}

fn screenshot(b: &mut dyn Backend, args: &Value) -> ToolResult {
    if let Err(e) = check_save_path(b, args) {
        return ToolResult::error(e);
    }
    let file = temp_path("screenshot");
    let path = file.to_string_lossy().to_string();
    match b.call("ui.screenshot", json!({"path": path})) {
        Ok(meta) => image_result(
            &file,
            Some(args.get("maxSize").and_then(Value::as_u64).unwrap_or(1600).clamp(64, 8192) as u32),
            args.get("format").and_then(Value::as_str).unwrap_or("png"),
            args.get("path").and_then(Value::as_str),
            meta,
        ),
        Err(e) => ToolResult::error(e),
    }
}

fn obj(args: &Value, keys: &[&str]) -> Value {
    let mut m = Map::new();
    for k in keys {
        if let Some(v) = args.get(*k).filter(|v| !v.is_null()) {
            m.insert((*k).to_string(), v.clone());
        }
    }
    Value::Object(m)
}

/// Run one tool.
pub fn call_tool(b: &mut dyn Backend, name: &str, args: &Value) -> ToolResult {
    let args = if args.is_null() { &json!({}) } else { args };
    if !args.is_object() {
        return ToolResult::error("tool arguments must be an object");
    }
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    if let Some(rest) = name.strip_prefix(COMMAND_TOOL_PREFIX) {
        // Command ids never contain `_`, so the mapping is reversible.
        return ToolResult::from(exec(b, &rest.replace('_', "."), args.clone()));
    }
    match name {
        "list_commands" => {
            let filter = s("filter").unwrap_or("").to_lowercase();
            ToolResult::from(b.call("engine.commands", json!({})).map(|v| {
                match v {
                    Value::Array(a) if !filter.is_empty() => Value::Array(
                        a.into_iter()
                            .filter(|c| {
                                c["id"].as_str().unwrap_or("").to_lowercase().contains(&filter)
                                    || c["label"].as_str().unwrap_or("").to_lowercase().contains(&filter)
                            })
                            .collect(),
                    ),
                    v => v,
                }
            }))
        }
        "run_command" => match s("command") {
            Some(c) => ToolResult::from(exec(b, c, args.get("params").cloned().filter(|p| !p.is_null()).unwrap_or(json!({})))),
            None => ToolResult::error("missing `command`"),
        },
        "import" => {
            let paths: Vec<String> = match args.get("paths") {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                Some(Value::String(p)) => vec![p.clone()],
                _ => return ToolResult::error("missing `paths`"),
            };
            let files = expand_paths(&paths);
            if files.is_empty() {
                return ToolResult::error(format!("no photos found in {paths:?}"));
            }
            let mut p = json!({"paths": files});
            for k in ["album", "mode", "destination", "organize", "rename"] {
                if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
                    p[k] = v.clone();
                }
            }
            ToolResult::from(exec(b, "library.import", p).map(|mut r| {
                if let Some(o) = r.as_object_mut() {
                    o.insert("files".into(), json!(files.len()));
                }
                r
            }))
        }
        "query_photos" => ToolResult::from(exec(b, "catalog.query", obj(args, &["filter", "sort", "offset", "limit"]))),
        "select_photos" => {
            let mut p = obj(args, &["ids", "active", "mode"]);
            if p.get("active").is_none()
                && let Some(first) = args["ids"].get(0)
            {
                p["active"] = first.clone();
            }
            ToolResult::from(exec(b, "library.select", p))
        }
        "list_controls" => ToolResult::from(exec(b, "develop.controls", obj(args, &["section"]))),
        "get_develop" => ToolResult::from(exec(b, "develop.get", obj(args, &["id"]))),
        "set_develop" => {
            let r = (|| {
                activate(b, args)?;
                let values = args.get("values").filter(|v| v.as_object().is_some_and(|o| !o.is_empty()));
                let settings = args.get("settings").filter(|v| v.as_object().is_some_and(|o| !o.is_empty()));
                if values.is_none() && settings.is_none() {
                    return Err("give `values` ({controlId: number}) and/or `settings` (partial develop JSON)".to_string());
                }
                if let Some(v) = values {
                    exec(b, "develop.set", json!({"values": v}))?;
                }
                if let Some(st) = settings {
                    let mut p = json!({"settings": st});
                    if let Some(l) = args.get("label") {
                        p["label"] = l.clone();
                    }
                    exec(b, "develop.merge", p)?;
                }
                // Echo the resulting values of the touched sliders.
                let controls = exec(b, "develop.controls", json!({}))?;
                let touched: Vec<Value> = match (values, controls) {
                    (Some(Value::Object(v)), Value::Array(all)) => all
                        .into_iter()
                        .filter(|c| c["id"].as_str().is_some_and(|id| v.contains_key(id)))
                        .map(|c| json!({"id": c["id"], "value": c["value"]}))
                        .collect(),
                    _ => Vec::new(),
                };
                Ok(json!({"ok": true, "controls": touched}))
            })();
            ToolResult::from(r)
        }
        "apply_preset" => match s("preset").or(s("id")) {
            Some(p) => {
                let mut params = obj(args, &["amount", "ids"]);
                params["id"] = json!(p);
                ToolResult::from(exec(b, "preset.apply", params))
            }
            None => ToolResult::error("missing `preset`"),
        },
        "crop" if args.get("rect").is_none() && args.get("angle").is_none() && args.get("reset").and_then(Value::as_bool) != Some(true) => {
            ToolResult::error("give `rect`, `angle` or `reset: true`")
        }
        "crop" => ToolResult::from(activate(b, args).and_then(|_| {
            if args.get("reset").and_then(Value::as_bool) == Some(true) {
                exec(b, "crop.reset", json!({}))
            } else {
                exec(b, "crop.set", obj(args, &["rect", "angle"]))
            }
        })),
        "render_photo" => render_photo(b, args),
        "export" => ToolResult::from(activate(b, args).and_then(|_| {
            exec(
                b,
                "app.export",
                obj(
                    args,
                    &[
                        "ids",
                        "path",
                        "dir",
                        "format",
                        "longEdge",
                        "shortEdge",
                        "width",
                        "height",
                        "megapixels",
                        "percent",
                        "dontEnlarge",
                        "ppi",
                        "quality",
                        "limitKb",
                        "sharpen",
                        "sharpenAmount",
                        "naming",
                        "startNumber",
                        "metadata",
                        "removeLocation",
                        "watermark",
                        "colorSpace",
                        "bitDepth",
                    ],
                ),
            )
        })),
        "screenshot" => screenshot(b, args),
        "inspect_ui" => ToolResult::from(b.call("ui.inspect", json!({}))),
        "set_ui" => ToolResult::from(b.call("ui.set", args.get("state").cloned().unwrap_or(json!({})))),
        "list_widgets" => ToolResult::from(b.call("ui.widgets", obj(args, &["filter"]))),
        "click" => {
            let mut p = obj(args, &["x", "y", "count", "button"]);
            match s("widget") {
                Some(w) => {
                    p["id"] = json!(w);
                    ToolResult::from(b.call("ui.clickWidget", p))
                }
                None if args.get("x").is_some() && args.get("y").is_some() => ToolResult::from(b.call("ui.click", p)),
                None => ToolResult::error("give `widget` or `x` and `y`"),
            }
        }
        "press_key" => ToolResult::from(b.call("ui.key", obj(args, &["key", "cmd", "shift", "alt", "ctrl"]))),
        "type_text" => ToolResult::from(b.call("ui.text", obj(args, &["text"]))),
        "pointer_gesture" => ToolResult::from(b.call("ui.pointer", obj(args, &["events", "alt", "shift", "cmd"]))),
        other => ToolResult::error(format!("unknown tool `{other}` (see tools/list)")),
    }
}
