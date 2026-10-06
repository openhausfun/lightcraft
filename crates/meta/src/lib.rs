//! Photo metadata for LightCraft: EXIF (TIFF/raw files, JPEG APP1, PNG `eXIf`, WebP `EXIF`), XMP packets
//! (read + write, standard namespaces plus our own `lc:` namespace) and basic IPTC-IIM.
//!
//! Entry points:
//! - [`read_exif`] — a TIFF-structured Exif block → [`Metadata`]; [`from_tiff`] for an already parsed stream.
//! - [`embedded`] / [`jpeg_segments`] / [`png_chunks`] / [`webp_chunks`] — locate Exif / XMP / ICC / IPTC blocks.
//! - [`parse_xmp`] / [`write_xmp`] — XMP packets (interchange fields + the opaque `lc:settings` JSON).
//! - [`parse_iptc`] — IPTC-IIM record 2 datasets.
//! - [`parse_gpx`] — GPS track logs (GPX) for geotagging by capture time.
//! - [`extract`] — all of the above for a whole file (JPEG, PNG, WebP, TIFF/DNG/raw).
#![forbid(unsafe_code)]

mod container;
mod datetime;
mod exif;
mod gpx;
mod iptc;
pub mod tags;
mod xmp;
mod xmp_merge;

pub use container::{Embedded, embedded, jpeg_segments, png_chunks, webp_chunks};
pub use datetime::DateTime;
pub use exif::{from_tiff, read_exif, strip_exif_header, try_read_exif, write_exif};
pub use gpx::{GpxError, Match, TrackPoint, Tracklog, parse_gpx};
pub use iptc::parse_iptc;
pub use lightcraft_geom::Orientation;
pub use tags::{TagRow, file_tag_rows, tag_rows};
pub use xmp::{CRS_NS, LC_NS, XmpData, XmpError, XmpValue, parse_xmp, write_xmp, write_xmp_lc};
pub use xmp_merge::{MergeRules, merge_xmp};

use serde::{Deserialize, Serialize};

/// GPS position (WGS 84). Latitude/longitude in signed decimal degrees (north / east positive).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gps {
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level (negative below).
    pub altitude: Option<f64>,
}

/// Exif `Flash` value decoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flash {
    pub fired: bool,
    /// The raw Exif `Flash` bit field.
    pub raw: u16,
}

/// Everything LightCraft shows or searches about a photo. All fields are optional; unknown = `None`/empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    // camera
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub software: Option<String>,
    // lens
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub lens_serial_number: Option<String>,
    /// Min/max focal length (mm) and min/max f-number at those, as in Exif `LensSpecification`.
    pub lens_spec: Option<[f64; 4]>,
    // capture
    pub capture_time: Option<DateTime>,
    /// Seconds.
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    /// Millimetres.
    pub focal_length: Option<f64>,
    pub focal_length_35mm: Option<f64>,
    pub flash: Option<Flash>,
    /// EV.
    pub exposure_bias: Option<f64>,
    /// Exif `ExposureProgram` code.
    pub exposure_program: Option<u16>,
    /// Exif `MeteringMode` code.
    pub metering_mode: Option<u16>,
    /// Exif `WhiteBalance` (0 auto, 1 manual).
    pub white_balance: Option<u16>,
    pub orientation: Option<Orientation>,
    pub gps: Option<Gps>,
    /// Pixel dimensions as recorded (not oriented).
    pub width: Option<u32>,
    pub height: Option<u32>,
    // descriptive / user
    pub artist: Option<String>,
    pub copyright: Option<String>,
    /// Copyright status (`xmpRights:Marked`): `true` = copyrighted, `false` = public domain, `None` = unknown.
    pub copyright_marked: Option<bool>,
    /// Rights usage terms (`xmpRights:UsageTerms`).
    pub usage_terms: Option<String>,
    /// Copyright info URL (`xmpRights:WebStatement`).
    pub copyright_url: Option<String>,
    pub title: Option<String>,
    pub caption: Option<String>,
    /// Accessibility text (`Iptc4xmpCore:AltTextAccessibility`).
    pub alt_text: Option<String>,
    /// Long accessibility description (`Iptc4xmpCore:ExtDescrAccessibility`).
    pub extended_description: Option<String>,
    /// Place within the city (`Iptc4xmpCore:Location`).
    pub sublocation: Option<String>,
    /// `photoshop:City`, `photoshop:State`, `photoshop:Country`.
    pub city: Option<String>,
    pub state: Option<String>,
    pub country: Option<String>,
    pub keywords: Vec<String>,
    /// Hierarchical keywords, `|`-separated paths (`lr:hierarchicalSubject` convention).
    pub hierarchical_keywords: Vec<String>,
    /// -1 = rejected, 0 = unrated, 1..=5 stars.
    pub rating: Option<i8>,
    /// Colour label name (e.g. "Red").
    pub label: Option<String>,
}

macro_rules! fill {
    ($self:ident, $o:ident, $($f:ident),*) => { $( if $self.$f.is_none() { $self.$f = $o.$f.clone(); } )* };
}
macro_rules! overlay {
    ($self:ident, $o:ident, $($f:ident),*) => { $( if $o.$f.is_some() { $self.$f = $o.$f.clone(); } )* };
}

