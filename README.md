<p align="center">
  <img src="apps/trigon-studio/assets/figma/Group.svg" width="120" alt="Trigon gradient logo">
</p>

# Trigon

**Free, open-source building modelling, drawings and IFC exchange.**

Work directly in Trigon Studio or let an AI agent operate the same tools over MCP. Build with
parametric walls, floors, roofs, windows, doors and balconies, then produce coordinated plans,
elevations, sections and schedules from the model. Projects stay in local files. No paid account
or AI service is required.

The AI controls the tool. Deterministic Rust code creates the geometry; an agent changes named
parameters through the same validated, undoable commands used by Studio.

[Website](https://trigon.sh) · [Downloads](https://github.com/WCKDai/trigon/releases) ·
[Documentation](docs) · [Issues](https://github.com/WCKDai/trigon/issues)

## Download and install

Desktop builds are available through [GitHub Releases](https://github.com/WCKDai/trigon/releases).
Read the notes for the selected version: Trigon is an early preview, and build verification does
not establish that a particular building model is complete or suitable for construction.

| Platform | Package | Install |
| --- | --- | --- |
| macOS, Apple silicon | `.dmg` | Open the disk image and drag **Trigon Studio** to **Applications**. |
| Windows, x64 | `Setup.exe` | Run the installer. It installs Studio and the CLI for your user account. |
| Windows, x64, portable | `.zip` | Extract the whole archive, then run `trigon-studio.exe`. |

Preview packages are unsigned and the macOS app is not notarized. Operating systems may display
security prompts. Release notes record the exact source revision, available checks and known
limitations; checksums and dependency notices accompany the downloads.

Start Studio with a blank document, model locally, and save to a `.trigon` project. The packages
include the `trigon` command-line application for scripting and MCP.

## What you can do

- **Model buildings:** edit footprints, levels, layered walls, slabs, stairs, roofs, dormers,
  rooflights and site elements. Refine selected source parameters through precise controls and
  undoable commands.
- **Build facades:** arrange windows, doors, loggias, French balconies and cantilever balconies
  with parametric dimensions, materials and reusable component types.
- **Draw from the model:** create floor plans, whole-front elevations, sections and presentation
  sheets. Add dimensions, text and filled regions; export SVG, PNG, vector PDF and DXF.
- **Reuse families:** define variants, share local `.trigonlib` libraries and update linked
  instances. Starter libraries are in [`libraries`](libraries).
- **Coordinate changes:** work with phases, opening schedules, source-linked selections,
  Undo/Redo and a command journal shared between the desktop interface and agents.
- **Exchange models:** export IFC4 building elements and self-contained glTF/GLB. Import and
  cross-check IFC through the supported readers.
- **Use source evidence:** register archive plans and elevations against the model. In Denmark,
  bring in addresses, GeoDanmark footprints, BBR records, terrain and Skråfoto imagery where
  access is configured. Source uncertainty still needs review.

See [Studio authoring](docs/studio-authoring.md), [coordinated editing](docs/coordinated-authoring.md),
[precision workflows](docs/precision-workflow.md), [component libraries](docs/component-libraries.md),
[opening schedules](docs/schedules.md) and [IFC export](docs/ifc-export.md).

## Connect an agent with MCP

Launch Studio, then configure your MCP client to run the bundled CLI:

```json
{
  "mcpServers": {
    "trigon": {
      "command": "/absolute/path/to/trigon",
      "args": ["mcp"]
    }
  }
}
```

On Windows, use the full path to `trigon.exe`. In a macOS installation, the CLI is at
`/Applications/Trigon Studio.app/Contents/MacOS/trigon`.

`trigon mcp` discovers the running Studio and applies commands in its session, so you can watch
the changes and undo them. With no Studio running, it uses a headless session. To select a
project explicitly, run `trigon mcp --doc /path/to/model.trigon`; use `--headless` to avoid
connecting to Studio.

Explore the shared command interface:

```sh
trigon commands
trigon schema building.set_storeys
trigon schema export.ifc
```

The local Studio control channel binds to `127.0.0.1` and uses a fresh token per launch.
See [control and discovery](docs/control-protocol.md) for connection details.

## Build from source

Install Rust **1.95 or later**, then build Studio and the CLI with the committed lockfile:

```sh
git clone https://github.com/WCKDai/trigon.git
cd trigon
cargo build --release --locked -p trigon-studio -p trigon
./target/release/trigon-studio
```

On Windows, run `.\target\release\trigon-studio.exe`. On Linux, Studio uses X11 and wgpu;
desktop release packages currently target macOS Apple silicon and Windows x64.

Local modelling does not require data-provider credentials. Optional Danish services read
credentials from files named by `TRIGON_DATAFORSYNINGEN_TOKEN_FILE` and
`TRIGON_DATAFORDELER_KEY_FILE`. Keep those files outside projects and the repository.

## Development

Trigon is a Rust workspace. `crates/model` owns parametric geometry; `crates/engine` owns commands,
validation, history and the journal. Drawing, exchange, data access, MCP and local control are
separate crates. `apps/trigon-studio` provides the egui/wgpu desktop interface; `apps/trigon`
provides the CLI.

Before committing:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Read [AGENTS.md](AGENTS.md) for engineering and clean-room rules, and
[the packaging guide](docs/releasing.md) for release preparation and verification.
Planning and research live in [`docs/planning`](docs/planning) and [`docs/research`](docs/research).

## License

Trigon is licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
The gradient logo and original BIM artwork use the same license; see
[asset attribution](ATTRIBUTION.md).

The unmodified IFC readers include MPL-2.0 components from
[ifc-lite](https://github.com/LTplus-AG/ifc-lite) and [bimifc](https://github.com/holg/bimifc).
Release packages include third-party notices and source-availability information. Bundled fonts
retain their SIL Open Font License notices. Retrieved maps, imagery and public data retain
their respective providers' terms, including OpenStreetMap's ODbL attribution requirements.
