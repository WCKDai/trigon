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
   leave the document unchanged. The journal records the underlying typed action, without its
   ephemeral preview token, so model command replay remains usable. Tokens are opaque, same-build conflict checks, not credentials,
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

## Captured wall-alignment references

`edit.align_walls` retains its interior-wall `reference` and `reference_face` / `target_face`
defaults. Existing unique interior selectors retain precedence, including legacy source IDs
containing colons; `wall:<source-id>` explicitly selects that source. When a selector does not
name an existing editable element, it also accepts convenience datum prefixes. Explicit objects
resolve only their stated kind and are unambiguous even if a legacy wall uses the same text ID:

- `{"kind":"annotation","id":"<id>"}` (or `annotation:<id>`) uses an existing `line` annotation in a saved **plan**. Its coordinates are
  site-local XY millimetres. Facade/section annotations are refused because their coordinates
  are a different plane. The line itself is the datum, so `reference_face` must be `center`.
- `{"kind":"facade","id":"<id>","level":1}` (or `facade:<id>:L1`) uses the represented facade
  wall's actual material on one storey.
  `left` is its inner line, `right` its outer line, and `center` the endpointwise midline.
  This uses the actual built quadrilateral, including skewed inner faces; it does not substitute
  an offset of the facade axis by nominal thickness. A missing, collapsed or ambiguous reference
  is refused. Roof/gable-only faces are not admitted by this selector.

Guide targets must be exact occurrences on the guide plan's storey. Material-face targets must
also belong to its building. A shared source spanning other levels must be selected as a storey
occurrence (`wall-id.L1`), so one floor's material cannot silently control different floors.
The existing interior-reference scope rules and 15-degree direction limit are unchanged.

Optional `max_move_mm` (0.001–100000 mm) bounds the largest source-endpoint displacement,
including rotation. The entire batch is checked before source changes. Omitting it retains the
legacy unbounded wall-alignment behavior. Lengths and door distances measured from source A
remain unchanged; door world positions can move with the rigidly aligned wall. Use an explicitly
reviewed movement bound appropriate to the source evidence.

```json
{"action":{"command":"edit.align_walls","params":{
  "targets":["partition.L1"],"reference":{"kind":"annotation","id":"measured-axis"},
  "target_face":"center","max_move_mm":10
}},"sanity":{"building":"b","levels":[1]}}
```

The additive `alignment` result reports the resolved source/datum ID, exact line, reference kind/scope, affected
levels and each target's before/after endpoints, signed rotation, maximum endpoint motion and
reference-normal residuals. Resulting `targets` remain the authoritative IDs after storey splitting.
These residuals describe source-axis/nominal target-face alignment, not a survey-accuracy claim
or proof that every joined target face is coincident. Inspect the rebuilt material and sanity
findings as well. `precision.preview` / `precision.apply` use these same edits. Changing a guide,
material reference or document invalidates an earlier preview token.

Use existing styled `annotation.define` lines for red dotted guides in a named coordination plan.
They remain view annotations and export with that view; keep a separate presentation view for
client drawings without them. They are not nonprinting reference planes or live constraints.
Changing a guide after alignment does not automatically move walls. A supporting engine is
required to replay journals containing the new selector forms; no native source-field migration
is introduced.

For source control points, `building.outline` now includes `point_mm: [x,y]` without rounding.
Its established `x` and `y` fields retain their whole-mm values for compatibility. Reading the
outline does not journal or change the stored footprint. Numeric precision remains separate from
the measurement and registration uncertainty of that footprint.

