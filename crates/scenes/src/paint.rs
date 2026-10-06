//! Scene painters. Coordinates: `x` = u·aspect (isotropic), `v` = 0 (top) .. 1 (bottom).
//! Colours are authored as linear sRGB and converted to linear Rec.2020 at the end.

use lightcraft_color::{REC2020, SRGB};
use lightcraft_raster::Rgb32f;
use rayon::prelude::*;

use crate::Kind;
use crate::noise::{fbm, hash, noise, rand01, ridged};

type C = [f32; 3];

fn hex(v: u32) -> C {
    let f = |c: u32| lightcraft_color::transfer::srgb_to_linear(((v >> c) & 0xff) as f32 / 255.0);
    [f(16), f(8), f(0)]
}
#[inline]
fn mix(a: C, b: C, t: f32) -> C {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
#[inline]
fn add(a: C, b: C) -> C {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn mul(a: C, s: f32) -> C {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
fn mulc(a: C, b: C) -> C {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Per-scene random parameters.
struct P {
    seed: u32,
    aspect: f32,
}

impl P {
    fn r(&self, i: u32) -> f32 {
        rand01(i as i32, 7, self.seed)
    }
}

struct Sky {
    zenith: C,
    horizon: C,
    horizon_v: f32,
    sun: (f32, f32),
    sun_col: C,
    sun_disc: f32,
    glow: f32,
    clouds: f32,
    cloud_col: C,
    stars: f32,
}

fn sky(p: &P, s: &Sky, x: f32, v: f32) -> C {
    let elev = ((s.horizon_v - v) / s.horizon_v).clamp(0.0, 1.0);
    let mut c = mix(s.horizon, s.zenith, elev.powf(0.55));
    let (dx, dy) = (x - s.sun.0, (v - s.sun.1) * 1.0);
    let d = (dx * dx + dy * dy).sqrt();
    c = add(c, mul(s.sun_col, s.glow * (0.9 * (-d * 9.0).exp() + 0.35 * (-d * 2.6).exp())));
    let disc_r = 0.028;
    if s.sun_disc > 0.0 {
        let e = smooth(disc_r, disc_r * 0.8, d);
        c = mix(c, mul(s.sun_col, s.sun_disc), e);
    }
    if s.clouds > 0.0 {
        let n = fbm(x * 1.7 + 3.0, v * 5.5, p.seed ^ 0xc10d, 6);
        let dens = smooth(1.0 - s.clouds, 1.25 - s.clouds, n * 0.5 + 0.5) * smooth(0.0, 0.12, elev);
        if dens > 0.0 {
            let edge = fbm(x * 6.0, v * 14.0, p.seed ^ 0xed9e, 3) * 0.5 + 0.5;
            let lit = (-d * 2.2).exp() * 2.5 + 0.35 * edge;
            let cc = add(mul(s.cloud_col, 0.55 + 0.3 * edge), mul(s.sun_col, lit * s.glow.min(1.2) * 0.5));
            c = mix(c, cc, dens * 0.92);
        }
    }
    if s.stars > 0.0 {
        let g = 900.0;
        let (cx, cy) = ((x * g) as i32, (v * g) as i32);
        let h = rand01(cx, cy, p.seed ^ 0x5a5a);
        if h > 0.9965 {
            let b = (h - 0.9965) / 0.0035;
            let tint = mix([0.8, 0.85, 1.0], [1.0, 0.9, 0.75], rand01(cx, cy, 99));
            c = add(c, mul(tint, s.stars * b * b * 1.6 * elev.sqrt()));
        }
    }
    c
}

struct Layer {
    base: f32,
    amp: f32,
    freq: f32,
    col: C,
    haze: f32,
    snow: f32,
    ridged: bool,
    seed: u32,
}

/// Low-octave height, for shading (slopes of the detailed crest alias into vertical streaks).
fn layer_h_lo(l: &Layer, x: f32) -> f32 {
    let n = if l.ridged {
        0.75 * ridged(x * l.freq, 0.37, l.seed, 2) + 0.25 * (fbm(x * l.freq * 0.5, 1.7, l.seed, 2) * 0.5 + 0.5)
    } else {
        fbm(x * l.freq, 0.37, l.seed, 2) * 0.5 + 0.5
    };
    l.base + l.amp * n
}

fn layer_h(l: &Layer, x: f32) -> f32 {
    let n = if l.ridged {
        0.75 * ridged(x * l.freq, 0.37, l.seed, 4) + 0.25 * (fbm(x * l.freq * 0.5, 1.7, l.seed, 3) * 0.5 + 0.5)
    } else {
        fbm(x * l.freq, 0.37, l.seed, 5) * 0.5 + 0.5
    };
    l.base + l.amp * n
}

/// Terrain shading for a layer hit at (x, v) whose crest is at `top`.
fn terrain(l: &Layer, x: f32, v: f32, top: f32, sun_x: f32, sun_col: C, haze_col: C) -> C {
    let depth = (v - top).max(0.0);
    // Facets: slope of the low-octave crest, sampled with a wide stencil (ridged noise has creases
    // whose slope jumps would otherwise become full-height vertical stripes) at an x warped with
    // depth so faces lean and break up instead of running straight down.
    let e = 0.035;
    let xs = x + fbm(x * 7.0, v * 7.0, l.seed ^ 31, 2) * 0.03 + depth * 0.15 * (sun_x - x).signum();
    let slope = (layer_h_lo(l, xs + e) - layer_h_lo(l, xs - e)) / (2.0 * e);
    // Light direction blends smoothly through the sun's x (no hard flip).
    let dir = ((sun_x - x) * 10.0).tanh();
    let face = (slope * dir * 1.6).clamp(-1.0, 1.0) * (1.0 - smooth(0.0, 0.6, depth) * 0.5)
        // aerial perspective: distant (hazy) ranges have little facet contrast
        * (1.0 - l.haze * 1.6).max(0.15);
    let tex = fbm(x * 38.0, v * 38.0, l.seed ^ 77, 4) * 0.12;
    let light = (0.62 + 0.38 * face + tex).max(0.05);
    let mut c = mul(l.col, light * (1.0 - (depth * 2.5).min(0.45)));
    c = add(c, mul(mulc(sun_col, l.col), 0.35 * face.max(0.0)));
    if l.snow > 0.0 {
        let s = l.snow * (0.6 + 0.5 * (fbm(x * 9.0, v * 4.0, l.seed ^ 5, 3) * 0.5 + 0.5));
        if depth < s * 0.18 * (1.0 - smooth(0.0, 0.18, depth) * 0.3) {
            let sc = mix([0.55, 0.6, 0.72], add(sun_col, [0.35, 0.35, 0.4]), (0.55 + 0.45 * face).clamp(0.0, 1.0));
            c = mix(c, sc, 0.95);
        }
    }
    mix(c, haze_col, l.haze)
}

fn pine(p: &P, x: f32, v: f32, ground: f32, height: f32, density: f32) -> Option<f32> {
    let tw = 0.035;
    let cell = (x / tw).floor() as i32;
    let mut best: Option<f32> = None;
    for c in cell - 1..=cell + 1 {
        if rand01(c, 1, p.seed ^ 0x7ee) > density {
            continue;
        }
        let cx = (c as f32 + rand01(c, 2, p.seed)) * tw;
        let th = height * (0.45 + 0.8 * rand01(c, 3, p.seed));
        let base = ground + 0.01 * rand01(c, 4, p.seed);
        let top = base - th;
        if v < top || v > base {
            continue;
        }
        let t = (v - top) / th;
        let jag = 0.75 + 0.25 * ((t * 22.0 + c as f32).sin() * 0.5 + 0.5);
        let hw = t * th * 0.26 * jag + 0.0015;
        let dx = (x - cx).abs();
        if dx < hw {
            best = Some(best.map_or(dx / hw, |b: f32| b.min(dx / hw)));
        }
    }
    best
}

fn water(p: &P, x: f32, v: f32, wl: f32, calm: f32, above: &dyn Fn(f32, f32) -> C, tint: C, sun: (f32, f32), sun_col: C) -> C {
    let d = v - wl;
    let rip = fbm(x * 9.0, d * 60.0 / (d + 0.05), p.seed ^ 0xaa, 4) * (1.0 - calm);
    let rv = (wl - d * 1.0 + rip * (0.01 + 0.06 * (1.0 - calm)) * (d * 10.0 + 0.2)).max(0.0);
    let rx = x + rip * 0.02 * (d * 6.0 + 0.1);
    let refl = above(rx, rv);
    let fres = 0.55 + 0.4 * (1.0 - (d * 3.0).min(1.0));
    let mut c = add(mul(refl, fres), mul(tint, 0.25));
    // glitter along the sun path
    let path = (-(x - sun.0).abs() * (14.0 - d * 8.0).max(3.0)).exp();
    let g = rand01((x * 700.0) as i32, (v * 1400.0) as i32, p.seed ^ 0x61);
    if g > 0.93 - 0.1 * path && path > 0.02 && sun.1 < wl {
        c = add(c, mul(sun_col, path * (g - 0.8) * 6.0 * (1.0 - calm * 0.6)));
    }
    c
}

pub fn render(kind: Kind, seed: u32, w: usize, h: usize) -> Rgb32f {
    let aspect = w as f32 / h as f32;
    let p = P { seed, aspect };
    let ss = if w * h <= 300_000 { 2 } else { 1 };
    let to2020 = SRGB.to_space(&REC2020).to_f32();
    let mut img = Rgb32f::new(w, h);
    img.data.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (xi, px) in row.iter_mut().enumerate() {
            let mut acc = [0.0f32; 3];
            for sy in 0..ss {
                for sx in 0..ss {
                    let u = (xi as f32 + (sx as f32 + 0.5) / ss as f32) / w as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / ss as f32) / h as f32;
                    acc = add(acc, shade(kind, &p, u * aspect, v));
                }
            }
            let c = mul(acc, 1.0 / (ss * ss) as f32);
            let m = &to2020;
            *px = [
                (m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2]).max(0.0),
                (m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2]).max(0.0),
                (m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2]).max(0.0),
            ];
        }
    });
    img
}

