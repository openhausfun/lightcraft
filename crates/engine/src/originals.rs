//! Never write over an original: exports, renders and other files LightCraft writes for the user
//! must not land on a catalogued photo's own file (or its XMP sidecar), whatever the conflict
//! setting or exact path asked for. [`OriginalGuard`] is a snapshot of the library's originals
//! that answers "would writing here replace one?" by file identity (the same file under another
//! spelling, through `..`, a link or a case-insensitive volume counts), not by comparing strings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::safe_file::same_file;
use lightcraft_catalog::{Catalog, Source};

/// The library's originals and their sidecars, looked up by file name.
#[derive(Debug, Default)]
pub struct OriginalGuard {
    /// lower-case file name → (path on disk, what it is: "the original of IMG_1.CR3", …)
    by_name: HashMap<String, Vec<(PathBuf, String)>>,
}

impl OriginalGuard {
    /// The originals of every photo in `catalog`.
    pub fn new(catalog: &Catalog) -> Self {
        let mut g = Self::default();
        for p in catalog.photos() {
            if let Source::File { path } = &p.source {
                g.add(Path::new(path), &p.file_name);
            }
        }
        g
    }

    /// Protect `path` (an original named `label`) and its sidecars (`IMG_1.xmp`, `IMG_1.CR3.xmp`).
    pub fn add(&mut self, path: &Path, label: &str) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else { return };
        let mut put = |file: String, p: PathBuf, what: String| self.by_name.entry(file.to_lowercase()).or_default().push((p, what));
        put(name.clone(), path.to_path_buf(), format!("the original of {label}"));
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        put(format!("{stem}.xmp"), path.with_extension("xmp"), format!("the XMP sidecar of {label}"));
        put(format!("{name}.xmp"), PathBuf::from(format!("{}.xmp", path.to_string_lossy())), format!("the XMP sidecar of {label}"));
    }

    /// `Err` (a message for the user) when writing `target` would replace a protected file.
    pub fn check(&self, target: &Path) -> Result<(), String> {
        if std::fs::symlink_metadata(target).is_err() {
            return Ok(()); // nothing there to replace
        }
        let mut names: Vec<String> = target.file_name().map(|n| n.to_string_lossy().to_lowercase()).into_iter().collect();
        if let Some(n) = std::fs::canonicalize(target).ok().and_then(|c| c.file_name().map(|n| n.to_string_lossy().to_lowercase())) {
            names.push(n);
        }
        for n in &names {
            for (p, what) in self.by_name.get(n).into_iter().flatten() {
                if same_file(p, target) {
                    return Err(format!(
                        "{} is {what} in the library: LightCraft never writes over an original (choose another folder or file name)",
                        target.display()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl crate::Session {
    /// A guard over this library's originals (see [`OriginalGuard`]).
    pub fn original_guard(&self) -> OriginalGuard {
        OriginalGuard::new(&self.catalog)
    }

    /// `Err` when writing `path` would replace a catalogued photo's original or sidecar. A free
    /// path (the usual case) is answered without building the guard over the whole library.
    pub fn check_write_target(&self, path: &str) -> Result<(), String> {
        let path = Path::new(path);
        if std::fs::symlink_metadata(path).is_err() {
            return Ok(()); // nothing there to replace
        }
        self.original_guard().check(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo, PhotoId};

    #[test]
    fn originals_and_their_sidecars_are_protected_by_identity() {
        let d = std::env::temp_dir().join(format!("lc-originals-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let orig = d.join("IMG_1.jpg");
        std::fs::write(&orig, b"original").unwrap();
        std::fs::write(d.join("IMG_1.xmp"), b"<x/>").unwrap();
        std::fs::write(d.join("other.jpg"), b"other").unwrap();
        let mut c = Catalog::default();
        let p = Photo::new(PhotoId(1), Source::File { path: orig.to_string_lossy().to_string() }, "IMG_1.jpg", "JPEG", 10, 10, "2026-01-01");
        c.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        let g = OriginalGuard::new(&c);
        assert!(g.check(&orig).unwrap_err().contains("original of IMG_1.jpg"));
        assert!(g.check(&d.join("sub/../IMG_1.jpg")).is_err(), "another spelling of the same file");
        assert!(g.check(&d.join("IMG_1.xmp")).unwrap_err().contains("sidecar"));
        assert!(g.check(&d.join("other.jpg")).is_ok(), "an ordinary existing file");
        assert!(g.check(&d.join("new.jpg")).is_ok(), "a free name");
        let _ = std::fs::remove_dir_all(&d);
    }
}
