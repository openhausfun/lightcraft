//! CPU ↔ GPU equivalence: the same settings rendered by the CPU pipeline (the reference) and by
//! `lightcraft_gpu::render`, compared in 8-bit sRGB.
//!
//! Bounds (documented in `docs/gpu-pipeline.md`): mean |Δ| < 0.5 LSB and max |Δ| ≤ 3 LSB per
//! channel for every case. Skips (passes with a note) when no GPU adapter exists, e.g. on CI.

use std::sync::Arc;

use lightcraft_develop::{BrushStroke, DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape, Spot, Treatment, VignetteStyle, WbMode, Wheel};
use lightcraft_geom::{Orientation, Point, Rect};
use lightcraft_pipeline::{Quality, RenderRequest, SourceInfo, StageCache, render};
use lightcraft_raster::{Rgb32f, Rgba8};

const MEAN_LSB: f64 = 0.5;
const MAX_LSB: u8 = 3;

fn gpu() -> bool {
    let ok = lightcraft_gpu::available();
    if !ok {
        eprintln!("skipped: no GPU adapter");
    }
    ok
}

#[test]
fn camera_tone_and_relative_wb() {
    if !gpu() {
        return;
    }
    let src = scene(2, 320, 240);
    let curve = lightcraft_pipeline::tone::CameraTone::new(std::array::from_fn(|i| {
        let x = 0.004 * 1.18f32.powi(i as i32);
        [x, 1.0 - (-2.0 * x).exp()]
    }))
    .unwrap();
    let info = SourceInfo { raw: true, relative_wb: true, camera_tone: Some(curve), ..Default::default() };
    let mut s = DevelopSettings::default();
    check("camera tone neutral", &src, &info, &s, &RenderRequest::fit(320, 240));
    s.light.exposure = 1.0;
    s.light.contrast = 40.0;
    s.wb.mode = WbMode::Custom;
    s.wb.temp = 8000.0;
    check("camera tone edited", &src, &info, &s, &RenderRequest::fit(320, 240));
}

fn scene(i: usize, w: usize, h: usize) -> Arc<Rgb32f> {
    Arc::new(lightcraft_scenes::demo_library()[i].render(w, h))
}

/// (mean |Δ|, max |Δ|, share of channels with |Δ| > 1).
fn diff(a: &Rgba8, b: &Rgba8) -> (f64, u8, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let (mut sum, mut max, mut over) = (0u64, 0u8, 0u64);
    for (p, q) in a.data.iter().zip(&b.data) {
        for c in 0..3 {
            let d = p[c].abs_diff(q[c]);
            sum += d as u64;
            max = max.max(d);
            over += (d > 1) as u64;
        }
    }
    let n = (a.data.len() * 3) as f64;
    (sum as f64 / n, max, over as f64 / n)
}

fn check(name: &str, src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest) -> (f64, u8) {
    let cpu = render(src, info, s, req).image;
    let gpu = lightcraft_gpu::render(src, info, s, req, None).expect("gpu render");
    let (mean, max, over) = diff(&cpu, &gpu.image);
    eprintln!("{name:<28} {}x{}  mean {mean:.4}  max {max}  >1: {:.4}%", cpu.width, cpu.height, over * 100.0);
    assert!(mean < MEAN_LSB && max <= MAX_LSB, "{name}: mean {mean:.4} LSB, max {max} LSB");
    (mean, max)
}

type Edit = fn(&mut DevelopSettings);

fn typical(s: &mut DevelopSettings) {
    s.light.exposure = 0.3;
    s.light.highlights = -40.0;
    s.light.shadows = 30.0;
    s.effects.clarity = 15.0;
    s.effects.texture = 10.0;
    s.effects.dehaze = 10.0;
    s.detail.nr_luminance = 30.0;
    s.detail.nr_color = 25.0;
    s.detail.sharpen_amount = 40.0;
}

