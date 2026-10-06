//! Synthetic merge inputs from procedural scenes (tests, demos, UI checks — no media files):
//! exposure brackets as linear DNGs and overlapping panorama views as PNGs.

use lightcraft_geom::Orientation;
use lightcraft_meta::Metadata;
use lightcraft_raster::Rgb32f;

use crate::frame::FrameColor;
use crate::linalg::{self, M3};
use crate::output::{DngSamples, write_linear_dng};
use crate::{MergeError, Result};

fn scene(kind: lightcraft_scenes::Kind) -> Result<lightcraft_scenes::Scene> {
    lightcraft_scenes::demo_library()
        .into_iter()
        .find(|s| s.kind == kind)
        .ok_or_else(|| MergeError::Mismatch(format!("no {kind:?} scene in the demo library")))
}

fn srgb_color() -> FrameColor {
    FrameColor::Linear { to_xyz_d50: lightcraft_color::bradford(lightcraft_color::D65, lightcraft_color::D50).mul(&lightcraft_color::SRGB.to_xyz()) }
}

/// Exposure brackets (`evs`, e.g. `[-2, 0, 2]`) of a sunset scene as 16-bit linear DNGs, each
/// shifted by a few pixels (handheld) with EXIF exposure times; `w × h` pixels.
pub fn bracket_dngs(w: usize, h: usize, evs: &[f64]) -> Result<Vec<Vec<u8>>> {
    let m = 12usize;
    let truth = scene(lightcraft_scenes::Kind::OceanSunset)?.render(w + 2 * m, h + 2 * m);
    let mut l: Vec<f32> = truth.data.iter().map(|p| p[1]).collect();
    l.sort_by(|a, b| a.total_cmp(b));
    let s = 0.18 / l.get(l.len() / 2).copied().unwrap_or(0.0).max(1e-6);
    let to_srgb = lightcraft_color::REC2020.to_space(&lightcraft_color::SRGB).to_f32();
    evs.iter()
        .enumerate()
        .map(|(i, ev)| {
            let k = 2f32.powf(*ev as f32) * s;
            let (dx, dy) = ((i as f32 * 2.3) % 5.0 - 2.0, (i as f32 * 1.7) % 4.0 - 1.5);
            let img = Rgb32f::from_fn(w, h, |x, y| {
                let p = truth.sample_bilinear(x as f32 + 0.5 + m as f32 - dx, y as f32 + 0.5 + m as f32 - dy);
                let q = [0, 1, 2].map(|r| to_srgb[r][0] * p[0] + to_srgb[r][1] * p[1] + to_srgb[r][2] * p[2]);
                q.map(|v| (v * k).clamp(0.0, 1.0))
            });
            let meta = Metadata {
                make: Some("LightCraft".into()),
                model: Some("Synthetic Bracket".into()),
                exposure_time: Some(2f64.powf(*ev) / 125.0),
                f_number: Some(8.0),
                iso: Some(100),
                ..Default::default()
            };
            write_linear_dng(&img, &srgb_color(), Orientation::Normal, &meta, 0.0, DngSamples::U16)
        })
        .collect()
}

/// `yaws.len()` overlapping views (degrees of yaw, small pitch/roll wobble) of a canyon scene used
/// as a 200° × 70° spherical environment, `w × h` pixels with focal length `f`, as linear sRGB.
pub fn pano_views(w: usize, h: usize, f: f64, yaws: &[f64]) -> Result<Vec<Rgb32f>> {
    let env = scene(lightcraft_scenes::Kind::Canyon)?.render(2400, 840);
    let mut l: Vec<f32> = env.data.iter().map(|p| p[1]).collect();
    l.sort_by(|a, b| a.total_cmp(b));
    let s = 0.25 / l.get(l.len() / 2).copied().unwrap_or(0.0).max(1e-6);
    let (theta, phi) = (200.0f64, 70.0f64);
    Ok(yaws
        .iter()
        .enumerate()
        .map(|(i, yaw)| {
            let wob = (i as f64 * 1.3).sin();
            let ry = linalg::rodrigues([0.0, yaw.to_radians(), 0.0]);
            let rx = linalg::rodrigues([(wob * 1.5).to_radians(), 0.0, 0.0]);
            let rz = linalg::rodrigues([0.0, 0.0, (wob * 0.8).to_radians()]);
            let c2w: M3 = linalg::mul3(&linalg::mul3(&ry, &rx), &rz);
            Rgb32f::from_fn(w, h, |x, y| {
                let d = linalg::apply3(&c2w, [(x as f64 + 0.5 - w as f64 / 2.0) / f, (y as f64 + 0.5 - h as f64 / 2.0) / f, 1.0]);
                let t = d[0].atan2(d[2]).to_degrees();
                let p = (d[1] / linalg::norm(d)).asin().to_degrees();
                if t.abs() > theta / 2.0 || p.abs() > phi / 2.0 {
                    return [0.0; 3];
                }
                let px = (t / theta + 0.5) * env.width as f64;
                let py = (p / phi + 0.5) * env.height as f64;
                env.sample_bilinear(px as f32, py as f32).map(|v| (v * s).min(1.0))
            })
        })
        .collect())
}
