//! RAW decoding for LightCraft.
//!
//! - [`probe`] recognises raw containers; [`decode`] turns a file into a [`RawImage`] (sensor data + everything
//!   needed to render it: CFA, black/white levels, active area, default crop, orientation, DNG colour tags,
//!   opcode lists, [`Metadata`]); [`embedded_preview`] returns the largest embedded JPEG.
//! - [`RawImage::normalized`] subtracts black, scales white to 1.0 and crops to the active area (applying DNG
//!   `OpcodeList1`/`OpcodeList2`); [`demosaic`] turns CFA data into camera-RGB [`Rgb32f`];
//!   [`RawImage::develop`] does all of it plus `OpcodeList3` and the default crop; [`RawImage::develop_binned`]
//!   produces the same at 1/k of the size straight from the mosaic (previews, thumbnails).
//! - [`color`] implements the DNG colour model (dual-illuminant interpolation, forward matrices, white balance)
//!   and produces camera → linear Rec.2020 D65 matrices; [`profile`] reads and applies a DNG's own profile
//!   look tables and tone curve.
//!
//! Formats: DNG (uncompressed, lossless JPEG, lossy JPEG (Smart Previews), Deflate incl. floating point, tiled/stripped, CFA and LinearRaw),
//! Canon CR2, Nikon NEF/NRW (uncompressed, Huffman lossless / lossy compressed), Sony ARW (uncompressed, ARW2, lossless), Fujifilm RAF (uncompressed Bayer
//! and X-Trans), Panasonic RW2 (packed 12/14-bit), Pentax PEF (uncompressed, Huffman), Olympus ORF (uncompressed).
//! [`embedded_preview`] covers all of them plus CR3. Variants we can't decode yet (Nikon "lossy after split" NEF,
//! Panasonic quantised RW2, compressed ORF/RAF, CR3) return [`RawError::Unsupported`]; each vendor module documents its sources
//! (public specifications, tag-name documentation, black-box analysis of CC0 samples) and gaps. Non-DNG files carry no
//! colour matrix: [`color`] falls back to a documented neutral model. The decoders never panic on malformed input.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod binned;
pub mod color;
pub mod demosaic;
mod dng;
pub mod dngwrite;
pub mod highlight;
pub mod ljpeg;
pub mod opcodes;
mod preview;
pub mod profile;
mod tiffraw;
mod unpack;
mod vendor;

pub use demosaic::{Method, demosaic};
pub use dngwrite::{DngCompression, DngWriteOptions, write_dng};
pub use lightcraft_color::Mat3;
pub use lightcraft_geom::Orientation;
pub use lightcraft_meta::Metadata;
pub use lightcraft_raster::Rgb32f;
pub use opcodes::{Opcode, OpcodeLists};
pub use preview::embedded_preview;

use lightcraft_color::Xy;
use lightcraft_tiff::{Tiff, TiffError};
use serde::{Deserialize, Serialize};

/// Upper bound on decoded samples (guards allocations driven by header values).
pub const MAX_SAMPLES: usize = 1 << 30;

/// Errors from raw decoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RawError {
    #[error("not a recognised raw file")]
    NotRaw,
    #[error("unsupported raw variant: {0}")]
    Unsupported(String),
    #[error("corrupt raw data: {0}")]
    Corrupt(String),
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    #[error(transparent)]
    Tiff(#[from] TiffError),
}

pub type Result<T> = std::result::Result<T, RawError>;

/// Raw container formats recognised by [`probe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RawFormat {
    Dng,
    Cr2,
    Cr3,
    Nef,
    Nrw,
    Arw,
    Raf,
    Orf,
    Rw2,
    Pef,
    Srw,
    /// Another TIFF-based raw (3FR, IIQ, ERF, KDC, DCR, MOS, …).
    OtherTiff,
}

impl RawFormat {
    /// Whether [`decode`] supports this container (possibly not every compression inside it).
    pub fn is_supported(self) -> bool {
        matches!(
            self,
            RawFormat::Dng | RawFormat::Cr2 | RawFormat::Nef | RawFormat::Nrw | RawFormat::Arw | RawFormat::Raf | RawFormat::Rw2 | RawFormat::Pef
        )
    }
}

