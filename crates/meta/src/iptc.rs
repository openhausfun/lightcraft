//! IPTC-IIM (Information Interchange Model 4.2) — the record-2 datasets people still carry in JPEGs.
//!
//! Stream of `0x1C record dataset size(u16 BE, or extended) data` tags. Text is UTF-8 when the record-1
//! `CodedCharacterSet` (1:90) is `ESC % G`, otherwise decoded as UTF-8 when valid and Latin-1 otherwise.

use crate::{DateTime, Metadata};

fn decode(b: &[u8], utf8: bool) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.trim_end_matches('\0').trim().to_string(),
        Err(_) if utf8 => String::from_utf8_lossy(b).trim().to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect::<String>().trim().to_string(),
    }
}

/// Parse an IIM stream into the fields [`Metadata`] carries (title, keywords, artist, copyright, caption,
/// creation date/time).
pub fn parse_iptc(b: &[u8]) -> Metadata {
    let mut m = Metadata::default();
    let mut utf8 = false;
    let mut date: Option<String> = None;
    let mut time: Option<String> = None;
    let mut i = 0usize;
    while i + 5 <= b.len() {
        if b[i] != 0x1c {
            break;
        }
        let (rec, ds) = (b[i + 1], b[i + 2]);
        let mut size = u16::from_be_bytes([b[i + 3], b[i + 4]]) as usize;
        let mut at = i + 5;
        if size & 0x8000 != 0 {
            // extended dataset: next (size & 0x7fff) bytes hold the length
            let n = size & 0x7fff;
            if n == 0 || n > 4 || at + n > b.len() {
                break;
            }
            size = b[at..at + n].iter().fold(0usize, |a, &c| (a << 8) | c as usize);
            at += n;
        }
        let Some(data) = b.get(at..at.saturating_add(size)) else { break };
        match (rec, ds) {
            (1, 90) => utf8 = data == b"\x1b%G",
            (2, 5) => m.title = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 25) => {
                let k = decode(data, utf8);
                if !k.is_empty() && !m.keywords.contains(&k) {
                    m.keywords.push(k);
                }
            }
            (2, 80) => m.artist = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 116) => m.copyright = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 120) => m.caption = Some(decode(data, utf8)).filter(|s| !s.is_empty()),
            (2, 55) => date = Some(decode(data, utf8)),
            (2, 60) => time = Some(decode(data, utf8)),
            _ => {}
        }
        i = at + size;
    }
    if let Some(d) = date.filter(|d| d.len() == 8 && d.bytes().all(|c| c.is_ascii_digit())) {
        let mut s = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8]);
        if let Some(t) = time.filter(|t| t.is_ascii() && t.len() >= 6 && t[..6].bytes().all(|c| c.is_ascii_digit())) {
            s += &format!("T{}:{}:{}", &t[..2], &t[2..4], &t[4..6]);
            if t.len() >= 11 {
                s += &format!("{}:{}", &t[6..9], &t[9..11]);
            }
        }
        m.capture_time = DateTime::parse_iso(&s);
    }
    m
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn ds(out: &mut Vec<u8>, rec: u8, ds: u8, data: &[u8]) {
        out.extend_from_slice(&[0x1c, rec, ds]);
        out.extend_from_slice(&(data.len() as u16).to_be_bytes());
        out.extend_from_slice(data);
    }

    pub(crate) fn sample_iim() -> Vec<u8> {
        let mut v = Vec::new();
        ds(&mut v, 1, 90, b"\x1b%G");
        ds(&mut v, 2, 0, &[0, 4]);
        ds(&mut v, 2, 5, "Tïtle".as_bytes());
        ds(&mut v, 2, 25, b"alpha");
        ds(&mut v, 2, 25, b"beta");
        ds(&mut v, 2, 25, b"alpha");
        ds(&mut v, 2, 80, b"Photographer");
        ds(&mut v, 2, 116, b"CC0");
        ds(&mut v, 2, 120, b"A caption");
        ds(&mut v, 2, 55, b"20210304");
        ds(&mut v, 2, 60, b"101112+0100");
        v
    }

    #[test]
    fn parses_record2() {
        let m = parse_iptc(&sample_iim());
        assert_eq!(m.title.as_deref(), Some("Tïtle"));
        assert_eq!(m.keywords, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(m.artist.as_deref(), Some("Photographer"));
        assert_eq!(m.copyright.as_deref(), Some("CC0"));
        assert_eq!(m.caption.as_deref(), Some("A caption"));
        assert_eq!(m.capture_time.unwrap().to_iso(), "2021-03-04T10:11:12+01:00");
    }

    #[test]
    fn latin1_and_extended_and_garbage() {
        let mut v = Vec::new();
        ds(&mut v, 2, 5, &[0x43, 0xe9]); // "Cé" in Latin-1
        v.extend_from_slice(&[0x1c, 2, 120, 0x80, 0x02, 0x00, 0x03]);
        v.extend_from_slice(b"abc");
        let m = parse_iptc(&v);
        assert_eq!(m.title.as_deref(), Some("Cé"));
        assert_eq!(m.caption.as_deref(), Some("abc"));
        let s = sample_iim();
        for n in 0..s.len() {
            let _ = parse_iptc(&s[..n]);
        }
        assert_eq!(parse_iptc(&[0x1c, 2, 5, 0xff, 0xff]), Metadata::default());
    }
}
