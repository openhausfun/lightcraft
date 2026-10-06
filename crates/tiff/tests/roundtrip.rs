use lightcraft_tiff::image::{Layout, chunk_bytes};
use lightcraft_tiff::writer::{rational, srational};
use lightcraft_tiff::{ByteOrder, FieldType, IfdBuilder, ImageData, ParseOptions, Tiff, TiffError, TiffWriter, Value, parse_ifd_at, tags};
use proptest::prelude::*;

const ORDERS: [ByteOrder; 2] = [ByteOrder::Little, ByteOrder::Big];

fn all_values() -> Vec<(u16, Value)> {
    vec![
        (1000, Value::Byte(vec![1, 2, 3, 255])),
        (1001, Value::Ascii("hello tiff".into())),
        (1002, Value::Short(vec![1, 65535, 3])),
        (1003, Value::Long(vec![7])),
        (1004, Value::Rational(vec![(1, 3), (22, 7)])),
        (1005, Value::SByte(vec![-1, 0, 127, -128])),
        (1006, Value::Undefined(vec![9; 17])),
        (1007, Value::SShort(vec![-32768, 32767])),
        (1008, Value::SLong(vec![-5, i32::MAX])),
        (1009, Value::SRational(vec![(-1, 3), (5, -2)])),
        (1010, Value::Float(vec![1.5, -0.25, f32::MAX])),
        (1011, Value::Double(vec![std::f64::consts::PI])),
        (1012, Value::Ifd(vec![0])),
        (1013, Value::Short(vec![42])),
        (1014, Value::Ascii(String::new())),
    ]
}

fn big_values() -> Vec<(u16, Value)> {
    vec![(1020, Value::Long8(vec![1 << 40, 3])), (1021, Value::SLong8(vec![-(1 << 50)])), (1022, Value::Ifd8(vec![0]))]
}

#[test]
fn all_field_types_roundtrip_every_variant() {
    for order in ORDERS {
        for big in [false, true] {
            let mut ifd = IfdBuilder::new();
            for (t, v) in all_values() {
                ifd.set(t, v);
            }
            if big {
                for (t, v) in big_values() {
                    ifd.set(t, v);
                }
            }
            let bytes = TiffWriter::new(order, big).write(&[ifd]).unwrap();
            let t = Tiff::parse(&bytes).unwrap();
            assert_eq!(t.order, order);
            assert_eq!(t.bigtiff, big);
            assert_eq!(Tiff::sniff(&bytes), Some((order, big)));
            let ifd0 = &t.ifds[0];
            for (tag, v) in all_values().into_iter().chain(if big { big_values() } else { vec![] }) {
                assert_eq!(ifd0.value(tag), Some(&v), "tag {tag} order {order:?} big {big}");
            }
        }
    }
}

#[test]
fn accessors() {
    let mut ifd = IfdBuilder::new();
    ifd.set(1, Value::Rational(vec![(28, 10), (0, 0)]));
    ifd.set(2, Value::Short(vec![5, 6]));
    ifd.set(3, Value::SLong(vec![-3]));
    ifd.set(4, Value::Ascii("Canon  ".into()));
    ifd.set(5, Value::Byte(b"<x:xmpmeta/>\0junk".to_vec()));
    let bytes = TiffWriter::default().write(&[ifd]).unwrap();
    let t = Tiff::parse(&bytes).unwrap();
    let i = &t.ifds[0];
    assert_eq!(i.f64(1), Some(2.8));
    assert_eq!(i.f64s(1).unwrap(), vec![2.8, 0.0]);
    assert_eq!(i.u32(2), Some(5));
    assert_eq!(i.u64s(2).unwrap(), vec![5, 6]);
    assert_eq!(i.u32(3), None);
    assert_eq!(i.i64(3), Some(-3));
    assert_eq!(i.string(4).as_deref(), Some("Canon"));
    assert_eq!(i.string(5).as_deref(), Some("<x:xmpmeta/>"));
    assert_eq!(i.get(4).unwrap().field_type(), FieldType::Ascii);
    assert!(i.u32(999).is_none());
    assert_eq!(t.find(2).map(|e| e.tag), Some(2));
}

