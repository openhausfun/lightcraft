# XMP sidecars and interchange

LightCraft keeps its catalog as the source of truth, and can also store each photo's metadata and develop settings in
an XMP sidecar next to the original. Sidecars let edits travel with the files (backups, another LightCraft library,
other tools), and let LightCraft pick up edits made elsewhere.

## Sidecar files

| | |
|---|---|
| Name | `<stem>.xmp` by default (`IMG_0001.CR3` → `IMG_0001.xmp`); `<file>.xmp` (`IMG_0001.CR3.xmp`) with `naming: "full"`. Reading accepts either (preferred first) and `.XMP`. With stem naming, files sharing a stem don't share a sidecar: see *Shared names* below. |
| Write | `photo.saveMetadataToFile {ids?}` (Photo ▸ Save Metadata to File, ⌘S), or automatically after every change with `library.xmpPreferences {autoWrite: true}` (File ▸ Automatically Write Changes into XMP). Slider drags are written once, when the drag ends; undo/redo rewrite the sidecar. An existing sidecar is **merged into, never replaced** (see *Saving into an existing sidecar*). Written atomically (temp file, fsync, rename). The result lists `written`, `merged` and `backups`. |
| Read | On import (`library.import`, the report counts `sidecars`), and `photo.readMetadataFromFile {ids?}` (one undo step). For raw/DNG files without a sidecar, the XMP embedded in the file is used. |
| Preferences | `library.xmpPreferences {autoWrite?, naming?: stem\|full}`, stored in the library's `prefs.json`. |

What we write (standard namespaces, so other tools can read the metadata):

| Field | XMP property |
|---|---|
| Rating 0–5 | `xmp:Rating` |
| Colour label | `xmp:Label` (`Red`, `Yellow`, `Green`, `Blue`, `Purple`) |
| Title / caption / copyright / creator | `dc:title` / `dc:description` / `dc:rights` / `dc:creator` |
| Copyright status | `xmpRights:Marked` (`True` copyrighted, `False` public domain, absent = unknown) |
| Rights usage terms / copyright info URL | `xmpRights:UsageTerms` / `xmpRights:WebStatement` |
| Keywords | `dc:subject` |
| Capture time, GPS | `exif:DateTimeOriginal`, `photoshop:DateCreated`, `exif:GPSLatitude`/`GPSLongitude` |
| Pick / reject flag | `lc:flag` (`pick`, `reject`, `none`) |
| Location | `lc:location` |
| Develop settings | `lc:settings` — our complete `DevelopSettings` as JSON (exact round trip, incl. masks, spots, crop) |

`lc:` is `http://ns.lightcraft.app/lc/1.0/`.

Reading merges into the catalog with the **sidecar winning** for every field it states; fields it doesn't state are
kept. The one exception is the **capture time**: a time embedded in the file (EXIF/IPTC) always wins; when the file
has none, the sidecar's `exif:DateTimeOriginal`, else `photoshop:DateCreated`, else `xmp:CreateDate` (ISO 8601) is
used — on import (it then also files a copied photo in its date folder) and by `photo.readMetadataFromFile` (undoable).
`xmp:Rating="-1"` (the XMP convention for rejected) sets the reject flag. Develop settings come from
`lc:settings` when present (exact); otherwise from the `crs:` fields below (approximate).

## Saving into an existing sidecar

A sidecar may already hold another application's data — e.g. its `crs:` develop settings and `xmpMM:History` — often
the only copy of those edits outside that application's catalog. Saving never drops it:

- LightCraft **owns** the properties in the table above: `xmp:Rating`, `xmp:Label`, `dc:title`, `dc:description`,
  `dc:rights`, `dc:creator`, `dc:subject`, `Iptc4xmpCore:Location`, `Iptc4xmpCore:AltTextAccessibility`,
  `Iptc4xmpCore:ExtDescrAccessibility`, `photoshop:City`/`State`/`Country`, `xmpRights:Marked`/`UsageTerms`/
  `WebStatement` and everything in `lc:`. They are replaced on every save, and removed when LightCraft has no value
  (clearing a title in LightCraft clears it in the file). All of them are read back on import, so the library starts
  from what the sidecar said.
- The **capture time** (`exif:DateTimeOriginal`, `photoshop:DateCreated`) and **GPS** (`exif:GPSLatitude`,
  `exif:GPSLongitude`) are replaced only when LightCraft has a value; otherwise the file's stay.
- **Everything else is kept byte for byte**: other namespaces (`crs:`, `xmpMM:`, `lr:hierarchicalSubject`, unknown
  ones), `xmp:CreatorTool`, comments, the packet wrapper and padding. LightCraft's properties go into one
  `rdf:Description` of their own; owned properties written by another application (in attribute or element form) are
  removed from its description, which otherwise stays as it was.
