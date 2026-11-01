# Dormers on skew roof hosts (#27)

A shed dormer must occupy one continuous, thick pitched or mansard roof plane. Eave
straightening can make that plane oblique to its source facade. Choose its orientation
explicitly with `yaw_deg` in the dormer source passed to `building.set_roof` or the roof's
`dormers` property. The source is persisted and journalled through those existing commands.

`yaw_deg` is finite and lies in **[-45, 45] degrees**, inclusive. Positive means right-handed
rotation about site **+Z** (counter-clockwise in plan), relative to the source facade's
start-to-end tangent. `u_mm` remains the reference point on the **original wall line**;
the new tangent and inward direction rotate about that point. `setback_mm` is measured
from that point along the rotated inward direction to the front, and all window, cheek,
shed-roof and host-cut dimensions use this rotated frame. All lengths remain millimetres.
The 45-degree bound keeps both axes facing toward their source-facade directions.
Yaw is a modelling choice, not evidence of survey accuracy.

Omitted yaw means zero. Zero, including negative zero, is omitted on serialization. The
zero-yaw construction path retains the original frame, dimensions, IDs and meshes. There
is no migration, automatic rotation or placement snapping. Rust consumers constructing a
`Dormer` struct literal must initialize `yaw_deg` (use `0.0` for legacy orientation). Older readers accept zero-yaw documents;
nonzero-yaw documents require a reader supporting this additive field. Before reverting
the implementation, restore a pre-yaw file or explicitly remove/rework oriented dormers
using their supported host geometry; silently dropping nonzero yaw changes the design.

The host's real upper and lower planes must be level across the chosen front, using the
existing slope tolerance. If they are not, feedback identifies the dormer and source
facade, reports along-front slopes in mm/mm, and gives the required `yaw_deg` in degrees
when the sampled planes admit one orientation. That suggestion still requires the normal
host checks: it is not a promise that the entire dormer fits. Do not infer a roof from a
facade, increase straightening/tolerances to hide a mismatch, or replace a rejected host
with invented geometry.

The existing limits still apply: at most 16 dormers and 64 footprint corners, at least
50 mm vertical host build-up, host pitch steeper than the shed, 200–5000 mm depth behind
the front lining, continuous upper and lower host surfaces clear of hips and slope breaks,
100 mm facade-end clearance and 100 mm between roof projections on the same facade.
For nonzero yaw, end clearance and spacing use the full rotated roof envelope projected
onto the original facade, including its overhang and rear underlap. This is conservative:
an oblique envelope may need more room along the source facade.
Oriented cheeks must retain positive height through the full rear underlap. The actual
host build-up and shed thickness can prevent this even after the basic 50 mm host check;
feedback reports the rear height in mm. The zero-yaw path retains its legacy behavior.

For nonzero yaw, front-wall polygon-overlay output is reconciled to its known source
corner and opening vertices within the unchanged `1e-6` mm numerical epsilon. This
keeps overlay faces, exact caps and reveals joined at fractional host elevations; the
input dimensions and orientation remain exact source choices.

## Preserve source eave directions across recesses

When the intended eave follows source walls across a recess, use the pitched roof's optional
`eave_facades` property before considering dormer yaw. Legacy `eave_outline` drops footprint
vertices and connects the remaining corners; that chord can rotate the roof plane away from a
long source wall. The opt-in selector instead intersects each pair of adjacent selected source
**lines**. Edge k belongs to selected facade k. No wall, footprint vertex or facade ID moves,
and no arbitrary new point, automatic rotation or inferred photo measurement is accepted.

```json
{"command":"element.set","params":{"id":"b.roof","properties":{
  "eave_facades":["b.f1","b.f4","b.f5","b.f6","b.f7","b.f8"]
}}}
```

