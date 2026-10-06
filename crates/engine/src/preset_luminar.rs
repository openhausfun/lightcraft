//! Luminar looks: `.lmp` files and `.mplumpack` collections.
//!
//! An `.lmp` look is an Apple XML property list: a dict whose `AdjustmentLayers` array holds layers
//! (`Identifier`, `Enabled`, `Amount` = opacity 0..1, `BlendModeIdentifier`, optional nested
//! `Sublayers.AdjustmentLayers`), each with `Effects` (`Identifier` such as
//! `MIPLDevelopCommonEffectID`, and `Parameters`: name → `{Value: number}`; parameters at their
//! default are left out). Newer looks may be a macOS bundle directory `Name.lmp/Contents/preset.lmp`
//! (plus `Info.plist` and LUT resources). An `.mplumpack` is a zip of `.lmp` files with a
//! `PresetsInfo.plist` (`GroupName`) and icons.
//!
//! The look's sliders with a clear counterpart become `crs:` fields (mapped by [`crate::crs`]);
//! everything else (AI tools, Orton, glow, LUTs…) is reported as unmapped. Sliders are on a
//! −100..100 scale; Exposure is read as ±100 ↔ ±4 EV. Layer opacity scales additive sliders.
//!
//! Written from black-box inspection of freely distributed looks and the public property-list
//! format; no Skylum code or look content was used.

use std::collections::BTreeMap;
use std::path::Path;

use crate::crs::Props;
use crate::preset_import::{Imported, build, group_from_dir, read_zip};

// ----------------------------------------------------------------------------- property lists

/// A value of an XML property list.
#[derive(Clone, Debug, PartialEq)]
pub enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    Str(String),
    Num(f64),
    Bool(bool),
    /// `<data>` (base64; not decoded)
    Data,
}