Other preview actions are `building.add`, `edit.move`, `edit.copy_to_levels`, `edit.align`,
`edit.align_walls`, `element.set`, `annotation.move_label`, `edit.delete`, `view.define`, `view.configure` and `annotation.define`.
These reuse the normal edit implementations. `edit.copy_to_levels` previews destination
collisions and exact source changes with its existing `replace` behavior; success journals the
underlying copy action once, and stale previews or invalid destinations leave source and Redo
unchanged. Storey indices are explicit; copying does not infer measured heights.
Drawing definitions/configuration and label moves also return `drawing_feedback`: the candidate's `view.audit` results at final physical scale,
including glyph/halo bounds, source IDs, crop findings and potential overlaps. Moving an annotation
between views audits both final views. Feedback does not change the source or repair the layout.
`validation.drawing_render: "passed"` means those checks ran, not that the layout is acceptable;
inspect each audit's status, findings and truncation. A stale view or the 2,000-label audit limit
returns explicit `unverified` feedback and an error, with `drawing_render: "unverified"`. Model and
annotation validity are separate. Existing repair actions remain available in that case; inspect
the drawing separately before accepting it. The action list still excludes file/provider operations.
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
- Visible stair openings intersecting actual joined interior/facade wall material (>1 mm²).
  `stair_wall_overlap` supplies the displayed storey, departing stair occurrence, wall ID,
  `opening_overlap_mm2` and read-only inspection commands for both sources. Repeated wall
  pieces are aggregated by wall ID. Receiving symbols are checked against the receiving
  storey's walls. A buildable stair can still collide with its enclosure; these findings are
  not automatically repaired or hidden by clipping the drawing.

Select `building` and/or `levels` to narrow the audit. Numeric thresholds are checked; bounded
comparison budgets reject excessive scopes. `total_findings` and `truncated` distinguish a
complete report from its capped output. `no_findings_in_scope` is not a survey, structural,
accessibility or exhaustive clash certificate. Annotation collisions are a separate view check.

`wall_end_in_door_opening` identifies an interior wall end whose entire cap lies inside a
recorded door void at the 1000 mm plan cut. It supplies the wall, host and opening IDs,
clearance to the nearest jamb, and read-only inspection commands. A cap touching a fixed
jamb is excluded. Buildable geometry can still contain this source conflict; the finding
requires review of the drawing and door placement before any repair. The audit does not
close an opening, move a wall or certify a structural connection.

Interior wall and room additions use the same preflight as movement and alignment. For
example, `precision.preview` accepts an `action` with `command: "interior.add_walls"` and
the ordinary command's `params`. Add an optional `sanity` object with the same building,
storeys and tolerances as `precision.audit` to inspect the candidate before changing the
project. The returned `sanity_feedback` retains complete counts and explicit truncation;
`validation.sanity` is `no_findings_in_scope`, `needs_review` or `unverified`. A model-build
pass does not imply that its connections make architectural sense. An unavailable audit
includes its error and never counts as passed.

Omitting `sanity` preserves the existing response and avoids the additional audit. The
inspection scope is independent of the token, which binds the source document and exact
edit. Applying the returned action creates one underlying `interior.add_walls` or
`interior.add_rooms` journal entry, without nesting command execution. Room detection from
registered scans is not a preflight action: these previews do not read files, infer new room
names, split existing room polygons or repair door conflicts automatically.

New room authoring uses the same polygon validation as room boundary edits: simple,
nonintersecting boundaries, coordinates within ±100,000,000 mm, edges of at least 1 mm
and area of at least 0.01 m². `interior.add_rooms` retains its 3–256 corner limit and
requires at least one full storey. It preserves valid coordinates and winding exactly.
A refused direct command or precision preview preserves the whole document and Redo,
including when an earlier room in the batch was valid. Reading existing native files
and the defensive rendering of malformed legacy room records are unchanged.

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


## Starting a local measured project

`building.add` creates one parametric building from a site-local `footprint`, explicit `id`,
`name`, `storeys` and `storey_height_mm`. Optional `ground_floor_mm`, `wall_thickness_mm` and
`evidence` describe its basis. It requires no account/provider and does not clear earlier
buildings. The default `preserve_vertices: true` keeps small measured deviations, normalizing
only winding and an exactly repeated closing vertex. Self-intersecting/touching outlines,
zero edges, duplicate building IDs and hostile coordinates are refused. Setting
`preserve_vertices: false` explicitly chooses the legacy 200 mm context simplification.
Use `building.set_levels` for unequal measured floors and `building.add_basement` for levels
below ground. A numeric source value is not a claim that its dimension was actually measured.


## Archival scan encoding

