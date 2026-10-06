//! Masking and Remove (spot) commands on the active photo.

use lightcraft_develop::{BrushStroke, LocalAdjustments, Mask, MaskComponent, MaskOp, MaskShape, RedEye, Spot, SpotMode};
use lightcraft_geom::Point;
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, cmd, f64_or, has_active, ok, point, str_param};
use crate::{Result, Session};

fn shape_from(kind: &str, p: &Value, c: &str) -> Result<MaskShape> {
    Ok(match kind {
        "brush" => MaskShape::Brush { strokes: vec![] },
        "linear" => {
            MaskShape::Linear { start: point(p, "start").unwrap_or(Point::new(0.5, 0.25)), end: point(p, "end").unwrap_or(Point::new(0.5, 0.6)) }
        }
        "radial" => MaskShape::Radial {
            center: point(p, "center").unwrap_or(Point::new(0.5, 0.5)),
            rx: f64_or(p, "rx", 0.22),
            ry: f64_or(p, "ry", 0.16),
            angle: f64_or(p, "angle", 0.0),
            feather: f64_or(p, "feather", 50.0),
            invert: bool_or(p, "invert", false),
        },
        "sky" => MaskShape::Sky,
        "subject" => MaskShape::Subject,
        "background" => MaskShape::Background,
        "luminanceRange" => MaskShape::LuminanceRange {
            lo: f64_or(p, "lo", 0.6),
            hi: f64_or(p, "hi", 1.0),
            lo_feather: f64_or(p, "loFeather", 0.1),
            hi_feather: f64_or(p, "hiFeather", 0.1),
        },
        "colorRange" => {
            let samples: Vec<[f64; 3]> = p
                .get("samples")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|s| Some([s.get(0)?.as_f64()?, s.get(1)?.as_f64()?, s.get(2)?.as_f64()?])).collect())
                .unwrap_or_default();
            MaskShape::ColorRange { samples, refine: f64_or(p, "refine", 50.0) }
        }
        other => return Err(bad(c, format!("unknown mask kind `{other}` (brush|linear|radial|sky|subject|background|luminanceRange|colorRange)"))),
    })
}

fn masks_edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut Vec<Mask>, &mut Option<u32>) -> Result<()>) -> Result<Value> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    let mut active = s.active_mask;
    f(&mut d.masks, &mut active)?;
    s.set_develop(id, d, label)?;
    s.active_mask = active;
    Ok(json!({"activeMask": active}))
}

fn mask_id(p: &Value, active: Option<u32>, c: &str) -> Result<u32> {
    p.get("id").and_then(Value::as_u64).map(|v| v as u32).or(active).ok_or_else(|| bad(c, "no mask selected (give `id`)"))
}

