# Drawing ownership performance

Drawing exports and live views share exact geometry and semantic ownership. The overlap check
first rejects disjoint segment bounds, including the existing perpendicular tolerance and a small
floating-point guard band. Surviving candidates use the unchanged exact collinearity and interval
calculation. This affects neither line geometry nor picking precision. Differential checks cover
reversed/partial segments, near-tolerance offsets, 2D/3D and the coordinate limit.

Before profiling, preserve a release CLI outside the build target. Rebuild the candidate with the
same toolchain/profile/dependencies, finish all builds, then run:

```sh
python3 tools/bench_drawing_exports.py \
  --baseline out/trigon-before --candidate target/release/trigon \
  --doc path/to/local-project.trigon --views plan facade section \
  --out out/drawing-benchmark
```

Use actual saved view IDs. The script opens a fresh CLI process for each export, warms each view
and binary, then interleaves five measured samples in deterministic random order. It records
commands, executable/document hashes, file sizes and metrics. Results include median, empirical
p95, spread and a bootstrap interval for the median ratio. It rejects changed SVG bytes, a changed
source document or replaced executables. It reads no credentials and makes no network requests.
Keep private models and their generated evidence in gitignored output directories.

The measurement includes process startup, model loading/building, projection and file writes.
It does not measure native FPS, GPU upload completion or input latency. Studio's separate
[pan/zoom profile](drawing-display.md) measures cache demand and serialized CPU raster work.
A headless egui pointer-event regression also checks that dragging the canvas changes its centre
without changing zoom. Native interaction and frame pacing still need a visible review window.

Projection chooses boundary-line frames in stable welded-edge order. This avoids hash-seed ULP
variation crossing an SVG rounding boundary. The legacy public SVG/DXF fingerprints remain
unchanged; do not relax byte comparisons when a larger model reveals an ordering defect.

## Studio 3D outlines

Studio resolves collinear unmatched triangle sides as intervals before classifying borders and
creases. This prevents long triangulation edges along aligned window sills from drawing lines
across flat brickwork. Real reveals and sharp folds stay visible; gaps remain empty. The repair
only changes the viewport's outline data, with no change to model triangles, materials, picking
ownership or exported geometry. A coarse candidate key is followed by a strict collinearity
check; uncertain candidates retain their original borders.

The CPU scene profile reuses Studio's scene builder without opening a window:

```sh
cargo run --release --locked -p trigon-studio --example scene-edge-profile
cargo run --release --locked -p trigon-studio --example scene-edge-profile -- path/to/local-project.trigon 9
```

The default is a synthetic 160-window building. It builds the model once, warms the scene builder
once, then records every scene preparation sample, median and p95. A same-toolchain signature
covers face vertices, pick ownership, element identities and fingerprints, excluding the changed
outlines. Freeze baseline and candidate executables, finish other builds and interleave their
runs on the same input before comparing results. Geometry generation, room/site append, GPU
upload and native frame pacing are outside this measurement; a native render remains necessary
to check the visible result.
