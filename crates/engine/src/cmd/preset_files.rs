//! Preset files: import `.lcpreset`, XMP, `.lrtemplate`, DNG-preset, Luminar looks and zip bundles; export `.lcpreset`.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, str_param};
use crate::preset_import::{group_from_dir, read_presets};
use crate::presets::{LCPRESET_EXT, expand_preset_paths, to_lcpreset};
use crate::{Result, Session};

fn strings(p: &Value, key: &str) -> Option<Vec<String>> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
}

fn import(s: &mut Session, p: &Value) -> Result<Value> {
    let paths = strings(p, "paths").filter(|v| !v.is_empty()).ok_or_else(|| bad("preset.import", "no paths"))?;
    let group = str_param(p, "group").map(str::trim).filter(|g| !g.is_empty()).map(str::to_string);
    let dry = p.get("dryRun").and_then(Value::as_bool).unwrap_or(false);
    let mut imported = Vec::new();
    let mut failed = Vec::new();
    let mut skipped = 0usize;
    for top in &paths {
        let from_dir = std::path::Path::new(top).is_dir();
        for f in expand_preset_paths(std::slice::from_ref(top)) {
            // presets in a folder go to a group named after it (unless the file names its own)
            // the path from the imported folder (inclusive) to the file's folder
            let base = std::path::Path::new(top).parent().unwrap_or(std::path::Path::new(""));
            let rel = std::path::Path::new(&f).parent().and_then(|d| d.strip_prefix(base).ok()).map(|d| d.to_string_lossy().to_string());
            let dir_group = rel.filter(|_| from_dir).and_then(|d| group_from_dir(&d));
            let parsed = std::fs::read(&f).map_err(|e| e.to_string()).and_then(|b| read_presets(&f, &b, dir_group));
            match parsed {
                Ok(list) => {
                    for mut it in list {
                        if let Some(g) = &group {
                            it.preset.group = g.clone();
                        }
                        let (name, grp) = (it.preset.name.clone(), it.preset.group.clone());
                        let id = if dry { Some(it.preset.id.clone()) } else { s.add_presets(vec![it.preset]).pop() };
                        match id {
                            Some(id) => imported.push(json!({"id": id, "name": name, "group": grp, "file": f, "unmapped": it.unmapped})),
                            None => skipped += 1,
                        }
                    }
                }
                Err(e) => failed.push(json!([f, e])),
            }
        }
    }
    Ok(json!({"imported": imported, "skipped": skipped, "failed": failed}))
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad("preset.export", "missing path"))?;
    let mut path = std::path::PathBuf::from(path);
    if path.extension().is_none() {
        path.set_extension(LCPRESET_EXT);
    }
    let ids = strings(p, "ids");
    let group = str_param(p, "group");
    let chosen: Vec<_> = s
        .presets
        .iter()
        .filter(|x| match (&ids, group) {
            (Some(ids), _) => ids.contains(&x.id),
            (None, Some(g)) => x.group == g,
            (None, None) => !x.builtin,
        })
        .cloned()
        .collect();
    if chosen.is_empty() {
        return Err(bad("preset.export", "no presets to export (create one first, or pass ids/group)"));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| crate::EngineError::Other(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(&path, to_lcpreset(&chosen)).map_err(|e| crate::EngineError::Other(format!("{}: {e}", path.display())))?;
    Ok(json!({"path": path.display().to_string(), "count": chosen.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "preset.import",
            "Import Presets",
            [],
            None,
            "{paths: [file or folder], group?, dryRun?} — .lcpreset, XMP presets, .lrtemplate, photos carrying edits (DNG/JPEG/TIFF \"DNG presets\"), Luminar looks (.lmp, .mplumpack collections) and .zip bundles of these; folders give their name as group; crs: fields mapped per docs/xmp-interop.md → {imported: [{id,name,group,file,unmapped}], skipped, failed}",
            always,
            import
        ),
        cmd!(
            "preset.export",
            "Export Presets",
            [],
            None,
            "{path, ids?: [presetId], group?: name} (default: all user presets) → {path, count}; writes a .lcpreset JSON file",
            always,
            export
        ),
    ]
}
