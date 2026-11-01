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

## Co-building with an agent

**Trigon Studio** is the desktop app where you and an agent build the model together, and you
watch the agent work: every command it runs lands in your window, in the activity feed (by the
agent's name, with parameters, result and duration) and in the model, where what it changed glows
for a few seconds.

Studio uses the Trigon orange identity, graphite controls and warm drawing paper. The **Model**
workspace's Project browser opens floor plans, 3D cuts and facades. Properties provides precise move,
copy, align, storey-copy and delete tools; saved selection groups run as one undoable batch.
**Families** browses and edits component types and creates parameter variants. The project menu
beside the Trigon mark saves reusable templates and starts detached copies. **Document** opens
saved plans, elevations and sections with precise annotation picking and snapping; see
[saved drawings](docs/annotations.md). See [precise editing](docs/editing.md) for
the shared commands, coordinate conventions and current boundaries.

The 3D workspace and 2D views share one model. Pick a wall, opening, stair, room, balcony, roof or
slab to edit its source properties; dimensional markers provide drag edits with one Undo step.
Saved annotations support font families/styles and filled regions. See
[coordinated authoring](docs/coordinated-authoring.md) for supported edits and current limits.

Reusable component libraries use local `.trigonlib` files. `types.export` saves a selection and
`types.import` loads it; explicit replacement updates every linked instance in one undoable batch.
See [component libraries](docs/component-libraries.md) and the original `libraries/balconies.trigonlib` starter pack.

Studio's **Schedules** workspace and `schedule.openings` list the built windows and doors with their model IDs, storeys, types,
dimensions, phase status and lining areas. It includes interior/cellar doors and explicitly
excludes loggia facade-hole areas from the totals. `schedule.export` saves the inventory as
CSV or JSON. See [opening schedules](docs/schedules.md).

Masonry uses physical-scale brick courses in Studio and self-contained GLB exports, with original
colour and joint-relief maps. `facade.set_finish` supports explicit yellow and red brick as well
as render; finish changes preserve geometry and are undoable. See [materials](docs/materials.md).

```sh
cargo build --release -p trigon-studio -p trigon
./target/release/trigon-studio path/to/model.trigon
```

Then point your agent's MCP configuration at `trigon mcp`, with no flags. It finds the running
Studio (through the discovery file Studio writes next to its control token) and runs every tool
call in Studio's session. With no Studio running it falls back to a session of its own.

```sh
# Claude Code
claude mcp add trigon -- /path/to/trigon/target/release/trigon mcp
```

```toml
# Codex (~/.codex/config.toml)
[mcp_servers.trigon]
command = "/path/to/trigon/target/release/trigon"
args = ["mcp"]
```

Bridged to Studio, the agent also gets `studio_view` (show you a level, a phase, the plan, or one
element up close) and `studio_screenshot` (see what you see). `trigon mcp --connect 127.0.0.1:PORT`
picks a particular window and `--headless` (or `--doc FILE`) keeps a session of its own.

In Studio:

- **3D**: drag to orbit, Shift-drag or right-drag to pan, scroll or pinch to zoom at the pointer,
  double-click or F to fit. "Up to level" cuts the model 1.2 m above a floor. Click an element to
  see its id, type and properties.
- **Plan and saved drawings**: click visible wall, window or door lines to select the same element
  used by 3D and Properties; drag to pan and scroll to zoom. The orange outline follows the
  selected linework. **Edit elements** switches a saved drawing to the model editing tools;
  annotation tools retain their own selection. See [drawing picking](docs/drawing-picking.md).
- **Activity**: newest first; click an entry for its full JSON. Undo and Redo (⌘Z, ⇧⌘Z) apply to
  your changes and the agent's alike.
- **Commands…** (⌘K): search every command, edit its parameters (prefilled from its schema) and
  run it yourself.

The control channel listens on 127.0.0.1 only, with a new random token per launch in a file only
you can read. See [`docs/control-protocol.md`](docs/control-protocol.md).

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
| Facade finish | `facade.set_finish`: brick (the building's facing brick), red_brick or render, for the building or single facades; `facade.set_bands`: rendered bands standing proud of the wall on chosen storeys (slab edges, spandrels), openings cut through |
| Cellar stairs | `building.set_cellar_stairs`: outdoor stairs down to the basement (kældernedgange) read off the basement plan: a pit along a facade between retaining walls with a kerb and railing, a straight flight along the wall, and a door into the basement |
| Components | Loggias (`loggia`: the facade opened from the sill, glazed door and window set back between brick returns under a soffit, with a rail across), windows with top lights (`top_light`), glazed white stair, shop and balcony doors, door + window units (`door_window`: door width and side, window over a panel), casement windows (1–3 sashes), fixed window, entrance door, French door, French balcony, cantilever balcony; define your own with `types.define` |
| Export | IFC4 as building elements for Revit and Vectorworks: layered walls with axes, wall types and joins, openings with typed windows and doors, floor slabs with stair openings, the roof, stairs of flights and landings, rooms as spaces, balconies as assemblies, colours and presentation layers, georeferenced with `IfcMapConversion` ([docs/ifc-export.md](docs/ifc-export.md), sample: [examples/sample-building.ifc](examples/sample-building.ifc)); and glTF 2.0. `export.gltf cut_level` makes a 3D plan cut (cut views): everything above `cut_mm` over that storey's floor is left out and the walls are closed at the cut |
| Opening schedules | `schedule.openings` returns a deterministic, read-only inventory of actual built windows and doors, including occurrence dimensions and IFC-compatible outer-lining areas, with building/storey/phase filters; loggia areas are explicitly excluded ([contract](docs/schedules.md)) |
| Drawings | `drawing.add` links a page of a scanned archive PDF (JPEG scans, read with lopdf + zune-jpeg); `drawing.view` shows it with a labelled pixel grid; `drawing.register_facade` ties a drawn elevation to a facade; `evidence.facade_image {drawing}` straightens it with the model drawn over it, like the photos. Only links are stored, never the scans |
| Phases | Revit-style: `phase.set` (e.g. 1938 → 2026 → forslag), `phase.activate`; every placement is created in a phase and may be demolished in a later one. `facade.place` in a later phase demolishes what stood in the slot instead of deleting it; `facade.demolish` (openings close the wall). Views take `phase` + `filter` (complete, demolition, new, all): overlays colour existing magenta, new green, demolished red; glTF draws demolished translucent red; IFC carries Pset_*Common.Status (EXISTING, NEW, DEMOLISH) |
| Balconies | Balconies are attachments (no opening): `balcony_slab` for existing ones and `steel_balcony` for the MinAltan stålaltan (UPE 160 frame, sunken Cumaru/fibre-concrete/composite/bamboo deck, Frederiksberg/Nørrebro/Vesterbro/Østerbro railing, galvanised/white/anthracite, optional hardwood handrail, matt-glass screens, tie rods) |
| Roofs | `building.set_roof`: flat, or pitched from the eaves by straight skeleton (geo-straight-skeleton, MIT/Apache) with gable ends on chosen facades; give `pitch_deg` or the `ridge` height read from a section and the pitch is derived. A pitched roof can run past the walls (`overhang_mm` at the eaves, `verge_mm` at the gables) on the same planes, carry a build-up (`thickness_mm`, closed with fascia, soffit and verge boards) and `gutters` |
| Basement walls | `building.set_basement_line`: below the ground storey the walls run straight along the inner corners of street-front steps up to `straighten_mm` deep, as basement plans show under bays; the steps above go down into the ground on their own foundation. Gables and wing corners keep their walls |
| Plinth | `building.set_plinth`: a rendered or concrete sokkel along the foot of every facade, from below the terrain to `height_mm`, `projection_mm` proud of the brickwork, with basement windows cut through |
| Floor plans | `drawing.register_plan` ties an archive plan to the site from two or more points (pixel ↔ site point or facade position), with residuals; `plan.image` draws a storey cut 1 m above the floor (walls, windows, closed door leaves, balconies labelled "Ca. depth x width", phase colours), as PNG or SVG; scan underlays are opt-in for checking; `building.outline` lists the corners |
| Presentation sheets | `plan.sheet` composes landscape plans with joined wall outlines, masonry hatching, simplified stair cores, balcony dimensions and entrance labels (on each stair's axis, aligned with the wing). Labels never overlap: a balcony size that collides steps out one line, and at small scales the sizes move to one "Altaner: n stk. Ca. …" line. One drawing scene (paths and texts in millimetres on named layers, DS/ISO 128 pens) produces SVG, antialiased PNG, multipage vector PDF and DXF. Sheets are at a standard scale (1:100, 1:200, …) when one fits within 25 % of filling the page, and say so. No scan underlay or automatic room-area labels; optional region and rotation change only the viewport |
| Elevations | `elevation.sheet` draws one facade square-on from the model itself, as a line drawing: hidden lines removed exactly (each edge clipped against the triangles in front of it), feature edges only (boundaries and folds over 10°, never the triangulation), the outline heavier, things further back lighter and thinner, light tonal fills with glass a little darker, the ground line heaviest. Balconies are labelled "Ca. depth x width" under the slab and staircases with their entrance ("TV 7 TH", mirrored as "TH 7 TV" from the courtyard). `along` (two site points) draws a stepped street front square-on to one line instead of one facade. SVG, PNG, PDF or DXF |
| Walls | Facade walls are built storey by storey: `building.set_wall_schedule` gives a thickness per storey, or `preset: copenhagen_masonry` (1½ sten 360 mm on the top two storeys, 2 sten 480 on the next two, 2½ sten 600 below and a basement half a brick more, 700). The wall steps on the inside at every floor and the floors rest on the steps; the outer face stays flush. `facades` gives single walls their own thickness (fire gables keep one thickness all the way up). Each storey's piece is a closed prism with its openings cut out, and an opening that crosses a floor cuts both storeys. Interior walls are joined per storey: an end within 300 mm of the facade or of another wall stops on that wall's face (butt joint), at a corner the thicker wall runs through, collinear pieces either side of a door stay apart. Exported as one IfcWallStandardCase per facade and storey |
| Interior | `interior.add_walls` / `add_stairs` / `add_rooms` / `clear` / `list`: interior walls (load-bearing or partitions) per storey, staircases with treads, rooms with name and area; exported as IfcWallStandardCase (.SOLIDWALL./.PARTITIONING.), IfcStair and IfcSpace and drawn in plans. `drawing.trace_walls` reads candidate walls off a registered plan (filled and outlined walls, adaptive to the paper, cleaned and merged), `add: true` to accept. `interior.regularize` aligns measured wall runs with explicit building axes, merges duplicates and repairs small junction gaps while retaining doorway-size gaps. `interior.doors_from_gaps` turns those gaps into doors: collinear pieces either side of a 600–1300 mm gap become one wall with a door, and a wall stopping a door's width short of another runs on to it with the door at its end; doors get a frame and a closed leaf in 3D and a closed leaf (no swing) in plans. Source review is still required to distinguish fixtures from walls |
| Sections | `section.sheet` cuts the model along a line (`from` → `to`, looking left; `flip` looks right) and fills the cut: masonry hatched, concrete grey, partitions light grey, steel black, outlined with the heaviest pen; loops are assembled from the plane cut without trusting meshes to be closed (open chains are drawn as heavy lines). What lies within `depth_mm` beyond is drawn as in an elevation. Storey levels (+0,00 at the ground storey floor, metres with a decimal comma) with their names, eaves and ridge, the ground line outside the building. SVG, PNG, PDF or DXF |
| DXF | AutoCAD 2000 (AC1015) ASCII, written by hand from Autodesk's public DXF reference: model space at 1:1 in mm ($INSUNITS 4), layers A-WALL-CUT, A-WALL-PATT, A-WALL, A-GLAZ, A-DOOR, A-BALC, A-ROOF, A-FLOR, A-FLOR-STRS, A-ELEV-PATT, A-ANNO, A-ANNO-TTLB, A-GRND and A-LEVL with colour, lineweight and linetype; LINE/LWPOLYLINE, HATCH (solid poche, ANSI31 masonry), TEXT, and MTEXT for the title block; Windows-1252 text. Plans are in site coordinates (turned by `rotate_deg` when given), elevations and sections in drawing coordinates (x along the line, y above the terrain) |
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
| `crates/mcp` | MCP over stdio, headless or bridged to Trigon Studio |
| `crates/control` | The control channel a running Studio listens on (token, discovery, limits) |
| `apps/trigon` | CLI |
| `apps/trigon-studio` | Trigon Studio, the desktop app (eframe/egui, wgpu) |

## Licence

MIT OR Apache-2.0. Trigon links two IFC readers used unmodified under MPL-2.0, whose source is at
[LTplus-AG/ifc-lite](https://github.com/LTplus-AG/ifc-lite) and [holg/bimifc](https://github.com/holg/bimifc) (0.1.0). Data you fetch keeps its own terms: Danish public data ("Indeholder data fra
Styrelsen for Dataforsyning og Infrastruktur") and OpenStreetMap (ODbL).

The plan renderer bundles unmodified Liberation Sans 2.1.5 under the SIL Open Font License 1.1.
See [`crates/engine/assets/liberation-sans/OFL.txt`](crates/engine/assets/liberation-sans/OFL.txt).