/// Linear sRGB variant (for tests/tools that don't want Rec.2020).
pub fn render_srgb_linear(kind: Kind, seed: u32, w: usize, h: usize) -> Rgb32f {
    let to709 = REC2020.to_space(&SRGB);
    render(kind, seed, w, h).map(|c| to709.apply_f32(c).map(|v| v.max(0.0)))
}

fn shade(kind: Kind, p: &P, x: f32, v: f32) -> C {
    match kind {
        Kind::AlpineLake => alpine(p, x, v, false),
        Kind::BlueHour => alpine(p, x, v, true),
        Kind::Dunes => dunes(p, x, v),
        Kind::OceanSunset => ocean(p, x, v),
        Kind::Aurora => aurora(p, x, v),
        Kind::MistyForest => forest(p, x, v),
        Kind::Lavender => lavender(p, x, v),
        Kind::Macro => macro_flower(p, x, v),
        Kind::Canyon => canyon(p, x, v),
        Kind::Beach => beach(p, x, v),
    }
}

fn alpine(p: &P, x: f32, v: f32, blue: bool) -> C {
    let hz = 0.62;
    let sun = (p.aspect * (0.2 + 0.6 * p.r(1)), if blue { 0.75 } else { 0.28 + 0.1 * p.r(2) });
    let (sun_col, sk) = if blue {
        (
            hex(0xffb4a0),
            Sky {
                zenith: hex(0x0b1a3c),
                horizon: hex(0x6a6fa8),
                horizon_v: hz,
                sun,
                sun_col: hex(0xff9a8a),
                sun_disc: 0.0,
                glow: 0.25,
                clouds: 0.0,
                cloud_col: hex(0x303a60),
                stars: 0.5,
            },
        )
    } else {
        let sc = hex(0xffc27a);
        (
            sc,
            Sky {
                zenith: hex(0x2f5c9e),
                horizon: hex(0xf2c08e),
                horizon_v: hz,
                sun,
                sun_col: sc,
                sun_disc: 22.0,
                glow: 1.1,
                clouds: 0.35 + 0.2 * p.r(3),
                cloud_col: hex(0x9aa4be),
                stars: 0.0,
            },
        )
    };
    let haze = if blue { hex(0x5d668f) } else { hex(0xc8b8b0) };
    let layers = [
        Layer { base: 0.02, amp: 0.05, freq: 3.0, col: hex(0x24331f), haze: 0.1, snow: 0.0, ridged: false, seed: p.seed + 4 },
        Layer {
            base: 0.08,
            amp: 0.16,
            freq: 1.6,
            col: if blue { hex(0x3a4466) } else { hex(0x5d5a58) },
            haze: 0.18,
            snow: 0.7,
            ridged: true,
            seed: p.seed + 3,
        },
        Layer {
            base: 0.16,
            amp: 0.28,
            freq: 1.0,
            col: if blue { hex(0x4d5a88) } else { hex(0x7b7a86) },
            haze: 0.42,
            snow: 1.0,
            ridged: true,
            seed: p.seed + 2,
        },
    ];
    let wl = hz + 0.035;
    let above = |x: f32, v: f32| -> C {
        if let Some(t) = pine(p, x, v, wl, 0.16, 0.55) {
            let rim = if blue { 0.0 } else { (1.0 - t).powi(4) * 0.0 + (t).powi(6) * 0.25 };
            return add(mul(hex(0x0c140d), 1.0), mul(sun_col, rim * 0.2));
        }
        for l in &layers {
            let top = hz - layer_h(l, x);
            if v >= top {
                return terrain(l, x, v, top, sun.0, sun_col, haze);
            }
        }
        sky(p, &sk, x, v)
    };
    if v > wl {
        let tint = if blue { hex(0x1a2440) } else { hex(0x20303a) };
        return water(p, x, v, wl, 0.75, &above, tint, sun, sun_col);
    }
    above(x, v)
}

