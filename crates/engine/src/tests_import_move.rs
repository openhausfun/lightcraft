//! Import → Move: files (and their sidecars) end up at the destination, and a source is removed
//! only when its move fully succeeded. Temp dirs only.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::Session;
use crate::import_move::{Fault, inject};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-move-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 5) as u8, (i / w * 7) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn xmp(rating: u8) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="{rating}"/></rdf:RDF></x:xmpmeta>"#
    )
}

fn session() -> Session {
    let mut s = Session::new().with_fs();
    // undated files are filed by the import time
    s.clock = Box::new(|| "2026-01-14T05:58:48".to_string());
    s
}

fn import_move(s: &mut Session, paths: &[&Path], dest: &Path, extra: Value) -> Value {
    let mut p =
        json!({"paths": paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(), "mode": "move", "destination": dest.to_string_lossy()});
    if let (Some(o), Some(e)) = (p.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    s.execute("library.import", &p).unwrap()
}

fn len(v: &Value, key: &str) -> usize {
    v[key].as_array().map(Vec::len).unwrap_or(0)
}

fn photo_paths(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s
        .catalog
        .photos()
        .filter_map(|p| match &p.source {
            lightcraft_catalog::Source::File { path } => Some(path.clone()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn files_under(d: &Path) -> Vec<String> {
    fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut out = Vec::new();
    walk(d, d, &mut out);
    out.sort();
    out
}

/// Same volume: renamed into the folder template, sidecars (both namings) along, sources gone,
/// the catalog points at the new files; undo forgets the photos but leaves the files.
#[test]
fn move_into_a_folder_template_with_sidecars() {
    let base = temp_dir("same");
    let (card, dest) = (base.join("card"), base.join("Photos"));
    write_png(&card.join("a.png"), 1);
    std::fs::write(card.join("a.xmp"), xmp(3)).unwrap();
    write_png(&card.join("b.png"), 2);
    std::fs::write(card.join("b.png.xmp"), xmp(4)).unwrap();
    let a_bytes = std::fs::read(card.join("a.png")).unwrap();
    let mut s = session();
    let r = import_move(&mut s, &[&card], &dest, json!({"organize": "{date:%Y}/{date:%Y%m%d}", "rename": "{date:%Y%m%d}_{seq:3}"}));
    assert_eq!(len(&r, "imported"), 2, "{r}");
    assert_eq!(len(&r, "moved"), 2, "{r}");
    assert_eq!(len(&r, "kept"), 0, "{r}");
    assert_eq!(
        files_under(&dest),
        ["2026/20260114/20260114_001.png", "2026/20260114/20260114_001.xmp", "2026/20260114/20260114_002.png", "2026/20260114/20260114_002.png.xmp"]
    );
    assert_eq!(files_under(&card), Vec::<String>::new(), "the card is empty");
    assert_eq!(std::fs::read(dest.join("2026/20260114/20260114_001.png")).unwrap(), a_bytes);
    let day = dest.join("2026").join("20260114");
    assert_eq!(photo_paths(&s), [day.join("20260114_001.png").to_string_lossy(), day.join("20260114_002.png").to_string_lossy()]);
    let mut ratings: Vec<u8> = s.catalog.photos().map(|p| p.rating).collect();
    ratings.sort();
    assert_eq!(ratings, [3, 4], "the sidecars were read");
    assert_eq!(r["moved"][0]["sidecars"][0], day.join("20260114_001.xmp").to_string_lossy().as_ref());
    // undo: the photos leave the library, the moved files stay where they are now
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.len(), 0);
    assert_eq!(files_under(&dest).len(), 4);
    let _ = std::fs::remove_dir_all(&base);
}

/// Another volume (no hard links): copied, verified and only then the source removed.
#[test]
fn move_across_volumes_copies_then_removes() {
    let base = temp_dir("xvol");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("a.png"), 1);
    std::fs::write(card.join("a.xmp"), xmp(2)).unwrap();
    let bytes = std::fs::read(card.join("a.png")).unwrap();
    let mut s = session();
    inject(Fault::NoLink);
    let r = import_move(&mut s, &[&card], &dest, json!({"organize": "flat"}));
    inject(Fault::None);
    assert_eq!(len(&r, "moved"), 1, "{r}");
    assert_eq!(files_under(&dest), ["a.png", "a.xmp"]);
    assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), bytes);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(dest.join("a.png")).unwrap().nlink(), 1, "a real copy, not a link");
    }
    assert!(!card.join("a.png").exists() && !card.join("a.xmp").exists());
    let _ = std::fs::remove_dir_all(&base);
}

