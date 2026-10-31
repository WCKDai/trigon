<img src="assets/icon.svg" width="64" height="64" alt="">

# Trigon

Free and open-source building modelling for existing and new buildings, driven by hand or by an
AI agent over MCP. Denmark first: give Trigon an address and it pulls the outline, heights and
oblique photos from public data. You (or an agent) then lay out each facade with parametric windows,
doors and balconies, and export IFC4 and glTF.

The AI **controls** Trigon through the same commands a person uses. It never generates geometry:
every wall, window and balcony is built by deterministic, parametric code from numbers you can
read and change.

Status: early MVP (week 1). Planning notes are in [`docs/planning`](docs/planning) and the
open-source landscape in [`docs/research`](docs/research).

## Try it

```sh
cargo build --release
export TRIGON_DATAFORSYNINGEN_TOKEN_FILE=~/path/to/token   # free at dataforsyningen.dk
export TRIGON_DATAFORDELER_KEY_FILE=~/path/to/key           # free at datafordeler.dk (GeoDanmark, BBR)
./target/release/trigon run --doc out/f28.trigon site.load_address '{"address":"Frederikssundsvej 28, 2400 København"}'
./target/release/trigon run --doc out/f28.trigon facade.list
./target/release/trigon script examples/frederikssundsvej-28.facades.json --doc out/f28.trigon
```

The script writes `out/frederikssundsvej-28.ifc` and `.glb`. Without a token, addresses come from
OpenStreetMap's Nominatim and heights are skipped (pass `storeys`).

### As an MCP server (Claude Code, etc.)

```sh
claude mcp add trigon -e TRIGON_DATAFORSYNINGEN_TOKEN_FILE=$HOME/.config/trigon/token -- \
  /path/to/trigon/target/release/trigon mcp --doc $HOME/trigon/model.trigon
```

Every command is a tool (`site_load_address`, `facade_list`, `evidence_photos`, `facade_set_bays`,
`facade_place`, `types_define`, `export_ifc`, `edit_undo`, …). `trigon commands` lists them and
`trigon schema <command>` prints a command's parameters.

## What it does today

