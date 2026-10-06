# GPU develop pipeline (M5.2)

The CPU pipeline (`crates/pipeline`) is the reference ("oracle"). `lightcraft-gpu` (`crates/gpu`, L3)
evaluates the same stages with wgpu compute shaders (WGSL) on Metal / Vulkan / DX12. Everything is
pure Rust (wgpu, naga); the drivers are the system's. No GL backend is compiled in.

## Backends, environment variables and troubleshooting (issue #136)
wgpu loads the driver of **every** backend in an instance's set while it enumerates adapters — even
when it then picks another one. A Vulkan driver that crashes there (issue #136: an access violation
in Intel's `igvk64.dll` on a UHD 630 under Windows 11) takes the process down before any window
appears, and a native crash cannot be caught. So LightCraft only lets wgpu touch the backends it
means to use (`lightcraft_gpu::backend`), for its compute device *and* for the desktop window
(eframe/egui-wgpu, `window_wgpu_options` in `apps/lightcraft/src/main.rs`):

| platform | default (window and compute) |
|---|---|
| Windows | DX12 only — the Vulkan driver is never loaded unless asked for |
| macOS | Metal |
| Linux / BSD | Vulkan (the window also lists GL, eframe's default; no GL backend is compiled in) |

Overrides (read once at launch):
- `LIGHTCRAFT_GPU_BACKEND=dx12 | vulkan | metal | gl | auto | off` (comma lists allowed, e.g.
  `vulkan,dx12`): the backends for both the window and GPU rendering; `off` turns GPU rendering off
  (CPU pipeline) — the window still needs a backend and keeps the platform default. A backend this
  build doesn't contain (`gl`) is ignored with a warning.
- else `WGPU_BACKEND` (wgpu's own variable, same names) — before #136 only the window honoured it,
  the compute device always used Vulkan + DX12 (+ Metal).
- `LIGHTCRAFT_GPU=0`: GPU rendering off for the process (the window is unaffected).

**Off the startup path.** The compute device is created on a background thread once the window is
up (`gpu::warm_up` from the first frame's settings), and never while GPU rendering is off: with
Settings ▸ Performance ▸ *Use the GPU for rendering* unchecked (applied before the window opens),
`LIGHTCRAFT_GPU=0` or `LIGHTCRAFT_GPU_BACKEND=off`, no GPU driver is loaded for rendering at all.

**Crash sentinel.** The desktop app writes `gpu-init.marker` into its settings folder (next to
`ui.json`: `%APPDATA%\LightCraft`, `~/Library/Application Support/LightCraft`,
`~/.config/lightcraft`) just before the compute device is created and removes it as soon as creation
returns, successfully or not. If the marker is still there at the next launch, the process died inside
the driver: LightCraft starts with GPU rendering off (the preference is saved unchecked), removes the
marker and says so in a notice. Checking *Use the GPU for rendering* again tries the GPU once more
(and re-arms the sentinel). Not with `LIGHTCRAFT_NO_PREFS` (tests, scripts). Killing the app during
the ~0.3 s of device creation, or two instances starting at the same moment, can trip it falsely —
harmless: rendering is then on the CPU until the box is checked again. The sentinel only covers the
compute device; a crash while the window's renderer starts is avoided by the backend defaults above
or worked around with `LIGHTCRAFT_GPU_BACKEND`.

**Troubleshooting a crash at startup (Windows).** Start LightCraft from a `.cmd` file or a terminal
with `set LIGHTCRAFT_GPU_BACKEND=dx12` (the default since #136), or `=off` to keep the GPU out of
rendering; `set VK_LOADER_DRIVERS_DISABLE=*igvk64*` (Vulkan loader) hides a specific Vulkan driver
from every program started with it. Help ▸ System Info and `app.gpu` show the adapter and backend in
use (e.g. `Intel(R) UHD Graphics 630 (Dx12)`).

## Where it is used
- `lightcraft_engine::media::develop` (called by every `RenderJob`): loupe / before / compare views,
  `render_now` (CLI, MCP, control channel renders) and exports render on the GPU when one is
  available; grid/filmstrip thumbnails (many small jobs in parallel) stay on the CPU.
- Anything the GPU path cannot do returns `None` and the CPU renders instead: no adapter (CI
  machines, software-only adapters), `LIGHTCRAFT_GPU=0` or `LIGHTCRAFT_GPU_BACKEND=off` (whole
  process; see [Backends](#backends-environment-variables-and-troubleshooting-issue-136)), the `app.gpu {enabled}`
  command (runtime preference; `ui.inspect` → `perf.gpu` shows the adapter), a buffer larger than
  the device's storage-buffer limit, or a render the device did not complete correctly (see
  [Limits, failures and the CPU fallback](#limits-failures-and-the-cpu-fallback)).
- wasm32: the crate compiles to the CPU fallback (WebGPU needs asynchronous device creation and a
  device shared with the canvas — open item below).

## Structure
- Shared parameters, so the two implementations cannot drift apart: `pipeline::plan` (effective
  settings, frame, output size, stage-cache keys), `Frame::sample_plan`, `local::{wb_matrix_for,
  nr_params, plane_sigmas, guided_fast_step, airlight_of}`, `finish::{FinishParams, mask_terms}`,
  `masks::brush_dabs`; exact tables (tone LUT, sRGB LUT, curve LUTs, resample taps) and the OkLab
  matrices are uploaded / generated into the WGSL prelude from the CPU values.
- Buffers, not textures: images are `array<f32>` with the CPU's interleaved layout (RGB, 1–3
  channels), so upload / readback are plain copies of `Rgb32f` / `Plane` / `Rgba8` data.
- Kernels (`crates/gpu/src/wgsl/`): `geom` (orientation pixel map, bilinear through the
  crop/straighten/flip affine or the full lens + perspective warp), `resize` (separable resample
  with the CPU's taps; Mitchell prefilter, box/bilinear for the fast guided filter), `blur` (box
  passes with running sums over pixel chunks; three each way = the CPU's Gaussian), `map`
  (log luminance, dark channel, guided-filter steps, white balance, luminance / colour NR, airlight
  sampling), `mask` (linear / radial / luminance / colour range / brush shapes, combine, finalize),
  `finish` (the whole per-pixel stage) — each mirrors a named CPU function.
- Per-stage hybrid: defringe and spot removal (rare, CPU-only for now) download the resampled image,
  run on the CPU and upload; heuristic mask shapes (Sky, Subject, Background, …) and brushes of more
  than 4096 dabs are evaluated on the CPU and uploaded; sources over the buffer limit are resampled
  on the CPU.
- Stage cache: `GpuStages` mirrors `StageCache` (same keys: source identity + `geo`, `lin_key`,
  per-plane radii) and lives in it as an extension, so the UI needs no change. The uploaded source is
  kept per view. A tone / colour / exposure drag re-runs one dispatch of `finish` plus the readback.
- One command encoder per render; the only mid-render readback is the airlight samples (dehaze, when
  the dark channel is recomputed). Dropped buffers are recycled (exact size, ≤ 2 GB pool) once the
  thread that dropped them has submitted — allocating and zero-filling fresh 100–300 MB buffers
  per pass cost as much as the passes.
- `LIGHTCRAFT_PROFILE=1` prints GPU stage timings (each stage is then submitted and waited for).

## Limits, failures and the CPU fallback
Issue #78: on an Intel UHD (ICL GT1) iGPU with Mesa/Vulkan, exports from the desktop app came out
as valid JPEGs that were entirely black, while previews, `lightcraft-cli render` and
`LIGHTCRAFT_GPU=0` were fine — and the GPU export took ~15 s against ~3 s on the CPU. The readback
returned a buffer the per-pixel stage had never written, with no error. The likeliest cause: the
whole export was recorded as one command submission lasting seconds on that GPU, and the driver
reset it (GPU hang check / preemption timeout — the app's own window keeps the GPU busy, the CLI
doesn't) without the error reaching wgpu. wgpu also reports a lost device only through the
device-lost callback, which was not set. So a GPU render now refuses to hand back anything it
cannot vouch for:

- **Short submissions.** A render submits after every stage and every 16 M kernel invocations
  (`FLUSH_INVOCATIONS`); the per-pixel stage runs in bands of ~4 MP. Every submission on a slow
  iGPU stays short enough to be preempted and never trips a watchdog (Windows TDR ~2 s, i915).
  Buffers released by earlier stages are reused by later ones, lowering peak device memory.
- **Error scopes.** Each render runs inside out-of-memory, validation and internal error scopes
  (thread-local in wgpu): errors fail that render instead of disappearing into the uncaptured
  handler. The device-lost callback and the uncaptured handler (other threads) stop GPU use.
- **Readback.** The wait has a timeout (60 s); a failed wait, map error or missing map callback
  fails the render (before, a lost device left the readback waiting forever).
- **Limits.** The device is created with the adapter's limits; every buffer is checked against
  `min(max_storage_buffer_binding_size, max_buffer_size)` when it is created. A render with a
  buffer over the limit (24 MP RGB f32 = 275 MiB; a 128 MiB WebGPU-default device; many masks)
  falls back for that render, without a device error. Dispatches stay under 65535 workgroups per
  dimension (`groups1` folds 1-D kernels onto two axes). Exports over the limit are not tiled:
  most stages need neighbourhoods tens to hundreds of pixels wide (guided filters at clarity /
  base radius, dehaze dark channel) and some need global values (airlight percentile), so tiles
  would need wide overlaps and a two-pass plan — the CPU renders those exports instead.
- **Sanity checks on the result.** The output buffer is cleared before the per-pixel stage, which
  writes alpha 255 everywhere: any pixel without it means work did not run, and the GPU is not used
  again (`incomplete image`). An entirely black result (n ≥ 4096 pixels) is re-rendered on the CPU
  for that render only — a really black photo costs one extra CPU render, nothing else; there are
  no false positives because the CPU result is what is returned.

| failure | this render | later renders |
|---|---|---|
| buffer over the device limit | CPU | GPU |
| out of device memory | CPU (buffer pool emptied) | GPU |
| entirely black result | CPU | GPU |
| validation / internal error, device lost, readback failure or timeout, incomplete image, panic | CPU | CPU (whole process) |

**Diagnosing.** `ui.inspect` → `perf.gpu` (adapter in use), `perf.gpuReason` (why renders don't use
the GPU, e.g. `"disabled by LIGHTCRAFT_GPU=0"`, `"software adapter (llvmpipe (LLVM 21.1.8, 256 bits))
skipped: …"`, `"stopped after a GPU failure: device lost …"`), `perf.gpuFallback` (the latest render
redone on the CPU and why, e.g. `"6000×4000: the GPU returned an incomplete image (24000000 of
24000000 pixels unwritten); the GPU is not used again"`). The same in `app.gpu` (`reason`,
`lastFallback`), Help ▸ System Info and Settings ▸ Performance. `LIGHTCRAFT_PROFILE=1` prints the
fallback with the stage timings; `RUST_LOG=warn` logs it.
**Reproducing a smaller GPU:** `LIGHTCRAFT_GPU_LIMITS=webgpu` (or `downlevel`) creates the device
with 128 MiB storage bindings / 256 MiB buffers, `LIGHTCRAFT_GPU_LIMITS=<n>` with n MiB / 2n MiB.
Tests inject failures with `lightcraft_gpu::inject_fault` (`crates/gpu/tests/fallback.rs`,
`export_falls_back_to_the_cpu_when_gpu_work_is_lost`).

## Correctness: CPU oracle and equivalence tests
`crates/gpu/tests/equivalence.rs` renders the same settings on both and compares 8-bit sRGB:
bounds **mean |Δ| < 0.5 LSB and max |Δ| ≤ 3 LSB** per channel. Measured (Apple M4 Pro, Metal):
**max 1 LSB, mean ≤ 0.0003 LSB** for every case — 26 settings cases (all tone/colour/effects tools,
3 vignette styles, grain, curves, grading, B&W, WB, NR, crop/straighten/flip, lens + perspective,
three mask sets incl. CPU-evaluated shapes, spots + defringe), all 8 orientations ± crop, embedded
DNG lens data, display-referred sources, draft quality, full-size renders; the 24 MP export in
`render_bench` also differs by max 1 LSB. Cached (slider-drag) GPU renders are bit-identical to
fresh ones. The tests skip (pass with a note) when no adapter exists.

Remaining differences come from f32 vs f64 coordinate math, fast-math transcendental functions on
Metal and running-sum order in the box filters — all far below one 8-bit step.

## Measurements
`render_bench` on `corpus/raw/arw-sony-a7m3-compressed.arw` (24 MP; loupe = 2560 px preview →
1920×1280), min of 7 runs, wall clock, on a heavily shared machine (load average 60–110 on 14 cores,
so CPU numbers are pessimistic; the GPU numbers are less affected):

| scenario | CPU | GPU |
|---|---|---|
| loupe 1920×1280 cold (new photo, incl. upload) | 415 ms | 32 ms |
| loupe draft 1152×768 cold | 169 ms | 21 ms |
| exposure drag 1920×1280 (warm) | 22–42 ms | 3.9 ms |
| highlights drag, draft (warm) | 14 ms | 3.2 ms |
| clarity drag, draft (warm) | 16–23 ms | 3.0 ms |
| NR drag, draft / 1920×1280 (warm) | 186 / 301–471 ms | 10 / 18 ms |
| per-pixel stage 1920×1280, + vibrance/saturation | 53–65 ms | 5 ms |
| export render 6000×4000 | 1037–1414 ms | 291–330 ms |

GPU timings include the readback of the 8-bit result and the histogram. Device creation + kernel
compilation: ~0.4 s once per process — the desktop app starts it on a background thread at launch
(`lightcraft_gpu::warm_up`), so the first loupe render doesn't wait for it; other processes create
the device on their first GPU render. `lightcraft_gpu::ready()` asks without blocking.

## Opening a photo (M5.4)
- The loupe shows a stand-in at once (`media::QuickJob`): the photo's cached view render for its
  current settings, else (raws with their import look) the embedded camera JPEG, else a
  thumbnail-level render; the full render replaces it in place.
- Raw sources for previews (2560 px) and thumbnails are binned straight from the mosaic
  (`RawImage::develop_binned`, 2× for a 24 MP preview); only exports / 1:1 demosaic at full size.
- The next and previous photos in filmstrip order are prepared in the background once the current
  one is rendered (decoded source kept, view render cached): stepping through photos shows the
  developed image in ~50 ms.

## Memory (M5.6)
- `library.memory` reports what the engine's caches hold (decoded thumbnail / preview / full-size
  sources, rendered previews) and the GPU renderer's device buffers (allocated, of which pooled
  and retired); `ui.inspect` → `memory` adds the loupe's stage caches (CPU images, GPU buffers)
  and the textures.
- Heap profile: build `lightcraft-cli` with `--features dhat-heap`; `library.memory` then also
  reports live/peak heap bytes and the run writes `dhat-heap.json` (`LIGHTCRAFT_DHAT_FILE`), whose
  allocation sites at the peak (`t-gmax`) show who holds the memory.
- Measuring the scenario (import 14 raws from `corpus/raw`, open the loupe, step 12 times):
  `/usr/bin/time -l lightcraft-cli snapshot <files> --script steps.jsonl -o out.png` → "maximum
  resident set size" and "peak memory footprint". Run it several times: the high-water mark is
  noisy (allocator caching, scheduling). On Apple silicon GPU buffers count in the footprint.
- Imports read raw headers only (`lightcraft_raw::probe_info`): no pixel data is decompressed.
- One budget (`memory::budget`, default min(25 % of RAM, 1.5 GiB); `LIGHTCRAFT_MEMORY_MB` or
  `app.memoryBudget {mb}`): half for the engine caches (decoded thumbnail / preview / full
  sources and rendered previews, evicted least recently used *across* them; the photo on screen
  stays), a quarter for decodes in flight (`memory::work_gate`: grid thumbnails, neighbour
  prefetch and import probes wait for room, loupe and export loads never wait), an eighth for the
  GPU buffer pool (reuse by best fit within 25 %; freed after 3 s idle).
- Decodes free the file's bytes and the raw samples as soon as they are consumed and colour /
  crop / orient in place. macOS keeps freed large blocks resident ("reusable") until the system
  is short of memory, so the resident size climbed with every photo decoded; the apps install
  `malloc_zone_pressure_relief` as `memory::set_release_hook`, called after each decode, import
  and idle trim.
- Result (M4 Pro, 14 raws, loupe, 12 steps): peak RSS 1.9–2.3 GB → 0.83–0.99 GB, peak footprint
  2.5–2.6 GB → 0.91–0.96 GB, heap peak 930 → ~600 MB; stepping median ~13 ms (unchanged).

## Open items
- Keep the loupe texture on the GPU (render into an `egui-wgpu` texture on eframe's device instead of
  reading back); share the device with eframe.
- WebGPU in the browser build (async init, single device per canvas).
- Defringe and spot removal kernels; Sky/Subject heuristics (replaced by the segmenter in M12).
- Tiling for sources / outputs beyond the buffer limit (24 MP+ on WebGPU-default limits, 60 MP+
  on most desktop adapters; these render on the CPU now): tile with overlap equal to the largest
  filter radius, with global values (airlight) from a first pass.
- Integrated GPUs much slower than the CPU (issue #78's GT1): a measured per-device choice of the
  faster path for exports.
- Large-radius blurs at full size (dehaze dark channel at 24 MP) dominate the export: a summed-area
  table or a downsampled dark channel would cut them further.
- GPU histogram (atomics) to skip the CPU pass over the readback.
