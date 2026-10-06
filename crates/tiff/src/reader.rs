//! Bounds-checked IFD parsing.

use crate::{ByteOrder, Entry, FieldType, Ifd, Result, Tiff, TiffError, Value, tags};
use std::collections::HashSet;

/// Limits applied while parsing. The defaults are generous for real files and safe for hostile ones.
#[derive(Clone, Debug)]
pub struct ParseOptions {
    /// Maximum number of IFDs visited in total (chain + children).
    pub max_ifds: usize,
    /// Maximum nesting depth of child IFDs.
    pub max_depth: usize,
    /// Maximum number of entries read from one IFD.
    pub max_entries: usize,
    /// Maximum total bytes of decoded values; `None` = `8 × input length + 1 MiB`.
    pub value_budget: Option<u64>,
    /// Follow `SubIFDs` / Exif / GPS / Interop pointers.
    pub follow_children: bool,
    /// Accept the TIFF-like magic numbers used by some raw formats (ORF `IIRO`/`IIRS`, RW2 `IIU`).
    pub accept_raw_magic: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self { max_ifds: 1024, max_depth: 10, max_entries: 8192, value_budget: None, follow_children: true, accept_raw_magic: true }
    }
}

/// Parse the header: (byte order, bigtiff, first IFD offset).
pub(crate) fn header(data: &[u8]) -> Result<(ByteOrder, bool, u64)> {
    header_with(data, true)
}

fn header_with(data: &[u8], raw_magic: bool) -> Result<(ByteOrder, bool, u64)> {
    let order = match data.get(0..2) {
        Some(b"II") => ByteOrder::Little,
        Some(b"MM") => ByteOrder::Big,
        _ => return Err(TiffError::NotTiff),
    };
    let magic = order.read_u16(data, 2).ok_or(TiffError::NotTiff)?;
    match magic {
        42 => Ok((order, false, order.read_u32(data, 4).ok_or(TiffError::Truncated(4))? as u64)),
        43 => {
            let bytesize = order.read_u16(data, 4).ok_or(TiffError::Truncated(4))?;
            let zero = order.read_u16(data, 6).ok_or(TiffError::Truncated(6))?;
            if bytesize != 8 || zero != 0 {
                return Err(TiffError::NotTiff);
            }
            Ok((order, true, order.read_u64(data, 8).ok_or(TiffError::Truncated(8))?))
        }
        0x4f52 | 0x5352 | 0x0055 if raw_magic => Ok((order, false, order.read_u32(data, 4).ok_or(TiffError::Truncated(4))? as u64)),
        _ => Err(TiffError::NotTiff),
    }
}

pub(crate) fn parse(data: &[u8], opts: &ParseOptions) -> Result<Tiff> {
    let (order, big, first) = header_with(data, opts.accept_raw_magic)?;
    let mut ctx = Ctx::new(data, order, big, opts);
    let mut ifds = Vec::new();
    let mut off = first;
    while off != 0 {
        match ctx.ifd(off, 0, 0) {
            Ok((ifd, next)) => {
                ifds.push(ifd);
                off = next;
            }
            Err(e) => {
                if ifds.is_empty() {
                    return Err(e);
                }
                break;
            }
        }
    }
    if ifds.is_empty() {
        return Err(TiffError::Invalid("no IFDs".into()));
    }
    Ok(Tiff { order, bigtiff: big, ifds })
}

/// Parse one IFD at absolute `offset` whose value offsets (and child IFD pointers) are relative to
/// `base` (absolute position = `base + stored offset`). Returns the IFD and the absolute offset of the
/// next IFD (0 when there is none). Used for maker notes, which often use their own offset base.
pub fn parse_ifd_at(data: &[u8], offset: u64, order: ByteOrder, base: u64, bigtiff: bool, opts: &ParseOptions) -> Result<(Ifd, u64)> {
    let mut ctx = Ctx::new(data, order, bigtiff, opts);
    ctx.ifd(offset, base, 0)
}

struct Ctx<'a> {
    data: &'a [u8],
    order: ByteOrder,
    big: bool,
    opts: &'a ParseOptions,
    visited: HashSet<u64>,
    budget: u64,
}

impl<'a> Ctx<'a> {
    fn new(data: &'a [u8], order: ByteOrder, big: bool, opts: &'a ParseOptions) -> Self {
        let budget = opts.value_budget.unwrap_or((data.len() as u64).saturating_mul(8).saturating_add(1 << 20));
        Self { data, order, big, opts, visited: HashSet::new(), budget }
    }

    fn read_off(&self, pos: usize) -> Option<u64> {
        if self.big { self.order.read_u64(self.data, pos) } else { self.order.read_u32(self.data, pos).map(|v| v as u64) }
    }

