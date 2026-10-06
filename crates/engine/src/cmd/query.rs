//! Read-only queries (not journaled): catalog, photos, develop state, controls, presets, albums.

use lightcraft_catalog::{Album, Photo, PhotoId};
use lightcraft_develop::{CONTROLS, controls};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_active};
use crate::Session;

pub fn photo_summary(p: &Photo) -> Value {
    json!({
        "id": p.id.0,
        "fileName": p.file_name,
        "format": p.format,
        "kind": p.kind,
        "width": p.width,
        "height": p.height,
        "captured": p.captured,
        "rating": p.rating,
        "flag": p.flag,
        "label": p.label,
        "edited": p.is_edited(),
        "title": p.meta.title,
        "keywords": p.meta.keywords,
        "camera": p.meta.camera,
        "deleted": p.deleted,
        "copyOf": p.copy_of.map(|c| c.0),
        "copyName": p.copy_name,
        // an undecodable raw variant, shown and edited from its embedded JPEG: why
        "previewOnly": p.preview_only,
    })
}

fn album_json(a: &Album, all: &[Album], cat: &lightcraft_catalog::Catalog) -> Value {
    let mut v = json!({
        "id": a.id.0,
        "name": a.name,
        "folder": a.folder,
        "count": cat.album_count(a.id),
        "cover": a.cover.map(|c| c.0),
        "children": all.iter().filter(|c| c.parent == Some(a.id)).map(|c| album_json(c, all, cat)).collect::<Vec<_>>(),
    });
    if let Some(rules) = &a.smart {
        v["smart"] = json!(true);
        v["rules"] = serde_json::to_value(rules).unwrap_or_default();
        v["rulesText"] = json!(rules.describe());
    }
    v
}

