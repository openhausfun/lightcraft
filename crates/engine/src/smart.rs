//! Smart previews: a compact proxy of each photo (its decoded preview-size source, about 1 MB)
//! kept in the library, so photos stay editable — and exportable at proxy size — while their
//! originals are offline (an unplugged drive). The proxy is the scene-linear source scaled into
//! 0..1 by a stored factor, sRGB-encoded and saved as a JPEG after a one-line header. The header
//! also keeps the decoder's file-local camera tone curve (Sony ARW camera look), which is applied
//! at render time and is not baked into the pixels.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lightcraft_catalog::Photo;
use lightcraft_color::transfer::linear_to_srgb;
use lightcraft_pipeline::tone::CameraTone;
use lightcraft_raster::{Rgb32f, Rgba8};

const MAGIC: &[u8] = b"LCSP1\n";

/// The proxy's file name for a photo (by its content, so copies share one).
pub fn file_name(p: &Photo) -> String {
    let h = lightcraft_preview::Hasher128::new().str(&crate::media::content_key(p)).finish();
    format!("{:032x}.lcsp", h.0)
}

/// The most header [`is_valid`] reads (the JSON line before the JPEG).
const HEADER_MAX: u64 = 4096;

/// Encode a source image (and the decoder's camera tone curve, if any) as a smart preview.
pub fn encode(img: &Rgb32f, tone: Option<&CameraTone>) -> Result<Vec<u8>, String> {
    // scale so all but the brightest 0.05 % fit into 0..1
    let mut lum: Vec<f32> = img.data.iter().map(|c| c[0].max(c[1]).max(c[2])).filter(|v| v.is_finite()).collect();
    let scale = if lum.is_empty() {
        1.0
    } else {
        let k = ((lum.len() as f64) * 0.9995) as usize;
        let k = k.min(lum.len() - 1);
        let (_, v, _) = lum.select_nth_unstable_by(k, f32::total_cmp);
        v.max(1.0)
    };
    let data: Vec<[u8; 4]> = img
        .data
        .iter()
        .map(|c| {
            let e = |v: f32| (linear_to_srgb((v / scale).clamp(0.0, 1.0)) * 255.0).round() as u8;
            [e(c[0]), e(c[1]), e(c[2]), 255]
        })
        .collect();
    let rgba = Rgba8 { width: img.width, height: img.height, data };
    let jpg = lightcraft_codecs::encode_jpeg(
        &lightcraft_codecs::EncodeImage::rgba8(&rgba),
        92,
        lightcraft_codecs::ChromaSubsampling::S444,
        &Default::default(),
    )
    .map_err(|e| e.to_string())?;
    let mut out = MAGIC.to_vec();
    let mut head = serde_json::json!({"w": img.width, "h": img.height, "scale": scale});
    if let Some(t) = tone {
        head["tone"] = serde_json::to_value(t).map_err(|e| e.to_string())?;
    }
    out.extend_from_slice(head.to_string().as_bytes());
    out.push(b'\n');
    out.extend_from_slice(&jpg);
    Ok(out)
}

/// Decode a smart preview back into a source image and its stored camera tone curve (an invalid
/// curve is ignored, like a missing one).
pub fn decode(bytes: &[u8]) -> Result<(Rgb32f, Option<CameraTone>), String> {
    let rest = bytes.strip_prefix(MAGIC).ok_or("not a smart preview")?;
    let nl = rest.iter().position(|b| *b == b'\n').ok_or("bad smart preview")?;
    let head: serde_json::Value = serde_json::from_slice(&rest[..nl]).map_err(|e| e.to_string())?;
    let scale = head["scale"].as_f64().unwrap_or(1.0) as f32;
    let tone = head.get("tone").and_then(|t| serde_json::from_value::<CameraTone>(t.clone()).ok());
    let d = lightcraft_codecs::decode(&rest[nl + 1..], Default::default()).map_err(|e| e.to_string())?;
    // the decoder undoes the sRGB encoding; the values are the source's own primaries
    let mut img = d.image;
    img.data.iter_mut().for_each(|c| *c = c.map(|v| v * scale));
    Ok((img, tone))
}

/// Where a library keeps its smart previews.
pub fn dir(library: &Path) -> PathBuf {
    library.join("Smart Previews")
}

