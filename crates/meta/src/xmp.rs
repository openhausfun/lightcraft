//! XMP packets (ISO 16684-1 / Adobe XMP Specification Part 1–2): read and write.
//!
//! Reading builds a small namespace-resolved tree with `quick-xml`, then collects every property of every
//! `rdf:Description` (attribute form, element form, `rdf:Seq`/`rdf:Bag`/`rdf:Alt` arrays, `rdf:parseType="Resource"`
//! and nested-description structs, flattened as `parent/field`). Prefixes are canonicalised by namespace URI, so
//! a packet that binds `http://purl.org/dc/elements/1.1/` to `foo:` still yields `dc:title`.
//!
//! Writing emits the interchange subset in standard namespaces (dc, xmp, photoshop, exif, exifEX, tiff, lr) and
//! LightCraft's full develop settings as an opaque JSON string in `lc:settings`.

use crate::{DateTime, Flash, Gps, Metadata, Orientation, parse_number};
use quick_xml::escape::{escape, partial_escape};
use quick_xml::events::Event;
use std::collections::BTreeMap;

/// LightCraft's XMP namespace URI (prefix `lc`).
pub const LC_NS: &str = "http://ns.lightcraft.app/lc/1.0/";

const NAMESPACES: &[(&str, &str)] = &[
    ("x", "adobe:ns:meta/"),
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("xmp", "http://ns.adobe.com/xap/1.0/"),
    ("xmpRights", "http://ns.adobe.com/xap/1.0/rights/"),
    ("photoshop", "http://ns.adobe.com/photoshop/1.0/"),
    ("exif", "http://ns.adobe.com/exif/1.0/"),
    ("exifEX", "http://cipa.jp/exif/1.0/"),
    ("tiff", "http://ns.adobe.com/tiff/1.0/"),
    ("aux", "http://ns.adobe.com/exif/1.0/aux/"),
    ("lr", "http://ns.adobe.com/lightroom/1.0/"),
    ("Iptc4xmpCore", "http://iptc.org/std/Iptc4xmpCore/1.0/xmlns/"),
    ("lc", LC_NS),
    // Read-only: develop settings written by other raw developers (interchange; see docs/xmp-interop.md).
    ("crs", CRS_NS),
];

/// The camera-raw-settings namespace URI (prefix `crs`), read for interchange only.
pub const CRS_NS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";

/// XMP read failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmpError {
    Xml(String),
    /// Nesting / size limits exceeded.
    TooComplex,
}

impl std::fmt::Display for XmpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XmpError::Xml(e) => write!(f, "XMP is not well-formed XML: {e}"),
            XmpError::TooComplex => write!(f, "XMP packet too deeply nested or too large"),
        }
    }
}

impl std::error::Error for XmpError {}

/// Result of [`parse_xmp`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmpData {
    pub metadata: Metadata,
    /// The opaque `lc:settings` JSON string, if present.
    pub lc_settings: Option<String>,
    /// Every property found, keyed `prefix:name` (struct fields as `prefix:name/prefix:field`); arrays keep
    /// item order, language alternatives put `x-default` first.
    pub properties: BTreeMap<String, Vec<String>>,
    /// Every top-level property with its full structure (arrays of structs such as local
    /// corrections, which [`XmpData::properties`] flattens).
    pub values: BTreeMap<String, XmpValue>,
}

/// A structured XMP value.
#[derive(Clone, Debug, PartialEq)]
pub enum XmpValue {
    Text(String),
    Array(Vec<XmpValue>),
    Struct(BTreeMap<String, XmpValue>),
}

impl XmpValue {
    /// A struct field (`prefix:name`).
    pub fn field(&self, k: &str) -> Option<&XmpValue> {
        match self {
            XmpValue::Struct(m) => m.get(k),
            _ => None,
        }
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            XmpValue::Text(s) => Some(s.trim()),
            XmpValue::Array(a) => a.first().and_then(XmpValue::text),
            XmpValue::Struct(_) => None,
        }
    }
    /// Array items (a lone value is a one-item array).
    pub fn items(&self) -> &[XmpValue] {
        match self {
            XmpValue::Array(a) => a,
            other => std::slice::from_ref(other),
        }
    }
}