fn dunes(p: &P, x: f32, v: f32) -> C {
    let hz = 0.42;
    let sun_col = hex(0xffd29a);
    let sun = (p.aspect * (0.1 + 0.25 * p.r(1)), 0.12);
    let sk = Sky {
        zenith: hex(0x2a64b8),
        horizon: hex(0xcfd9e6),
        horizon_v: hz,
        sun,
        sun_col,
        sun_disc: 0.0,
        glow: 0.5,
        clouds: 0.0,
        cloud_col: hex(0xffffff),
        stars: 0.0,
    };
    // Dune layers from near (low) to far (high). Crest = sharp ridge via |sin|.
    for i in 0..5 {
        let fi = i as f32;
        let freq = 1.3 + fi * 0.6;
        let ph = p.r(10 + i) * 6.0;
        let hfun = |x: f32| {
            let w = (x * freq + ph + 0.4 * fbm(x * 0.8, fi, p.seed + i, 3)).sin();
            let crest = 1.0 - w.abs().powf(0.6);
            0.05 + fi * 0.085 + crest * (0.16 - fi * 0.02)
        };
        let top = 0.98 - hfun(x) - fi * 0.08;
        if v >= top {
            let e = 0.001;
            let slope = (hfun(x + e) - hfun(x - e)) / (2.0 * e);
            let lit = (0.5 - slope * 1.2).clamp(0.0, 1.0);
            let ripple = ((x * 260.0 + v * 90.0 + 8.0 * fbm(x * 3.0, v * 3.0, p.seed, 3)).sin() * 0.5 + 0.5) * 0.08;
            let sand = mix(hex(0xb4562a), hex(0xf2a55c), (1.0 - (v - top) * 3.0).clamp(0.2, 1.0));
            let shadow_fill = hex(0x5e3a3a);
            let shade = smooth(0.35, 0.65, lit);
            let c = mix(mix(mul(sand, 0.28), shadow_fill, 0.45), mul(sand, 1.05 + ripple), shade);
            let far = fi / 5.0;
            return mix(mul(c, 0.9), hex(0xe8c8b0), far * 0.45);
        }
    }
    sky(p, &sk, x, v)
}

