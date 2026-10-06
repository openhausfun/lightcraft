//! Library commands: view source, filter/sort, selection, ratings/flags/labels, rotate, delete,
//! metadata, albums, import.

use lightcraft_catalog::{Album, AlbumId, ColorLabel, CopyrightStatus, Flag, GroupBy, Op, PhotoId, Sort, SortKey};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, bool_or, cmd, has_active, has_selection, ok, str_param};
use crate::{LibrarySource, Result, Selection, Session};

/// The auto-import folder's visible files with their sizes (the file-system half of
/// `library.autoImportScan`; the app lists on a worker thread and passes `listing`).
pub fn list_auto_import_folder(folder: &str) -> std::result::Result<Vec<(String, u64)>, String> {
    let rd = std::fs::read_dir(folder).map_err(|e| format!("{folder}: {e}"))?;
    Ok(rd
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let m = std::fs::metadata(e.path()).ok()?;
            m.is_file().then(|| (e.path().to_string_lossy().to_string(), m.len()))
        })
        .collect())
}

/// `library.import`'s params, checked (no file-system calls).
pub struct ImportRequest {
    pub paths: Vec<String>,
    pub opts: crate::import::ImportOptions,
    pub album: Option<u64>,
    pub album_name: Option<String>,
}

/// Parse and check `library.import`'s params (the app's import task runs the import itself, on a
/// worker thread, with the same options).
pub fn import_params(s: &Session, p: &Value) -> Result<ImportRequest> {
    let paths = strs(p, "paths");
    let mode = match str_param(p, "mode").unwrap_or("add") {
        "add" => crate::import::ImportMode::Add,
        "copy" => crate::import::ImportMode::Copy,
        "move" => crate::import::ImportMode::Move,
        other => return Err(bad("library.import", format!("unknown mode `{other}` (add|copy|move)"))),
    };
    let preset = match str_param(p, "preset").filter(|x| !x.is_empty()) {
        Some(id) => Some(s.presets.iter().find(|x| x.id == id).cloned().ok_or_else(|| bad("library.import", format!("unknown preset `{id}`")))?),
        None => None,
    };
    let album = p.get("album").and_then(Value::as_u64);
    if let Some(a) = album
        && s.catalog.album(AlbumId(a)).is_none_or(|al| al.folder || al.is_smart())
    {
        return Err(bad("library.import", "album must be a regular album"));
    }
    let organize = match str_param(p, "organize") {
        Some(o) => crate::import::Organize::parse(o).ok_or_else(|| {
            bad("library.import", format!("unknown organize `{o}` (date|month|flat, or a folder template like {{date:%Y}}/{{date:%Y%m%d}})"))
        })?,
        None => Default::default(),
    };
    if let crate::import::Organize::Template(t) = &organize
        && let Some(e) = crate::rename::folder_template_error(t)
    {
        return Err(bad("library.import", e));
    }
    let metadata_preset = str_param(p, "metadataPreset").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    if let Some(n) = &metadata_preset
        && !s.metadata_presets.iter().any(|m| m.name.eq_ignore_ascii_case(n))
    {
        return Err(bad("library.import", format!("unknown metadata preset `{n}`")));
    }
    let opts = crate::import::ImportOptions {
        mode,
        preset,
        keywords: strs(p, "keywords"),
        destination: str_param(p, "destination").map(str::to_string),
        organize,
        rename: str_param(p, "rename").map(str::to_string),
        rename_start: p.get("renameStart").and_then(Value::as_u64).unwrap_or(1) as usize,
        metadata_preset,
        convert_dng: bool_or(p, "dng", false),
        local: bool_or(p, "local", false),
    };
    let album_name = str_param(p, "albumName").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    Ok(ImportRequest { paths, opts, album, album_name })
}

/// After a batch of an import was committed: the album (a new one named `album_name` when there
/// is no `album` and something was imported) gets the photos. Returns the report as JSON (with
/// `album`).
pub fn import_batch_done(s: &mut Session, report: crate::import::ImportReport, mut album: Option<u64>, album_name: Option<&str>) -> Result<Value> {
    let mut report = serde_json::to_value(report).unwrap_or_default();
    let imported: Vec<u64> = report["imported"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
    if album.is_none()
        && let Some(name) = album_name
        && !imported.is_empty()
    {
        let r = s.execute("album.create", &json!({"name": name}))?;
        album = r["id"].as_u64();
    }
    if let Some(a) = album
        && !imported.is_empty()
    {
        // (album.addPhotos wants a selection: the new photos are about to be it)
        s.selection = Selection::single(PhotoId(imported[0]));
        s.execute("album.addPhotos", &json!({"id": a, "ids": imported}))?;
        report["album"] = json!(a);
    }
    Ok(report)
}

fn album_param(p: &Value, key: &str, c: &str) -> Result<AlbumId> {
    p.get(key).and_then(Value::as_u64).map(AlbumId).ok_or_else(|| bad(c, format!("missing album `{key}`")))
}

fn strs(p: &Value, key: &str) -> Vec<String> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn ids_param(p: &Value) -> Option<Vec<PhotoId>> {
    p.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(PhotoId).collect())
}

/// Apply one op per target as a single undo step.
fn for_targets(s: &mut Session, p: &Value, label: &str, f: impl Fn(PhotoId) -> Option<Op>) -> Result<Value> {
    let targets = s.targets(p);
    let ops: Vec<Op> = targets.iter().filter_map(|id| f(*id)).collect();
    let n = ops.len();
    if n > 0 {
        s.commit(label, Op::Batch { ops })?;
    }
    Ok(json!({"changed": n}))
}

fn advance_if(s: &mut Session, p: &Value) {
    if bool_or(p, "advance", false) {
        let _ = step(s, 1);
    }
}

fn step(s: &mut Session, d: isize) -> Result<Value> {
    let vis = s.visible_cloned();
    if vis.is_empty() {
        return ok();
    }
    let cur = s.selection.active.and_then(|a| vis.iter().position(|x| *x == a));
    let next = match cur {
        Some(i) => (i as isize + d).clamp(0, vis.len() as isize - 1) as usize,
        None => 0,
    };
    s.end_interaction()?;
    s.selection = Selection::single(vis[next]);
    s.active_mask = None;
    s.active_spot = None;
    Ok(json!({"active": vis[next].0}))
}