/// Check that smart previews can be written to `dir`: it must be (or, when its parent exists,
/// become) a directory that accepts a new file. Never creates missing parents, so an unplugged
/// drive's mount point is not silently recreated on the system drive.
pub fn check_writable(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        let parent_ok = dir.parent().is_some_and(|p| p.is_dir());
        if !parent_ok {
            return Err(format!("{} is not available — reconnect the drive or choose another folder", dir.display()));
        }
        std::fs::create_dir(dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    }
    let probe = dir.join(format!(".lightcraft-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"x").map_err(|e| format!("{} is not writable (is the drive full or read-only?): {e}", dir.display()))?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// The number of smart previews in `dir` and their total size in bytes.
pub fn stats(dir: &Path) -> (usize, u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0) };
    rd.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lcsp"))
        .fold((0, 0), |(n, b), e| (n + 1, b + e.metadata().map_or(0, |m| m.len())))
}

/// What to do with the smart previews in the old folder when the location changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    /// Move them to the new folder (copy and delete across drives).
    Move,
    /// Leave them where they are (they stop being used; build again in the new folder).
    Leave,
    /// Delete them.
    Discard,
}

impl Existing {
    pub fn parse(s: &str) -> Option<Existing> {
        match s {
            "move" => Some(Existing::Move),
            "leave" => Some(Existing::Leave),
            "discard" => Some(Existing::Discard),
            _ => None,
        }
    }
}

/// Apply `what` to the smart previews in `from` for the new folder `to` → (moved or removed, failed).
pub fn migrate(from: &Path, to: &Path, what: Existing) -> (usize, Vec<String>) {
    let (mut done, mut failed) = (0, Vec::new());
    if what == Existing::Leave {
        return (done, failed);
    }
    let Ok(rd) = std::fs::read_dir(from) else { return (done, failed) };
    for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "lcsp")) {
        let src = e.path();
        let r = match what {
            Existing::Discard => std::fs::remove_file(&src),
            _ => {
                let dst = to.join(e.file_name());
                if is_valid(&dst) {
                    // named by content: the complete copy already there is the same preview
                    std::fs::remove_file(&src)
                } else {
                    // across drives: copied atomically (never a partial proxy at the new place)
                    std::fs::rename(&src, &dst).or_else(|_| {
                        std::fs::read(&src)
                            .and_then(|b| lightcraft_catalog::safe_file::write_atomic(&dst, &b))
                            .and_then(|()| std::fs::remove_file(&src))
                    })
                }
            }
        };
        match r {
            Ok(()) => done += 1,
            Err(err) => failed.push(format!("{}: {err}", src.display())),
        }
    }
    (done, failed)
}

/// A cheap check that the smart preview at `path` is complete: our header, then a JPEG from its
/// start marker to its end marker. A file cut short by a crash, a full drive or an unplugged one
/// fails it (and is then rebuilt by Build Smart Previews, not counted as there).
pub fn is_valid(path: &Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = Vec::with_capacity(1024);
    if (&mut f).take(HEADER_MAX).read_to_end(&mut head).is_err() {
        return false;
    }
    let Some(rest) = head.strip_prefix(MAGIC) else { return false };
    let Some(nl) = rest.iter().position(|b| *b == b'\n') else { return false };
    let size_ok = rest
        .get(..nl)
        .and_then(|h| serde_json::from_slice::<serde_json::Value>(h).ok())
        .is_some_and(|h| h["w"].as_u64().is_some_and(|w| w > 0) && h["h"].as_u64().is_some_and(|h| h > 0));
    if !size_ok || rest.get(nl + 1..nl + 3) != Some(&[0xFF, 0xD8][..]) {
        return false;
    }
    let mut end = [0u8; 2];
    f.seek(SeekFrom::End(-2)).is_ok() && f.read_exact(&mut end).is_ok() && end == [0xFF, 0xD9]
}