/// Recognise a raw file from its first bytes / TIFF structure.
pub fn probe(bytes: &[u8]) -> Option<RawFormat> {
    if bytes.len() >= 12 && &bytes[4..12] == b"ftypcrx " {
        return Some(RawFormat::Cr3);
    }
    if bytes.starts_with(b"FUJIFILMCCD-RAW") {
        return Some(RawFormat::Raf);
    }
    if bytes.starts_with(b"IIRO") || bytes.starts_with(b"IIRS") || bytes.starts_with(b"MMOR") {
        return Some(RawFormat::Orf);
    }
    if bytes.starts_with(b"IIU\0") {
        return Some(RawFormat::Rw2);
    }
    if bytes.len() >= 10 && bytes.starts_with(b"II*\0") && &bytes[8..10] == b"CR" {
        return Some(RawFormat::Cr2);
    }
    let (_, _) = Tiff::sniff(bytes)?;
    let opts = lightcraft_tiff::ParseOptions { max_ifds: 256, ..Default::default() };
    let t = Tiff::parse_with(bytes, &opts).ok()?;
    let ifd0 = t.ifds.first()?;
    if ifd0.contains(lightcraft_tiff::tags::DNG_VERSION) {
        return Some(RawFormat::Dng);
    }
    let make = t.find(lightcraft_tiff::tags::MAKE).and_then(|e| e.value.as_str()).unwrap_or_default().to_ascii_uppercase();
    let has_cfa = t.all_ifds().iter().any(|i| {
        i.u16(lightcraft_tiff::tags::PHOTOMETRIC) == Some(lightcraft_tiff::tags::photometric::CFA)
            || i.u16(lightcraft_tiff::tags::COMPRESSION).is_some_and(|c| c == 34713 || c == 32767 || c == 32769 || c == 32770)
    });
    if make.starts_with("CANON") && t.ifds.len() >= 4 && t.ifds[3].u16(lightcraft_tiff::tags::COMPRESSION) == Some(6) {
        return Some(RawFormat::Cr2);
    }
    if make.starts_with("NIKON") {
        return Some(if has_cfa || t.all_ifds().len() > 1 { RawFormat::Nef } else { RawFormat::Nrw });
    }
    if make.starts_with("SONY") {
        return Some(RawFormat::Arw);
    }
    if make.starts_with("PENTAX") || make.starts_with("RICOH") {
        return Some(RawFormat::Pef);
    }
    if make.starts_with("SAMSUNG") {
        return Some(RawFormat::Srw);
    }
    if has_cfa {
        return Some(RawFormat::OtherTiff);
    }
    None
}

/// Decode a raw file.
pub fn decode(bytes: &[u8]) -> Result<RawImage> {
    decode_with(bytes, Mode::Full)
}

/// Everything [`decode`] learns about a raw file except its samples: geometry, orientation, CFA,
/// colour data, as-shot white balance, opcode lists (embedded lens corrections), metadata — read
/// from the headers without decompressing the pixel data, for imports. Equal to
/// [`RawImage::info`] of the decoded image. Black and white levels are not included (some formats
/// measure them from the samples).
///
/// A few uncompressed vendor formats derive part of this from the samples themselves (Nikon
/// NEF: optically masked trailing columns; Olympus ORF: the CFA phase and bit depth; Pentax PEF
/// without crop tags: dark borders); for those the samples are read (unpacked, nothing to
/// decompress) and dropped.
pub fn probe_info(bytes: &[u8]) -> Result<RawInfo> {
    decode_with(bytes, Mode::Header).map(RawImage::into_info)
}

/// How much of a file a decoder reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Headers and samples.
    Full,
    /// Headers only: the returned image has no samples, and black/white levels measured from
    /// the samples are placeholders. Only [`RawImage::into_info`] may look at it.
    Header,
}

fn decode_with(bytes: &[u8], mode: Mode) -> Result<RawImage> {
    match probe(bytes).ok_or(RawError::NotRaw)? {
        RawFormat::Dng => dng::decode(bytes, mode),
        RawFormat::Cr2 => vendor::cr2::decode(bytes, mode),
        RawFormat::Nef | RawFormat::Nrw => vendor::nef::decode(bytes),
        RawFormat::Arw => vendor::arw::decode(bytes, mode),
        RawFormat::Raf => vendor::raf::decode(bytes, mode),
        RawFormat::Rw2 => vendor::rw2::decode(bytes, mode),
        RawFormat::Pef => vendor::pef::decode(bytes, mode),
        RawFormat::Orf => vendor::orf::decode(bytes),
        other => Err(RawError::Unsupported(format!("{other:?} files are not decoded yet"))),
    }
}

