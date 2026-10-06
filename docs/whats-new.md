# What's new in LightCraft

## October 2026

### Presets and profiles
- Import presets from other editors: XMP presets, classic `.lrtemplate` files, "DNG presets" from mobile apps and `.zip`
  bundles of any of these — whole folders at once, grouped by pack. Masks inside presets come along.
- Luminar looks: `.lmp` files and `.mplumpack` collections import as presets (grouped by collection); the sliders
  with a counterpart here come along, the rest is listed.
- 23 new built-in presets: Portrait, Landscape, Urban, Food, Seasons, Vintage and B&W toners.

### Reliability
- LightCraft no longer crashes at launch on Windows PCs whose Vulkan driver is broken (issue #136, e.g. some Intel UHD
  630 drivers): on Windows the window and GPU rendering use DirectX 12 only and never load the Vulkan driver unless
  asked to. `LIGHTCRAFT_GPU_BACKEND=dx12 | vulkan | metal | off` (or wgpu's `WGPU_BACKEND`, which GPU rendering
  ignored before) chooses the graphics backend; `off` renders on the CPU. The GPU now starts after the window
  is up, and only when Settings ▸ Performance ▸ Use the GPU for rendering is on; if LightCraft ever dies while
  starting the GPU, the next launch starts with GPU rendering off and says how to turn it back on.
- Exports and renders never write over a photo's original (issue #93): exporting into the photo's own folder with
  the same name and "Overwrite" (or Export with Previous repeating it), an exact output path from the control
  channel or MCP, a merge preview path or `lightcraft-cli render IMG.jpg -o IMG.jpg` is refused with a clear
  message, and the original is left byte for byte. Ordinary earlier exports are still overwritten when asked.
  Exported files are written to a temp file and then renamed into place, so a full disk or an unplugged drive
  never leaves a truncated file; the XMP sidecar of an "Original" export follows the "If file exists" choice too.