impl Plist {
    pub fn get(&self, key: &str) -> Option<&Plist> {
        match self {
            Plist::Dict(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Plist::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn num(&self) -> Option<f64> {
        match self {
            Plist::Num(n) => Some(*n),
            _ => None,
        }
    }
    pub fn array(&self) -> &[Plist] {
        match self {
            Plist::Array(a) => a,
            _ => &[],
        }
    }
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
    /// Nesting of the element being read (bounded: a hostile file must not overflow the stack).
    depth: usize,
}

/// Deepest `<dict>` / `<array>` nesting read (real looks nest a handful of levels).
const MAX_DEPTH: usize = 64;

fn unescape(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl Parser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("property list: {what} at byte {}", self.i))
    }
    /// Skip whitespace, the XML declaration, the doctype and comments.
    fn skip(&mut self) {
        loop {
            let r = &self.s[self.i..];
            let t = r.trim_start();
            self.i += r.len() - t.len();
            let close = if t.starts_with("<?") {
                "?>"
            } else if t.starts_with("<!--") {
                "-->"
            } else if t.starts_with("<!") {
                ">"
            } else {
                return;
            };
            match t.find(close) {
                Some(e) => self.i += e + close.len(),
                None => {
                    self.i = self.s.len();
                    return;
                }
            }
        }
    }
    /// The next tag: (name, closing, self-closing).
    fn tag(&mut self) -> Result<(String, bool, bool), String> {
        self.skip();
        let r = &self.s[self.i..];
        if !r.starts_with('<') {
            return self.err("expected a tag");
        }
        let Some(end) = r.find('>') else { return self.err("unterminated tag") };
        let inner = &r[1..end];
        self.i += end + 1;
        let closing = inner.starts_with('/');
        let selfc = inner.ends_with('/');
        let name = inner.trim_start_matches('/').trim_end_matches('/').split_whitespace().next().unwrap_or("").to_string();
        Ok((name, closing, selfc))
    }
    fn peek_close(&mut self, name: &str) -> bool {
        self.skip();
        let r = &self.s[self.i..];
        let t = format!("</{name}>");
        if r.starts_with(&t) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn text(&mut self, name: &str) -> Result<String, String> {
        let r = &self.s[self.i..];
        let t = format!("</{name}>");
        let Some(end) = r.find(&t) else { return self.err(&format!("missing {t}")) };
        self.i += end + t.len();
        Ok(unescape(&r[..end]))
    }
    fn value(&mut self) -> Result<Plist, String> {
        if self.depth >= MAX_DEPTH {
            return self.err("nested too deeply");
        }
        self.depth += 1;
        let v = self.value_inner();
        self.depth -= 1;
        v
    }
    fn value_inner(&mut self) -> Result<Plist, String> {
        let (name, closing, selfc) = self.tag()?;
        if closing {
            return self.err(&format!("unexpected </{name}>"));
        }
        let leaf = |p: &mut Self, f: &dyn Fn(String) -> Result<Plist, String>| if selfc { f(String::new()) } else { p.text(&name).and_then(f) };
        match name.as_str() {
            "plist" => {
                if selfc {
                    return Ok(Plist::Dict(Vec::new()));
                }
                let v = self.value()?;
                self.peek_close("plist");
                Ok(v)
            }
            "dict" => {
                let mut m = Vec::new();
                if !selfc {
                    while !self.peek_close("dict") {
                        let (k, c, sc) = self.tag()?;
                        if k != "key" || c {
                            return self.err("expected <key>");
                        }
                        let key = if sc { String::new() } else { self.text("key")? };
                        m.push((key, self.value()?));
                    }
                }
                Ok(Plist::Dict(m))
            }
            "array" => {
                let mut a = Vec::new();
                if !selfc {
                    while !self.peek_close("array") {
                        a.push(self.value()?);
                    }
                }
                Ok(Plist::Array(a))
            }
            "string" | "date" => leaf(self, &|t| Ok(Plist::Str(t))),
            "real" | "integer" => {
                leaf(self, &|t| t.trim().parse::<f64>().map(Plist::Num).map_err(|_| format!("property list: bad number {:?}", t.trim())))
            }
            "data" => leaf(self, &|_| Ok(Plist::Data)),
            "true" | "false" => {
                if !selfc {
                    self.text(&name)?;
                }
                Ok(Plist::Bool(name == "true"))
            }
            _ => self.err(&format!("unknown element <{name}>")),
        }
    }
}

/// Parse an XML property list.
pub fn parse_plist(bytes: &[u8]) -> Result<Plist, String> {
    if bytes.starts_with(b"bplist") {
        return Err("binary property lists are not supported".into());
    }
    let text = String::from_utf8_lossy(bytes);
    let mut p = Parser { s: text.trim_start_matches('\u{feff}'), i: 0, depth: 0 };
    p.value()
}

// ------------------------------------------------------------------------------------ mapping

/// Luminar's colour bands (`h`/`s`/`l` + band in the HSL tool) — the same eight as `crs:`.
const BANDS: [&str; 8] = ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];

/// Effects whose parameters carry the basic tone / colour sliders under their own names.
const BASIC: &[&str] = &[
    "MIPLDevelopCommonEffectID",
    "MIPLExposureEffect",
    "MIPLContrastEffect",
    "MIPLHighlightsEffect",
    "MIPLBlackWhiteEffect",
    "MIPLWhiteBalanceEffect",
    "MIPLSaturationEffect",
    "MIPLVibranceEffect",
    "MIPLClarityEffect",
];

/// How one Luminar slider maps: the `crs:` field, a scale, whether it adds up across layers
/// (and fades with layer opacity), and its clamp range.
struct Target {
    crs: String,
    scale: f64,
    additive: bool,
    range: (f64, f64),
}

fn target(effect: &str, param: &str, value: f64) -> Option<Target> {
    let t = |crs: &str, scale: f64, additive: bool, range: (f64, f64)| Some(Target { crs: crs.to_string(), scale, additive, range });
    let pm = (-100.0, 100.0);
    if BASIC.contains(&effect) {
        return match param {
            "Exposure" => t("Exposure2012", 4.0 / 100.0, true, (-5.0, 5.0)),
            "Contrast" => t("Contrast2012", 1.0, true, pm),
            "Highlights" => t("Highlights2012", 1.0, true, pm),
            "Shadows" => t("Shadows2012", 1.0, true, pm),
            "Whites" => t("Whites2012", 1.0, true, pm),
            "Blacks" => t("Blacks2012", 1.0, true, pm),
            // an absolute colour temperature (raw develop) vs. a relative shift
            "Temperature" if value.abs() > 1000.0 => t("Temperature", 1.0, false, (2000.0, 50000.0)),
            "Temperature" => t("IncrementalTemperature", 1.0, true, pm),
            "Tint" => t("IncrementalTint", 1.0, true, pm),
            "Saturation" => t("Saturation", 1.0, true, pm),
            "Vibrance" => t("Vibrance", 1.0, true, pm),
            "Clarity" => t("Clarity2012", 1.0, true, pm),
            "Dehaze" => t("Dehaze", 1.0, true, pm),
            _ => None,
        };
    }
    match (effect, param) {
        ("MIPLAIStructureEffect" | "MIPLStructureEffect", "Amount") => t("Clarity2012", 1.0, true, pm),
        ("MIPLDehazeEffect", "Amount") => t("Dehaze", 1.0, true, pm),
        ("MIPLVignetteEffect", "Amount") => t("PostCropVignetteAmount", 1.0, true, pm),
        ("MIPLVignetteEffect", "Vignette Size" | "Size") => t("PostCropVignetteMidpoint", 1.0, false, (0.0, 100.0)),
        ("MIPLGrainNewEffect" | "MIPLGrainEffect", "Amount") => t("GrainAmount", 1.0, true, (0.0, 100.0)),
        ("MIPLChannelsEffect" | "MIPLHSLEffect", _) => {
            let (kind, band) = param.split_at(param.char_indices().nth(1).map_or(param.len(), |(i, _)| i));
            let field = match kind {
                "h" => "HueAdjustment",
                "s" => "SaturationAdjustment",
                "l" => "LuminanceAdjustment",
                _ => return None,
            };
            BANDS.contains(&band).then(|| Target { crs: format!("{field}{band}"), scale: 1.0, additive: true, range: pm })
        }
        _ => None,
    }
}

/// A point curve stored as `OptionalData`: a leading value, then x, y pairs in 0..1 with x
/// increasing. Anything else isn't read.
fn curve_points(param: &Plist) -> Option<Vec<(f64, f64)>> {
    let data: Vec<f64> = param.get("OptionalData")?.array().iter().map(Plist::num).collect::<Option<_>>()?;
    if data.len() < 5 || data.len().is_multiple_of(2) {
        return None;
    }
    let pts: Vec<(f64, f64)> = data[1..].chunks(2).map(|c| (c[0], c[1])).collect();
    let ok = pts.iter().all(|&(x, y)| (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) && pts.windows(2).all(|w| w[1].0 > w[0].0);
    ok.then_some(pts)
}

/// `MIPLSmartContrastEffect` → `SmartContrast`.
fn short(effect: &str) -> &str {
    let e = effect.strip_prefix("MIPL").unwrap_or(effect);
    ["_v1_Effect", "EffectID", "Effect"].iter().find_map(|s| e.strip_suffix(s)).unwrap_or(e)
}

#[derive(Default)]
struct Acc {
    sums: BTreeMap<String, (f64, (f64, f64))>,
    curves: BTreeMap<String, Vec<(f64, f64)>>,
    unmapped: Vec<String>,
}

impl Acc {
    fn skip(&mut self, what: String) {
        if !self.unmapped.contains(&what) {
            self.unmapped.push(what);
        }
    }

    fn layers(&mut self, layers: &[Plist], weight: f64) {
        for layer in layers {
            if matches!(layer.get("Enabled"), Some(Plist::Bool(false))) {
                continue;
            }
            let opacity = weight * layer.get("Amount").and_then(Plist::num).unwrap_or(1.0).clamp(0.0, 1.0);
            if opacity <= 0.0 {
                continue;
            }
            let blend = layer.get("BlendModeIdentifier").and_then(Plist::str).unwrap_or("Normal");
            // a layer with a mask applies locally: not a global look
            let empty = |v: &Plist| {
                matches!(v, Plist::Bool(false)) || matches!(v, Plist::Dict(d) if d.is_empty()) || matches!(v, Plist::Array(a) if a.is_empty())
            };
            let masked = matches!(layer, Plist::Dict(m) if m.iter().any(|(k, v)| k.to_ascii_lowercase().contains("mask") && !empty(v)));
            let literal = blend.eq_ignore_ascii_case("normal") && !masked;
            let mut reported = false;
            for effect in layer.get("Effects").map(Plist::array).unwrap_or_default() {
                let id = effect.get("Identifier").and_then(Plist::str).unwrap_or("?");
                let Some(Plist::Dict(params)) = effect.get("Parameters") else { continue };
                for (name, pv) in params {
                    let value = pv.get("Value").and_then(Plist::num).or(pv.num());
                    if literal && id == "MIPLCurveEffect" {
                        let crs = match name.as_str() {
                            "RGB" => Some("ToneCurvePV2012"),
                            "Red" => Some("ToneCurvePV2012Red"),
                            "Green" => Some("ToneCurvePV2012Green"),
                            "Blue" => Some("ToneCurvePV2012Blue"),
                            _ => None,
                        };
                        if let (Some(crs), Some(pts)) = (crs, curve_points(pv))
                            && !self.curves.contains_key(crs)
                        {
                            // a curve at partial opacity: the points fade toward the identity
                            self.curves.insert(crs.to_string(), pts.into_iter().map(|(x, y)| (x, x + (y - x) * opacity)).collect());
                            continue;
                        }
                        if pv.get("OptionalData").is_some() {
                            self.skip(format!("{}.{name}", short(id)));
                        }
                        continue;
                    }
                    let Some(v) = value.filter(|v| *v != 0.0) else { continue };
                    match target(id, name, v).filter(|_| literal) {
                        Some(t) => {
                            let x = v * t.scale * if t.additive { opacity } else { 1.0 };
                            let e = self.sums.entry(t.crs).or_insert((0.0, t.range));
                            e.0 = if t.additive { e.0 + x } else { x };
                        }
                        None => {
                            reported = true;
                            self.skip(format!("{}.{name}", short(id)));
                        }
                    }
                }
            }
            if !literal && reported {
                let lid = layer.get("Identifier").and_then(Plist::str).unwrap_or("layer");
                self.skip(if masked { format!("{lid}: mask") } else { format!("{lid}: blend mode {blend}") });
            }
            if let Some(sub) = layer.get("Sublayers").and_then(|s| s.get("AdjustmentLayers")) {
                self.layers(sub.array(), opacity);
            }
        }
    }
}

/// The name of a look stored at `path`: its file name, or the bundle's for
/// `Name.lmp/Contents/preset.lmp`.
fn look_name(path: &str) -> String {
    let p = Path::new(path);
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_string());
    let bundle = p.parent().filter(|d| d.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Contents"))).and_then(Path::parent);
    match bundle {
        Some(b) if b.extension().is_some_and(|e| e.eq_ignore_ascii_case("lmp")) => stem(b),
        _ => stem(p),
    }
    .unwrap_or_else(|| "Look".into())
}

/// Read one Luminar look (`.lmp`) into a preset in `group` (default "Imported Presets").
pub fn read_lmp(path: &str, bytes: &[u8], group: Option<String>) -> Result<Imported, String> {
    let root = parse_plist(bytes)?;
    let layers = root.get("AdjustmentLayers").ok_or("not a Luminar look (no adjustment layers)")?;
    let mut acc = Acc::default();
    acc.layers(layers.array(), 1.0);
    let mut props = Props::new();
    for (k, (v, (lo, hi))) in &acc.sums {
        props.insert(format!("crs:{k}"), vec![format!("{}", v.clamp(*lo, *hi))]);
    }
    for (k, pts) in &acc.curves {
        props.insert(format!("crs:{k}"), pts.iter().map(|(x, y)| format!("{}, {}", x * 255.0, y * 255.0)).collect());
    }
    if let Some(u) = root.get("uuid").and_then(Plist::str).filter(|u| !u.trim().is_empty()) {
        props.insert("crs:UUID".into(), vec![u.trim().to_string()]);
    }
    let name = root.get("Name").and_then(Plist::str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| look_name(path));
    match build(&props, &Default::default(), name, group, false) {
        Some(mut i) => {
            i.preset.id = i.preset.id.replacen("user.xmp.", "user.lmp.", 1);
            i.unmapped.extend(acc.unmapped);
            Ok(i)
        }
        None if acc.unmapped.is_empty() => Err("this look has no adjustments".into()),
        None => Err(format!("none of this look's settings have a counterpart here ({})", acc.unmapped.join(", "))),
    }
}

/// Read a Luminar looks collection (`.mplumpack`, a zip): every look, grouped by the pack's
/// `GroupName` (else the pack's file name).
pub fn read_mplumpack(path: &str, bytes: &[u8]) -> Result<Vec<Imported>, String> {
    let entries = read_zip(bytes)?;
    let info = entries
        .iter()
        .find(|(n, _)| n.rsplit('/').next().is_some_and(|b| b.eq_ignore_ascii_case("PresetsInfo.plist")))
        .and_then(|(_, b)| parse_plist(b).ok())
        .and_then(|p| p.get("GroupName").and_then(Plist::str).map(str::trim).filter(|g| !g.is_empty()).map(str::to_string));
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string());
    let pack = info.or_else(|| stem.as_deref().and_then(group_from_dir)).unwrap_or_else(|| "Luminar Looks".into());
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (inner, data) in &entries {
        if !Path::new(inner).extension().is_some_and(|e| e.eq_ignore_ascii_case("lmp")) {
            continue;
        }
        match read_lmp(inner, data, Some(pack.clone())) {
            Ok(i) => out.push(i),
            Err(e) => errors.push(format!("{inner}: {e}")),
        }
    }
    if out.is_empty() {
        return Err(if errors.is_empty() { "no looks in this collection".into() } else { errors.join("; ") });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset_import::read_presets;
    use serde_json::json;

    /// One effect's parameter as Luminar writes it.
    fn param(name: &str, v: f64) -> String {
        format!("<key>{name}</key><dict><key>OptionalDataType</key><integer>0</integer><key>Value</key><real>{v}</real></dict>")
    }

    fn layer(id: &str, amount: f64, blend: &str, effects: &[(&str, String)], sub: &str) -> String {
        let fx: String = effects
            .iter()
            .map(|(e, p)| format!("<dict><key>Identifier</key><string>{e}</string><key>Parameters</key><dict>{p}</dict></dict>"))
            .collect();
        let sub =
            if sub.is_empty() { String::new() } else { format!("<key>Sublayers</key><dict><key>AdjustmentLayers</key><array>{sub}</array></dict>") };
        format!(
            "<dict><key>Amount</key><real>{amount}</real><key>BlendModeIdentifier</key><string>{blend}</string><key>Effects</key><array>{fx}</array>\
             <key>Enabled</key><true/><key>Identifier</key><string>{id}</string>{sub}</dict>"
        )
    }

    fn look(layers: &str, extra: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict><key>AdjustmentLayers</key><array>{layers}</array>{extra}<key>group_identifier</key><string>Custom</string></dict></plist>"
        )
    }

    /// A newer-style look (develop sliders nested in sublayers) written for this test.
    fn modern() -> String {
        let curve = "<key>RGB</key><dict><key>OptionalData</key><array><real>0.5</real><real>0</real><real>0.1</real><real>0.5</real><real>0.5</real><real>1</real><real>0.9</real></array><key>OptionalDataType</key><integer>1</integer><key>Value</key><real>50</real></dict>";
        let sub = [
            layer(
                "DevelopAdjustmentSubLayer",
                1.0,
                "Normal",
                &[(
                    "MIPLDevelopCommonEffectID",
                    param("Exposure", 25.0) + &param("Contrast", 12.0) + &param("Highlights", -30.0) + &param("Temperature", 15.0),
                )],
                &layer("CurveLayer", 1.0, "Normal", &[("MIPLCurveEffect", curve.to_string())], ""),
            ),
            layer("Disabled", 1.0, "Normal", &[("MIPLDevelopCommonEffectID", param("Shadows", 50.0))], "").replace("<true/>", "<false/>"),
        ]
        .concat();
        let layers = [
            layer("DevelopAdjustmentLayer", 1.0, "Normal", &[], &sub),
            layer("AIStructureEffect", 1.0, "Normal", &[("MIPLAIStructureEffect", param("Amount", 20.0) + &param("Boost", 10.0))], ""),
            layer(
                "Half",
                0.5,
                "Normal",
                &[("MIPLVibranceEffect", param("Vibrance", 30.0)), ("MIPLSaturationEffect", param("Saturation", -10.0))],
                "",
            ),
            layer("Hsl", 1.0, "Normal", &[("MIPLChannelsEffect", param("hOrange", -8.0) + &param("sBlue", 12.0) + &param("lGreen", 4.0))], ""),
            layer("Vignette", 1.0, "Normal", &[("MIPLVignetteEffect", param("Amount", -20.0) + &param("Vignette Size", 40.0))], ""),
            layer("Orton", 1.0, "Normal", &[("MIPLOrtonFilterEffect", param("Amount", 15.0))], ""),
            layer("Screened", 1.0, "Screen", &[("MIPLContrastEffect", param("Contrast", 40.0))], ""),
            layer("Untouched", 1.0, "Normal", &[("MIPLSkyEnhancerEffect", String::new())], ""),
        ]
        .concat();
        look(&layers, "<key>uuid</key><string>AB12-CD34</string><key>kMPPresetIdentifierKey</key><string>x 2.lmp</string>")
    }

    #[test]
    fn plists_parse() {
        let p = parse_plist(b"<?xml version=\"1.0\"?><!-- c --><plist><dict><key>a&amp;b</key><string>x &lt;y&#x41;</string><key>n</key><integer>-3</integer><key>e</key><dict/><key>t</key><true/><key>d</key><data>AAEC</data><key>s</key><string/></dict></plist>").unwrap();
        assert_eq!(p.get("a&b").and_then(Plist::str), Some("x <yA"));
        assert_eq!(p.get("n").and_then(Plist::num), Some(-3.0));
        assert_eq!(p.get("e"), Some(&Plist::Dict(vec![])));
        assert_eq!(p.get("t"), Some(&Plist::Bool(true)));
        assert_eq!(p.get("d"), Some(&Plist::Data));
        assert_eq!(p.get("s").and_then(Plist::str), Some(""));
        assert!(parse_plist(b"<plist><dict><key>a</key>").is_err());
        assert!(parse_plist(b"bplist00\x01\x02").is_err());
    }

    #[test]
    fn hostile_nesting_is_an_error_not_a_crash() {
        let deep = format!("<plist>{}{}</plist>", "<array>".repeat(200_000), "</array>".repeat(200_000));
        assert!(parse_plist(deep.as_bytes()).unwrap_err().contains("nested too deeply"));
        let ok = format!("<plist>{}<true/>{}</plist>", "<array>".repeat(20), "</array>".repeat(20));
        assert!(parse_plist(ok.as_bytes()).is_ok());
    }

    #[test]
    fn look_sliders_map_and_the_rest_is_reported() {
        let i = read_presets("/x/Soft Contrast.lmp", modern().as_bytes(), None).unwrap().remove(0);
        let p = &i.preset;
        assert_eq!((p.name.as_str(), p.group.as_str(), p.id.as_str()), ("Soft Contrast", "Imported Presets", "user.lmp.ab12-cd34"));
        let s = &p.settings;
        assert_eq!(s["light"]["exposure"], json!(1.0), "±100 ↔ ±4 EV");
        assert_eq!(s["light"]["contrast"], json!(12.0));
        assert_eq!(s["light"]["highlights"], json!(-30.0));
        assert!(s["light"].get("shadows").is_none(), "disabled layers are ignored");
        assert_eq!(s["effects"]["clarity"], json!(20.0), "Structure → clarity");
        assert_eq!(s["color"]["vibrance"], json!(15.0), "layer opacity scales the slider");
        assert_eq!(s["color"]["saturation"], json!(-5.0));
        assert_eq!(s["mixer"]["orange"]["hue"], json!(-8.0));
        assert_eq!(s["mixer"]["blue"]["sat"], json!(12.0));
        assert_eq!(s["mixer"]["green"]["lum"], json!(4.0));
        assert_eq!(s["vignette"]["amount"], json!(-20.0));
        assert_eq!(s["vignette"]["midpoint"], json!(40.0));
        assert!(s["wb"]["temp"].as_f64().unwrap() > 6500.0, "a warm shift: {}", s["wb"]);
        let c = s["curve"]["master"].as_array().unwrap();
        assert_eq!(c.len(), 3);
        assert!((c[0]["y"].as_f64().unwrap() - 0.1).abs() < 1e-9 && (c[2]["y"].as_f64().unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(i.unmapped, ["AIStructure.Boost", "OrtonFilter.Amount", "Contrast.Contrast", "Screened: blend mode Screen"]);
        assert!(s["light"]["contrast"] == json!(12.0), "the screen-blended layer isn't folded in");
        lightcraft_develop::DevelopSettings::default().merged(s).expect("valid develop settings");
    }

    #[test]
    fn classic_looks_and_bundles() {
        // the older flat layout: one layer per tool, the look's name inside
        let layers = [
            layer(
                "ToneLayer",
                1.0,
                "Normal",
                &[("MIPLExposureEffect", param("Exposure", 0.0)), ("MIPLHighlightsEffect", param("Shadows", 10.0))],
                "",
            ),
            layer("ClarityLayer", 1.0, "Normal", &[("MIPLClarityEffect", param("Clarity", 20.0))], ""),
            layer("Wb", 1.0, "Normal", &[("MIPLWhiteBalanceEffect", param("Temperature", -15.0) + &param("Tint", 10.0))], ""),
        ]
        .concat();
        let lmp = look(&layers, "<key>Name</key><string>Blue Hour</string>");
        let i = read_presets("whatever.lmp", lmp.as_bytes(), Some("Mine".into())).unwrap().remove(0);
        assert_eq!((i.preset.name.as_str(), i.preset.group.as_str()), ("Blue Hour", "Mine"));
        assert_eq!(i.preset.settings["light"], json!({"shadows": 10.0}));
        assert_eq!(i.preset.settings["effects"]["clarity"], json!(20.0));
        assert_eq!(i.preset.settings["wb"]["tint"], json!(10.0));
        assert!(i.preset.settings["wb"]["temp"].as_f64().unwrap() < 6500.0);
        assert!(i.unmapped.is_empty(), "{:?}", i.unmapped);
        // a bundle's inner file is named after the bundle
        let b = read_presets("/p/Vintage.lmp/Contents/preset.lmp", modern().as_bytes(), None).unwrap();
        assert_eq!(b[0].preset.name, "Vintage");
        // nothing we can carry over: an error that says what was there
        let only = look(&layer("Orton", 1.0, "Normal", &[("MIPLOrtonFilterEffect", param("Amount", 15.0))], ""), "");
        let e = read_presets("Glow.lmp", only.as_bytes(), None).unwrap_err();
        assert!(e.contains("OrtonFilter.Amount"), "{e}");
        assert!(read_presets("x.lmp", b"<plist><dict/></plist>", None).is_err());
    }

    #[test]
    fn packs_group_by_their_name() {
        use crate::preset_import::tests::zip;
        let a = look(&layer("C", 1.0, "Normal", &[("MIPLClarityEffect", param("Clarity", 20.0))], ""), "<key>Name</key><string>Pop 1</string>");
        let b = look(&layer("S", 1.0, "Normal", &[("MIPLSaturationEffect", param("Saturation", 25.0))], ""), "");
        let info = b"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>GroupName</key><string>Magic Light</string></dict></plist>";
        let pack = zip(
            &[
                ("Pop 1.lmp", a.as_bytes()),
                ("__MACOSX/._Pop 1.lmp", b"junk"),
                ("Pop 2.lmp", b.as_bytes()),
                ("icon@2x.png", b"\x89PNG"),
                ("PresetsInfo.plist", info),
            ],
            true,
        );
        let v = read_presets("/d/Magic Light-2.mplumpack", &pack, None).unwrap();
        let got: Vec<_> = v.iter().map(|i| (i.preset.name.as_str(), i.preset.group.as_str())).collect();
        assert_eq!(got, [("Pop 1", "Magic Light"), ("Pop 2", "Magic Light")]);
        // without PresetsInfo.plist the pack's file name is the group
        let v = read_presets("Seaside Looks.mplumpack", &zip(&[("Pop 2.lmp", b.as_bytes())], false), None).unwrap();
        assert_eq!(v[0].preset.group, "Seaside Looks");
        // a pack inside a downloaded zip, and bundle-style looks in a zip folder
        let outer = zip(&[("Archive/Magic Light-2.mplumpack", &pack), ("Archive/Magic Light.plist", b"<plist/>")], false);
        assert_eq!(read_presets("download.zip", &outer, None).unwrap().len(), 2);
        let bundles = zip(
            &[
                ("Wild Pack/Vintage.lmp/Contents/preset.lmp", modern().as_bytes()),
                ("Wild Pack/Vintage.lmp/Contents/Info.plist", b"<plist><dict/></plist>"),
                ("Wild Pack/Flat.lmp", a.as_bytes()),
            ],
            true,
        );
        let v = read_presets("wild.zip", &bundles, None).unwrap();
        let got: Vec<_> = v.iter().map(|i| (i.preset.name.as_str(), i.preset.group.as_str())).collect();
        assert_eq!(got, [("Vintage", "Wild Pack"), ("Pop 1", "Wild Pack")]);
        assert!(read_presets("empty.mplumpack", &zip(&[("icon.png", b"x")], false), None).is_err());
    }

    #[test]
    fn import_command_reads_looks_and_bundle_folders() {
        let dir = std::env::temp_dir().join(format!("lc-luminar-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Wild/Vintage.lmp/Contents/resources")).unwrap();
        std::fs::write(dir.join("Wild/Vintage.lmp/Contents/preset.lmp"), modern()).unwrap();
        std::fs::write(dir.join("Wild/Vintage.lmp/Contents/Info.plist"), "<plist><dict/></plist>").unwrap();
        std::fs::write(dir.join("Wild/Vintage.lmp/Contents/PkgInfo"), "LMP?????").unwrap();
        let mut s = crate::Session::with_demo();
        let r = s.execute("preset.import", &json!({"paths": [dir.join("Wild").to_string_lossy()]})).unwrap();
        let im = &r["imported"][0];
        assert_eq!((im["name"].as_str(), im["group"].as_str()), (Some("Vintage"), Some("Wild")), "{r}");
        assert!(im["unmapped"].as_array().unwrap().iter().any(|u| u == "OrtonFilter.Amount"));
        // a dropped bundle folder on its own
        let r = s.execute("preset.import", &json!({"paths": [dir.join("Wild/Vintage.lmp").to_string_lossy()], "dryRun": true})).unwrap();
        assert_eq!(r["imported"][0]["group"], json!("Imported Presets"), "{r}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