#[test]
fn chain_subifds_and_pointer_ifds() {
    for order in ORDERS {
        for big in [false, true] {
            let mut interop = IfdBuilder::new();
            interop.set(1, Value::Ascii("R98".into()));
            let mut exif = IfdBuilder::new();
            exif.set(tags::F_NUMBER, Value::Rational(vec![(56, 10)]));
            exif.set_child(tags::INTEROP_IFD, interop);
            let mut gps = IfdBuilder::new();
            gps.set(tags::GPS_LATITUDE_REF, Value::Ascii("N".into()));
            let mut ifd0 = IfdBuilder::new();
            ifd0.set(tags::MAKE, Value::Ascii("Maker".into()));
            ifd0.set_child(tags::EXIF_IFD, exif);
            ifd0.set_child(tags::GPS_IFD, gps);
            let mut raw = IfdBuilder::new();
            raw.set(tags::IMAGE_WIDTH, Value::Long(vec![4]));
            let mut nested = IfdBuilder::new();
            nested.set(tags::IMAGE_WIDTH, Value::Long(vec![2]));
            raw.add_sub_ifd(nested);
            ifd0.add_sub_ifd(raw);
            ifd0.add_sub_ifd(IfdBuilder::new().with(tags::IMAGE_WIDTH, Value::Short(vec![3])));
            let ifd1 = IfdBuilder::new().with(tags::IMAGE_WIDTH, Value::Short(vec![160]));
            let ifd2 = IfdBuilder::new().with(tags::IMAGE_WIDTH, Value::Short(vec![80]));
            let bytes = TiffWriter::new(order, big).write(&[ifd0, ifd1, ifd2]).unwrap();
            let t = Tiff::parse(&bytes).unwrap();
            assert_eq!(t.ifds.len(), 3);
            assert_eq!(t.ifds[1].u32(tags::IMAGE_WIDTH), Some(160));
            assert_eq!(t.ifds[2].u32(tags::IMAGE_WIDTH), Some(80));
            let i0 = &t.ifds[0];
            assert_eq!(i0.sub_ifds.len(), 2);
            assert_eq!(i0.sub_ifds[0].u32(tags::IMAGE_WIDTH), Some(4));
            assert_eq!(i0.sub_ifds[0].sub_ifds[0].u32(tags::IMAGE_WIDTH), Some(2));
            assert_eq!(i0.sub_ifds[1].u32(tags::IMAGE_WIDTH), Some(3));
            let exif = t.exif().unwrap();
            assert_eq!(exif.f64(tags::F_NUMBER), Some(5.6));
            assert_eq!(exif.interop.as_ref().unwrap().string(1).as_deref(), Some("R98"));
            assert_eq!(t.gps().unwrap().string(tags::GPS_LATITUDE_REF).as_deref(), Some("N"));
            // IFD0, sub0, sub0.sub, sub1, exif, interop, gps, ifd1, ifd2
            assert_eq!(t.all_ifds().len(), 9);
        }
    }
}

fn gradient(w: usize, h: usize) -> Vec<u8> {
    (0..w * h).map(|i| (i * 7 % 251) as u8).collect()
}

