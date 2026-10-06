//! DNG opcode lists (`OpcodeList1/2/3`, DNG 1.7 chapter 7). Opcode lists are always big-endian.
//!
//! Parsed: every opcode (unknown ones are kept as [`Opcode::Unknown`]). Applied: `WarpRectilinear`,
//! `FixVignetteRadial`, `FixBadPixelsConstant`, `FixBadPixelsList`, `MapTable`, `MapPolynomial`, `GainMap`,
//! `DeltaPerRow/Column`, `ScalePerRow/Column`. `TrimBounds` and `WarpFisheye` are recorded but not applied.
//!
//! Value convention: list 1 runs on raw sample values (16-bit scale), lists 2 and 3 on values normalised to
//! [0, 1]; table/polynomial/delta opcodes are defined on the normalised range and are rescaled accordingly.

use crate::{Cfa, Rgb32f};
use serde::{Deserialize, Serialize};

/// Rectangular area + plane selection + pitch shared by the "area" opcodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
    pub plane: u32,
    pub planes: u32,
    pub row_pitch: u32,
    pub col_pitch: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Opcode {
    /// Per-plane radial (kr0..kr3) + tangential (kt0, kt1) coefficients and the relative optical centre.
    WarpRectilinear {
        planes: Vec<[f64; 6]>,
        center: [f64; 2],
    },
    WarpFisheye {
        planes: Vec<[f64; 4]>,
        center: [f64; 2],
    },
    FixVignetteRadial {
        k: [f64; 5],
        center: [f64; 2],
    },
    FixBadPixelsConstant {
        constant: u32,
        bayer_phase: u32,
    },
    FixBadPixelsList {
        bayer_phase: u32,
        points: Vec<(u32, u32)>,
        rects: Vec<[u32; 4]>,
    },
    TrimBounds {
        top: u32,
        left: u32,
        bottom: u32,
        right: u32,
    },
    MapTable {
        area: Area,
        table: Vec<u16>,
    },
    MapPolynomial {
        area: Area,
        coefficients: Vec<f64>,
    },
    GainMap {
        area: Area,
        points_v: u32,
        points_h: u32,
        spacing: [f64; 2],
        origin: [f64; 2],
        map_planes: u32,
        gains: Vec<f32>,
    },
    DeltaPerRow {
        area: Area,
        deltas: Vec<f32>,
    },
    DeltaPerColumn {
        area: Area,
        deltas: Vec<f32>,
    },
    ScalePerRow {
        area: Area,
        scales: Vec<f32>,
    },
    ScalePerColumn {
        area: Area,
        scales: Vec<f32>,
    },
    Unknown {
        id: u32,
        flags: u32,
        params: Vec<u8>,
    },
}

impl Opcode {
    /// Whether [`apply_list`] / [`apply_list3`] implement this opcode.
    pub fn is_applied(&self) -> bool {
        !matches!(self, Opcode::Unknown { .. } | Opcode::TrimBounds { .. } | Opcode::WarpFisheye { .. })
    }
}

/// The three opcode lists of a DNG raw IFD.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpcodeLists {
    pub list1: Vec<Opcode>,
    pub list2: Vec<Opcode>,
    pub list3: Vec<Opcode>,
}

struct Rd<'a> {
    d: &'a [u8],
    p: usize,
}

impl Rd<'_> {
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_be_bytes(self.d.get(self.p..self.p + 4)?.try_into().ok()?);
        self.p += 4;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_be_bytes(self.d.get(self.p..self.p + 2)?.try_into().ok()?);
        self.p += 2;
        Some(v)
    }
    fn f64(&mut self) -> Option<f64> {
        let v = f64::from_be_bytes(self.d.get(self.p..self.p + 8)?.try_into().ok()?);
        self.p += 8;
        Some(v)
    }
    fn f32(&mut self) -> Option<f32> {
        let v = f32::from_be_bytes(self.d.get(self.p..self.p + 4)?.try_into().ok()?);
        self.p += 4;
        Some(v)
    }
    fn area(&mut self) -> Option<Area> {
        Some(Area {
            top: self.u32()?,
            left: self.u32()?,
            bottom: self.u32()?,
            right: self.u32()?,
            plane: self.u32()?,
            planes: self.u32()?,
            row_pitch: self.u32()?.max(1),
            col_pitch: self.u32()?.max(1),
        })
    }
    fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }
}

