//! Panorama stitching on synthetic input: a procedural scene is used as a spherical environment
//! (longitude/latitude map) and photographed by pinhole cameras with known rotations and focal
//! length. The stitch must recover the geometry and reproduce the environment without seams.

use lightcraft_merge::frame::Frame;
use lightcraft_merge::linalg::{self, M3};
use lightcraft_merge::no_progress;
use lightcraft_merge::pano::{PanoOptions, Projection, stitch};
use lightcraft_raster::Rgb32f;

const THETA: f64 = 200.0; // degrees of longitude covered by the environment map
const PHI: f64 = 70.0;

fn env() -> Rgb32f {
    let scene = lightcraft_scenes::demo_library().into_iter().find(|s| s.kind == lightcraft_scenes::Kind::Canyon).unwrap();
    let img = scene.render(2400, 840);
    let mut l: Vec<f32> = img.data.iter().map(|p| p[1]).collect();
    l.sort_by(|a, b| a.total_cmp(b));
    let s = 0.25 / l[l.len() / 2].max(1e-6);
    img.map(|p| p.map(|v| (v * s).min(0.98)))
}

fn sample_env(e: &Rgb32f, d: [f64; 3]) -> Option<[f32; 3]> {
    let n = linalg::norm(d);
    let t = d[0].atan2(d[2]).to_degrees();
    let p = (d[1] / n).asin().to_degrees();
    if t.abs() > THETA / 2.0 || p.abs() > PHI / 2.0 {
        return None;
    }
    let x = (t / THETA + 0.5) * e.width as f64;
    let y = (p / PHI + 0.5) * e.height as f64;
    Some(e.sample_bilinear(x as f32, y as f32))
}

fn rot(yaw: f64, pitch: f64, roll: f64) -> M3 {
    // camera-to-world = Ry(yaw) · Rx(pitch) · Rz(roll); world→camera is its transpose
    let ry = linalg::rodrigues([0.0, yaw.to_radians(), 0.0]);
    let rx = linalg::rodrigues([pitch.to_radians(), 0.0, 0.0]);
    let rz = linalg::rodrigues([0.0, 0.0, roll.to_radians()]);
    linalg::transpose3(&linalg::mul3(&linalg::mul3(&ry, &rx), &rz))
}

fn shoot(e: &Rgb32f, r: &M3, f: f64, w: usize, h: usize, gain: f32) -> Frame {
    let rt = linalg::transpose3(r);
    let img = Rgb32f::from_fn(w, h, |x, y| {
        let d = [(x as f64 + 0.5 - w as f64 / 2.0) / f, (y as f64 + 0.5 - h as f64 / 2.0) / f, 1.0];
        sample_env(e, linalg::apply3(&rt, d)).map(|p| p.map(|v| v * gain)).unwrap_or([0.0; 3])
    });
    Frame::from_linear_rec2020(img, None)
}

fn angle_between(a: &M3, b: &M3) -> f64 {
    let d = linalg::mul3(a, &linalg::transpose3(b));
    linalg::norm(linalg::rotation_vector(&d)).to_degrees()
}

struct Shot {
    yaw: f64,
    pitch: f64,
    roll: f64,
    gain: f32,
}

fn run(shots: &[Shot], f: f64, opts: &PanoOptions) -> (lightcraft_merge::pano::PanoResult, Vec<M3>, Rgb32f) {
    let e = env();
    let rs: Vec<M3> = shots.iter().map(|s| rot(s.yaw, s.pitch, s.roll)).collect();
    let frames: Vec<Frame> = shots.iter().zip(&rs).map(|(s, r)| shoot(&e, r, f, 800, 600, s.gain)).collect();
    let t0 = std::time::Instant::now();
    let res = stitch(frames, opts, &no_progress).unwrap();
    eprintln!(
        "stitch {} frames → {}×{} ({:?}) in {:?}; BA rms {:.3} → {:.3} px; fov {:.0}°×{:.0}°",
        shots.len(),
        res.image.width,
        res.image.height,
        res.projection,
        t0.elapsed(),
        res.rms_before,
        res.rms,
        res.fov.0,
        res.fov.1
    );
    (res, rs, e)
}

