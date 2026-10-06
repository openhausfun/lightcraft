//! Issue #78: a GPU render that cannot be trusted — over the device's buffer limits, a device
//! error, work that silently did not run — must come back as `None` (the caller renders on the
//! CPU) with the reason recorded, never as an image of zeros. One test, sequential: the fatal
//! faults stop GPU use for the whole process until `reset_failures`.
//! Skips (passes with a note) when no GPU adapter exists.

use std::sync::Arc;

use lightcraft_develop::{DevelopSettings, Mask, MaskComponent, MaskOp, MaskShape};
use lightcraft_geom::Point;
use lightcraft_gpu::Fault;
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};
use lightcraft_raster::Rgba8;

fn mean_abs_diff(a: &Rgba8, b: &Rgba8) -> f64 {
    let sum: u64 = a.data.iter().zip(&b.data).map(|(p, q)| (0..3).map(|c| p[c].abs_diff(q[c]) as u64).sum::<u64>()).sum();
    sum as f64 / (a.data.len() * 3) as f64
}

fn masked(s: &mut DevelopSettings, k: usize) {
    for i in 0..k {
        let c = Point::new(0.2 + 0.2 * i as f64, 0.5);
        s.masks.push(Mask {
            id: i as u32 + 1,
            components: vec![MaskComponent {
                name: None,
                op: MaskOp::Add,
                invert: false,
                shape: MaskShape::Radial { center: c, rx: 0.2, ry: 0.15, angle: 0.0, feather: 50.0, invert: false },
            }],
            ..Default::default()
        });
        s.masks[i].adjust.exposure = 0.5;
    }
}

#[test]
fn failed_gpu_renders_fall_back_with_a_reason() {
    if !lightcraft_gpu::available() {
        eprintln!("skipped: no GPU adapter ({:?})", lightcraft_gpu::unavailable_reason());
        return;
    }
    assert_eq!(lightcraft_gpu::unavailable_reason(), None);
    let (w, h) = (1200, 800);
    let src = Arc::new(lightcraft_scenes::demo_library()[0].render(w, h));
    let info = SourceInfo { raw: true, ..Default::default() };
    let mut s = DevelopSettings::default();
    s.light.exposure = 0.3;
    s.effects.clarity = 20.0;
    masked(&mut s, 4);
    let req = RenderRequest::fit(w, h);
    let cpu = render(&src, &info, &s, &req).image;
    let ok = |what: &str| {
        let g = lightcraft_gpu::render(&src, &info, &s, &req, None)
            .unwrap_or_else(|| panic!("{what}: gpu render ({:?})", lightcraft_gpu::last_fallback()));
        let d = mean_abs_diff(&cpu, &g.image);
        assert!(d < 0.5, "{what}: GPU differs from the CPU by {d:.3} LSB");
    };
    ok("healthy");

    // the output itself is over the limit: refused up front, the GPU stays in use
    lightcraft_gpu::inject_fault(Fault::Limit(1 << 20));
    assert!(lightcraft_gpu::render(&src, &info, &s, &req, None).is_none());
    let why = lightcraft_gpu::last_fallback().unwrap_or_default();
    assert!(why.contains("limit"), "{why}");
    assert!(lightcraft_gpu::available(), "a limit is no device failure");

    // the output fits but the mask planes (4 masks: 4 planes in one buffer) don't: caught when the
    // buffer is created, without a device error, and the GPU stays in use
    lightcraft_gpu::inject_fault(Fault::Limit((w * h * 3 * 4 + 1024) as u64));
    assert!(lightcraft_gpu::render(&src, &info, &s, &req, None).is_none());
    let why = lightcraft_gpu::last_fallback().unwrap_or_default();
    assert!(why.contains("exceeds"), "{why}");
    assert!(lightcraft_gpu::available(), "{:?}", lightcraft_gpu::unavailable_reason());
    ok("after the limit faults");

    // a device (validation) error: captured by the render's error scope; the GPU is not used again
    lightcraft_gpu::inject_fault(Fault::Validation);
    assert!(lightcraft_gpu::render(&src, &info, &s, &req, None).is_none());
    let why = lightcraft_gpu::last_fallback().unwrap_or_default();
    assert!(why.contains("device error"), "{why}");
    assert!(!lightcraft_gpu::available());
    assert!(lightcraft_gpu::unavailable_reason().is_some_and(|r| r.contains("device error")), "{:?}", lightcraft_gpu::unavailable_reason());
    lightcraft_gpu::reset_failures();
    ok("after a device error");

    // the per-pixel work never runs (a driver dropping a submission without an error): before
    // the fix this came back as a fully black image
    lightcraft_gpu::inject_fault(Fault::DropWork);
    assert!(lightcraft_gpu::render(&src, &info, &s, &req, None).is_none(), "an unwritten image must not be returned");
    let why = lightcraft_gpu::last_fallback().unwrap_or_default();
    assert!(why.contains("incomplete"), "{why}");
    assert!(!lightcraft_gpu::available());
    lightcraft_gpu::reset_failures();
    ok("after dropped work");

    // a black photo is still rendered (on the CPU, to be sure)
    let black = Arc::new(lightcraft_raster::Rgb32f::from_fn(256, 256, |_, _| [0.0; 3]));
    let s0 = DevelopSettings::default();
    assert!(lightcraft_gpu::render(&black, &info, &s0, &RenderRequest::fit(256, 256), None).is_none());
    assert!(lightcraft_gpu::last_fallback().unwrap_or_default().contains("black"));
    assert!(lightcraft_gpu::available());
}
