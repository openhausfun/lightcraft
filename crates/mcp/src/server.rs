//! JSON-RPC 2.0 framing and the MCP lifecycle / tools / resources methods.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::backend::Backend;
use crate::tools::{call_tool, tool_definitions};

/// The MCP revision we implement.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions we can speak; anything else negotiates down to [`PROTOCOL_VERSION`].
const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const RESOURCE_NOT_FOUND: i64 = -32002;

const INSTRUCTIONS: &str = "LightCraft is a non-destructive photo library and raw developer (a Lightroom alternative). \
Photos have integer ids; most editing acts on the *active* photo (tools that take `id` make that photo active first). \
Typical loop: query_photos → select_photos (or pass id) → list_controls → set_develop {values: {\"light.exposure\": 0.5}} → \
render_photo to look at the result → export. Positions (crop rects, mask points, brush strokes) are normalized image \
coordinates 0..1 with the origin at the top-left. Every action is a command: list_commands shows ids and parameter \
docs; run_command runs any of them. Edits are undoable (undo / redo).";

/// Resource URIs.
pub const LIBRARY_URI: &str = "lightcraft://library";
pub const PHOTOS_URI: &str = "lightcraft://photos";
pub const PHOTO_URI: &str = "lightcraft://photo/active";
pub const DEVELOP_URI: &str = "lightcraft://develop/active";
pub const CONTROLS_URI: &str = "lightcraft://controls";

/// An MCP server bound to one backend.
pub struct Server {
    backend: Box<dyn Backend>,
    initialized: bool,
    command_tools: bool,
}

fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn resource(uri: &str, name: &str, title: &str, description: &str) -> Value {
    json!({"uri": uri, "name": name, "title": title, "description": description, "mimeType": "application/json"})
}

impl Server {
    pub fn new(backend: Box<dyn Backend>) -> Self {
        Self { backend, initialized: false, command_tools: true }
    }

    /// Whether `tools/list` includes one `cmd_*` tool per registered command (default true).
    /// Off, every command stays reachable through `run_command`.
    pub fn with_command_tools(mut self, on: bool) -> Self {
        self.command_tools = on;
        self
    }

    pub fn backend(&mut self) -> &mut dyn Backend {
        self.backend.as_mut()
    }

    /// Whether the client has sent `notifications/initialized`.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Serve newline-delimited JSON-RPC until `input` closes. Logs go to stderr only (stdout is
    /// the protocol stream).
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if let Some(reply) = self.handle_line(&line) {
                output.write_all(reply.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Handle one line; returns the reply line (None for notifications and blank lines).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(Value::Array(batch)) => {
                // Batches were removed in 2025-06-18; still answer older clients sensibly.
                if batch.is_empty() {
                    Some(error(Value::Null, INVALID_REQUEST, "empty batch"))
                } else {
                    let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle(m)).collect();
                    (!replies.is_empty()).then_some(Value::Array(replies))
                }
            }
            Ok(msg) => self.handle(msg),
            Err(e) => Some(error(Value::Null, PARSE_ERROR, format!("parse error: {e}"))),
        };
        reply.map(|r| r.to_string())
    }

    /// Handle one JSON-RPC message; `None` for notifications and responses.
    pub fn handle(&mut self, msg: Value) -> Option<Value> {
        let Value::Object(o) = &msg else { return Some(error(Value::Null, INVALID_REQUEST, "message must be an object")) };
        let id = o.get("id").cloned();
        let Some(method) = o.get("method").and_then(Value::as_str) else {
            // A response to a server→client request (we send none) — ignore; anything else is invalid.
            if o.contains_key("result") || o.contains_key("error") {
                return None;
            }
            return Some(error(id.filter(|i| i.is_string() || i.is_number()).unwrap_or(Value::Null), INVALID_REQUEST, "missing `method`"));
        };
        let params = o.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            self.notification(method, &params);
            return None;
        };
        if !(id.is_string() || id.is_number()) {
            return Some(error(Value::Null, INVALID_REQUEST, "`id` must be a string or number"));
        }
        if !(params.is_object() || params.is_null()) {
            return Some(error(id, INVALID_PARAMS, "`params` must be an object"));
        }
        Some(match self.request(method, &params) {
            Ok(r) => response(id, r),
            Err((code, m)) => error(id, code, m),
        })
    }

    fn notification(&mut self, method: &str, _params: &Value) {
        match method {
            "notifications/initialized" => self.initialized = true,
            "notifications/cancelled" | "notifications/progress" | "notifications/roots/list_changed" => {}
            other => log::debug!("ignoring notification {other}"),
        }
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSION };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "resources": {"subscribe": false, "listChanged": false}},
                    "serverInfo": {"name": "lightcraft", "title": "LightCraft", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": format!("{INSTRUCTIONS} Backend: {}.", self.backend.describe()),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions(self.backend.as_mut(), self.command_tools)})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing tool `name`".to_string()))?;
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);
                Ok(call_tool(self.backend.as_mut(), name, &args).to_value())
            }
            "resources/list" => Ok(json!({"resources": [
                resource(LIBRARY_URI, "library", "Library state", "Current source, filter, sort, selection, active photo/mask and undo/redo labels (library.state)"),
                resource(PHOTOS_URI, "photos", "Photos in view", "The photos in the current view with ids, ratings, flags and metadata (catalog.query)"),
                resource(PHOTO_URI, "active-photo", "Active photo", "Everything about the active photo: file, metadata, develop settings, albums, history (photo.inspect)"),
                resource(DEVELOP_URI, "develop", "Develop settings", "The active photo's full develop settings JSON (develop.get)"),
                resource(CONTROLS_URI, "controls", "Develop controls", "Every develop slider with id, range, default and current value (develop.controls)"),
            ]})),
            "resources/templates/list" => Ok(json!({"resourceTemplates": []})),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or((INVALID_PARAMS, "missing `uri`".to_string()))?;
                let cmd = match uri {
                    LIBRARY_URI => "library.state",
                    PHOTOS_URI => "catalog.query",
                    PHOTO_URI => "photo.inspect",
                    DEVELOP_URI => "develop.get",
                    CONTROLS_URI => "develop.controls",
                    _ => return Err((RESOURCE_NOT_FOUND, format!("resource not found: {uri}"))),
                };
                let v = self.backend.call("engine.execute", json!({"command": cmd, "params": {}})).map_err(|e| (INTERNAL_ERROR, e))?;
                Ok(json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}]}))
            }
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        }
    }
}
