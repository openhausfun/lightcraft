//! Geometry for LightCraft: points, rectangles, affine and projective transforms, and the crop model.
//!
//! Conventions: image coordinates are y-down. *Normalized* coordinates map the full (uncropped,
//! oriented) image to `0..1` on both axes, so tools and settings are resolution independent.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod crop;
mod homography;

pub use crop::{CropGeometry, crop_fit_angle, max_inscribed_scale};
pub use homography::Homography;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Point) -> f64 {
        (self - o).len()
    }
    pub fn lerp(self, o: Point, t: f64) -> Point {
        Point::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
    pub fn to_vec(self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dot(self, o: Vec2) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: Vec2) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn normalized(self) -> Vec2 {
        let l = self.len();
        if l > 0.0 { Vec2::new(self.x / l, self.y / l) } else { self }
    }
    pub fn angle(self) -> f64 {
        self.y.atan2(self.x)
    }
    pub fn rotate(self, a: f64) -> Vec2 {
        let (s, c) = a.sin_cos();
        Vec2::new(self.x * c - self.y * s, self.x * s + self.y * c)
    }
}

impl std::ops::Sub for Point {
    type Output = Vec2;
    fn sub(self, o: Point) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Add<Vec2> for Point {
    type Output = Point;
    fn add(self, v: Vec2) -> Point {
        Point::new(self.x + v.x, self.y + v.y)
    }
}
impl std::ops::Sub<Vec2> for Point {
    type Output = Point;
    fn sub(self, v: Vec2) -> Point {
        Point::new(self.x - v.x, self.y - v.y)
    }
}
impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}
impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}
impl std::ops::Mul<f64> for Vec2 {
    type Output = Vec2;
    fn mul(self, s: f64) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
}
impl std::ops::Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// Axis-aligned rectangle `[x0, x1) × [y0, y1)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub const UNIT: Rect = Rect { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };
    pub const fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1 }
    }
    pub fn from_xywh(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self::new(x, y, x + w, y + h)
    }
    pub fn from_center(c: Point, w: f64, h: f64) -> Self {
        Self::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0)
    }
    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
    pub fn area(&self) -> f64 {
        self.width().max(0.0) * self.height().max(0.0)
    }
    pub fn center(&self) -> Point {
        Point::new((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn aspect(&self) -> f64 {
        if self.height() != 0.0 { self.width() / self.height() } else { 0.0 }
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x < self.x1 && p.y >= self.y0 && p.y < self.y1
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        Rect::new(self.x0.max(o.x0), self.y0.max(o.y0), self.x1.min(o.x1), self.y1.min(o.y1))
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::new(self.x0.min(o.x0), self.y0.min(o.y0), self.x1.max(o.x1), self.y1.max(o.y1))
    }
    pub fn inflate(&self, dx: f64, dy: f64) -> Rect {
        Rect::new(self.x0 - dx, self.y0 - dy, self.x1 + dx, self.y1 + dy)
    }
    pub fn scale(&self, sx: f64, sy: f64) -> Rect {
        Rect::new(self.x0 * sx, self.y0 * sy, self.x1 * sx, self.y1 * sy)
    }
    pub fn translate(&self, v: Vec2) -> Rect {
        Rect::new(self.x0 + v.x, self.y0 + v.y, self.x1 + v.x, self.y1 + v.y)
    }
    pub fn corners(&self) -> [Point; 4] {
        [Point::new(self.x0, self.y0), Point::new(self.x1, self.y0), Point::new(self.x1, self.y1), Point::new(self.x0, self.y1)]
    }
    /// Largest rectangle of aspect `a` (w/h) centred in `self`.
    pub fn fit_aspect(&self, a: f64) -> Rect {
        if a <= 0.0 || self.is_empty() {
            return *self;
        }
        let (w, h) = (self.width(), self.height());
        let (nw, nh) = if w / h > a { (h * a, h) } else { (w, w / a) };
        Rect::from_center(self.center(), nw, nh)
    }
    /// Round outward to integer pixel bounds.
    pub fn round_out(&self) -> Rect {
        Rect::new(self.x0.floor(), self.y0.floor(), self.x1.ceil(), self.y1.ceil())
    }
}

/// 2-D affine transform `[a b c d e f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine(pub [f64; 6]);

impl Default for Affine {
    fn default() -> Self {
        Affine::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    pub fn translate(v: Vec2) -> Affine {
        Affine([1.0, 0.0, 0.0, 1.0, v.x, v.y])
    }
    pub fn scale(sx: f64, sy: f64) -> Affine {
        Affine([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }
    pub fn rotate(a: f64) -> Affine {
        let (s, c) = a.sin_cos();
        Affine([c, s, -s, c, 0.0, 0.0])
    }
    pub fn rotate_about(a: f64, p: Point) -> Affine {
        Affine::translate(p.to_vec()) * Affine::rotate(a) * Affine::translate(-p.to_vec())
    }
    pub fn apply(&self, p: Point) -> Point {
        let [a, b, c, d, e, f] = self.0;
        Point::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }
    pub fn apply_vec(&self, v: Vec2) -> Vec2 {
        let [a, b, c, d, ..] = self.0;
        Vec2::new(a * v.x + c * v.y, b * v.x + d * v.y)
    }
    pub fn determinant(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }
    pub fn inverse(&self) -> Option<Affine> {
        let det = self.determinant();
        if det.abs() < 1e-300 {
            return None;
        }
        let [a, b, c, d, e, f] = self.0;
        let inv = 1.0 / det;
        Some(Affine([d * inv, -b * inv, -c * inv, a * inv, (c * f - d * e) * inv, (b * e - a * f) * inv]))
    }
    /// Bounding box of a transformed rectangle.
    pub fn transform_rect_bbox(&self, r: &Rect) -> Rect {
        let pts = r.corners().map(|p| self.apply(p));
        let mut out = Rect::new(pts[0].x, pts[0].y, pts[0].x, pts[0].y);
        for p in &pts[1..] {
            out = out.union(&Rect::new(p.x, p.y, p.x, p.y));
        }
        out
    }
}

impl std::ops::Mul for Affine {
    type Output = Affine;
    /// `self * o` applies `o` first, then `self`.
    fn mul(self, o: Affine) -> Affine {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = o.0;
        Affine([a * a2 + c * b2, b * a2 + d * b2, a * c2 + c * d2, b * c2 + d * d2, a * e2 + c * f2 + e, b * e2 + d * f2 + f])
    }
}

/// EXIF orientation (1..=8) as a transform of the stored pixel grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Normal,
    FlipH,
    Rotate180,
    FlipV,
    Transpose,
    Rotate90,
    Transverse,
    Rotate270,
}

impl Orientation {
    pub fn from_exif(v: u16) -> Orientation {
        match v {
            2 => Orientation::FlipH,
            3 => Orientation::Rotate180,
            4 => Orientation::FlipV,
            5 => Orientation::Transpose,
            6 => Orientation::Rotate90,
            7 => Orientation::Transverse,
            8 => Orientation::Rotate270,
            _ => Orientation::Normal,
        }
    }
    pub fn to_exif(self) -> u16 {
        match self {
            Orientation::Normal => 1,
            Orientation::FlipH => 2,
            Orientation::Rotate180 => 3,
            Orientation::FlipV => 4,
            Orientation::Transpose => 5,
            Orientation::Rotate90 => 6,
            Orientation::Transverse => 7,
            Orientation::Rotate270 => 8,
        }
    }
    /// Whether width and height swap.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Orientation::Transpose | Orientation::Rotate90 | Orientation::Transverse | Orientation::Rotate270)
    }
    /// Decompose into (clockwise quarter turns applied after an optional horizontal flip).
    pub fn to_parts(self) -> (bool, u8) {
        match self {
            Orientation::Normal => (false, 0),
            Orientation::Rotate90 => (false, 1),
            Orientation::Rotate180 => (false, 2),
            Orientation::Rotate270 => (false, 3),
            Orientation::FlipH => (true, 0),
            Orientation::Transverse => (true, 1),
            Orientation::FlipV => (true, 2),
            Orientation::Transpose => (true, 3),
        }
    }
    pub fn from_parts(flip: bool, quarter_turns_cw: u8) -> Orientation {
        match (flip, quarter_turns_cw % 4) {
            (false, 0) => Orientation::Normal,
            (false, 1) => Orientation::Rotate90,
            (false, 2) => Orientation::Rotate180,
            (false, _) => Orientation::Rotate270,
            (true, 0) => Orientation::FlipH,
            (true, 1) => Orientation::Transverse,
            (true, 2) => Orientation::FlipV,
            (true, _) => Orientation::Transpose,
        }
    }
    /// Rotate the (already oriented) image a further quarter turn clockwise (`cw`) or counter-clockwise.
    pub fn rotated(self, cw: bool) -> Orientation {
        let (f, t) = self.to_parts();
        Orientation::from_parts(f, if cw { t + 1 } else { t + 3 })
    }
    /// Flip the displayed image horizontally.
    pub fn flipped_h(self) -> Orientation {
        let (f, t) = self.to_parts();
        // flipH ∘ rot(t) = rot(-t) ∘ flipH
        Orientation::from_parts(!f, (4 - t) % 4)
    }
    /// Map a point in stored pixel space (w×h stored dims) to oriented space.
    pub fn map(self, x: f64, y: f64, w: f64, h: f64) -> (f64, f64) {
        let (flip, t) = self.to_parts();
        let (mut x, y2, mut w2, mut h2) = (x, y, w, h);
        if flip {
            x = w2 - x;
        }
        let mut y = y2;
        for _ in 0..t {
            // rotate 90° cw: (x, y) -> (h - y, x)
            let nx = h2 - y;
            let ny = x;
            x = nx;
            y = ny;
            std::mem::swap(&mut w2, &mut h2);
        }
        (x, y)
    }
}

pub fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

pub fn deg(r: f64) -> f64 {
    r.to_degrees()
}

pub fn rad(d: f64) -> f64 {
    d.to_radians()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        a.dist(b) < 1e-9
    }

    #[test]
    fn affine_compose_and_inverse() {
        let t = Affine::translate(Vec2::new(3.0, -2.0)) * Affine::rotate(0.7) * Affine::scale(2.0, 0.5);
        let inv = t.inverse().unwrap();
        let p = Point::new(1.5, 4.0);
        assert!(close(inv.apply(t.apply(p)), p));
        assert!(close((t * inv).apply(p), p));
    }

    #[test]
    fn rotate_about_fixes_centre() {
        let c = Point::new(10.0, 5.0);
        assert!(close(Affine::rotate_about(1.1, c).apply(c), c));
    }

    #[test]
    fn rect_ops() {
        let r = Rect::new(0.0, 0.0, 4.0, 2.0);
        assert_eq!(r.aspect(), 2.0);
        let f = r.fit_aspect(1.0);
        assert_eq!((f.width(), f.height()), (2.0, 2.0));
        assert_eq!(f.center(), r.center());
        assert!(r.contains(Point::new(3.9, 1.9)));
        assert!(!r.contains(Point::new(4.0, 1.0)));
        assert!(r.intersect(&Rect::new(5.0, 5.0, 6.0, 6.0)).is_empty());
    }

    #[test]
    fn orientation_roundtrip_and_dims() {
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            assert_eq!(o.to_exif(), v);
            let (f, t) = o.to_parts();
            assert_eq!(Orientation::from_parts(f, t), o);
        }
        assert!(Orientation::Rotate90.swaps_axes());
        // Rotate90: top-left of stored goes to top-right of display.
        let (x, y) = Orientation::Rotate90.map(0.0, 0.0, 4.0, 2.0);
        assert_eq!((x, y), (2.0, 0.0));
        assert_eq!(Orientation::Normal.rotated(true).rotated(true), Orientation::Rotate180);
        assert_eq!(Orientation::Normal.rotated(false), Orientation::Rotate270);
        assert_eq!(Orientation::Normal.flipped_h(), Orientation::FlipH);
        assert_eq!(Orientation::Rotate90.flipped_h().flipped_h(), Orientation::Rotate90);
    }

    proptest::proptest! {
        #[test]
        fn affine_inverse_prop(a in -3.0f64..3.0, sx in 0.1f64..5.0, sy in 0.1f64..5.0, tx in -50.0f64..50.0, px in -20.0f64..20.0, py in -20.0f64..20.0) {
            let t = Affine::translate(Vec2::new(tx, -tx)) * Affine::rotate(a) * Affine::scale(sx, sy);
            let p = Point::new(px, py);
            let q = t.inverse().unwrap().apply(t.apply(p));
            proptest::prop_assert!(q.dist(p) < 1e-7);
        }
    }
}
