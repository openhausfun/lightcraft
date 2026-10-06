//! Interchange: camera-raw-settings (`crs:`) develop fields from XMP → our develop settings.
//!
//! Other raw developers store their edits as `crs:` properties in XMP sidecars, in DNG-embedded
//! XMP and in XMP preset files. This module maps the commonly used, documented-by-observation
//! fields to a *partial* [`DevelopSettings`](lightcraft_develop::DevelopSettings) JSON object
//! (only the fields present in the packet), which is then merged like a preset. Our pipeline is
//! not theirs, so the result is a best-effort approximation of the look, not a pixel match.
//! The full table is in `docs/xmp-interop.md`.
//!
//! Implemented from the public XMP specification (ISO 16684-1) and black-box observation of what
//! each field means; no third-party code or preset content was used.

use std::collections::BTreeMap;

use lightcraft_develop::{MIXER_BANDS, Preset};
use serde_json::{Map, Value, json};

/// Properties keyed `prefix:name` (as produced by [`lightcraft_meta::parse_xmp`]).
pub type Props = BTreeMap<String, Vec<String>>;

/// Band names as they appear in `crs:` field names, in our mixer order.
const CRS_BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// `crs:` fields that describe the packet rather than an adjustment.
const NON_ADJUSTMENT: &[&str] = &[
    "crs:Version",
    "crs:ProcessVersion",
    "crs:Name",
    "crs:ShortName",
    "crs:Group",
    "crs:UUID",
    "crs:PresetType",
    "crs:SupportsAmount",
    "crs:SupportsColor",
    "crs:SupportsMonochrome",
    "crs:SupportsHighDynamicRange",
    "crs:SupportsNormalDynamicRange",
    "crs:SupportsSceneReferred",
    "crs:SupportsOutputReferred",
    "crs:CameraModelRestriction",
    "crs:Copyright",
    "crs:ContactInfo",
    "crs:HasSettings",
    "crs:AlreadyApplied",
    "crs:RawFileName",
    "crs:HasCrop",
];

/// True if the packet carries any `crs:` adjustment (not just bookkeeping fields).
pub fn has_adjustments(props: &Props) -> bool {
    if props.get("crs:AlreadyApplied").and_then(|v| v.first()).is_some_and(|s| s.eq_ignore_ascii_case("true")) {
        // The pixels already contain these settings (e.g. an exported/rendered file).
        return false;
    }
    props.keys().any(|k| k.starts_with("crs:") && !NON_ADJUSTMENT.contains(&k.split('/').next().unwrap_or(k)))
}

thread_local! {
    /// Keys read by [`to_partial`] while [`to_partial_report`] runs.
    static READ: std::cell::RefCell<Option<std::collections::BTreeSet<String>>> = const { std::cell::RefCell::new(None) };
}

fn note(k: &str) {
    READ.with(|r| {
        if let Some(s) = r.borrow_mut().as_mut() {
            s.insert(k.to_string());
        }
    });
}

fn first<'a>(props: &'a Props, k: &str) -> Option<&'a str> {
    note(k);
    props.get(k).and_then(|v| v.first()).map(|s| s.trim()).filter(|s| !s.is_empty())
}

