//! Every EXIF / TIFF / GPS tag of a file as readable `(group, name, value)` rows (the Info
//! panel's "All Metadata"). Names follow the public TIFF 6.0 / EXIF 2.3 / DNG specifications;
//! common enumerations are spelled out, everything else is shown as stored. Maker notes and
//! binary blobs are summarised by size.

use lightcraft_tiff::{Ifd, Tiff, Value};

/// One metadata row.
#[derive(Clone, Debug, PartialEq)]
pub struct TagRow {
    /// `TIFF`, `EXIF`, `GPS`, `Interop`.
    pub group: &'static str,
    pub tag: u16,
    pub name: String,
    pub value: String,
}

const NAMES: &[(u16, &str)] = &[
    // TIFF / IFD0
    (0x00fe, "Subfile Type"),
    (0x0100, "Image Width"),
    (0x0101, "Image Height"),
    (0x0102, "Bits Per Sample"),
    (0x0103, "Compression"),
    (0x0106, "Photometric Interpretation"),
    (0x010e, "Image Description"),
    (0x010f, "Make"),
    (0x0110, "Model"),
    (0x0112, "Orientation"),
    (0x0115, "Samples Per Pixel"),
    (0x0116, "Rows Per Strip"),
    (0x0214, "Reference Black/White"),
    (0x9216, "TIFF/EP Standard ID"),
    (0x828d, "CFA Repeat Pattern Dim"),
    (0x828e, "CFA Pattern"),
    (0xa302, "CFA Pattern"),
    (0x9217, "Sensing Method"),
    (0xc4a5, "Print Image Matching"),
    (0xa500, "Gamma"),
    (0x011a, "X Resolution"),
    (0x011b, "Y Resolution"),
    (0x011c, "Planar Configuration"),
    (0x0128, "Resolution Unit"),
    (0x0131, "Software"),
    (0x0132, "Date/Time Modified"),
    (0x013b, "Artist"),
    (0x013e, "White Point"),
    (0x013f, "Primary Chromaticities"),
    (0x0213, "YCbCr Positioning"),
    (0x8298, "Copyright"),
    (0xc612, "DNG Version"),
    (0xc614, "Unique Camera Model"),
    (0xc62f, "Camera Serial Number"),
    // EXIF
    (0x829a, "Exposure Time"),
    (0x829d, "F-Number"),
    (0x8822, "Exposure Program"),
    (0x8824, "Spectral Sensitivity"),
    (0x8827, "ISO Speed"),
    (0x8830, "Sensitivity Type"),
    (0x8832, "Recommended Exposure Index"),
    (0x9000, "Exif Version"),
    (0x9003, "Date/Time Original"),
    (0x9004, "Date/Time Digitized"),
    (0x9010, "Offset Time"),
    (0x9011, "Offset Time Original"),
    (0x9012, "Offset Time Digitized"),
    (0x9101, "Components Configuration"),
    (0x9102, "Compressed Bits Per Pixel"),
    (0x9201, "Shutter Speed Value"),
    (0x9202, "Aperture Value"),
    (0x9203, "Brightness Value"),
    (0x9204, "Exposure Compensation"),
    (0x9205, "Max Aperture Value"),
    (0x9206, "Subject Distance"),
    (0x9207, "Metering Mode"),
    (0x9208, "Light Source"),
    (0x9209, "Flash"),
    (0x920a, "Focal Length"),
    (0x9214, "Subject Area"),
    (0x927c, "Maker Note"),
    (0x9286, "User Comment"),
    (0x9290, "Sub-second Time"),
    (0x9291, "Sub-second Time Original"),
    (0x9292, "Sub-second Time Digitized"),
    (0xa000, "FlashPix Version"),
    (0xa001, "Color Space"),
    (0xa002, "Pixel X Dimension"),
    (0xa003, "Pixel Y Dimension"),
    (0xa20e, "Focal Plane X Resolution"),
    (0xa20f, "Focal Plane Y Resolution"),
    (0xa210, "Focal Plane Resolution Unit"),
    (0xa215, "Exposure Index"),
    (0xa217, "Sensing Method"),
    (0xa300, "File Source"),
    (0xa301, "Scene Type"),
    (0xa401, "Custom Rendered"),
    (0xa402, "Exposure Mode"),
    (0xa403, "White Balance"),
    (0xa404, "Digital Zoom Ratio"),
    (0xa405, "Focal Length in 35mm Film"),
    (0xa406, "Scene Capture Type"),
    (0xa407, "Gain Control"),
    (0xa408, "Contrast"),
    (0xa409, "Saturation"),
    (0xa40a, "Sharpness"),
    (0xa40c, "Subject Distance Range"),
    (0xa420, "Image Unique ID"),
    (0xa430, "Camera Owner Name"),
    (0xa431, "Body Serial Number"),
    (0xa432, "Lens Specification"),
    (0xa433, "Lens Make"),
    (0xa434, "Lens Model"),
    (0xa435, "Lens Serial Number"),
    // GPS
    (0x0000, "GPS Version"),
    (0x0001, "GPS Latitude Ref"),
    (0x0002, "GPS Latitude"),
    (0x0003, "GPS Longitude Ref"),
    (0x0004, "GPS Longitude"),
    (0x0005, "GPS Altitude Ref"),
    (0x0006, "GPS Altitude"),
    (0x0007, "GPS Time Stamp"),
    (0x000c, "GPS Speed Ref"),
    (0x000d, "GPS Speed"),
    (0x0010, "GPS Img Direction Ref"),
    (0x0011, "GPS Img Direction"),
    (0x0012, "GPS Map Datum"),
    (0x001d, "GPS Date Stamp"),
];