fn ocean(p: &P, x: f32, v: f32) -> C {
    let hz = 0.56;
    let sun_col = hex(0xffa050);
    let sun = (p.aspect * (0.35 + 0.3 * p.r(1)), hz - 0.06 - 0.05 * p.r(2));
    let sk = Sky {
        zenith: hex(0x3b3f78),
        horizon: hex(0xff9c55),
        horizon_v: hz,
        sun,
        sun_col,
        sun_disc: 30.0,
        glow: 1.6,
        clouds: 0.45,
        cloud_col: hex(0x5a4a6a),
        stars: 0.0,
    };
    // Headland silhouette on one side.
    let side = if p.r(5) > 0.5 { x } else { p.aspect - x };
    let head = 0.18 * (1.0 - smooth(0.0, 0.35 * p.aspect, side)) * (0.8 + 0.4 * fbm(x * 4.0, 0.0, p.seed, 4));
    let above = |x: f32, v: f32| -> C {
        let side = if p.r(5) > 0.5 { x } else { p.aspect - x };
        let head = 0.18 * (1.0 - smooth(0.0, 0.35 * p.aspect, side)) * (0.8 + 0.4 * fbm(x * 4.0, 0.0, p.seed, 4));
        if v > hz - head && v <= hz {
            return mix(hex(0x1c1420), hex(0x3a2230), (hz - v) * 2.0);
        }
        sky(p, &sk, x, v)
    };
    if v > hz {
        if v < hz + head * 0.3 {
            return hex(0x120c14);
        }
        return water(p, x, v, hz, 0.35, &above, hex(0x1a2040), sun, sun_col);
    }
    let _ = head;
    above(x, v)
}

