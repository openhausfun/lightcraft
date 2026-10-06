//! Photo management commands: batch rename, capture time, colour label names and sets.

use serde_json::{Value, json};

use lightcraft_catalog::{ColorLabel, Op};

use super::{CommandSpec, always, bad, bool_or, cmd, f64_or, has_selection, str_param};
use crate::Result;

fn rename_args(s: &crate::Session, p: &Value, c: &str) -> Result<(Vec<lightcraft_catalog::PhotoId>, String, usize)> {
    let template = str_param(p, "template").ok_or_else(|| bad(c, "missing `template`"))?.to_string();
    let start = p.get("start").and_then(Value::as_u64).unwrap_or(1) as usize;
    Ok((s.targets(p), template, start))
}

/// A named set of colour-label names (red, yellow, green, blue, purple; empty = the colour's name).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LabelSet {
    pub name: String,
    pub names: [String; 5],
}

/// Our built-in sets.
pub fn builtin_label_sets() -> Vec<LabelSet> {
    let set = |name: &str, n: [&str; 5]| LabelSet { name: name.into(), names: n.map(str::to_string) };
    vec![set("Colors", ["", "", "", "", ""]), set("Review", ["Reject", "Needs Work", "Approved", "Retouch", "Print"])]
}

fn all_label_sets(s: &crate::Session) -> Vec<LabelSet> {
    builtin_label_sets().into_iter().chain(s.label_sets.iter().cloned()).collect()
}

/// The set whose names are the catalog's current ones.
fn current_label_set(s: &crate::Session) -> Option<String> {
    let cur: [String; 5] = ColorLabel::ALL.map(|l| s.catalog.custom_label_name(l).unwrap_or("").to_string());
    all_label_sets(s)
        .into_iter()
        .find(|x| x.names.iter().zip(&cur).all(|(a, b)| a.trim() == b.trim() || (a.trim().is_empty() && b.is_empty())))
        .map(|x| x.name)
}

/// Every set (built-ins first) and the one in use: `{sets: [{name, names, builtin}], current}`.
pub fn label_sets_json(s: &crate::Session) -> Value {
    let builtin = builtin_label_sets().len();
    let sets: Vec<Value> =
        all_label_sets(s).iter().enumerate().map(|(i, x)| json!({"name": x.name, "names": x.names, "builtin": i < builtin})).collect();
    json!({"sets": sets, "current": current_label_set(s)})
}

fn set_names_ops(s: &crate::Session, names: &[String; 5]) -> Vec<Op> {
    ColorLabel::ALL
        .iter()
        .zip(names)
        .filter_map(|(l, n)| {
            let n = n.trim();
            let name = (!n.is_empty() && !n.eq_ignore_ascii_case(&format!("{l:?}"))).then(|| n.to_string());
            (s.catalog.custom_label_name(*l) != name.as_deref()).then_some(Op::SetLabelName { label: *l, name })
        })
        .collect()
}

