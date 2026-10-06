//! Decoders must never panic on malformed input: random bytes behind every magic number, and
//! mutated/truncated valid files. Uses `decode_unguarded` so panics in our own code are not masked
//! by the public entry point's panic guard.

use lightcraft_codecs::exif::minimal_exif;
use lightcraft_codecs::icc::write_named;
use lightcraft_codecs::*;
use lightcraft_raster::Rgba8;
use proptest::prelude::*;
use std::sync::OnceLock;

const MAGICS: &[&[u8]] = &[
    b"\xFF\xD8\xFF",
    b"\x89PNG\r\n\x1a\n",
    b"II*\0",
    b"MM\0*",
    b"RIFF\0\0\0\0WEBP",
    b"GIF89a",
    b"BM\0\0\0\0\0\0\0\0\0\0\0\0\x28\0\0\0",
    b"8BPS\0\x01",
    b"\xFF\x0A",
    b"\0\0\0\x0CJXL \x0D\x0A\x87\x0A",
];

fn small_opts() -> DecodeOptions {
    DecodeOptions { max_size: None, max_pixels: 1 << 22 }
}

fn try_all(bytes: &[u8]) {
    if let Some(f) = sniff(bytes) {
        let _ = decode_unguarded(bytes, f, &small_opts());
        let _ = decode_unguarded(bytes, f, &DecodeOptions { max_size: Some((8, 8)), max_pixels: 1 << 22 });
    }
    let _ = decode(bytes, small_opts());
    let _ = decode_thumbnail(bytes, 16);
    let _ = lightcraft_codecs::icc::parse(bytes);
}

fn seeds() -> &'static Vec<Vec<u8>> {
    static S: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    S.get_or_init(|| {
        let img = Rgba8::from_fn(24, 16, |x, y| [(x * 10) as u8, (y * 15) as u8, ((x ^ y) * 8) as u8, (x * y) as u8]);
        let icc = write_named(NamedSpace::DisplayP3);
        let exif = minimal_exif(6);
        let meta = EncodeMeta { icc: Some(&icc), exif: Some(&exif), xmp: Some("<x/>"), ..Default::default() };
        let e = EncodeImage::rgba8(&img);
        let u16s: Vec<u16> = (0..24 * 16 * 3).map(|i| (i * 331) as u16).collect();
        let f32s: Vec<f32> = (0..24 * 16 * 3).map(|i| i as f32 * 0.01).collect();
        vec![
            encode_jpeg(&e, 80, ChromaSubsampling::S420, &meta).unwrap(),
            encode_png(&e, &meta).unwrap(),
            encode_png(&EncodeImage::new(24, 16, 3, Samples::U16(&u16s)), &meta).unwrap(),
            encode_tiff(&e, TiffCompression::Lzw, &meta).unwrap(),
            encode_tiff(&EncodeImage::new(24, 16, 3, Samples::F32(&f32s)), TiffCompression::Deflate, &meta).unwrap(),
            encode_webp_lossless(&e, &meta).unwrap(),
        ]
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn random_after_magic(m in 0usize..MAGICS.len(), tail in proptest::collection::vec(any::<u8>(), 0..600)) {
        let mut b = MAGICS[m].to_vec();
        b.extend_from_slice(&tail);
        try_all(&b);
    }

    #[test]
    fn mutated_valid_files(which in 0usize..6, flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..12), cut in any::<prop::sample::Index>()) {
        let mut b = seeds()[which].clone();
        for (i, v) in flips {
            let i = i.index(b.len());
            b[i] = v;
        }
        try_all(&b);
        let n = cut.index(b.len());
        try_all(&b[..n]);
    }
}
