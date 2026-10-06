//! XMP sidecars end to end: save → delete library → re-import restores edits; auto-write; foreign
//! `crs:` sidecars; DNG-embedded XMP; read metadata from file (undoable).

use std::path::{Path, PathBuf};

use lightcraft_catalog::{ColorLabel, Flag, PhotoId};
use serde_json::json;

use crate::Session;

pub(crate) fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("lc-xmp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_png(path: &Path, seed: u8) {
    let (w, h) = (40usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 6) as u8, (i / w * 9) as u8, seed, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn only(s: &Session) -> PhotoId {
    s.catalog.photos().next().unwrap().id
}

#[test]
fn save_delete_library_reimport_restores_edits() {
    let src = temp_dir("roundtrip-src");
    let lib = temp_dir("roundtrip-lib");
    write_png(&src.join("dune.png"), 1);
    write_png(&src.join("plain.png"), 2);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("dune.png").to_string_lossy()]})).unwrap();
    let id = only(&s);
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    s.execute("photo.pick", &json!({})).unwrap();
    s.execute("photo.label", &json!({"label": "blue"})).unwrap();
    s.execute("photo.setMeta", &json!({"title": "Dune", "caption": "Evening light", "copyright": "© me", "keywords": ["sand", "dusk"]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.8})).unwrap();
    s.execute("develop.set", &json!({"control": "effects.clarity", "value": 25})).unwrap();
    s.execute("mask.add", &json!({"kind": "radial"})).unwrap();
    let edited = s.catalog.photo(id).unwrap().clone();
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    assert_eq!(r["written"][0], src.join("dune.xmp").to_string_lossy().as_ref(), "{r}");
    assert!(src.join("dune.xmp").is_file());
    assert!(!src.join("plain.xmp").exists());
    drop(s);

    // delete the library; a fresh library re-imports the folder
    std::fs::remove_dir_all(&lib).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["sidecars"], 1, "{r}");
    let p = s.catalog.photos().find(|p| p.file_name == "dune.png").unwrap().clone();
    assert_eq!(p.develop, edited.develop, "develop settings restored exactly");
    assert_eq!(p.develop.masks.len(), 1);
    assert_eq!((p.rating, p.flag, p.label), (4, Flag::Pick, Some(ColorLabel::Blue)));
    assert_eq!(p.meta.title, "Dune");
    assert_eq!(p.meta.caption, "Evening light");
    assert_eq!(p.meta.copyright, "© me");
    assert_eq!(p.meta.keywords, vec!["sand".to_string(), "dusk".to_string()]);
    assert!(p.edited.is_some());
    let plain = s.catalog.photos().find(|p| p.file_name == "plain.png").unwrap();
    assert!(plain.develop.is_unedited() && plain.rating == 0);
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

#[test]
fn auto_write_and_naming_preference() {
    let src = temp_dir("auto");
    let lib = temp_dir("auto-lib");
    write_png(&src.join("a.png"), 3);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 2})).unwrap();
    assert!(!src.join("a.xmp").exists(), "auto-write is off by default");

    let v = s.execute("library.xmpPreferences", &json!({"autoWrite": true, "naming": "full"})).unwrap();
    assert_eq!(v, json!({"autoWrite": true, "naming": "full"}));
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    let sidecar = src.join("a.png.xmp");
    assert!(sidecar.is_file());
    assert!(std::fs::read_to_string(&sidecar).unwrap().contains("<xmp:Rating>5</xmp:Rating>"));
    // a slider drag writes once, at the end
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.5})).unwrap();
    let exposure = |p: &Path| match crate::sidecar::parse_sidecar(&std::fs::read_to_string(p).unwrap(), false).unwrap().develop {
        Some(crate::sidecar::DevelopPatch::Full(d)) => d.light.exposure,
        other => panic!("{other:?}"),
    };
    assert_eq!(exposure(&sidecar), 0.0, "not written mid-drag");
    s.execute("develop.endInteraction", &json!({})).unwrap();
    assert_eq!(exposure(&sidecar), 1.5);
    // undo rewrites too
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(exposure(&sidecar), 0.0);

    // the preference persists with the library
    drop(s);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    assert!(s.xmp.auto_write);
    assert_eq!(s.xmp.naming, crate::sidecar::SidecarNaming::Full);
    let v = s.execute("library.toggleAutoWriteXmp", &json!({})).unwrap();
    assert_eq!(v["autoWrite"], false);
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Hand-written sidecar in the style other raw developers use (attribute form, `crs:` fields).
const FOREIGN: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="3" xmp:Label="Red"
    crs:ProcessVersion="11.0" crs:IncrementalTemperature="+20" crs:IncrementalTint="-5"
    crs:Exposure2012="+1.10" crs:Shadows2012="+35" crs:Vibrance="+15" crs:Clarity2012="+10"
    crs:SplitToningShadowHue="220" crs:SplitToningShadowSaturation="18"
    crs:HasCrop="True" crs:CropLeft="0.1" crs:CropTop="0" crs:CropRight="0.9" crs:CropBottom="1" crs:CropAngle="0">
   <dc:subject><rdf:Bag><rdf:li>harbour</rdf:li></rdf:Bag></dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

