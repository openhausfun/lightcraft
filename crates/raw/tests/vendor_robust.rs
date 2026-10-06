//! Malformed vendor raw files (NEF, ARW, PEF, ORF, RW2, CR2, RAF) must decode to an error, never panic.

use lightcraft_tiff::tags as t;
use lightcraft_tiff::{ByteOrder, IfdBuilder, ImageData, TiffWriter, Value};
use proptest::prelude::*;

const W: u32 = 32;
const H: u32 = 8;

/// A CFA TIFF the way most vendors lay it out: IFD0 (make) + a SubIFD holding the raw strip.
fn cfa_tiff(make: &str, compression: u16, bits: u16, order: ByteOrder) -> Vec<u8> {
    let px = (W * H) as usize;
    // Sony's ARW2 (32767) packs a sample per byte
    let len = if compression == 32767 { px } else { px * 2 };
    let strip: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
    let mut raw = IfdBuilder::new();
    raw.set(t::NEW_SUBFILE_TYPE, Value::Long(vec![0]));
    raw.set(t::IMAGE_WIDTH, Value::Long(vec![W]));
    raw.set(t::IMAGE_LENGTH, Value::Long(vec![H]));
    raw.set(t::BITS_PER_SAMPLE, Value::Short(vec![bits]));
    raw.set(t::COMPRESSION, Value::Short(vec![compression]));
    raw.set(t::PHOTOMETRIC, Value::Short(vec![t::photometric::CFA]));
    raw.set(t::CFA_REPEAT_PATTERN_DIM, Value::Short(vec![2, 2]));
    raw.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
    raw.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![strip] });
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii(make.into()));
    ifd0.set(t::MODEL, Value::Ascii("TEST".into()));
    ifd0.add_sub_ifd(raw);
    TiffWriter::new(order, false).write(&[ifd0]).unwrap()
}

/// An Olympus ORF (TIFF with an `IIRO` magic) or Panasonic RW2 (`IIU\0`), raw image in IFD0.
fn magic_tiff(magic: &[u8; 4], make: &str) -> Vec<u8> {
    let px = (W * H) as usize;
    let mut ifd = IfdBuilder::new();
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![W]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![H]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::COMPRESSION, Value::Short(vec![1]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![1]));
    ifd.set(t::MAKE, Value::Ascii(make.into()));
    ifd.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![(0..px * 2).map(|i| (i * 13) as u8).collect()] });
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[..4].copy_from_slice(magic);
    b
}

/// A Panasonic RW2: sensor size, bit depth and raw format in IFD0, 12-bit packed blocks.
fn rw2() -> Vec<u8> {
    let mut ifd = IfdBuilder::new();
    ifd.set(0x0002, Value::Short(vec![W as u16]));
    ifd.set(0x0003, Value::Short(vec![H as u16]));
    ifd.set(0x0004, Value::Short(vec![1]));
    ifd.set(0x0005, Value::Short(vec![2]));
    ifd.set(0x0006, Value::Short(vec![H as u16 - 1]));
    ifd.set(0x0007, Value::Short(vec![W as u16 - 2]));
    ifd.set(0x000a, Value::Short(vec![12]));
    ifd.set(0x002d, Value::Short(vec![5]));
    ifd.set(0x001c, Value::Short(vec![64]));
    ifd.set(0x001d, Value::Short(vec![64]));
    ifd.set(0x001e, Value::Short(vec![64]));
    ifd.set(t::MAKE, Value::Ascii("Panasonic".into()));
    let need = (W * H).div_ceil(10) as usize * 16;
    ifd.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![(0..need).map(|i| (i * 29) as u8).collect()] });
    let mut b = TiffWriter::new(ByteOrder::Little, false).write(&[ifd]).unwrap();
    b[..4].copy_from_slice(b"IIU\0");
    b
}

/// A Pentax PEF: the raw image in IFD0, wide enough for the dark-border trim.
fn pef(compression: u16) -> Vec<u8> {
    let (w, h) = (264u32, 4u32);
    let mut ifd = IfdBuilder::new();
    ifd.set(t::MAKE, Value::Ascii("PENTAX".into()));
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![w]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![h]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::COMPRESSION, Value::Short(vec![compression]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![t::photometric::CFA]));
    let strip: Vec<u8> = (0..(w * h * 2) as usize).map(|i| if i % 528 < 8 { 0 } else { (i * 7 % 200) as u8 }).collect();
    ifd.set_image(ImageData::Strips { rows_per_strip: h, strips: vec![strip] });
    TiffWriter::new(ByteOrder::Big, false).write(&[ifd]).unwrap()
}

/// A Canon CR2: three placeholder IFDs and IFD3 with a 2-component lossless JPEG frame.
fn cr2() -> Vec<u8> {
    let img: Vec<u16> = (0..(W * H) as usize).map(|i| 1024 + (i * 7919 % 12000) as u16).collect();
    let enc = lightcraft_raw::ljpeg::encode(&img, W as usize / 2, H as usize, 2, 14, 1, 0);
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(t::MAKE, Value::Ascii("Canon".into()));
    let mut ifd3 = IfdBuilder::new();
    ifd3.set(t::COMPRESSION, Value::Short(vec![6]));
    ifd3.set(0xc640, Value::Short(vec![1, 16, 16]));
    ifd3.set_image(ImageData::Strips { rows_per_strip: H, strips: vec![enc] });
    let blank = || IfdBuilder::new().with(1, Value::Short(vec![0]));
    TiffWriter::new(ByteOrder::Little, false).write(&[ifd0, blank(), blank(), ifd3]).unwrap()
}

