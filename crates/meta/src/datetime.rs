//! Capture date/time with optional UTC offset (Exif `DateTimeOriginal` + `OffsetTimeOriginal`, ISO 8601 in XMP).

use serde::{Deserialize, Serialize};

/// A civil date-time as recorded by the camera; `offset_minutes` is the UTC offset when known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Sub-second part in milliseconds.
    pub millis: u16,
    pub offset_minutes: Option<i16>,
}

fn num(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl DateTime {
    fn valid(self) -> Option<Self> {
        let ok = (1..=12).contains(&self.month)
            && (1..=31).contains(&self.day)
            && self.hour < 24
            && self.minute < 60
            && self.second < 61
            && self.year > 0
            && self.offset_minutes.is_none_or(|o| o.abs() <= 18 * 60);
        ok.then_some(self)
    }

    /// Exif form `YYYY:MM:DD HH:MM:SS` (also tolerates `-` date separators and a missing time).
    pub fn parse_exif(s: &str) -> Option<DateTime> {
        let s = s.trim().trim_end_matches('\0');
        let (d, t) = s.split_once([' ', 'T']).unwrap_or((s, ""));
        let dp: Vec<&str> = d.split([':', '-']).collect();
        if dp.len() != 3 {
            return None;
        }
        let tp: Vec<&str> = if t.is_empty() { vec![] } else { t.split(':').collect() };
        let g = |v: &Vec<&str>, i: usize| v.get(i).map(|x| num(x.trim())).unwrap_or(Some(0));
        DateTime {
            year: num(dp[0])?.try_into().ok()?,
            month: num(dp[1])?.try_into().ok()?,
            day: num(dp[2])?.try_into().ok()?,
            hour: g(&tp, 0)?.try_into().ok()?,
            minute: g(&tp, 1)?.try_into().ok()?,
            second: g(&tp, 2)?.try_into().ok()?,
            millis: 0,
            offset_minutes: None,
        }
        .valid()
    }

    /// Exif `OffsetTime*` form `+HH:MM` / `-HH:MM` (or `Z`).
    pub fn parse_offset(s: &str) -> Option<i16> {
        let s = s.trim().trim_end_matches('\0');
        if s == "Z" {
            return Some(0);
        }
        let (sign, rest) = match s.as_bytes().first()? {
            b'+' => (1, &s[1..]),
            b'-' => (-1, &s[1..]),
            _ => return None,
        };
        let (h, m) = match rest.split_once(':') {
            Some((h, m)) => (num(h)?, num(m)?),
            None if rest.len() == 4 => (num(rest.get(..2)?)?, num(rest.get(2..)?)?),
            None => (num(rest)?, 0),
        };
        if h > 18 || m > 59 {
            return None;
        }
        Some(sign * (h * 60 + m) as i16)
    }

    /// Set the sub-second part from an Exif `SubSecTime*` string (digits are a decimal fraction).
    pub fn with_subsec(mut self, s: &str) -> DateTime {
        let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).take(3).collect();
        if !digits.is_empty() {
            let v: u16 = digits.parse().unwrap_or(0);
            self.millis = v * 10u16.pow(3 - digits.len() as u32);
        }
        self
    }

    /// ISO 8601 (XMP date): `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, `YYYY-MM-DDThh:mm[:ss[.s+]][TZD]`.
    pub fn parse_iso(s: &str) -> Option<DateTime> {
        let s = s.trim();
        let (d, t) = s.split_once('T').unwrap_or((s, ""));
        let dp: Vec<&str> = d.split('-').collect();
        let year = num(dp.first()?)?.try_into().ok()?;
        let month = dp.get(1).map(|m| num(m)).unwrap_or(Some(1))?.try_into().ok()?;
        let day = dp.get(2).map(|m| num(m)).unwrap_or(Some(1))?.try_into().ok()?;
        if dp.len() > 3 {
            return None;
        }
        let mut dt = DateTime { year, month, day, ..Default::default() };
        if !t.is_empty() {
            let (clock, off) = match t.find(['Z', '+', '-']) {
                Some(i) => (&t[..i], Some(&t[i..])),
                None => (t, None),
            };
            let cp: Vec<&str> = clock.split(':').collect();
            dt.hour = num(cp.first()?)?.try_into().ok()?;
            dt.minute = num(cp.get(1)?)?.try_into().ok()?;
            if let Some(sec) = cp.get(2) {
                let (whole, frac) = sec.split_once('.').unwrap_or((sec, ""));
                dt.second = num(whole)?.try_into().ok()?;
                dt = dt.with_subsec(frac);
            }
            if let Some(o) = off {
                dt.offset_minutes = Some(Self::parse_offset(o)?);
            }
        }
        dt.valid()
    }

    /// Exif form `YYYY:MM:DD HH:MM:SS`.
    pub fn to_exif(&self) -> String {
        format!("{:04}:{:02}:{:02} {:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second)
    }

    /// ISO 8601 with milliseconds when non-zero and the offset when known.
    pub fn to_iso(&self) -> String {
        let mut s = format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", self.year, self.month, self.day, self.hour, self.minute, self.second);
        if self.millis != 0 {
            s += &format!(".{:03}", self.millis);
        }
        if let Some(o) = self.offset_minutes {
            let sign = if o < 0 { '-' } else { '+' };
            s += &format!("{sign}{:02}:{:02}", o.abs() / 60, o.abs() % 60);
        }
        s
    }

    /// Seconds since 1970-01-01T00:00:00 of the civil time (UTC when the offset is known, else "local" seconds).
    pub fn unix_seconds(&self) -> i64 {
        // days from civil (proleptic Gregorian), Howard Hinnant's public-domain algorithm
        let (y, m, d) = (self.year as i64 - if self.month <= 2 { 1 } else { 0 }, self.month as i64, self.day as i64);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        days * 86400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64 - self.offset_minutes.unwrap_or(0) as i64 * 60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_forms() {
        let d = DateTime::parse_exif("2024:06:01 13:45:59").unwrap();
        assert_eq!((d.year, d.month, d.day, d.hour, d.minute, d.second), (2024, 6, 1, 13, 45, 59));
        assert_eq!(d.to_exif(), "2024:06:01 13:45:59");
        assert!(DateTime::parse_exif("0000:00:00 00:00:00").is_none());
        assert!(DateTime::parse_exif("    :  :     :  :  ").is_none());
        assert!(DateTime::parse_exif("2024:13:01 00:00:00").is_none());
        assert_eq!(DateTime::parse_exif("2024-06-01").unwrap().hour, 0);
        assert_eq!(DateTime::parse_exif("2024:06:01 13:45:59").unwrap().with_subsec("5").millis, 500);
        assert_eq!(DateTime::parse_exif("2024:06:01 13:45:59").unwrap().with_subsec("0123").millis, 12);
    }

    #[test]
    fn offsets() {
        assert_eq!(DateTime::parse_offset("+02:00"), Some(120));
        assert_eq!(DateTime::parse_offset("-05:30"), Some(-330));
        assert_eq!(DateTime::parse_offset("Z"), Some(0));
        assert_eq!(DateTime::parse_offset("+0930"), Some(570));
        assert_eq!(DateTime::parse_offset("   :  "), None);
        assert_eq!(DateTime::parse_offset("+99:00"), None);
    }

    #[test]
    fn unicode_offsets_are_rejected_without_panicking() {
        for offset in ["+1€", "-1€", "+€1", "-€1"] {
            assert_eq!(DateTime::parse_offset(offset), None);
            let date = format!("2024-06-01T13:45:59{offset}");
            let metadata = crate::Metadata { capture_time: DateTime::parse_iso("2024-06-01T13:45:59Z"), ..Default::default() };
            let xmp = crate::write_xmp(&metadata, None).replace("2024-06-01T13:45:59+00:00", &date);
            assert!(crate::parse_xmp(&xmp).unwrap().metadata.capture_time.is_none());
        }
    }

    #[test]
    fn iso_roundtrip() {
        for s in ["2024-06-01T13:45:59", "2024-06-01T13:45:59.250+02:00", "1999-12-31T23:59:00-05:30", "2024-02-29T00:00:00Z"] {
            let d = DateTime::parse_iso(s).unwrap();
            let again = DateTime::parse_iso(&d.to_iso()).unwrap();
            assert_eq!(d, again, "{s}");
        }
        assert_eq!(DateTime::parse_iso("2024-02-29T00:00:00Z").unwrap().to_iso(), "2024-02-29T00:00:00+00:00");
        let d = DateTime::parse_iso("2019").unwrap();
        assert_eq!((d.year, d.month, d.day), (2019, 1, 1));
        assert_eq!(DateTime::parse_iso("2019-07-04T10:11").unwrap().minute, 11);
        assert!(DateTime::parse_iso("nonsense").is_none());
        assert!(DateTime::parse_iso("2019-07-04Tab:cd").is_none());
    }

    #[test]
    fn unix() {
        assert_eq!(DateTime::parse_iso("1970-01-01T00:00:00Z").unwrap().unix_seconds(), 0);
        assert_eq!(DateTime::parse_iso("2000-03-01T00:00:00Z").unwrap().unix_seconds(), 951868800);
        assert_eq!(DateTime::parse_iso("2000-03-01T02:00:00+02:00").unwrap().unix_seconds(), 951868800);
    }
}
