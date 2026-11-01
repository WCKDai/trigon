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
