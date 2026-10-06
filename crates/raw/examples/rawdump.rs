//! Debug helper: `rawdump IN OUT` writes the decoded sensor data as `u32 width, u32 height` (little-endian)
//! followed by `width × height × cpp` little-endian `u16` samples, for analysis in external tools.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = lightcraft_raw::decode(&std::fs::read(&a[0]).expect("read")).expect("decode");
    let lightcraft_raw::RawData::U16(d) = &r.data else { panic!("float data") };
    let mut out = Vec::with_capacity(8 + d.len() * 2);
    out.extend_from_slice(&(r.width as u32).to_le_bytes());
    out.extend_from_slice(&(r.height as u32).to_le_bytes());
    for v in d {
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(&a[1], out).expect("write");
}
