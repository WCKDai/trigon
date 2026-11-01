# Parametric mansard roofs

`building.set_roof` accepts an opt-in `mansard` kind. Its lower and upper pitches,
horizontal break run, gables, eave straightening, overhang, verge and normal build-up
are source parameters. `element.get` and `element.set` expose those same values;
commands remain atomic, journaled and exactly undoable. Existing flat and pitched
roof geometry and defaults are unchanged.

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