#[test]
fn strips_layout_and_bytes() {
    let (w, h) = (10usize, 7usize);
    let px = gradient(w, h);
    let rps = 3;
    let strips: Vec<Vec<u8>> = px.chunks(w * rps).map(|c| c.to_vec()).collect();
    let mut ifd = IfdBuilder::new();
    ifd.set(tags::IMAGE_WIDTH, Value::Long(vec![w as u32]));
    ifd.set(tags::IMAGE_LENGTH, Value::Long(vec![h as u32]));
    ifd.set(tags::BITS_PER_SAMPLE, Value::Short(vec![8]));
    ifd.set(tags::COMPRESSION, Value::Short(vec![1]));
    ifd.set_image(ImageData::Strips { rows_per_strip: rps as u32, strips });
    for order in ORDERS {
        for big in [false, true] {
            let bytes = TiffWriter::new(order, big).write(std::slice::from_ref(&ifd)).unwrap();
            let t = Tiff::parse(&bytes).unwrap();
            let info = t.ifds[0].image().unwrap();
            assert_eq!(info.layout, Layout::Strips { rows_per_strip: 3 });
            assert_eq!(info.grid(), (1, 3));
            let chunks = info.chunks(bytes.len() as u64);
            assert_eq!(chunks.len(), 3);
            assert_eq!(chunks[2].height, 1);
            let mut out = Vec::new();
            for c in &chunks {
                out.extend_from_slice(chunk_bytes(&bytes, c).unwrap());
            }
            assert_eq!(out, px);
        }
    }
}

#[test]
fn tiles_layout_and_bytes() {
    let (w, h, tw, th) = (20usize, 13usize, 16usize, 8usize);
    let px = gradient(w, h);
    let (ta, td) = (w.div_ceil(tw), h.div_ceil(th));
    let mut tiles = Vec::new();
    for ty in 0..td {
        for tx in 0..ta {
            let mut t = vec![0u8; tw * th];
            for y in 0..th {
                for x in 0..tw {
                    let (sx, sy) = (tx * tw + x, ty * th + y);
                    if sx < w && sy < h {
                        t[y * tw + x] = px[sy * w + sx];
                    }
                }
            }
            tiles.push(t);
        }
    }
    let mut ifd = IfdBuilder::new();
    ifd.set(tags::IMAGE_WIDTH, Value::Short(vec![w as u16]));
    ifd.set(tags::IMAGE_LENGTH, Value::Short(vec![h as u16]));
    ifd.set_image(ImageData::Tiles { tile_width: tw as u32, tile_height: th as u32, tiles });
    let bytes = TiffWriter::new(ByteOrder::Big, false).write(&[ifd]).unwrap();
    let t = Tiff::parse(&bytes).unwrap();
    let info = t.ifds[0].image().unwrap();
    assert_eq!(info.grid(), (2, 2));
    let chunks = info.chunks(bytes.len() as u64);
    assert_eq!(chunks.len(), 4);
    let mut out = vec![0u8; w * h];
    for c in &chunks {
        let b = chunk_bytes(&bytes, c).unwrap();
        for y in 0..c.height as usize {
            for x in 0..c.width as usize {
                let (sx, sy) = (c.x as usize + x, c.y as usize + y);
                if sx < w && sy < h {
                    out[sy * w + sx] = b[y * tw + x];
                }
            }
        }
    }
    assert_eq!(out, px);
}

#[test]
fn missing_byte_counts_are_inferred() {
    let mut ifd = IfdBuilder::new();
    ifd.set(tags::IMAGE_WIDTH, Value::Long(vec![4]));
    ifd.set(tags::IMAGE_LENGTH, Value::Long(vec![2]));
    ifd.set_image(ImageData::Strips { rows_per_strip: 1, strips: vec![vec![1; 4], vec![2; 4]] });
    let bytes = TiffWriter::default().write(&[ifd]).unwrap();
    let t = Tiff::parse(&bytes).unwrap();
    let mut info = t.ifds[0].image().unwrap();
    info.byte_counts.clear();
    let c = info.chunks(bytes.len() as u64);
    assert_eq!(c[0].len, 4);
    assert_eq!(&chunk_bytes(&bytes, &c[0]).unwrap()[..4], &[1; 4]);
    assert!(c[1].len >= 4);
}

#[test]
fn bad_headers() {
    assert_eq!(Tiff::parse(b""), Err(TiffError::NotTiff));
    assert_eq!(Tiff::parse(b"XX*\0\0\0\0\0"), Err(TiffError::NotTiff));
    assert_eq!(Tiff::parse(b"II\x2b\0\x04\0\0\0"), Err(TiffError::NotTiff));
    assert!(Tiff::parse(b"II*\0").is_err());
    assert!(Tiff::parse(b"II*\0\x08\0\0\0").is_err());
    assert!(Tiff::parse(b"II*\0\x08\0\0\0\0\0").is_err());
    assert!(Tiff::sniff(b"MM\0*\0\0\0\x08").is_some());
}