fn parse_one(id: u32, flags: u32, params: &[u8]) -> Option<Opcode> {
    let mut r = Rd { d: params, p: 0 };
    let f = |r: &mut Rd| r.f64().filter(|v| v.is_finite());
    Some(match id {
        1 => {
            let n = r.u32()? as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let mut planes = Vec::with_capacity(n);
            for _ in 0..n {
                planes.push([f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?]);
            }
            Opcode::WarpRectilinear { planes, center: [f(&mut r)?, f(&mut r)?] }
        }
        2 => {
            let n = r.u32()? as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let mut planes = Vec::with_capacity(n);
            for _ in 0..n {
                planes.push([f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?]);
            }
            Opcode::WarpFisheye { planes, center: [f(&mut r)?, f(&mut r)?] }
        }
        3 => Opcode::FixVignetteRadial { k: [f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?, f(&mut r)?], center: [f(&mut r)?, f(&mut r)?] },
        4 => Opcode::FixBadPixelsConstant { constant: r.u32()?, bayer_phase: r.u32()? },
        5 => {
            let bayer_phase = r.u32()?;
            let (np, nr) = (r.u32()? as usize, r.u32()? as usize);
            if np.saturating_mul(8).saturating_add(nr.saturating_mul(16)) > r.remaining() {
                return None;
            }
            let points = (0..np).map(|_| Some((r.u32()?, r.u32()?))).collect::<Option<_>>()?;
            let rects = (0..nr).map(|_| Some([r.u32()?, r.u32()?, r.u32()?, r.u32()?])).collect::<Option<_>>()?;
            Opcode::FixBadPixelsList { bayer_phase, points, rects }
        }
        6 => Opcode::TrimBounds { top: r.u32()?, left: r.u32()?, bottom: r.u32()?, right: r.u32()? },
        7 => {
            let area = r.area()?;
            let n = r.u32()? as usize;
            if n == 0 || n > 65536 || n * 2 > r.remaining() {
                return None;
            }
            Opcode::MapTable { area, table: (0..n).map(|_| r.u16()).collect::<Option<_>>()? }
        }
        8 => {
            let area = r.area()?;
            let deg = r.u32()? as usize;
            if deg > 8 {
                return None;
            }
            Opcode::MapPolynomial { area, coefficients: (0..=deg).map(|_| f(&mut r)).collect::<Option<_>>()? }
        }
        9 => {
            let area = r.area()?;
            let (pv, ph) = (r.u32()?, r.u32()?);
            let spacing = [f(&mut r)?, f(&mut r)?];
            let origin = [f(&mut r)?, f(&mut r)?];
            let mp = r.u32()?;
            let n = (pv as usize).checked_mul(ph as usize)?.checked_mul(mp as usize)?;
            if n == 0 || n * 4 > r.remaining() {
                return None;
            }
            let gains = (0..n).map(|_| r.f32()).collect::<Option<_>>()?;
            Opcode::GainMap { area, points_v: pv, points_h: ph, spacing, origin, map_planes: mp, gains }
        }
        10..=13 => {
            let area = r.area()?;
            let n = r.u32()? as usize;
            if n * 4 > r.remaining() {
                return None;
            }
            let v: Vec<f32> = (0..n).map(|_| r.f32()).collect::<Option<_>>()?;
            match id {
                10 => Opcode::DeltaPerRow { area, deltas: v },
                11 => Opcode::DeltaPerColumn { area, deltas: v },
                12 => Opcode::ScalePerRow { area, scales: v },
                _ => Opcode::ScalePerColumn { area, scales: v },
            }
        }
        _ => Opcode::Unknown { id, flags, params: params.to_vec() },
    })
}

