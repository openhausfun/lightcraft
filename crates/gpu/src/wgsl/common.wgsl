// Shared helpers, prepended to every kernel (after the generated constants and bindings).
// Each function mirrors its CPU twin in `lightcraft-pipeline` / `lightcraft-color`; keep them in step.

fn pu(i: u32) -> u32 {
    return P[i];
}

fn pf(i: u32) -> f32 {
    return bitcast<f32>(P[i]);
}

// Linear index of a 1-D kernel dispatched on a 2-D grid of 256-wide workgroups.
fn lin_index(g: vec3<u32>, n: vec3<u32>) -> u32 {
    return g.x + g.y * n.x * 256u;
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn rem_euclid(a: f32, b: f32) -> f32 {
    let r = a % b;
    return select(r, r + b, r < 0.0);
}

const PI: f32 = 3.14159265358979323846;
const TAU: f32 = 6.28318530717958647692;
const GREY: f32 = 0.18;

fn wrap_angle(a: f32) -> f32 {
    return rem_euclid(a + PI, TAU) - PI;
}

// Rec.2020 luminance (`lightcraft_color::luminance_2020`).
fn lum2020(c: vec3<f32>) -> f32 {
    return c.x * 0.2627 + c.y * 0.6780 + c.z * 0.0593;
}

// `lightcraft_pipeline::local::log_lum`.
fn log_lum(c: vec3<f32>) -> f32 {
    return log2(max(lum2020(c), 1e-7) / GREY);
}

fn mul3(m: array<vec3<f32>, 3>, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        m[0].x * v.x + m[0].y * v.y + m[0].z * v.z,
        m[1].x * v.x + m[1].y * v.y + m[1].z * v.z,
        m[2].x * v.x + m[2].y * v.y + m[2].z * v.z,
    );
}

fn cbrt(x: f32) -> f32 {
    return sign(x) * pow(abs(x), 1.0 / 3.0);
}

// Linear Rec.2020 → OkLab.
fn oklab(c: vec3<f32>) -> vec3<f32> {
    let lms = mul3(OK_TO_LMS, c);
    return mul3(OK_TO_LAB, vec3<f32>(cbrt(lms.x), cbrt(lms.y), cbrt(lms.z)));
}

// OkLab → linear Rec.2020.
fn oklab_inv(lab: vec3<f32>) -> vec3<f32> {
    let l = mul3(OK_FROM_LAB, lab);
    return mul3(OK_FROM_LMS, l * l * l);
}
