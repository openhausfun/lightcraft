//! Parameter blocks: the per-pixel stage's [`FinishParams`] packed into the 32-bit words the
//! `finish` kernel reads (`F_*` constants, generated from [`FIELDS`]), plus its auxiliary tables.

use lightcraft_develop::VignetteStyle;
use lightcraft_pipeline::colorops::POINT_WORDS;
use lightcraft_pipeline::finish::{FinishParams, MASK_TERMS, srgb_lut};

/// Entries per tone-curve table (`lightcraft_pipeline::finish` builds them at this size).
pub const CURVE_N: u32 = 1024;

/// Field name and number of 32-bit words.
const FIELDS: &[(&str, usize)] = &[
    ("W", 1),
    ("H", 1),
    ("NMASK", 1),
    ("MASK_OFF", 1),
    ("SRGB_OFF", 1),
    ("CURVE_OFF", 1),
    ("CURVES", 1),
    ("GAIN", 1),
    ("EV", 1),
    ("AIR", 1),
    ("AIR_PRE", 1),
    ("HL", 1),
    ("SH", 1),
    ("CLAR", 1),
    ("TEX", 1),
    ("DEHAZE", 1),
    ("SHARPEN", 1),
    ("SHARPEN_MASK", 1),
    ("HAS_CLAR", 1),
    ("HAS_TEX", 1),
    ("HAS_DARK", 1),
    ("HAS_CHROMA", 1),
    ("OPS_IDENTITY", 1),
    ("VIBRANCE", 1),
    ("SATURATION", 1),
    ("MIXER", 1),
    ("MIX_HUE", 8),
    ("MIX_SAT", 8),
    ("MIX_LUM", 8),
    ("NPC", 1),
    ("PC", 8 * POINT_WORDS),
    ("BW", 1),
    ("BW_MIX", 8),
    ("GRADING", 1),
    ("WHEELS", 12),
    ("BLENDING", 1),
    ("BALANCE", 1),
    ("SKIN", 1),
    ("BANDH", 8),
    ("VIG", 1),
    ("VIG_AMOUNT", 1),
    ("VIG_START", 1),
    ("VIG_WIDTH", 1),
    ("VIG_ASPECT_MIX", 1),
    ("VIG_POWER", 1),
    ("VIG_HL", 1),
    ("VIG_STYLE", 1),
    ("GRAIN", 1),
    ("GRAIN_AMT", 1),
    ("GRAIN_SC", 1),
    ("GRAIN_ROUGH", 1),
    ("GRAIN_SEED", 1),
    ("GRAIN_AFF", 6),
    ("REFINE_SAT", 1),
    ("CALIB", 1),
    ("CALIB_M", 9),
    ("SHADOW_TINT", 1),
    ("OUT_M", 9),
    ("OUT_Y", 3),
    ("OUT_TRC", 1),
    ("OUT_GAMMA", 1),
    // first row of a band dispatch (the kernel runs over rows Y0.., see `render`)
    ("Y0", 1),
];

/// `(name, index)` of every field (for the WGSL constants).
pub fn finish_fields() -> Vec<(&'static str, usize)> {
    let mut i = 0;
    FIELDS
        .iter()
        .map(|(n, len)| {
            let r = (*n, i);
            i += len;
            r
        })
        .collect()
}

/// Offset of field `name`; `None` (and a debug assertion) for a name not in [`FIELDS`].
pub fn index(name: &str) -> Option<usize> {
    let mut i = 0;
    for (n, len) in FIELDS {
        if *n == name {
            return Some(i);
        }
        i += len;
    }
    debug_assert!(false, "unknown finish field {name}");
    None
}

/// A parameter block being filled by name.
pub struct Block(pub Vec<u32>);

impl Block {
    fn new() -> Block {
        Block(vec![0; FIELDS.iter().map(|f| f.1).sum()])
    }
    fn f(&mut self, name: &str, v: f32) {
        self.u(name, v.to_bits());
    }
    fn u(&mut self, name: &str, v: u32) {
        if let Some(slot) = index(name).and_then(|i| self.0.get_mut(i)) {
            *slot = v;
        }
    }
    fn b(&mut self, name: &str, v: bool) {
        self.u(name, v as u32);
    }
    fn fs(&mut self, name: &str, v: &[f32]) {
        let Some(i) = index(name) else { return };
        for (slot, x) in self.0.iter_mut().skip(i).zip(v) {
            *slot = x.to_bits();
        }
    }
}

/// Which optional planes are bound.
pub struct Present {
    pub clarity: bool,
    pub texture: bool,
    pub dark: bool,
    /// The blurred chromaticity follows the mask planes in the `masks` buffer.
    pub chroma: bool,
}