/// A raw file's description without its samples (see [`probe_info`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawInfo {
    pub format: RawFormat,
    /// Full sensor data dimensions (including masked borders).
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    pub cfa: Option<Cfa>,
    pub bits: u32,
    pub active_area: Rect,
    /// Default crop relative to the active area.
    pub crop: Rect,
    pub orientation: Orientation,
    pub color: ColorData,
    pub wb_multipliers: Option<[f32; 3]>,
    pub opcodes: OpcodeLists,
    pub metadata: Metadata,
}

impl RawInfo {
    /// Size of the developed image before orientation: the default crop, else the active area.
    pub fn developed_size(&self) -> (usize, usize) {
        let c = self.crop.clipped(self.active_area.width, self.active_area.height);
        if c.width > 1 && c.height > 1 { (c.width, c.height) } else { (self.active_area.width, self.active_area.height) }
    }
}

/// Sensor samples.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RawData {
    U16(Vec<u16>),
    /// Floating-point DNGs (already linear, typically white = 1.0).
    F32(Vec<f32>),
}

impl RawData {
    pub fn len(&self) -> usize {
        match self {
            RawData::U16(v) => v.len(),
            RawData::F32(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn get(&self, i: usize) -> f32 {
        match self {
            RawData::U16(v) => v[i] as f32,
            RawData::F32(v) => v[i],
        }
    }
}

/// Colour filter array pattern. Colour indices: 0 = red, 1 = green, 2 = blue.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Cfa {
    pub width: usize,
    pub height: usize,
    /// `width × height` colour indices, row-major, anchored at image pixel (0, 0).
    pub pattern: Vec<u8>,
}

impl Cfa {
    /// A 2×2 Bayer pattern from a string such as `"RGGB"`.
    pub fn bayer(s: &str) -> Option<Cfa> {
        let p: Vec<u8> = s
            .chars()
            .map(|c| {
                Some(match c {
                    'R' => 0,
                    'G' => 1,
                    'B' => 2,
                    _ => return None,
                })
            })
            .collect::<Option<_>>()?;
        (p.len() == 4).then_some(Cfa { width: 2, height: 2, pattern: p })
    }
    /// A 2×2 Bayer layout named in code (`"RGGB"`, `"BGGR"`, `"GRBG"`, `"GBRG"`). Only for literal
    /// names (tested below); anything else falls back to RGGB instead of failing.
    pub(crate) fn bayer_static(s: &'static str) -> Cfa {
        Cfa::bayer(s).unwrap_or(Cfa { width: 2, height: 2, pattern: vec![0, 1, 1, 2] })
    }
    /// The Fujifilm X-Trans 6×6 layout (as commonly documented), anchored at (0, 0).
    pub fn xtrans() -> Cfa {
        let rows = ["GGRGGB", "GGBGGR", "BRGRBG", "GGBGGR", "GGRGGB", "RBGBRG"];
        let pattern = rows
            .iter()
            .flat_map(|r| {
                r.chars().map(|c| match c {
                    'R' => 0,
                    'G' => 1,
                    _ => 2,
                })
            })
            .collect();
        Cfa { width: 6, height: 6, pattern }
    }
    #[inline]
    pub fn color_at(&self, x: usize, y: usize) -> u8 {
        self.pattern[(y % self.height) * self.width + (x % self.width)]
    }
    /// The same pattern anchored at `(dx, dy)` of the current anchor (e.g. after cropping to an active area).
    pub fn shifted(&self, dx: usize, dy: usize) -> Cfa {
        let pattern = (0..self.height).flat_map(|y| (0..self.width).map(move |x| (x, y))).map(|(x, y)| self.color_at(x + dx, y + dy)).collect();
        Cfa { width: self.width, height: self.height, pattern }
    }
    pub fn is_bayer(&self) -> bool {
        if self.width != 2 || self.height != 2 {
            return false;
        }
        let mut c = [0; 3];
        for &p in &self.pattern {
            if p > 2 {
                return false;
            }
            c[p as usize] += 1;
        }
        c == [1, 2, 1]
    }
    /// Pattern name like `"RGGB"` for 2×2 patterns.
    pub fn name(&self) -> String {
        self.pattern.iter().map(|&c| ['R', 'G', 'B', '?'][c.min(3) as usize]).collect()
    }
    fn valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.width <= 16 && self.height <= 16 && self.pattern.len() == self.width * self.height
    }
}

/// Integer rectangle (pixels).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Rect {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Rect {
        Rect { x, y, width, height }
    }
    /// Clip to a `w × h` area.
    pub fn clipped(self, w: usize, h: usize) -> Rect {
        let x = self.x.min(w);
        let y = self.y.min(h);
        Rect { x, y, width: self.width.min(w - x), height: self.height.min(h - y) }
    }
}

