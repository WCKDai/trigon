# Two Cut–Plumb roof edges

`building.set_roof` accepts optional `fascia_depth_mm` on pitched and half-hipped
roofs. Properties exposes **Rafter cut** (`plumb` or `two_cut_plumb`) and
**Two-cut vertical fascia depth** (20 mm by default until enabled). Setting the
depth also enables the cut. The uncut mode retains the existing roof geometry.
The cut preserves the declared normal build-up, upper roof planes, ridge and wall
bearing. It adds a short vertical fascia followed by a horizontal underside return;
it does not reduce the entire roof to the fascia depth.

```json
{
  "kind": "pitched",
  "pitch_deg": 46,
  "gables": ["b.f2", "b.f4"],
  "thickness_mm": 200,
  "overhang_mm": 400,
  "fascia_depth_mm": 20
}
```

For an existing roof, use a single scoped `element.set` transaction. This preserves
selected source eaves, dormers, rooflights, drainage and all other base parameters:

```json
{
  "id": "b.roof",
  "properties": {"fascia_depth_mm": 20, "overhang_mm": 400}
}
```

`rafter_cut: "two_cut_plumb"` enables a 20 mm depth unless an existing or explicit
depth is supplied. `rafter_cut: "plumb"`, or `fascia_depth_mm: null`, removes the
cut. Conflicting mode/depth properties refuse atomically. All assignments in a
property transaction precede cross-field validation, so their lexical order cannot
invalidate a coordinated depth/overhang edit.

## Support and geometry

Depth is vertical, from 1 mm through the declared normal thickness. The opt-in mode
requires a positive normal thickness and the existing 5–75° pitched-roof range.
Flat, mansard and nested cuts are unsupported and refuse explicitly. Zero depth is
rejected to avoid a collapsed fascia face. Gutters use the fascia toe datum;
explicit pipe bottoms and source-edge identities are retained.

For pitch `a`, normal thickness `T`, overhang `O`, fascia depth `D` and wall bearing
`H`, the original vertical lift is `L = T / cos(a)`. The fascia toe is at
`H - O * tan(a) + L - D`. Each original upward roof triangle is split at that
height before constructing the upper skin and the underside `max(original, toe)`.
Identical canonical intersections split the original boundary, retaining a closed
shell even when source vertices are close to the return.

The horizontal return run is `(L - D) / tan(a)`. Overhang must cover that run to
retain wall bearing, plus the actual projection of any cornice seated at the wall
top. Failure reports the required overhang; Trigon does not move walls, thin the
construction, truncate a cornice or invent a supporting measurement. At 46°,
T200/D20, the return is 258.718943 mm; a 135 mm cornice needs at least 393.718943 mm
overhang. Those are geometric constraints, not surveyed dimensions.

## Persistence and exchange

Native source uses a distinct `kind: "two_cut_plumb"` wrapper with
`fascia_depth_mm` and one existing `roof` source. Legacy source serialization and
the public `Buildup`/`pitched_with` interfaces remain unchanged. `Roof` gains an enum
variant; exhaustive Rust consumers need to account for it. `base()`/`base_mut()`
inspect one protected level, and existing dormer/rooflight/half-hip accessors retain
the wrapper. Older readers reject the new kind rather than silently discard the
cut. Removing it restores the original base kind; save a separate review copy when
interchanging with older builds.

IFC exports the actual closed cover on `IfcRoof`, even without openings. It keeps
the declared normal thickness in the material build-up and records `RafterCut`
and `FasciaDepth` in `Trigon_RoofGeometry`. Heuristic plane slabs cannot represent
the horizontal return and are omitted for this mode. GLB, sections, picking and
roof openings consume the same built geometry. Reader/schema verification does
not establish editable Revit family behavior or actual Revit import acceptance.

Sections, ranged plans and elevations include the actual closed facade-band
solids, with explicit material parts exported once. Their drawing ownership leads
back to the editable host facade; brick cornices use masonry section hatching.

The terminology follows the [Autodesk rafter-cut documentation](https://help.autodesk.com/cloudhelp/2023/ENU/Revit-ArchDesign/files/GUID-AAEDDF4E-C84B-4DEF-90CF-B38480AFEA31.htm).
This is an original parametric implementation; no external application code is used.

Rollback: revert the feature commit and open a pre-cut project copy. For a new
project saved with the feature, first remove the cut using the current reader;
an older reader intentionally refuses the wrapped kind.