fn rotate(s: &mut Session, p: &Value, cw: bool) -> Result<Value> {
    let targets = s.targets(p);
    let mut ops = Vec::new();
    for id in targets {
        if let Some(d) = s.develop_of(id) {
            let mut d = (*d).clone();
            d.orientation = d.orientation.rotated(cw);
            ops.extend(s.develop_op(id, d, "Rotate"));
        }
    }
    let n = ops.len();
    s.commit(if cw { "Rotate Right" } else { "Rotate Left" }, Op::Batch { ops })?;
    Ok(json!({"changed": n}))
}

fn flip(s: &mut Session, p: &Value, horizontal: bool) -> Result<Value> {
    let targets = s.targets(p);
    let mut ops = Vec::new();
    for id in targets {
        if let Some(d) = s.develop_of(id) {
            let mut d = (*d).clone();
            if horizontal {
                d.crop.flip_h = !d.crop.flip_h;
            } else {
                d.crop.flip_v = !d.crop.flip_v;
            }
            ops.extend(s.develop_op(id, d, "Flip"));
        }
    }
    let n = ops.len();
    s.commit("Flip", Op::Batch { ops })?;
    Ok(json!({"changed": n}))
}

/// `library.selectBy`: select the photos in view that match every given criterion.
fn select_by(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.selectBy";
    let flag: Option<lightcraft_catalog::Flag> = match p.get("flag") {
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| bad(C, "flag is pick, reject or none"))?),
        None => None,
    };
    let rating = p.get("rating").and_then(Value::as_u64).map(|r| r.min(5) as u8);
    let op = p.get("ratingOp").and_then(Value::as_str).unwrap_or("gte");
    let label: Option<Option<lightcraft_catalog::ColorLabel>> = match p.get("label").and_then(Value::as_str) {
        Some("none") => Some(None),
        Some(l) => Some(Some(serde_json::from_value(json!(l)).map_err(|_| bad(C, format!("unknown label `{l}`")))?)),
        None => None,
    };
    if flag.is_none() && rating.is_none() && label.is_none() {
        return Err(bad(C, "give flag, rating and/or label"));
    }
    let matches = |ph: &lightcraft_catalog::Photo| {
        flag.is_none_or(|f| ph.flag == f)
            && rating.is_none_or(|r| match op {
                "eq" => ph.rating == r,
                "lte" => ph.rating <= r,
                _ => ph.rating >= r,
            })
            && label.is_none_or(|l| ph.label == l)
    };
    let mut ids: Vec<PhotoId> = if bool_or(p, "add", false) { s.selection.ids.clone() } else { Vec::new() };
    for id in s.visible_cloned() {
        if !ids.contains(&id) && s.catalog.photo(id).is_some_and(|ph| matches(ph)) {
            ids.push(id);
        }
    }
    let active = s.selection.active.filter(|a| ids.contains(a)).or(ids.first().copied());
    let n = ids.len();
    s.selection = Selection { ids, active };
    Ok(json!({"selected": n}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        // ---- view source / filter / sort
        cmd!(
            "library.source",
            "Show Source",
            [],
            None,
            "{kind: all|recentlyAdded|album|recentlyDeleted|picks|missing, id?: albumId}",
            always,
            |s, p| {
                let kind = str_param(p, "kind").unwrap_or("all");
                s.source = match kind {
                    "all" => LibrarySource::All,
                    "recentlyAdded" => LibrarySource::RecentlyAdded,
                    "recentlyDeleted" => LibrarySource::RecentlyDeleted,
                    "picks" => LibrarySource::Picks,
                    "missing" => LibrarySource::Missing,
                    "album" => {
                        let a = album_param(p, "id", "library.source")?;
                        if s.catalog.album(a).is_none_or(|a| a.folder) {
                            return Err(bad("library.source", "no such album"));
                        }
                        LibrarySource::Album(a)
                    }
                    other => return Err(bad("library.source", format!("unknown source `{other}`"))),
                };
                let vis = s.visible_cloned();
                if s.selection.active.is_none_or(|a| !vis.contains(&a)) {
                    s.selection = vis.first().map(|f| Selection::single(*f)).unwrap_or_default();
                }
                Ok(json!({"count": vis.len()}))
            }
        ),
        cmd!(
            "library.filter",
            "Filter",
            [],
            None,
            "partial Filter: {text?, rating?, ratingOp?: atLeast|exactly|atMost, flag?: pick|reject|none|null, label?, kind?, merged?: hdr|panorama|hdrPanorama|any, edited?, date?, keyword?, camera?}",
            always,
            |s, p| {
                let mut v = serde_json::to_value(&s.filter).unwrap_or_default();
                lightcraft_develop::presets::deep_merge(&mut v, p);
                s.filter = serde_json::from_value(v).map_err(|e| bad("library.filter", e.to_string()))?;
                Ok(json!({"count": s.visible().len()}))
            }
        ),
        cmd!("library.clearFilter", "Clear Filters", ["View"], None, "{}", always, |s, _| {
            s.filter = Default::default();
            Ok(json!({"count": s.visible().len()}))
        }),
        cmd!(
            "library.sort",
            "Sort",
            ["View", "Sort"],
            None,
            "{key?: captureDate|importDate|editDate|fileName|rating|fileSize, ascending?: bool, group?: auto|none|day|month|year}",
            always,
            |s, p| {
                let key: SortKey = match p.get("key") {
                    Some(k) => serde_json::from_value(k.clone()).map_err(|e| bad("library.sort", e.to_string()))?,
                    None => s.sort.key,
                };
                let group = match str_param(p, "group") {
                    Some(g) => GroupBy::parse(g).ok_or_else(|| bad("library.sort", "group must be auto|none|day|month|year"))?,
                    None => s.sort.group,
                };
                s.sort = Sort { key, ascending: bool_or(p, "ascending", s.sort.ascending), group };
                ok()
            }
        ),
        cmd!(
            query "library.groups",
            "Date Groups",
            [],
            None,
            "{by?: day|month|year (default: the sort's grouping; auto = day)} → [{key, label, start, count}] date headers of the grid",
            always,
            |s, p| {
                let by = match str_param(p, "by") {
                    Some(g) => GroupBy::parse(g).ok_or_else(|| bad("library.groups", "by must be auto|none|day|month|year"))?,
                    None => s.sort.group,
                };
                let vis = s.visible_cloned();
                Ok(serde_json::to_value(s.catalog.date_runs(&vis, s.sort.key, by)).unwrap_or_default())
            }
        ),
        // ---- selection
        cmd!("library.select", "Select Photos", [], None, "{ids: [id], active?: id, mode?: replace|add|toggle|range}", always, |s, p| {
            let ids = ids_param(p).unwrap_or_default();
            let mode = str_param(p, "mode").unwrap_or("replace");
            s.end_interaction()?;
            match mode {
                "replace" => {
                    s.selection =
                        Selection { ids: ids.clone(), active: p.get("active").and_then(Value::as_u64).map(PhotoId).or(ids.first().copied()) };
                }
                "add" => {
                    for id in ids {
                        if !s.selection.contains(id) {
                            s.selection.ids.push(id);
                        }
                        s.selection.active = Some(id);
                    }
                }
                "toggle" => ids.into_iter().for_each(|id| s.selection.toggle(id)),
                "range" => {
                    let vis = s.visible_cloned();
                    if let Some(id) = ids.first() {
                        s.selection.extend_to(*id, &vis);
                    }
                }
                other => return Err(bad("library.select", format!("unknown mode `{other}`"))),
            }
            s.active_mask = None;
            s.active_spot = None;
            Ok(json!({"selected": s.selection.ids.len()}))
        }),
        cmd!("library.selectAll", "Select All", ["Edit"], Some("Cmd+A"), "{}", always, |s, _| {
            let vis = s.visible_cloned();
            s.selection = Selection { active: s.selection.active.filter(|a| vis.contains(a)).or(vis.first().copied()), ids: vis };
            Ok(json!({"selected": s.selection.ids.len()}))
        }),
        cmd!(query "library.devices", "Cameras and Cards", [], None, "{} → [{name, path (its DCIM folder), root}] — mounted volumes with a DCIM folder", always, |_, _| {
            Ok(serde_json::to_value(crate::devices::devices_now()).unwrap_or_default())
        }),
        cmd!(query "photo.copyMetadata", "Copy Metadata", ["Photo"], None, "{} — title, caption, copyright (notice, status, usage terms, info URL), creator, location and keywords of the active photo", has_active, |s, _| {
            let id = s.active().ok_or_else(|| bad("photo.copyMetadata", "no active photo"))?;
            let m = &s.catalog.photo(id).ok_or_else(|| bad("photo.copyMetadata", "no photo"))?.meta;
            let v = json!({"title": m.title, "caption": m.caption, "altText": m.alt_text, "extendedDescription": m.extended_description,
                "copyright": m.copyright, "copyrightStatus": m.copyright_status.id(), "usageTerms": m.usage_terms, "copyrightUrl": m.copyright_url,
                "creator": m.creator, "location": m.location, "city": m.city, "state": m.state, "country": m.country,
                "keywords": m.keywords});
            s.meta_clipboard = Some(v.clone());
            Ok(v)
        }),
        cmd!(
            "photo.pasteMetadata",
            "Paste Metadata",
            ["Photo"],
            None,
            "{ids?, fields?: [title|caption|copyright|copyrightStatus|usageTerms|copyrightUrl|creator|location|keywords] (default: all copied)}",
            has_selection,
            |s, p| {
                let c = "photo.pasteMetadata";
                let clip = s.meta_clipboard.clone().ok_or_else(|| bad(c, "nothing copied (photo.copyMetadata)"))?;
                let fields: Option<Vec<String>> =
                    p.get("fields").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
                let mut params = serde_json::Map::new();
                for (k, v) in clip.as_object().into_iter().flatten() {
                    if fields.as_ref().is_none_or(|f| f.iter().any(|x| x == k)) {
                        params.insert(k.clone(), v.clone());
                    }
                }
                if let Some(ids) = p.get("ids") {
                    params.insert("ids".into(), ids.clone());
                }
                s.execute("photo.setMeta", &Value::Object(params))
            }
        ),
        cmd!(
            "library.selectBy",
            "Select by Flag, Rating or Label",
            [],
            None,
            "{flag?: pick|reject|none, rating?: 0..5, ratingOp?: gte|eq|lte (default gte), label?: red|…|none, add?: bool (extend the selection)} — among the photos in view",
            always,
            select_by
        ),
        cmd!("library.selectNone", "Deselect All", ["Edit"], Some("Cmd+Shift+A"), "{}", always, |s, _| {
            let a = s.selection.active;
            s.selection = Selection { ids: a.into_iter().collect(), active: a };
            ok()
        }),
        cmd!("library.next", "Next Photo", [], Some("Right"), "{}", always, |s, _| step(s, 1)),
        cmd!("library.previous", "Previous Photo", [], Some("Left"), "{}", always, |s, _| step(s, -1)),
        // ---- rating / flags / labels
        cmd!("photo.rate", "Set Rating", ["Photo", "Set Rating"], None, "{rating: 0..5, ids?, advance?: bool}", has_selection, |s, p| {
            let r = super::f64_req(p, "rating", "photo.rate")? as i64;
            if !(0..=5).contains(&r) {
                return Err(bad("photo.rate", "rating must be 0..5"));
            }
            let v = for_targets(s, p, "Set Rating", |id| Some(Op::SetRating { id, rating: r as u8 }))?;
            advance_if(s, p);
            Ok(v)
        }),
        cmd!("photo.flag", "Set Flag", ["Photo", "Set Flag"], None, "{flag: pick|reject|none, ids?, advance?: bool}", has_selection, |s, p| {
            let f = str_param(p, "flag").and_then(Flag::parse).ok_or_else(|| bad("photo.flag", "flag must be pick|reject|none"))?;
            let v = for_targets(s, p, "Set Flag", |id| Some(Op::SetFlag { id, flag: f }))?;
            advance_if(s, p);
            Ok(v)
        }),
        cmd!("photo.pick", "Flag as Pick", ["Photo", "Set Flag"], Some("P"), "{ids?}", has_selection, |s, p| {
            for_targets(s, p, "Flag as Pick", |id| Some(Op::SetFlag { id, flag: Flag::Pick }))
        }),
        cmd!("photo.reject", "Flag as Reject", ["Photo", "Set Flag"], Some("X"), "{ids?}", has_selection, |s, p| {
            for_targets(s, p, "Flag as Reject", |id| Some(Op::SetFlag { id, flag: Flag::Reject }))
        }),
        cmd!("photo.unflag", "Unflag", ["Photo", "Set Flag"], Some("U"), "{ids?}", has_selection, |s, p| {
            for_targets(s, p, "Unflag", |id| Some(Op::SetFlag { id, flag: Flag::None }))
        }),
        cmd!(
            "photo.label",
            "Set Color Label",
            ["Photo", "Set Color Label"],
            None,
            "{label: red|yellow|green|blue|purple|none, ids?}",
            has_selection,
            |s, p| {
                let l = match str_param(p, "label") {
                    None | Some("none") => None,
                    Some(x) => Some(ColorLabel::parse(x).ok_or_else(|| bad("photo.label", "unknown label"))?),
                };
                for_targets(s, p, "Set Color Label", |id| Some(Op::SetLabel { id, label: l }))
            }
        ),
        // ---- orientation
        cmd!("photo.rotateLeft", "Rotate Left", ["Photo"], Some("Cmd+["), "{ids?}", has_selection, |s, p| rotate(s, p, false)),
        cmd!("photo.rotateRight", "Rotate Right", ["Photo"], Some("Cmd+]"), "{ids?}", has_selection, |s, p| rotate(s, p, true)),
        cmd!("photo.flipHorizontal", "Flip Horizontal", ["Photo"], None, "{ids?}", has_selection, |s, p| flip(s, p, true)),
        cmd!("photo.flipVertical", "Flip Vertical", ["Photo"], None, "{ids?}", has_selection, |s, p| flip(s, p, false)),
        // ---- delete / restore
        cmd!("photo.delete", "Delete Photo", ["Photo"], Some("Delete"), "{ids?} — moves to Recently Deleted", has_selection, |s, p| {
            let v = for_targets(s, p, "Delete", |id| Some(Op::SetDeleted { id, deleted: true }))?;
            let vis = s.visible_cloned();
            s.selection = vis.first().map(|f| Selection::single(*f)).unwrap_or_default();
            Ok(v)
        }),
        cmd!("photo.restore", "Restore", [], None, "{ids?}", has_selection, |s, p| for_targets(s, p, "Restore", |id| Some(Op::SetDeleted {
            id,
            deleted: false
        }))),
        cmd!("photo.deletePermanently", "Delete Permanently", [], None, "{ids?}", has_selection, |s, p| {
            let t = s.targets(p);
            let ops = t.iter().map(|id| s.catalog.delete_permanently_ops(*id)).collect::<Vec<_>>();
            s.commit("Delete Permanently", Op::Batch { ops })?;
            s.selection = Selection::default();
            Ok(json!({"deleted": t.len()}))
        }),
        // ---- metadata
        cmd!(
            "photo.setMeta",
            "Edit Info",
            [],
            None,
            "{ids?, title?, caption?, altText?, extendedDescription?, copyright?, copyrightStatus?: unknown|copyrighted|publicDomain, usageTerms?, copyrightUrl?, creator?, location?, city?, state?, country?, gps?: \"lat, lon\" | [lat, lon] | null, keywords?: [..], addKeywords?: [..], removeKeywords?: [..]}",
            has_selection,
            |s, p| {
                let strs =
                    |k: &str| p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                let targets = s.targets(p);
                // GPS: "lat, lon" (decimal or 51°30'26"N 0°7'39"W), [lat, lon], or null / "" to clear
                let gps: Option<Option<(f64, f64)>> = match p.get("gps") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(Value::String(t)) if t.trim().is_empty() => Some(None),
                    Some(Value::String(t)) => Some(Some(
                        parse_gps(t).ok_or_else(|| bad("photo.setMeta", format!("can't read `{t}` as coordinates (e.g. 51.5072, -0.1276)")))?,
                    )),
                    Some(Value::Array(a)) if a.len() == 2 => match (a[0].as_f64(), a[1].as_f64()) {
                        (Some(la), Some(lo)) if la.abs() <= 90.0 && lo.abs() <= 180.0 => Some(Some((la, lo))),
                        _ => return Err(bad("photo.setMeta", "gps is [latitude, longitude] in degrees")),
                    },
                    Some(_) => return Err(bad("photo.setMeta", "gps is \"lat, lon\", [lat, lon] or null")),
                };
                let status = match str_param(p, "copyrightStatus") {
                    Some(t) => Some(
                        CopyrightStatus::parse(t)
                            .ok_or_else(|| bad("photo.setMeta", format!("copyrightStatus `{t}`: unknown, copyrighted or publicDomain")))?,
                    ),
                    None => None,
                };
                let mut ops = Vec::new();
                for id in targets {
                    let Some(ph) = s.catalog.photo(id) else { continue };
                    let mut m = ph.meta.clone();
                    if let Some(g) = gps {
                        m.gps = g;
                    }
                    if let Some(st) = status {
                        m.copyright_status = st;
                    }
                    for (k, field) in [
                        ("title", &mut m.title),
                        ("caption", &mut m.caption),
                        ("copyright", &mut m.copyright),
                        ("usageTerms", &mut m.usage_terms),
                        ("copyrightUrl", &mut m.copyright_url),
                        ("creator", &mut m.creator),
                        ("location", &mut m.location),
                        ("city", &mut m.city),
                        ("state", &mut m.state),
                        ("country", &mut m.country),
                        ("altText", &mut m.alt_text),
                        ("extendedDescription", &mut m.extended_description),
                    ] {
                        if let Some(v) = str_param(p, k) {
                            *field = v.to_string();
                        }
                    }
                    if let Some(k) = strs("keywords") {
                        m.keywords = k;
                    }
                    for k in strs("addKeywords").unwrap_or_default() {
                        if !m.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                            m.keywords.push(k);
                        }
                    }
                    for k in strs("removeKeywords").unwrap_or_default() {
                        m.keywords.retain(|x| !x.eq_ignore_ascii_case(&k));
                    }
                    ops.push(Op::SetMeta { id, meta: Box::new(m) });
                }
                let n = ops.len();
                s.commit("Edit Info", Op::Batch { ops })?;
                // keywords just added become the Recent Keywords set
                let added: Vec<String> = strs("addKeywords").unwrap_or_default().into_iter().chain(strs("keywords").unwrap_or_default()).collect();
                if n > 0 && !added.is_empty() {
                    crate::cmd::keywords::note_recent(s, &added);
                }
                Ok(json!({"changed": n}))
            }
        ),
        // ---- albums
        cmd!("album.create", "New Album", ["File"], None, "{name, parent?: folderId, folder?: bool, addSelected?: bool}", always, |s, p| {
            let name = str_param(p, "name").unwrap_or("Untitled Album").trim().to_string();
            if name.is_empty() {
                return Err(bad("album.create", "empty name"));
            }
            let folder = bool_or(p, "folder", false);
            let parent = p.get("parent").and_then(Value::as_u64).map(AlbumId);
            let photos = if !folder && bool_or(p, "addSelected", false) { s.targets(&Value::Null) } else { vec![] };
            let id = s.catalog.alloc_album_id();
            let cover = photos.first().copied();
            s.commit(
                if folder { "New Folder" } else { "New Album" },
                Op::AddAlbum { album: Album { id, name, parent, folder, photos, cover, smart: None, quick: false } },
            )?;
            Ok(json!({"id": id.0}))
        }),
        cmd!(
            "album.createSmart",
            "New Smart Album…",
            [],
            None,
            "{name, rules?: partial Filter (rating, ratingOp, flag, label, kind, edited, keyword, camera, lens, dateFrom, dateTo, date, text, album, ruleSet: {match: all|any|none, rules: [{field, op, value} | {group: ruleSet}]} — see album.ruleFields), parent?: folderId} — without `rules`, saves the current view (source + filter)",
            always,
            |s, p| {
                let name = str_param(p, "name").unwrap_or("Smart Album").trim().to_string();
                if name.is_empty() {
                    return Err(bad("album.createSmart", "empty name"));
                }
                let rules = match p.get("rules") {
                    Some(r) => merge_rules(&lightcraft_catalog::Filter::default(), r, "album.createSmart")?,
                    None => view_rules(s),
                };
                let parent = p.get("parent").and_then(Value::as_u64).map(AlbumId);
                let id = s.catalog.alloc_album_id();
                let album = Album { parent, smart: Some(Box::new(rules)), ..Album::new(id, name) };
                s.commit("New Smart Album", Op::AddAlbum { album })?;
                Ok(json!({"id": id.0, "count": s.catalog.album_count(id)}))
            }
        ),
        cmd!(query "album.ruleFields", "Smart Album Rule Fields", [], None, "{} → [{field, label, kind, ops: [{op, label}], choices?}] for ruleSet rules", always, |_, _| {
            use lightcraft_catalog::rules::{FIELDS, Kind, ops_for};
            Ok(json!(FIELDS
                .iter()
                .map(|(id, label, kind)| {
                    let k = match kind {
                        Kind::Text => "text",
                        Kind::Keywords => "keywords",
                        Kind::Number => "number",
                        Kind::Date => "date",
                        Kind::Choice(_) => "choice",
                        Kind::Bool => "bool",
                    };
                    let mut v = json!({"field": id, "label": label, "kind": k, "ops": ops_for(*kind).iter().map(|(o, l)| json!({"op": o, "label": l})).collect::<Vec<_>>()});
                    if let Kind::Choice(c) = kind {
                        v["choices"] = json!(c);
                    }
                    v
                })
                .collect::<Vec<_>>()))
        }),
        cmd!(
            "album.setRules",
            "Edit Smart Album",
            [],
            None,
            "{id, rules?: partial Filter merged onto the current rules (null clears a field), replace?: bool, fromView?: bool (use the current view)}",
            always,
            |s, p| {
                let id = album_param(p, "id", "album.setRules")?;
                let cur = s.catalog.album(id).and_then(|a| a.smart.as_deref().cloned()).ok_or_else(|| bad("album.setRules", "not a smart album"))?;
                let rules = if bool_or(p, "fromView", false) {
                    view_rules(s)
                } else {
                    let base = if bool_or(p, "replace", false) { Default::default() } else { cur };
                    merge_rules(&base, p.get("rules").unwrap_or(&Value::Null), "album.setRules")?
                };
                s.commit("Edit Smart Album", Op::SetAlbumRules { id, rules: Box::new(rules) })?;
                Ok(json!({"count": s.catalog.album_count(id)}))
            }
        ),
        cmd!("album.rename", "Rename Album", [], None, "{id, name}", always, |s, p| {
            let id = album_param(p, "id", "album.rename")?;
            let name = str_param(p, "name").ok_or_else(|| bad("album.rename", "missing name"))?.to_string();
            s.commit("Rename Album", Op::RenameAlbum { id, name })?;
            ok()
        }),
        cmd!("album.delete", "Delete Album", [], None, "{id}", always, |s, p| {
            let id = album_param(p, "id", "album.delete")?;
            s.commit("Delete Album", Op::RemoveAlbum { id })?;
            if s.source == LibrarySource::Album(id) {
                s.source = LibrarySource::All;
            }
            ok()
        }),
        cmd!("album.move", "Move Album", [], None, "{id, parent?: folderId|null}", always, |s, p| {
            let id = album_param(p, "id", "album.move")?;
            let parent = p.get("parent").and_then(Value::as_u64).map(AlbumId);
            s.commit("Move Album", Op::MoveAlbum { id, parent })?;
            ok()
        }),
        cmd!("album.addPhotos", "Add to Album", ["Photo"], None, "{id: albumId, ids?: [photoIds]} (default: selection)", has_selection, |s, p| {
            let id = album_param(p, "id", "album.addPhotos")?;
            let targets = ids_param(p).unwrap_or_else(|| s.targets(&Value::Null));
            let al = s.catalog.album(id).ok_or_else(|| bad("album.addPhotos", "no such album"))?;
            if al.is_smart() || al.folder {
                return Err(bad("album.addPhotos", "smart albums and folders can't hold photos"));
            }
            let mut photos = al.photos.clone();
            let before = photos.len();
            for t in targets {
                if !photos.contains(&t) {
                    photos.push(t);
                }
            }
            let added = photos.len() - before;
            let cover = al.cover.or(photos.first().copied());
            s.commit("Add to Album", Op::Batch { ops: vec![Op::SetAlbumPhotos { id, photos }, Op::SetAlbumCover { id, cover }] })?;
            Ok(json!({"added": added}))
        }),
        cmd!(
            "album.toggleTarget",
            "Add to Target Album",
            [],
            None,
            "{ids?} — B: adds the photos to the target album (the Quick Collection unless one is set), or removes them when they're all in it → {album, added}",
            has_selection,
            |s, p| {
                let targets = ids_param(p).unwrap_or_else(|| s.targets(&Value::Null));
                let target = s.target_album.filter(|a| s.catalog.album(*a).is_some_and(|al| !al.is_smart() && !al.folder));
                let id = match target.or_else(|| s.catalog.quick_collection()) {
                    Some(id) => id,
                    None => {
                        let id = s.catalog.alloc_album_id();
                        let album = Album { quick: true, ..Album::new(id, "Quick Collection") };
                        s.commit("New Quick Collection", Op::AddAlbum { album })?;
                        id
                    }
                };
                let al = s.catalog.album(id).ok_or_else(|| bad("album.toggleTarget", "no target album"))?;
                let all_in = !targets.is_empty() && targets.iter().all(|t| al.photos.contains(t));
                let photos: Vec<PhotoId> = if all_in {
                    al.photos.iter().copied().filter(|x| !targets.contains(x)).collect()
                } else {
                    al.photos.iter().copied().chain(targets.iter().copied().filter(|t| !al.photos.contains(t))).collect()
                };
                let name = al.name.clone();
                let cover = al.cover.filter(|c| photos.contains(c)).or(photos.first().copied());
                s.commit(
                    if all_in { "Remove from Target Album" } else { "Add to Target Album" },
                    Op::Batch { ops: vec![Op::SetAlbumPhotos { id, photos }, Op::SetAlbumCover { id, cover }] },
                )?;
                Ok(json!({"album": id.0, "name": name, "added": !all_in, "count": s.catalog.album_count(id)}))
            }
        ),
        cmd!("album.setTarget", "Set as Target Album", [], None, "{id: albumId | null} (null = the Quick Collection)", always, |s, p| {
            s.target_album = match p.get("id").and_then(Value::as_u64) {
                Some(a) => {
                    let id = AlbumId(a);
                    let al = s.catalog.album(id).ok_or_else(|| bad("album.setTarget", "no such album"))?;
                    if al.is_smart() || al.folder {
                        return Err(bad("album.setTarget", "smart albums and folders can't hold photos"));
                    }
                    Some(id)
                }
                None => None,
            };
            Ok(json!({"target": s.target_album.map(|a| a.0)}))
        }),
        cmd!("album.clearQuick", "Clear Quick Collection", [], None, "{}", always, |s, _| {
            let Some(id) = s.catalog.quick_collection() else { return ok() };
            s.commit(
                "Clear Quick Collection",
                Op::Batch { ops: vec![Op::SetAlbumPhotos { id, photos: vec![] }, Op::SetAlbumCover { id, cover: None }] },
            )?;
            ok()
        }),
        cmd!(
            "library.autoImport",
            "Auto Import Settings",
            [],
            None,
            "{folder?: path | null (off), copy?: bool (copy into the library's Originals, else add in place), album?: name | null} — a watched folder whose new photos are added as they arrive (library.autoImportScan; the app scans every few seconds) → the settings",
            always,
            |s, p| {
                if let Some(f) = p.get("folder") {
                    s.import_defaults.auto_folder = match f.as_str().map(str::trim).filter(|f| !f.is_empty()) {
                        Some(f) if std::path::Path::new(f).is_dir() || cfg!(target_arch = "wasm32") => Some(f.to_string()),
                        Some(f) => return Err(bad("library.autoImport", format!("`{f}` is not a folder"))),
                        None => None,
                    };
                }
                if let Some(c) = p.get("copy").and_then(Value::as_bool) {
                    s.import_defaults.auto_copy = c;
                }
                if let Some(a) = p.get("album") {
                    s.import_defaults.auto_album = a.as_str().map(str::trim).filter(|a| !a.is_empty()).map(str::to_string);
                }
                s.save_prefs()?;
                let d = &s.import_defaults;
                Ok(json!({"folder": d.auto_folder, "copy": d.auto_copy, "album": d.auto_album}))
            }
        ),
        cmd!(
            "library.autoImportScan",
            "Auto Import Now",
            [],
            None,
            "{listing?: [[path, size]] (the folder as listed by the caller, e.g. on a worker thread), start?: bool (false: return the `library.import` params as `import` instead of importing)} — add the watched folder's new photos (files the library doesn't have yet; partial / still-copying files wait for the next scan) → {imported, folder, import?}",
            always,
            |s, p| {
                let Some(folder) = s.import_defaults.auto_folder.clone() else { return Ok(json!({"imported": [], "folder": null})) };
                // only files that stopped growing: a file still being written is left for later
                let known: std::collections::HashSet<String> = s
                    .catalog
                    .photos()
                    .filter_map(|p| match &p.source {
                        lightcraft_catalog::Source::File { path } => Some(path.clone()),
                        _ => None,
                    })
                    .collect();
                // the folder's files and sizes: listed here, or already listed on a worker thread (the app)
                let listing: Vec<(String, u64)> = match p.get("listing").and_then(Value::as_array) {
                    Some(l) => l.iter().filter_map(|e| Some((e.get(0)?.as_str()?.to_string(), e.get(1)?.as_u64()?))).collect(),
                    None => list_auto_import_folder(&folder).map_err(|e| bad("library.autoImportScan", e))?,
                };
                let mut fresh = Vec::new();
                for (ps, size) in listing {
                    if known.contains(&ps) {
                        continue;
                    }
                    // each file is tried once (a non-photo isn't retried every scan)
                    if s.auto_import_seen.get(&ps) == Some(&u64::MAX) {
                        continue;
                    }
                    let seen = s.auto_import_seen.insert(ps.clone(), size);
                    if size > 0 && seen == Some(size) {
                        fresh.push(ps);
                    }
                }
                if fresh.is_empty() {
                    return Ok(json!({"imported": [], "folder": folder}));
                }
                let mode = if s.import_defaults.auto_copy { "copy" } else { "add" };
                let mut params = json!({"paths": fresh, "mode": mode});
                if let Some(a) = s.import_defaults.auto_album.clone() {
                    match s.catalog.albums().find(|al| al.name.eq_ignore_ascii_case(&a) && !al.folder && !al.is_smart()) {
                        Some(al) => params["album"] = json!(al.id.0),
                        None => params["albumName"] = json!(a),
                    }
                }
                for f in &fresh {
                    s.auto_import_seen.insert(f.clone(), u64::MAX);
                }
                if p.get("start").and_then(Value::as_bool) == Some(false) {
                    // the caller imports them (the app: on a worker thread)
                    return Ok(json!({"imported": [], "folder": folder, "import": params}));
                }
                let sel = s.selection.clone();
                let r = s.execute("library.import", &params)?;
                // arriving photos don't take over the selection
                s.selection = sel;
                Ok(json!({"imported": r["imported"], "folder": folder}))
            }
        ),
        cmd!("album.removePhotos", "Remove from Album", [], None, "{id: albumId, ids?}", has_selection, |s, p| {
            let id = album_param(p, "id", "album.removePhotos")?;
            let targets = ids_param(p).unwrap_or_else(|| s.targets(&Value::Null));
            let al = s.catalog.album(id).ok_or_else(|| bad("album.removePhotos", "no such album"))?;
            if al.is_smart() {
                return Err(bad("album.removePhotos", "smart albums update automatically: change their rules"));
            }
            let photos: Vec<PhotoId> = al.photos.iter().copied().filter(|x| !targets.contains(x)).collect();
            s.commit("Remove from Album", Op::SetAlbumPhotos { id, photos })?;
            ok()
        }),
        cmd!("album.setCover", "Set as Album Cover", [], None, "{id: albumId, photo?: photoId}", has_active, |s, p| {
            let id = album_param(p, "id", "album.setCover")?;
            let photo = p.get("photo").and_then(Value::as_u64).map(PhotoId).or(s.active());
            s.commit("Set Album Cover", Op::SetAlbumCover { id, cover: photo })?;
            ok()
        }),
        // ---- import
        cmd!(
            query "library.importPreview",
            "Review Import",
            [],
            None,
            "{paths: [file or folder (recursive)]} → {candidates: [{path, name, format, kind, width, height, fileSize, captured, duplicate?: path|content, existing?, error?, previewOnly?: why a raw can only be shown from its embedded preview}], duplicates, scanned} — nothing is added",
            always,
            |s, p| {
                let paths = strs(p, "paths");
                if paths.is_empty() {
                    return Err(bad("library.importPreview", "no paths"));
                }
                let c = crate::import::scan(s, &paths);
                let dups = c.iter().filter(|c| c.duplicate.is_some()).count();
                Ok(json!({"scanned": c.len(), "duplicates": dups, "candidates": c}))
            }
        ),
        cmd!(
            "library.import",
            "Import Photos",
            ["File"],
            Some("Cmd+Shift+I"),
            "{paths: [file or folder (recursive)], mode?: add|copy|move (add = reference the files in place; copy = into the library's Originals/YYYY/YYYY-MM-DD/; move = as copy, then each original and its XMP sidecars are removed from the source — only after the copy is verified (a hard link on the same volume, else copied, synced and compared byte for byte) and its catalog record is saved; failed, duplicate and unchecked files keep their sources; a taken name gets -1, -2…; undo removes the photos from the library but leaves the files at the destination), destination?: folder for copies / moves, organize?: date (YYYY/YYYY-MM-DD) | month (YYYY/YYYY-MM) | flat | a folder template, e.g. `{date:%Y}/{date:%Y%m%d}` → 2026/20260114 (the template's `/` make the folders, each level expanded with the rename tokens and made a safe folder name: never outside the destination; must be relative, no `..`; a level with missing metadata is `unknown`) — dated by capture time, else the import time, rename?: file-name template for copies, original extension added (tokens: {name} {num} {seq} {seq:N} {date} {date:%Y%m%d} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext}; photo.renameTokens explains each; blank = keep names), renameStart?: 1, metadataPreset?: name, dng?: bool (copy raws as DNG; copy only), local?: bool (browsing: the photos stay out of the library, like library.browse; not with move), album?: albumId, albumName?: new album, preset?: presetId, keywords?: [..]} → {imported, duplicates, failed, moved?: [{from, to, sidecars?}], kept?: [{path, reason}] (move: sources left in place and why), album?}",
            always,
            |s, p| {
                let req = import_params(s, p)?;
                if req.paths.is_empty() {
                    return Err(bad("library.import", "no paths"));
                }
                let undo0 = s.undo.len();
                let report = crate::import::import_with(s, &req.paths, &req.opts)?;
                let report = import_batch_done(s, report, req.album, req.album_name.as_deref())?;
                // one undo step for the whole import
                let imported: Vec<u64> = report["imported"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                let n = s.undo.len().saturating_sub(undo0);
                s.merge_undo(n, &format!("Add {} Photo{}", imported.len(), if imported.len() == 1 { "" } else { "s" }));
                if let Some(f) = imported.first() {
                    s.selection = Selection::single(PhotoId(*f));
                }
                Ok(report)
            }
        ),
        // ---- persistence
        cmd!(query "library.info", "Library Info", [], None, "{}", always, |s, _| {
            let photos = s.catalog.len();
            let albums = s.catalog.albums().count();
            let (rendered_n, rendered_bytes) = s.media.rendered.mem_usage();
            let (sources_n, sources_bytes) = s.media.source_usage();
            let disk = s.media.rendered.disk().map(|d| {
                use std::sync::atomic::Ordering::Relaxed;
                json!({
                    "path": d.dir().display().to_string(),
                    "bytes": d.size(),
                    "hits": d.hits.load(Relaxed),
                    "misses": d.misses.load(Relaxed),
                    "writes": d.writes.load(Relaxed),
                })
            });
            let cache = json!({
                "renderedInMemory": rendered_n, "renderedBytes": rendered_bytes,
                "sourcesInMemory": sources_n, "sourceBytes": sources_bytes,
                "disk": disk,
            });
            let Some(lib) = &s.library else {
                return Ok(json!({"persistent": false, "photos": photos, "albums": albums, "cache": cache}));
            };
            let j = lib.journal();
            Ok(json!({
                "persistent": true,
                "path": lib.dir.display().to_string(),
                "photos": photos,
                "albums": albums,
                "seq": j.seq(),
                "snapshotSeq": j.snapshot_seq(),
                "logRecords": j.log_records(),
                "logBytes": j.log_bytes(),
                "lastError": lib.last_error,
                // settings files that were unreadable or damaged at open (kept, defaults used)
                "settingsWarnings": lib.settings_warnings,
                // changes applied in memory whose write failed (retried by every save)
                "unsavedOps": s.unsaved().map_or(0, |u| u.0),
                "unsavedError": s.unsaved().map(|u| u.1),
                // untouched Local records forgotten when the library opened
                "forgotLocal": lib.forgot_local.as_ref().map(|p| json!({"forgotten": p.evict.len(), "local": p.local, "keptRecent": p.kept_recent, "keptTouched": p.kept_touched, "keptInUse": p.kept_in_use})),
                "persistence": j.stats(),
                "cache": cache,
                "load": {
                    "created": lib.report.created,
                    "replayed": lib.report.replayed,
                    "tornBytes": lib.report.torn_bytes,
                    "damaged": lib.report.damaged,
                },
            }))
        }),
        cmd!("library.compact", "Optimize Library", ["File"], None, "{}", has_library, |s, _| {
            s.compact_library()?;
            ok()
        }),
        cmd!("library.clearPreviews", "Clear Preview Cache", ["File"], None, "{}", always, |s, _| {
            s.media.rendered.clear();
            ok()
        }),
    ]
}

