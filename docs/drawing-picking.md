# Semantic drawing picking

Generated presentation plans and saved plan, facade and section views expose model-element
ownership alongside their existing SVG, transform and snap segments. This is additive, read-only
engine metadata. The coordinated authoring extension (#23, continuing #15) adds filled semantic
areas and further element kinds without changing existing drawing geometry or export bytes.
Source property editing is documented in [coordinated authoring](coordinated-authoring.md).

## Rust API

`trigon_engine::drawing_view` and `trigon_engine::plan_drawing` return `DrawingPreview` with an
`element_segments` and `element_regions` vectors. Each `DrawingElementSegment` has:

- `id: String`: an existing built semantic element ID.
- `kind: DrawingElementKind`: `Opening`, `InteriorWall`, `FacadeWall`, `Room`, `Stair`,
  `Balcony`, `Railing`, `Roof` or `Slab` (snake_case JSON).
- `segment: [[f64; 2]; 2]`: the owned portion of an actual visible stroked drawing path.

`preview.pick_element(point, tolerance_mm)` returns `Option<DrawingElementHit>`. A hit has `id`,
`kind`, the nearest `point` on the owned segment, and `distance_mm`. Filled-area hits return the
query point with zero distance. Picking never changes the document. Each `DrawingElementRegion`
has an `id`, `kind`, even-odd polygon `rings`, and an optional affine `depth_plane`
(`[constant, x_gradient, y_gradient]`). Regions respect logical scene clipping and the sheet crop.
The types are public in the engine's drawing implementation; consumers can use the preview's
fields and inherent method without importing their names.

```rust
let preview = trigon_engine::drawing_view(doc, "ground-plan", phase)?;
let drawing_point = preview.transform.to_model(sheet_point);
let tolerance_mm = tolerance_sheet_px * preview.transform.mm_per_px;
if let Some(hit) = preview.pick_element(drawing_point, tolerance_mm) {
    select_existing_element(&hit.id);
}
```

`sheet_point` means pixels on the original 1600 × 900 sheet, not screen pixels after UI zoom or
pan. Undo screen transforms before using `to_model`. Plan coordinates are site-local XY mm;
facade coordinates are distance along its frame and model Z; section coordinates are distance
from the effective cut start and model Z. A flipped section uses its reversed start and direction.
Use the same phase/filter for the drawing and the semantic model. Refresh the preview after edits.

## IDs and ownership

- Facade wall boundaries and their plinth use `BuiltWall.id`, for example `b.f1`, not its
  per-storey cut-mesh ID `b.f1.L0`.
- Interior walls use `BuiltInterior.id`, including the built storey suffix, for example
  `partition.L0`.
- Windows and doors use `BuiltOpening.fill.id` verbatim, for example `b.f1.L0.B0` or
  `partition.L0.D0`. They do not use facade opening-void IDs ending in `.opening`. No `:fill`
  suffix, random pick ID, UUID, component type ID or annotation ID is synthesized.
- IDs are those used by the current built model and Studio scene, not a new persisted identity
  scheme. Facade placement IDs identify built level/bay slots; model edits can change those slots.

Ownership starts at the primitives/meshes already used to draw each view. Plan polygon ownership
is intersected with the union's stroked boundary, so a removed internal seam is not pickable.
Projected feature edges retain only source-mesh edge spans at the same 3D location, followed by
the existing depth clipping and hidden-line removal. Collinear line merging retains each source's
subspans rather than assigning an entire merged facade to one owner. Section poche ownership
comes from the actual mesh/plane intersections and is intersected with the final union boundary.
The final metadata is intersected with the remaining stroked paths and sheet clip rectangle.
Metadata endpoints alone are canonicalized to 0.00000001 mm (and positive zero), then sorted and
deduplicated. This removes sub-ULP projection/merge ordering noise without altering any exported
path or existing snap segment. Nearest-hit calculations use these canonical metadata endpoints.

Projected collinear boundaries choose their exact line frames in sorted welded-edge order.
Random hash iteration therefore cannot move an exported coordinate across a rounding boundary;
this does not change the existing mesh welding or metadata precision.

The attribution checks accommodate existing mesh welding (0.1 mm in projected 3D provenance,
0.5 mm for chained/unioned drawing boundaries); returned segments follow the drawn path within
the 0.00000001 mm metadata precision. These are not cursor hit tolerances. No model bounding box, image recognition, hidden mesh edge or projected opening void is
used as a boundary target. Filled areas come from the corresponding model plan polygons,
projected faces or mesh cut rings; rooms use their source polygons. An attic above a plan cut
does not replace the selected floor slab.

## Nearest hit and validation

The helper measures Euclidean distance to the finite segment centerline, clamping to its
endpoints. It includes a hit exactly at `tolerance_mm`. It does not expand for paper pen width,
snap to the cursor grid. If no visible owned stroke is within tolerance, it tests the even-odd
filled areas. Holes remain misses. Where areas overlap, the closest orthographic depth wins,
then the smallest area, then kind and lexical ID. Plan areas use equal depth, allowing a room
or stair area to win over the larger floor below it.

Candidates sort by exact floating-point distance, then kind (`Opening`, `InteriorWall`,
`FacadeWall`), then case-sensitive lexical ID, then nearest point X and Y. Kind precedence only
breaks exact distance ties; a nearer host line beats a farther opening line. This total ordering
is independent of segment order and does not use an order-dependent distance epsilon. Recessed
door/window reveal lines belong to the host wall when only that wall line is visible; nearby
visible frame/leaf lines belong to the fill.

Query coordinates must be finite and within ±1,000,000 mm; tolerance must be finite and within
0–100,000 mm, inclusive. Zero requests an exact boundary or an interior-area hit. The Rust helper returns `None` for an
invalid query or a miss. It ignores empty IDs, degenerate segments and non-finite or out-of-range
metadata coordinates (beyond ±100,000,000 mm).

## Read-only command

`view.pick` is registered for the CLI and MCP automatically:

```json
{"id":"ground-plan","point":[3000,3920],"tolerance_mm":5,"phase":"proposal","filter":"complete"}
```

`phase` and `filter` are optional and use existing drawing-command defaults. A hit returns:

```json
{"hit":{"id":"partition.L0","kind":"interior_wall","point":[3000,3925],"distance_mm":5}}
```

A valid miss returns `{"hit":null}`. Invalid numbers, unknown fields, unknown views, invalid
view sources, or invalid phases/filters return command errors. The command is neither mutating
nor journaled and does not alter dirty, undo or redo state.

## Visibility and limits

Picking inherits the drawing generator's phase/filter behavior, rather than independently
projecting elements from another view. Complete views omit demolished openings and include
visible new ones; earlier phases do not expose future placements. Plans retain the generator's
red/dashed demolished symbols under `all`/`demolition`. Facades and sections currently omit
demolished fill meshes even under those filters, and picking omits them too. Existing host
walls and non-phased interior elements follow the generator's existing behavior.

Only facade walls, interior walls, windows and doors are attributed. Roofs, slabs, stairs,
balconies/railings, room labels, generated dimensions, hatching, ground/level marks and saved
annotations are not semantic targets. Annotation picking remains a separate UI concern. Raw
registered archive-image plans are not semantic targets; use generated presentation drawings.
The engine does not add SVG attributes or metadata to exported SVG/DXF/PNG/PDF files.

## Verification and rollback

Run focused evidence with:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo test -p trigon-engine --lib --locked views::picking_tests
CARGO_TARGET_DIR="$PWD/target" cargo test -p trigon-engine --lib --locked sheet::project::tests
```

Tests cover plan/facade/section fills and hosts, tolerant near-lines and misses, exact tie
precedence and input-order independence, invalid queries/geometry, phase visibility, hidden
edges, welded/unioned ownership, section cuts/direction, storeys, clipping, excluded annotations,
and read-only command state. Fixed SVG/DXF fingerprints were captured from baseline `3327d49`
before provenance was attached. Existing snap-segment bytes are compared with the baseline
extraction algorithm on the same scene because its underlying face order is not stable between
separate generations. Registered command tests include `hostile_params_never_panic` and object
schemas. Full workspace formatting, locked Clippy and locked tests remain required before commit.

Recorded implementation evidence (2026-10-31, macOS arm64, Rust/Cargo 1.99.1):

- Before implementation, `CARGO_TARGET_DIR="$PWD/target" cargo test -p trigon-engine --lib --locked`
  passed 77 tests; no pre-existing failure was observed.
- The focused picking command above passed 11 tests; the full engine suite now passes 90 tests.
- `cargo fmt --all` passed.
- `CARGO_TARGET_DIR="$PWD/target" cargo clippy --workspace --all-targets --locked -- -D warnings`
  passed with no warnings.
- `CARGO_TARGET_DIR="$PWD/target" cargo test --workspace --locked` passed all 224 tests, with no
  failures or ignored tests; doc-test targets also passed.
- `git diff --check` passed. Only drawing modules, this document and the approved `worker.rs`
  exhaustive-destructure compatibility fix changed; no dependencies or persisted schemas changed.
- An initial workspace run exposed sub-ULP metadata drift in the new repeated-generation test.
  Canonicalization fixed it; the test now checks eight regenerations per view, and a deterministic
  injected-noise test verifies canonical ownership without rewriting path/snap coordinates.
- The engineering kit was absent. `eng verify`, `eng impact`, `eng evidence verify` and kit security
  scanning were unavailable and were not treated as passing; the kit was not installed.

Studio retains this metadata through its worker and uses the actual drawing transform after pan
and zoom, with a ten-window-point hit tolerance. A hit resolves to the existing 3D scene element,
clears any previous typed target list and displays its Properties. Orange strokes identify the
selected element. In Document mode, saved annotations take selection priority; active annotation
point-picking tools keep their existing behavior. **Edit elements** opens model editing tools in
the current drawing. A blank-paper click clears model selection. The existing editing command
restrictions still apply: selecting a facade host or an individual interior door does not make it
an independently movable target.

Integration checks cover all metadata IDs resolving in Studio's scene across an unsaved plan and
saved plan/facade/section; selecting a visible partition or window, running `edit.move`, and exact
document restoration through `edit.undo`; and a screen-sized tolerance after pan/zoom with stale
geometry cleared on drawing failure. The #23 native pass also exercises Studio pointer events
on AB Knudepunktet: 3D dimension edits regenerate drawings, and 2D elevation edits regenerate
the exported model, with one command and exact Undo. Whole-window and GPU viewport captures
provide visual evidence. These checks use the app control input path; an independent physical
mouse pass and large-model performance benchmarking remain unverified.

## Reproduce the complete local workflow

```sh
cargo run -p trigon-engine --example drawing-workflow --locked -- out/drawing-workflow
cargo run -p trigon-studio --locked -- out/drawing-workflow/sample.trigon
```

The first command builds the existing synthetic sample entirely offline, creates named plan,
facade and section views through the shared commands, and writes each view as SVG, PNG, vector
PDF and DXF. It checks preview/`view.pick` agreement, read-only picking, an actual Move followed
by exact Undo, and a saved local project plus GLB and IFC. Both independent IFC readers must
report no failures, match every model element, and agree with its bounds within 0.01 mm.
`verification.json` records the queries and reader results. Open `sample.trigon` in Studio to
exercise the desktop selection workflow; the example itself does not certify native mouse input.
No vendor/reference assets, account, paid service, external data or AI calls are needed.

Picking metadata needs no document migration. Revert the corresponding feature commit to roll
back the code; see [coordinated authoring](coordinated-authoring.md) for new authoring file variants.
