//! DNG camera-profile look data carried by the file itself (Adobe DNG Specification 1.6/1.7,
//! chapter 6 and the `ProfileHueSatMap*`, `ProfileLookTable*` and `ProfileToneCurve` tags).
//!
//! - A hue/saturation/value table holds, for a grid of (value, hue, saturation) inputs, a hue shift
//!   in degrees, a saturation scale and a value scale (value-major, hue-middle, saturation-minor).
//!   Hue samples are spread evenly around the circle with wrap-around, saturation and value samples
//!   evenly over 0..=1; a dimension with one division is constant. Lookups are tri-linear.
//! - Tables are applied in HSV of linear ProPhoto RGB (D50). With encoding 1 ("sRGB") the value
//!   axis is indexed through the sRGB transfer curve (3D tables only).
//! - `ProfileHueSatMapData1/2` follow the colour matrices (interpolated by the same illuminant
//!   weight); `ProfileLookTableData` comes after exposure compensation and before any tone curve.
//! - `ProfileToneCurve`: (input, output) pairs in linear gamma from (0, 0) to (1, 1).
//!
//! The data is read from the user's own file at run time; nothing here ships profile data.
//! Values above 1.0 (highlight headroom) keep their headroom: the table's value scale only ever
//! lowers them, so a lookup can't clip what the scene-referred pipeline still needs.

use lightcraft_color::transfer::{linear_to_srgb, srgb_to_linear};
use lightcraft_color::{D50, D65, Mat3, PROPHOTO, REC2020, bradford};
use serde::{Deserialize, Serialize};

/// Upper bound on table entries (Adobe profiles use ≤ 90 × 30 × 16; guards header-driven sizes).
const MAX_ENTRIES: usize = 1 << 20;

/// A DNG hue/saturation/value mapping table (`ProfileHueSatMapData*` / `ProfileLookTableData`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HsvTable {
    pub hue_divisions: usize,
    pub sat_divisions: usize,
    pub val_divisions: usize,
    /// `(hue shift °, saturation scale, value scale)`, value-major, hue-middle, saturation-minor.
    pub data: Vec<[f32; 3]>,
    /// The value axis is indexed through the sRGB encoding curve (`…Encoding` = 1).
    pub srgb_value: bool,
}

impl HsvTable {
    /// A table from its `…Dims` (hue, saturation, value divisions), data floats and encoding.
    /// `None` when the dimensions are out of range, the data length doesn't match or a value is
    /// not finite.
    pub fn from_tags(dims: &[u64], data: &[f64], encoding: u64) -> Option<HsvTable> {
        let [h, s, v] = <[u64; 3]>::try_from(dims).ok()?;
        let (h, s, v) = (usize::try_from(h).ok()?, usize::try_from(s).ok()?, usize::try_from(v).ok()?);
        if h < 1 || s < 2 || v < 1 || h > 4096 || s > 4096 || v > 4096 {
            return None;
        }
        let n = h.checked_mul(s)?.checked_mul(v)?;
        if n > MAX_ENTRIES || data.len() != n * 3 || data.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let data = data.as_chunks::<3>().0.iter().map(|c| [c[0] as f32, (c[1] as f32).max(0.0), (c[2] as f32).max(0.0)]).collect();
        Some(HsvTable { hue_divisions: h, sat_divisions: s, val_divisions: v, data, srgb_value: encoding == 1 && v > 1 })
    }

    /// The entry-wise blend `g · a + (1 − g) · b` of two tables of the same shape (`None` otherwise).
    pub fn blend(a: &HsvTable, b: &HsvTable, g: f64) -> Option<HsvTable> {
        if (a.hue_divisions, a.sat_divisions, a.val_divisions) != (b.hue_divisions, b.sat_divisions, b.val_divisions) {
            return None;
        }
        let g = g.clamp(0.0, 1.0) as f32;
        let data = a.data.iter().zip(&b.data).map(|(x, y)| std::array::from_fn(|i| g * x[i] + (1.0 - g) * y[i])).collect();
        Some(HsvTable { data, ..a.clone() })
    }