/// Load the proxy at `path`.
pub fn load(path: &Path) -> Result<crate::media::DecodedSource, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    decode(&b).map(|(image, camera_tone)| crate::media::DecodedSource { image: Arc::new(image), info: None, camera_tone })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-smart-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writable_check_does_not_create_missing_parents() {
        let base = temp("writable");
        assert!(check_writable(&base.join("new")).is_ok(), "one missing level is created");
        assert!(base.join("new").is_dir());
        let err = check_writable(&base.join("gone/deeper")).unwrap_err();
        assert!(err.contains("not available"), "{err}");
        assert!(!base.join("gone").exists(), "no fallback onto the current drive");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn migrate_moves_leaves_or_discards() {
        let base = temp("migrate");
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("1.lcsp"), b"one").unwrap();
        std::fs::write(a.join("note.txt"), b"keep").unwrap();
        assert_eq!(stats(&a), (1, 3));
        assert_eq!(migrate(&a, &b, Existing::Leave).0, 0);
        assert_eq!(stats(&a).0, 1);
        assert_eq!(migrate(&a, &b, Existing::Move).0, 1);
        assert_eq!((stats(&a).0, stats(&b).0), (0, 1));
        assert!(a.join("note.txt").exists(), "only smart previews are touched");
        assert_eq!(migrate(&b, &a, Existing::Discard).0, 1);
        assert_eq!(stats(&b).0, 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn damaged_proxies_are_invalid_and_never_kept_over_good_ones() {
        let base = temp("valid");
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let img = lightcraft_scenes::demo_library()[0].render(64, 40);
        let good = encode(&img, None).unwrap();
        std::fs::write(a.join("1.lcsp"), &good).unwrap();
        assert!(is_valid(&a.join("1.lcsp")));
        for cut in [good.len() - 1, good.len() / 2, 10, 0] {
            std::fs::write(b.join("1.lcsp"), &good[..cut]).unwrap();
            assert!(!is_valid(&b.join("1.lcsp")), "cut at {cut}");
        }
        assert!(!is_valid(&b.join("missing.lcsp")));
        // moving onto a damaged copy replaces it instead of deleting the good one
        assert_eq!(migrate(&a, &b, Existing::Move).0, 1);
        assert_eq!(std::fs::read(b.join("1.lcsp")).unwrap(), good);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn camera_tone_travels_with_the_proxy() {
        let tone = CameraTone::new(std::array::from_fn(|i| {
            let x = 0.004 * 1.17f32.powi(i as i32);
            [x, 0.95 * (1.0 - (-2.7 * x).exp())]
        }))
        .unwrap();
        let img = lightcraft_scenes::demo_library()[0].render(64, 40);
        let bytes = encode(&img, Some(&tone)).unwrap();
        assert_eq!(decode(&bytes).unwrap().1, Some(tone));
        let dir = temp("tone");
        let path = dir.join("t.lcsp");
        std::fs::write(&path, &bytes).unwrap();
        // the longer header is still read by the validity check, and the curve by `load`
        assert!(is_valid(&path));
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.camera_tone, Some(tone));
        let header = lightcraft_pipeline::SourceInfo { raw: true, ..Default::default() };
        assert_eq!(loaded.info_or(header).camera_tone, Some(tone));
        // a hostile curve (reversing knots) is ignored, not trusted
        let nl = MAGIC.len() + bytes[MAGIC.len()..].iter().position(|b| *b == b'\n').unwrap();
        let mut head: serde_json::Value = serde_json::from_slice(&bytes[MAGIC.len()..nl]).unwrap();
        let mut knots: Vec<[f32; 2]> = serde_json::from_value(head["tone"]["knots"].clone()).unwrap();
        knots.swap(3, 4);
        head["tone"]["knots"] = serde_json::json!(knots);
        let mut hostile = MAGIC.to_vec();
        hostile.extend_from_slice(head.to_string().as_bytes());
        hostile.extend_from_slice(&bytes[nl..]);
        let (pixels, tone) = decode(&hostile).unwrap();
        assert_eq!((pixels.width, tone), (img.width, None));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_keeps_the_picture() {
        let img = lightcraft_scenes::demo_library()[0].render(160, 100);
        let (back, tone) = decode(&encode(&img, None).unwrap()).unwrap();
        assert_eq!((back.width, back.height), (img.width, img.height));
        assert!(tone.is_none());
        let mut err = 0.0f64;
        for (a, b) in img.data.iter().zip(&back.data) {
            for k in 0..3 {
                let (x, y) = (linear_to_srgb(a[k].clamp(0.0, 1.0)), linear_to_srgb(b[k].clamp(0.0, 1.0)));
                err += ((x - y) as f64).abs();
            }
        }
        let mean = err / (img.data.len() * 3) as f64;
        assert!(mean < 0.02, "mean encoded error {mean}");
        assert!(decode(b"nope").is_err());
    }
}
