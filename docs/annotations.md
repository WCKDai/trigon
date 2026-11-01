# Saved drawing views and annotations

`view.define` stores a named drawing frame in the `.trigon` document. `annotation.define`
stores a note in that frame. Studio, the CLI and MCP use the same command schemas. Every
change is atomic, journaled and undoable through `edit.undo` / `edit.redo`.

Presentation plans retain the model's stair treads, landing outlines and direction lines, on the
`A-STAIR` layer with 0.18/0.13 mm pens finer than the wall cut. Departure and receiving storeys use
the same source stair, including rounded turns and winders; drawing a plan does not change its
geometry. A pale background replaces the former dotted shaft-only presentation.

```json
{"command":"view.define","params":{"id":"ground","name":"Ground floor","kind":{"view":"plan","level":0}}}
{"command":"annotation.define","params":{"id":"wall-note","view":"ground","size_mm":2.5,"kind":{"annotation":"text","at":[1000,2000],"text":"Existing brickwork"}}}
{"command":"annotation.define","params":{"id":"width","view":"ground","kind":{"annotation":"dimension","from":[1000,1000],"to":[4000,1000],"offset_mm":-500}}}
{"command":"view.export","params":{"id":"ground","path":"out/ground.pdf"}}
```

A saved frame's `kind.view` is `plan`, `facade` or `section`:

- Plan: `{"view":"plan","level":0}`. Coordinates are site-local x/y millimetres.
- Facade: `{"view":"facade","facade":"b1.f1"}`. x runs along the facade from its start;
  y is height above the terrain in millimetres.
- Section: `{"view":"section","from":[-15000,0],"to":[15000,0],"depth_mm":30000,"flip":false}`.
  x runs from the cut line's start towards its end, looking left; y is height above terrain.
  `flip` reverses the line and looks right instead.

The annotation `kind.annotation` is one of:

- `text`: `at`, `text`, optional `rotation_deg` (counter-clockwise, default 0).
- `dimension`: `from`, `to`, `offset_mm`. The aligned measurement is calculated from the two
  points, in millimetres. Positive offset is to the left of from → to. It cannot be overridden
  by a misleading label.
- `leader`: `tip`, `elbow`, `at`, `text`. The arrow points to `tip`; the text is next to `at`.
- `level`: `at`, optional `datum_mm` (default 0) and `label` (default empty). The mark shows
  `(at.y − datum_mm)` in metres, with a signed value and decimal comma. These vertical level
  marks belong to facade and section views.

`size_mm` is the text height (em) in **paper** millimetres (default 2.5). Geometry, annotation
positions and offsets are in drawing millimetres. Text stays the same printed size at any
sheet scale. Annotations use layer `A-ANNO`. SVG, PNG, vector PDF and DXF all use the same
scene. DXF stays at 1:1 in drawing-plane millimetres. `view.export` accepts the same optional
`phase` and `filter` as the existing sheet exports.

Use `view.list` to inspect saved views, their validity and annotation counts. Use
`annotation.list` with an optional `view` to list notes. Defining an existing id replaces it;
removal uses `annotation.remove` or `view.remove` with `{"id":"…"}`. A view can be renamed
while it has annotations. Changing its coordinate frame or deleting it requires removing its
annotations first, so their coordinates cannot silently change meaning. If model edits make a
view's facade or storey reference stale, `view.list` reports the error and the view stays
repairable.

Limits: 256 saved views, 5,000 annotations, ids of 1–80 ASCII letters/digits/dots/underscores/
hyphens, view names up to 200 bytes and single-line annotation text up to 512 UTF-8 bytes.
Coordinates are finite and within ±1,000,000 mm; dimension offsets within ±100,000 mm; paper
text sizes 0.5–12 mm. Zero-length dimensions and first leader segments are refused.

Persistence is additive within `trigon/1`: old files load with empty `views` and `annotations`,
and empty fields are omitted when saved. New files round-trip the annotations and reusable
project templates retain them. Older Trigon versions ignore these new fields and **cannot
preserve annotations when saving**. The original `plan.sheet`, `elevation.sheet` and
`section.sheet` commands keep their existing behavior; use `view.export` to include notes owned
by a particular saved frame. Annotations are drafting geometry, not associative constraints:
a model edit does not move their measured points automatically.