| | |
|---|---|
| Address | Dataforsyningen GSearch (DAR id, EPSG:25832); Nominatim without a token |
| Outline | GeoDanmark via Datafordeler (surveyed wall line, 0.15 m); OpenStreetMap without a key |
| Register | BBR via Datafordeler: storeys, year built, use, wall and roof material, areas |
| Storeys and terrain | Storey count from BBR; storey height from the eaves height in Danmarks Højdemodel (DSM − DTM) |
| Photos | Skråfoto 2021–2025, matched to the facades whose camera looks at them |
| Facade images | `evidence.facade_image` straightens a facade from the photo that shows it best (only the needed tiles are downloaded) and draws the model's storeys, bays and openings over it; over MCP the agent receives the image itself |
| Facades | Bay grid per facade; slots take typed components; every change is checked (fits the wall and storey, no overlaps, no openings in party walls). Raised ground floors (`building.set_storeys ground_floor`), stairwell windows that cross floors (`spans_storeys`) and openings below floor level down to the terrain |
| Components | Door + window units (`door_window`: door width and side, window over a panel), casement windows (1–3 sashes), fixed window, entrance door, French door, French balcony, cantilever balcony; define your own with `types.define` |
| Export | IFC4 (walls voided by openings, typed windows and doors, slabs, railings, georeferenced with `IfcMapConversion`) and glTF 2.0. `export.gltf cut_level` makes a 3D plan cut (cut views): everything above `cut_mm` over that storey's floor is left out and the walls are closed at the cut |
| Drawings | `drawing.add` links a page of a scanned archive PDF (JPEG scans, read with lopdf + zune-jpeg); `drawing.view` shows it with a labelled pixel grid; `drawing.register_facade` ties a drawn elevation to a facade; `evidence.facade_image {drawing}` straightens it with the model drawn over it, like the photos. Only links are stored, never the scans |
| Phases | Revit-style: `phase.set` (e.g. 1938 → 2026 → forslag), `phase.activate`; every placement is created in a phase and may be demolished in a later one. `facade.place` in a later phase demolishes what stood in the slot instead of deleting it; `facade.demolish` (openings close the wall). Views take `phase` + `filter` (complete, demolition, new, all): overlays colour existing magenta, new green, demolished red; glTF draws demolished translucent red; IFC carries Pset_*Common.Status (EXISTING, NEW, DEMOLISH) |
| Balconies | Balconies are attachments (no opening): `balcony_slab` for existing ones and `steel_balcony` for the MinAltan stålaltan (UPE 160 frame, sunken Cumaru/fibre-concrete/composite/bamboo deck, Frederiksberg/Nørrebro/Vesterbro/Østerbro railing, galvanised/white/anthracite, optional hardwood handrail, matt-glass screens, tie rods) |
| Roofs | `building.set_roof`: flat, or pitched from the eaves by straight skeleton (geo-straight-skeleton, MIT/Apache) with gable ends on chosen facades; give `pitch_deg` or the `ridge` height read from a section and the pitch is derived. A pitched roof can run past the walls (`overhang_mm` at the eaves, `verge_mm` at the gables) on the same planes, carry a build-up (`thickness_mm`, closed with fascia, soffit and verge boards) and `gutters` |
| Plinth | `building.set_plinth`: a rendered or concrete sokkel along the foot of every facade, from below the terrain to `height_mm`, `projection_mm` proud of the brickwork, with basement windows cut through |
| Floor plans | `drawing.register_plan` ties an archive plan to the site from two or more points (pixel ↔ site point or facade position), with residuals; `plan.image` draws a storey cut 1 m above the floor (walls, windows, closed door leaves, balconies labelled "Ca. depth x width", phase colours), as PNG or SVG; scan underlays are opt-in for checking; `building.outline` lists the corners |
| Presentation sheets | `plan.sheet` composes landscape plans with joined wall outlines, masonry hatching, simplified stair cores, balcony dimensions and entrance labels (on each stair's axis, aligned with the wing). Labels never overlap: a balcony size that collides steps out one line, and at small scales the sizes move to one "Altaner: n stk. Ca. …" line. One vector scene produces SVG, antialiased PNG and multipage vector PDF. No scan underlay or automatic room-area labels; optional region and rotation change only the viewport |
| Elevations | `elevation.sheet` draws one facade square-on from the model itself: walls, windows, doors, balconies and the roof behind the eaves are projected and painted back to front, balconies are labelled "Ca. depth x width" under the slab and staircases with their entrance ("TV 7 TH", mirrored as "TH 7 TV" from the courtyard). `along` (two site points) draws a stepped street front square-on to one line instead of one facade. SVG, PNG or PDF |
| Walls | Facade walls are built storey by storey: `building.set_wall_schedule` gives a thickness per storey, or `preset: copenhagen_masonry` (1½ sten 360 mm on the top two storeys, 2 sten 480 on the next two, 2½ sten 600 below and a basement half a brick more, 700). The wall steps on the inside at every floor and the floors rest on the steps; the outer face stays flush. `facades` gives single walls their own thickness (fire gables keep one thickness all the way up). Each storey's piece is a closed prism with its openings cut out, and an opening that crosses a floor cuts both storeys. Interior walls are joined per storey: an end within 300 mm of the facade or of another wall stops on that wall's face (butt joint), at a corner the thicker wall runs through, collinear pieces either side of a door stay apart. Exported as one IfcWall per facade and storey |
| Interior | `interior.add_walls` / `add_stairs` / `add_rooms` / `clear` / `list`: interior walls (load-bearing or partitions) per storey, staircases with treads, rooms with name and area; exported as IfcWall (.SOLIDWALL./.PARTITIONING.) and drawn in plans. `drawing.trace_walls` reads candidate walls off a registered plan (filled and outlined walls, adaptive to the paper, cleaned and merged), `add: true` to accept. `interior.regularize` aligns measured wall runs with explicit building axes, merges duplicates and repairs small junction gaps while retaining doorway-size gaps. `interior.doors_from_gaps` turns those gaps into doors: collinear pieces either side of a 600–1300 mm gap become one wall with a door, and a wall stopping a door's width short of another runs on to it with the door at its end; doors get a frame and a closed leaf in 3D and a closed leaf (no swing) in plans. Source review is still required to distinguish fixtures from walls |
| IFC import | `ifc.check` reads a file with two independent readers (ifc-lite, bimifc) and compares them with each other and with the model; `reference.link` shows another IFC model with this one, placed through its georeference |
| History | Undo/redo, and a journal of every change saved in the `.trigon` file |

## Layout

| Crate | |
|---|---|
| `crates/model` | Document, component families, footprint clean-up, facade layout, geometry (2.5D: openings are 2D holes in wall faces), storey-by-storey masonry walls (`walls`) and wall joins (`joins`) |
| `crates/data` | Danish data providers, OSM fallback, coordinates |
| `crates/io` | GLB and IFC4 writers, written from the public specifications |
| `crates/ifcread` | IFC import and cross-checking through ifc-lite and bimifc |
| `crates/engine` | Commands, session, undo, journal |
| `crates/mcp` | MCP over stdio |
| `apps/trigon` | CLI |

## Licence

MIT OR Apache-2.0. Trigon links two IFC readers used unmodified under MPL-2.0, whose source is at
[LTplus-AG/ifc-lite](https://github.com/LTplus-AG/ifc-lite) and [holg/bimifc](https://github.com/holg/bimifc) (0.1.0). Data you fetch keeps its own terms: Danish public data ("Indeholder data fra
Styrelsen for Dataforsyning og Infrastruktur") and OpenStreetMap (ODbL).

The plan renderer bundles unmodified Liberation Sans 2.1.5 under the SIL Open Font License 1.1.
See [`crates/engine/assets/liberation-sans/OFL.txt`](crates/engine/assets/liberation-sans/OFL.txt).
