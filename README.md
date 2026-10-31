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
| Outline | OpenStreetMap (GeoDanmark via Datafordeler is next) |
| Storeys and terrain | Danmarks Højdemodel: DSM − DTM inside the outline |
| Photos | Skråfoto 2021–2025, matched to the facades whose camera looks at them |
| Facades | Bay grid per facade; slots take typed components; every change is checked (fits the wall and storey, no overlaps, no openings in party walls) |
| Components | Casement windows (1–3 sashes), fixed window, entrance door, French door, French balcony, cantilever balcony; define your own with `types.define` |
| Export | IFC4 (walls voided by openings, typed windows and doors, slabs, railings, georeferenced with `IfcMapConversion`) and glTF 2.0 |
| History | Undo/redo, and a journal of every change saved in the `.trigon` file |

## Layout

| Crate | |
|---|---|
| `crates/model` | Document, component families, footprint clean-up, facade layout, geometry (2.5D: openings are 2D holes in wall faces) |
| `crates/data` | Danish data providers, OSM fallback, coordinates |
| `crates/io` | GLB and IFC4 writers, written from the public specifications |
| `crates/engine` | Commands, session, undo, journal |
| `crates/mcp` | MCP over stdio |
| `apps/trigon` | CLI |

## Licence

MIT OR Apache-2.0. Data you fetch keeps its own terms: Danish public data ("Indeholder data fra
Styrelsen for Dataforsyning og Infrastruktur") and OpenStreetMap (ODbL).
