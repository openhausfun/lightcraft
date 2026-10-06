//! GPS track logs in GPX 1.0 / 1.1 (the open GPS Exchange Format, <https://www.topografix.com/gpx.asp>):
//! the timed track points of every `<trk>`, and the position at any moment for geotagging photos.
//!
//! What we use from the format: `<trk>` → `<trkseg>` → `<trkpt lat=… lon=…>` with optional `<ele>`
//! (metres) and `<time>` (an `xsd:dateTime`, UTC per the schema — a time without a zone is read as
//! UTC). Points without a time can't be matched to a photo and are left out; waypoints (`<wpt>`) and
//! routes (`<rte>`) carry no timeline and are ignored. The schema defines a track segment as one
//! continuous span of logging, so positions are only interpolated *within* a segment, never across
//! the gap where reception was lost or the logger was off.

use quick_xml::events::Event;

use crate::{DateTime, Gps};

/// Nesting / size limits (a track log is flat; these only stop hostile input).
const MAX_DEPTH: usize = 64;
const MAX_POINTS: usize = 5_000_000;

/// One timed point of a track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackPoint {
    /// Seconds since 1970-01-01T00:00:00Z (fractional when the log has sub-second times).
    pub time: f64,
    pub latitude: f64,
    pub longitude: f64,
    /// Metres above sea level.
    pub elevation: Option<f64>,
    /// Track segment number, counted across the whole file (continuous logging spans).
    pub segment: u32,
}

/// How a position was found by [`Tracklog::locate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// The time falls between two points of the same segment: linear interpolation.
    Interpolated,
    /// The nearest point (at a segment's start or end, or across a gap).
    Nearest,
}

/// The timed points of a GPX file, sorted by time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracklog {
    pub points: Vec<TrackPoint>,
    /// Tracks (`<trk>`) in the file.
    pub tracks: usize,
    /// Track points without a usable time (left out of `points`).
    pub untimed: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GpxError {
    Xml(String),
    /// Not a GPX document (no `<gpx>` root).
    NotGpx,
    /// Nesting / size limits exceeded.
    TooComplex,
}

impl std::fmt::Display for GpxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpxError::Xml(e) => write!(f, "malformed GPX: {e}"),
            GpxError::NotGpx => f.write_str("not a GPX file (no <gpx> element)"),
            GpxError::TooComplex => f.write_str("GPX file too large or too deeply nested"),
        }
    }
}

impl std::error::Error for GpxError {}

/// Local name of a possibly prefixed XML name (`gpx:trkpt` → `trkpt`).
fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|&b| b == b':').next().unwrap_or(name)
}

/// An `xsd:dateTime` as seconds since the Unix epoch; no zone = UTC (the GPX schema's convention).
fn parse_time(s: &str) -> Option<f64> {
    let mut d = DateTime::parse_iso(s)?;
    // a bare date (no time of day) is too coarse to place a photo
    if !s.contains('T') {
        return None;
    }
    d.offset_minutes.get_or_insert(0);
    Some(d.unix_seconds() as f64 + d.millis as f64 / 1000.0)
}

#[derive(Default)]
struct Pending {
    lat: Option<f64>,
    lon: Option<f64>,
    ele: Option<f64>,
    time: Option<f64>,
}

/// Parse a GPX document.
pub fn parse_gpx(s: &str) -> Result<Tracklog, GpxError> {
    let mut reader = quick_xml::Reader::from_str(s);
    reader.config_mut().check_end_names = false;
    let mut path: Vec<Vec<u8>> = Vec::new();
    let mut seen_gpx = false;
    let mut log = Tracklog::default();
    let mut segment: u32 = 0;
    let mut pt: Option<Pending> = None;
    let mut text = String::new();
    loop {
        let ev = reader.read_event().map_err(|e| GpxError::Xml(e.to_string()))?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let name = local(e.name().as_ref()).to_vec();
                let parent = path.last().map(Vec::as_slice);
                match name.as_slice() {
                    b"gpx" if path.is_empty() => seen_gpx = true,
                    b"trk" if parent == Some(b"gpx") => log.tracks += 1,
                    b"trkseg" if parent == Some(b"trk") => {
                        if log.points.last().is_some_and(|p| p.segment == segment) {
                            segment += 1;
                        }
                    }
                    b"trkpt" if parent == Some(b"trkseg") => {
                        let mut p = Pending::default();
                        for a in e.attributes().with_checks(false).flatten() {
                            let v = String::from_utf8_lossy(&a.value);
                            match local(a.key.as_ref()) {
                                b"lat" => p.lat = v.trim().parse().ok(),
                                b"lon" => p.lon = v.trim().parse().ok(),
                                _ => {}
                            }
                        }
                        pt = Some(p);
                    }
                    _ => {}
                }
                text.clear();
                if matches!(ev, Event::Start(_)) {
                    if path.len() >= MAX_DEPTH {
                        return Err(GpxError::TooComplex);
                    }
                    path.push(name);
                } else if name == b"trkpt" {
                    finish_point(&mut log, pt.take(), segment)?;
                }
            }
            Event::Text(t) => text.push_str(&t.decode().map_err(|e| GpxError::Xml(e.to_string()))?),
            Event::CData(t) => text.push_str(&String::from_utf8_lossy(&t)),
            Event::End(_) => {
                let Some(name) = path.pop() else { continue };
                let parent = path.last().map(Vec::as_slice);
                match name.as_slice() {
                    b"ele" if parent == Some(b"trkpt") => {
                        if let Some(p) = pt.as_mut() {
                            p.ele = text.trim().parse().ok();
                        }
                    }
                    b"time" if parent == Some(b"trkpt") => {
                        if let Some(p) = pt.as_mut() {
                            p.time = parse_time(text.trim());
                        }
                    }
                    b"trkpt" => finish_point(&mut log, pt.take(), segment)?,
                    _ => {}
                }
                text.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !seen_gpx {
        return Err(GpxError::NotGpx);
    }
    // stable: points logged at the same second keep their file order
    log.points.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(log)
}

fn finish_point(log: &mut Tracklog, p: Option<Pending>, segment: u32) -> Result<(), GpxError> {
    let Some(p) = p else { return Ok(()) };
    let (Some(lat), Some(lon)) = (p.lat, p.lon) else { return Ok(()) };
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Ok(());
    }
    let Some(time) = p.time else {
        log.untimed += 1;
        return Ok(());
    };
    if log.points.len() >= MAX_POINTS {
        return Err(GpxError::TooComplex);
    }
    log.points.push(TrackPoint { time, latitude: lat, longitude: lon, elevation: p.ele, segment });
    Ok(())
}

