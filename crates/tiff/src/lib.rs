//! TIFF / IFD reading and writing for LightCraft.
//!
//! - [`Tiff::parse`] reads classic TIFF and BigTIFF in both byte orders, following the IFD chain,
//!   `SubIFDs`, and the Exif / GPS / Interoperability IFD pointers. All field types of TIFF 6.0,
//!   the TIFF 6.0 supplement (`IFD`) and BigTIFF (`LONG8`, `SLONG8`, `IFD8`) are decoded.
//! - [`parse_ifd_at`] parses a single IFD with an arbitrary offset base — the building block for
//!   maker-note IFDs ([`makernote`]).
//! - [`image::ImageInfo`] describes the pixel layout of an IFD (strips or tiles, planar config,
//!   compression, predictor) and enumerates its [`image::Chunk`]s.
//! - [`writer::TiffWriter`] writes IFD trees (with strips/tiles, `SubIFDs` and pointer IFDs) in
//!   either byte order, classic or BigTIFF.
//!
//! The reader never panics on malformed input: every offset is bounds-checked, IFD loops are
//! detected, nesting depth / IFD count are limited and the total amount of decoded value bytes is
//! capped relative to the input size (so a hostile file cannot make us allocate unboundedly).
//!
//! Sources: TIFF 6.0 specification (Adobe, 1992), TIFF Technical Note / supplements, the BigTIFF
//! design notes (AWare Systems), Adobe DNG specification 1.7 (tag numbers), CIPA DC-008 (Exif 2.32).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod image;
pub mod makernote;
mod reader;
pub mod tags;
pub mod writer;

pub use reader::{ParseOptions, parse_ifd_at};
pub use writer::{IfdBuilder, ImageData, TiffWriter};

use serde::{Deserialize, Serialize};

/// Errors from TIFF parsing / writing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TiffError {
    #[error("not a TIFF stream (bad header)")]
    NotTiff,
    #[error("data truncated at offset {0}")]
    Truncated(u64),
    #[error("invalid TIFF structure: {0}")]
    Invalid(String),
    #[error("resource limit exceeded: {0}")]
    Limit(&'static str),
    #[error("missing required tag {0}")]
    MissingTag(u16),
}

pub type Result<T> = std::result::Result<T, TiffError>;

/// Byte order of a TIFF stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ByteOrder {
    /// `II` — Intel, little-endian.
    #[default]
    Little,
    /// `MM` — Motorola, big-endian.
    Big,
}

impl ByteOrder {
    #[inline]
    pub fn u16(self, b: [u8; 2]) -> u16 {
        match self {
            ByteOrder::Little => u16::from_le_bytes(b),
            ByteOrder::Big => u16::from_be_bytes(b),
        }
    }
    #[inline]
    pub fn u32(self, b: [u8; 4]) -> u32 {
        match self {
            ByteOrder::Little => u32::from_le_bytes(b),
            ByteOrder::Big => u32::from_be_bytes(b),
        }
    }
    #[inline]
    pub fn u64(self, b: [u8; 8]) -> u64 {
        match self {
            ByteOrder::Little => u64::from_le_bytes(b),
            ByteOrder::Big => u64::from_be_bytes(b),
        }
    }
    /// Read a `u16` at `off`, or `None` if out of bounds.
    #[inline]
    pub fn read_u16(self, data: &[u8], off: usize) -> Option<u16> {
        data.get(off..off.checked_add(2)?).map(|s| self.u16([s[0], s[1]]))
    }
    #[inline]
    pub fn read_u32(self, data: &[u8], off: usize) -> Option<u32> {
        data.get(off..off.checked_add(4)?).map(|s| self.u32([s[0], s[1], s[2], s[3]]))
    }
    #[inline]
    pub fn read_u64(self, data: &[u8], off: usize) -> Option<u64> {
        data.get(off..off.checked_add(8)?).map(|s| self.u64([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }
    pub fn put_u16(self, out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        });
    }
    pub fn put_u32(self, out: &mut Vec<u8>, v: u32) {
        out.extend_from_slice(&match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        });
    }
    pub fn put_u64(self, out: &mut Vec<u8>, v: u64) {
        out.extend_from_slice(&match self {
            ByteOrder::Little => v.to_le_bytes(),
            ByteOrder::Big => v.to_be_bytes(),
        });
    }
}

