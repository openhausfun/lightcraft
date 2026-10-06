//! Render every demo scene into a contact sheet PNG (simple clamp + sRGB encode, no develop).
//! `cargo run --release -p lightcraft-scenes --example contact_sheet -- out.png`
use lightcraft_color::{REC2020, SRGB, transfer::encode_srgb8};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "contact.png".into());
    let lib = lightcraft_scenes::demo_library();
    let (cw, ch, cols) = (360usize, 240usize, 6usize);
    let rows = lib.len().div_ceil(cols);
    let mut sheet = image::RgbImage::new((cw * cols) as u32, (ch * rows) as u32);
    let m = REC2020.to_space(&SRGB);
    for (i, s) in lib.iter().enumerate() {
        let img = s.render_fit(cw.min(ch * 3 / 2).max(ch));
        let (ox, oy) = ((i % cols) * cw + (cw - img.width.min(cw)) / 2, (i / cols) * ch + (ch - img.height.min(ch)) / 2);
        for y in 0..img.height.min(ch) {
            for x in 0..img.width.min(cw) {
                let c = m.apply_f32(img.get(x, y));
                // simple Reinhard-ish shoulder for preview
                let t = |v: f32| {
                    let v = v.max(0.0);
                    encode_srgb8(v / (1.0 + v * 0.35))
                };
                sheet.put_pixel((ox + x) as u32, (oy + y) as u32, image::Rgb([t(c[0]), t(c[1]), t(c[2])]));
            }
        }
    }
    sheet.save(&out).unwrap();
    println!("{out}");
}
