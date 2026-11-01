# Precise placement and review

Trigon coordinates are site-local millimetres stored as `f64`. Display rounding is not a
placement grid. Generated plans and 3D geometry use the same source parameters.

## Stair occurrences and entrances

`core.L1` selects one departing stair occurrence; `stair:core` selects the complete source.
Moving, aligning or changing a shared occurrence splits its source and leaves other floors
in place. A single physical entrance and its apartment records stay intact.

Entrance records retain the legacy `stair` default. The optional `stair_by_level` object maps
departure-storey indices to explicit source IDs after an occurrence is edited. Empty maps are
omitted. Plans use the current departure, or the previous departure when that incoming flight
is visible. Thus the highest edited flight also keeps its label on the receiving floor.
Copies do not duplicate entrances. Replacing an occupied destination remaps its association;
deletion prunes dangling associations. Clearing stairs clears associations, not entrances or
units. Adding a basement shifts override keys alongside stair storeys. Loading units again
replaces the entrance records and rebinds them using the register workflow.

Old documents need no migration. Feature-bearing files must be opened in this version or
newer: older readers can ignore the override map and lose individual-floor associations on
save. For downgrade, Undo the occurrence edits and save a separate copy. Do not infer lineage
from generated `.edit` source IDs.

Source association correctness does not establish physical accuracy. Distinct stairs on
adjacent departures may intentionally overlap. A scan-based relocation needs registration,
source evidence and an explicit reviewed correction; an apparent double symbol is not enough
to delete a staircase.

## Physical drawing scale

Configured plan views place generated balcony size lettering using glyph ink from the same
bundled fonts and SVG shaping used by Studio, PNG and PDF. The lettering retains at least
0.5 paper mm of visible clearance beyond its dimension pen and white outline at the final
view scale. A scale change therefore cannot compress that clearance to a fraction of a model
millimetre. Unconfigured legacy export layouts retain their existing fingerprints.

Font measurements are paper millimetres and include actual style and anchoring; wide and
narrow letters are not interchangeable. DXF consumers may substitute fonts, so these exact
ink measurements describe Trigon's shared SVG renderer.

## Inspect → preview → apply → validate

Every command below is registered with a typed JSON schema in the engine. Studio's command
channel and CLI use dotted IDs; MCP replaces dots with underscores (`precision.preview` becomes
`precision_preview`). No additional MCP server or paid service is required.

1. Inspect the source with `element.get`, `view.pick`, `view.snap`, `annotation.list` and the
   registered drawing. `drawing.to_site` accepts `exact: true` for unrounded coordinates; its
   default array of whole-mm coordinates remains compatible. `drawing.measure_stair` returns
   an unrounded candidate and source mm/pixel resolution without adding stairs or journaling.
2. Run `precision.audit` on the building/storeys and `view.audit` on each affected saved view.
3. Call `precision.preview` with the typed action. Inspect its exact JSON-pointer source changes,
   returned target IDs and model/annotation validation. This uses a separate document; source,
   dirty state, Undo and Redo stay intact. No file or provider operation is allowed in an action.
4. Pass the returned `preview_token` and unchanged action to `precision.apply`. A changed document
   or action invalidates the token. Success produces exactly one journal/Undo step. Failed edits
   leave the document unchanged. Tokens are opaque, same-build conflict checks, not credentials,
   authorization, durable revision IDs or a cryptographic security mechanism.
5. Audit again, inspect drawings/3D and export the affected IFC/drawing deliverables. Use
   `edit.undo` if the correction is unsatisfactory. A successful build establishes constructible
   geometry; agreement with the evidence needs its own review.

Example MCP arguments for a small, explicitly measured wall junction:

```json
{
  "action": {
    "command": "edit.connect_walls",
    "params": {
      "endpoints": [
        {"target": "west.L1", "end": "b"},
        {"target": "east.L1", "end": "a"}
      ],
      "junction": [5002.5, 4000.0],
      "max_move_mm": 3.0
    }
  }
}
```

`precision_apply` takes that same `action` plus `preview_token`. `edit.connect_walls` permits
2–16 distinct interior-wall sources with identical building/storey scope. Each endpoint must be
within the explicit 0.001–300 mm bound. Shared definitions split only on selected storeys.
Hosted doors keep their world positions; a change that would rotate them off the new wall axis
is refused. Groups, overlapping source/occurrence selectors and ambiguous IDs are refused.
A centre-line junction is explicit: this command does not decide which side of a thick wall
should align. Use `edit.align_walls` for source axis/left/right face alignment first.

Other preview actions are `edit.move`, `edit.align`, `edit.align_walls`, `element.set`,
`annotation.move_label` and `edit.delete`. These reuse the normal edit implementations.
A zero-change or overly large (>4096 changed fields) preview is refused; split the work.

## Sanity checks and interpretation

`precision.audit` reports exact occurrence IDs and measured candidates:

- Small positive wall-material gaps at interior wall ends, using actual door-cut polygons and
  facade openings at 1000 mm above each selected floor. Default search tolerance: 10 mm.
- Nearly parallel wall material overlaps (>1 mm²), measured after joining and door cuts.
  Ordinary crossing wall junctions are excluded from this overlap rule.
- Nearby end-to-end wall runs whose axes and both face alternatives fail to align. This accounts
  for different-thickness walls intentionally aligned on one face. Default mismatch: 2 mm;
  endpoint neighbourhood: 300 mm.
- Abrupt joint angles outside the explicit expected angles, with a default tolerance of 0.5°.
  Defaults are 0° and 90°; use `expected_joint_angles_deg: [0,45,90]` for deliberate diagonal
  transitions. These are design criteria for review, not constraints applied to the model.
- Overlapping stair symbols/departing sources, duplicate source IDs and model build failures.
  Adjacent-storey stair overlap can be intentional; departure levels are reported separately.

Select `building` and/or `levels` to narrow the audit. Numeric thresholds are checked; bounded
comparison budgets reject excessive scopes. `total_findings` and `truncated` distinguish a
complete report from its capped output. `no_findings_in_scope` is not a survey, structural,
accessibility or exhaustive clash certificate. Annotation collisions are a separate view check.

`view.audit` shapes lettering with the bundled SVG fonts at the final physical scale. It returns
rotated ink/halo corners, visible polygons, clipping, actual displayed text and unrounded
measurement values. Saved annotations and generated room/balcony labels retain their exact
source IDs and roles in transient metadata; other generated lettering has view-local keys.
The audit reports potential box collisions using paper-mm clearance (default 0.5 mm), not
pixel-level intersections of individual glyphs. Suppressed labels and generic model-line/text
clashes are outside this check. No annotations are moved or hidden by an audit.

`annotation.move_label` preserves content, font/style and witness semantics. Text moves its
insertion point; leaders move their label end while retaining tip and elbow. Dimensions change
only their perpendicular offset. Level marks move horizontally; vertical motion is refused
because it changes the measured elevation. Generated room/balcony labels are not saved manual
annotations and cannot be moved through this command.

Measured scans, dimensioned plans and IFC are the reconstruction basis; imagery provides
corroboration. Preserve contradictory evidence for review. Recognition confidence, calculation
precision and dimensional uncertainty are separate concepts. The current scan registration
supplies mm/pixel resolution, not a survey residual or a physical accuracy guarantee. FilArkiv
provider access remains optional; ordinary local PDF/image workflows do not depend on it.
