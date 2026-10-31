# Trigon: open-source landscape

Status: planning research, 2026-10-31. Licenses and maintenance status **must be re-verified before
adoption**. Items marked *(checked)* were looked up during this pass; everything else comes from
prior knowledge and may be stale.

> **Read with [scope.md](../planning/scope.md) §2.** Trigon is built on Polykrom/PolyCore (pure Rust,
> MIT OR Apache-2.0, clean-room). Under that policy, everything below marked **L** or **C** (IfcOpenShell,
> OCCT, CGAL, FreeCAD, Bonsai, LibreDWG…) is usable **only as an external program** (a test oracle or a
> reference you use as a user), never linked, copied or read as source. The in-app options are in §14.

License legend: **P** = permissive (MIT/BSD/Apache/MPL/Boost), **L** = LGPL (fine if dynamically
linked), **C** = strong copyleft (GPL/AGPL: keep it out of the shipped binary, or run it as a
separate process or service), **NC** = non-commercial weights or data (research only, so do not ship), **?** = verify.

---

## 1. BIM data model and IFC (the backbone)

| Project | What it gives Trigon | Lic |
|---|---|---|
| **IfcOpenShell** (C++/Python) | Most complete open IFC toolkit: IFC2x3/4/4.3 read/write, geometry via OCCT/CGAL, `ifcopenshell.api` for authoring, IfcConvert, IfcTester (IDS), IfcDiff, IfcPatch, BCF, `ifcopenshell.draw` (SVG drawings). v0.8.5 (Apr 2026) added `ifcquery`, `ifcedit`, **`ifcopenshell-mcp`** and `ifcchat` *(checked)* | L |
| **Bonsai** (ex-BlenderBIM) | Full native-IFC authoring app in Blender. It's the best reference for how an open BIM authoring UX works (walls, openings, types, drawings, sheets, schedules). 0.8.5 adds parametric gizmos for doors, windows, stairs, railings and roofs *(checked)* | C (as Blender add-on) |
| **That Open Engine** (web-ifc, Fragments, components) | Browser IFC: WASM parser, Flatbuffers "Fragments" format. 3.2 added in-browser geometry editing, 3.4 a 2D drawing system *(checked)* | P (MIT) ? |
| **xeokit SDK** | High-performance web BIM viewer (XKT format) | C (AGPL) / commercial |
| **IFC++ (ifcplusplus)** | Alternative C++ IFC reader/writer | P |
| **FreeCAD BIM / Arch** | Parametric BIM workbench on OCCT. Useful for its window/door presets, TechDraw sections and IFC round-trip | L |
| **IFC5 / IFCX** (buildingSMART) | JSON-based, layered and composable IFC (USD-like). Still alpha; the "IFC X Core & Modularisation" plan went to vote in Jul–Aug 2026 *(checked)*. IfcOpenShell has an IFC5-alpha project | P ? |
| **bSDD API**, **IDS**, **BCF** | Classification dictionary, model requirement checks, issue exchange | Open standards |
| **Speckle** | Object-based model exchange/versioning server with connectors (Revit, Rhino, AutoCAD, Blender, QGIS…). Could be Trigon's bridge to the closed world | P (Apache-2.0) |

## 2. Geometry kernels and solvers

| Project | Role | Lic |
|---|---|---|
| **OpenCASCADE (OCCT)** | B-rep, booleans, STEP/IGES, HLR (hidden-line removal for drawings) | L (+exception) |
| **Manifold** | Fast, robust mesh booleans (used by Blender/OpenSCAD) | P |
| **truck**, **Fornjot** (Rust) | Young Rust B-rep kernels. Watch them if Trigon or Polykrom are in Rust | P |
| **CGAL** | Straight skeleton (roof generation!), polygon offsetting, arrangements, Kinetic Shape Reconstruction | C (GPL) / commercial |
| **geogram**, **libigl** | Mesh processing, remeshing, parameterization | P / MPL |
| **Clipper2**, **earcut** | 2D polygon booleans/offsets, triangulation (floor plans, wall joins) | P |
| **PlaneGCS** (from FreeCAD) | 2D geometric constraint solver for sketches and plan constraints | L |
| **SolveSpace** solver | Constraint solver | C |

## 3. Import/export (the "super wide range of formats" requirement)

