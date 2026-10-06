//! Procedurally generated, photographic-looking demo scenes.
//!
//! Scenes are resolution independent (`render(w, h)` evaluates a continuous function) and return
//! scene-referred linear Rec.2020 radiance with real highlight headroom (the sun is ≫ 1.0), so the
//! develop pipeline's highlight recovery, tone mapping and colour tools have something real to do.
//! Every scene is deterministic for its seed. No external assets: zero licensing or privacy risk.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod noise;
mod paint;

use lightcraft_raster::Rgb32f;
use serde::Serialize;

pub use paint::render_srgb_linear;

/// Kinds of scene the generator knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    AlpineLake,
    Dunes,
    OceanSunset,
    Aurora,
    MistyForest,
    Lavender,
    Macro,
    Canyon,
    Beach,
    BlueHour,
}

/// Fake capture metadata so the library looks like a real shoot.
#[derive(Clone, Debug, Serialize)]
pub struct SceneMeta {
    pub camera: &'static str,
    pub lens: &'static str,
    pub focal_mm: f32,
    pub aperture: f32,
    pub shutter: &'static str,
    pub iso: u32,
    /// ISO 8601 local time.
    pub captured: String,
    pub location: &'static str,
    pub keywords: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Scene {
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub seed: u32,
    /// Native pixel size (what an import would report).
    pub width: u32,
    pub height: u32,
    pub meta: SceneMeta,
}

impl Scene {
    pub fn aspect(&self) -> f32 {
        self.width as f32 / self.height as f32
    }

    /// Render at `w × h` (any size; the aspect should match `aspect()`).
    pub fn render(&self, w: usize, h: usize) -> Rgb32f {
        paint::render(self.kind, self.seed, w, h)
    }

    /// Render fitting within `max_edge` on the long side.
    pub fn render_fit(&self, max_edge: usize) -> Rgb32f {
        let a = self.aspect();
        let (w, h) = if a >= 1.0 { (max_edge, (max_edge as f32 / a).round() as usize) } else { ((max_edge as f32 * a).round() as usize, max_edge) };
        self.render(w.max(1), h.max(1))
    }
}

struct Def {
    kind: Kind,
    name: &'static str,
    seed: u32,
    portrait: bool,
    location: &'static str,
    keywords: &'static [&'static str],
}

const DEFS: &[Def] = &[
    Def {
        kind: Kind::AlpineLake,
        name: "Lake at first light",
        seed: 11,
        portrait: false,
        location: "Dolomites",
        keywords: &["mountains", "lake", "sunrise"],
    },
    Def { kind: Kind::Dunes, name: "Dune ridges", seed: 7, portrait: false, location: "Namib", keywords: &["desert", "dunes"] },
    Def { kind: Kind::OceanSunset, name: "Last light over the sea", seed: 3, portrait: false, location: "Big Sur", keywords: &["ocean", "sunset"] },
    Def {
        kind: Kind::Aurora,
        name: "Aurora over the ridge",
        seed: 21,
        portrait: false,
        location: "Lofoten",
        keywords: &["night", "aurora", "stars"],
    },
    Def { kind: Kind::MistyForest, name: "Morning fog", seed: 5, portrait: false, location: "Black Forest", keywords: &["forest", "fog"] },
    Def { kind: Kind::Lavender, name: "Lavender rows", seed: 9, portrait: false, location: "Valensole", keywords: &["fields", "lavender", "summer"] },
    Def { kind: Kind::Macro, name: "Cosmos bloom", seed: 13, portrait: true, location: "Home garden", keywords: &["flower", "macro", "bokeh"] },
    Def { kind: Kind::Canyon, name: "Canyon walls", seed: 17, portrait: false, location: "Utah", keywords: &["canyon", "rock"] },
    Def { kind: Kind::Beach, name: "Lagoon", seed: 19, portrait: false, location: "Maldives", keywords: &["beach", "tropical"] },
    Def { kind: Kind::BlueHour, name: "Blue hour peaks", seed: 23, portrait: false, location: "Patagonia", keywords: &["mountains", "blue hour"] },
    Def { kind: Kind::AlpineLake, name: "Still water", seed: 41, portrait: false, location: "Banff", keywords: &["mountains", "lake"] },
    Def { kind: Kind::OceanSunset, name: "Golden horizon", seed: 44, portrait: false, location: "Algarve", keywords: &["ocean", "sunset"] },
    Def { kind: Kind::Dunes, name: "Afternoon sand", seed: 57, portrait: true, location: "Sahara", keywords: &["desert"] },
    Def { kind: Kind::Aurora, name: "Green curtains", seed: 60, portrait: false, location: "Yukon", keywords: &["night", "aurora"] },
    Def { kind: Kind::Macro, name: "Pink petals", seed: 71, portrait: false, location: "Home garden", keywords: &["flower", "macro"] },
    Def { kind: Kind::MistyForest, name: "Valley haze", seed: 73, portrait: false, location: "Oregon", keywords: &["forest", "fog"] },
    Def { kind: Kind::Canyon, name: "Red layers", seed: 79, portrait: true, location: "Arizona", keywords: &["canyon"] },
    Def { kind: Kind::Beach, name: "Turquoise shallows", seed: 83, portrait: false, location: "Bahamas", keywords: &["beach"] },
    Def { kind: Kind::Lavender, name: "Evening fields", seed: 89, portrait: false, location: "Provence", keywords: &["fields"] },
    Def { kind: Kind::BlueHour, name: "Cold summit", seed: 97, portrait: false, location: "Alps", keywords: &["mountains", "snow"] },
    Def { kind: Kind::AlpineLake, name: "Alpenglow", seed: 101, portrait: true, location: "Zermatt", keywords: &["mountains"] },
    Def { kind: Kind::OceanSunset, name: "Calm sea", seed: 107, portrait: false, location: "Crete", keywords: &["ocean"] },
    Def { kind: Kind::Macro, name: "Violet bloom", seed: 113, portrait: false, location: "Kew", keywords: &["flower"] },
    Def { kind: Kind::Dunes, name: "Wind lines", seed: 127, portrait: false, location: "Death Valley", keywords: &["desert"] },
];

