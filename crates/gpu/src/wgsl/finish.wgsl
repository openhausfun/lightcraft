// The per-pixel stage: a straight port of `lightcraft_pipeline::finish` (keep in step with it).
// Bindings: img (rgb, pre-exposure), log_l, base, clar, tex, dark, masks (NMASK planes, then the
// blurred chromaticity when HAS_CHROMA), aux
// (tone LUT | sRGB LUT | curve LUTs | mask terms), out (packed RGBA8).

fn tone_apply(y: f32) -> f32 {
    if (y <= 0.0) {
        return 0.0;
    }
    let ev = log2(y / GREY);
    let f = clamp((ev - TONE_MIN_EV) / (TONE_MAX_EV - TONE_MIN_EV), 0.0, 1.0) * f32(TONE_N - 1u);
    let i = min(u32(f), TONE_N - 2u);
    let t = f - f32(i);
    let v = aux[i] + (aux[i + 1u] - aux[i]) * t;
    if (ev < TONE_MIN_EV) {
        return v * (y / (GREY * TONE_MIN_GAIN));
    }
    return v;
}

fn encode_srgb(v: f32) -> f32 {
    let o = pu(F_SRGB_OFF);
    let f = clamp(v, 0.0, 1.0) * f32(SRGB_N);
    let i = min(u32(f), SRGB_N - 1u);
    let t = f - f32(i);
    return aux[o + i] + (aux[o + i + 1u] - aux[o + i]) * t;
}

fn curve(ch: u32, x: f32) -> f32 {
    let o = pu(F_CURVE_OFF) + ch * CURVE_N;
    let f = clamp(x, 0.0, 1.0) * f32(CURVE_N - 1u);
    let i = min(u32(f), CURVE_N - 2u);
    let t = f - f32(i);
    return aux[o + i] + (aux[o + i + 1u] - aux[o + i]) * t;
}

fn band_weights(h: f32) -> array<f32, 8> {
    var w: array<f32, 8>;
    for (var i = 0u; i < 8u; i++) {
        let a = pf(F_BANDH + i);
        let b = pf(F_BANDH + (i + 1u) % 8u);
        let span = rem_euclid(wrap_angle(b - a), TAU);
        let d = rem_euclid(wrap_angle(h - a), TAU);
        if (d <= span) {
            let t = d / span;
            let s = 0.5 - 0.5 * cos(t * PI);
            w[i] += 1.0 - s;
            w[(i + 1u) % 8u] += s;
            break;
        }
    }
    return w;
}

// `PointK::apply` for Point Color sample `k` (OkLCh in, OkLCh out).
fn point_color(k: u32, lch: vec3<f32>) -> vec3<f32> {
    let o = F_PC + k * POINT_WORDS;
    let sl = pf(o);
    let sc = pf(o + 1u);
    let sh = pf(o + 2u);
    let var_k = pf(o + 6u);
    let wh_ = pf(o + 7u);
    let wc_ = pf(o + 8u);
    let wl_ = pf(o + 9u);
    var l = lch.x;
    var c = lch.y;
    var h = lch.z;
    var wh = 1.0;
    if (sc >= 0.02) {
        wh = (1.0 - sstep(0.5 * wh_, wh_, abs(wrap_angle(h - sh)))) * sstep(0.005, 0.025, c);
    }
    if (wh <= 0.0) {
        return lch;
    }
    let wc = 1.0 - sstep(0.5 * wc_, wc_, abs(c - sc));
    let wl = 1.0 - sstep(0.5 * wl_, wl_, abs(l - sl));
    let w = wh * wc * wl;
    if (w <= 0.0) {
        return lch;
    }
    h += w * (var_k * wrap_angle(h - sh) + pf(o + 3u));
    c += w * var_k * (c - sc);
    c = max(c * (1.0 + w * pf(o + 4u)), 0.0);
    l += w * (var_k * (l - sl) + pf(o + 5u));
    return vec3<f32>(l, c, h);
}