/// A render big enough for the per-pixel stage to run in several bands of rows (and the render
/// in many submissions), with grain (position-dependent) and masks.
#[test]
fn banded_full_size_render_matches() {
    if !gpu() {
        return;
    }
    let (w, h) = (3000, 2000);
    let src = scene(3, w, h);
    let mut s = DevelopSettings::default();
    typical(&mut s);
    s.grain.amount = 30.0;
    s.vignette.amount = -30.0;
    s.masks = vec![Mask {
        components: vec![MaskComponent {
            name: None,
            op: MaskOp::Add,
            invert: false,
            shape: MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.9) },
        }],
        adjust: lightcraft_develop::LocalAdjustments { exposure: -0.6, ..Default::default() },
        ..Default::default()
    }];
    check("banded 3000×2000", &src, &SourceInfo { raw: true, ..Default::default() }, &s, &RenderRequest::fit(w, h));
}

fn cases() -> Vec<(&'static str, Edit)> {
    vec![
        ("default", |_| {}),
        ("typical", typical),
        ("exposure/contrast/whites", |s| {
            s.light.exposure = -0.7;
            s.light.contrast = 45.0;
            s.light.whites = 30.0;
            s.light.blacks = -25.0;
        }),
        ("vibrance/saturation", |s| {
            s.color.vibrance = 40.0;
            s.color.saturation = -20.0;
        }),
        ("colour mixer", |s| {
            s.mixer.blue.sat = -60.0;
            s.mixer.orange.hue = 30.0;
            s.mixer.green.lum = 40.0;
        }),
        ("point color", |s| {
            use lightcraft_develop::PointColor;
            s.point_colors = vec![
                PointColor {
                    lum: 0.62,
                    chroma: 0.09,
                    hue: 60.0,
                    hue_shift: 40.0,
                    sat_shift: -30.0,
                    lum_shift: 20.0,
                    range: 80.0,
                    ..Default::default()
                },
                PointColor { lum: 0.5, chroma: 0.12, hue: 270.0, variance: -60.0, sat_shift: 40.0, hue_range: 80.0, ..Default::default() },
                PointColor { lum: 0.4, chroma: 0.01, hue: 0.0, lum_shift: -30.0, ..Default::default() },
            ];
        }),
        ("b&w mix", |s| {
            s.treatment = Treatment::Bw;
            s.bw_mix.blue = 60.0;
            s.bw_mix.red = -30.0;
        }),
        ("colour grading", |s| {
            s.grading.shadows = Wheel { hue: 220.0, sat: 40.0, lum: -10.0 };
            s.grading.highlights = Wheel { hue: 40.0, sat: 30.0, lum: 10.0 };
            s.grading.global = Wheel { hue: 300.0, sat: 10.0, lum: 0.0 };
            s.grading.blending = 70.0;
            s.grading.balance = 20.0;
        }),
        ("tone curves", |s| {
            s.curve.highlights = -30.0;
            s.curve.shadows = 25.0;
            s.curve.master = vec![Point::new(0.0, 0.05), Point::new(0.5, 0.55), Point::new(1.0, 0.95)];
            s.curve.blue = vec![Point::new(0.0, 0.0), Point::new(0.5, 0.45), Point::new(1.0, 1.0)];
        }),
        ("tone curves + refine saturation", |s| {
            s.curve.master = vec![Point::new(0.0, 0.0), Point::new(0.25, 0.15), Point::new(0.75, 0.88), Point::new(1.0, 1.0)];
            s.curve.lights = 30.0;
            s.curve.refine_saturation = 20.0;
        }),
        ("vignette (highlight)", |s| {
            s.vignette.amount = -60.0;
            s.vignette.highlights = 50.0;
            s.vignette.roundness = -40.0;
        }),
        ("vignette (paint, +)", |s| {
            s.vignette.amount = 40.0;
            s.vignette.style = VignetteStyle::PaintOverlay;
        }),
        ("vignette (paint, -)", |s| {
            s.vignette.amount = -40.0;
            s.vignette.style = VignetteStyle::PaintOverlay;
        }),
        ("grain", |s| {
            s.grain.amount = 50.0;
            s.grain.size = 40.0;
            s.grain.roughness = 60.0;
        }),
        ("dehaze -", |s| s.effects.dehaze = -50.0),
        ("dehaze +", |s| s.effects.dehaze = 60.0),
        ("clarity/texture -", |s| {
            s.effects.clarity = -60.0;
            s.effects.texture = -40.0;
        }),
        ("sharpen masking", |s| {
            s.detail.sharpen_amount = 90.0;
            s.detail.sharpen_masking = 60.0;
        }),
        ("white balance", |s| {
            s.wb.mode = WbMode::Custom;
            s.wb.temp = 8200.0;
            s.wb.tint = 15.0;
        }),
        ("noise reduction", |s| {
            s.detail.nr_luminance = 70.0;
            s.detail.nr_detail = 30.0;
            s.detail.nr_color = 60.0;
            s.detail.nr_color_smoothness = 50.0;
        }),
        ("crop + straighten + flip", |s| {
            s.crop.geometry.rect = Rect::new(0.1, 0.05, 0.85, 0.9);
            s.crop.geometry.angle = 7.5;
            s.crop.flip_h = true;
        }),
        ("orientation", |s| s.orientation = Orientation::Rotate90),
        ("lens + perspective", |s| {
            s.optics.distortion = 30.0;
            s.optics.vignetting = 40.0;
            s.optics.ca_red = 50.0;
            s.optics.ca_blue = -40.0;
            s.geometry.vertical = 20.0;
        }),
        ("masks (gradients)", |s| {
            s.masks = vec![
                Mask {
                    components: vec![MaskComponent {
                        name: None,
                        op: MaskOp::Add,
                        invert: false,
                        shape: MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.6) },
                    }],
                    adjust: lightcraft_develop::LocalAdjustments { exposure: -0.8, temp: -30.0, saturation: 20.0, ..Default::default() },
                    ..Default::default()
                },
                Mask {
                    components: vec![
                        MaskComponent {
                            name: None,
                            op: MaskOp::Add,
                            invert: false,
                            shape: MaskShape::Radial { center: Point::new(0.4, 0.6), rx: 0.25, ry: 0.15, angle: 20.0, feather: 60.0, invert: false },
                        },
                        MaskComponent {
                            name: None,
                            op: MaskOp::Intersect,
                            invert: true,
                            shape: MaskShape::Linear { start: Point::new(0.0, 0.0), end: Point::new(1.0, 1.0) },
                        },
                    ],
                    adjust: lightcraft_develop::LocalAdjustments {
                        shadows: 40.0,
                        clarity: 30.0,
                        contrast: 20.0,
                        color_hue: 30.0,
                        color_sat: 50.0,
                        hue: 10.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ];
        }),
        ("masks (brush + ranges)", |s| {
            let stroke = BrushStroke {
                points: vec![Point::new(0.1, 0.3), Point::new(0.5, 0.4), Point::new(0.8, 0.35)],
                size: 0.04,
                flow: 60.0,
                ..Default::default()
            };
            let erase = BrushStroke { points: vec![Point::new(0.5, 0.4)], size: 0.03, erase: true, ..Default::default() };
            s.masks = vec![
                Mask {
                    components: vec![MaskComponent {
                        name: None,
                        op: MaskOp::Add,
                        invert: false,
                        shape: MaskShape::Brush { strokes: vec![stroke, erase] },
                    }],
                    adjust: lightcraft_develop::LocalAdjustments { exposure: 0.7, whites: 20.0, blacks: -20.0, dehaze: 30.0, ..Default::default() },
                    ..Default::default()
                },
                Mask {
                    components: vec![MaskComponent {
                        name: None,
                        op: MaskOp::Add,
                        invert: false,
                        shape: MaskShape::LuminanceRange { lo: 0.5, hi: 0.8, lo_feather: 0.1, hi_feather: 0.1 },
                    }],
                    adjust: lightcraft_develop::LocalAdjustments { highlights: -50.0, texture: 30.0, sharpness: 40.0, ..Default::default() },
                    ..Default::default()
                },
                Mask {
                    components: vec![MaskComponent {
                        name: None,
                        op: MaskOp::Add,
                        invert: false,
                        shape: MaskShape::ColorRange { samples: vec![[0.6, -0.05, -0.08]], refine: 50.0 },
                    }],
                    adjust: lightcraft_develop::LocalAdjustments { saturation: 40.0, tint: 20.0, ..Default::default() },
                    invert: true,
                    ..Default::default()
                },
            ];
        }),
        ("masks (cpu shapes, ops, amount)", |s| {
            s.masks = vec![
                Mask {
                    components: vec![
                        MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Sky },
                        MaskComponent {
                            name: None,
                            op: MaskOp::Subtract,
                            invert: false,
                            shape: MaskShape::Radial { center: Point::new(0.7, 0.2), rx: 0.1, ry: 0.1, angle: 0.0, feather: 30.0, invert: false },
                        },
                    ],
                    adjust: lightcraft_develop::LocalAdjustments { exposure: -0.5, dehaze: 40.0, amount: 70.0, ..Default::default() },
                    ..Default::default()
                },
                Mask {
                    components: vec![MaskComponent { name: None, op: MaskOp::Intersect, invert: false, shape: MaskShape::Subject }],
                    adjust: lightcraft_develop::LocalAdjustments { exposure: 0.5, ..Default::default() },
                    ..Default::default()
                },
                Mask {
                    components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: true, shape: MaskShape::Background }],
                    adjust: lightcraft_develop::LocalAdjustments { saturation: -60.0, ..Default::default() },
                    invert: true,
                    ..Default::default()
                },
            ];
        }),
        ("masks (local noise, moiré, defringe)", |s| {
            let radial = MaskShape::Radial { center: Point::new(0.4, 0.6), rx: 0.3, ry: 0.25, angle: 0.0, feather: 50.0, invert: false };
            let linear = MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.7) };
            let m = |shape, adjust| Mask {
                components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape }],
                adjust,
                ..Default::default()
            };
            use lightcraft_develop::LocalAdjustments as L;
            s.masks = vec![
                m(radial, L { noise: 80.0, moire: 60.0, defringe: 100.0, ..Default::default() }),
                m(linear, L { noise: -60.0, moire: -40.0, ..Default::default() }),
            ];
        }),
        ("masks (auto mask brush)", |s| {
            let st = |pts: &[(f64, f64)], auto_mask, erase| BrushStroke {
                points: pts.iter().map(|p| Point::new(p.0, p.1)).collect(),
                size: 0.06,
                feather: 40.0,
                flow: 80.0,
                auto_mask,
                erase,
                ..Default::default()
            };
            let strokes = vec![
                st(&[(0.2, 0.55), (0.5, 0.6), (0.8, 0.5)], true, false),
                st(&[(0.3, 0.2), (0.6, 0.25)], false, false),
                st(&[(0.5, 0.58)], true, true),
            ];
            s.masks = vec![Mask {
                components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes } }],
                adjust: lightcraft_develop::LocalAdjustments { exposure: 1.0, saturation: -50.0, ..Default::default() },
                ..Default::default()
            }];
        }),
        ("calibration", |s| {
            s.calibration.shadows_tint = 40.0;
            s.calibration.red_hue = 50.0;
            s.calibration.red_sat = -30.0;
            s.calibration.green_hue = -40.0;
            s.calibration.blue_sat = 60.0;
            s.calibration.blue_hue = 25.0;
        }),
        ("spots + defringe (cpu stage)", |s| {
            s.spots = vec![Spot { points: vec![Point::new(0.3, 0.3)], size: 0.03, source_offset: Some(Point::new(0.1, 0.0)), ..Default::default() }];
            s.optics.defringe_purple_amount = 5.0;
        }),
    ]
}

