# Copenhagen joinery families

`types.install_copenhagen {}` explicitly installs an original, optional library. It is atomic,
journaled and undoable. Identical installed presets are retained; a customised conflicting ID is
refused. Existing document libraries and the starting types are unchanged.

The `historic_window` family supports rectangular, circular-segment and round profiles,
one to four lights, an optional top light and a pane grid. `muntins: "true_divided"` seats separate
panes in real glazing bars. `"applied"` retains a continuous pane with attached bars on both
faces (pynte-/energisprosser for presentation). They are different constructions. Round lights
are fixed and require equal nominal width and height. A `panel_door` has one or two leaves,
a recessed panel grid, and optional glazed upper rows.

The masonry void, stepped lining, sash, glazing and IFC opening use the same outline. Arches
use 24 circular-segment chords; circles use 48. Width and height remain the nominal bounding
rectangle, as do placement IDs and source edits. Opening schedules and IFC quantities use the
actual faceted outer-lining area for these new families. Legacy rectangular quantities remain
width × height. The reveal retains the existing family placement depth where the host can contain it.

These are family defaults, **not measured client joinery**: 46 mm lining sightline, 32 mm sash,
22 mm muntins, 2 mm sash gap, 6 mm illustrative glazing, seated gaskets, and the existing
sill/threshold and hardware defaults. Small cells adapt profiles to retain a positive opening.
Curved upper lights are fixed; straight rectangular lower lights may have casement hardware.
The presets include Dannebrog, crosspost, three-light, small-pane, applied-bar, segmental,
arched fixed and round windows, plus solid, glazed and double panel doors.

Run the public, offline gallery and exact edit/Undo export check:

```sh
CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=3 cargo run --locked -p trigon-engine \
  --example copenhagen-openings -- out/copenhagen-openings
```

New families are additive to `trigon/1`; older applications that do not recognise them cannot
open such a document. Keep a copy before editing with an older application. No migration or
automatic replacement of established families occurs.

## Design references

The municipal [Aarhus historicist style guide](https://aarhus.dk/borger/bolig-byggeri-og-miljoe/for-grundejere/kulturarv/stilguide/de-historicistiske-villaer)
describes painted timber Dannebrog windows, panel doors and contrasting masonry/render
details. The [Frederikssund historicist style sheet](https://www.frederikssund.dk/Borger/byudvikling/Planlaegning/kulturarv/stilblade-for-bevaringsvaerdige-huse/den-historicistiske-villa)
provides another regional reference. These help establish a vocabulary; they do not specify
universal Copenhagen joinery dimensions. [Rationel's shaped-window overview](https://www.rationel.dk/vinduer/andre-former-vinduer/)
illustrates available curved and round configurations. No product geometry or assets are copied.

Private drawings and photos guide project-specific choices outside Git. A photographed window
is not evidence of its concealed section, hardware dimensions, glazing performance or approval.

### Host wall seating

The 70 mm frame uses the existing 120 mm exterior reveal when the host wall is at
least 190 mm thick. For thinner supported walls, the reveal is constrained to
`wall thickness − 70 mm`, so the frame's rear seats at the inner wall face. Sills
and door/window composite infills follow that same mounting station. This fixes
a reproducible floating assembly on 100 mm hosts without changing the opening
rectangle, source identity or scheduled sizes. Operating handles may project
beyond a frame face; their attachment remains on the leaf or sash.

`opening-mounting` is an offline export and exact wall edit/Undo fixture. Its
horizontal cut at 1500 mm makes the host/frame intersection inspectable in GLB.
This geometry constraint does not establish the cause of all native viewport
transparency or edge artifacts; those require the parent's renderer review.
