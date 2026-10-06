# Sony ARW starting look

ARW decoding supplies a Bayer mosaic and camera white-balance multipliers, but currently no measured camera colour matrix. Treating camera RGB as linear sRGB produces a dark, muted default rendering. The ARW loader now estimates a file-local starting look from that file's embedded camera JPEG, only when a usable camera matrix is absent. All output pixels still come from the RAW mosaic; there is no JPEG replacement or uniform saturation boost.

## Colour and tone are separate

A fixed sensor proxy of at most 96×96 is developed with the file's WB, black/white levels and active crop. The embedded JPEG is decoded with DCT scaling and a 64-million-pixel input limit, converted through the codec's colour management, and reduced to the same size. Aspect ratios must agree within 2%. Near-black, clipped and nonfinite samples are excluded. At least 256 paired pixels, 5% coloured samples and a nonsingular input covariance with bounded condition are required.

Two thirds of the usable pixels train a ridge-regularised 3×3 **chromaticity** correction (luminance-normalised RGB). This avoids trying to encode the camera's nonlinear tone curve in a matrix, which left skin grey in the earlier matrix-only estimate. Coefficients are bounded by 8. Luminance is fitted separately: 32 equal-population bins with robust median knots and pooled-adjacent-violators isotonic regression. The curve preserves black, cannot reverse, and extends beyond observed highlights with a bounded shoulder.

The matrix is applied to scene-linear sensor pixels. The curve is carried in `SourceInfo` and applied in the pipeline finish stage, after exposure/WB/local edits. It is never baked into decoded luminance or clipped to JPEG precision. CPU and GPU use the same tone LUT. Contrast, whites and blacks adjust the starting look through the display-domain controls. JPEG neutral rendering and the existing DNG colour model are unchanged.

One third of the pixels are held out from both fits. The combined model must reduce linear-display squared error by at least 30% and have per-channel RMS error ≤0.10 (0.055 rejected half of the public raw.pixls.us samples tried — ILCE-6400, -6700, -7M4 — whose fits were still 1.5–3× closer to the camera JPEG than the fallback and visibly better). These are provisional acceptance gates, not a colour-accuracy certification. Monochrome, singular, unrelated, aspect-mismatched and poor-fitting references retain the documented fallback. Both fits use the same fixed proxy at every decode resolution.

## Source metadata and white balance

Previously `SourceRef::load()` discarded the decoder's `SourceInfo`, and render jobs used header-only facts (including a hard-coded RAW WB). Decoded pixels and source facts now travel and remain cached together, including across web-worker renders. Smart previews keep the tone curve in their header (the matrix is already in their pixels), so an offline original renders and exports with the same look; proxies built before this fall back to the generic curve until rebuilt. Pixel-statistics/colour-sampling commands use decoded facts too. Render cache version 6 invalidates old thumbnails and loupe renders.

Vendor RGB WB multipliers alone do **not** identify an absolute illuminant without camera calibration. The generic model inferred approximately 6829 K / −127 tint for one Sony sample; changing only temperature retained that spurious tint and caused a pink cast. Uncalibrated ARWs now keep the file's vendor WB at decode and expose **relative** WB controls around the as-shot look. 6500/0 is the internal neutral reference of that relative scale, not a claim about the capture's measured Kelvin temperature. Presets, picker/auto and manual controls use this reference consistently. Switching from As Shot to Custom first resolves both controls; changing one cannot inherit stale catalog tint. Reset restores the current as-shot reference. Measured/absolute-Kelvin Sony WB remains future camera-calibration work.

Sony's proprietary `ColorMatrix` is not assumed to be a DNG XYZ-to-camera matrix. No Adobe profile/matrix or GPL decoder/calibration source is used. The product remains pure Rust.

## Validation and scope

Synthetic tests cover colour with a nonlinear camera tone, preserved scene-linear headroom, invalid/singular/unrelated references, monotone black-preserving curves, hostile serialized knots, decode-to-render/cache metadata propagation, old-catalog WB transitions and CPU/GPU tone equivalence. Private photos are never committed or uploaded.

Five local Sony ILCE-6700 ARWs were accepted. Held-out per-channel linear-display RMS values were 0.04244, 0.03349, 0.04552, 0.02871 and 0.03100. All five unedited supplied JPEG exports stayed pixel-identical to the installed pre-change renderer at equal size/quality. Four supplied JPEGs use Display P3 and two have a narrower crop than the RAW, so they are visual references rather than aligned pixel ground truth.

Public CC0 samples (raw.pixls.us; 10 files, ILCE-6000, -6400 ×2, -6700, -7M3, -7M4 ×2, -7RM4, -7SM3, DSC-RX100M3), rendered at 1200 px and compared with each file's embedded JPEG (mean CIE76 ΔE at 200 px): 7 fits accepted, ΔE 17–26 → 3–10 with matching mean lightness. Rejected and unchanged: the RX100M3 (held-out RMS 0.103; its fallback render already has a strong green cast), an ILCE-7RM4 dusk scene whose fit was worse than the fallback (ΔE 7.6), and a reduced-size lossless ARW (no fit attempted). Saturated hues can still differ (an orange-red hull renders yellow).

On one sample, Exposure +1 EV, Saturation +100, Contrast +50, warm/cool WB, tint, highlights and shadows all changed the output. The same matrix and curve were selected for 400-pixel and full-resolution renders. Headless UI checks wait until the loupe reports `source: render` and no jobs remain; the camera JPEG stand-in does not count as verification.

This is a per-file camera-look estimate, **not measured spectral calibration or Lightroom parity**. Camera local tone, picture styles, noise reduction and lens warps cannot all be reproduced by a global matrix/curve; same-aspect crops may evade the aspect check. Skin, mixed-light scenes and saturated highlights can still differ from a supplied JPEG. Broader Sony model/lighting coverage, chart-based calibration, absolute WB and other manufacturers remain unverified. Rejected fits and other RAW formats retain their existing behaviour.