fn aurora(p: &P, x: f32, v: f32) -> C {
    let hz = 0.64;
    let sk = Sky {
        zenith: hex(0x03060f),
        horizon: hex(0x0d2230),
        horizon_v: hz,
        sun: (-5.0, 5.0),
        sun_col: [0.0; 3],
        sun_disc: 0.0,
        glow: 0.0,
        clouds: 0.0,
        cloud_col: [0.0; 3],
        stars: 1.2,
    };
    let aur = |x: f32, v: f32| -> C {
        let yc = 0.40 + 0.12 * fbm(x * 0.9, 1.0, p.seed ^ 0xa0, 4);
        let d = yc - v;
        let rays = 0.55 + 0.45 * noise(x * 38.0, v * 1.5, p.seed ^ 0xb1).abs() * 1.6;
        let i = if d > 0.0 { (-d / 0.16).exp() } else { (-(d / 0.018).powi(2)).exp() };
        let fold = 0.6 + 0.4 * fbm(x * 3.0, 0.3, p.seed ^ 0xc2, 3);
        let g = mul(hex(0x2cff8a), i * rays * fold * 2.2);
        let pur = mul(hex(0xa040ff), i * smooth(0.06, 0.3, d) * 1.4);
        add(g, pur)
    };
    let layers = [
        Layer { base: 0.03, amp: 0.05, freq: 2.2, col: hex(0x0a0e12), haze: 0.0, snow: 0.0, ridged: false, seed: p.seed + 3 },
        Layer { base: 0.07, amp: 0.2, freq: 1.2, col: hex(0x1a2430), haze: 0.1, snow: 0.9, ridged: true, seed: p.seed + 2 },
    ];
    let wl = hz + 0.05;
    let above = |x: f32, v: f32| -> C {
        for l in &layers {
            let top = hz - layer_h(l, x);
            if v >= top {
                let t = terrain(l, x, v, top, x + 1.0, hex(0x2c8a60), hex(0x0a1418));
                return mul(t, 0.35);
            }
        }
        add(sky(p, &sk, x, v), aur(x, v))
    };
    if v > wl {
        return water(p, x, v, wl, 0.9, &above, hex(0x020408), (-5.0, 5.0), [0.0; 3]);
    }
    above(x, v)
}

