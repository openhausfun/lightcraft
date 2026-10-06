//! Library preferences (Settings → Import / Performance): develop defaults applied on import and
//! the thumbnail cache budget. Saved in the library's `prefs.json` (see [`crate::library`]).

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd};
use crate::import::{CameraDefault, ImportDefaults};
use crate::{Result, Session};

const ID: &str = "library.preferences";

fn current(s: &Session) -> Value {
    json!({
        "xmp": s.xmp,
        "import": s.import_defaults,
        "cacheMb": if s.cache_mb == 0 { s.cache_bytes() >> 20 } else { u64::from(s.cache_mb) },
        "forgetLocalDays": s.forget_local_days,
        "persistent": s.library.is_some(),
    })
}

/// `"default"`, `""` or null = no preset; anything else must name an existing preset.
fn preset_ref(s: &Session, v: &Value) -> Result<Option<String>> {
    match v {
        Value::Null => Ok(None),
        Value::String(x) if x.is_empty() || x == "default" => Ok(None),
        Value::String(x) if s.presets.iter().any(|p| p.id == *x) => Ok(Some(x.clone())),
        Value::String(x) => Err(bad(ID, format!("unknown preset `{x}` (see preset.list)"))),
        other => Err(bad(ID, format!("preset must be a preset id or \"default\", not {other}"))),
    }
}

fn prefs(s: &mut Session, p: &Value) -> Result<Value> {
    let mut d: ImportDefaults = s.import_defaults.clone();
    let mut changed = false;
    if let Some(i) = p.get("import") {
        if let Some(v) = i.get("rawPreset") {
            d.raw_preset = preset_ref(s, v)?;
        }
        if let Some(v) = i.get("otherPreset") {
            d.other_preset = preset_ref(s, v)?;
        }
        if let Some(v) = i.get("perCamera").and_then(Value::as_bool) {
            d.per_camera = v;
        }
        if let Some(v) = i.get("copyright").and_then(Value::as_str) {
            d.copyright = v.trim().to_string();
        }
        if let Some(v) = i.get("creator").and_then(Value::as_str) {
            d.creator = v.trim().to_string();
        }
        match i.get("metadataPreset") {
            Some(Value::String(n)) if n.is_empty() || n == "none" => d.metadata_preset = None,
            Some(Value::String(n)) => {
                if !s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(n)) {
                    return Err(bad(ID, format!("unknown metadata preset `{n}` (see metadata.presets)")));
                }
                d.metadata_preset = Some(n.clone());
            }
            Some(Value::Null) => d.metadata_preset = None,
            _ => {}
        }
        if let Some(a) = i.get("cameras").and_then(Value::as_array) {
            d.cameras.clear();
            for c in a {
                let camera = c.get("camera").and_then(Value::as_str).unwrap_or_default().trim().to_string();
                if camera.is_empty() {
                    return Err(bad(ID, "each camera default needs `camera` (make + model)"));
                }
                let preset = preset_ref(s, c.get("preset").unwrap_or(&Value::Null))?;
                d.cameras.push(CameraDefault { camera, preset });
            }
        }
        changed = true;
    }
    // set or clear one camera's default: {camera: {camera: "Make Model", preset: id|null}}
    if let Some(c) = p.get("camera") {
        let camera = c.get("camera").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if camera.is_empty() {
            return Err(bad(ID, "`camera.camera` (make + model) is required"));
        }
        let preset = preset_ref(s, c.get("preset").unwrap_or(&Value::Null))?;
        d.cameras.retain(|x| !x.camera.eq_ignore_ascii_case(&camera));
        if c.get("remove").and_then(Value::as_bool) != Some(true) {
            d.cameras.push(CameraDefault { camera, preset });
        }
        changed = true;
    }
    s.import_defaults = d;
    if let Some(v) = p.get("forgetLocalDays") {
        let days = v.as_u64().ok_or_else(|| bad(ID, "forgetLocalDays must be a whole number of days (0 = never)"))?;
        s.forget_local_days = days.min(36_500) as u32;
        changed = true;
    }
    if let Some(mb) = p.get("cacheMb") {
        let mb = mb.as_u64().ok_or_else(|| bad(ID, "cacheMb must be a number of megabytes"))?;
        s.set_cache_mb(mb.clamp(0, 1 << 20) as u32)?;
    } else if changed {
        s.save_prefs()?;
    }
    Ok(current(s))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "library.preferences",
        "Library Preferences",
        [],
        None,
        "{import?: {rawPreset?: presetId|\"default\", otherPreset?: presetId|\"default\", perCamera?: bool, cameras?: [{camera: \"Make Model\", preset: presetId|null}], copyright?: text, creator?: text (given to imported photos without one)}, camera?: {camera, preset?, remove?: bool}, cacheMb?: n (0 = default), forgetLocalDays?: n (forget untouched Local photos of folders not browsed for n days; 0 = never)} — develop defaults applied on import (raws / other images / per camera) and the thumbnail cache size, saved with the library → {xmp, import, cacheMb, forgetLocalDays, persistent}",
        always,
        prefs
    )]
}