| Format family | Open tooling | Notes |
|---|---|---|
| IFC 2x3 / 4 / 4.3 / 5 | IfcOpenShell, web-ifc | Must be first-class, round-trip safe |
| DXF | **ezdxf** (P), netDxf (P), libdxfrw (C) | ezdxf is excellent for writing drawings |
| DWG | **LibreDWG** (C, GPL-3), ODA File Converter (free, closed), ODA SDK (commercial) | No permissive DWG reader exists. Plan to license ODA, or run LibreDWG out-of-process |
| RVT / RFA (Revit) | None open. Use ODA BimRv SDK (commercial), Speckle connector, or require IFC export | Be honest about this in marketing |
| PDF (vector) | PDFium (P), MuPDF (C/commercial), pdf.js (P), pypdfium2 (P) | Vector PDF plans → extract linework and text directly, no ML needed |
| PDF/raster scans | OpenCV (P), Tesseract/PaddleOCR (P), potrace (C) | See §5 |
| SVG | resvg/usvg (P), lunasvg (P) | |
| STEP / IGES / BREP | OCCT | Fabrication, MEP, product handoff to Polykrom |
| 3DM (Rhino) | **rhino3dm / openNURBS** (P-ish) | Architects use Rhino heavily |
| SKP (SketchUp) | SketchUp C SDK (free, closed) | |
| glTF / GLB | tinygltf, fastgltf, cgltf (P) | Primary visualization export |
| 3D Tiles / I3S | Cesium 3D Tiles tooling (P), py3dtiles (P?) | City-scale streaming |
| OBJ / FBX / PLY / STL / DAE | Assimp (P), **ufbx** (P) | |
| USD / USDZ | OpenUSD (P, Apache-mod) | Apple RoomPlan exports USDZ; Omniverse/visualization |
| CityGML / CityJSON | **cjio** (P), citygml4j (P), CityJSON spec | LoD1–LoD3 city models |
| E57 / LAS / LAZ / COPC | libE57Format (P), **PDAL** (P), LASzip (L), laz-perf (P) | Scans and national point clouds |
| GeoJSON / GPKG / SHP / GeoTIFF / COG | **GDAL/OGR** (P), PROJ (P) | All GIS inputs; CRS handling (EPSG:25832 for DK) |
| gbXML | gbxml schema, Ladybug/OpenStudio | Energy-analysis exchange |
| Speckle objects | Speckle SDKs (P) | Interop hub |

## 4. Photogrammetry, scans and 3D reconstruction from imagery

**Classical SfM/MVS**
- **COLMAP** (P) / **GLOMAP** (P): SfM baseline; GLOMAP is the faster global variant.
- **OpenMVG + OpenMVS** (MPL / AGPL!), **AliceVision/Meshroom** (MPL-2), **OpenDroneMap** (AGPL, run as a service).
- **nerfstudio** (P), **gsplat** (P): NeRF and Gaussian splatting. **2DGS**, **PGSR**, **GOF** give
  surface-accurate splats you can mesh (licenses vary, often NC; verify).

**Feed-forward "foundation" reconstruction** (big shift since 2024: poses + depth from a few photos in one pass)
- **VGGT** (Meta): multi-view pointmaps, depth, pose. Original weights NC; check whether a commercial checkpoint exists.
- **MapAnything** (Meta, 3DV 2026): one model for 12+ reconstruction tasks that also accepts
  intrinsics, poses and depth as optional inputs, which suits fusing skråfoto metadata *(checked; license ?)*.
- **DUSt3R / MASt3R** (Naver): CC BY-NC (NC). **π³**: VGGT variant without a reference frame.
- **Depth Anything 3** (ByteDance): code Apache-2.0, some weights reportedly CC BY-NC *(checked: mixed)*.
- **Depth Anything V2** (small model Apache, larger NC), **Metric3D v2**, **UniDepth**, **MoGe** (MIT): monocular metric depth.

**Point cloud processing**
- **PDAL** (P), **Open3D** (P), **CloudCompare** (C, run as a tool), **Potree** (P, web viewer), **PCL** (P).
- Plane/primitive extraction: Open3D RANSAC, **Efficient RANSAC** (CGAL, C), **pyransac3d** (P).

## 5. Old plans → structured floor plans (raster/vector → walls, rooms, openings)

