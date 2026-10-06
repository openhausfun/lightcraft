//! Debug helper: per-column / per-row statistics of the decoded raw (outliers above the bit range).
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let r = lightcraft_raw::decode(&std::fs::read(&a[0]).unwrap()).unwrap();
    let lightcraft_raw::RawData::U16(d) = &r.data else { return };
    let lim = (1u32 << r.bits) as u16;
    let (w, h) = (r.width, r.height);
    let bad_cols: Vec<(usize, usize)> = (0..w).map(|x| (x, (0..h).filter(|&y| d[y * w + x] >= lim).count())).filter(|c| c.1 > 0).collect();
    let bad_rows: Vec<(usize, usize)> = (0..h).map(|y| (y, (0..w).filter(|&x| d[y * w + x] >= lim).count())).filter(|c| c.1 > 0).collect();
    println!("cols with >= {lim}: {} {:?}", bad_cols.len(), &bad_cols[..bad_cols.len().min(20)]);
    println!("rows with >= {lim}: {} {:?}", bad_rows.len(), &bad_rows[..bad_rows.len().min(10)]);
    let mx = d.iter().filter(|&&v| v < lim).max();
    println!(
        "max in range: {mx:?}; right-edge col means: {:?}",
        (w - 8..w).map(|x| (0..h).map(|y| d[y * w + x] as u64).sum::<u64>() / h as u64).collect::<Vec<_>>()
    );
}