fn put_area(o: &mut Vec<u8>, a: &Area) {
    for v in [a.top, a.left, a.bottom, a.right, a.plane, a.planes, a.row_pitch, a.col_pitch] {
        o.extend_from_slice(&v.to_be_bytes());
    }
}

fn params(op: &Opcode) -> (u32, u32, Vec<u8>) {
    let mut o = Vec::new();
    let f64s = |o: &mut Vec<u8>, v: &[f64]| v.iter().for_each(|x| o.extend_from_slice(&x.to_be_bytes()));
    let u32s = |o: &mut Vec<u8>, v: &[u32]| v.iter().for_each(|x| o.extend_from_slice(&x.to_be_bytes()));
    let f32s = |o: &mut Vec<u8>, v: &[f32]| v.iter().for_each(|x| o.extend_from_slice(&x.to_be_bytes()));
    let id = match op {
        Opcode::WarpRectilinear { planes, center } => {
            u32s(&mut o, &[planes.len() as u32]);
            planes.iter().for_each(|p| f64s(&mut o, p));
            f64s(&mut o, center);
            1
        }
        Opcode::WarpFisheye { planes, center } => {
            u32s(&mut o, &[planes.len() as u32]);
            planes.iter().for_each(|p| f64s(&mut o, p));
            f64s(&mut o, center);
            2
        }
        Opcode::FixVignetteRadial { k, center } => {
            f64s(&mut o, k);
            f64s(&mut o, center);
            3
        }
        Opcode::FixBadPixelsConstant { constant, bayer_phase } => {
            u32s(&mut o, &[*constant, *bayer_phase]);
            4
        }
        Opcode::FixBadPixelsList { bayer_phase, points, rects } => {
            u32s(&mut o, &[*bayer_phase, points.len() as u32, rects.len() as u32]);
            points.iter().for_each(|&(r, c)| u32s(&mut o, &[r, c]));
            rects.iter().for_each(|r| u32s(&mut o, r));
            5
        }
        Opcode::TrimBounds { top, left, bottom, right } => {
            u32s(&mut o, &[*top, *left, *bottom, *right]);
            6
        }
        Opcode::MapTable { area, table } => {
            put_area(&mut o, area);
            u32s(&mut o, &[table.len() as u32]);
            table.iter().for_each(|v| o.extend_from_slice(&v.to_be_bytes()));
            7
        }
        Opcode::MapPolynomial { area, coefficients } => {
            put_area(&mut o, area);
            u32s(&mut o, &[coefficients.len().saturating_sub(1) as u32]);
            f64s(&mut o, coefficients);
            8
        }
        Opcode::GainMap { area, points_v, points_h, spacing, origin, map_planes, gains } => {
            put_area(&mut o, area);
            u32s(&mut o, &[*points_v, *points_h]);
            f64s(&mut o, spacing);
            f64s(&mut o, origin);
            u32s(&mut o, &[*map_planes]);
            f32s(&mut o, gains);
            9
        }
        Opcode::DeltaPerRow { area, deltas: v }
        | Opcode::DeltaPerColumn { area, deltas: v }
        | Opcode::ScalePerRow { area, scales: v }
        | Opcode::ScalePerColumn { area, scales: v } => {
            put_area(&mut o, area);
            u32s(&mut o, &[v.len() as u32]);
            f32s(&mut o, v);
            match op {
                Opcode::DeltaPerRow { .. } => 10,
                Opcode::DeltaPerColumn { .. } => 11,
                Opcode::ScalePerRow { .. } => 12,
                _ => 13,
            }
        }
        Opcode::Unknown { id, flags, params } => return (*id, *flags, params.clone()),
    };
    (id, 0, o)
}

/// Serialise an opcode list (inverse of [`parse_list`]).
pub fn write_list(list: &[Opcode]) -> Vec<u8> {
    let mut out = (list.len() as u32).to_be_bytes().to_vec();
    for op in list {
        let (id, flags, p) = params(op);
        for v in [id, 0x0103_0000, flags, p.len() as u32] {
            out.extend_from_slice(&v.to_be_bytes());
        }
        out.extend_from_slice(&p);
    }
    out
}