// `ColorOps::apply`.
fn color_ops(rgb: vec3<f32>, local_sat: f32, local_hue: f32) -> vec3<f32> {
    if (pu(F_OPS_IDENTITY) != 0u && local_sat == 0.0 && local_hue == 0.0) {
        return rgb;
    }
    let lab0 = oklab(rgb);
    var l = lab0.x;
    var c = sqrt(lab0.y * lab0.y + lab0.z * lab0.z);
    var h = atan2(lab0.z, lab0.y);
    if (pu(F_MIXER) != 0u) {
        let w = band_weights(h);
        var dh = 0.0;
        var ds = 0.0;
        var dl = 0.0;
        for (var i = 0u; i < 8u; i++) {
            dh += w[i] * pf(F_MIX_HUE + i);
            ds += w[i] * pf(F_MIX_SAT + i);
            dl += w[i] * pf(F_MIX_LUM + i);
        }
        let chroma_w = min(c / 0.12, 1.0);
        h += dh * chroma_w;
        c *= max(1.0 + ds, 0.0);
        l += dl * chroma_w * sqrt(max(l, 0.05));
    }
    for (var k = 0u; k < pu(F_NPC); k++) {
        let r = point_color(k, vec3<f32>(l, c, h));
        l = r.x;
        c = r.y;
        h = r.z;
    }
    let vib = pf(F_VIBRANCE);
    if (vib != 0.0) {
        let low = 1.0 - clamp(c / 0.22, 0.0, 1.0);
        var skin = 1.0;
        if (vib > 0.0) {
            let q = wrap_angle(h - pf(F_SKIN)) / 0.35;
            skin = 1.0 - 0.6 * exp(-(q * q));
        }
        c *= max(1.0 + vib * low * low * skin * 1.2, 0.0);
    }
    let sat = pf(F_SATURATION);
    if (sat != 0.0 || local_sat != 0.0) {
        c *= max(1.0 + sat + local_sat, 0.0);
    }
    h += local_hue;
    if (pu(F_BW) != 0u) {
        let w = band_weights(h);
        var mix = 0.0;
        for (var i = 0u; i < 8u; i++) {
            mix += w[i] * pf(F_BW_MIX + i);
        }
        l = max(l + mix * min(c / 0.2, 1.0) * 0.25, 0.0);
        c = 0.0;
    }
    var lab = vec3<f32>(l, c * cos(h), c * sin(h));
    if (pu(F_GRADING) != 0u) {
        let m = 0.5 - pf(F_BALANCE) * 0.25;
        let width = 0.15 + pf(F_BLENDING) * 0.5;
        let ws = 1.0 - sstep(m - width, m + width * 0.25, lab.x);
        let wh = sstep(m - width * 0.25, m + width, lab.x);
        let wm = max(1.0 - ws - wh, 0.0);
        let wts = array<f32, 4>(ws, wm, wh, 1.0);
        for (var k = 0u; k < 4u; k++) {
            let wt = wts[k];
            lab.y += pf(F_WHEELS + 3u * k) * wt;
            lab.z += pf(F_WHEELS + 3u * k + 1u) * wt;
            lab.x += pf(F_WHEELS + 3u * k + 2u) * wt;
        }
    }
    return oklab_inv(lab);
}

// `colorops::calibrate`: primaries matrix, then the shadows tint (luminance kept).
fn calibrate(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    if (pu(F_CALIB) != 0u) {
        let m = array<vec3<f32>, 3>(
            vec3<f32>(pf(F_CALIB_M), pf(F_CALIB_M + 1u), pf(F_CALIB_M + 2u)),
            vec3<f32>(pf(F_CALIB_M + 3u), pf(F_CALIB_M + 4u), pf(F_CALIB_M + 5u)),
            vec3<f32>(pf(F_CALIB_M + 6u), pf(F_CALIB_M + 7u), pf(F_CALIB_M + 8u)),
        );
        c = max(mul3(m, c), vec3<f32>(0.0));
    }
    let st = pf(F_SHADOW_TINT);
    if (st != 0.0) {
        let y0 = lum2020(c);
        let w = 1.0 - sstep(-5.0, -0.5, log2(max(y0, 1e-7) / 0.18));
        c.y *= max(1.0 - SHADOW_TINT_K * st * w, 0.0);
        let y1 = max(lum2020(c), 1e-9);
        c = c * y0 / y1;
    }
    return c;
}

// `finish::refine_saturation`.
fn refine_saturation(before: vec3<f32>, after: vec3<f32>, refine: f32) -> vec3<f32> {
    let lw = vec3<f32>(0.2126, 0.7152, 0.0722);
    let y0 = dot(before, lw);
    let y1 = dot(after, lw);
    let s0 = (max(before.x, max(before.y, before.z)) - min(before.x, min(before.y, before.z))) / max(y0, 1e-4);
    let s1 = (max(after.x, max(after.y, after.z)) - min(after.x, min(after.y, after.z))) / max(y1, 1e-4);
    if (s1 <= 1e-6 || s0 <= 1e-6) {
        return after;
    }
    let k = clamp(pow(s0 / s1, 1.0 - refine), 0.0, 4.0);
    return y1 + (after - y1) * k;
}