/// `base` with a partial Filter (JSON) merged on top.
fn merge_rules(base: &lightcraft_catalog::Filter, patch: &Value, c: &str) -> Result<lightcraft_catalog::Filter> {
    let mut v = serde_json::to_value(base).unwrap_or_default();
    lightcraft_develop::presets::deep_merge(&mut v, patch);
    let f: lightcraft_catalog::Filter = serde_json::from_value(v).map_err(|e| bad(c, e.to_string()))?;
    if let Some(problem) = f.rule_set.as_ref().and_then(|r| r.problems().into_iter().next()) {
        return Err(bad(c, problem));
    }
    Ok(f)
}

/// The current view (source + filter) as smart-album rules. Viewing a smart album starts from
/// its rules with the filter bar's settings on top.
fn view_rules(s: &Session) -> lightcraft_catalog::Filter {
    use lightcraft_catalog::Filter;
    let smart = match s.source {
        LibrarySource::Album(a) => s.catalog.album(a).and_then(|a| a.smart.as_deref().cloned()),
        _ => None,
    };
    match smart {
        Some(base) => {
            // overlay the fields the filter bar changed
            let cur = serde_json::to_value(&s.filter).unwrap_or_default();
            let def = serde_json::to_value(Filter::default()).unwrap_or_default();
            let mut patch = serde_json::Map::new();
            if let (Some(c), Some(d)) = (cur.as_object(), def.as_object()) {
                for (k, v) in c {
                    if d.get(k) != Some(v) {
                        patch.insert(k.clone(), v.clone());
                    }
                }
            }
            merge_rules(&base, &Value::Object(patch), "").unwrap_or(base)
        }
        None => {
            let mut f = s.source.to_filter(&s.filter, &s.catalog);
            f.deleted = false;
            f
        }
    }
}