/// Check the recovered geometry and compare the panorama with the environment rendered through the
/// recovered projection; returns (median, p99) of the log error over covered pixels.
fn check(res: &lightcraft_merge::pano::PanoResult, rs: &[M3], e: &Rgb32f, f: f64) -> (f32, f32, f32) {
    let cams: Vec<_> = res.cameras.iter().map(|c| c.expect("all frames used")).collect();
    for c in &cams {
        assert!((c.f - f).abs() / f < 0.01, "focal {} vs {f}", c.f);
    }
    for i in 0..cams.len() {
        for j in i + 1..cams.len() {
            let truth = linalg::mul3(&rs[i], &linalg::transpose3(&rs[j]));
            let got = linalg::mul3(&cams[i].r, &linalg::transpose3(&cams[j].r));
            let err = angle_between(&truth, &got);
            assert!(err < 0.1, "relative rotation {i}-{j} off by {err:.3}°");
        }
    }
    // world alignment: recovered world → environment world
    let mut acc = [[0.0; 3]; 3];
    for (c, r) in cams.iter().zip(rs) {
        let m = linalg::mul3(&linalg::transpose3(r), &c.r);
        for a in 0..3 {
            for b in 0..3 {
                acc[a][b] += m[a][b];
            }
        }
    }
    let wmap = linalg::orthonormalize(&acc);
    let (w, h) = (res.image.width, res.image.height);
    let scale = res.focal;
    let (bx, by) = res.origin;
    // the environment rendered through the recovered geometry (alpha 0 where it has no data)
    let mut truth = Rgb32f::new(w, h);
    let mut talpha = lightcraft_raster::Plane::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let d = res.projection.inverse(x as f64 + 0.5 + bx, y as f64 + 0.5 + by, scale);
            if let Some(t) = sample_env(e, linalg::apply3(&wmap, d))
                && res.alpha.get(x, y) >= 1.0
            {
                truth.set(x, y, t);
                talpha.set(x, y, 1.0);
            }
        }
    }
    // global level (exposure compensation fixes relative, not absolute, exposure)
    let mut lr: Vec<f32> =
        (0..w * h).filter(|&k| talpha.data[k] > 0.0).map(|k| (res.image.data[k][1] + 0.01).ln() - (truth.data[k][1] + 0.01).ln()).collect();
    lr.sort_by(|a, b| a.total_cmp(b));
    let offset = lr[lr.len() / 2];
    let mut errs = Vec::new();
    let mut emap = Rgb32f::new(w, h);
    for k in 0..w * h {
        if talpha.data[k] == 0.0 {
            continue;
        }
        let (g, t) = (res.image.data[k], truth.data[k]);
        let er = (0..3).map(|c| ((g[c] + 0.01).ln() - offset - (t[c] + 0.01).ln()).abs()).fold(0.0, f32::max);
        errs.push(er);
        emap.data[k] = [er * 5.0, 1.0 - er * 5.0, 0.0];
    }
    if let Ok(dir) = std::env::var("LC_MERGE_DUMP") {
        let tag = format!("{:?}-{}", res.projection, w);
        ppm(&format!("{dir}/{tag}.ppm"), &res.image);
        ppm(&format!("{dir}/{tag}-err.ppm"), &emap);
    }
    // seams: the column-to-column change of mean log luminance must follow the truth's
    let steps = column_steps(&res.image, &talpha);
    let tsteps = column_steps(&truth, &talpha);
    // summed over 5 columns: resampling legitimately spreads a sharp scene edge over neighbours
    let win = |v: &[Option<f32>], x: usize| -> Option<f32> { (x..x + 5).map(|k| v.get(k).copied().flatten()).sum() };
    let seam = (0..steps.len().saturating_sub(5)).filter_map(|x| Some((win(&steps, x)? - win(&tsteps, x)?).abs())).fold(0.0, f32::max);
    errs.sort_by(|a, b| a.total_cmp(b));
    (errs[errs.len() / 2], errs[errs.len() * 99 / 100], seam)
}