#[test]
fn foreign_crs_sidecar_on_import_and_read_from_file() {
    let src = temp_dir("foreign");
    write_png(&src.join("harbour.png"), 4);
    std::fs::write(src.join("harbour.xmp"), FOREIGN).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(r["sidecars"], 1);
    let p = s.catalog.photos().next().unwrap().clone();
    assert_eq!((p.rating, p.label), (3, Some(ColorLabel::Red)));
    assert_eq!(p.meta.keywords, vec!["harbour".to_string()]);
    let d = &p.develop;
    assert_eq!((d.light.exposure, d.light.shadows, d.color.vibrance, d.effects.clarity), (1.1, 35.0, 15.0, 10.0));
    assert!(d.wb.temp > 6500.0 && d.wb.tint == -5.0, "relative WB for a rendered file: {:?}", d.wb);
    assert_eq!((d.grading.shadows.hue, d.grading.shadows.sat), (220.0, 18.0));
    assert_eq!((d.crop.geometry.rect.x0, d.crop.geometry.rect.x1), (0.1, 0.9));
    assert!(s.render_now(p.id, 32, 32).is_ok());

    // change things, then read the sidecar again: one undo step restores the edits
    s.execute("library.select", &json!({"ids": [p.id.0]})).unwrap();
    s.execute("develop.reset", &json!({})).unwrap();
    s.execute("photo.rate", &json!({"rating": 1})).unwrap();
    let r = s.execute("photo.readMetadataFromFile", &json!({})).unwrap();
    assert_eq!(r["read"].as_array().unwrap().len(), 1, "{r}");
    let q = s.catalog.photo(p.id).unwrap();
    assert_eq!(q.rating, 3);
    assert_eq!(q.develop.light.exposure, 1.1);
    s.execute("edit.undo", &json!({})).unwrap();
    let q = s.catalog.photo(p.id).unwrap();
    assert_eq!((q.rating, q.develop.light.exposure), (1, 0.0));
    // no sidecar → reported, not an error
    std::fs::remove_file(src.join("harbour.xmp")).unwrap();
    let r = s.execute("photo.readMetadataFromFile", &json!({})).unwrap();
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&src);
}

fn synthetic_dng(xmp: &str) -> Vec<u8> {
    synthetic_dng_with(Some(xmp), lightcraft_meta::Metadata::default())
}

/// A tiny Bayer DNG written by our own DNG writer (optional embedded XMP, camera metadata).
pub(crate) fn synthetic_dng_with(xmp: Option<&str>, metadata: lightcraft_meta::Metadata) -> Vec<u8> {
    use lightcraft_raw::*;
    let (w, h) = (32usize, 24usize);
    let cfa = Cfa::bayer("RGGB").unwrap();
    let data: Vec<u16> = (0..w * h).map(|i| 256 + ((i % w) * 300 + (i / w) * 200) as u16).collect();
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(cfa),
        bits: 14,
        black: BlackLevel::uniform(256.0),
        white: vec![16000.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Normal,
        color: ColorData {
            illuminant: [17, 21],
            color_matrix: [
                Some(lightcraft_color::Mat3([[0.9, 0.2, -0.15], [-0.3, 1.25, 0.08], [0.02, -0.12, 0.85]])),
                Some(lightcraft_color::Mat3([[0.7, 0.3, -0.1], [-0.35, 1.3, 0.1], [0.05, -0.2, 1.0]])),
            ],
            as_shot_neutral: Some([0.5, 1.0, 0.7]),
            ..Default::default()
        },
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata,
    };
    write_dng(&raw, &DngWriteOptions { xmp: xmp.map(str::to_string), ..Default::default() }).unwrap()
}

