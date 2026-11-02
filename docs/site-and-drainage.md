# Parametric drainage and courtyard objects

Tracking: [Trigon #27](https://github.com/WCKDai/trigon/issues/27), using the
GitHub tracking exception recorded in #15 while Plane and the engineering kit are unavailable.

## Optional roof drainage detail

Existing `gutters: true` documents retain their original geometry and omitted fields. Set the
roof's `drainage` property with `element.set`, or supply `drainage` to `building.set_roof`, to opt
into half-round metal channels, rolled rims, mitred corners, terminal stop ends and sleeve joints. Roof cover,
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
`downpipes` list adds channels only. Adjacent eaves meet at a common mitre computed from their
actual directions, for convex and reentrant corners; no square-corner assumption is used. There
are no endcaps between connected runs. Gable terminations have full metal stop plates of
`wall_mm` thickness. Rolled rims are integrated into the channel section; separate closed sleeve
fittings have 0.25 mm clearance. Aperture edges close the sheet thickness.

Pipe knees and shoes use tangent circular elbows at the actual change in direction. Their
centreline radius is normally 1.5 pipe diameters; short legs reduce the radius while leaving 10%
straight clearance. Paths needing a radius no larger than the outside pipe radius plus wall
thickness are refused. The 32-sided pipes and at most 7.5-degree elbow samples stay below Studio's
20-degree feature-edge threshold. Two closed collars follow that same centreline at its thirds,
with 1 mm radial clearance, including short shoes. No internal knee endcaps are generated.

This intentionally corrects the opt-in detailed geometry. Serialized drainage parameters,
per-eave `building.roof.gutterN` IDs and explicit `building.roof.downpipe.ID` IDs are unchanged.
Individual eave meshes share open mitre boundaries; the assembled metal shell is closed and
oriented, without coincident joint caps. Reversing corners, mitres that consume a short run and
outlets crossing a mitre are refused rather than folded or silently resized. `drainage: null`
and omitted drainage still produce the exact legacy gutter geometry.

Hydraulic sizing, roof falls, underground drainage and general collision checking are not implemented.
These presentation details are not measured evidence or construction documentation.

Limits include 100–300 mm gutters, 1–5 mm walls, 1–10 m joint spacing, 256 eave corners,
64 explicitly named downpipes and 1024 joints. Pipe dimensions and facade distances are checked.
Commands validate before success, journal the change and support exact Undo.

IFC4 exports use `IfcPipeSegment` with `GUTTER` for channels and `RIGIDSEGMENT` for explicitly
positioned downpipe assemblies. These are tessellated geometric assemblies without hydraulic
ports. Their IDs are stable under dimension edits. See the public buildingSMART
[IfcPipeSegmentTypeEnum specification](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD2/HTML/schema/ifchvacdomain/lexical/ifcpipesegmenttypeenum.htm).

### Source ownership on selected eaves

Pitched roofs can opt into ordered source facade lines with `eave_facades`; see
[dormer-hosts.md](dormer-hosts.md#preserve-source-eave-directions-across-recesses).
Detailed drainage retains an explicit original source edge for every selected run. Each pipe's
`along_mm` remains anchored to its original facade, then projects to that facade's own parallel
eave at the declared overhang. No nearest-edge guessing or ring-position alias is used.
Absent selected hosts and gables are refused with the pipe/facade IDs and corrective feedback.
Coincident parallel lines cannot both be adjacent selected hosts; choose the source ownership
explicitly. All dimensional, mitre, outlet separation, closure and joint guards still apply.

For an opted-in ring, `building.roof.gutterN` uses the **original zero-based footprint edge** N,
so a cyclic change of the selector's starting facade preserves every run's ID and geometry.
These IDs may differ from legacy compact ring indices when first opting in. Downpipes retain
`building.roof.downpipe.ID`; roof, source-wall and cornice identities are unchanged. There is
one channel for every selected sloping run, with joints and shared mitres; no run is silently
omitted. The existing public drainage `build` API retains its signature, behavior and legacy
IDs; a separate selected-host helper validates explicit source ownership. `eave_facades: null`
continues down the exact legacy path.

A roof-bearing cornice on a selected source facade follows that source eave's soffit without
moving its source wall or band. A declared bearing cap on an unselected facade is refused with
its facade ID; revise the explicit cap/roof selection. Projection and minimum remaining cap
depth checks remain unchanged. Selecting eaves does not design cornice returns at gables.

### Seating an eave cornice

On an opted-in detailed pitched roof, a band whose top is exactly the wall bearing height gets
a bevelled upper face against the roof underside. Other bands, gable bands and legacy `None`
drainage retain their existing geometry. This changes neither the roof planes nor their bearing,
cover thickness, wall height, source band dimensions or material ownership. A cut view clips
the seated geometry rather than rebuilding a flat top at the cut.

Let `H` be the top storey's elevation plus height, `P` the projection, `a` the roof pitch, and `d`
distance outward from the wall. The cap top at the wall is `H`, and its top outward is
`H - d * tan(a)`. Set the band's storey-relative `top` to that storey's height. Its bottom and
any opening head must stay at least 1 mm below `H - P * tan(a)`; a practical cap has further
construction clearance. `P` must fit inside `overhang_mm`, and the facade must survive roof
straightening. Violations return an error rather than producing inverted or crossing faces.

For example, a 135 mm projection at 46 degrees needs more than 140.8 mm of cap depth at the
wall; 180 mm leaves about 40.2 mm at its outer face. Merely raising a 50 mm rectangular band to
`H` cannot fit. Place the lower stepped courses beneath this cap, retaining their own finishes.
The roof's underside already bears at `H`; its outer eave is at `H - overhang_mm * tan(a)` and
its cover rises vertically by `thickness_mm / cos(a)`. Do not add that cover lift to the cornice.

At an adjacent gable, the existing band mitre can project past the gable wall. A covered cornice
return requires sufficient `verge_mm` for that actual plan projection, or a separately designed
termination. This seating operation does not invent those returns or change gable topology.

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

Saved ground plans with an explicit view range instead cut/project the actual site meshes within
that height interval, retaining their source IDs for picking. Demolished site meshes form a red
overlay in demolition/all views and do not occlude retained geometry. Editing a site's lifetime
can hide it from the current view; the command still returns its stable source ID and supports
exact Undo. Upper-storey ranged plans omit site objects, matching the ordinary plan convention.

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