/// A copy that fails while writing, or comes out different, leaves nothing behind and keeps the
/// source; nothing is catalogued.
#[test]
fn a_failed_or_corrupt_copy_keeps_the_source() {
    for fault in [Fault::FailWrite, Fault::Corrupt] {
        let base = temp_dir(&format!("fail-{fault:?}"));
        let (card, dest) = (base.join("card"), base.join("out"));
        write_png(&card.join("a.png"), 1);
        std::fs::write(card.join("a.xmp"), xmp(2)).unwrap();
        let bytes = std::fs::read(card.join("a.png")).unwrap();
        let mut s = session();
        inject(fault);
        let r = import_move(&mut s, &[&card], &dest, json!({"organize": "flat"}));
        inject(Fault::None);
        assert_eq!(len(&r, "failed"), 1, "{fault:?}: {r}");
        assert_eq!(len(&r, "moved"), 0, "{fault:?}: {r}");
        assert_eq!(s.catalog.len(), 0);
        assert_eq!(std::fs::read(card.join("a.png")).unwrap(), bytes, "{fault:?}: source intact");
        assert!(card.join("a.xmp").is_file());
        assert_eq!(files_under(&dest), Vec::<String>::new(), "{fault:?}: no partial copy left");
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Issue #96: Import → Copy used a plain unverified copy (and check-then-copy naming that could
/// replace a file). It now gets the same verified copy as Move: a copy that fails or comes out
/// different is removed and reported as a failed import, nothing is catalogued, and the card
/// is untouched; a good copy never replaces an existing file.
#[test]
fn copy_import_verifies_each_copy() {
    let import_copy = |s: &mut Session, card: &Path, dest: &Path| {
        s.execute(
            "library.import",
            &json!({"paths": [card.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "flat"}),
        )
        .unwrap()
    };
    for fault in [Fault::FailWrite, Fault::Corrupt] {
        let base = temp_dir(&format!("copyfail-{fault:?}"));
        let (card, dest) = (base.join("card"), base.join("out"));
        write_png(&card.join("a.png"), 3);
        let bytes = std::fs::read(card.join("a.png")).unwrap();
        let mut s = session();
        inject(fault);
        let r = import_copy(&mut s, &card, &dest);
        inject(Fault::None);
        assert_eq!(len(&r, "failed"), 1, "{fault:?}: {r}");
        assert_eq!(len(&r, "imported"), 0, "{fault:?}: {r}");
        assert!(r.to_string().contains("the original is untouched"), "{r}");
        assert_eq!(s.catalog.len(), 0);
        assert_eq!(std::fs::read(card.join("a.png")).unwrap(), bytes, "{fault:?}: source intact");
        assert_eq!(files_under(&dest), Vec::<String>::new(), "{fault:?}: no bad copy left");
        let _ = std::fs::remove_dir_all(&base);
    }
    // a good copy: verified, and a taken name gets -1 instead of being replaced
    let base = temp_dir("copyok");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("a.png"), 4);
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("a.png"), b"someone else's file").unwrap();
    let mut s = session();
    let r = import_copy(&mut s, &card, &dest);
    assert_eq!(len(&r, "imported"), 1, "{r}");
    assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"someone else's file");
    assert_eq!(std::fs::read(dest.join("a-1.png")).unwrap(), std::fs::read(card.join("a.png")).unwrap());
    let _ = std::fs::remove_dir_all(&base);
}

