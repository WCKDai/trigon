# Drawing display and pan/zoom profiling

Studio retains exact engine vector geometry for picking, snapping and handle projection. Display
pixels are a current-generation cache, transformed immediately during pan/zoom. A pending raster
for a different pan position must not suspend those interactions. A changed document still waits
for its own generation before interaction. Completed obsolete/superseded raster images are dropped
before `tex.set`; only the latest matching image can cause a texture upload.

Raster regions include bounded overscan on a 128-pixel grid. Resolution uses upward 1/8-octave
buckets, normally at or above screen resolution, so small pointer increments share a cached raster.
Each side remains capped at 4096 pixels; very large Retina viewports can use a lower display
resolution to respect that memory budget. View snapshots rasterize the exact viewport separately.
Model geometry, model-to-screen transforms, picking tolerance and export paper weights do not
use the display cache key. The raster worker coalesces requests; the UI coalesces completed images.

## Repeatable CPU profile

Build with an isolated target, then let the build finish before measuring:

```sh
CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=1 cargo build --release --locked -p trigon-studio --example drawing-pan-profile
./target/release/examples/drawing-pan-profile 1 > out/pan-profile.jsonl
```

The optional number is device pixels per window point (0.5–4, default 1). The deterministic
synthetic fixture is 24 × 16 m, five storeys, 160 windows. The profile builds a plan and elevation,
warms the renderer, then measures five repetitions of 96 pan increments (0.5/0.25 window points)
and 96 zoom increments (×1.005), in a 1000 × 700 point viewport. Output includes parse time,
allocation/render/conversion times, raster demands, readiness stalls and candidate image bytes.
It calls the same region-key/readiness functions and CPU rasterizer as the live drawing view.
It uses the preset display trees; it does not measure the newer adaptive stroke parser/cache.
It opens no app, uses no GPU, reads no credentials and requires no network or private model.

Before the cache change, this workload demanded 96 rasters and exact-key readiness stalled on
all 96 ticks. The cache version demanded 1 raster for pan and 8 for zoom, with one initial
readiness wait. In five warm release repetitions on a Mac15,6 (36 GiB, macOS arm64, Rust 1.99.1),
median serialized CPU work for plan pan was 288 → 6 ms, elevation pan 2465 → 27 ms. Elevation
rendering itself dominated the baseline (about 25–32 ms/raster), while conversion was about
0.3 ms. Overscan increases an individual image allocation; the reduced work is due to cache
reuse, not a faster geometry kernel or a different renderer. Uncontrolled native GPU activity,
scheduling and memory pressure can affect timings. Do not benchmark during concurrent builds.

These counters and timings describe a serial CPU workload. They are not native FPS, texture
upload timing, GPU performance, or end-to-end model rebuild measurements. Native/GPU QA must
measure frame pacing and actual upload costs separately. The profile is intended for repeated
before/after runs on the same machine and input, not a cross-machine speed ranking.

`ui.view` exposes `drawing_raster` counters and last allocation/render/conversion/texture-enqueue
CPU timings for native profiling. The texture enqueue timer measures the UI call, not GPU upload
completion; `gpu_upload_measured` is explicitly false.

## Adaptive screen detail

The model worker prepares the exact SVG tree and retains full, reduced and overview SVG sources
once per drawing generation. Zoom buckets select a source; the raster worker adapts its strokes
and parses it away from the UI thread. Pan never reparses SVG or rebuilds model geometry.
Below roughly 80% sheet zoom, display pens become finer and masonry hatch becomes a pale solid
fill. Fine glazing/stair/railing lines shorter than 0.75 screen points are omitted; dense parallel
fine detail is sampled on sheet-anchored cells. Cut walls, major wall/roof/floor silhouettes and
manual annotations are retained. Overview below roughly 40% uses a coarser sampling cell.
Full detail returns on zoom-in. The detail band follows window zoom independently of Retina DPI.

These are display styles. Export SVG/PNG/PDF/DXF retain the selected paper pens and full detail.
The exact vector scene still supplies every pick, snap and handle; no semantic ID is deleted by
screen LOD. Each band is deterministic and pan-invariant, and shares the bounded raster cache.

## Zoom-aware screen pens

Explicit model and manual path strokes use logical window zoom, independently of Retina density
and the raster resolution cap. At 1× their source SVG is byte-identical. Close views increase
geometry spacing while pen widths approach separate bounded limits, retaining fine/medium/cut
order. For example, the actual 0.35 mm plan-cut pen approaches about 2.25 logical points at 8×,
instead of growing to about 13.23 points. This is a display policy, not a change in paper scale.
Below 1×, a small per-pen floor keeps useful lines visible alongside the existing detail bands.

Dash lengths, gaps and offsets follow the pen's display factor, including round zero-length red
dots. Text, text halos, pattern definitions, fill-coloured seam covers, coordinates, clipping,
colours and opacity remain unchanged. CSS-styled or inherited strokes are left to the SVG parser.
The adapter accepts bounded Studio-generated SVG, not arbitrary imported markup.

Stroke zoom uses 1/8-octave buckets. Each raster worker caches one adapted tree; pan, pixel density
and small zoom steps inside its bucket reuse it. Crossing a bucket or detail band prepares another
tree. Returning to an evicted bucket can reparse. Completed raster delivery has one bounded slot,
and dropped or superseded work is retried for the latest view. A previous image stays movable
while new styling is pending; picking continues against exact current-generation vectors.

Viewport and window screenshot requests wait for the current source and stroke bucket. The
viewport snapshot reuses the worker's styled tree and renders the exact current viewport.
`drawing_raster` diagnostics expose `logical_zoom`, `stroke_zoom_bucket`,
`shown_stroke_zoom_bucket`, `last_style_ms`, and `last_style_cache_hit`; these are CPU/display
feedback, not GPU timing or a model accuracy claim. The new preparation cost needs native
measurement; the historical CPU profile above does not establish an adaptive-rendering speedup.