The scan importer reads original JPEG and 1-bit grayscale CCITT Group 4 PDF image streams,
without rendering, resampling or recompressing the page. Group 4 polarity follows PDF
`BlackIs1` and `Decode`; dimensions/rows are checked before allocating the grayscale image.
The default end-of-block marker is required to reject truncated input. With
`EndOfBlock: false`, every declared row must decode. Single-filter arrays and indirect decode
parameter dictionaries are supported. Group 3, indexed palettes, image masks, byte-aligned
CCITT rows and compound filter chains remain unsupported and return explicit errors.
The existing 1 GiB file and 400-million-pixel limits still apply; Group 4 dimensions must also
fit the codec's 16-bit width/height. No external converter or service is required.

A plan storey index may exist in only some buildings. Symbolic plans omit buildings without
that storey; their footprints and incoming stair symbols do not expand or contaminate that
floor's drawing. Saved views accept a level present in any building. Physical-range plans
anchor their absolute cut to the first building containing that level, preserving the existing
single-building convention, and show other geometry only within the specified height range.

Basement windows and doors may extend below terrain, bounded by their departure storey's
floor. Above-ground openings retain the terrain bound, and non-opening facade attachments
retain their existing bounds. The model does not infer the associated light well or earthworks.

`building.set_roof` accepts `dormers` on ordinary pitched roofs as well as mansards. Each
requires a continuous straight host face with at least 50 mm build-up, real roof cut-outs,
hip/overlap clearance and a window family. The same exact dormer IDs support drawing/3D
picking, `element.get` / `element.set`, precision previews, Undo and exchange. Empty pitched
dormer lists are omitted in saved files, preserving legacy serialization and geometry. Feature
files require a reader supporting pitched dormers; older readers can discard this new optional
field. Roof coverings remain mansard-only; flat roofs reject dormer parameters explicitly.


Facade bands accept an optional `finish`: `brick`, `red_brick`, `yellow_brick`,
`render` (white) or `render_terracotta` (a warm presentation preset, not a paint survey).
Missing `finish` preserves legacy white geometry and serialized fields. Explicit finishes
share the same opening cuts, and material-separated parts are drawn/exported once in Studio,
GLB and IFC. IFC gives each finish its own covering identity and material. Existing GLB
material indices are unchanged; the new preset is appended and omitted when unused.

`element.get` / `element.set` expose facade `bands` as a typed property array, so the same
checked parameters are available through Properties, MCP previews and Undo. Each band
requires existing storeys, finite bounded heights and 1–500 mm projection; at most 50 bands
are accepted per facade. Files using the new finish need an updated reader. The additive
Rust `Band.finish` and derived `BuiltWall.trim_parts` fields require source consumers that
construct those structs directly to initialize them; legacy JSON is unchanged.

Saved view templates optionally override printed `caption` (40 characters) and `note`
(80 characters). Missing fields retain the generated frame text and legacy serialization;
an empty string hides that part of the frame. Overrides are single-line text, do not rename
the building or alter model geometry/manual annotations/picking, and save/reopen/Undo through
`view.configure`. Studio exposes the same controls in the view template settings. This lets
a multi-building review sheet carry its actual project caption and evidence note.

At an exact shared endpoint, collinear wall pieces of matching thickness are treated as
one continuous run when resolving a T or four-way junction. This prevents both pieces
from extending through the same branch and duplicating material. Thickness, combined run
length and source order resolve priority; the saved axes, source IDs and lifetimes remain
unchanged. Intentional gaps and noncoincident source ends are not snapped by this correction.
Mixed-thickness transitions and other audit candidates still need source and joint review.


## Source-complete entrance lettering

Plan presentation tags retain every source-known `TV`, `TH`, `MF` and numeric door
designation. Case and surrounding whitespace are normalized for display only; unit
records remain unchanged. Empty, `?`, `unknown`, `ukendt`, `uoplyst` and `ikke oplyst`
designations share the inspector's unknown classification. Elevations show the union
of known designations for the entrance, not an invented TV/TH pair or a promise that
every storey has those sides. The unit register is not phase-specific evidence.

Generated text carries transient ownership `{kind:"entrance", id:"b.entrance0",
role:"unit_register"}`. The suffix is the entrance's **zero-based current-document
index** within building `b`. This selector is not durable across provider replacement,
insertion or reordering. It introduces no native schema or persisted stable ID.
`view.audit` supplies `entrance.get {id}` inspection suggestions; the local entrance
editor must verify expected address/unit data before a later edit. Generic element
picking is unchanged by this lettering-only ownership.

