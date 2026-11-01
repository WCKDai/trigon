# Precise placement and review

Trigon coordinates are site-local millimetres stored as `f64`. Display rounding is not a
placement grid. Generated plans and 3D geometry use the same source parameters.

## Stair occurrences and entrances

`core.L1` selects one departing stair occurrence; `stair:core` selects the complete source.
Moving, aligning or changing a shared occurrence splits its source and leaves other floors
in place. A single physical entrance and its apartment records stay intact.

Entrance records retain the legacy `stair` default. The optional `stair_by_level` object maps
departure-storey indices to explicit source IDs after an occurrence is edited. Empty maps are
omitted. Plans use the current departure, or the previous departure when that incoming flight
is visible. Thus the highest edited flight also keeps its label on the receiving floor.
Copies do not duplicate entrances. Replacing an occupied destination remaps its association;
deletion prunes dangling associations. Clearing stairs clears associations, not entrances or
units. Adding a basement shifts override keys alongside stair storeys. Loading units again
replaces the entrance records and rebinds them using the register workflow.

Old documents need no migration. Feature-bearing files must be opened in this version or
newer: older readers can ignore the override map and lose individual-floor associations on
save. For downgrade, Undo the occurrence edits and save a separate copy. Do not infer lineage
from generated `.edit` source IDs.

Source association correctness does not establish physical accuracy. Distinct stairs on
adjacent departures may intentionally overlap. A scan-based relocation needs registration,
source evidence and an explicit reviewed correction; an apparent double symbol is not enough
to delete a staircase.

## Physical drawing scale

Configured plan views place generated balcony size lettering using glyph ink from the same
bundled fonts and SVG shaping used by Studio, PNG and PDF. The lettering retains at least
0.5 paper mm of visible clearance beyond its dimension pen and white outline at the final
view scale. A scale change therefore cannot compress that clearance to a fraction of a model
millimetre. Unconfigured legacy export layouts retain their existing fingerprints.

Font measurements are paper millimetres and include actual style and anchoring; wide and
narrow letters are not interchangeable. DXF consumers may substitute fonts, so these exact
ink measurements describe Trigon's shared SVG renderer.
