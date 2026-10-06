//! Exif / TIFF tags → [`Metadata`] (CIPA DC-008 Exif 2.32, TIFF 6.0, DNG 1.7 tag semantics).

use crate::{DateTime, Flash, Gps, Metadata, Orientation};
use lightcraft_tiff::{Ifd, Tiff, TiffError, tags as t};

/// Strip an `Exif\0\0` (or `Exif\0\xff`) prefix as found in JPEG APP1 and some WebP `EXIF` chunks.
pub fn strip_exif_header(b: &[u8]) -> &[u8] {
    if b.len() >= 6 && &b[..4] == b"Exif" && b[4] == 0 { &b[6..] } else { b }
}

/// Read metadata from a TIFF-structured Exif block (a whole TIFF/DNG/raw file, or the payload of a JPEG APP1
/// Exif segment with or without the `Exif\0\0` prefix). Malformed input yields whatever could be read.
pub fn read_exif(bytes: &[u8]) -> Metadata {
    try_read_exif(bytes).unwrap_or_default()
}

/// Like [`read_exif`] but reports a structurally invalid TIFF header / IFD0.
pub fn try_read_exif(bytes: &[u8]) -> Result<Metadata, TiffError> {
    let tiff = Tiff::parse(strip_exif_header(bytes))?;
    Ok(from_tiff(&tiff))
}

fn text(ifd: Option<&Ifd>, tag: u16) -> Option<String> {
    let s = ifd?.string(tag)?;
    let s = s.trim().to_string();
    (!s.is_empty() && !s.bytes().all(|b| b == b' ' || b == 0)).then_some(s)
}

