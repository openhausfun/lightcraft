//! Fonts from the optional craft-fonts build input (storytold/craft-fonts).
//!
//! Built with `CRAFT_FONTS_DIR=<craft-fonts checkout>`, [`CRAFT_FONTS`] holds every font in its
//! manifest (today: the Japanese UI and document fonts); otherwise it is empty and LightCraft uses
//! only its own bundled fonts (Inter). Both the egui UI and the export watermark renderer read it
//! from here. See craftrules `standards/fonts.md`.

/// A font from the optional craft-fonts build input (empty unless built with `CRAFT_FONTS_DIR`).
pub struct CraftFont {
    pub family: &'static str,
    pub style: &'static str,
    /// ISO 15924 scripts the font is for, e.g. `"Jpan"`.
    pub scripts: &'static [&'static str],
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

impl CraftFont {
    /// Whether the font is meant for `script` (ISO 15924, e.g. `"Jpan"`).
    pub fn covers(&self, script: &str) -> bool {
        self.scripts.contains(&script)
    }

    /// Mincho (serif) faces, the preferred Japanese faces for document-like text.
    pub fn is_mincho(&self) -> bool {
        self.family.contains("Mincho")
    }
}

/// The craft-fonts faces for Japanese, in `fonts`' order (empty without craft-fonts).
pub fn japanese(fonts: &'static [CraftFont]) -> impl Iterator<Item = &'static CraftFont> {
    fonts.iter().filter(|f| f.covers("Jpan"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn craft_fonts_are_well_formed_or_absent() {
        if CRAFT_FONTS.is_empty() {
            eprintln!("built without CRAFT_FONTS_DIR: no craft-fonts to check");
            return;
        }
        for f in CRAFT_FONTS {
            assert!(!f.family.is_empty() && !f.style.is_empty() && !f.scripts.is_empty(), "{}", f.family);
            assert!(ab_glyph::FontRef::try_from_slice(f.bytes).is_ok(), "{} {} parses", f.family, f.style);
        }
        assert!(japanese(CRAFT_FONTS).next().is_some(), "craft-fonts carries Japanese faces");
    }
}