/// Parse an opcode list blob. Malformed opcodes become [`Opcode::Unknown`]; a truncated list stops early.
pub fn parse_list(d: &[u8]) -> Vec<Opcode> {
    let mut r = Rd { d, p: 0 };
    let Some(count) = r.u32() else { return vec![] };
    let mut out = Vec::new();
    for _ in 0..count.min(4096) {
        let (Some(id), Some(_ver), Some(flags), Some(size)) = (r.u32(), r.u32(), r.u32(), r.u32()) else { break };
        let Some(params) = d.get(r.p..r.p.saturating_add(size as usize)) else { break };
        r.p += size as usize;
        out.push(parse_one(id, flags, params).unwrap_or(Opcode::Unknown { id, flags, params: params.to_vec() }));
    }
    out
}

fn area_iter(a: &Area, w: usize, h: usize, cpp: usize) -> impl Iterator<Item = (usize, usize, usize)> + '_ {
    let (t, l) = (a.top as usize, a.left as usize);
    let (b, r) = ((a.bottom as usize).min(h), (a.right as usize).min(w));
    let (p0, p1) = (a.plane as usize, (a.plane as usize + a.planes.max(1) as usize).min(cpp));
    (t..b.max(t))
        .step_by(a.row_pitch.max(1) as usize)
        .flat_map(move |y| (l..r.max(l)).step_by(a.col_pitch.max(1) as usize).flat_map(move |x| (p0..p1.max(p0)).map(move |p| (x, y, p))))
}

fn gain_at(points_v: u32, points_h: u32, spacing: [f64; 2], origin: [f64; 2], map_planes: u32, gains: &[f32], rv: f64, rh: f64, plane: usize) -> f32 {
    let (pv, ph) = (points_v as usize, points_h as usize);
    let mp = map_planes as usize;
    let p = plane.min(mp - 1);
    let fy = if spacing[0] > 0.0 { ((rv - origin[0]) / spacing[0]).clamp(0.0, (pv - 1) as f64) } else { 0.0 };
    let fx = if spacing[1] > 0.0 { ((rh - origin[1]) / spacing[1]).clamp(0.0, (ph - 1) as f64) } else { 0.0 };
    let (y0, x0) = (fy.floor() as usize, fx.floor() as usize);
    let (y1, x1) = ((y0 + 1).min(pv - 1), (x0 + 1).min(ph - 1));
    let (ty, tx) = ((fy - y0 as f64) as f32, (fx - x0 as f64) as f32);
    let g = |y: usize, x: usize| gains[(y * ph + x) * mp + p];
    let top = g(y0, x0) + (g(y0, x1) - g(y0, x0)) * tx;
    let bot = g(y1, x0) + (g(y1, x1) - g(y1, x0)) * tx;
    top + (bot - top) * ty
}

/// Radius normaliser: distance from the centre to the farthest image corner.
fn max_radius(cx: f64, cy: f64, w: f64, h: f64) -> f64 {
    [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|&(x, y)| (x - cx).hypot(y - cy)).fold(0.0, f64::max).max(1e-9)
}

