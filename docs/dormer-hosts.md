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
is no migration, automatic rotation or placement snapping. Older readers accept zero-yaw documents;
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

## Parent integration: individual element properties

The typed roof commands already carry the field and validate the resulting geometry
atomically. The following exact, **unapplied** hunk for `crates/engine/src/elements.rs`
adds the field to individual dormer `element.get` / `element.set` properties. Apply
after the precision work is integrated; this model change does not edit that owned file.

```diff
--- a/crates/engine/src/elements.rs
+++ b/crates/engine/src/elements.rs
@@ -523,6 +523,7 @@
                         number("width_mm", "Dormer window width", spec.width_mm, "mm", 300.0, 3000.0),
                         number("height_mm", "Dormer window height", spec.height_mm, "mm", 300.0, 2200.0),
                         number("u_mm", "Centre along host facade", spec.u_mm, "mm", 0.0, 1e8),
+                        number("yaw_deg", "Rotation about +Z from source facade", spec.yaw_deg, "deg", -45.0, 45.0),
                         number("setback_mm", "Front setback from wall", spec.setback_mm, "mm", 100.0, 1500.0),
                         number("cheek_mm", "Side cheek thickness", spec.cheek_mm, "mm", 50.0, 200.0),
                         number("head_mm", "Head above window", spec.head_mm, "mm", 50.0, 400.0),
@@ -1081,6 +1082,7 @@
                     "width_mm" => d.width_mm = numeric(key, value)?,
                     "height_mm" => d.height_mm = numeric(key, value)?,
                     "u_mm" => d.u_mm = numeric(key, value)?,
+                    "yaw_deg" => d.yaw_deg = numeric(key, value)?,
                     "setback_mm" => d.setback_mm = numeric(key, value)?,
                     "cheek_mm" => d.cheek_mm = numeric(key, value)?,
                     "head_mm" => d.head_mm = numeric(key, value)?,
```

Native/Revit import acceptance belongs to the parent integration and remains unverified
by the model tests. The existing IFC/GLB writers consume the rotated built geometry.
