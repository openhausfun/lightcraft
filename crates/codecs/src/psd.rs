//! Photoshop PSD/PSB merged-composite reader (clean-room, from the public file-format description):
//! 8/16/32-bit gray, RGB, CMYK, indexed, duotone (as gray); raw or PackBits/RLE image data;
//! ICC (resource 1039), EXIF (1058), XMP (1060); transparency when the layer count is negative.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::{DecodeOptions, Decoded, Error, Format, Result};

const F: Format = Format::Psd;

fn err(s: &str) -> Error {
    Error::Malformed(F, s.to_string())
}

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.p.checked_add(n).filter(|&e| e <= self.b.len()).ok_or_else(|| err("truncated"))?;
        let s = &self.b[self.p..end];
        self.p = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let s = self.take(8)?;
        Ok(u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
    }
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut r = Rd { b: bytes, p: 0 };
    if r.take(4)? != b"8BPS" {
        return Err(err("bad signature"));
    }
    let version = r.u16()?;
    let psb = match version {
        1 => false,
        2 => true,
        _ => return Err(err("bad version")),
    };
    r.take(6)?;
    let channels = r.u16()? as usize;
    let height = r.u32()? as usize;
    let width = r.u32()? as usize;
    let depth = r.u16()?;
    let mode = r.u16()?;
    check_size(F, width as u64, height as u64, opts)?;
    if channels == 0 || channels > 56 {
        return Err(err("bad channel count"));
    }

    // Colour mode data (palette for indexed).
    let cm_len = r.u32()? as usize;
    let color_mode_data = r.take(cm_len)?;

    // Image resources.
    let res_len = r.u32()? as usize;
    let res = r.take(res_len)?;
    let (icc, exif, xmp) = resources(res);

    // Layer and mask information: only the layer count sign matters (merged transparency).
    let lm_len = if psb { r.u64()? as usize } else { r.u32()? as usize };
    let lm = r.take(lm_len)?;
    let merged_transparency = if lm.len() >= if psb { 10 } else { 6 } {
        let off = if psb { 8 } else { 4 };
        let count = i16::from_be_bytes([lm[off], lm[off + 1]]);
        count < 0
    } else {
        false
    };

    // Image data.
    let compression = r.u16()?;
    let bpc = match depth {
        8 => 1usize,
        16 => 2,
        32 => 4,
        1 => return Err(Error::Unsupported(F, "1-bit bitmap PSD")),
        _ => return Err(err("bad depth")),
    };
    let plane = width * height * bpc;
    let row_bytes = width * bpc;
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(channels);
    match compression {
        0 => {
            for _ in 0..channels {
                planes.push(r.take(plane)?.to_vec());
            }
        }
        1 => {
            let counts_len = channels * height;
            let mut counts = Vec::with_capacity(counts_len);
            for _ in 0..counts_len {
                counts.push(if psb { r.u32()? as usize } else { r.u16()? as usize });
            }
            for c in 0..channels {
                let mut out = Vec::with_capacity(plane);
                for y in 0..height {
                    let src = r.take(counts[c * height + y])?;
                    unpackbits(src, row_bytes, &mut out);
                }
                planes.push(out);
            }
        }
        _ => return Err(Error::Unsupported(F, "ZIP-compressed merged image data")),
    }

    let (model, n_color) = match mode {
        1 | 8 => (Model::Gray, 1),
        2 => (Model::Rgb, 1),
        3 => (Model::Rgb, 3),
        4 => (Model::Cmyk, 4),
        7 => (Model::Gray, 1),
        9 => return Err(Error::Unsupported(F, "Lab PSD")),
        _ => return Err(err("unknown colour mode")),
    };
    if channels < n_color {
        return Err(err("too few channels"));
    }
    let alpha = merged_transparency && channels > n_color && mode != 2;
    let used = n_color + alpha as usize;
    let n = width * height;

    // Interleave to device samples.
    let buf = if mode == 2 {
        // Indexed → 8-bit RGB via the 768-byte palette.
        if color_mode_data.len() < 768 || depth != 8 {
            return Err(err("bad indexed palette"));
        }
        let mut v = Vec::with_capacity(n * 3);
        for &i in &planes[0] {
            let i = i as usize;
            v.extend_from_slice(&[color_mode_data[i], color_mode_data[256 + i], color_mode_data[512 + i]]);
        }
        Buf::U8(v)
    } else {
        match bpc {
            1 => {
                let mut v = vec![0u8; n * used];
                for (c, p) in planes.iter().take(used).enumerate() {
                    for i in 0..n {
                        let x = p[i];
                        // PSD CMYK stores 255 = no ink; our CMYK convention is 0 = no ink.
                        v[i * used + c] = if model == Model::Cmyk && c < 4 { 255 - x } else { x };
                    }
                }
                Buf::U8(v)
            }
            2 => {
                let mut v = vec![0u16; n * used];
                for (c, p) in planes.iter().take(used).enumerate() {
                    for i in 0..n {
                        let x = u16::from_be_bytes([p[2 * i], p[2 * i + 1]]);
                        v[i * used + c] = if model == Model::Cmyk && c < 4 { 65535 - x } else { x };
                    }
                }
                Buf::U16(v)
            }
            _ => {
                let mut v = vec![0f32; n * used];
                for (c, p) in planes.iter().take(used).enumerate() {
                    for i in 0..n {
                        let x = f32::from_be_bytes([p[4 * i], p[4 * i + 1], p[4 * i + 2], p[4 * i + 3]]);
                        v[i * used + c] = if model == Model::Cmyk && c < 4 { 1.0 - x } else { x };
                    }
                }
                Buf::F32(v)
            }
        }
    };
    let raw = Raw { width, height, model, alpha, premultiplied: false, buf, bit_depth: depth as u8 };
    finish(F, raw, Meta { icc, exif, xmp, ..Default::default() }, (width as u32, height as u32), opts)
}