fn forest(p: &P, x: f32, v: f32) -> C {
    let sun = (p.aspect * (0.55 + 0.3 * p.r(1)), 0.1);
    let sun_col = hex(0xfff0c8);
    let fog = hex(0xcfd6c8);
    let mut c = mix(hex(0xf4f0e0), hex(0xb8c4c0), (1.0 - v).powf(1.5));
    for i in 0..6 {
        let fi = i as f32;
        let ground = 0.5 + fi * 0.1;
        let depth = 1.0 - fi / 5.0;
        let col = mix(mix(hex(0x0c1c10), hex(0x1c3020), depth), fog, depth.powf(0.8) * 0.88);
        if pine(&P { seed: p.seed + i * 17, aspect: p.aspect }, x * (1.0 - fi * 0.08) + fi * 3.1, v, ground, 0.22 + fi * 0.07, 0.85).is_some()
            || v > ground
        {
            c = col;
        }
    }
    // volumetric light shafts in front of everything, strongest in the fog
    let (dx, dy) = (x - sun.0, v - sun.1);
    let ang = dy.atan2(dx);
    let rays = (fbm(ang * 9.0, 0.0, p.seed ^ 0x9a, 4) * 0.5 + 0.5).powf(3.0);
    let dist = (dx * dx + dy * dy).sqrt();
    add(c, mul(sun_col, rays * (-dist * 1.3).exp() * 0.7 + (-dist * 7.0).exp() * 1.6))
}

fn lavender(p: &P, x: f32, v: f32) -> C {
    let hz = 0.45;
    let sun_col = hex(0xffb070);
    let sun = (p.aspect * (0.6 + 0.2 * p.r(1)), hz - 0.08);
    let sk = Sky {
        zenith: hex(0x4a6ea8),
        horizon: hex(0xffc896),
        horizon_v: hz,
        sun,
        sun_col,
        sun_disc: 18.0,
        glow: 1.2,
        clouds: 0.3,
        cloud_col: hex(0xa89ab8),
        stars: 0.0,
    };
    // lone tree
    let tx = p.aspect * (0.22 + 0.1 * p.r(2));
    let (dx, dy) = ((x - tx) / 0.09, (v - (hz - 0.11)) / 0.075);
    let blob = dx * dx + dy * dy + 0.35 * fbm(x * 30.0, v * 30.0, p.seed, 3);
    if blob < 1.0 {
        return mix(hex(0x1a2412), hex(0x3a4a1c), (1.0 - blob) * 0.3);
    }
    if (x - tx).abs() < 0.006 && v > hz - 0.06 && v < hz + 0.004 {
        return hex(0x1a1410);
    }
    if v > hz {
        let dz = v - hz;
        let z = 0.06 / (dz + 0.002);
        let xw = (x - p.aspect * 0.5) / (dz + 0.004);
        let row = (xw * 9.0).sin() * smooth(0.0, 0.08, dz) + (1.0 - smooth(0.0, 0.08, dz)) * 0.3;
        let tex = fbm(xw * 3.0, z * 3.0, p.seed ^ 0x1a, 3);
        let purple = mix(hex(0x5a3a9a), hex(0x9a70d8), (0.5 + 0.5 * tex).clamp(0.0, 1.0));
        let gap = hex(0x3a3a1e);
        let t = smooth(-0.4, 0.2, row + tex * 0.4 * (1.0 - smooth(0.0, 0.2, dz)));
        let mut c = mix(gap, purple, t);
        c = mul(c, 0.55 + 0.6 * smooth(0.0, 0.5, dz));
        c = add(c, mul(sun_col, 0.18 * (-dz * 8.0).exp()));
        let haze = (-dz * 25.0).exp();
        return mix(c, hex(0xe8b8a0), haze * 0.6);
    }
    sky(p, &sk, x, v)
}

