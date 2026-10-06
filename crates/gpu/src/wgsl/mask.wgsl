// Mask evaluation (`lightcraft_pipeline::masks`): one component shape → `c`, then combined into
// the mask's alpha plane. Bindings: img (rgb, pre-exposure), log_l, aux (shape data), c, alpha.

// Position of output pixel (x, y) in long-edge units: P[4..10] = affine (a, b, c, d, e, f).
fn pos(x: u32, y: u32) -> vec2<f32> {
    let px = f32(x) + 0.5;
    let py = f32(y) + 0.5;
    return vec2<f32>(pf(4u) * px + pf(6u) * py + pf(8u), pf(5u) * px + pf(7u) * py + pf(9u));
}

// P: w, h, kind, invert, affine[6], then per kind:
//   0 linear: a.xy, d.xy, |d|²         1 radial: centre.xy, cos, sin, rx, ry, feather, invert
//   2 luminance range: lo, hi, lo feather, hi feather, ev
//   3 colour range: tol, gain, sample count (OkLab triples in aux)
//   4 brush: stroke count (stroke records + dabs in aux)
// `masks::chromaticity` of pixel i.
fn chroma_at(i: u32) -> vec3<f32> {
    let c = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
    return c / max(lum2020(c), 1e-6);
}

// `masks::auto_similarity`.
fn auto_similarity(l: f32, ch: vec3<f32>, rl: f32, rch: vec3<f32>) -> f32 {
    let dl = (l - rl) / AUTO_TOL_EV;
    let dc = length(ch - rch) / AUTO_TOL_CHROMA;
    return 1.0 - sstep(0.5, 1.0, sqrt(dl * dl + dc * dc));
}

@compute @workgroup_size(16, 16)
fn shape(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let h = pu(1u);
    if (g.x >= w || g.y >= h) {
        return;
    }
    let i = g.y * w + g.x;
    let kind = pu(2u);
    var v = 0.0;
    if (kind == 0u) {
        let p = pos(g.x, g.y);
        let a = vec2<f32>(pf(10u), pf(11u));
        let d = vec2<f32>(pf(12u), pf(13u));
        let t = dot(p - a, d) / pf(14u);
        v = 1.0 - sstep(0.0, 1.0, t);
    } else if (kind == 1u) {
        let p = pos(g.x, g.y);
        let d = p - vec2<f32>(pf(10u), pf(11u));
        let co = pf(12u);
        let s = pf(13u);
        let x = d.x * co - d.y * s;
        let y = d.x * s + d.y * co;
        let qx = x / pf(14u);
        let qy = y / pf(15u);
        let r = sqrt(qx * qx + qy * qy);
        let f = pf(16u);
        v = 1.0 - sstep(1.0 - max(f, 0.01), 1.0, r);
        if (pu(17u) != 0u) {
            v = 1.0 - v;
        }
    } else if (kind == 2u) {
        let lo = pf(10u);
        let hi = pf(11u);
        let y = clamp((log_l[i] + pf(14u) + 8.0) / 12.0, 0.0, 1.0);
        v = sstep(lo - pf(12u), lo, y) * (1.0 - sstep(hi, hi + pf(13u), y));
    } else if (kind == 3u) {
        let tol = pf(10u);
        let gain = pf(11u);
        let c = vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]) * gain;
        let lab = oklab(c / (1.0 + c));
        var best = 3.402823e38;
        for (var k = 0u; k < pu(12u); k++) {
            let s = vec3<f32>(aux[3u * k], aux[3u * k + 1u], aux[3u * k + 2u]);
            let da = lab.y - s.y;
            let db = lab.z - s.z;
            let dl = lab.x - s.x;
            best = min(best, sqrt(da * da + db * db + 0.25 * (dl * dl)));
        }
        v = 1.0 - sstep(tol * 0.5, tol, best);
    } else if (kind == 4u) {
        // stroke record: dab offset, dab count, r, hard, flow, density, erase, bbox x0 y0 x1 y1, auto
        let px = f32(g.x) + 0.5;
        let py = f32(g.y) + 0.5;
        let lp = log_l[i];
        let chp = chroma_at(i);
        for (var k = 0u; k < pu(10u); k++) {
            let o = 12u * k;
            let auto_mask = aux[o + 11u] != 0.0;
            if (px < aux[o + 7u] || py < aux[o + 8u] || px > aux[o + 9u] || py > aux[o + 10u]) {
                continue;
            }
            let off = u32(aux[o]);
            let cnt = u32(aux[o + 1u]);
            let r = aux[o + 2u];
            let hard = aux[o + 3u];
            let flow = aux[o + 4u];
            var keep = 1.0;
            for (var j = 0u; j < cnt; j++) {
                let dx = px - aux[off + 2u * j];
                let dy = py - aux[off + 2u * j + 1u];
                let dd = sqrt(dx * dx + dy * dy);
                if (dd > r) {
                    continue;
                }
                var a = 1.0;
                if (dd > hard) {
                    a = 1.0 - sstep(hard, r, dd);
                }
                if (auto_mask) {
                    let rx = min(u32(max(floor(aux[off + 2u * j]), 0.0)), w - 1u);
                    let ry = min(u32(max(floor(aux[off + 2u * j + 1u]), 0.0)), h - 1u);
                    let ri = ry * w + rx;
                    a *= auto_similarity(lp, chp, log_l[ri], chroma_at(ri));
                }
                keep *= 1.0 - a * flow;
            }
            let sa = min(1.0 - keep, aux[o + 5u]);
            if (aux[o + 6u] != 0.0) {
                v = v * (1.0 - sa);
            } else {
                v = max(v, sa);
            }
        }
    }
    if (pu(3u) != 0u) {
        v = 1.0 - v;
    }
    c[i] = v;
}

// Combine `c` into mask plane P[2]: P[3] = 0 copy, 1 add (max), 2 subtract, 3 intersect.
@compute @workgroup_size(256)
fn combine(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    let n = pu(0u);
    if (i >= n) {
        return;
    }
    let j = pu(2u) * n + i;
    let op = pu(3u);
    let a = alpha[j];
    let v = c[i];
    if (op == 0u) {
        alpha[j] = v;
    } else if (op == 1u) {
        alpha[j] = max(a, v);
    } else if (op == 2u) {
        alpha[j] = a * (1.0 - v);
    } else {
        alpha[j] = a * v;
    }
}

// Finish mask plane P[2]: P[3] = invert, P[4] = amount (applied when ≠ 1).
@compute @workgroup_size(256)
fn finalize(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    let n = pu(0u);
    if (i >= n) {
        return;
    }
    let j = pu(2u) * n + i;
    var a = alpha[j];
    if (pu(3u) != 0u) {
        a = 1.0 - a;
    }
    let amt = pf(4u);
    if (abs(amt - 1.0) > 1e-6) {
        a = a * amt;
    }
    alpha[j] = a;
}
