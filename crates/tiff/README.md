# lightcraft-tiff (L0)

TIFF / IFD reader and writer. Used by `lightcraft-raw` (DNG, CR2, NEF, ARW…), `lightcraft-meta` (Exif) and
later the TIFF/DNG exporters.

## Features

- Classic TIFF and BigTIFF, little- and big-endian (`II`/`MM`). Also accepts the TIFF-like magic numbers of
  some raw formats (ORF `IIRO`/`IIRS`, RW2 `IIU`) when `ParseOptions::accept_raw_magic` is set (default).
- Every field type: BYTE, ASCII, SHORT, LONG, RATIONAL, SBYTE, UNDEFINED, SSHORT, SLONG, SRATIONAL, FLOAT,
  DOUBLE, IFD, LONG8, SLONG8, IFD8.
- IFD chain + `SubIFDs` (330) + Exif (34665) / GPS (34853) / Interoperability (40965) pointer IFDs, resolved
  into a tree (`Ifd::sub_ifds`, `exif`, `gps`, `interop`). `Entry::offset` records the absolute position of
  each value (needed for maker notes and embedded blobs).
- `parse_ifd_at(data, offset, order, base, bigtiff, opts)` parses an IFD whose offsets are relative to an
  arbitrary base; `makernote::parse_makernote` knows the header / byte-order / offset-base conventions of
  Canon, Nikon (types 1 and 3), Sony, Fujifilm, Olympus (old and new), Panasonic and Pentax notes.
- `image::ImageInfo` — width/height, bits, samples, compression, photometric, planar config, predictor,
  sample format and the strip or tile chunk list (`chunks()`, `chunk_bytes()`); missing byte counts are
  inferred.
- `TiffWriter` writes IFD chains with strips or tiles, `SubIFDs` and pointer IFDs, in either byte order,
  classic or BigTIFF; `writer::rational` / `srational` approximate floats as rationals.

## Robustness

The reader never panics on malformed input (proptests feed random and mutated files): offsets are
bounds-checked; out-of-range or unknown-type entries are skipped; IFD loops are detected; IFD count,
nesting depth and entries per IFD are limited; the total bytes of decoded values are capped at
`8 × input + 1 MiB` so a file cannot make us allocate far more than its own size.

## Sources

Public specifications only: TIFF 6.0 (Adobe, 1992), the TIFF 6.0 supplements (IFD type, SubIFDs), the
BigTIFF format description (AWare Systems), Adobe DNG Specification 1.7 (tag numbers), CIPA DC-008 Exif 2.32,
and the ExifTool tag-name documentation (maker-note header conventions, prose only). No third-party code
was consulted.