For live integrations, the engine's `drawing_view` and `plan_drawing` APIs return SVG together
with the exact `DrawingTransform` and path segments. `to_sheet` maps drawing mm to the
1600 × 900 sheet, `to_model` is its inverse. Picking must additionally invert the UI's pan and
zoom; it must never infer a scale from the raster image.

## Studio workflow and precise picking

Choose **Document** in the workspace tabs. Create a plan, facade or section frame, or select
an existing one. The **Facade elevations** project browser also opens a real 2D elevation,
creating a saved frame through `view.define` on first use. The 3D and unsaved Plan views remain
available in the toolbar. In a saved drawing, the properties panel lists its annotations and
provides typed Text, Dimension, Leader and Level forms. The document toolbar starts these
annotation tools directly; **Select** returns to selection without adding a note. **Export
drawing** in the app bar opens the current saved drawing's export settings.

**Pick** fills a position from the drawing. Dimension picks progress from From to To; leader
picks progress from arrow tip to bend to text position. Click **Add annotation** or **Update
annotation** to commit the form through the engine. Picking alone does not change the document.
Click a dimension/leader line or a text/level insertion point to select it; annotations are also
selectable from the list. **New annotation** starts an independent note, **Delete annotation**
removes the selected one. Escape cancels a pick. Middle/right drag pans; the wheel zooms around
the cursor; Alt temporarily bypasses snaps. Typed coordinate fields keep full model precision.

Endpoint, midpoint, intersection and grid snaps use visible drawing paths in millimetres.
The nearest candidate within ten window points wins; exact ties prefer endpoints, intersections,
midpoints, then grid points. Intersection work is limited to the 128 nearest segments through
the cursor neighbourhood. The cursor label identifies the snap and shows its exact coordinates.

The same helper is available through `view.snap`:

```json
{"command":"view.snap","params":{"id":"ground","point":[1001,1999],"tolerance_mm":20,"options":{"endpoints":true,"midpoints":true,"intersections":true,"grid_mm":100}}}
```

It is read-only and returns `point` and `snap` (null if none is close). The optional `phase` and
`filter` select the same geometry as `view.export`. The Studio control/MCP view accepts
`{"mode":"drawing","drawing":"ground","fit":true}`. Its returned `drawing_mapping`, when
ready, maps drawing coordinates to window points with `screen_x = origin.x + x * points_per_mm`
and `screen_y = origin.y - y * points_per_mm`. It is updated after pan/zoom and omitted while a
new drawing is being prepared.

Click visible wall, window or door linework in a generated plan, facade or section to select its
model element. Studio highlights the owned strokes in orange and opens its normal Properties.
**Edit elements** exposes the existing Move, Copy and Align tools while retaining the drawing
viewport. Clicking blank paper clears the picked element. Saved annotations have selection
priority in Document mode; starting a Text/Dimension/Leader/Level pick fills annotation coordinates
instead of selecting model geometry. The editing command still decides which element classes
support a change; facade hosts and individual interior door leaves remain inspection targets.
See [the drawing-picking contract](drawing-picking.md) for boundary-only hit testing, exact IDs,
phase visibility and the read-only `view.pick` command.

