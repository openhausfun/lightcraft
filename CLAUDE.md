# LightCraft — instructions for agents

LightCraft is a clean-room, open-source, pure-Rust photo library + non-destructive raw developer targeting Adobe Lightroom parity (and beyond). Native on macOS, Windows, Linux; web via WASM. Sibling of `../printcraft` (Acrobat), `../photocraft` (Photoshop), `../drawcraft` (Illustrator) and `../filmcraft` (Premiere), with the same conventions.

## Start every session here
1. Read `plan/STATUS.md` (current milestone, next unchecked task, blockers).
2. Read the task in `plan/execution-plan.md` §3, the relevant section of `plan/architecture.md`, and the README/docs of the crate you touch. Behaviour/visual reference: `plan/lightroom/` (incl. `10-observed-ui.md` + screenshots).
3. **Know where we stand:** read [`ROADMAP.md`](ROADMAP.md) → *Where we stand* (honest assessment by dimension and
   by kind of user) and *Where we're going* (ordered priorities). The checklist counts features that *exist*; the real
   gaps are quality and coverage: camera colour calibration, CR3 / compressed raws, per-model verification, render
   fidelity against Lightroom, AI models, HDR / video / Classic modules. A ✅ row is not proof of parity — if you find a
   ✅ feature that is wrong or incomplete, downgrade it to 🟡 with a note.
4. **Pick work** from [`docs/parity.md`](docs/parity.md) → *Top gaps* (ordered by user impact; the tracker has one row per feature, menu item and shortcut). When you land a feature, update its row(s) and the gap list in the same commit, and the *Where we stand* / *Where we're going* sections of `ROADMAP.md` when a listed gap closes; `cargo xtask parity` (part of `ci`) checks that every `cmd:`/`ctl:` id and path the tracker cites still exists, and `cargo xtask parity --write` refreshes its summary.
5. Follow the autonomous operation protocol (`plan/execution-plan.md` §7). Don't stop to ask unless §7 lists the decision as the user's.

`plan/` is gitignored (local-only).

**Merging agent branches:** merge one branch, run `cargo xtask ci`, fix, commit — then the next. Two individually green
branches can still break each other (e.g. a new struct field vs. a new constructor).

**Resuming after a crash / another session:** check `git status` on main, `git worktree list`, and each worktree's
`git log main..HEAD` + `git status` for unmerged commits or uncommitted work before starting anything new. Commit small
and often so a crash loses minutes, not hours.