fn macro_flower(p: &P, x: f32, v: f32) -> C {
    let ymax = 1.0;
    let mut c = mix(hex(0x0c2a10), hex(0x3e6a1a), (1.0 - v / ymax).powf(1.2));
    // bokeh discs
    for i in 0..36u32 {
        let cx = p.r(100 + i) * p.aspect;
        let cy = p.r(200 + i);
        let r = 0.03 + 0.09 * p.r(300 + i);
        let d = ((x - cx).powi(2) + (v - cy).powi(2)).sqrt();
        if d < r {
            let pal = [hex(0xd8e870), hex(0x80c040), hex(0xfff0b0), hex(0xf0a0c0), hex(0x60a8a0)];
            let col = pal[(hash(i as i32, 1, p.seed) % 5) as usize];
            let rim = smooth(r * 0.8, r, d) * 0.4;
            let a = smooth(r, r * 0.93, d) * (0.25 + 0.35 * p.r(400 + i));
            c = add(c, mul(col, a * (1.0 + rim)));
        }
    }
    let center = (p.aspect * (0.45 + 0.1 * p.r(1)), 0.46);
    let rr = 0.28;
    // stem
    if (x - center.0 - (v - center.1) * 0.15).abs() < 0.008 && v > center.1 {
        return mix(hex(0x2a5a1a), hex(0x4a8a2a), 0.5);
    }
    let (dx, dy) = (x - center.0, v - center.1);
    let r = (dx * dx + dy * dy).sqrt() / rr;
    let th = dy.atan2(dx) + p.r(2) * 3.0;
    let n = 8.0;
    let petal = ((th * n / 2.0).cos()).abs();
    let edge = 0.35 + 0.65 * petal.powf(0.45) + 0.03 * noise(th * 20.0, 0.0, p.seed);
    if r < edge {
        let (pc, tip) = if p.seed.is_multiple_of(2) { (hex(0xe0408a), hex(0xffc0e0)) } else { (hex(0x8a3ad0), hex(0xe0c0ff)) };
        let vein = ((th * n * 7.0).sin() * 0.5 + 0.5) * 0.12;
        let t = (r / edge).clamp(0.0, 1.0);
        let mut col = mix(mul(pc, 0.55), tip, t.powf(1.5));
        col = mul(col, 0.85 + vein + 0.25 * (1.0 - petal));
        // centre
        if r < 0.2 {
            let dots = rand01((x * 900.0) as i32, (v * 900.0) as i32, p.seed) * 0.4;
            col = mul(mix(hex(0xffb000), hex(0xffe060), 1.0 - r * 5.0), 0.8 + dots);
        }
        let light = 1.2 - 0.4 * (dx + dy + 0.2).clamp(-1.0, 1.0);
        return mul(col, light);
    }
    c
}

