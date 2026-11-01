# Explicit roof storeys

A pitched or mansard building may opt into one editable **last roof storey** with
`building.set_attic`. This represents a structural floor below the eaves and the knee wall
between that floor and the eaves. It does not establish occupancy approval, measured source
accuracy, room height, headroom or habitable area.

```json
{"command":"building.set_attic","params":{"building":"b1","floor":9450,"name":"3. sal / tagkvist","source":"Section drawing; scale-derived, not surveyed"}}
```

`floor` is a site-local elevation in millimetres. Creation appends a storey, shortens the last
full storey, and preserves the existing eaves, roof/dormer world geometry, lower floor levels,
storey names/indices, nominal facade placements and existing `.slab.L*` floor identities. The
historical `<building>.attic` floor moves from the eaves to this new floor; there is no second
slab at the eaves. Its thickness remains `attic_thickness_mm` (200 mm by default).

Full storeys retain the 2000–10000 mm floor-to-floor guard. Only the explicitly marked last
storey may use a 100–2000 mm knee wall on a pitched/mansard host. **A 750 mm knee wall is not a
750 mm room height.** No unmarked short storey is inferred to be a roof storey. Invalid positions,
shortened full storeys, ordinary openings/interior doors colliding with the floor, incompatible
hosts, nonfinite values and counts above 60 levels fail atomically. Explicit `spans_storeys`
stairwell openings keep their existing cross-floor meaning; their source must provide the stair
shaft/void rather than a fictitious automatic opening.

Facade-band occurrences in the affected upper levels are rehosted with adjusted relative
heights. A repeated definition may split so lower occurrences keep their original offsets.
World heights, finishes and projections survive creation, movement and removal; a cornice at
the eaves stays at the eaves. Rehosting that would exceed the existing band limits is rejected.

## Editing, removal and stairs

Run the same command with another `floor` to move the floor while keeping the eaves fixed;
omit `name` to retain the name. The `.attic` element exposes floor elevation and a property
labelled **Knee wall to eaves (not room height; keeps eaves fixed)**. Its existing
`storey_height_mm` property key edits the knee dimension through the same operation. Both
operations adjust the preceding full-storey height and preserve roof and band world heights.
Roof-level facade placements must be removed or explicitly rehosted before moving their
source floor; existing full-storey placements and independent dormers remain fixed.

```json
{"command":"building.set_attic","params":{"building":"b1","floor":9500}}
{"command":"building.set_attic","params":{"building":"b1","remove":true}}
```

Removal merges the knee wall back into the preceding full storey and restores the legacy eaves
slab. It rejects roof-level hosted placements, entrance links, saved/registered plans and group
occurrences that would become dangling. Remove those references explicitly first. Global saved
plan/group checks are deliberately conservative in multi-building documents. Undo/Redo restores
the complete source document and journal exactly. `source`/`attribution` are optional evidence;
they do not confer survey accuracy.

`building.set_storeys` and `building.set_levels` reject an active explicit roof level; remove it
before resetting the full stack. `building.set_roof` rejects a flat host while the marker is
active. `building.add_basement` shifts the marker and band references with the other storey
references. Editing other full-floor properties retains their existing scope; a full-storey
height change can still move the storeys and roof above it.

**No stair flights are copied or inferred by this command.** Existing explicit departure levels
remain the only source of stairs. A flight on the preceding full storey arrives at the new roof
floor and opens that slab using its existing source outline. Otherwise the new slab is solid.
Use `edit.copy_to_levels` on selected, evidenced stair occurrences or explicitly authored
`interior.add_stairs` definitions to continue a staircase. The ordinary floor/stair-hole overlap
checks still apply. Departures from within the roof level are unsupported.

## Inspection, drawings and exchange

`document.info` identifies the roof index, name, floor, knee dimension, slab ID and unsupported
interior quantities. Its roof-level row uses `knee_wall_mm`, not an apparent habitable `height`.
`full_storeys_above_ground` excludes basements and the roof level. Existing BBR attributes are
left unchanged; BBR full-storey counts are not incremented by the roof floor.

Studio's level selector, plans, sections and opening schedules consume the same storey names
and indices. When a shared index mixes a full storey and a roof level in different wings,
plan/browser labels show the distinct names together; per-building inspection remains authoritative
for elevation and role. Dormers keep their IDs and world geometry and are scheduled on the new roof
level. Sections show separate roof-floor and eaves datums. In an opt-in document, section
datums come from the wing(s) actually crossed by the finite cut line; labels distinguish multiple
cut wings. The first cut wing's ground floor is the common datum for the section. IFC uses the new floor elevation for
`IfcBuildingStorey`, contains `.attic` on that level, and retains the slab's GlobalId and real
stair voids. The storey description identifies the roof role and the interior limitation.
Structural slab footprint/net areas are slab quantities, not habitable room areas.

Marked roof levels use the existing mesh-based plan-range renderer automatically in Studio and
`plan.sheet`, with `top_mm:2300`, `cut_mm:1200`, `bottom_mm:0`, `depth_mm:-1000`. This cuts the
actual roof/dormer geometry instead of producing an empty symbolic plan above the knee walls.
Unmarked floors retain their legacy plan output. Use a saved plan view/template for an explicit
range or crop; rotating a marked roof plan and the symbolic `plan.image`/scan-overlay path are
unsupported and fail explicitly. A plan range is a world-space cut based on the first building
at the selected index; buildings with different floor elevations need separate review/crops.
A shared numeric index is not an assertion that separate wings have the same floor elevation or
name.

## Bounded interior and compatibility limits

Roof-clipped interior walls, rooms, room detection, headroom/habitable-area calculations and
stairs departing within the roof are not implemented. Commands reject those sources on a roof
level, including copies and tracing paths. The model builder rejects externally supplied
unsupported roof interiors, and IFC does not fabricate roof spaces from the knee dimension.
This prevents fictitious 550 mm high attic interiors and room volumes for a 750 mm knee with a
200 mm slab. Interior rooms on full storeys below the new floor remain supported.

The serialized opt-in marker is `Storey.roof_level: true`. It is omitted when false, so old
unmarked files retain their exact serialization/default geometry/export behavior. New files
round-trip the explicit role. **Older readers that ignore unknown fields cannot safely interpret
feature-bearing files.** Native file-generation/version protection is separate integration work;
these files must not be published to old readers until that protection is reviewed. This feature
makes no native-client, Revit, full 360-degree or overall drawing-quality acceptance claim.