/// Black level model (DNG `BlackLevelRepeatDim`, `BlackLevel`, `BlackLevelDeltaH/V`). The repeat pattern and
/// the deltas are anchored at the active area's top-left corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlackLevel {
    pub repeat_rows: usize,
    pub repeat_cols: usize,
    /// `repeat_rows × repeat_cols × cpp` values.
    pub values: Vec<f32>,
    /// Per active-area column.
    pub delta_h: Vec<f32>,
    /// Per active-area row.
    pub delta_v: Vec<f32>,
}

impl Default for BlackLevel {
    fn default() -> Self {
        BlackLevel { repeat_rows: 1, repeat_cols: 1, values: vec![0.0], delta_h: vec![], delta_v: vec![] }
    }
}

impl BlackLevel {
    pub fn uniform(v: f32) -> BlackLevel {
        BlackLevel { values: vec![v], ..Default::default() }
    }
    /// Black level at active-area coordinates (x, y) for sample `s` of `cpp`.
    #[inline]
    pub fn at(&self, x: usize, y: usize, s: usize, cpp: usize) -> f32 {
        let (r, c) = (self.repeat_rows.max(1), self.repeat_cols.max(1));
        let idx = ((y % r) * c + (x % c)) * cpp + s;
        let base = self.values.get(idx).or_else(|| self.values.first()).copied().unwrap_or(0.0);
        base + self.delta_h.get(x).copied().unwrap_or(0.0) + self.delta_v.get(y).copied().unwrap_or(0.0)
    }
    /// Mean of the repeat-pattern values (for display / heuristics).
    pub fn mean(&self) -> f32 {
        if self.values.is_empty() { 0.0 } else { self.values.iter().sum::<f32>() / self.values.len() as f32 }
    }
}

/// DNG colour tags (DNG 1.7 chapter 6). Matrices are 3×3 (three-colour cameras).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ColorData {
    /// `CalibrationIlluminant1/2` (Exif LightSource codes; 0 = unknown).
    pub illuminant: [u16; 2],
    /// `ColorMatrix1/2`: XYZ → reference camera.
    pub color_matrix: [Option<Mat3>; 2],
    /// `ForwardMatrix1/2`: white-balanced camera → XYZ D50.
    pub forward_matrix: [Option<Mat3>; 2],
    /// `CameraCalibration1/2`.
    pub camera_calibration: [Option<Mat3>; 2],
    pub analog_balance: Option<[f64; 3]>,
    pub as_shot_neutral: Option<[f64; 3]>,
    pub as_shot_white_xy: Option<Xy>,
    /// EV to add for a "normal" rendering (`BaselineExposure` + `BaselineExposureOffset`).
    pub baseline_exposure: f64,
    /// The file's own camera-profile look (`ProfileHueSatMap*`, `ProfileLookTable*`,
    /// `ProfileToneCurve`), applied by [`color`]'s users at render time.
    #[serde(default)]
    pub profile: profile::ProfileLook,
}