    fn entry(&self, v: usize, h: usize, s: usize) -> [f32; 3] {
        self.data.get((v * self.hue_divisions + h) * self.sat_divisions + s).copied().unwrap_or([0.0, 1.0, 1.0])
    }

    /// Tri-linear lookup at hue `h` (degrees), saturation `s` and value `v` (table coordinates).
    pub fn lookup(&self, h: f32, s: f32, v: f32) -> [f32; 3] {
        let hf = (h.rem_euclid(360.0) / 360.0 * self.hue_divisions as f32).clamp(0.0, self.hue_divisions as f32);
        let h0 = (hf as usize).min(self.hue_divisions - 1);
        let h1 = (h0 + 1) % self.hue_divisions;
        let th = (hf - h0 as f32).clamp(0.0, 1.0);
        let axis = |x: f32, n: usize| {
            let f = x.clamp(0.0, 1.0) * (n - 1) as f32;
            let i0 = (f as usize).min(n.saturating_sub(2));
            let i1 = (i0 + 1).min(n - 1);
            (i0, i1, (f - i0 as f32).clamp(0.0, 1.0))
        };
        let (s0, s1, ts) = axis(s, self.sat_divisions);
        let (v0, v1, tv) = if self.val_divisions > 1 { axis(v, self.val_divisions) } else { (0, 0, 0.0) };
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] { std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t) };
        let plane = |vi: usize| {
            let lo = lerp(self.entry(vi, h0, s0), self.entry(vi, h0, s1), ts);
            let hi = lerp(self.entry(vi, h1, s0), self.entry(vi, h1, s1), ts);
            lerp(lo, hi, th)
        };
        if v0 == v1 { plane(v0) } else { lerp(plane(v0), plane(v1), tv) }
    }

    /// Apply the table to one linear ProPhoto RGB colour.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        if max.is_nan() || max <= 0.0 || !max.is_finite() {
            return rgb;
        }
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        let d = max - min;
        let hue = hue_of(rgb, max, d);
        let sat = d / max;
        let v_index = if self.srgb_value { linear_to_srgb(max.min(1.0)) } else { max };
        let [shift, ss, vs] = self.lookup(hue, sat, v_index);
        // saturation and value: scaled and clipped to 1.0 (DNG chapter 6), except that inputs
        // already above 1 (out-of-ProPhoto colours, highlight headroom) are only ever lowered.
        let s2 = limited(sat, ss);
        let v2 = if self.srgb_value && max <= 1.0 {
            srgb_to_linear((linear_to_srgb(max) * vs).min(1.0))
        } else if self.srgb_value {
            max * srgb_to_linear(vs.min(1.0)).min(1.0)
        } else {
            limited(max, vs)
        };
        hsv_to_rgb(hue + shift, s2, v2)
    }
}

/// `x · k` clipped to 1, for `x ≤ 1`; above 1, only scale factors below 1 apply (continuous at 1).
fn limited(x: f32, k: f32) -> f32 {
    if x <= 1.0 { (x * k).min(1.0) } else { x * k.min(1.0) }
}

fn hue_of(c: [f32; 3], max: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    h * 60.0
}

/// HSV → RGB for any saturation ≥ 0 (above 1 yields negative components, the inverse of
/// [`hue_of`] for out-of-gamut inputs).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let hp = h.rem_euclid(360.0) / 60.0;
    let sector = (hp as u32).min(5);
    let f = hp - sector as f32;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match sector {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// A DNG tone curve (`ProfileToneCurve`): linear-gamma (input, output) pairs, increasing input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneCurve {
    pub points: Vec<[f32; 2]>,
}