#[test]
fn gpu_matches_cpu() {
    if !gpu() {
        return;
    }
    let src = scene(0, 960, 640);
    let raw = SourceInfo { raw: true, ..Default::default() };
    let req = RenderRequest::fit(720, 720);
    let mut worst = (0.0f64, 0u8);
    for (name, edit) in cases() {
        let mut s = DevelopSettings::default();
        edit(&mut s);
        let (m, x) = check(name, &src, &raw, &s, &req);
        worst = (worst.0.max(m), worst.1.max(x));
    }
    eprintln!("worst: mean {:.4} LSB, max {} LSB", worst.0, worst.1);
}

#[test]
fn rendered_sources_and_other_scenes() {
    if !gpu() {
        return;
    }
    // A display-referred source (JPEG-like: display tone map) and other scenes / sizes / draft.
    let jpeg = SourceInfo::default();
    for (i, (w, h)) in [(3usize, (800usize, 533usize)), (5, (640, 960)), (7, (1200, 800))] {
        let src = scene(i, w, h);
        let mut s = DevelopSettings::default();
        typical(&mut s);
        s.color.vibrance = 25.0;
        check(&format!("scene {i} display-referred"), &src, &jpeg, &s, &RenderRequest::fit(700, 700));
        let draft = RenderRequest { quality: Quality::Draft, ..RenderRequest::fit(500, 500) };
        check(&format!("scene {i} draft"), &src, &SourceInfo { raw: true, ..Default::default() }, &s, &draft);
        check(&format!("scene {i} full size"), &src, &SourceInfo { raw: true, ..Default::default() }, &s, &RenderRequest::fit(w, h));
    }
}