fn pos(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

/// Build [`Metadata`] from a parsed TIFF stream (IFD0 + Exif + GPS; DNG tags as fallbacks).
pub fn from_tiff(tiff: &Tiff) -> Metadata {
    let ifd0 = tiff.ifds.first();
    let exif = tiff.exif();
    let gps = tiff.gps();
    let either = |tag: u16| exif.and_then(|e| e.value(tag)).or_else(|| ifd0.and_then(|i| i.value(tag)));
    let mut m = Metadata {
        make: text(ifd0, t::MAKE),
        model: text(ifd0, t::MODEL).or_else(|| text(ifd0, t::UNIQUE_CAMERA_MODEL)),
        software: text(ifd0, t::SOFTWARE),
        artist: text(ifd0, t::ARTIST),
        copyright: text(ifd0, t::COPYRIGHT),
        caption: text(ifd0, t::IMAGE_DESCRIPTION),
        serial_number: text(exif, t::BODY_SERIAL_NUMBER).or_else(|| text(ifd0, t::CAMERA_SERIAL_NUMBER)),
        lens_make: text(exif, t::LENS_MAKE),
        lens_model: text(exif, t::LENS_MODEL),
        lens_serial_number: text(exif, t::LENS_SERIAL_NUMBER),
        ..Default::default()
    };
    // Exif requires LensSpecification/LensInfo as 4 rationals.
    let spec = exif.and_then(|e| e.f64s(t::LENS_SPECIFICATION)).or_else(|| ifd0.and_then(|i| i.f64s(t::LENS_INFO)));
    if let Some(s) = spec.filter(|s| s.len() == 4 && s[0] > 0.0) {
        m.lens_spec = Some([s[0], s[1], s[2], s[3]]);
    }
    m.orientation = ifd0.and_then(|i| i.u16(t::ORIENTATION)).filter(|v| (1..=8).contains(v)).map(Orientation::from_exif);
    m.rating = ifd0.and_then(|i| i.i64(t::RATING)).map(|r| r.clamp(-1, 5) as i8);

    m.exposure_time = pos(either(t::EXPOSURE_TIME).and_then(|v| v.get_f64(0)))
        .or_else(|| exif.and_then(|e| e.f64(t::SHUTTER_SPEED_VALUE)).filter(|v| v.abs() < 64.0).map(|tv| 2f64.powf(-tv)));
    m.f_number = pos(either(t::F_NUMBER).and_then(|v| v.get_f64(0)))
        .or_else(|| exif.and_then(|e| e.f64(t::APERTURE_VALUE)).filter(|v| v.abs() < 64.0).map(|av| 2f64.powf(av / 2.0)));
    let iso = either(t::ISO_SPEED).and_then(|v| v.get_u64(0)).filter(|&v| v > 0);
    let iso_ext = exif.and_then(|e| e.u64(t::RECOMMENDED_EXPOSURE_INDEX).or_else(|| e.u64(t::ISO_SPEED_RATINGS_EXT))).filter(|&v| v > 0);
    m.iso = match (iso, iso_ext) {
        (Some(65535), Some(x)) | (None, Some(x)) => Some(x),
        (Some(v), _) => Some(v),
        _ => None,
    }
    .map(|v| v.min(u32::MAX as u64) as u32);
    m.focal_length = pos(either(t::FOCAL_LENGTH).and_then(|v| v.get_f64(0)));
    m.focal_length_35mm = pos(exif.and_then(|e| e.f64(t::FOCAL_LENGTH_35MM)));
    m.exposure_bias = exif.and_then(|e| e.f64(t::EXPOSURE_BIAS)).filter(|v| v.is_finite() && v.abs() < 100.0);
    m.exposure_program = exif.and_then(|e| e.u16(t::EXPOSURE_PROGRAM));
    m.metering_mode = exif.and_then(|e| e.u16(t::METERING_MODE));
    m.white_balance = exif.and_then(|e| e.u16(t::WHITE_BALANCE));
    m.flash = exif.and_then(|e| e.u16(t::FLASH)).map(|raw| Flash { fired: raw & 1 != 0, raw });

    // capture time: DateTimeOriginal, then DateTimeDigitized, then IFD0 DateTime
    let candidates = [
        (exif, t::DATE_TIME_ORIGINAL, t::OFFSET_TIME_ORIGINAL, t::SUBSEC_TIME_ORIGINAL),
        (exif, t::DATE_TIME_DIGITIZED, t::OFFSET_TIME_DIGITIZED, t::SUBSEC_TIME_DIGITIZED),
        (ifd0, t::DATE_TIME, t::OFFSET_TIME, t::SUBSEC_TIME),
    ];
    for (ifd, dt_tag, off_tag, sub_tag) in candidates {
        if let Some(mut dt) = text(ifd, dt_tag).and_then(|s| DateTime::parse_exif(&s)) {
            if let Some(s) = text(exif, sub_tag) {
                dt = dt.with_subsec(&s);
            }
            dt.offset_minutes = text(exif, off_tag).and_then(|s| DateTime::parse_offset(&s));
            m.capture_time = Some(dt);
            break;
        }
    }

    m.width = exif.and_then(|e| e.u32(t::PIXEL_X_DIMENSION)).filter(|&v| v > 0).or_else(|| ifd0.and_then(|i| i.u32(t::IMAGE_WIDTH)));
    m.height = exif.and_then(|e| e.u32(t::PIXEL_Y_DIMENSION)).filter(|&v| v > 0).or_else(|| ifd0.and_then(|i| i.u32(t::IMAGE_LENGTH)));

    m.gps = gps.and_then(read_gps);
    if let Some(iptc) = ifd0.and_then(|i| i.value(t::IPTC_NAA)) {
        // IPTC-NAA is often typed LONG; re-serialise to bytes in file order.
        let bytes = match iptc {
            lightcraft_tiff::Value::Byte(b) | lightcraft_tiff::Value::Undefined(b) => b.clone(),
            other => lightcraft_tiff::writer::encode(tiff.order, other),
        };
        m.fill_missing(&crate::parse_iptc(&bytes));
    }
    if let Some(x) = ifd0.and_then(|i| i.bytes(t::XMP)).map(String::from_utf8_lossy)
        && let Ok(x) = crate::parse_xmp(&x)
    {
        m.overlay_user_fields(&x.metadata);
        m.fill_missing(&x.metadata);
    }
    m
}

fn dms(ifd: &Ifd, tag: u16) -> Option<f64> {
    let v = ifd.f64s(tag)?;
    let d = *v.first()?;
    let deg = d + v.get(1).copied().unwrap_or(0.0) / 60.0 + v.get(2).copied().unwrap_or(0.0) / 3600.0;
    deg.is_finite().then_some(deg)
}

fn read_gps(g: &Ifd) -> Option<Gps> {
    let mut lat = dms(g, t::GPS_LATITUDE)?;
    let mut lon = dms(g, t::GPS_LONGITUDE)?;
    if g.string(t::GPS_LATITUDE_REF).is_some_and(|s| s.starts_with('S')) {
        lat = -lat;
    }
    if g.string(t::GPS_LONGITUDE_REF).is_some_and(|s| s.starts_with('W')) {
        lon = -lon;
    }
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }
    let altitude = g.f64(t::GPS_ALTITUDE).filter(|a| a.is_finite()).map(|a| if g.u32(t::GPS_ALTITUDE_REF) == Some(1) { -a } else { a });
    Some(Gps { latitude: lat, longitude: lon, altitude })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lightcraft_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value};

    pub(crate) fn sample_exif(order: ByteOrder) -> Vec<u8> {
        let mut exif = IfdBuilder::new();
        exif.set(t::EXPOSURE_TIME, Value::Rational(vec![(1, 250)]));
        exif.set(t::F_NUMBER, Value::Rational(vec![(28, 10)]));
        exif.set(t::ISO_SPEED, Value::Short(vec![400]));
        exif.set(t::DATE_TIME_ORIGINAL, Value::Ascii("2023:07:14 18:30:05".into()));
        exif.set(t::OFFSET_TIME_ORIGINAL, Value::Ascii("+02:00".into()));
        exif.set(t::SUBSEC_TIME_ORIGINAL, Value::Ascii("42".into()));
        exif.set(t::EXPOSURE_BIAS, Value::SRational(vec![(-2, 3)]));
        exif.set(t::METERING_MODE, Value::Short(vec![5]));
        exif.set(t::FLASH, Value::Short(vec![0x19]));
        exif.set(t::FOCAL_LENGTH, Value::Rational(vec![(50, 1)]));
        exif.set(t::FOCAL_LENGTH_35MM, Value::Short(vec![75]));
        exif.set(t::WHITE_BALANCE, Value::Short(vec![0]));
        exif.set(t::EXPOSURE_PROGRAM, Value::Short(vec![3]));
        exif.set(t::LENS_MAKE, Value::Ascii("LensCo".into()));
        exif.set(t::LENS_MODEL, Value::Ascii("50mm F1.8".into()));
        exif.set(t::BODY_SERIAL_NUMBER, Value::Ascii("12345".into()));
        exif.set(t::LENS_SPECIFICATION, Value::Rational(vec![(50, 1), (50, 1), (18, 10), (18, 10)]));
        exif.set(t::PIXEL_X_DIMENSION, Value::Long(vec![6000]));
        exif.set(t::PIXEL_Y_DIMENSION, Value::Long(vec![4000]));
        let mut gps = IfdBuilder::new();
        gps.set(t::GPS_LATITUDE_REF, Value::Ascii("S".into()));
        gps.set(t::GPS_LATITUDE, Value::Rational(vec![(33, 1), (51, 1), (3564, 100)]));
        gps.set(t::GPS_LONGITUDE_REF, Value::Ascii("E".into()));
        gps.set(t::GPS_LONGITUDE, Value::Rational(vec![(151, 1), (12, 1), (4032, 100)]));
        gps.set(t::GPS_ALTITUDE_REF, Value::Byte(vec![1]));
        gps.set(t::GPS_ALTITUDE, Value::Rational(vec![(125, 10)]));
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::MAKE, Value::Ascii("Maker Inc.".into()));
        ifd0.set(t::MODEL, Value::Ascii("Model X ".into()));
        ifd0.set(t::ORIENTATION, Value::Short(vec![6]));
        ifd0.set(t::ARTIST, Value::Ascii("Jane".into()));
        ifd0.set(t::COPYRIGHT, Value::Ascii("(c) Jane".into()));
        ifd0.set(t::RATING, Value::Short(vec![3]));
        ifd0.set_child(t::EXIF_IFD, exif);
        ifd0.set_child(t::GPS_IFD, gps);
        TiffWriter::new(order, false).write(&[ifd0]).unwrap()
    }

    #[test]
    fn reads_all_fields() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let m = read_exif(&sample_exif(order));
            assert_eq!(m.make.as_deref(), Some("Maker Inc."));
            assert_eq!(m.model.as_deref(), Some("Model X"));
            assert_eq!(m.exposure_time, Some(0.004));
            assert_eq!(m.f_number, Some(2.8));
            assert_eq!(m.iso, Some(400));
            let dt = m.capture_time.unwrap();
            assert_eq!(dt.to_iso(), "2023-07-14T18:30:05.420+02:00");
            assert!((m.exposure_bias.unwrap() + 0.6667).abs() < 1e-3);
            assert_eq!(m.metering_mode, Some(5));
            assert_eq!(m.flash, Some(Flash { fired: true, raw: 0x19 }));
            assert_eq!(m.focal_length, Some(50.0));
            assert_eq!(m.focal_length_35mm, Some(75.0));
            assert_eq!(m.orientation, Some(Orientation::Rotate90));
            assert_eq!(m.lens_model.as_deref(), Some("50mm F1.8"));
            assert_eq!(m.lens_make.as_deref(), Some("LensCo"));
            assert_eq!(m.serial_number.as_deref(), Some("12345"));
            assert_eq!(m.lens_spec, Some([50.0, 50.0, 1.8, 1.8]));
            assert_eq!((m.width, m.height), (Some(6000), Some(4000)));
            assert_eq!(m.artist.as_deref(), Some("Jane"));
            assert_eq!(m.rating, Some(3));
            let g = m.gps.unwrap();
            assert!((g.latitude + 33.8599).abs() < 1e-4, "{g:?}");
            assert!((g.longitude - 151.2112).abs() < 1e-4);
            assert_eq!(g.altitude, Some(-12.5));
        }
    }

    #[test]
    fn exif_prefix_and_garbage() {
        let mut b = b"Exif\0\0".to_vec();
        b.extend(sample_exif(ByteOrder::Little));
        assert_eq!(read_exif(&b).iso, Some(400));
        assert_eq!(read_exif(b"garbage"), Metadata::default());
        assert!(try_read_exif(b"garbage").is_err());
    }

    #[test]
    fn apex_fallbacks_and_extended_iso() {
        let mut exif = IfdBuilder::new();
        exif.set(t::SHUTTER_SPEED_VALUE, Value::SRational(vec![(8, 1)]));
        exif.set(t::APERTURE_VALUE, Value::Rational(vec![(4, 1)]));
        exif.set(t::ISO_SPEED, Value::Short(vec![65535]));
        exif.set(t::RECOMMENDED_EXPOSURE_INDEX, Value::Long(vec![102400]));
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::DATE_TIME, Value::Ascii("2020:01:02 03:04:05".into()));
        ifd0.set_child(t::EXIF_IFD, exif);
        let m = read_exif(&TiffWriter::default().write(&[ifd0]).unwrap());
        assert_eq!(m.exposure_time, Some(1.0 / 256.0));
        assert_eq!(m.f_number, Some(4.0));
        assert_eq!(m.iso, Some(102400));
        assert_eq!(m.capture_time.unwrap().to_exif(), "2020:01:02 03:04:05");
    }

    #[test]
    fn xmp_in_ifd0_overrides_user_fields() {
        let mut meta = Metadata { rating: Some(5), title: Some("From XMP".into()), ..Default::default() };
        meta.keywords = vec!["a".into()];
        let x = crate::write_xmp(&meta, None);
        let mut ifd0 = IfdBuilder::new();
        ifd0.set(t::RATING, Value::Short(vec![1]));
        ifd0.set(t::XMP, Value::Byte(x.into_bytes()));
        let m = read_exif(&TiffWriter::default().write(&[ifd0]).unwrap());
        assert_eq!(m.rating, Some(5));
        assert_eq!(m.title.as_deref(), Some("From XMP"));
    }
}

