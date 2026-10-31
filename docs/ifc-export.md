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
French balcony, interior walls and rooms. Regenerate it with
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
| Facade wall, one piece per storey (`<facade>.L<level>`) | `IfcWallStandardCase` .SOLIDWALL. | Placed at the outer face's start at the bottom of the piece, x along the facade, y into the building. `Axis` `Curve2D`: the outer face (the reference line), (0,0)–(length,0). `Body` `SweptSolid`: the plan outline (mitred at corners, stepping inside at every floor) extruded up the piece's height | `IfcWallType` per build-up, e.g. "Ydervæg, massiv mur 2½ sten, 600 mm" = brick 585 + lime plaster 15; a party wall is "Brandgavl, …". `IfcMaterialLayerSetUsage` AXIS2, POSITIVE, offset 0 (layers from the outer face in). `Pset_WallCommon` (Reference = type name, IsExternal, LoadBearing), `Qto_WallBaseQuantities` (Length, Width, Height, footprint, side and volume quantities, net of openings) |
| Facade piece that isn't a straight band (a wedge where a short jog collapses the inner line) | `IfcWall` | As above, the plan outline as it is | Same type; the material layer set itself (no usage) |
| Interior wall per storey (`<id>.L<level>`) | `IfcWallStandardCase` .SOLIDWALL. (load-bearing) or .PARTITIONING. | Placed at the joined centre line's start, x along it. `Axis`: the centre line. `Body`: the joined outline (ends cut on the faces they meet) | "Bærende indervæg, massiv mur 1 sten, 240 mm" (plaster 10 + brick + plaster 10) or "Let skillevæg, 100 mm" (gypsum 13 + studs and mineral wool + gypsum 13). Usage AXIS2, POSITIVE, offset −thickness/2 |
| Wall joins | `IfcRelConnectsPathElements` | Facade pieces at each footprint corner (ATEND–ATSTART); an interior wall end on a facade (ATSTART/ATEND–ATPATH) or on another wall (ATPATH in its middle, ATSTART/ATEND at its ends) | |
| Opening | `IfcOpeningElement` .OPENING. + `IfcRelVoidsElement` | One per wall piece the opening cuts (a stairwell window across a floor cuts two pieces), placed in the piece at the opening's outer bottom-left corner seen from outside; a box as wide and high as the opening within that piece, through the piece's thickness | |
| Window, door | `IfcWindow`, `IfcDoor` + `IfcRelFillsElement` | Placed relative to its opening (`PlacementRelTo` = the opening's placement), origin at the opening's outer bottom-left corner, x along the facade, z up, +y into the building. `OverallWidth`, `OverallHeight`. `Body` `Tessellation`: frame and sashes, leaves, glass and sill as separately coloured face sets | `IfcWindowType` (partitioning SINGLE/DOUBLE/TRIPLE_PANEL_VERTICAL) or `IfcDoorType` (operation) per Trigon type. `Pset_WindowCommon`/`Pset_DoorCommon` (Reference = type name, IsExternal, Status) |
| Floor (`<building>.slab.L<n>`, attic floor) | `IfcSlab` .FLOOR. (.BASESLAB. on the ground) | Extruded from the floor's outline; stair openings are inner loops of the profile (`IfcArbitraryProfileDefWithVoids`) | "Etagedæk, træbjælkelag 200 mm" (ceiling plaster 20, joists with pugging, floorboards 22) or "Terrændæk, beton 200 mm"; usage AXIS3, POSITIVE (from the underside up). `Pset_SlabCommon`, `Qto_SlabBaseQuantities` |
| Pitched roof | `IfcRoof` (GABLE_ROOF or HIP_ROOF) aggregating `IfcSlab` .ROOF. per roof plane | Each plane placed in its own plane (z up its normal, x level along it) and extruded 250 mm down from the roof surface. A plane next to a gable, warped where the ridge was slid onto the gable, is a closed triangulated solid that follows Trigon's surface | "Tag, tegl på spær 250 mm" (tiles on battens 50, rafters 200); usage AXIS3, NEGATIVE (from the surface down). `Pset_SlabCommon.PitchAngle` |
| Gable | `IfcWall` .SOLIDWALL. | The triangle above the eaves in the facade's plane, extruded into the building by the top storey's wall thickness | The facade's wall type |
| Flat roof | `IfcRoof` .FLAT_ROOF. aggregating one `IfcSlab` .ROOF. | Extruded outline | "Tagdæk, fladt 300 mm" |
| Staircase per storey (`<stair>.L<level>`) | `IfcStair` (STRAIGHT_RUN, HALF_TURN, HALF_WINDING, TWO_QUARTER_WINDING) aggregating `IfcStairFlight` (.STRAIGHT. / .WINDER.) and the half landing as `IfcSlab` .LANDING. | The steps of each flight and the landing as triangulated solids | `Pset_StairCommon` (NumberOfRiser, NumberOfTreads, RiserHeight, TreadLength) on the stair; each flight has its own counts as attributes (`NumberOfRisers`, `NumberOfTreads`, `RiserHeight`, `TreadLength`) and in `Pset_StairFlightCommon` |
| Room per storey | `IfcSpace` .INTERNAL. | The room polygon extruded from the floor to the underside of the floor above | Name = number `<storey index>.<nn>` (Revit's room number), LongName = the room's name ("Rum" when unnamed), Description = Trigon id. `Qto_SpaceBaseQuantities` (Height, GrossPerimeter, Gross/NetFloorArea, volumes) |
| Balcony (slab balcony, cantilever balcony with its door, MinAltan steel balcony) | `IfcElementAssembly` .USERDEFINED. 'Altan' aggregating its parts | Parts: deck as `IfcSlab` .USERDEFINED. 'Altandæk', railing as `IfcRailing` .GUARDRAIL., the steel balcony's UPE 160 frame as four `IfcBeam`s and its tie rods as `IfcMember`s 'Trækstang'. Placed on the facade at the balcony's lower left corner | `Trigon_Balcony` (TypeName, TypeId, Width, Depth, RailingHeight, and for steel balconies Railing, Finish, Deck, Handrail, Screens, TieRods, Status); each part its `Pset_*Common` and material |
| French balcony rail | `IfcRailing` .GUARDRAIL. | Triangulated | `Pset_RailingCommon` (Height) |

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

## Materials and layers

Every material is an `IfcMaterial` with a colour (`IfcMaterialDefinitionRepresentation` with an
`IfcSurfaceStyle`), and every shape item is styled with the colour of what it is made of: brick,
plaster, gypsum, timber, concrete, roof tiles, window frames, glass (transparent), doors, sills,
painted and galvanised steel, decking, stairs, rooms (translucent).

Every shape is on an `IfcPresentationLayerAssignment` (Vectorworks turns these into classes):
`A-Ydervægge` (facades and gables), `A-Indervægge`, `A-Dæk` (floors), `A-Tag`, `A-Vinduer`,
`A-Døre`, `A-Trapper`, `A-Rum`, `A-Altaner` (balconies and French balcony rails), `A-Åbninger`
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
  within 0.5 %. For the sample: 0 errors, 171 of 171 shapes, 34 walls within 3·10⁻¹¹.

## Limits

- Revit and Vectorworks are not run by the tests; the checklist below is for the owner.
- Window and door types carry no lining or panel property sets and no representation maps, so
  receiving tools show Trigon's geometry, not their own parametric windows. +y of a window is into
  the building whatever way its sashes open.
- Roof planes are extruded square to their plane, so neighbouring planes overlap or leave small
  wedges at hips and ridges; the roof surface itself is exact.
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
   sashes", …). The stairwell windows on the wing's east side cut through the floor line.
4. Floors per storey; the first floor has the stair hole. The roof is one roof of four planes with
   two gable walls.
5. Two stairs (ground to first, first to second floor), each two flights and a landing; the stair's
   IFC parameters give NumberOfRiser and RiserHeight.
6. Rooms (with Open IFC or Link IFC, depending on the Revit version they come in as rooms or as
   generic elements): 14, numbered "0.01" … with names Butik, Lager, Kontor, Stue, Værelse, Køkken,
   Bad, Trapperum.
7. Balconies: on the street side two steel balconies, each an assembly of a deck, a railing, four
   beams and two tie rods (the parts should land in Floors, Railings and Structural Framing or
   Generic Models), Status NEW; at the back a cantilever balcony and a French balcony.
8. Colours: brick walls, white frames, transparent glass, red roof.
9. Manage → Coordinates → Report Shared Coordinates on the survey point (after Acquire Coordinates
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
