//! Lossless JPEG ("LJ92"): ITU-T T.81 / ISO 10918-1 process 14 (Annex H), Huffman-coded, as used by DNG,
//! Canon CR2 and Sony lossless ARW.
//!
//! - All seven predictors, 2–16 bit precision, 1–4 interleaved components (1×1 sampling), point transform,
//!   restart intervals, `SSSS = 16` differences.
//! - [`decode`] returns the frame as interleaved samples in raster order (`width × height × components`),
//!   which callers re-tile (DNG tiles, CR2 slices).
//! - [`encode`] writes a conforming stream with per-component optimal Huffman tables built with the
//!   T.81 Annex K.2 procedure — used by tests (bit-exact round trips) and later by DNG export.

use crate::RawError;

/// A decoded lossless-JPEG frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub components: usize,
    pub precision: u8,
    pub predictor: u8,
    pub point_transform: u8,
    /// `width × height × components` interleaved samples.
    pub data: Vec<u16>,
}

fn err(s: &str) -> RawError {
    RawError::Corrupt(format!("lossless JPEG: {s}"))
}

/// Huffman decoding table (T.81 Annex C "BITS"/"HUFFVAL").
#[derive(Clone, Debug)]
pub struct Huffman {
    /// For peeks of `LOOKUP` bits: (code length, symbol); length 0 = slow path.
    fast: Vec<(u8, u8)>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: Vec<u8>,
}

const LOOKUP: u32 = 10;

impl Huffman {
    /// Build from the 16 code-length counts and the symbol list.
    pub fn new(counts: &[u8; 16], values: &[u8]) -> Result<Huffman, RawError> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total > values.len() || total > 256 || total == 0 {
            return Err(err("bad Huffman table"));
        }
        let mut maxcode = [-1i32; 18];
        let mut valptr = [0i32; 17];
        let mut mincode = [0i32; 17];
        let mut fast = vec![(0u8, 0u8); 1 << LOOKUP];
        let mut code = 0i32;
        let mut k = 0usize;
        for len in 1..=16usize {
            let n = counts[len - 1] as i32;
            valptr[len] = k as i32;
            mincode[len] = code;
            for _ in 0..n {
                if len as u32 <= LOOKUP {
                    let shift = LOOKUP - len as u32;
                    let base = (code as usize) << shift;
                    for i in 0..(1usize << shift) {
                        if let Some(slot) = fast.get_mut(base + i) {
                            *slot = (len as u8, values[k]);
                        }
                    }
                }
                code += 1;
                k += 1;
            }
            maxcode[len] = if n > 0 { code - 1 } else { -1 };
            if code > (1 << len) {
                return Err(err("over-subscribed Huffman table"));
            }
            code <<= 1;
        }
        maxcode[17] = i32::MAX;
        Ok(Huffman { fast, maxcode, valptr, mincode, values: values[..total].to_vec() })
    }

    #[inline]
    pub fn decode(&self, br: &mut BitReader) -> Result<u8, RawError> {
        let peek = br.peek(LOOKUP);
        let (len, sym) = self.fast[peek as usize];
        if len > 0 {
            br.consume(len as u32);
            return Ok(sym);
        }
        let mut code = br.peek(LOOKUP + 1) as i32;
        let mut len = LOOKUP as usize + 1;
        while len <= 16 && code > self.maxcode[len] {
            len += 1;
            code = br.peek(len as u32) as i32;
        }
        if len > 16 {
            return Err(err("invalid Huffman code"));
        }
        br.consume(len as u32);
        let idx = self.valptr[len] + code - self.mincode[len];
        self.values.get(idx as usize).copied().ok_or_else(|| err("invalid Huffman code"))
    }
}