/// TIFF field types (TIFF 6.0 §2 + supplements + BigTIFF).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u16)]
pub enum FieldType {
    Byte = 1,
    Ascii = 2,
    Short = 3,
    Long = 4,
    Rational = 5,
    SByte = 6,
    Undefined = 7,
    SShort = 8,
    SLong = 9,
    SRational = 10,
    Float = 11,
    Double = 12,
    Ifd = 13,
    Long8 = 16,
    SLong8 = 17,
    Ifd8 = 18,
}

impl FieldType {
    pub fn from_u16(v: u16) -> Option<FieldType> {
        use FieldType::*;
        Some(match v {
            1 => Byte,
            2 => Ascii,
            3 => Short,
            4 => Long,
            5 => Rational,
            6 => SByte,
            7 => Undefined,
            8 => SShort,
            9 => SLong,
            10 => SRational,
            11 => Float,
            12 => Double,
            13 => Ifd,
            16 => Long8,
            17 => SLong8,
            18 => Ifd8,
            _ => return None,
        })
    }
    /// Size in bytes of one value.
    pub fn size(self) -> usize {
        use FieldType::*;
        match self {
            Byte | Ascii | SByte | Undefined => 1,
            Short | SShort => 2,
            Long | SLong | Float | Ifd => 4,
            Rational | SRational | Double | Long8 | SLong8 | Ifd8 => 8,
        }
    }
}

/// A decoded field value. Arrays are stored in their native type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Byte(Vec<u8>),
    /// ASCII text (bytes up to the first NUL, decoded lossily as UTF-8).
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    Rational(Vec<(u32, u32)>),
    SByte(Vec<i8>),
    Undefined(Vec<u8>),
    SShort(Vec<i16>),
    SLong(Vec<i32>),
    SRational(Vec<(i32, i32)>),
    Float(Vec<f32>),
    Double(Vec<f64>),
    Ifd(Vec<u32>),
    Long8(Vec<u64>),
    SLong8(Vec<i64>),
    Ifd8(Vec<u64>),
}

impl Value {
    pub fn field_type(&self) -> FieldType {
        match self {
            Value::Byte(_) => FieldType::Byte,
            Value::Ascii(_) => FieldType::Ascii,
            Value::Short(_) => FieldType::Short,
            Value::Long(_) => FieldType::Long,
            Value::Rational(_) => FieldType::Rational,
            Value::SByte(_) => FieldType::SByte,
            Value::Undefined(_) => FieldType::Undefined,
            Value::SShort(_) => FieldType::SShort,
            Value::SLong(_) => FieldType::SLong,
            Value::SRational(_) => FieldType::SRational,
            Value::Float(_) => FieldType::Float,
            Value::Double(_) => FieldType::Double,
            Value::Ifd(_) => FieldType::Ifd,
            Value::Long8(_) => FieldType::Long8,
            Value::SLong8(_) => FieldType::SLong8,
            Value::Ifd8(_) => FieldType::Ifd8,
        }
    }

    /// Number of values (for ASCII: bytes including the terminating NUL).
    pub fn count(&self) -> usize {
        match self {
            Value::Byte(v) | Value::Undefined(v) => v.len(),
            Value::Ascii(s) => s.len() + 1,
            Value::Short(v) => v.len(),
            Value::Long(v) | Value::Ifd(v) => v.len(),
            Value::Rational(v) => v.len(),
            Value::SByte(v) => v.len(),
            Value::SShort(v) => v.len(),
            Value::SLong(v) => v.len(),
            Value::SRational(v) => v.len(),
            Value::Float(v) => v.len(),
            Value::Double(v) => v.len(),
            Value::Long8(v) | Value::Ifd8(v) => v.len(),
            Value::SLong8(v) => v.len(),
        }
    }

    /// Value `i` as `f64` (rationals divided; zero denominators give `None`). ASCII has no numbers.
    pub fn get_f64(&self, i: usize) -> Option<f64> {
        Some(match self {
            Value::Byte(v) | Value::Undefined(v) => *v.get(i)? as f64,
            Value::Ascii(_) => return None,
            Value::Short(v) => *v.get(i)? as f64,
            Value::Long(v) | Value::Ifd(v) => *v.get(i)? as f64,
            Value::Rational(v) => {
                let (n, d) = *v.get(i)?;
                if d == 0 {
                    return None;
                }
                n as f64 / d as f64
            }
            Value::SByte(v) => *v.get(i)? as f64,
            Value::SShort(v) => *v.get(i)? as f64,
            Value::SLong(v) => *v.get(i)? as f64,
            Value::SRational(v) => {
                let (n, d) = *v.get(i)?;
                if d == 0 {
                    return None;
                }
                n as f64 / d as f64
            }
            Value::Float(v) => *v.get(i)? as f64,
            Value::Double(v) => *v.get(i)?,
            Value::Long8(v) | Value::Ifd8(v) => *v.get(i)? as f64,
            Value::SLong8(v) => *v.get(i)? as f64,
        })
    }