impl Tracklog {
    /// First and last point times (seconds since the epoch, UTC).
    pub fn span(&self) -> Option<(f64, f64)> {
        Some((self.points.first()?.time, self.points.last()?.time))
    }

    /// Where the logger was at `time` (seconds since the epoch, UTC).
    ///
    /// Between two points of the same segment that are at most `max_gap` seconds apart, the position
    /// is interpolated linearly in time. Otherwise the nearest point counts if it is within `max_gap`
    /// seconds; beyond that there is no position (the photo was taken while nothing was logged).
    pub fn locate(&self, time: f64, max_gap: f64) -> Option<(Gps, Match)> {
        let pts = &self.points;
        if pts.is_empty() || !time.is_finite() {
            return None;
        }
        let i = pts.partition_point(|p| p.time < time);
        let next = pts.get(i);
        let prev = i.checked_sub(1).and_then(|j| pts.get(j));
        if let Some(n) = next
            && n.time == time
        {
            return Some((gps(n), Match::Interpolated));
        }
        if let (Some(a), Some(b)) = (prev, next)
            && a.segment == b.segment
            && b.time - a.time <= max_gap
        {
            let t = (time - a.time) / (b.time - a.time);
            return Some((lerp(a, b, t), Match::Interpolated));
        }
        let near = match (prev, next) {
            (Some(a), Some(b)) => {
                if time - a.time <= b.time - time {
                    a
                } else {
                    b
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => return None,
        };
        ((near.time - time).abs() <= max_gap).then(|| (gps(near), Match::Nearest))
    }
}

fn gps(p: &TrackPoint) -> Gps {
    Gps { latitude: p.latitude, longitude: p.longitude, altitude: p.elevation }
}

/// Linear interpolation, taking the short way across the ±180° meridian.
fn lerp(a: &TrackPoint, b: &TrackPoint, t: f64) -> Gps {
    let mut dlon = b.longitude - a.longitude;
    if dlon > 180.0 {
        dlon -= 360.0;
    } else if dlon < -180.0 {
        dlon += 360.0;
    }
    let mut lon = a.longitude + dlon * t;
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon < -180.0 {
        lon += 360.0;
    }
    let altitude = match (a.elevation, b.elevation) {
        (Some(x), Some(y)) => Some(x + (y - x) * t),
        (x, y) => x.or(y),
    };
    Gps { latitude: a.latitude + (b.latitude - a.latitude) * t, longitude: lon, altitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <wpt lat="1.0" lon="1.0"><time>2026-05-01T10:00:30Z</time></wpt>
  <trk><name>Walk</name>
    <trkseg>
      <trkpt lat="46.0000" lon="7.0000"><ele>1600</ele><time>2026-05-01T10:00:00Z</time></trkpt>
      <trkpt lat="46.0010" lon="7.0020"><ele>1610</ele><time>2026-05-01T10:01:00Z</time></trkpt>
      <trkpt lat="46.0020" lon="7.0040"><time>2026-05-01T10:02:00.500Z</time></trkpt>
      <trkpt lat="46.5" lon="7.5"/>
    </trkseg>
    <trkseg>
      <trkpt lat="47.0000" lon="8.0000"><time>2026-05-01T12:00:00Z</time></trkpt>
      <trkpt lat="47.0010" lon="8.0010"><time>2026-05-01T12:00:10+00:00</time></trkpt>
    </trkseg>
  </trk>
</gpx>"#;

    fn t(s: &str) -> f64 {
        parse_time(s).unwrap()
    }

    #[test]
    fn parses_timed_track_points() {
        let log = parse_gpx(LOG).unwrap();
        assert_eq!(log.tracks, 1);
        assert_eq!(log.points.len(), 5, "waypoints and the untimed point are left out");
        assert_eq!(log.untimed, 1);
        assert_eq!(log.points[0].elevation, Some(1600.0));
        assert_eq!(log.points[2].time, t("2026-05-01T10:02:00Z") + 0.5);
        assert_eq!((log.points[0].segment, log.points[2].segment, log.points[3].segment), (0, 0, 1));
        assert_eq!(log.span(), Some((t("2026-05-01T10:00:00Z"), t("2026-05-01T12:00:10Z"))));
        // 2026-05-01T10:00:00Z
        assert_eq!(log.points[0].time, 1_777_629_600.0);
    }

    #[test]
    fn interpolates_within_a_segment() {
        let log = parse_gpx(LOG).unwrap();
        let (g, m) = log.locate(t("2026-05-01T10:00:30Z"), 300.0).unwrap();
        assert_eq!(m, Match::Interpolated);
        assert!((g.latitude - 46.0005).abs() < 1e-9 && (g.longitude - 7.0010).abs() < 1e-9, "{g:?}");
        assert_eq!(g.altitude, Some(1605.0));
        // exactly on a point
        let (g, _) = log.locate(t("2026-05-01T10:01:00Z"), 300.0).unwrap();
        assert_eq!((g.latitude, g.longitude), (46.0010, 7.0020));
    }

    #[test]
    fn never_interpolates_across_segments() {
        let log = parse_gpx(LOG).unwrap();
        // between the segments (10:02 … 12:00): only near either end
        let (g, m) = log.locate(t("2026-05-01T10:04:00Z"), 300.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 46.0020));
        let (g, m) = log.locate(t("2026-05-01T11:58:00Z"), 300.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 47.0));
        assert!(log.locate(t("2026-05-01T11:00:00Z"), 300.0).is_none(), "an hour from any point");
        // before the first and after the last point
        assert!(log.locate(t("2026-05-01T09:59:00Z"), 300.0).is_some());
        assert!(log.locate(t("2026-05-01T09:50:00Z"), 300.0).is_none());
        assert!(log.locate(t("2026-05-01T12:10:00Z"), 300.0).is_none());
    }

    #[test]
    fn max_gap_limits_interpolation() {
        let log = parse_gpx(LOG).unwrap();
        // the points are 60 s apart: with a 20 s limit, only the nearest point within 20 s counts
        assert!(log.locate(t("2026-05-01T10:00:30Z"), 20.0).is_none());
        let (g, m) = log.locate(t("2026-05-01T10:00:50Z"), 20.0).unwrap();
        assert_eq!((m, g.latitude), (Match::Nearest, 46.0010));
    }

    #[test]
    fn gpx_1_0_prefixes_and_zones() {
        let s = r#"<gpx:gpx version="1.0" xmlns:gpx="http://www.topografix.com/GPX/1/0"><gpx:trk><gpx:trkseg>
            <gpx:trkpt lat="-33.9" lon="151.2"><gpx:time>2026-01-02T10:00:00+11:00</gpx:time></gpx:trkpt>
            <gpx:trkpt lat="-33.8" lon="151.3"><gpx:time>2026-01-01T23:01:00</gpx:time></gpx:trkpt>
        </gpx:trkseg></gpx:trk></gpx:gpx>"#;
        let log = parse_gpx(s).unwrap();
        assert_eq!(log.points.len(), 2);
        // +11:00 is converted to UTC; no zone = UTC
        assert_eq!(log.points[1].time - log.points[0].time, 60.0);
    }

    #[test]
    fn interpolation_across_the_antimeridian() {
        let a = TrackPoint { time: 0.0, latitude: 0.0, longitude: 179.0, elevation: None, segment: 0 };
        let b = TrackPoint { time: 10.0, latitude: 0.0, longitude: -179.0, elevation: None, segment: 0 };
        let g = lerp(&a, &b, 0.75);
        assert!((g.longitude - -179.5).abs() < 1e-9, "{g:?}");
    }

    #[test]
    fn rejects_non_gpx_and_bad_input() {
        assert_eq!(parse_gpx("<kml><Document/></kml>"), Err(GpxError::NotGpx));
        assert!(parse_gpx("").is_err());
        assert!(
            parse_gpx("<gpx><trk><trkseg><trkpt lat=\"x\" lon=\"1\"><time>2026-01-01T00:00:00Z</time></trkpt></trkseg></trk></gpx>")
                .unwrap()
                .points
                .is_empty()
        );
        let deep = "<gpx>".to_string() + &"<a>".repeat(100);
        assert_eq!(parse_gpx(&deep), Err(GpxError::TooComplex));
    }
}