fn ppm(path: &str, img: &Rgb32f) {
    let mut b = format!("P6 {} {} 255\n", img.width, img.height).into_bytes();
    for p in &img.data {
        for c in 0..3 {
            b.push((p[c].clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
        }
    }
    std::fs::write(path, b).unwrap();
}

/// Per column: mean change of log luminance to the next column (where both are covered).
fn column_steps(img: &Rgb32f, alpha: &lightcraft_raster::Plane) -> Vec<Option<f32>> {
    let (w, h) = (img.width, img.height);
    (0..w - 1)
        .map(|x| {
            let mut s = 0.0;
            let mut n = 0;
            for y in 0..h {
                if alpha.get(x, y) >= 1.0 && alpha.get(x + 1, y) >= 1.0 {
                    s += (img.get(x + 1, y)[1] + 0.01).ln() - (img.get(x, y)[1] + 0.01).ln();
                    n += 1;
                }
            }
            (n > h / 3).then(|| s / n as f32)
        })
        .collect()
}

#[test]
fn spherical_panorama_recovers_geometry_without_seams() {
    let shots = [
        Shot { yaw: -60.0, pitch: 1.5, roll: 0.8, gain: 1.0 },
        Shot { yaw: -30.0, pitch: -1.0, roll: -0.5, gain: 1.0 },
        Shot { yaw: 0.0, pitch: 0.5, roll: 1.0, gain: 1.0 },
        Shot { yaw: 31.0, pitch: -1.5, roll: 0.0, gain: 1.3 }, // auto exposure drifted
        Shot { yaw: 61.0, pitch: 1.0, roll: -1.0, gain: 1.0 },
    ];
    let f = 700.0;
    let opts = PanoOptions { projection: Projection::Spherical, max_megapixels: 100.0, ..Default::default() };
    let (res, rs, e) = run(&shots, f, &opts);
    assert_eq!(res.used.len(), 5);
    assert!(res.rms < 0.5, "BA rms {}", res.rms);
    assert!((res.fov.0 - 182.0).abs() < 8.0, "fov {:?}", res.fov);
    // exposure compensation undid the 1.3× drift (relative to its neighbours)
    let g = &res.gains;
    let rel = g[3] / ((g[2] + g[4]) / 2.0);
    assert!((rel - 1.0 / 1.3).abs() < 0.03, "gains {g:?}");
    let (med, p99, seam) = check(&res, &rs, &e, f);
    eprintln!("log error vs environment: median {med:.4}, p99 {p99:.4}, seam step {seam:.4}");
    assert!(med < 0.02 && p99 < 0.15, "median {med}, p99 {p99}");
    // (an unblended 1.3× exposure seam would be a step of ln 1.3 ≈ 0.26)
    assert!(seam < 0.05, "a column step of {seam} beyond the scene's suggests a seam");
}

#[test]
fn auto_projection_and_finishing() {
    let shots = [
        Shot { yaw: -22.0, pitch: 0.5, roll: 0.3, gain: 1.0 },
        Shot { yaw: 0.0, pitch: -0.4, roll: 0.0, gain: 1.0 },
        Shot { yaw: 21.0, pitch: 0.8, roll: -0.4, gain: 1.0 },
    ];
    let f = 800.0;
    let (res, rs, e) = run(&shots, f, &PanoOptions { projection: Projection::Auto, auto_crop: true, max_megapixels: 100.0, ..Default::default() });
    assert_eq!(res.projection, Projection::Perspective, "a ~90° set is shown rectilinear");
    let (med, p99, seam) = check(&res, &rs, &e, f);
    eprintln!("perspective: median {med:.4}, p99 {p99:.4}, seam step {seam:.4}");
    assert!(med < 0.02 && p99 < 0.1 && seam < 0.05);
    let c = res.crop.expect("auto crop");
    assert!(c[2] * c[3] > 0.6, "crop {c:?}");
    let (w, h) = (res.image.width as f64, res.image.height as f64);
    for y in (c[1] * h) as usize..((c[1] + c[3]) * h) as usize {
        for x in (c[0] * w) as usize..((c[0] + c[2]) * w) as usize {
            assert!(res.alpha.get(x, y) >= 0.999, "crop must be inside the image data");
        }
    }
    // boundary warp fills the canvas; cylindrical also works
    let (warped, _, _) =
        run(&shots, f, &PanoOptions { projection: Projection::Cylindrical, boundary_warp: 100.0, fill_edges: true, ..Default::default() });
    let cover = warped.alpha.data.iter().filter(|a| **a >= 0.5).count() as f64 / warped.alpha.data.len() as f64;
    assert!(cover > 0.97, "boundary warp coverage {cover}");
    assert!(warped.image.data.iter().all(|p| p.iter().all(|v| v.is_finite())));
}

#[test]
fn unrelated_photos_do_not_stitch() {
    let a = lightcraft_scenes::demo_library();
    let frames: Vec<Frame> = a.iter().take(2).map(|s| Frame::from_linear_rec2020(s.render(400, 300), None)).collect();
    assert!(stitch(frames, &PanoOptions::default(), &no_progress).is_err());
}