fn find(masks: &mut [Mask], id: u32, c: &str) -> Result<usize> {
    masks.iter().position(|m| m.id == id).ok_or_else(|| bad(c, format!("no mask {id}")))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mask.add",
            "Create New Mask",
            [],
            None,
            "{kind: brush|linear|radial|sky|subject|background|luminanceRange|colorRange, ...shape params (start/end, center/rx/ry/angle/feather, lo/hi…), name?}",
            has_active,
            |s, p| {
                let kind = str_param(p, "kind").unwrap_or("radial").to_string();
                let shape = shape_from(&kind, p, "mask.add")?;
                let name = str_param(p, "name").map(str::to_string);
                let next = s.active().and_then(|id| s.develop_of(id)).map(|d| d.next_mask_id()).unwrap_or(1);
                masks_edit(s, "mask.add", "Add Mask", |masks, active| {
                    masks.push(Mask {
                        id: next,
                        name: name.unwrap_or_else(|| format!("Mask {next}")),
                        components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
                        ..Default::default()
                    });
                    *active = Some(next);
                    Ok(())
                })
            }
        ),
        cmd!(
            "mask.addComponent",
            "Add/Subtract/Intersect Mask",
            [],
            None,
            "{id?: maskId, op: add|subtract|intersect, kind, ...shape params}",
            has_active,
            |s, p| {
                let kind = str_param(p, "kind").unwrap_or("brush").to_string();
                let shape = shape_from(&kind, p, "mask.addComponent")?;
                let op: MaskOp =
                    serde_json::from_value(p.get("op").cloned().unwrap_or(json!("add"))).map_err(|e| bad("mask.addComponent", e.to_string()))?;
                let active = s.active_mask;
                let mid = mask_id(p, active, "mask.addComponent")?;
                masks_edit(s, "mask.addComponent", "Edit Mask", |masks, _| {
                    let i = find(masks, mid, "mask.addComponent")?;
                    masks[i].components.push(MaskComponent { name: None, op, invert: bool_or(p, "invert", false), shape });
                    Ok(())
                })
            }
        ),
        cmd!(
            "mask.brushStroke",
            "Brush Stroke",
            [],
            None,
            "{id?: maskId, points: [[x,y],…] normalized, size?: fraction of long edge (0.03), feather?, flow?, density?, erase?: bool, autoMask?: bool}",
            has_active,
            |s, p| {
                let pts: Vec<Point> = p
                    .get("points")
                    .and_then(Value::as_array)
                    .ok_or_else(|| bad("mask.brushStroke", "missing points"))?
                    .iter()
                    .filter_map(|q| Some(Point::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?)))
                    .collect();
                let d = BrushStroke::default();
                let stroke = BrushStroke {
                    points: pts,
                    size: f64_or(p, "size", d.size),
                    feather: f64_or(p, "feather", d.feather),
                    flow: f64_or(p, "flow", d.flow),
                    density: f64_or(p, "density", d.density),
                    erase: bool_or(p, "erase", false),
                    auto_mask: bool_or(p, "autoMask", false),
                };
                let next = s.active().and_then(|id| s.develop_of(id)).map(|d| d.next_mask_id()).unwrap_or(1);
                let want = p.get("id").and_then(Value::as_u64).map(|v| v as u32).or(s.active_mask);
                masks_edit(s, "mask.brushStroke", "Brush", |masks, active| {
                    let i = match want.and_then(|m| masks.iter().position(|x| x.id == m)) {
                        Some(i) => i,
                        None => {
                            masks.push(Mask { id: next, name: format!("Mask {next}"), ..Default::default() });
                            *active = Some(next);
                            masks.len() - 1
                        }
                    };
                    let m = &mut masks[i];
                    if let Some(MaskComponent { name: None, shape: MaskShape::Brush { strokes }, .. }) =
                        m.components.iter_mut().rev().find(|c| matches!(c.shape, MaskShape::Brush { .. }))
                    {
                        strokes.push(stroke);
                    } else {
                        m.components.push(MaskComponent {
                            name: None,
                            op: MaskOp::Add,
                            invert: false,
                            shape: MaskShape::Brush { strokes: vec![stroke] },
                        });
                    }
                    Ok(())
                })
            }
        ),
        cmd!("mask.update", "Update Mask Shape", [], None, "{id?, component?: index (0), shape: MaskShape JSON}", has_active, |s, p| {
            let shape: MaskShape =
                serde_json::from_value(p.get("shape").cloned().unwrap_or_default()).map_err(|e| bad("mask.update", e.to_string()))?;
            let comp = p.get("component").and_then(Value::as_u64).unwrap_or(0) as usize;
            let mid = mask_id(p, s.active_mask, "mask.update")?;
            masks_edit(s, "mask.update", "Edit Mask", |masks, _| {
                let i = find(masks, mid, "mask.update")?;
                let c = masks[i].components.get_mut(comp).ok_or_else(|| bad("mask.update", "no such component"))?;
                c.shape = shape;
                Ok(())
            })
        }),
        cmd!(
            "mask.sampleColor",
            "Sample Mask Color",
            [],
            None,
            "{x, y: normalized image point, add?: bool (⇧: add to the samples, max 5), id?, component?} — the colour range of the selected mask (its first colour-range component) samples the colour there → {samples}",
            has_active,
            |s, p| {
                let c = "mask.sampleColor";
                let (x, y) = (super::f64_req(p, "x", c)?, super::f64_req(p, "y", c)?);
                let add = p.get("add").and_then(Value::as_bool).unwrap_or(false);
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let src = s.source_now(id, crate::media::SourceLevel::Thumb).map_err(|e| bad(c, e))?;
                let info = s.source_info(id);
                let d = s.develop_of(id).unwrap_or_default();
                let req = lightcraft_pipeline::RenderRequest::fit(384, 384);
                let lab = lightcraft_pipeline::color_range_sample(&src, &info, &d, &req, lightcraft_geom::Point::new(x, y))
                    .ok_or_else(|| bad(c, "the point is outside the photo"))?;
                let mid = mask_id(p, s.active_mask, c)?;
                let comp = p.get("component").and_then(Value::as_u64).map(|v| v as usize);
                let mut out = Vec::new();
                masks_edit(s, c, "Color Range", |masks, _| {
                    let i = find(masks, mid, c)?;
                    let k = comp
                        .or_else(|| masks[i].components.iter().position(|x| matches!(x.shape, MaskShape::ColorRange { .. })))
                        .ok_or_else(|| bad(c, "the mask has no colour range"))?;
                    let Some(MaskShape::ColorRange { samples, .. }) = masks[i].components.get_mut(k).map(|x| &mut x.shape) else {
                        return Err(bad(c, "not a colour range component"));
                    };
                    if !add {
                        samples.clear();
                    }
                    if samples.len() < 5 {
                        samples.push(lab);
                    }
                    out = samples.clone();
                    Ok(())
                })?;
                Ok(json!({"samples": out}))
            }
        ),
        cmd!(
            "mask.refine",
            "Refine Mask Edges",
            [],
            None,
            "{id?, value: 0..100} — the mask's edges snap to the photo's (guided filter; rendered on the CPU)",
            has_active,
            |s, p| {
                let v = f64_or(p, "value", 0.0).clamp(0.0, 100.0);
                let mid = mask_id(p, s.active_mask, "mask.refine")?;
                masks_edit(s, "mask.refine", "Refine Edges", |masks, _| {
                    let i = find(masks, mid, "mask.refine")?;
                    masks[i].refine = v;
                    Ok(())
                })
            }
        ),
        cmd!(
            "mask.adjust",
            "Set Mask Adjustments",
            [],
            None,
            "{id?, values: {exposure, contrast, highlights, shadows, whites, blacks, temp, tint, texture, clarity, dehaze, hue, saturation, sharpness, noise, moire, defringe, color_hue, color_sat, amount}}",
            has_active,
            |s, p| {
                let vals = p.get("values").cloned().ok_or_else(|| bad("mask.adjust", "missing values"))?;
                let mid = mask_id(p, s.active_mask, "mask.adjust")?;
                masks_edit(s, "mask.adjust", "Mask Adjustment", |masks, _| {
                    let i = find(masks, mid, "mask.adjust")?;
                    let mut v = serde_json::to_value(masks[i].adjust).unwrap_or_default();
                    lightcraft_develop::presets::deep_merge(&mut v, &vals);
                    let a: LocalAdjustments = serde_json::from_value(v).map_err(|e| bad("mask.adjust", e.to_string()))?;
                    masks[i].adjust = clamp_local(a);
                    Ok(())
                })
            }
        ),
        cmd!("mask.select", "Select Mask", [], None, "{id|null}", has_active, |s, p| {
            s.active_mask = p.get("id").and_then(Value::as_u64).map(|v| v as u32);
            ok()
        }),
        cmd!("mask.delete", "Delete Mask", [], None, "{id?}", has_active, |s, p| {
            let mid = mask_id(p, s.active_mask, "mask.delete")?;
            masks_edit(s, "mask.delete", "Delete Mask", |masks, active| {
                let i = find(masks, mid, "mask.delete")?;
                masks.remove(i);
                *active = masks.last().map(|m| m.id);
                Ok(())
            })
        }),
        cmd!("mask.deleteAll", "Delete All Masks", [], None, "{}", has_active, |s, _| {
            masks_edit(s, "mask.deleteAll", "Delete All Masks", |masks, active| {
                masks.clear();
                *active = None;
                Ok(())
            })
        }),
        cmd!("mask.rename", "Rename Mask", [], None, "{id?, name}", has_active, |s, p| {
            let name = str_param(p, "name").ok_or_else(|| bad("mask.rename", "missing name"))?.to_string();
            let mid = mask_id(p, s.active_mask, "mask.rename")?;
            masks_edit(s, "mask.rename", "Rename Mask", |masks, _| {
                let i = find(masks, mid, "mask.rename")?;
                masks[i].name = name;
                Ok(())
            })
        }),
        cmd!("mask.invert", "Invert Mask", [], None, "{id?}", has_active, |s, p| {
            let mid = mask_id(p, s.active_mask, "mask.invert")?;
            masks_edit(s, "mask.invert", "Invert Mask", |masks, _| {
                let i = find(masks, mid, "mask.invert")?;
                masks[i].invert = !masks[i].invert;
                Ok(())
            })
        }),
        cmd!("mask.visible", "Show/Hide Mask", [], None, "{id?, visible?: bool}", has_active, |s, p| {
            let mid = mask_id(p, s.active_mask, "mask.visible")?;
            masks_edit(s, "mask.visible", "Toggle Mask", |masks, _| {
                let i = find(masks, mid, "mask.visible")?;
                masks[i].visible = bool_or(p, "visible", !masks[i].visible);
                Ok(())
            })
        }),
        cmd!(
            "mask.duplicate",
            "Duplicate Mask",
            [],
            None,
            "{id?, invert?: bool (Duplicate and Invert)} — the copy goes right after the original and is selected",
            has_active,
            |s, p| {
                let mid = mask_id(p, s.active_mask, "mask.duplicate")?;
                let invert = bool_or(p, "invert", false);
                let next = s.active().and_then(|id| s.develop_of(id)).map(|d| d.next_mask_id()).unwrap_or(1);
                let label = if invert { "Duplicate and Invert Mask" } else { "Duplicate Mask" };
                masks_edit(s, "mask.duplicate", label, |masks, active| {
                    let i = find(masks, mid, "mask.duplicate")?;
                    let mut m = masks[i].clone();
                    m.id = next;
                    m.name = format!("{} copy", m.name);
                    if invert {
                        m.invert = !m.invert;
                    }
                    masks.insert(i + 1, m);
                    *active = Some(next);
                    Ok(())
                })
            }
        ),
        cmd!(
            "mask.component",
            "Edit Mask Component",
            [],
            None,
            "{id?, component: index, action: invert|duplicate|delete|rename|op, name? (rename; empty clears), op?: add|subtract|intersect} — one component of a mask (deleting the last one deletes the mask)",
            has_active,
            |s, p| {
                let c = "mask.component";
                let mid = mask_id(p, s.active_mask, c)?;
                let k = p.get("component").and_then(Value::as_u64).ok_or_else(|| bad(c, "missing component"))? as usize;
                let action = str_param(p, "action").ok_or_else(|| bad(c, "missing action"))?.to_string();
                let op: Option<MaskOp> = match p.get("op") {
                    Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| bad(c, e.to_string()))?),
                    None => None,
                };
                let label = match action.as_str() {
                    "invert" => "Invert Component",
                    "duplicate" => "Duplicate Component",
                    "delete" => "Delete Component",
                    "rename" => "Rename Component",
                    "op" => "Change Component Mode",
                    a => return Err(bad(c, format!("unknown action {a:?} (invert|duplicate|delete|rename|op)"))),
                };
                masks_edit(s, c, label, |masks, active| {
                    let i = find(masks, mid, c)?;
                    let comps = &mut masks[i].components;
                    let comp = comps.get_mut(k).ok_or_else(|| bad(c, "no such component"))?;
                    match action.as_str() {
                        "invert" => comp.invert = !comp.invert,
                        "rename" => comp.name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string),
                        "op" => comp.op = op.ok_or_else(|| bad(c, "missing op"))?,
                        "duplicate" => {
                            let copy = comp.clone();
                            comps.insert(k + 1, copy);
                        }
                        _ => {
                            comps.remove(k);
                            if comps.is_empty() {
                                masks.remove(i);
                                *active = masks.last().map(|m| m.id);
                            }
                        }
                    }
                    Ok(())
                })
            }
        ),
        cmd!("mask.move", "Move Mask", [], None, "{id?, to?: index (0 = top), delta?: ±n} — reorder the masks list", has_active, |s, p| {
            let mid = mask_id(p, s.active_mask, "mask.move")?;
            let mut masks = s.active().and_then(|id| s.develop_of(id)).map(|d| d.masks.clone()).unwrap_or_default();
            let i = find(&mut masks, mid, "mask.move")?;
            let to = match (p.get("to").and_then(Value::as_i64), p.get("delta").and_then(Value::as_i64)) {
                (Some(t), _) => t,
                (None, Some(d)) => i as i64 + d,
                (None, None) => return Err(bad("mask.move", "give `to` (index) or `delta`")),
            }
            .clamp(0, masks.len() as i64 - 1) as usize;
            if to == i {
                return Ok(json!({"activeMask": s.active_mask}));
            }
            masks_edit(s, "mask.move", "Reorder Masks", |masks, _| {
                let i = find(masks, mid, "mask.move")?;
                let m = masks.remove(i);
                masks.insert(to, m);
                Ok(())
            })
        }),
        // ---- Remove tool (spots)
        cmd!(
            "spot.findDust",
            "Find Dust Spots",
            [],
            None,
            "{sensitivity?: 0..100 (50), add?: bool (true)} — find sensor-dust spots (small soft dark spots on smooth areas) and add a heal spot on each (one undo step) → {spots: [{x, y, size}], added}",
            has_active,
            |s, p| {
                let c = "spot.findDust";
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                // the uncropped photo, so positions are the spots' own coordinates
                let job = s.render_job(id, 1600, 1600, false, false).ok_or_else(|| bad(c, "no photo"))?;
                let img = job.run().rendered.map_err(|e| bad(c, e))?.image;
                let found = lightcraft_pipeline::dust::detect(&img, f64_or(p, "sensitivity", 50.0) as f32);
                let out: Vec<Value> = found.iter().map(|d| json!({"x": d.x, "y": d.y, "size": d.radius})).collect();
                if !p.get("add").and_then(Value::as_bool).unwrap_or(true) || found.is_empty() {
                    return Ok(json!({"spots": out, "added": 0}));
                }
                let mut dd = (*s.develop_of(id).unwrap_or_default()).clone();
                let base = Spot::default();
                for d in &found {
                    let mut spot = Spot {
                        mode: SpotMode::Heal,
                        points: vec![Point::new(d.x, d.y)],
                        size: d.radius.clamp(SPOT_SIZE.0, SPOT_SIZE.1),
                        feather: base.feather,
                        opacity: 100.0,
                        source_offset: None,
                    };
                    spot.source_offset = pick_source(s, id, &dd, &spot, None);
                    dd.spots.push(spot);
                }
                s.set_develop(id, dd, "Find Dust Spots")?;
                Ok(json!({"spots": out, "added": found.len()}))
            }
        ),
        cmd!(
            "spot.add",
            "Add Remove Spot",
            [],
            None,
            "{mode?: remove|heal|clone, points: [[x,y],…], size?: fraction of long edge, feather?, opacity?, source?: [dx,dy] (default: the best match nearby)} — selects the new spot; returns {index}",
            has_active,
            |s, p| {
                let mode: SpotMode =
                    serde_json::from_value(p.get("mode").cloned().unwrap_or(json!("remove"))).map_err(|e| bad("spot.add", e.to_string()))?;
                let pts: Vec<Point> = p
                    .get("points")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(|q| Some(Point::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?))).collect())
                    .unwrap_or_default();
                if pts.is_empty() {
                    return Err(bad("spot.add", "missing points"));
                }
                let d = Spot::default();
                let mut spot = Spot {
                    mode,
                    points: pts,
                    size: f64_or(p, "size", d.size).clamp(SPOT_SIZE.0, SPOT_SIZE.1),
                    feather: f64_or(p, "feather", d.feather).clamp(0.0, 100.0),
                    opacity: f64_or(p, "opacity", d.opacity).clamp(0.0, 100.0),
                    source_offset: point(p, "source"),
                };
                let id = s.active().ok_or_else(|| bad("spot.add", "no active photo"))?;
                let mut dd = (*s.develop_of(id).unwrap_or_default()).clone();
                if spot.source_offset.is_none() {
                    // resolve the automatic source now: it gets a pin and stays put at every size
                    spot.source_offset = pick_source(s, id, &dd, &spot, None);
                }
                dd.spots.push(spot);
                let n = dd.spots.len();
                s.set_develop(id, dd, "Remove")?;
                s.active_spot = Some(n - 1);
                Ok(json!({"index": n - 1}))
            }
        ),
        cmd!("spot.select", "Select Spot", [], None, "{index|null}", has_active, |s, p| {
            let n = spots_len(s);
            s.active_spot = match p.get("index").and_then(Value::as_u64) {
                Some(i) if (i as usize) < n => Some(i as usize),
                Some(_) => return Err(bad("spot.select", "no such spot")),
                None => None,
            };
            Ok(json!({"activeSpot": s.active_spot}))
        }),
        cmd!(
            "spot.update",
            "Edit Spot",
            [],
            None,
            "{index? (the selected spot), move?: [dx,dy] (the target, normalized), source?: [dx,dy] (offset from the target), moveSource?: [dx,dy], size?, feather?, opacity?, mode?}",
            has_active,
            |s, p| {
                let c = "spot.update";
                let i = spot_index(s, p, c)?;
                let mode: Option<SpotMode> = match p.get("mode") {
                    Some(m) => Some(serde_json::from_value(m.clone()).map_err(|e| bad(c, e.to_string()))?),
                    None => None,
                };
                spots_edit(s, c, "Edit Spot", |spots| {
                    let sp = &mut spots[i];
                    if let Some(d) = point(p, "move") {
                        sp.points.iter_mut().for_each(|q| *q = Point::new(q.x + d.x, q.y + d.y));
                        // the source moves along (its offset is relative to the target)
                    }
                    if let Some(o) = point(p, "source") {
                        sp.source_offset = Some(o);
                    }
                    if let Some(d) = point(p, "moveSource") {
                        let o = sp.source_offset.unwrap_or(Point::new(0.0, 0.0));
                        sp.source_offset = Some(Point::new(o.x + d.x, o.y + d.y));
                    }
                    if let Some(v) = p.get("size").and_then(Value::as_f64) {
                        sp.size = v.clamp(SPOT_SIZE.0, SPOT_SIZE.1);
                    }
                    if let Some(v) = p.get("feather").and_then(Value::as_f64) {
                        sp.feather = v.clamp(0.0, 100.0);
                    }
                    if let Some(v) = p.get("opacity").and_then(Value::as_f64) {
                        sp.opacity = v.clamp(0.0, 100.0);
                    }
                    if let Some(m) = mode {
                        sp.mode = m;
                    }
                    Ok(())
                })?;
                Ok(json!({"index": i}))
            }
        ),
        cmd!(
            "spot.refreshSource",
            "Refresh Source",
            [],
            None,
            "{index? (the selected spot)} — picks the next-best source away from the current one",
            has_active,
            |s, p| {
                let c = "spot.refreshSource";
                let i = spot_index(s, p, c)?;
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let dd = (*s.develop_of(id).unwrap_or_default()).clone();
                let spot = dd.spots[i].clone();
                let next = pick_source(s, id, &dd, &spot, spot.source_offset).ok_or_else(|| bad(c, "no other source fits in the photo"))?;
                spots_edit(s, c, "Refresh Source", |spots| {
                    spots[i].source_offset = Some(next);
                    Ok(())
                })?;
                Ok(json!({"index": i, "source": [next.x, next.y]}))
            }
        ),
        cmd!("spot.delete", "Delete Spot", [], None, "{index? (the selected spot)}", has_active, |s, p| {
            let c = "spot.delete";
            let i = spot_index(s, p, c)?;
            spots_edit(s, c, "Delete Spot", |spots| {
                spots.remove(i);
                Ok(())
            })?;
            s.active_spot = match s.active_spot {
                Some(a) if a == i => None,
                Some(a) if a > i => Some(a - 1),
                a => a,
            };
            ok()
        }),
        // ---- Red eye / pet eye
        cmd!(
            "redeye.add",
            "Add Red Eye Correction",
            [],
            None,
            "{center: [x,y] normalized, rx, ry: radii as fractions of the long edge, pet?: bool, pupilSize?: 0..100, darken?: 0..100} — the pupil inside is found automatically; returns {index}",
            has_active,
            |s, p| {
                let c = "redeye.add";
                let center = point(p, "center").ok_or_else(|| bad(c, "missing center"))?;
                let d = RedEye::default();
                let eye = RedEye {
                    center,
                    rx: f64_or(p, "rx", d.rx).clamp(1e-4, 0.5),
                    ry: f64_or(p, "ry", d.ry).clamp(1e-4, 0.5),
                    pupil_size: f64_or(p, "pupilSize", d.pupil_size).clamp(0.0, 100.0),
                    darken: f64_or(p, "darken", d.darken).clamp(0.0, 100.0),
                    pet: bool_or(p, "pet", false),
                    catchlight: None,
                };
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let mut dd = (*s.develop_of(id).unwrap_or_default()).clone();
                dd.red_eye.push(eye);
                let n = dd.red_eye.len();
                s.set_develop(id, dd, if eye.pet { "Pet Eye" } else { "Red Eye" })?;
                Ok(json!({"index": n - 1}))
            }
        ),
        cmd!(
            "redeye.catchlight",
            "Pet Eye Catchlight",
            [],
            None,
            "{index, on?: bool (default true), offset?: [dx, dy] from the pupil centre in pupil radii (default [-0.35, -0.35])}",
            has_active,
            |s, p| {
                let c = "redeye.catchlight";
                let i = super::f64_req(p, "index", c)? as usize;
                let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
                let mut dd = (*s.develop_of(id).unwrap_or_default()).clone();
                let eye = dd.red_eye.get_mut(i).ok_or_else(|| bad(c, "no such eye"))?;
                if !eye.pet {
                    return Err(bad(c, "catchlights are for pet eyes"));
                }
                let off = point(p, "offset").or(eye.catchlight).unwrap_or(Point::new(-0.35, -0.35));
                eye.catchlight = bool_or(p, "on", true).then(|| Point::new(off.x.clamp(-1.0, 1.0), off.y.clamp(-1.0, 1.0)));
                s.set_develop(id, dd, "Catchlight")?;
                ok()
            }
        ),
        cmd!("redeye.delete", "Delete Red Eye Correction", [], None, "{index}", has_active, |s, p| {
            let c = "redeye.delete";
            let i = super::f64_req(p, "index", c)? as usize;
            let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
            let mut dd = (*s.develop_of(id).unwrap_or_default()).clone();
            if i >= dd.red_eye.len() {
                return Err(bad(c, "no such eye"));
            }
            dd.red_eye.remove(i);
            s.set_develop(id, dd, "Delete Red Eye")?;
            ok()
        }),
    ]
}

