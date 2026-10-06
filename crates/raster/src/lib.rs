//! Image buffers and basic raster operations for LightCraft.
//!
//! - [`Image<T>`]: interleaved, row-major pixels (`Rgb32f = Image<[f32; 3]>`, `Rgba8 = Image<[u8; 4]>`,
//!   `Plane = Image<f32>`).
//! - Resampling ([`resample`]): separable Lanczos-3 / Mitchell / box, rayon-parallel.
//! - Blur ([`blur`]): separable Gaussian via repeated box filters (O(1) per pixel in the radius).
//! - [`Histogram`] of display-encoded RGB + luminance.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod blur;
pub mod histogram;
pub mod resample;

pub use histogram::Histogram;

/// Run `f` over row chunks, in parallel when the `parallel` feature is on.
pub fn par_rows<T: Send>(data: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync + Send) {
    if row_len == 0 {
        return;
    }
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        data.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| f(y, row));
    }
    #[cfg(not(feature = "parallel"))]
    {
        data.chunks_mut(row_len).enumerate().for_each(|(y, row)| f(y, row));
    }
}

/// Run `a` and `b` potentially in parallel (sequentially without the `parallel` feature). For
/// independent passes that each scale poorly on their own (small images, short rows).
pub fn par_join<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    #[cfg(feature = "parallel")]
    {
        rayon::join(a, b)
    }
    #[cfg(not(feature = "parallel"))]
    {
        (a(), b())
    }
}

/// Interleaved row-major image.
#[derive(Clone, Debug, PartialEq)]
pub struct Image<T> {
    pub width: usize,
    pub height: usize,
    pub data: Vec<T>,
}

pub type Rgb32f = Image<[f32; 3]>;
pub type Rgba8 = Image<[u8; 4]>;
pub type Plane = Image<f32>;

impl<T: Copy + Default> Image<T> {
    pub fn new(width: usize, height: usize) -> Self {
        Self { width, height, data: vec![T::default(); width * height] }
    }
    pub fn filled(width: usize, height: usize, v: T) -> Self {
        Self { width, height, data: vec![v; width * height] }
    }
    pub fn from_fn(width: usize, height: usize, f: impl Fn(usize, usize) -> T) -> Self {
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self { width, height, data }
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> T {
        self.data[y * self.width + x]
    }
    /// Clamped access (edge extension).
    #[inline]
    pub fn get_clamped(&self, x: isize, y: isize) -> T {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.data[y * self.width + x]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: T) {
        self.data[y * self.width + x] = v;
    }
    pub fn row(&self, y: usize) -> &[T] {
        &self.data[y * self.width..(y + 1) * self.width]
    }
    pub fn row_mut(&mut self, y: usize) -> &mut [T] {
        &mut self.data[y * self.width..(y + 1) * self.width]
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    /// [`Self::map`] into the same buffer (no second image in memory).
    pub fn map_in_place(&mut self, f: impl Fn(T) -> T + Sync + Send)
    where
        T: Send,
    {
        let w = self.width;
        par_rows(&mut self.data, w, |_, row| {
            for p in row.iter_mut() {
                *p = f(*p);
            }
        });
    }

    pub fn map<U: Copy + Default + Send>(&self, f: impl Fn(T) -> U + Sync + Send) -> Image<U>
    where
        T: Sync,
    {
        let mut out = Image::<U>::new(self.width, self.height);
        let w = self.width;
        par_rows(&mut out.data, w, |y, row| {
            let src = &self.data[y * w..(y + 1) * w];
            for (o, s) in row.iter_mut().zip(src) {
                *o = f(*s);
            }
        });
        out
    }
    /// Per-pixel combination of two images of the same size (row-parallel).
    pub fn zip_map<U: Copy + Default + Sync, V: Copy + Default + Send>(&self, other: &Image<U>, f: impl Fn(T, U) -> V + Sync + Send) -> Image<V>
    where
        T: Sync,
    {
        assert_eq!((self.width, self.height), (other.width, other.height), "zip_map: size mismatch");
        let mut out = Image::<V>::new(self.width, self.height);
        let w = self.width;
        par_rows(&mut out.data, w, |y, row| {
            let (a, b) = (&self.data[y * w..(y + 1) * w], &other.data[y * w..(y + 1) * w]);
            for ((o, s), t) in row.iter_mut().zip(a).zip(b) {
                *o = f(*s, *t);
            }
        });
        out
    }
    /// Crop to `[x0, x0+w) × [y0, y0+h)` (clamped to the image).
    pub fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Self {
        let x0 = x0.min(self.width);
        let y0 = y0.min(self.height);
        let w = w.min(self.width - x0);
        let h = h.min(self.height - y0);
        let mut data = Vec::with_capacity(w * h);
        for y in y0..y0 + h {
            data.extend_from_slice(&self.data[y * self.width + x0..y * self.width + x0 + w]);
        }
        Self { width: w, height: h, data }
    }
    /// [`Self::crop`] in place: rows move to the front of the same buffer (no second image).
    pub fn into_crop(mut self, x0: usize, y0: usize, w: usize, h: usize) -> Self {
        let x0 = x0.min(self.width);
        let y0 = y0.min(self.height);
        let w = w.min(self.width - x0);
        let h = h.min(self.height - y0);
        for (i, y) in (y0..y0 + h).enumerate() {
            let src = y * self.width + x0;
            self.data.copy_within(src..src + w, i * w);
        }
        self.data.truncate(w * h);
        Self { width: w, height: h, data: self.data }
    }
    /// Rotate 90° clockwise.
    pub fn rotate_cw(&self) -> Self {
        let (w, h) = (self.width, self.height);
        Image::from_fn(h, w, |x, y| self.get(y, h - 1 - x))
    }
    pub fn rotate_180(&self) -> Self {
        let mut d = self.data.clone();
        d.reverse();
        Self { width: self.width, height: self.height, data: d }
    }
    pub fn flip_h(&self) -> Self {
        let mut out = self.clone();
        for y in 0..self.height {
            out.row_mut(y).reverse();
        }
        out
    }
    /// Apply an EXIF orientation (stored → displayed).
    /// [`Self::oriented`] without a copy when `o` is the identity.
    pub fn into_oriented(self, o: lightcraft_geom::Orientation) -> Self {
        if o == lightcraft_geom::Orientation::Normal { self } else { self.oriented(o) }
    }

    pub fn oriented(&self, o: lightcraft_geom::Orientation) -> Self {
        let (flip, turns) = o.to_parts();
        let mut img = if flip { self.flip_h() } else { self.clone() };
        match turns {
            1 => img = img.rotate_cw(),
            2 => img = img.rotate_180(),
            3 => img = img.rotate_180().rotate_cw(),
            _ => {}
        }
        img
    }
}

impl Rgb32f {
    /// Bilinear sample at continuous pixel coordinates (pixel centres at +0.5).
    #[inline]
    pub fn sample_bilinear(&self, x: f32, y: f32) -> [f32; 3] {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = fx - x0;
        let ty = fy - y0;
        let (x0, y0) = (x0 as isize, y0 as isize);
        let a = self.get_clamped(x0, y0);
        let b = self.get_clamped(x0 + 1, y0);
        let c = self.get_clamped(x0, y0 + 1);
        let d = self.get_clamped(x0 + 1, y0 + 1);
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bot = c[i] + (d[i] - c[i]) * tx;
            top + (bot - top) * ty
        })
    }

