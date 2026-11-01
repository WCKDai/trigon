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