    /// Returns (ifd, absolute next offset or 0).
    fn ifd(&mut self, offset: u64, base: u64, depth: usize) -> Result<(Ifd, u64)> {
        if depth > self.opts.max_depth {
            return Err(TiffError::Limit("IFD nesting depth"));
        }
        if self.visited.len() >= self.opts.max_ifds {
            return Err(TiffError::Limit("IFD count"));
        }
        if !self.visited.insert(offset) {
            return Err(TiffError::Invalid(format!("IFD loop at {offset}")));
        }
        let pos = usize::try_from(offset).map_err(|_| TiffError::Truncated(offset))?;
        let (n, head, entry_size, inline) = if self.big {
            (self.order.read_u64(self.data, pos).ok_or(TiffError::Truncated(offset))?, 8usize, 20usize, 8usize)
        } else {
            (self.order.read_u16(self.data, pos).ok_or(TiffError::Truncated(offset))? as u64, 2, 12, 4)
        };
        let table = pos + head;
        let fit = (self.data.len().saturating_sub(table) / entry_size) as u64;
        let n_read = n.min(fit).min(self.opts.max_entries as u64) as usize;
        if n == 0 {
            return Err(TiffError::Invalid(format!("empty IFD at {offset}")));
        }
        if n_read == 0 {
            return Err(TiffError::Truncated(offset));
        }
        let mut entries = Vec::with_capacity(n_read);
        for i in 0..n_read {
            let p = table + i * entry_size;
            if let Some(e) = self.entry(p, base, inline) {
                entries.push(e);
            }
        }
        entries.sort_by_key(|e| e.tag);
        entries.dedup_by_key(|e| e.tag);
        let next_pos = table + (n as usize).saturating_mul(entry_size);
        let next = if n as usize == n_read {
            match self.read_off(next_pos) {
                Some(0) | None => 0,
                Some(v) => base.checked_add(v).unwrap_or(0),
            }
        } else {
            0
        };
        let mut ifd = Ifd { offset, entries, ..Default::default() };
        if self.opts.follow_children {
            self.children(&mut ifd, base, depth);
        }
        Ok((ifd, next))
    }

    fn children(&mut self, ifd: &mut Ifd, base: u64, depth: usize) {
        if let Some(v) = ifd.value(tags::SUB_IFDS) {
            let offs = v.to_u64_vec();
            for o in offs {
                if let Some(abs) = base.checked_add(o)
                    && let Ok((sub, _)) = self.ifd(abs, base, depth + 1)
                {
                    ifd.sub_ifds.push(sub);
                }
            }
        }
        let child = |tag: u16, ctx: &mut Self| -> Option<Box<Ifd>> {
            let o = ifd.value(tag)?.get_u64(0)?;
            let abs = base.checked_add(o)?;
            ctx.ifd(abs, base, depth + 1).ok().map(|(i, _)| Box::new(i))
        };
        let exif = child(tags::EXIF_IFD, self);
        let gps = child(tags::GPS_IFD, self);
        let interop = child(tags::INTEROP_IFD, self);
        ifd.exif = exif;
        ifd.gps = gps;
        ifd.interop = interop;
    }

    fn entry(&mut self, p: usize, base: u64, inline: usize) -> Option<Entry> {
        let o = self.order;
        let tag = o.read_u16(self.data, p)?;
        let typ = FieldType::from_u16(o.read_u16(self.data, p + 2)?)?;
        let (count, field) = if self.big { (o.read_u64(self.data, p + 4)?, p + 12) } else { (o.read_u32(self.data, p + 4)? as u64, p + 8) };
        let total = count.checked_mul(typ.size() as u64)?;
        let start = if total <= inline as u64 {
            field as u64
        } else {
            let rel = self.read_off(field)?;
            base.checked_add(rel)?
        };
        let end = start.checked_add(total)?;
        if end > self.data.len() as u64 {
            return None;
        }
        if total > self.budget {
            return None;
        }
        self.budget -= total;
        let bytes = &self.data[start as usize..end as usize];
        Some(Entry { tag, value: decode(o, typ, bytes), offset: start })
    }
}

/// Decode `bytes` (exactly `count × size` long) as values of type `typ`.
pub(crate) fn decode(o: ByteOrder, typ: FieldType, b: &[u8]) -> Value {
    fn arr<const N: usize>(c: &[u8]) -> [u8; N] {
        let mut a = [0u8; N];
        a.copy_from_slice(c);
        a
    }
    match typ {
        FieldType::Byte => Value::Byte(b.to_vec()),
        FieldType::Undefined => Value::Undefined(b.to_vec()),
        FieldType::Ascii => {
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            Value::Ascii(String::from_utf8_lossy(&b[..end]).into_owned())
        }
        FieldType::SByte => Value::SByte(b.iter().map(|&x| x as i8).collect()),
        FieldType::Short => Value::Short(b.as_chunks::<2>().0.iter().map(|c| o.u16(arr(c))).collect()),
        FieldType::SShort => Value::SShort(b.as_chunks::<2>().0.iter().map(|c| o.u16(arr(c)) as i16).collect()),
        FieldType::Long => Value::Long(b.as_chunks::<4>().0.iter().map(|c| o.u32(arr(c))).collect()),
        FieldType::Ifd => Value::Ifd(b.as_chunks::<4>().0.iter().map(|c| o.u32(arr(c))).collect()),
        FieldType::SLong => Value::SLong(b.as_chunks::<4>().0.iter().map(|c| o.u32(arr(c)) as i32).collect()),
        FieldType::Float => Value::Float(b.as_chunks::<4>().0.iter().map(|c| f32::from_bits(o.u32(arr(c)))).collect()),
        FieldType::Rational => Value::Rational(b.as_chunks::<8>().0.iter().map(|c| (o.u32(arr(&c[..4])), o.u32(arr(&c[4..])))).collect()),
        FieldType::SRational => {
            Value::SRational(b.as_chunks::<8>().0.iter().map(|c| (o.u32(arr(&c[..4])) as i32, o.u32(arr(&c[4..])) as i32)).collect())
        }
        FieldType::Double => Value::Double(b.as_chunks::<8>().0.iter().map(|c| f64::from_bits(o.u64(arr(c)))).collect()),
        FieldType::Long8 => Value::Long8(b.as_chunks::<8>().0.iter().map(|c| o.u64(arr(c))).collect()),
        FieldType::Ifd8 => Value::Ifd8(b.as_chunks::<8>().0.iter().map(|c| o.u64(arr(c))).collect()),
        FieldType::SLong8 => Value::SLong8(b.as_chunks::<8>().0.iter().map(|c| o.u64(arr(c)) as i64).collect()),
    }
}