/// Serialise the interchange subset of `meta` as a TIFF-structured Exif block (little-endian, no
/// `Exif\0\0` prefix): IFD0 (make, model, software, artist, copyright, description, orientation,
/// date), the Exif IFD (capture settings, lens) and, when present, the GPS IFD.
pub fn write_exif(meta: &Metadata) -> Vec<u8> {
    use lightcraft_tiff::writer::{rational, srational};
    use lightcraft_tiff::{ByteOrder, IfdBuilder, TiffWriter, Value};
    let ascii = |s: &Option<String>| s.as_ref().filter(|v| !v.is_empty()).map(|v| Value::Ascii(v.clone()));
    let mut ifd0 = IfdBuilder::new();
    for (tag, v) in [
        (t::MAKE, ascii(&meta.make)),
        (t::MODEL, ascii(&meta.model)),
        (t::SOFTWARE, ascii(&meta.software)),
        (t::ARTIST, ascii(&meta.artist)),
        (t::COPYRIGHT, ascii(&meta.copyright)),
        (t::IMAGE_DESCRIPTION, ascii(&meta.caption)),
        (t::DATE_TIME, meta.capture_time.map(|d| Value::Ascii(d.to_exif()))),
        (t::ORIENTATION, meta.orientation.map(|o| Value::Short(vec![o.to_exif()]))),
    ] {
        if let Some(v) = v {
            ifd0.set(tag, v);
        }
    }
    let mut ex = IfdBuilder::new();
    ex.set(t::EXIF_VERSION, Value::Undefined(b"0232".to_vec()));
    let r = |v: Option<f64>| v.filter(|v| v.is_finite() && *v > 0.0).map(|v| Value::Rational(vec![rational(v)]));
    for (tag, v) in [
        (t::DATE_TIME_ORIGINAL, meta.capture_time.map(|d| Value::Ascii(d.to_exif()))),
        (t::EXPOSURE_TIME, r(meta.exposure_time)),
        (t::F_NUMBER, r(meta.f_number)),
        (t::FOCAL_LENGTH, r(meta.focal_length)),
        (t::ISO_SPEED, meta.iso.map(|i| Value::Short(vec![i.min(65_535) as u16]))),
        (t::LENS_MAKE, ascii(&meta.lens_make)),
        (t::LENS_MODEL, ascii(&meta.lens_model)),
        (t::EXPOSURE_BIAS, meta.exposure_bias.map(|v| Value::SRational(vec![srational(v)]))),
    ] {
        if let Some(v) = v {
            ex.set(tag, v);
        }
    }
    ifd0.set_child(t::EXIF_IFD, ex);
    if let Some(g) = meta.gps {
        let dms = |v: f64| {
            let v = v.abs();
            let d = v.floor();
            let m = ((v - d) * 60.0).floor();
            let s = (v - d - m / 60.0) * 3600.0;
            Value::Rational(vec![(d as u32, 1), (m as u32, 1), ((s * 1000.0).round() as u32, 1000)])
        };
        let mut gps = IfdBuilder::new();
        gps.set(t::GPS_VERSION_ID, Value::Byte(vec![2, 3, 0, 0]));
        gps.set(t::GPS_LATITUDE_REF, Value::Ascii(if g.latitude < 0.0 { "S" } else { "N" }.into()));
        gps.set(t::GPS_LATITUDE, dms(g.latitude));
        gps.set(t::GPS_LONGITUDE_REF, Value::Ascii(if g.longitude < 0.0 { "W" } else { "E" }.into()));
        gps.set(t::GPS_LONGITUDE, dms(g.longitude));
        if let Some(a) = g.altitude {
            gps.set(t::GPS_ALTITUDE_REF, Value::Byte(vec![u8::from(a < 0.0)]));
            gps.set(t::GPS_ALTITUDE, Value::Rational(vec![rational(a.abs())]));
        }
        ifd0.set_child(t::GPS_IFD, gps);
    }
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0]).unwrap_or_default()
}

