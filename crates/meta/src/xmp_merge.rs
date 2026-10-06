//! Merging a freshly written XMP packet into an existing one (a sidecar another application
//! wrote, e.g. with its develop settings and edit history), keeping everything the writer does not
//! own byte for byte.
//!
//! The existing packet is scanned once (namespace-resolved, with byte positions). Only top-level
//! properties of the `rdf:Description` elements directly under `rdf:RDF` are considered, in
//! attribute form (`xmp:Rating="3"`) and element form (`<dc:subject>…</dc:subject>`):
//! - properties the writer owns ([`MergeRules::owned`]) are removed, and written from the fresh
//!   packet when it has them (so a cleared field is cleared in the file too);
//! - properties owned only when stated ([`MergeRules::owned_if_present`]) are replaced when the
//!   fresh packet has them and kept otherwise;
//! - any other property of the fresh packet (e.g. `xmp:CreatorTool`) is not written: the file
//!   keeps describing itself as its creator left it;
//! - everything else — other namespaces (e.g. `crs:`), `xmpMM:History`, unknown properties,
//!   comments, the packet wrapper and padding — is copied unchanged.
//!
//! The written properties go into one new `rdf:Description` (declaring its own namespaces, so
//! the existing prefix bindings never matter) just before `</rdf:RDF>`. A description left empty
//! by the removals is dropped, so repeated saves don't accumulate. The result is checked by
//! parsing it again: every written property must read back as in the fresh packet.

use std::collections::BTreeSet;

use quick_xml::events::Event;

use crate::xmp::{XmpError, canonical_name, parse_xmp};

/// Which properties [`merge_xmp`] takes from the fresh packet. Names are canonical
/// `prefix:name` (prefixes as [`crate::parse_xmp`] reports them); `prefix:*` is a whole namespace.
#[derive(Clone, Copy, Debug, Default)]
pub struct MergeRules<'a> {
    /// Always replaced: removed from the existing packet, written from the fresh one if stated.
    pub owned: &'a [&'a str],
    /// Replaced only when the fresh packet states them; otherwise the existing value stays.
    pub owned_if_present: &'a [&'a str],
}

fn listed(list: &[&str], name: &str) -> bool {
    list.iter().any(|p| match p.strip_suffix('*') {
        Some(ns) => name.starts_with(ns),
        None => *p == name,
    })
}

#[derive(Debug)]
struct Attr {
    raw: String,
    name: String,
    /// The value as written (still escaped).
    value: String,
}

#[derive(Debug)]
struct Prop {
    name: String,
    start: usize,
    end: usize,
}

#[derive(Debug, Default)]
struct Desc {
    tag_start: usize,
    tag_end: usize,
    /// The element name as written (`rdf:Description`, or another prefix for the RDF namespace).
    raw_name: String,
    empty: bool,
    attrs: Vec<Attr>,
    props: Vec<Prop>,
    /// The end of `</rdf:Description>` (`tag_end` when self-closing).
    end: usize,
}

#[derive(Debug, Default)]
struct Scan {
    descs: Vec<Desc>,
    /// Where the first `</rdf:RDF>` starts.
    rdf_close: Option<usize>,
}

enum Kind {
    Other,
    Rdf,
    Desc(usize),
    Prop { desc: usize, start: usize, name: String },
}

fn is_meta_attr(name: &str) -> bool {
    name.starts_with("xmlns")
        || matches!(name, "rdf:about" | "rdf:ID" | "rdf:nodeID" | "xml:lang" | "rdf:parseType" | "rdf:resource" | "rdf:datatype")
}

fn pos(r: &quick_xml::Reader<&[u8]>) -> usize {
    usize::try_from(r.buffer_position()).unwrap_or(usize::MAX)
}

