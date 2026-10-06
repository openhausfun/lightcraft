//! Panorama projections. World directions use camera-style axes: x right, y down, z forward.
//! Surface coordinates are in units of the output focal length `f` (pixels).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    /// Pick from the field of view (see [`crate::pano::choose_projection`]).
    #[default]
    Auto,
    /// Longitude/latitude (equirectangular): any field of view; straight lines bend.
    Spherical,
    /// Longitude/height on a cylinder: wide horizontal panoramas; verticals stay straight.
    Cylindrical,
    /// Rectilinear (gnomonic): straight lines stay straight; fields of view < ~120°.
    Perspective,
}

impl Projection {
    /// Project a world direction to surface coordinates (`None` when not representable).
    #[inline]
    pub fn forward(self, d: [f64; 3], f: f64) -> Option<(f64, f64)> {
        match self {
            Projection::Perspective => {
                if d[2] <= 1e-6 {
                    return None;
                }
                Some((f * d[0] / d[2], f * d[1] / d[2]))
            }
            Projection::Cylindrical => {
                let r = (d[0] * d[0] + d[2] * d[2]).sqrt();
                if r < 1e-12 {
                    return None;
                }
                Some((f * d[0].atan2(d[2]), f * d[1] / r))
            }
            Projection::Spherical | Projection::Auto => {
                let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if n < 1e-12 {
                    return None;
                }
                Some((f * d[0].atan2(d[2]), f * (d[1] / n).clamp(-1.0, 1.0).asin()))
            }
        }
    }

    /// The world direction (not normalised) of surface coordinates.
    #[inline]
    pub fn inverse(self, x: f64, y: f64, f: f64) -> [f64; 3] {
        match self {
            Projection::Perspective => [x / f, y / f, 1.0],
            Projection::Cylindrical => {
                let t = x / f;
                [t.sin(), y / f, t.cos()]
            }
            Projection::Spherical | Projection::Auto => {
                let (t, p) = (x / f, y / f);
                [t.sin() * p.cos(), p.sin(), t.cos() * p.cos()]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_round_trip() {
        for p in [Projection::Perspective, Projection::Cylindrical, Projection::Spherical] {
            for d in [[0.1, -0.2, 1.0], [-0.5, 0.3, 0.8], [0.0, 0.0, 1.0]] {
                let (x, y) = p.forward(d, 1000.0).unwrap();
                let back = p.inverse(x, y, 1000.0);
                let (n1, n2) = (crate::linalg::normalize(d), crate::linalg::normalize(back));
                for c in 0..3 {
                    assert!((n1[c] - n2[c]).abs() < 1e-9, "{p:?} {d:?} {back:?}");
                }
            }
        }
        assert!(Projection::Perspective.forward([1.0, 0.0, -0.1], 1.0).is_none());
    }
}