/// MSB-first bit reader over JPEG entropy-coded data (handles `FF 00` stuffing; stops at markers, feeding zeros).
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    bits: u32,
    /// A marker (`FF xx`, xx ≠ 0) was reached.
    pub marker: Option<u8>,
    /// Bits fed past the end of data (for corruption detection).
    pub overrun: u32,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0, buf: 0, bits: 0, marker: None, overrun: 0 }
    }
    #[inline]
    fn fill(&mut self) {
        while self.bits <= 56 {
            let byte = if self.marker.is_some() || self.pos >= self.data.len() {
                self.overrun += 8;
                0
            } else {
                let b = self.data[self.pos];
                if b == 0xff {
                    let next = self.data.get(self.pos + 1).copied().unwrap_or(0);
                    if next == 0 {
                        self.pos += 2;
                        0xff
                    } else {
                        self.marker = Some(next);
                        self.overrun += 8;
                        0
                    }
                } else {
                    self.pos += 1;
                    b
                }
            };
            self.buf |= (byte as u64) << (56 - self.bits);
            self.bits += 8;
        }
    }
    #[inline]
    pub fn peek(&mut self, n: u32) -> u32 {
        if self.bits < n {
            self.fill();
        }
        (self.buf >> (64 - n)) as u32
    }
    #[inline]
    pub fn consume(&mut self, n: u32) {
        self.buf <<= n;
        self.bits -= n;
    }
    #[inline]
    pub fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.consume(n);
        v
    }
    /// Discard buffered bits and skip to just after the next `RSTn` marker. Returns false if none found.
    fn restart(&mut self) -> bool {
        self.buf = 0;
        self.bits = 0;
        self.marker = None;
        self.overrun = 0;
        while self.pos + 1 < self.data.len() {
            if self.data[self.pos] == 0xff && (0xd0..=0xd7).contains(&self.data[self.pos + 1]) {
                self.pos += 2;
                return true;
            }
            self.pos += 1;
        }
        false
    }
    /// Bytes consumed so far (approximate while bits are buffered).
    pub fn position(&self) -> usize {
        self.pos
    }
}

/// Difference value for category `ssss` (T.81 Table H.2 / F.12 EXTEND).
#[inline]
pub fn diff_value(br: &mut BitReader, ssss: u8) -> i32 {
    match ssss {
        0 => 0,
        16 => 32768,
        s => {
            let s = s.min(16) as u32;
            let v = br.get(s) as i32;
            if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
        }
    }
}

struct Header {
    precision: u8,
    height: usize,
    width: usize,
    comps: Vec<u8>,
    tables: [Option<Huffman>; 4],
    table_for: Vec<usize>,
    predictor: u8,
    pt: u8,
    restart: usize,
    scan_start: usize,
}

fn be16(d: &[u8], i: usize) -> Result<usize, RawError> {
    d.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as usize).ok_or_else(|| err("truncated header"))
}

