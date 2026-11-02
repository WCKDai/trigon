# Coordinated 2D and 3D authoring

Studio's 3D workspace, floor plans, elevations, sections, schedules, CLI and MCP all use one
parametric `Document`. Changing source parameters rebuilds the geometry and drawings. A drawn
line is not a second, disconnected wall, and annotation geometry does not become a building part.
This increment is tracked in WCKDai/trigon#23, continuing #15 and #21.

## In Studio

Open the Model workspace and select an element in 3D or in a plan, elevation or section. The
Properties browser shows its source parameters and edit scope. Change fields and choose
**Apply model properties**, or drag one of the orange dimensional markers. A completed drag
creates one command; Escape cancels it. Selection persists across a successful rebuild, including
when an edit splits a definition shared by several storeys.

Picking covers displayed facade/interior walls, openings, balcony decks and rails, stairs,
roofs, slabs and rooms. Rooms have floor picking proxies in 3D and polygon targets in plans.
Their proxies add no visible surfaces or exported geometry. Drawing boundary strokes keep their
existing nearest-hit precedence; filled areas allow selection away from a boundary. Hidden and
cropped geometry must not become an area target.

Properties include opening dimensions and position; facade corners, classification, finish and
storey thicknesses; interior walls and doors; stair dimensions and outlines; room names and
polygons; cellar stair and door dimensions; roof parameters; and slab/storey parameters. A
derived balcony part labels properties that affect its host assembly. Component-specific changes
create a private type variant instead of silently changing every use of a family.

Scalar drag handles cover the dimensions/positions advertised by each descriptor. Polygon and
outline changes currently use validated JSON coordinate fields. Roofs, rooms and cellar objects
have property editing, but do not yet have point-by-point drag grips. This is an initial authoring
workflow, with further constraint, joining and modelling tools still to build.

Historical, demolished and ambiguous same-ID phase occurrences are read-only. Rebuilds and
failed results invalidate cached properties and handles; a stale drag cannot modify a newer
selection. Geometry validation may reject a value within a field's numeric range, for example
when an enlarged opening overlaps its neighbour. The entire change then leaves the document
unchanged. Undo/Redo uses the same history for both workspaces.

## Shared commands

`element.get` is read-only:

```json
{"id":"b.f1.L0.B0","phase":"proposal","filter":"complete"}
```

Its descriptor contains `id`, `kind`, `building`, `level`, `editable`, `reason`, `status`,
`properties` and `handles`. Each property has `key`, `label`, `value`, `editable`, and optional
`unit`, `min`, `max` or `choices`. Handle positions use site-local XYZ millimetres; `direction`
is a unit world vector, and `gain` converts edge movement to a parameter change (2 for a centred
width). Numeric bounds do not replace model validation.

`element.set` applies a batch atomically, journals it once, and returns the resulting selection
IDs in `targets`:

```json
{"id":"b.f1.L0.B0","properties":{"width_mm":1500,"height_mm":1800,"sill_mm":800}}
```

Optional `phase` and `filter` use existing view defaults. Edits require the active complete view.
Unknown/read-only properties, invalid numbers, unsupported values and invalid geometry are
rejected before the candidate document is assigned. Interior walls, stairs and rooms shared
between storeys split at the selected occurrence; unrelated storeys retain their parameters.
Canonical room IDs are `<source>.space.L<level>`, matching IFC space identity. Existing precise
editing selectors also retain the older `<source>.L<level>` room alias.

The worker builds geometry once and prepares the scene, schedule and property descriptors from
that same result. Drawings use the same document revision and phase. The control channel exposes
the actual projected marker positions through `ui.view`; `ui.drag` delivers checked pointer
events to the window. See [control protocol](control-protocol.md) and [drawing picking](drawing-picking.md).

## Drawing text and filled regions

The Document workspace edits saved annotation text, paper text size, rotation, font family
(sans, serif or mono), bold and italic. Styles persist per annotation and reach SVG, PNG, PDF
and DXF. The SVG/PNG/PDF renderer bundles OFL Liberation faces and embeds actual fonts in PDF.
DXF uses named font styles; external consumers need those free fonts or substitutes. Its existing
ACI colour quantization remains.

Filled regions are saved annotations with a simple, unclosed polygon, RGB colour and optional
border. They are drawn before model graphics so opaque colour does not hide walls or openings.
Their polygon, text and style controls use the same validated, undoable annotation commands.
Regions are view-specific drafting objects, not model rooms or material surfaces.

Old documents retain their existing text and slab defaults. Additional slab thickness parameters
and text styles are omitted when unset/default; a new region annotation is an explicit new file
variant and is not promised to work in older Trigon binaries.

## Verification and boundaries

Regression checks cover source identity with multiple unnamed siblings, per-storey isolation,
candidate atomicity, hostile inputs, 2D/3D rebuilds and exact Undo/Redo. Drawing tests retain the
pre-existing SVG/DXF fingerprints and verify boundary precedence, region holes/depth, logical
and sheet clipping, roof/floor separation, and exact balcony rail identities. Font checks render
distinct faces and inspect PDF embedding. Studio checks exercise cache invalidation and pointer
routing as well as handle projection math.

The local integration pass ran all 293 workspace tests, strict locked workspace Clippy, format
checks, and release builds of the CLI and Studio. Native Studio checks on the private full
building exercised a 3D width drag and a 2D elevation height drag. Each made one source command,
changed the corresponding drawing and GLB, and restored the document and both exports exactly
with one Undo. Window captures verified the markers and Properties. The pass also caught and
fixed overlapping small-scale handle hit boxes; the nearest marker now wins. Independent
physical mouse testing and large-model performance benchmarking remain unverified.

AB Knudepunktet is the private integration project; source photographs, reference PDFs, project
files and review captures stay outside the repository. These checks establish software behaviour,
not a measured survey or acceptance of the building's drawing completeness. Revert this feature
commit to roll back the code; retain a copy of the original project before authoring new variants.
