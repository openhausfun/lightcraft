//! Issue #100: a library that can't be opened at launch is never replaced by a silent in-memory
//! demo session. The window says why and offers Try Again / Choose Another Library… / Continue
//! Without Saving / Quit; a temporary session shows a banner and never writes to the library.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::panels::library_problem::LibraryProblem;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

fn has(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(w, _)| w == id)
}

#[test]
fn unopenable_library_asks_instead_of_running_a_demo() {
    let dir = std::env::temp_dir().join(format!("lc-ui-libproblem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib = dir.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let img = lightcraft_raster::Rgba8 { width: 8, height: 8, data: vec![[90, 3, 9, 255]; 64] };
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    let photo = dir.join("a.png");
    std::fs::write(&photo, png).unwrap();

    // another program has the library open: the launch fails like the desktop host's would
    let held = lightcraft_catalog::LibraryLock::acquire(&lib, "another LightCraft").unwrap();
    let mut session = lightcraft_engine::Session::new().with_fs();
    let err = session.open_library(&lib, true).unwrap_err().to_string();
    let png = |img: &lightcraft_raster::Rgba8| {
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(img), &lightcraft_codecs::EncodeMeta::default()).unwrap_or_default()
    };
    let mut app = LightcraftApp::new(
        session,
        Services {
            png: Some(Box::new(png)),
            write: Some(Box::new(|p: &str, b: &[u8]| std::fs::write(p, b).map_err(|e| e.to_string()))),
            ..Default::default()
        },
    );
    app.library_problem =
        Some(LibraryProblem { pending_import: vec![photo.to_string_lossy().to_string()], ..LibraryProblem::new(lib.to_string_lossy(), err) });
    let mut h = Headless::new(app, [1300.0, 900.0], 1.0);
    h.step();
    h.step();
    assert!(h.app.session.catalog.is_empty(), "no demo photos");
    for b in ["button:libraryRetry", "button:libraryChoose", "button:libraryTemporary", "button:libraryQuit"] {
        assert!(has(&h, b), "{b}");
    }
    if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&p).join("dialog.png").to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    let inspect = h.request("ui.inspect", json!({}), T);
    let p = &inspect["result"]["libraryProblem"];
    assert!(p["error"].as_str().unwrap().contains("already open in another LightCraft"), "{p}");
    assert_eq!(p["temporarySession"], false);

    // still locked: Try Again keeps the window, with the error
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:libraryRetry"}), T)["ok"], true);
    h.step();
    assert!(h.app.session.library.is_none());
    assert!(has(&h, "button:libraryRetry"));

    // Continue Without Saving: a banner, the command-line photo in the temporary session, and
    // nothing written into the library folder
    let before: Vec<_> = std::fs::read_dir(&lib).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:libraryTemporary"}), T)["ok"], true);
    h.step();
    h.step();
    assert!(!has(&h, "button:libraryRetry"));
    assert!(has(&h, "indicator:temporarySession"));
    assert!(h.app.session.library.is_none());
    assert_eq!(h.app.session.catalog.len(), 1, "the command-line photo is imported into the temporary session");
    if let Some(p) = std::env::var_os("LIGHTCRAFT_TEST_SHOTS") {
        let r = h.request("ui.screenshot", json!({"path": std::path::Path::new(&p).join("banner.png").to_string_lossy(), "headless": true}), T);
        assert_eq!(r["ok"], true, "{r}");
    }
    h.request("engine.execute", json!({"command": "photo.rate", "params": {"rating": 3}}), T);
    let after: Vec<_> = std::fs::read_dir(&lib).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(before, after, "the temporary session never writes to the library");

    // the other program quits; Open Library… → Try Again opens it, the banner goes away
    drop(held);
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:libraryReopen"}), T)["ok"], true);
    h.step();
    assert!(has(&h, "button:libraryRetry"));
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:libraryRetry"}), T)["ok"], true);
    h.step();
    h.step();
    assert!(h.app.session.library.is_some());
    assert!(h.app.library_problem.is_none());
    assert!(!has(&h, "indicator:temporarySession") && !has(&h, "button:libraryRetry"));
    assert!(h.app.session.catalog.is_empty(), "an opened (new) library is never seeded with demo photos here");
    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}
