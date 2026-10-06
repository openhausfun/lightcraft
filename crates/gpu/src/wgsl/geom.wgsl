// Geometry (`lightcraft_pipeline::geometry::Frame::sample`): orientation, and the single bilinear
// resample through the crop/straighten/flip affine or the full lens + perspective warp.
// Bindings: src (rgb), dst (rgb).

fn src_px(w: u32, h: u32, x: i32, y: i32) -> vec3<f32> {
    let cx = u32(clamp(x, 0, i32(w) - 1));
    let cy = u32(clamp(y, 0, i32(h) - 1));
    let j = 3u * (cy * w + cx);
    return vec3<f32>(src[j], src[j + 1u], src[j + 2u]);
}

// `Rgb32f::sample_bilinear` (pixel centres at +0.5, clamped edges).
fn bilinear(w: u32, h: u32, x: f32, y: f32) -> vec3<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let a = src_px(w, h, ix, iy);
    let b = src_px(w, h, ix + 1, iy);
    let c = src_px(w, h, ix, iy + 1);
    let d = src_px(w, h, ix + 1, iy + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    return top + (bot - top) * ty;
}

fn put(i: u32, v: vec3<f32>) {
    dst[3u * i] = v.x;
    dst[3u * i + 1u] = v.y;
    dst[3u * i + 2u] = v.z;
}

// Orientation: dst (ow × oh) pixel (x, y) = src pixel (P[4]·x + P[5]·y + P[6], P[7]·x + P[8]·y + P[9])
// (integers). P: sw, sh, ow, oh, map[6].
@compute @workgroup_size(16, 16)
fn orient(@builtin(global_invocation_id) g: vec3<u32>) {
    let sw = pu(0u);
    let ow = pu(2u);
    let oh = pu(3u);
    if (g.x >= ow || g.y >= oh) {
        return;
    }
    let x = i32(g.x);
    let y = i32(g.y);
    let m = array<i32, 6>(bitcast<i32>(pu(4u)), bitcast<i32>(pu(5u)), bitcast<i32>(pu(6u)), bitcast<i32>(pu(7u)), bitcast<i32>(pu(8u)), bitcast<i32>(pu(9u)));
    let sx = u32(m[0] * x + m[1] * y + m[2]);
    let sy = u32(m[3] * x + m[4] * y + m[5]);
    let j = 3u * (sy * sw + sx);
    put(g.y * ow + g.x, vec3<f32>(src[j], src[j + 1u], src[j + 2u]));
}

// Bilinear resample through an affine (output px → base px). P: bw, bh, w, h, affine[6].
@compute @workgroup_size(16, 16)
fn sample_affine(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(2u);
    let h = pu(3u);
    if (g.x >= w || g.y >= h) {
        return;
    }
    let px = f32(g.x) + 0.5;
    let py = f32(g.y) + 0.5;
    let x = pf(4u) * px + pf(6u) * py + pf(8u);
    let y = pf(5u) * px + pf(7u) * py + pf(9u);
    put(g.y * w + g.x, bilinear(pu(0u), pu(1u), x, y));
}

// --- the lens / perspective warp (`lightcraft_pipeline::optics::Warp`), in oriented-source px.
// P layout (see `geom_warp_params`): 0 bw, 1 bh, 2 w, 3 h, 4..10 output → transformed affine,
// 10 sx, 11 sy, 12 W, 13 H, 14 persp?, 15..24 persp_inv, 24 k1, 25..28 ca, 28 lens_dist,
// 29 warp?, 30..48 warp planes (3 × 6), 48 warp centre x, 49 y, 50 warp radius, 51 vig_stops,
// 52 vig_power, 53 lens_vig, 54 vignette?, 55..60 vignette k, 60 vignette centre x, 61 y,
// 62 vignette radius, 63 per-channel?, 64 gain?
const WP_N: u32 = 65u;

fn centre() -> vec2<f32> {
    return vec2<f32>(pf(12u) / 2.0, pf(13u) / 2.0);
}

fn to_corrected(p: vec2<f32>) -> vec2<f32> {
    if (pu(14u) == 0u) {
        return p;
    }
    let c = centre();
    let l = max(pf(12u), pf(13u)) / 2.0;
    let q = (p - c) / l;
    var wq = pf(21u) * q.x + pf(22u) * q.y + pf(23u);
    if (abs(wq) < 1e-30) {
        wq = 1e-30;
    }
    let x = (pf(15u) * q.x + pf(16u) * q.y + pf(17u)) / wq;
    let y = (pf(18u) * q.x + pf(19u) * q.y + pf(20u)) / wq;
    return c + vec2<f32>(x, y) * l;
}