#[test]
fn ifd_loops_are_detected() {
    // IFD0 at 8 whose next pointer points back to itself, and whose SubIFDs point to itself too.
    let mut b = b"II*\0\x08\0\0\0".to_vec();
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&330u16.to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&8u32.to_le_bytes());
    let t = Tiff::parse(&b).unwrap();
    assert_eq!(t.ifds.len(), 1);
    assert!(t.ifds[0].sub_ifds.is_empty());
}

#[test]
fn out_of_range_entries_are_skipped() {
    let mut ifd = IfdBuilder::new();
    ifd.set(1, Value::Long(vec![1, 2, 3]));
    ifd.set(2, Value::Short(vec![5]));
    let mut bytes = TiffWriter::default().write(&[ifd]).unwrap();
    let t = Tiff::parse(&bytes).unwrap();
    let e = t.ifds[0].get(1).unwrap().clone();
    // point tag 1's value offset past EOF
    let ifd_off = t.ifds[0].offset as usize;
    let field = ifd_off + 2 + 8;
    assert_eq!(u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as u64, e.offset);
    bytes[field..field + 4].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    let t = Tiff::parse(&bytes).unwrap();
    assert!(t.ifds[0].get(1).is_none());
    assert_eq!(t.ifds[0].u32(2), Some(5));
}

#[test]
fn value_budget_limits_allocation() {
    // 50 entries all pointing at the same 1000-byte blob.
    let mut ifd = IfdBuilder::new();
    ifd.set(1, Value::Undefined(vec![7; 1000]));
    let bytes = TiffWriter::default().write(&[ifd]).unwrap();
    let t = Tiff::parse(&bytes).unwrap();
    let blob = t.ifds[0].get(1).unwrap().offset as u32;
    let mut b = b"II*\0".to_vec();
    b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    let mut file = bytes.clone();
    file[..8].copy_from_slice(&b);
    file.extend_from_slice(&50u16.to_le_bytes());
    for i in 0..50u16 {
        file.extend_from_slice(&i.to_le_bytes());
        file.extend_from_slice(&7u16.to_le_bytes());
        file.extend_from_slice(&1000u32.to_le_bytes());
        file.extend_from_slice(&blob.to_le_bytes());
    }
    file.extend_from_slice(&0u32.to_le_bytes());
    let opts = ParseOptions { value_budget: Some(10_000), ..Default::default() };
    let t = Tiff::parse_with(&file, &opts).unwrap();
    assert_eq!(t.ifds[0].entries.len(), 10);
    let t = Tiff::parse(&file).unwrap();
    assert_eq!(t.ifds[0].entries.len(), 50);
}

#[test]
fn parse_ifd_with_base() {
    // an IFD blob embedded at 100 whose offsets are relative to 100
    let mut blob = Vec::new();
    let o = ByteOrder::Big;
    o.put_u16(&mut blob, 1);
    o.put_u16(&mut blob, 5);
    o.put_u16(&mut blob, 4);
    o.put_u32(&mut blob, 3);
    o.put_u32(&mut blob, 18);
    o.put_u32(&mut blob, 0);
    for v in [10u32, 20, 30] {
        o.put_u32(&mut blob, v);
    }
    let mut data = vec![0u8; 100];
    data.extend(blob);
    let (ifd, next) = parse_ifd_at(&data, 100, o, 100, false, &ParseOptions::default()).unwrap();
    assert_eq!(next, 0);
    assert_eq!(ifd.u64s(5).unwrap(), vec![10, 20, 30]);
    assert_eq!(ifd.get(5).unwrap().offset, 118);
}