fn num(props: &Props, k: &str) -> Option<f64> {
    let s = first(props, k)?;
    let s = s.strip_prefix('+').unwrap_or(s);
    if let Some((n, d)) = s.split_once('/') {
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        return (d != 0.0).then_some(n / d).filter(|v| v.is_finite());
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn boolean(props: &Props, k: &str) -> Option<bool> {
    first(props, k).map(|s| s.eq_ignore_ascii_case("true") || s == "1")
}

/// Insert `v` at the dotted `path` of a JSON object, creating intermediate objects.
fn put(out: &mut Value, path: &str, v: Value) {
    let mut cur = out;
    let mut parts = path.split('.').peekable();
    while let Some(p) = parts.next() {
        let Value::Object(m) = cur else { return };
        if parts.peek().is_none() {
            m.insert(p.to_string(), v);
            return;
        }
        cur = m.entry(p.to_string()).or_insert_with(|| Value::Object(Map::new()));
    }
}

/// Relative temperature (−100..100, used for rendered files) → Kelvin, matching the UI's scale
/// (a mired shift around 6500 K).
pub fn rel_to_kelvin(r: f64) -> f64 {
    1e6 / (1e6 / 6500.0 - r.clamp(-100.0, 100.0) * 0.8)
}

/// Parse a point curve stored as an `rdf:Seq` of `"x, y"` strings in 0..255.
fn curve(props: &Props, k: &str) -> Option<Value> {
    note(k);
    let items = props.get(k)?;
    let mut pts = Vec::new();
    for it in items {
        let (x, y) = it.split_once(',')?;
        let (x, y) = (x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?);
        pts.push(json!({"x": (x / 255.0).clamp(0.0, 1.0), "y": (y / 255.0).clamp(0.0, 1.0)}));
    }
    if pts.len() < 2 {
        return None;
    }
    // A straight 0,0 → 255,255 line is the identity: store it as "no curve".
    let identity = pts.len() == 2 && items.iter().all(|it| it.split(',').map(|v| v.trim()).collect::<Vec<_>>().windows(2).all(|w| w[0] == w[1]));
    Some(if identity { json!([]) } else { Value::Array(pts) })
}

/// Map the `crs:` fields of an XMP packet to a partial develop-settings JSON object.
///
/// `raw`: the target is a raw file (absolute Kelvin white balance) — `Some(false)` prefers the
/// relative `Incremental*` white balance used for rendered files, `None` (presets) takes whichever
/// is present, absolute first.
pub fn to_partial(props: &Props, raw: Option<bool>) -> Value {
    let mut out = Value::Object(Map::new());
    let o = &mut out;
    let n = |o: &mut Value, crs: &str, path: &str| {
        if let Some(v) = num(props, &format!("crs:{crs}")) {
            put(o, path, json!(v));
        }
    };
    // ---- Light (process version 2012+ names)
    n(o, "Exposure2012", "light.exposure");
    n(o, "Contrast2012", "light.contrast");
    n(o, "Highlights2012", "light.highlights");
    n(o, "Shadows2012", "light.shadows");
    n(o, "Whites2012", "light.whites");
    n(o, "Blacks2012", "light.blacks");
    // older process versions (most `.lrtemplate` presets): approximate the 2012 sliders from the
    // earlier ones, relative to their defaults (contrast 25, blacks 5, brightness 50)
    let has = |o: &Value, path: &str| path.split('.').try_fold(o, |v, k| v.get(k)).is_some();
    // presets written by newer versions keep the old fields at their defaults: ignore them there
    let pv2012 = props.keys().any(|k| k.ends_with("2012"));
    let old = |o: &mut Value, crs: &str, path: &str, f: &dyn Fn(f64) -> f64| {
        let v = num(props, &format!("crs:{crs}"));
        if !pv2012
            && !has(o, path)
            && let Some(v) = v
        {
            put(o, path, json!(f(v).clamp(-100.0, 100.0)));
        }
    };
    old(o, "Exposure", "light.exposure", &|v| v);
    old(o, "Contrast", "light.contrast", &|v| v - 25.0);
    old(o, "HighlightRecovery", "light.highlights", &|v| -v);
    old(o, "FillLight", "light.shadows", &|v| v);
    old(o, "Shadows", "light.blacks", &|v| -(v - 5.0) * 4.0);
    old(o, "Brightness", "light.whites", &|v| (v - 50.0) * 0.6);
    // ---- Presence
    n(o, "Texture", "effects.texture");
    n(o, "Clarity2012", "effects.clarity");
    old(o, "Clarity", "effects.clarity", &|v| v);
    n(o, "Dehaze", "effects.dehaze");
    n(o, "Vibrance", "color.vibrance");
    n(o, "Saturation", "color.saturation");

    // ---- White balance
    let mode = first(props, "crs:WhiteBalance").map(|m| match m.to_ascii_lowercase().replace(' ', "").as_str() {
        "asshot" => "asShot",
        "auto" => "auto",
        "daylight" => "daylight",
        "cloudy" => "cloudy",
        "shade" => "shade",
        "tungsten" => "tungsten",
        "fluorescent" => "fluorescent",
        "flash" => "flash",
        _ => "custom",
    });
    let abs = num(props, "crs:Temperature").map(|t| (t, num(props, "crs:Tint")));
    let rel = num(props, "crs:IncrementalTemperature")
        .or(num(props, "crs:IncrementalTint").map(|_| 0.0))
        .map(|t| (rel_to_kelvin(t), num(props, "crs:IncrementalTint")));
    let wb = match raw {
        Some(true) => abs,
        Some(false) => rel.or(abs),
        None => abs.or(rel),
    };
    match (mode, wb) {
        (Some(m), _) if m != "custom" => put(o, "wb.mode", json!(m)),
        (m, Some((t, tint))) => {
            put(o, "wb.mode", json!(m.unwrap_or("custom")));
            put(o, "wb.temp", json!(t));
            if let Some(tint) = tint {
                put(o, "wb.tint", json!(tint));
            }
        }
        (Some(m), None) => put(o, "wb.mode", json!(m)),
        (None, None) => {}
    }

    // ---- Treatment + B&W mix
    if let Some(bw) = boolean(props, "crs:ConvertToGrayscale") {
        put(o, "treatment", json!(if bw { "bw" } else { "color" }));
    }
    for (band, crs) in MIXER_BANDS.iter().zip(CRS_BANDS) {
        n(o, &format!("HueAdjustment{crs}"), &format!("mixer.{band}.hue"));
        n(o, &format!("SaturationAdjustment{crs}"), &format!("mixer.{band}.sat"));
        n(o, &format!("LuminanceAdjustment{crs}"), &format!("mixer.{band}.lum"));
        n(o, &format!("GrayMixer{crs}"), &format!("bw_mix.{band}"));
    }

    // ---- Tone curve: parametric regions + point curves
    n(o, "ParametricShadows", "curve.shadows");
    n(o, "ParametricDarks", "curve.darks");
    n(o, "ParametricLights", "curve.lights");
    n(o, "ParametricHighlights", "curve.highlights");
    n(o, "ParametricShadowSplit", "curve.split_shadows");
    n(o, "ParametricMidtoneSplit", "curve.split_mid");
    n(o, "ParametricHighlightSplit", "curve.split_highlights");
    n(o, "CurveRefineSaturation", "curve.refine_saturation");
    if let Some(c) = curve(props, "crs:ToneCurve").filter(|c| c.as_array().is_some_and(|a| !a.is_empty())) {
        // the older single curve, used when no 2012 curve is present
        if !props.contains_key("crs:ToneCurvePV2012") {
            put(o, "curve.master", c);
        }
    }
    for (crs, ch) in
        [("ToneCurvePV2012", "master"), ("ToneCurvePV2012Red", "red"), ("ToneCurvePV2012Green", "green"), ("ToneCurvePV2012Blue", "blue")]
    {
        if let Some(c) = curve(props, &format!("crs:{crs}")) {
            put(o, &format!("curve.{ch}"), c);
        }
    }

    // ---- Color grading (split toning fields are shared with the older split-toning panel)
    n(o, "SplitToningShadowHue", "grading.shadows.hue");
    n(o, "SplitToningShadowSaturation", "grading.shadows.sat");
    n(o, "ColorGradeShadowLum", "grading.shadows.lum");
    n(o, "SplitToningHighlightHue", "grading.highlights.hue");
    n(o, "SplitToningHighlightSaturation", "grading.highlights.sat");
    n(o, "ColorGradeHighlightLum", "grading.highlights.lum");
    n(o, "ColorGradeMidtoneHue", "grading.midtones.hue");
    n(o, "ColorGradeMidtoneSat", "grading.midtones.sat");
    n(o, "ColorGradeMidtoneLum", "grading.midtones.lum");
    n(o, "ColorGradeGlobalHue", "grading.global.hue");
    n(o, "ColorGradeGlobalSat", "grading.global.sat");
    n(o, "ColorGradeGlobalLum", "grading.global.lum");
    n(o, "ColorGradeBlending", "grading.blending");
    n(o, "SplitToningBalance", "grading.balance");

    // ---- Detail
    n(o, "Sharpness", "detail.sharpen_amount");
    n(o, "SharpenRadius", "detail.sharpen_radius");
    n(o, "SharpenDetail", "detail.sharpen_detail");
    n(o, "SharpenEdgeMasking", "detail.sharpen_masking");
    n(o, "LuminanceSmoothing", "detail.nr_luminance");
    n(o, "LuminanceNoiseReductionDetail", "detail.nr_detail");
    n(o, "LuminanceNoiseReductionContrast", "detail.nr_contrast");
    n(o, "ColorNoiseReduction", "detail.nr_color");
    n(o, "ColorNoiseReductionDetail", "detail.nr_color_detail");
    n(o, "ColorNoiseReductionSmoothness", "detail.nr_color_smoothness");

    // ---- Effects: post-crop vignette + grain
    n(o, "PostCropVignetteAmount", "vignette.amount");
    n(o, "PostCropVignetteMidpoint", "vignette.midpoint");
    n(o, "PostCropVignetteRoundness", "vignette.roundness");
    n(o, "PostCropVignetteFeather", "vignette.feather");
    n(o, "PostCropVignetteHighlightContrast", "vignette.highlights");
    if let Some(st) = num(props, "crs:PostCropVignetteStyle") {
        let style = match st as i64 {
            2 => "colorPriority",
            3 => "paintOverlay",
            _ => "highlightPriority",
        };
        put(o, "vignette.style", json!(style));
    }
    n(o, "GrainAmount", "grain.amount");
    n(o, "GrainSize", "grain.size");
    n(o, "GrainFrequency", "grain.roughness");

    // ---- Optics (manual corrections; lens profiles are ours, only the switch carries over)
    if let Some(b) = boolean(props, "crs:LensProfileEnable") {
        put(o, "optics.lens_profile", json!(b));
    }
    if let Some(b) = boolean(props, "crs:AutoLateralCA") {
        put(o, "optics.remove_ca", json!(b));
    }
    n(o, "LensManualDistortionAmount", "optics.distortion");
    n(o, "VignetteAmount", "optics.vignetting");
    n(o, "VignetteMidpoint", "optics.vignetting_midpoint");
    n(o, "DefringePurpleAmount", "optics.defringe_purple_amount");
    n(o, "DefringePurpleHueLo", "optics.defringe_purple_hue_lo");
    n(o, "DefringePurpleHueHi", "optics.defringe_purple_hue_hi");
    n(o, "DefringeGreenAmount", "optics.defringe_green_amount");
    n(o, "DefringeGreenHueLo", "optics.defringe_green_hue_lo");
    n(o, "DefringeGreenHueHi", "optics.defringe_green_hue_hi");

    // ---- Calibration
    n(o, "ShadowTint", "calibration.shadows_tint");
    n(o, "RedHue", "calibration.red_hue");
    n(o, "RedSaturation", "calibration.red_sat");
    n(o, "GreenHue", "calibration.green_hue");
    n(o, "GreenSaturation", "calibration.green_sat");
    n(o, "BlueHue", "calibration.blue_hue");
    n(o, "BlueSaturation", "calibration.blue_sat");

    // ---- Geometry
    n(o, "PerspectiveVertical", "geometry.vertical");
    n(o, "PerspectiveHorizontal", "geometry.horizontal");
    n(o, "PerspectiveRotate", "geometry.rotate");
    n(o, "PerspectiveScale", "geometry.scale");
    n(o, "PerspectiveAspect", "geometry.aspect");
    n(o, "PerspectiveX", "geometry.offset_x");
    n(o, "PerspectiveY", "geometry.offset_y");
    if let Some(u) = num(props, "crs:PerspectiveUpright") {
        let mode = match u as i64 {
            1 => "auto",
            2 => "level",
            3 => "vertical",
            4 => "full",
            5 => "guided",
            _ => "off",
        };
        put(o, "geometry.upright", json!(mode));
    }

    // ---- Crop (normalized edges of the unrotated image + straighten angle)
    match boolean(props, "crs:HasCrop") {
        Some(true) => {
            let e = |k: &str, d: f64| num(props, &format!("crs:Crop{k}")).unwrap_or(d).clamp(0.0, 1.0);
            let (l, t, r, b) = (e("Left", 0.0), e("Top", 0.0), e("Right", 1.0), e("Bottom", 1.0));
            if r > l && b > t {
                put(
                    o,
                    "crop.geometry",
                    json!({"rect": {"x0": l, "y0": t, "x1": r, "y1": b}, "angle": num(props, "crs:CropAngle").unwrap_or(0.0).clamp(-45.0, 45.0)}),
                );
            }
        }
        Some(false) => put(o, "crop.geometry", json!({"rect": {"x0": 0.0, "y0": 0.0, "x1": 1.0, "y1": 1.0}, "angle": 0.0})),
        None => {}
    }
    out
}

/// [`to_partial`], plus the adjustments in the packet it could not carry over (field names
/// without the `crs:` prefix, grouped: `CameraProfile`, `Look`, `MaskGroupBasedCorrections`…).
///
/// With `values` (the packet's structured properties), local corrections become masks too
/// ([`crate::crs_masks`]; radial masks fitted to `aspect` = width / height); components that
/// can't be carried over are reported as `Mask: <kind>`.
pub fn to_partial_report(props: &Props, values: Option<&crate::crs_masks::Values>, raw: Option<bool>, aspect: f64) -> (Value, Vec<String>) {
    READ.with(|r| *r.borrow_mut() = Some(Default::default()));
    let mut out = to_partial(props, raw);
    let mut read = READ.with(|r| r.borrow_mut().take()).unwrap_or_default();
    let mut mask_skips = Vec::new();
    if let Some(values) = values {
        let (masks, skipped) = crate::crs_masks::masks(values, aspect);
        if !masks.is_empty() {
            put(&mut out, "masks", Value::Array(masks));
        }
        if values.keys().any(|k| crate::crs_masks::CONTAINERS.contains(&k.as_str())) {
            read.extend(crate::crs_masks::CONTAINERS.iter().map(|c| c.to_string()));
            mask_skips = skipped.into_iter().map(|k| format!("Mask: {k}")).collect();
        }
    }
    // fields that only switch a panel on/off or name things: not adjustments by themselves
    let quiet = |k: &str| k.starts_with("Enable") || k.starts_with("ToneCurveName") || k == "AutoTone" || k == "AutoGrayscaleMix";
    let mut unmapped: Vec<String> = props
        .keys()
        .filter(|k| k.starts_with("crs:"))
        .map(|k| k.split('/').next().unwrap_or(k))
        .filter(|k| !NON_ADJUSTMENT.contains(k) && !read.contains(*k))
        .map(|k| k.trim_start_matches("crs:").to_string())
        .filter(|k| !quiet(k))
        .collect();
    unmapped.dedup();
    unmapped.extend(mask_skips);
    (out, unmapped)
}

/// Read an XMP preset (`crs:` fields + `crs:Name` / `crs:Group`) into one of our presets.
/// Returns `None` when the packet has no adjustments we understand.
pub fn preset_from_xmp(xmp: &str, fallback_name: &str) -> Option<Preset> {
    let d = lightcraft_meta::parse_xmp(xmp).ok()?;
    let props = &d.properties;
    let settings = to_partial(props, None);
    if settings.as_object().is_none_or(Map::is_empty) {
        return None;
    }
    let name = first(props, "crs:Name").unwrap_or(fallback_name).to_string();
    let group = first(props, "crs:Group").unwrap_or("Imported Presets").to_string();
    let uuid = first(props, "crs:UUID").map(|u| u.to_ascii_lowercase());
    let id = format!("user.xmp.{}", uuid.unwrap_or_else(|| crate::presets::slug(&format!("{group}-{name}"))));
    Some(Preset { id, name, group, settings, favorite: false, builtin: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::{DevelopSettings, Upright, VignetteStyle, WbMode, apply_partial};

    /// A hand-written sidecar in attribute form (as many tools write it).
    const SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    crs:Version="99.0" crs:ProcessVersion="11.0" crs:WhiteBalance="Custom" crs:Temperature="5150" crs:Tint="+12"
    crs:Exposure2012="+0.65" crs:Contrast2012="-14" crs:Highlights2012="-58" crs:Shadows2012="+41"
    crs:Whites2012="+9" crs:Blacks2012="-17" crs:Texture="+6" crs:Clarity2012="+11" crs:Dehaze="+4"
    crs:Vibrance="+19" crs:Saturation="-3" crs:HueAdjustmentOrange="-7" crs:SaturationAdjustmentBlue="-22"
    crs:LuminanceAdjustmentAqua="+13" crs:ParametricShadows="+5" crs:ParametricHighlights="-8"
    crs:ParametricMidtoneSplit="55" crs:CurveRefineSaturation="40" crs:SplitToningShadowHue="210" crs:SplitToningShadowSaturation="14"
    crs:SplitToningHighlightHue="40" crs:SplitToningHighlightSaturation="9" crs:SplitToningBalance="+20"
    crs:ColorGradeMidtoneHue="120" crs:ColorGradeMidtoneSat="6" crs:ColorGradeBlending="70"
    crs:Sharpness="55" crs:SharpenRadius="+1.2" crs:SharpenDetail="30" crs:SharpenEdgeMasking="12"
    crs:LuminanceSmoothing="18" crs:ColorNoiseReduction="25" crs:PostCropVignetteAmount="-21"
    crs:PostCropVignetteStyle="2" crs:GrainAmount="15" crs:GrainSize="30" crs:GrainFrequency="60"
    crs:PerspectiveUpright="2" crs:PerspectiveVertical="-10" crs:AutoLateralCA="1" crs:ConvertToGrayscale="False"
    crs:ShadowTint="-7" crs:RedHue="+12" crs:RedSaturation="+20" crs:GreenHue="-5" crs:GreenSaturation="+3"
    crs:BlueHue="-15" crs:BlueSaturation="+25"
    crs:HasCrop="True" crs:CropTop="0.1" crs:CropLeft="0.05" crs:CropBottom="0.9" crs:CropRight="0.8" crs:CropAngle="1.5">
   <crs:ToneCurvePV2012>
    <rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>64, 52</rdf:li><rdf:li>192, 205</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq>
   </crs:ToneCurvePV2012>
   <crs:ToneCurvePV2012Red>
    <rdf:Seq><rdf:li>0, 0</rdf:li><rdf:li>255, 255</rdf:li></rdf:Seq>
   </crs:ToneCurvePV2012Red>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    fn props(x: &str) -> Props {
        lightcraft_meta::parse_xmp(x).unwrap().properties
    }

    #[test]
    fn maps_common_fields() {
        let p = props(SIDECAR);
        assert!(has_adjustments(&p));
        let partial = to_partial(&p, Some(true));
        let s = apply_partial(&DevelopSettings::for_raw(5000.0, 0.0), &partial, 1.0);
        assert_eq!(s.wb.mode, WbMode::Custom);
        assert_eq!((s.wb.temp, s.wb.tint), (5150.0, 12.0));
        assert_eq!(s.light.exposure, 0.65);
        assert_eq!((s.light.contrast, s.light.highlights, s.light.shadows, s.light.whites, s.light.blacks), (-14.0, -58.0, 41.0, 9.0, -17.0));
        assert_eq!((s.effects.texture, s.effects.clarity, s.effects.dehaze), (6.0, 11.0, 4.0));
        assert_eq!((s.color.vibrance, s.color.saturation), (19.0, -3.0));
        assert_eq!(s.mixer.orange.hue, -7.0);
        assert_eq!(s.mixer.blue.sat, -22.0);
        assert_eq!(s.mixer.aqua.lum, 13.0);
        assert_eq!((s.curve.shadows, s.curve.highlights, s.curve.split_mid), (5.0, -8.0, 55.0));
        assert_eq!(s.curve.master.len(), 4);
        assert_eq!(s.curve.refine_saturation, 40.0);
        assert!((s.curve.master[1].x - 64.0 / 255.0).abs() < 1e-9 && (s.curve.master[1].y - 52.0 / 255.0).abs() < 1e-9);
        assert!(s.curve.red.is_empty(), "identity channel curve maps to no curve");
        assert_eq!((s.grading.shadows.hue, s.grading.shadows.sat), (210.0, 14.0));
        assert_eq!((s.grading.highlights.hue, s.grading.highlights.sat), (40.0, 9.0));
        assert_eq!((s.grading.midtones.hue, s.grading.midtones.sat), (120.0, 6.0));
        assert_eq!((s.grading.balance, s.grading.blending), (20.0, 70.0));
        assert_eq!((s.detail.sharpen_amount, s.detail.sharpen_radius, s.detail.sharpen_detail, s.detail.sharpen_masking), (55.0, 1.2, 30.0, 12.0));
        assert_eq!((s.detail.nr_luminance, s.detail.nr_color), (18.0, 25.0));
        assert_eq!(s.vignette.amount, -21.0);
        assert_eq!(s.vignette.style, VignetteStyle::ColorPriority);
        assert_eq!((s.grain.amount, s.grain.size, s.grain.roughness), (15.0, 30.0, 60.0));
        assert_eq!(s.geometry.upright, Upright::Level);
        assert_eq!(s.geometry.vertical, -10.0);
        assert!(s.optics.remove_ca);
        assert_eq!(s.treatment, lightcraft_develop::Treatment::Color);
        let cal = s.calibration;
        assert_eq!(
            (cal.shadows_tint, cal.red_hue, cal.red_sat, cal.green_hue, cal.green_sat, cal.blue_hue, cal.blue_sat),
            (-7.0, 12.0, 20.0, -5.0, 3.0, -15.0, 25.0)
        );
        let c = s.crop.geometry;
        assert_eq!((c.rect.x0, c.rect.y0, c.rect.x1, c.rect.y1, c.angle), (0.05, 0.1, 0.8, 0.9, 1.5));
    }

    #[test]
    fn only_present_fields_are_in_the_partial() {
        let x = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            xmlns:c="http://ns.adobe.com/camera-raw-settings/1.0/" c:Vibrance="+30" c:GrainAmount="10"/></rdf:RDF>"#;
        let partial = to_partial(&props(x), None);
        assert_eq!(partial, json!({"color": {"vibrance": 30.0}, "grain": {"amount": 10.0}}));
    }

    #[test]
    fn element_form_bw_and_incremental_wb() {
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/">
            <crs:ConvertToGrayscale>True</crs:ConvertToGrayscale>
            <crs:GrayMixerBlue>-35</crs:GrayMixerBlue>
            <crs:IncrementalTemperature>25</crs:IncrementalTemperature>
            <crs:IncrementalTint>-6</crs:IncrementalTint>
            <crs:Temperature>4800</crs:Temperature>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let p = props(x);
        let rendered = apply_partial(&DevelopSettings::default(), &to_partial(&p, Some(false)), 1.0);
        assert_eq!(rendered.treatment, lightcraft_develop::Treatment::Bw);
        assert_eq!(rendered.bw_mix.blue, -35.0);
        assert!((rendered.wb.temp - rel_to_kelvin(25.0)).abs() < 1e-6 && rendered.wb.temp > 6500.0);
        assert_eq!(rendered.wb.tint, -6.0);
        let raw = apply_partial(&DevelopSettings::default(), &to_partial(&p, Some(true)), 1.0);
        assert_eq!(raw.wb.temp, 4800.0);
    }

    #[test]
    fn named_wb_and_already_applied() {
        let x = r#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
             crs:WhiteBalance="Daylight" crs:Temperature="5500" crs:Tint="10" crs:AlreadyApplied="True"/>"#;
        let p = props(x);
        assert_eq!(to_partial(&p, Some(true)), json!({"wb": {"mode": "daylight"}}));
        assert!(!has_adjustments(&p));
        let bookkeeping = r#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
             xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Version="1" crs:HasSettings="True"/>"#;
        assert!(!has_adjustments(&props(bookkeeping)));
    }

    #[test]
    fn xmp_preset_with_name_and_group() {
        // Written for this test: a minimal preset-shaped packet with localized name/group.
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
          <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
             crs:PresetType="Normal" crs:UUID="0123ABCD" crs:Contrast2012="+25" crs:SupportsAmount="True">
            <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Punchy Test</rdf:li></rdf:Alt></crs:Name>
            <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">My Looks</rdf:li></rdf:Alt></crs:Group>
          </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let p = preset_from_xmp(x, "file").unwrap();
        assert_eq!((p.name.as_str(), p.group.as_str(), p.id.as_str()), ("Punchy Test", "My Looks", "user.xmp.0123abcd"));
        assert_eq!(p.settings, json!({"light": {"contrast": 25.0}}));
        assert!(!p.builtin);
        assert!(preset_from_xmp("<x/>", "f").is_none());
    }
}