- A sidecar that can't be read as XMP (not well-formed, not UTF-8, no `rdf:RDF`) is first copied to
  `<name>.xmp.bak-<date><time>` (`-1`, `-2`… if taken — a backup is never overwritten), then replaced. A sidecar that
  can't be read at all (permissions) is left alone and the save fails.

Limits: LightCraft writes keywords to `dc:subject` only, so another application's `lr:hierarchicalSubject` is kept as
it was and may still list keywords removed in LightCraft. LightCraft doesn't write `crs:`: the other application's
develop settings stay as that application left them, next to LightCraft's own (`lc:settings`, which LightCraft reads
first).

## Shared names

With stem naming, `IMG_0001.CR3` and `IMG_0001.JPG` map to the same `IMG_0001.xmp`. When two or more files in the
library share a stem, the stem sidecar belongs to one of them — a raw first, otherwise the first by file name — and
the others write and read `<file>.xmp` (`IMG_0001.JPG.xmp`), falling back to the stem sidecar for reading when they
have none. So saving one photo never overwrites the other's metadata (`Session::sidecar_naming`).

## Reading `crs:` develop fields

Many raw developers store edits as `crs:` properties (`http://ns.adobe.com/camera-raw-settings/1.0/`) in sidecars, in
DNG files and in XMP presets. LightCraft reads the common ones and maps them to its own controls. We implemented this
from the public XMP specification and by observing what each field does; no third-party code or preset files were used.
Our pipeline renders differently, so **values carry over but the look is approximate**.

We read these fields; we never write them. Only fields in the packet are applied: the result is a partial settings
object that gets merged like a preset, so everything else keeps its current or default value. Packets marked
`crs:AlreadyApplied="True"` are skipped, because those pixels already contain the edit. Only process-version 2012+ field
names are read (e.g. `Exposure2012`, not the older `Exposure`).

| `crs:` field(s) | LightCraft control | Notes |
|---|---|---|
| `Exposure2012` | `light.exposure` | EV, 1:1 |
| `Contrast2012`, `Highlights2012`, `Shadows2012`, `Whites2012`, `Blacks2012` | `light.contrast` … `light.blacks` | −100..100, 1:1 |
| `WhiteBalance` | `wb.mode` | `As Shot`, `Auto`, `Daylight`, `Cloudy`, `Shade`, `Tungsten`, `Fluorescent`, `Flash`; other names → custom |
| `Temperature`, `Tint` | `wb.temp`, `wb.tint` | Kelvin / tint for raw files (and presets) |
| `IncrementalTemperature`, `IncrementalTint` | `wb.temp`, `wb.tint` | rendered files: −100..100 on our relative scale (mired shift around 6500 K, same as the Temp slider) |
| `Vibrance`, `Saturation` | `color.vibrance`, `color.saturation` | 1:1 |
| `Texture`, `Clarity2012`, `Dehaze` | `effects.texture`, `effects.clarity`, `effects.dehaze` | 1:1 |
| `HueAdjustment<Band>`, `SaturationAdjustment<Band>`, `LuminanceAdjustment<Band>` | `mixer.<band>.hue/sat/lum` | bands Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta |
| `ConvertToGrayscale` | `treatment` | `True` → B&W |
| `GrayMixer<Band>` | `bw_mix.<band>` | |
| `ParametricShadows`, `ParametricDarks`, `ParametricLights`, `ParametricHighlights` | `curve.shadows/darks/lights/highlights` | |
| `ParametricShadowSplit`, `ParametricMidtoneSplit`, `ParametricHighlightSplit` | `curve.split_shadows/split_mid/split_highlights` | |
| `CurveRefineSaturation` | `curve.refine_saturation` | 0..100 (100 = curve saturation unchanged) |
| `ToneCurvePV2012`, `ToneCurvePV2012Red/Green/Blue` | `curve.master/red/green/blue` | `"x, y"` points in 0..255 → 0..1; a straight 0→255 line = no curve |
| `SplitToningShadowHue/Saturation`, `SplitToningHighlightHue/Saturation` | `grading.shadows/highlights.hue/sat` | |
| `ColorGradeShadowLum`, `ColorGradeHighlightLum` | `grading.shadows/highlights.lum` | |
| `ColorGradeMidtoneHue/Sat/Lum`, `ColorGradeGlobalHue/Sat/Lum` | `grading.midtones/global.*` | |
| `ColorGradeBlending`, `SplitToningBalance` | `grading.blending`, `grading.balance` | |
| `Sharpness`, `SharpenRadius`, `SharpenDetail`, `SharpenEdgeMasking` | `detail.sharpen_*` | |
| `LuminanceSmoothing`, `LuminanceNoiseReductionDetail`, `LuminanceNoiseReductionContrast` | `detail.nr_luminance/nr_detail/nr_contrast` | |
| `ColorNoiseReduction`, `ColorNoiseReductionDetail`, `ColorNoiseReductionSmoothness` | `detail.nr_color/nr_color_detail/nr_color_smoothness` | |
| `PostCropVignetteAmount/Midpoint/Roundness/Feather/HighlightContrast` | `vignette.amount/midpoint/roundness/feather/highlights` | |
| `PostCropVignetteStyle` | `vignette.style` | 1 highlight priority, 2 colour priority, 3 paint overlay |
| `GrainAmount`, `GrainSize`, `GrainFrequency` | `grain.amount`, `grain.size`, `grain.roughness` | |
| `LensProfileEnable`, `AutoLateralCA` | `optics.lens_profile`, `optics.remove_ca` | the switch only; lens profiles are our own |
| `LensManualDistortionAmount`, `VignetteAmount`, `VignetteMidpoint` | `optics.distortion`, `optics.vignetting`, `optics.vignetting_midpoint` | |
| `DefringePurple/GreenAmount/HueLo/HueHi` | `optics.defringe_*` | |
| `ShadowTint`, `RedHue/Saturation`, `GreenHue/Saturation`, `BlueHue/Saturation` | `calibration.shadows_tint`, `calibration.red_hue/red_sat`, … | Calibration panel, 1:1 |
| `PerspectiveVertical/Horizontal/Rotate/Scale/Aspect/X/Y` | `geometry.vertical/horizontal/rotate/scale/aspect/offset_x/offset_y` | |
| `PerspectiveUpright` | `geometry.upright` | 0 off, 1 auto, 2 level, 3 vertical, 4 full, 5 guided |
| `HasCrop`, `CropLeft/Top/Right/Bottom`, `CropAngle` | `crop.geometry` | normalized edges → rect; angle in degrees; `HasCrop="False"` → no crop |

