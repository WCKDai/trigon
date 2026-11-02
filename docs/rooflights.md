# Inclined rooflights

`rooflight.add` and `rooflight.remove` use the same journalled commands in Studio, CLI and MCP.
They also work inside `precision.preview` / `precision.apply`, so a proposed placement can be
validated before committing it. A rejected placement leaves the source and Undo/Redo unchanged.

[`examples/rooflights.json`](../examples/rooflights.json) is a synthetic command recipe.
All lengths are millimetres in the project's local frame. `u_mm` is the centre along the
original source facade; `run_mm` is the horizontal inward run to the lower frame edge.
`width_mm` and `height_mm` describe the aperture in the inclined roof plane. `stand_off_mm`
is normal to the finished roof skin; `flashing_mm` is the surrounding roof-plane width.
Optional `yaw_deg` defaults to zero and rotates the source facade frame within ±45 degrees.

The window family must be rectangular `Window` or `FixedWindow`. Its lights and transom
controls are reused; source dimensions on the instance determine the frame's size. A pitched
or mansard host is required. The complete outer flashing must fit one supported roof plane,
with at least 50 mm of normal build-up. Hip and slope crossings, overlapping rooflights,
dormer intersections, duplicate IDs and hostile parameters are rejected. The limit is 64
rooflights per building and 64 source footprint corners.

Support and carving are restricted to the selected top and underside planes. The opposite
slope stays intact even when it overlaps the aperture in a steep roof's normal projection;
coverage tolerances and hip/slope-break rejection remain unchanged.

The stable built IDs are `building.rooflight.local-id`, with `.opening` and `.flashing`
children. `element.get` and `element.set` expose the source placement and family parameters
from any of these parts. Studio selection and Properties use those commands. Opening
schedules include the inclined aperture dimensions, area and roof host. Drawing line
ownership uses the same editable instance ID as 3D selection. IFC exports a window, its
opening and host relationship; GLB preserves the source product identities.

The `.opening` ID returned by `rooflight.add` is also an authoring alias. Editing it changes
the coordinated rooflight source, so the void, frame and flashing regenerate together.

## Compatibility and verification limits

The native `rooflights` lists are optional on pitched and mansard roofs. Empty lists are
omitted; projects without rooflights retain their existing source and geometry. Rust callers
constructing public roof literals must supply the new list (`Vec::new()` preserves the old
behaviour); `BuiltRoof` also gains a rooflight list. `Scene::from_built` remains unchanged.
Use this feature with a reader containing rooflight support. Older readers can discard fields;
keep an independent pre-feature project copy before trying older builds. No file-format
generation or native-writer migration is activated by this change.

These are parametric design assemblies, not manufacturer-certified products. A successful
geometric or IFC check does not establish current building conditions, watertight construction,
thermal performance, an editable Revit family, or native GPU acceptance. Actual receiving-app
review remains necessary.