Saved annotations also support sans/serif/mono fonts, bold/italic faces and filled polygon regions.
See [coordinated authoring](coordinated-authoring.md#drawing-text-and-filled-regions) for styles,
export behaviour and current editing boundaries.

## Manual drafting lines and temporary alignment

`annotation.define` also accepts `{"annotation":"line","from":[1000,1000],"to":[6000,1000],"stroke":{"colour":[184,65,44],"width_mm":0.5,"pattern":"dash_dot"}}` as its `kind`.
Patterns are `solid`, `dashed`, `dotted` and `dash_dot`. Widths are paper mm, 0.05–2.11;
RGB channels are integers 0–255. Zero-length lines and hostile coordinates are refused atomically.
Studio's Line tool picks two points; Select or the annotation list reopens its coordinates,
colour, width and pattern. These are manual marks; they make no BIM/apartment separation claim.
All four exports use the same scene. SVG/PNG/PDF retain RGB; AC1015 DXF approximates colours
with indexed ACI colours and rounds weights to the nearest supported 1/100 mm. The export result
reports those limits. Dash lengths are 2 mm on / 1 mm off; dots repeat at 1 mm, and dash-dot
uses 2 / 1 / dot / 1 mm, scaled by `$LTSCALE` in DXF.

Temporary alignment guides use drawing x/y axes through visible endpoints, midpoints, text
insertions and points already picked, plus infinite lines parallel to drawn segments. Guide offset
is a perpendicular drawing-mm distance on either side. Exact enabled endpoint/intersection/midpoint
snaps take priority over guides, followed by grid fallback. Guide tolerance is ten window points
through the actual pan/zoom transform. Disable Temporary alignment guides or hold Alt to bypass
snaps; typed coordinates remain available. Guides do not create constraints or journal entries.
`view.snap` exposes the same behavior with optional `alignment: {"anchors":[[1000,2000]],"offset_mm":500}`;
at most 512 anchors are accepted. Its result includes drawing-plane `guides`.

## Persistent view templates, paper scale and plan ranges

A saved view optionally owns a reusable `template`. Omitting it, or using
`view.configure {"id":"ground","template":null}`, preserves legacy rendering and fingerprints.
`view.configure` changes the template atomically; `view.apply_template {"source":"ground","targets":["upper","section-a"]}`
copies it to existing saved views while retaining each coordinate frame and annotations. A range
can only be applied to plans. Undo restores the exact prior document. Templates travel with project
files and project templates; they are local, with no account or service requirement.

```json
{"command":"view.configure","params":{"id":"ground","template":{"name":"Plan 100","line_weights":"standard","sizing":{"mode":"fixed","denominator":100,"centre":[6000,4000]},"crop":[-1000,-1000,13000,9000],"range":{"top_mm":2300,"cut_mm":1200,"bottom_mm":0,"depth_mm":-1000}}}}
```

Studio exposes these staged settings under **View template · paper · range**; **Apply view
template** runs the shared command. **Use saved template** copies another view's configuration.
Fine/Standard/Bold profiles multiply generated model pens by 0.75/1/1.4 (bounded 0.05–2.11 mm).
Manual drafting line weights remain as explicitly chosen. Profiles affect all four formats.

Sizing `{"mode":"fit"}` fits generated geometry and manual annotations, reporting its actual
fitted scale. `{"mode":"fixed","denominator":100,"centre":[6000,4000]}` means physical 1:100
on the existing 338.6667 × 190.5 mm sheet: 10,000 model mm prints as 100 paper mm at 100% print size.
It crops instead of silently shrinking. SVG uses 96 CSS px/inch; PDF embeds the physical page size.
PNG is 1600 × 900 at that same intended paper size; image viewers/printers can override print size.
DXF remains model-space 1:1 with paper weights and linetype scale. An optional drawing-mm `crop`
rectangle clips geometry before fitting or fixed paper placement. Viewport clipping is applied to
exported DXF and picking too; text outside a crop is removed by insertion point (glyph outlines at
its edge may extend past the DXF crop). Full paths/fills are clipped, not just hidden in SVG.

Range offsets are mm relative to the first building's selected storey floor; the same horizontal
planes apply to all buildings. They require `depth <= bottom < cut < top`, each within ±100,000 mm.
An explicit range uses model mesh cuts, opaque cut faces and hidden-line projection. The primary
range runs from cut down to bottom; visible geometry below bottom down to depth is dotted and
lighter. Sloped edges split at the exact bottom plane. Uncut objects above cut up to top are dashed;
objects intersecting the cut retain their cut outline instead of a duplicate overhead outline.
Adjacent-storey elements participate when inside the range; objects outside top/depth are clipped.
Floor and room picking retains source IDs. Mesh ranges show actual stair meshes instead of legacy
symbolic tread/door-swing/label conventions. Leaving range unset retains the legacy plan symbols.
