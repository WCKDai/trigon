# Parametric mansard roofs

`building.set_roof` accepts an opt-in `mansard` kind. Its lower and upper pitches,
horizontal break run, gables, eave straightening, overhang, verge and normal build-up
are source parameters. `element.get` and `element.set` expose those same values;
commands remain atomic, journaled and exactly undoable. Existing flat and pitched
roof geometry and defaults are unchanged.

Mansard gables follow the explicitly assigned finish of their host facade in
Studio, drawings and GLB/FBX, matching the existing IFC material assignment.

For example:

```json
{"kind":"mansard","lower_pitch_deg":68,"upper_pitch_deg":25,"break_run_mm":1300,"thickness_mm":220,"overhang_mm":250,"gutters":true}
```

The break run is measured horizontally inward from the original wall line, on the
underside. Both outer slopes have the requested normal thickness. Their planes
intersect at a mitred break; the top break therefore differs from the underside's.
The shell includes soffits and edge fascias. Hipped, gabled, chamfered and reentrant
footprints use the same straight skeleton, with each face split before lifting.
A break that consumes the upper slope, or a thickness that consumes the lower
slope, is rejected. Geometry is deterministic, finite and bounded by the footprint.

Mansard gutters are open half-round 150 mm channels with 2 mm walls, rather than
solid rectangular bars. Each facade run has capped ends. Outlets, inter-run
connections and rainwater sizing are not inferred; those require explicit project
information. These and all roof dimensions above are **illustrative family
defaults**, not measured construction or drainage specifications.

The detailed `drainage` parameter remains supported on pitched roofs only. Supplying a
non-null drainage specification to a mansard command is rejected atomically; explicit
downpipes are never silently discarded.

GLB/FBX carry the actual shell. IFC exports it as a tessellated `IfcRoof` with
`MANSARD_ROOF` predefined type; its gable walls remain separate wall products.
This retains the mitred geometry rather than approximating it with overlapping
single-pitch extrusions. New roof parameters are additive to `trigon/1`;
older software that lacks the `mansard` enum cannot open that new variant.

The public `copenhagen-building` example exports an original apartment-block
study with historic joinery, a chamfered corner, stepped cornice bands and this
roof. It also verifies exact GLB restoration after a roof edit and Undo. The study
uses no client assets or network data.

Copenhagen's [roof-housing guidance](https://www.kk.dk/borger/bolig-og-byggeri/byggetilladelse/byggetilladelse-trin-for-trin/tagboliger)
provides architectural context: read the existing roof's form and building
character before choosing additions. It does not supply a universal family
section or make these illustrative defaults an approved project design.

## Shed dormers

The optional `dormers` array uses stable local IDs, a host facade ID, centre along
that facade, window type and nominal window width/height. The first family is a
shed dormer on one straight mansard face. It derives depth from the actual
intersection of the shed and host roof planes. Placements crossing a hip or
slope break, overlapping roof projections or consuming a supported roof face
are rejected. At most 16 dormers and 64 footprint corners are supported.

```json
{"id":"street-1","facade":"b.f2","u_mm":8000,"window_type":"dk.window.dannebrog","width_mm":1100,"height_mm":1200}
```

Defaults are a 200 mm front setback, 100 mm cheek/front construction, 150 mm
head, 80 mm sill clearance above the host roof at the back of the front lining,
and a 65 mm shed roof at 6°. The shed and cheeks underlap the host construction
by 70 mm; the shed projects 50 mm at its front and sides. These are original,
unmeasured family defaults. The IFC front uses an illustrative timber build-up,
not an inferred masonry wall.

The host shell has a real, capped opening behind the front lining. Its split
faces conform at every shared clipping station; the new glazing sits above the
roof and within the front construction. The front wall uses the window's actual
rectangular, curved or round profile. Cheeks and shed close the assembly and
underlap the host roof. Changing a window dimension recomputes that intersection;
there is no fixed floating box or viewport-only mask.

Studio, drawings, schedules, GLB and IFC share the same front and window identities.
Window, front, roof and cheek selections edit the same source dormer through
`element.set`. Roof properties also expose the full typed `dormers` array.
Shed roofs and cheek cladding remain separate IFC products; their windows use the
normal opening/fill relationships and actual dimensions. Dormers follow the
existing unphased roof source and are associated with the highest modelled storey;
this family does not infer an extra inhabited attic storey or attic room layout.

## Slate courses and tagpap

`covering` is optional and leaves the existing envelope when absent. Each slope
can use `plain`, `slate` or `felt`:

```json
{"covering":{"lower":{"material":"slate"},"upper":{"material":"felt"}}}
```

Slate defaults are 300 × 400 × 6 mm, a 160 mm course exposure and 3 mm side joints.
Courses stagger by half a slate. Their slope seats each course on the preceding
course throughout the lap; validation retains a double lap with at least 20 mm
headlap and at most three overlapping courses. Slate width, length, exposure,
thickness and joint width are typed parameters. Tagpap defaults are 1000 mm wide
sheets, 100 mm laps and 3 mm thickness, with a raised second layer at each lap.
These dimensions are original presentation defaults, not a Danish statutory
section, manufacturer's specification or surveyed roof.

Coverings use the actual roof faces, including dormer voids. They remain real
closed pieces in GLB/FBX and separate material-labelled `IfcCovering` products in
IFC. Dormer sill clearance is measured above the declared finished roof envelope;
changing lap thickness therefore recomputes its derived elevation and depth,
retaining its source ID and nominal window width/height. The structural roof
build-up remains the existing source envelope; covering geometry is additional.

The builder limits a roof to 20,000 candidate slates/laps and rejects larger
requests before generating them. Use a larger module or a plain surface for
larger context buildings. Ridge/valley flashings, battens, fixings, insulation,
ventilation and drainage design remain project-specific details; they are not
inferred by the surface family.