fn parse_header(d: &[u8]) -> Result<Header, RawError> {
    if d.len() < 4 || d[0] != 0xff || d[1] != 0xd8 {
        return Err(err("missing SOI"));
    }
    let mut h = Header {
        precision: 0,
        height: 0,
        width: 0,
        comps: vec![],
        tables: [None, None, None, None],
        table_for: vec![],
        predictor: 1,
        pt: 0,
        restart: 0,
        scan_start: 0,
    };
    let mut i = 2;
    let mut have_sof = false;
    loop {
        // skip fill bytes / stray data up to the next marker
        while i < d.len() && d[i] != 0xff {
            i += 1;
        }
        while i < d.len() && d[i] == 0xff {
            i += 1;
        }
        let Some(&m) = d.get(i) else { return Err(err("no scan")) };
        i += 1;
        if m == 0xd8 || (0xd0..=0xd7).contains(&m) {
            continue;
        }
        if m == 0xd9 {
            return Err(err("EOI before scan"));
        }
        let len = be16(d, i)?;
        if len < 2 || i + len > d.len() {
            return Err(err("truncated segment"));
        }
        let seg = &d[i + 2..i + len];
        match m {
            0xc3 => {
                if seg.len() < 6 {
                    return Err(err("short SOF3"));
                }
                h.precision = seg[0];
                h.height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                h.width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                let n = seg[5] as usize;
                if n == 0 || n > 4 || seg.len() < 6 + 3 * n {
                    return Err(err("bad component count"));
                }
                for c in 0..n {
                    let (id, samp) = (seg[6 + 3 * c], seg[7 + 3 * c]);
                    if samp != 0x11 {
                        return Err(RawError::Unsupported("lossless JPEG with subsampled components".into()));
                    }
                    h.comps.push(id);
                }
                if !(2..=16).contains(&h.precision) {
                    return Err(err("bad precision"));
                }
                have_sof = true;
            }
            0xc0..=0xc2 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(RawError::Unsupported(format!("JPEG process SOF{:X} (not lossless Huffman)", m - 0xc0)));
            }
            0xc4 => {
                let mut j = 0;
                while j + 17 <= seg.len() {
                    let (class, id) = (seg[j] >> 4, (seg[j] & 15) as usize);
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[j + 1..j + 17]);
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let vals = seg.get(j + 17..j + 17 + n).ok_or_else(|| err("truncated DHT"))?;
                    if class == 0 && id < 4 {
                        h.tables[id] = Some(Huffman::new(&counts, vals)?);
                    }
                    j += 17 + n;
                }
            }
            0xdd => h.restart = be16(seg, 0)?,
            0xda => {
                if !have_sof {
                    return Err(err("SOS before SOF3"));
                }
                let ns = *seg.first().ok_or_else(|| err("short SOS"))? as usize;
                if ns != h.comps.len() || seg.len() < 1 + 2 * ns + 3 {
                    return Err(RawError::Unsupported("lossless JPEG with multiple scans".into()));
                }
                for c in 0..ns {
                    let cid = seg[1 + 2 * c];
                    let pos = h.comps.iter().position(|&x| x == cid).ok_or_else(|| err("unknown scan component"))?;
                    if pos != c {
                        return Err(err("scan component order"));
                    }
                    let td = (seg[2 + 2 * c] >> 4) as usize;
                    if td > 3 || h.tables[td].is_none() {
                        return Err(err("missing Huffman table"));
                    }
                    h.table_for.push(td);
                }
                h.predictor = seg[1 + 2 * ns];
                h.pt = seg[3 + 2 * ns] & 15;
                if !(0..=7).contains(&h.predictor) {
                    return Err(err("bad predictor"));
                }
                if h.pt >= h.precision {
                    return Err(err("bad point transform"));
                }
                h.scan_start = i + len;
                return Ok(h);
            }
            _ => {}
        }
        i += len;
    }
}

/// Parse only the frame header: (width, height, components, precision).
pub fn frame_info(d: &[u8]) -> Result<(usize, usize, usize, u8), RawError> {
    let h = parse_header(d)?;
    Ok((h.width, h.height, h.comps.len(), h.precision))
}

/// Decode a lossless JPEG stream. `max_samples` bounds the allocation (hostile headers).
pub fn decode(d: &[u8], max_samples: usize) -> Result<Frame, RawError> {
    let h = parse_header(d)?;
    let (w, ht, nc) = (h.width, h.height, h.comps.len());
    if w == 0 || ht == 0 {
        return Err(err("zero frame size"));
    }
    let total = w.checked_mul(ht).and_then(|v| v.checked_mul(nc)).ok_or_else(|| err("frame too large"))?;
    if total > max_samples {
        return Err(RawError::Limit("lossless JPEG frame larger than expected"));
    }
    // The entropy data must hold at least ~1 bit per sample; reject absurd headers early.
    let entropy = &d[h.scan_start..];
    if (entropy.len() as u64 + 64) * 8 < total as u64 {
        return Err(err("entropy data too short for frame"));
    }
    let tables: Vec<&Huffman> = h
        .table_for
        .iter()
        .map(|&t| h.tables.get(t).and_then(Option::as_ref).ok_or_else(|| err("scan uses an undefined Huffman table")))
        .collect::<Result<_, RawError>>()?;
    let mut out = vec![0u16; total];
    let mut br = BitReader::new(entropy);
    let init = 1i32 << (h.precision - h.pt - 1);
    let mask = 0xffffi32;
    let row_len = w * nc;
    let pred = h.predictor;
    let mut restart_left = h.restart;
    let mut first_line = true; // first line of image or restart interval
    let mut reset_pending = true;
    for y in 0..ht {
        let (prev_rows, cur_rows) = out.split_at_mut(y * row_len);
        let cur = &mut cur_rows[..row_len];
        let prev = if y > 0 { &prev_rows[(y - 1) * row_len..] } else { &[][..] };
        for x in 0..w {
            if h.restart > 0 {
                if restart_left == 0 {
                    if !br.restart() {
                        return Err(err("missing restart marker"));
                    }
                    restart_left = h.restart;
                    first_line = true;
                    reset_pending = true;
                }
                restart_left -= 1;
            }
            for c in 0..nc {
                let i = x * nc + c;
                let p = if reset_pending {
                    init
                } else if first_line {
                    if x == 0 { prev.get(i).map_or(init, |&v| v as i32) } else { cur[i - nc] as i32 }
                } else if x == 0 || pred == 0 {
                    if pred == 0 { 0 } else { prev[i] as i32 }
                } else {
                    let ra = cur[i - nc] as i32;
                    let rb = prev[i] as i32;
                    let rc = prev[i - nc] as i32;
                    match pred {
                        1 => ra,
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        _ => (ra + rb) >> 1,
                    }
                };
                let ssss = tables[c].decode(&mut br)?;
                let diff = diff_value(&mut br, ssss);
                cur[i] = ((p + diff) & mask) as u16;
            }
            reset_pending = false;
        }
        first_line = false;
        if br.overrun > 64 * 8 {
            return Err(err("entropy data exhausted"));
        }
    }
    if h.pt > 0 {
        for v in &mut out {
            *v <<= h.pt;
        }
    }
    Ok(Frame { width: w, height: ht, components: nc, precision: h.precision, predictor: pred, point_transform: h.pt, data: out })
}