fn embedded_warp(p: vec2<f32>, ch: u32) -> vec2<f32> {
    let w = pf(12u);
    let h = pf(13u);
    let cc = vec2<f32>(pf(48u) * w, pf(49u) * h);
    let m = max(pf(50u) * max(w, h), 1e-9);
    let o = 30u + 6u * min(ch, 2u);
    let d = (p - cc) / m;
    let r2 = d.x * d.x + d.y * d.y;
    let f = pf(o) + r2 * (pf(o + 1u) + r2 * (pf(o + 2u) + r2 * pf(o + 3u)));
    let kt0 = pf(o + 4u);
    let kt1 = pf(o + 5u);
    let sx = d.x * f + kt0 * 2.0 * d.x * d.y + kt1 * (r2 + 2.0 * d.x * d.x);
    let sy = d.y * f + kt1 * 2.0 * d.x * d.y + kt0 * (r2 + 2.0 * d.y * d.y);
    return cc + m * vec2<f32>(sx, sy);
}

fn corrected_to_source(p: vec2<f32>, ch: u32) -> vec2<f32> {
    let c = centre();
    let hd = max(sqrt(pf(12u) * pf(12u) + pf(13u) * pf(13u)) / 2.0, 1e-9);
    var q = p;
    let k1 = pf(24u);
    if (k1 != 0.0) {
        let d = q - c;
        let r2 = (d.x * d.x + d.y * d.y) / (hd * hd);
        q = c + d * (1.0 + k1 * r2);
    }
    let ld = pf(28u);
    if (ld != 0.0 && pu(29u) != 0u) {
        let s = embedded_warp(q, ch);
        q = q + (s - q) * ld;
    }
    let k = pf(25u + ch);
    if (k != 0.0) {
        q = c + (q - c) * (1.0 + k);
    }
    return q;
}

fn warp_gain(s: vec2<f32>) -> f32 {
    var g = 1.0;
    let stops = pf(51u);
    let w = pf(12u);
    let h = pf(13u);
    if (stops != 0.0) {
        let hd = max(sqrt(w * w + h * h) / 2.0, 1e-9);
        let r = min(length(s - centre()) / hd, 1.5);
        g *= exp2(stops * pow(r, pf(52u)));
    }
    let lv = pf(53u);
    if (lv != 0.0 && pu(54u) != 0u) {
        let cc = vec2<f32>(pf(60u) * w, pf(61u) * h);
        let m = max(pf(62u) * max(w, h), 1e-9);
        let d = s - cc;
        let r2 = (d.x * d.x + d.y * d.y) / (m * m);
        var vg = 1.0;
        var rp = r2;
        for (var k = 0u; k < 5u; k++) {
            vg += pf(55u + k) * rp;
            rp *= r2;
        }
        g *= 1.0 + (vg - 1.0) * lv;
    }
    return g;
}

@compute @workgroup_size(16, 16)
fn sample_warp(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(2u);
    let h = pu(3u);
    if (g.x >= w || g.y >= h) {
        return;
    }
    let bw = pu(0u);
    let bh = pu(1u);
    let px = f32(g.x) + 0.5;
    let py = f32(g.y) + 0.5;
    let t = vec2<f32>(pf(4u) * px + pf(6u) * py + pf(8u), pf(5u) * px + pf(7u) * py + pf(9u));
    let c = to_corrected(t);
    let s = corrected_to_source(c, 1u);
    let sx = pf(10u);
    let sy = pf(11u);
    if (!(s.x >= -0.5 && s.y >= -0.5 && s.x <= pf(12u) + 0.5 && s.y <= pf(13u) + 0.5)) {
        put(g.y * w + g.x, vec3<f32>(BLANK_R, BLANK_G, BLANK_B));
        return;
    }
    var v = bilinear(bw, bh, s.x * sx, s.y * sy);
    if (pu(63u) != 0u) {
        let qr = corrected_to_source(c, 0u);
        v.x = bilinear(bw, bh, qr.x * sx, qr.y * sy).x;
        let qb = corrected_to_source(c, 2u);
        v.z = bilinear(bw, bh, qb.x * sx, qb.y * sy).z;
    }
    if (pu(64u) != 0u) {
        v = v * warp_gain(s);
    }
    put(g.y * w + g.x, v);
}
