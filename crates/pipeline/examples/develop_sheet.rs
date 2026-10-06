//! Render a few scenes with default and edited settings side by side (PNG, via the pipeline).
//! `cargo run --release -p lightcraft-pipeline --example develop_sheet -- out.png`
use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, SourceInfo, render};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "develop.png".into());
    let lib = lightcraft_scenes::demo_library();
    let picks = [0usize, 2, 3, 6, 8, 5];
    let (cw, ch) = (420usize, 280usize);
    let mut sheet = image::RgbImage::new((cw * 2) as u32, (ch * picks.len()) as u32);
    for (row, &i) in picks.iter().enumerate() {
        let src = lib[i].render_fit(900);
        let mut edited = DevelopSettings::default();
        edited.light.shadows = 45.0;
        edited.light.highlights = -55.0;
        edited.light.contrast = 15.0;
        edited.effects.clarity = 25.0;
        edited.effects.dehaze = 12.0;
        edited.color.vibrance = 30.0;
        edited.vignette.amount = -25.0;
        edited.grading.shadows = lightcraft_develop::Wheel { hue: 210.0, sat: 18.0, lum: 0.0 };
        edited.grading.highlights = lightcraft_develop::Wheel { hue: 40.0, sat: 14.0, lum: 0.0 };
        for (col, s) in [DevelopSettings::default(), edited].iter().enumerate() {
            let r = render(&src, &SourceInfo::default(), s, &RenderRequest::fit(cw, ch)).image;
            let (ox, oy) = (col * cw + (cw - r.width) / 2, row * ch + (ch - r.height) / 2);
            for y in 0..r.height {
                for x in 0..r.width {
                    let p = r.get(x, y);
                    sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb([p[0], p[1], p[2]]));
                }
            }
        }
    }
    sheet.save(&out).unwrap();
    println!("{out}");
}
