# Shaped balcony families

The additive `shaped_balcony` family makes a slab and inset guard from one convex
parametric plan. It is an attachment: one opening and one balcony can occupy the
same facade bay. Installing or editing this family uses the existing typed
`types.define`, `facade.place` and `element.set` commands, with atomic validation,
journaling and Undo.

`element.set` exposes width, guard height, base elevation, depth, slab thickness
and the typed `balcony_plan` object. Assembly-specific plan edits create a private
type variant and preserve the reusable library definition.

Example type definition:

```json
{"id":"original.bowed","name":"Original bowed balcony","width":3000,"height":1100,"sill":0,"kind":{"family":"shaped_balcony","depth":1500,"thickness":180,"plan":{"shape":"bowed","return_fraction":0.35}}}
```

Plans can be `bowed`, `chamfered` with `corner_mm`, or `polygon` with normalized
`points`. Polygon points start at `[-0.5,0]`, run clockwise around the front, and
end at `[0.5,0]`. X is a fraction of width; Y is a fraction of outward depth.
Only simple convex outlines of 3–32 points are accepted. Bowed fronts use 24
ellipse segments. The width range is 600–6000 mm; depth is 400–3000 mm. Very
narrow plans that cannot contain the guard are rejected.

The guard centreline sits 50 mm inside the slab edge. Half-plane clipping handles
consumed inset edges on elongated curved plans. Rails are continuous mitred
sweeps; posts, balusters and baseplates attach to the slab. The rear edge remains
open. The same outline supplies the plan drawing and its existing source region;
the picking algorithm is unchanged. Legacy rectangular family geometry is
unchanged.

Slab and guard retain separate, stable source-derived identities, phases and IFC
classes within an `IfcElementAssembly`. IFC also carries the parametric plan as
a full JSON `IfcText` type property, so custom outlines are not constrained by
the [255-character IfcLabel limit](https://standards.buildingsmart.org/IFC/RELEASE/IFC4/FINAL/HTML/schema/ifcmeasureresource/lexical/ifclabel.htm).
GLB/FBX retain the actual curved or faceted mesh. Width/depth
annotations are bounding dimensions, not an area or fabrication take-off.

These sections, baluster spacing, concrete thickness and attachment arrangement
are illustrative family defaults. They do not establish load capacity, verified
wall anchorage or statutory compliance. Project-specific engineering data must
be supplied explicitly. The public `copenhagen-building` example includes bowed
street balconies and chamfered courtyard balconies with matching French doors;
it uses no client assets.