/// Copy verifies against the probe's content hash (issue #134): the hash, not a re-read of the
/// source, is the reference — a copy that matches the source but not the hash is refused, a
/// corrupted copy is caught, and the file hash agrees with `hash_bytes` across read chunks.
#[test]
fn copy_is_verified_against_the_probe_hash() {
    use crate::import_move::copy_verified;
    let base = temp_dir("copyhash");
    let src = base.join("src.bin");
    // larger than one 1 MB read, not a multiple of it
    let bytes: Vec<u8> = (0..(3 << 20) + 12_345u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    std::fs::write(&src, &bytes).unwrap();
    let good = lightcraft_preview::hash_bytes(&bytes);
    copy_verified(&src, &base.join("ok.bin"), Some(good)).unwrap();
    assert_eq!(std::fs::read(base.join("ok.bin")).unwrap(), bytes);
    // a hash that doesn't match: refused (a byte compare with the source would have passed)
    let stale = lightcraft_preview::hash_bytes(b"what the probe read earlier");
    let e = copy_verified(&src, &base.join("stale.bin"), Some(stale)).unwrap_err();
    assert!(e.to_string().contains("differs"), "{e}");
    assert!(!base.join("stale.bin").exists(), "the refused copy is removed");
    // a copy that comes out different: caught by the hash
    inject(Fault::Corrupt);
    let r = copy_verified(&src, &base.join("bad.bin"), Some(good));
    inject(Fault::None);
    assert!(r.is_err());
    assert!(!base.join("bad.bin").exists());
    assert_eq!(std::fs::read(&src).unwrap(), bytes, "the source is untouched");
    let _ = std::fs::remove_dir_all(&base);
}

/// A file replaced after Review Import probed it is not imported on the strength of the old probe:
/// its copy no longer matches the probe's hash, so it's reported as failed and nothing is left.
#[test]
fn copy_import_refuses_a_file_changed_since_the_review() {
    let base = temp_dir("copychanged");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("a.png"), 3);
    let mut s = session();
    let r = s.execute("library.importPreview", &json!({"paths": [card.to_string_lossy()]})).unwrap();
    assert_eq!(r["scanned"], 1, "{r}");
    write_png(&card.join("a.png"), 9); // other pixels
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [card.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "flat"}),
        )
        .unwrap();
    assert_eq!(len(&r, "imported"), 0, "{r}");
    assert_eq!(len(&r, "failed"), 1, "{r}");
    assert!(r.to_string().contains("changed meanwhile"), "{r}");
    assert_eq!(files_under(&dest), Vec::<String>::new());
    // importing again probes afresh and succeeds
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [card.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "flat"}),
        )
        .unwrap();
    assert_eq!(len(&r, "imported"), 1, "{r}");
    let _ = std::fs::remove_dir_all(&base);
}

/// A source that can't be removed (a read-only card) stays and is reported; the photo is
/// imported from its copy.
#[test]
fn a_source_that_cannot_be_removed_is_kept_and_reported() {
    let base = temp_dir("ro");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("a.png"), 1);
    let mut s = session();
    inject(Fault::FailRemove);
    let r = import_move(&mut s, &[&card], &dest, json!({"organize": "flat"}));
    inject(Fault::None);
    assert_eq!(len(&r, "imported"), 1, "{r}");
    assert_eq!(len(&r, "moved"), 0, "{r}");
    assert_eq!(len(&r, "kept"), 1, "{r}");
    assert!(r["kept"][0]["reason"].as_str().unwrap().contains("could not remove"), "{r}");
    assert!(card.join("a.png").is_file() && dest.join("a.png").is_file());
    assert_eq!(photo_paths(&s), [dest.join("a.png").to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&base);
}

