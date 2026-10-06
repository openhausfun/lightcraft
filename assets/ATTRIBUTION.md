# Asset attribution

Every asset shipped in this repository is listed here (see the asset rules in `AGENTS.md`). No asset may come from
Adobe products. Add an entry in the same commit as the asset.
`cargo xtask assets` (part of `cargo xtask ci`) fails if any image, icon, font, sound, video, raw file or colour profile
in the repository is not matched by a path pattern in the first column (`*` = any characters except `/`; a trailing `/`
covers a whole directory), or if a licence file referenced in the Licence column is missing.

| Path | Asset | Creator | Source | Licence | Added | Modifications |
|---|---|---|---|---|---|---|
| `assets/fonts/Inter-*.ttf` (Regular, Medium, SemiBold) | Inter | Rasmus Andersson | https://github.com/rsms/inter | SIL Open Font License 1.1 (`assets/fonts/OFL-Inter.txt`) | 2026-09-30 | none |
| `assets/app-icon/` (`lightcraft.svg` and the PNG/ICNS/ICO/SVG files derived from it) | LightCraft app icon (lynx) | LightCraft project owner (drawn in ArtCraft) | original work (vectorised from the owner's drawing; see `assets/app-icon/README.md`) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | 2026-10-02 | vectorised to SVG; rendered to PNG/ICNS/ICO by `packaging/icons.sh` |
| `crates/scenes/` (generated images) | Procedural demo photographs | LightCraft contributors | original work (generated at runtime, no source imagery) | MIT OR Apache-2.0 | 2026-09-30 | n/a |
| `crates/ui-egui/src/icons.rs` | UI icons drawn as vector paths in code | LightCraft contributors | original work | MIT OR Apache-2.0 | 2026-09-30 | n/a |
| `docs/images/*-tetons.jpg`, `docs/images/grid-pd.jpg` (screenshots containing it) | "The Tetons and the Snake River" | Ansel Adams (1942), U.S. National Archives | https://commons.wikimedia.org/wiki/File:Adams_The_Tetons_and_the_Snake_River.jpg | Public domain (U.S. federal government work) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-migrant-mother.jpg`, `docs/images/grid-pd.jpg` | "Migrant Mother" | Dorothea Lange (1936), Farm Security Administration / Library of Congress | https://commons.wikimedia.org/wiki/File:Lange-MigrantMother02.jpg | Public domain (U.S. federal government work) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-earthrise.jpg`, `docs/images/grid-pd.jpg` | "Earthrise" (AS08-14-2383) | NASA / Bill Anders, Apollo 8 (1968) | https://commons.wikimedia.org/wiki/File:NASA-Apollo8-Dec24-Earthrise.jpg | Public domain (NASA) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-blue-marble.jpg`, `docs/images/grid-pd.jpg` | "The Blue Marble" (AS17-148-22727) | NASA, Apollo 17 crew (1972) | https://commons.wikimedia.org/wiki/File:The_Earth_seen_from_Apollo_17.jpg | Public domain (NASA) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*.jpg` (UI) | LightCraft application screenshots | LightCraft contributors | original work (captured with `docs/showcase/`) | MIT OR Apache-2.0 | 2026-09-30 | n/a |
| `docs/brand/` (artcraft-logo and artcraft-mark, SVG and PNG) | ArtCraft wordmark and mark | ArtCraft Team | original work (https://getartcraft.com/) | ArtCraft trademark, see `docs/brand/LICENSE-brand.txt` (not open source) | 2026-10-01 | none |
| craft-fonts: optional build input, not files in this repository | BIZ UDPGothic (Regular, Bold), BIZ UDMincho Regular, Shippori Mincho Regular | Morisawa Inc. / The BIZ UDGothic and BIZ UDMincho Project Authors; The Shippori Mincho Project Authors (per craft-fonts' attribution) | https://github.com/storytold/craft-fonts/blob/main/ATTRIBUTION.md | SIL Open Font License 1.1 (each font's OFL.txt in craft-fonts; release packages ship it as OFL-<family>.txt) | 2026-10-06 | none; embedded only in builds made with `CRAFT_FONTS_DIR` (all official releases); the web (wasm32) build embeds BIZ UDPGothic Regular only |

Notes
- Fonts authored or published by Adobe (Source Sans/Serif/Code, Source Han, …) are not used, even though some are
  OFL-licensed: the asset rule excludes anything from Adobe. Source Sans 3 was removed on 2026-09-30 and replaced by Inter.
- Japanese fonts (BIZ UDPGothic, BIZ UDMincho, Shippori Mincho) are not in this repository. They live in
  [storytold/craft-fonts](https://github.com/storytold/craft-fonts), with their licences and attribution in its
  `ATTRIBUTION.md`, and are embedded only in builds made with the optional `CRAFT_FONTS_DIR` build input (all official
  releases). Never commit font files here; add new fonts to craft-fonts (craftrules `standards/fonts.md`).