/// Apply a list to an interleaved `w × h × cpp` buffer. `cfa` (when the data is a mosaic) is used by the
/// bad-pixel opcodes. `scale` is the value of 1.0 in the buffer: 65535 for list 1 (raw 16-bit values), 1 for
/// lists 2 and 3 (normalised values).
pub fn apply_list(list: &[Opcode], buf: &mut [f32], w: usize, h: usize, cpp: usize, cfa: Option<&Cfa>, scale: f32) {
    for op in list {
        match op {
            Opcode::MapTable { area, table } => {
                let n = table.len();
                for (x, y, p) in area_iter(area, w, h, cpp) {
                    let v = &mut buf[(y * w + x) * cpp + p];
                    let idx = ((*v / scale * 65535.0).round().max(0.0) as usize).min(n - 1);
                    *v = table[idx] as f32 / 65535.0 * scale;
                }
            }
            Opcode::MapPolynomial { area, coefficients } => {
                for (x, y, p) in area_iter(area, w, h, cpp) {
                    let v = &mut buf[(y * w + x) * cpp + p];
                    let t = (*v / scale) as f64;
                    let r = coefficients.iter().rev().fold(0.0, |acc, &c| acc * t + c);
                    *v = r as f32 * scale;
                }
            }
            Opcode::GainMap { area, points_v, points_h, spacing, origin, map_planes, gains } => {
                for (x, y, p) in area_iter(area, w, h, cpp) {
                    let rv = y as f64 / h as f64;
                    let rh = x as f64 / w as f64;
                    let g = gain_at(*points_v, *points_h, *spacing, *origin, *map_planes, gains, rv, rh, p - area.plane as usize);
                    buf[(y * w + x) * cpp + p] *= g;
                }
            }
            Opcode::DeltaPerRow { area, deltas } | Opcode::DeltaPerColumn { area, deltas } => {
                let per_row = matches!(op, Opcode::DeltaPerRow { .. });
                for (x, y, p) in area_iter(area, w, h, cpp) {
                    let i =
                        if per_row { (y - area.top as usize) / area.row_pitch as usize } else { (x - area.left as usize) / area.col_pitch as usize };
                    if let Some(d) = deltas.get(i) {
                        buf[(y * w + x) * cpp + p] += d * scale;
                    }
                }
            }
            Opcode::ScalePerRow { area, scales } | Opcode::ScalePerColumn { area, scales } => {
                let per_row = matches!(op, Opcode::ScalePerRow { .. });
                for (x, y, p) in area_iter(area, w, h, cpp) {
                    let i =
                        if per_row { (y - area.top as usize) / area.row_pitch as usize } else { (x - area.left as usize) / area.col_pitch as usize };
                    if let Some(s) = scales.get(i) {
                        buf[(y * w + x) * cpp + p] *= s;
                    }
                }
            }
            Opcode::FixVignetteRadial { k, center } => {
                let (cx, cy) = (center[0] * w as f64, center[1] * h as f64);
                let m = max_radius(cx, cy, w as f64, h as f64);
                for y in 0..h {
                    for x in 0..w {
                        let r2 = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)) / (m * m);
                        let g = 1.0 + r2 * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4]))));
                        for p in 0..cpp {
                            buf[(y * w + x) * cpp + p] *= g as f32;
                        }
                    }
                }
            }
            Opcode::FixBadPixelsConstant { constant, .. } => {
                let c = (*constant as f64 / 65535.0 * scale as f64) as f32;
                let bad: Vec<(usize, usize)> =
                    (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| cpp == 1 && buf[y * w + x] == c).collect();
                fix_pixels(buf, w, h, cpp, cfa, &bad);
            }
            Opcode::FixBadPixelsList { points, rects, .. } => {
                let mut bad: Vec<(usize, usize)> = points.iter().map(|&(r, c)| (c as usize, r as usize)).filter(|&(x, y)| x < w && y < h).collect();
                for r in rects {
                    let [t, l, b, rr] = r.map(|v| v as usize);
                    for y in t..b.min(h) {
                        for x in l..rr.min(w) {
                            bad.push((x, y));
                        }
                    }
                    if bad.len() > 1 << 22 {
                        break;
                    }
                }
                fix_pixels(buf, w, h, cpp, cfa, &bad);
            }
            _ => {}
        }
    }
}