// ------------------------------------------------------------------------------------------------ encoder

/// Build Huffman code lengths from frequencies (T.81 Annex K.2, Figures K.1–K.3), limited to 16 bits.
fn code_lengths(freq_in: &[u32; 17]) -> ([u8; 16], Vec<u8>) {
    let mut freq = [0i64; 18];
    for (i, &f) in freq_in.iter().enumerate() {
        freq[i] = f as i64;
    }
    freq[17] = 1; // reserved symbol so no code is all ones
    let mut codesize = [0usize; 18];
    let mut others = [-1i32; 18];
    loop {
        // V1: least frequency > 0 (largest index on ties), V2: next least
        let mut v1 = -1i32;
        let mut v2 = -1i32;
        for i in 0..18 {
            if freq[i] > 0 && (v1 < 0 || freq[i] <= freq[v1 as usize]) {
                v1 = i as i32;
            }
        }
        for i in 0..18 {
            if freq[i] > 0 && i as i32 != v1 && (v2 < 0 || freq[i] <= freq[v2 as usize]) {
                v2 = i as i32;
            }
        }
        if v2 < 0 {
            break;
        }
        let (a, b) = (v1 as usize, v2 as usize);
        freq[a] += freq[b];
        freq[b] = 0;
        let mut x = a;
        codesize[x] += 1;
        while others[x] >= 0 {
            x = others[x] as usize;
            codesize[x] += 1;
        }
        others[x] = b as i32;
        let mut x = b;
        codesize[x] += 1;
        while others[x] >= 0 {
            x = others[x] as usize;
            codesize[x] += 1;
        }
    }
    let mut bits = [0i32; 33];
    for &cs in &codesize {
        if cs > 0 {
            bits[cs.min(32)] += 1;
        }
    }
    // Adjust_BITS (Figure K.3)
    let mut i = 32;
    while i > 16 {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
        i -= 1;
    }
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1; // remove the reserved code
    let mut counts = [0u8; 16];
    for l in 1..=16 {
        counts[l - 1] = bits[l] as u8;
    }
    // Sort_input (Figure K.4): symbols by code size
    let mut values = Vec::new();
    for l in 1..=32 {
        for (s, &cs) in codesize.iter().enumerate().take(17) {
            if cs == l {
                values.push(s as u8);
            }
        }
    }
    (counts, values)
}

/// Canonical codes for a table: symbol → (code, length).
fn canonical_codes(counts: &[u8; 16], values: &[u8]) -> [(u32, u8); 17] {
    let mut table = [(0u32, 0u8); 17];
    let mut code = 0u32;
    let mut k = 0;
    for len in 1..=16u8 {
        for _ in 0..counts[len as usize - 1] {
            if let Some(&s) = values.get(k)
                && (s as usize) < 17
            {
                table[s as usize] = (code, len);
            }
            code += 1;
            k += 1;
        }
        code <<= 1;
    }
    table
}

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn put(&mut self, v: u32, bits: u32) {
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (v as u64 & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            let b = (self.acc >> (self.n - 8)) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
            self.n -= 8;
        }
    }
    fn flush(&mut self) {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
    }
}