fn canyon(p: &P, x: f32, v: f32) -> C {
    let hz = 0.3;
    let sun_col = hex(0xffe0b0);
    let sk = Sky {
        zenith: hex(0x1e5aa8),
        horizon: hex(0x9ac4e8),
        horizon_v: hz + 0.2,
        sun: (p.aspect * 0.8, -0.1),
        sun_col,
        sun_disc: 0.0,
        glow: 0.3,
        clouds: 0.2,
        cloud_col: hex(0xe8eef4),
        stars: 0.0,
    };
    // Two walls meeting in a V-shaped gap.
    let mid = p.aspect * (0.5 + 0.1 * (p.r(1) - 0.5));
    let gap = (x - mid).abs() / p.aspect;
    let wall_top = hz + 0.25 - gap * 0.9 + 0.08 * fbm(x * 3.0, 0.0, p.seed, 5);
    if v > wall_top {
        let strata = ((v * 70.0 + 3.0 * fbm(x * 2.0, v * 2.0, p.seed ^ 2, 4)).sin() * 0.5 + 0.5).powf(1.5);
        let base = mix(hex(0xa8401c), hex(0xe89050), strata);
        let cream = hex(0xe8c8a0);
        let mut c = mix(base, cream, smooth(0.8, 1.0, strata) * 0.5);
        let lit_side = x < mid;
        let depth = (v - wall_top).max(0.0);
        let light = if lit_side { 1.0 - depth * 0.6 } else { 0.35 - depth * 0.2 };
        c = mul(c, light.max(0.08) * (0.9 + 0.2 * fbm(x * 40.0, v * 40.0, p.seed ^ 9, 3)));
        if !lit_side {
            c = add(c, mul(hex(0x6a4a60), 0.12));
        }
        return c;
    }
    sky(p, &sk, x, v)
}

fn beach(p: &P, x: f32, v: f32) -> C {
    let hz = 0.42;
    let sun_col = hex(0xffffff);
    let sk = Sky {
        zenith: hex(0x1f6fd0),
        horizon: hex(0xb8e0f8),
        horizon_v: hz,
        sun: (p.aspect * 0.8, -0.2),
        sun_col,
        sun_disc: 0.0,
        glow: 0.25,
        clouds: 0.4,
        cloud_col: hex(0xffffff),
        stars: 0.0,
    };
    // palm silhouette (right side)
    let px0 = p.aspect * 0.86;
    let trunk_x = |v: f32| px0 - (1.0 - v) * 0.25 * (1.0 - v);
    if v > 0.18 && (x - trunk_x(v)).abs() < 0.012 * (0.6 + v * 0.6) {
        return hex(0x2a2018);
    }
    let crown = (trunk_x(0.18), 0.18);
    for f in 0..9 {
        let a0 = -2.9 + f as f32 * 0.7 + 0.2 * p.r(50 + f);
        let len = 0.22 + 0.06 * p.r(60 + f);
        // parametric droop
        let steps = 24;
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let fx = crown.0 + a0.cos() * len * t;
            let fy = crown.1 + a0.sin() * len * t + t * t * 0.12;
            let d = ((x - fx).powi(2) + (v - fy).powi(2)).sqrt();
            let leaf = 0.022 * (1.0 - t) * (0.6 + 0.4 * ((t * 60.0).sin()).abs());
            if d < leaf + 0.002 {
                return mix(hex(0x0e2410), hex(0x1a3a18), t);
            }
        }
    }
    if v <= hz {
        return sky(p, &sk, x, v);
    }
    let shore = 0.68 + 0.04 * (x * 3.0 + p.r(1) * 5.0).sin() + 0.02 * fbm(x * 6.0, 0.0, p.seed, 3);
    if v < shore {
        let t = (v - hz) / (shore - hz);
        let deep = hex(0x0d4a8a);
        let turq = hex(0x30d0c8);
        let mut c = mix(deep, turq, t.powf(1.4));
        let caust = (fbm(x * 30.0, v * 40.0, p.seed ^ 3, 3) * 0.5 + 0.5).powf(3.0) * t;
        c = add(c, mul([0.6, 0.9, 0.9], caust * 0.35));
        let foam = smooth(shore - 0.012, shore, v) * (0.6 + 0.4 * noise(x * 60.0, 0.0, p.seed));
        c = mix(c, hex(0xf4f8f8), foam);
        let refl = mul(sky(p, &sk, x, hz - (v - hz) * 0.8), 0.25 * (1.0 - t));
        return add(c, refl);
    }
    let wet = smooth(shore + 0.04, shore, v);
    let sand = mul(hex(0xf0dcb8), 0.9 + 0.12 * fbm(x * 80.0, v * 80.0, p.seed ^ 4, 3));
    mix(sand, mul(hex(0xc8b090), 0.9), wet)
}