| Project / dataset | Notes | Lic |
|---|---|---|
| **CubiCasa5K** (dataset + model) | 5k annotated Finnish plans, 80+ categories. Standard baseline | NC |
| **Raster-to-Graph** (2024) | Autoregressive transformer → wall-junction graph. Includes a dataset of 10k+ plans *(checked)* | ? |
| **FloorplanTransformation** (Raster-to-Vector, 2017) | Classic integer-programming vectorizer *(checked: MIT)* | P |
| **MLSTRUCT-FP** | Wall polygon extraction dataset + tools *(checked)* | ? |
| **RoomFormer**, **PolyRoom**, **HEAT**, **FloorSP** | Room polygons from density maps or point clouds (scan → plan) | ? |
| **FloorPlanCAD**, **ArchCAD-400K**, **CADTransformer**, **SymPoint** | Symbol spotting in CAD drawings (doors, windows, fixtures) | ? |
| **MSD (Modified Swiss Dwellings)**, **Swiss Dwellings**, **ResPlan**, **RPLAN**, **Structured3D** | Training/eval data for plan understanding and generation. RPLAN is restricted | ? |
| **House-GAN++**, **Graph2Plan**, **HouseDiffusion** | Generative layout: room graph → plan | ? |
| OCR: **PaddleOCR** (P), **docTR** (P), **Tesseract** (P), **Florence-2** (MIT), **Qwen2.5/3-VL** (Apache, most sizes) | Room labels, dimension strings, title blocks, scale bars | P |
| Line detection: **DeepLSD**, **HAWP**, OpenCV LSD | Wall linework from scans | ? |

Key insight: **dimension strings on old plans are the ground truth for scale.** Parse them with a
VLM or OCR and use them as hard constraints in a solver. Don't trust pixel scale.

## 6. Facades: parsing, rectification and LoD3

**Datasets for facade element segmentation** (windows, doors, balconies, shops, cornices)
- CMP Facade, ECP2011, eTRIMS, Graz50, ArtDeco, LabelMeFacade, ZJU Facade, **Oxford RobotCar Facade** (windows, balconies, doors; public) *(checked)*,
  **CFP / RTFP** (602 HR images) *(checked)*, Hangzhou HR facade dataset (2025, 43k windows; license ?) *(checked)*.
- **TUM-FAÇADE**, **TUM2TWIN**: point clouds labelled with facade elements; LoD3 benchmark.
- **Japan PLATEAU**: open CityGML LoD1–LoD3 for many cities. Real LoD3 facades with openings; probably the best open training/eval target for "LoD2 → LoD3".

**Models**
- Generic open-vocabulary detection/segmentation: **SAM 2 / SAM 3** (Meta, check license),
  **Grounding DINO** (P), **Florence-2** (P), **RF-DETR / D-FINE / RT-DETR** (P), Mask2Former (P).
  Avoid **Ultralytics YOLO** (AGPL) unless licensed.
- Facade-specific: Facade R-CNN, DeepFacade, and regularity priors (windows lie on grids: exploit row/column alignment).

**LoD2 → LoD3 pipelines (research code)**
- **LOD3-Model** (Xia & Stoter, TU Delft): oblique-imagery openings projected onto LoD2. Code on GitHub *(checked)*. Very close to the "skråfoto → facade" use case.
- **Scan2LoD3** (Wysocki, TUM): fuses point cloud, images and LoD2 for openings.
- **LoD2BIM** (Xia & Rüppel): CityGML LoD2 → IFC via Blender *(checked)*.
- Hong Kong LoD2→LoD3 with BIM component-library retrieval (ISPRS 2026) *(checked)*: the same
  "detect → retrieve parametric component" idea Trigon wants.
- Spherical street imagery + OSM → LoD3 (ISPRS 2026) *(checked)*.

**Rectification:** vanishing-point detection (e.g. **NeurVPS**, **VP-detection** libs), homography
from skråfoto camera models, and texture atlas extraction.

## 7. Roofs and building shells (LoD1/LoD2)

- **roofer / geoflow** (TU Delft 3D geoinformation): engine behind **3DBAG**, footprints + point cloud → LoD1.2/1.3/2.2 *(license ?)*.
- **3dfier** (P?): footprints + lidar → extruded LoD1.
- **City3D**, **PolyFit** (Liangliang Nan; GPL), **KSR** (CGAL).
- **Building3D**, **RoofN3D**, **BuildingNet**: roof/building datasets.
- **CGAL straight skeleton**: hip/gable roof generation from footprints (parametric roof tool).
- **3DBAG** (NL) is the reference product to emulate for Denmark.

## 8. Procedural and parametric architecture (window/door/balcony families, facade grammars)

- **CGA-style shape grammars:** **symbios-shape** (Rust, MIT, most complete open CGA engine) *(checked)*,
  **ShapeML** (C++), JS CGA parser. Use a split grammar for facade layout (floors → bays → openings).
- **Blender:** **Building Tools** (C), **Buildify** (geometry nodes), **Archipack** (C), **Sverchok** (C).
  Treat these as references for parameter sets, not as code to ship.
- **Bonsai/FreeCAD parametric door & window presets:** good reference parameter schemas
  (IfcWindowLiningProperties, IfcDoorLiningProperties, panel operations).