#[test]
fn dng_embedded_crs_settings_are_read_on_import() {
    let src = temp_dir("dng");
    let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
        xmp:Rating="2" crs:WhiteBalance="Custom" crs:Temperature="4300" crs:Tint="+7" crs:Contrast2012="+30" crs:Dehaze="+12"/>
      </rdf:RDF></x:xmpmeta>"#;
    std::fs::write(src.join("synth.dng"), synthetic_dng(x)).unwrap();
    let mut s = Session::new().with_fs();
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!((r["imported"].as_array().unwrap().len(), r["sidecars"].as_u64()), (1, Some(1)), "{r}");
    let p = s.catalog.photos().next().unwrap();
    assert_eq!(p.kind, lightcraft_catalog::MediaKind::Raw);
    assert_eq!((p.develop.wb.temp, p.develop.wb.tint), (4300.0, 7.0));
    assert_eq!((p.develop.light.contrast, p.develop.effects.dehaze), (30.0, 12.0));
    assert_eq!(p.develop.detail.sharpen_amount, 40.0, "raw defaults kept for fields the XMP doesn't set");
    assert_eq!(p.rating, 2);

    // a sidecar next to the DNG takes precedence over the embedded XMP
    let id = p.id;
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("develop.set", &json!({"control": "light.contrast", "value": -10})).unwrap();
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let mut s2 = Session::new().with_fs();
    s2.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    assert_eq!(s2.catalog.photos().next().unwrap().develop.light.contrast, -10.0);
    let _ = std::fs::remove_dir_all(&src);
}

#[test]
fn demo_photos_have_no_sidecar() {
    let mut s = Session::with_demo();
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    assert_eq!(r["written"].as_array().unwrap().len(), 0);
    assert_eq!(r["failed"].as_array().unwrap().len(), 1);
}

/// All Metadata: EXIF rows from the file and its XMP, for a photo on disk.
#[test]
fn all_metadata_lists_exif_and_xmp() {
    let dir = temp_dir("allmeta");
    let path = dir.join("tagged.jpg");
    let img = lightcraft_raster::Rgba8 { width: 16, height: 8, data: vec![[90, 120, 200, 255]; 128] };
    let exif = lightcraft_meta::write_exif(&lightcraft_meta::Metadata { make: Some("Maker".into()), iso: Some(800), ..Default::default() });
    let meta = lightcraft_codecs::EncodeMeta { exif: Some(&exif), ..Default::default() };
    let jpg = lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 90, Default::default(), &meta).unwrap();
    std::fs::write(&path, jpg).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [path.to_string_lossy()]})).unwrap();
    s.execute("photo.setMeta", &json!({"title": "A title"})).unwrap();
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let r = s.execute("photo.allMetadata", &json!({})).unwrap();
    let exif = r["exif"].as_array().unwrap();
    assert!(exif.iter().any(|e| e["name"] == "Make" && e["value"] == "Maker"), "{r}");
    assert!(exif.iter().any(|e| e["name"] == "ISO Speed" && e["value"] == "800"));
    assert!(r["xmp"].as_array().unwrap().iter().any(|x| x["name"] == "dc:title" && x["value"] == "A title"), "the sidecar's fields: {r}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Convert to DNG: a corpus raw (skipped without the corpus) becomes a DNG next to it with the
/// photo relinked and its edits kept; undo goes back to the original; non-raws are skipped.
#[test]
fn convert_raw_to_dng() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
        .join("raw/nef-nikon-d5100-uncompressed.nef");
    let mut s = Session::new().with_fs();
    // a non-raw is skipped
    let dir = temp_dir("todng");
    let png = dir.join("a.png");
    write_png(&png, 3);
    s.execute("library.import", &json!({"paths": [png.to_string_lossy()]})).unwrap();
    let r = s.execute("photo.convertToDng", &json!({})).unwrap();
    assert_eq!((r["converted"].as_array().unwrap().len(), r["skipped"].as_array().unwrap().len()), (0, 1));
    if !corpus.exists() {
        eprintln!("skip: {} absent", corpus.display());
        return;
    }
    let nef = dir.join("shot.nef");
    std::fs::copy(&corpus, &nef).unwrap();
    s.execute("library.import", &json!({"paths": [nef.to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.6})).unwrap();
    let r = s.execute("photo.convertToDng", &json!({})).unwrap();
    assert_eq!(r["converted"].as_array().unwrap().len(), 1, "{r}");
    let p = s.catalog.photo(id).unwrap().clone();
    assert_eq!((p.file_name.as_str(), p.format.as_str()), ("shot.dng", "DNG"));
    assert!(dir.join("shot.dng").exists() && nef.exists(), "the original is kept");
    assert_eq!(p.develop.light.exposure, 0.6, "edits kept");
    let bytes = std::fs::read(dir.join("shot.dng")).unwrap();
    assert_eq!(lightcraft_raw::probe(&bytes), Some(lightcraft_raw::RawFormat::Dng));
    assert!(s.render_now(id, 64, 64).is_ok(), "the DNG renders");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().file_name, "shot.nef");
    // Copy as DNG at import: only the DNG lands in the destination
    let card = dir.join("card");
    std::fs::create_dir_all(&card).unwrap();
    std::fs::copy(&corpus, card.join("DSC_1.NEF")).unwrap();
    let dest = dir.join("dest");
    let mut s = Session::new().with_fs();
    let r = s
        .execute(
            "library.import",
            &json!({"paths": [card.to_string_lossy()], "mode": "copy", "destination": dest.to_string_lossy(), "organize": "flat", "dng": true}),
        )
        .unwrap();
    assert_eq!(r["imported"].as_array().unwrap().len(), 1, "{r}");
    let names: Vec<String> = std::fs::read_dir(&dest).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names, ["DSC_1.dng"]);
    assert!(card.join("DSC_1.NEF").exists(), "the card is untouched");
    let id2 = lightcraft_catalog::PhotoId(r["imported"][0].as_u64().unwrap());
    assert_eq!(s.catalog.photo(id2).unwrap().format, "DNG");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Issue #106: Convert to DNG wrote straight to the final name (no temp file, sync or check) and
