//! End-to-end tests of the `lightcraft-cli` binary: `mcp` over real stdio pipes, `render`, `commands`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_lightcraft-cli");

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lightcraft-cli-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn gradient_png(path: &std::path::Path) {
    let img = lightcraft_raster::Rgba8::from_fn(120, 80, |x, y| [(x * 2) as u8, (y * 3) as u8, 100, 255]);
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &Default::default()).unwrap();
    std::fs::write(path, png).unwrap();
}

fn mean(path: &std::path::Path) -> f64 {
    let d = lightcraft_codecs::decode(&std::fs::read(path).unwrap(), Default::default()).unwrap();
    let img = d.to_srgb8();
    img.data.iter().map(|p| (p[0] as u32 + p[1] as u32 + p[2] as u32) as f64).sum::<f64>() / (3 * img.data.len()) as f64
}

#[test]
fn mcp_over_stdio_sets_exposure_and_renders() {
    let input = tmp("in.png");
    gradient_png(&input);
    let mut child =
        Command::new(BIN).args(["mcp", input.to_str().unwrap()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut rpc = |id: u64, method: &str, params: Value| -> Value {
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], id);
        v
    };
    let init = rpc(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "cli-test", "version": "0"}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let tools = rpc(2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() > 50);
    let base = tmp("base.png");
    let bright = tmp("bright.png");
    let r = rpc(3, "tools/call", json!({"name": "render_photo", "arguments": {"path": base.to_str().unwrap()}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(4, "tools/call", json!({"name": "set_develop", "arguments": {"values": {"light.exposure": 1.0}}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    let r = rpc(5, "tools/call", json!({"name": "render_photo", "arguments": {"path": bright.to_str().unwrap()}}));
    assert_eq!(r["result"]["isError"], false, "{r}");
    assert_eq!(r["result"]["content"][0]["type"], "image");
    drop(stdin);
    assert!(child.wait().unwrap().success());
    let (a, b) = (mean(&base), mean(&bright));
    println!("cli mcp: mean sRGB exposure 0 → {a:.1}, +1 → {b:.1}");
    assert!(b > a + 10.0, "{a} → {b}");
}

#[test]
fn render_subcommand() {
    let input = tmp("r-in.png");
    gradient_png(&input);
    let plain = tmp("r-plain.png");
    let out = tmp("r-out.jpg");
    let st = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", plain.to_str().unwrap()]).status().unwrap();
    assert!(st.success());
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1", "--size", "60"])
        .status()
        .unwrap();
    assert!(st.success());
    let d = lightcraft_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (60, 40));
    let d = lightcraft_codecs::decode(&std::fs::read(&plain).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (120, 80));
    assert!(mean(&out) > mean(&plain) + 10.0);
    // Bad control id fails cleanly.
    let o = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "nope=1"]).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown control"));
}

/// Issue #93: `render IMG -o IMG` replaced its own input with the render.
#[test]
fn render_refuses_to_overwrite_its_input() {
    let input = tmp("self-in.png");
    gradient_png(&input);
    let before = std::fs::read(&input).unwrap();
    let dir = input.parent().unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let other_spelling = dir.join("sub/../self-in.png");
    for out in [&input, &other_spelling] {
        let o =
            Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1"]).output().unwrap();
        assert!(!o.status.success());
        assert!(String::from_utf8_lossy(&o.stderr).contains("never writes over an original"), "{}", String::from_utf8_lossy(&o.stderr));
    }
    assert_eq!(std::fs::read(&input).unwrap(), before, "the input is untouched");
}

#[test]
fn render_export_options() {
    let input = tmp("o-in.png");
    gradient_png(&input);
    let out = tmp("o-out.tif");
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .args(["--opt", "shortEdge=40", "--opt", "colorSpace=displayP3", "--opt", "bitDepth=16"])
        .status()
        .unwrap();
    assert!(st.success());
    let d = lightcraft_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (60, 40));
    assert_eq!(d.space.named, Some(lightcraft_codecs::NamedSpace::DisplayP3));
    // original + sidecar named after the output
    let out = tmp("o-copy.png");
    let st = Command::new(BIN)
        .args(["render", input.to_str().unwrap(), "-o", out.to_str().unwrap(), "--set", "light.exposure=1", "--opt", "format=original"])
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(&input).unwrap());
    let xmp = std::fs::read_to_string(out.with_extension("xmp")).unwrap();
    assert!(xmp.contains("exposure"), "{xmp}");
    // unknown extension fails with a hint
    let o = Command::new(BIN).args(["render", input.to_str().unwrap(), "-o", tmp("x.bmp").to_str().unwrap()]).output().unwrap();
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("unknown extension"));
}

