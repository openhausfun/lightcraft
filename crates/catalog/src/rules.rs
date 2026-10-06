//! Smart-album rules: a list of conditions matched all / any / none, with nested groups.
//!
//! Each [`Rule`] is `{field, op, value}`; [`FIELDS`] lists the fields with their kind, which
//! decides the operators ([`ops_for`]). Text compares case-insensitively; dates are ISO strings
//! compared by prefix; "in the last N days/weeks/months/years" is relative to [`now`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Catalog, Photo};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Match {
    #[default]
    All,
    Any,
    None,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    #[serde(default, rename = "match")]
    pub mode: Match,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Rule {
    /// A nested group with its own match mode.
    Group { group: RuleSet },
    Field {
        field: String,
        op: String,
        #[serde(default)]
        value: Value,
    },
}

/// How a field's value is compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// Keywords: text ops over each keyword (any keyword matches; `isEmpty` = none).
    Keywords,
    Number,
    Date,
    /// One of a fixed set (`choices`).
    Choice(&'static [&'static str]),
    Bool,
}

/// The fields rules can test: (id, label, kind).
pub const FIELDS: &[(&str, &str, Kind)] = &[
    ("rating", "Rating", Kind::Number),
    ("flag", "Pick Flag", Kind::Choice(&["pick", "reject", "none"])),
    ("label", "Color Label", Kind::Choice(&["red", "yellow", "green", "blue", "purple", "none"])),
    ("kind", "File Type", Kind::Choice(&["image", "raw", "video"])),
    ("edited", "Has Edits", Kind::Bool),
    ("keywords", "Keywords", Kind::Keywords),
    ("text", "Any Searchable Text", Kind::Text),
    ("fileName", "Filename", Kind::Text),
    ("format", "File Format", Kind::Text),
    ("title", "Title", Kind::Text),
    ("caption", "Caption", Kind::Text),
    ("camera", "Camera", Kind::Text),
    ("lens", "Lens", Kind::Text),
    ("location", "Location", Kind::Text),
    ("creator", "Creator", Kind::Text),
    ("copyright", "Copyright", Kind::Text),
    ("copyrightStatus", "Copyright Status", Kind::Choice(&["copyrighted", "publicDomain", "unknown"])),
    ("captureDate", "Capture Date", Kind::Date),
    ("importDate", "Import Date", Kind::Date),
    ("editDate", "Edit Date", Kind::Date),
    ("iso", "ISO Speed", Kind::Number),
    ("aperture", "Aperture", Kind::Number),
    ("focalLength", "Focal Length", Kind::Number),
    ("megapixels", "Megapixels", Kind::Number),
    ("hasGps", "Has GPS", Kind::Bool),
    ("virtualCopy", "Virtual Copy", Kind::Bool),
    ("album", "Album", Kind::Number),
    ("sharpness", "Focus (assisted culling)", Kind::Number),
    ("bestOfGroup", "Best of Similar Shots", Kind::Bool),
];

/// The operators for a field kind: (id, label).
pub fn ops_for(kind: Kind) -> &'static [(&'static str, &'static str)] {
    match kind {
        Kind::Text => &[
            ("contains", "contains"),
            ("notContains", "doesn't contain"),
            ("is", "is"),
            ("isNot", "isn't"),
            ("startsWith", "starts with"),
            ("endsWith", "ends with"),
            ("isEmpty", "is empty"),
            ("isNotEmpty", "isn't empty"),
        ],
        Kind::Keywords => &[
            ("contains", "contains"),
            ("notContains", "doesn't contain"),
            ("is", "is"),
            ("startsWith", "starts with"),
            ("isEmpty", "are empty"),
            ("isNotEmpty", "aren't empty"),
        ],
        Kind::Number => {
            &[("is", "is"), ("isNot", "isn't"), ("gte", "is ≥"), ("lte", "is ≤"), ("gt", "is >"), ("lt", "is <"), ("between", "is between")]
        }
        Kind::Date => &[
            ("is", "is"),
            ("after", "is after"),
            ("before", "is before"),
            ("between", "is between"),
            ("inLast", "is in the last"),
            ("notInLast", "isn't in the last"),
            ("isEmpty", "is unknown"),
        ],
        Kind::Choice(_) => &[("is", "is"), ("isNot", "isn't")],
        Kind::Bool => &[("is", "is")],
    }
}