const CAMERAS: &[(&str, &str, f32)] = &[
    ("Synthetic S1", "24-70mm f/2.8", 35.0),
    ("Synthetic S1", "70-200mm f/4", 135.0),
    ("Synthetic X2", "16-35mm f/4", 18.0),
    ("Synthetic X2", "90mm f/2.8 Macro", 90.0),
];

/// The demo library: 24 scenes, 6000×4000 (or 4000×6000) nominal size.
pub fn demo_library() -> Vec<Scene> {
    DEFS.iter()
        .enumerate()
        .map(|(i, d)| {
            let cam = if d.kind == Kind::Macro { CAMERAS[3] } else { CAMERAS[i % 3] };
            let shutter = match d.kind {
                Kind::Aurora => "8",
                Kind::BlueHour => "1/4",
                Kind::MistyForest => "1/60",
                _ => ["1/250", "1/500", "1/125", "1/1000"][i % 4],
            };
            let iso = match d.kind {
                Kind::Aurora => 3200,
                Kind::BlueHour => 400,
                _ => [100, 200, 64, 100][i % 4],
            };
            let (w, h) = if d.portrait { (4000, 6000) } else { (6000, 4000) };
            Scene {
                id: i as u32 + 1,
                name: d.name.to_string(),
                kind: d.kind,
                seed: d.seed,
                width: w,
                height: h,
                meta: SceneMeta {
                    camera: cam.0,
                    lens: cam.1,
                    focal_mm: cam.2,
                    aperture: [2.8, 8.0, 11.0, 4.0][i % 4],
                    shutter,
                    iso,
                    captured: format!("2026-{:02}-{:02}T{:02}:{:02}:00", 3 + i % 7, 1 + (i * 5) % 27, 5 + (i * 3) % 15, (i * 17) % 60),
                    location: d.location,
                    keywords: d.keywords.to_vec(),
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_is_deterministic_and_varied() {
        let lib = demo_library();
        assert_eq!(lib.len(), 24);
        let a = lib[0].render(64, 43);
        let b = lib[0].render(64, 43);
        assert_eq!(a, b);
        // every scene renders finite, non-negative, non-trivial images
        for s in &lib {
            let img = s.render_fit(48);
            assert!(img.data.iter().all(|p| p.iter().all(|v| v.is_finite() && *v >= 0.0)), "{}", s.name);
            let mean: f32 = img.data.iter().map(|p| p[1]).sum::<f32>() / img.len() as f32;
            assert!(mean > 0.005 && mean < 3.0, "{} mean {mean}", s.name);
        }
    }

    #[test]
    fn sunsets_have_highlight_headroom() {
        let lib = demo_library();
        let s = lib.iter().find(|s| s.kind == Kind::OceanSunset).unwrap();
        let img = s.render_fit(200);
        let max = img.data.iter().map(|p| p[0]).fold(0.0f32, f32::max);
        assert!(max > 2.0, "{max}");
    }

    #[test]
    fn resolution_independent() {
        let s = &demo_library()[1];
        let small = s.render(60, 40);
        let big = s.render(240, 160);
        let down = lightcraft_raster::resample::resize(&big, 60, 40, lightcraft_raster::resample::Filter::Box);
        let err: f32 = small.data.iter().zip(&down.data).map(|(a, b)| (a[1] - b[1]).abs()).sum::<f32>() / small.len() as f32;
        assert!(err < 0.08, "{err}");
    }
}