fn ssss_of(diff: i32) -> u8 {
    if diff == 0 {
        0
    } else if diff == 32768 || diff == -32768 {
        16
    } else {
        32 - (diff.unsigned_abs()).leading_zeros() as u8
    }
}

/// Encode samples (`width × height × components`, interleaved) as a lossless JPEG with `predictor` (1–7) and
/// `precision` bits. `restart` > 0 inserts restart markers every `restart` MCUs (pixels).
pub fn encode(data: &[u16], width: usize, height: usize, components: usize, precision: u8, predictor: u8, restart: usize) -> Vec<u8> {
    assert!((1..=4).contains(&components) && (2..=16).contains(&precision) && (1..=7).contains(&predictor));
    assert_eq!(data.len(), width * height * components);
    let init = 1i32 << (precision - 1);
    let row_len = width * components;
    // compute differences in the same order as the decoder
    let mut diffs = vec![0i32; data.len()];
    let mut restart_left = restart;
    let mut first_line = true;
    let mut reset_pending = true;
    let mut marks = Vec::new(); // sample index where an RST marker precedes
    for y in 0..height {
        for x in 0..width {
            if restart > 0 {
                if restart_left == 0 {
                    marks.push((y * width + x) * components);
                    restart_left = restart;
                    first_line = true;
                    reset_pending = true;
                }
                restart_left -= 1;
            }
            for c in 0..components {
                let i = y * row_len + x * components + c;
                let p = if reset_pending {
                    init
                } else if first_line {
                    if x == 0 { data[i - row_len] as i32 } else { data[i - components] as i32 }
                } else if x == 0 {
                    data[i - row_len] as i32
                } else {
                    let ra = data[i - components] as i32;
                    let rb = data[i - row_len] as i32;
                    let rc = data[i - row_len - components] as i32;
                    match predictor {
                        1 => ra,
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        _ => (ra + rb) >> 1,
                    }
                };
                // modulo 2^16 difference mapped into [-32767, 32768]
                let mut d = (data[i] as i32 - p) & 0xffff;
                if d > 32768 {
                    d -= 65536;
                }
                diffs[i] = d;
            }
            reset_pending = false;
        }
        first_line = false;
    }
    let mut tables = Vec::new();
    for c in 0..components {
        let mut freq = [0u32; 17];
        for d in diffs.iter().skip(c).step_by(components) {
            freq[ssss_of(*d) as usize] += 1;
        }
        let (counts, values) = code_lengths(&freq);
        tables.push((counts, values));
    }
    let mut out = vec![0xff, 0xd8];
    // SOF3
    out.extend_from_slice(&[0xff, 0xc3]);
    out.extend_from_slice(&((8 + 3 * components) as u16).to_be_bytes());
    out.push(precision);
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.push(components as u8);
    for c in 0..components {
        out.extend_from_slice(&[c as u8 + 1, 0x11, 0]);
    }
    // DHT
    for (c, (counts, values)) in tables.iter().enumerate() {
        out.extend_from_slice(&[0xff, 0xc4]);
        out.extend_from_slice(&((2 + 17 + values.len()) as u16).to_be_bytes());
        out.push(c as u8);
        out.extend_from_slice(counts);
        out.extend_from_slice(values);
    }
    if restart > 0 {
        out.extend_from_slice(&[0xff, 0xdd, 0, 4]);
        out.extend_from_slice(&(restart as u16).to_be_bytes());
    }
    // SOS
    out.extend_from_slice(&[0xff, 0xda]);
    out.extend_from_slice(&((6 + 2 * components) as u16).to_be_bytes());
    out.push(components as u8);
    for c in 0..components {
        out.extend_from_slice(&[c as u8 + 1, (c as u8) << 4]);
    }
    out.extend_from_slice(&[predictor, 0, 0]);
    let codes: Vec<[(u32, u8); 17]> = tables.iter().map(|(c, v)| canonical_codes(c, v)).collect();
    let mut bw = BitWriter { out, acc: 0, n: 0 };
    let mut mark_iter = marks.iter().peekable();
    let mut rst = 0u8;
    for (i, &d) in diffs.iter().enumerate() {
        if mark_iter.peek().is_some_and(|&&m| m == i) {
            mark_iter.next();
            bw.flush();
            bw.out.extend_from_slice(&[0xff, 0xd0 + rst]);
            rst = (rst + 1) & 7;
            bw.acc = 0;
        }
        let c = i % components;
        let s = ssss_of(d);
        let (code, len) = codes[c][s as usize];
        bw.put(code, len as u32);
        if s > 0 && s < 16 {
            let v = if d < 0 { d - 1 } else { d };
            bw.put(v as u32 & ((1 << s) - 1), s as u32);
        }
    }
    bw.flush();
    let mut out = bw.out;
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, bits: u32, seed: u64) -> Vec<u16> {
        let mut s = seed;
        (0..n)
            .map(|i| {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let smooth = ((i % 97) as u32 * 37) % (1 << bits);
                let jitter = ((s >> 33) as u32) % 64;
                ((smooth + jitter) % (1 << bits)) as u16
            })
            .collect()
    }

    #[test]
    fn roundtrip_all_predictors_components_precisions() {
        for pred in 1..=7u8 {
            for comps in 1..=4usize {
                for bits in [8u8, 12, 14, 16] {
                    let (w, h) = (13, 7);
                    let data = noise(w * h * comps, bits as u32, (pred as u64) * 31 + comps as u64);
                    let enc = encode(&data, w, h, comps, bits, pred, 0);
                    let f = decode(&enc, usize::MAX).unwrap();
                    assert_eq!((f.width, f.height, f.components, f.precision, f.predictor), (w, h, comps, bits, pred));
                    assert_eq!(f.data, data, "pred {pred} comps {comps} bits {bits}");
                }
            }
        }
    }

    #[test]
    fn roundtrip_extremes_and_restarts() {
        // alternating 0 / 65535 forces SSSS = 16 differences
        let data: Vec<u16> = (0..64).map(|i| if i % 2 == 0 { 0 } else { 65535 }).collect();
        for pred in 1..=7 {
            let enc = encode(&data, 8, 8, 1, 16, pred, 0);
            assert_eq!(decode(&enc, usize::MAX).unwrap().data, data);
        }
        let data = noise(20 * 9 * 2, 12, 5);
        for rst in [1usize, 7, 20, 40] {
            let enc = encode(&data, 20, 9, 2, 12, 6, rst);
            assert_eq!(decode(&enc, usize::MAX).unwrap().data, data, "restart {rst}");
        }
    }

    #[test]
    fn constant_image_single_symbol_table() {
        let data = vec![1234u16; 16 * 4];
        let enc = encode(&data, 16, 4, 1, 14, 1, 0);
        assert_eq!(decode(&enc, usize::MAX).unwrap().data, data);
    }

    #[test]
    fn rejects_bad_streams() {
        assert!(decode(&[], 100).is_err());
        assert!(decode(&[0xff, 0xd8, 0xff, 0xd9], 100).is_err());
        let data = noise(64, 12, 1);
        let enc = encode(&data, 8, 8, 1, 12, 1, 0);
        assert!(matches!(decode(&enc, 10), Err(RawError::Limit(_))));
        for n in 0..enc.len() {
            let _ = decode(&enc[..n], usize::MAX);
        }
        let mut bad = enc.clone();
        for i in (20..bad.len()).step_by(3) {
            bad[i] ^= 0x5a;
        }
        let _ = decode(&bad, usize::MAX);
    }

    #[test]
    fn code_lengths_are_limited_to_16() {
        // Fibonacci-like frequencies produce very long codes before adjustment
        let mut f = [0u32; 17];
        let (mut a, mut b) = (1u32, 1u32);
        for x in f.iter_mut() {
            *x = a;
            let c = a.saturating_add(b);
            a = b;
            b = c;
        }
        let (counts, values) = code_lengths(&f);
        assert_eq!(counts.iter().map(|&c| c as usize).sum::<usize>(), 17);
        assert_eq!(values.len(), 17);
        assert!(Huffman::new(&counts, &values).is_ok());
    }
}
