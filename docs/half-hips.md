# Clipped gable roofs

`half_hipped` is an explicit single-pitch roof kind with one or two clipped gable ends
(halvvalm). Each `half_hips` entry names an existing source gable facade and gives `drop_mm`
below its original underside tip at the wall line. The end plane uses the existing roof
pitch. The eaves, wall axes, storeys and side gutters stay fixed. At least 100 mm of gable
above the eaves and 100 mm of the original ridge must remain. A tapered wing can have a
sloping ridge; the retained portion follows the original side-plane intersection.
The inclusive 100 mm retained-gable limit accounts for scale-dependent `f64` arithmetic roundoff after
rigid transforms. Coordinates are not snapped and physical margins are unchanged; the
regression matrix also verifies refusal of a deliberate 0.001 mm shortfall.

The initial supported layout is a strictly convex four-edge eave ring with two opposite
gables. Drops must be finite and 1–10000 mm, with positive 1–1000 mm roof build-up. The
ordinary roof pitch/overhang/verge bounds still apply. Duplicate, absent or non-gable hosts,
consumed ridges, inadequate retained gables and lowered eave corners are refused. No convex
hull or wall straightening repairs an unsupported source outline. End caps use the source
wall line, including its existing endpoint/ownership checks under selected eaves.

To change an existing single-pitch roof without resetting its other parameters, use
`element.set` on its roof ID:

```json
{"id":"b.roof","properties":{"half_hips":[{"facade":"b.f4","drop_mm":1200}]}}
```

The source becomes `half_hipped`. `half_hips: []` returns it to `pitched` while preserving
all other roof parameters. Both edits support precision previews, one journal entry, exact
Undo/Redo and Properties. `building.set_roof` also accepts `kind: "half_hipped"` with an
explicit nonempty cap list; like its other kinds, it replaces the roof configuration.
This command also supports precision preview/apply, including its source-evidence fields.
See [the public command example](../examples/half-hipped.json).

The actual cap is part of the closed top/soffit/fascia shell, including its real mitres.
Existing dormers and rooflights must still pass their actual supporting-plane, hip and
mutual-opening guards. Caps do not relocate or shrink an opening. A transverse gutter or
an attic gable window is not inferred. Explicit [attic gable windows](gable-windows.md) can
be placed on eligible source facades. Cap dimensions and facade appearance are modelling
inputs, not measured construction or structural verification.

IFC exports the actual shell even when it has no openings, with `HIPPED_GABLE_ROOF` on
the occurrence and roof type. This is the shape defined by the
[IFC4 roof enumeration](https://standards.buildingsmart.org/MVD/RELEASE/IFC4/ADD2_TC1/RV1_2/HTML/schema/ifcsharedbldgelements/lexical/ifcrooftypeenum.htm).
It retains the declared normal thickness and the existing written-shell certification and
quantity limits; self-intersection and structural adequacy are not certified. The clipped
gable remains an `IfcWall` with its source facade finish and stable gable identity.

Existing `flat`, `pitched` and `mansard` source serialization and geometry are unchanged.
The new native kind deliberately requires a reader that supports it: older versions refuse
the unknown kind. This makes no global file-generation or writer/backup-policy change and
does not protect other optional features from older readers. Keep pre-feature project copies
for deliberate downgrade. Public Rust matches over `Roof` must handle the new `HalfHipped`
variant; existing `Roof::Pitched` struct literals remain unchanged. The separate gable-window
feature adds `BuiltRoof.gable_walls`; external `BuiltRoof` literals must initialize this derived
collection, usually empty. It introduces no native source field.

Rollback code by reverting the feature commit. Use Undo or remove the explicit caps in a
supporting version to return a project to an ordinary pitched roof; dependent openings
must continue to pass the resulting host checks.