#[test]
fn commands_subcommand_lists_registry() {
    let o = Command::new(BIN).args(["commands", "--json"]).output().unwrap();
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    let ids: Vec<&str> = v.as_array().unwrap().iter().filter_map(|c| c["id"].as_str()).collect();
    for id in ["develop.set", "photo.rate", "library.import", "edit.undo", "app.export"] {
        assert!(ids.contains(&id), "{id}");
    }
    let o = Command::new(BIN).arg("controls").output().unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("light.exposure"));
}

#[test]
fn snapshot_subcommand_renders_the_ui_headlessly() {
    let script = tmp("snap.jsonl");
    let a = tmp("snap-grid.png");
    let b = tmp("snap-export.png");
    std::fs::write(
        &script,
        format!(
            "# comment\n{}\n{}\n{}\n{}\n",
            json!({"method": "ui.set", "params": {"view": "photoGrid"}}),
            json!({"method": "ui.screenshot"}),
            json!({"method": "engine.execute", "params": {"command": "dialog.export"}}),
            json!({"method": "ui.screenshot", "params": {"path": b.to_str().unwrap()}}),
        ),
    )
    .unwrap();
    let o = Command::new(BIN)
        .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", a.to_str().unwrap(), "--size", "640x400"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4);
    assert!(replies.iter().all(|r| r["ok"] == true), "{replies:?}");
    for (p, dimmed) in [(&a, false), (&b, true)] {
        let d = lightcraft_codecs::decode(&std::fs::read(p).unwrap(), Default::default()).unwrap();
        assert_eq!((d.width, d.height), (640, 400));
        // the export dialog dims everything around it
        assert_eq!(mean(p) < mean(&a) - 2.0, dimmed, "{}", p.display());
    }
    // no script: one settled screenshot, at 2× scale
    let o = Command::new(BIN).args(["snapshot", "-o", a.to_str().unwrap(), "--size", "320x240", "--scale", "2"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let d = lightcraft_codecs::decode(&std::fs::read(&a).unwrap(), Default::default()).unwrap();
    assert_eq!((d.width, d.height), (640, 480));
}

/// Issue #136: with the GPU switched off from the environment the UI starts and renders on the CPU,
/// without creating a GPU device (no driver is loaded), and says why.
#[test]
fn snapshot_starts_without_a_gpu() {
    let script = tmp("nogpu.jsonl");
    std::fs::write(
        &script,
        format!(
            "{}\n{}\n{}\n",
            json!({"method": "ui.set", "params": {"view": "photoGrid"}}),
            json!({"method": "engine.execute", "params": {"command": "app.gpu"}}),
            json!({"method": "ui.screenshot"}),
        ),
    )
    .unwrap();
    for (var, value, reason) in [("LIGHTCRAFT_GPU", "0", "LIGHTCRAFT_GPU=0"), ("LIGHTCRAFT_GPU_BACKEND", "off", "LIGHTCRAFT_GPU_BACKEND=off")] {
        let out = tmp(&format!("nogpu-{var}.png"));
        let o = Command::new(BIN)
            .env_remove("LIGHTCRAFT_GPU")
            .env_remove("LIGHTCRAFT_GPU_BACKEND")
            .env(var, value)
            .args(["snapshot", "--demo", "--script", script.to_str().unwrap(), "-o", out.to_str().unwrap(), "--size", "480x320"])
            .output()
            .unwrap();
        assert!(o.status.success(), "{var}: {}", String::from_utf8_lossy(&o.stderr));
        let replies: Vec<Value> = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(replies.iter().all(|r| r["ok"] == true), "{replies:?}");
        let gpu = &replies[1]["result"];
        assert_eq!(gpu["available"], false, "{var}: {gpu}");
        assert_eq!(gpu["adapter"], Value::Null, "{var}: no device was created: {gpu}");
        assert!(gpu["reason"].as_str().is_some_and(|r| r.contains(reason)), "{var}: {gpu}");
        let d = lightcraft_codecs::decode(&std::fs::read(&out).unwrap(), Default::default()).unwrap();
        assert_eq!((d.width, d.height), (480, 320));
    }
}

fn run_cli(args: &[&str], stdin: Option<&str>) -> (bool, Vec<Value>, String) {
    let mut child = Command::new(BIN).arg("run").args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    if let Some(text) = stdin {
        child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    }
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    let lines = String::from_utf8_lossy(&o.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    (o.status.success(), lines, String::from_utf8_lossy(&o.stderr).to_string())
}

#[test]
fn run_subcommand_chains_commands_and_persists_a_library() {
    let input = tmp("run-in.png");
    gradient_png(&input);
    let lib = tmp("run-lib");
    let _ = std::fs::remove_dir_all(&lib);
    let out = tmp("run-out.jpg");
    let (lib_s, in_s, out_s) = (lib.to_str().unwrap(), input.to_str().unwrap(), out.to_str().unwrap());
    // import into a library, edit, export, in one invocation
    let (ok, lines, err) = run_cli(
        &[
            "--library",
            lib_s,
            "--import",
            in_s,
            "develop.set",
            "control=light.exposure",
            "value=0.75",
            "app.export",
            &format!("path={out_s}"),
            "width=60",
        ],
        None,
    );
    assert!(ok, "{err}");
    assert_eq!(lines.len(), 2);
    assert!(lines.iter().all(|l| l["ok"] == true), "{lines:?}");
    assert_eq!(lines[1]["result"]["width"], 60);
    assert!(out.exists());
    // a second invocation sees the saved edit (same file: selected again, not re-imported)
    let (ok, lines, err) = run_cli(&["--library", lib_s, "--import", in_s, "develop.get"], None);
    assert!(ok, "{err}");
    assert_eq!(lines[0]["result"]["light"]["exposure"], 0.75);
    // JSON-lines script from stdin, a failing command stops the run with a non-zero status
    let script = "{\"command\": \"library.info\"}\n# comment\n{\"command\": \"no.such\"}\n{\"command\": \"library.info\"}\n";
    let (ok, lines, _) = run_cli(&["--demo", "--script", "-"], Some(script));
    assert!(!ok);
    assert_eq!(lines.len(), 2, "stopped after the failure: {lines:?}");
    assert_eq!(lines[1]["ok"], false);
    let (ok, lines, _) = run_cli(&["--demo", "--keep-going", "--script", "-"], Some(script));
    assert!(!ok);
    assert_eq!(lines.len(), 3);
    // control-protocol methods work headlessly too; bad usage is reported
    let (ok, lines, _) = run_cli(&["--demo", "engine.commands"], None);
    assert!(ok && lines[0]["result"].as_array().is_some_and(|a| a.len() > 50));
    let (ok, _, err) = run_cli(&["--demo", "x=1"], None);
    assert!(!ok && err.contains("before any command"), "{err}");
}

#[test]
fn devices_are_listed_and_imported_from() {
    let base = tmp("devices");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("CARD/DCIM/100TEST")).unwrap();
    gradient_png(&base.join("CARD/DCIM/100TEST/IMG_0001.png"));
    let o = Command::new(BIN).env("LIGHTCRAFT_DEVICE_ROOTS", &base).args(["run", "--demo", "library.devices"]).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let line: Value = serde_json::from_slice(o.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    let dev = &line["result"][0];
    assert_eq!(dev["name"], "CARD", "{line}");
    // the import review of its DCIM folder finds the photo
    let dcim = dev["path"].as_str().unwrap().to_string();
    let o = Command::new(BIN).args(["run", "library.importPreview", &format!("paths=[\"{dcim}\"]")]).output().unwrap();
    let line: Value = serde_json::from_slice(o.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(line["result"]["candidates"].as_array().map(Vec::len), Some(1), "{line}");
}

/// Issue #99: a library open in one process (here `mcp --library`) is refused by a second one,
/// with who has it and how to drive the running app instead; free again once the first exits.
#[test]
fn a_library_open_in_another_process_is_refused() {
    let lib = tmp("locked-lib");
    let _ = std::fs::remove_dir_all(&lib);
    let lib_s = lib.to_str().unwrap();
    let mut holder =
        Command::new(BIN).args(["mcp", "--library", lib_s]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
    let mut err = BufReader::new(holder.stderr.take().unwrap());
    let mut line = String::new();
    while !line.contains("opened library") {
        line.clear();
        assert!(err.read_line(&mut line).unwrap() > 0, "mcp exited before opening the library");
    }
    let (ok, _, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(!ok);
    assert!(stderr.contains("already open in lightcraft-cli") && stderr.contains(&format!("process {}", holder.id())), "{stderr}");
    assert!(stderr.contains("mcp --connect"), "{stderr}");

    drop(holder.stdin.take()); // EOF: the server exits and lets go of the library
    assert!(holder.wait().unwrap().success());
    let (ok, lines, stderr) = run_cli(&["--library", lib_s, "library.info"], None);
    assert!(ok, "{stderr}");
    assert_eq!(lines[0]["ok"], true);
    let _ = std::fs::remove_dir_all(&lib);
}