/// Geotag photos from a GPX track log by capture time (Lightroom Classic's Map ▸ Tracklog ▸
/// Auto-Tag Photos). GPX times are UTC; capture times are the camera's local clock, so a photo's
/// recorded zone (Exif `OffsetTimeOriginal`) or the `offset` parameter converts them.
fn auto_tag_tracklog(s: &mut crate::Session, p: &Value) -> Result<Value> {
    use lightcraft_meta::{DateTime, Match, parse_gpx};
    const C: &str = "photo.autoTagTracklog";
    let text = match (str_param(p, "gpx"), str_param(p, "path")) {
        (Some(t), _) => t.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path).map_err(|e| bad(C, format!("can't read {path}: {e}")))?,
        (None, None) => return Err(bad(C, "missing `path` (a .gpx file) or `gpx` (its text)")),
    };
    let log = parse_gpx(&text).map_err(|e| bad(C, e.to_string()))?;
    let (start, end) = log.span().ok_or_else(|| bad(C, "the track log has no timed track points"))?;
    // minutes east of UTC; `None` = not given (zone-less capture times are then read as UTC)
    let offset: Option<i64> = match p.get("offset") {
        None | Some(Value::Null) => None,
        Some(Value::String(o)) if o.trim().is_empty() => None,
        Some(Value::Number(h)) => {
            let h = h.as_f64().unwrap_or(0.0);
            if h.abs() > 18.0 {
                return Err(bad(C, "offset is at most ±18 hours"));
            }
            Some((h * 60.0).round() as i64)
        }
        Some(Value::String(o)) => {
            Some(DateTime::parse_offset(o).ok_or_else(|| bad(C, format!("can't read offset `{o}` (e.g. +02:00 or -5)")))? as i64)
        }
        Some(_) => return Err(bad(C, "offset is `+HH:MM` or a number of hours")),
    };
    let max_gap = f64_or(p, "maxGap", 600.0);
    if max_gap.is_nan() || max_gap < 0.0 {
        return Err(bad(C, "maxGap must be ≥ 0 seconds"));
    }
    let replace = bool_or(p, "replace", false);
    let (mut no_time, mut outside, mut has_gps, mut assumed_utc, mut interpolated) = (0, 0, 0, 0, 0);
    let mut ops = Vec::new();
    let mut photos = Vec::new();
    for id in s.targets(p) {
        let Some(ph) = s.catalog.photo(id) else { continue };
        if ph.meta.gps.is_some() && !replace {
            has_gps += 1;
            continue;
        }
        let Some(dt) = ph.captured.as_deref().and_then(DateTime::parse_iso) else {
            no_time += 1;
            continue;
        };
        // UTC seconds: the recorded zone wins, else the caller's offset
        let utc = match dt.offset_minutes {
            Some(_) => dt.unix_seconds(),
            None => {
                if offset.is_none() {
                    assumed_utc += 1;
                }
                dt.unix_seconds() - offset.unwrap_or(0) * 60
            }
        } as f64
            + dt.millis as f64 / 1000.0;
        let Some((g, m)) = log.locate(utc, max_gap) else {
            outside += 1;
            continue;
        };
        if m == Match::Interpolated {
            interpolated += 1;
        }
        let pos = ((g.latitude * 1e7).round() / 1e7, (g.longitude * 1e7).round() / 1e7);
        photos.push(json!({"id": id.0, "gps": [pos.0, pos.1], "match": if m == Match::Interpolated { "interpolated" } else { "nearest" }}));
        if ph.meta.gps != Some(pos) {
            let mut meta = ph.meta.clone();
            meta.gps = Some(pos);
            ops.push(Op::SetMeta { id, meta: Box::new(meta) });
        }
    }
    let tagged = photos.len();
    if !bool_or(p, "dryRun", false) && !ops.is_empty() {
        s.commit("Auto-Tag from Tracklog", Op::Batch { ops })?;
    }
    let iso = |t: f64| format!("{}Z", lightcraft_catalog::dates::civil(t.floor() as i64));
    Ok(json!({
        "tagged": tagged,
        "interpolated": interpolated,
        "nearest": tagged - interpolated,
        "skipped": {"noTime": no_time, "outside": outside, "hasGps": has_gps},
        "assumedUtc": assumed_utc,
        "points": log.points.len(),
        "start": iso(start),
        "end": iso(end),
        "photos": photos,
    }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "photo.renameTokens", "Rename Template Tags", [], None, "{} → {tokens: [{tag, aliases, meaning, example}], dateDirectives: [{directive, meaning}], notes: [..], sample} — the file-name template tags shared by photo.rename, library.import (rename) and app.export (naming); examples are for a sample photo", always, |_, _| {
            Ok(crate::rename::token_help_json())
        }),
        cmd!(query "photo.renamePreview", "Rename Preview", [], None, "{template: e.g. `{date}_{name}`, `Trip-{seq:3}` (tokens: {name} {num} {seq} {seq:N} {date} {date:%Y%m%d} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext}; photo.renameTokens explains each), start?: first sequence number (1), ids?} — renames the files on disk (sidecars too, never overwriting: collisions get -1, -2…); undoable", has_selection, |s, p| {
            let (ids, template, start) = rename_args(s, p, "photo.renamePreview")?;
            Ok(serde_json::to_value(s.plan_rename(&ids, &template, start)).unwrap_or_default())
        }),
        cmd!(
            "photo.rename",
            "Rename Photos",
            [],
            None,
            "{template: e.g. `{date}_{name}`, `Trip-{seq:3}` (tokens: {name} {num} {seq} {seq:N} {date} {date:%Y%m%d} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext}; photo.renameTokens explains each), start?: first sequence number (1), ids?} — renames the files on disk (sidecars too, never overwriting: collisions get -1, -2…); undoable",
            has_selection,
            |s, p| {
                let (ids, template, start) = rename_args(s, p, "photo.rename")?;
                let plans = s.plan_rename(&ids, &template, start);
                let n = s.apply_rename(&plans)?;
                Ok(json!({"renamed": n, "plans": plans}))
            }
        ),
        cmd!(
            "photo.setCaptureTime",
            "Edit Capture Time",
            [],
            None,
            "{ids?, time?: `2026-09-30T14:05:00` (the active photo gets it, the others shift by the same amount), each?: bool (every photo gets `time`), shift?: seconds, hours?: time-zone shift in hours} → {changed, captured: [..]}",
            has_selection,
            |s, p| {
                use lightcraft_catalog::dates::{iso_seconds, normalize_iso, shift_iso};
                let c = "photo.setCaptureTime";
                let targets: Vec<_> = s.targets(p).into_iter().filter(|id| s.catalog.photo(*id).is_some()).collect();
                if targets.is_empty() {
                    return Err(bad(c, "no photos"));
                }
                // photos without a capture time start from their import time
                let base = |s: &crate::Session, id| s.catalog.photo(id).map(|p| p.date().to_string()).unwrap_or_default();
                let mut delta = (f64_or(p, "shift", 0.0) + f64_or(p, "hours", 0.0) * 3600.0).round() as i64;
                let mut each: Option<String> = None;
                if let Some(t) = str_param(p, "time") {
                    let t = normalize_iso(t).ok_or_else(|| bad(c, format!("`{t}` is not a date (YYYY-MM-DDTHH:MM:SS)")))?;
                    if bool_or(p, "each", false) {
                        each = Some(t);
                    } else {
                        let anchor = s.active().filter(|a| targets.contains(a)).unwrap_or(targets[0]);
                        let from = iso_seconds(&base(s, anchor)).ok_or_else(|| bad(c, "the photo's date doesn't parse"))?;
                        delta += iso_seconds(&t).unwrap_or(from) - from;
                    }
                }
                let mut ops = Vec::new();
                let mut out = Vec::new();
                for id in &targets {
                    let new = match &each {
                        Some(t) => shift_iso(t, delta),
                        None => shift_iso(&base(s, *id), delta),
                    };
                    let Some(new) = new else { continue };
                    out.push(json!(new));
                    if s.catalog.photo(*id).and_then(|p| p.captured.as_deref()) != Some(new.as_str()) {
                        ops.push(Op::SetCaptured { id: *id, captured: Some(new) });
                    }
                }
                let n = ops.len();
                if n > 0 {
                    s.commit("Edit Capture Time", Op::Batch { ops })?;
                }
                Ok(json!({"changed": n, "captured": out}))
            }
        ),
        cmd!(
            "photo.autoTagTracklog",
            "Auto-Tag Photos from Tracklog",
            [],
            None,
            "{path?: GPX file | gpx?: GPX text, ids?, offset?: camera clock's UTC offset for photos whose capture time has no zone (`+02:00`, or hours; default UTC), maxGap?: seconds (600), replace?: bool (also photos that already have GPS), dryRun?: bool} → {tagged, interpolated, nearest, skipped: {noTime, outside, hasGps}, assumedUtc, points, start, end, photos: [{id, gps, match}]} — positions from the track log by capture time; one undo step",
            has_selection,
            auto_tag_tracklog
        ),
        cmd!(query "label.names", "Color Label Names", [], None, "{} → [{label, name, custom}]", always, |s, _| {
            Ok(json!(ColorLabel::ALL
                .iter()
                .map(|l| json!({"label": format!("{l:?}").to_lowercase(), "name": s.catalog.label_name(*l), "custom": s.catalog.custom_label_name(*l)}))
                .collect::<Vec<_>>()))
        }),
        cmd!(
            "label.setNames",
            "Edit Color Label Names",
            [],
            None,
            "{names: {red?: name|null, yellow?, green?, blue?, purple?}} — null or empty restores the colour's name",
            always,
            |s, p| {
                let names = p.get("names").and_then(Value::as_object).ok_or_else(|| bad("label.setNames", "missing `names`"))?;
                let mut ops = Vec::new();
                for (k, v) in names {
                    let label = ColorLabel::parse(k).ok_or_else(|| bad("label.setNames", format!("unknown label `{k}`")))?;
                    let name =
                        v.as_str().map(str::trim).filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&format!("{label:?}"))).map(str::to_string);
                    if s.catalog.custom_label_name(label) != name.as_deref() {
                        ops.push(Op::SetLabelName { label, name });
                    }
                }
                let n = ops.len();
                if n > 0 {
                    s.commit("Edit Label Names", Op::Batch { ops })?;
                }
                Ok(json!({"changed": n}))
            }
        ),
        cmd!(query "label.sets", "Color Label Sets", [], None, "{} → {sets: [{name, names: [red, yellow, green, blue, purple], builtin}], current: name|null}", always, |s, _| Ok(label_sets_json(s))),
        cmd!("label.applySet", "Apply Color Label Set", [], None, "{name} — use a set's label names (undoable)", always, |s, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("label.applySet", "missing `name`"))?;
            let set = all_label_sets(s)
                .into_iter()
                .find(|x| x.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| bad("label.applySet", format!("no label set `{name}`")))?;
            let ops = set_names_ops(s, &set.names);
            let n = ops.len();
            if n > 0 {
                s.commit(&format!("Label Set: {}", set.name), Op::Batch { ops })?;
            }
            Ok(json!({"changed": n}))
        }),
        cmd!(
            "label.saveSet",
            "Save Color Label Set",
            [],
            None,
            "{name} — save the current label names as a set (replaces a user set of that name)",
            always,
            |s, p| {
                let name =
                    str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad("label.saveSet", "missing `name`"))?.to_string();
                if builtin_label_sets().iter().any(|b| b.name.eq_ignore_ascii_case(&name)) {
                    return Err(bad("label.saveSet", format!("`{name}` is a built-in set")));
                }
                let set = LabelSet { name: name.clone(), names: ColorLabel::ALL.map(|l| s.catalog.custom_label_name(l).unwrap_or("").to_string()) };
                match s.label_sets.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)) {
                    Some(x) => *x = set,
                    None => s.label_sets.push(set),
                }
                s.save_prefs()?;
                Ok(json!({"name": name}))
            }
        ),
        cmd!("label.deleteSet", "Delete Color Label Set", [], None, "{name} — user sets only", always, |s, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("label.deleteSet", "missing `name`"))?;
            let before = s.label_sets.len();
            s.label_sets.retain(|x| !x.name.eq_ignore_ascii_case(name));
            if s.label_sets.len() == before {
                return Err(bad("label.deleteSet", format!("no user label set `{name}`")));
            }
            s.save_prefs()?;
            Ok(json!({"deleted": name}))
        }),
    ]
}
