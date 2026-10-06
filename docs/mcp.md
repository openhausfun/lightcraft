# LightCraft MCP server

`lightcraft-cli mcp` exposes LightCraft to AI agents through the
[Model Context Protocol](https://modelcontextprotocol.io): newline-delimited JSON-RPC 2.0 over
stdio (protocol revision `2025-06-18`; `2025-03-26` and `2024-11-05` clients are accepted). The
server lives in `crates/mcp` (`lightcraft-mcp`, layer L5) and is hand-written: no async runtime,
no C dependencies.

It runs in one of two modes:

| Mode | Command | What it drives |
|---|---|---|
| **Headless** (default) | `lightcraft-cli mcp [--demo] [FILES/FOLDERS…]` | An in-process engine `Session`. Develop, render and export without a window. |
| **Connect** | `lightcraft-cli mcp --connect [127.0.0.1:7980]` | A running desktop app started with `lightcraft --control 7980`, through its loopback JSON-lines control channel ([control-protocol.md](control-protocol.md)). Adds the UI tools (screenshot, clicks, keys, pointer gestures). |

Options: `--library DIR` opens (or creates) a persistent LightCraft library — the same crash-safe
format the desktop app uses (`~/Pictures/LightCraft Library` by default there) — so ratings, edits and albums
survive between sessions (with `--demo`, a new library is seeded with the demo photos). A library
is open in one program at a time (`catalog.lock` in the library folder): while the desktop app has
it open, `--library` on the same folder fails with "This library is already open in LightCraft
(process N …)" — use connect mode to work with the running app instead;
`--demo` starts the headless session with the procedurally generated demo library;
`--compact` lists only the helper tools (see below). In connect mode the server starts even when
the app is not running yet and connects on the first call (and reconnects if the app restarts).

Logs go to stderr; stdout carries only protocol messages.

## Wiring it into a client

Build once: `cargo build --release -p lightcraft-cli` (binary: `target/release/lightcraft-cli`).

### Claude Code

```sh
# headless, with a folder of photos imported at start
claude mcp add lightcraft -- /path/to/lightcraft/target/release/lightcraft-cli mcp ~/Pictures/shoot

# or: drive the running desktop app (start it with `lightcraft --control 7980`)
claude mcp add lightcraft-app -- /path/to/lightcraft/target/release/lightcraft-cli mcp --connect 127.0.0.1:7980
```

Or check a project-scoped `.mcp.json` into your repo:

```json
{
  "mcpServers": {
    "lightcraft": {
      "command": "/path/to/lightcraft/target/release/lightcraft-cli",
      "args": ["mcp", "--connect", "127.0.0.1:7980"]
    }
  }
}
```

### Other clients (Claude Desktop, Cursor, …)

Every stdio MCP client takes the same shape: a `command` plus `args`. For example
`claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "lightcraft": {
      "command": "/path/to/lightcraft/target/release/lightcraft-cli",
      "args": ["mcp", "--demo"]
    }
  }
}
```

During development you can also point the client at `cargo run --release -p lightcraft-cli -- mcp`
(with `"cwd"` set to the repository), at the cost of a slower start.

## Tools

### Helpers

| Tool | Does |
|---|---|
| `list_commands {filter?}` | Every command: id, label, menu, shortcut, parameter doc, enabled now |
| `run_command {command, params?}` | Run any command by id |
| `import {paths, album?, mode?, destination?, organize?, rename?}` | Import files/folders (folders are scanned recursively); the first new photo becomes active. `mode: "copy"` copies into `destination` (default: the library's Originals/), `mode: "move"` moves there (each original and its XMP sidecars are removed from the source only after the copy is verified and catalogued; duplicates and failures keep their sources; the result's `moved` / `kept` list what moved and what stayed, and why; undo leaves the moved files at the destination), filed by `organize`: `date` (YYYY/YYYY-MM-DD), `month`, `flat` or a folder template such as `{date:%Y}/{date:%Y%m%d}` (→ `2026/20260114`; capture date, else the import date; always inside the destination), and named by the `rename` template (`run_command photo.renameTokens` lists the tags) |
| `query_photos {filter?, sort?, offset?, limit?}` | Photos in the current view (or matching a catalog `Filter`) |
| `select_photos {ids, active?, mode?}` | Set the selection / active photo |
| `list_controls {section?}` | Every develop slider: id (`light.exposure`…), range, default, current value |
| `get_develop {id?}` | Full develop-settings JSON |
| `set_develop {id?, values?, settings?, label?}` | `values`: `{controlId: number}`; `settings`: partial develop JSON deep-merged. Undoable |
| `apply_preset {preset, amount?, ids?}` | Apply a preset (ids from `cmd_presets_list`) |
| `crop {id?, rect?, angle?, reset?}` | Normalized crop rect `[x0,y0,x1,y1]` and straighten angle; at least one of `rect`, `angle`, `reset: true` |
| `render_photo {id?, size?, format?, path?}` | Render with current settings → **image content** (PNG, or JPEG with `format: "jpeg"`), long edge `size` (default 1024) |
| `export {path \| dir, id? \| ids?, format?, longEdge? \| shortEdge? \| width?/height? \| megapixels? \| percent?, dontEnlarge?, ppi?, quality?, colorSpace?, bitDepth?, …}` | Full-quality render to `.png` / `.jpg` / `.tif` / `.webp` / `.avif`; `format: "original"` copies the file + an XMP sidecar with the edits, `format: "dng"` writes raw photos as DNG with the edits embedded. No size param = 3000 px long edge; `longEdge: 0` = full size (cropped, native resolution); `width` + `height` fit either orientation; `dontEnlarge` defaults to true |

Tools taking `id` make that photo active first; without it they act on the active photo.

### Connect mode only

| Tool | Does |
|---|---|
| `screenshot {maxSize?, format?, path?}` | The app window as an image, after pending renders finish |
| `inspect_ui` | View, panel, window/image rects, selection, status |
| `set_ui {state}` | Merge UI state, e.g. `{"view": "detail"}` |
| `list_widgets {filter?}` / `click {widget \| x,y, count?}` | Widgets by automation id; real egui clicks |
| `press_key {key, cmd?, shift?, alt?}` / `type_text {text}` | Keyboard input (shortcuts) |
| `pointer_gesture {events}` | Gestures in normalized image coordinates (brush strokes, gradients, crop handles) |

In headless mode these return a tool error explaining how to start the app.

### One tool per command

Everything is a command in LightCraft, so `tools/list` also contains one tool per entry of the
command registry (engine commands, plus the app's UI commands such as `view.detail` when
connected): the id with `.` replaced by `_` and a `cmd_` prefix — `photo.rate` → `cmd_photo_rate`,
`develop.set` → `cmd_develop_set`, `edit.undo` → `cmd_edit_undo`. Arguments are the command's
JSON params (documented in each tool's description and by `list_commands`). Pass `--compact` to
leave these out (about 90 headless / 140 connected) when a client struggles with many tools;
`run_command` still reaches every command.

## Resources

`resources/list` / `resources/read` serve JSON snapshots:

| URI | Content |
|---|---|
| `lightcraft://library` | Source, filter, sort, selection, undo/redo labels (`library.state`) |
| `lightcraft://photos` | Photos in the current view (`catalog.query`) |
| `lightcraft://photo/active` | Everything about the active photo (`photo.inspect`) |
| `lightcraft://develop/active` | The active photo's develop settings (`develop.get`) |
| `lightcraft://controls` | Every develop control with its current value (`develop.controls`) |

## Example session

```text
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"me","version":"1"}}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{…},"resources":{…}},"serverInfo":{"name":"lightcraft",…},"instructions":"…"}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
→ {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"set_develop","arguments":{"values":{"light.exposure":0.7}}}}
← {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{…}"}],"isError":false,"structuredContent":{"ok":true,"controls":[{"id":"light.exposure","value":0.7}]}}}
→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render_photo","arguments":{"size":768}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"image","data":"iVBORw0…","mimeType":"image/png"},{"type":"text","text":"{\"id\":21,\"width\":512,\"height\":768}"}],"isError":false}}
```

Errors from commands (unknown control, nothing selected, app not reachable) come back as tool
results with `isError: true` so the model can read and correct them; malformed JSON-RPC gets the
standard error codes (-32700 parse, -32600 invalid request, -32601 method not found, -32602
invalid params, -32002 resource not found).

With `--library`, a command whose change can't be written to disk (disk full, …) is an error too:
`saved in memory but not written to disk: <reason>; LightCraft will retry` — the change is applied in the session and
written by the next successful save (see [control-protocol.md](control-protocol.md#when-the-library-cant-be-saved)).

## One-shot commands: `lightcraft-cli run`

For agents that prefer a shell over an MCP session: run any chain of commands in one process and read one JSON
line per command (`{"command", "ok", "result" | "error", "ms"}`; non-zero exit status on failure). A word without
`=` starts the next command; `key=value` values are JSON when they parse, else strings; a `'{…}'` argument merges a
JSON object into the params. Quote values with brackets or spaces for your shell (`'ids=[3,4]'`; zsh treats `[…]` as a glob).

```sh
# headless: import, edit, export
lightcraft-cli run --import ~/Pictures/a.dng develop.set control=light.exposure value=0.7 \
    develop.auto app.export path=/tmp/a.jpg shortEdge=1080 colorSpace=displayP3
# a persistent library: edits are saved, later invocations see them
lightcraft-cli run --library ~/lc-lib --import ~/Pictures/shoot library.info
lightcraft-cli run --library ~/lc-lib library.select ids=[3] develop.get
# the running app (same commands, plus ui.* methods)
lightcraft-cli run --connect ui.set view=detail ui.screenshot path=/tmp/ui.png
# JSON lines from a file or stdin: {"command": id, "params": {…}} or {"method": "ui.inspect"}
lightcraft-cli run --demo --script steps.jsonl --keep-going
```

## Other CLI subcommands

```sh
lightcraft-cli render in.dng -o out.tif --opt colorSpace=displayP3 --opt bitDepth=16 --opt percent=50
lightcraft-cli render in.dng -o out.jpg --set light.exposure=0.5 --set light.contrast=20 --size 2048
lightcraft-cli render in.jpg -o out.png --settings look.json --preset <presetId>
lightcraft-cli commands [--json]   # the command registry
lightcraft-cli controls [--json]   # develop control ids and ranges
```

## Tests

- `crates/mcp/tests/e2e.rs` — M0.9 acceptance: over the stdio framing, set exposure and render;
  checks the decoded PNG gets brighter/darker. Runs headless and through the TCP transport
  (`Remote`) against a stand-in control server; also import → render → JPEG export of a real file.
- `apps/lightcraft-cli/tests/cli.rs` — spawns `lightcraft-cli mcp` with real pipes; `render`;
  `commands`.