/// Duplicates, unreadable files and files that fail (here: a different sidecar already at the
/// destination) keep their sources; the rest of the batch still moves.
#[test]
fn duplicates_and_failures_keep_their_sources() {
    let base = temp_dir("dup");
    let (lib_src, card, dest) = (base.join("old"), base.join("card"), base.join("out"));
    write_png(&lib_src.join("known.png"), 7);
    let mut s = session();
    s.execute("library.import", &json!({"paths": [lib_src.to_string_lossy()]})).unwrap();
    std::fs::create_dir_all(&card).unwrap();
    std::fs::copy(lib_src.join("known.png"), card.join("same.png")).unwrap(); // same bytes
    write_png(&card.join("a.png"), 1);
    std::fs::write(card.join("a.xmp"), xmp(1)).unwrap();
    write_png(&card.join("b.png"), 2);
    std::fs::write(card.join("broken.jpg"), "garbage").unwrap();
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("a.xmp"), xmp(5)).unwrap(); // someone else's sidecar
    let r = import_move(&mut s, &[&card], &dest, json!({"organize": "flat"}));
    assert_eq!(len(&r, "duplicates"), 1, "{r}");
    assert_eq!(len(&r, "failed"), 2, "{r}"); // broken.jpg (unreadable) and a.png (sidecar clash)
    assert_eq!(len(&r, "moved"), 1, "{r}");
    assert_eq!(r["moved"][0]["from"], card.join("b.png").to_string_lossy().as_ref());
    assert_eq!(files_under(&card), ["a.png", "a.xmp", "broken.jpg", "same.png"]);
    assert_eq!(files_under(&dest), ["a.xmp", "b.png"], "a.png's copy was taken back");
    assert_eq!(std::fs::read_to_string(dest.join("a.xmp")).unwrap(), xmp(5), "never overwritten");
    let _ = std::fs::remove_dir_all(&base);
}

/// A taken name gets -1 (never overwritten), its sidecar follows the new name; a file already
/// inside the destination is added where it is, not moved.
#[test]
fn collisions_are_numbered_and_files_in_the_destination_stay() {
    let base = temp_dir("coll");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("a.png"), 1);
    std::fs::write(card.join("a.xmp"), xmp(2)).unwrap();
    write_png(&dest.join("a.png"), 9); // a different photo, same name
    let theirs = std::fs::read(dest.join("a.png")).unwrap();
    write_png(&dest.join("inbox/c.png"), 3);
    let mut s = session();
    let r = import_move(&mut s, &[&card, &dest.join("inbox")], &dest, json!({"organize": "flat"}));
    assert_eq!(len(&r, "imported"), 2, "{r}");
    assert_eq!(len(&r, "moved"), 1, "{r}");
    assert_eq!(len(&r, "kept"), 1, "{r}");
    assert_eq!(files_under(&dest), ["a-1.png", "a-1.xmp", "a.png", "inbox/c.png"]);
    assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), theirs);
    assert!(photo_paths(&s).contains(&dest.join("inbox").join("c.png").to_string_lossy().to_string()));
    let _ = std::fs::remove_dir_all(&base);
}

/// A stem-named sidecar shared with a photo that isn't moved is copied along but stays beside
/// that photo.
#[test]
fn a_shared_sidecar_stays_with_the_photo_left_behind() {
    let base = temp_dir("shared");
    let (card, dest) = (base.join("card"), base.join("out"));
    write_png(&card.join("x.png"), 1);
    std::fs::write(card.join("x.tif"), "the other half (not imported)").unwrap();
    std::fs::write(card.join("x.xmp"), xmp(2)).unwrap();
    let mut s = session();
    let r = import_move(&mut s, &[&card.join("x.png")], &dest, json!({"organize": "flat"}));
    assert_eq!(len(&r, "moved"), 1, "{r}");
    assert_eq!(files_under(&dest), ["x.png", "x.xmp"]);
    assert_eq!(files_under(&card), ["x.tif", "x.xmp"]);
    assert!(r["kept"][0]["reason"].as_str().unwrap().contains("x.tif"), "{r}");
    let _ = std::fs::remove_dir_all(&base);
}