/// Replace listed pixels with the mean of the nearest same-colour neighbours that are not themselves bad.
fn fix_pixels(buf: &mut [f32], w: usize, h: usize, cpp: usize, cfa: Option<&Cfa>, bad: &[(usize, usize)]) {
    if bad.is_empty() {
        return;
    }
    let mut is_bad = std::collections::HashSet::with_capacity(bad.len());
    for &b in bad {
        is_bad.insert(b);
    }
    let step: isize = match cfa {
        Some(c) if c.width == 2 && c.height == 2 => 2,
        Some(c) => c.width.max(c.height) as isize,
        None => 1,
    };
    for &(x, y) in bad {
        for p in 0..cpp {
            let mut sum = 0.0;
            let mut n = 0;
            for (dx, dy) in [(-step, 0), (step, 0), (0, -step), (0, step), (-step, -step), (step, step), (-step, step), (step, -step)] {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize || is_bad.contains(&(nx as usize, ny as usize)) {
                    continue;
                }
                sum += buf[(ny as usize * w + nx as usize) * cpp + p];
                n += 1;
            }
            if n > 0 {
                buf[(y * w + x) * cpp + p] = sum / n as f32;
            }
        }
    }
}

/// Apply `OpcodeList3` (on the demosaiced RGB image, values normalised to [0, 1]).
pub fn apply_list3(list: &[Opcode], img: &mut Rgb32f) {
    if list.is_empty() {
        return;
    }
    let (w, h) = (img.width, img.height);
    for op in list {
        match op {
            Opcode::WarpRectilinear { planes, center } => *img = warp_rectilinear(img, planes, *center),
            Opcode::WarpFisheye { .. } | Opcode::TrimBounds { .. } | Opcode::Unknown { .. } => {}
            other => {
                let mut flat: Vec<f32> = img.data.iter().flat_map(|p| *p).collect();
                apply_list(std::slice::from_ref(other), &mut flat, w, h, 3, None, 1.0);
                for (d, c) in img.data.iter_mut().zip(flat.as_chunks::<3>().0) {
                    *d = [c[0], c[1], c[2]];
                }
            }
        }
    }
}