- **Topologic** (cell complexes for building topology: spaces, adjacency): AGPL ?, ideas are gold.
- **Grasshopper ecosystem ideas** (Ladybug, Karamba, Hops). Trigon's node graph could cover similar ground.
- Component libraries: there is no great open parametric window/door library. **Building one is a
  Trigon moat.** Parametric families: casement, tilt-turn, sash, pivot, fixed, French door, sliding,
  bay/oriel, dormer, Juliet/French balcony, cantilever balcony, loggia, railings by pattern.

## 9. Drawings: plans, sections, elevations, sheets

- **IfcOpenShell draw / Bonsai Drawings**: SVG plans/sections/elevations from IFC with annotation, sheets and schedules. The best open reference.
- **OCCT HLR / HLRBRep**: exact hidden-line removal for B-rep.
- **FreeCAD TechDraw** (L): section views, hatching, dimensions.
- **That Open 3.4 drawings** *(checked)*: web drawing system.
- Output: **ezdxf** (DXF), PDF via Skia/PDFium/cairo, SVG.
- Standards to encode: ISO 128 (lines), ISO 7519/ISO 4157 (building drawings), ISO 13567 (CAD layers),
  Danish **Molio/bips** conventions and **CCS** classification, Uniclass/OmniClass for export.

## 10. Engineering and analysis (civil and structural hooks)

- Structural: **OpenSees / OpenSeesPy** (BSD-ish), **Code_Aster** (C), **Kratos** (BSD), **PyNite** (MIT), **CalculiX** (C).
- Energy/daylight: **EnergyPlus** (BSD-ish), **OpenStudio** (BSD), **Radiance** (BSD-ish), **Ladybug Tools** (AGPL/GPL ?).
- Civil/infra: IFC 4.3 alignment (IfcOpenShell), **QGIS** (C, as a reference/tool), **GRASS**, **WhiteboxTools**, PDAL for terrain.
- Code checking: IDS + IfcTester. Danish **BR18** rule sets are an opportunity (not available open source).

## 11. App platform (viewer, UI, collaboration)

- Web 3D: **three.js** (P), **Babylon.js** (P), **CesiumJS** (P), **deck.gl** (P), **iTowns**, **MapLibre** (P).
- Native/Rust: **wgpu**, **Bevy**, **rend3**. For a desktop shell: Tauri (P) or Electron.
- Collaboration/versioning: **Yjs / Automerge** (CRDT), **IfcGit**-style diffs, Speckle commits.
- Agent integration: **ifcopenshell-mcp** shows how an LLM can query and edit models through tools.
  Trigon should expose its whole model API as MCP tools from day one.

## 12. Data sources (beyond Google Earth)

**Denmark (strongest open-data stack in the world for this)**
- **Skråfoto** (SDFI): ~5M oblique images, ~10 cm, served as **STAC + COG**; frontend and API are open source *(checked)*. Images include camera parameters, so you can do direct photogrammetry per facade.
- **DHM** (Danmarks Højdemodel): national lidar point cloud + DSM/DTM.
- **GeoDanmark** building footprints and orthophotos (spring and summer).
- **BBR** (via Datafordeler; INSPIRE GeoPackage on Dataforsyningen, dataset 3829): year built, storeys, areas, roof/wall material, units. Note: **DAWA stopped serving BBR in April 2024** *(checked)*.
- **Matrikel** (cadastre), **DAR** (addresses), **FBB** (listed/worthy buildings with SAVE values).
- **Municipal building-case archives** (byggesagsarkiver, Weblager etc.): scanned historic plans,
  sections and elevations for millions of buildings. Arguably Trigon's single most valuable source.

**International open**
- **OpenStreetMap** (ODbL, share-alike on derived databases), **Overture Maps** buildings, **Google/Microsoft Open Buildings** footprints, **EUBUCCO** (EU building stock attributes).
- LoD2 open data: Germany (NRW, Bavaria, Berlin, Hamburg…), **swissBUILDINGS3D**, **3DBAG** (NL), Helsinki, Vienna, **PLATEAU** (JP LoD3), NYC.
- National lidar: AHN (NL), swisstopo, many German states, USGS 3DEP.
- Street-level: **Mapillary** (CC BY-SA), **Panoramax** (open, self-hostable), KartaView.

**User-captured**
- Phone photos (feed-forward recon), iPhone LiDAR / **Apple RoomPlan** (USDZ, closed API but on-device), drone flights (ODM), terrestrial scans (E57), tape-measure sketches.

**Restricted, do not build on**
- **Google Earth / Photorealistic 3D Tiles / Street View**: the Maps Platform ToS forbids extracting,
  caching, or creating content from them, and reportedly bans ML training *(checked: secondhand
  forum readings of the ToS; get legal review)*. Use them only as a live visual reference layer inside the app.