fn node_struct(n: &Node) -> XmpValue {
    let mut m = BTreeMap::new();
    for (k, v) in &n.attrs {
        if !is_rdf_meta_attr(k) {
            m.insert(k.clone(), XmpValue::Text(v.clone()));
        }
    }
    for c in &n.children {
        m.insert(c.name.clone(), node_value(c));
    }
    XmpValue::Struct(m)
}

fn node_value(n: &Node) -> XmpValue {
    if let Some(arr) = n.children.iter().find(|c| matches!(c.name.as_str(), "rdf:Seq" | "rdf:Bag" | "rdf:Alt")) {
        return XmpValue::Array(arr.children.iter().filter(|li| li.name == "rdf:li").map(node_value).collect());
    }
    if let Some(d) = n.children.iter().find(|c| c.name == "rdf:Description") {
        return node_struct(d);
    }
    if !n.children.is_empty() || n.attrs.iter().any(|(k, _)| !is_rdf_meta_attr(k)) {
        return node_struct(n);
    }
    XmpValue::Text(n.attr("rdf:resource").map(str::to_string).unwrap_or_else(|| n.text.trim().to_string()))
}

#[derive(Debug, Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

impl Node {
    fn attr(&self, n: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str())
    }
}

const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 200_000;

/// `prefix:local` with the prefix canonicalised by namespace URI (see [`NAMESPACES`]).
pub(crate) fn canonical_name(qname: &str, scopes: &[Vec<(String, String)>]) -> String {
    canonical(qname, scopes)
}

fn canonical(qname: &str, scopes: &[Vec<(String, String)>]) -> String {
    let (prefix, local) = qname.split_once(':').unwrap_or(("", qname));
    if prefix == "xml" || prefix == "xmlns" {
        return qname.to_string();
    }
    let uri = scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|(p, _)| p == prefix).map(|(_, u)| u.as_str());
    match uri {
        Some(u) => match NAMESPACES.iter().find(|(_, nu)| *nu == u) {
            Some((p, _)) => format!("{p}:{local}"),
            None if prefix.is_empty() => local.to_string(),
            None => qname.to_string(),
        },
        None => qname.to_string(),
    }
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let n = name.strip_prefix('#')?;
            let v = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(v)?
        }
    })
}

