//! Before / After: what the "before" side shows. By default a photo's import state (camera
//! defaults plus the import preset, see [`Photo::import_defaults`]); it can be set from the
//! current settings, a history step or a version, and copied or swapped with the current
//! settings. The before side always uses the current crop and orientation so the two line up.

use std::sync::Arc;

use lightcraft_catalog::Photo;
use lightcraft_develop::DevelopSettings;
use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, has_active};
use crate::{Result, Session};

impl Session {
    /// The settings the "before" side of photo `p` renders with.
    pub fn before_settings(&self, p: &Photo) -> DevelopSettings {
        let base = self.before.get(&p.id).map(|b| (**b).clone()).unwrap_or_else(|| p.import_defaults());
        DevelopSettings { crop: p.develop.crop, orientation: p.develop.orientation, ..base }
    }
}

fn active(s: &Session, c: &str) -> Result<lightcraft_catalog::PhotoId> {
    s.active().ok_or_else(|| bad(c, "no active photo"))
}

fn state(s: &Session) -> Result<Value> {
    let custom = s.active().is_some_and(|id| s.before.contains_key(&id));
    Ok(json!({"before": if custom { "custom" } else { "import" }}))
}

/// `{source: import | current | history | version, index?, name?}`
fn set_before(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "beforeAfter.setBefore";
    let id = active(s, C)?;
    let photo = s.catalog.photo(id).ok_or_else(|| bad(C, "no photo"))?;
    let settings: Option<Arc<DevelopSettings>> = match p.get("source").and_then(Value::as_str).unwrap_or("current") {
        "import" | "original" => None,
        "current" => Some(photo.develop.clone()),
        "history" => {
            let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(C, "history needs `index`"))? as usize;
            Some(photo.history.get(i).ok_or_else(|| bad(C, format!("no history step {i}")))?.settings.clone())
        }
        "version" => {
            let name = p.get("name").and_then(Value::as_str).ok_or_else(|| bad(C, "version needs `name`"))?;
            Some(photo.versions.iter().find(|v| v.name == name).ok_or_else(|| bad(C, format!("no version `{name}`")))?.settings.clone())
        }
        other => return Err(bad(C, format!("unknown source `{other}` (import, current, history, version)"))),
    };
    match settings {
        Some(b) => s.before.insert(id, b),
        None => s.before.remove(&id),
    };
    state(s)
}

/// Copy the before settings to the photo (an edit, undoable); `swap` also makes the old
/// current settings the new before.
fn before_to_after(s: &mut Session, swap: bool) -> Result<Value> {
    let c = if swap { "beforeAfter.swap" } else { "beforeAfter.copyBeforeToAfter" };
    let id = active(s, c)?;
    let photo = s.catalog.photo(id).ok_or_else(|| bad(c, "no photo"))?;
    let after = photo.develop.clone();
    let before = s.before.get(&id).map(|b| (**b).clone()).unwrap_or_else(|| photo.import_defaults());
    s.set_develop(id, before, if swap { "Swap Before and After" } else { "Copy Before to After" })?;
    if swap {
        s.before.insert(id, after);
    }
    state(s)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "beforeAfter.setBefore",
            "Set Before",
            [],
            None,
            "{source: import|current|history|version, index? (history step), name? (version)} — what the before side shows (this session)",
            has_active,
            set_before
        ),
        cmd!(
            "beforeAfter.copyAfterToBefore",
            "Copy After's Settings to Before",
            ["View", "Before/After Settings"],
            None,
            "{}",
            has_active,
            |s, _| { set_before(s, &json!({"source": "current"})) }
        ),
        cmd!(
            "beforeAfter.copyBeforeToAfter",
            "Copy Before's Settings to After",
            ["View", "Before/After Settings"],
            None,
            "{}",
            has_active,
            |s, _| { before_to_after(s, false) }
        ),
        cmd!("beforeAfter.swap", "Swap Before and After Settings", ["View", "Before/After Settings"], None, "{}", has_active, |s, _| {
            before_to_after(s, true)
        }),
        cmd!("beforeAfter.resetBefore", "Reset Before to Import State", ["View", "Before/After Settings"], None, "{}", has_active, |s, _| {
            set_before(s, &json!({"source": "import"}))
        }),
    ]
}