/// A decoded raw image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawImage {
    pub format: RawFormat,
    /// Full sensor data dimensions (including masked borders).
    pub width: usize,
    pub height: usize,
    /// Samples per pixel: 1 = CFA or monochrome, 3 = linear RGB (LinearRaw / demosaiced DNG).
    pub cpp: usize,
    pub data: RawData,
    /// CFA pattern anchored at pixel (0, 0) of the full data; `None` for linear / monochrome data.
    pub cfa: Option<Cfa>,
    /// Significant bits of the stored samples (informational).
    pub bits: u32,
    pub black: BlackLevel,
    /// White (clip) level per sample (after linearization).
    pub white: Vec<f32>,
    /// Area of valid image data within the full data.
    pub active_area: Rect,
    /// Default crop relative to the active area.
    pub crop: Rect,
    pub orientation: Orientation,
    pub color: ColorData,
    /// As-shot white-balance multipliers from vendor maker notes (green = 1), when known (non-DNG).
    pub wb_multipliers: Option<[f32; 3]>,
    /// Whether a `LinearizationTable` was applied to `data`.
    pub linearized: bool,
    pub opcodes: OpcodeLists,
    pub metadata: Metadata,
}

/// Black-subtracted, white-normalised data covering the active area.
#[derive(Clone, Debug, PartialEq)]
pub struct Normalized {
    pub width: usize,
    pub height: usize,
    pub cpp: usize,
    /// Nominal range [0, 1] (values may exceed 1 above white or be negative from noise).
    pub data: Vec<f32>,
    /// CFA anchored at (0, 0) of this buffer.
    pub cfa: Option<Cfa>,
}