impl ToneCurve {
    /// From the tag's flat float pairs. `None` unless there are 2..=65536 finite pairs within
    /// 0..=1 with strictly increasing inputs.
    pub fn from_tag(v: &[f64]) -> Option<ToneCurve> {
        if !v.len().is_multiple_of(2) || v.len() < 4 || v.len() > 131_072 {
            return None;
        }
        let points: Vec<[f32; 2]> = v.as_chunks::<2>().0.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
        let ok = points.iter().all(|p| p.iter().all(|x| x.is_finite() && (0.0..=1.0).contains(x))) && points.windows(2).all(|w| w[1][0] > w[0][0]);
        ok.then_some(ToneCurve { points })
    }

    /// The curve at `x` (0..=1): monotone cubic (Fritsch–Carlson) through the samples.
    pub fn eval(&self, x: f32) -> f32 {
        let p = &self.points;
        let n = p.len();
        let x = x.clamp(p[0][0], p[n - 1][0]);
        let i = p.partition_point(|q| q[0] <= x).clamp(1, n - 1) - 1;
        let slope = |k: usize| (p[k + 1][1] - p[k][1]) / (p[k + 1][0] - p[k][0]);
        let tangent = |k: usize| -> f32 {
            if k == 0 {
                return slope(0);
            }
            if k == n - 1 {
                return slope(n - 2);
            }
            let (a, b) = (slope(k - 1), slope(k));
            if a * b <= 0.0 { 0.0 } else { 2.0 / (1.0 / a + 1.0 / b) }
        };
        let (x0, x1) = (p[i][0], p[i + 1][0]);
        let h = x1 - x0;
        let t = (x - x0) / h;
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[i][1]
            + (t3 - 2.0 * t2 + t) * h * tangent(i)
            + (-2.0 * t3 + 3.0 * t2) * p[i + 1][1]
            + (t3 - t2) * h * tangent(i + 1);
        y.clamp(0.0, 1.0)
    }
}

/// The DNG profile look tags of a file (all optional).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileLook {
    /// `ProfileHueSatMapData1/2` (same shape, `ProfileHueSatMapDims`).
    pub hue_sat_map: [Option<HsvTable>; 2],
    /// `ProfileLookTableData`.
    pub look_table: Option<HsvTable>,
    /// `ProfileToneCurve`.
    pub tone_curve: Option<ToneCurve>,
}

impl ProfileLook {
    pub fn is_empty(&self) -> bool {
        self.hue_sat_map.iter().all(Option::is_none) && self.look_table.is_none() && self.tone_curve.is_none()
    }

    /// The hue/saturation map for calibration-1 weight `g` (one table: used for any white).
    pub fn hue_sat_for(&self, g: f64) -> Option<HsvTable> {
        match &self.hue_sat_map {
            [Some(a), Some(b)] => HsvTable::blend(a, b, g).or_else(|| Some(a.clone())),
            [Some(a), None] | [None, Some(a)] => Some(a.clone()),
            [None, None] => None,
        }
    }
}

/// The tables resolved for one white balance, applied to linear Rec.2020 D65 pixels.
#[derive(Clone, Debug)]
pub struct ProfileTables {
    hue_sat: Option<HsvTable>,
    look: Option<HsvTable>,
    to_prophoto: [[f32; 3]; 3],
    from_prophoto: [[f32; 3]; 3],
}

impl ProfileTables {
    /// `None` when the profile has no hue/saturation map and no look table.
    pub fn new(look: &ProfileLook, illuminant_weight: f64) -> Option<ProfileTables> {
        let hue_sat = look.hue_sat_for(illuminant_weight);
        if hue_sat.is_none() && look.look_table.is_none() {
            return None;
        }
        let to: Mat3 = PROPHOTO.from_xyz().mul(&bradford(D65, D50)).mul(&REC2020.to_xyz());
        let from = to.inverse()?;
        Some(ProfileTables { hue_sat, look: look.look_table.clone(), to_prophoto: to.to_f32(), from_prophoto: from.to_f32() })
    }

