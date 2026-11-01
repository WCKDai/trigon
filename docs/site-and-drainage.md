# Parametric drainage and courtyard objects

Tracking: [Trigon #27](https://github.com/WCKDai/trigon/issues/27), using the
GitHub tracking exception recorded in #15 while Plane and the engineering kit are unavailable.

## Optional roof drainage detail

Existing `gutters: true` documents retain their original geometry and omitted fields. Set the
roof's `drainage` property with `element.set`, or supply `drainage` to `building.set_roof`, to opt
into half-round metal channels, rolled rims, capped run ends and sleeve joints. Roof cover,
gables, building source and roof identity stay unchanged. `null` restores legacy gutters.

```json
{"command":"element.set","params":{"id":"b1.roof","properties":{"drainage":{
  "gutter_width_mm":150,"wall_mm":2,"joint_spacing_mm":3000,"downpipes":[
    {"id":"rain-a","facade":"b1.f1","along_mm":2500,"diameter_mm":80,
     "wall_offset_mm":100,"bottom_mm":150,"shoe_mm":200}
  ]
}}}}
```

The distances are explicit design inputs in millimetres. A downpipe needs an eave facade that
survives roof straightening. Its open channel aperture, outlet, offset knees, pipe and shoe are
one geometric assembly; no pipe is inferred from a photograph or placed automatically. An empty
`downpipes` list adds channels only. Adjacent gutter runs have capped ends; corner fittings,
hydraulic sizing, roof falls, underground drainage and collision checking are not implemented.
These presentation details are not measured evidence or construction documentation.

Limits include 100–300 mm gutters, 1–5 mm walls, 1–10 m joint spacing, 256 eave corners,
64 explicitly named downpipes and 1024 joints. Pipe dimensions and facade distances are checked.
Commands validate before success, journal the change and support exact Undo.

IFC4 exports use `IfcPipeSegment` with `GUTTER` for channels and `RIGIDSEGMENT` for explicitly
positioned downpipe assemblies. These are tessellated geometric assemblies without hydraulic
ports. Their IDs are stable under dimension edits. See the public buildingSMART
[IfcPipeSegmentTypeEnum specification](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2/HTML/schema/ifchvacdomain/lexical/ifcpipesegmenttypeenum.htm).

## Courtyard/site sources

`Document.site_objects` is an optional source list, omitted when empty. The legacy building
build result is unchanged. `trigon_model::site::build` builds these objects separately, including
site-only documents. Studio's `Scene::from_built(built, phases)` signature is unchanged;
`with_site` adds real pickable meshes with the same source IDs, inheriting that exact phase-color boolean. Presentation ground plans use material colors; check plans retain phase colors.

- `ground`: simple unclosed polygon, elevation, thickness and paving/gravel/grass surface.
- `tree`: position, height, crown radius and trunk radius; a generic presentation tree.
- `bench`: position, length, depth, seat height and rotation; original timber/steel geometry.
- `planter`: position, length, width, height, wall thickness and rotation; hollow shell and soil.

All sources require an explicit `intent` (`existing` or `proposal`) independent of their phase
status. They have optional `created`/`demolished` phase IDs. `site.add` defaults creation to the
active phase; `site.set` replaces a typed source while preserving its ID; `site.delete` removes
it from all phases. Each mutation is atomic, journaled and undoable. `site.list` reports source
parameters. Displayed objects support `element.get`/`element.set` and Studio Properties, including
position handles and tree height/radius handles. Ground polygons are edited as exact coordinates.

```json
{"command":"site.add","params":{"object":{
  "id":"site.tree-a","name":"Proposed courtyard tree","intent":"proposal",
  "geometry":{"kind":"tree","position_mm":[-5000,-4000,0],
    "height_mm":4500,"crown_radius_mm":1700,"trunk_radius_mm":100}
}}}
```

Ground plans draw and pick the actual parametric footprints, including canopy symbols; upper
storey plans, facades and sections do not display or pick site objects. Auto-fit includes only
phase-visible site footprints in the ground plan. Explicit regions retain their crop. Site objects
do not enter window/door or room schedules. This is a flat site vocabulary, without surveyed
terrain, site boundary assertions, grading, species data, collision analysis or accessibility checks.

Positions and resulting extents must be finite, within 100 km of the site origin horizontally
and -50–500 m vertically. Simple ground polygons require 3–256 vertices, 0.01–1,000,000 m² area,
1 mm minimum edges and 10–1000 mm thickness. Crossing/touching edges are rejected, never cleaned
or silently moved. A document is limited to 512 objects and 8192 ground vertices. Trees have
1–20 m height, bounded crown/trunk radii; furniture dimensions and rotations are bounded.

`export.gltf` includes these independent meshes and exact source JSON in node `extras`.
`export.ifc` spatially contains explicitly labelled `IfcBuildingElementProxy` objects in `IfcSite`,
using `USERDEFINED` with an ObjectType. This is an honest exchange proxy classification, with
phase status, design intent and source JSON in `Trigon_SiteObject`; it is not a formal landscape
entity or engineering quantity. Sources use `IfcText` rather than a length-limited `IfcLabel`.
See the public [IFC4 proxy specification](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/FINAL/HTML/schema/ifcsharedbldgelements/lexical/ifcbuildingelementproxy.htm).

The IO `write` functions retain their building-only interfaces. Call `glb::write_with_site` and
`ifc::write_with_site` with the separately built site view when exporting both. Existing material
indices are preserved; seven site materials are appended. Empty site exports retain the original
building material list. The public-safe fixture is `crates/model/tests/fixtures/courtyard.trigon`.
Private photographs, reference PDFs, project models and proposal commands stay in gitignored `out`.
