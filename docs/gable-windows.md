# Attic gable windows

An explicitly placed rectangular attic window can cut a real masonry gable below a
`half_hipped` roof. It uses the ordinary facade placement, window family, lifetime and
stable slot identity. No window is inferred from a cap or photograph.

The bounded initial host is a strictly convex, counter-clockwise **four-source-edge**
footprint, with two opposite explicit gable facades and an explicit attic floor. A roof
with four selected eaves in a larger footprint can support caps but not these gable
windows. The type must be Window, FixedWindow or rectangular HistoricWindow, with no
storey spanning or party-wall placement. Low knee-wall openings retain their existing
construction. An above-eaves aperture must clear the eaves seam and the actual roof
underside by 150 mm; the underside is checked at front and back head corners through
the full source wall thickness. Unsupported hosts fail with source identity and numeric
clearance feedback rather than moving the opening.

`facade.place` now accepts optional `u`, a facade-local centre in model millimetres.
An explicit `u` requires exactly one explicitly selected level and bay. It overrides that
slot's centre without moving the facade's bay centres. Omission preserves the established
bay-based placement. The command is also a `precision.preview` / `precision.apply` action:

```json
{"action":{"command":"facade.place","params":{"facade":"b.f4","type":"window.casement.2","levels":[3],"bays":[0],"u":6000,"width":1200,"height":1000,"sill":2700}}}
```

This example assumes the [public synthetic setup](../examples/attic-gable-window.json).
Its dimensions are modelling inputs. `sill` is above the selected attic floor, not above
the eaves. Preview does not change the document; successful application is one undoable,
replayable journal entry. Failed host, range, seam and head checks preserve the document
and retained Redo. Existing phase replacement rules still apply.

The normal fill ID is `b.f4.L3.B0`; its host is `b.f4.gable`. Both support element inspection
and Properties. Window position and dimensions use the existing handles. Gable thickness
edits affect the source facade at that roof level; endpoints, finish and bands are shared
with that facade. The shaped wall does not manufacture a rectangular storey segment.
Drawing picking, GLB, Studio scene and opening schedules consume the actual host and fill.

The model retains closed gross and opened masonry following the actual roof underside
over the source wall's mitred band. The old flat gable face is removed for that host;
other wall, roof and opening geometry is preserved. Up to 32 visible windows are supported
per gable. Phase overlays cut the union of their apertures while retaining individual fill
identities. This is geometry and source placement validation, not a structural assessment.

IFC exports one shaped `IfcWall`, normal `IfcOpeningElement` and `IfcWindow` occurrences,
void/fill relationships, source material layers and window typing. Wall gross/net volumes
are checked against the written meshes; side areas come from the gross profile and actual
net front. Uncertifiable quantities are omitted with explicit metadata. The source solid
remains authoritative when an independent reader retessellates it. At translated coordinates
the Rust readers' float grid introduces small geometric differences; tests separate that
reader allowance from exact source/written-quantity checks. Actual Revit family conversion
and native editing in Revit are not established by these checks.

`BuiltRoof.gable_walls` and `build::BuiltGable` are new public derived geometry interfaces.
Consumers constructing `BuiltRoof` literals must initialize `gable_walls` (empty for legacy
fixtures). `Built::exterior_walls()` includes these hosts once. Existing native placements
and their optional `u` field are reused; no native format or file-writer policy changes.
The containing `half_hipped` kind already requires a supporting reader. Keep a pre-feature
project copy for downgrade. Remove the placement or use Undo before reverting the feature;
no document migration is required.
