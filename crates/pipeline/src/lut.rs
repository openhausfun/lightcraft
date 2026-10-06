//! 3D LUT profiles: `.cube` files (the plain-text Cube LUT format: `LUT_3D_SIZE`, optional
//! `DOMAIN_MIN` / `DOMAIN_MAX`, then size³ "r g b" lines with red changing fastest), applied to
//! the display-encoded output colour and blended by the profile amount. Profiles are registered
//! here by id (`lut:…`) and found by the finishing stage; LUT profiles render on the CPU.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// A 3D colour lookup table.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<[f32; 3]>,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub title: Option<String>,
}

impl Lut3d {
    /// Parse a `.cube` file.
    pub fn parse_cube(text: &str) -> Result<Lut3d, String> {
        let (mut size, mut title) = (0usize, None);
        let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
        let mut data = Vec::new();
        let three = |rest: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        };
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some(r) = l.strip_prefix("TITLE") {
                title = Some(r.trim().trim_matches('"').to_string());
            } else if let Some(r) = l.strip_prefix("LUT_3D_SIZE") {
                size = r.trim().parse().map_err(|_| format!("bad LUT_3D_SIZE `{}`", r.trim()))?;
                if !(2..=256).contains(&size) {
                    return Err(format!("LUT_3D_SIZE {size} out of range"));
                }
            } else if l.starts_with("LUT_1D_SIZE") {
                return Err("1D LUTs are not supported (only 3D .cube files)".into());
            } else if let Some(r) = l.strip_prefix("DOMAIN_MIN") {
                min = three(r).ok_or("bad DOMAIN_MIN")?;
            } else if let Some(r) = l.strip_prefix("DOMAIN_MAX") {
                max = three(r).ok_or("bad DOMAIN_MAX")?;
            } else if l.starts_with(|c: char| c.is_ascii_alphabetic()) {
                // other keywords (LUT_IN_VIDEO_RANGE…) are ignored
            } else {
                data.push(three(l).ok_or_else(|| format!("bad line `{l}`"))?);
            }
        }
        if size == 0 {
            return Err("not a 3D .cube LUT (no LUT_3D_SIZE)".into());
        }
        if data.len() != size * size * size {
            return Err(format!("expected {} entries, found {}", size * size * size, data.len()));
        }
        Ok(Lut3d { size, data, domain_min: min, domain_max: max, title })
    }

    /// Look up `c` (trilinear).
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let n = self.size;
        let s = (n - 1) as f32;
        let mut idx = [0usize; 3];
        let mut frac = [0.0f32; 3];
        for k in 0..3 {
            let span = (self.domain_max[k] - self.domain_min[k]).max(1e-6);
            let x = (((c[k] - self.domain_min[k]) / span).clamp(0.0, 1.0)) * s;
            let i = (x.floor() as usize).min(n - 2);
            idx[k] = i;
            frac[k] = x - i as f32;
        }
        let at = |r: usize, g: usize, b: usize| self.data[r + n * (g + n * b)];
        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
        let (r, g, b) = (idx[0], idx[1], idx[2]);
        let c00 = lerp(at(r, g, b), at(r + 1, g, b), frac[0]);
        let c10 = lerp(at(r, g + 1, b), at(r + 1, g + 1, b), frac[0]);
        let c01 = lerp(at(r, g, b + 1), at(r + 1, g, b + 1), frac[0]);
        let c11 = lerp(at(r, g + 1, b + 1), at(r + 1, g + 1, b + 1), frac[0]);
        lerp(lerp(c00, c10, frac[1]), lerp(c01, c11, frac[1]), frac[2])
    }
}

fn registry() -> &'static RwLock<HashMap<String, Arc<Lut3d>>> {
    static R: OnceLock<RwLock<HashMap<String, Arc<Lut3d>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// Make a LUT available to renders under profile id `id` (`lut:…`).
pub fn register(id: &str, lut: Lut3d) {
    registry().write().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Arc::new(lut));
}

/// The LUT registered for profile `id`.
pub fn get(id: &str) -> Option<Arc<Lut3d>> {
    if !id.starts_with("lut:") {
        return None;
    }
    registry().read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
}

/// Forget a LUT profile.
pub fn unregister(id: &str) {
    registry().write().unwrap_or_else(|e| e.into_inner()).remove(id);
}

/// Profile ids that render with a LUT (and so on the CPU).
pub fn is_lut_profile(id: &str) -> bool {
    id.starts_with("lut:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(f: impl Fn([f32; 3]) -> [f32; 3], n: usize) -> String {
        let mut s = format!("TITLE \"test\"\nLUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = f([r as f32 / (n - 1) as f32, g as f32 / (n - 1) as f32, b as f32 / (n - 1) as f32]);
                    s.push_str(&format!("{} {} {}\n", c[0], c[1], c[2]));
                }
            }
        }
        s
    }

    #[test]
    fn identity_and_swap() {
        let id = Lut3d::parse_cube(&cube(|c| c, 17)).unwrap();
        assert_eq!(id.title.as_deref(), Some("test"));
        let v = id.apply([0.2, 0.55, 0.9]);
        assert!(v.iter().zip([0.2, 0.55, 0.9]).all(|(a, b)| (a - b).abs() < 1e-5), "{v:?}");
        let swap = Lut3d::parse_cube(&cube(|c| [c[2], c[1], c[0]], 9)).unwrap();
        let v = swap.apply([0.1, 0.5, 0.8]);
        assert!((v[0] - 0.8).abs() < 1e-5 && (v[2] - 0.1).abs() < 1e-5, "{v:?}");
        assert!(Lut3d::parse_cube("LUT_1D_SIZE 4\n0 0 0").is_err());
        assert!(Lut3d::parse_cube("LUT_3D_SIZE 2\n0 0 0\n").is_err(), "too few entries");
    }
}