#[cfg(test)]
mod write_tests {
    use super::*;

    #[test]
    fn exif_round_trips() {
        let m = Metadata {
            make: Some("Synthetic".into()),
            model: Some("X2".into()),
            copyright: Some("© 2026 A. Person".into()),
            artist: Some("A. Person".into()),
            capture_time: DateTime::parse_iso("2026-09-20T05:40:00"),
            exposure_time: Some(1.0 / 250.0),
            f_number: Some(2.8),
            iso: Some(400),
            focal_length: Some(35.0),
            lens_model: Some("35mm F2".into()),
            gps: Some(Gps { latitude: 43.7904, longitude: -110.6818, altitude: Some(2000.0) }),
            ..Default::default()
        };
        let back = read_exif(&write_exif(&m));
        assert_eq!(back.make, m.make);
        assert_eq!(back.model, m.model);
        assert_eq!(back.copyright, m.copyright);
        assert_eq!(back.artist, m.artist);
        assert_eq!(back.capture_time.map(|d| d.to_exif()), m.capture_time.map(|d| d.to_exif()));
        assert_eq!(back.iso, Some(400));
        assert!((back.f_number.unwrap() - 2.8).abs() < 1e-6);
        assert!((back.exposure_time.unwrap() - 0.004).abs() < 1e-9);
        assert_eq!(back.lens_model, m.lens_model);
        let g = back.gps.unwrap();
        assert!((g.latitude - 43.7904).abs() < 1e-5 && (g.longitude + 110.6818).abs() < 1e-5, "{g:?}");
    }
}