/// Into the open library's Originals/ (default destination): the records are saved before the
/// sources go, and are there after reopening. Move needs somewhere to go and never browses.
#[test]
fn move_into_the_library_is_saved_before_sources_go() {
    let base = temp_dir("lib");
    let (card, lib) = (base.join("card"), base.join("lib"));
    write_png(&card.join("a.png"), 1);
    let mut s = session();
    let p = json!({"paths": [card.to_string_lossy()], "mode": "move"});
    assert!(s.execute("library.import", &p).is_err(), "no destination, no library");
    let browse = json!({"paths": [card.to_string_lossy()], "mode": "move", "local": true, "destination": base.join("x").to_string_lossy()});
    assert!(s.execute("library.import", &browse).is_err(), "browsing never moves");
    assert!(card.join("a.png").is_file());
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &p).unwrap();
    assert_eq!(len(&r, "moved"), 1, "{r}");
    assert!(!card.join("a.png").exists());
    let want = lib.join("Originals").join("2026").join("2026-01-14").join("a.png");
    assert!(want.is_file());
    drop(s);
    let mut again = Session::new().with_fs();
    again.open_library(&lib, false).unwrap();
    assert_eq!(photo_paths(&again), [want.to_string_lossy()]);
    let _ = std::fs::remove_dir_all(&base);
}

/// Timing for issue #134 (not a pass/fail test): `cargo test -p lightcraft-engine --release
/// write_policy_timing -- --ignored --nocapture`. Writes N export-sized files durably (temp +
/// sync + rename) and atomically only, and copies N files verified byte for byte and against the
/// probe hash, alternating so a loaded machine affects both alike. `LIGHTCRAFT_BENCH_DIR` puts
/// the files on another volume (a USB drive or a NAS is where the difference shows).
#[test]
#[ignore]
fn write_policy_timing() {
    use crate::import_move::copy_verified;
    use lightcraft_catalog::safe_file::{write_atomic, write_atomic_nosync};
    use std::time::{Duration, Instant};
    let root = std::env::var_os("LIGHTCRAFT_BENCH_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let base = root.join(format!("lc-write-policy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let data = |n: usize, seed: u32| -> Vec<u8> { (0..n as u32).map(|i| (i.wrapping_add(seed).wrapping_mul(2_654_435_761) >> 11) as u8).collect() };
    let (n, size) = (200usize, 300 << 10);
    let (mut synced, mut unsynced) = (Duration::ZERO, Duration::ZERO);
    for i in 0..n {
        let b = data(size, i as u32);
        let t = Instant::now();
        write_atomic(&base.join(format!("sync-{i}.jpg")), &b).unwrap();
        synced += t.elapsed();
        let t = Instant::now();
        write_atomic_nosync(&base.join(format!("nosync-{i}.jpg")), &b).unwrap();
        unsynced += t.elapsed();
    }
    println!("export {n} × {} KB: write_atomic (synced) {synced:?}, write_atomic_nosync {unsynced:?}", size >> 10);
    let (n, size) = (40usize, 8 << 20);
    let card = base.join("card");
    std::fs::create_dir_all(&card).unwrap();
    let hashes: Vec<_> = (0..n)
        .map(|i| {
            let b = data(size, 7 * i as u32);
            std::fs::write(card.join(format!("{i}.raw")), &b).unwrap();
            lightcraft_preview::hash_bytes(&b)
        })
        .collect();
    let (mut bytewise, mut hashed) = (Duration::ZERO, Duration::ZERO);
    for (i, h) in hashes.iter().enumerate() {
        let src = card.join(format!("{i}.raw"));
        let t = Instant::now();
        copy_verified(&src, &base.join(format!("bytes-{i}.raw")), None).unwrap();
        bytewise += t.elapsed();
        let t = Instant::now();
        copy_verified(&src, &base.join(format!("hash-{i}.raw")), Some(*h)).unwrap();
        hashed += t.elapsed();
    }
    println!("copy {n} × {} MB: byte compare with the source {bytewise:?}, probe hash {hashed:?}", size >> 20);
    let _ = std::fs::remove_dir_all(&base);
}