pub fn field_kind(field: &str) -> Option<Kind> {
    FIELDS.iter().find(|f| f.0 == field).map(|f| f.2)
}

thread_local! {
    static NOW: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Set the time "in the last…" rules count back from (the engine's clock; tests).
pub fn set_now(iso: Option<String>) {
    NOW.with(|n| *n.borrow_mut() = iso);
}

/// The current time as ISO 8601: [`set_now`]'s, else the system clock (UTC).
pub fn now() -> String {
    if let Some(n) = NOW.with(|n| n.borrow().clone()) {
        return n;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        crate::dates::civil(secs)
    }
    #[cfg(target_arch = "wasm32")]
    {
        "2026-01-01T00:00:00".to_string()
    }
}

fn lower(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_lowercase(),
        Value::Null => String::new(),
        other => other.to_string().to_lowercase(),
    }
}

fn text_op(op: &str, have: &str, want: &str) -> bool {
    let have = have.trim().to_lowercase();
    match op {
        "contains" => want.split_whitespace().all(|w| have.contains(w)),
        "notContains" => !want.split_whitespace().any(|w| have.contains(w)),
        "is" => have == want,
        "isNot" => have != want,
        "startsWith" => have.starts_with(want),
        "endsWith" => have.ends_with(want),
        "isEmpty" => have.is_empty(),
        "isNotEmpty" => !have.is_empty(),
        _ => false,
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().trim_start_matches("f/").trim_end_matches("mm").trim().parse().ok(),
        _ => None,
    }
}

fn num_op(op: &str, have: Option<f64>, value: &Value) -> bool {
    if op == "between" {
        let (a, b) = match value {
            Value::Array(v) if v.len() == 2 => (number(&v[0]), number(&v[1])),
            _ => (None, None),
        };
        return matches!((have, a, b), (Some(h), Some(a), Some(b)) if h >= a.min(b) && h <= a.max(b));
    }
    let (Some(h), Some(w)) = (have, number(value)) else { return op == "isNot" && have.is_none() };
    match op {
        "is" => (h - w).abs() < 1e-6,
        "isNot" => (h - w).abs() >= 1e-6,
        "gte" => h >= w - 1e-9,
        "lte" => h <= w + 1e-9,
        "gt" => h > w,
        "lt" => h < w,
        _ => false,
    }
}

/// `value` for in-the-last rules: `{n, unit: days|weeks|months|years}` or a number of days.
fn last_secs(value: &Value) -> Option<i64> {
    let (n, unit) = match value {
        Value::Object(o) => (o.get("n").and_then(number)?, o.get("unit").and_then(Value::as_str).unwrap_or("days")),
        v => (number(v)?, "days"),
    };
    let day = 86_400.0;
    let per = match unit {
        "hours" => 3600.0,
        "weeks" => 7.0 * day,
        "months" => 30.4375 * day,
        "years" => 365.25 * day,
        _ => day,
    };
    Some((n * per) as i64)
}

fn date_op(op: &str, have: Option<&str>, value: &Value) -> bool {
    if op == "isEmpty" {
        return have.is_none_or(str::is_empty);
    }
    let Some(h) = have.filter(|h| !h.is_empty()) else { return op == "notInLast" };
    let s = |v: &Value| v.as_str().map(str::trim).unwrap_or("").to_string();
    match op {
        // a prefix: 2026, 2026-04, 2026-04-12
        "is" => {
            let w = s(value);
            !w.is_empty() && h.starts_with(&w)
        }
        "after" => {
            let w = s(value);
            !w.is_empty() && h > w.as_str() && !h.starts_with(&w)
        }
        "before" => {
            let w = s(value);
            !w.is_empty() && h < w.as_str()
        }
        "between" => match value {
            Value::Array(v) if v.len() == 2 => {
                let (a, b) = (s(&v[0]), s(&v[1]));
                h >= a.as_str() && (h <= b.as_str() || h.starts_with(&b))
            }
            _ => false,
        },
        "inLast" | "notInLast" => {
            let Some(secs) = last_secs(value) else { return false };
            let from = crate::dates::shift_iso(&now(), -secs).unwrap_or_default();
            (h >= from.as_str()) == (op == "inLast")
        }
        _ => false,
    }
}

impl Rule {
    pub fn matches(&self, p: &Photo, cat: &Catalog) -> bool {
        let (field, op, value) = match self {
            Rule::Group { group } => return group.matches(p, cat),
            Rule::Field { field, op, value } => (field.as_str(), op.as_str(), value),
        };
        let m = &p.meta;
        let want = lower(value);
        let text = |have: &str| text_op(op, have, &want);
        match field {
            "rating" => num_op(op, Some(p.rating as f64), value),
            "flag" => {
                let f = format!("{:?}", p.flag).to_lowercase();
                (f == want || (want == "picked" && f == "pick") || (want == "rejected" && f == "reject")) == (op == "is")
            }
            "label" => {
                let l = p.label.map(|l| format!("{l:?}").to_lowercase()).unwrap_or_else(|| "none".into());
                // custom label names count too
                let named = p.label.is_some_and(|l| cat.label_name(l).to_lowercase() == want);
                (l == want || named) == (op == "is")
            }
            "kind" => (format!("{:?}", p.kind).to_lowercase() == want) == (op == "is"),
            "edited" => p.is_edited() == value.as_bool().unwrap_or(true),
            "hasGps" => m.gps.is_some() == value.as_bool().unwrap_or(true),
            "virtualCopy" => p.copy_of.is_some() == value.as_bool().unwrap_or(true),
            "keywords" => match op {
                "isEmpty" => m.keywords.is_empty(),
                "isNotEmpty" => !m.keywords.is_empty(),
                "notContains" => !m.keywords.iter().any(|k| text_op("contains", k, &want)),
                _ => m.keywords.iter().any(|k| text_op(op, k, &want) || (op == "is" && k.to_lowercase().split('|').any(|part| part == want))),
            },
            "text" => {
                let all = [
                    p.file_name.as_str(),
                    &m.title,
                    &m.caption,
                    &m.camera,
                    &m.lens,
                    &m.location,
                    &m.city,
                    &m.country,
                    &p.format,
                    &m.keywords.join(" "),
                ]
                .join(" ");
                text(&all)
            }
            "fileName" => text(&p.file_name),
            "format" => text(&p.format),
            "title" => text(&m.title),
            "caption" => text(&m.caption),
            "camera" => text(&m.camera),
            "lens" => text(&m.lens),
            "location" => text(&[m.location.as_str(), &m.city, &m.state, &m.country].join(" ")),
            "creator" => text(&m.creator),
            "copyright" => text(&m.copyright),
            "copyrightStatus" => {
                let want = crate::CopyrightStatus::parse(&want);
                (want == Some(m.copyright_status)) == (op == "is")
            }
            "captureDate" => date_op(op, p.captured.as_deref(), value),
            "importDate" => date_op(op, Some(&p.imported), value),
            "editDate" => date_op(op, p.edited.as_deref(), value),
            "iso" => num_op(op, m.iso.map(|v| v as f64), value),
            "aperture" => num_op(op, m.aperture.map(|v| v as f64), value),
            "focalLength" => num_op(op, m.focal_mm.map(|v| v as f64), value),
            "megapixels" => num_op(op, Some(p.width as f64 * p.height as f64 / 1e6), value),
            "sharpness" => num_op(op, p.analysis.map(|a| a.sharpness as f64), value),
            "bestOfGroup" => p.analysis.is_some_and(|a| a.best || a.group.is_none()) == value.as_bool().unwrap_or(true),
            "album" => {
                let id = number(value).map(|v| crate::AlbumId(v as u64));
                id.is_some_and(|a| cat.album(a).is_some_and(|al| !al.is_smart()) && cat.album_contains(a, p)) == (op == "is")
            }
            _ => false,
        }
    }
}

impl RuleSet {
    pub fn matches(&self, p: &Photo, cat: &Catalog) -> bool {
        if self.rules.is_empty() {
            return self.mode != Match::Any;
        }
        match self.mode {
            Match::All => self.rules.iter().all(|r| r.matches(p, cat)),
            Match::Any => self.rules.iter().any(|r| r.matches(p, cat)),
            Match::None => !self.rules.iter().any(|r| r.matches(p, cat)),
        }
    }

    /// Whether matches depend on the clock ("in the last…" rules, nested groups included): the
    /// same photos can enter or leave the set without any catalog change.
    pub fn depends_on_now(&self) -> bool {
        self.rules.iter().any(|r| match r {
            Rule::Group { group } => group.depends_on_now(),
            Rule::Field { op, .. } => op == "inLast" || op == "notInLast",
        })
    }

    /// Unknown fields or operators (for command validation), as readable messages.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for r in &self.rules {
            match r {
                Rule::Group { group } => out.extend(group.problems()),
                Rule::Field { field, op, .. } => match field_kind(field) {
                    None => out.push(format!("unknown field `{field}`")),
                    Some(k) if !ops_for(k).iter().any(|o| o.0 == op) => out.push(format!("`{field}` has no operator `{op}`")),
                    _ => {}
                },
            }
        }
        out
    }

    /// A short readable summary ("rating is ≥ 3 and keywords contains travel").
    pub fn describe(&self) -> String {
        let join = match self.mode {
            Match::All => " and ",
            Match::Any => " or ",
            Match::None => " nor ",
        };
        let parts: Vec<String> = self
            .rules
            .iter()
            .map(|r| match r {
                Rule::Group { group } => format!("({})", group.describe()),
                Rule::Field { field, op, value } => {
                    let label = FIELDS.iter().find(|f| f.0 == field).map_or(field.as_str(), |f| f.1).to_lowercase();
                    let op = field_kind(field).and_then(|k| ops_for(k).iter().find(|o| o.0 == op)).map_or(op.as_str(), |o| o.1);
                    let v = match value {
                        Value::String(s) => s.clone(),
                        Value::Null => String::new(),
                        Value::Object(o) => format!(
                            "{} {}",
                            o.get("n").map(|n| n.to_string()).unwrap_or_default(),
                            o.get("unit").and_then(Value::as_str).unwrap_or("days")
                        ),
                        Value::Array(a) => a.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect::<Vec<_>>().join(" – "),
                        other => other.to_string(),
                    };
                    format!("{label} {op} {v}").trim().to_string()
                }
            })
            .collect();
        let s = parts.join(join);
        if self.mode == Match::None { format!("none of: {s}") } else { s }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorLabel, Flag, PhotoId, Source};
    use serde_json::json;

    fn photo(id: u64) -> Photo {
        let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, "IMG_0042.CR2", "CR2", 6000, 4000, "2026-09-20T10:00:00");
        p.rating = 4;
        p.flag = Flag::Pick;
        p.label = Some(ColorLabel::Red);
        p.captured = Some("2026-08-14T18:30:00".into());
        p.meta.keywords = vec!["travel|italy|rome".into(), "food".into()];
        p.meta.camera = "Model X2".into();
        p.meta.iso = Some(1600);
        p.meta.aperture = Some(2.8);
        p
    }

    fn rs(v: serde_json::Value) -> RuleSet {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn fields_ops_and_groups() {
        let cat = Catalog::new();
        let p = photo(1);
        let yes = |v: serde_json::Value| assert!(rs(v.clone()).matches(&p, &cat), "{v}");
        let no = |v: serde_json::Value| assert!(!rs(v.clone()).matches(&p, &cat), "{v}");
        yes(json!({"rules": [{"field": "rating", "op": "gte", "value": 3}, {"field": "flag", "op": "is", "value": "pick"}]}));
        no(json!({"rules": [{"field": "rating", "op": "gt", "value": 4}]}));
        yes(json!({"match": "any", "rules": [{"field": "rating", "op": "is", "value": 1}, {"field": "label", "op": "is", "value": "red"}]}));
        no(json!({"match": "none", "rules": [{"field": "label", "op": "is", "value": "red"}]}));
        yes(json!({"rules": [{"field": "keywords", "op": "is", "value": "italy"}]}));
        yes(
            json!({"rules": [{"field": "keywords", "op": "contains", "value": "ROME"}, {"field": "keywords", "op": "notContains", "value": "beach"}]}),
        );
        no(json!({"rules": [{"field": "keywords", "op": "isEmpty"}]}));
        yes(json!({"rules": [{"field": "fileName", "op": "startsWith", "value": "img_"}, {"field": "fileName", "op": "endsWith", "value": ".cr2"}]}));
        yes(json!({"rules": [{"field": "camera", "op": "contains", "value": "x2"}, {"field": "title", "op": "isEmpty"}]}));
        yes(json!({"rules": [{"field": "iso", "op": "between", "value": [800, 3200]}, {"field": "aperture", "op": "lte", "value": "f/4"}]}));
        yes(json!({"rules": [{"field": "megapixels", "op": "gte", "value": 24}]}));
        yes(
            json!({"rules": [{"field": "captureDate", "op": "is", "value": "2026-08"}, {"field": "captureDate", "op": "before", "value": "2026-09-01"}]}),
        );
        yes(json!({"rules": [{"field": "captureDate", "op": "between", "value": ["2026-08-01", "2026-08"]}]}));
        no(json!({"rules": [{"field": "captureDate", "op": "after", "value": "2026-08"}]}));
        yes(json!({"rules": [{"field": "kind", "op": "isNot", "value": "video"}, {"field": "edited", "op": "is", "value": false}]}));
        // copyright status (unknown until set)
        yes(json!({"rules": [{"field": "copyrightStatus", "op": "is", "value": "unknown"}]}));
        no(json!({"rules": [{"field": "copyrightStatus", "op": "is", "value": "copyrighted"}]}));
        let mut pd = photo(2);
        pd.meta.copyright_status = crate::CopyrightStatus::PublicDomain;
        assert!(rs(json!({"rules": [{"field": "copyrightStatus", "op": "is", "value": "publicDomain"}]})).matches(&pd, &cat));
        assert!(rs(json!({"rules": [{"field": "copyrightStatus", "op": "isNot", "value": "copyrighted"}]})).matches(&pd, &cat));
        // nested: rating ≥ 4 and (label is blue or keywords contain food)
        yes(json!({"rules": [{"field": "rating", "op": "gte", "value": 4}, {"group": {"match": "any", "rules": [
            {"field": "label", "op": "is", "value": "blue"}, {"field": "keywords", "op": "contains", "value": "food"}]}}]}));
        // in the last N days, relative to now
        set_now(Some("2026-09-01T00:00:00".into()));
        yes(json!({"rules": [{"field": "captureDate", "op": "inLast", "value": {"n": 30, "unit": "days"}}]}));
        no(json!({"rules": [{"field": "captureDate", "op": "inLast", "value": {"n": 1, "unit": "weeks"}}]}));
        yes(json!({"rules": [{"field": "captureDate", "op": "notInLast", "value": 7}]}));
        set_now(None);
        // an empty rule list: all → everything, any → nothing
        yes(json!({"rules": []}));
        no(json!({"match": "any", "rules": []}));
    }

    #[test]
    fn problems_and_description() {
        let r = rs(json!({"rules": [{"field": "rating", "op": "contains", "value": 1}, {"group": {"rules": [{"field": "nope", "op": "is"}]}}]}));
        assert_eq!(r.problems(), vec!["`rating` has no operator `contains`".to_string(), "unknown field `nope`".to_string()]);
        let r = rs(
            json!({"match": "any", "rules": [{"field": "rating", "op": "gte", "value": 3}, {"field": "captureDate", "op": "inLast", "value": {"n": 2, "unit": "weeks"}}]}),
        );
        assert_eq!(r.describe(), "rating is ≥ 3 or capture date is in the last 2 weeks");
        // every field has a kind with operators
        for (f, _, k) in FIELDS {
            assert!(!ops_for(*k).is_empty(), "{f}");
        }
    }
}