/// DNG `WarpRectilinear`: for each output pixel, the source position is the radial + tangential model around the
/// optical centre, with coordinates normalised by the distance to the farthest corner.
pub fn warp_rectilinear(img: &Rgb32f, planes: &[[f64; 6]], center: [f64; 2]) -> Rgb32f {
    let (w, h) = (img.width, img.height);
    let (cx, cy) = (center[0] * (w as f64 - 1.0), center[1] * (h as f64 - 1.0));
    let m = max_radius(cx, cy, w as f64 - 1.0, h as f64 - 1.0);
    let mut out = Rgb32f::new(w, h);
    let coef = |p: usize| planes[p.min(planes.len() - 1)];
    lightcraft_raster::par_rows(&mut out.data, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            for (c, v) in px.iter_mut().enumerate() {
                let [kr0, kr1, kr2, kr3, kt0, kt1] = coef(c);
                let dx = (x as f64 - cx) / m;
                let dy = (y as f64 - cy) / m;
                let r2 = dx * dx + dy * dy;
                let f = kr0 + r2 * (kr1 + r2 * (kr2 + r2 * kr3));
                let sx = dx * f + kt0 * 2.0 * dx * dy + kt1 * (r2 + 2.0 * dx * dx);
                let sy = dy * f + kt1 * 2.0 * dx * dy + kt0 * (r2 + 2.0 * dy * dy);
                let (fx, fy) = (cx + m * sx, cy + m * sy);
                *v = img.sample_bilinear(fx as f32 + 0.5, fy as f32 + 0.5)[c];
            }
        }
    });
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) struct ListWriter(pub Vec<u8>, pub u32);

    impl ListWriter {
        pub fn new() -> Self {
            ListWriter(Vec::new(), 0)
        }
        pub fn op(&mut self, id: u32, params: &[u8]) -> &mut Self {
            for v in [id, 0x01030000, 0, params.len() as u32] {
                self.0.extend_from_slice(&v.to_be_bytes());
            }
            self.0.extend_from_slice(params);
            self.1 += 1;
            self
        }
        pub fn finish(&self) -> Vec<u8> {
            let mut v = self.1.to_be_bytes().to_vec();
            v.extend_from_slice(&self.0);
            v
        }
    }

    pub(crate) fn be(vals: &[f64]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_be_bytes()).collect()
    }
    pub(crate) fn area(a: [u32; 8]) -> Vec<u8> {
        a.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn parse_and_apply_area_ops() {
        let (w, h) = (4usize, 3usize);
        let mut gm = area([0, 0, 3, 4, 0, 1, 1, 1]);
        for v in [2u32, 2] {
            gm.extend_from_slice(&v.to_be_bytes());
        }
        gm.extend(be(&[1.0, 1.0, 0.0, 0.0]));
        gm.extend_from_slice(&1u32.to_be_bytes());
        for g in [1.0f32, 2.0, 1.0, 2.0] {
            gm.extend_from_slice(&g.to_be_bytes());
        }
        let mut dpr = area([0, 0, 3, 4, 0, 1, 1, 1]);
        dpr.extend_from_slice(&3u32.to_be_bytes());
        for d in [0.0f32, 0.1, 0.2] {
            dpr.extend_from_slice(&d.to_be_bytes());
        }
        let mut poly = area([0, 0, 3, 4, 0, 1, 1, 2]);
        poly.extend_from_slice(&1u32.to_be_bytes());
        poly.extend(be(&[0.0, 2.0]));
        let blob = ListWriter::new().op(9, &gm).op(10, &dpr).op(8, &poly).op(99, &[1, 2, 3]).finish();
        let list = parse_list(&blob);
        assert_eq!(list.len(), 4);
        assert!(matches!(list[3], Opcode::Unknown { id: 99, .. }));
        assert!(list[..3].iter().all(|o| o.is_applied()));
        let mut buf = vec![0.25f32; w * h];
        apply_list(&list, &mut buf, w, h, 1, None, 1.0);
        // gain map: horizontal ramp 1 → 2 across relative x 0..1 (x / w)
        let expect = |x: usize, y: usize| {
            let g = 1.0 + x as f32 / w as f32;
            let v = 0.25 * g + [0.0, 0.1, 0.2][y];
            if x.is_multiple_of(2) { v * 2.0 } else { v }
        };
        for y in 0..h {
            for x in 0..w {
                assert!((buf[y * w + x] - expect(x, y)).abs() < 1e-6, "{x},{y}: {} vs {}", buf[y * w + x], expect(x, y));
            }
        }
    }

    #[test]
    fn map_table_raw_scale_and_bad_pixels() {
        let mut t = area([0, 0, 2, 2, 0, 1, 1, 1]);
        t.extend_from_slice(&3u32.to_be_bytes());
        for v in [0u16, 30000, 65535] {
            t.extend_from_slice(&v.to_be_bytes());
        }
        let list = parse_list(&ListWriter::new().op(7, &t).finish());
        let mut buf = vec![0.0, 1.0 / 65535.0, 2.0 / 65535.0, 0.5];
        apply_list(&list, &mut buf, 2, 2, 1, None, 1.0);
        assert_eq!(buf[0], 0.0);
        assert!((buf[1] - 30000.0 / 65535.0).abs() < 1e-7);
        assert_eq!(buf[3], 1.0);

        let mut p = vec![];
        for v in [7u32, 0] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        let list = parse_list(&ListWriter::new().op(4, &p).finish());
        let cfa = Cfa::bayer("RGGB").unwrap();
        let mut img: Vec<f32> = (0..36).map(|i| 100.0 + (i % 6) as f32).collect();
        img[2 * 6 + 2] = 7.0;
        apply_list(&list, &mut img, 6, 6, 1, Some(&cfa), 65535.0);
        assert_eq!(img[2 * 6 + 2], 102.0);
    }

    #[test]
    fn vignette_and_warp_identity() {
        let mut img = Rgb32f::from_fn(9, 7, |x, y| [x as f32 / 9.0, y as f32 / 7.0, 0.5]);
        let orig = img.clone();
        let mut p = be(&[0.0; 5]);
        p.extend(be(&[0.5, 0.5]));
        let mut warp = 1u32.to_be_bytes().to_vec();
        warp.extend(be(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5]));
        let list = parse_list(&ListWriter::new().op(3, &p).op(1, &warp).finish());
        assert_eq!(list.len(), 2);
        apply_list3(&list, &mut img);
        for (a, b) in img.data.iter().zip(&orig.data) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-5);
            }
        }
        // positive k0 brightens the corners more than the centre
        let mut p = be(&[1.0, 0.0, 0.0, 0.0, 0.0]);
        p.extend(be(&[0.5, 0.5]));
        let list = parse_list(&ListWriter::new().op(3, &p).finish());
        let mut flat = Rgb32f::filled(9, 9, [0.5; 3]);
        apply_list3(&list, &mut flat);
        assert!(flat.get(0, 0)[0] > 0.85 && (flat.get(4, 4)[0] - 0.5).abs() < 0.01, "{:?}", flat.get(0, 0));
    }

    #[test]
    fn warp_barrel_moves_pixels_outward() {
        let img = Rgb32f::from_fn(31, 31, |x, y| if x == 24 && y == 15 { [1.0; 3] } else { [0.0; 3] });
        // source radius = r (1 + k r²) with k < 0 → content moves outward
        let out = warp_rectilinear(&img, &[[1.0, -0.6, 0.0, 0.0, 0.0, 0.0]], [0.5, 0.5]);
        let (bx, _) = (0..31).map(|x| (x, out.get(x, 15)[0])).fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a });
        assert!(bx >= 25, "{bx}");
    }

    #[test]
    fn write_parse_roundtrip() {
        let a = Area { top: 1, left: 2, bottom: 30, right: 40, plane: 0, planes: 3, row_pitch: 1, col_pitch: 2 };
        let list = vec![
            Opcode::WarpRectilinear { planes: vec![[1.0, 0.01, -0.002, 0.0, 1e-4, -2e-4]; 3], center: [0.5, 0.49] },
            Opcode::WarpFisheye { planes: vec![[1.0, 0.1, 0.0, 0.0]], center: [0.5, 0.5] },
            Opcode::FixVignetteRadial { k: [0.1, 0.2, 0.0, 0.0, 0.0], center: [0.5, 0.5] },
            Opcode::FixBadPixelsConstant { constant: 0, bayer_phase: 1 },
            Opcode::FixBadPixelsList { bayer_phase: 0, points: vec![(3, 4)], rects: vec![[1, 1, 2, 2]] },
            Opcode::TrimBounds { top: 1, left: 1, bottom: 9, right: 9 },
            Opcode::MapTable { area: a, table: vec![0, 100, 65535] },
            Opcode::MapPolynomial { area: a, coefficients: vec![0.0, 1.0, -0.1] },
            Opcode::GainMap { area: a, points_v: 2, points_h: 1, spacing: [1.0, 1.0], origin: [0.0, 0.0], map_planes: 1, gains: vec![1.0, 1.5] },
            Opcode::DeltaPerRow { area: a, deltas: vec![0.1, 0.2] },
            Opcode::DeltaPerColumn { area: a, deltas: vec![0.3] },
            Opcode::ScalePerRow { area: a, scales: vec![1.1] },
            Opcode::ScalePerColumn { area: a, scales: vec![0.9, 1.0] },
            Opcode::Unknown { id: 77, flags: 1, params: vec![1, 2, 3] },
        ];
        assert_eq!(parse_list(&write_list(&list)), list);
    }

    #[test]
    fn truncated_lists_do_not_panic() {
        let mut gm = area([0, 0, 3, 4, 0, 1, 1, 1]);
        gm.extend_from_slice(&[0xff; 64]);
        let blob = ListWriter::new().op(9, &gm).op(5, &[0xff; 12]).op(1, &[0, 0, 0, 9]).finish();
        for n in 0..blob.len() {
            let l = parse_list(&blob[..n]);
            let mut buf = vec![0.5f32; 12];
            apply_list(&l, &mut buf, 4, 3, 1, None, 1.0);
        }
    }
}
