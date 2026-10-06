//! Export end to end: output sizes, original + XMP, DNG.

use serde_json::json;

use crate::Session;
use crate::export::{ExportFormat, ExportOptions, export_photo};
use crate::tests_xmp::{synthetic_dng_with, temp_dir};

#[test]
fn full_size_export_of_a_cropped_photo_is_not_upscaled() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let (pw, ph) = {
        let p = s.catalog.photo(id).unwrap();
        (p.width as usize, p.height as usize)
    };
    s.execute("crop.set", &json!({"rect": [0.25, 0.25, 0.75, 0.75]})).unwrap();
    let e = export_photo(&mut s, id, &ExportOptions::default(), 1).unwrap();
    assert!(e.width.abs_diff(pw / 2) <= 1 && e.height.abs_diff(ph / 2) <= 1, "{}×{} from {pw}×{ph}", e.width, e.height);
    // and the size modes apply to the cropped size
    let o = ExportOptions::from_json(&json!({"percent": 50}));
    let e = export_photo(&mut s, id, &o, 1).unwrap();
    assert!(e.width.abs_diff(pw / 4) <= 1, "{} vs {}", e.width, pw / 4);
    let o = ExportOptions::from_json(&json!({"width": 200, "height": 200, "format": "png"}));
    let e = export_photo(&mut s, id, &o, 1).unwrap();
    assert_eq!(e.width.max(e.height), 200);
    // the print resolution lands in the file
    let i = e.bytes.windows(4).position(|w| w == b"pHYs").expect("pHYs");
    assert_eq!(u32::from_be_bytes(e.bytes[i + 4..i + 8].try_into().unwrap()), 9449, "240 ppi");
}

#[test]
fn original_export_copies_the_file_and_writes_its_edits_beside_it() {
    let dir = temp_dir("export-orig");
    let src = dir.join("Shot.dng");
    let dng = synthetic_dng_with(None, Default::default());
    std::fs::write(&src, &dng).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.7})).unwrap();

    let o = ExportOptions::from_json(&json!({"format": "original", "naming": "{name}-{seq}"}));
    let e = export_photo(&mut s, id, &o, 3).unwrap();
    assert_eq!(e.file_name, "Shot-003.dng");
    assert_eq!(e.bytes, dng, "the original, byte for byte");
    assert_eq!(e.sidecars.len(), 1);
    let (ext, xmp) = &e.sidecars[0];
    assert_eq!(*ext, "xmp");
    let name = "Shot-003.xmp";
    let copy = dir.join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join(&e.file_name), &e.bytes).unwrap();
    std::fs::write(copy.join(name), xmp).unwrap();
    let mut s2 = Session::new().with_fs();
    s2.execute("library.import", &json!({"paths": [copy.join(&e.file_name).to_string_lossy()]})).unwrap();
    assert_eq!(s2.catalog.photos().next().unwrap().develop.light.exposure, 0.7, "edits travel in the sidecar");

    // DNG: re-encoded raw data with the edits embedded
    let o = ExportOptions { format: ExportFormat::Dng, ..Default::default() };
    let e = export_photo(&mut s, id, &o, 1).unwrap();
    assert_eq!(e.file_name, "Shot.dng");
    assert!(e.sidecars.is_empty());
    let back = lightcraft_raw::decode(&e.bytes).expect("our DNG decodes");
    let orig = lightcraft_raw::decode(&dng).unwrap();
    assert_eq!((back.width, back.height), (orig.width, orig.height));
    assert_eq!(back.data, orig.data, "lossless");
    let out = dir.join("out.dng");
    std::fs::write(&out, &e.bytes).unwrap();
    let mut s2 = Session::new().with_fs();
    s2.execute("library.import", &json!({"paths": [out.to_string_lossy()]})).unwrap();
    assert_eq!(s2.catalog.photos().next().unwrap().develop.light.exposure, 0.7, "edits embedded in the DNG are read back");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn original_and_dng_need_a_file() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    for f in ["original", "dng"] {
        let err = export_photo(&mut s, id, &ExportOptions::from_json(&json!({"format": f})), 1).err().unwrap();
        assert!(err.contains("no original file"), "{err}");
    }
}