#[test]
fn rationals() {
    assert_eq!(rational(2.8), (28, 10));
    assert_eq!(rational(0.004), (4, 1000));
    assert_eq!(rational(-1.0), (0, 1));
    assert_eq!(srational(-0.333), (-333, 1000));
    assert_eq!(srational(7.0), (7, 1));
    let (n, d) = rational(1.0 / 3.0);
    assert!((n as f64 / d as f64 - 1.0 / 3.0).abs() < 1e-6);
}

fn sample_file() -> Vec<u8> {
    let mut exif = IfdBuilder::new();
    exif.set(tags::F_NUMBER, Value::Rational(vec![(4, 1)]));
    exif.set(tags::MAKER_NOTE, Value::Undefined(b"Nikon\0\x02\x10\0\0II*\0\x08\0\0\0\x01\0\x01\0\x03\0\x01\0\0\0\x05\0\0\0\0\0\0\0".to_vec()));
    let mut ifd0 = IfdBuilder::new();
    ifd0.set(tags::IMAGE_WIDTH, Value::Long(vec![8]));
    ifd0.set(tags::IMAGE_LENGTH, Value::Long(vec![4]));
    ifd0.set(tags::MAKE, Value::Ascii("NIKON".into()));
    ifd0.set_child(tags::EXIF_IFD, exif);
    ifd0.set_image(ImageData::Strips { rows_per_strip: 2, strips: vec![vec![1; 16], vec![2; 16]] });
    ifd0.add_sub_ifd(IfdBuilder::new().with(tags::IMAGE_WIDTH, Value::Long(vec![2])));
    TiffWriter::default().write(&[ifd0, IfdBuilder::new().with(1, Value::Short(vec![1]))]).unwrap()
}

fn exercise(t: &Tiff, data: &[u8]) {
    for ifd in t.all_ifds() {
        if let Ok(info) = ifd.image() {
            for c in info.chunks(data.len() as u64).iter().take(64) {
                let _ = chunk_bytes(data, c);
            }
        }
        for e in &ifd.entries {
            let _ = e.value.to_f64_vec();
            let _ = e.value.as_str();
        }
    }
    if let Some(e) = t.exif().and_then(|x| x.get(tags::MAKER_NOTE)) {
        let _ = lightcraft_tiff::makernote::parse_makernote(data, e.offset, e.count() as u64, t.order, "NIKON");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, .. ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic(mut data in proptest::collection::vec(any::<u8>(), 0..512), hdr in 0usize..4) {
        let heads: [&[u8]; 4] = [b"II*\0", b"MM\0*", b"II+\0\x08\0\0\0", b"MM\0+\0\x08\0\0"];
        let h = heads[hdr];
        if data.len() >= h.len() {
            data[..h.len()].copy_from_slice(h);
        }
        if let Ok(t) = Tiff::parse(&data) {
            exercise(&t, &data);
        }
    }

    #[test]
    fn mutated_files_never_panic(flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..16), cut in any::<usize>()) {
        let mut data = sample_file();
        for (i, v) in flips {
            let n = data.len();
            data[i % n] = v;
        }
        let keep = if cut % 4 == 0 { cut % data.len() } else { data.len() };
        let data = &data[..keep];
        if let Ok(t) = Tiff::parse(data) {
            exercise(&t, data);
        }
    }

    #[test]
    fn random_short_arrays_roundtrip(v in proptest::collection::vec(any::<u16>(), 1..300), big in any::<bool>(), le in any::<bool>()) {
        let order = if le { ByteOrder::Little } else { ByteOrder::Big };
        let ifd = IfdBuilder::new().with(500, Value::Short(v.clone())).with(501, Value::Double(v.iter().map(|&x| x as f64 / 3.0).collect()));
        let bytes = TiffWriter::new(order, big).write(&[ifd]).unwrap();
        let t = Tiff::parse(&bytes).unwrap();
        prop_assert_eq!(t.ifds[0].value(500), Some(&Value::Short(v.clone())));
        prop_assert_eq!(t.ifds[0].f64s(501).unwrap().len(), v.len());
    }
}