/// Unescape attribute values (predefined + numeric references; unknown references kept verbatim).
fn unescape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest.find(';').and_then(|j| entity(&rest[1..j]).map(|c| (c, j))) {
            Some((c, j)) => {
                out.push(c);
                rest = &rest[j + 1..];
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

fn parse_tree(s: &str) -> Result<Node, XmpError> {
    let mut reader = quick_xml::Reader::from_str(s);
    reader.config_mut().check_end_names = false;
    let mut stack: Vec<Node> = vec![Node { name: "#root".into(), ..Default::default() }];
    let mut scopes: Vec<Vec<(String, String)>> = vec![vec![("xml".into(), "http://www.w3.org/XML/1998/namespace".into())]];
    let mut count = 0usize;
    loop {
        let ev = reader.read_event().map_err(|e| XmpError::Xml(e.to_string()))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                count += 1;
                if count > MAX_NODES || stack.len() > MAX_DEPTH {
                    return Err(XmpError::TooComplex);
                }
                let mut scope = Vec::new();
                let mut raw_attrs = Vec::new();
                for a in e.attributes().with_checks(false).flatten() {
                    let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    let v = unescape_attr(&String::from_utf8_lossy(&a.value));
                    if k == "xmlns" {
                        scope.push((String::new(), v));
                    } else if let Some(p) = k.strip_prefix("xmlns:") {
                        scope.push((p.to_string(), v));
                    } else {
                        raw_attrs.push((k, v));
                    }
                }
                scopes.push(scope);
                let name = canonical(&String::from_utf8_lossy(e.name().as_ref()), &scopes);
                let attrs = raw_attrs.into_iter().map(|(k, v)| (canonical(&k, &scopes), v)).collect();
                let node = Node { name, attrs, ..Default::default() };
                if matches!(ev, Event::Empty(_)) {
                    scopes.pop();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(node);
                    }
                } else {
                    stack.push(node);
                }
            }
            Event::End(_) => {
                if stack.len() > 1 {
                    let n = stack.pop().expect("len > 1");
                    scopes.pop();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(n);
                    }
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t.decode().map_err(|e| XmpError::Xml(e.to_string()))?);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                let name = String::from_utf8_lossy(&r).into_owned();
                if let Some(top) = stack.last_mut() {
                    match entity(&name) {
                        Some(c) => top.text.push(c),
                        None => {
                            top.text.push('&');
                            top.text.push_str(&name);
                            top.text.push(';');
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // close unterminated elements leniently
    while stack.len() > 1 {
        let n = stack.pop().expect("len > 1");
        stack.last_mut().expect("root").children.push(n);
    }
    Ok(stack.pop().expect("root"))
}

fn is_rdf_meta_attr(k: &str) -> bool {
    matches!(k, "rdf:about" | "rdf:ID" | "rdf:nodeID" | "xml:lang" | "rdf:parseType" | "rdf:resource" | "rdf:datatype") || k.starts_with("xmlns")
}

fn collect_description(d: &Node, prefix: &str, props: &mut BTreeMap<String, Vec<String>>) {
    for (k, v) in &d.attrs {
        if !is_rdf_meta_attr(k) {
            props.entry(format!("{prefix}{k}")).or_default().push(v.clone());
        }
    }
    for c in &d.children {
        collect_property(c, prefix, props);
    }
}

fn collect_property(p: &Node, prefix: &str, props: &mut BTreeMap<String, Vec<String>>) {
    let key = format!("{prefix}{}", p.name);
    if let Some(arr) = p.children.iter().find(|c| matches!(c.name.as_str(), "rdf:Seq" | "rdf:Bag" | "rdf:Alt")) {
        let mut items: Vec<(bool, String)> = arr
            .children
            .iter()
            .filter(|li| li.name == "rdf:li")
            .map(|li| {
                (li.attr("xml:lang") == Some("x-default"), li.attr("rdf:resource").map(str::to_string).unwrap_or_else(|| li.text.trim().to_string()))
            })
            .collect();
        if arr.name == "rdf:Alt" {
            items.sort_by_key(|(default, _)| !*default);
        }
        props.entry(key).or_default().extend(items.into_iter().map(|(_, v)| v));
        return;
    }
    let struct_prefix = format!("{key}/");
    if p.attr("rdf:parseType") == Some("Resource") {
        for c in &p.children {
            collect_property(c, &struct_prefix, props);
        }
        return;
    }
    if let Some(d) = p.children.iter().find(|c| c.name == "rdf:Description") {
        collect_description(d, &struct_prefix, props);
        return;
    }
    let field_attrs: Vec<&(String, String)> = p.attrs.iter().filter(|(k, _)| !is_rdf_meta_attr(k)).collect();
    if !field_attrs.is_empty() && p.children.is_empty() && p.text.trim().is_empty() {
        for (k, v) in field_attrs {
            props.entry(format!("{struct_prefix}{k}")).or_default().push(v.clone());
        }
        return;
    }
    let v = p.attr("rdf:resource").map(str::to_string).unwrap_or_else(|| p.text.trim().to_string());
    props.entry(key).or_default().push(v);
}

fn find_descriptions<'a>(n: &'a Node, out: &mut Vec<&'a Node>) {
    if n.name == "rdf:Description" {
        out.push(n);
        return;
    }
    for c in &n.children {
        find_descriptions(c, out);
    }
}

/// XMP GPS coordinate `DDD,MM,SSk` or `DDD,MM.mmk` (k = N/S/E/W) → signed degrees.
fn parse_gps_coord(s: &str) -> Option<f64> {
    let s = s.trim();
    let dir = s.chars().last()?;
    let body = &s[..s.len() - dir.len_utf8()];
    let parts: Vec<f64> = body.split(',').map(|p| p.trim().parse::<f64>().ok()).collect::<Option<_>>()?;
    let v = match parts.as_slice() {
        [d] => *d,
        [d, m] => d + m / 60.0,
        [d, m, sec] => d + m / 60.0 + sec / 3600.0,
        _ => return None,
    };
    let v = match dir.to_ascii_uppercase() {
        'N' | 'E' => v,
        'S' | 'W' => -v,
        _ => return None,
    };
    v.is_finite().then_some(v)
}

fn fmt_gps_coord(v: f64, pos: char, neg: char) -> String {
    let a = v.abs();
    let d = a.trunc();
    let m = (a - d) * 60.0;
    format!("{},{:.6}{}", d as i64, m, if v < 0.0 { neg } else { pos })
}

/// Parse an XMP packet (with or without the `<?xpacket?>` wrapper / `x:xmpmeta` element).
pub fn parse_xmp(s: &str) -> Result<XmpData, XmpError> {
    let s = s.trim_start_matches('\u{feff}');
    let tree = parse_tree(s)?;
    let mut descs = Vec::new();
    find_descriptions(&tree, &mut descs);
    let mut props = BTreeMap::new();
    let mut values = BTreeMap::new();
    for d in descs {
        collect_description(d, "", &mut props);
        if let XmpValue::Struct(m) = node_struct(d) {
            values.extend(m);
        }
    }
    let first = |k: &str| props.get(k).and_then(|v| v.first()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let num = |k: &str| first(k).and_then(|s| parse_number(&s));
    let mut m = Metadata {
        make: first("tiff:Make"),
        model: first("tiff:Model"),
        serial_number: first("exifEX:BodySerialNumber").or_else(|| first("aux:SerialNumber")),
        software: first("xmp:CreatorTool"),
        lens_make: first("exifEX:LensMake"),
        lens_model: first("exifEX:LensModel").or_else(|| first("aux:Lens")),
        lens_serial_number: first("exifEX:LensSerialNumber").or_else(|| first("aux:LensSerialNumber")),
        exposure_time: num("exif:ExposureTime").filter(|v| *v > 0.0),
        f_number: num("exif:FNumber").filter(|v| *v > 0.0),
        focal_length: num("exif:FocalLength").filter(|v| *v > 0.0),
        focal_length_35mm: num("exif:FocalLengthIn35mmFilm").filter(|v| *v > 0.0),
        exposure_bias: num("exif:ExposureBiasValue"),
        exposure_program: num("exif:ExposureProgram").map(|v| v as u16),
        metering_mode: num("exif:MeteringMode").map(|v| v as u16),
        white_balance: num("exif:WhiteBalance").map(|v| v as u16),
        orientation: num("tiff:Orientation").map(|v| v as u16).filter(|v| (1..=8).contains(v)).map(Orientation::from_exif),
        width: num("tiff:ImageWidth").or_else(|| num("exif:PixelXDimension")).map(|v| v as u32),
        height: num("tiff:ImageLength").or_else(|| num("exif:PixelYDimension")).map(|v| v as u32),
        title: first("dc:title"),
        caption: first("dc:description"),
        alt_text: first("Iptc4xmpCore:AltTextAccessibility"),
        extended_description: first("Iptc4xmpCore:ExtDescrAccessibility"),
        sublocation: first("Iptc4xmpCore:Location"),
        city: first("photoshop:City"),
        state: first("photoshop:State"),
        country: first("photoshop:Country"),
        copyright: first("dc:rights"),
        copyright_marked: first("xmpRights:Marked").and_then(|s| match s.to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }),
        usage_terms: first("xmpRights:UsageTerms"),
        copyright_url: first("xmpRights:WebStatement"),
        rating: num("xmp:Rating").map(|v| v.round().clamp(-1.0, 5.0) as i8),
        label: first("xmp:Label"),
        ..Default::default()
    };
    m.iso = num("exif:ISOSpeedRatings").or_else(|| num("exifEX:PhotographicSensitivity")).filter(|v| *v > 0.0).map(|v| v as u32);
    if let Some(v) = props.get("exif:ISOSpeedRatings").and_then(|v| v.first()).and_then(|s| parse_number(s)) {
        m.iso = Some(v as u32);
    }
    if let Some(spec) = props.get("exifEX:LensSpecification").filter(|v| v.len() == 4) {
        let v: Vec<f64> = spec.iter().filter_map(|s| parse_number(s)).collect();
        if v.len() == 4 {
            m.lens_spec = Some([v[0], v[1], v[2], v[3]]);
        }
    }
    if let Some(creators) = props.get("dc:creator").filter(|c| !c.is_empty()) {
        m.artist = Some(creators.join("; "));
    }
    m.keywords = props.get("dc:subject").cloned().unwrap_or_default().into_iter().filter(|k| !k.is_empty()).collect();
    m.hierarchical_keywords = props.get("lr:hierarchicalSubject").cloned().unwrap_or_default().into_iter().filter(|k| !k.is_empty()).collect();
    m.capture_time =
        ["exif:DateTimeOriginal", "photoshop:DateCreated", "xmp:CreateDate"].iter().find_map(|k| first(k).and_then(|s| DateTime::parse_iso(&s)));
    if let Some(fired) = first("exif:Flash/exif:Fired") {
        let b = |k: &str| first(k).is_some_and(|s| s.eq_ignore_ascii_case("true"));
        let n = |k: &str| num(k).unwrap_or(0.0) as u16;
        let fired = fired.eq_ignore_ascii_case("true");
        let raw = fired as u16
            | (n("exif:Flash/exif:Return") & 3) << 1
            | (n("exif:Flash/exif:Mode") & 3) << 3
            | (b("exif:Flash/exif:Function") as u16) << 5
            | (b("exif:Flash/exif:RedEyeMode") as u16) << 6;
        m.flash = Some(Flash { fired, raw });
    }
    if let (Some(lat), Some(lon)) =
        (first("exif:GPSLatitude").and_then(|s| parse_gps_coord(&s)), first("exif:GPSLongitude").and_then(|s| parse_gps_coord(&s)))
    {
        let altitude = num("exif:GPSAltitude").map(|a| if first("exif:GPSAltitudeRef").as_deref() == Some("1") { -a } else { a });
        m.gps = Some(Gps { latitude: lat, longitude: lon, altitude });
    }
    let lc_settings = props.get("lc:settings").and_then(|v| v.first()).cloned();
    Ok(XmpData { metadata: m, lc_settings, properties: props, values })
}

fn frac(v: f64) -> String {
    if v > 0.0 && v < 1.0 {
        let inv = 1.0 / v;
        if (inv - inv.round()).abs() < 1e-6 {
            return format!("1/{}", inv.round() as u64);
        }
    }
    let (n, d) = lightcraft_tiff::writer::rational(v);
    format!("{n}/{d}")
}

fn sfrac(v: f64) -> String {
    let (n, d) = lightcraft_tiff::writer::srational(v);
    format!("{n}/{d}")
}

/// Serialise the interchange subset of `meta` (plus `lc_settings`, an opaque JSON string, in `lc:settings`)
/// as a complete XMP packet.
pub fn write_xmp(meta: &Metadata, lc_settings: Option<&str>) -> String {
    let lc: Vec<(&str, &str)> = lc_settings.map(|s| ("settings", s)).into_iter().collect();
    write_xmp_lc(meta, &lc)
}

/// Like [`write_xmp`], with any number of simple `lc:<name>` properties (e.g. `settings`, `flag`).
pub fn write_xmp_lc(meta: &Metadata, lc: &[(&str, &str)]) -> String {
    let mut simple: Vec<(&str, String)> = Vec::new();
    let mut push = |k: &'static str, v: Option<String>| {
        if let Some(v) = v {
            simple.push((k, v));
        }
    };
    push("tiff:Make", meta.make.clone());
    push("tiff:Model", meta.model.clone());
    push("tiff:Orientation", meta.orientation.map(|o| o.to_exif().to_string()));
    push("tiff:ImageWidth", meta.width.map(|v| v.to_string()));
    push("tiff:ImageLength", meta.height.map(|v| v.to_string()));
    push("xmp:CreatorTool", meta.software.clone());
    push("xmp:Rating", meta.rating.map(|v| v.to_string()));
    push("xmp:Label", meta.label.clone());
    push("exif:DateTimeOriginal", meta.capture_time.map(|d| d.to_iso()));
    push("photoshop:DateCreated", meta.capture_time.map(|d| d.to_iso()));
    push("exif:ExposureTime", meta.exposure_time.map(frac));
    push("exif:FNumber", meta.f_number.map(frac));
    push("exif:FocalLength", meta.focal_length.map(frac));
    push("exif:FocalLengthIn35mmFilm", meta.focal_length_35mm.map(|v| format!("{}", v.round() as i64)));
    push("exif:ExposureBiasValue", meta.exposure_bias.map(sfrac));
    push("exif:ExposureProgram", meta.exposure_program.map(|v| v.to_string()));
    push("exif:MeteringMode", meta.metering_mode.map(|v| v.to_string()));
    push("exif:WhiteBalance", meta.white_balance.map(|v| v.to_string()));
    push("exifEX:BodySerialNumber", meta.serial_number.clone());
    push("exifEX:LensMake", meta.lens_make.clone());
    push("exifEX:LensModel", meta.lens_model.clone());
    push("exifEX:LensSerialNumber", meta.lens_serial_number.clone());
    push("Iptc4xmpCore:Location", meta.sublocation.clone());
    push("photoshop:City", meta.city.clone());
    push("photoshop:State", meta.state.clone());
    push("photoshop:Country", meta.country.clone());
    push("xmpRights:Marked", meta.copyright_marked.map(|m| if m { "True" } else { "False" }.to_string()));
    push("xmpRights:WebStatement", meta.copyright_url.clone());
    if let Some(g) = meta.gps {
        push("exif:GPSLatitude", Some(fmt_gps_coord(g.latitude, 'N', 'S')));
        push("exif:GPSLongitude", Some(fmt_gps_coord(g.longitude, 'E', 'W')));
        if let Some(a) = g.altitude {
            push("exif:GPSAltitude", Some(frac(a.abs())));
            push("exif:GPSAltitudeRef", Some(if a < 0.0 { "1" } else { "0" }.into()));
        }
    }

    let mut x = String::new();
    x.push_str("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    x.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"LightCraft\">\n");
    x.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    x.push_str("  <rdf:Description rdf:about=\"\"");
    for (p, u) in NAMESPACES.iter().skip(2).filter(|(p, _)| *p != "crs") {
        x.push_str(&format!("\n    xmlns:{p}=\"{u}\""));
    }
    x.push_str(">\n");
    for (k, v) in &simple {
        x.push_str(&format!("   <{k}>{}</{k}>\n", escape(v.as_str())));
    }
    for (k, v) in lc {
        if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            // element content: quotes needn't be escaped, which keeps the JSON readable
            x.push_str(&format!("   <lc:{k}>{}</lc:{k}>\n", partial_escape(*v)));
        }
    }
    let array = |x: &mut String, k: &str, kind: &str, items: &[String], lang: bool| {
        if items.is_empty() {
            return;
        }
        x.push_str(&format!("   <{k}>\n    <rdf:{kind}>\n"));
        for it in items {
            if lang {
                x.push_str(&format!("     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n", escape(it.as_str())));
            } else {
                x.push_str(&format!("     <rdf:li>{}</rdf:li>\n", escape(it.as_str())));
            }
        }
        x.push_str(&format!("    </rdf:{kind}>\n   </{k}>\n"));
    };
    let one = |v: &Option<String>| v.iter().cloned().collect::<Vec<_>>();
    array(&mut x, "dc:title", "Alt", &one(&meta.title), true);
    array(&mut x, "dc:description", "Alt", &one(&meta.caption), true);
    array(&mut x, "Iptc4xmpCore:AltTextAccessibility", "Alt", &one(&meta.alt_text), true);
    array(&mut x, "Iptc4xmpCore:ExtDescrAccessibility", "Alt", &one(&meta.extended_description), true);
    array(&mut x, "dc:rights", "Alt", &one(&meta.copyright), true);
    array(&mut x, "xmpRights:UsageTerms", "Alt", &one(&meta.usage_terms), true);
    array(&mut x, "dc:creator", "Seq", &one(&meta.artist), false);
    array(&mut x, "dc:subject", "Bag", &meta.keywords, false);
    array(&mut x, "lr:hierarchicalSubject", "Bag", &meta.hierarchical_keywords, false);
    array(&mut x, "exif:ISOSpeedRatings", "Seq", &meta.iso.map(|v| vec![v.to_string()]).unwrap_or_default(), false);
    if let Some(s) = meta.lens_spec {
        array(&mut x, "exifEX:LensSpecification", "Seq", &s.iter().map(|&v| frac(v)).collect::<Vec<_>>(), false);
    }
    if let Some(f) = meta.flash {
        let r = f.raw;
        let tf = |b: bool| if b { "True" } else { "False" };
        x.push_str(&format!(
            "   <exif:Flash rdf:parseType=\"Resource\">\n    <exif:Fired>{}</exif:Fired>\n    <exif:Return>{}</exif:Return>\n    <exif:Mode>{}</exif:Mode>\n    <exif:Function>{}</exif:Function>\n    <exif:RedEyeMode>{}</exif:RedEyeMode>\n   </exif:Flash>\n",
            tf(f.fired),
            (r >> 1) & 3,
            (r >> 3) & 3,
            tf(r & 0x20 != 0),
            tf(r & 0x40 != 0)
        ));
    }
    x.push_str("  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Metadata {
        Metadata {
            make: Some("Maker & Sons".into()),
            model: Some("X <1>".into()),
            serial_number: Some("SN1".into()),
            software: Some("LightCraft".into()),
            lens_make: Some("L".into()),
            lens_model: Some("24-70mm".into()),
            lens_serial_number: Some("LS".into()),
            lens_spec: Some([24.0, 70.0, 2.8, 2.8]),
            capture_time: DateTime::parse_iso("2022-05-06T07:08:09.123-04:00"),
            exposure_time: Some(1.0 / 125.0),
            f_number: Some(5.6),
            iso: Some(800),
            focal_length: Some(35.0),
            focal_length_35mm: Some(52.0),
            flash: Some(Flash { fired: true, raw: 0x59 }),
            exposure_bias: Some(-1.0 / 3.0),
            exposure_program: Some(2),
            metering_mode: Some(3),
            white_balance: Some(1),
            orientation: Some(Orientation::Rotate270),
            gps: Some(Gps { latitude: 48.858222, longitude: -2.2945, altitude: Some(-3.5) }),
            width: Some(8000),
            height: Some(6000),
            artist: Some("Ann \"Photo\" Lee".into()),
            copyright: Some("© 2022".into()),
            copyright_marked: Some(true),
            usage_terms: Some("Editorial use only; no <resale>".into()),
            copyright_url: Some("https://example.com/licence?a=1&b=2".into()),
            title: Some("Tïtle".into()),
            caption: Some("Line one\nline two".into()),
            alt_text: Some("A tree by the sea".into()),
            extended_description: Some("Long <description>".into()),
            sublocation: Some("Champ de Mars".into()),
            city: Some("Paris".into()),
            state: Some("Île-de-France".into()),
            country: Some("France".into()),
            keywords: vec!["tree".into(), "sky & sea".into()],
            hierarchical_keywords: vec!["Places|France|Paris".into()],
            rating: Some(-1),
            label: Some("Green".into()),
        }
    }

    #[test]
    fn roundtrip_all_fields() {
        let m = full();
        let json = r#"{"version":1,"exposure":0.5,"note":"<&>"}"#;
        let x = write_xmp(&m, Some(json));
        let d = parse_xmp(&x).unwrap();
        assert_eq!(d.lc_settings.as_deref(), Some(json));
        let mut got = d.metadata.clone();
        // floats go through rationals
        assert!((got.exposure_bias.unwrap() + 1.0 / 3.0).abs() < 1e-5);
        got.exposure_bias = m.exposure_bias;
        let g = got.gps.unwrap();
        assert!((g.latitude - 48.858222).abs() < 1e-6 && (g.longitude + 2.2945).abs() < 1e-6, "{g:?}");
        got.gps = m.gps;
        assert_eq!(got, m);
    }

    #[test]
    fn empty_metadata_roundtrip() {
        let x = write_xmp(&Metadata::default(), None);
        let d = parse_xmp(&x).unwrap();
        assert_eq!(d.metadata, Metadata::default());
        assert_eq!(d.lc_settings, None);
    }

    #[test]
    fn attribute_form_foreign_prefixes_and_structs() {
        let x = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:a="http://ns.adobe.com/xap/1.0/" xmlns:d="http://purl.org/dc/elements/1.1/"
               xmlns:e="http://ns.adobe.com/exif/1.0/" a:Rating="4" a:Label="Blue" e:FNumber="40/10" e:GPSLatitude="10,30N" e:GPSLongitude="20,15,36W">
              <d:title><rdf:Alt><rdf:li xml:lang="de">Titel</rdf:li><rdf:li xml:lang="x-default">Title &amp; more &#233;</rdf:li></rdf:Alt></d:title>
              <e:Flash><rdf:Description e:Fired="False" e:Mode="2"/></e:Flash>
              <d:subject><rdf:Bag><rdf:li>k1</rdf:li><rdf:li>k2</rdf:li></rdf:Bag></d:subject>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let d = parse_xmp(x).unwrap();
        let m = d.metadata;
        assert_eq!(m.rating, Some(4));
        assert_eq!(m.label.as_deref(), Some("Blue"));
        assert_eq!(m.f_number, Some(4.0));
        assert_eq!(m.title.as_deref(), Some("Title & more é"));
        assert_eq!(m.keywords, vec!["k1".to_string(), "k2".to_string()]);
        assert_eq!(m.flash, Some(Flash { fired: false, raw: 0x10 }));
        let g = m.gps.unwrap();
        assert!((g.latitude - 10.5).abs() < 1e-9);
        assert!((g.longitude + 20.26).abs() < 1e-9);
        assert_eq!(d.properties["dc:title"], vec!["Title & more é".to_string(), "Titel".to_string()]);
    }

    /// Copyright status, usage terms and info URL (XMP Rights Management schema): written as
    /// `xmpRights:Marked` (Boolean), `xmpRights:UsageTerms` (Lang Alt) and `xmpRights:WebStatement` (URI).
    #[test]
    fn rights_management_fields() {
        let m = Metadata { copyright_marked: Some(false), usage_terms: Some("CC0".into()), ..Default::default() };
        let x = write_xmp(&m, None);
        assert!(x.contains("<xmpRights:Marked>False</xmpRights:Marked>"), "{x}");
        assert!(x.contains("<xmpRights:UsageTerms>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">CC0</rdf:li>"), "{x}");
        assert!(!x.contains("WebStatement"), "unset fields are left out");
        assert_eq!(parse_xmp(&x).unwrap().metadata, m);
        // attribute form, other prefixes; anything but True / False is "unknown"
        let read = |marked: &str| {
            let x = format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <rdf:Description rdf:about="" xmlns:r="http://ns.adobe.com/xap/1.0/rights/" r:Marked="{marked}" r:WebStatement="https://example.org/c"/>
                </rdf:RDF></x:xmpmeta>"#
            );
            parse_xmp(&x).unwrap().metadata
        };
        assert_eq!(read("True").copyright_marked, Some(true));
        assert_eq!(read("false").copyright_marked, Some(false));
        assert_eq!(read("").copyright_marked, None);
        assert_eq!(read("maybe").copyright_marked, None);
        assert_eq!(read("True").copyright_url.as_deref(), Some("https://example.org/c"));
    }

    #[test]
    fn malformed_inputs() {
        assert!(parse_xmp("<a><b></a>").is_ok() || parse_xmp("<a><b></a>").is_err());
        assert_eq!(parse_xmp("").unwrap().metadata, Metadata::default());
        assert_eq!(parse_xmp("not xml at all").unwrap().metadata, Metadata::default());
        let deep = "<a>".repeat(1000);
        assert_eq!(parse_xmp(&deep), Err(XmpError::TooComplex));
        let x = write_xmp(&full(), Some("{}"));
        for n in (0..x.len()).step_by(7) {
            if x.is_char_boundary(n) {
                let _ = parse_xmp(&x[..n]);
            }
        }
    }

    #[test]
    fn lc_extras_and_crs_prefix() {
        let x = write_xmp_lc(&Metadata::default(), &[("settings", "{\"a\":1}"), ("flag", "pick"), ("bad name", "x")]);
        assert!(!x.contains("xmlns:crs"), "we never write crs:");
        let d = parse_xmp(&x).unwrap();
        assert_eq!(d.lc_settings.as_deref(), Some("{\"a\":1}"));
        assert_eq!(d.properties["lc:flag"], vec!["pick".to_string()]);
        assert!(!x.contains("bad name"));
        let foreign = r#"<rdf:Description xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:cr="http://ns.adobe.com/camera-raw-settings/1.0/" cr:Exposure2012="+0.5"/>"#;
        assert_eq!(parse_xmp(foreign).unwrap().properties["crs:Exposure2012"], vec!["+0.5".to_string()]);
    }

    #[test]
    fn gps_coords() {
        assert_eq!(parse_gps_coord("45,30N"), Some(45.5));
        assert_eq!(parse_gps_coord("45,30,36S"), Some(-45.51));
        assert_eq!(parse_gps_coord("45X"), None);
        assert_eq!(parse_gps_coord(""), None);
        assert_eq!(parse_gps_coord("é"), None);
        assert_eq!(fmt_gps_coord(-45.51, 'N', 'S'), "45,30.600000S");
    }
}