    pub fn luminance(&self) -> Plane {
        self.map(lightcraft_color::luminance_2020)
    }

    /// Encode to 8-bit sRGB from linear sRGB values (no gamut conversion).
    pub fn to_srgb8(&self) -> Rgba8 {
        use lightcraft_color::transfer::encode_srgb8;
        self.map(|p| [encode_srgb8(p[0]), encode_srgb8(p[1]), encode_srgb8(p[2]), 255])
    }
}

impl Rgba8 {
    /// Raw bytes (RGBA, row-major).
    pub fn as_bytes(&self) -> Vec<u8> {
        self.data.iter().flat_map(|p| *p).collect()
    }
    pub fn from_bytes(width: usize, height: usize, bytes: &[u8]) -> Option<Self> {
        if bytes.len() != width * height * 4 {
            return None;
        }
        Some(Self { width, height, data: bytes.as_chunks::<4>().0.iter().map(|c| [c[0], c[1], c[2], c[3]]).collect() })
    }
    /// Linear Rec.709/sRGB primaries float image from 8-bit sRGB.
    pub fn to_linear(&self) -> Rgb32f {
        use lightcraft_color::transfer::decode_srgb8;
        self.map(|p| [decode_srgb8(p[0]), decode_srgb8(p[1]), decode_srgb8(p[2])])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_geom::Orientation;

    #[test]
    fn crop_and_rotate() {
        let img = Image::<u32>::from_fn(4, 3, |x, y| (y * 10 + x) as u32);
        let c = img.crop(1, 1, 2, 2);
        assert_eq!(c.data, vec![11, 12, 21, 22]);
        // the in-place variants equal the copying ones
        for (x, y, w, h) in [(1, 1, 2, 2), (0, 0, 4, 3), (2, 0, 9, 9), (0, 2, 4, 1), (3, 2, 1, 1)] {
            assert_eq!(img.clone().into_crop(x, y, w, h), img.crop(x, y, w, h));
        }
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            assert_eq!(img.clone().into_oriented(o), img.oriented(o));
        }
        let mut m = img.clone();
        m.map_in_place(|p| p * 2 + 1);
        assert_eq!(m, img.map(|p| p * 2 + 1));
        let r = img.rotate_cw();
        assert_eq!((r.width, r.height), (3, 4));
        // top-left of rotated = bottom-left of original
        assert_eq!(r.get(0, 0), 20);
        assert_eq!(r.get(2, 0), 0);
        assert_eq!(img.rotate_cw().rotate_cw().rotate_cw().rotate_cw(), img);
        assert_eq!(img.rotate_cw().rotate_cw(), img.rotate_180());
    }

    #[test]
    fn orientation_matches_geom_mapping() {
        let img = Image::<u32>::from_fn(5, 3, |x, y| (y * 10 + x) as u32);
        for v in 1..=8 {
            let o = Orientation::from_exif(v);
            let out = img.oriented(o);
            for (sx, sy) in [(0usize, 0usize), (4, 0), (2, 1), (0, 2)] {
                let (dx, dy) = o.map(sx as f64 + 0.5, sy as f64 + 0.5, 5.0, 3.0);
                assert_eq!(out.get(dx as usize, dy as usize), img.get(sx, sy), "orientation {v}");
            }
        }
    }

    #[test]
    fn bilinear_at_centres_is_exact() {
        let img = Rgb32f::from_fn(3, 3, |x, y| [x as f32, y as f32, 1.0]);
        assert_eq!(img.sample_bilinear(1.5, 2.5), [1.0, 2.0, 1.0]);
        let m = img.sample_bilinear(1.0, 1.5);
        assert!((m[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn srgb8_roundtrip() {
        let img = Rgba8::from_fn(16, 16, |x, y| [(x * 16) as u8, (y * 16) as u8, 7, 255]);
        assert_eq!(img.to_linear().to_srgb8(), img);
        assert_eq!(Rgba8::from_bytes(16, 16, &img.as_bytes()).unwrap(), img);
    }
}
