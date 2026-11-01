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

Large mesh groups also use a spatial tree to find candidate source edges for picking ownership.
Groups below 1,024 source edges retain the linear path. Larger groups query conservative 3D
bounds, then run the unchanged 0.1 mm overlap predicate in original source order. This avoids
comparing every roof-course edge with every triangle edge while retaining exact geometry and
ownership records. Differential tests compare exact strokes, owners, planes and near-cut regions,
the existing exporter-normalized face contours and SVG/DXF bytes with the linear reference,
including joined sources; a separate candidate-count check guards
against returning an entire detailed surface for a local query.

The cheap overlap rejection wrapper carries an inline hint: profiling the final 2D stroke
attribution exposed out-of-line predicate cost after projection changed. Benchmark both paths
on existing plans, facades and sections as well as dense roofs; faster projection alone does
not prove that complete drawing generation improved.

Check a saved detailed facade against a local time budget with:

```sh
python3 tools/check_drawing_latency.py --cli target/release/trigon \
  --doc path/to/local-project.trigon --view detailed-facade \
  --out out/facade-latency --limit-s 30
```

This records command/input/output hashes, elapsed time, completion or timeout and logs. A timeout
is a failed check; use a new output directory for each binary. The source and executable must
remain unchanged. Keep this machine-dependent latency budget separate from frame-pacing claims.

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
