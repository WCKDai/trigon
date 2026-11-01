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