- Real-estate listing floor plans: copyrighted.

## 13. Commercial landscape to study (not open source)

Revit, ArchiCAD, Vectorworks, Rhino+VisualARQ, **Snaptrude**, **Arcol**, **Motif**, **Qonic**, **Hypar**, Archistar, **Matterport** (scan → BIM),
**Pointfuse**, **Plannerly**, **Augmenta**, Esri **CityEngine** (CGA), Bentley iTwin (whose iTwin.js is open source, MIT, and worth studying).

## 14. Rust crates that fit the Polykrom stack (checked on crates.io, 2026-10-31)

| Need | Crate | License | Notes |
|---|---|---|---|
| IFC (codegen route) | **ruststep / espr** | Apache-2.0 | EXPRESS → Rust compiler + P21, by RICOS (truck's authors). Last release 2024-09 |
| IFC (ready-made) | **ifc-lite** (core, geometry, wasm) | MPL-2.0 | Very active (updated 2026-10-30). MPL is file-level copyleft, so it needs a policy decision |
| IFC (alt) | ifc_rs | MIT | Alpha, last release 2024-12 |
| 2D polygon booleans/offsets | **i_overlay** | MIT/Apache | 11M downloads; walls, openings, slabs |
| Roof straight skeleton | geo-straight-skeleton | MIT/Apache | Released 2026-10, tiny usage. Evaluate or write our own |
| Geometry/GIS primitives | geo, geojson, spade (CDT) | MIT/Apache | |
| CRS | proj4rs | MIT/Apache | Pure Rust; EPSG:25832 |
| STAC (skråfoto) | **stac** (rustac) | MIT/Apache | Active |
| Point clouds | las, laz, copc-rs, **wblidar** (LAS/LAZ/COPC/PLY/E57) | MIT/Apache | wblidar is new (Whitebox next-gen) |
| DXF | **dxf** (ixmilia) | MIT | Read/write, active |
| glTF | gltf (already in Polykrom) | MIT/Apache | |
| ML inference | **ort** (ONNX Runtime), candle | MIT/Apache | CoreML/DirectML execution providers |
| PDF raster/vector | pdfium-render (PDFium BSD/Apache), lopdf | MIT/Apache | Archive plans |
| USD | usd (bindings, C++) | ? | Immature; PolyCore RFC 0002 plans its own USD path |

## 15. Kernel claims checked (2026-10-31)

An AI-generated overview of Rust and BIM kernels was checked against crates.io, GitHub and web
search. Three of its projects could not be found and are treated as non-existent until someone
shows a repository: **Aetheris**, **Spare Kit** and **ConstructCAD `cc_geom`**.

| Project | Found | Licence | First release / last update | Downloads | Fit for Trigon |
|---|---|---|---|---|---|
| solidcore | crates.io 1.0.1 | Apache-2.0 | 2026-10-13 / 2026-10-30 | 55 | Already in PolyCore's plan (port source, watched) |
| vcad / vcad_kernel | crates.io 0.10.0 | MIT (PolyCore's survey read Apache-2.0 in the repo) | 2026-01 / 2026-09 | 689 | PolyCore survey: skip (broad, no evidence on blends or SSI) |
| **arris** | crates.io 0.9.1 | MIT OR Apache-2.0 | 2026-09-05 / 2026-10-24 | 232 | New, not in PolyCore's survey: B-rep with tolerance-carrying topology, booleans with provenance, blends, STEP. Bench it |
| **geomcore** | crates.io 0.42.0 | MIT OR Apache-2.0 | 2026-07-02 / 2026-10-30 | 549 | New, not in the survey: curves and surfaces only. Bench its evaluators |
| BREP_kernel (next.BREP) | crates.io 0.11.0 | Custom (modifications assigned to Autodrop3d) | 2026-08 / 2026-10 | 579 | Excluded: not open source in our sense |
| ifc-lite (core, geometry) | crates.io 27.1.0 | MPL-2.0 | 2026-01 / 2026-10-30 | 35k | IFC import candidate; MPL policy decision still open |
| bimifc (geometry) | crates.io 0.3.0 | MPL-2.0 | 2026-03 / 2026-05 | 588 | Second IFC reader to compare; slower-moving |
| OpenSCAD | GitHub | GPL-2.0 | | | Code-first CSG. Can't be linked; its CSG speed comes from Manifold (Apache-2.0), already on PolyCore's list |
| O3DE | o3de.org | Apache-2.0 OR MIT | | | C++ game engine. Not a kernel; at most a glTF/USD visualisation target |