Values pass through our control specs, so anything outside our slider ranges gets clamped.

**Not mapped:** camera profiles and looks (`CameraProfile`, `Look`; we have our own profile set), local adjustments
(masks, gradients, brushes), spot removal, red eye, lens blur, process-version 2010 field names, and AI features.

## Local corrections (masks)

Masks stored as `crs:` structures are read from sidecars, DNG-embedded XMP, XMP presets and `.lrtemplate` files
(`crates/engine/src/crs_masks.rs`). Four containers hold them: `MaskGroupBasedCorrections` (current) and the older
`GradientBasedCorrections`, `CircularGradientBasedCorrections` and `PaintBasedCorrections`. Each correction becomes one
mask: `CorrectionName` → name, `CorrectionAmount` → Amount, inactive corrections are skipped.

| Field | Ours | Notes |
|---|---|---|
| `LocalExposure2012` | `adjust.exposure` | stored as a fraction, ×4 EV |
| `LocalContrast2012`, `LocalHighlights2012`, `LocalShadows2012`, `LocalWhites2012`, `LocalBlacks2012`, `LocalClarity2012`, `LocalTexture`, `LocalDehaze`, `LocalTemperature`, `LocalTint`, `LocalSaturation`, `LocalHue`, `LocalSharpness`, `LocalLuminanceNoise`, `LocalMoire`, `LocalDefringe`, `LocalToningSaturation` | the matching `adjust.*` | stored −1..1, ×100 |
| `LocalToningHue` | `adjust.color_hue` | degrees |
| `Mask/Gradient` (`FullX/Y`, `ZeroX/Y`) | linear gradient | full effect at the Full point |
| `Mask/CircularGradient` (`Top/Left/Bottom/Right`, `Angle`, `Feather`, `Flipped`) | radial gradient | box fractions → long-edge radii (presets assume 3:2; sidecars use the photo's shape); `Flipped` → invert |
| `Mask/Paint` (`Dabs` "d x y", `Radius`, `Flow`, `CenterWeight`, `MaskValue`) | brush | one brush component per correction; `MaskValue` ≤ 0 erases |
| `Mask/Image` `MaskSubType` 1 / 2 | Subject / Sky | other AI selections are reported, not guessed |
| `Mask/RangeMask` `Type` 2 / 3 (`LumRange`, `DepthRange`) | luminance / depth range | lightness converted to our range scale; colour ranges are reported |
| `MaskBlendMode` 0 / 1 / 2, `MaskInverted` | add / subtract / intersect, invert | |

Applying a preset adds its masks to the photo's own (masks the photo already has are not added twice), and the Amount
slider scales them. A sidecar's masks replace the photo's, because a sidecar holds the whole edit. Components we can't
carry over are listed in `preset.import`'s `unmapped` as `Mask: <kind>`.

## Preset files

| | |
|---|---|
| Ours: `.lcpreset` | JSON `{"format": "lightcraft.preset", "version": 1, "presets": [{id, name, group, settings}]}`, where `settings` is a partial develop-settings object (only the groups the preset includes). A file can hold one preset or many, and every preset keeps its group. Import also accepts a bare preset object or an array of them. |
| Export | `preset.export {path, ids?, group?}`: all user presets by default, or the given ids or one group. In the app: File ▸ Export Presets…, Presets panel ▸ ⋯ ▸ Export User Presets…, or right-click a group ▸ Export Group…. |
| Import | `preset.import {paths, group?, dryRun?}`: files or folders (recursive): `.lcpreset`, `.xmp`, classic `.lrtemplate` (a Lua table: `value.settings` holds the same field names as `crs:`), photos that carry their edits in XMP ("DNG presets" from mobile apps; their crop, geometry and custom white balance are left out) and `.zip` bundles of any of these. Presets in a folder (or a folder inside a zip) go to a group named after it, unless the file names its own group. The result lists, per preset, the settings that couldn't be carried over (`unmapped`, e.g. `CameraProfile`, `Look`, local masks), and the app's toast names them. Older (process version 2010) fields — `Exposure`, `Contrast`, `FillLight`, `HighlightRecovery`, `Shadows`, `Brightness`, `Clarity`, `ToneCurve` — are approximated with today's sliders when a preset has no 2012-era fields. Dropping preset files on the window imports them too. In the app: File ▸ Import Presets…, or Presets panel ▸ ⋯ ▸ Import Presets…. A preset that's already there (same name, group and settings) is skipped. If an id clashes, the import gets a fresh `user.*` id, and built-in presets are never replaced. |
| XMP presets | Read with the `crs:` table above, with `crs:Name` as the name (falling back to the file name) and `crs:Group` as the group (falling back to "Imported Presets"). Only the fields the preset sets are included, so applying it leaves everything else alone and the Amount slider scales it like any other preset. We only read XMP presets; we don't write them. |

LightCraft ships no third-party presets. Its built-in presets are its own values (`crates/engine/src/presets.rs`).

## Luminar looks (`.lmp`, `.mplumpack`)

`preset.import` (and drag & drop, File ▸ Import Profiles & Presets…) also reads Luminar looks: an `.lmp` file is an
XML property list of adjustment layers (each with effects and named sliders, mostly on a −100..100 scale); newer looks
may be a bundle folder `Name.lmp/Contents/preset.lmp`. An `.mplumpack` collection is a zip of `.lmp` files whose
`PresetsInfo.plist` names the group (`GroupName`); the pack's file name is the fallback. Looks get their own name
(`Name`) or their file / bundle name. The sliders with a clear counterpart are carried over; everything else (AI tools,
Orton, glow, LUT layers, colour balance…) is listed in `unmapped` as `Tool.Slider` (e.g. `OrtonFilter.Amount`).
Disabled layers are ignored, layer opacity scales the sliders (and fades curves toward a straight line), and layers
with a blend mode other than Normal or with a mask are reported, not applied. Binary property lists are not read.

| Luminar tool.slider | Ours |
|---|---|
| Develop / Light / Exposure `Exposure` (±100) | `light.exposure` (±4 EV — the scale is our reading, not documented) |
| `Contrast`, `Highlights`, `Shadows`, `Whites`, `Blacks` | `light.*` (same scale) |
| `Temperature`, `Tint` (relative) | white balance shift as `crs:IncrementalTemperature` / `IncrementalTint`; values above 1000 as Kelvin |
| `Saturation`, `Vibrance` | `color.saturation`, `color.vibrance` |
| Clarity `Clarity`, Structure / AI Structure `Amount` | `effects.clarity` (summed) |
| Dehaze `Amount` | `effects.dehaze` |
| HSL (`MIPLChannelsEffect`) `h`/`s`/`l` + Red…Magenta | `mixer.<band>.hue / sat / lum` |
| Curves `RGB` / `Red` / `Green` / `Blue` (x, y points in 0..1) | `curve.master / red / green / blue` |
| Vignette `Amount`, `Vignette Size` | `vignette.amount`, `vignette.midpoint` |
| Grain `Amount` | `grain.amount` |

## Profiles: 3D LUTs (`.cube`)

`profile.import {paths}` (File ▸ Import Profiles & Presets…, drag & drop) reads `.cube` 3D LUTs — single files, folders
or `.zip` bundles — as creative profiles: they appear in the profile browser under their folder's (or zip's) name and
take the Amount slider (0–200 %) like the built-in looks. The LUT is applied to the finished, display-encoded colour
(trilinear; `DOMAIN_MIN` / `DOMAIN_MAX` honoured; 1D LUTs are not supported); photos with a LUT profile render on the
CPU. A library on disk keeps a copy of each file in its `Profiles/` folder. Adobe's own profile formats (`.dcp`, XMP
camera/creative profiles with embedded tables) are deliberately not read.