/// relinked the photo even when the write was cut short. A failed write now leaves no DNG and
/// the photo on its raw; a good one is verified and never replaces an existing file.
#[test]
fn convert_to_dng_is_verified_and_atomic() {
    let dir = temp_dir("todng-safe");
    // a synthetic raw (stored as DNG, catalogued as a NEF so Convert to DNG takes it)
    let raw = dir.join("shot.dng");
    let bytes = synthetic_dng_with(None, Default::default());
    std::fs::write(&raw, &bytes).unwrap();
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [raw.to_string_lossy()]})).unwrap();
    let id = s.catalog.photos().next().unwrap().id;
    let path = raw.to_string_lossy().to_string();
    s.catalog
        .apply(lightcraft_catalog::Op::Relink {
            id,
            file_name: "shot.dng".into(),
            source: lightcraft_catalog::Source::File { path: path.clone() },
            format: Some("NEF".into()),
        })
        .unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    let names = || {
        let mut v: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
        v.sort();
        v
    };
    {
        // the drive fills up part-way
        let _fault = lightcraft_catalog::safe_file::fail_writes_after(200);
        let r = s.execute("photo.convertToDng", &json!({})).unwrap();
        assert_eq!(r["converted"].as_array().unwrap().len(), 0, "{r}");
        assert!(r["skipped"][0][1].as_str().unwrap().contains("the raw is kept"), "{r}");
    }
    assert_eq!(names(), ["shot.dng"], "no partial DNG");
    assert_eq!(s.catalog.photo(id).unwrap().source, lightcraft_catalog::Source::File { path: path.clone() }, "not relinked");
    // a good conversion: a new name (the existing file is not replaced), decodable, relinked
    let r = s.execute("photo.convertToDng", &json!({})).unwrap();
    let out = r["converted"][0]["path"].as_str().unwrap().to_string();
    assert!(out.ends_with("shot-2.dng"), "{r}");
    assert_eq!(std::fs::read(&raw).unwrap(), bytes, "the raw is untouched");
    let back = lightcraft_raw::decode(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(back.data, lightcraft_raw::decode(&bytes).unwrap().data);
    assert_eq!(s.catalog.photo(id).unwrap().source, lightcraft_catalog::Source::File { path: out });
    assert_eq!(names(), ["shot-2.dng", "shot.dng"], "no temp file left");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Edit in External Editor (engine half): a 16-bit TIFF `-Edit` copy with the edits, next to
/// the original, added and stacked on top of it; a second one doesn't overwrite the first.
#[test]
fn external_edit_copy_is_stacked() {
    let dir = temp_dir("extedit");
    let src = dir.join("beach.png");
    write_png(&src, 9);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let orig = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    let r = s.execute("photo.editExternal", &json!({})).unwrap();
    let path = r["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("beach-Edit.tif"), "{path}");
    let bytes = std::fs::read(&path).unwrap();
    let d = lightcraft_codecs::decode(&bytes, Default::default()).unwrap();
    assert_eq!(d.bit_depth, 16);
    let new = lightcraft_catalog::PhotoId(r["id"].as_u64().unwrap());
    assert_eq!(s.active(), Some(new));
    let st = s.catalog.stack_of(new).expect("stacked");
    assert_eq!((st.top(), st.photos.contains(&orig)), (new, true));
    let r2 = s.execute("photo.editExternal", &json!({"colorSpace": "proPhoto"})).unwrap();
    assert!(r2["path"].as_str().unwrap().ends_with("beach-Edit-Edit.tif") || r2["path"].as_str().unwrap().contains("-2"), "{r2}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reload: a file changed on disk (an external editor saved it) gets its new size / hash, the
/// cached source is dropped and renders change; unchanged files are left alone.
#[test]
fn reload_picks_up_changed_files() {
    let dir = temp_dir("reload");
    let f = dir.join("x.png");
    write_png(&f, 10);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [f.to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let before = s.render_now(id, 32, 32).unwrap().image.data;
    let key0 = s.thumb_job(id, 128).unwrap().key;
    assert_eq!(s.execute("photo.reload", &json!({})).unwrap()["reloaded"], json!([]), "unchanged");
    write_png(&f, 200);
    let r = s.execute("photo.reload", &json!({})).unwrap();
    assert_eq!(r["reloaded"], json!([id.0]));
    assert_ne!(s.thumb_job(id, 128).unwrap().key, key0, "the UI sees a new render key");
    assert_ne!(s.render_now(id, 32, 32).unwrap().image.data, before, "renders the new pixels");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_copies_the_file_and_the_edits() {
    let dir = temp_dir("dup");
    let f = dir.join("pic.png");
    write_png(&f, 5);
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [f.to_string_lossy()]})).unwrap();
    let orig = s.active().unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.8})).unwrap();
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    let alb = s.execute("album.create", &json!({"name": "Keep", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
    let r = s.execute("photo.duplicate", &json!({})).unwrap();
    let dup = lightcraft_catalog::PhotoId(r["ids"][0].as_u64().unwrap());
    let d = s.catalog.photo(dup).unwrap().clone();
    assert_eq!(d.file_name, "pic-copy.png");
    assert!(dir.join("pic-copy.png").exists());
    assert_eq!((d.develop.light.exposure, d.rating), (0.8, 4));
    assert_eq!(s.catalog.album_count(lightcraft_catalog::AlbumId(alb)), 2);
    assert_ne!(dup, orig);
    s.execute("edit.undo", &json!({})).unwrap();
    assert!(s.catalog.photo(dup).is_none(), "one undo step");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gps_typed_in_is_saved_to_xmp() {
    let mut s = Session::with_demo();
    let id = s.active().unwrap_or_else(|| s.catalog.photos().next().unwrap().id);
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("photo.setMeta", &json!({"gps": "48°51'30\"N 2°17'40\"E"})).unwrap();
    let (la, lo) = s.catalog.photo(id).unwrap().meta.gps.unwrap();
    assert!((la - 48.8583).abs() < 1e-3 && (lo - 2.2944).abs() < 1e-3, "{la} {lo}");
    let x = crate::sidecar::sidecar_packet(s.catalog.photo(id).unwrap(), &s.catalog);
    let back = lightcraft_meta::parse_xmp(&x).unwrap().metadata.gps.unwrap();
    assert!((back.latitude - la).abs() < 1e-5 && (back.longitude - lo).abs() < 1e-5);
    assert!(s.execute("photo.setMeta", &json!({"gps": "north pole-ish"})).is_err());
    s.execute("photo.setMeta", &json!({"gps": null})).unwrap();
    assert!(s.catalog.photo(id).unwrap().meta.gps.is_none());
}

/// DNG export compression: lossless, zip and none all decode; none is the biggest.
#[test]
fn dng_export_compression_choices() {
    let corpus = std::env::var_os("LIGHTCRAFT_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"))
        .join("raw/nef-nikon-d5100-uncompressed.nef");
    if !corpus.exists() {
        eprintln!("skip: {} absent", corpus.display());
        return;
    }
    let mut s = Session::new().with_fs();
    s.execute("library.import", &json!({"paths": [corpus.to_string_lossy()]})).unwrap();
    let id = s.active().unwrap();
    let mut sizes = Vec::new();
    for c in ["lossless", "deflate", "uncompressed"] {
        let o = crate::export::ExportOptions::from_json(&json!({"format": "dng", "dngCompression": c}));
        let mut out = Vec::new();
        let mut write = |_: &str, b: &[u8]| {
            out = b.to_vec();
            Ok(())
        };
        crate::export::export_batch(&mut s, &[id], &o, &crate::export::Destination { dir: "x".into(), exact: None }, &mut write, &|_| false).unwrap();
        let raw = lightcraft_raw::decode(&out).unwrap_or_else(|e| panic!("{c}: {e}"));
        assert!(raw.width > 1000, "{c}");
        sizes.push((c, out.len()));
    }
    assert!(sizes[2].1 > sizes[0].1 && sizes[2].1 > sizes[1].1, "uncompressed is the biggest: {sizes:?}");
}

fn write_jpeg(path: &Path, capture: Option<&str>) {
    let (w, h) = (32usize, 24usize);
    let data: Vec<[u8; 4]> = (0..w * h).map(|i| [(i % w * 7) as u8, (i / w * 9) as u8, 90, 255]).collect();
    let img = lightcraft_raster::Rgba8 { width: w, height: h, data };
    let exif = capture.map(|c| {
        lightcraft_meta::write_exif(&lightcraft_meta::Metadata { capture_time: lightcraft_meta::DateTime::parse_iso(c), ..Default::default() })
    });
    let meta = lightcraft_codecs::EncodeMeta { exif: exif.as_deref(), ..Default::default() };
    let bytes = lightcraft_codecs::encode_jpeg(&lightcraft_codecs::EncodeImage::rgba8(&img), 90, Default::default(), &meta).unwrap();
    std::fs::write(path, bytes).unwrap();
}

const DATED_SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:exif="http://ns.adobe.com/exif/1.0/"
      exif:DateTimeOriginal="2026-01-14T05:58:48" />
  </rdf:RDF>
</x:xmpmeta>"#;

#[test]
fn sidecar_capture_time_fills_in_when_the_file_has_none() {
    let src = temp_dir("sidecar-date");
    write_jpeg(&src.join("sample.jpg"), None);
    std::fs::write(src.join("sample.jpg.xmp"), DATED_SIDECAR).unwrap();
    // the embedded capture time wins over the sidecar's
    write_jpeg(&src.join("dated.jpg"), Some("2020-05-06T07:08:09"));
    std::fs::write(src.join("dated.jpg.xmp"), DATED_SIDECAR).unwrap();
    let lib = temp_dir("sidecar-date-lib");
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    // copy: the sidecar's date also files the copy
    let r = s.execute("library.import", &json!({"paths": [src.to_string_lossy()], "mode": "copy"})).unwrap();
    assert_eq!(r["sidecars"], 2, "{r}");
    let by_name = |s: &Session, n: &str| s.catalog.photos().find(|p| p.file_name == n).unwrap().clone();
    let sample = by_name(&s, "sample.jpg");
    assert_eq!(sample.captured.as_deref(), Some("2026-01-14T05:58:48"));
    let lightcraft_catalog::Source::File { path } = &sample.source else { panic!("not a file") };
    assert!(Path::new(path).starts_with(lib.join("Originals").join("2026").join("2026-01-14")), "{path}");
    assert_eq!(by_name(&s, "dated.jpg").captured.as_deref(), Some("2020-05-06T07:08:09"));

    // Read Metadata from File fills in a missing capture time too
    let id = sample.id;
    s.catalog.apply(lightcraft_catalog::Op::SetCaptured { id, captured: None }).unwrap();
    std::fs::copy(src.join("sample.jpg.xmp"), format!("{path}.xmp")).unwrap();
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("photo.readMetadataFromFile", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().captured.as_deref(), Some("2026-01-14T05:58:48"));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Copyright status, rights usage terms and copyright info URL: edited with `photo.setMeta` (one
/// undo step), copied / pasted, kept in metadata presets, written to the sidecar as XMP Rights
/// Management fields and read back on a fresh import.
#[test]
fn copyright_status_usage_terms_and_url_round_trip() {
    use lightcraft_catalog::CopyrightStatus;
    let src = temp_dir("rights-src");
    let lib = temp_dir("rights-lib");
    write_png(&src.join("a.png"), 1);
    write_png(&src.join("b.png"), 2);
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = |s: &Session, name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().id;
    let (a, b) = (id(&s, "a.png"), id(&s, "b.png"));
    let meta = |s: &Session, id: PhotoId| s.catalog.photo(id).unwrap().meta.clone();
    assert_eq!(meta(&s, a).copyright_status, CopyrightStatus::Unknown);
    s.execute("library.select", &json!({"ids": [a.0]})).unwrap();
    s.execute(
        "photo.setMeta",
        &json!({"copyright": "© 2026 A. Person", "copyrightStatus": "copyrighted", "usageTerms": "Editorial use only", "copyrightUrl": "https://example.com/rights"}),
    )
    .unwrap();
    let m = meta(&s, a);
    assert_eq!(
        (m.copyright_status, m.usage_terms.as_str(), m.copyright_url.as_str()),
        (CopyrightStatus::Copyrighted, "Editorial use only", "https://example.com/rights")
    );
    assert!(s.execute("photo.setMeta", &json!({"copyrightStatus": "maybe"})).is_err());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(meta(&s, a).copyright_status, CopyrightStatus::Unknown, "one undo step");
    s.execute("edit.redo", &json!({})).unwrap();
    // copy → paste onto b
    let clip = s.execute("photo.copyMetadata", &json!({})).unwrap();
    assert_eq!(clip["copyrightStatus"], "copyrighted");
    s.execute("photo.pasteMetadata", &json!({"ids": [b.0], "fields": ["copyrightStatus", "usageTerms"]})).unwrap();
    let mb = meta(&s, b);
    assert_eq!((mb.copyright_status, mb.usage_terms.as_str(), mb.copyright_url.as_str()), (CopyrightStatus::Copyrighted, "Editorial use only", ""));
    // a metadata preset from the active photo carries the copyright fields; applying it sets them
    s.execute("metadata.savePreset", &json!({"name": "Rights"})).unwrap();
    let presets = s.execute("metadata.presets", &json!({})).unwrap();
    assert_eq!(presets[0]["fields"]["copyrightStatus"], "copyrighted", "{presets}");
    assert_eq!(presets[0]["fields"]["copyrightUrl"], "https://example.com/rights");
    s.execute("photo.setMeta", &json!({"ids": [b.0], "copyrightStatus": "publicDomain", "usageTerms": ""})).unwrap();
    s.execute("metadata.applyPreset", &json!({"name": "Rights", "ids": [b.0]})).unwrap();
    let mb = meta(&s, b);
    assert_eq!((mb.copyright_status, mb.copyright_url.as_str()), (CopyrightStatus::Copyrighted, "https://example.com/rights"));
    assert!(s.execute("metadata.savePreset", &json!({"name": "Bad", "fields": {"copyrightStatus": "sort of"}})).is_err());
    // b becomes public domain; smart-album rule on the status
    s.execute("photo.setMeta", &json!({"ids": [b.0], "copyrightStatus": "public domain"})).unwrap();
    let r = s
        .execute(
            "album.createSmart",
            &json!({"name": "PD", "rules": {"ruleSet": {"rules": [{"field": "copyrightStatus", "op": "is", "value": "publicDomain"}]}}}),
        )
        .unwrap();
    assert_eq!(r["count"], 1, "{r}");
    // sidecars: XMP Rights Management fields, read back by a fresh library
    s.execute("photo.saveMetadataToFile", &json!({"ids": [a.0, b.0]})).unwrap();
    let xmp = std::fs::read_to_string(src.join("a.xmp")).unwrap();
    assert!(xmp.contains("<xmpRights:Marked>True</xmpRights:Marked>"), "{xmp}");
    assert!(xmp.contains("<xmpRights:WebStatement>https://example.com/rights</xmpRights:WebStatement>"), "{xmp}");
    assert!(std::fs::read_to_string(src.join("b.xmp")).unwrap().contains("<xmpRights:Marked>False</xmpRights:Marked>"));
    let expect_a = meta(&s, a);
    drop(s);
    std::fs::remove_dir_all(&lib).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let ma = meta(&s, id(&s, "a.png"));
    assert_eq!(
        (ma.copyright.as_str(), ma.copyright_status, ma.usage_terms.as_str(), ma.copyright_url.as_str()),
        (expect_a.copyright.as_str(), CopyrightStatus::Copyrighted, "Editorial use only", "https://example.com/rights")
    );
    assert_eq!(meta(&s, id(&s, "b.png")).copyright_status, CopyrightStatus::PublicDomain);
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// A sidecar another raw developer wrote: develop settings in attributes and elements, a
/// structured edit history, an unknown namespace.
const OTHER_APP_SIDECAR: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Other Toolkit">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:stEvt="http://ns.adobe.com/xap/1.0/sType/ResourceEvent#"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmlns:other="http://example.com/other/1.0/"
    xmp:Rating="2"
    crs:Version="99.0"
    crs:Exposure2012="+0.65"
    crs:HasSettings="True"
    other:Secret="keep me">
   <xmpMM:History>
    <rdf:Seq>
     <rdf:li stEvt:action="derived" stEvt:parameters="converted from image/x-raw to image/png"/>
    </rdf:Seq>
   </xmpMM:History>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>128, 140</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

/// Issue #92: saving merges into another app's sidecar instead of replacing it.
#[test]
fn saving_merges_into_another_apps_sidecar() {
    let src = temp_dir("merge");
    let lib = temp_dir("merge-lib");
    write_png(&src.join("shot.png"), 1);
    std::fs::write(src.join("shot.xmp"), OTHER_APP_SIDECAR).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.join("shot.png").to_string_lossy()]})).unwrap();
    let id = only(&s);
    s.execute("library.select", &json!({"ids": [id.0]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    s.execute("photo.setMeta", &json!({"title": "Harbour", "keywords": ["boats"]})).unwrap();
    s.execute("develop.set", &json!({"control": "effects.clarity", "value": 30})).unwrap();
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    assert_eq!(r["merged"][0], src.join("shot.xmp").to_string_lossy().as_ref(), "{r}");
    let out = std::fs::read_to_string(src.join("shot.xmp")).unwrap();
    // the other app's data, byte for byte
    for part in [
        "crs:Exposure2012=\"+0.65\"",
        "other:Secret=\"keep me\"",
        "   <xmpMM:History>\n    <rdf:Seq>\n     <rdf:li stEvt:action=\"derived\" stEvt:parameters=\"converted from image/x-raw to image/png\"/>\n    </rdf:Seq>\n   </xmpMM:History>",
        "   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>\n     <rdf:li>128, 140</rdf:li>",
    ] {
        assert!(out.contains(part), "lost {part:?}:\n{out}");
    }
    let d = lightcraft_meta::parse_xmp(&out).unwrap();
    assert_eq!(d.metadata.rating, Some(5), "ours replaces theirs");
    assert_eq!(d.metadata.title.as_deref(), Some("Harbour"));
    assert_eq!(d.properties.get("crs:Version"), Some(&vec!["99.0".to_string()]));
    let saved = s.catalog.photo(id).unwrap().develop.clone();
    // reading it back restores our settings (lc:settings wins over crs:)
    s.execute("develop.set", &json!({"control": "effects.clarity", "value": 0})).unwrap();
    s.execute("photo.readMetadataFromFile", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(id).unwrap().develop, saved);
    // saving again keeps one LightCraft description and the other app's data
    s.execute("photo.rate", &json!({"rating": 3})).unwrap();
    s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let again = std::fs::read_to_string(src.join("shot.xmp")).unwrap();
    assert_eq!(again.matches("<rdf:Description").count(), 2, "{again}");
    assert!(again.contains("other:Secret=\"keep me\"") && again.contains("crs:Exposure2012=\"+0.65\""));
    assert_eq!(lightcraft_meta::parse_xmp(&again).unwrap().metadata.rating, Some(3));

    // a sidecar that isn't XMP is kept as a backup before it is replaced
    std::fs::write(src.join("shot.xmp"), b"<not xmp").unwrap();
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let backup = r["backups"][0]["backup"].as_str().unwrap().to_string();
    assert!(backup.contains("shot.xmp.bak-"), "{r}");
    assert_eq!(std::fs::read(&backup).unwrap(), b"<not xmp");
    assert_eq!(lightcraft_meta::parse_xmp(&std::fs::read_to_string(src.join("shot.xmp")).unwrap()).unwrap().metadata.rating, Some(3));
    // … and another one never overwrites the first backup
    std::fs::write(src.join("shot.xmp"), b"<still not xmp").unwrap();
    let r = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    let second = r["backups"][0]["backup"].as_str().unwrap().to_string();
    assert_ne!(second, backup);
    assert_eq!(std::fs::read(&backup).unwrap(), b"<not xmp");
    assert_eq!(std::fs::read(&second).unwrap(), b"<still not xmp");
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}

/// Issue #92: two catalogued files with one stem (`IMG_1.jpg` + `IMG_1.png`) don't share a
/// Stem-named sidecar: the first by name keeps `IMG_1.xmp`, the other writes and reads
/// `IMG_1.png.xmp`.
#[test]
fn files_sharing_a_stem_get_their_own_sidecars() {
    let src = temp_dir("stem");
    let lib = temp_dir("stem-lib");
    write_png(&src.join("IMG_1.png"), 1);
    let img = lightcraft_raster::Rgba8::from_fn(40, 24, |x, y| [(x * 6) as u8, (y * 9) as u8, 7, 255]);
    let jpg =
        crate::export::encode_image(&img, &crate::export::ExportOptions { format: crate::export::ExportFormat::Jpeg, ..Default::default() }).unwrap();
    std::fs::write(src.join("IMG_1.jpg"), jpg).unwrap();
    let mut s = Session::new().with_fs();
    s.open_library(&lib, false).unwrap();
    s.execute("library.import", &json!({"paths": [src.to_string_lossy()]})).unwrap();
    let id = |s: &Session, name: &str| s.catalog.photos().find(|p| p.file_name == name).unwrap().id;
    let (j, p) = (id(&s, "IMG_1.jpg"), id(&s, "IMG_1.png"));
    assert_eq!(s.sidecar_naming(j), crate::sidecar::SidecarNaming::Stem);
    assert_eq!(s.sidecar_naming(p), crate::sidecar::SidecarNaming::Full);
    s.execute("photo.rate", &json!({"ids": [j.0], "rating": 2})).unwrap();
    s.execute("photo.rate", &json!({"ids": [p.0], "rating": 5})).unwrap();
    let r = s.execute("photo.saveMetadataToFile", &json!({"ids": [j.0, p.0]})).unwrap();
    assert_eq!(r["written"].as_array().unwrap().len(), 2, "{r}");
    let rating = |f: &str| lightcraft_meta::parse_xmp(&std::fs::read_to_string(src.join(f)).unwrap()).unwrap().metadata.rating;
    assert_eq!(rating("IMG_1.xmp"), Some(2));
    assert_eq!(rating("IMG_1.png.xmp"), Some(5));
    // each reads its own back
    s.execute("photo.rate", &json!({"ids": [j.0, p.0], "rating": 0})).unwrap();
    s.execute("photo.readMetadataFromFile", &json!({"ids": [j.0, p.0]})).unwrap();
    assert_eq!((s.catalog.photo(j).unwrap().rating, s.catalog.photo(p).unwrap().rating), (2, 5));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&lib);
}