impl Metadata {
    /// Fill every empty field of `self` from `other`.
    pub fn fill_missing(&mut self, other: &Metadata) {
        fill!(
            self,
            other,
            make,
            model,
            serial_number,
            software,
            lens_make,
            lens_model,
            lens_serial_number,
            lens_spec,
            capture_time,
            exposure_time,
            f_number,
            iso,
            focal_length,
            focal_length_35mm,
            flash,
            exposure_bias,
            exposure_program,
            metering_mode,
            white_balance,
            orientation,
            gps,
            width,
            height,
            artist,
            copyright,
            copyright_marked,
            usage_terms,
            copyright_url,
            title,
            caption,
            alt_text,
            extended_description,
            sublocation,
            city,
            state,
            country,
            rating,
            label
        );
        if self.keywords.is_empty() {
            self.keywords = other.keywords.clone();
        }
        if self.hierarchical_keywords.is_empty() {
            self.hierarchical_keywords = other.hierarchical_keywords.clone();
        }
    }

    /// Replace the user-editable fields (rating, label, title, caption, artist, copyright, keywords, GPS) with
    /// `other`'s where `other` has them — the XMP-over-EXIF precedence rule.
    pub fn overlay_user_fields(&mut self, other: &Metadata) {
        overlay!(self, other, rating, label, title, caption, artist, copyright, copyright_marked, usage_terms, copyright_url, gps);
        if !other.keywords.is_empty() {
            self.keywords = other.keywords.clone();
        }
        if !other.hierarchical_keywords.is_empty() {
            self.hierarchical_keywords = other.hierarchical_keywords.clone();
        }
    }

    /// Exposure time formatted like cameras show it: `1/250`, `0.5`, `2″`.
    pub fn exposure_display(&self) -> Option<String> {
        let t = self.exposure_time?;
        if t.is_nan() || t <= 0.0 {
            return None;
        }
        Some(if t < 0.3 {
            format!("1/{}", (1.0 / t).round())
        } else if t < 1.0 {
            format!("{}", (t * 10.0).round() / 10.0)
        } else {
            format!("{}″", (t * 10.0).round() / 10.0)
        })
    }

    /// Human name of the metering mode.
    pub fn metering_name(&self) -> Option<&'static str> {
        Some(match self.metering_mode? {
            1 => "Average",
            2 => "Center-weighted average",
            3 => "Spot",
            4 => "Multi-spot",
            5 => "Pattern",
            6 => "Partial",
            255 => "Other",
            _ => "Unknown",
        })
    }

    /// Human name of the exposure program.
    pub fn exposure_program_name(&self) -> Option<&'static str> {
        Some(match self.exposure_program? {
            1 => "Manual",
            2 => "Program AE",
            3 => "Aperture-priority AE",
            4 => "Shutter speed priority AE",
            5 => "Creative (slow speed)",
            6 => "Action (high speed)",
            7 => "Portrait",
            8 => "Landscape",
            9 => "Bulb",
            _ => "Not defined",
        })
    }
}

/// Read everything we understand from a whole file (JPEG, PNG, WebP, TIFF-based incl. DNG/raw).
/// Precedence: EXIF, then IPTC fills gaps, then XMP overrides user fields and fills the rest.
pub fn extract(bytes: &[u8]) -> Metadata {
    let e = embedded(bytes);
    let mut m = e.exif.as_deref().map(read_exif).unwrap_or_default();
    if let Some(i) = e.iptc.as_deref() {
        m.fill_missing(&parse_iptc(i));
    }
    if let Some(x) = e.xmp.as_deref().and_then(|x| parse_xmp(x).ok()) {
        m.overlay_user_fields(&x.metadata);
        m.fill_missing(&x.metadata);
    }
    m
}

/// Lenient number parsing used by XMP/IPTC: `"28/10"`, `"2.8"`, `"+0.7"`.
pub(crate) fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((n, d)) = s.split_once('/') {
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        return (d != 0.0).then_some(n / d).filter(|v| v.is_finite());
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_number("28/10"), Some(2.8));
        assert_eq!(parse_number(" +0.7 "), Some(0.7));
        assert_eq!(parse_number("1/0"), None);
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("inf"), None);
    }

    #[test]
    fn exposure_display() {
        let mut m = Metadata { exposure_time: Some(1.0 / 250.0), ..Default::default() };
        assert_eq!(m.exposure_display().as_deref(), Some("1/250"));
        m.exposure_time = Some(0.5);
        assert_eq!(m.exposure_display().as_deref(), Some("0.5"));
        m.exposure_time = Some(2.0);
        assert_eq!(m.exposure_display().as_deref(), Some("2″"));
        m.exposure_time = Some(0.0);
        assert_eq!(m.exposure_display(), None);
    }

    #[test]
    fn merge_rules() {
        let mut a = Metadata { make: Some("A".into()), rating: Some(1), ..Default::default() };
        let b = Metadata { make: Some("B".into()), model: Some("M".into()), rating: Some(4), keywords: vec!["k".into()], ..Default::default() };
        a.fill_missing(&b);
        assert_eq!(a.make.as_deref(), Some("A"));
        assert_eq!(a.model.as_deref(), Some("M"));
        assert_eq!(a.rating, Some(1));
        a.overlay_user_fields(&b);
        assert_eq!(a.rating, Some(4));
        assert_eq!(a.make.as_deref(), Some("A"));
        assert_eq!(a.keywords, vec!["k".to_string()]);
    }

    #[test]
    fn names() {
        let m = Metadata { metering_mode: Some(5), exposure_program: Some(3), ..Default::default() };
        assert_eq!(m.metering_name(), Some("Pattern"));
        assert_eq!(m.exposure_program_name(), Some("Aperture-priority AE"));
    }
}