#[test]
fn geometry_variants() {
    if !gpu() {
        return;
    }
    use lightcraft_develop::{EmbeddedLens, EmbeddedVignette, EmbeddedWarp};
    let src = scene(2, 900, 600);
    let raw = SourceInfo { raw: true, ..Default::default() };
    use Orientation::*;
    for o in [Normal, Rotate90, Rotate180, Rotate270, FlipH, Transverse, FlipV, Transpose] {
        for (crop, req) in [(false, RenderRequest::fit(900, 900)), (true, RenderRequest::fit(500, 500))] {
            let mut s = DevelopSettings { orientation: o, ..Default::default() };
            if crop {
                s.crop.geometry.rect = Rect::new(0.2, 0.1, 0.9, 0.8);
                s.crop.geometry.angle = -4.0;
                s.crop.flip_v = true;
            }
            check(&format!("{o:?} crop={crop}"), &src, &raw, &s, &req);
        }
    }
    // embedded DNG lens corrections (per-plane warp + vignette) with manual CA
    let lens = EmbeddedLens {
        warp: Some(EmbeddedWarp {
            planes: [[1.0, -0.03, 0.01, 0.0, 0.001, -0.002], [1.0, -0.028, 0.01, 0.0, 0.001, -0.002], [1.0, -0.026, 0.01, 0.0, 0.001, -0.002]],
            center: Point::new(0.52, 0.48),
            radius: 0.6,
        }),
        vignette: Some(EmbeddedVignette { k: [0.4, -0.1, 0.02, 0.0, 0.0], center: Point::new(0.5, 0.5), radius: 0.6 }),
    };
    let info = SourceInfo { lens: Some(lens), ..raw };
    let mut s = DevelopSettings::default();
    s.optics.lens_profile = true;
    s.optics.ca_red = 30.0;
    s.geometry.horizontal = -15.0;
    check("embedded lens + perspective", &src, &info, &s, &RenderRequest::fit(700, 700));
    s.orientation = Rotate270;
    check("embedded lens rotated", &src, &info, &s, &RenderRequest::fit(700, 700));
}

