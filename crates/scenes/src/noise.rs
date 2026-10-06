//! Deterministic gradient noise and fractal sums.

#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    h
}

/// Uniform in 0..1.
#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    (hash(x, y, seed) & 0x00ff_ffff) as f32 / 16_777_216.0
}

#[inline]
fn grad(ix: i32, iy: i32, seed: u32, dx: f32, dy: f32) -> f32 {
    let a = rand01(ix, iy, seed) * std::f32::consts::TAU;
    let (s, c) = a.sin_cos();
    c * dx + s * dy
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Gradient noise in roughly −0.7..0.7.
pub fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (ix, iy) = (x0 as i32, y0 as i32);
    let (fx, fy) = (x - x0, y - y0);
    let (u, v) = (fade(fx), fade(fy));
    let a = grad(ix, iy, seed, fx, fy);
    let b = grad(ix + 1, iy, seed, fx - 1.0, fy);
    let c = grad(ix, iy + 1, seed, fx, fy - 1.0);
    let d = grad(ix + 1, iy + 1, seed, fx - 1.0, fy - 1.0);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}

/// Fractal Brownian motion, roughly −1..1.
pub fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut f, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += noise(x * f, y * f, seed.wrapping_add(o * 101)) * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm * 1.4
}

/// Ridged multifractal, 0..1 (sharp crests — mountain ridges).
pub fn ridged(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut f, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        let n = 1.0 - (noise(x * f, y * f, seed.wrapping_add(o * 131)) * 1.4).abs();
        sum += n * n * amp;
        norm += amp;
        amp *= 0.5;
        f *= 2.1;
    }
    sum / norm
}
