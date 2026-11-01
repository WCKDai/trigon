# Physically scaled masonry

`facade.set_finish` accepts `brick`, `red_brick`, `yellow_brick` and `render`. The yellow option
is explicit: existing `brick` defaults and old project files keep their previous meaning. Finish
changes are journaled and undoable and do not change wall geometry, openings or element IDs.
The new finish also has a named yellow-brick material in IFC. Projects that use `yellow_brick`
require this updated Trigon version; existing project files continue to load unchanged.

Studio and GLB use site-anchored material coordinates. A texture repeat covers 480 × 132 mm,
with two bricks and two staggered courses. The template uses a 228 × 54 mm face and assumes
12 mm joints. The brick face follows the [Danish normal format documented by Randers Tegl](https://www.randerstegl.dk/dk/mursten/produkt/rt483-classica-gammelroed).
The bond, joint size and colour are a presentation template; they are not a measured survey of
the existing building. Red/yellow brick classifications may be supported by photographs without
supporting a precise colour calibration or masonry-bond assignment.

The live shader varies brick tone and grain, softens joint boundaries and fades subpixel courses
to their area-weighted mean. Its UV coordinates use original site millimetres, so recentering the
rendering scene does not move the material. Selection and demolition overlays retain their
separate colours. Render, plaster, metal and glass do not receive a masonry pattern.

GLB embeds original, deterministically generated sRGB colour PNGs and a linear joint/bevel normal
map. Texture samplers repeat; no external asset paths or network requests are needed. Per-face
UVs, normals and tangents follow the [Khronos glTF 2.0 specification](https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html).
Vertices at a shared corner may be split for material coordinates, but the geometric triangle
positions, winding and semantic node names are preserved. No new masonry geometry is inferred.
If the infallible GLB writer cannot encode a colour PNG, that material keeps its flat-colour
fallback; failure of the optional normal PNG leaves the colour texture usable.

`png` is now a direct IO dependency for embedded image encoding; this version was already in the
locked dependency closure. No image from a manufacturer, private reference or commercial product
is distributed. The procedural texture source has the same licence as Trigon.

Verification covers physical scale, non-finite inputs, original project/material values, exact
triangle preservation, image decoding, repeating joints, normal/tangent frames, attribute counts,
embedded buffer bounds, unchanged glass blending, Undo/Redo and texture stability after a scene
origin change. Native GPU and independent importer checks are recorded with each implementation
increment; tests alone do not establish that a renderer displays every material correctly.
