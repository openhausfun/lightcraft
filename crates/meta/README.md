# lightcraft-meta (L1)

Photo metadata: EXIF, XMP (read + write), IPTC-IIM, and the containers that carry them.

## API

| Function | Purpose |
|---|---|
| `read_exif(&[u8]) -> Metadata` | TIFF-structured Exif block (whole TIFF/DNG/raw file, or JPEG APP1 payload with/without `Exif\0\0`). `try_read_exif` reports header errors. |
| `from_tiff(&Tiff) -> Metadata` | Same, from an already parsed stream (used by `lightcraft-raw`). |
| `embedded(&[u8]) -> Embedded` | Sniffs JPEG / PNG / WebP / TIFF and returns the Exif, XMP (+ extended XMP), ICC and IPTC blocks. `jpeg_segments`, `png_chunks`, `webp_chunks` are the per-container walkers. |
| `parse_xmp(&str) -> Result<XmpData>` | Metadata + the opaque `lc:settings` JSON + every property (`prefix:name`, structs flattened as `a:b/c:d`). |
| `write_xmp(&Metadata, Option<&str>) -> String` | Complete `<?xpacket?>` packet; the second argument is LightCraft's full develop settings JSON. |
| `parse_iptc(&[u8]) -> Metadata` | IIM record 2: title (2:5), keywords (2:25), by-line (2:80), copyright (2:116), caption (2:120), date/time created (2:55/2:60). |
| `extract(&[u8]) -> Metadata` | Everything for a whole file. Precedence: EXIF → IPTC fills gaps → XMP overrides user fields (rating, label, title, caption, artist, copyright, keywords, GPS) and fills the rest. |

`Metadata` carries camera (make, model, serial, software), lens (make, model, serial, specification), capture
(date-time with UTC offset and milliseconds, exposure time, f-number, ISO incl. extended ISO, focal length and
35 mm equivalent, flash, exposure bias, program, metering, white balance), orientation, GPS, dimensions and the
user fields (artist, copyright, title, caption, keywords, hierarchical keywords, rating −1…5, colour label).

## XMP namespaces

Written: `dc`, `xmp`, `photoshop`, `exif`, `exifEX`, `tiff`, `lr` (`hierarchicalSubject`) and ours,
`lc` = `http://ns.lightcraft.app/lc/1.0/` (`lc:settings`, an opaque JSON string). Reading canonicalises
prefixes by namespace URI, handles attribute and element forms, `rdf:Seq`/`Bag`/`Alt` (x-default first),
`rdf:parseType="Resource"` and nested-description structs, predefined and numeric entities. Nesting depth
and node count are limited.

## Robustness

No panics on malformed input: proptests feed random bytes to every container walker and random text to the
XMP parser; truncation tests cut a sample JPEG at every byte.

## Sources

CIPA DC-008 (Exif 2.32), TIFF 6.0, Adobe DNG 1.7 (tag numbers), ISO 16684-1 / Adobe XMP Specification
Parts 1–3 (packet, RDF serialisation, standard schemas, JPEG extended XMP), IPTC-IIM 4.2 and the IPTC Photo
Metadata Standard, ITU-T T.81 (JPEG markers), ICC.1 (APP2 embedding), PNG Specification 3rd ed. (`eXIf`,
`iTXt`), the WebP RIFF container specification, Adobe Photoshop File Format Specification (image resource
blocks). XML parsing: `quick-xml` (MIT). No third-party metadata code was consulted.