Default-scale `plan.sheet`, live presentation plans and elevation exports keep a
preferred legacy anchor if the real bundled-font ink and exported halo fit. Colliding
tags try bounded positions along their known outward axis, then an explicitly named
legend in whitespace outside the drawing's geometry bounds. Font sizes are retained.
Configured saved sheets repeat this placement after the final physical scale, crop,
room tags and saved annotations are known. Missing explicit stair associations use a
named legend and carry a placement note; no basement association or doorway is inferred.

Drawing metrics and `view.audit.entrance_labels` include requested, placed, alternate,
legend and unplaced counts, plus per-tag text, unit-record/unknown-door counts, source
selector and result. If the bounded search or glyph comparison budget cannot place a
complete label, it remains in the unclipped audit scene with `unplaced_entrance_tag`
and a visible sheet note; it is never silently deleted or called successfully placed.
Unplaced tags retain their original anchors; they are never pushed onto crop edges.
A clipped export may show only part of nearby unresolved text. Resolve its source/layout
before client delivery. Glyph shaping errors remain errors.

The clearance check covers actual glyph boxes/halos against other text and filled
wall material. Legends avoid the full model-geometry bounds. It is not a general
line/leader routing solver, proof of the unit register, or a door-opening association.
Ranged and roof-only plan consumers that do not yet generate entrance tags are outside
this slice. Existing lettering/wall overlap findings remain visible. DXF applications
may substitute fonts; final receiving-application review is still required.

## Local entrance editing

`entrance.list {building?}` and `entrance.get {id}` inspect the stored register without
calling a provider. `entrance.set_unit` checks the exact `expected_address`, unit index
and `expected_unit` before replacing one record. `entrance.bind_stair` checks that same
address and the expected stored binding. Omitted `level` edits the ground-serving
default; a level edits its explicit departing-flight override. Clearing an override
resumes the default. These commands validate existing source stairs without using a
nearest-stair heuristic. A valid binding does not establish its physical accuracy.

Both writes support `precision.preview` and exact Undo/Redo. Their previews now audit
affected saved plans and elevations before Apply. `validation.annotations` describes
structural validity; `validation.drawing_quality` and `drawing_feedback` describe the
actual lettering review. Missing views or failed bounded checks remain unverified.

## Studio coordination controls

Architecture's ribbon includes reference guides, source outline inspection, wall-face
alignment and copying to named levels. The Project Browser exposes saved plan lines,
entrances/apartment records and terrain. Properties uses the same typed commands as MCP.
An active saved Plan supplies its actual storey; a facade or section does not silently
become a plan datum. The outline inspector requires a floor choice when no plan plane
is active.

Guides use red dotted Line annotations on named coordination plans. Their captured
endpoints retain source precision. Before Apply, Properties shows the captured action,
reference, endpoint movement/rotation, source changes and verification status. Inputs
are locked during that review; Cancel allows revision. A new preview request clears
the previous Apply action even if the replacement fails. Document changes invalidate
the preview token. These are one-time edits rather than persistent constraints.

Terrain Properties exposes the existing DHM grid command and a bounded, validated local
Terrain JSON import. Heights remain site-local millimetres relative to the existing
site datum; source and attribution are retained. Import requires an explicit action.
Coarse terrain samples are context, not a replacement for measured grading data.

## Material-bounded floors and rooms

Floor profiles reconcile consumed corners against the filled facade material instead
of trusting a potentially folded indexed inner line. Zero-area consumed bands own no
material. The resulting usable profile must remain simple, strictly inside the exterior
and connected; disconnected pieces are explicitly refused. Stair void sides follow
their ring winding, with outward closed solids and consistent volume.

`Building::storey_usable_outline` exposes that same boundary before stair openings.
It does not replace the facade-indexed `storey_inner` contract. Room detection uses the
canonical boundary and clips its raster-derived candidates to it, preventing exterior
phantom rooms. Existing room definitions change only through an explicit command.
This checks geometric consistency; the evidence still determines the correct walls.