- Convert to DNG, Copy as DNG, Photo Merge and smart previews no longer write straight to the final file (issue
  #106): a DNG is checked against the raw data, written to a temp file, synced and read back before it gets its
  name (never replacing a file), and only then is the photo relinked or the raw copy removed — a failed write
  leaves no DNG and keeps the raw. Smart previews are written the same way; a damaged one (cut short by a crash or
  a full drive) no longer counts as built and Build Smart Previews replaces it.
- Import ▸ Copy verifies every copy, like Move (issue #96): each file is written as a new file, synced to disk and
  checked against the content read from the card. A copy that fails or differs is removed and reported as a failed
  import — so "import complete" means the copies are good before you format the card — and a name that is taken
  gets -1, -2… instead of being replaced.
- Faster exports and card imports, with the same protection for what can't be recreated (issue #134): exports,
  renders and screenshots are still written to a temp file and renamed into place, but no longer forced to disk
  one by one — they can always be exported again, and on a USB drive or a NAS that per-file sync dominated a large
  export. The catalog, XMP sidecars, settings, DNGs, merges, smart previews and the copies an import makes are still
  synced. Import ▸ Copy now checks each copy against the content hash taken while scanning the card instead of
  reading the card a third time (Move still compares byte for byte before it deletes a source); a file that changed
  on the card after Review Import is reported instead of being imported with stale details. Checking an export path
  against the library's originals no longer scans the whole library when nothing is at that path.
- Saving metadata to an XMP sidecar another application wrote no longer replaces it (issue #92): LightCraft merges its
  fields in and keeps the rest — e.g. that application's develop settings and edit history — byte for byte. A
  sidecar that isn't valid XMP is copied to `<name>.xmp.bak-<time>` first. With the default stem naming, a raw and a
  JPEG with the same name (`IMG_0001.CR3` + `IMG_0001.JPG`) no longer share one sidecar: the raw keeps `IMG_0001.xmp`,
  the JPEG uses `IMG_0001.JPG.xmp`.
- A save that fails part-way (a full disk, a network share that drops) no longer looks like a damaged catalog
  afterwards (issue #101): the partial write is cut off before LightCraft retries, so the next launch replays every
  change. Catalogs already holding such a fragment load in full. Quitting while the catalog log can't be written
  still saves your queued changes in the closing snapshot.
- The catalog has a format version (issue #102). Opening a library from an older LightCraft upgrades it; a library
  written by a newer LightCraft is refused with "this library was written by a newer version of LightCraft" and left
  untouched — older versions no longer read part of it as a damaged log. Once this version has opened a library,
  LightCraft 0.2.0 and older refuse it ("unsupported format … v2").
- The control port (`--control`) closes a connection as soon as it receives anything that isn't a JSON request
  (issue #94): an HTTP request from a web page can no longer carry a command in its body. Lines are capped at
  4 MiB and connections at 16.
- A sleeping NAS, a dropped network share or a USB drive spinning up no longer freezes the window (issue #104):
  whether originals are there is checked on a worker thread (grid thumbnails, the Info panel, the photo menu, Missing
  Photos); importing (also by drag and drop), Find Missing Photos, Build / Discard Smart Previews, the Rename preview,
  the Local folder tree and Auto Import read the disk on worker threads too. The import progress window has Cancel.
- Browser version (experimental), keeping a library safe (issue #107): File ▸ Back Up Library… downloads the catalog
  and every imported photo as one zip, and File ▸ Restore Library from Backup… brings it back (the current library
  is kept). A failed save (storage full) now shows the unsaved warning and is retried, a photo that can't be stored
  isn't added, a second tab shows a message instead of overwriting the first, `?reset` asks first, and the page says
  when the browser may evict the library. Hosting: the sample cache headers no longer mark the (unhashed) files
  immutable, and HOSTING.md describes the actual build.
- Canon CR2 photos from the EOS 7D, 50D, 60D, 550D, 600D, 1200D, 1300D, 5D Mark II and 1D Mark IV (and other
  models whose sensor starts on a green-blue row) no longer come out magenta (issue #85): the colour-filter
  layout is read from each file instead of assumed.
- Exports are never black because of the GPU (issue #78): a GPU render that runs out of device
  memory, exceeds the GPU's buffer limits, hits a driver error or reset, or comes back
  incomplete is redone on the CPU — the file is the same image either way. Work is sent to the GPU
  in short pieces so slow integrated GPUs aren't reset by their watchdog. `ui.inspect` → `perf`
  (`gpuReason`, `gpuFallback`), Help ▸ System Info and Settings ▸ Performance say why the GPU isn't
  used (e.g. a skipped software adapter such as llvmpipe) and why the last render fell back.
- The thumbnail cache only ever counts and deletes its own files (issue #98): a library opened on a folder that
  already has a `thumbs/` folder of other pictures no longer loses them when the cache is trimmed or cleared.
- Settings files are never quietly reset (issue #103): a damaged `prefs.json`, `presets.json` or `view.json` is kept
  as `<name>.corrupt-<time>` and you're told; one that can't be read (e.g. locked by another program) is left alone
  for the session instead of being overwritten with defaults. The app settings (`ui.json`, which remembers your
  library) are written atomically and saved as soon as you open another library, not only at quit. Quitting while
  changes couldn't be saved tries once more, then asks: Try Saving Again, Quit Anyway or Cancel.

### Library
- Rename Photos never overwrites another photo when only the letter case changes (issue #95): on case-sensitive
  volumes (Linux, case-sensitive APFS) `img_1.JPG` next to `IMG_1.JPG` is a different photo and the renamed one gets
  `img_1-1.JPG`; on case-insensitive volumes the case change still goes through.
- Rename Photos reports files it could not move back after a failure (issue #105), e.g. when a network share drops
  mid-batch: the error lists them (old → new) and the library points at their new names (an undoable partial rename),
  so none shows as missing. Renaming one of a raw + JPEG pair copies their shared `IMG_0001.xmp` instead of taking
  it away from the other (issue #92). Find Missing Photos also finds renamed files by their content, prefers a content match
  over a same-name same-size look-alike, and skips (and reports) photos it can't tell apart instead of guessing.
- Rename Folder… and Move Folder To… (Local) can be undone and redone (issue #97): the folder moves back on disk with
  its photos and XMP sidecars, and the photos point at it again. If something now occupies the old place, the undo
  is refused and nothing is overwritten.
- Smart albums with a rule editor: match all / any / none, nested groups, 26 fields.
- Quick Collection and target album (B in the grid), keyword sets (⌥1–⌥9), colour-label sets.
- Colour-label filter with several labels at once; expandable folder tree in Local.
- Import: copy to any folder, by day / by month / one folder, rename on import, metadata preset, Copy as DNG.
- Import ▸ Move: photos go into the destination (with the same folders and renaming as Copy, e.g.
  `Photos/2026/20260114/20260114_001.jpg`) together with their XMP sidecars; each original leaves the card only after its
  copy is verified and in the library. Duplicates and files that fail stay where they were.
- Watched-folder auto import; Convert to DNG; Duplicate; Build Standard / 1:1 / Smart Previews.
- Export file names use the Rename Photos tokens ({title}, {seq:2}, {date:%Y-%m-%d}…), plus new {num}, {folder}, {lens}, {iso}, {rating}, {creator}.
- Copyright status, rights usage terms and copyright info URL in Info, metadata presets and exports.
- Auto-Tag from Tracklog: GPS locations for your photos from a GPX track log, matched by capture time.
- A change that can't be saved to disk (full or unplugged drive) is no longer silent: the command reports
  "saved in memory but not written to disk", the top bar shows a warning, and LightCraft keeps retrying until the
  save goes through.
- Smaller, faster catalogs: photos you only looked at in Local (never added, rated or edited) are forgotten once
  their folder has not been browsed for 30 days — your files and sidecars stay, and browsing the folder shows them
  again. Change the period (or turn it off) in Settings → Performance.
- A library is open in one program at a time (issue #99): opening a library that LightCraft or `lightcraft-cli` already
  has open — on this computer or another one sharing the folder — says who has it ("already open in LightCraft (process
  123 on studio-mac)") instead of letting both write and silently drop each other's edits. A crash never leaves the
  library locked: the lock is the operating system's and goes away with the program.
- If your library can't be opened at launch (open in another program, unreadable, on a drive that isn't connected,
  written by a newer LightCraft), LightCraft says so and why, and offers Try Again, Choose Another Library…, Continue
  Without Saving and Quit (issue #100). It no longer quietly starts a demo session that looked like a reset library and
  lost everything at quit; a temporary session shows a banner the whole time and never writes to your library.

### Editing
- Auto Sync: edits apply to every selected photo. Auto B&W mix. Automatic versions.
- Colour-range masks: click the photo to sample. Luminance ranges: range bar, smoothness, luminance map.
- ⌘-drag to straighten, ⇧G Guided Upright, a grid while transforming.

### Viewing and sharing
- Slideshow, second window, All Metadata, System Info.
- Edit in External Editor (⇧⌘E): a 16-bit TIFF copy, stacked, refreshed when you come back.
- Lossy DNG files and Smart Previews open as raw photos.
- Compressed Nikon NEFs (lossless and lossy compressed, 12- and 14-bit — e.g. D3200, D5100, D7000, D750, D850, Z 50)
  now develop from the raw data instead of the camera's embedded JPEG, so a B&W or other picture style set in the
  camera no longer gets baked in. Photos already imported as "preview only" switch over on Reload. (Files that
  Nikon splits into two differently compressed halves still use the preview for now.)
