//! XMP sidecar commands: save/read metadata to/from files, sidecar preferences.

use lightcraft_catalog::Op;
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_selection, str_param};
use crate::sidecar::SidecarNaming;
use crate::{Result, Session};

fn save(s: &mut Session, p: &Value) -> Result<Value> {
    let mut written = Vec::new();
    let mut merged = Vec::new();
    let mut backups = Vec::new();
    let mut failed = Vec::new();
    let owners = crate::sidecar::StemOwners::of(&s.catalog);
    for id in s.targets(p) {
        match s.save_sidecar_with(id, &owners) {
            Ok(r) => {
                let path = r.path.display().to_string();
                if r.merged {
                    merged.push(path.clone());
                }
                if let Some(b) = r.backup {
                    backups.push(json!({"path": path, "backup": b.display().to_string()}));
                }
                written.push(path);
            }
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    Ok(json!({"written": written, "merged": merged, "backups": backups, "failed": failed}))
}

fn read(s: &mut Session, p: &Value) -> Result<Value> {
    let mut ops = Vec::new();
    let mut read = Vec::new();
    let mut failed = Vec::new();
    for id in s.targets(p) {
        match s.read_sidecar_op(id) {
            Ok(Some((op, from))) => {
                ops.push(op);
                read.push(json!({"id": id.0, "from": from.display().to_string()}));
            }
            Ok(None) => failed.push(json!({"id": id.0, "error": "no XMP sidecar or embedded XMP"})),
            Err(e) => failed.push(json!({"id": id.0, "error": e.to_string()})),
        }
    }
    if !ops.is_empty() {
        s.commit("Read Metadata from File", Op::Batch { ops })?;
    }
    Ok(json!({"read": read, "failed": failed}))
}

fn prefs(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(a) = p.get("autoWrite").and_then(Value::as_bool) {
        s.xmp.auto_write = a;
    }
    if let Some(n) = str_param(p, "naming") {
        s.xmp.naming = match n {
            "stem" => SidecarNaming::Stem,
            "full" => SidecarNaming::Full,
            other => return Err(bad("library.xmpPreferences", format!("unknown naming `{other}` (stem|full)"))),
        };
    }
    if p.as_object().is_some_and(|o| !o.is_empty()) {
        s.save_prefs()?;
    }
    Ok(serde_json::to_value(s.xmp).unwrap_or_default())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "photo.saveMetadataToFile",
            "Save Metadata to File",
            ["Photo"],
            Some("Cmd+S"),
            "{ids?} — writes each photo's XMP sidecar (metadata + develop settings) next to the original, merged into an existing one (other apps' data kept; an unreadable one is backed up first) → {written: [path], merged: [path], backups: [{path, backup}], failed}",
            has_selection,
            save
        ),
        cmd!(
            "photo.readMetadataFromFile",
            "Read Metadata from File",
            ["Photo"],
            None,
            "{ids?} — reads each photo's XMP sidecar (or a raw/DNG's embedded XMP): metadata and develop settings, one undo step → {read, failed}",
            has_selection,
            read
        ),
        cmd!(
            "library.xmpPreferences",
            "XMP Sidecar Preferences",
            [],
            None,
            "{autoWrite?: bool, naming?: stem (IMG_1.xmp, default) | full (IMG_1.CR3.xmp)} → current preferences (saved with the library)",
            always,
            prefs
        ),
        cmd!("library.toggleAutoWriteXmp", "Automatically Write Changes into XMP", ["File"], None, "{} → {autoWrite}", always, |s, _| {
            let on = !s.xmp.auto_write;
            prefs(s, &json!({"autoWrite": on}))
        }),
    ]
}