    /// Value `i` as an unsigned integer (integer types only; negative values give `None`).
    pub fn get_u64(&self, i: usize) -> Option<u64> {
        match self {
            Value::Byte(v) | Value::Undefined(v) => v.get(i).map(|&x| x as u64),
            Value::Short(v) => v.get(i).map(|&x| x as u64),
            Value::Long(v) | Value::Ifd(v) => v.get(i).map(|&x| x as u64),
            Value::Long8(v) | Value::Ifd8(v) => v.get(i).copied(),
            Value::SByte(v) => v.get(i).and_then(|&x| u64::try_from(x).ok()),
            Value::SShort(v) => v.get(i).and_then(|&x| u64::try_from(x).ok()),
            Value::SLong(v) => v.get(i).and_then(|&x| u64::try_from(x).ok()),
            Value::SLong8(v) => v.get(i).and_then(|&x| u64::try_from(x).ok()),
            _ => None,
        }
    }

    /// Value `i` as a signed integer (integer types only).
    pub fn get_i64(&self, i: usize) -> Option<i64> {
        match self {
            Value::SByte(v) => v.get(i).map(|&x| x as i64),
            Value::SShort(v) => v.get(i).map(|&x| x as i64),
            Value::SLong(v) => v.get(i).map(|&x| x as i64),
            Value::SLong8(v) => v.get(i).copied(),
            _ => self.get_u64(i).and_then(|x| i64::try_from(x).ok()),
        }
    }

    /// All values as `f64` (missing/invalid entries skipped for rationals with zero denominators → 0).
    pub fn to_f64_vec(&self) -> Vec<f64> {
        (0..self.len_values()).map(|i| self.get_f64(i).unwrap_or(0.0)).collect()
    }

    /// All values as `u64` (integer types only; otherwise empty).
    pub fn to_u64_vec(&self) -> Vec<u64> {
        (0..self.len_values()).map_while(|i| self.get_u64(i)).collect()
    }

    fn len_values(&self) -> usize {
        match self {
            Value::Ascii(_) => 0,
            _ => self.count(),
        }
    }

    /// Text for ASCII values (trimmed of trailing NULs / spaces); `Byte`/`Undefined` are decoded as UTF-8 (lossy)
    /// up to the first NUL, which covers XMP packets and UCS-less Exif strings.
    pub fn as_str(&self) -> Option<String> {
        match self {
            Value::Ascii(s) => Some(s.trim_end_matches([' ', '\0']).to_string()),
            Value::Byte(b) | Value::Undefined(b) => {
                let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
                Some(String::from_utf8_lossy(&b[..end]).trim_end().to_string())
            }
            _ => None,
        }
    }

    /// Raw bytes for `Byte` / `Undefined` / `SByte` / `Ascii`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Byte(b) | Value::Undefined(b) => Some(b),
            Value::Ascii(s) => Some(s.as_bytes()),
            _ => None,
        }
    }
}

/// One IFD entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub tag: u16,
    pub value: Value,
    /// Absolute position in the parsed buffer of the value bytes (inline or out-of-line).
    pub offset: u64,
}

impl Entry {
    pub fn field_type(&self) -> FieldType {
        self.value.field_type()
    }
    pub fn count(&self) -> usize {
        self.value.count()
    }
}