/// Locate the descriptions, their top-level properties and `</rdf:RDF>` in a packet.
fn scan(s: &str) -> Result<Scan, XmpError> {
    let mut reader = quick_xml::Reader::from_reader(s.as_bytes());
    reader.config_mut().check_end_names = true;
    let mut out = Scan::default();
    let mut stack: Vec<Kind> = Vec::new();
    let mut scopes: Vec<Vec<(String, String)>> = vec![vec![("xml".into(), "http://www.w3.org/XML/1998/namespace".into())]];
    let mut count = 0usize;
    loop {
        let before = pos(&reader);
        let ev = reader.read_event().map_err(|e| XmpError::Xml(e.to_string()))?;
        let after = pos(&reader);
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                count += 1;
                if count > 200_000 || stack.len() > 64 {
                    return Err(XmpError::TooComplex);
                }
                let empty = matches!(ev, Event::Empty(_));
                let mut scope = Vec::new();
                let mut raw_attrs = Vec::new();
                for a in e.attributes() {
                    let a = a.map_err(|e| XmpError::Xml(e.to_string()))?;
                    let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
                    let v = String::from_utf8_lossy(&a.value).into_owned();
                    if k == "xmlns" {
                        scope.push((String::new(), v.clone()));
                    } else if let Some(p) = k.strip_prefix("xmlns:") {
                        scope.push((p.to_string(), v.clone()));
                    }
                    raw_attrs.push((k, v));
                }
                scopes.push(scope);
                let raw_name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let name = canonical_name(&raw_name, &scopes);
                let kind = match stack.last() {
                    Some(Kind::Rdf) if name == "rdf:Description" => {
                        let attrs = raw_attrs.into_iter().map(|(raw, value)| Attr { name: canonical_name(&raw, &scopes), raw, value }).collect();
                        out.descs.push(Desc { tag_start: before, tag_end: after, raw_name, empty, attrs, props: Vec::new(), end: after });
                        Kind::Desc(out.descs.len() - 1)
                    }
                    Some(Kind::Desc(i)) => Kind::Prop { desc: *i, start: before, name },
                    _ if name == "rdf:RDF" && out.rdf_close.is_none() => Kind::Rdf,
                    _ => Kind::Other,
                };
                if empty {
                    scopes.pop();
                    if let Kind::Prop { desc, start, name } = kind
                        && let Some(d) = out.descs.get_mut(desc)
                    {
                        d.props.push(Prop { name, start, end: after });
                    }
                } else {
                    stack.push(kind);
                }
            }
            Event::End(_) => {
                scopes.pop();
                match stack.pop() {
                    Some(Kind::Prop { desc, start, name }) => {
                        if let Some(d) = out.descs.get_mut(desc) {
                            d.props.push(Prop { name, start, end: after });
                        }
                    }
                    Some(Kind::Desc(i)) => {
                        if let Some(d) = out.descs.get_mut(i) {
                            d.end = after;
                        }
                    }
                    Some(Kind::Rdf) => out.rdf_close = out.rdf_close.or(Some(before)),
                    Some(Kind::Other) => {}
                    None => return Err(XmpError::Xml("unbalanced end tag".into())),
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(XmpError::Xml("unclosed element".into()));
    }
    Ok(out)
}

/// `start` moved back over the indentation and line break before it (so a removed property
/// takes its line with it).
fn line_start(s: &str, start: usize) -> usize {
    let b = s.as_bytes();
    let mut i = start;
    while i > 0 && matches!(b.get(i - 1), Some(b' ' | b'\t')) {
        i -= 1;
    }
    if i > 0 && b.get(i - 1) == Some(&b'\n') {
        i -= 1;
        if i > 0 && b.get(i - 1) == Some(&b'\r') {
            i -= 1;
        }
    }
    i
}

/// Merge `fresh` (a packet as [`crate::write_xmp_lc`] writes it) into `existing` following
/// `rules` (see the module docs). Fails — and the caller must not overwrite `existing` blindly —
/// when `existing` isn't a well-formed XMP packet.
pub fn merge_xmp(existing: &str, fresh: &str, rules: MergeRules) -> Result<String, XmpError> {
    let (bom, old) = match existing.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", existing),
    };
    parse_xmp(old)?;
    let ex = scan(old)?;
    let rdf_close = ex.rdf_close.ok_or_else(|| XmpError::Xml("no rdf:RDF element".into()))?;
    let new = scan(fresh)?;
    let fresh_desc = new.descs.first().ok_or_else(|| XmpError::Xml("the new packet has no rdf:Description".into()))?;
    let fresh_names: BTreeSet<&str> = fresh_desc.props.iter().map(|p| p.name.as_str()).collect();
    let remove = |name: &str| listed(rules.owned, name) || (listed(rules.owned_if_present, name) && fresh_names.contains(name));
    let write = |name: &str| listed(rules.owned, name) || listed(rules.owned_if_present, name);

    // edits on `old`: (start, end, replacement), applied back to front
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for d in &ex.descs {
        let gone_attrs: Vec<&Attr> = d.attrs.iter().filter(|a| !is_meta_attr(&a.name) && remove(&a.name)).collect();
        let gone_props: Vec<&Prop> = d.props.iter().filter(|p| remove(&p.name)).collect();
        if gone_attrs.is_empty() && gone_props.is_empty() {
            continue;
        }
        let attrs_left = d.attrs.iter().any(|a| !is_meta_attr(&a.name) && !remove(&a.name));
        if !attrs_left && gone_props.len() == d.props.len() {
            // nothing of its own left: drop the whole description
            edits.push((line_start(old, d.tag_start), d.end, String::new()));
            continue;
        }
        if !gone_attrs.is_empty() {
            let mut tag = format!("<{}", d.raw_name);
            for a in d.attrs.iter().filter(|a| is_meta_attr(&a.name) || !remove(&a.name)) {
                tag.push_str(&format!("\n    {}=\"{}\"", a.raw, a.value.replace('"', "&quot;")));
            }
            tag.push_str(if d.empty { "/>" } else { ">" });
            edits.push((d.tag_start, d.tag_end, tag));
        }
        for p in gone_props {
            edits.push((line_start(old, p.start), p.end, String::new()));
        }
    }
    // the new description: its own namespace declarations, the properties to write
    let mut block = String::from("\n  <rdf:Description rdf:about=\"\"\n    xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"");
    for a in fresh_desc.attrs.iter().filter(|a| a.raw.starts_with("xmlns:") && a.raw != "xmlns:rdf") {
        block.push_str(&format!("\n    {}=\"{}\"", a.raw, a.value));
    }
    block.push('>');
    let mut written = 0;
    for p in fresh_desc.props.iter().filter(|p| write(&p.name)) {
        let text = fresh.get(p.start..p.end).ok_or_else(|| XmpError::Xml("bad property span".into()))?;
        block.push_str("\n   ");
        block.push_str(text);
        written += 1;
    }
    block.push_str("\n  </rdf:Description>");
    if written > 0 {
        let at = old.get(..rdf_close).map_or(rdf_close, |h| h.trim_end().len());
        edits.push((at, at, block));
    }
    edits.sort_by_key(|e| (e.0, e.1));
    // overlapping edits can't happen (properties sit inside their description, which is either
    // dropped whole or edited piecewise), but never splice blindly
    let mut out = String::with_capacity(old.len() + 4096);
    out.push_str(bom);
    let mut at = 0;
    for (s, e, r) in &edits {
        if *s < at {
            return Err(XmpError::Xml("overlapping edits".into()));
        }
        out.push_str(old.get(at..*s).ok_or_else(|| XmpError::Xml("bad span".into()))?);
        out.push_str(r);
        at = *e;
    }
    out.push_str(old.get(at..).ok_or_else(|| XmpError::Xml("bad span".into()))?);

    // read back: everything written must come out as written
    let merged = parse_xmp(&out)?;
    let want = parse_xmp(fresh)?;
    for (k, v) in &want.properties {
        let top = k.split('/').next().unwrap_or(k);
        if write(top) && merged.properties.get(k) != Some(v) {
            return Err(XmpError::Xml(format!("{k} did not survive the merge")));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: MergeRules =
        MergeRules { owned: &["xmp:Rating", "xmp:Label", "dc:subject", "dc:title", "lc:*"], owned_if_present: &["exif:DateTimeOriginal"] };

    /// A sidecar as another raw developer writes it: attribute-form simple properties, develop
    /// settings, a structured history, an unknown namespace.
    const FOREIGN: &str = "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Some Toolkit 1.0\">
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">
  <rdf:Description rdf:about=\"\"
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"
    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"
    xmlns:stEvt=\"http://ns.adobe.com/xap/1.0/sType/ResourceEvent#\"
    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"
    xmlns:foo=\"http://example.com/foo/\"
    xmp:Rating=\"2\"
    xmp:CreatorTool=\"Other App\"
    crs:Exposure2012=\"+0.50\"
    crs:Contrast2012=\"-12\"
    foo:Thing=\"a &amp; b\">
   <xmpMM:History>
    <rdf:Seq>
     <rdf:li stEvt:action=\"saved\" stEvt:when=\"2026-01-02T03:04:05\"/>
    </rdf:Seq>
   </xmpMM:History>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>old</rdf:li>
    </rdf:Bag>
   </dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
";

    /// With the packet's whitespace padding and trailer.
    fn foreign() -> String {
        format!("{FOREIGN}{}\n<?xpacket end=\"w\"?>", " ".repeat(100))
    }

    fn fresh(rating: i8, keywords: &[&str]) -> String {
        let m = crate::Metadata {
            software: Some("LightCraft".into()),
            rating: Some(rating),
            keywords: keywords.iter().map(|k| k.to_string()).collect(),
            ..Default::default()
        };
        crate::write_xmp_lc(&m, &[("settings", "{\"a\":1}"), ("flag", "pick")])
    }

    #[test]
    fn keeps_foreign_properties_and_replaces_owned_ones() {
        let out = merge_xmp(&foreign(), &fresh(4, &["new", "two"]), RULES).unwrap();
        let d = parse_xmp(&out).unwrap();
        let p = |k: &str| d.properties.get(k).cloned().unwrap_or_default();
        assert_eq!(p("xmp:Rating"), vec!["4"]);
        assert_eq!(p("dc:subject"), vec!["new", "two"]);
        assert_eq!(p("lc:settings"), vec!["{\"a\":1}"]);
        assert_eq!(p("lc:flag"), vec!["pick"]);
        assert_eq!(p("xmp:CreatorTool"), vec!["Other App"], "not ours: kept");
        assert_eq!(p("crs:Exposure2012"), vec!["+0.50"]);
        assert_eq!(p("foo:Thing").len(), 1);
        // untouched parts are byte for byte the same
        for part in [
            "   <xmpMM:History>\n    <rdf:Seq>\n     <rdf:li stEvt:action=\"saved\" stEvt:when=\"2026-01-02T03:04:05\"/>\n    </rdf:Seq>\n   </xmpMM:History>",
            "   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>",
            "crs:Contrast2012=\"-12\"",
            "foo:Thing=\"a &amp; b\"",
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Some Toolkit 1.0\">",
            &format!("</x:xmpmeta>\n{}\n<?xpacket end=\"w\"?>", " ".repeat(100)),
        ] {
            assert!(out.contains(part), "lost {part:?} in\n{out}");
        }
        assert!(!out.contains("xmp:Rating=\"2\""));
        assert!(!out.contains("<rdf:li>old</rdf:li>"));
        // saving again changes only our part, and doesn't pile up descriptions
        let again = merge_xmp(&out, &fresh(1, &[]), RULES).unwrap();
        let d = parse_xmp(&again).unwrap();
        assert_eq!(d.properties.get("xmp:Rating"), Some(&vec!["1".to_string()]));
        assert!(!d.properties.contains_key("dc:subject"), "a cleared field is cleared");
        assert_eq!(again.matches("<rdf:Description").count(), 2, "{again}");
        assert_eq!(merge_xmp(&again, &fresh(1, &[]), RULES).unwrap(), again, "idempotent");
    }

    #[test]
    fn owned_if_present_and_prefix_independence() {
        // the existing packet binds the namespaces to other prefixes, and has a capture time
        let ex = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><r:RDF xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
            <r:Description r:about=\"\" xmlns:a=\"http://ns.adobe.com/xap/1.0/\" xmlns:e=\"http://ns.adobe.com/exif/1.0/\" \
            a:Rating=\"5\" e:DateTimeOriginal=\"2020-01-01T00:00:00\"/></r:RDF></x:xmpmeta>";
        let out = merge_xmp(ex, &fresh(3, &[]), RULES).unwrap();
        let d = parse_xmp(&out).unwrap();
        assert_eq!(d.properties.get("xmp:Rating"), Some(&vec!["3".to_string()]));
        assert_eq!(d.properties.get("exif:DateTimeOriginal"), Some(&vec!["2020-01-01T00:00:00".to_string()]), "fresh has none: kept");
        let mut m = crate::Metadata { rating: Some(3), ..Default::default() };
        m.capture_time = crate::DateTime::parse_iso("2024-05-06T07:08:09");
        let out = merge_xmp(ex, &crate::write_xmp_lc(&m, &[]), RULES).unwrap();
        let d = parse_xmp(&out).unwrap();
        assert_eq!(d.properties.get("exif:DateTimeOriginal"), Some(&vec!["2024-05-06T07:08:09".to_string()]));
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        for bad in ["", "not xml at all", "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF", "<a><b></a>", "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>"] {
            assert!(merge_xmp(bad, &fresh(1, &[]), RULES).is_err(), "{bad:?}");
        }
    }
}