## Never crash (outranks feature work)
People trust LightCraft with their photo libraries and edits; a crash loses their work. A malformed raw/JPEG/XMP, a bad
command, control or MCP argument, a corrupt catalog or settings file, or a full disk must produce an error the user (or
agent) can act on, never a panic. Don't ship a feature by adding a panic path, and fix a crash before building on top
of it. Full standard: `../craftrules/standards/never-crash.md`
([storytold/craftrules](https://github.com/storytold/craftrules/blob/main/standards/never-crash.md)).
- **Non-test code never panics:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, and
  no `unsafe`. Return the crate's error type through `Result` and `?`; use `ok_or(..)?`, `let … else`, `if let`, or a
  fallback (`unwrap_or…`) only where it can't silently corrupt a document. An unfinished feature returns an
  "unsupported" error or is disabled. Sole exception: a provably infallible literal, as `#[allow(clippy::expect_used)]`
  + `.expect("why it can't fail")`.
- **`unsafe` lives only in `crates/sysmem`** (one FFI call, `malloc_zone_pressure_relief`, that returns freed
  allocator pages to macOS after raw decodes). Every other production crate root has `#![forbid(unsafe_code)]`. A new
  unsafe need goes in an isolated, well-tested helper crate like it: `// SAFETY:` on every block, a safe API, a safe
  fallback where possible, and a line here naming it.
- **Input-derived numbers are hostile:** `get()` instead of `[i]`/`[a..b]` for offsets from files, users, agents or
  arithmetic on them; checked/saturating math for lengths, offsets and counts; no division by zero, NaN/inf or negative
  casts to `usize`; cap allocations sized by input; slice strings only at char boundaries.
- **Bound recursion** (depth limits or seen-sets: IFD chains, nested metadata, collections). **Don't cascade:**
  `lock().unwrap_or_else(PoisonError::into_inner)` or an error; worker-thread joins are `Result`s.
- **Last-resort guard:** a panic hook plus `catch_unwind` around command dispatch and import/export turns an escaped
  panic into an error dialog and keeps the document. It's a safety net, not a licence; keep `panic = "unwind"` on native.
- **Prove it:** every crash fix lands with a small synthetic regression test that panicked before the fix.
- Enforced by clippy: root `clippy.toml` allows unwrap/expect/panic/indexing in tests only, and every production crate
  root (`lib.rs`, each binary's `main.rs`) carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
  clippy::unimplemented, clippy::todo, clippy::unreachable)]`. Not `[workspace.lints]`: those would also hit
  integration tests, examples and benches. New crates start with the attribute.

## Non-negotiables
- **Clean-room.** Never read/disassemble anything inside Adobe app bundles (names/listings only). Never copy Adobe icons, presets, profiles (DCP), lens profiles (LCP), camera matrices, fonts. Observation of the installed Lightroom is read-only (it syncs the user's personal library: never import/edit/rate/delete there). Never copy GPL/LGPL/AGPL code (darktable, RawTherapee, ART, LibRaw, rawspeed, rawloader, rawler, lensfun, dcraw-derived GPL code…).
- **Pure Rust** in the product. No C/C++ dependencies.
- **Layering** (`plan/architecture.md` §3, enforced by `cargo xtask layers`): nothing below L5 depends on egui/eframe/winit/rfd.
- **Everything is a command** (`crates/engine`): id, label, menu path, shortcut, params, enabled(), run(). UI, CLI, control channel and MCP all dispatch by id. Every slider is a `develop` control spec.
- **Resolution independence:** settings use normalized image coordinates and relative radii; previews and exports must match.
- **Quality gates** before every commit: `cargo xtask ci` (fmt, clippy -D warnings, tests, layers, assets, wasm).
- **Commits:** one task id per commit (`M2.3: local Laplacian highlights/shadows`). Only green states. End messages with the attribution line required by the environment.

## Assets: icons, images, fonts (ABSOLUTE RULE — never violate)
- **Never use any iconography, image, artwork, font, sound or other asset from Adobe products** (no Lightroom/Creative Cloud icons, no screenshots, no presets/profiles/LUTs, no UI bitmaps — not even as a temporary placeholder or "reference copy"). Observing Adobe's UI to imitate *layout and behaviour* is allowed; copying or tracing its assets is not.
- **This includes Adobe's open-licensed assets**: no Source Sans/Serif/Code or Source Han fonts, no Adobe Fonts, no
  Adobe-published icon sets, sample photos, colour profiles or LUTs — even when OFL/MIT. The UI font is Inter (OFL);
  Japanese fonts come from craft-fonts (below).
- Every asset in the repository must be one of: **our own original work** (e.g. icons drawn in code as vectors, procedurally generated demo photos), **public domain / CC0**, **Creative Commons** (CC-BY / CC-BY-SA with attribution honoured), **OFL** (fonts), or **permissive open-source** (MIT/Apache-2.0/BSD/ISC) — or contributed by a person who created the asset and licenses it openly.
- **Exception: `docs/brand/`.** The ArtCraft name, wordmark and logos there are ArtCraft Team trademarks, not open source and not covered by LightCraft's MIT OR Apache-2.0 licence (`LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`); their terms are in `docs/brand/LICENSE-brand.txt`. Use them only unmodified and never redraw, recolour or derive from them.
- **Every asset must have an entry in `assets/ATTRIBUTION.md`** (path, title, author/creator, source URL or "original work", licence, date added, modifications) and its licence text when required (e.g. `assets/fonts/OFL-*.txt`). Add the entry in the same commit as the asset. Assets without an attribution entry must not be committed.
- **Fonts live in [storytold/craft-fonts](https://github.com/storytold/craft-fonts), never in this repo.** Don't commit
  font files here (Inter, already in `assets/fonts/`, is the one exception); add new fonts to craft-fonts. LightCraft
  uses it as the optional build input `CRAFT_FONTS_DIR`: `git clone https://github.com/storytold/craft-fonts ../craft-fonts`
  then `CRAFT_FONTS_DIR=../craft-fonts cargo run -p lightcraft` (or any cargo/xtask command). `crates/engine/build.rs`
  embeds the manifest's fonts as `lightcraft_engine::CRAFT_FONTS` (wasm32: BIZ UDPGothic Regular only); the UI
  (`theme::font_definitions`) and the export watermark renderer use its Japanese faces as fallbacks after Inter. Unset,
  `CRAFT_FONTS` is empty: everything builds, tests and runs, but Japanese text has no glyphs. Releases always build
  with it (`release.yml`, `CRAFT_FONTS_REQUIRED=1`) and ship the fonts' OFL licences. Tests that need these fonts skip
  without it; the FreeBSD CI job runs them with it. Rules: craftrules
  [`standards/fonts.md`](https://github.com/storytold/craftrules/blob/main/standards/fonts.md).
- Icons drawn in code (e.g. `crates/ui-egui/src/icons.rs`) are original work and are recorded in `assets/ATTRIBUTION.md` as such; do not trace them from Adobe icons.
- Demo/test images: generated procedurally by `lightcraft-scenes`, or CC0 downloads kept in the gitignored `corpus/` with their source recorded. Screenshots of Adobe apps live only in the gitignored `plan/` and are never committed or published.
- **Enforced:** `cargo xtask assets` (in `ci`) fails when an image/icon/font/sound/video/raw/ICC/XMP file is not matched
  by a path pattern in the first column of `assets/ATTRIBUTION.md`, when a referenced licence file is missing, when a
  file name suggests Adobe material, or when an Adobe profile/template format (`.dcp`, `.lcp`, `.lrtemplate`, …) appears.
  Never weaken this check to make a commit pass — fix the attribution or remove the asset.
- When in doubt about an asset's licence: don't use it.

## Running and looking at the app
- `cargo run --release -p lightcraft -- --control 7980` opens the desktop app with the JSON-lines control server (see `docs/control-protocol.md`).
- **Never send OS-level synthetic input** (osascript/System Events keystrokes or clicks, `cliclick`, accessibility
  automation): it goes to whatever window is frontmost — the user's terminal or other apps. Drive LightCraft only
  through its control channel (`ui.key`, `ui.pointer`, `ui.clickWidget`, `ui.menu.invoke`) or headless snapshots.
- For UI work, **look at the result**: drive via the control channel and take `ui.screenshot`, compare with `plan/lightroom/screenshots/`.
- **Unattended (display asleep/locked, CI, nightly runs): prefer headless snapshots** — no window needed:
  `lightcraft-cli snapshot --demo --script tour.jsonl -o out.png --size 1600x1000` (control-protocol requests,
  one per line; see `docs/control-protocol.md` → Headless rendering). In a running app use
  `ui.screenshot {"path": …, "headless": true}`; windowed screenshots fall back to headless after 2 s.
- MCP: `lightcraft-cli mcp` (headless, `--demo` for the procedural library) or `lightcraft-cli mcp --connect` (drives
  the running app). See `docs/mcp.md`. Quick non-UI checks: `lightcraft-cli render in.jpg -o out.jpg --set light.exposure=1`.
- Export goes through `lightcraft_engine::export` (one encoder for app, CLI, MCP and web); UI-only commands live in
  `crates/ui-egui/src/menus.rs`.
- Shell gotcha: `mv`/`cp` are aliased interactive here — use `/bin/mv -f` / `/bin/cp -f`.
- Parallel agents: separate git worktrees and `CARGO_TARGET_DIR=target/agent-<name>`; each agent uses its **own control port** (pick one in 18000–19999, never the default 7980) and its own scratch subfolder (`<scratch>/<agent-name>/`) — never `rm -rf` shared paths; delete your target dir when done (disk is shared); keep every `Cargo.toml` valid at all times (the `crates/*` glob means one broken manifest breaks everyone).
- Test corpora: `cargo xtask corpus --download` into `corpus/` (gitignored, CC0 only). Never commit media.
- Shared real-file test corpora (Photoshop-authored PSDs, etc.) live in [`storytold/photocraft-corpus`](https://github.com/storytold/photocraft-corpus), explained in [craftrules `standards/test-corpora.md`](https://github.com/storytold/craftrules/blob/main/standards/test-corpora.md). Never commit large binary fixtures to this repo; fetch them pinned by commit and sha256-verified, as PhotoCraft does with `cargo xtask corpus`.

## Testing & performance (do this often)
- Unit/property tests next to the code; end-to-end tests drive real binaries (`crates/mcp/tests/e2e.rs`,
  `apps/lightcraft-cli/tests/`). New features need at least one test that would fail without them.
- **Run the app after every user-visible change**: launch with `--control 7980`, drive it with a JSON-lines script
  (`docs/showcase/run.py file.jsonl`), take `ui.screenshot`, and look at it. Check `ui.inspect` → `perf`.
- **Benchmarks:** `cargo xtask bench` (24 MP raw from corpus; CPU and GPU columns) appends to `target/bench/history.jsonl`
  and flags CPU-time regressions > 20 % vs the previous run (`--strict` to fail). Run it before and after perf work.
- **Measure, don't guess**: `LIGHTCRAFT_PROFILE=1` prints per-stage pipeline timings to stderr; time CLI renders
  with `/usr/bin/time`. Record numbers in `plan/STATUS.md` → Metrics. Budgets: slider update ≤ 16 ms (draft) / loupe
  ≤ 60 ms on ~2.5 MP; export ≤ 1 s per 24 MP JPEG.
- Never commit media: test images are procedural (`lightcraft-scenes`) or CC0 downloads in the gitignored `corpus/`.

## Map of the code
`geom`, `color`, `raster`, `tiff` (L0) → `raw`, `codecs`, `meta`, `develop` (L1) → `pipeline` → `catalog` → `engine`
→ `ui-egui`, `mcp` (L5) → apps `lightcraft` (desktop), `lightcraft-cli` (render/commands/MCP). `scenes` generates demo
photos. `xtask` = tooling (`ci`, `layers`, `assets`, `parity`, `wasm`, `corpus`, `stats`).