#[test]
fn cached_renders_match_uncached() {
    if !gpu() {
        return;
    }
    // Slider drags reuse device-resident stages: the result must equal a fresh render.
    let src = scene(1, 900, 600);
    let info = SourceInfo { raw: true, ..Default::default() };
    let cache = StageCache::default();
    let req = RenderRequest::fit(640, 640);
    let mut s = DevelopSettings::default();
    typical(&mut s);
    for k in 0..4 {
        s.light.exposure = 0.1 * k as f64;
        s.effects.clarity = 10.0 + 5.0 * (k / 2) as f64;
        s.detail.nr_luminance = 20.0 + 10.0 * (k % 2) as f64;
        let warm = lightcraft_gpu::render(&src, &info, &s, &req, Some(&cache)).expect("gpu");
        let fresh = lightcraft_gpu::render(&src, &info, &s, &req, None).expect("gpu");
        assert_eq!(warm.image, fresh.image, "step {k}");
    }
}

#[test]
fn overlays_match() {
    if !gpu() {
        return;
    }
    // Diagnostic overlays run on the finished 8-bit image, after either renderer.
    let src = scene(0, 900, 600);
    let raw = SourceInfo { raw: true, ..Default::default() };
    let s = DevelopSettings {
        point_colors: vec![lightcraft_develop::PointColor { lum: 0.6, chroma: 0.05, hue: 300.0, hue_shift: 50.0, range: 90.0, ..Default::default() }],
        ..Default::default()
    };
    let req = RenderRequest { overlay: lightcraft_pipeline::Overlay::PointColorRange(0), ..RenderRequest::fit(640, 640) };
    check("point color range overlay", &src, &raw, &s, &req);
    // Visualize Spots is a binary threshold of a high-pass: a 1-LSB difference near the threshold
    // flips a pixel, so compare the share of differing pixels instead of LSBs.
    let s = DevelopSettings::default();
    for t in [20u8, 50, 90] {
        let req = RenderRequest { overlay: lightcraft_pipeline::Overlay::Spots(t), ..RenderRequest::fit(640, 640) };
        let cpu = render(&src, &raw, &s, &req).image;
        let gpu = lightcraft_gpu::render(&src, &raw, &s, &req, None).expect("gpu").image;
        let differ = cpu.data.iter().zip(&gpu.data).filter(|(a, b)| a != b).count() as f64 / cpu.data.len() as f64;
        let white = cpu.data.iter().filter(|p| p[0] == 255).count() as f64 / cpu.data.len() as f64;
        eprintln!("visualize spots t={t:<3}          white {:.3}%  differing {:.4}%", white * 100.0, differ * 100.0);
        assert!(differ < 0.002, "spots t={t}: {differ}");
    }
    // Mask overlays: the alpha the GPU evaluated (or, for a hidden mask, the CPU's) drawn the same way.
    let mut s = DevelopSettings::default();
    let stroke = BrushStroke { points: vec![Point::new(0.2, 0.3), Point::new(0.6, 0.5)], size: 0.05, ..Default::default() };
    s.masks = vec![
        Mask {
            id: 1,
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Radial { center: Point::new(0.4, 0.6), rx: 0.25, ry: 0.15, angle: 20.0, feather: 60.0, invert: false },
            }],
            ..Default::default()
        },
        Mask {
            id: 2,
            components: vec![MaskComponent { name: None, op: MaskOp::Add, invert: false, shape: MaskShape::Brush { strokes: vec![stroke] } }],
            ..Default::default()
        },
        Mask {
            id: 3,
            visible: false,
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Linear { start: Point::new(0.5, 0.0), end: Point::new(0.5, 0.6) },
            }],
            ..Default::default()
        },
    ];
    s.masks[0].adjust.exposure = 0.5;
    use lightcraft_pipeline::{MaskView, Overlay};
    for (id, view) in
        [(1, MaskView::Color), (2, MaskView::ColorOnBw), (2, MaskView::WhiteOnBlack), (3, MaskView::ImageOnWhite), (1, MaskView::ImageOnBlack)]
    {
        let req = RenderRequest { overlay: Overlay::Mask { id, view, color: [230, 30, 40], opacity: 50 }, ..RenderRequest::fit(640, 640) };
        check(&format!("mask {id} overlay {}", view.name()), &src, &raw, &s, &req);
    }
}

