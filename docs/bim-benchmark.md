# Reproducible BIM workflow comparison

Trigon is evaluated through correct, coordinated building workflows. There are no
measured Revit, Archicad or Vectorworks speed results in this repository. Official
feature documentation establishes a comparison target, not speed or quality parity.

## Native baseline

Build and run the existing public offline fixture, then collect one discarded
warmup and at least five measured trials:

```sh
cargo build --release --locked -p trigon-engine --example drawing-workflow
python3 tools/benchmark-workflow.py --out out/bim-benchmark/workflow-profile
```

The runner refuses an existing output directory. Each trial must pass real picking,
exact edit/Undo and schedule restoration, and both IFC readers. Plan, facade,
section, GLB and schedule hashes must match across trials. Failure stops a passing
performance claim. Raw logs, verified exports, binary hash, checkout context,
OS/architecture, median, nearest-rank p95, range and MAD stay with the results.
Record the CPU model, RAM, power mode and competing workloads separately before
using these data for a comparison. Five trials give weak tail estimates.

The metric includes process startup, geometry, three views in SVG/PNG/PDF/DXF,
editing, schedules, native/GLB/IFC export and two-reader validation. It does not
measure native mouse latency, GPU presentation, peak resident memory, cold-disk
performance or editable IFC roundtrip. Native save/reopen and IFC exchange are
separate acceptance conditions. Do not compare this full workflow with a cached
redraw time or another product's unrelated benchmark.

## Equivalent user-run trials

The default executable follows Cargo's platform naming, including `.exe` on Windows.
Workflow subprocess output, verification JSON, logs and reports use UTF-8 explicitly,
including Unicode output paths on Windows with legacy locale defaults.

Report schema `trigon-benchmark/2` separates `checkout_context` from the selected
executable. `binary_path` and `binary_sha256` identify the actual program. Its source
revision, dirty state and profile remain `null` with `binary_provenance: unverified`;
the script cannot establish them from the checkout, filename or hash. Pair a result
with independently recorded build commands and source evidence before attributing it
to a revision/profile. Existing version-1 reports are retained as historical evidence;
their revision/profile fields alone do not prove executable provenance.

Use the same physical machine, public fixture and units. Freeze phase/filter,
cut/depth range, physical scale, crop, detail, visible objects, fonts and output
format. Record application/build versions and any substitutions. Competitor trials
are user-supplied; agents do not drive, monitor or extract from commercial CAD apps.

For every trial, inspect the final plan, section, facade, 3D and schedule. Stop a
full-update timer only when all specified outputs reflect the change correctly.
Record unsupported operations, stale views, manual repairs and failures. Repeat
at least five exports/launches; use more trials for interactive timing and report
distributions. Keep startup, pan/zoom, model-to-drawing update, export, human task
completion and memory measurements separate.

Compare these outcomes using the same dimensions and expected affected IDs:

- Select a window in plan and 3D; change width; verify void, geometry, dimensions
  and schedule; Undo once and restore the exact source state.
- Align and connect mixed-width walls; preserve length, door positions and other
  storeys; inspect material gaps/overlap and abrupt angle changes.
- Fit stairs to a measured enclosure; inspect treads, landings, walk line, upper
  and lower symbols, and text clearances at final paper scale.
- Edit a shared component type; verify every intended occurrence and all local
  overrides, then perform the same check for one selected occurrence.
- Change scale, template and view range; verify line hierarchy, clipping,
  annotation placement, picking visibility and current drawing generations.
- Save, reopen and exchange IFC; count products and check solids/materials/
  placements. Report import-as-reference separately from editable native elements.

## Official comparison targets (checked 2026-11-02)

[Revit 2027's view-performance guidance](https://help.autodesk.com/cloudhelp/2027/ENU/Revit-Customize/files/GUID-A0444FA3-9AD1-4D1E-B309-90FD7B556810.htm)
identifies view depth, detail and open views as relevant performance factors.
[Archicad 30 documents view update modes](https://help.graphisoft.com/AC/30/INT/Topics/050_ViewsVB/050_ViewsVB-3.html),
and [Vectorworks 2027 documents explicit viewport updates](https://app-help.vectorworks.net/2027/eng/VW2027_Guide/Viewports2/Updating_viewports.htm).
Freshness must therefore be a correctness condition before timing completion.

[Archicad's IFC conversion options](https://help.graphisoft.com/AC/30/INT/Topics/121_IFC/121_IFC-30.html)
distinguish editable construction elements from Object/Morph fallback.
[Vectorworks' 2D-component workflow](https://app-help.vectorworks.net/2027/eng/VW2027_Guide/Symbols/Creating_2D_components_for_symbol_definitions_and_plug-in.htm)
distinguishes supplied symbols from generated output. These are reasons to score
editability and coordination explicitly; they are not comparative measurements.
