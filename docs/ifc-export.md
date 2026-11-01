# IFC export

`export.ifc` writes the model as an IFC4 (ADD2 TC1) file of building elements that BIM tools such
as Revit and Vectorworks import as walls, windows, doors, floors, a roof, stairs, rooms and
balconies, not as anonymous shapes. The writer (`crates/io/src/ifc/`) is written by hand from the
public buildingSMART specification (the IFC4 ADD2 TC1 EXPRESS schema and property set
definitions); no other implementation is linked or read.

```sh
trigon run --doc model.trigon export.ifc '{"path": "out/model.ifc"}'
trigon run --doc model.trigon export.ifc '{"path": "out/forslag.ifc", "phase": "forslag", "filter": "all"}'
```

`phase` and `filter` (complete, demolition, new, all) choose the view, as for plans and glTF.

A made-up sample is in [`examples/sample-building.ifc`](../examples/sample-building.ifc): an L-shaped
block of three storeys on a 300 mm raised ground floor, masonry stepping from 480 to 360 mm, a fire
gable, a 40° roof with two gables, a U-turn staircase with stairwell windows across the floors, a
steel balcony with tie rods on two floors (new in the phase "forslag"), a cantilever balcony and a
French balcony, interior walls with doors, a 500 mm plinth, a rendered courtyard facade, rendered
bands at the floors of the street front, a roof build-up (250 mm, 300 mm overhang, 100 mm verges)
with gutters, and rooms. Regenerate it with
`TRIGON_WRITE_SAMPLE=1 cargo test -p trigon-io --test sample_building`.

## The file

| | |
|---|---|
| Schema | `IFC4`, header `ViewDefinition [DesignTransferView_V1.0]`: openings are voids the importer subtracts, and walls carry material layer set usages, which is Design Transfer View behaviour. The geometry stays within what Reference View readers handle (extrusions and triangulated face sets, no B-reps, no booleans other than opening voids) |
| Units | Lengths in millimetres, areas m², volumes m³, angles radians |
| Coordinates | Site-local, Z up; `IfcMapConversion` to `IfcProjectedCRS` EPSG:25832 (ETRS89 / UTM 32N, DVR90 heights) with the site origin's easting, northing and terrain height; scale 0.001 (mm to m) |
| Spatial structure | `IfcProject` → `IfcSite` → `IfcBuilding` → `IfcBuildingStorey` per storey, placed at its floor level (`Elevation` is the same). Rooms are `IfcSpace`s aggregated to their storey |
| GlobalIds | Derived from Trigon ids (`trigon:<id>`), so an element keeps its GlobalId from one export to the next |
| Phases | With more than one phase every element's `Pset_*Common` has `Status` (`IfcPropertyEnumeratedValue` of `PEnum_ElementStatus`: NEW, EXISTING, DEMOLISH). Walls, floors, roof, stairs and rooms aren't phased in Trigon, so they are EXISTING |

## What maps to what

