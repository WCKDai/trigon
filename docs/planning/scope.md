# Trigon: scope (draft v1)

Status: draft, 2026-10-31. v0 assumed a commercial product. That framing is gone; this version
records the owner's decisions and works out what they imply.

## 1. Decisions (owner, 2026-10-31)

| # | Decision |
|---|---|
| D1 | Trigon is **completely free and open source**. |
| D2 | **Denmark first.** |
| D3 | **No buyer.** The owner is the main user, so scope follows the owner's own work. |
| D4 | **Desktop app** for macOS and Windows. |
| D5 | **AI controls the tool; it does not generate the model.** An agent drives Trigon through an **MCP server**, using the same commands a person uses. Geometry always comes from deterministic, parametric code. |
| D6 | **Built on Polykrom's stack:** the PolyCore kernel (Rust, vendored truck) and Polykrom's app shell. |
| D7 | **Own the platform** rather than build on FreeCAD BIM or Bonsai. It must stay free and open source. Bonsai/IfcOpenShell may run as *external programs* (IFC oracle, interim drawings) but are never linked. |

## 2. What D6 actually means (observed in the local checkouts, 2026-10-31)

Taken from `~/wckdboy/polykrom` (0.21.0-rc.1) and `~/wckdboy/polycore`:

- **Language:** Rust 2024, `unsafe` forbidden, no panics in non-test code, guarded kernel calls, crash reports.
- **Kernel:** PolyCore (`solidcraft-kernel`, `solidcraft-geom`, Apache-2.0) with a vendored truck
  closure (modeling, shapeops, meshalgo, stepio, topology). It is pinned by git revision. Its README
  says **"Polykrom is its only consumer"**, so making Trigon a second consumer changes that contract.
- **App shell:** layered crates `geom → sketch/kernel/render → doc → io → engine → ui-egui → apps`.
  The UI is egui 0.36 + eframe, rendering is wgpu 30, and everything below the apps builds for
  `wasm32` (a browser build comes for free).
- **"Everything is a command":** `CommandSpec` with JSON params and a schema (schemars). The toolbar, CLI,
  scripts, control channel and **MCP server (`solidcraft-mcp`, JSON-RPC 2.0 over stdio)** all run the
  same commands. This is exactly what D5 needs.
- **Drawings are starting:** `solidcraft_geom::section` (W01a) holds neutral section geometry, the
  first piece of a drawing plan. Trigon needs this even more than Polykrom does, so it is a natural shared investment.
- **License policy:** MIT OR Apache-2.0, clean-room. **Agents must never read, copy or link
  GPL/LGPL/AGPL code** (OpenCASCADE, FreeCAD, SolveSpace…). GPL tools may only run as *separate
  programs* used as test oracles (the `tools/freecad-oracle/` precedent).
- **Application code is Rust only.** Python is allowed for tooling, not as a runtime dependency.

### Consequences for Trigon

Recommendation: **inherit Polykrom's license policy as-is.** It keeps code shareable both ways, and
D1 is still satisfied because MIT/Apache *is* free and open source. Under that policy these items from the
landscape doc change category:

| Was planned as | Now |
|---|---|
| IfcOpenShell as the IFC backbone (LGPL) | **External oracle only** (validate Trigon's IFC output by running IfcConvert/IfcTester as a separate program). In-app IFC must be Rust: write our own on **ruststep/espr** (Apache-2.0, EXPRESS → Rust codegen, from the truck authors), or use **ifc-lite** (MPL-2.0, active, needs a policy call), or **ifc_rs** (MIT, stale since 2024) |
| OCCT for HLR and drawings | Not allowed. Build planar HLR/section cutting in Rust, on Polykrom's W01 drawing work |
| CGAL straight skeleton for roofs | **geo-straight-skeleton** (MIT/Apache, new, 40 downloads, so treat it as unproven) or our own |
| Bonsai / FreeCAD BIM as code references | UX/behaviour reference only (use the app, read the docs). **Don't read their source** |
| LibreDWG for DWG | Not linkable. DXF first via **dxf** (ixmilia, MIT). DWG later, clean-room from the public ODA spec PDF |
| Python ML pipelines in-app | Models exported to **ONNX**, run via **ort** (MIT/Apache; CoreML on Mac, DirectML on Windows) or **candle**. Training stays in offline tooling |
| Manifold (C++) for booleans | Not needed. See §4.2: building geometry avoids 3D booleans |

## 3. What Trigon is (restated for a single expert user)

> A desktop BIM tool for existing and new buildings, driven by hand or by an agent over MCP.
> Point it at a Danish address and it pulls everything public about the building (footprint,
> lidar, oblique photos, BBR, cadastre, archive plans). You and/or the agent then build a clean,
> parametric, semantically typed model with walls, floors, roofs, typed windows, doors and balconies,
> and get plans, sections and elevations out as drawings, with IFC/DXF/PDF/glTF exports.

### What "AI controls, doesn't generate" means concretely

- The agent sees what a person sees, plus more: **evidence is exposed as MCP resources/tools**. Examples are a
  rectified facade image, the skråfoto images that cover a wall, an archive plan page, BBR fields, a
  lidar profile, and a viewport screenshot.
- The agent acts only through **commands**: `facade.grid.set`, `facade.slot.place {type: "window.casement.2"}`,
  `opening.set {sill: 900}`, `storey.add`, `wall.draw`, `balcony.place`, `drawing.section.create`…
- Perception models (opening detector, plan vectorizer) are **optional helper commands** that return
  *suggestions* (`survey.detect_openings → candidates[]`). They never write the model. The agent, or you,
  accepts them through ordinary commands.
- Every command call is recorded with its arguments, so the agent's work is replayable, undoable and diffable.
  That makes the AI's actions auditable in the same way as your own.

## 4. Architecture

### 4.1 Sharing with Polykrom

Two ways to share:

- **A. Trigon is its own app on shared crates (recommended).** Extract the domain-neutral parts of
  Polykrom into a shared platform (command framework, MCP/control channel, `ui_kit`, navigation,
  gpu renderer, crash/recovery, doc-format machinery, `geom`, `section`). Polykrom and Trigon both
  consume it, the way Polykrom consumes PolyCore today.
- **B. Trigon is a "Building" workspace inside Polykrom.** This is faster at first, but it ties a building
  domain to a mechanical CAD app's releases, format and UI.

Start with A in a lightweight form: Trigon depends on Polykrom/PolyCore crates by git revision.
Then use Phase 0 to measure which crates are actually domain-neutral before extracting anything. The
extraction itself is Polykrom-side work and should go through Polykrom's own process (RFC/PR).

### 4.2 Building geometry: mostly 2.5D, with the kernel where it pays

Buildings are overwhelmingly planar extrusions, which suits a young B-rep kernel (truck booleans are the risky part):

- **Wall** = axis + layer set → 2D profile extruded. **Openings are 2D holes in the wall's elevation
  profile** (using `i_overlay`, MIT/Apache, for 2D booleans), so there are no 3D booleans for the 95% case.
- **Slab** = polygon with holes × thickness. **Roof** = straight-skeleton/parametric planes.
- **Components** (windows, doors, balconies) = parametric generators producing B-rep via PolyCore
  (frames, sashes, glazing, sills, railings). They are instanced by type, so a facade with 200
  windows holds maybe 8 types.
- 3D booleans through PolyCore are kept for exceptions (odd intersections, special roof cases).
- Sections and plans: the cut plane against mostly planar solids, plus planar HLR, gives robust,
  exact linework. This is much easier than general free-form HLR.

### 4.3 Data model (own schema, IFC-mapped)

```
Site (matrikel, CRS EPSG:25832, DVR90 heights)
└─ Building (BBR id, address)
   ├─ Storey[] (elevation, height)
   │   ├─ Wall[] (axis, layer set) ─ Opening[] → Component instance (type + params)
   │   ├─ Slab[], Space[] (rooms, areas), Stair[]
   ├─ Roof[]
   ├─ Facade[] (wall group + grid: levels × bays × slots; regularity rules)
   └─ Evidence[] (source ref, region, confidence)  ← linked from any element
Types library: WindowType, DoorType, BalconyType, RailingType, WallType … (parametric families)
Views: plan / section / elevation definitions → generated drawings → sheets
```

Each class maps to IFC4.3 (`IfcWall`, `IfcOpeningElement`, `IfcWindow`+`IfcWindowType`,
`IfcSlab`, `IfcSpace`, `IfcRailing`, `IfcBuildingStorey`…). Track IFC5/IFCX, whose JSON + layer model
fits Trigon's command log well.

### 4.4 Denmark data adapters (Rust)

| Source | Access | Rust crates |
|---|---|---|
| Skråfoto | STAC API + COG, with camera parameters | `stac` (rustac, MIT/Apache), `tiff`, `reqwest` |
| DHM lidar / DSM / DTM | Dataforsyningen downloads/WCS | `las`, `laz`, `copc-rs`, `wblidar` (MIT/Apache) |
| Buildings / ortho | GeoDanmark (WFS/vector) | `geo`, `geojson`, `flatgeobuf` |
| BBR, DAR, Matrikel | Datafordeler (DAWA no longer serves BBR) | `reqwest` + serde |
| CRS | EPSG:25832 ↔ 4326, DVR90 | `proj4rs` (pure Rust) |
| Archive plans | Municipal byggesagsarkiver (varies) | PDF: `pdfium-render` (PDFium is BSD/Apache) or `lopdf`; `image` |

Skråfoto ships calibrated cameras, so projecting a wall onto a photo is plain math: no SfM, no
neural reconstruction. **Facade rectification is cheap and exact (to within the camera/DSM accuracy).**

## 5. Phases (each ends in something the owner uses)

### Phase 0: Skeleton (spike)
- Repo on the Polykrom/PolyCore crates; empty Trigon app opens, has `--control` and an MCP server.
- Data model v0 + `.trigon` format + commands: `site.load_address`, `storey.add`, `wall.draw`, `slab.from_footprint`.
- Danish adapters: address → matrikel/BBR → footprint → DHM heights → skråfoto items for the building.
- **Exit:** "load Strandboulevarden 1" (any address) gives an LoD1/LoD2 shell with BBR attributes, by
  hand or over MCP, exported to glTF and **IFC4** (moved up from Phase 1 so Bonsai can be the interim drawing end),
  checked by IfcOpenShell as an external oracle. Sites use a local origin (see polycore-requirements §2.1).

### Phase 1: Facade editor (the core of the project)
- Skråfoto → rectified facade image per wall, shown as an underlay in a facade editor.
- Facade grid (levels × bays), slots, regularity rules, type swaps by row/column/pattern.
- Component library v1 (parametric): casement 1/2/3-light, tilt-turn, sash, fixed, French door,
  entrance door, Juliet/French balcony, cantilever balcony, loggia, dormer, railings.
- Elevation drawings → SVG/PDF/DXF (needs kernel `section`/`project`, see [polycore-requirements.md](polycore-requirements.md)).
- Until Trigon's own drawings are ready, open its IFC in Bonsai (an external program) for sheets.
- MCP: evidence resources + all facade commands, so an agent can build a facade by looking at the photo.
- **Exit:** the owner (or an agent) models a Copenhagen block facade from skråfoto and gets a dimensioned elevation.

### Phase 2: Plans and sections
- Archive-plan underlay: import PDF page, set scale from a dimension or two picked points, register to footprint.
- Plan editor on the underlay (trace walls, openings, rooms). Openings link to facade slots.
- Generated floor plans and sections (shared `section` work with Polykrom), sheets, title block, room/window/door schedules.
- Optional helpers: plan vectorizer and opening detector (ONNX) returning suggestions.

### Phase 3: Engineering and broader I/O
- Renovation phases (existing/demolish/new), area calculations per Danish rules, IDS checks.
- Energy export (gbXML), structural hand-off, CityJSON/CityGML, 3D Tiles, USD, E57/LAS scans as underlays.
- Phone/drone capture as an underlay source.

### Later
- DWG (clean-room), IFC5, other countries' adapters, collaboration.

## 6. Non-goals (for now)

MEP, rebar/steel detailing, rendering (export glTF/USD instead), generative "make me a building",
an embedded LLM (the agent is external, over MCP), RVT import, Google Earth data extraction (ToS).

## 7. Risks

1. **Shared-crate coupling:** Polykrom's crates aren't published and PolyCore declares one consumer.
   Pin by rev, and agree the extraction with Polykrom's own process before relying on it.
2. **IFC in pure Rust** is real work (schema codegen, P21, geometry representations). Polykrom already
   parses ISO 10303-21 for STEP, which is the same container format, so reuse that.
3. **Drawings quality** (line weights, hatching, Danish conventions) is a long tail. Scope the first
   version to elevations and simple plans.
4. **Archive plan access** varies by municipality.
5. **ML weights licensing:** many SOTA models are non-commercial, which conflicts with D1's spirit and
   with redistribution. Prefer helpers trained on permissive or own data, or leave the perception to the agent.

## 8. Remaining open questions

1. Should Trigon adopt Polykrom's license policy wholesale, including whether MPL-2.0 (ifc-lite) is acceptable?
2. Option A (own app on shared crates) or B (workspace inside Polykrom)?
3. Repo home: `wckdboy/trigon` or under `WCKDai`?
4. What is the first real building you want to model? It becomes the acceptance test for Phase 0–1.