#[test]
fn dng_export_rejects_non_raw_photos() {
    let dir = temp_dir("export-dng-jpeg");
    let src = dir.join("a.png");
    let img = lightcraft_raster::Rgba8::from_fn(32, 24, |x, y| [(x * 8) as u8, (y * 10) as u8, 90, 255]);
    std::fs::write(&src, crate::export::encode_image(&img, &ExportOptions { format: ExportFormat::Png, ..Default::default() }).unwrap()).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    let err = export_photo(&mut s, id, &ExportOptions { format: ExportFormat::Dng, ..Default::default() }, 1).err().unwrap();
    assert!(err.contains("needs a raw photo"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn presets_conflicts_and_subfolders() {
    use crate::export::{Destination, export_batch};
    let mut s = Session::with_demo();
    let ids: Vec<_> = s.visible().iter().copied().take(2).collect();
    // built-ins and user presets; the call's own params override the preset's
    let names: Vec<String> =
        s.execute("export.presets", &json!({})).unwrap().as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    assert!(names.contains(&"JPEG (Small)".to_string()) && names.contains(&"Original + Settings".to_string()), "{names:?}");
    let p = s.export_params(&json!({"preset": "jpeg (small)", "quality": 50})).unwrap();
    assert_eq!((p["longEdge"].as_u64(), p["quality"].as_u64(), p.get("preset")), (Some(2048), Some(50), None));
    assert!(s.export_params(&json!({"preset": "nope"})).unwrap_err().contains("unknown export preset"));
    assert!(s.execute("export.savePreset", &json!({"name": "JPEG (Large)", "params": {}})).is_err(), "built-in names are reserved");
    s.execute("export.savePreset", &json!({"name": "Web", "params": {"format": "png", "width": 64, "ids": [1], "dir": "/x"}})).unwrap();
    let web = s.export_params(&json!({"preset": "Web"})).unwrap();
    assert_eq!(web, json!({"format": "png", "width": 64}), "targets are not part of a preset");
    s.execute("export.savePreset", &json!({"name": "web", "params": {"format": "webp"}})).unwrap();
    assert_eq!(s.export_presets.len(), 1, "same name (any case) replaces");
    s.execute("export.deletePreset", &json!({"name": "WEB"})).unwrap();
    assert!(s.export_presets.is_empty());

    // a fake file system: what exists, what was written
    let existing = ["out/Sub/LC01347.png".to_string()];
    let to = Destination { dir: "out".into(), exact: None };
    let batch = |s: &mut Session, ids: &[_], o: &ExportOptions| {
        let mut written: Vec<String> = Vec::new();
        let mut write = |p: &str, _: &[u8]| {
            written.push(p.to_string());
            Ok(())
        };
        let files = export_batch(s, ids, o, &to, &mut write, &|p| existing.contains(&p.to_string())).unwrap();
        (files, written)
    };
    let o = ExportOptions::from_json(&json!({"format": "png", "width": 32, "subfolder": "Sub", "naming": "LC01347"}));
    // both photos get the same name from the template: the first is renamed past the existing
    // file, the second past the first
    let (files, written) = batch(&mut s, &ids, &o);
    assert_eq!(written, ["out/Sub/LC01347-2.png", "out/Sub/LC01347-3.png"]);
    assert_eq!(files[0]["width"], 32);
    let (files, written) = batch(&mut s, &ids, &ExportOptions { conflict: crate::export::Conflict::Skip, ..o.clone() });
    assert_eq!(files.iter().filter(|f| f["skipped"] == "out/Sub/LC01347.png").count(), 2, "{files:?}");
    assert!(written.is_empty());
    let (_, written) = batch(&mut s, &ids[..1], &ExportOptions { conflict: crate::export::Conflict::Overwrite, ..o });
    assert_eq!(written, ["out/Sub/LC01347.png"]);
}

#[test]
fn prepared_exports_run_on_another_thread() {
    fn send<T: Send>(_: &T) {}
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    let o = ExportOptions::from_json(&json!({"format": "png", "width": 48}));
    let items: Vec<_> = (1..=2).map(|seq| crate::export::prepare_export(&mut s, id, &o, seq).unwrap()).collect();
    send(&items);
    let to = crate::export::Destination::default();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let files = std::thread::spawn(move || {
        let mut written = Vec::new();
        let mut write = |p: &str, b: &[u8]| {
            written.push((p.to_string(), b.len()));
            Ok(())
        };
        // cancel after the first photo
        crate::export::run_batch(items, &o, &to, &mut write, &|_| false, false, &mut |done, name| {
            seen2.lock().unwrap().push((done, name.to_string()));
            done < 1
        })
        .unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(files.len(), 1, "cancelled before the second: {files:?}");
    assert_eq!(files[0]["width"], 48);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

/// Issue #78: a GPU render whose work never ran (a driver that drops a submission without an
/// error) gave an all-black export. The export must come from the CPU instead — the same
/// image — with the reason recorded.
#[test]
fn export_falls_back_to_the_cpu_when_gpu_work_is_lost() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.4})).unwrap();
    s.execute("develop.set", &json!({"control": "effects.clarity", "value": 20})).unwrap();
    let o = ExportOptions::from_json(&json!({"format": "png"}));
    let mean = |b: &[u8]| {
        let d = lightcraft_codecs::decode(b, Default::default()).unwrap().image;
        d.data.iter().map(|p| (p[0] + p[1] + p[2]) as f64).sum::<f64>() / (3 * d.data.len()) as f64
    };
    let gpu = lightcraft_gpu::available();
    let healthy = mean(&export_photo(&mut s, id, &o, 1).unwrap().bytes);
    lightcraft_gpu::inject_fault(lightcraft_gpu::Fault::DropWork);
    let faulted = export_photo(&mut s, id, &o, 1).unwrap().bytes;
    if gpu {
        let why = lightcraft_gpu::last_fallback().unwrap_or_default();
        assert!(why.contains("incomplete"), "{why}");
        // the GPU is off for the process now: this export renders on the CPU
        assert!(!lightcraft_gpu::available());
    }
    let cpu = export_photo(&mut s, id, &o, 1).unwrap().bytes;
    lightcraft_gpu::reset_failures();
    let px = |b: &[u8]| lightcraft_codecs::decode(b, Default::default()).unwrap().image.data;
    assert!(px(&faulted) == px(&cpu), "the fallback is the CPU render");
    let m = mean(&faulted);
    assert!(m > 0.02, "not black: mean {m}");
    assert!((m - healthy).abs() < 0.01, "GPU {healthy} vs CPU {m}");
}

/// A JPEG original `dir/IMG_1.jpg` imported into a library on disk.
fn library_with_jpeg(tag: &str) -> (Session, lightcraft_catalog::PhotoId, std::path::PathBuf, std::path::PathBuf, Vec<u8>) {
    let dir = temp_dir(tag);
    let src = dir.join("IMG_1.jpg");
    let img = lightcraft_raster::Rgba8::from_fn(48, 32, |x, y| [(x * 5) as u8, (y * 7) as u8, 120, 255]);
    let bytes = crate::export::encode_image(&img, &ExportOptions::default()).unwrap();
    std::fs::write(&src, &bytes).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    s.execute("develop.set", &json!({"ids": [id.0], "control": "light.exposure", "value": 1.0})).unwrap();
    (s, id, dir, src, bytes)
}

fn disk_batch(
    s: &mut Session,
    id: lightcraft_catalog::PhotoId,
    o: &ExportOptions,
    to: &crate::export::Destination,
) -> Result<Vec<serde_json::Value>, String> {
    crate::export::export_batch(s, &[id], o, to, &mut crate::export::write_file, &|p| std::path::Path::new(p).exists())
}

/// Issue #93: exporting into the photo's own folder as `{name}` with "Overwrite" replaced the
/// original with the re-encoded render. It is refused now, whatever the policy or exact path.
#[test]
fn export_never_overwrites_an_original() {
    use crate::export::{Conflict, Destination};
    let (mut s, id, dir, src, original) = library_with_jpeg("export-guard");
    let folder = Destination { dir: dir.to_string_lossy().to_string(), exact: None };
    let o = ExportOptions { naming: "{name}".into(), conflict: Conflict::Overwrite, ..Default::default() };
    let err = disk_batch(&mut s, id, &o, &folder).unwrap_err();
    assert!(err.contains("original of IMG_1.jpg"), "{err}");
    assert_eq!(std::fs::read(&src).unwrap(), original, "the original is byte-identical");

    // the exact path of the control channel / MCP, also spelled another way
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    for exact in [src.to_string_lossy().to_string(), dir.join("sub/../IMG_1.jpg").to_string_lossy().to_string()] {
        let to = Destination { dir: String::new(), exact: Some(exact) };
        assert!(disk_batch(&mut s, id, &o, &to).unwrap_err().contains("never writes over an original"));
    }
    // the "Original" format re-writing the file onto itself (and its sidecar)
    let orig = ExportOptions { format: ExportFormat::Original, ..o.clone() };
    assert!(disk_batch(&mut s, id, &orig, &folder).is_err());
    assert_eq!(std::fs::read(&src).unwrap(), original);
    assert!(s.execute("export.checkTarget", &json!({"path": src.to_string_lossy()})).is_err());
    assert!(s.execute("export.checkTarget", &json!({"path": dir.join("free.jpg").to_string_lossy()})).is_ok());

    // Unique moves past it; Overwrite still replaces an ordinary earlier export
    let unique = ExportOptions { conflict: Conflict::Unique, ..o.clone() };
    let files = disk_batch(&mut s, id, &unique, &folder).unwrap();
    assert!(files[0]["path"].as_str().unwrap().ends_with("IMG_1-2.jpg"), "{files:?}");
    let earlier = dir.join("IMG_1-2.jpg");
    std::fs::write(&earlier, b"an earlier export").unwrap();
    let named = ExportOptions { naming: "{name}-2".into(), ..o };
    disk_batch(&mut s, id, &named, &folder).unwrap();
    assert_ne!(std::fs::read(&earlier).unwrap(), b"an earlier export", "an ordinary file is overwritten");
    assert_eq!(std::fs::read(&src).unwrap(), original);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #93: the XMP sidecar of an "Original" export ignored the conflict policy.
#[test]
fn export_sidecars_follow_the_conflict_policy() {
    use crate::export::{Conflict, Destination};
    let (mut s, id, dir, _src, original) = library_with_jpeg("export-sidecar");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let theirs = out.join("IMG_1.xmp");
    std::fs::write(&theirs, b"someone else's sidecar").unwrap();
    let to = Destination { dir: out.to_string_lossy().to_string(), exact: None };
    let o = ExportOptions { format: ExportFormat::Original, naming: "{name}".into(), ..Default::default() };

    // Unique: the photo and its sidecar both move to the next free name
    let files = disk_batch(&mut s, id, &ExportOptions { conflict: Conflict::Unique, ..o.clone() }, &to).unwrap();
    assert!(files[0]["path"].as_str().unwrap().ends_with("IMG_1-2.jpg"), "{files:?}");
    assert!(files[0]["sidecars"][0].as_str().unwrap().ends_with("IMG_1-2.xmp"), "{files:?}");
    assert_eq!(std::fs::read(&theirs).unwrap(), b"someone else's sidecar");
    assert_eq!(std::fs::read(out.join("IMG_1-2.jpg")).unwrap(), original);
    // Skip: a taken sidecar name skips the photo
    std::fs::remove_file(out.join("IMG_1-2.jpg")).unwrap();
    std::fs::remove_file(out.join("IMG_1-2.xmp")).unwrap();
    let files = disk_batch(&mut s, id, &ExportOptions { conflict: Conflict::Skip, ..o.clone() }, &to).unwrap();
    assert!(files[0]["skipped"].as_str().is_some(), "{files:?}");
    assert!(!out.join("IMG_1.jpg").exists());
    assert_eq!(std::fs::read(&theirs).unwrap(), b"someone else's sidecar");
    // Overwrite: replaced, as asked
    disk_batch(&mut s, id, &ExportOptions { conflict: Conflict::Overwrite, ..o }, &to).unwrap();
    assert!(std::fs::read_to_string(&theirs).unwrap().contains("xmpmeta"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #93: the native writer truncated the target first; a failure mid-write (full disk,
/// unplugged drive) now leaves the previous file intact and no partial one.
#[test]
fn a_failed_export_write_keeps_the_previous_file() {
    use crate::export::{Conflict, Destination};
    let (mut s, id, dir, _src, _) = library_with_jpeg("export-midwrite");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let prev = out.join("IMG_1.jpg");
    std::fs::write(&prev, b"last week's export").unwrap();
    let to = Destination { dir: out.to_string_lossy().to_string(), exact: None };
    let o = ExportOptions { conflict: Conflict::Overwrite, ..Default::default() };
    {
        let _fault = lightcraft_catalog::safe_file::fail_writes_after(100);
        let err = disk_batch(&mut s, id, &o, &to).unwrap_err();
        assert!(err.contains("injected"), "{err}");
    }
    assert_eq!(std::fs::read(&prev).unwrap(), b"last week's export");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 1, "no temp file left behind");
    disk_batch(&mut s, id, &o, &to).unwrap();
    assert_ne!(std::fs::read(&prev).unwrap(), b"last week's export");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #134: exports can always be made again, so they are written atomically but not synced to
/// disk file by file; an edit copy that becomes a library photo still is.
#[test]
fn exports_are_atomic_without_a_sync() {
    use crate::export::{Conflict, Destination};
    use lightcraft_catalog::safe_file::syncs_on_this_thread;
    let (mut s, id, dir, _src, _) = library_with_jpeg("export-nosync");
    let out = dir.join("out");
    let to = Destination { dir: out.to_string_lossy().to_string(), exact: None };
    let o = ExportOptions { conflict: Conflict::Unique, ..Default::default() };
    let before = syncs_on_this_thread();
    for _ in 0..3 {
        disk_batch(&mut s, id, &o, &to).unwrap();
    }
    assert_eq!(syncs_on_this_thread(), before, "no per-file sync for exports");
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 3, "three exports, no temp files");
    crate::export::write_file_durable(&out.join("edit.tif").to_string_lossy(), b"tiff").unwrap();
    assert!(syncs_on_this_thread() > before, "the durable writer syncs");
    let _ = std::fs::remove_dir_all(&dir);
}