Select 3–256 unique known facade IDs in counter-clockwise footprint order, wrapping once;
any cyclic starting facade is allowed. Parallel adjacent selected lines have no unique corner.
Intersections must be finite, within 100 km of the site origin and 200 m of both source starts;
source lengths and forward eave runs must be 1–200000 mm. The derived ring must be simple,
counter-clockwise, at most 1000000 m², and cover every interval of the original footprint
boundary, including edges crossing concavities. An inward roof that merely covers the vertices
is refused. Named gables must be selected and retain both original endpoints; include their
adjacent source lines rather than extrapolating an unsupported gable wall.

The selector overrides `straighten_mm` while present. Set it through the existing roof
Properties/`element.set` command; `building.set_roof` still replaces the roof with its legacy
selection default. Omitted or `null` selection preserves exact legacy source serialization,
geometry and export paths. An empty list is invalid. This feature is not supported on mansards.
Rust `Roof::Pitched` literals initialize `eave_facades: None` for legacy behavior. Opted-in
files require a supporting reader: older readers may ignore this field. For rollback, restore
a pre-selection copy or deliberately rework dependent dormers/drainage/caps with commands;
silently deleting the selector changes the design. Clearing it is rejected if a hosted feature
would become invalid. Successful edits are journalled and undoable; failed edits are atomic.

Dormers retain the same source-wall `u_mm` anchors and all existing continuous-plane, build-up,
hip, clearance, spacing and through-void checks. A matching source eave normally permits zero
yaw; selecting lines does not guarantee that a whole dormer fits. On selected hosts, the
existing source-vertex reconciliation also joins zero-yaw front-wall overlay vertices to exact
caps/reveals within the unchanged `1e-6` mm epsilon. Legacy `None` meshes remain unchanged.

[`examples/roof-source-eaves.json`](../examples/roof-source-eaves.json) is an entirely synthetic
command recipe with a notched footprint, selected eave, explicit downpipe and a zero-yaw dormer.
Without the selector the dormer requires approximately −18.434948823 degrees of yaw because of
chording; the recipe corrects the eave instead. No private survey coordinates enter the example.
Use the resulting model and exported views for source review through a complete 360 degrees;
this construction option does not establish that any real building is complete or measured.

## Public synthetic reproduction

Use `building.add` with id `b`, three storeys at 3400 mm and the entirely synthetic outline
`[[0,0],[12000,0],[24000,-1000],[24000,11000],[0,11000]]` (site-local mm).
The default 1200 mm roof straightening removes the shallow reflex corner at `[12000,0]`.
The resulting front eave is the chord `[0,0]` to `[24000,-1000]`, oblique to `b.f1`.

Pass the following source to `building.set_roof`:

```json
{
  "kind": "pitched",
  "pitch_deg": 46,
  "thickness_mm": 220,
  "overhang_mm": 250,
  "dormers": [{
    "id": "skew",
    "facade": "b.f1",
    "u_mm": 6000,
    "yaw_deg": -2.3859440303888126,
    "window_type": "window.casement.2",
    "width_mm": 1100,
    "height_mm": 1100
  }]
}
```

Without yaw this is rejected because the actual host rises along the facade. With the
explicit chord orientation it builds a real roof void and outward closed host, front,
cheeks and shed solids, with glazing above the uncut host envelope. Reflecting the
outline tests positive yaw; translating and rotating it tests site-frame independence.
These fixtures are independent of any client building.

Run the focused regression with:

```sh
CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=1 cargo test -p trigon-model --locked build::dormer::tests
```

## Individual element properties

The displayed front, cheeks, shed roof and window expose `yaw_deg` through
`element.get` and `element.set`, alongside their existing width, height and setback
properties. The Properties panel uses the same finite degree bounds and typed edit.
The source facade's `u_mm` anchor stays unchanged when only yaw changes. A rejected
orientation leaves the document and history unchanged; accepted edits are journalled,
undoable and retained by save/reopen. The existing canonical edit target is the
dormer's window ID, for example `b.dormer.skew.window`.

Native/Revit import acceptance belongs to the parent integration and remains unverified
by the model tests. The existing IFC/GLB writers consume the rotated built geometry.