/// PackBits: append exactly `row_bytes` bytes (zero-padded if the run data is short).
fn unpackbits(mut src: &[u8], row_bytes: usize, out: &mut Vec<u8>) {
    let target = out.len() + row_bytes;
    while out.len() < target && !src.is_empty() {
        let h = src[0] as i8;
        src = &src[1..];
        if h >= 0 {
            let n = (h as usize + 1).min(src.len()).min(target - out.len());
            out.extend_from_slice(&src[..n]);
            src = &src[(h as usize + 1).min(src.len())..];
        } else if h != -128 {
            let n = ((1 - h as isize) as usize).min(target - out.len());
            if let Some(&b) = src.first() {
                out.extend(std::iter::repeat_n(b, n));
                src = &src[1..];
            }
        }
    }
    out.resize(target, 0);
}

fn resources(mut b: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>, Option<String>) {
    let (mut icc, mut exif, mut xmp) = (None, None, None);
    while b.len() >= 12 && &b[0..4] == b"8BIM" {
        let id = u16::from_be_bytes([b[4], b[5]]);
        let name_len = b[6] as usize;
        let name_total = (1 + name_len + 1) & !1;
        let p = 6 + name_total;
        if p + 4 > b.len() {
            break;
        }
        let size = u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]) as usize;
        let start = p + 4;
        let Some(data) = start.checked_add(size).and_then(|end| b.get(start..end)) else { break };
        match id {
            1039 => icc = Some(data.to_vec()),
            1058 => exif = Some(if data.starts_with(b"Exif\0\0") { data[6..].to_vec() } else { data.to_vec() }),
            1060 => xmp = Some(String::from_utf8_lossy(data).trim_end_matches('\0').to_string()),
            _ => {}
        }
        let next = start + ((size + 1) & !1);
        b = b.get(next..).unwrap_or(&[]);
    }
    (icc, exif, xmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits() {
        let mut out = Vec::new();
        unpackbits(&[0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A], 6, &mut out);
        assert_eq!(out, vec![0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A]);
        let mut out = Vec::new();
        unpackbits(&[0x05, 1], 4, &mut out);
        assert_eq!(out, vec![1, 0, 0, 0]);
    }
}
