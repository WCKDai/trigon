# Guarded footprint editing

`building.set_footprint` edits existing corners in site-local millimetres through the same
journaled engine command used by CLI, MCP and `precision.preview` / `precision.apply`.
It is a precision adjustment of an existing ring, not a replacement/topology operation.

```json
{
  "command": "building.set_footprint",
  "params": {
    "building": "b",
    "expected_footprint": [[0,0],[24000,0],[24000,12000],[0,12000]],
    "moves": [{"corner":0,"point_mm":[-80,-120]}, {"corner":1,"point_mm":[23920,-120]}],
    "anchor_policy": "project_wall_line_centres"
  }
}
```

Read `building.outline` and use its **zero-based `corner`** and exact `point_mm` values.
`expected_footprint` must equal the complete current source ring, in its existing order.
A stale expectation, duplicate index, missing policy, unknown field, no-op or invalid candidate
fails without changing the document, journal, Undo, retained Redo, save path or dirty state.
Preview runs the same validation on a separate document; apply journals the underlying action
once, so saved journals replay without an ephemeral preview token.

The policies are required, with no default:

- `keep_facade_u` keeps every facade-local station. Hosted objects follow the changed facade.
- `project_wall_line_centres` projects each previous site-local **wall-line centre/anchor** onto
  the new source-facade tangent: `u' = dot(old_origin - new_origin + old_tangent * u, new_tangent)`.
  This is tangential registration. It does **not** preserve perpendicular world position when
  the wall moves. Bay centres, explicit placement centres, dormer/rooflight `u_mm` anchors and
  drainage `along_mm` use this rule. Roof run/setback, yaw, height and wall offsets stay relative
  to their host; this does not promise to preserve the geometric centroid of an offset or
  inclined component. Implicit placements still share their original bay, without adding an
  override or changing other storeys' placement records.

There is no bay, placement, facade or ID sorting. Corners must stay within 49% of their shorter
original adjacent edge, capped at 10000 mm, and every edge must keep a positive dot product
with its previous tangent. These conservative bounds prevent cyclic reindexing, reversal and
large relocation disguised as a precision edit. The ring remains simple, CCW, open, with
3–2000 corners, edges of 1.001–200000 mm and area at least 4 m². Coordinates are finite and
within ±100000000 mm of the site origin. Existing measured near-collinear corners are retained;
the constructor's 200 mm cleanup is never called.

The engine maps and checks dependents before accepting the candidate:

| Consumer | Contract |
| --- | --- |
| Facade `edge`, bays, placements, phase/level/bay IDs | Exact mapping/order retained; all placements resolve against the new host, including historical ones. |
| Pitched, half-hipped, mansard and Two Cut–Plumb roofs | Source parameters and selected facade lines retained; geometry rebuilt by `crates/model`. Dormer, rooflight, gable and drainage failures propagate. |
| Implicit roof/basement straightening | Refuse changed kept-corner mappings; do not silently consume a different facade. |
| Cellar stairs | Keep-u follows the host. Projection supports parallel host motion; rotated projected multi-station assemblies are refused with the stair ID and angular/linear change. |
| Plinths, bands, slabs, stair holes | Keep all source properties and identities; regenerate through the model and validate. |
| Interior walls and doors | Source stays site-fixed. Refuse automatic join changes or disappearance instead of silently moving doors or deleting degenerate geometry. |
| Rooms, stairs and interior source lines | Stay site-fixed; refuse a new footprint crossing or excluding them. |
| Groups, occurrences, entrance/stair bindings, families, evidence | Preserve all source records and references; check built semantic identity inventories in every phase. |
| Drawing registrations and annotations | Preserve stored coordinates, evidence and transforms. Plan underlays remain site-fixed. Facade end registrations inherit the changed host length; report old/new scale and require source review. No automatic redimensioning or accuracy claim. |

The site-fixed-source containment guard is deliberately conservative: all room polygons, stair
shafts and interior source axes must be inside the candidate footprint. Rooms must also fit
each occupied storey’s actual built floor boundary, so an inward wall move cannot consume
room floor while leaving the polygon nominally inside the outer wall line. It can refuse an
already-outside source axis even when the existing model builder clips/joins it into valid built
geometry. Such a refusal is a scope limitation, not proof that the previous built wall is
invalid; it does not authorize moving that source or bypassing the guard.

A full latest model build, annotation validation and per-phase identity checks run before
success. The result includes exact moved corners, affected facade frames/lengths, anchor station
and site-coordinate deltas, roof-height changes and drawing-registration feedback. Diagnostics
include affected IDs and numerical context in mm; downstream failures are never converted into
a successful preview. Feedback is bounded to 4096 anchor records, each fixed-shape containment
check to 4096 edge intervals (including crossings at concave vertices), and phase validation
to 64 phases; exceeding a bound is a refusal, not truncated success.

`FORMAT` and persisted source fields are unchanged. Existing commands retain their schemas and
defaults; precision's closed action union gains this one action. Old readers can read the saved
source geometry, but cannot replay the new journal command. Undo restores exact old source and
geometry; reverting the implementation commit removes the command, not journal entries already
saved in separate project files. Keep an original project copy when sharing with older tools.

The command proves the stated geometric and identity checks, not source-survey accuracy,
building-wide coordination, structural adequacy or downstream application acceptance. Source
uncertainty and independent drawing review remain separate from millimetre calculations.