/// An Image File Directory with its child IFDs already resolved.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ifd {
    /// Absolute offset of the IFD in the buffer.
    pub offset: u64,
    /// Entries sorted by tag (duplicate tags: the first occurrence wins).
    pub entries: Vec<Entry>,
    /// IFDs referenced by `SubIFDs` (330).
    pub sub_ifds: Vec<Ifd>,
    /// Exif private IFD (34665).
    pub exif: Option<Box<Ifd>>,
    /// GPS IFD (34853).
    pub gps: Option<Box<Ifd>>,
    /// Interoperability IFD (40965).
    pub interop: Option<Box<Ifd>>,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.binary_search_by_key(&tag, |e| e.tag).ok().map(|i| &self.entries[i])
    }
    pub fn contains(&self, tag: u16) -> bool {
        self.get(tag).is_some()
    }
    pub fn value(&self, tag: u16) -> Option<&Value> {
        self.get(tag).map(|e| &e.value)
    }
    /// First value as `u32` (integer types, saturating from wider types).
    pub fn u32(&self, tag: u16) -> Option<u32> {
        self.value(tag)?.get_u64(0).map(|v| v.min(u32::MAX as u64) as u32)
    }
    pub fn u64(&self, tag: u16) -> Option<u64> {
        self.value(tag)?.get_u64(0)
    }
    pub fn u16(&self, tag: u16) -> Option<u16> {
        self.value(tag)?.get_u64(0).map(|v| v.min(u16::MAX as u64) as u16)
    }
    pub fn i64(&self, tag: u16) -> Option<i64> {
        self.value(tag)?.get_i64(0)
    }
    pub fn f64(&self, tag: u16) -> Option<f64> {
        self.value(tag)?.get_f64(0)
    }
    pub fn f64s(&self, tag: u16) -> Option<Vec<f64>> {
        self.value(tag).map(|v| v.to_f64_vec())
    }
    pub fn u64s(&self, tag: u16) -> Option<Vec<u64>> {
        self.value(tag).map(|v| v.to_u64_vec())
    }
    /// Text value (ASCII, or UTF-8 bytes).
    pub fn string(&self, tag: u16) -> Option<String> {
        self.value(tag)?.as_str().filter(|s| !s.is_empty())
    }
    pub fn bytes(&self, tag: u16) -> Option<&[u8]> {
        self.value(tag)?.as_bytes()
    }

    /// This IFD and all descendants (sub-IFDs, Exif, GPS, Interop), depth-first, pre-order.
    pub fn walk(&self) -> Vec<&Ifd> {
        let mut out = vec![self];
        for s in &self.sub_ifds {
            out.extend(s.walk());
        }
        for c in [&self.exif, &self.gps, &self.interop].into_iter().flatten() {
            out.extend(c.walk());
        }
        out
    }

    /// Image layout of this IFD, if it describes an image.
    pub fn image(&self) -> Result<image::ImageInfo> {
        image::ImageInfo::from_ifd(self)
    }
}

/// A parsed TIFF stream (IFD chain with resolved children).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tiff {
    pub order: ByteOrder,
    pub bigtiff: bool,
    /// The main IFD chain (IFD0, IFD1, …).
    pub ifds: Vec<Ifd>,
}

impl Tiff {
    /// Parse a TIFF stream (the header must be at offset 0 of `data`).
    pub fn parse(data: &[u8]) -> Result<Tiff> {
        reader::parse(data, &ParseOptions::default())
    }
    pub fn parse_with(data: &[u8], opts: &ParseOptions) -> Result<Tiff> {
        reader::parse(data, opts)
    }
    /// Header check only: returns the byte order and BigTIFF flag.
    pub fn sniff(data: &[u8]) -> Option<(ByteOrder, bool)> {
        reader::header(data).ok().map(|(o, b, _)| (o, b))
    }
    /// Every IFD (main chain and all children), depth-first.
    pub fn all_ifds(&self) -> Vec<&Ifd> {
        self.ifds.iter().flat_map(|i| i.walk()).collect()
    }
    /// The Exif IFD attached to IFD0 (or any IFD).
    pub fn exif(&self) -> Option<&Ifd> {
        self.all_ifds().into_iter().find_map(|i| i.exif.as_deref())
    }
    pub fn gps(&self) -> Option<&Ifd> {
        self.all_ifds().into_iter().find_map(|i| i.gps.as_deref())
    }
    /// First entry for `tag` in IFD0, then the Exif IFD, then any IFD.
    pub fn find(&self, tag: u16) -> Option<&Entry> {
        if let Some(e) = self.ifds.first().and_then(|i| i.get(tag)) {
            return Some(e);
        }
        if let Some(e) = self.exif().and_then(|i| i.get(tag)) {
            return Some(e);
        }
        self.all_ifds().into_iter().find_map(|i| i.get(tag))
    }
}