    /// Hue/saturation map, then `gain` (exposure compensation), then the look table:
    /// linear Rec.2020 D65 in, linear Rec.2020 D65 out.
    #[inline]
    pub fn apply(&self, rgb: [f32; 3], gain: f32) -> [f32; 3] {
        let m = &self.to_prophoto;
        let mut p: [f32; 3] = std::array::from_fn(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2]);
        if let Some(t) = &self.hue_sat {
            p = t.apply(p);
        }
        p = p.map(|v| v * gain);
        if let Some(t) = &self.look {
            p = t.apply(p);
        }
        let m = &self.from_prophoto;
        std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(h: usize, s: usize, v: usize) -> HsvTable {
        HsvTable { hue_divisions: h, sat_divisions: s, val_divisions: v, data: vec![[0.0, 1.0, 1.0]; h * s * v], srgb_value: false }
    }

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn identity_table_is_identity() {
        let t = identity(6, 3, 1);
        for c in [[0.2, 0.5, 0.1], [0.9, 0.9, 0.9], [0.0, 0.0, 0.4], [3.0, 1.0, 0.5], [0.3, -0.05, 0.2]] {
            assert!(close(t.apply(c), c, 1e-5), "{c:?} → {:?}", t.apply(c));
        }
        assert_eq!(t.apply([0.0; 3]), [0.0; 3]);
        assert_eq!(t.apply([-1.0, -2.0, -0.5]), [-1.0, -2.0, -0.5]);
    }

    #[test]
    fn parses_tags_and_rejects_bad_shapes() {
        let data: Vec<f64> = (0..4).flat_map(|_| [0.0, 1.0, 1.0]).collect();
        assert!(HsvTable::from_tags(&[2, 2, 1], &data, 0).is_some());
        assert!(HsvTable::from_tags(&[2, 2], &data, 0).is_none());
        assert!(HsvTable::from_tags(&[2, 1, 2], &data, 0).is_none(), "saturation needs ≥ 2 divisions");
        assert!(HsvTable::from_tags(&[2, 2, 2], &data, 0).is_none(), "length mismatch");
        assert!(HsvTable::from_tags(&[u64::MAX, 2, 2], &data, 0).is_none());
        let mut nan = data.clone();
        nan[1] = f64::NAN;
        assert!(HsvTable::from_tags(&[2, 2, 1], &nan, 0).is_none());
        // the sRGB encoding only applies to 3D tables
        assert!(!HsvTable::from_tags(&[2, 2, 1], &data, 1).unwrap().srgb_value);
    }

    #[test]
    fn saturation_scale_follows_the_table_layout() {
        // 4 hues (0°, 90°, 180°, 270°), 2 saturations; saturation-minor: entry (h, s) at h·2 + s.
        let mut t = identity(4, 2, 1);
        t.data[1] = [0.0, 1.5, 1.0]; // hue 0°, saturation 1 → ×1.5
        let red = [0.5, 0.4, 0.4]; // hue 0°, saturation 0.2
        let out = t.apply(red);
        // half-way along the saturation axis is ×1.1 (0.2 → 0.22 sat), value kept
        let sat = (out[0] - out[1].min(out[2])) / out[0];
        assert!((sat - 0.2 * 1.1).abs() < 1e-4, "{out:?}");
        assert!((out[0] - 0.5).abs() < 1e-6);
        // other hues are untouched (hue 180°)
        let cyan = [0.3, 0.4, 0.4];
        assert!(close(t.apply(cyan), cyan, 1e-6));
        // saturation clips to 1.0
        assert!(t.apply([1.0, 0.1, 0.1]).iter().all(|v| *v >= -1e-6));
    }