fn photo_arg(s: &Session, p: &Value, c: &str) -> crate::Result<PhotoId> {
    p.get("id").and_then(Value::as_u64).map(PhotoId).or(s.active()).ok_or_else(|| bad(c, "no photo"))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "catalog.query", "Query Photos", [], None, "{filter?: Filter, sort?: Sort, offset?, limit?} — omit filter to list the current view", always, |s, p| {
            let ids = if p.get("filter").is_some() || p.get("sort").is_some() {
                let f = p.get("filter").map(|f| serde_json::from_value(f.clone())).transpose().map_err(|e| bad("catalog.query", e.to_string()))?.unwrap_or_default();
                let so = p.get("sort").map(|f| serde_json::from_value(f.clone())).transpose().map_err(|e| bad("catalog.query", e.to_string()))?.unwrap_or_default();
                s.catalog.query(&f, &so)
            } else {
                s.visible_cloned()
            };
            let off = p.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
            let lim = p.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize;
            let items: Vec<Value> = ids.iter().skip(off).take(lim).filter_map(|id| s.catalog.photo(*id)).map(|p| photo_summary(p)).collect();
            Ok(json!({"total": ids.len(), "photos": items}))
        }),
        cmd!(query "catalog.stats", "Catalog Statistics", [], None, "{}", always, |s, _| {
            let all: Vec<_> = s.catalog.photos().filter(|p| p.in_library()).collect();
            Ok(json!({
                "photos": all.len(),
                "edited": all.iter().filter(|p| p.is_edited()).count(),
                "picks": all.iter().filter(|p| p.flag == lightcraft_catalog::Flag::Pick).count(),
                "rejects": all.iter().filter(|p| p.flag == lightcraft_catalog::Flag::Reject).count(),
                "deleted": s.catalog.photos().filter(|p| p.deleted).count(),
                "albums": s.catalog.albums().filter(|a| !a.folder).count(),
                "byDate": s.catalog.date_groups(),
                "keywords": s.catalog.keywords(),
            }))
        }),
        cmd!(query "library.state", "Library State", [], None, "{}", always, |s, _| {
            let label = s.source.label(&s.catalog);
            let n = s.visible().len();
            Ok(json!({
                "source": s.source,
                "sourceLabel": label,
                "filter": s.filter,
                "sort": s.sort,
                "selection": s.selection,
                "visibleCount": n,
                "activeMask": s.active_mask,
                "activeSpot": s.active_spot,
                "undo": s.undo.last().map(|u| u.label.clone()),
                "redo": s.redo.last().map(|u| u.label.clone()),
                "clipboard": s.clipboard.is_some(),
            }))
        }),
        cmd!(query "photo.allMetadata", "All Metadata", [], None, "{id?} → {exif: [{group, tag, name, value}], xmp: [{name, value}]} — every EXIF / TIFF / GPS tag of the file and its XMP properties", always, |s, p| {
            let id = photo_arg(s, p, "photo.allMetadata")?;
            let ph = s.catalog.photo(id).ok_or_else(|| super::bad("photo.allMetadata", "no such photo"))?.clone();
            let lightcraft_catalog::Source::File { path } = &ph.source else {
                return Ok(json!({"exif": [], "xmp": [], "note": "a generated demo photo has no file"}));
            };
            let bytes = match &s.media.file_bytes {
                Some(r) => r(path),
                None => std::fs::read(path).map_err(|e| format!("{path}: {e}")),
            }
            .map_err(|e| super::bad("photo.allMetadata", e))?;
            let exif: Vec<Value> = lightcraft_meta::file_tag_rows(&bytes)
                .into_iter()
                .map(|r| json!({"group": r.group, "tag": r.tag, "name": r.name, "value": r.value}))
                .collect();
            // XMP: the sidecar if there is one, else the file's own packet
            let packet = crate::sidecar::read_packet(path, ph.kind, s.sidecar_naming(ph.id)).map(|(x, _)| x).or_else(|| lightcraft_meta::embedded(&bytes).xmp);
            let xmp: Vec<Value> = packet
                .and_then(|x| lightcraft_meta::parse_xmp(&x).ok())
                .map(|d| d.properties.into_iter().filter(|(k, _)| !k.starts_with("lc:")).map(|(k, v)| json!({"name": k, "value": v.join("; ")})).collect())
                .unwrap_or_default();
            Ok(json!({"exif": exif, "xmp": xmp}))
        }),
        cmd!(query "albums.list", "List Albums", [], None, "{}", always, |s, _| {
            let all: Vec<Album> = s.catalog.albums().cloned().collect();
            Ok(Value::Array(all.iter().filter(|a| a.parent.is_none()).map(|a| album_json(a, &all, &s.catalog)).collect()))
        }),
        cmd!(query "photo.inspect", "Inspect Photo", [], None, "{id?}", always, |s, p| {
            let id = photo_arg(s, p, "photo.inspect")?;
            let ph = s.catalog.photo(id).ok_or_else(|| bad("photo.inspect", "no such photo"))?;
            let mut v = serde_json::to_value(ph.as_ref()).unwrap_or_default();
            v["albums"] = json!(s.catalog.albums_of(id).iter().map(|a| a.0).collect::<Vec<_>>());
            v["history"] = json!(ph.history.iter().map(|h| h.label.clone()).collect::<Vec<_>>());
            if let Some(st) = s.catalog.stack_of(id) {
                v["stack"] = json!({
                    "id": st.id.0,
                    "photos": st.photos.iter().map(|p| p.0).collect::<Vec<_>>(),
                    "position": st.position(id),
                    "collapsed": st.collapsed,
                });
            }
            Ok(v)
        }),
        cmd!(query "develop.get", "Get Develop Settings", [], None, "{id?}", always, |s, p| {
            let id = photo_arg(s, p, "develop.get")?;
            Ok(s.develop_of(id).map(|d| d.to_json()).unwrap_or(Value::Null))
        }),
        cmd!(query "develop.controls", "List Develop Controls", [], None, "{section?} — every slider with range, default and current value", always, |s, p| {
            let d = s.active().and_then(|id| s.develop_of(id)).unwrap_or_default();
            let sec = p.get("section").and_then(|v| serde_json::from_value::<lightcraft_develop::Section>(v.clone()).ok());
            Ok(Value::Array(
                CONTROLS
                    .iter()
                    .filter(|c| sec.is_none_or(|x| x == c.section))
                    .map(|c| {
                        let mut v = serde_json::to_value(c).unwrap_or_default();
                        v["value"] = json!(controls::get(&d, c.id));
                        v
                    })
                    // indexed controls of the list elements that exist (`pointColor.0.hueShift`, …)
                    .chain(controls::indexed_instances(&d).into_iter().filter(|(_, c)| sec.is_none_or(|x| x == c.section)).map(|(id, c)| {
                        let mut v = serde_json::to_value(c).unwrap_or_default();
                        v["value"] = json!(controls::get(&d, &id));
                        v["id"] = json!(id);
                        v
                    }))
                    .collect(),
            ))
        }),
        cmd!(query "presets.list", "List Presets", [], None, "{}", always, |s, _| {
            Ok(Value::Array(s.presets.iter().map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": p.favorite, "builtin": p.builtin})).collect()))
        }),
        cmd!(query "profiles.list", "List Profiles", [], None, "{} — every profile with its group and favourite flag", always, |s, _| {
            Ok(Value::Array(
                crate::presets::PROFILES
                    .iter()
                    .map(|p| json!({"id": p.id, "name": p.name, "group": p.group, "favorite": s.profile_favorites.iter().any(|f| f == p.id)}))
                    .collect(),
            ))
        }),
        cmd!(query "profiles.menu", "Profile Menu", [], None, "{} — favourites, recent (newest first) and groups", always, |s, _| Ok(s.profile_menu())),
        cmd!(query "history.list", "List History", [], None, "{id?}", has_active, |s, p| {
            let id = photo_arg(s, p, "history.list")?;
            let ph = s.catalog.photo(id).ok_or_else(|| bad("history.list", "no such photo"))?;
            Ok(json!({
                "history": ph.history.iter().map(|h| h.label.clone()).collect::<Vec<_>>(),
                "versions": ph.versions.iter().map(|v| json!({"name": v.name, "created": v.created})).collect::<Vec<_>>(),
            }))
        }),
        cmd!(query "app.gpu", "GPU Rendering", [], None, "{enabled?: bool} — allow/forbid GPU rendering (CPU fallback; LIGHTCRAFT_GPU=0 forbids it for the process); returns {enabled, available, adapter, reason (why the GPU is off), lastFallback (latest render redone on the CPU, and why)}", always, |_, p| {
            if let Some(on) = p.get("enabled").and_then(Value::as_bool) {
                lightcraft_gpu::set_enabled(on);
            }
            Ok(json!({
                "enabled": lightcraft_gpu::enabled(),
                "available": lightcraft_gpu::available(),
                "adapter": lightcraft_gpu::adapter_name(),
                "reason": lightcraft_gpu::unavailable_reason(),
                "lastFallback": lightcraft_gpu::last_fallback(),
            }))
        }),
        cmd!(query "library.memory", "Memory Usage", [], None, "{} — bytes held by each cache (decoded sources, rendered previews, GPU buffers; heap when instrumented)", always, |s, _| {
            Ok(serde_json::to_value(s.memory_report()).unwrap_or_default())
        }),
        cmd!(query "app.memoryBudget", "Memory Budget", [], None, "{mb?: number} — set the memory budget shared by the caches (default: a quarter of RAM, at most 1536 MB); returns the report", always, |s, p| {
            if let Some(mb) = p.get("mb").and_then(Value::as_f64) {
                if !(64.0..=1_048_576.0).contains(&mb) {
                    return Err(bad("app.memoryBudget", "mb must be 64..1048576"));
                }
                s.set_memory_budget((mb * 1048576.0) as usize);
            }
            Ok(serde_json::to_value(s.memory_report()).unwrap_or_default())
        }),
        cmd!(query "journal.list", "Command Journal", [], None, "{limit?}", always, |s, p| {
            let lim = p.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
            let start = s.journal.len().saturating_sub(lim);
            Ok(Value::Array(s.journal[start..].iter().map(|(c, p)| json!({"command": c, "params": p})).collect()))
        }),
    ]
}
