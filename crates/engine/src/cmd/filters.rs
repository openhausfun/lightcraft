//! Filter presets: the filter bar's settings saved under a name ("Five stars", "Unedited raws")
//! and applied in one step. Saved with the library.

use lightcraft_catalog::Filter;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::{Result, Session};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FilterPreset {
    pub name: String,
    pub filter: Filter,
}

fn list(s: &Session) -> Value {
    json!(s.filter_presets)
}

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "filter.savePreset";
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "missing `name`"))?.to_string();
    if s.filter == Filter::default() {
        return Err(bad(C, "the filter is empty: set some filters first"));
    }
    let preset = FilterPreset { name: name.clone(), filter: s.filter.clone() };
    match s.filter_presets.iter_mut().find(|f| f.name.eq_ignore_ascii_case(&name)) {
        Some(f) => *f = preset,
        None => s.filter_presets.push(preset),
    }
    s.save_prefs()?;
    Ok(list(s))
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "filter.applyPreset";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let f = s.filter_presets.iter().find(|f| f.name.eq_ignore_ascii_case(name)).ok_or_else(|| bad(C, format!("no filter preset `{name}`")))?;
    s.filter = f.filter.clone();
    Ok(json!({"photos": s.visible().len()}))
}

fn delete(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "filter.deletePreset";
    let name = str_param(p, "name").ok_or_else(|| bad(C, "missing `name`"))?;
    let n = s.filter_presets.len();
    s.filter_presets.retain(|f| !f.name.eq_ignore_ascii_case(name));
    if n == s.filter_presets.len() {
        return Err(bad(C, format!("no filter preset `{name}`")));
    }
    s.save_prefs()?;
    Ok(list(s))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "filter.presets", "Filter Presets", [], None, "{} → [{name, filter}]", always, |s, _| Ok(list(s))),
        cmd!(
            "filter.savePreset",
            "Save Filter Preset",
            [],
            None,
            "{name} — the current filter (library.filter), saved with the library",
            always,
            save
        ),
        cmd!("filter.applyPreset", "Apply Filter Preset", [], None, "{name} — replaces the current filter → {photos}", always, apply),
        cmd!("filter.deletePreset", "Delete Filter Preset", [], None, "{name}", always, delete),
    ]
}
