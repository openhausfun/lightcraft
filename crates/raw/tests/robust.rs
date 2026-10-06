//! Malformed-input robustness: decoding random or corrupted files must never panic.

use lightcraft_raw::{
    BlackLevel, Cfa, ColorData, DngCompression, DngWriteOptions, Method, OpcodeLists, Orientation, RawData, RawFormat, RawImage, Rect,
};
use proptest::prelude::*;

fn sample(comp: DngCompression) -> Vec<u8> {
    let (w, h) = (24usize, 16usize);
    let data: Vec<u16> = (0..w * h).map(|i| (i * 97 % 4000) as u16).collect();
    let raw = RawImage {
        format: RawFormat::Dng,
        width: w,
        height: h,
        cpp: 1,
        data: RawData::U16(data),
        cfa: Some(Cfa::bayer("RGGB").unwrap()),
        bits: 12,
        black: BlackLevel::uniform(64.0),
        white: vec![4095.0],
        active_area: Rect::new(0, 0, w, h),
        crop: Rect::new(0, 0, w, h),
        orientation: Orientation::Normal,
        color: ColorData::default(),
        wb_multipliers: None,
        linearized: false,
        opcodes: OpcodeLists::default(),
        metadata: Default::default(),
    };
    lightcraft_raw::write_dng(&raw, &DngWriteOptions { compression: comp, ..Default::default() }).unwrap()
}

fn exercise(bytes: &[u8]) {
    let _ = lightcraft_raw::probe(bytes);
    let _ = lightcraft_raw::embedded_preview(bytes);
    if let Ok(img) = lightcraft_raw::decode(bytes)
        && img.width * img.height <= 1 << 16
    {
        let _ = img.develop(Method::Ahd);
        let xy = lightcraft_raw::color::as_shot_white_xy(&img);
        let _ = lightcraft_raw::color::camera_transform(&img, xy);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, .. ProptestConfig::default() })]

    #[test]
    fn mutated_dngs_never_panic(flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 1..24), cut in any::<usize>(), lj in any::<bool>()) {
        let mut data = sample(if lj { DngCompression::Lj92 { tile: 16 } } else { DngCompression::Uncompressed });
        for (i, v) in flips {
            let n = data.len();
            data[i % n] = v;
        }
        let keep = if cut % 3 == 0 { cut % data.len() } else { data.len() };
        exercise(&data[..keep]);
    }

    #[test]
    fn random_tiff_like_bytes_never_panic(mut data in proptest::collection::vec(any::<u8>(), 8..700), kind in 0usize..4) {
        let heads: [&[u8]; 4] = [b"II*\0\x08\0\0\0", b"MM\0*\0\0\0\x08", b"II*\0\x10\0\0\0CR\x02\0", b"II+\0\x08\0\0\0"];
        let h = heads[kind];
        let n = h.len().min(data.len());
        data[..n].copy_from_slice(&h[..n]);
        exercise(&data);
    }

    #[test]
    fn random_lj92_never_panics(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        let mut s = vec![0xff, 0xd8, 0xff, 0xc3, 0, 11, 12, 0, 8, 0, 8, 1, 1, 0x11, 0];
        s.extend_from_slice(&data);
        let _ = lightcraft_raw::ljpeg::decode(&s, 1 << 20);
    }
}

#[test]
fn truncation_at_every_length() {
    for comp in [DngCompression::Uncompressed, DngCompression::Lj92 { tile: 16 }] {
        let d = sample(comp);
        for n in (0..d.len()).step_by(7) {
            exercise(&d[..n]);
        }
    }
}

#[test]
fn hostile_dimensions_are_rejected_quickly() {
    // a DNG claiming a 60000 × 60000 16-bit image with 10 bytes of data
    use lightcraft_tiff::tags as t;
    use lightcraft_tiff::{IfdBuilder, ImageData, TiffWriter, Value};
    let mut ifd = IfdBuilder::new();
    ifd.set(t::DNG_VERSION, Value::Byte(vec![1, 4, 0, 0]));
    ifd.set(t::IMAGE_WIDTH, Value::Long(vec![60000]));
    ifd.set(t::IMAGE_LENGTH, Value::Long(vec![60000]));
    ifd.set(t::BITS_PER_SAMPLE, Value::Short(vec![16]));
    ifd.set(t::PHOTOMETRIC, Value::Short(vec![32803]));
    ifd.set(t::CFA_PATTERN_EP, Value::Byte(vec![0, 1, 1, 2]));
    ifd.set(t::COMPRESSION, Value::Short(vec![7]));
    ifd.set_image(ImageData::Tiles { tile_width: 60000, tile_height: 60000, tiles: vec![vec![0xff, 0xd8, 0xff, 0xd9]] });
    let bytes = TiffWriter::default().write(&[ifd]).unwrap();
    let t0 = std::time::Instant::now();
    assert!(lightcraft_raw::decode(&bytes).is_err());
    assert!(t0.elapsed().as_secs_f64() < 2.0);
}
