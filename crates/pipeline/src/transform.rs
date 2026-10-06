//! Perspective transforms: the manual Transform sliders (vertical/horizontal keystone, rotate, aspect, scale,
//! offset) as one homography, composed with the Upright correction.
//!
//! All homographies here map *lens-corrected* centred coordinates to *transformed* centred coordinates, where
//! centred coordinates are `(p − image centre) / (long edge / 2)`. Keystone corrections are modelled as a virtual
//! camera rotation `K·R·K⁻¹` (focal length [`DEFAULT_FOCAL`] in those units), re-centred so the image centre stays
//! put and its local scale is preserved.

use lightcraft_develop::Geometry;
use lightcraft_geom::Homography;

/// Assumed focal length in half-long-edge units (≈ 27 mm on a 36 mm-wide frame).
pub const DEFAULT_FOCAL: f64 = 1.5;
/// Vertical/Horizontal ±100 → virtual camera rotation (radians).
const KEYSTONE_MAX: f64 = 0.5;

pub type Mat3 = [[f64; 3]; 3];

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn to_h(m: &Mat3) -> Homography {
    Homography([m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2]])
}

/// `K·R·K⁻¹` for a rotation `r` (rows) and focal length `f`.
pub fn rotation_homography(r: &Mat3, f: f64) -> Homography {
    let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
    let ki = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
    to_h(&mat_mul(&k, &mat_mul(r, &ki)))
}

pub fn rot_x(a: f64) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

pub fn rot_y(a: f64) -> Mat3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// Translate so the origin maps to itself and scale so the local area there is unchanged.
pub fn recentre(h: &Homography) -> Homography {
    let o = h.apply(lightcraft_geom::Point::new(0.0, 0.0));
    let t = Homography([1.0, 0.0, -o.x, 0.0, 1.0, -o.y, 0.0, 0.0, 1.0]).mul(h);
    // Jacobian at the origin (numerically)
    let e = 1e-4;
    let px = t.apply(lightcraft_geom::Point::new(e, 0.0));
    let py = t.apply(lightcraft_geom::Point::new(0.0, e));
    let det = ((px.x * py.y - px.y * py.x) / (e * e)).abs();
    let s = if det > 1e-12 { 1.0 / det.sqrt() } else { 1.0 };
    Homography([s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0]).mul(&t)
}

/// The manual Transform sliders as a homography (lens-corrected → transformed, centred coordinates).
/// Signs (like Lightroom): Vertical − widens the top (fixes converging verticals shot from below), Horizontal −
/// enlarges the left side, Rotate + turns the image clockwise, Aspect + stretches vertically, Scale > 100 zooms in,
/// Offset X + moves the image right, Offset Y + moves it up.
pub fn manual(g: &Geometry) -> Homography {
    let mut h = Homography::IDENTITY;
    if g.vertical != 0.0 || g.horizontal != 0.0 {
        let r = mat_mul(&rot_y(g.horizontal / 100.0 * KEYSTONE_MAX), &rot_x(-g.vertical / 100.0 * KEYSTONE_MAX));
        h = recentre(&rotation_homography(&r, DEFAULT_FOCAL));
    }
    if g.rotate != 0.0 {
        let (s, c) = g.rotate.to_radians().sin_cos();
        h = Homography([c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0]).mul(&h);
    }
    if g.aspect != 0.0 {
        let k = (g.aspect / 200.0).exp2();
        h = Homography([1.0 / k, 0.0, 0.0, 0.0, k, 0.0, 0.0, 0.0, 1.0]).mul(&h);
    }
    let s = (g.scale / 100.0).clamp(0.1, 10.0);
    if s != 1.0 {
        h = Homography([s, 0.0, 0.0, 0.0, s, 0.0, 0.0, 0.0, 1.0]).mul(&h);
    }
    if g.offset_x != 0.0 || g.offset_y != 0.0 {
        h = Homography([1.0, 0.0, g.offset_x / 100.0 * 0.5, 0.0, 1.0, -g.offset_y / 100.0 * 0.5, 0.0, 0.0, 1.0]).mul(&h);
    }
    h
}