#[test]
fn red_eye_matches() {
    if !gpu() {
        return;
    }
    // A face-like patch with a red pupil (left) and a glowing pet pupil (right).
    let src = Arc::new(Rgb32f::from_fn(800, 400, |x, y| {
        let d = |cx: f32, cy: f32| ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
        if d(250.0, 200.0) < 20.0 {
            [0.55, 0.05, 0.04]
        } else if d(560.0, 190.0) < 24.0 {
            [0.5, 0.7, 0.3]
        } else if d(250.0, 200.0) < 40.0 || d(560.0, 190.0) < 45.0 {
            [0.12, 0.1, 0.08]
        } else {
            [0.45, 0.3, 0.22]
        }
    }));
    let info = SourceInfo::default();
    let eyes = vec![
        lightcraft_develop::RedEye { center: Point::new(0.31, 0.5), rx: 0.06, ry: 0.05, darken: 70.0, ..Default::default() },
        lightcraft_develop::RedEye {
            center: Point::new(0.7, 0.48),
            rx: 0.06,
            ry: 0.06,
            pupil_size: 60.0,
            pet: true,
            catchlight: Some(Point::new(-0.35, -0.35)),
            ..Default::default()
        },
    ];
    let s = DevelopSettings { red_eye: eyes.clone(), ..Default::default() };
    check("red + pet eye", &src, &info, &s, &RenderRequest::fit(800, 800));
    let g = lightcraft_gpu::render(&src, &info, &s, &RenderRequest::fit(800, 800), None).expect("gpu").image;
    let (red, pet) = (g.data[200 * 800 + 250], g.data[190 * 800 + 560]);
    assert!(red[0] < 80 && red[0].abs_diff(red[1]) < 10, "red pupil fixed on the GPU: {red:?}");
    assert!(pet[1] < 80 && pet[0].abs_diff(pet[1]) < 5, "pet pupil darkened on the GPU: {pet:?}");
    check("red + pet eye (small)", &src, &info, &s, &RenderRequest::fit(300, 300));
    // with a spot the scene-linear stage runs on the CPU (eyes included)
    let s = DevelopSettings {
        red_eye: eyes,
        spots: vec![Spot { points: vec![Point::new(0.1, 0.2)], size: 0.02, source_offset: Some(Point::new(0.05, 0.0)), ..Default::default() }],
        ..Default::default()
    };
    check("red eye + spot (cpu stage)", &src, &info, &s, &RenderRequest::fit(600, 600));
}

fn saturated(s: &mut DevelopSettings) {
    typical(s);
    s.color.saturation = 70.0;
    s.color.vibrance = 40.0;
    s.curve.highlights = 30.0;
    s.curve.shadows = -20.0;
    s.grain.amount = 30.0;
}

#[test]
fn output_spaces_match() {
    if !gpu() {
        return;
    }
    // Export colour spaces: matrix to the target primaries, gamut mapping into the target gamut and
    // its encoding curve, on both renderers. Saturated edits push colours to the gamut edges.
    use lightcraft_pipeline::OutputSpace;
    let src = scene(0, 900, 600);
    let raw = SourceInfo { raw: true, ..Default::default() };
    for space in OutputSpace::ALL {
        for (name, edit) in [("typical", typical as Edit), ("saturated + curves + grain", saturated as Edit)] {
            let mut s = DevelopSettings::default();
            edit(&mut s);
            let req = RenderRequest { space, ..RenderRequest::fit(640, 640) };
            check(&format!("{space:?} {name}"), &src, &raw, &s, &req);
        }
    }
}
