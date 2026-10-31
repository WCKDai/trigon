# Saved drawing views and annotations

`view.define` stores a named drawing frame in the `.trigon` document. `annotation.define`
stores a note in that frame. Studio, the CLI and MCP use the same command schemas. Every
change is atomic, journaled and undoable through `edit.undo` / `edit.redo`.

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