/// Spot radius limits (fraction of the long edge).
pub const SPOT_SIZE: (f64, f64) = (0.001, 0.25);

fn spots_len(s: &Session) -> usize {
    s.active().and_then(|id| s.develop_of(id)).map(|d| d.spots.len()).unwrap_or(0)
}

/// `index` from the params, else the selected spot.
fn spot_index(s: &Session, p: &Value, c: &str) -> Result<usize> {
    let i = p.get("index").and_then(Value::as_u64).map(|v| v as usize).or(s.active_spot).ok_or_else(|| bad(c, "no spot selected (give `index`)"))?;
    if i >= spots_len(s) {
        return Err(bad(c, "no such spot"));
    }
    Ok(i)
}

fn spots_edit(s: &mut Session, c: &str, label: &str, f: impl FnOnce(&mut Vec<Spot>) -> Result<()>) -> Result<()> {
    let id = s.active().ok_or_else(|| bad(c, "no active photo"))?;
    let mut d = (*s.develop_of(id).unwrap_or_default()).clone();
    f(&mut d.spots)?;
    s.set_develop(id, d, label)
}

/// An automatic source for `spot` on photo `id` (a small proxy of the photo, framed by `d`).
fn pick_source(s: &mut Session, id: crate::PhotoId, d: &lightcraft_develop::DevelopSettings, spot: &Spot, avoid: Option<Point>) -> Option<Point> {
    let src = s.source_now(id, crate::media::SourceLevel::Thumb).ok()?;
    let info = s.source_info(id);
    lightcraft_pipeline::spots::pick_source(&src, &info, d, spot, avoid)
}

fn clamp_local(mut a: LocalAdjustments) -> LocalAdjustments {
    let c = |v: &mut f64, lo: f64, hi: f64| *v = if v.is_finite() { v.clamp(lo, hi) } else { 0.0 };
    c(&mut a.exposure, -4.0, 4.0);
    for v in [
        &mut a.temp,
        &mut a.tint,
        &mut a.contrast,
        &mut a.highlights,
        &mut a.shadows,
        &mut a.whites,
        &mut a.blacks,
        &mut a.texture,
        &mut a.clarity,
        &mut a.dehaze,
        &mut a.hue,
        &mut a.saturation,
        &mut a.sharpness,
        &mut a.noise,
        &mut a.moire,
        &mut a.defringe,
    ] {
        c(v, -100.0, 100.0);
    }
    c(&mut a.color_hue, 0.0, 360.0);
    c(&mut a.color_sat, 0.0, 100.0);
    c(&mut a.amount, 0.0, 200.0);
    a
}