/// GPS tags reuse low numbers, so they are looked up separately.
fn name(group: &str, tag: u16) -> String {
    let gps = group == "GPS";
    NAMES
        .iter()
        .find(|(t, n)| *t == tag && (n.starts_with("GPS") == gps || (gps && tag > 0x1f)))
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| format!("Tag 0x{tag:04X}"))
}

fn rational(n: f64, d: f64) -> String {
    if d == 0.0 {
        return "—".into();
    }
    let v = n / d;
    if (v - v.round()).abs() < 1e-9 { format!("{}", v.round()) } else { format!("{:.4}", v).trim_end_matches('0').to_string() }
}

fn list<T: ToString>(v: &[T]) -> String {
    let shown: Vec<String> = v.iter().take(16).map(|x| x.to_string()).collect();
    if v.len() > 16 { format!("{} … ({} values)", shown.join(", "), v.len()) } else { shown.join(", ") }
}

/// Spelled-out enumerations for the commonest tags.
fn enumerated(group: &str, tag: u16, v: i64) -> Option<&'static str> {
    if group == "GPS" {
        return None;
    }
    Some(match (tag, v) {
        (0x0112, 1) => "Horizontal (normal)",
        (0x0112, 3) => "Rotated 180°",
        (0x0112, 6) => "Rotated 90° CW",
        (0x0112, 8) => "Rotated 90° CCW",
        (0x0112, 2) => "Mirrored horizontally",
        (0x0112, 4) => "Mirrored vertically",
        (0x0103, 1) => "Uncompressed",
        (0x0103, 6) => "JPEG (old-style)",
        (0x0103, 7) => "JPEG",
        (0x0103, 8) => "Deflate",
        (0x0103, 32773) => "PackBits",
        (0x0103, 34892) => "Lossy JPEG",
        (0x0106, 0) => "WhiteIsZero",
        (0x0106, 1) => "BlackIsZero",
        (0x0106, 2) => "RGB",
        (0x0106, 6) => "YCbCr",
        (0x0106, 32803) => "Color Filter Array",
        (0x0106, 34892) => "Linear Raw",
        (0x011c, 1) => "Chunky",
        (0x011c, 2) => "Planar",
        (0x0128, 2) => "inches",
        (0x0128, 3) => "centimetres",
        (0x8822, 1) => "Manual",
        (0x8822, 2) => "Program",
        (0x8822, 3) => "Aperture priority",
        (0x8822, 4) => "Shutter priority",
        (0x9207, 2) => "Center-weighted average",
        (0x9207, 3) => "Spot",
        (0x9207, 5) => "Pattern",
        (0xa001, 1) => "sRGB",
        (0xa001, 0xffff) => "Uncalibrated",
        (0xa402, 0) => "Auto",
        (0xa402, 1) => "Manual",
        (0xa402, 2) => "Auto bracket",
        (0xa403, 0) => "Auto",
        (0xa403, 1) => "Manual",
        (0xa406, 0) => "Standard",
        (0xa406, 1) => "Landscape",
        (0xa406, 2) => "Portrait",
        (0xa406, 3) => "Night",
        (0x9209, 0) => "No flash",
        (0x9209, 1) => "Fired",
        (0x9209, 5) => "Fired, return not detected",
        (0x9209, 7) => "Fired, return detected",
        (0x9209, 9) => "On, fired",
        (0x9209, 16) => "Off, did not fire",
        (0x9209, 24) => "Auto, did not fire",
        (0x9209, 25) => "Auto, fired",
        (0x9209, 32) => "No flash function",
        (0x9208, 0) => "Unknown",
        (0x9208, 1) => "Daylight",
        (0x9208, 2) => "Fluorescent",
        (0x9208, 3) => "Tungsten",
        (0x9208, 4) => "Flash",
        _ => return None,
    })
}