    #[test]
    fn hue_wraps_and_value_scales() {
        // 2 hues: entries at 0° and 180°, hue shift +20° at 0°, value ×0.5 at 180°.
        let mut t = identity(2, 2, 1);
        t.data = vec![[20.0, 1.0, 1.0], [20.0, 1.0, 1.0], [0.0, 1.0, 0.5], [0.0, 1.0, 0.5]];
        // 350° sits between the last sample (180°) and the first (0° = 360°), 10/180 from 360
        assert!((t.lookup(350.0, 0.5, 0.5)[0] - 20.0 * (170.0 / 180.0)).abs() < 1e-3);
        assert!((t.lookup(180.0, 0.5, 0.5)[2] - 0.5).abs() < 1e-6);
        // headroom above 1 only goes down
        let bright = [2.0, 2.4, 2.4];
        let out = t.apply(bright);
        assert!((out[1] - 1.2).abs() < 1e-5, "{out:?}");
        let mut up = identity(1, 2, 1);
        up.data = vec![[0.0, 1.0, 2.0]; 2];
        assert!((up.apply([0.4, 0.4, 0.4])[0] - 0.8).abs() < 1e-6);
        assert!((up.apply([0.8, 0.8, 0.8])[0] - 1.0).abs() < 1e-6, "clipped to 1");
        assert!((up.apply([3.0, 3.0, 3.0])[0] - 3.0).abs() < 1e-6, "headroom kept");
    }

    #[test]
    fn value_axis_and_srgb_encoding() {
        // 1 hue, 2 sats, 2 values: saturation ×2 at value 1 only
        let mut t = identity(1, 2, 2);
        t.data[3] = [0.0, 2.0, 1.0];
        let sat = |c: [f32; 3]| (c[0] - c[2]) / c[0];
        let dark = [0.25, 0.2, 0.2]; // V 0.25, S 0.2
        // linear: 1/4 of the way → S scale at s=0.2: lerp(1, lerp(1,2,0.2)=1.2, 0.25) = 1.05
        assert!((sat(t.apply(dark)) - 0.2 * 1.05).abs() < 1e-4);
        t.srgb_value = true;
        let k = linear_to_srgb(0.25); // ≈ 0.537
        let want = 0.2 * (1.0 + 0.2 * k);
        assert!((sat(t.apply(dark)) - want).abs() < 1e-4);
    }

    #[test]
    fn blend_and_tables_round_trip_through_prophoto() {
        let a = identity(2, 2, 1);
        let mut b = identity(2, 2, 1);
        b.data[1] = [10.0, 2.0, 1.0];
        let m = HsvTable::blend(&a, &b, 0.25).unwrap();
        assert_eq!(m.data[1], [7.5, 1.75, 1.0]);
        assert!(HsvTable::blend(&a, &identity(3, 2, 1), 0.5).is_none());
        let look = ProfileLook { hue_sat_map: [Some(a.clone()), None], look_table: Some(a), tone_curve: None };
        let t = ProfileTables::new(&look, 0.5).unwrap();
        let c = [0.3, 0.2, 0.1];
        assert!(close(t.apply(c, 2.0), [0.6, 0.4, 0.2], 1e-4), "{:?}", t.apply(c, 2.0));
        assert!(ProfileTables::new(&ProfileLook::default(), 0.5).is_none());
    }

    #[test]
    fn tone_curve_interpolates_monotonically() {
        assert!(ToneCurve::from_tag(&[0.0, 0.0]).is_none());
        assert!(ToneCurve::from_tag(&[0.0, 0.0, 0.5, 0.6, 0.4, 0.7, 1.0, 1.0]).is_none(), "inputs must increase");
        assert!(ToneCurve::from_tag(&[0.0, 0.0, 1.0, 1.5]).is_none());
        let c = ToneCurve::from_tag(&[0.0, 0.0, 0.25, 0.4, 0.5, 0.7, 1.0, 1.0]).unwrap();
        assert_eq!(c.eval(0.0), 0.0);
        assert!((c.eval(0.25) - 0.4).abs() < 1e-6);
        assert!((c.eval(1.0) - 1.0).abs() < 1e-6);
        let mut last = 0.0;
        for i in 0..=100 {
            let y = c.eval(i as f32 / 100.0);
            assert!(y >= last - 1e-6, "monotone at {i}");
            last = y;
        }
        assert!(c.eval(0.1) > 0.1 && c.eval(0.1) < 0.4);
    }
}
