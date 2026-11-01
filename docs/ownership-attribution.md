# Merged-edge ownership profiling

`sheet::style::edges` merges visible projected lines, then attributes each resulting line to
its original owned spans. Before the spatial lookup, it checked every source span for every
merged line, even when their bounds were disjoint. This affects facade and section generation.
It does not measure SVG upload, native drawing, pan latency or frame rate.

The temporary 2D bounds tree contains each valid source once. A query expands the drawn line's
bounds by `0.5 + 1e-6` mm, which contains the unchanged `overlap` predicate's bounding rejection
at `0.5 + 1e-7` mm, including rounding near its existing ±1e8 mm coordinate limit. Every intersecting
node and leaf is visited, independently of source direction or length. In particular, short
nonparallel sources whose endpoints fit the old distance tolerance remain candidates. The
existing exact predicate still clips and decides ownership.

Candidates are sorted by their original flattened source position, preserving overlapping
owners, duplicate records and deterministic append order. Fewer than 256 source spans use the
original scan. If a query returns more than half the sources, it uses the complete original
order instead of sorting most of the sources. Dense or very long overlapping lines can still
require quadratic exact attribution; a query costs O(N + K log K) in the worst case for K
sorted candidates. Building the balanced median tree takes O(N log N) work and O(N) temporary memory, and has
logarithmic recursion depth. The index is discarded after the drawing is generated.

## Reproduce

Run from the repository root, with checkout-local build output and ordinary checks enabled:

```sh
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=1
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0

cargo test -p trigon-engine --locked sheet::style::

TRIGON_OWNERSHIP_PROFILE="$PWD/out/ownership-profile/debug" \
  cargo test -p trigon-engine --locked \
  public_synthetic_ownership_matches_full_scan_and_profiles -- --nocapture

CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true \
  TRIGON_OWNERSHIP_PROFILE="$PWD/out/ownership-profile/optimized" \
  cargo test -p trigon-engine --release --locked \
  public_synthetic_ownership_matches_full_scan_and_profiles -- --nocapture
```

Without the environment variable, the normal test uses a smaller 3-bay, 2-storey fixture.
With it, the test authors original 4-bay/3-storey and 8-bay/5-storey rectangular buildings
through normal commands. Both front and rear facades contain the installed public Dannebrog,
divided-pane and segmental-arch window families. Their dimensions are illustrative, not surveyed.
The test uses real parametric model meshes and hidden-line projection for an exterior facade
and a section seen from an interior cut plane. The isolated section stage measures projected
visible line styling/ownership. A separate complete sheet comparison uses the actual elevation
and section generators, including section poche, clipping and annotations.

The test retains an independent copy of the original full-scan loop. Each fixture warms both
paths, then runs nine same-binary, same-data pairs in deterministic randomized order. Both paths
include styling, merging, source flattening, exact attribution and scene creation. The paired
timings exclude authoring, model build/projection, exports, equality checks, upload and pan.
Separate timings record authoring, build/projection and the baseline merge/attribution stages.
Test-only source-visit counters update once per path, not once per predicate invocation.
The `full_sheet_generation_samples` array separately records nine randomized paired timings
through the complete elevation/section generators. A test-only thread-local switch selects the
old attribution loop; it is absent from production builds. These pairs include model build,
projection, fills, ownership, clipping, poche and annotation placement. SVG/DXF encoding and
file writes remain outside the timed generation region.

The ignored directory receives:

- `samples.json`: raw nanosecond timings, pair order, source visits, scene sizes and allocated
  index/query buffer capacities. The memory figure excludes shared input, existing flattened
  source clones, scene/export buffers and allocator overhead; it is not process peak RSS.
- Source document JSON and per-view projected coordinates/owner records, suitable for SHA-256
  checks when reproducing the dataset on the same revision.
- Baseline and candidate SVG, DXF and full ownership metadata, asserted byte-identical within
  each comparison.
- `full-sheet/`: the corresponding complete production sheet artifacts, also asserted identical.
- Pick probes at distributed line endpoints/midpoints with 0, 0.5 and 5 mm tolerances. Complete
  ordered scene equality additionally preserves the inputs to all downstream exact picking.

Report each strategy's median, nearest-rank p95 and min–max spread from the raw samples. With
nine samples, nearest-rank p95 is the maximum; this is a coarse tail estimate. Compare paired
ratios as well as separate medians, and report competing-load noise or inconclusive results.
Record the executable and dataset SHA-256, machine, Rust version, profile and all flags. Never
compare a development baseline against an optimized candidate.

## Regression checks and limits

The source-visit regression on the 4-bay/3-storey facade fails before the optimization at
7,269,248 visits, the complete line × source product. It requires fewer than one tenth of
that count after indexing, without using a wall-clock assertion.

Adversarial tests compare the exact old/new ordered ownership records around ±0.5 mm, nearly
parallel/offset lines, tiny and zero-length lines, long/reversed/diagonal lines, spatial
partition boundaries, repeated owners, signed zero, subnormals, extreme finite coordinates,
nonfinite values and empty/all-invalid indexes. Merged adversarial scenes also compare SVG/DXF
bytes and exact picks. Existing merge semantics, coordinates, tolerances, pick priority,
schemas, journals, Undo/Redo, defaults and golden expectations are unchanged.

Raw un-clipped projected face-region rings already vary in cyclic start/winding between
independent projections, including when the original loop is compared with itself. This change
does not normalize or alter those projection records. The isolated stage compares both loops
against exactly the same projection, including every region record. The complete sheet tests
also require exact equality after separate runs of the production generators.

These public synthetic measurements establish the attribution cost and its reduction. They
do not establish native Studio responsiveness, Revit behavior or completion of any building.
Final native/exchange acceptance and the full 360-degree comparison against all source
material remain integration responsibilities.
