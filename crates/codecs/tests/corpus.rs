//! Corpus tests over `corpus/images/**`, `corpus/pngsuite/**` and `corpus/raw/**` (git-ignored; fetched
//! with `cargo xtask corpus --download`). Each test skips cleanly when its directory is absent.
//! `LIGHTCRAFT_CORPUS` overrides the corpus root.

use lightcraft_codecs::*;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    std::env::var_os("LIGHTCRAFT_CORPUS").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

#[test]
fn corpus_images_decode() {
    let dir = corpus_root().join("images");
    if !dir.is_dir() {
        eprintln!("skip: {} absent", dir.display());
        return;
    }
    let (mut ok, mut unsupported) = (0, 0);
    for p in files(&dir) {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let Some(f) = sniff(&bytes) else { continue };
        if !f.can_decode() {
            unsupported += 1;
            assert!(decode(&bytes, DecodeOptions::default()).is_err());
            continue;
        }
        let d = decode(&bytes, DecodeOptions::default()).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert!(d.width > 0 && d.height > 0);
        assert!(d.image.data.iter().all(|px| px.iter().all(|v| v.is_finite())), "{}: non-finite", p.display());
        let t = decode_thumbnail(&bytes, 256).unwrap_or_else(|e| panic!("{} thumb: {e}", p.display()));
        assert!(t.image.width <= 256 && t.image.height <= 256);
        ok += 1;
    }
    eprintln!("corpus/images: {ok} decoded, {unsupported} unsupported formats");
}

#[test]
fn corpus_pngsuite() {
    let dir = corpus_root().join("pngsuite");
    if !dir.is_dir() {
        eprintln!("skip: {} absent", dir.display());
        return;
    }
    let mut n = 0;
    for p in files(&dir) {
        if p.extension().and_then(|e| e.to_str()) != Some("png") {
            continue;
        }
        let bytes = std::fs::read(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let r = decode(&bytes, DecodeOptions::default());
        if name.starts_with('x') {
            // Deliberately corrupt files: must fail gracefully (never panic).
            continue;
        }
        let d = r.unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(d.width > 0);
        n += 1;
    }
    eprintln!("pngsuite: {n} valid files decoded");
}

#[test]
fn corpus_raw_is_routed() {
    let dir = corpus_root().join("raw");
    if !dir.is_dir() {
        eprintln!("skip: {} absent", dir.display());
        return;
    }
    for p in files(&dir) {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if matches!(ext.as_str(), "txt" | "md" | "xmp" | "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let f = sniff(&bytes);
        assert!(f.is_some_and(|f| f.is_raw()), "{}: sniffed {f:?}", p.display());
    }
}

/// Parse the OS-provided ICC profiles when present (macOS ColorSync; read-only, nothing copied).
#[test]
fn system_icc_profiles() {
    use lightcraft_codecs::icc::{IccColorModel, IccKind, parse};
    let dir = Path::new("/System/Library/ColorSync/Profiles");
    if !dir.is_dir() {
        eprintln!("skip: no system ICC profiles");
        return;
    }
    let expect = [
        ("sRGB Profile.icc", Some(NamedSpace::Srgb)),
        ("Display P3.icc", Some(NamedSpace::DisplayP3)),
        ("AdobeRGB1998.icc", Some(NamedSpace::AdobeRgb)),
        ("ROMM RGB.icc", Some(NamedSpace::ProPhoto)),
        ("ITU-2020.icc", Some(NamedSpace::Rec2020)),
    ];
    for (name, named) in expect {
        let Ok(b) = std::fs::read(dir.join(name)) else { continue };
        let info = parse(&b).unwrap_or_else(|| panic!("{name}"));
        assert_eq!(info.named, named, "{name}");
        assert!(matches!(info.kind, IccKind::MatrixTrc { .. }), "{name}");
    }
    if let Ok(b) = std::fs::read(dir.join("Generic CMYK Profile.icc")) {
        let info = parse(&b).unwrap();
        assert_eq!(info.model, IccColorModel::Cmyk);
        assert_eq!(info.kind, IccKind::NeedsCms);
        // CMYK TIFF tagged with it: paper white stays near white, 100% K near black.
        use tiff::encoder::{TiffEncoder, colortype};
        let mut cur = std::io::Cursor::new(Vec::new());
        {
            let mut enc = TiffEncoder::new(&mut cur).unwrap();
            let mut im = enc.new_image::<colortype::CMYK8>(2, 1).unwrap();
            im.encoder().write_tag(tiff::tags::Tag::IccProfile, &b[..]).unwrap();
            im.write_data(&[0u8, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        }
        let d = decode(&cur.into_inner(), DecodeOptions::default()).unwrap();
        assert_eq!(d.space.origin, SpaceOrigin::IccCms);
        let w = d.image.get(0, 0);
        let k = d.image.get(1, 0);
        assert!(w.iter().all(|&v| v > 0.8), "{w:?}");
        assert!(k.iter().all(|&v| v < 0.1), "{k:?}");
    }
    for p in files(dir) {
        let b = std::fs::read(&p).unwrap();
        let _ = parse(&b);
    }
}