impl RawImage {
    /// The description without the samples (what [`probe_info`] returns for the same file).
    pub fn info(&self) -> RawInfo {
        RawInfo {
            format: self.format,
            width: self.width,
            height: self.height,
            cpp: self.cpp,
            cfa: self.cfa.clone(),
            bits: self.bits,
            active_area: self.active_area,
            crop: self.crop,
            orientation: self.orientation,
            color: self.color.clone(),
            wb_multipliers: self.wb_multipliers,
            opcodes: self.opcodes.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// [`Self::info`], dropping the samples.
    pub fn into_info(self) -> RawInfo {
        RawInfo {
            format: self.format,
            width: self.width,
            height: self.height,
            cpp: self.cpp,
            cfa: self.cfa,
            bits: self.bits,
            active_area: self.active_area,
            crop: self.crop,
            orientation: self.orientation,
            color: self.color,
            wb_multipliers: self.wb_multipliers,
            opcodes: self.opcodes,
            metadata: self.metadata,
        }
    }

    /// Validate what a decoder read in `mode` ([`Mode::Header`]: everything but the samples).
    pub(crate) fn validate_for(&self, mode: Mode) -> Result<()> {
        self.check(mode == Mode::Full)
    }

    /// Validate internal consistency (dimensions vs data length, CFA shape, rectangles).
    pub fn validate(&self) -> Result<()> {
        self.check(true)
    }

    fn check(&self, samples: bool) -> Result<()> {
        if self.width == 0 || self.height == 0 || !(1..=4).contains(&self.cpp) {
            return Err(RawError::Corrupt("bad dimensions".into()));
        }
        if samples && self.data.len() != self.width * self.height * self.cpp {
            return Err(RawError::Corrupt("data length mismatch".into()));
        }
        if let Some(c) = &self.cfa
            && (!c.valid() || c.pattern.iter().any(|&p| p > 2))
        {
            return Err(RawError::Unsupported(format!("CFA pattern {}", c.name())));
        }
        let a = self.active_area;
        if a.width == 0 || a.height == 0 || a.x + a.width > self.width || a.y + a.height > self.height {
            return Err(RawError::Corrupt("active area outside image".into()));
        }
        Ok(())
    }

    /// White level for sample `s`.
    pub fn white_at(&self, s: usize) -> f32 {
        self.white.get(s).or_else(|| self.white.first()).copied().unwrap_or(65535.0)
    }

    /// Black-subtract, white-scale and crop to the active area, applying `OpcodeList1` (on raw values) first and
    /// `OpcodeList2` (on normalised values) after. 1.0 = white level.
    pub fn normalized(&self) -> Result<Normalized> {
        self.validate()?;
        let (w, cpp) = (self.width, self.cpp);
        let a = self.active_area;
        // stage 1 (full image) opcodes need a mutable float copy only when present
        let stage1: Option<Vec<f32>> = if self.opcodes.list1.is_empty() {
            None
        } else {
            let mut v: Vec<f32> = (0..self.data.len()).map(|i| self.data.get(i)).collect();
            opcodes::apply_list(&self.opcodes.list1, &mut v, self.width, self.height, cpp, self.cfa.as_ref(), 65535.0);
            Some(v)
        };
        let src = |i: usize| match &stage1 {
            Some(v) => v[i],
            None => self.data.get(i),
        };
        let scale: Vec<f32> = (0..cpp)
            .map(|s| {
                let range = self.white_at(s) - self.black.mean();
                if range > 0.0 { 1.0 / range } else { 1.0 }
            })
            .collect();
        let mut out = vec![0f32; a.width * a.height * cpp];
        let row_len = a.width * cpp;
        use rayon::prelude::*;
        out.par_chunks_mut(row_len).enumerate().for_each(|(y, row)| {
            let base = ((a.y + y) * w + a.x) * cpp;
            for x in 0..a.width {
                for s in 0..cpp {
                    let v = src(base + x * cpp + s);
                    let b = self.black.at(x, y, s, cpp);
                    row[x * cpp + s] = (v - b) * scale[s];
                }
            }
        });
        let cfa = self.cfa.as_ref().map(|c| c.shifted(a.x, a.y));
        opcodes::apply_list(&self.opcodes.list2, &mut out, a.width, a.height, cpp, cfa.as_ref(), 1.0);
        Ok(Normalized { width: a.width, height: a.height, cpp, data: out, cfa })
    }

    /// Full "raw → camera RGB" path: normalise, demosaic with `method`, apply `OpcodeList3`, then the default crop.
    /// The result is camera RGB (not white balanced), white level = 1.0, not oriented.
    pub fn develop(&self, method: Method) -> Result<Rgb32f> {
        let n = self.normalized()?;
        let mut rgb = demosaic(&n, method);
        drop(n);
        opcodes::apply_list3(&self.opcodes.list3, &mut rgb);
        let c = self.crop.clipped(rgb.width, rgb.height);
        if c.width == 0 || c.height == 0 || (c.x == 0 && c.y == 0 && c.width == rgb.width && c.height == rgb.height) {
            return Ok(rgb);
        }
        Ok(rgb.into_crop(c.x, c.y, c.width, c.height))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfa_helpers() {
        let c = Cfa::bayer("RGGB").unwrap();
        assert!(c.is_bayer());
        assert_eq!(c.color_at(0, 0), 0);
        assert_eq!(c.color_at(3, 3), 2);
        assert_eq!(c.shifted(1, 0).name(), "GRBG");
        assert_eq!(c.shifted(1, 1).name(), "BGGR");
        assert!(Cfa::bayer("RGGX").is_none());
        for name in ["RGGB", "BGGR", "GRBG", "GBRG"] {
            assert_eq!(Cfa::bayer_static(name).name(), name);
        }
        let x = Cfa::xtrans();
        assert!(!x.is_bayer());
        let greens = x.pattern.iter().filter(|&&p| p == 1).count();
        assert_eq!(greens, 20);
        // every 3×3 block of the X-Trans layout has 5 greens
        for by in 0..2 {
            for bx in 0..2 {
                let g = (0..9).filter(|i| x.color_at(bx * 3 + i % 3, by * 3 + i / 3) == 1).count();
                assert_eq!(g, 5);
            }
        }
    }

    #[test]
    fn black_levels() {
        let b = BlackLevel { repeat_rows: 2, repeat_cols: 2, values: vec![1.0, 2.0, 3.0, 4.0], delta_h: vec![0.5, 0.0], delta_v: vec![0.0, 10.0] };
        assert_eq!(b.at(0, 0, 0, 1), 1.5);
        assert_eq!(b.at(1, 1, 0, 1), 14.0);
        assert_eq!(b.at(3, 2, 0, 1), 2.0);
        assert_eq!(b.mean(), 2.5);
        assert_eq!(BlackLevel::uniform(7.0).at(9, 9, 0, 3), 7.0);
    }

    #[test]
    fn probe_rejects_junk() {
        assert_eq!(probe(b""), None);
        assert_eq!(probe(b"hello world, not a raw"), None);
        assert_eq!(probe(b"\0\0\0\x18ftypcrx \0\0\0\x01"), Some(RawFormat::Cr3));
        assert_eq!(probe(b"FUJIFILMCCD-RAW 0201"), Some(RawFormat::Raf));
        assert!(decode(b"FUJIFILMCCD-RAW 0201").is_err());
        assert_eq!(decode(b"junk"), Err(RawError::NotRaw));
    }
}