fn value_text(group: &str, tag: u16, v: &Value) -> String {
    let single = |x: i64| enumerated(group, tag, x).map(str::to_string).unwrap_or_else(|| x.to_string());
    match v {
        Value::Ascii(s) => s.trim().to_string(),
        Value::Short(a) if a.len() == 1 => single(a[0] as i64),
        Value::Long(a) if a.len() == 1 => single(a[0] as i64),
        Value::Short(a) => list(a),
        Value::Long(a) => list(a),
        Value::SShort(a) => list(a),
        Value::SLong(a) => list(a),
        Value::Long8(a) => list(a),
        Value::SLong8(a) => list(a),
        Value::Float(a) => list(a),
        Value::Double(a) => list(a),
        Value::Rational(a) if tag == 0x829a && group != "GPS" && a.len() == 1 && a[0].1 != 0 => {
            // exposure time as a fraction of a second
            let (n, d) = a[0];
            let s = n as f64 / d as f64;
            if s >= 1.0 || n == 0 { format!("{} s", rational(n as f64, d as f64)) } else { format!("1/{} s", (1.0 / s).round()) }
        }
        Value::Rational(a) if tag == 0x829d && group != "GPS" && a.len() == 1 => format!("f/{}", rational(a[0].0 as f64, a[0].1 as f64)),
        Value::Rational(a) if tag == 0x920a && group != "GPS" && a.len() == 1 => format!("{} mm", rational(a[0].0 as f64, a[0].1 as f64)),
        Value::Rational(a) => a.iter().map(|(n, d)| rational(*n as f64, *d as f64)).collect::<Vec<_>>().join(", "),
        Value::SRational(a) => a.iter().map(|(n, d)| rational(*n as f64, *d as f64)).collect::<Vec<_>>().join(", "),
        Value::Byte(b) | Value::Undefined(b) => {
            // version tags and short printable values read as text
            let printable = b.len() <= 64 && b.iter().all(|c| c.is_ascii_graphic() || *c == b' ' || *c == 0);
            if printable && !b.is_empty() {
                String::from_utf8_lossy(b).trim_end_matches('\0').trim().to_string()
            } else if b.len() <= 8 {
                list(b)
            } else {
                format!("({} bytes)", b.len())
            }
        }
        Value::SByte(a) => list(a),
        Value::Ifd(a) => format!("(IFD at {})", list(a)),
        Value::Ifd8(a) => format!("(IFD at {})", list(a)),
    }
}

fn rows_of(group: &'static str, ifd: &Ifd, out: &mut Vec<TagRow>) {
    for e in &ifd.entries {
        // pointers to other IFDs and image data are structure, not metadata
        if matches!(e.tag, 0x8769 | 0x8825 | 0xa005 | 0x014a | 0x0111 | 0x0117 | 0x0144 | 0x0145 | 0x0201 | 0x0202 | 0x02bc | 0x83bb | 0x8773)
            && group != "GPS"
        {
            continue;
        }
        out.push(TagRow { group, tag: e.tag, name: name(group, e.tag), value: value_text(group, e.tag, &e.value) });
    }
}

/// Every tag of a parsed TIFF structure (a TIFF-based raw, a TIFF, or an EXIF block): IFD0,
/// then the EXIF, GPS and interoperability IFDs.
pub fn tag_rows(tiff: &Tiff) -> Vec<TagRow> {
    let mut out = Vec::new();
    if let Some(ifd0) = tiff.ifds.first() {
        rows_of("TIFF", ifd0, &mut out);
        if let Some(e) = &ifd0.exif {
            rows_of("EXIF", e, &mut out);
            if let Some(i) = &e.interop {
                rows_of("Interop", i, &mut out);
            }
        }
        if let Some(g) = &ifd0.gps {
            rows_of("GPS", g, &mut out);
        }
    }
    out
}

/// [`tag_rows`] of a file's bytes: a TIFF-based file directly, else its embedded EXIF block
/// (JPEG, PNG, WebP, HEIF…). Empty when there is none.
pub fn file_tag_rows(bytes: &[u8]) -> Vec<TagRow> {
    if Tiff::sniff(bytes).is_some()
        && let Ok(t) = Tiff::parse(bytes)
    {
        return tag_rows(&t);
    }
    let Some(exif) = crate::embedded(bytes).exif else { return Vec::new() };
    Tiff::parse(crate::strip_exif_header(&exif)).map(|t| tag_rows(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_from_written_exif() {
        let m = crate::Metadata {
            make: Some("Maker".into()),
            model: Some("Model X".into()),
            exposure_time: Some(1.0 / 250.0),
            f_number: Some(2.8),
            iso: Some(400),
            focal_length: Some(35.0),
            gps: Some(crate::Gps { latitude: 51.5, longitude: -0.12, altitude: Some(20.0) }),
            ..Default::default()
        };
        let exif = crate::write_exif(&m);
        let t = Tiff::parse(crate::strip_exif_header(&exif)).unwrap();
        let rows = tag_rows(&t);
        let get = |n: &str| rows.iter().find(|r| r.name == n).map(|r| r.value.clone());
        assert_eq!(get("Make").as_deref(), Some("Maker"));
        assert_eq!(get("Exposure Time").as_deref(), Some("1/250 s"));
        assert_eq!(get("F-Number").as_deref(), Some("f/2.8"));
        assert_eq!(get("Focal Length").as_deref(), Some("35 mm"));
        assert_eq!(get("ISO Speed").as_deref(), Some("400"));
        assert!(rows.iter().any(|r| r.group == "GPS" && r.name == "GPS Latitude"), "{rows:?}");
        assert!(!rows.iter().any(|r| r.tag == 0x8769), "IFD pointers are left out");
    }
}