/// The `finish` kernel's parameter block and auxiliary table (tone LUT | sRGB LUT | curve LUTs |
/// mask terms) for `fp` with `masks` (their local adjustments' terms).
pub fn finish_block(fp: &FinishParams, masks: &[[f32; MASK_TERMS]], present: &Present) -> (Vec<u32>, Vec<f32>) {
    let mut aux: Vec<f32> = fp.tone.lut().to_vec();
    let srgb_off = aux.len();
    aux.extend_from_slice(&srgb_lut()[..]);
    let curve_off = aux.len();
    if let Some(c) = &fp.curves {
        for l in c {
            assert_eq!(l.v.len(), CURVE_N as usize);
            aux.extend_from_slice(&l.v);
        }
    }
    let mask_off = aux.len();
    for m in masks {
        aux.extend_from_slice(m);
    }

    let mut p = Block::new();
    p.u("W", fp.w as u32);
    p.u("H", fp.h as u32);
    p.u("NMASK", masks.len() as u32);
    p.u("MASK_OFF", mask_off as u32);
    p.u("SRGB_OFF", srgb_off as u32);
    p.u("CURVE_OFF", curve_off as u32);
    p.b("CURVES", fp.curves.is_some());
    p.f("REFINE_SAT", fp.refine_sat);
    p.f("GAIN", fp.gain);
    p.f("EV", fp.ev);
    p.f("AIR", fp.air);
    p.f("AIR_PRE", fp.air_pre);
    p.f("HL", fp.hl);
    p.f("SH", fp.sh);
    p.f("CLAR", fp.clar);
    p.f("TEX", fp.tex);
    p.f("DEHAZE", fp.dehaze);
    p.f("SHARPEN", fp.sharpen);
    p.f("SHARPEN_MASK", fp.sharpen_mask);
    p.b("HAS_CLAR", present.clarity);
    p.b("HAS_TEX", present.texture);
    p.b("HAS_DARK", present.dark);
    p.b("HAS_CHROMA", present.chroma);

    let ops = &fp.ops;
    p.b("OPS_IDENTITY", ops.is_identity());
    p.f("VIBRANCE", ops.vibrance);
    p.f("SATURATION", ops.saturation);
    p.b("MIXER", ops.mixer);
    p.fs("MIX_HUE", &ops.hue);
    p.fs("MIX_SAT", &ops.sat);
    p.fs("MIX_LUM", &ops.lum);
    p.u("NPC", ops.points.len().min(8) as u32);
    let pc: Vec<f32> = ops.points.iter().take(8).flat_map(|k| k.words()).collect();
    p.fs("PC", &pc);
    p.b("BW", ops.bw.is_some());
    p.fs("BW_MIX", &ops.bw.unwrap_or([0.0; 8]));
    if let Some((wheels, blending, balance)) = &ops.grading {
        p.b("GRADING", true);
        let w: Vec<f32> = wheels.iter().flat_map(|k| [k.a, k.b, k.lum]).collect();
        p.fs("WHEELS", &w);
        p.f("BLENDING", *blending);
        p.f("BALANCE", *balance);
    }
    p.f("SKIN", ops.skin);
    p.fs("BANDH", lightcraft_pipeline::colorops::band_hues());

    if let Some(v) = &fp.vig {
        p.b("VIG", true);
        p.f("VIG_AMOUNT", v.amount);
        p.f("VIG_START", v.start);
        p.f("VIG_WIDTH", v.width);
        p.f("VIG_ASPECT_MIX", v.aspect_mix);
        p.f("VIG_POWER", v.power);
        p.f("VIG_HL", v.highlights);
        p.u(
            "VIG_STYLE",
            match v.style {
                VignetteStyle::HighlightPriority => 1,
                VignetteStyle::PaintOverlay => 2,
                VignetteStyle::ColorPriority => 0,
            },
        );
    }
    if let Some(m) = &fp.calib {
        p.b("CALIB", true);
        p.fs("CALIB_M", m.as_flattened());
    }
    p.f("SHADOW_TINT", fp.shadow_tint);
    p.fs("OUT_M", fp.to_out.as_flattened());
    p.fs("OUT_Y", &fp.out_luma);
    let (trc, gamma) = fp.out_trc.code();
    p.u("OUT_TRC", trc);
    p.f("OUT_GAMMA", gamma);
    if let Some((amt, cell, rough, seed)) = fp.grain {
        p.b("GRAIN", true);
        p.f("GRAIN_AMT", amt);
        p.f("GRAIN_SC", fp.px_per_long as f32 / cell);
        p.f("GRAIN_ROUGH", rough);
        p.u("GRAIN_SEED", seed);
        let long = fp.ow.max(fp.oh);
        let [a, b, c, d, e, f] = fp.out_to_norm.0;
        let (kx, ky) = (fp.ow / long, fp.oh / long);
        p.fs("GRAIN_AFF", &[(a * kx) as f32, (b * ky) as f32, (c * kx) as f32, (d * ky) as f32, (e * kx) as f32, (f * ky) as f32]);
    }
    (p.0, aux)
}
