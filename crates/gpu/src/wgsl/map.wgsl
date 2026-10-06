// Element-wise kernels (one thread per pixel, 1-D on a 2-D grid). Bindings: a, b, c (inputs), dst.
// P[0] = pixel count; further parameters per kernel. Each mirrors a CPU expression in
// `lightcraft_pipeline::local` (named in the comment).

fn rgb_a(i: u32) -> vec3<f32> {
    return vec3<f32>(a[3u * i], a[3u * i + 1u], a[3u * i + 2u]);
}

fn put_rgb(i: u32, v: vec3<f32>) {
    dst[3u * i] = v.x;
    dst[3u * i + 1u] = v.y;
    dst[3u * i + 2u] = v.z;
}

// `log_lum` of an RGB image.
@compute @workgroup_size(256)
fn log_lum_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = log_lum(rgb_a(i));
}

// `dark_of`: the dark channel (dehaze).
@compute @workgroup_size(256)
fn dark_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let c = rgb_a(i);
    dst[i] = min(min(c.x, c.y), c.z);
}

// Guided filter input pair (p, p²).
@compute @workgroup_size(256)
fn guided_pre(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let p = a[i];
    dst[2u * i] = p;
    dst[2u * i + 1u] = p * p;
}

// Guided filter coefficients (a, b) from blurred (mean, corr). P[1] = eps.
@compute @workgroup_size(256)
fn guided_ab(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let m = a[2u * i];
    let c = a[2u * i + 1u];
    let v = max(c - m * m, 0.0);
    let k = v / (v + pf(1u));
    dst[2u * i] = k;
    dst[2u * i + 1u] = m - k * m;
}

// `q = a·p + b` (a: p, b: interleaved blurred coefficients).
@compute @workgroup_size(256)
fn guided_apply(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = b[2u * i] * a[i] + b[2u * i + 1u];
}

// Cross guided filter (`masks::guided_cross`), a: guide, b: input. P[1] = 0: (guide, input);
// 1: (guide·input, guide²).
@compute @workgroup_size(256)
fn xguided_pre(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let gi = a[i];
    let p = b[i];
    if (pu(1u) == 0u) {
        dst[2u * i] = gi;
        dst[2u * i + 1u] = p;
    } else {
        dst[2u * i] = gi * p;
        dst[2u * i + 1u] = gi * gi;
    }
}

// Coefficients (a, b) from blurred means (a: guide, input) and correlations (b: guide·input,
// guide²). P[1] = eps.
@compute @workgroup_size(256)
fn xguided_ab(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let mi = a[2u * i];
    let mp = a[2u * i + 1u];
    let v = max(b[2u * i + 1u] - mi * mi, 0.0);
    let k = (b[2u * i] - mi * mp) / (v + pf(1u));
    dst[2u * i] = k;
    dst[2u * i + 1u] = mp - k * mi;
}

// `max(p, clamp(a·guide + b, 0, 1))` (a: guide, b: blurred coefficients, c: the input p).
@compute @workgroup_size(256)
fn xguided_apply(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = max(c[i], clamp(b[2u * i] * a[i] + b[2u * i + 1u], 0.0, 1.0));
}

// White balance: 3×3 matrix (P[1] = has matrix, P[2..11] row-major), clamped at 0.
@compute @workgroup_size(256)
fn wb_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    var c = rgb_a(i);
    if (pu(1u) != 0u) {
        c = vec3<f32>(
            pf(2u) * c.x + pf(3u) * c.y + pf(4u) * c.z,
            pf(5u) * c.x + pf(6u) * c.y + pf(7u) * c.z,
            pf(8u) * c.x + pf(9u) * c.y + pf(10u) * c.z,
        );
    }
    put_rgb(i, max(c * 1.0, vec3<f32>(0.0)));
}

// Luminance NR: scale by 2^((f − l)·k) (a: image, b: log luminance l, c: filtered f). P[1] = k.
@compute @workgroup_size(256)
fn nr_lum(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let d = (c[i] - b[i]) * pf(1u);
    put_rgb(i, rgb_a(i) * exp2(d));
}

// Chromaticity rgb / Y.
@compute @workgroup_size(256)
fn chroma_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let c0 = rgb_a(i);
    let y = max(lum2020(c0), 1e-6);
    put_rgb(i, vec3<f32>(c0.x / y, c0.y / y, c0.z / y));
}

// Colour NR: blend chromaticity towards its blur and re-apply luminance (a: image, b: chroma,
// c: blurred chroma). P[1] = t.
@compute @workgroup_size(256)
fn nr_col(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let yl = lum2020(rgb_a(i));
    let t = pf(1u);
    let c0 = vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]);
    let cb = vec3<f32>(c[3u * i], c[3u * i + 1u], c[3u * i + 2u]);
    put_rgb(i, max((c0 + (cb - c0) * t) * yl, vec3<f32>(0.0)));
}

// Every P[1]-th value of `a` (airlight sampling). P[0] = output count.
@compute @workgroup_size(256)
fn subsample(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = a[i * pu(1u)];
}

// `redeye::eye_pixel` (a: image). P[1] = width, P[2] = eye count, then EYE_WORDS per eye:
// cx, cy, r, darken, pet, has catchlight, catchlight x, y, r (output pixels).
@compute @workgroup_size(256)
fn redeye_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let w = pu(1u);
    let x = f32(i % w) + 0.5;
    let y = f32(i / w) + 0.5;
    var c = rgb_a(i);
    for (var e = 0u; e < pu(2u); e++) {
        let o = 3u + e * EYE_WORDS;
        let ex = x - pf(o);
        let ey = y - pf(o + 1u);
        let d = sqrt(ex * ex + ey * ey) / pf(o + 2u);
        var m = 1.0 - sstep(1.0, 1.35, d);
        if (pf(o + 4u) != 0.0) {
            m = 1.0 - sstep(0.95, 1.2, d);
        }
        let darken = pf(o + 3u);
        if (m > 0.0) {
            if (pf(o + 4u) != 0.0) {
                let k = min(lum2020(c), 0.04) * (1.0 - 0.85 * darken);
                c = c + (vec3<f32>(k) - c) * m;
            } else {
                let red = (c.x - max(c.y, c.z)) / max(c.x, 1e-6);
                let a = m * sstep(0.25, 0.5, red);
                let n = 0.5 * (c.y + c.z) * (1.0 - 0.9 * darken);
                c = c + (vec3<f32>(n) - c) * a;
            }
        }
        if (pf(o + 5u) != 0.0) {
            let lx = x - pf(o + 6u);
            let ly = y - pf(o + 7u);
            let dc = sqrt(lx * lx + ly * ly) / pf(o + 8u);
            let mc = 1.0 - sstep(0.5, 1.0, dc);
            if (mc > 0.0) {
                c = c + (vec3<f32>(0.9) - c) * mc;
            }
        }
    }
    put_rgb(i, c);
}
