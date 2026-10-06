# Lightroom parity tracker

A living checklist of every Lightroom feature LightCraft aims to match, what we have, and what is missing. Agents and
humans pick work from **[Top gaps](#top-gaps)**; whoever lands a feature updates its row in the same commit.

- **Ids** come from the (local, gitignored) reference in `plan/lightroom/`: `LR-…` = feature catalog (03),
  `MENU-…` = menu items (04), `KEY-…` / `KEYC-…` = keyboard shortcuts (06, desktop / Classic), `LRC-…` = Classic
  extras (08). Ids are stable; never renumber.
- **Tier** as in the catalog: **P0** core, **P1** important parity, **P2** later / AI / niche, **OOS** out of scope.
- **Status**: ✅ done (usable end to end; polish may be noted) · 🟡 partial (note says what is missing, or the status
  is unverified) · ⬜ missing · 🚫 out of scope / not applicable.
- **Evidence**: `cmd:<id>` = command id (engine `command_specs()` or UI `UI_COMMANDS`), `ctl:<id>` = develop control
  id (`lightcraft-cli controls`), plus source files. A trailing `*` matches a prefix (`ctl:mixer.*`).
- Feature names and notes are our own words. No Adobe text, screenshots or assets belong here.

`cargo xtask parity` checks that every `cmd:`/`ctl:` id and file path below still exists (part of `cargo xtask ci`)
and prints the summary; `cargo xtask parity --write` refreshes the summary table below.

## Summary

<!-- parity:summary -->
| Section | ✅ | 🟡 | ⬜ | 🚫 | P0 done | P1 done |
|---|---:|---:|---:|---:|---:|---:|
| A. Import (IMP) | 11 | 3 | 1 | 1 | 3/5 (60%) | 5/5 (100%) |
| B. Library management (LIB) | 22 | 1 | 2 | 2 | 9/9 (100%) | 9/9 (100%) |
| C. Views & navigation (VIEW) | 16 | 0 | 1 | 0 | 9/9 (100%) | 4/4 (100%) |
| D. Search & filter (FILT) | 11 | 0 | 2 | 0 | 4/4 (100%) | 4/4 (100%) |
| E. Metadata (META) | 5 | 1 | 0 | 0 | 2/2 (100%) | 2/2 (100%) |
| F. Edit panel — global adjustments (EDIT) | 42 | 1 | 5 | 1 | 28/28 (100%) | 13/14 (93%) |
| G. Profiles (PROF) | 6 | 2 | 3 | 0 | 3/4 (75%) | 2/3 (67%) |
| H. Crop & rotate (CROP) | 9 | 0 | 0 | 1 | 6/6 (100%) | 3/3 (100%) |
| I. Remove / healing (REM) | 7 | 1 | 2 | 2 | 4/4 (100%) | 2/3 (67%) |
| J. Red eye (EYE) | 2 | 0 | 0 | 0 | — | 1/1 (100%) |
| K. Masking (MASK) | 14 | 3 | 6 | 0 | 8/8 (100%) | 5/5 (100%) |
| L. Presets (PRE) | 6 | 0 | 1 | 1 | 2/2 (100%) | 2/2 (100%) |
| M. Versions & history (VER) | 5 | 0 | 0 | 0 | 1/1 (100%) | 3/3 (100%) |
| N. Copy / paste / sync (SYNC) | 5 | 0 | 0 | 0 | 3/3 (100%) | 1/1 (100%) |
| O. Merge (MERGE) | 4 | 0 | 0 | 0 | — | — |
| P. Enhance (ENH) | 0 | 0 | 2 | 0 | — | — |
| Q. HDR (HDR) | 0 | 0 | 5 | 0 | — | — |
| R. Video (VID) | 0 | 0 | 5 | 1 | — | 0/2 (0%) |
| S. Export (EXP) | 15 | 1 | 2 | 0 | 7/7 (100%) | 7/7 (100%) |
| T. Share (SHARE) | 0 | 0 | 0 | 4 | — | — |
| U. Map & location (MAP) | 0 | 1 | 1 | 0 | — | 0/1 (0%) |
| V. Preferences (PREF) | 5 | 0 | 3 | 3 | 1/1 (100%) | 4/4 (100%) |
| W. Cloud & AI infrastructure (CLOUD / AI) | 1 | 0 | 1 | 2 | — | — |
| X. Cross-cutting behaviours (BEHAV) | 15 | 3 | 2 | 1 | 8/8 (100%) | 6/8 (75%) |
| Y. Menus | 81 | 0 | 4 | 8 | 47/47 (100%) | 23/23 (100%) |
| Z. Keyboard shortcuts (desktop) | 74 | 3 | 3 | 1 | 49/52 (94%) | 22/23 (96%) |
| Lightroom Classic extras | 31 | 9 | 40 | 9 | — | 21/22 (95%) |
| **Total** | 387 | 29 | 91 | 37 | 194/200 (97%) | 139/149 (93%) |

Weighted completion (✅ = 1, 🟡 = ½, 🚫 left out): **79.2%** of 507 in-scope rows — P0 98.5% of 200 · P1 95.6% of 149 · P2 39.2% of 158.
<!-- /parity:summary -->

## Top gaps

Ordered by user impact, then tier, then effort. The checklist above counts features that *exist*; these are the gaps
that decide whether a photographer can switch (see the honest assessment in [ROADMAP.md](../ROADMAP.md#where-we-stand)).
Take the first one nobody is working on.

1. **LR-PROF-CAMERACOLOR** (P0): our own camera colour calibration. Sony ARW now gets a guarded file-local fit to its own embedded JPEG (colour matrix + tone curve, relative WB; 7 of 10 public samples accepted, mean ΔE vs the camera JPEG 17–26 → 3–10); measured calibration and fidelity remain missing. Other non-DNG raws and rejected fits still use a neutral matrix. Expand validated preview fitting and use matrices the files carry themselves; never Adobe data.
2. **LR-IMP-FORMATS** (P0): **CR3** first (every Canon body since ~2018), then compressed RAF / ORF, RW2 v4, Nikon
   "lossy after split" NEF, Canon sRAW; HEIC/AVIF decode. Clean-room, from prose descriptions only (see
   `crates/raw/src/vendor/nefc.rs` for how compressed NEF was done). Until decoded, such photos are `preview_only`.
3. **LR-IMP-CAMERA-COVERAGE** (P0): per-model verification; grow the CC0 corpus and fix per-model bugs (like the CR2
   colour-filter layout, fixed in #85 by reading the file's own tag).
4. **LR-BEHAV-RENDER-FIDELITY** (P1): a side-by-side fidelity suite against Lightroom renders (kept local in `plan/`),
   then tune tone, highlights, texture/clarity/dehaze, NR and sharpening against it.
5. **LR-EDIT-OPTICS-PROFILE** (P1): a lens-profile database of our own (embedded DNG/maker corrections work today).
6. **AI masks and Enhance** (LR-MASK-SUBJECT / SKY / PEOPLE / OBJECTS, LR-EDIT-DETAIL-DENOISE, SUPERRES, LENSBLUR):
   blocked on a model strategy (permissively licensed weights or our own training, pure-Rust inference). A maintainer
   decision, not just engineering.
7. **HDR** (Q. HDR, LR-EXP-HDR), **video** (R. Video), **Classic output modules** (Map view, Book, Slideshow module,
   Print, publish): large, well understood, lower priority than 1–5.

## Shortcuts: conflicts and missing bindings

Compared `plan/lightroom/06-shortcuts.md` (desktop part) with our bindings: command specs in `crates/engine/src/cmd/`,
`UI_COMMANDS` in `crates/ui-egui/src/menus.rs` and secondary bindings (`ALIASES`) in `crates/ui-egui/src/shortcuts.rs`.
`no_conflicting_bindings` (same file) fails when one key fires two actions.

**Fixed (M16.1):** added Lightroom-desktop keys as secondary bindings for existing commands — ⌘D Select None, ⇧E
Export dialog, Space Toggle zoom, ⇧M Create Version, ⇧X Reject + advance, ⇧U Unflag + advance. `⇧Y` fired both
Before/After Split and the History panel; History lost the binding. `W` and `⇧⌘I` also fired the engine command
under the UI command that wraps it (a no-op error); the UI command now wins.

**Deliberate differences (our key → Lightroom desktop key)** — each is a conflict with another binding we have:

| Action | Ours | Lightroom desktop | Why |
|---|---|---|---|
| Pick flag | P | Z | Z = toggle zoom (Classic convention); P is the Classic pick key |
| Photos panel (left) | ⌘⇧L | P | P = pick |
| Expand/collapse edit sections | ⌘⌥1–5 | ⌘1–6 | ⌘0 = Zoom to Fit, ⌘1 = Zoom 100 %; Geometry lives in the Crop panel |
| Histogram | ⌘⇧H | ⌘0 | ⌘0 = Zoom to Fit |
| Square Grid | ⇧G | (none; ⇧G = Guided Upright) | Guided Upright is a button in the Crop panel |
| Export dialog | ⌘⇧E (+ ⇧E) | ⇧E | ⌘⇧E is "Edit in Photoshop" there; no external-editor command yet |
| Crop overlay cycle | ⇧O | O | O = mask overlay; ⇧O (mask colour / overlay orientation) unused otherwise |
| Create Version | ⌘⇧S (+ ⇧M) | ⇧M (Windows: Ctrl+⇧S) | — |
| Select None | ⌘⇧A (+ ⌘D) | ⌘D | — |

**Still missing / broken:**
- No command yet: F1 help, ⇧6–9 label + advance (verify the rest of the old list: full screen, settings, stacks,
  visualize spots and merges have commands now).
- `H` opens Remove; Lightroom also uses it (Classic) to hide pins — pins toggle from View → Show Mask Pins.
- ⌘M / ⌘H / ⌘Q / ⌘W rely on the platform window defaults (unverified).

<!-- Sections below hold one row per id. Keep the column order: Id | Feature | Tier | Status | Evidence | Notes. -->

## A. Import (IMP)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-IMP-ADD-DIALOG | Add photos/folders | P0 | ✅ | `cmd:file.addPhotos`, `cmd:library.importPreview`, `cmd:library.import` (`mode`, `album`, `albumName`, `preset`, `keywords`), `crates/ui-egui/src/import.rs` | review dialog (Import Photos): the scanned source (a scanned folder is not added to Local), candidate grid with thumbnails and checkboxes (duplicates by path/content marked and unchecked), transfer mode (add in place / copy / move, each explained), album (existing/new), preset, keywords; the folder scan runs on a background thread with a progress window and Cancel (a NAS folder no longer freezes the window); the import itself (probing, sidecars, copying / moving files) runs on a worker thread and its batches join the library between frames, with a progress window and Cancel; one undo step. Drag-and-drop goes through the same background import |
| LR-IMP-DRAGDROP | Drop files/folders to import | P0 | ✅ | `crates/ui-egui/src/lib.rs` (dropped files → `cmd:library.import`) | dropping onto a specific album not supported |
| LR-IMP-DUPES | Skip duplicates by content | P1 | ✅ | `crates/engine/src/import.rs`, `crates/engine/src/tests_import.rs` | |
| LR-IMP-DEVICE | Import from camera/card | P1 | ✅ | `cmd:library.devices`, `cmd:file.addFromDevice`, `crates/engine/src/devices.rs` | mounted volumes with a DCIM folder (macOS /Volumes, Linux /media and /run/media, Windows drive letters); File → Import from Device → the import review, copying into the library by default; menus serve the last scan and rescan in the background (never blocking a frame; hot-plugs repaint); copies are verified (a new file, synced, checked against the content hash the scan computed — byte compare with the source when there is none, issue #134; a bad copy is removed and reported as failed, never counted as imported; taken names get -1, -2…, never replaced — issue #96, `crates/engine/src/import_move.rs`); no PTP/MTP (cameras that don't mount as a disk) |
| LR-IMP-AUTO | Watched-folder auto import | P2 | ✅ | `cmd:library.autoImport`, `cmd:library.autoImportScan`, `crates/ui-egui/src/panels/settings.rs` | Settings ▸ Import ▸ Auto Import: a watched folder whose new photos are added (in place or copied) once complete, into an optional album; scanned every 3 s |
| LR-IMP-PRESET | Preset on import | P2 | ✅ | `cmd:library.import` (`preset`) | chosen in the import review; one History entry |
| LR-IMP-RAWDEFAULT | Raw defaults | P1 | ✅ | `cmd:library.preferences`, `crates/engine/src/import.rs`, `crates/ui-egui/src/panels/settings.rs` | LightCraft default / a preset / per camera (make + model); non-raw default too; the preset look counts as unedited and Reset returns to it |
| LR-IMP-MIGRATE | Migrate other catalogs | OOS | 🚫 | | |
| LR-IMP-PROFILES | Import profiles & presets | P1 | ✅ | `cmd:file.importPresets`, `cmd:preset.import`, `cmd:profile.import`, `crates/engine/src/preset_import.rs`, `crates/engine/src/preset_luminar.rs` | presets: .lcpreset, XMP, classic .lrtemplate, photos carrying edits ("DNG presets"), Luminar looks (.lmp, .mplumpack collections; sliders with a counterpart), .zip bundles, folders (folder → group), drag & drop; masks carried over; unmapped settings reported; profiles: .cube 3D LUTs; Adobe profile formats deliberately unsupported |
| LR-IMP-LOCAL | Work on files in place | P0 | ✅ | `cmd:library.browse`, `cmd:photo.addToLibrary`, `cmd:library.import` (mode add), `cmd:local.addRoot`, `cmd:local.hide`, `cmd:local.restoreHidden`, `crates/engine/src/cmd/browse.rs`, `crates/ui-egui/src/panels/left.rs` (`local_section`), `crates/ui-egui/src/panels/grid.rs` (`folder_header`) | Local: browse Pictures / Desktop / Downloads / Home or any folder without adding it (breadcrumb, Include subfolders, Add N to My Photos); browsed photos stay out of All Photos, albums and counts; folders picked with Browse Folder… (or Keep in Local / Add to Local on a folder row) stay listed across restarts, and browsing a folder inside a listed one highlights it inside that tree (opened down to it, siblings reachable) instead of adding a root; edits go to XMP sidecars; importing promotes them; right-click a top-level location → Remove from Local hides the shortcut only (nothing on disk or in the catalog changes), saved with the UI state, and “Show N hidden locations” (or Browse Folder… on it) brings it back; a folder is one location however its path is spelled (`/` or `\`, trailing separator, `.`/`..`, drive-letter case; `folder_key` in `crates/catalog/src/query.rs`); browsed photos nobody changed are forgotten once their folder has not been browsed for 30 days (Settings → Performance → Local folders, 0 = never; checked when the library opens, or `cmd:library.forgetLocal` with `dryRun`; files and sidecars stay, browsing again brings them back; see `crates/catalog/src/local.rs` for what counts as a change) |
| LR-IMP-SIDECAR-SPLIT | Separate XMP sidecar variants | P2 | ⬜ | `cmd:library.xmpPreferences` | sidecar naming option exists (stem/full), no split sidecars |
| LR-IMP-FORMATS | Supported formats | P0 | 🟡 | `crates/codecs/src/lib.rs`, `crates/raw/src/lib.rs` | JPEG, PNG, TIFF, WebP, JXL, PSD, GIF, BMP; DNG (incl. lossy / Smart Preview DNG), CR2 (colour-filter layout from the file's `CR2CFAPattern` tag, issue #85), ARW, NEF (uncompressed, lossless and lossy compressed 12/14-bit, `crates/raw/src/vendor/nefc.rs`), RAF, RW2, PEF, ORF. Missing: CR3, compressed RAF/ORF, RW2 v4, NEF "lossy after split" (preview only: imported with `preview_only` = the decoder's reason, rendered as a rendered JPEG, flagged by a grid badge and loupe/Edit/Info notices, `previewOnly` in `catalog.query`; Reload clears it once the file decodes), HEIC/AVIF decode |
| LR-IMP-CAMERA-COVERAGE | Camera coverage, verified per model | P0 | 🟡 | `crates/raw/tests/corpus.rs`, `xtask/src/main.rs` | a supported container is not the same as every camera that writes it decoding correctly: ~40 CC0 corpus files (plus ~60 NEFs checked by hand) against the >1,000 models Lightroom lists; per-model differences still surface (e.g. the CR2 colour-filter layout differed by model until it was read from the file's own tag, issue #85, checked on 35 CR2 bodies). Grow the CC0 corpus per model and test each sample decodes with plausible colour |
| LR-IMP-CULL-AT-IMPORT | Culling analysis at import | P2 | 🟡 | `cmd:photo.analyze` | run Assisted Culling on the imported photos (they're selected after an import); not automatic |
| LR-IMP-MOVE | Move on import [Classic] | P1 | ✅ | `cmd:library.import` (`mode` move), `crates/engine/src/import_move.rs`, `crates/engine/src/tests_import_move.rs`, `crates/ui-egui/src/import.rs` | Transfer ▸ Move with Copy's destination, folders (day / month / one folder / custom template), rename template and example destination; XMP sidecars (both namings) move along; each source is removed only after its destination is written (hard link on the same volume, else copied, synced and compared byte for byte) and its catalog record is saved; failed, duplicate, unchecked files keep their sources, taken names get -1, -2…, never overwritten; files already in the destination/library are added in place; sources that can't be removed (read-only card) are kept and reported (`kept`). Undo removes the photos from the library but leaves the moved files at the destination. No Copy as DNG while moving |
| LR-IMP-DNG-CONVERT | Convert to DNG on import [Classic] | P2 | ✅ | `cmd:library.import` (`dng`), `crates/ui-egui/src/import.rs` | copy imports: Raw files ▸ Copy as DNG (lossless; the card is untouched); the raw copy is removed only once its DNG is verified and on disk (issue #106) |

## B. Library management (LIB)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-LIB-ALLPHOTOS | All photos | P0 | ✅ | `cmd:library.source`, `crates/ui-egui/src/panels/left.rs` | |
| LR-LIB-RECENT-ADDED | Recently added | P1 | ✅ | `cmd:library.source` (`recentlyAdded`), `crates/engine/src/view.rs` (`RECENT_DAYS`) | photos imported in the 30 days up to the latest import, newest import first, grouped by import day |
| LR-LIB-BYDATE | Browse by date | P1 | ✅ | `cmd:library.filter` (`date`), `crates/ui-egui/src/panels/left.rs` (`date_row`), `crates/catalog/src/query.rs` (`date_groups`) | year → month → day tree with library-wide counts; a row filters the current source by that date. Active filters show as removable chips under the grid header (`crates/ui-egui/src/panels/chips.rs`, with “+N more” and Clear all), the header counts “N of M photos” (matching of the source's total), a badge on the top-bar filter icon counts them, and an empty result says no photos match the active filters |
| LR-LIB-ALBUM | Albums | P0 | ✅ | `cmd:album.create`, `cmd:album.rename`, `cmd:album.delete`, `cmd:album.addPhotos`, `cmd:album.removePhotos`, `cmd:dialog.newAlbum` | no drag photos onto album (LR-BEHAV-DRAGDROP), no album sort |
| LR-LIB-FOLDER | Folders of albums | P0 | ✅ | `cmd:album.create` (`folder`), `cmd:album.move` | moving is command-only (no drag, no "Move to" menu) |
| LR-LIB-SMARTALBUM | Smart albums | P1 | ✅ | `cmd:album.createSmart`, `cmd:album.setRules`, `crates/catalog/src/query.rs`, `crates/ui-egui/src/panels/filterbar.rs`, `crates/ui-egui/src/lib.rs` (`album_counts`) | saved filters (rating/flag/label/kind/edited/keyword/camera/lens/date range/text/album), live; match-all only (no any/none rule groups, no rule editor dialog — rules come from the filter bar or `album.setRules`); sidebar counts are cached per catalog revision (and per minute while an “in the last…” rule is used), counted without building id lists |
| LR-LIB-SHARED-ALBUM | Shared albums | P2 | ⬜ | | needs a sharing service |
| LR-LIB-OFFLINE | Keep album offline | P2 | 🚫 | | not applicable: local-first library |
| LR-LIB-TARGET | Target album | P2 | ✅ | `cmd:album.setTarget`, `cmd:album.toggleTarget` | see LRC-LIB-COLLECTIONS |
| LR-LIB-RATING | Star ratings | P0 | ✅ | `cmd:photo.rate` (`advance`), `crates/ui-egui/src/shortcuts.rs` | |
| LR-LIB-FLAG | Pick / reject flags | P0 | ✅ | `cmd:photo.pick`, `cmd:photo.reject`, `cmd:photo.unflag`, `cmd:photo.flag` | pick key is P (see Shortcuts); no flag cycle |
| LR-LIB-LABEL | Colour labels | P1 | ✅ | `cmd:photo.label`, `cmd:label.setNames`, `cmd:label.names`, `cmd:dialog.labelNames`, keys 6–9 in `crates/ui-egui/src/shortcuts.rs` | Photo menu and grid context menu (coloured, named), Info-panel swatches, label dot in grid cells; editable label names (undoable, journaled); no purple key (as in Lightroom), no custom extra labels |
| LR-LIB-KEYWORD | Keywords | P0 | ✅ | `cmd:panel.keywords`, `cmd:photo.setMeta` (`addKeywords`/`removeKeywords`), `cmd:keyword.list`, `cmd:keyword.rename`, `cmd:keyword.delete`, `cmd:keyword.merge`, `cmd:keyword.suggest`, `crates/catalog/src/keywords.rs`, `crates/ui-egui/src/panels/left.rs` | left-panel keyword tree with counts (click filters, children included; context menu rename / merge / delete / add / remove); library-wide ops are one undo step and replay from the op log; suggestions (co-occurring / most used / completions) in the Keywords panel; no keyword drag-and-drop |
| LR-LIB-PEOPLE | People / faces | P2 | ⬜ | | |
| LR-LIB-STACK | Stacks | P1 | ✅ | `cmd:stack.group`, `cmd:stack.ungroup`, `cmd:stack.toggle`, `cmd:stack.setTop`, `cmd:stack.remove`, `cmd:stack.auto`, `crates/catalog/src/stacks.rs` | grid/filmstrip count badges, expand/collapse, auto-stack by capture time; no visual-similarity auto-stack |
| LR-LIB-VERSIONS | Versions | P1 | ✅ | `cmd:version.create` | see section M |
| LR-LIB-DELETE | Delete / Recently Deleted | P0 | ✅ | `cmd:photo.delete`, `cmd:photo.restore`, `cmd:photo.deletePermanently` | no confirmation dialog, no auto-purge after N days, no "Empty" |
| LR-LIB-REMOVE-ALBUM | Remove from album | P0 | ✅ | `cmd:album.removePhotos` | |
| LR-LIB-DUPLICATE | Duplicate a photo | P2 | ✅ | `cmd:photo.duplicate` | Photo ▸ Duplicate: a real `-copy` file with the same settings, metadata and albums (virtual copies share the file) |
| LR-LIB-RENAME | Batch rename | P1 | ✅ | `cmd:photo.rename`, `cmd:photo.renamePreview`, `cmd:photo.renameTokens`, `cmd:dialog.rename`, `crates/engine/src/rename.rs` (`TOKENS`) | template tokens {name} {num} {seq:N} {date:%Y%m%d} {folder} {camera} {lens} {iso} {rating} {title} {creator} {ext} (shared with export naming and import rename); a Tags picker beside every template field (Rename Photos, import Rename, export File name) lists each tag with meaning + live example, the date directives and the rules, and inserts the clicked tag at the text cursor; unknown tags are flagged; preview; renames files on disk with their XMP sidecars (a stem sidecar another file still shares, e.g. raw + JPEG, is copied rather than taken), never overwriting (-1, -2… suffixes; a change of letter case only is checked by file identity, so on case-sensitive volumes `img_1.jpg` next to `IMG_1.JPG` is a collision), rolls back on failure (a file that can't be moved back is reported old → new and the library follows it as an undoable partial rename; the same for undo/redo); undo/redo move the files; virtual copies follow |
| LR-LIB-CAPTURETIME | Edit capture time | P1 | ✅ | `cmd:photo.setCaptureTime`, `cmd:dialog.captureTime`, `crates/catalog/src/dates.rs` | set (the other selected photos shift by the same amount, or `each`), shift by days/hours/minutes, time-zone shift; one undo step, journaled; Info panel button; no “revert to original capture time” |
| LR-LIB-SHOWFINDER | Reveal original in file manager | P0 | ✅ | `cmd:app.showInFinder` | ⌘R (see MENU-FILE-SHOWFINDER) |
| LR-LIB-COVER | Album cover | P2 | ✅ | `cmd:album.setCover` | |
| LR-LIB-CULL | Assisted culling | P2 | 🟡 | `cmd:photo.analyze`, `cmd:dialog.cull`, `crates/pipeline/src/cull.rs` | Photo ▸ Assisted Culling…: focus score (0–100), clipping, burst grouping (look-alike shots within 10 s) with the sharpest marked; reject below a focus score, pick each burst's best; classical measures — no eyes-closed / expression detection |
| LR-LIB-ACTIVITY | Comments & likes | OOS | 🚫 | | |
| LR-LIB-QUICKCOLL | Quick collection [Classic] | P2 | ✅ | `cmd:album.toggleTarget`, `cmd:album.clearQuick` | B in the grids |
| LR-LIB-VIRTUALCOPY | Virtual copies [Classic] | P1 | ✅ | `cmd:photo.virtualCopy`, `crates/engine/src/cmd/organize.rs` | “Copy N” badge, stacked with the original, same albums, no XMP writes; no “Set Copy as Master” |

## C. Views & navigation (VIEW)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-VIEW-PHOTOGRID | Justified photo grid | P0 | ✅ | `cmd:view.photoGrid`, `cmd:library.groups`, `cmd:library.sort` (`group`), `crates/ui-egui/src/panels/grid.rs`, `crates/catalog/src/dates.rs` | grouped by capture / import / edit date (day headers “Wednesday, 30 September 2026 · 12 photos”, months or years when zoomed out, or a fixed day/month/year/none choice in the sort menu); sticky header; clicking a header selects its photos; stacks never split; scrolls to the active photo only when it changes (or the grid comes back on screen), never on its own |
| LR-VIEW-SQUAREGRID | Square grid | P0 | ✅ | `cmd:view.squareGrid` | same date headers as the photo grid |
| LR-VIEW-DETAIL | Single-photo view | P0 | ✅ | `cmd:view.detail`, `crates/ui-egui/src/panels/detail.rs` | |
| LR-VIEW-EDIT | Edit view | P0 | ✅ | `cmd:panel.edit` | |
| LR-VIEW-FULLSCREEN | Full-screen preview | P1 | ✅ | `cmd:view.fullScreenPreview`, `cmd:view.enterFullScreen` | photo on black, arrows step, Esc exits; ⇧⌘F window full screen |
| LR-VIEW-FILMSTRIP | Filmstrip | P0 | ✅ | `cmd:view.filmstrip`, `crates/ui-egui/src/panels/detail.rs` | the mouse wheel scrolls it sideways; it follows the active photo only when that changes (centred if off screen), so a scrolled strip stays put |
| LR-VIEW-ZOOM | Zoom & pan | P0 | ✅ | `cmd:view.zoomFit`, `cmd:view.zoom100`, `cmd:view.zoomIn`, `cmd:view.zoomOut`, `cmd:view.zoomToggle`, `cmd:view.clickZoom` | steps 25–800 % (not 6–1600 %); Fill only in the bottom bar; a click on the photo (and Z / Space) eases to the click-zoom ratio (1:1 default, 2:1, 3:1, 4:1, 8:1 in the bottom bar) |
| LR-VIEW-NAVIGATOR | Navigator mini map | P1 | ✅ | `cmd:view.navigator`, `crates/ui-egui/src/panels/detail.rs` | shown while zoomed (bottom right); click/drag pans |
| LR-VIEW-BEFOREAFTER | Before / after | P0 | ✅ | `cmd:view.showOriginal`, `cmd:view.beforeAfter`, `cmd:view.beforeAfterSplit`, `cmd:view.beforeAfterTopBottom`, `cmd:view.beforeAfterSplitTopBottom`, `cmd:beforeAfter.setBefore`, `cmd:beforeAfter.copyAfterToBefore`, `cmd:beforeAfter.copyBeforeToAfter`, `cmd:beforeAfter.swap`, `cmd:beforeAfter.resetBefore` | all four layouts; before = the import state (defaults + import preset) or a chosen history step / version / the current settings; copy and swap (View → Before/After Settings, History row menu). The chosen before lasts for the session |
| LR-VIEW-COMPARE | Compare two photos | P1 | ✅ | `cmd:view.compare`, `cmd:compare.swap`, `cmd:compare.makeSelect`, `crates/ui-egui/src/panels/compare.rs` | select / candidate, synced zoom + pan, arrows move the candidate; no zoom-link toggle |
| LR-VIEW-SURVEY | Survey view [Classic] | P2 | ✅ | `cmd:view.survey`, `crates/ui-egui/src/panels/compare.rs` | selection tiled (≤ 48), keys act on the active photo, hover × removes |
| LR-VIEW-INFOOVERLAY | Info overlay on the photo | P1 | ✅ | `cmd:view.infoOverlay` | off / file + date + size / exposure + camera; ⌘I cycles (I in full screen; elsewhere I stays the Info panel) |
| LR-VIEW-SLIDESHOW | Slideshow | P2 | ✅ | `cmd:view.slideshow` | View ▸ Slideshow (⌥⌘↩): the photos in view full screen, every 4 s (`interval`), wrapping; Space pauses, ←/→ step, Esc ends |
| LR-VIEW-SECONDWINDOW | Second display window [Classic] | P2 | ✅ | `cmd:view.secondWindow`, `crates/ui-egui/src/panels/second.rs` | Window ▸ Second Window (⌘F11): the active photo fitted in its own native window with its own render (a floating panel where there are no native windows); loupe view only (no grid / compare / survey there) |
| LR-VIEW-CLIPPING | Clipping indicators | P0 | ✅ | `cmd:view.clipping` | |
| LR-VIEW-HISTOGRAM | Histogram | P0 | ✅ | `cmd:view.histogram`, `crates/ui-egui/src/panels/edit.rs` | no drag-to-adjust on the histogram |
| LR-VIEW-HDR-DISPLAY | HDR display output | P2 | ⬜ | | |

## D. Search & filter (FILT)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-FILT-SEARCH-META | Text search | P0 | ✅ | `cmd:library.filter` (`text`), `crates/ui-egui/src/panels/topbar.rs`, `crates/catalog/src/query.rs` | fielded tokens (`rating:3`, `iso:>800`, `camera:…`); no suggestions dropdown |
| LR-FILT-SEARCH-AI | Natural-language search | P2 | ⬜ | | |
| LR-FILT-RATING | Rating filter | P0 | ✅ | `cmd:library.filter` (`rating`, `ratingOp`), `crates/ui-egui/src/panels/filterbar.rs` | ≥ / = / ≤ stars in the filter bar (`cmd:view.filterBar`) |
| LR-FILT-FLAG | Flag filter | P0 | ✅ | `cmd:library.filter` (`flag`), `crates/ui-egui/src/panels/filterbar.rs` | picked / rejected / unflagged (one at a time) |
| LR-FILT-LABEL | Colour-label filter | P1 | ✅ | `cmd:library.filter` (`label`), `crates/ui-egui/src/panels/filterbar.rs` | one label at a time; no “no label” choice |
| LR-FILT-TYPE | Type / edited filter | P1 | ✅ | `cmd:library.filter` (`kind`, `merged`, `edited`), `crates/ui-egui/src/panels/filterbar.rs`, `crates/catalog/src/query.rs` (`merged_kind`) | photos / raw / videos / HDR / panoramas / HDR panoramas (merge results by name), edited / unedited; no depth kind |
| LR-FILT-KEYWORD | Keyword filter | P1 | ✅ | `cmd:library.filter` (`keyword`), `crates/ui-egui/src/panels/filterbar.rs` | keyword picker |
| LR-FILT-CAMERA | Camera / lens filter | P1 | ✅ | `cmd:library.filter` (`camera`, `lens`), `crates/ui-egui/src/panels/filterbar.rs` | camera and lens pickers |
| LR-FILT-LOCATION | Location filter | P2 | ✅ | `cmd:library.filter` (`text`, `ruleSet` field `location`) | free text, or a rule on location / city / state / country (smart albums, `library.filter`) |
| LR-FILT-PEOPLE | People filter | P2 | ⬜ | | |
| LR-FILT-CULL | Culling-score filters | P2 | ✅ | `cmd:library.filter` (`ruleSet` fields `sharpness`, `bestOfGroup`) | Focus and Best of Similar Shots in the rule editor / smart albums |
| LR-FILT-SORT | Sort | P0 | ✅ | `cmd:library.sort`, `crates/ui-egui/src/panels/bottombar.rs` | no colour-label or custom (manual) order |
| LR-FILT-SAVED | Filter presets [Classic] | P2 | ✅ | `cmd:filter.savePreset`, `cmd:filter.applyPreset`, `cmd:filter.presets`, `cmd:filter.deletePreset`, `crates/ui-egui/src/panels/filterbar.rs` | filter bar → Presets: apply, save current filter, delete (right-click); saved with the library |

## E. Metadata (META)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-META-INFO | Info panel | P0 | ✅ | `cmd:panel.info`, `cmd:photo.setMeta`, `crates/ui-egui/src/panels/right.rs` (`info`, `camera_card`) | camera card (camera, lens, size, format, focal length / shutter / aperture / ISO); title, caption, alt text, extended description, copyright, copyright status, rights usage terms, copyright info URL, creator; file name (rename), file path (reveal), capture time (edit); location, city, state, country; GPS. No flash, map snippet or people |
| LR-META-COPYRIGHT-DEFAULT | Default copyright on import | P1 | ✅ | `cmd:library.preferences` (`import.copyright`, `import.creator`), `crates/ui-egui/src/panels/settings.rs` | Settings → Import → Metadata; fills only photos without their own |
| LR-META-LOCATION | Location editing | P2 | 🟡 | `cmd:photo.setMeta` (`location`, `city`, `state`, `country`, `gps`) | place fields and GPS (decimal or degrees / minutes / seconds) edited in Info, read/written as IPTC Core / Photoshop / EXIF XMP; Show on Map opens OpenStreetMap; no embedded map, no geocoding |
| LR-META-COPYPASTE | Copy / paste metadata | P2 | ✅ | `cmd:photo.copyMetadata`, `cmd:photo.pasteMetadata` | title, caption, alt text, extended description, copyright (notice, status, usage terms, info URL), creator, place fields, keywords; `fields` picks a subset |
| LR-META-XMP | XMP read/write | P0 | ✅ | `cmd:photo.saveMetadataToFile`, `cmd:photo.readMetadataFromFile`, `cmd:library.xmpPreferences`, `crates/engine/src/sidecar.rs`, `docs/xmp-interop.md` | sidecar wins for metadata; a sidecar capture time (`exif:DateTimeOriginal` / `photoshop:DateCreated` / `xmp:CreateDate`) fills in only when the file has none; saving merges into an existing sidecar (`crates/meta/src/xmp_merge.rs`): another app's `crs:`, `xmpMM:History` and unknown namespaces are kept byte for byte, an unreadable sidecar is backed up first; with stem naming, files sharing a stem (raw + JPEG) get separate sidecars (`<file>.xmp` for all but the raw) |
| LR-META-EXIF-FULL | Full EXIF/IPTC [Classic] | P1 | ✅ | `cmd:photo.allMetadata`, `cmd:dialog.allMetadata`, `crates/meta/src/tags.rs`, `crates/meta/src/exif.rs`, `crates/meta/src/iptc.rs` | read and written on export; Info ▸ All Metadata…: every TIFF / EXIF / GPS / interop tag (named, common values spelled out) and the XMP fields, searchable; metadata presets |

## F. Edit panel — global adjustments (EDIT)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-EDIT-AUTO | Auto settings | P0 | ✅ | `cmd:develop.auto`, `crates/pipeline/src/auto.rs` | |
| LR-EDIT-BW | Black & white | P0 | ✅ | `cmd:develop.treatment` | |
| LR-EDIT-HDR-MODE | HDR editing | P2 | ⬜ | | |
| LR-EDIT-LIGHT-EXPOSURE | Exposure | P0 | ✅ | `ctl:light.exposure` | |
| LR-EDIT-LIGHT-CONTRAST | Contrast | P0 | ✅ | `ctl:light.contrast` | |
| LR-EDIT-LIGHT-HIGHLIGHTS | Highlights | P0 | ✅ | `ctl:light.highlights` | |
| LR-EDIT-LIGHT-SHADOWS | Shadows | P0 | ✅ | `ctl:light.shadows` | |
| LR-EDIT-LIGHT-WHITES | Whites | P0 | ✅ | `ctl:light.whites` | |
| LR-EDIT-LIGHT-BLACKS | Blacks | P0 | ✅ | `ctl:light.blacks` | |
| LR-EDIT-LIGHT-CURVE-PARAM | Parametric curve | P0 | ✅ | `ctl:curve.highlights`, `ctl:curve.lights`, `ctl:curve.darks`, `ctl:curve.shadows`, `ctl:curve.split*` | |
| LR-EDIT-LIGHT-CURVE-POINT | Point curve | P0 | ✅ | `cmd:develop.curve`, `cmd:curve.reset`, `cmd:curve.presets`, `cmd:curve.applyPreset`, `cmd:curve.savePreset`, `cmd:curve.deletePreset`, `cmd:curve.importPresets`, `cmd:curve.exportPresets`, `cmd:file.importCurvePresets`, `cmd:file.exportCurvePresets`, `crates/engine/src/cmd/curves.rs`, `crates/ui-egui/src/panels/edit.rs` | click to add a point; drag a point in both axes (between its neighbours, input / output readout, one undo step per drag); drag empty space to add and drag; double-click removes; reset: double-click a channel selector (that channel), the Reset button under the graph (every curve incl. parametric) or right-click the graph (channel / all); Point Curve preset dropdown: own Linear / Medium Contrast / Strong Contrast, user presets (save the current point curves, delete by right-click; saved with the library), import / export as `.lccurve` JSON |
| LR-EDIT-LIGHT-CURVE-RGB | Per-channel curves | P0 | ✅ | `cmd:develop.curve` (`channel`) | |
| LR-EDIT-LIGHT-CURVE-REFINESAT | Curve saturation compensation | P1 | ✅ | `ctl:curve.refineSaturation` | |
| LR-EDIT-LIGHT-CURVE-TAT | Drag-on-image curve adjust | P1 | ✅ | `cmd:develop.targeted` (`target: curve`) | |
| LR-EDIT-COLOR-WB-PRESET | White-balance presets | P0 | ✅ | `cmd:develop.wb` | |
| LR-EDIT-COLOR-WB-PICKER | White-balance eyedropper | P0 | ✅ | `cmd:tool.wbPicker`, `cmd:develop.wbPick` | no magnified loupe while picking |
| LR-EDIT-COLOR-TEMP | Temperature | P0 | ✅ | `ctl:wb.temp` | relative scale for non-raw in the UI |
| LR-EDIT-COLOR-TINT | Tint | P0 | ✅ | `ctl:wb.tint` | |
| LR-EDIT-COLOR-VIBRANCE | Vibrance | P0 | ✅ | `ctl:color.vibrance` | |
| LR-EDIT-COLOR-SATURATION | Saturation | P0 | ✅ | `ctl:color.saturation` | |
| LR-EDIT-COLOR-MIXER-HSL | 8-band colour mixer | P0 | ✅ | `ctl:mixer.*` | no targeted (drag-on-image) mode |
| LR-EDIT-COLOR-MIXER-BW | B&W mix | P1 | ✅ | `ctl:bw.*`, `cmd:develop.autoBwMix` | eight bands; Auto pushes each hue band's colourful pixels away from the mean lightness (own rule) |
| LR-EDIT-COLOR-POINTCOLOR | Point colour | P1 | ✅ | `cmd:pointColor.pick`, `cmd:pointColor.delete` | |
| LR-EDIT-COLOR-GRADING | Colour grading wheels | P0 | ✅ | `ctl:grading.*` | |
| LR-EDIT-EFFECTS-TEXTURE | Texture | P0 | ✅ | `ctl:effects.texture` | |
| LR-EDIT-EFFECTS-CLARITY | Clarity | P0 | ✅ | `ctl:effects.clarity` | |
| LR-EDIT-EFFECTS-DEHAZE | Dehaze | P0 | ✅ | `ctl:effects.dehaze` | |
| LR-EDIT-EFFECTS-VIGNETTE | Post-crop vignette | P0 | ✅ | `ctl:vignette.*`, `crates/pipeline/src/finish.rs`, `crates/ui-egui/src/panels/edit.rs` | style picker (Highlight / Color / Paint) in the Effects section |
| LR-EDIT-EFFECTS-GRAIN | Grain | P1 | ✅ | `ctl:grain.*` | |
| LR-EDIT-DETAIL-SHARPEN | Sharpening | P0 | ✅ | `ctl:detail.sharpenAmount`, `ctl:detail.sharpenRadius`, `ctl:detail.sharpenDetail`, `ctl:detail.sharpenMasking` | no Alt-drag mask preview |
| LR-EDIT-DETAIL-NR | Luminance noise reduction | P0 | ✅ | `ctl:detail.nrLuminance`, `ctl:detail.nrDetail`, `ctl:detail.nrContrast` | |
| LR-EDIT-DETAIL-CNR | Colour noise reduction | P0 | ✅ | `ctl:detail.nrColor`, `ctl:detail.nrColorDetail`, `ctl:detail.nrColorSmoothness` | |
| LR-EDIT-DETAIL-DENOISE | AI denoise | P2 | ⬜ | | settings field reserved, not rendered |
| LR-EDIT-DETAIL-RAWDETAILS | Improved demosaic toggle | P2 | ⬜ | | |
| LR-EDIT-DETAIL-SUPERRES | Super resolution | P2 | ⬜ | | |
| LR-EDIT-DETAIL-AISHARPEN | AI sharpen | OOS | 🚫 | | |
| LR-EDIT-OPTICS-CA | Remove chromatic aberration | P1 | ✅ | `crates/ui-egui/src/panels/edit.rs` (checkbox), `ctl:optics.caRed`, `ctl:optics.caBlue` | |
| LR-EDIT-OPTICS-PROFILE | Lens profile corrections | P1 | 🟡 | `ctl:optics.profileDistortion`, `ctl:optics.profileVignetting`, `crates/pipeline/src/optics.rs` | uses corrections embedded in DNG/raw files; no lens-profile database |
| LR-EDIT-OPTICS-DEFRINGE | Defringe | P1 | ✅ | `ctl:optics.defringe*` | no fringe eyedropper |
| LR-EDIT-OPTICS-MANUAL | Manual distortion / vignetting | P1 | ✅ | `ctl:optics.distortion`, `ctl:optics.vignetting`, `ctl:optics.vignettingMidpoint` | |
| LR-EDIT-GEOM-UPRIGHT | Upright | P1 | ✅ | `cmd:geometry.upright`, `cmd:geometry.guides` | |
| LR-EDIT-GEOM-MANUAL | Manual transform | P1 | ✅ | `ctl:geometry.*` | |
| LR-EDIT-GEOM-CONSTRAIN | Constrain crop | P1 | ✅ | `crates/ui-egui/src/panels/right.rs` (checkbox → `cmd:develop.merge`) | |
| LR-EDIT-GEOM-GRID | Grid while transforming | P2 | ✅ | `crates/ui-egui/src/panels/detail.rs` | a fine grid over the photo while a geometry slider is dragged |
| LR-EDIT-LENSBLUR | Lens blur | P2 | ⬜ | | settings field reserved, not rendered |
| LR-EDIT-CALIB | Calibration [Classic] | P1 | ✅ | `ctl:calibration.*` | shadows tint, red/green/blue primary hue and saturation; read/written in XMP |
| LR-EDIT-SECTION-TOGGLE | Section on/off | P1 | ✅ | `cmd:develop.sectionEnabled` | |
| LR-EDIT-RESET | Reset all / section / slider | P0 | ✅ | `cmd:develop.reset`, `cmd:develop.resetSection`, `cmd:develop.resetControl`, `crates/ui-egui/src/widgets.rs` (double-click) | no "reset to open" |
| LR-EDIT-SHOWORIG | Show original | P0 | ✅ | `cmd:view.showOriginal` | |

## G. Profiles (PROF)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-PROF-DROPDOWN | Profile menu | P0 | ✅ | `cmd:develop.profile`, `cmd:profiles.list`, `cmd:profiles.menu`, `cmd:profile.favorite`, `crates/ui-egui/src/panels/edit.rs` | Favorites, Recent (last 5), group submenus, favourite toggle; favourites/recent persist with the library; Amount slider under the menu for non-default profiles; Browse… opens the profile browser; resting on an entry previews it in the loupe |
| LR-PROF-BROWSER | Profile browser | P1 | ✅ | `cmd:panel.profiles`, `crates/ui-egui/src/panels/profiles.rs`, `crates/engine/src/media.rs` | grid of live variant thumbnails of the current photo per group (Favorites first), star toggles favourites, amount slider, hover = loupe preview (no history), click applies |
| LR-PROF-ADOBE | Standard raw looks (own equivalents) | P0 | ✅ | `crates/engine/src/presets.rs` (`PROFILES`), `crates/pipeline/src/profiles.rs` | six own looks: Color, Neutral, Vivid, Landscape, Portrait, Monochrome |
| LR-PROF-ADAPTIVE | Adaptive profiles | P2 | ⬜ | | |
| LR-PROF-CAMERA | Camera-matching looks | P2 | ⬜ | | |
| LR-PROF-CAMERACOLOR | Camera colour calibration (own) | P0 | 🟡 | `crates/raw/src/color.rs`, `crates/raw/src/profile.rs`, `crates/engine/src/camera_preview.rs` | DNG files use the colour matrices and the camera-profile look they carry (`ProfileHueSatMap`, `ProfileLookTable`, `ProfileToneCurve`, read from the file at run time per DNG spec ch. 6 — Lightroom-converted DNGs rendered flat and muted without them, issue #138); Sony ARW gets a guarded file-local fit to its own embedded JPEG — chromaticity matrix + tone curve, relative WB, kept with smart previews (see docs/camera-preview-colour.md); on 10 public raw.pixls.us samples from 8 bodies 7 fits were accepted, mean ΔE vs the camera JPEG 17–26 → 3–10; this is not measured camera calibration; other raws and rejected fits use a neutral fallback (camera RGB ≈ linear sRGB, `matrix_is_fallback`) with as-shot white balance, so colours are muted and not accurate. Needs our own per-camera calibration: matrices the files carry themselves (e.g. Olympus `ColorMatrix`), fitting each camera to its own embedded JPEG, then chart shots. Adobe matrices / DCPs are never used. Biggest image-quality gap today |
| LR-PROF-CREATIVE | Creative profiles (own) | P2 | ✅ | `cmd:develop.profile`, `crates/pipeline/src/profiles.rs` | 16 own looks in Film / Cinematic / Muted / B&W (tone + point-curve fades, colour grading, mixer / B&W mix); scale with `ctl:profile.amount`; sliders untouched |
| LR-PROF-LEGACY | Legacy profiles | P2 | ⬜ | | |
| LR-PROF-NONRAW | Profiles for non-raw files | P0 | ✅ | `cmd:develop.profile` | same looks apply to JPEG/TIFF |
| LR-PROF-AMOUNT | Profile amount | P1 | ✅ | `ctl:profile.amount` | |
| LR-PROF-IMPORT | Import profiles | P1 | 🟡 | `cmd:profile.import`, `cmd:profile.deleteImported`, `crates/pipeline/src/lut.rs`, `crates/engine/src/cmd/lut_profiles.rs` | `.cube` 3D LUTs (files, folders, zips) become creative profiles with Amount, grouped in the profile browser, kept with the library; rendered on the CPU; Adobe profile formats deliberately unsupported |

## H. Crop & rotate (CROP)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-CROP-RECT | Crop rectangle | P0 | ✅ | `cmd:crop.set`, `crates/ui-egui/src/panels/detail.rs` | double-click inside the box applies the crop (like Return) |
| LR-CROP-ASPECT | Aspect ratios | P0 | ✅ | `cmd:crop.aspect`, `cmd:crop.rotateAspect` | no "As Shot"; custom ratio via command params only |
| LR-CROP-STRAIGHTEN | Straighten tool | P0 | ✅ | `cmd:crop.straighten`, `cmd:crop.autoStraighten` | Straighten Tool button: drag along a horizon/vertical; double-click or Auto levels automatically |
| LR-CROP-AUTO | Auto straighten | P1 | ✅ | `cmd:crop.autoStraighten` | crop-angle leveling from detected horizon/plumb lines (consensus required) |
| LR-CROP-ANGLE | Angle slider | P0 | ✅ | `ctl:crop.angle` | |
| LR-CROP-ROTATE90 | Rotate 90° | P0 | ✅ | `cmd:photo.rotateLeft`, `cmd:photo.rotateRight` | |
| LR-CROP-FLIP | Flip | P0 | ✅ | `cmd:photo.flipHorizontal`, `cmd:photo.flipVertical` | |
| LR-CROP-OVERLAY | Crop overlays | P1 | ✅ | `cmd:view.cropOverlay`, `cmd:view.cropOverlayOrientation`, `crates/ui-egui/src/panels/crop_overlay.rs` | thirds, grid, golden ratio, diagonal, triangle, golden spiral (mirrored with ⇧O while cropping); no aspect-ratio overlays |
| LR-CROP-ZOOM | Zoom while cropping | P1 | ✅ | `cmd:view.zoom100`, `cmd:view.zoomIn`, `crates/ui-egui/src/panels/detail.rs` | zoom levels apply with the crop tool open (verified: 100% shows the photo at native size with the crop frame) |
| LR-CROP-GENEXPAND | Generative expand | OOS | 🚫 | | |

## I. Remove / healing (REM)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-REM-CONTENTAWARE | Content-aware remove | P1 | 🟡 | `cmd:spot.add` (`mode: remove`), `crates/pipeline/src/spots.rs` | heal with automatic source; no patch synthesis (M8.4) |
| LR-REM-HEAL | Heal | P0 | ✅ | `cmd:spot.add` (`mode: heal`) | |
| LR-REM-CLONE | Clone | P0 | ✅ | `cmd:spot.add` (`mode: clone`) | |
| LR-REM-GEN | Generative remove | OOS | 🚫 | | |
| LR-REM-DETECT | Object detection for remove | P2 | ⬜ | | |
| LR-REM-BRUSH-PARAMS | Brush size / feather / opacity | P0 | ✅ | `cmd:spot.add` (`size`, `feather`, `opacity`), `cmd:brush.smaller`, `cmd:brush.larger`, `cmd:brush.featherLess`, `cmd:brush.featherMore`, `crates/ui-egui/src/panels/right.rs` | Size / Feather / Opacity sliders (for new spots and the selected one); `[` `]` size, ⇧`[` ⇧`]` feather (also the Masking brush) |
| LR-REM-SPOT-EDIT | Edit existing spots | P0 | ✅ | `cmd:spot.select`, `cmd:spot.update`, `cmd:spot.refreshSource`, `cmd:spot.delete` | click a pin to select, drag target or source, ⌫ deletes the selected spot, `/` picks another source; automatic sources are resolved when the spot is added |
| LR-REM-VISUALIZE | Visualize spots | P1 | ✅ | `cmd:view.visualizeSpots` | |
| LR-REM-PEOPLE | Remove people (generative) | OOS | 🚫 | | |
| LR-REM-REFLECT | Remove reflections | P2 | ⬜ | | |
| LR-REM-DUST | Dust detection | P2 | ✅ | `cmd:spot.findDust`, `crates/pipeline/src/dust.rs` | Remove panel ▸ Find Dust Spots: small, soft, round dark spots on smooth areas become heal spots (sensitivity; one undo step); Visualize Spots for checking by eye |
| LR-REM-SYNC | Sync spots | P1 | ✅ | `cmd:develop.copy` (`groups`), `crates/develop/src/presets.rs` | |

## J. Red eye (EYE)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-EYE-RED | Red-eye correction | P1 | ✅ | `cmd:panel.redeye`, `cmd:redeye.add`, `cmd:redeye.delete`, `cmd:redeye.catchlight` | red eye and pet eye (with catchlight) |
| LR-EYE-PET | Pet eye | P2 | ✅ | `cmd:redeye.add` (`pet`), `cmd:redeye.catchlight` | |

## K. Masking (MASK)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-MASK-PANEL | Masks panel | P0 | ✅ | `cmd:panel.masking`, `cmd:mask.add`, `cmd:mask.select`, `cmd:mask.rename`, `cmd:mask.duplicate` (`invert`), `cmd:mask.move`, `cmd:mask.visible`, `cmd:mask.delete`, `crates/ui-egui/src/panels/masking.rs` | list with per-mask show/hide eye, double-click rename, right-click menu (duplicate, duplicate and invert, invert, hide, move up/down, rename, delete); fits any panel width (create tiles go to three columns, mask actions wrap, long component names are cut short); no drag-to-reorder |
| LR-MASK-SUBJECT | Select subject | P2 | 🟡 | `cmd:mask.add` (`subject`), `crates/pipeline/src/masks.rs` | saliency heuristic, no segmentation model |
| LR-MASK-SKY | Select sky | P2 | 🟡 | `cmd:mask.add` (`sky`) | heuristic |
| LR-MASK-BACKGROUND | Select background | P2 | 🟡 | `cmd:mask.add` (`background`) | inverse of the subject heuristic |
| LR-MASK-OBJECTS | Object selection | P2 | ⬜ | | shape exists (falls back to the subject heuristic); no UI |
| LR-MASK-PEOPLE | People parts | P2 | ⬜ | | |
| LR-MASK-LANDSCAPE | Landscape classes | P2 | ⬜ | | shape exists, evaluates empty |
| LR-MASK-BRUSH | Brush mask | P0 | ✅ | `cmd:tool.brush`, `cmd:mask.brushStroke` (`autoMask`), `crates/pipeline/src/masks.rs` | size/feather/flow/density/erase; Auto Mask: dabs weighted by similarity to the colour under the dab centre, refined by a guided filter on luminance (CPU + GPU); no A/B brushes, no pressure |
| LR-MASK-LINEAR | Linear gradient | P0 | ✅ | `cmd:tool.linear`, `cmd:mask.update` | |
| LR-MASK-RADIAL | Radial gradient | P0 | ✅ | `cmd:tool.radial`, `cmd:mask.update` | |
| LR-MASK-COLORRANGE | Colour range | P1 | ✅ | `cmd:mask.add` (`colorRange`), `cmd:mask.sampleColor`, `cmd:mask.update`, `crates/pipeline/src/lib.rs` (`color_range_sample`) | Color tile → click the photo to sample (⇧-click adds, up to 5; samples taken in the space the mask compares in), Pick button, Refine slider |
| LR-MASK-LUMRANGE | Luminance range | P1 | ✅ | `cmd:mask.add` (`luminanceRange`), `cmd:mask.update`, `crates/ui-egui/src/panels/masking.rs` (`range_controls`) | range bar with two handles (one undo step per drag), Smoothness, Show Luminance Map (B&W photo with the range tinted) |
| LR-MASK-DEPTHRANGE | Depth range | P2 | ⬜ | | shape exists, needs depth data |
| LR-MASK-COMBINE | Add / subtract / intersect | P0 | ✅ | `cmd:mask.addComponent` | |
| LR-MASK-INVERT | Invert | P0 | ✅ | `cmd:mask.invert` | |
| LR-MASK-AMOUNT | Mask amount | P1 | ✅ | `cmd:mask.adjust` (`amount`), `crates/ui-egui/src/panels/masking.rs` | |
| LR-MASK-FEATHER-EDGE | Refine mask edges | P2 | ✅ | `cmd:mask.refine`, `crates/pipeline/src/masks.rs` (`evaluate_one`) | Refine Edges slider per mask: guided filter on the photo's luminance snaps soft mask edges to the photo's edges (rendered on the CPU) |
| LR-MASK-SLIDERS | Local adjustment sliders | P0 | ✅ | `cmd:mask.adjust`, `crates/pipeline/src/finish.rs` | every slider renders (CPU + GPU), incl. Noise, Moiré, Defringe (negative Defringe has no effect); no local curve or effect presets |
| LR-MASK-OVERLAY | Mask overlay | P0 | ✅ | `cmd:view.maskOverlay`, `cmd:view.maskOverlayMode`, `cmd:view.maskOverlayColor`, `crates/pipeline/src/visualize.rs` | rendered alpha of the selected mask (CPU + GPU): colour, colour on B&W, image on black/white, white on black; ⇧O cycles while masking; no auto-show on hover |
| LR-MASK-PINS | Pins | P1 | ✅ | `cmd:view.maskPins`, `crates/ui-egui/src/panels/detail.rs` | a pin per radial/linear/brush component: click selects its mask, drag moves it; no pins for range/AI components, no Auto mode |
| LR-MASK-UPDATE | Recompute AI masks | P2 | ⬜ | | |
| LR-MASK-SYNC | Copy masks to other photos | P1 | ✅ | `cmd:develop.copy` (`groups`) | |
| LR-MASK-ADAPTIVE | Adaptive presets | P2 | ⬜ | | |

## L. Presets (PRE)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-PRE-PANEL | Presets panel | P0 | ✅ | `cmd:panel.presets`, `cmd:preset.apply`, `crates/ui-egui/src/panels/presets.rs` | grouped list with amount; resting on a preset previews it in the loupe (no history entry); optional live thumbnails (⋯ → Show Thumbnails) |
| LR-PRE-CREATE | Create preset | P0 | ✅ | `cmd:dialog.createPreset`, `cmd:preset.create` (`groups`) | name, group and a checklist of settings groups (crop, masks, remove, red eye off by default; All / None) |
| LR-PRE-MANAGE | Manage presets | P1 | ✅ | `cmd:preset.delete`, `cmd:preset.favorite`, `cmd:preset.import`, `cmd:preset.export`, `cmd:preset.rename`, `cmd:preset.update`, `cmd:preset.move` | rename, update with current settings, move to a group (existing or new); no hiding of groups |
| LR-PRE-AMOUNT | Preset amount | P1 | ✅ | `cmd:preset.apply` (`amount` 0–200) | |
| LR-PRE-ADAPTIVE | Adaptive presets | P2 | ⬜ | | |
| LR-PRE-PREMIUM | Built-in presets (own) | P2 | ✅ | `crates/engine/src/presets.rs` | 41 own-authored presets in 10 groups (Color, Film, B&W incl. toners, Portrait, Landscape, Urban, Food, Seasons, Vintage, Style) |
| LR-PRE-RECOMMENDED | Community recommendations | OOS | 🚫 | | |
| LR-PRE-ONIMPORT | Apply during import | P2 | ✅ | `cmd:library.import` (`preset`) | chosen in the import review; raw / per-camera defaults in Settings |

## M. Versions & history (VER)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-VER-CREATE | Create version | P1 | ✅ | `cmd:version.create` | |
| LR-VER-PANEL | Versions panel | P1 | ✅ | `cmd:panel.versions`, `cmd:version.create`, `cmd:version.restore`, `cmd:version.delete`, `cmd:version.rename`, `cmd:version.update`, `crates/ui-egui/src/panels/right.rs` (`versions`) | Named / Auto tabs, a thumbnail of each version's look, name and date, current marker; resting on a version previews it in the loupe; click restores, double-click renames; row menu (update, rename, set as before, delete). No automatic versions are created yet (LR-VER-AUTO) |
| LR-VER-AUTO | Automatic versions | P2 | ✅ | `crates/engine/src/lib.rs` (`auto_version`) | leaving an edited photo keeps its settings as an Auto version (when they differ from the latest version; 20 kept; not an undo step); Versions panel ▸ Auto |
| LR-VER-HISTORY | Edit history | P1 | ✅ | `cmd:panel.activity`, `cmd:history.list`, `cmd:history.restore` | |
| LR-VER-UNDO | Undo / redo | P0 | ✅ | `cmd:edit.undo`, `cmd:edit.redo` | |

## N. Copy / paste / sync (SYNC)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-SYNC-COPY | Copy edit settings | P0 | ✅ | `cmd:develop.copy` | |
| LR-SYNC-CHOOSE | Choose settings to copy | P0 | ✅ | `cmd:dialog.copySettings` | groups are coarser than per-slider |
| LR-SYNC-PASTE | Paste to selection | P0 | ✅ | `cmd:develop.paste` | no separate "paste selected" (choose at copy time instead) |
| LR-SYNC-SYNCBTN | Sync active → selected | P1 | ✅ | `cmd:develop.sync` | |
| LR-SYNC-PREVIOUS | Paste from previous / auto sync [Classic] | P2 | ✅ | `cmd:develop.pastePrevious`, `cmd:develop.autoSync` | ⌥⌘V pastes the previously active photo's settings (copy groups); Auto Sync (⌥⇧⌘A) |

## O. Merge (MERGE)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-MERGE-HDR | HDR merge | P2 | ✅ | `cmd:merge.hdr`, `cmd:dialog.mergeHdr` | auto align, deghost None–High + overlay, auto settings, float DNG, Create Stack; JPEG brackets treated as linear |
| LR-MERGE-PANO | Panorama | P2 | ✅ | `cmd:merge.panorama`, `cmd:dialog.mergePanorama` | spherical/cylindrical/perspective + auto, boundary warp, auto crop, fill edges (diffusion), DNG; no lens model / 360° wrap |
| LR-MERGE-HDRPANO | HDR panorama | P2 | ✅ | `cmd:merge.hdrPanorama`, `cmd:dialog.mergeHdrPanorama` | brackets grouped by EXIF |
| LR-MERGE-HEADLESS | Merge with last settings | P2 | ✅ | `cmd:merge.hdrLast`, `cmd:merge.panoramaLast`, `cmd:merge.hdrPanoramaLast` | no dialog; the options of the last merge of that kind (defaults the first time) |

## P. Enhance (ENH)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-ENH-DIALOG | Enhance dialog | P2 | ⬜ | | |
| LR-ENH-INPLACE | In-place enhance | P2 | ⬜ | | |

## Q. HDR (HDR)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-HDR-EDIT | HDR editing | P2 | ⬜ | | |
| LR-HDR-SDRPREVIEW | SDR preview of HDR | P2 | ⬜ | | |
| LR-HDR-VISUALIZE | Visualize HDR range | P2 | ⬜ | | |
| LR-HDR-LIMIT | HDR headroom limit | P2 | ⬜ | | |
| LR-HDR-EXPORT | HDR export | P2 | ⬜ | | |

## R. Video (VID)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-VID-PLAY | Video playback | P1 | ⬜ | `crates/catalog/src/model.rs` | videos can be catalogued (kind, duration); no player |
| LR-VID-TRIM | Trim | P1 | ⬜ | | |
| LR-VID-EDIT | Edits on video | P2 | ⬜ | | |
| LR-VID-COVER | Cover frame | P2 | ⬜ | | |
| LR-VID-EXPORT | Video export | P2 | ⬜ | | |
| LR-VID-PHOTO2VIDEO | Photo to video (generative) | OOS | 🚫 | | |

## S. Export (EXP)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-EXP-DIALOG | Export dialog | P0 | ✅ | `cmd:dialog.export`, `cmd:app.export` (`preset`), `cmd:export.presets`, `cmd:export.savePreset`, `cmd:export.deletePreset`, `crates/ui-egui/src/panels/dialogs.rs`, `crates/engine/src/export.rs` | batch export; built-in presets (JPEG Small 2048 px / Large full size, Original + Settings, DNG) and saved user presets (library prefs) load into the dialog; deleting a user preset is command-only |
| LR-EXP-TYPE | File types | P0 | ✅ | `cmd:app.export` (`format`), `crates/engine/src/export.rs` (`ExportFormat`), `crates/engine/src/tests_export.rs` | JPEG, PNG, TIFF, WebP, AVIF, DNG (raw photos: lossless re-encode with the edits in the embedded XMP), Original (+ XMP sidecar). No JXL encoder; non-raw → DNG not supported |
| LR-EXP-DIM | Output size | P0 | ✅ | `cmd:app.export` (`longEdge`, `shortEdge`, `width`, `height`, `megapixels`, `percent`, `dontEnlarge`, `ppi`), `crates/engine/src/export.rs` (`Resize`), `crates/ui-egui/src/panels/dialogs.rs` | full size = the cropped native size (no longer upscaled); W × H fits either orientation; ppi written to JFIF / pHYs / TIFF tags |
| LR-EXP-QUALITY | JPEG quality | P0 | ✅ | `cmd:app.export` (`quality`, `limitKb`) | |
| LR-EXP-BITDEPTH | Bit depth | P1 | ✅ | `cmd:app.export` (`bitDepth`), `crates/pipeline/src/output.rs` (`OutputDepth`), `crates/engine/src/export.rs` | 16-bit PNG/TIFF rendered at 16 bits (TIFF defaults to 16), 32-bit float linear TIFF with a linear profile, 10-bit AVIF; high-bit-depth renders run on the CPU |
| LR-EXP-COMPRESSION | TIFF compression | P1 | ✅ | `cmd:app.export` (`tiffCompression`: none / lzw / zip) | |
| LR-EXP-COLORSPACE | Output colour space | P0 | ✅ | `cmd:app.export` (`colorSpace`), `crates/pipeline/src/output.rs`, `crates/engine/src/export.rs` | sRGB, Display P3, Adobe RGB (1998) compatible, ProPhoto RGB, Rec. 2020: rendered from the working space with gamut mapping into the target gamut (CPU + GPU), own ICC profile embedded; AVIF stays sRGB (muxer has no ICC) |
| LR-EXP-HDR | HDR output | P2 | ⬜ | | |
| LR-EXP-SHARPEN | Output sharpening | P1 | ✅ | `cmd:app.export` (`sharpen`, `sharpenAmount`) | |
| LR-EXP-METADATA | Metadata policy | P1 | ✅ | `cmd:app.export` (`metadata`, `removeLocation`) | |
| LR-EXP-WATERMARK | Watermark | P1 | ✅ | `cmd:app.export` (`watermark`: text or `image` + `imageWidth`), `crates/engine/src/export.rs` (`Watermark`) | text (size, colour, shadow) or a graphic with transparency (width as % of the photo, converted to the output colour space); position, inset, opacity |
| LR-EXP-NAMING | File naming | P1 | ✅ | `cmd:app.export` (`naming`, `startNumber`), `crates/engine/src/rename.rs` (`expand_tokens`) | one template language for export, Rename Photos and import renaming: `{name}`, `{num}` (number at the end of the name), `{seq}` / `{seq:N}`, `{date}` / `{date:%Y-%m-%d}`, `{folder}`, `{camera}`, `{lens}`, `{iso}`, `{rating}`, `{title}`, `{creator}`, `{ext}`; free-form template rather than a list of named schemes |
| LR-EXP-LOCATION | Destination folder | P0 | ✅ | `cmd:app.export` (`dir`, `subfolder`, `conflict`: unique / overwrite / skip), `cmd:export.checkTarget`, `crates/ui-egui/src/panels/dialogs.rs` (Choose… folder picker), `crates/engine/src/originals.rs`, `crates/catalog/src/safe_file.rs` | folder picker on the desktop; existing files get `-2`, `-3`… by default; the policy covers a file and its sidecars together; a catalogued original (or its sidecar) is never written over, whatever the policy or an exact `path` (export, `ui.render`, MCP `render_photo`, merge `previewPath`, `lightcraft-cli render`); files are written to a temp file and renamed into place (issue #93; not synced file by file — exports can be made again, issue #134) |
| LR-EXP-PREVIOUS | Export with previous settings | P0 | ✅ | `cmd:app.exportPrevious`, `cmd:dialog.export` | last options persist in prefs.json; dialog prefilled; no named export presets yet |
| LR-EXP-DNGOPT | DNG options | P2 | 🟡 | `cmd:app.export` (`dngCompression`), `crates/engine/src/export.rs` | compression: lossless JPEG (default), ZIP or none; no embedded JPEG preview size, no lossy DNG output |
| LR-EXP-ORIGINAL | Original + XMP | P1 | ✅ | `cmd:app.export` (`format: original`), `crates/engine/src/tests_export.rs` | file copied byte for byte, sidecar named after the output and subject to the conflict policy |
| LR-EXP-PHOTOS | Export to the system photo library | P2 | ⬜ | | |
| LR-EXP-PSD | Round trip to an external editor | P2 | ✅ | `cmd:photo.editExternal`, `cmd:photo.editInExternal`, `crates/engine/src/cmd/convert.rs` | a 16-bit TIFF `-Edit` copy with the edits (Adobe RGB / ProPhoto / P3 / sRGB) next to the original, added stacked on top of it and opened in the editor set in Settings ▸ General (or the system default) |

## T. Share (SHARE)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-SHARE-LINK | Web share link | OOS | 🚫 | | |
| LR-SHARE-INVITE | Invite collaborators | OOS | 🚫 | | |
| LR-SHARE-WEBGALLERY | Web galleries | OOS | 🚫 | | |
| LR-SHARE-COMMUNITY | Community edits | OOS | 🚫 | | |

## U. Map & location (MAP)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-MAP-INFO | Location in the info panel | P1 | 🟡 | `cmd:photo.setMeta` (`location`, `city`, `state`, `country`, `gps`), `crates/ui-egui/src/panels/right.rs` | location, city, state/province, country and GPS editable; Show on Map (OpenStreetMap in the browser); no map in the panel |
| LR-MAP-MODULE | Map module [Classic] | P2 | ⬜ | | |

## V. Preferences (PREF)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-PREF-GENERAL | General settings | P0 | ✅ | `cmd:app.settings`, `cmd:app.openLibrary`, `crates/ui-egui/src/panels/settings.rs`, `crates/ui-egui/src/panels/notices.rs` | General / Import / Performance / Interface tabs; app settings in ui.json (written atomically, saved when the library changes and every few seconds), library settings in prefs.json; a damaged settings file is kept as `.corrupt-<time>` and reported, an unreadable one is not overwritten that session (`library.info` → `settingsWarnings`); quitting with unsaved changes retries, then asks |
| LR-PREF-LOCALSTORAGE | Storage & cache | P1 | ✅ | `cmd:library.preferences`, `cmd:library.clearPreviews`, `cmd:library.compact` | thumbnail cache size + clear in Settings → Performance; library location + Open Library… in General |
| LR-PREF-ACCOUNT | Account | OOS | 🚫 | | |
| LR-PREF-INTERFACE | Interface options | P1 | ✅ | `cmd:app.settings` | filmstrip names/badges, grid badges (auto/always/never), square-grid names, navigator, info overlay |
| LR-PREF-PERFORMANCE | GPU / performance | P1 | ✅ | `cmd:app.gpu`, `cmd:app.memoryBudget`, `cmd:app.settings` | GPU on/off, preview size (1600–5120 px), memory budget, thumbnail cache size in Settings |
| LR-PREF-PEOPLE | Face recognition | P2 | ⬜ | | |
| LR-PREF-WATERMARK | Watermark settings | P1 | ✅ | `cmd:export.savePreset`, `crates/ui-egui/src/panels/dialogs.rs` | set in the Export dialog; kept with Export with Previous and in saved export presets |
| LR-PREF-SHORTCUTS | Shortcut customisation | — | 🚫 | | not customisable in the reference app either; a keymap editor would be an extra |
| LR-PREF-TECHPREVIEW | Early-access toggles | P2 | ⬜ | | |
| LR-PREF-NOTIFICATIONS | Notifications | OOS | 🚫 | | |
| LR-PREF-DEVICE | Device settings | P2 | ⬜ | | |

## W. Cloud & AI infrastructure (CLOUD / AI)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-CLOUD-SYNC | Cloud sync | OOS | 🚫 | | |
| LR-CLOUD-SMARTPREVIEW | Editable proxies | P2 | ✅ | `cmd:library.smartPreviews`, `cmd:library.smartPreviewsLocation`, `cmd:photo.smartPreview`, `crates/engine/src/smart.rs`, `crates/ui-egui/src/panels/settings.rs` | File ▸ Previews ▸ Build / Discard Smart Previews (on a worker thread, progress and Stop like Build Previews): ~1 MB proxies in the library; with the original offline the photo renders, edits and exports (at proxy size) from its proxy; Info shows the status; the proxy folder is chosen per library (Settings → Performance → Smart previews, saved in the library): the effective path, count and size are shown, changing it needs a choice for the proxies already built (move / leave / delete), and an unavailable or unwritable folder is an error, never a fallback to the library drive; proxies are written atomically and a damaged one (cut short by a crash or a full drive) doesn't count as built, is rebuilt (`repaired`) and is never moved over a good one (issue #106). The thumbnail cache stays in the library |
| LR-AI-UPDATE-INDICATOR | AI-settings update indicator | P2 | ⬜ | | |
| LR-AI-CREDITS | Generative credits | OOS | 🚫 | | |

## X. Cross-cutting behaviours (BEHAV)

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LR-BEHAV-AUTOSAVE | Instant autosave | P0 | ✅ | `crates/catalog/src/journal.rs`, `crates/engine/src/library.rs` | every change fsynced before the command returns; a failed write fails the command ("saved in memory but not written to disk"), keeps the change queued and retries; top-bar warning until saved; a write that fails part-way is cut back to the last whole record so the retry never reads back as damage (logs from older builds with such a fragment load in full); quitting saves queued changes in the closing snapshot even when the log can't be appended to; versioned catalog format (`journal::VERSION`): older libraries are upgraded on open, a library from a newer LightCraft is refused untouched ("written by a newer version") |
| LR-BEHAV-UNDO | Global undo | P0 | ✅ | `cmd:edit.undo`, `crates/engine/src/tests.rs` (`rating_flag_undo_redo`) | covers ratings, albums, deletes, edits |
| LR-BEHAV-MULTISELECT | Multi-selection | P0 | ✅ | `cmd:library.select` (`replace`/`add`/`toggle`/`range`), `cmd:library.selectAll` | |
| LR-BEHAV-BATCH | Batch apply to selection | P0 | ✅ | `cmd:photo.rate`, `cmd:develop.paste`, `cmd:preset.apply`, `cmd:app.export` | |
| LR-BEHAV-PREVIEW-HOVER | Hover previews | P1 | ✅ | `crates/ui-egui/src/panels/presets.rs`, `crates/ui-egui/src/panels/profiles.rs`, `crates/ui-egui/src/panels/right.rs` (`versions`), `crates/ui-egui/src/panels/detail.rs` | presets, profile menu, profile browser and versions preview in the loupe |
| LR-BEHAV-PROGRESSIVE | Progressive rendering | P0 | ✅ | `crates/engine/src/media.rs`, `crates/preview/src/lib.rs` | |
| LR-BEHAV-BG-TASKS | Background tasks | P0 | ✅ | `crates/preview/src/lib.rs` (`JobPool`), `crates/ui-egui/src/export_task.rs`, `crates/ui-egui/src/import.rs` | renders off the UI thread; exports started from the UI run on a worker thread with a progress panel and Cancel (`ui.inspect` → `export`); imports show a progress window. No combined activity centre |
| LR-BEHAV-OFFLINE | Offline editing | P1 | ✅ | | local-first: everything works offline |
| LR-BEHAV-WEB-SAFETY | Library safety in the browser build | P1 | 🟡 | `cmd:file.backupLibrary`, `cmd:file.restoreLibrary`, `apps/lightcraft-web/src/safety.rs`, `apps/lightcraft-web/src/backup.rs`, `apps/lightcraft-web/src/files.rs` | web only (experimental): Back Up Library (zip of the catalog and every stored original) and a non-destructive Restore; failed saves show as unsaved and are retried; a photo that can't be stored isn't added; one tab per library (Web Locks); notices for storage that isn't persistent and for a library that can't open; `?reset` asks first; a panic shows a message. Missing: `?safe` start, deleting originals of removed photos, zip64 (backups > 4 GB) |
| LR-BEHAV-GPU | GPU acceleration | P0 | ✅ | `crates/gpu/src/render.rs`, `cmd:app.gpu`, `docs/gpu-pipeline.md` | CPU fallback per render on device limits, out of memory, device errors / loss, incomplete or black results (`perf.gpuReason`, `perf.gpuFallback`); short submissions for slow iGPUs; backend choice (DX12 only on Windows, `LIGHTCRAFT_GPU_BACKEND` / `WGPU_BACKEND`), device created after the window opens, crash sentinel (issue #136); no tiling beyond the buffer limit (CPU) |
| LR-BEHAV-RENDER-FIDELITY | Rendering matches Lightroom | P1 | 🟡 | `crates/pipeline/src/lib.rs`, `crates/pipeline/src/tone.rs` | every slider exists and works, but the character of the result (tone curve shape, highlight recovery, texture / clarity / dehaze, noise reduction, sharpening, default look) is tuned by eye; there is no systematic side-by-side comparison against Lightroom on the same CC0 raws. Needs a fidelity suite: Lightroom reference renders kept only in the local `plan/` (never committed), compared per slider and preset with a perceptual metric |
| LR-BEHAV-DRAGDROP | Drag and drop | P1 | ✅ | `crates/ui-egui/src/lib.rs`, `crates/ui-egui/src/panels/grid.rs` (`drag_feedback`), `crates/ui-egui/src/panels/left.rs` (`drop_target`) | files → app (import); grid photos → an album row (adds the selection, with a count badge while dragging) |
| LR-BEHAV-TOAST | Toast notifications | P1 | ✅ | `crates/ui-egui/src/panels/mod.rs` | |
| LR-BEHAV-PANEL-RESIZE | Resizable side panels | P1 | ✅ | `crates/ui-egui/src/panels/mod.rs` (`resizable_side`), `crates/ui-egui/src/state.rs` (`LEFT_WIDTH`, `RIGHT_WIDTH`), `crates/ui-egui/src/tests_panels.rs` | drag the left sidebar's right edge (200–480 pt) or the right panel's left edge (250–520 pt); the photo area keeps ≥ 360 pt; widths are kept across panels, views and restarts (`leftWidth` / `rightWidth` in the UI state); the Presets column stays fixed |
| LR-BEHAV-EMPTY-STATES | Empty states | P1 | ✅ | `crates/ui-egui/src/panels/mod.rs` (`empty_message`) | |
| LR-BEHAV-TOOLTIPS | Tooltips with shortcuts | P0 | ✅ | `crates/ui-egui/src/panels/bottombar.rs` | |
| LR-BEHAV-ACCESS | Accessibility | P2 | 🟡 | `crates/ui-egui/src/widgets.rs`, `crates/ui-egui/src/panels/grid.rs` | AccessKit (VoiceOver / Narrator / AT-SPI): sliders announce control and value, buttons / icon buttons / dropdowns / section headers / sources their labels and state, grid thumbnails file name, rating, flag and label; the canvas tools (crop, masks) are pointer-only; not audited with a screen reader |
| LR-BEHAV-LOCALIZE | Localisation | P2 | 🟡 | `crates/ui-egui/src/i18n.rs`, `crates/ui-egui/locales/`, `docs/localization-ja.md` | Japanese/English UI with persisted language preference and Japanese regular/bold fonts; menus, edit, crop/masks, settings, import/export and primary progress messages. Technical errors/release notes and other locales remain untranslated. |
| LR-BEHAV-LEARN | Tutorials | OOS | 🚫 | | |
| LR-BEHAV-WHATSNEW | What's new | P2 | ✅ | `cmd:app.whatsNew`, `docs/whats-new.md` | Help ▸ What's New: release highlights |
| LR-BEHAV-AI-EA | Early-access badges | P2 | ⬜ | | |

## Y. Menus

From `04-menu-tree.md`. ✅ = a command exists and is reachable from the UI (button, panel or shortcut). There is no
visible menu bar yet: the menu model is only exposed through the control channel (`ui.menu.list`).

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| MENU-BAR | Menu bar rendering | P1 | ✅ | `crates/ui-egui/src/menubar.rs`, `apps/lightcraft/src/native_menu.rs` | native macOS menu bar (muda) with live labels/enabled/checked; in-window menus on web/Windows/Linux; ⌫ and X stay egui-handled (contextual), so they show no key in the native menu |
| MENU-APP-ABOUT | About | P2 | ✅ | `cmd:app.about` | |
| MENU-APP-SETTINGS | Settings… | P0 | ✅ | `cmd:app.settings` | app menu on macOS, Edit menu elsewhere |
| MENU-APP-UPDATES | Check for updates | P2 | ⬜ | | |
| MENU-APP-SYNC | Sync status / pause | OOS | 🚫 | | |
| MENU-APP-SIGNOUT | Sign out | OOS | 🚫 | | |
| MENU-APP-HIDE | Hide / hide others / show all | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | the system's own items in the app menu (⌘H, ⌥⌘H) |
| MENU-APP-QUIT | Quit | P0 | ✅ | `cmd:app.quit`, `apps/lightcraft/src/native_menu.rs` | macOS: app menu (native); elsewhere: File → Quit LightCraft, always the last item in its own group (after commands the File layout does not list) |
| MENU-FILE-ADDPHOTOS | Import Photos… (was Add Photos…) | P0 | ✅ | `cmd:file.addPhotos`, `crates/ui-egui/src/menus.rs` | first item of File, ⇧⌘I; file picker → the import review |
| MENU-FILE-ADDFOLDER | Import from Folder… (was Add Folder…) | P0 | ✅ | `cmd:file.addFolder`, `cmd:library.importPreview` | folder picker (desktop) → the import review, subfolders included; `path` param for agents. Worded as importing: it doesn't save a Local location (that is Local → Browse Folder…) |
| MENU-FILE-MIGRATE | Migrate photos | OOS | 🚫 | | |
| MENU-FILE-NEWALBUM | New Album… | P0 | ✅ | `cmd:dialog.newAlbum` | |
| MENU-FILE-NEWFOLDER | New Folder… | P0 | ✅ | `cmd:dialog.newFolder` | ⇧⌘N |
| MENU-FILE-NEWSMART | New Smart Album… | P1 | ✅ | `cmd:dialog.newSmartAlbum` | saves the current view (source + filter) |
| MENU-FILE-IMPORTPROFILES | Import Profiles & Presets… | P1 | ✅ | `cmd:file.importPresets` | presets in every common format and .cube LUT profiles (see LR-IMP-PROFILES) |
| MENU-FILE-EXPORT | Export… | P0 | ✅ | `cmd:dialog.export` | |
| MENU-FILE-EXPORTPREV | Export with Previous | P0 | ✅ | `cmd:app.exportPrevious` | ⌥⇧⌘E |
| MENU-FILE-EXPORTPRESETS | Export preset submenu | P0 | ✅ | `cmd:app.export` (`preset`), `crates/ui-egui/src/menubar.rs` (Export with Preset) | built-ins, then user presets, then Custom… (the dialog); exports to the last folder |
| MENU-FILE-SHARE | Share / get link / invite | OOS | 🚫 | | |
| MENU-FILE-PHOTOSHOP | Edit in external editor | P2 | ✅ | `cmd:photo.editInExternal` | Photo ▸ Edit in External Editor |
| MENU-FILE-SHOWFINDER | Show in Finder | P0 | ✅ | `cmd:app.showInFinder` | ⌘R; Explorer on Windows, the folder on Linux; disabled for demo scenes and on the web |
| MENU-FILE-OFFLINE | Store album locally | P2 | 🚫 | | not applicable: local-first |
| MENU-FILE-CLOSE | Close Window | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | the system's Close Window item at the end of File (⌘W) |
| MENU-EDIT-UNDO | Undo | P0 | ✅ | `cmd:edit.undo` | label does not name the step |
| MENU-EDIT-REDO | Redo | P0 | ✅ | `cmd:edit.redo` | |
| MENU-EDIT-COPYPASTE | Copy / paste (edit settings) | P0 | ✅ | `cmd:develop.copy`, `cmd:develop.paste` | |
| MENU-EDIT-CHOOSECOPY | Choose Edit Settings to Copy… | P0 | ✅ | `cmd:dialog.copySettings` | |
| MENU-EDIT-PASTESELECTED | Paste Selected Settings | P0 | ✅ | `cmd:dialog.pasteSettings`, `cmd:develop.paste` (`groups`) | checklist prefilled with the copied groups; only copied groups can be pasted |
| MENU-EDIT-SELECTALL | Select All | P0 | ✅ | `cmd:library.selectAll` | |
| MENU-EDIT-SELECTNONE | Select None | P0 | ✅ | `cmd:library.selectNone` | |
| MENU-EDIT-SELECTBY | Select by flag / rating | P1 | ✅ | `cmd:library.selectBy` | Edit → Select by: picks, rejects, unflagged, ★…★★★★★ and higher, unrated, colour labels; `add` extends the selection |
| MENU-EDIT-FIND | Find… | P0 | ✅ | `cmd:view.focusSearch`, `crates/ui-egui/src/panels/topbar.rs` | ⌘F focuses the search field |
| MENU-VIEW-PHOTOGRID | Photo Grid | P0 | ✅ | `cmd:view.photoGrid` | |
| MENU-VIEW-SQUAREGRID | Square Grid | P0 | ✅ | `cmd:view.squareGrid` | |
| MENU-VIEW-DETAIL | Detail | P0 | ✅ | `cmd:view.detail` | |
| MENU-VIEW-EDIT | Edit | P0 | ✅ | `cmd:panel.edit` | |
| MENU-VIEW-FULLSCREENPREVIEW | Full Screen Preview | P1 | ✅ | `cmd:view.fullScreenPreview` | |
| MENU-VIEW-ENTERFULLSCREEN | Enter Full Screen | P1 | ✅ | `cmd:view.enterFullScreen` | |
| MENU-VIEW-PHOTOSPANEL | Show/Hide photos panel | P0 | ✅ | `cmd:view.leftPanel` | |
| MENU-VIEW-FILMSTRIP | Show/Hide filmstrip | P0 | ✅ | `cmd:view.filmstrip` | |
| MENU-VIEW-INFO | Show/Hide info | P0 | ✅ | `cmd:panel.info` | |
| MENU-VIEW-KEYWORDS | Show/Hide keywords | P0 | ✅ | `cmd:panel.keywords` | |
| MENU-VIEW-ACTIVITY | Show/Hide activity (comments) | OOS | 🚫 | | our History panel is `panel.activity` |
| MENU-VIEW-VERSIONS | Show/Hide versions | P1 | ✅ | `cmd:panel.versions` | |
| MENU-VIEW-HISTOGRAM | Show/Hide histogram | P0 | ✅ | `cmd:view.histogram` | |
| MENU-VIEW-INFOOVERLAY | Show info overlay | P1 | ✅ | `cmd:view.infoOverlay` | cycles |
| MENU-VIEW-SHOWORIGINAL | Show Original | P0 | ✅ | `cmd:view.showOriginal` | |
| MENU-VIEW-BEFOREAFTER | Before/After submenu | P0 | ✅ | `cmd:view.beforeAfter`, `cmd:view.beforeAfterSplit`, `cmd:view.beforeAfterTopBottom`, `cmd:view.beforeAfterSplitTopBottom` | |
| MENU-VIEW-ZOOM | Zoom in / out / toggle / fit / 1:1 | P0 | ✅ | `cmd:view.zoomIn`, `cmd:view.zoomOut`, `cmd:view.zoomToggle`, `cmd:view.zoomFit`, `cmd:view.zoom100` | |
| MENU-VIEW-CLIPPING | Show Clipping | P0 | ✅ | `cmd:view.clipping` | |
| MENU-VIEW-MASKOVERLAY | Mask overlay / cycle colour | P0 | ✅ | `cmd:view.maskOverlay`, `cmd:view.maskOverlayMode`, `cmd:view.maskOverlayColor` | colour cycles through the panel's swatches (no params) or takes `color` / `opacity` |
| MENU-VIEW-INCLUDESUBFOLDERS | Include subfolders | P1 | ✅ | `cmd:library.browse` (`subfolders`) | toggle in the folder header; imports of folders are recursive |
| MENU-VIEW-SORT | Sort submenu | P0 | ✅ | `cmd:library.sort` | no colour-label key |
| MENU-VIEW-STACKS | Expand/collapse stacks | P1 | ✅ | `cmd:stack.expandAll`, `cmd:stack.collapseAll` | |
| MENU-VIEW-PHOTOCOUNT | Show photo counts | P2 | ✅ | `cmd:view.photoCounts` | View ▸ Show Photo Counts toggles the left panel's counts |
| MENU-VIEW-HDR | HDR display options | P2 | ⬜ | | |
| MENU-PHOTO-ADDTOALBUM | Add to album | P0 | ✅ | `cmd:album.addPhotos` | |
| MENU-PHOTO-REMOVEFROMALBUM | Remove from album | P0 | ✅ | `cmd:album.removePhotos` | |
| MENU-PHOTO-RATE | Rate submenu | P0 | ✅ | `cmd:photo.rate` | |
| MENU-PHOTO-FLAG | Flag submenu | P0 | ✅ | `cmd:photo.flag` | |
| MENU-PHOTO-LABEL | Colour label submenu | P1 | ✅ | `cmd:photo.label`, `cmd:dialog.labelNames` | shows custom names; Edit Label Names… |
| MENU-PHOTO-ROTATE | Rotate left / right | P0 | ✅ | `cmd:photo.rotateLeft`, `cmd:photo.rotateRight` | |
| MENU-PHOTO-FLIP | Flip horizontal / vertical | P0 | ✅ | `cmd:photo.flipHorizontal`, `cmd:photo.flipVertical` | |
| MENU-PHOTO-CREATEVERSION | Create Version… | P1 | ✅ | `cmd:version.create` | no name prompt |
| MENU-PHOTO-STACK | Stack submenu | P1 | ✅ | `cmd:stack.group`, `cmd:stack.ungroup`, `cmd:dialog.autoStack` | |
| MENU-PHOTO-MERGE | Photo merge submenu | P2 | ✅ | `cmd:dialog.mergeHdr`, `cmd:dialog.mergePanorama`, `cmd:dialog.mergeHdrPanorama`, `cmd:merge.hdrLast` | |
| MENU-PHOTO-ENHANCE | Enhance… | P2 | ⬜ | | |
| MENU-PHOTO-AUTO | Auto settings | P0 | ✅ | `cmd:develop.auto` | |
| MENU-PHOTO-BW | Convert to B&W | P0 | ✅ | `cmd:develop.treatment` | |
| MENU-PHOTO-RESET | Reset edits / crop | P0 | ✅ | `cmd:develop.reset`, `cmd:crop.reset` | |
| MENU-PHOTO-UPDATEAI | Update AI settings | P2 | ⬜ | | |
| MENU-PHOTO-RENAME | Rename N photos… | P1 | ✅ | `cmd:dialog.rename` | live “Rename N Photos…” label |
| MENU-PHOTO-CAPTURETIME | Edit capture time… | P1 | ✅ | `cmd:dialog.captureTime` | |
| MENU-PHOTO-COVER | Set as album cover | P2 | ✅ | `cmd:album.setCover` | |
| MENU-PHOTO-DELETE | Delete N photos… | P0 | ✅ | `cmd:photo.delete` | no confirmation; static label |
| MENU-PHOTO-MOVETOCLOUD | Move/copy to cloud | OOS | 🚫 | | |
| MENU-WINDOW-MINIMIZE | Minimize / zoom | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | the system's Minimize (⌘M) and Zoom items at the top of Window |
| MENU-WINDOW-PANELS | Panel switches | P0 | ✅ | `cmd:panel.edit`, `cmd:panel.crop`, `cmd:panel.remove`, `cmd:panel.masking`, `cmd:panel.presets`, `cmd:panel.versions` | |
| MENU-WINDOW-BRINGFRONT | Bring all to front | P2 | ✅ | `apps/lightcraft/src/native_menu.rs` | the system's item at the end of Window |
| MENU-HELP-HELP | Help | P2 | ✅ | `cmd:app.help` | opens the documentation |
| MENU-HELP-TUTORIALS | Tutorials | OOS | 🚫 | | |
| MENU-HELP-WHATSNEW | What's new | P2 | ✅ | `cmd:app.whatsNew` | |
| MENU-HELP-SHORTCUTS | Keyboard shortcuts | P1 | ✅ | `cmd:app.shortcuts` | |
| MENU-HELP-FEEDBACK | Send feedback | P2 | ✅ | `cmd:app.feedback` | opens a new issue on the project's GitHub |
| MENU-HELP-SYSINFO | System info | P2 | ✅ | `cmd:app.systemInfo`, `cmd:library.info` | Help ▸ System Info…: version, OS, CPU threads, GPU, memory budget, preview size, library, timings; Copy to Clipboard; JSON for agents (`open: false`) |
| MENU-CTX-GRID | Photo context menu | P0 | ✅ | `crates/ui-egui/src/panels/grid.rs` (`context_menu`) | rate, flag, label, add to / remove from album, rename, virtual copy, version, stack, copy / paste / paste selected, reset, merge, rotate, show in Finder, export / export with preset, set as album cover (in an album), delete |
| MENU-CTX-DETAIL | Loupe context menu | P1 | ✅ | `crates/ui-egui/src/panels/detail.rs` | Zoom submenu (fit, 100%, in, out), then the photo menu |
| MENU-CTX-ALBUM | Album / folder row menu | P0 | ✅ | `crates/ui-egui/src/panels/left.rs` (`folder_menu`), `cmd:album.move`, `cmd:dialog.export` | add selected, export album (dialog / preset), move to a folder or the top level, rename, delete; smart albums: update rules |
| MENU-CTX-MASK | Mask / component menu | P0 | ✅ | `crates/ui-egui/src/panels/masking.rs` (`mask_menu`, `component_row_menu`), `cmd:mask.component` | mask rows: duplicate (and invert), invert, show/hide, move, rename, delete; component rows (right-click or "…"): invert, duplicate, mode (add / subtract / intersect), intersect with / subtract a new component, rename (also double-click), delete (the last one deletes the mask) |
| MENU-CTX-PRESET | Preset menu | P1 | ✅ | `crates/ui-egui/src/panels/presets.rs` | favourite, update with current settings, rename, move to group, delete; group: export |
| MENU-CTX-PROFILE | Profile favourites | P2 | ✅ | `cmd:profile.favorite`, `crates/ui-egui/src/panels/profiles.rs` | star in the profile browser; Favorites group first |
| MENU-CTX-VERSION | Version menu | P1 | ✅ | `crates/ui-egui/src/panels/right.rs` (`versions`) | restore, update with current settings, rename, set as before, delete |
| MENU-CTX-KEYWORD | Keyword chip menu | P1 | ✅ | `crates/ui-egui/src/panels/right.rs`, `cmd:keyword.delete` | remove from photo, show photos with keyword, rename keyword, delete keyword |

## Z. Keyboard shortcuts (desktop)

From `06-shortcuts.md` part 1. Evidence is our binding; conflicts are explained in
[Shortcuts: conflicts and missing bindings](#shortcuts-conflicts-and-missing-bindings).

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| KEY-CROP | Crop & rotate — C | P0 | ✅ | `cmd:panel.crop` | |
| KEY-DETAIL | Detail — D | P0 | ✅ | `cmd:view.detail` | |
| KEY-EDIT | Edit — E | P0 | ✅ | `cmd:panel.edit` | |
| KEY-FULLSCREEN | Full-screen preview — F | P1 | ✅ | `cmd:view.fullScreenPreview` | |
| KEY-GRID | Grid — G | P0 | ✅ | `cmd:view.gridToggle`, `cmd:view.photoGrid` | G toggles Photo Grid ↔ Square Grid |
| KEY-INFO | Info — I | P0 | ✅ | `cmd:panel.info` | |
| KEY-KEYWORDS | Keywords — K | P0 | ✅ | `cmd:panel.keywords` | |
| KEY-CLIPBOARD | Copy / paste edit settings — ⌘C / ⌘V | P0 | ✅ | `cmd:develop.copy`, `cmd:develop.paste` | ⌘X has nothing to cut outside text fields |
| KEY-UNDOREDO | Undo / redo — ⌘Z / ⇧⌘Z | P0 | ✅ | `cmd:edit.undo`, `cmd:edit.redo` | |
| KEY-MINIMIZE | Minimize — ⌘M | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | native Window ▸ Minimize |
| KEY-AUTO | Auto — ⇧A | P0 | ✅ | `cmd:develop.auto` | |
| KEY-PHOTOSHOP | External editor — ⇧⌘E | P2 | ✅ | `cmd:photo.editInExternal` | ⇧⌘E as observed; the export dialog is ⇧E |
| KEY-ROTATE | Rotate — ⌘[ / ⌘] | P0 | ✅ | `cmd:photo.rotateLeft`, `cmd:photo.rotateRight` | |
| KEY-ZOOM | Zoom in / out — ⌘= / ⌘− | P0 | ✅ | `cmd:view.zoomIn`, `cmd:view.zoomOut` | |
| KEY-SELECTALL | Select all — ⌘A | P0 | ✅ | `cmd:library.selectAll` | |
| KEY-SELECTNONE | Select none — ⌘D | P0 | ✅ | `cmd:library.selectNone` | secondary binding (primary ⌘⇧A) |
| KEY-PASTESELECTED | Paste selected — ⇧⌘V | P0 | ✅ | `cmd:dialog.pasteSettings` | |
| KEY-PREFS | Settings — ⌘, | P0 | ✅ | `cmd:app.settings` | |
| KEY-SEARCH | Search — ⌘F | P0 | ✅ | `cmd:view.focusSearch` | |
| KEY-VISUALIZESPOTS | Visualize spots — A | P1 | ✅ | `cmd:view.visualizeSpots` | |
| KEY-CYCLEOVERLAY | Cycle overlay — O | P0 | ✅ | `cmd:view.maskOverlay`, `cmd:view.maskOverlayMode`, `cmd:view.cropOverlay` | O toggles the mask overlay; ⇧O cycles mask overlay modes while masking, crop overlays elsewhere |
| KEY-PHOTOSPANEL | Photos panel — P | P0 | 🟡 | `cmd:view.leftPanel` | bound to ⌘⇧L; P = pick |
| KEY-LINEAR | Linear gradient — L | P0 | ✅ | `cmd:tool.linear` | |
| KEY-RADIAL | Radial gradient — R | P0 | ✅ | `cmd:tool.radial` | |
| KEY-CLIPPING | Clipping — J | P0 | ✅ | `cmd:view.clipping` | |
| KEY-WB | White-balance selector — W | P0 | ✅ | `cmd:tool.wbPicker` | |
| KEY-FILMSTRIP | Filmstrip — / | P0 | ✅ | `cmd:view.filmstrip` | |
| KEY-SHOWORIGINAL | Show original — \ | P0 | ✅ | `cmd:view.showOriginal` | |
| KEY-TOGGLEZOOM | Toggle zoom — Space | P0 | ✅ | `cmd:view.zoomToggle` | Space is a secondary binding (primary Z) |
| KEY-MASKCOLOR | Cycle mask colour — ⇧O | P1 | ✅ | `cmd:view.maskOverlayColor`, `crates/ui-egui/src/shortcuts.rs` | ⇧O while masking cycles the overlay colour (while cropping: the guides' orientation); the overlay mode is in View ▸ Cycle Mask Overlay Mode |
| KEY-EXPORTPREV | Export with previous — ⌘E | P0 | ✅ | `cmd:app.exportPrevious` | ⌘E (alias) and ⌥⇧⌘E (Classic) |
| KEY-EXPORTDIALOG | Export dialog — ⇧E | P0 | ✅ | `cmd:dialog.export` | secondary binding (primary ⌘⇧E) |
| KEY-ENTERFULLSCREEN | Window full screen — ⇧⌘F | P1 | ✅ | `cmd:view.enterFullScreen` | |
| KEY-STACK | Group / ungroup stack — ⌘G / ⇧⌘G | P1 | ✅ | `cmd:stack.group`, `cmd:stack.ungroup` | also S expand/collapse, ⇧S top of stack |
| KEY-GUIDEDUPRIGHT | Guided Upright — ⇧G | P1 | ✅ | `cmd:tool.guidedUpright`, `cmd:geometry.upright` | opens Crop & Geometry with Guided Upright on and the guide tool active; G toggles Photo Grid ↔ Square Grid (`cmd:view.gridToggle`) as observed |
| KEY-HIDE | Hide / hide others — ⌘H / ⌥⌘H | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | native app-menu items |
| KEY-QUIT | Quit — ⌘Q | P0 | ✅ | `cmd:app.quit` | ⌘Q (Ctrl+Q off macOS) |
| KEY-CREATEVERSION | Create version — ⇧M | P1 | ✅ | `cmd:version.create` | secondary binding (primary ⌘⇧S) |
| KEY-CLOSEWINDOW | Close window — ⌘W | P1 | ✅ | `apps/lightcraft/src/native_menu.rs` | native File ▸ Close Window |
| KEY-DELETE | Delete photo — ⌫ | P0 | ✅ | `cmd:photo.delete` | |
| KEY-ADDPHOTOS | Import photos — ⇧⌘I | P0 | ✅ | `cmd:file.addPhotos` | |
| KEY-VERSIONS | Versions panel — ⇧V | P1 | ✅ | `cmd:panel.versions` | |
| KEY-SECTIONS | Expand/collapse edit sections — ⌘1…⌘6 | P1 | ✅ | `cmd:section.light`, `cmd:section.color`, `cmd:section.effects`, `cmd:section.detail`, `cmd:section.optics` | ⌘1–⌘5 (Light, Color, Effects, Detail, Optics) as observed; our Edit panel has no Lens Blur / Geometry section for ⌘6 / ⌘7; Zoom 100% moved to ⌥⌘0 |
| KEY-PRESETS | Presets panel — ⇧P | P0 | ✅ | `cmd:panel.presets` | |
| KEY-HISTOGRAM | Histogram — ⌘0 | P0 | 🟡 | `cmd:view.histogram` | bound to ⌘⇧H; ⌘0 = zoom to fit |
| KEY-BRUSHSIZE | Brush size — `[` / `]` | P0 | ✅ | `cmd:brush.smaller`, `cmd:brush.larger` | Masking brush and Remove tool (and its selected spot) |
| KEY-BRUSHFEATHER | Brush feather — ⇧`[` / ⇧`]` | P0 | ✅ | `cmd:brush.featherLess`, `cmd:brush.featherMore` | |
| KEY-BRUSH | Brush — B | P0 | ✅ | `cmd:tool.brush` | |
| KEY-HEAL | Remove / heal — H | P0 | ✅ | `cmd:panel.remove` | |
| KEY-MERGE | HDR / panorama merges — ⌃H ⇧⌃H ⌃M ⇧⌃M | P2 | ✅ | `cmd:dialog.mergeHdr`, `cmd:dialog.mergePanorama`, `cmd:merge.hdrLast`, `cmd:merge.panoramaLast` | |
| KEY-PICK | Pick — Z | P0 | 🟡 | `cmd:photo.pick` | bound to P; Z toggles zoom |
| KEY-UNFLAG | Unflag — U | P0 | ✅ | `cmd:photo.unflag` | |
| KEY-REJECT | Reject — X | P0 | ✅ | `cmd:photo.reject` | swaps crop aspect while cropping |
| KEY-RATING | Ratings — 0…5 | P0 | ✅ | `crates/ui-egui/src/shortcuts.rs` → `cmd:photo.rate` | |
| KEY-LABELS | Labels — 6…9 | P1 | ✅ | `crates/ui-egui/src/shortcuts.rs` → `cmd:photo.label` | |
| KEY-MASKING | Masking — M | P0 | ✅ | `cmd:panel.masking` | |
| KEY-ERASE | Erase while held — ⌥ | P0 | ✅ | `crates/ui-egui/src/panels/detail.rs`, `crates/ui-egui/src/panels/masking.rs` | ⌥ while painting flips Add ↔ Erase for that stroke; Add/Erase buttons too |
| KEY-RATEADVANCE | Rate and advance — ⇧0…5 | P1 | ✅ | `crates/ui-egui/src/shortcuts.rs` → `cmd:photo.rate` (`advance`) | |
| KEY-FLAGADVANCE | Flag and advance — ⇧Z / ⇧X / ⇧U | P1 | ✅ | `cmd:photo.flag` (`advance`), `crates/ui-egui/src/shortcuts.rs` | ⇧Z pick, ⇧X reject, ⇧U unflag, each moving to the next photo |
| KEY-NEXTPREV | Next / previous — → / ← | P0 | ✅ | `cmd:library.next`, `cmd:library.previous` | |
| KEY-BA-CYCLE | Before/after — Y | P0 | ✅ | `cmd:view.beforeAfter` | toggles side by side (no cycling) |
| KEY-BA-TOPBOTTOM | Before/after top/bottom — ⌥Y | P1 | ✅ | `cmd:view.beforeAfterTopBottom`, `cmd:view.beforeAfterSplitTopBottom` | ⌥Y, ⇧⌥Y |
| KEY-BA-SPLIT | Split before/after — ⇧Y | P0 | ✅ | `cmd:view.beforeAfterSplit` | |
| KEY-CHOOSECOPY | Choose settings to copy — ⇧⌘C | P0 | ✅ | `cmd:dialog.copySettings` | |
| KEY-RESETALL | Reset all — ⇧⌘R | P0 | ✅ | `cmd:develop.reset` | |
| KEY-GENAI | Generative toggles / variations — ⌥⇧G, ⌥←/→ | OOS | 🚫 | | |
| KEY-DETECT | Detect objects — ⌥⇧O | P2 | ⬜ | | |
| KEY-CROP-CONSTRAIN | Lock crop aspect — A | P1 | ✅ | `cmd:crop.aspect` (`toggle`, `current`) | A while cropping locks the current shape / unlocks |
| KEY-CROP-SWAP | Swap crop orientation — X | P0 | ✅ | `cmd:crop.rotateAspect` | |
| KEY-CROP-OVERLAYORIENT | Crop overlay orientation — ⇧O | P1 | ✅ | `cmd:view.cropOverlayOrientation` | while cropping: O cycles the overlay, ⇧O its orientation |
| KEY-CROP-RESET | Reset crop — ⌥⌘R | P0 | ✅ | `cmd:crop.reset` | |
| KEY-STRAIGHTEN | Straighten while held — ⌘ drag | P1 | ✅ | `cmd:crop.straighten`, `crates/ui-egui/src/panels/detail.rs` (`straighten_overlay`) | ⌘-drag in Crop draws a level line; the crop tool stays active |
| KEY-SLIDER-RESET | Reset slider — double-click | P0 | ✅ | `crates/ui-egui/src/widgets.rs` | |
| KEY-SLIDER-NUDGE | Nudge slider — ↑/↓ | P1 | ✅ | `crates/ui-egui/src/widgets.rs` (`nudged`), `cmd:develop.adjust` | ↑/↓ over any slider: ≈ 1/200 of its range (exposure 0.05, most sliders 1, temperature 50 K); ⇧ ×5; one undo step each |
| KEY-SHORTCUTS | Shortcut list — ⌘/ | P1 | ✅ | `cmd:app.shortcuts` | |
| KEY-HELP | Help — F1 | P2 | ✅ | `cmd:app.help` | |
| KEY-VIDEO-PLAY | Play/pause video — Space | P1 | ⬜ | | |
| KEY-ESC | Leave tool / view — Esc | P0 | ✅ | `cmd:view.back` | |
| KEY-COMMIT | Commit tool — Return | P1 | ✅ | `cmd:tool.done` | Return closes Crop / Remove / Red Eye / Masking back to Edit (edits apply live, as in Lightroom) |
| KEY-DELETE-PIN | Delete selected pin — ⌫ | P0 | ✅ | `cmd:mask.delete`, `cmd:spot.delete` | ⌫ deletes the active mask (Masking) or the selected spot (Remove), never the photo while retouching |
| KEY-HIDEPINS | Hide pins — H | P2 | ⬜ | | H = Remove panel |

## Lightroom Classic extras

From `08-lightroom-classic-extras.md` (Classic-only features) and part 2 of `06-shortcuts.md` (Classic keys, grouped).

| Id | Feature | Tier | Status | Evidence | Missing / notes |
|---|---|---|---|---|---|
| LRC-LIB-IMPORT | Full import dialog | P1 | ✅ | `cmd:library.import` (`mode`, `destination`, `organize`, `rename`, `metadataPreset`, `preset`, `keywords`, `album`), `crates/ui-egui/src/import.rs` | review grid with duplicates skipped; add in place or copy (library Originals or any folder; by day / by month / one folder / custom folder template such as `{date:%Y}/{date:%Y%m%d}` → 2026/20260114, kept inside the destination; example destination shown; rename template with live example, Tags picker (insert at cursor) and unknown-tag warning, numbered across the import); develop preset, metadata preset, keywords, album on import |
| LRC-LIB-AUTOIMPORT | Watched-folder import | P2 | ✅ | `cmd:library.autoImport` | see LR-IMP-AUTO |
| LRC-LIB-TETHER | Tethered capture | P2 | ⬜ | | |
| LRC-LIB-VIEWS | Grid / loupe / compare / survey / people | P1 | 🟡 | `cmd:view.photoGrid`, `cmd:view.squareGrid`, `cmd:view.detail`, `cmd:view.compare`, `cmd:view.survey`, `cmd:view.gridInfo` | grid, square grid, loupe, compare, survey, second window; square-grid captions: file name, exposure or capture date (View ▸ Grid Info); no people view |
| LRC-LIB-COMPARE | Compare view | P1 | ✅ | `cmd:view.compare` | ⇧C (C is Crop here); swap, make select; zoom always linked |
| LRC-LIB-SURVEY | Survey view | P2 | ✅ | `cmd:view.survey` | N |
| LRC-LIB-REFVIEW | Reference view | P2 | ✅ | `cmd:view.reference`, `cmd:photo.setReference`, `crates/ui-egui/src/panels/compare.rs` (`show_reference`) | View ▸ Reference View (⇧R): the reference photo beside the active one (edited with the Edit panel); photo menu ▸ Set as Reference Photo |
| LRC-LIB-CATALOG-PANEL | Catalog sets | P1 | ✅ | `cmd:library.source` (`missing`) | all, recently added, picks, recently deleted, missing photos (when any) |
| LRC-LIB-CATALOG-LOCK | Catalog open in one program at a time | P1 | ✅ | `cmd:app.openLibrary`, `crates/catalog/src/lock.rs`, `crates/engine/src/library.rs` (`open_library`) | exclusive OS lock on `catalog.lock` for the session (app and `lightcraft-cli --library`); a second opener is refused with who has it (`catalog.lock.owner`: program, process, computer); released by the OS on a crash, so never stale; file systems without locks open unprotected with a warning; no read-only mode; the browser build (OPFS) has no guard across tabs yet. A library that can't be opened at launch (locked, unreadable, newer format, drive missing) shows a window — Try Again, Choose Another Library…, Continue Without Saving (then a banner; nothing written), Quit — instead of a silent demo session (`crates/ui-egui/src/panels/library_problem.rs`) |
| LRC-LIB-FOLDERS | Disk folder tree | P1 | ✅ | `cmd:library.browse`, `cmd:folder.rename`, `cmd:folder.move`, `crates/ui-egui/src/panels/left.rs` (`local_section`, `folder_tree`) | standard places + the browsed folder + Browse Folder…, each an expandable tree of subfolders, breadcrumb navigation; context menu: Rename Folder…, Move Folder To…, Show in Finder (on disk, photos relinked; one undo step each: undo / redo move the folder back and forth, refused when the destination is taken) |
| LRC-LIB-COLLECTIONS | Collections & sets | P1 | ✅ | `cmd:album.create`, `cmd:album.createSmart`, `cmd:album.toggleTarget`, `cmd:album.setTarget`, `cmd:album.clearQuick` | albums + folders (sets), smart albums (rule editor), Quick Collection and target album (B in the grids adds / removes the selection; album menu ▸ Set as Target Album; the target is marked +) |
| LRC-LIB-SMARTCOLL | Smart-collection rules | P1 | ✅ | `cmd:album.createSmart`, `cmd:album.setRules`, `cmd:album.ruleFields`, `cmd:dialog.smartAlbum`, `crates/catalog/src/rules.rs`, `crates/ui-egui/src/panels/rules_editor.rs` | rule editor: match all / any / none, nested groups (⌥+ or + Group), 26 fields (rating, flag, label, type, edits, keywords, any text, filename, format, title, caption, camera, lens, location, creator, copyright, copyright status, capture / import / edit date, ISO, aperture, focal length, megapixels, GPS, virtual copy, album) with text / number / date / in-the-last operators; live match count; also usable as a library filter (`ruleSet`) |
| LRC-LIB-PUBLISH | Publish services | P2 | ⬜ | | |
| LRC-LIB-FILTERBAR | Library filter bar | P1 | ✅ | `cmd:view.filterBar`, `cmd:filter.applyPreset`, `cmd:library.filter` (`labels`, `ruleSet`), `crates/ui-egui/src/panels/filterbar.rs` | rating/flag/label (several labels at once = any of them)/kind/edited/camera/lens/keyword, clear, save as smart album, filter presets; the filter stays as you change albums (always locked, as in the desktop app); arbitrary conditions via the smart-album rule editor |
| LRC-LIB-STACKS | Stacks (full) | P1 | ✅ | `crates/catalog/src/stacks.rs`, `cmd:stack.group`, `cmd:stack.split`, `cmd:stack.moveUp`, `cmd:stack.moveDown` | group / ungroup / toggle / set top / remove / auto by time / split / move up and down |
| LRC-LIB-VC | Virtual copies | P1 | ✅ | `cmd:photo.virtualCopy` | ⌘' |
| LRC-LIB-LABELS | Colour-label sets | P1 | ✅ | `cmd:label.sets`, `cmd:label.applySet`, `cmd:label.saveSet`, `cmd:label.deleteSet`, `cmd:label.setNames`, `crates/engine/src/cmd/manage.rs` | built-in Colors / Review sets + user sets (Photo ▸ Set Color Label, Edit Label Names… dialog); names written to and read from `xmp:Label` |
| LRC-LIB-KEYWORDS | Hierarchical keywords, sets, painter | P1 | ✅ | `crates/catalog/src/keywords.rs`, `cmd:keyword.list`, `cmd:keyword.sets`, `cmd:keyword.toggleFromSet`, `cmd:keyword.saveSet`, `cmd:tool.keywordPainter` | hierarchical `a\|b\|c` keywords (tree, parent filters include children, rename moves children); keyword sets + Recent Keywords (⌥1–⌥9); keyword painter (Keywords panel ▸ Paint: click photos in the grid to toggle a keyword, Esc stops) |
| LRC-LIB-METADATA | Metadata panel & presets | P1 | ✅ | `cmd:photo.setMeta`, `cmd:metadata.savePreset`, `cmd:metadata.applyPreset`, `cmd:metadata.presets`, `cmd:metadata.deletePreset`, `cmd:dialog.captureTime` | IPTC core, accessibility, place, capture-time edit; copyright status (unknown / copyrighted / public domain), rights usage terms and copyright info URL (`photo.setMeta` `copyrightStatus`, `usageTerms`, `copyrightUrl`; XMP Rights Management fields, kept by "copyright only" exports, a smart-album rule); metadata presets (Photo → Metadata Preset, Save Metadata Preset… from the active photo; applied on import from Settings) |
| LRC-LIB-QUICKDEV | Quick develop | P2 | ✅ | `cmd:develop.quickAdjust`, `crates/ui-egui/src/panels/edit.rs` (`quick_develop`) | grid with several photos selected: Edit panel ▸ Quick Develop steps (exposure ⅓ / 1 stop, contrast, highlights, shadows, whites, blacks, clarity, vibrance, temperature) added to each photo's own value, one undo step |
| LRC-LIB-PEOPLE | People view | P2 | ⬜ | | |
| LRC-LIB-COMMENTS | Comments panel | P2 | ⬜ | | |
| LRC-LIB-VISUALSEARCH | Find similar photos | P2 | ✅ | `cmd:library.findSimilar`, `crates/pipeline/src/cull.rs` (`signature`) | photo menu / Photo ▸ Find Similar Photos: look-alike photos (composition and tones), most similar first, as a filter (`only`) |
| LRC-LIB-MISSING | Missing files & relink | P1 | ✅ | `cmd:library.missing`, `cmd:photo.relink`, `cmd:library.findMissing`, `cmd:file.findMissing`, `cmd:photo.locate`, `crates/engine/src/cmd/missing.rs` (`checked_path`) | File → Find Missing Photos… (anywhere in a folder: same name, size and content hash; renamed files by size + content hash; same name and size alone only when unambiguous — look-alikes are skipped and reported; searched on a worker thread, one undo step), photo menu → Locate Missing File…; unreadable files show "!" in the grid and a reason in the loupe; undo never moves files. Missing Photos in the sidebar (counted on a worker thread; the view, the Info panel, the grid's context menu and thumbnails read file availability from a cache a worker thread fills, so an offline NAS never stalls a frame); it covers library photos only — Local browse records are never checked, for the count, the view, `library.missing` and Find Missing alike |
| LRC-LIB-CONVERT | Convert to DNG | P2 | ✅ | `cmd:photo.convertToDng`, `crates/engine/src/cmd/convert.rs` | Photo ▸ Convert to DNG: lossless DNG next to the raw with the settings embedded, photo (and its virtual copies) relinked; originals kept; undoable; the DNG is decoded and compared with the raw data, written to a temp file, synced, read back and only then given a free name (never replacing a file) — a failed write leaves no DNG and the photo on its raw (issue #106) |
| LRC-LIB-PREVIEWS | Build / discard previews | P1 | ✅ | `cmd:library.buildPreviews`, `cmd:library.previewProgress`, `cmd:library.cancelPreviews`, `cmd:library.clearPreviews`, `crates/engine/src/cmd/previews.rs`, `crates/preview/src/lib.rs` | File ▸ Previews: build standard-sized (Settings → Performance size) or 1:1 previews of the selected / visible photos in the background (progress toasts, stop), discard the cache; disk thumbnail + view cache; no smart previews (offline proxies) |
| LRC-LIB-SLIDESHOW-IMPROMPTU | Impromptu slideshow | P2 | ✅ | `cmd:view.slideshow` | see LR-VIEW-SLIDESHOW |
| LRC-DEV-SNAPSHOTS | Named snapshots | P1 | ✅ | `cmd:version.create`, `cmd:version.restore` | = versions |
| LRC-DEV-HISTORY | Full history panel | P1 | ✅ | `cmd:history.list`, `cmd:history.restore`, `cmd:history.clear` | row menu: copy step to before, create version from step, clear history |
| LRC-DEV-SOFTPROOF | Soft proofing | P2 | 🟡 | `cmd:view.softProof`, `crates/pipeline/src/output.rs` (`Proof`), `crates/ui-egui/src/panels/edit.rs` (`soft_proofing`) | S in the loupe (in grids S stays Expand/Collapse Stack): paper-white surround and "Proof Preview", proof profile (sRGB / Display P3 / Adobe RGB / ProPhoto / Rec. 2020), destination (red) and display (blue) gamut warnings, Create Proof Copy (a named virtual copy). Missing: printer ICC profiles, rendering intent, Simulate Paper & Ink |
| LRC-DEV-AUTOSYNC | Sync / auto sync / paste previous | P1 | ✅ | `cmd:develop.sync`, `cmd:develop.autoSync`, `cmd:develop.pastePrevious` | Edit ▸ Sync Settings / Auto Sync (⌥⇧⌘A): only the changed settings carry over, one undo step, slider drags sync on release, spots / red eye stay per photo; Edit panel banner |
| LRC-DEV-MATCHEXP | Match total exposures | P2 | ✅ | `cmd:develop.matchExposure` | Photo ▸ Match Total Exposures: the selected photos' Exposure set so shutter × ISO ÷ aperture² plus the slider matches the active photo's |
| LRC-DEV-CALIB | Calibration panel | P1 | ✅ | `ctl:calibration.*` | |
| LRC-DEV-TAT | Targeted adjustment tools | P1 | ✅ | `cmd:develop.targeted` (`target`: curve / hue / sat / lum) | |
| LRC-DEV-DEFAULTS | Per-camera raw defaults | P1 | ✅ | `cmd:library.preferences` (`camera`, `import.perCamera`), `crates/ui-egui/src/panels/settings.rs` | Settings → Import: raw default and per-camera presets |
| LRC-DEV-VIEWOPTIONS | Develop view options | P2 | ⬜ | | |
| LRC-DEV-VIDEO | Video frame capture | P2 | ⬜ | | |
| LRC-MAP-VIEW | Map view | P2 | ⬜ | | |
| LRC-MAP-GEOTAG | Drag photos onto the map | P2 | ⬜ | | |
| LRC-MAP-LOCATIONS | Saved locations | P2 | ⬜ | | |
| LRC-MAP-TRACKLOG | GPS track logs | P2 | ✅ | `cmd:photo.autoTagTracklog`, `cmd:photo.tagFromTracklog`, `crates/meta/src/gpx.rs` | Photo ▸ Auto-Tag from Tracklog…: a GPX 1.0 / 1.1 track log sets the GPS of the selected photos by capture time — interpolated between the points of a track segment, else the nearest point within `maxGap` (10 min); never across segment breaks. The camera's time zone comes from the photo (Exif offset) or is asked for; photos that already have a location keep it unless `replace`; `dryRun` previews; one undo step. No track drawn on a map (no Map module) |
| LRC-MAP-FILTER | Location filter bar | P2 | ⬜ | | |
| LRC-MAP-REVGEO | Reverse geocoding | OOS | 🚫 | | |
| LRC-BOOK-SETTINGS | Book settings | P2 | ⬜ | | |
| LRC-BOOK-AUTOLAYOUT | Book auto layout | P2 | ⬜ | | |
| LRC-BOOK-PAGE | Book pages & templates | P2 | ⬜ | | |
| LRC-BOOK-GUIDES | Book guides | P2 | ⬜ | | |
| LRC-BOOK-CELL | Book cell padding | P2 | ⬜ | | |
| LRC-BOOK-TEXT | Book photo/page text | P2 | ⬜ | | |
| LRC-BOOK-TYPE | Book typography | P2 | ⬜ | | |
| LRC-BOOK-BG | Book backgrounds | P2 | ⬜ | | |
| LRC-BOOK-VIEWS | Book views | P2 | ⬜ | | |
| LRC-BOOK-EXPORT | Book export (PDF/JPEG) | P2 | ⬜ | | |
| LRC-SS-TEMPLATES | Slideshow templates | P2 | ⬜ | | |
| LRC-SS-OPTIONS | Slideshow options | P2 | ⬜ | | |
| LRC-SS-LAYOUT | Slideshow layout | P2 | ⬜ | | |
| LRC-SS-OVERLAYS | Slideshow overlays | P2 | ⬜ | | |
| LRC-SS-BACKDROP | Slideshow backdrop | P2 | ⬜ | | |
| LRC-SS-TITLES | Slideshow titles | P2 | ⬜ | | |
| LRC-SS-MUSIC | Slideshow music | P2 | ⬜ | | |
| LRC-SS-PLAYBACK | Slideshow playback | P2 | ⬜ | | |
| LRC-SS-EXPORT | Slideshow export | P2 | ⬜ | | |
| LRC-PRINT-LAYOUTSTYLE | Print layout styles | P2 | ⬜ | | |
| LRC-PRINT-IMAGESETTINGS | Print image settings | P2 | ⬜ | | |
| LRC-PRINT-LAYOUT | Print layout | P2 | ⬜ | | |
| LRC-PRINT-GUIDES | Print guides | P2 | ⬜ | | |
| LRC-PRINT-CELLS | Picture-package cells | P2 | ⬜ | | |
| LRC-PRINT-PAGE | Print page options | P2 | ⬜ | | |
| LRC-PRINT-JOB | Print job & colour management | P2 | ⬜ | | |
| LRC-PRINT-TEMPLATES | Print templates | P2 | ⬜ | | |
| LRC-WEB-LAYOUT | Web gallery layouts | OOS | 🚫 | | |
| LRC-WEB-SITEINFO | Web gallery site info | OOS | 🚫 | | |
| LRC-WEB-COLOR | Web gallery colours | OOS | 🚫 | | |
| LRC-WEB-APPEARANCE | Web gallery appearance | OOS | 🚫 | | |
| LRC-WEB-IMAGEINFO | Web gallery image info | OOS | 🚫 | | |
| LRC-WEB-OUTPUT | Web gallery output | OOS | 🚫 | | |
| LRC-WEB-UPLOAD | Web gallery upload | OOS | 🚫 | | |
| KEYC-PANELS | Classic panel keys (Tab, ⇧Tab, T, F5–F8, solo) | P2 | ⬜ | | |
| KEYC-MODULES | Classic module switching (⌘⌥1–7) | P2 | 🚫 | | no modules in LightCraft |
| KEYC-VIEWS | Classic view keys (E, G, C, N, L, F, I, ⇧R, ⌘⌥0) | P2 | 🟡 | `cmd:view.photoGrid`, `cmd:view.zoom100` | G works; E opens Edit (not loupe); no compare/survey/lights-out/screen modes |
| KEYC-SECONDWINDOW | Classic secondary-window keys | P2 | ⬜ | | |
| KEYC-CATALOG | Classic photo/catalog keys (⇧⌘I, ⌘', ⌘R, F2, ⌫, ⇧⌘E…) | P2 | ✅ | `cmd:library.import`, `cmd:photo.delete`, `cmd:photo.virtualCopy`, `cmd:app.showInFinder`, `cmd:dialog.rename`, `cmd:photo.editInExternal` | ⇧⌘I add, ⌘' virtual copy, ⌘R show in Finder, F2 rename, ⌫ delete, ⇧⌘E external editor |
| KEYC-COMPARE | Classic grid/compare keys (Z, Home/End, =/−, ⌘⇧D, S…) | P2 | 🟡 | `cmd:view.zoomToggle` | Z toggles zoom; no compare, stacks, thumbnail-size keys |
| KEYC-RATING | Classic rating/flag keys (1–5, ⇧1–5, 6–9, P, X, U, ⇧X, ⇧U, `[` `]`, \`) | P2 | 🟡 | `cmd:photo.rate`, `cmd:photo.pick`, `cmd:photo.flag` | most work; no ⇧P / ⇧6–9 advance, rating `[` `]`, flag cycle, filter-bar keys |
| KEYC-COLLECTIONS | Classic collection keys (⌘N, B…) | P2 | 🟡 | `cmd:dialog.newAlbum` | ⌘N new album; no quick collection |
| KEYC-METADATA | Classic keyword/metadata keys (⌘K, ⌘S, ⌘⌥⇧C/V…) | P2 | 🟡 | `cmd:photo.saveMetadataToFile` | ⌘S saves metadata; no keyword sets, metadata copy/paste |
| KEYC-DEVELOP | Classic develop keys (V, ⌘U, ⇧⌘U, R, Q, K, M, ⇧M, ⇧W, ⇧J, ⇧Q…) | P2 | 🟡 | `cmd:develop.treatment`, `cmd:develop.reset`, `cmd:crop.reset` | V, ⇧⌘R, ⌥⌘R, W, J, Y, ⇧Y, \ match; R/K/M/⇧M differ; no Classic keymap layer |
| KEYC-MODULE-OUTPUT | Book / slideshow / print / map / web keys | P2 | ⬜ | | modules not implemented |
| KEYC-HELP | Classic help keys (⌘/, F1) | P2 | 🟡 | `cmd:app.shortcuts` | ⌘/ only |

### Japanese text watermarks

`app.export` watermark objects accept `vertical` (boolean, defaults to false). The export
dialog offers Horizontal/Vertical and multiline text. Japanese glyphs fall back to the craft-fonts
Mincho faces (BIZ UDMincho; on the web build, BIZ UDPGothic) when built with `CRAFT_FONTS_DIR`. Vertical lettering uses upright em cells and right-to-left newline columns;
Latin stays upright. Advanced Japanese composition remains open. Regression coverage:
`export::tests::japanese_watermarks_support_vertical_columns`, `japanese_watermark_options_and_legacy_defaults`, `watermarks_work_without_craft_fonts`.