/// A Fujifilm RAF: header, records, raw block (LE TIFF whose IFD0 points to a 0xf000 raw IFD).
fn raf_file(w: u32, h: u32, bits: u32, strip: Vec<u8>, layout: Option<[u8; 36]>, jpeg: &[u8]) -> Vec<u8> {
    let mut block = b"II*\0\x08\0\0\0".to_vec();
    let sub_off: u32 = 8 + 2 + 12 + 4;
    block.extend_from_slice(&1u16.to_le_bytes());
    block.extend_from_slice(&[0x00, 0xf0, 13, 0, 1, 0, 0, 0]);
    block.extend_from_slice(&sub_off.to_le_bytes());
    block.extend_from_slice(&0u32.to_le_bytes());
    let n = 7u32;
    let wb_off = sub_off + 2 + 12 * n + 4;
    let strip_off = wb_off + 12;
    let entries: [(u16, u32, u32); 7] = [
        (0xf001, 1, w),
        (0xf002, 1, h),
        (0xf003, 1, bits),
        (0xf007, 1, strip_off),
        (0xf008, 1, strip.len() as u32),
        (0xf00a, 1, 64),
        (0xf00e, 3, wb_off),
    ];
    block.extend_from_slice(&(n as u16).to_le_bytes());
    for (tag, count, v) in entries {
        block.extend_from_slice(&tag.to_le_bytes());
        block.extend_from_slice(&4u16.to_le_bytes());
        block.extend_from_slice(&count.to_le_bytes());
        block.extend_from_slice(&v.to_le_bytes());
    }
    block.extend_from_slice(&0u32.to_le_bytes());
    for v in [300u32, 600, 450] {
        block.extend_from_slice(&v.to_le_bytes());
    }
    block.extend_from_slice(&strip);
    let mut recs: Vec<u8> = Vec::new();
    let mut nrec = 0u32;
    let mut add = |tag: u16, d: &[u8]| {
        recs.extend_from_slice(&tag.to_be_bytes());
        recs.extend_from_slice(&(d.len() as u16).to_be_bytes());
        recs.extend_from_slice(d);
        nrec += 1;
    };
    add(0x0100, &[(h >> 8) as u8, h as u8, (w >> 8) as u8, w as u8]);
    add(0x0110, &[0, 2, 0, 4]);
    add(0x0111, &[0, (h - 2) as u8, 0, (w - 4) as u8]);
    if let Some(l) = layout {
        add(0x0131, &l);
    }
    let mut dir = nrec.to_be_bytes().to_vec();
    dir.extend_from_slice(&recs);
    let mut out = b"FUJIFILMCCD-RAW 0201FF000000TEST".to_vec();
    out.resize(108, 0);
    let jpeg_off = 160u32;
    let dir_off = jpeg_off + jpeg.len() as u32;
    let raw_off = dir_off + dir.len() as u32;
    for (i, v) in [jpeg_off, jpeg.len() as u32, dir_off, dir.len() as u32, raw_off, block.len() as u32].iter().enumerate() {
        out[84 + i * 4..88 + i * 4].copy_from_slice(&v.to_be_bytes());
    }
    out.resize(160, 0);
    out.extend_from_slice(jpeg);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&block);
    out
}

fn raf() -> Vec<u8> {
    let strip: Vec<u8> = (0..(W * H * 2) as usize).map(|i| (i * 11) as u8).collect();
    let layout: [u8; 36] = std::array::from_fn(|i| (i % 3) as u8);
    raf_file(W, H, 16, strip, Some(layout), b"\xff\xd8\xff\xd9")
}

fn samples() -> Vec<Vec<u8>> {
    vec![
        cfa_tiff("NIKON CORPORATION", 1, 12, ByteOrder::Big),
        cfa_tiff("NIKON CORPORATION", 34713, 14, ByteOrder::Big),
        cfa_tiff("SONY", 32767, 12, ByteOrder::Little),
        cfa_tiff("SONY", 1, 14, ByteOrder::Little),
        pef(1),
        pef(65535),
        cfa_tiff("SAMSUNG", 1, 12, ByteOrder::Little),
        cfa_tiff("ACME", 1, 16, ByteOrder::Little),
        magic_tiff(b"IIRO", "OLYMPUS IMAGING CORP."),
        rw2(),
        cr2(),
        raf(),
    ]
}

fn exercise(bytes: &[u8]) {
    let _ = lightcraft_raw::probe(bytes);
    let _ = lightcraft_raw::probe_info(bytes);
    let _ = lightcraft_raw::embedded_preview(bytes);
    if let Ok(img) = lightcraft_raw::decode(bytes)
        && img.width * img.height <= 1 << 16
    {
        let _ = img.develop(lightcraft_raw::Method::Ahd);
    }
}

#[test]
fn vendor_samples_truncated_at_every_length() {
    for s in samples() {
        exercise(&s);
        for n in (0..s.len()).step_by(3) {
            exercise(&s[..n]);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    #[test]
    fn mutated_vendor_files_never_panic(kind in 0usize..12, flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..16), cut in any::<usize>()) {
        let mut data = samples().swap_remove(kind);
        let n = data.len();
        for (i, v) in flips {
            data[i % n] = v;
        }
        let keep = if cut % 3 == 0 { cut % n } else { n };
        exercise(&data[..keep]);
    }
}