impl Session {
    /// The current view (source + filter) as smart-album rules (see `album.createSmart`).
    pub fn view_rules(&self) -> lightcraft_catalog::Filter {
        view_rules(self)
    }
}

fn has_library(s: &Session) -> std::result::Result<(), String> {
    if s.library.is_some() { Ok(()) } else { Err("no library is open (in-memory session)".into()) }
}

/// Coordinates typed by a person: "51.5072, -0.1276", "51.5072 N 0.1276 W",
/// "51°30'26\"N 0°7'39\"W" (degrees, minutes, seconds; N/S/E/W or signs).
pub(crate) fn parse_gps(t: &str) -> Option<(f64, f64)> {
    let t = t.trim();
    // split into the two coordinates: at a comma, else after the first N/S hemisphere letter
    let (a, b) = match t.split_once(',') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => {
            let i = t.find(['N', 'S', 'n', 's']).map(|i| i + 1).or_else(|| t.find(char::is_whitespace))?;
            (t[..i].trim().to_string(), t[i..].trim().to_string())
        }
    };
    let one = |s: &str, pos: char, neg: char| -> Option<f64> {
        let up = s.to_ascii_uppercase();
        let sign = if up.contains(neg) || up.trim_start().starts_with('-') { -1.0 } else { 1.0 };
        let nums: Vec<f64> =
            up.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|x| !x.is_empty()).map(|x| x.parse::<f64>().ok()).collect::<Option<_>>()?;
        let v = match nums.as_slice() {
            [d] => *d,
            [d, m] => d + m / 60.0,
            [d, m, s] => d + m / 60.0 + s / 3600.0,
            _ => return None,
        };
        let _ = pos;
        Some(sign * v)
    };
    let (la, lo) = (one(&a, 'N', 'S')?, one(&b, 'E', 'W')?);
    (la.abs() <= 90.0 && lo.abs() <= 180.0).then_some((la, lo))
}

#[cfg(test)]
mod gps_tests {
    #[test]
    fn coordinates_people_type() {
        let p = super::parse_gps;
        assert_eq!(p("51.5072, -0.1276"), Some((51.5072, -0.1276)));
        let (la, lo) = p("51°30'26\"N 0°7'39\"W").unwrap();
        assert!((la - 51.50722).abs() < 1e-4 && (lo + 0.1275).abs() < 1e-4, "{la} {lo}");
        let (la, lo) = p("33.8688 S, 151.2093 E").unwrap();
        assert!((la + 33.8688).abs() < 1e-9 && (lo - 151.2093).abs() < 1e-9);
        assert_eq!(p("95, 10"), None);
        assert_eq!(p("hello"), None);
    }
}
