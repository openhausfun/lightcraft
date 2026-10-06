# LightCraft roadmap

Milestones toward full Adobe Lightroom parity (cloud Lightroom first, then every Lightroom Classic module), with wall-clock
estimates for continuous (24/7) agent-driven development with 4–6 parallel agents. Estimates are calibrated on the sibling
projects (DrawCraft reached its first four milestones in ≈ 4½ h) and are revised as milestones land.

## Where we stand

*Honest assessment, 2026-10-05. Agents: read this before picking work. The checklist in
[`docs/parity.md`](docs/parity.md) counts features that **exist**; this section is about whether a photographer can
**switch** from Lightroom. Update it when a gap below closes.*

**In one line:** the checklist says **79%** (P0 98.5%, P1 95.9%, P2 39%), but measured by whether a working
photographer could replace Lightroom without noticing, we are at roughly **60–70%**. The remaining gap is mostly
**quality of results and camera coverage**, not missing buttons.

Caveat on the checklist: ✅ is set by whoever lands a feature, and nobody has systematically checked rows against
Lightroom's behaviour or output. Bugs keep turning up in ✅ areas (CR2 colour-filter phase on some Canon models #85,
duplicate Local entries #22, black GPU exports on an Intel iGPU #78).

### By dimension

| Dimension | Estimate | What's true today | Biggest gaps |
|---|---:|---|---|
| **Feature checklist** | 79% | P0 core and P1 nearly complete: import (Add / Copy / Move, templates, devices), library, grid/loupe/compare/survey, every Edit slider, curves, colour grading, masking tools, crop/Upright, heal/clone, presets/profiles, versions/history, sync, export, menus, shortcuts | P1: lens-profile database, content-aware fill (patch synthesis), video playback/trim |
| **RAW coverage** (formats people shoot) | ~50% | DNG (all kinds), CR2, ARW, NEF (uncompressed + Huffman lossless/lossy), uncompressed RAF/ORF, packed RW2, PEF; every container's embedded preview (incl. CR3) | **CR3** (every Canon since ~2018), compressed RAF/ORF, RW2 v4, Nikon lossy-after-split, Canon sRAW, HEIC/AVIF. Per-model verification is thin (~40 corpus files vs >1,000 models) |
| **Colour & image quality** | ~55–65% | Pipeline is complete and fast; GPU path CPU-exact within 1/255 | **No measured camera calibration database**: ARW has a guarded per-file embedded-JPEG colour estimate (docs/camera-preview-colour.md); other non-DNG raws and rejected estimates use a neutral matrix. Colour fidelity remains incomplete. No lens-profile database. No measured fidelity against Lightroom (tone, highlights, texture/clarity, NR, sharpening are tuned by eye) |
| **AI & computational** | ~15–20% | Assisted culling (focus, bursts), auto tone, HDR/panorama merge; subject/sky/background masks as classical heuristics | Real segmentation masks (subject, sky, people, objects, landscape, depth), AI denoise, super resolution, lens blur, generative remove, faces/people, natural-language search. **Blocked on a model strategy** (licensable weights or our own training; pure-Rust inference is feasible) |
| **Workflow & library** | ~85% (single machine) | Robust catalog (journal + snapshots, background compaction, crash-tested), 85k-photo libraries stay responsive, Local browsing with automatic cleanup, XMP interop, keywords, smart albums, Move import | Opening an 85k library takes 1.7–4.7 s; no cloud sync (out of scope), no tablet companion (#74, roadmap), shared albums, publish services, tethering |
| **Classic modules** | ~30% | Geotagging from GPX track logs, soft proofing (partial), slideshow (basic) | **Map view, Book, Print, Slideshow module, Web, publish services**: ~40 tracker rows ⬜ |
| **HDR & video** | 0% | | HDR edit/display/export; video play/trim/edit/export |
| **Platform & robustness** | ~70% | macOS native; Windows/Linux builds; web via WASM; no-panic lints workspace-wide, `unsafe` confined to `crates/sysmem`; failed saves are reported; GPU errors fall back to CPU | Windows installer UI unverified on Windows (PR #79); GPU path proven only on Apple + user reports; Japanese/English UI (see docs/localization-ja.md); remaining technical errors and other languages; accessibility partial; headless UI tests time out under machine load |

### By kind of user

| User | Readiness | What blocks them |
|---|---:|---|
| JPEG / DNG shooter, single machine | ~85% | Fidelity polish, AI masks |
| Nikon / Sony / older-Canon raw shooter | ~65% | Camera colour fidelity and coverage (ARW preview estimates are only a starting point) |
| Canon CR3 / Fujifilm / Olympus shooter | ~35% | Their raws open as embedded previews only |
| Lightroom Classic power user | ~45% | Print, Book, Map, publish, tethering |
| Relies on AI (masks, denoise) | ~25% | No segmentation or AI denoise models |

## Where we're going

Priorities, in order. Each points at tracker rows in [`docs/parity.md`](docs/parity.md) → *Top gaps*.

1. **Camera colour calibration of our own** (LR-PROF-CAMERACOLOR, P0): fit each camera to its own embedded JPEG, use
   matrices the files carry themselves, then chart shots. Sony ARW's file-local fit (matrix + tone curve from its own
   JPEG) is the first step; generalise it to the other makes' raws (NEF, RW2, PEF, ORF…), then validate fidelity.
2. **Raw formats, clean-room** (LR-IMP-FORMATS, P0): **CR3** first, then compressed RAF / ORF, RW2 v4, NEF
   lossy-after-split, sRAW. Decided 2026-10-05: write our own decoders from prose descriptions (never decoder source,
   no LGPL dependency); compressed NEF (#86) is the template.
3. **Verified camera coverage** (LR-IMP-CAMERA-COVERAGE, P0): a CC0 sample per model in the corpus, each decoded and
   checked for plausible colour; fix per-model bugs (#85).
4. **Render fidelity suite** (LR-BEHAV-RENDER-FIDELITY, P1): measure our output against Lightroom on the same CC0 raws
   (references stay in the local `plan/`), then tune against the numbers.
5. **Lens profiles of our own** (LR-EDIT-OPTICS-PROFILE, P1).
6. **AI model strategy** (maintainer decision): which permissively licensed models (or our own training) for
   segmentation masks and denoise; then pure-Rust inference. Unblocks M12 and Enhance.
7. **Then:** HDR (Q), the Classic output modules (Print first, then Map view, Book, Slideshow), video (R), localisation
   and accessibility.

Already closed in the week of 2026-10-03: compressed NEF (#10), Move import (#29), per-library smart-preview folder,
Local roots / cleanup, grid and catalog performance at 85k photos (#35, #37), failed-save reporting, GPU export
hardening (#78), copyright metadata (#51), GPX geotagging (#60), import tag help and folder templates (#31, #32).

## Milestones

**Status legend:** ✅ done · 🚧 in progress · ⬜ not started

| # | Milestone | Scope (summary) | Estimate (h) | Status |
|---|---|---|---|---|
| M0 | Skeleton + visual shell | workspace, xtask CI + layering, geom/color/raster, develop model, pipeline v0, catalog v0, engine commands, Lightroom-look UI (grid, loupe, filmstrip, Edit panel), control channel, MCP, web build | 3–5 | ✅ |
| M1 | Library core | import (JPEG/PNG/TIFF/WebP), EXIF/XMP, persistent catalog (op log + snapshots), albums, ratings/flags/labels, filter/search/sort, thumbnail cache, 100k-photo grid | 6–10 | ✅ (100k-photo grid scale test pending) |
| M2 | Pipeline v1 (quality) | WB temp/tint, profiles, local tone mapping (highlights/shadows), curves, HSL, point colour, colour grading, texture/clarity/dehaze, vignette, grain, B&W, auto tone/WB, histogram, before/after | 10–15 | ✅ (look tuning vs our references ongoing) |
| M3 | RAW I | TIFF/DNG (LJ92, deflate, tiles, opcodes), demosaic (AHD/PPG/bilinear), highlight recovery, DNG colour model, CR2, NEF, ARW, embedded previews | 10–15 | 🚧 (DNG, CR2, ARW, NEF uncompressed + Huffman lossless/lossy, embedded previews ✅; NEF lossy-after-split ⬜) |
| M4 | Crop, geometry, optics | crop tool + overlays, straighten, Upright (auto/level/vertical/full/guided), manual transforms, CA, defringe, manual lens corrections | 6–10 | ✅ |
| M5 | Performance | source pyramids, wgpu compute pipeline (CPU oracle), draft/full renders, prefetch, budgets (16 ms slider updates on 24 MP) | 10–15 | 🚧 (stage cache, source pyramid, wgpu pipeline, prefetch, memory budget ✅; colour NR at half resolution, GPU histogram ⬜) |
| M6 | Masking | brush, linear/radial gradients, colour/luminance/depth range, add/subtract/intersect/invert, all local adjustments, masks panel | 8–12 | 🚧 (brush/linear/radial/colour/luminance range, add/subtract/intersect, masks panel ✅; depth range, AI masks ⬜) |
| M7 | Detail | sharpening + masking preview, luminance/colour NR, Denoise, Raw Details, Super Resolution | 6–10 | 🚧 (sharpening, luminance/colour NR ✅; AI Denoise, Raw Details, Super Resolution ⬜) |
| M8 | Heal / Remove | content-aware remove (PatchMatch), heal, clone, brush spots, visualize spots, red/pet eye | 6–10 | 🚧 (heal, clone, auto source, visualize spots, red/pet eye ✅; PatchMatch remove ⬜) |
| M9 | Presets, profiles, versions, sync | preset browser + amount, create/import presets, profile browser, versions, history, copy/paste/sync settings | 5–8 | ✅ |
| M10 | Export & share | export dialog (JPEG/PNG/TIFF/DNG/AVIF/JXL/original), sizing, sharpening, metadata, watermark, naming, batch jobs, XMP sidecars, HDR export | 6–10 | 🚧 (all formats incl. DNG/original, sizing, presets, background jobs ✅; JXL encode, HDR export ⬜) |
| M11 | RAW II | CR3, RAF (X-Trans), ORF, RW2, PEF, SRW, 3FR, IIQ + long tail; camera calibration DB; HEIC/AVIF/JXL import | 20–35 | 🚧 (RAF uncompressed, RW2 packed, PEF, ORF uncompressed ✅; **camera colour calibration** 🚧 (guarded ARW preview fitting; measured database still missing), CR3, compressed ORF/RAF ⬜) |
| M12 | AI & smart features | subject/sky/background/people/object masks, semantic search, faces/People (permissively licensed models, pure-Rust inference) | 20–40 | ⬜ |
| M13 | Merge | HDR merge (deghost), panorama (projections, boundary warp, fill edges), HDR panorama | 10–15 | ✅ |
| M14 | Video | import/playback/trim via FilmCraft crates, global edits + presets on video, video export | 6–10 | ⬜ |
| M15 | Classic modules | Map, Book, Slideshow, Print, Web; smart collections, stacks, virtual copies, publish services, tethering | 25–40 | 🚧 (smart albums, stacks, virtual copies, compare/survey ✅; Map/Book/Slideshow/Print/Web ⬜) |
| M16 | 1.0 polish | preferences, shortcut editor, accessibility, localization, packaging (dmg/msi/AppImage/web), hardening | 10–20 | 🚧 (settings, keyboard shortcuts sheet, packaging basics ✅; Japanese/English localisation 🟡; accessibility and other locales ⬜) |

## Parity estimate (feature count updated 2026-10-05; effort estimate from 2026-10-02)

**By feature count** — from `docs/parity.md` (one row per Lightroom feature, menu item and shortcut; `cargo xtask
parity` prints this line on every run, so it stays current):

| Scope | Weighted completion | Rows | 2026-10-02 |
|---|---:|---:|---:|
| P0 (core) | **98.5%** | 200 | 98.7% |
| P1 (important parity) | **95.9%** | 147 | 94.4% |
| P2 (later / AI / niche, incl. Classic modules) | **39.2%** | 158 | 29.4% |
| **All in-scope rows** | **79.2%** | 505 | 75.6% |

The 2026-10-05 count includes three new 🟡 rows that make quality gaps visible (camera colour calibration, verified
camera coverage, render fidelity), which is why P0 dipped slightly.

✅ counts 1, 🟡 ½, ⬜ 0; out-of-scope rows (cloud sharing, Adobe accounts…) are left out.

**By remaining effort** — rows are not equal: a shortcut and the whole Book module are one row each, and what is
left is the heavy part (AI, video, Classic output modules, undocumented raw codecs). Remaining work in **Opus agent-hours**
(one agent working continuously; calibrated on this project — a single lead agent closed ≈ 45 tracker rows of
UI/feature work in ≈ 5 h on 2026-10-01, and ≈ 60 more (preset import incl. `.lrtemplate` / DNG / zip and masks, smart-album
rule editor, auto sync, keyword and label sets, import options and DNG conversion, smart previews, external-editor round
trip, slideshow, auto import…) in ≈ 12 h on 2026-10-02; four to six parallel agents built M0–M13 in ≈ 25 active hours):

| Work package | Tracker rows | Agent-hours | Risk |
|---|---|---:|---|
| Remaining P0/P1 UI and library features (folder rename/move, keyword painter, people view…) | ≈ 8 | 5–10 | low |
| Raw codecs: CR3 (CRX), compressed ORF / RAF, NEF lossy-after-split, RW2 v4, HEIC/AVIF decode, JPEG XL DNG | LR-IMP-FORMATS | 40–80 | **high** — clean-room black-box analysis, no permissive specs |
| Lens-profile database of our own (calibration targets, fitting, data) | LR-EDIT-OPTICS-PROFILE | 15–30 | data collection |
| Video: playback, trim, edits, export (pure-Rust decode, ideally shared with FilmCraft) | R. Video | 20–40 | medium |
| AI: subject / sky / background / people / object masks, object-aware remove, AI denoise, super resolution, lens blur, people & faces, natural-language search, culling | ≈ 30 | 80–150 | **high** — permissively licensed weights, pure-Rust inference, maybe training |
| HDR editing, display, visualisation and export | Q. HDR, LR-EXP-HDR | 15–25 | medium |
| Classic modules: Map, Book, Slideshow module, Print, publish services, tethering, soft proofing | ≈ 50 | 60–100 | medium (large, well-understood) |
| Smaller P2 items: Enhance dialog, export to Photos, help / what's new, accessibility, localisation, sidecar variants | ≈ 15 | 12–25 | low |
| Look tuning, performance budgets (colour NR at half resolution on CPU + GPU, 100k-photo library), packaging, hardening | — | 25–45 | medium |
| **Total remaining** | | **≈ 270–505** | |

Spent so far ≈ 105–145 agent-hours, so **by effort the project is ≈ 20–35% of the way to complete Lightroom + Classic
parity**, and ≈ 45–60% of the way for cloud-Lightroom parity without AI and the Classic modules (remaining ≈ 130–255 h,
most of it the raw codecs, lens data, video and HDR).

**Wall clock:** ≈ 270–505 h for one agent working alone; with 4–6 parallel agents (≈ 70% parallel efficiency, merges and
CI under load cost the rest) **≈ 65–125 h of continuous work**. The AI package and the raw codecs carry most of the
uncertainty: they can finish faster if suitable permissive models / documentation turn up, or stall on licensing.

## Totals

Remaining from 2026-10-02 evening (see *Parity estimate* above for the breakdown):

| Target | Remaining agent-hours | Wall clock, 4–6 parallel agents |
|---|---:|---:|
| Cloud-Lightroom parity without AI or Classic modules | ≈ 130–255 h | ≈ 35–65 h |
| Full parity incl. AI, video and the Classic modules | ≈ 270–505 h | ≈ 65–125 h |

The milestone estimates in the table above were made before work started and are kept for calibration (M0–M13 took
≈ 25 active hours with parallel agents against an estimate of ≈ 110–170 h for those milestones).

## Risks that coding hours alone don't retire

- **AI features** (subject/sky/people masks, generative remove) need model weights with licences we can ship; classical
  fallbacks first. No permissively licensed sky-segmentation or raw-denoise model was found — we may need to train our own.
- **Camera colour and lens data** is a data problem: we never use Adobe's matrices, DCPs or LCPs. DNG-embedded data first,
  then our own calibration; long-tail camera/lens coverage grows over time.
- **Raw-format sources:** decided 2026-10-05: decoders are written from *prose* format descriptions (even ones
  published alongside GPL code); decoder source is never read. Still open: freedom-to-operate review for local
  Laplacian filters, PatchMatch and HEVC (HEIC).
- **Look parity** with Adobe's default rendering is tuned by eye today; the planned fidelity suite (LR-BEHAV-RENDER-FIDELITY)
  turns it into measured comparisons against local-only Lightroom references.

## Raw format coverage and known gaps

Decoded (CC0 corpus from raw.pixls.us, `cargo xtask corpus --download`, `crates/raw/tests/corpus.rs`): DNG (uncompressed,
LJ92, lossy JPEG / Smart Previews, Deflate, float, linear), CR2, ARW (uncompressed, ARW2, LJ92), NEF/NRW uncompressed and Huffman-compressed (lossless, lossy type 1/2, 12/14-bit), RAF uncompressed (Bayer and
X-Trans), RW2 packed 12/14-bit, PEF (uncompressed and Huffman), ORF uncompressed (16-bit and 12-bit packed). Every
supported container also yields its embedded JPEG preview (CR3 too), and the engine shows that preview for raw variants
it can't decode yet.

Not decoded yet — preview only (no permissively licensed description; black-box analysis incomplete):
- **Nikon "lossy after split" NEF** (non-zero split row in maker note `0x0096`, e.g. some D3400/D5000/D5200/D5500/D5600
  files): rows above the split decode with the regular lossy table; from the split row on a different code is used
  that our black-box analysis has not recovered yet (none of the four regular tables fits, also not with byte
  alignment or reset predictors at the split row). The other Nikon Huffman variants are decoded (`crates/raw/src/vendor/nefc.rs` documents the analysis).
- **Panasonic RW2 raw format 4** (quantised): the block layout is known (0x4000-byte chunks rotated by 0x1ff8; 128-bit
  blocks of two 12-bit seeds plus four groups of a 2-bit scale and three 8-bit codes; scales 0/1 are ×1/×2 differences
  from the same-colour pixel two to the left), the reconstruction rule for scales 2/3 is not established.
- **Olympus compressed ORF**, **Fujifilm compressed RAF**, **Canon CR3/CRX** (M11.1), **Canon sRAW/mRAW**, lossy DNG.

**Camera colour matrices:** ARW files can use guarded, separate chromaticity and tone estimates from their own embedded JPEG (see `docs/camera-preview-colour.md`); this is a per-file camera-look estimate with relative WB, not measured calibration or absolute-Kelvin WB. Other non-DNG raws and rejected fits use the documented neutral fallback (camera RGB ≈ linear sRGB, flagged
`matrix_is_fallback`) with the file's as-shot white-balance multipliers. Clean sources to evaluate next: manufacturer
matrices stored in the files themselves (Olympus ImageProcessing `ColorMatrix`, Pentax/Panasonic equivalents) and our
own chart-based calibration (M11.4). Adobe matrices are never used.

## Log
- 2026-09-30: roadmap created; M0 in progress; research docs (Lightroom reference, Rust imaging ecosystem) complete.
- 2026-09-30 (later): app running with the full Lightroom-style UI; pipeline v0; DNG/CR2/ARW; README showcase. ≈ 8 h elapsed.
- 2026-10-02: parity estimate added (xtask parity prints weighted completion); milestone statuses refreshed.
- 2026-10-01: RAW II formats: RAF (uncompressed Bayer + X-Trans), RW2 (packed), PEF (incl. Huffman), ORF (uncompressed); embedded previews for every container incl. CR3; raw corpus test with 37 CC0 samples.
- 2026-10-03 – 10-05: community PRs (#42–#51, #60) and 25+ issues landed: compressed NEF, Move import, folder templates,
  Local roots and cleanup, 85k-photo grid and catalog performance, background compaction, failed-save errors, GPU
  export hardening, clippy 1.99. Added the honest *Where we stand* assessment and *Where we're going* priorities; added
  tracker rows for camera colour, camera coverage and render fidelity.

## Japanese interface and text watermarks

English/Japanese interface language is persisted in UI state. Core menus have Japanese
translations; untranslated panels and dialogs retain English. Japanese glyphs (UI: BIZ UDPGothic;
watermarks: BIZ UDMincho) come from storytold/craft-fonts, embedded by builds made with the
optional `CRAFT_FONTS_DIR` input (all releases), so no system fonts are needed. Text watermarks now accept
`vertical: true` in export JSON/presets and expose an orientation selector. Japanese
characters stay upright in top-to-bottom columns, with newlines starting columns to the
left. This is basic lettering, without tate-chu-yoko, ruby, kinsoku, or general vertical
OpenType shaping. The same coverage renderer serves 8/16/32-bit exports.
