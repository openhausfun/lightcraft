// Separable resample with precomputed taps (`lightcraft_raster::resample::weights`, uploaded):
// table[3i..3i+3] = (first source index, tap count, weight offset), weights stored as f32 bits.
// Bindings: src, table (u32), dst.

// Horizontal. P: sw, h, dw, nc.
@compute @workgroup_size(16, 16)
fn resize_h(@builtin(global_invocation_id) g: vec3<u32>) {
    let sw = pu(0u);
    let h = pu(1u);
    let dw = pu(2u);
    let nc = pu(3u);
    let x = g.x;
    let y = g.y;
    if (x >= dw || y >= h) {
        return;
    }
    let lo = table[3u * x];
    let cnt = table[3u * x + 1u];
    let off = table[3u * x + 2u];
    for (var c = 0u; c < nc; c++) {
        var acc = 0.0;
        for (var k = 0u; k < cnt; k++) {
            acc = acc + src[(y * sw + lo + k) * nc + c] * bitcast<f32>(table[off + k]);
        }
        dst[(y * dw + x) * nc + c] = acc;
    }
}

// Vertical. P: w, sh, dh, nc.
@compute @workgroup_size(16, 16)
fn resize_v(@builtin(global_invocation_id) g: vec3<u32>) {
    let w = pu(0u);
    let sh = pu(1u);
    let dh = pu(2u);
    let nc = pu(3u);
    let x = g.x;
    let y = g.y;
    if (x >= w || y >= dh) {
        return;
    }
    let lo = table[3u * y];
    let cnt = table[3u * y + 1u];
    let off = table[3u * y + 2u];
    for (var c = 0u; c < nc; c++) {
        var acc = 0.0;
        for (var k = 0u; k < cnt; k++) {
            acc = acc + src[((lo + k) * w + x) * nc + c] * bitcast<f32>(table[off + k]);
        }
        dst[(y * w + x) * nc + c] = acc;
    }
}