fn ghash(i: i32, j: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(i) * GRAIN_H0) ^ (bitcast<u32>(j) * GRAIN_H1) ^ (seed * GRAIN_H2);
    v ^= v >> 13u;
    v = v * GRAIN_H3;
    v ^= v >> 15u;
    return f32(v & 0xffffu) / 32768.0 - 1.0;
}

fn grain_noise(x: f32, y: f32, seed: u32) -> f32 {
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let i = i32(x0);
    let j = i32(y0);
    let u = fx * fx * (3.0 - 2.0 * fx);
    let v = fy * fy * (3.0 - 2.0 * fy);
    let a = ghash(i, j, seed) + (ghash(i + 1, j, seed) - ghash(i, j, seed)) * u;
    let b = ghash(i, j + 1, seed) + (ghash(i + 1, j + 1, seed) - ghash(i, j + 1, seed)) * u;
    return a + (b - a) * v;
}

fn enc8(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}

// sRGB-curve-encoded value → the output space's own curve (`OutputTrc`: 0 sRGB, 1 gamma, 2 Rec.709).
fn out_encode(v: f32) -> f32 {
    let kind = pu(F_OUT_TRC);
    if (kind == 0u) {
        return v;
    }
    let e = clamp(v, 0.0, 1.0);
    var l = e / 12.92;
    if (e > 0.04045) {
        l = pow((e + 0.055) / 1.055, 2.4);
    }
    if (kind == 1u) {
        return pow(l, 1.0 / pf(F_OUT_GAMMA));
    }
    if (l < 0.018) {
        return l * 4.5;
    }
    return 1.099 * pow(l, 0.45) - 0.099;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = pu(F_W);
    let h = pu(F_H);
    let x = gid.x;
    let y = gid.y + pu(F_Y0);
    if (x >= w || y >= h) {
        return;
    }
    let i = y * w + x;
    let n = w * h;
    let gain = pf(F_GAIN);
    let ev = pf(F_EV);
    var c = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
    if (gain != 1.0) {
        c = c * gain;
    }
    let l_pre = log_l[i];
    let l0 = l_pre + ev;

    // --- local (mask) contributions
    var lt: array<f32, MASK_SUMS>;
    var tint_on = false;
    var tint_dir = vec2<f32>(0.0, 0.0);
    var tint_amt = 0.0;
    let nm = pu(F_NMASK);
    for (var m = 0u; m < nm; m++) {
        let a = masks[m * n + i];
        if (a <= 0.0) {
            continue;
        }
        let t = pu(F_MASK_OFF) + m * MASK_TERMS;
        for (var k = 0u; k < MASK_SUMS; k++) {
            lt[k] += a * aux[t + k];
        }
        if (aux[t + MASK_SUMS] > 0.0) {
            tint_on = true;
            tint_dir = vec2<f32>(aux[t + MASK_SUMS + 1u], aux[t + MASK_SUMS + 2u]);
            tint_amt = a * aux[t + MASK_SUMS + 3u];
        }
    }
    let l_exp = lt[0];
    let l_temp = lt[1];
    let l_tint = lt[2];
    let l_noise = lt[14];

    // --- local Moiré (and the colour part of Noise): chromaticity towards its blur
    let mt = clamp(lt[15] + 0.5 * max(l_noise, 0.0), -1.0, 1.0);
    if (mt != 0.0 && pu(F_HAS_CHROMA) != 0u) {
        let raw = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
        let y = lum2020(c);
        let ch0 = raw / max(lum2020(raw), 1e-6);
        let o = nm * n + 3u * i;
        let cb = vec3<f32>(masks[o], masks[o + 1u], masks[o + 2u]);
        c = max((ch0 + (cb - ch0) * mt) * y, vec3<f32>(0.0));
    }
    // --- local Defringe: desaturate purple / green fringes along edges
    let df = clamp(lt[16], 0.0, 1.0);
    if (df > 0.0 && pu(F_HAS_TEX) != 0u) {
        let yd = max(lum2020(c), 1e-6);
        let purple = (min(c.x, c.z) - c.y) / yd;
        let green = (c.y - max(c.x, c.z)) / yd;
        let k = df * sstep(0.04, 0.3, abs(l_pre - tex[i])) * max(sstep(0.02, 0.2, purple), sstep(0.02, 0.2, green));
        if (k > 0.0) {
            let y = lum2020(c);
            c = c + (y - c) * k;
        }
    }

    // --- dehaze (scene linear)
    let dz = pf(F_DEHAZE) + lt[10];
    if (dz != 0.0 && pu(F_HAS_DARK) != 0u) {
        let d = clamp(dark[i] / pf(F_AIR_PRE), 0.0, 1.0);
        let air = pf(F_AIR);
        if (dz > 0.0) {
            let t = max(1.0 - 0.95 * min(dz, 1.0) * d, 0.12);
            c = max((c - air * (1.0 - t)) / t, vec3<f32>(0.0));
        } else {
            let k = min(-dz, 1.0) * 0.7 * (0.35 + 0.65 * d);
            c = c + (air * 0.9 - c) * k;
        }
    }

    // --- local exposure / temp / tint
    if (l_exp != 0.0) {
        c = c * exp2(l_exp);
    }
    if (l_temp != 0.0 || l_tint != 0.0) {
        let y0 = lum2020(c);
        c = vec3<f32>(c.x * (1.0 + 0.3 * l_temp), c.y * (1.0 - 0.22 * l_tint), c.z * max(1.0 - 0.3 * l_temp, 0.0));
        let y1 = max(lum2020(c), 1e-9);
        c = c * y0 / y1;
    }

    // --- local tone in log luminance
    var l1 = l0;
    if (dz != 0.0 || l_exp != 0.0) {
        l1 = log_lum(c);
    }
    let shift = l1 - l0;
    let base = base_p[i] + ev + shift;
    var delta = 0.0;
    let hh = pf(F_HL) + lt[4];
    let ss = pf(F_SH) + lt[5];
    if (hh != 0.0 || ss != 0.0) {
        let ws = 1.0 - sstep(-4.8, 0.3, base);
        let wh = sstep(-1.0, 2.8, base);
        delta += ss * 1.7 * ws * sqrt(ws) + hh * 1.7 * wh;
    }
    if (lt[6] != 0.0) {
        delta += lt[6] * 0.8 * sstep(0.5, 3.0, l1);
    }
    if (lt[7] != 0.0) {
        delta += lt[7] * 0.8 * (1.0 - sstep(-6.0, -1.5, l1));
    }
    if (lt[3] != 0.0) {
        delta += lt[3] * 0.14 * clamp(l1, -6.0, 4.0);
    }
    let cl = pf(F_CLAR) + lt[9];
    if (cl != 0.0 && pu(F_HAS_CLAR) != 0u) {
        let det = clamp(l_pre - clar[i], -2.5, 2.5);
        let q = base / 3.2;
        let mid = exp(-(q * q));
        delta += cl * 0.85 * det * (0.35 + 0.65 * mid);
    }
    let tx = pf(F_TEX) + lt[8];
    let sp = lt[13] * 0.6 + pf(F_SHARPEN);
    if ((tx != 0.0 || sp != 0.0) && pu(F_HAS_TEX) != 0u) {
        let det = l_pre - tex[i];
        let tame = 1.0 - 0.6 * sstep(0.4, 1.6, abs(det));
        delta += tx * 1.1 * clamp(det, -1.0, 1.0) * tame;
        if (sp != 0.0) {
            let sm = pf(F_SHARPEN_MASK);
            var mk = 1.0;
            if (sm > 0.0) {
                mk = sstep(sm * 0.25, sm * 0.25 + 0.15, abs(det));
            }
            delta += sp * 1.3 * clamp(det, -0.8, 0.8) * mk;
        }
    }
    // local Noise: smooth (or, negative, boost) small-amplitude detail, keep edges
    if (l_noise != 0.0 && pu(F_HAS_TEX) != 0u) {
        let det = l_pre - tex[i];
        delta -= clamp(l_noise, -1.0, 1.0) * 0.9 * det * (1.0 - sstep(0.1, 0.5, abs(det)));
    }
    if (delta != 0.0) {
        c = c * exp2(delta);
    }

    // --- calibration (scene linear, before the tone map)
    if (pu(F_CALIB) != 0u || pf(F_SHADOW_TINT) != 0.0) {
        c = calibrate(c);
    }

    // --- tone map on luminance, highlight desaturation
    let yl = lum2020(c);
    let o = tone_apply(yl);
    var d = vec3<f32>(0.0);
    if (yl > 1e-9) {
        d = c * o / yl;
    }
    let mx = max(d.x, max(d.y, d.z));
    if (mx > 1.0) {
        let t = clamp((mx - 1.0) / max(mx - o, 1e-6), 0.0, 1.0);
        d = d + (o - d) * t;
    }

    // --- colour
    d = color_ops(d, lt[11], lt[12]);
    if (tint_on) {
        let lab = oklab(d);
        d = oklab_inv(vec3<f32>(lab.x, lab.y + tint_dir.x * 0.08 * tint_amt, lab.z + tint_dir.y * 0.08 * tint_amt));
    }

    // --- vignette (display linear, post-crop)
    if (pu(F_VIG) != 0u) {
        let fw = f32(w);
        let fh = f32(h);
        let aspect = fw / fh;
        let amount = pf(F_VIG_AMOUNT);
        let mixa = pf(F_VIG_ASPECT_MIX);
        let power = pf(F_VIG_POWER);
        let u = (f32(x) + 0.5) / fw * 2.0 - 1.0;
        let vv = (f32(y) + 0.5) / fh * 2.0 - 1.0;
        let sx = 1.0 + (aspect - 1.0) * mixa;
        let sy = 1.0 + (1.0 / aspect - 1.0) * mixa;
        let ax = abs(u * max(sx, 1.0) / max(sx, sy));
        let ay = abs(vv * max(sy, 1.0) / max(sx, sy));
        let dist = pow(pow(ax, power) + pow(ay, power), 1.0 / power);
        let start = pf(F_VIG_START);
        let t = sstep(start, start + pf(F_VIG_WIDTH), dist);
        if (t > 0.0) {
            let lum = clamp(lum2020(d), 0.0, 1.0);
            if (amount < 0.0) {
                var f = 1.0 + amount * t;
                let style = pu(F_VIG_STYLE);
                if (style == 1u) {
                    f += (1.0 - f) * pf(F_VIG_HL) * sstep(0.4, 1.0, lum);
                }
                if (style == 2u) {
                    d = d * (1.0 - (-amount) * t) + 0.0;
                } else {
                    d = d * f;
                }
            } else {
                d = d + (1.0 - d) * amount * t * 0.85;
            }
        }
    }

    // --- gamut map to the output space (desaturate towards luminance until in range)
    let om = array<vec3<f32>, 3>(
        vec3<f32>(pf(F_OUT_M), pf(F_OUT_M + 1u), pf(F_OUT_M + 2u)),
        vec3<f32>(pf(F_OUT_M + 3u), pf(F_OUT_M + 4u), pf(F_OUT_M + 5u)),
        vec3<f32>(pf(F_OUT_M + 6u), pf(F_OUT_M + 7u), pf(F_OUT_M + 8u)),
    );
    var r = mul3(om, d);
    let yy = clamp(pf(F_OUT_Y) * r.x + pf(F_OUT_Y + 1u) * r.y + pf(F_OUT_Y + 2u) * r.z, 0.0, 1.0);
    var tg = 1.0;
    for (var k = 0u; k < 3u; k++) {
        let cc = r[k];
        if (cc < 0.0) {
            tg = min(tg, yy / max(yy - cc, 1e-9));
        } else if (cc > 1.0) {
            tg = min(tg, (1.0 - yy) / max(cc - yy, 1e-9));
        }
    }
    if (tg < 1.0) {
        r = yy + (r - yy) * tg;
    }

    // --- encode, curves, grain
    var e = vec3<f32>(encode_srgb(r.x), encode_srgb(r.y), encode_srgb(r.z));
    if (pu(F_CURVES) != 0u) {
        let e0 = e;
        e = vec3<f32>(curve(0u, e.x), curve(1u, e.y), curve(2u, e.z));
        let rs = pf(F_REFINE_SAT);
        if (rs < 1.0) {
            e = refine_saturation(e0, e, rs);
        }
    }
    if (pu(F_GRAIN) != 0u) {
        let px = f32(x) + 0.5;
        let py = f32(y) + 0.5;
        let gx = pf(F_GRAIN_AFF) * px + pf(F_GRAIN_AFF + 2u) * py + pf(F_GRAIN_AFF + 4u);
        let gy = pf(F_GRAIN_AFF + 1u) * px + pf(F_GRAIN_AFF + 3u) * py + pf(F_GRAIN_AFF + 5u);
        let sc = pf(F_GRAIN_SC);
        let rough = pf(F_GRAIN_ROUGH);
        let seed = pu(F_GRAIN_SEED);
        var g = grain_noise(gx * sc, gy * sc, seed);
        g = g * (1.0 - rough * 0.5) + grain_noise(gx * sc * 2.3, gy * sc * 2.3, seed ^ 0x55u) * rough * 0.7;
        let lum = 0.2126 * e.x + 0.7152 * e.y + 0.0722 * e.z;
        let k = pf(F_GRAIN_AMT) * g * (0.35 + 2.6 * lum * (1.0 - lum));
        e = e + k;
    }
    e = vec3<f32>(out_encode(e.x), out_encode(e.y), out_encode(e.z));
    out[i] = enc8(e.x) | (enc8(e.y) << 8u) | (enc8(e.z) << 16u) | (255u << 24u);
}
