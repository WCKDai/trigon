# Trigon: instructions for agents

Trigon is a free, open-source, pure-Rust building modelling tool that agents drive over MCP. It
follows Polykrom's engineering rules (`~/wckdboy/polykrom/AGENTS.md`), summarised here.

- **The AI controls the tool; it doesn't generate the model.** Geometry comes only from
  parametric code in `crates/model`. Perception helpers may suggest; commands decide.
- **Everything is a command** (`crates/engine/src/commands.rs`): a dotted id, a params struct
  with a JSON schema, a summary. The CLI and MCP pick it up automatically. Commands that change
  the model are journaled and undoable; a failing command leaves the document unchanged.
- **Never crash.** No `unwrap`/`expect`/`panic!`/`todo!`/`unreachable!` outside tests, no
  `unsafe`. Input numbers are hostile: check them (`trigon_model::checked`). New commands must
  pass `hostile_params_never_panic`.
- **Clean room.** Never read, copy or link GPL/LGPL/AGPL code (OpenCASCADE, FreeCAD,
  IfcOpenShell, CGAL, LibreDWG, Blender…). They may run as separate programs to check results
  (for example IfcOpenShell validating our IFC). Formats come from public specifications.
- **Credentials** come from the environment and never enter a document, a log, a fixture or an
  error message. Dataforsyningen echoes the token in URLs and responses: store photo ids, never
  asset URLs.
- **Units:** millimetres, Z up, site-local coordinates (origin at the address, EPSG:25832
  offsets in `Site::origin_utm`). Never put map coordinates in the model.
- **Gates before a commit:** `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