| Trigon | IFC | Geometry and placement | Type, material, properties |
|---|---|---|---|
| Facade wall, one piece per storey (`<facade>.L<level>`) | `IfcWallStandardCase` .SOLIDWALL. | Placed at the outer face's start at the bottom of the piece, x along the facade, y into the building. `Axis` `Curve2D`: the outer face (the reference line), (0,0)–(length,0). `Body` `SweptSolid`: the plan outline (mitred at corners, stepping inside at every floor) extruded up the piece's height | `IfcWallType` per build-up and finish, e.g. "Ydervæg, massiv mur 2½ sten, 600 mm" = brick 585 + lime plaster 15; "Ydervæg, pudset mur …" = render 15 + brick + plaster 15; "Ydervæg, massiv mur, røde sten …"; a party wall is "Brandgavl, …". The body has the finish's colour. `IfcMaterialLayerSetUsage` AXIS2, POSITIVE, offset 0 (layers from the outer face in). `Pset_WallCommon` (Reference = type name, IsExternal, LoadBearing), `Qto_WallBaseQuantities` (Length, Width, Height, footprint, side and volume quantities, net of openings) |
| Facade piece that isn't a straight band (a wedge where a short jog collapses the inner line) | `IfcWall` | As above, the plan outline as it is | Same type; the material layer set itself (no usage) |
| Interior wall per storey (`<id>.L<level>`) | `IfcWallStandardCase` .SOLIDWALL. (load-bearing) or .PARTITIONING. | Placed at the joined centre line's start, x along it. `Axis`: the centre line. `Body`: the joined outline (ends cut on the faces they meet) | "Bærende indervæg, massiv mur 1 sten, 240 mm" (plaster 10 + brick + plaster 10) or "Let skillevæg, 100 mm" (gypsum 13 + studs and mineral wool + gypsum 13). Usage AXIS2, POSITIVE, offset −thickness/2 |
| Wall joins | `IfcRelConnectsPathElements` | Facade pieces at each footprint corner (ATEND–ATSTART); an interior wall end on a facade (ATSTART/ATEND–ATPATH) or on another wall (ATPATH in its middle, ATSTART/ATEND at its ends) | |
| Opening | `IfcOpeningElement` .OPENING. + `IfcRelVoidsElement` | One per wall piece the opening cuts (a stairwell window across a floor cuts two pieces), placed in the piece at the opening's outer bottom-left corner seen from outside; a box as wide and high as the opening within that piece, through the piece's thickness | |
| Window, door | `IfcWindow`, `IfcDoor` + `IfcRelFillsElement` | Placed relative to its opening (`PlacementRelTo` = the opening's placement), origin at the opening's outer bottom-left corner, x along the facade, z up, +y into the building. `OverallWidth`, `OverallHeight`. `Body` `Tessellation`: frame and sashes, leaves, glass and sill as separately coloured face sets | `IfcWindowType` (partitioning SINGLE/DOUBLE/TRIPLE_PANEL_VERTICAL) or `IfcDoorType` (operation) per Trigon type. `Pset_WindowCommon`/`Pset_DoorCommon` (Reference = type name, IsExternal, Status) |
| Floor (`<building>.slab.L<n>`, attic floor) | `IfcSlab` .FLOOR. (.BASESLAB. on the ground) | Extruded from the floor's outline; stair openings are inner loops of the profile (`IfcArbitraryProfileDefWithVoids`) | "Etagedæk, træbjælkelag 200 mm" (ceiling plaster 20, joists with pugging, floorboards 22) or "Terrændæk, beton 200 mm"; usage AXIS3, POSITIVE (from the underside up). `Pset_SlabCommon`, `Qto_SlabBaseQuantities` |
| Pitched roof | `IfcRoof` (GABLE_ROOF or HIP_ROOF) aggregating `IfcSlab` .ROOF. per roof plane | Each plane placed in its own plane (z up its normal, x level along it). With a build-up (`thickness_mm`, overhang, verge) the plane is the solid between its soffit and its top face, which lies straight above it (the fascia and verge boards are vertical): a closed triangulated solid, because an extrusion that isn't square to its profile is misread by some readers (bimifc 0.1.0 put it metres off; ifc-lite and IfcOpenShell read it right). A bare roof surface gets a 250 mm slab hanging square below it, as an extrusion. A plane next to a gable, warped where the ridge was slid onto the gable, is always a triangulated solid that follows Trigon's surface | "Tag, tegl på spær 250 mm" (rafters 200, tiles on battens 50) at the roof's thickness; usage AXIS3, POSITIVE (from the soffit up). `Pset_SlabCommon.PitchAngle` |
| Gutters | `IfcPipeSegment` .GUTTER. 'Tagrende' per eave | Triangulated, in the top storey | Zinc |
| Gable | `IfcWall` .SOLIDWALL. | The triangle above the eaves in the facade's plane, extruded into the building by the top storey's wall thickness | The facade's wall type |
| Flat roof | `IfcRoof` .FLAT_ROOF. aggregating one `IfcSlab` .ROOF. | Extruded outline | "Tagdæk, fladt 300 mm" |
| Staircase per storey (`<stair>.L<level>`) | `IfcStair` (STRAIGHT_RUN, HALF_TURN, HALF_WINDING, TWO_QUARTER_WINDING) aggregating `IfcStairFlight` (.STRAIGHT. / .WINDER.) and the half landing as `IfcSlab` .LANDING. | The steps of each flight and the landing as triangulated solids | `Pset_StairCommon` (NumberOfRiser, NumberOfTreads, RiserHeight, TreadLength) on the stair; each flight has its own counts as attributes (`NumberOfRisers`, `NumberOfTreads`, `RiserHeight`, `TreadLength`) and in `Pset_StairFlightCommon` |
| Room per storey | `IfcSpace` .INTERNAL. | The room polygon extruded from the floor to the underside of the floor above | Name = number `<storey index>.<nn>` (Revit's room number), LongName = the room's name ("Rum" when unnamed), Description = Trigon id. `Qto_SpaceBaseQuantities` (Height, GrossPerimeter, Gross/NetFloorArea, volumes) |
| Balcony (slab balcony, cantilever balcony with its door, MinAltan steel balcony) | `IfcElementAssembly` .USERDEFINED. 'Altan' aggregating its parts | Parts: deck as `IfcSlab` .USERDEFINED. 'Altandæk', railing as `IfcRailing` .GUARDRAIL., the steel balcony's UPE 160 frame as four `IfcBeam`s and its tie rods as `IfcMember`s 'Trækstang'. Placed on the facade at the balcony's lower left corner | `Trigon_Balcony` (TypeName, TypeId, Width, Depth, RailingHeight, and for steel balconies Railing, Finish, Deck, Handrail, Screens, TieRods, Status); each part its `Pset_*Common` and material |
| French balcony rail | `IfcRailing` .GUARDRAIL. | Triangulated | `Pset_RailingCommon` (Height) |
| Door in an interior wall | `IfcOpeningElement` through the wall + `IfcDoor` (`IfcDoorType` 'door.interior') | The opening placed at the door's bottom corner on the wall's right side, through the wall; the door (frame and closed leaf) placed in it | `Pset_DoorCommon` IsExternal false; the wall's NetVolume and NetSideArea are net of its doors |
| Plinth (sokkel) | `IfcCovering` .CLADDING. 'Sokkel' per facade + `IfcRelCoversBldgElements` to the facade's lowest piece | Triangulated, in front of the wall's foot, with the facade's openings cut through it | Rendered plinth material, `Pset_CoveringCommon` |
| Rendered bands | `IfcCovering` .MOLDING. 'Bånd' per facade, covering the piece where the lowest band runs | Triangulated, proud of the wall, openings cut through | Facade render |
| Cellar stair (kældernedgang) | `IfcStair` .STRAIGHT_RUN_STAIR. 'Kældernedgang' aggregating an `IfcStairFlight` (one riser per step), the pit's retaining walls as `IfcWall` .USERDEFINED. 'Støttemur' and the kerb railing as `IfcRailing` | Triangulated, along the facade; its door is an ordinary opening and door in the basement wall | `Pset_StairCommon`, `Qto_WallBaseQuantities` volumes for the retaining walls |
| Stair balustrade (U-turn stairs) | `IfcRailing` .BALUSTRADE. 'Trappeværn', part of the stair | Triangulated | `Pset_RailingCommon` |

Why these choices:

- **Stair openings in floors are profile voids**, not opening elements: Trigon derives the floor's
  shape with the stair shaft cut out, and where a shaft touches the walls the outline itself takes a
  notch, which only a profile can express. Every reader then sees the same slab without a boolean
  (one of our two test readers doesn't apply `IfcRelVoidsElement`), and Revit and Vectorworks read
  the inner loops as holes in the floor.
- **Balconies are `IfcElementAssembly`**: IFC4 has no balcony entity. An assembly keeps each part in
  its own class (floor, railing, structural framing on import) while the assembly carries the
  balcony's type and dimensions; a proxy would lose the parts' classes.
- **Wall reference lines**: facade walls on the outer face, which stays flush from the basement to
  the eaves while the inside steps, so every storey's piece shares one line; interior walls on the
  centre line, as Trigon draws them.
- **Wall volumes** (`NetVolume`) are the exported solid's: plan outline × height less the openings
  through the wall. Wherever Trigon's own wall piece is a closed solid this is its volume (the tests
  check it); see the limits below for where it isn't.

### Roof fragments and solid coverage

Roof cuts can leave small positive triangular caps. The ordinary profile path still requires
three corners after the 0.01 mm cleanup, area greater than 1 mm² and depth greater than 1 mm.
When that path rejects a fragment, a bounded fallback writes its existing `plane_solid` as an
`IfcTriangulatedFaceSet`, without cleaning, retriangulating or enlarging its cap. The fallback
accepts only a single triangular source cap matching its three boundary vertices. Multiple
triangles need a separate proof of cap coverage: summing absolute triangle areas can hide
overlap or cancellation.

The fallback checks finite coordinates, a stable plane within 0.000001 mm, outward orientation,
positive volume and a closed shell after welding at the writer's 0.000001 mm coordinate
precision. Rounding must not invert or collapse a triangle or change volume by more than 0.1%.
Area and volume must also remain positive at the existing quantity precision (0.001 mm² and
1 mm³). Invalid, degenerate, unstable or unsupported fragments stay rejected and must be
reported as uncovered. This is not permission to discard them from the source-model count.
The slab keeps its source GlobalId, roof aggregation, `IfcSlab` .ROOF. class/type, tile style,
material layers, `A-Tag` layer, common properties and base quantities. Fallback quantities use
the uncleaned cap. Ordinary extrusions and existing non-square/warped tessellations retain their
previous path.

Solid coverage needs three separate checks: a source product exists in STEP, each reader builds
it with the expected bounds, and its actual triangles preserve a finite outward closed solid.
Zero reader failures or a successful schema check alone cannot detect a product omitted before
export. `ifc.check` reports matched-product bounds; it does not certify complete solid coverage.
The public regressions in `crates/io/src/ifc/tests/roof_fragments.rs` exercise small caps and a
long thin cap that loses a corner during profile cleanup, and check the retained semantics and
quantities. They also characterize a reader limitation: ifc-lite 27.1 drops the thin cap faces
below its approximately 0.015259 mm triangle-height cleanup grid. That fixture has a valid closed
IFC solid and a product in both readers, but its ifc-lite solid coverage is **incomplete**, even
though product counts and bounds agree. Bimifc retains its closed solid. Neither the source nor
the reader thresholds are changed to conceal that limitation.

### Window and door base quantities

Window and door occurrences carry `Qto_WindowBaseQuantities` or
`Qto_DoorBaseQuantities` through `IfcRelDefinesByProperties`, including interior and cellar
doors. The existing types, common property sets, opening/fill relationships, placements and
geometry are unchanged. Quantities follow the exported phase view, not all placements in the
document. Quantity-set and relationship GlobalIds are derived from the fill id and type and remain
stable when its dimensions change. Existing fill ids can be reused by phased replacements; a
deterministic occurrence ordinal prevents additional quantity-GUID collisions if both share an id
and type. For such indistinguishable replacements, identity across phase filters is not guaranteed;
the pre-existing product GlobalIds are unchanged by this increment.

- `Height` and `Width` are `IfcQuantityLength` values in millimetres, using the occurrence's
  `BuiltElement.height` and `BuiltElement.width`, including placement overrides. In these
  rectangular families the model builds the outer lining to those limits, also exported as
  `OverallHeight` and `OverallWidth`. IFC4 ADD2 TC1 defines these quantities as the total outer
  height and width of the lining, only for rectangular windows or doors; they are not clear
  passage dimensions or separately measured construction-opening dimensions.
- `Area` is an `IfcQuantityArea` in m². Rectangular families use `width × height / 1_000_000`;
  shaped families use the actual opening-profile area, which is smaller than the bounding
  rectangle for arched and round windows. For rectangular families it is the area
  enclosed by the outer lining, not glazing area, exposed frame-material area, mesh surface area,
  or the area of an individual storey's opening void when a window spans floors. A compound
  door/window is measured as the modeled unit, not just its door leaf.
- Loggias are omitted: their existing dimensions describe the facade hole, while the recessed
  door/window lining extends down to the floor. Treating that hole as lining quantities would
  misstate the IFC definitions. No lining depth is inferred from wall thickness, reveal or balcony
  depth. Optional `Perimeter` and manufacturer/performance data are outside this increment.

Sources are the public buildingSMART IFC4 ADD2 TC1 definitions for
[windows](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2_TC1/HTML/schema/ifcsharedbldgelements/qset/qto_windowbasequantities.htm)
and [doors](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2_TC1/HTML/schema/ifcsharedbldgelements/qset/qto_doorbasequantities.htm).
The exact release XMLs are also archived by buildingSMART:
[window quantities](https://github.com/buildingSMART/IFC4.x-development/blob/5d08761e63557bd3ded140139eede665ec31511c/reference_schemas/psd_IFC4_ADD2_TC1/Qto_WindowBaseQuantities.xml),
[door quantities](https://github.com/buildingSMART/IFC4.x-development/blob/5d08761e63557bd3ded140139eede665ec31511c/reference_schemas/psd_IFC4_ADD2_TC1/Qto_DoorBaseQuantities.xml).
These schedule inputs are not a claim of Revit/Vectorworks import certification or a vendor-tested
quantity takeoff; no vendor files are used in the regression fixtures.

## Materials and layers

Every material is an `IfcMaterial` with a colour (`IfcMaterialDefinitionRepresentation` with an
`IfcSurfaceStyle`), and every shape item is styled with the colour of what it is made of: brick,
plaster, gypsum, timber, concrete, roof tiles, window frames, glass (transparent), doors, sills,
painted and galvanised steel, decking, stairs, rooms (translucent).

Every shape is on an `IfcPresentationLayerAssignment` (Vectorworks turns these into classes):
`A-Ydervægge` (facades and gables), `A-Indervægge`, `A-Dæk` (floors), `A-Tag`, `A-Vinduer`,
`A-Døre`, `A-Trapper` (with balustrades), `A-Rum`, `A-Altaner` (balconies and French balcony rails), `A-Åbninger`
(opening elements).

## Checks

- `cargo test --workspace`: the sample building and an engine model with every element kind are read
  back with both readers (ifc-lite and bimifc): no failures, the readers agree, and every product's
  extent matches what Trigon built within 0.01 mm; GlobalIds are unique and stable; every facade
  piece's exported net volume equals Trigon's closed piece.
- `trigon run --doc model.trigon ifc.check '{"path": "out/model.ifc"}'` does the reader comparison on
  any export.
- IfcOpenShell, run as a separate program only (it is LGPL; Trigon never links or reads it):
  `uv run -q --with ifcopenshell --with pytest python -I tools/ifc_oracle.py out/model.ifc` reports
  schema and where-rule errors (`ifcopenshell.validate` with EXPRESS rules), whether a shape is built
  for every product, and each wall's volume from IfcOpenShell (openings cut) against its
  `NetVolume`. It exits 0 when there are no errors, every product has a shape and every wall is
  within 0.5 %. For the sample: 0 errors, 206 of 206 shapes, 34 walls within 3·10⁻¹¹, and each roof plane's
  volume equals its NetVolume.

## Limits

- Revit and Vectorworks are not run by the tests; the checklist below is for the owner.
- Window and door types carry no lining or panel property sets and no representation maps, so
  receiving tools show Trigon's geometry, not their own parametric windows. +y of a window is into
  the building whatever way its sashes open.
- A bare roof surface's planes hang square below it, so neighbouring planes overlap or leave small
  wedges at hips and ridges; the roof surface itself is exact. Built-up roofs follow Trigon's slab.
- Tiny fragments amplify reader coordinate precision: the Rust readers return f32 geometry, so
  sub-millimetre bounds agreement does not imply accurate volume for every narrow solid. Check
  volume independently at higher precision before using such fragments for takeoff. Quantity
  rounding itself is 0.001 mm² / 1 mm³; tiny nonzero quantities have limited relative precision.
- No space boundaries (`IfcRelSpaceBoundary`); rooms are volumes only.
- Wall build-ups (plaster thickness, floor and roof layers) are typical Copenhagen constructions,
  not surveyed ones; the total thickness is always the modelled one.
- Where Trigon's facade piece isn't closed (a corner whose inner line collapsed, e.g. a short facade
  between two thick walls) the export writes the piece's plan outline, and its extent can differ
  from Trigon's displayed wall there; `ifc.check` shows these pieces as the largest deviations.

## Manual check for the owner

Open `examples/sample-building.ifc` (and then a real export) and tick off:

**Revit 2024 or later**: File → Open → IFC (or Insert → Link IFC to keep it as a linked file).

1. It opens without errors. Levels "Stueetage", "1. sal" and "2. sal" at +300, +3300 and +6100 mm.
2. Walls are in the Walls category; selecting a facade wall shows the type "Ydervæg, massiv mur 2
   sten, 480 mm" on the ground floor and "… 1½ sten, 360 mm" above; the west wall is a "Brandgavl".
   The outer face is flush, the inner face steps at each floor. The IFC parameters show
   Pset_WallCommon (IsExternal, LoadBearing) and the base quantities.
3. Windows and doors sit in cut openings; their type names are Trigon's ("Casement window, 2
   sashes", …). The stairwell windows on the wing's east side cut through the floor line. Interior
   walls have 11 doors in cut openings.
4. The courtyard side of the main block is "Ydervæg, pudset mur …" in render white; the street
   front has rendered bands at the first and second floors.
5. Floors per storey; the first floor has the stair hole. The roof is one roof of four planes,
   250 mm thick, 300 mm past the walls at the eaves, with two gable walls and four gutters. A
   plinth runs along the foot of every facade.
6. Two stairs (ground to first, first to second floor), each two flights, a landing and a
   balustrade along the well; the stair's IFC parameters give NumberOfRiser and RiserHeight.
7. Rooms (with Open IFC or Link IFC, depending on the Revit version they come in as rooms or as
   generic elements): 14, numbered "0.01" … with names Butik, Lager, Kontor, Stue, Værelse, Køkken,
   Bad, Trapperum.
8. Balconies: on the street side two steel balconies, each an assembly of a deck, a railing, four
   beams and two tie rods (the parts should land in Floors, Railings and Structural Framing or
   Generic Models), Status NEW; at the back a cantilever balcony and a French balcony.
9. Colours: brick walls, white frames, transparent glass, red roof.
10. Manage → Coordinates → Report Shared Coordinates on the survey point (after Acquire Coordinates
   if needed): E 724 321.791, N 6 178 467.512 in EPSG:25832 for the sample.

**Vectorworks 2024 or later**: File → Import → Import IFC, with the option to convert IFC entities
to native Vectorworks objects where possible.

1. One story (design layer) per storey at the same elevations.
2. Classes `A-Ydervægge`, `A-Indervægge`, `A-Dæk`, `A-Tag`, `A-Vinduer`, `A-Døre`, `A-Trapper`,
   `A-Rum`, `A-Altaner`, `A-Åbninger`; turning a class off hides those elements.
3. Standard-case walls become Vectorworks walls with components (brick and plaster, or gypsum and
   studs) and the wall style named after the type; the wall's control line is the outer face for
   facades and the centre line for interior walls. Windows and doors sit in the walls.
4. Spaces become space objects with the room names and areas; slabs, roof faces and stairs come in
   as IFC objects with their data in the Object Info palette (Pset_*, Qto_*, Trigon_Balcony).
5. The georeference (Document georeferencing) is EPSG:25832 at the sample's easting and northing.

Report anything that comes in as a generic object instead of the element above, with the element's
GlobalId.
