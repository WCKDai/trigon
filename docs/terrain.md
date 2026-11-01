# Ground and terrain

Terrain is an explicit parametric height grid in site-local millimetres, Z up. It is absent
by default. A flat 2 × 2 grid gives a groundplane; varied node heights give a triangulated
surface. Rows run south to north and columns west to east. Adjacent cells share their sample
heights, with a deterministic south-west to north-east diagonal.

```json
{"command":"site.set_terrain","params":{"terrain":{
  "origin":[-10000,-10000],"spacing_mm":[60000,40000],
  "heights_mm":[[0,0],[0,0]],"source":"illustrative:flat-ground"
}}}
```

This example is an unmeasured presentation assumption. Use `site.set_terrain {"terrain":null}`
to remove it; an omitted field is refused. Changes are atomic, journaled and undoable. A building
must exist. Grids are limited to 2–65 rows and columns, spacing 100–200000 mm, bounded finite
site coordinates and heights. Missing/ragged/nonfinite input is rejected.

After `site.load_address`, an existing configured Dataforsyningen key permits local DTM sampling:

```json
{"command":"site.sample_terrain","params":{
  "origin":[-40000,-40000],"spacing_mm":[2000,2000],"columns":41,"rows":41
}}
```

The command requests `dhm_terraen` through the existing WCS provider. Raster pixel centres
coincide with the requested grid nodes. It reverses north-first GeoTIFF scanlines, subtracts
`Site::elevation` (DVR90 metres) and converts metres to mm. It stores the source ID and required
attribution, never a credential-bearing URL. Missing samples fail the command, preserving the
previous model. DTM sampling is a terrain data source, not a measured site survey.

Building footprints trim the terrain so the surface cannot occupy floor slabs. Trimmed points
remain on the original grid triangle plane. The surface is open, with no invented underground
solid or retaining construction. It appears as one `site.terrain` element in Studio and GLB,
and as an [IFC4 IfcGeographicElement](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/ADD1/HTML/schema/ifcproductextension/lexical/ifcgeographicelement.htm)
of predefined type `TERRAIN` in IFC. It is static across phase views; grading phases and 2D
contour annotations are not implemented. Neutral ground colour does not assert a soil/paving
specification. The terrain command edits the grid; individual mesh triangles are not editable
source objects.

Old `trigon/1` documents without terrain load unchanged. The optional field is omitted when
absent, and the new ground material is appended after existing material indices. Use the
updated application to save terrain documents: older applications may ignore this new field.
