//! The command list. Add a command by adding a params struct, a function and one entry in
//! [`COMMANDS`]; the CLI and MCP pick it up from there.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, PhaseFilter, View, even_bays, resolve_facade};
use trigon_model::{Building, ComponentType, Document, FacadeKind, ModelError, Placement, Site};

use crate::{EngineError, Result, Session, time};

pub struct CommandSpec {
    /// Dotted lower-case id, `<area>.<action>`.
    pub id: &'static str,
    pub summary: &'static str,
    /// Changes the document (it is snapshotted and restored on failure).
    pub mutates: bool,
    /// Recorded in the journal and the undo history.
    pub journaled: bool,
    pub schema: fn() -> Value,
    pub run: fn(&mut Session, &Value) -> Result<Value>,
}

impl CommandSpec {
    pub fn params_schema(&self) -> Value {
        (self.schema)()
    }
}

fn schema<T: JsonSchema>() -> Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| json!({"type": "object"}))
}

fn parse<T: DeserializeOwned>(id: &str, v: &Value) -> Result<T> {
    serde_json::from_value(v.clone()).map_err(|e| EngineError::Params { command: id.to_string(), message: e.to_string() })
}

pub fn commands() -> &'static [CommandSpec] {
    COMMANDS
}

static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "document.info",
        summary: "Summarise the document: site, buildings, storeys, facades, placements and the component types.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| parse::<NoParams>("document.info", v).map(|_| info(&s.doc)),
    },
    CommandSpec {
        id: "document.new",
        summary: "Start an empty document (the default component library included).",
        mutates: true,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("document.new", v)?;
            s.doc = Document::default();
            s.path = None;
            s.dirty = false;
            s.reset_history();
            Ok(json!({"ok": true}))
        },
    },
    CommandSpec {
        id: "document.open",
        summary: "Open a .trigon document.",
        mutates: true,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("document.open", v)?;
            let path = checked_path(&p.path, "trigon")?;
            s.doc = read_document(&path)?;
            s.path = Some(path);
            s.dirty = false;
            s.reset_history();
            Ok(info(&s.doc))
        },
    },
    CommandSpec {
        id: "document.save",
        summary: "Save the document as a .trigon file (to `path`, or where it was opened from).",
        mutates: false,
        journaled: false,
        schema: schema::<OptPathParams>,
        run: |s, v| {
            let p: OptPathParams = parse("document.save", v)?;
            let path = match (p.path, &s.path) {
                (Some(p), _) => checked_path(&p, "trigon")?,
                (None, Some(p)) => p.clone(),
                (None, None) => return Err(EngineError::Io("the document has no file yet; give a path".into())),
            };
            let text = serde_json::to_string_pretty(&s.doc).map_err(|e| EngineError::Io(e.to_string()))?;
            write_file(&path, text.as_bytes())?;
            s.path = Some(path.clone());
            s.dirty = false;
            Ok(json!({"saved": path}))
        },
    },
    CommandSpec {
        id: "edit.undo",
        summary: "Undo the last model change.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("edit.undo", v)?;
            Ok(json!({"undone": s.undo()}))
        },
    },
    CommandSpec {
        id: "edit.redo",
        summary: "Redo the last undone model change.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("edit.redo", v)?;
            Ok(json!({"redone": s.redo()}))
        },
    },
    CommandSpec {
        id: "site.load_address",
        summary: "Find a Danish address and create its site and building: outline, storeys from the elevation model, one facade per wall. \
                  Sources are recorded as evidence. The document must be empty (document.new).",
        mutates: true,
        journaled: true,
        schema: schema::<LoadAddress>,
        run: |s, v| load_address(s, parse("site.load_address", v)?),
    },
    CommandSpec {
        id: "building.set_storeys",
        summary: "Set the number of storeys, the floor-to-floor height and the ground floor's level above the terrain (mm). \
                  Placements above the new top storey are removed.",
        mutates: true,
        journaled: true,
        schema: schema::<SetStoreys>,
        run: |s, v| {
            let p: SetStoreys = parse("building.set_storeys", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let height = p.height.unwrap_or_else(|| b.storeys.first().map_or(3000.0, |s| s.height));
            let ground = p.ground_floor.unwrap_or(b.ground_floor);
            b.set_storeys_from(p.count, height, ground)?;
            check_building(&s.doc, p.building.as_deref())?;
            Ok(info(&s.doc))
        },
    },
    CommandSpec {
        id: "building.set_levels",
        summary: "Set the storeys from explicit floor levels (mm above the site's terrain, rising) and the top of the last storey \
                  (eaves or attic floor), for storeys of different heights read from a section drawing. `source` records where \
                  the levels came from as evidence. Placements above the new top storey are removed.",
        mutates: true,
        journaled: true,
        schema: schema::<SetLevels>,
        run: |s, v| {
            let p: SetLevels = parse("building.set_levels", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.set_levels(&p.floors, p.top)?;
            if let Some(src) = p.source.filter(|x| !x.trim().is_empty()) {
                b.evidence.push(trigon_model::Evidence { source: src, supplies: "levels".into(), attribution: p.attribution.unwrap_or_default() });
            }
            check_building(&s.doc, p.building.as_deref())?;
            Ok(info(&s.doc))
        },
    },
    CommandSpec {
        id: "building.set_roof",
        summary: "Set the roof: flat, or pitched from the eaves (the top of the walls) at `pitch_deg`, or at the pitch that puts \
                  the ridge at `ridge` (mm above the terrain, e.g. read from a section). `gables` lists the facades that end in a \
                  gable (wing ends, walls against neighbours). `source` records where the values came from.",
        mutates: true,
        journaled: true,
        schema: schema::<SetRoof>,
        run: |s, v| set_roof(s, parse("building.set_roof", v)?),
    },
    CommandSpec {
        id: "building.set_wall_thickness",
        summary: "Set the outer wall thickness (mm).",
        mutates: true,
        journaled: true,
        schema: schema::<SetWallThickness>,
        run: |s, v| {
            let p: SetWallThickness = parse("building.set_wall_thickness", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.wall_thickness = trigon_model::checked("wall thickness", p.thickness, trigon_model::doc::MIN_WALL, trigon_model::doc::MAX_WALL)?;
            check_building(&s.doc, p.building.as_deref())?;
            Ok(json!({"thickness": p.thickness}))
        },
    },
    CommandSpec {
        id: "facade.list",
        summary: "List the facades: id, length, which way it faces, kind, bays and placements.",
        mutates: false,
        journaled: false,
        schema: schema::<BuildingParam>,
        run: |s, v| {
            let p: BuildingParam = parse("facade.list", v)?;
            Ok(json!(facades(&s.doc, s.doc.building(p.building.as_deref())?)))
        },
    },
    CommandSpec {
        id: "facade.set_kind",
        summary: "Classify a facade: street, courtyard, party (shared with a neighbour; no openings) or unknown.",
        mutates: true,
        journaled: true,
        schema: schema::<SetKind>,
        run: |s, v| {
            let p: SetKind = parse("facade.set_kind", v)?;
            let b = s.doc.building_of_facade_mut(&p.facade)?;
            b.facade_mut(&p.facade)?.kind = p.kind;
            let id = b.id.clone();
            check_building(&s.doc, Some(&id))?;
            Ok(json!({"facade": p.facade, "kind": p.kind}))
        },
    },
    CommandSpec {
        id: "facade.set_bays",
        summary: "Set a facade's bay grid: `count` evenly spaced bays (with `margin` mm clear at each end), or explicit `centres` \
                  (mm from the facade start). Placements in bays that no longer exist are removed.",
        mutates: true,
        journaled: true,
        schema: schema::<SetBays>,
        run: |s, v| set_bays(s, parse("facade.set_bays", v)?),
    },
    CommandSpec {
        id: "facade.place",
        summary: "Place a component type in facade slots. `levels` and `bays` are lists of indices (omit for all). Replaces what was \
                  in those slots. Optional width, height and sill (mm) override the type for these slots.",
        mutates: true,
        journaled: true,
        schema: schema::<Place>,
        run: |s, v| place(s, parse("facade.place", v)?),
    },
    CommandSpec {
        id: "facade.demolish",
        summary: "Demolish facade elements in a phase (default: the active phase), as in Revit: they stay in earlier phases, show \
                  red in demolition views and are gone afterwards. `what`: all, openings (windows and doors; the wall closes) or \
                  balconies. Elements built in that same phase are simply removed.",
        mutates: true,
        journaled: true,
        schema: schema::<Demolish>,
        run: |s, v| demolish(s, parse("facade.demolish", v)?),
    },
    CommandSpec {
        id: "phase.list",
        summary: "List the phases (time order), the active phase and how many placements each phase builds and demolishes.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("phase.list", v)?;
            Ok(phase_list(&s.doc))
        },
    },
    CommandSpec {
        id: "phase.set",
        summary: "Set the phases in time order, for example [1938 built, 2026 existing, forslag proposal]. Elements without a \
                  phase belong to the first. Every phase an element refers to must stay in the list.",
        mutates: true,
        journaled: true,
        schema: schema::<PhaseSet>,
        run: |s, v| {
            let p: PhaseSet = parse("phase.set", v)?;
            if p.phases.is_empty() || p.phases.len() > 50 {
                return Err(ModelError::Invalid("give 1 to 50 phases".into()).into());
            }
            for (i, ph) in p.phases.iter().enumerate() {
                if ph.id.trim().is_empty() || p.phases[..i].iter().any(|o| o.id == ph.id) {
                    return Err(ModelError::Invalid(format!("phase ids must be unique and not empty ({:?})", ph.id)).into());
                }
            }
            s.doc.phases = p.phases;
            if s.doc.active_phase.as_deref().is_some_and(|a| s.doc.phase_index(a).is_err()) {
                s.doc.active_phase = None;
            }
            for b in s.doc.buildings.clone() {
                check_building(&s.doc, Some(&b.id))?;
            }
            Ok(phase_list(&s.doc))
        },
    },
    CommandSpec {
        id: "phase.activate",
        summary: "Make a phase the active one: new placements are built in it and views default to it.",
        mutates: true,
        journaled: true,
        schema: schema::<PhaseId>,
        run: |s, v| {
            let p: PhaseId = parse("phase.activate", v)?;
            s.doc.phase_index(&p.id)?;
            s.doc.active_phase = Some(p.id);
            Ok(phase_list(&s.doc))
        },
    },
    CommandSpec {
        id: "facade.clear",
        summary: "Remove placements from facade slots (`levels` and `bays` omitted = all).",
        mutates: true,
        journaled: true,
        schema: schema::<Clear>,
        run: |s, v| {
            let p: Clear = parse("facade.clear", v)?;
            let f = s.doc.building_of_facade_mut(&p.facade)?.facade_mut(&p.facade)?;
            let before = f.placements.len();
            f.placements.retain(|pl| !(selected(&p.levels, pl.level) && selected(&p.bays, pl.bay)));
            Ok(json!({"removed": before - f.placements.len()}))
        },
    },
    CommandSpec {
        id: "types.list",
        summary: "List the component types (parametric windows, doors and balconies) with their parameters.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("types.list", v)?;
            Ok(serde_json::to_value(&s.doc.types).unwrap_or(Value::Null))
        },
    },
    CommandSpec {
        id: "types.define",
        summary: "Add a component type or replace one with the same id. Every facade using it is re-checked.",
        mutates: true,
        journaled: true,
        schema: schema::<ComponentType>,
        run: |s, v| {
            let t: ComponentType = parse("types.define", v)?;
            t.validate()?;
            match s.doc.types.iter_mut().find(|x| x.id == t.id) {
                Some(x) => *x = t.clone(),
                None => s.doc.types.push(t.clone()),
            }
            for b in s.doc.buildings.clone() {
                check_building(&s.doc, Some(&b.id))?;
            }
            Ok(json!({"defined": t.id}))
        },
    },
    CommandSpec {
        id: "evidence.photos",
        summary: "List the Skråfoto oblique photos covering the building, and for each facade the photos whose camera looks at it.",
        mutates: false,
        journaled: false,
        schema: schema::<BuildingParam>,
        run: |s, v| photos(s, parse("evidence.photos", v)?),
    },
    CommandSpec {
        id: "evidence.facade_image",
        summary: "Straighten a facade from the Skråfoto photo that shows it best: an orthographic, to-scale PNG of the wall from \
                  the ground to the eaves (left = facade start), with the model's storey lines (cyan), bay lines (yellow) and \
                  openings (magenta) drawn on it. Look at it before setting bays and placing windows, and again after.",
        mutates: false,
        journaled: false,
        schema: schema::<FacadeImageParams>,
        run: |s, v| {
            let p: FacadeImageParams = parse("evidence.facade_image", v)?;
            let out = p.path.as_deref().map(|x| checked_path(x, "png")).transpose()?;
            if let Some(d) = p.drawing.as_deref() {
                if p.photo.is_some() {
                    return Err(EngineError::Params { command: "evidence.facade_image".into(), message: "give photo or drawing, not both".into() });
                }
                let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
                return crate::drawings::facade_image(s, &p.facade, d, p.mm_per_px.unwrap_or(50.0), p.overlay.unwrap_or(true), out, view);
            }
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let img =
                crate::facade_image::render(s, &p.facade, p.photo.as_deref(), p.mm_per_px.unwrap_or(50.0), p.overlay.unwrap_or(true), out, view)?;
            Ok(json!({
                "image_path": img.path,
                "photo": img.photo.id,
                "camera_looks": img.photo.direction,
                "taken": img.photo.datetime,
                "width_px": img.width,
                "height_px": img.height,
                "mm_per_px": img.mm_per_px,
                "facade_length_mm": img.length_mm.round(),
                "facade_height_mm": img.height_mm.round(),
                "photo_coverage": (img.coverage * 1000.0).round() / 1000.0,
                "how_to_read": "x = mm along the facade from its start / mm_per_px; y = (facade height − height above ground) / mm_per_px. \
                                Trees, other buildings and the roof overhang can hide parts of the wall.",
            }))
        },
    },
    CommandSpec {
        id: "drawing.add",
        summary: "Link a page of a scanned drawing PDF (plans, sections, elevations from an archive) as an underlay. Only the link \
                  is stored. Then read coordinates with drawing.view and tie elevations to facades with drawing.register_facade.",
        mutates: true,
        journaled: true,
        schema: schema::<DrawingAdd>,
        run: |s, v| {
            let p: DrawingAdd = parse("drawing.add", v)?;
            let path = checked_path(&p.path, "pdf")?;
            crate::drawings::add(s, &path, p.page.unwrap_or(1), p.name, p.rotate.unwrap_or(0), p.note)
        },
    },
    CommandSpec {
        id: "drawing.list",
        summary: "List the linked drawings and the facades registered on them.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("drawing.list", v)?;
            Ok(serde_json::to_value(&s.doc.drawings).unwrap_or(Value::Null))
        },
    },
    CommandSpec {
        id: "drawing.view",
        summary: "Look at a linked drawing or a region of it, with grid lines labelled in drawing pixels, to read positions \
                  (facade ends, floor lines, window edges) for drawing.register_facade and for measuring.",
        mutates: false,
        journaled: false,
        schema: schema::<DrawingView>,
        run: |s, v| {
            let p: DrawingView = parse("drawing.view", v)?;
            let out = p.path.as_deref().map(|x| checked_path(x, "png")).transpose()?;
            crate::drawings::view(s, &p.name, p.region, p.max_px.unwrap_or(1600), p.grid, out)
        },
    },
    CommandSpec {
        id: "drawing.register_facade",
        summary: "Tie an elevation on a linked drawing to a facade: the drawing x of the facade's start and end, and two drawing y \
                  positions with their heights above the terrain (mm). Then evidence.facade_image with `drawing` shows the \
                  drawing straightened to the facade with the model over it.",
        mutates: true,
        journaled: true,
        schema: schema::<DrawingRegister>,
        run: |s, v| {
            let p: DrawingRegister = parse("drawing.register_facade", v)?;
            let reg = trigon_model::FacadeRegistration { facade: p.facade, x_start: p.x_start, x_end: p.x_end, y_levels: p.y_levels };
            crate::drawings::register(s, &p.name, reg)
        },
    },
    CommandSpec {
        id: "building.outline",
        summary: "The building's outline corners in site mm (x east, y north of the site origin) with the facades meeting at each, \
                  for tying drawings to the site.",
        mutates: false,
        journaled: false,
        schema: schema::<BuildingParam>,
        run: |s, v| {
            let p: BuildingParam = parse("building.outline", v)?;
            let b = s.doc.building(p.building.as_deref())?;
            let n = b.footprint.len();
            Ok(json!(
                b.footprint
                    .iter()
                    .enumerate()
                    .map(|(i, c)| json!({
                        "corner": i,
                        "x": c[0].round(),
                        "y": c[1].round(),
                        "ends": b.facades.iter().find(|f| (f.edge + 1) % n == i).map(|f| f.id.clone()),
                        "starts": b.facades.iter().find(|f| f.edge == i).map(|f| f.id.clone()),
                    }))
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "drawing.register_plan",
        summary: "Tie a plan on a linked drawing to the site for the storeys it shows: two or more points, each a drawing pixel \
                  (px, py) and either a site point (x, y in mm) or a facade point (facade, u = mm along it from its start, on the \
                  outer face). Fits scale, rotation and shift and reports the residual at each point.",
        mutates: true,
        journaled: true,
        schema: schema::<RegisterPlan>,
        run: |s, v| register_plan(s, parse("drawing.register_plan", v)?),
    },
    CommandSpec {
        id: "plan.image",
        summary: "Draw a floor plan of a storey, cut 1 m above its floor: walls, windows, door swings and balconies with their \
                  size (\"Ca. depth x width\"), coloured by phase (green new, red dashed demolished), the model only (set \
                  show_scan to put the registered archive scan underneath for checking). `path` ending in .png gives an image (returned to agents), .svg a vector drawing.",
        mutates: false,
        journaled: false,
        schema: schema::<PlanImage>,
        run: |s, v| {
            let p: PlanImage = parse("plan.image", v)?;
            let path = PathBuf::from(&p.path);
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::plan::render(
                s,
                p.level,
                p.drawing.as_deref(),
                p.show_scan.unwrap_or(false),
                view,
                p.mm_per_px.unwrap_or(20.0),
                p.rotate_deg.unwrap_or(0.0),
                path,
            )
        },
    },
    CommandSpec {
        id: "export.gltf",
        summary: "Export the model as glTF 2.0 binary (.glb), metres, Y up. `phase` and `filter` choose the view (default: the \
                  active phase, complete); demolished elements are translucent red.",
        mutates: false,
        journaled: false,
        schema: schema::<ExportParams>,
        run: |s, v| {
            let p: ExportParams = parse("export.gltf", v)?;
            let path = checked_path(&p.path, "glb")?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let refs = reference_meshes(&s.doc)?;
            let bytes = trigon_io::glb::write(&trigon_model::build::build_view(&s.doc, view)?, &refs);
            write_file(&path, &bytes)?;
            Ok(json!({"written": path, "bytes": bytes.len()}))
        },
    },
    CommandSpec {
        id: "export.ifc",
        summary: "Export the model as IFC4 (.ifc), georeferenced to EPSG:25832. `phase` and `filter` choose the view; with \
                  phases, every window, door, railing and balcony carries Pset_*Common.Status (EXISTING, NEW, DEMOLISH).",
        mutates: false,
        journaled: false,
        schema: schema::<ExportParams>,
        run: |s, v| {
            let p: ExportParams = parse("export.ifc", v)?;
            let path = checked_path(&p.path, "ifc")?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("model.ifc").to_string();
            let text = trigon_io::ifc::write(&s.doc, &trigon_model::build::build_view(&s.doc, view)?, &name, &time::now_utc());
            write_file(&path, text.as_bytes())?;
            Ok(json!({"written": path, "bytes": text.len()}))
        },
    },
    CommandSpec {
        id: "ifc.check",
        summary: "Read an IFC file with two independent readers (ifc-lite and bimifc), compare them element by element, and compare \
                  both with this document's model where GlobalIds match. Reports counts, failures and bounding-box deviations (mm).",
        mutates: false,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("ifc.check", v)?;
            ifc_check(&s.doc, &checked_path(&p.path, "ifc")?)
        },
    },
    CommandSpec {
        id: "reference.link",
        summary: "Link an IFC model (neighbours, an architect's model, a survey) to show with this one. It is placed through its \
                  georeference when it has one in EPSG:25832, otherwise at the site origin. Only the link is stored.",
        mutates: true,
        journaled: true,
        schema: schema::<LinkParams>,
        run: |s, v| link_reference(s, parse("reference.link", v)?),
    },
    CommandSpec {
        id: "reference.list",
        summary: "List the linked IFC models.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("reference.list", v)?;
            Ok(serde_json::to_value(&s.doc.references).unwrap_or(Value::Null))
        },
    },
    CommandSpec {
        id: "reference.remove",
        summary: "Unlink an IFC model by name.",
        mutates: true,
        journaled: true,
        schema: schema::<NameParams>,
        run: |s, v| {
            let p: NameParams = parse("reference.remove", v)?;
            let before = s.doc.references.len();
            s.doc.references.retain(|r| r.name != p.name);
            if s.doc.references.len() == before {
                return Err(ModelError::NotFound(format!("reference {}", p.name)).into());
            }
            Ok(json!({"removed": p.name}))
        },
    },
];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FacadeImageParams {
    facade: String,
    /// A Skråfoto photo id from evidence.photos; omit to use the photo that shows the facade largest.
    photo: Option<String>,
    /// Image scale, mm per pixel (default 50).
    mm_per_px: Option<f64>,
    /// Draw storeys, bays and openings over the photo (default true).
    overlay: Option<bool>,
    /// Where to write the PNG; default `trigon-evidence/` next to the document.
    path: Option<String>,
    /// Use a linked drawing (registered to this facade with drawing.register_facade) instead of a photo.
    drawing: Option<String>,
    /// Phase to show (default: the active phase).
    phase: Option<String>,
    /// complete (default), demolition, new or all.
    filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportParams {
    path: String,
    /// Phase to export (default: the active phase).
    phase: Option<String>,
    /// complete (default), demolition, new or all.
    filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PhaseSet {
    /// The phases in time order, for example 1938 (built), 2026 (existing), forslag (proposal).
    phases: Vec<trigon_model::Phase>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PhaseId {
    id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Demolish {
    facade: String,
    levels: Option<Vec<usize>>,
    bays: Option<Vec<usize>>,
    /// all (default), openings or balconies.
    what: Option<String>,
    /// Phase the demolition happens in (default: the active phase).
    phase: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DrawingAdd {
    /// A PDF of scanned drawings.
    path: String,
    /// 1-based page (default 1).
    page: Option<u32>,
    /// Name to refer to it by; defaults to "<file> p<page>".
    name: Option<String>,
    /// Clockwise rotation that makes the scan upright: 0, 90, 180 or 270 (default 0).
    rotate: Option<u32>,
    /// Title, archive number, scale.
    note: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DrawingView {
    name: String,
    /// [x0, y0, x1, y1] in drawing pixels; omit for the whole sheet.
    region: Option<[f64; 4]>,
    /// Longest side of the image, pixels (default 1600).
    max_px: Option<u32>,
    /// Grid spacing in drawing pixels (default: about ten lines).
    grid: Option<f64>,
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DrawingRegister {
    name: String,
    facade: String,
    /// Drawing x (pixels) where the facade starts (its first footprint corner).
    x_start: f64,
    /// Drawing x where the facade ends.
    x_end: f64,
    /// Two [drawing y in pixels, height above the terrain in mm] pairs, for example the eaves and a floor line.
    y_levels: [[f64; 2]; 2],
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanPoint {
    px: f64,
    py: f64,
    x: Option<f64>,
    y: Option<f64>,
    facade: Option<String>,
    u: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegisterPlan {
    name: String,
    /// Storey indices the plan shows.
    levels: Vec<usize>,
    points: Vec<PlanPoint>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanImage {
    level: usize,
    /// .png or .svg
    path: String,
    /// A drawing with a plan registered for this level (default: the first one found).
    drawing: Option<String>,
    /// Draw the registered archive scan under the plan, for checking and tracing (default false: output material
    /// shows only the model).
    show_scan: Option<bool>,
    phase: Option<String>,
    filter: Option<String>,
    /// Scale, mm per pixel (default 20).
    mm_per_px: Option<f64>,
    /// Rotate the drawing counter-clockwise by this many degrees (0 = north up).
    rotate_deg: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NameParams {
    name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LinkParams {
    /// Path to an .ifc file.
    path: String,
    /// Name to show; defaults to the file name.
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathParams {
    /// File path.
    path: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OptPathParams {
    /// File path; omit to save where the document came from.
    path: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BuildingParam {
    /// Building id; omit for the first building.
    building: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LoadAddress {
    /// A Danish address, for example "Frederikssundsvej 28, 2400 København".
    address: String,
    /// Storey count; omit to estimate it from the elevation model.
    storeys: Option<usize>,
    /// Floor-to-floor height, mm (default 3000).
    storey_height: Option<f64>,
    /// Outer wall thickness, mm (default 400).
    wall_thickness: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetStoreys {
    building: Option<String>,
    count: usize,
    /// Floor-to-floor height, mm; omit to keep the current height.
    height: Option<f64>,
    /// Floor level of the ground storey above the terrain, mm (raised ground floor); omit to keep it.
    ground_floor: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetLevels {
    building: Option<String>,
    /// Floor level of each storey, mm above the terrain, rising (ground floor first).
    floors: Vec<f64>,
    /// Top of the last storey (eaves or attic floor), mm above the terrain.
    top: f64,
    /// Where the levels come from, for example a drawing's archive number.
    source: Option<String>,
    attribution: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRoof {
    building: Option<String>,
    /// flat or pitched.
    kind: String,
    /// Roof pitch in degrees.
    pitch_deg: Option<f64>,
    /// Ridge height, mm above the terrain; the pitch is worked out from it when `pitch_deg` is omitted.
    ridge: Option<f64>,
    /// Facades that end in a gable.
    gables: Option<Vec<String>>,
    /// Facade steps up to this deep (mm) get no roof faces of their own; the eave runs straight (default 1200, 0 = follow every step).
    straighten_mm: Option<f64>,
    source: Option<String>,
    attribution: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetWallThickness {
    building: Option<String>,
    thickness: f64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetKind {
    facade: String,
    kind: FacadeKind,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetBays {
    facade: String,
    /// Number of evenly spaced bays.
    count: Option<usize>,
    /// Explicit bay centre lines, mm from the facade start.
    centres: Option<Vec<f64>>,
    /// Clear wall at each end for evenly spaced bays, mm (default 600).
    margin: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Place {
    facade: String,
    /// Component type id, see types.list.
    #[serde(rename = "type")]
    type_id: String,
    /// Storey indices (0 = ground floor); omit for all.
    levels: Option<Vec<usize>>,
    /// Bay indices; omit for all.
    bays: Option<Vec<usize>>,
    width: Option<f64>,
    height: Option<f64>,
    /// mm above the storey's floor; may be negative down to the terrain.
    sill: Option<f64>,
    /// Allow the opening to run past the floor above (stairwell and landing windows).
    spans_storeys: Option<bool>,
    /// Phase the new element is built in (default: the active phase). Whatever stood in the slot
    /// before that phase is demolished in it, not deleted.
    phase: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Clear {
    facade: String,
    levels: Option<Vec<usize>>,
    bays: Option<Vec<usize>>,
}

fn selected(list: &Option<Vec<usize>>, i: usize) -> bool {
    list.as_ref().is_none_or(|l| l.contains(&i))
}

/// Paths must carry the expected extension, so a command can't overwrite an unrelated file.
fn checked_path(p: &str, ext: &str) -> Result<PathBuf> {
    let path = PathBuf::from(p);
    if path.extension().and_then(|e| e.to_str()) != Some(ext) {
        return Err(EngineError::Io(format!("{p}: the file name must end in .{ext}")));
    }
    Ok(path)
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::Io(format!("{}: {e}", dir.display())))?;
    }
    std::fs::write(path, bytes).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))
}

pub fn read_document(path: &Path) -> Result<Document> {
    let text = std::fs::read_to_string(path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    let doc: Document = serde_json::from_str(&text).map_err(|e| EngineError::Io(format!("{}: not a Trigon document ({e})", path.display())))?;
    if doc.format != trigon_model::FORMAT {
        return Err(EngineError::Io(format!("{}: format {} is not supported (expected {})", path.display(), doc.format, trigon_model::FORMAT)));
    }
    // A document that doesn't build is reported now, not at export.
    trigon_model::build(&doc)?;
    Ok(doc)
}

fn check_building(doc: &Document, id: Option<&str>) -> Result<()> {
    let b = doc.building(id)?;
    for f in &b.facades {
        resolve_facade(doc, b, f)?;
    }
    Ok(())
}

fn compass(bearing: f64) -> &'static str {
    const NAMES: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    NAMES.get(((bearing / 45.0).round() as usize) % 8).copied().unwrap_or("?")
}

fn facades(doc: &Document, b: &Building) -> Vec<Value> {
    b.facades
        .iter()
        .filter_map(|f| {
            let (a, e) = b.edge(f.edge)?;
            let fr = FacadeFrame::new(a, e)?;
            Some(json!({
                "id": f.id,
                "length_mm": fr.length.round(),
                "faces": compass(fr.bearing()),
                "bearing_deg": fr.bearing().round(),
                "kind": f.kind,
                "bays": f.bays.iter().map(|c| c.round()).collect::<Vec<_>>(),
                "placements": f.placements.len(),
                "openings_ok": resolve_facade(doc, b, f).is_ok(),
            }))
        })
        .collect()
}

fn info(doc: &Document) -> Value {
    json!({
        "format": doc.format,
        "site": doc.site.as_ref().map(|s| json!({
            "name": s.name,
            "origin": s.origin,
            "origin_utm": s.origin_utm,
            "terrain_m_dvr90": s.elevation,
            "evidence": s.evidence,
        })),
        "buildings": doc.buildings.iter().map(|b| json!({
            "id": b.id,
            "name": b.name,
            "storeys": b.storeys.len(),
            "storey_height_mm": b.storeys.first().map(|s| s.height),
            "levels_mm": b.storeys.iter().map(|s| json!({"name": s.name, "floor": s.elevation, "height": s.height})).collect::<Vec<_>>(),
            "ground_floor_mm": b.ground_floor,
            "height_mm": b.height(),
            "wall_thickness_mm": b.wall_thickness,
            "attributes": b.attributes,
            "facades": facades(doc, b),
            "evidence": b.evidence,
        })).collect::<Vec<_>>(),
        "types": doc.types.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        "phases": doc.phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        "active_phase": doc.phases.get(doc.active_index()).map(|p| p.id.as_str()),
        "journal_entries": doc.journal.len(),
    })
}

fn load_address(s: &mut Session, p: LoadAddress) -> Result<Value> {
    if s.doc.site.is_some() || !s.doc.buildings.is_empty() {
        return Err(EngineError::Model(ModelError::Invalid("the document already has a site; start with document.new".into())));
    }
    let storey_height = trigon_model::checked("storey height", p.storey_height.unwrap_or(3000.0), 2000.0, 10000.0)?;
    let provider = s.provider();
    let addr = provider.geocode(&p.address)?;
    let fp = provider.footprint(&addr)?;
    let mut notes = Vec::new();
    let heights = match provider.heights(&fp.ring_utm) {
        Ok(h) => Some(h),
        Err(e) => {
            notes.push(format!("no heights from the elevation model: {e}"));
            None
        }
    };
    let register = match fp.bbr_id.as_deref().map(|id| provider.register(id)) {
        Some(Ok(r)) => r,
        Some(Err(e)) => {
            notes.push(format!("no BBR data: {e}"));
            None
        }
        None => None,
    };
    // Storeys: given, else BBR, else the median surface height over a default storey height.
    let bbr_storeys = register.as_ref().and_then(|r| r.storeys).filter(|n| (1..=60).contains(n)).map(|n| n as usize);
    let estimated = heights.as_ref().map(|h| ((h.median * 1000.0) / storey_height).round().max(1.0) as usize);
    let storeys = p.storeys.or(bbr_storeys).or(estimated).unwrap_or(4);
    // Storey height: given, else the eaves height (elevation model) shared by the storeys.
    let storey_height = match (p.storey_height, &heights) {
        (None, Some(h)) if h.eaves > 2.0 => ((h.eaves * 1000.0) / storeys as f64).clamp(2400.0, 4500.0).round(),
        _ => storey_height,
    };
    let local: Vec<[f64; 2]> = fp.ring_utm.iter().map(|q| [(q[0] - addr.utm[0]) * 1000.0, (q[1] - addr.utm[1]) * 1000.0]).collect();
    let mut b = Building::new("b1", &addr.text, &local, storeys, storey_height)?;
    if let Some(t) = p.wall_thickness {
        b.wall_thickness = trigon_model::checked("wall thickness", t, trigon_model::doc::MIN_WALL, trigon_model::doc::MAX_WALL)?;
    }
    b.evidence.push(fp.evidence.clone());
    if let Some(r) = &register {
        b.evidence.push(r.evidence.clone());
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                b.attributes.insert(k.to_string(), v);
            }
        };
        put("bbr_id", Some(r.bbr_id.clone()));
        put("year_built", r.year_built.map(|y| y.to_string()));
        put("use", r.use_text.clone());
        put("wall_material", r.wall_material.clone());
        put("roof_material", r.roof_material.clone());
        put("built_area_m2", r.built_area_m2.map(|a| a.to_string()));
        put("floor_area_m2", r.floor_area_m2.map(|a| a.to_string()));
    }
    if let Some(h) = &heights {
        b.evidence.push(h.evidence.clone());
    }
    s.doc.site = Some(Site {
        name: addr.text.clone(),
        origin: addr.wgs,
        origin_utm: addr.utm,
        elevation: heights.as_ref().map_or(0.0, |h| h.terrain),
        evidence: vec![addr.evidence.clone()],
    });
    s.doc.buildings.push(b);
    let b = s.doc.building(None)?;
    Ok(json!({
        "address": addr.text,
        "dar_id": addr.id,
        "footprint_source": fp.evidence.source,
        "footprint_tags": fp.tags,
        "heights_m": heights.as_ref().map(|h| json!({"terrain": h.terrain, "eaves_above_terrain": h.eaves, "median_above_terrain": h.median, "top_above_terrain": h.top, "samples": h.samples})),
        "register": register,
        "storeys": storeys,
        "storey_height_mm": storey_height,
        "storeys_source": if p.storeys.is_some() { "given" } else if bbr_storeys.is_some() { "BBR" } else if estimated.is_some() { "estimated from median height" } else { "default" },
        "facades": facades(&s.doc, b),
        "notes": notes,
    }))
}

fn set_bays(s: &mut Session, p: SetBays) -> Result<Value> {
    let b = s.doc.building_of_facade_mut(&p.facade)?;
    let f = b.facade(&p.facade)?;
    let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::NotFound(format!("facade {}", p.facade)))?;
    let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {} has zero length", p.facade)))?;
    let centres = match (p.count, p.centres) {
        (Some(_), Some(_)) => {
            return Err(EngineError::Params { command: "facade.set_bays".into(), message: "give count or centres, not both".into() });
        }
        (Some(n), None) => {
            if n == 0 || n > 200 {
                return Err(ModelError::Invalid(format!("bay count must be 1 to 200 (got {n})")).into());
            }
            let margin = trigon_model::checked("margin", p.margin.unwrap_or(600.0), 0.0, fr.length / 2.0)?;
            even_bays(fr.length, n, margin)
        }
        (None, Some(c)) => {
            if c.len() > 200 {
                return Err(ModelError::Invalid("at most 200 bays".into()).into());
            }
            for x in &c {
                trigon_model::checked("bay centre", *x, 0.0, fr.length)?;
            }
            c
        }
        (None, None) => return Err(EngineError::Params { command: "facade.set_bays".into(), message: "give count or centres".into() }),
    };
    let n = centres.len();
    let id = b.id.clone();
    let f = b.facade_mut(&p.facade)?;
    f.bays = centres.clone();
    f.placements.retain(|pl| pl.bay < n);
    check_building(&s.doc, Some(&id))?;
    Ok(json!({"facade": p.facade, "bays": centres.iter().map(|c| c.round()).collect::<Vec<_>>()}))
}

/// The phase view a command asks for: `phase` (default: the active phase) and `filter`.
fn set_roof(s: &mut Session, p: SetRoof) -> Result<Value> {
    let b = s.doc.building_mut(p.building.as_deref())?;
    let roof = match p.kind.as_str() {
        "flat" => trigon_model::Roof::Flat,
        "pitched" => {
            let gables = p.gables.unwrap_or_default();
            for g in &gables {
                b.facade(g)?;
            }
            let pitch = match (p.pitch_deg, p.ridge) {
                (Some(d), None) => d,
                (None, Some(ridge)) => {
                    let eaves = b.height();
                    let kept = trigon_model::roof::eave_outline(&b.footprint, p.straighten_mm.unwrap_or(1200.0));
                    let outline: Vec<[f64; 2]> = kept.iter().filter_map(|&i| b.footprint.get(i).copied()).collect();
                    let depth = trigon_model::roof::max_depth(&outline)?;
                    let rise = trigon_model::checked("ridge", ridge, eaves + 500.0, eaves + 30_000.0)? - eaves;
                    (rise / depth).atan().to_degrees()
                }
                _ => {
                    return Err(EngineError::Params { command: "building.set_roof".into(), message: "give pitch_deg or ridge (one of them)".into() });
                }
            };
            trigon_model::Roof::Pitched { pitch_deg: pitch, gables, straighten_mm: p.straighten_mm.unwrap_or(1200.0) }
        }
        other => return Err(ModelError::Invalid(format!("roof kind {other}: use flat or pitched")).into()),
    };
    b.roof = roof;
    if let Some(src) = p.source.filter(|x| !x.trim().is_empty()) {
        b.evidence.push(trigon_model::Evidence { source: src, supplies: "roof".into(), attribution: p.attribution.unwrap_or_default() });
    }
    let id = b.id.clone();
    let built = trigon_model::build(&s.doc)?;
    let r = built.iter().find(|x| x.building_id == id).and_then(|x| x.roof.as_ref());
    Ok(json!({
        "roof": s.doc.building(Some(&id))?.roof,
        "eaves_mm": r.map(|r| r.eaves.round()),
        "ridge_mm": r.map(|r| r.ridge.round()),
        "pitch_deg": r.map(|r| (r.pitch_deg * 10.0).round() / 10.0),
    }))
}

fn register_plan(s: &mut Session, p: RegisterPlan) -> Result<Value> {
    let storeys = s.doc.building(None)?.storeys.len();
    if p.levels.is_empty() || p.levels.iter().any(|l| *l >= storeys) {
        return Err(ModelError::Invalid(format!("levels must be storey indices below {storeys}")).into());
    }
    if p.points.len() > 50 {
        return Err(ModelError::Invalid("at most 50 points".into()).into());
    }
    let mut pairs = Vec::with_capacity(p.points.len());
    for (i, q) in p.points.iter().enumerate() {
        let site = match (q.x, q.y, q.facade.as_deref(), q.u) {
            (Some(x), Some(y), None, None) => [x, y],
            (None, None, Some(fid), Some(u)) => {
                let b = s
                    .doc
                    .buildings
                    .iter()
                    .find(|b| b.facades.iter().any(|f| f.id == fid))
                    .ok_or_else(|| ModelError::NotFound(format!("facade {fid}")))?;
                let f = b.facade(fid)?;
                let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::NotFound(format!("facade {fid}")))?;
                let fr = FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {fid} has zero length")))?;
                let u = trigon_model::checked("u", u, -5000.0, fr.length + 5000.0)?;
                let pt = fr.point(u, 0.0, 0.0);
                [pt[0], pt[1]]
            }
            _ => {
                return Err(EngineError::Params {
                    command: "drawing.register_plan".into(),
                    message: format!("point {i}: give x and y, or facade and u"),
                });
            }
        };
        pairs.push(([q.px, q.py], site));
    }
    let reg = trigon_model::PlanRegistration::fit(p.levels.clone(), &pairs)?;
    let residuals: Vec<f64> = pairs
        .iter()
        .map(|(px, q)| {
            let r = reg.to_site(*px);
            ((r[0] - q[0]).powi(2) + (r[1] - q[1]).powi(2)).sqrt().round()
        })
        .collect();
    let d = s.doc.drawings.iter_mut().find(|d| d.name == p.name).ok_or_else(|| ModelError::NotFound(format!("drawing {}", p.name)))?;
    d.plans.retain(|r| !r.levels.iter().any(|l| p.levels.contains(l)));
    d.plans.push(reg.clone());
    Ok(json!({
        "drawing": p.name,
        "levels": p.levels,
        "px_per_m": (reg.px_per_m() * 100.0).round() / 100.0,
        "rotation_deg": (reg.b.atan2(reg.a).to_degrees() * 100.0).round() / 100.0,
        "residuals_mm": residuals,
    }))
}

fn phase_list(doc: &Document) -> Value {
    let active = doc.active_index();
    let mut built = vec![0usize; doc.phases.len()];
    let mut torn = vec![0usize; doc.phases.len()];
    for b in &doc.buildings {
        for f in &b.facades {
            for pl in &f.placements {
                if let Ok((c, d)) = doc.lifetime(pl) {
                    if let Some(x) = built.get_mut(c) {
                        *x += 1;
                    }
                    if let Some(x) = torn.get_mut(d) {
                        *x += 1;
                    }
                }
            }
        }
    }
    json!({
        "active": doc.phases.get(active).map(|p| p.id.clone()),
        "phases": doc.phases.iter().enumerate().map(|(i, p)| json!({"id": p.id, "name": p.name, "builds": built[i], "demolishes": torn[i]})).collect::<Vec<_>>(),
    })
}

pub(crate) fn view_of(doc: &Document, phase: Option<&str>, filter: Option<&str>) -> Result<View> {
    let phase = match phase {
        Some(id) => doc.phase_index(id)?,
        None => doc.active_index(),
    };
    let filter = match filter.unwrap_or("complete") {
        "complete" => PhaseFilter::Complete,
        "demolition" => PhaseFilter::Demolition,
        "new" => PhaseFilter::New,
        "all" => PhaseFilter::All,
        other => return Err(ModelError::Invalid(format!("filter {other}: use complete, demolition, new or all")).into()),
    };
    Ok(View { phase, filter })
}

/// For each type id, whether it cuts an opening (else it's a balcony hung on the wall).
fn opening_types(doc: &Document) -> std::collections::BTreeMap<String, bool> {
    doc.types.iter().map(|t| (t.id.clone(), t.kind.is_opening())).collect()
}

/// Ends a placement's life in phase `k` (Revit's "demolish"): removed outright if it was built in
/// `k`, otherwise marked demolished in `k`. Returns true if it was standing in `k`.
fn end_in(doc_phases: &[trigon_model::Phase], pl: &mut Placement, k: usize) -> Option<bool> {
    let idx = |id: &Option<String>, default: usize| id.as_deref().and_then(|i| doc_phases.iter().position(|p| p.id == i)).unwrap_or(default);
    let (c, d) = (idx(&pl.created, 0), idx(&pl.demolished, doc_phases.len()));
    if !(c <= k && k < d) {
        return None;
    }
    if c == k {
        Some(true) // remove
    } else {
        pl.demolished = doc_phases.get(k).map(|p| p.id.clone());
        Some(false)
    }
}

fn place(s: &mut Session, p: Place) -> Result<Value> {
    let new_is_opening = s.doc.component(&p.type_id)?.kind.is_opening();
    let k = match p.phase.as_deref() {
        Some(id) => s.doc.phase_index(id)?,
        None => s.doc.active_index(),
    };
    let phases = s.doc.phases.clone();
    let created = if k == 0 { None } else { phases.get(k).map(|ph| ph.id.clone()) };
    let layers = opening_types(&s.doc);
    let b = s.doc.building_of_facade_mut(&p.facade)?;
    let levels = b.storeys.len();
    let id = b.id.clone();
    let f = b.facade_mut(&p.facade)?;
    if f.bays.is_empty() {
        return Err(ModelError::Invalid(format!("facade {} has no bays yet; run facade.set_bays first", p.facade)).into());
    }
    let level_list: Vec<usize> = p.levels.clone().unwrap_or_else(|| (0..levels).collect());
    let bay_list: Vec<usize> = p.bays.clone().unwrap_or_else(|| (0..f.bays.len()).collect());
    for &l in &level_list {
        if l >= levels {
            return Err(ModelError::Invalid(format!("level {l} doesn't exist (the building has {levels} storeys)")).into());
        }
    }
    for &bay in &bay_list {
        if bay >= f.bays.len() {
            return Err(ModelError::Invalid(format!("bay {bay} doesn't exist (facade {} has {} bays)", p.facade, f.bays.len())).into());
        }
    }
    let (mut count, mut demolished) = (0, 0);
    for &level in &level_list {
        for &bay in &bay_list {
            // A slot holds one opening and one balcony. Whatever of the same kind stands there in
            // this phase is replaced: deleted if built in this phase, demolished if older.
            let mut keep = Vec::with_capacity(f.placements.len());
            for mut pl in std::mem::take(&mut f.placements) {
                let same_slot = pl.level == level && pl.bay == bay && layers.get(&pl.type_id).copied().unwrap_or(true) == new_is_opening;
                match same_slot.then(|| end_in(&phases, &mut pl, k)).flatten() {
                    Some(true) => continue,
                    Some(false) => demolished += 1,
                    None => {}
                }
                keep.push(pl);
            }
            f.placements = keep;
            f.placements.push(Placement {
                level,
                bay,
                type_id: p.type_id.clone(),
                width: p.width,
                height: p.height,
                sill: p.sill,
                spans_storeys: p.spans_storeys.unwrap_or(false),
                created: created.clone(),
                demolished: None,
            });
            count += 1;
        }
    }
    f.placements.sort_by_key(|pl| (pl.level, pl.bay));
    check_building(&s.doc, Some(&id))?;
    Ok(json!({"facade": p.facade, "placed": count, "type": p.type_id, "phase": phases.get(k).map(|ph| ph.id.clone()), "demolished": demolished}))
}

fn demolish(s: &mut Session, p: Demolish) -> Result<Value> {
    let k = match p.phase.as_deref() {
        Some(id) => s.doc.phase_index(id)?,
        None => s.doc.active_index(),
    };
    let what = p.what.as_deref().unwrap_or("all");
    if !matches!(what, "all" | "openings" | "balconies") {
        return Err(ModelError::Invalid(format!("what {what}: use all, openings or balconies")).into());
    }
    let phases = s.doc.phases.clone();
    let layers = opening_types(&s.doc);
    let b = s.doc.building_of_facade_mut(&p.facade)?;
    let id = b.id.clone();
    let f = b.facade_mut(&p.facade)?;
    let (mut removed, mut demolished) = (0, 0);
    let mut keep = Vec::with_capacity(f.placements.len());
    for mut pl in std::mem::take(&mut f.placements) {
        let opening = layers.get(&pl.type_id).copied().unwrap_or(true);
        let picked = selected(&p.levels, pl.level) && selected(&p.bays, pl.bay) && (what == "all" || (what == "openings") == opening);
        match picked.then(|| end_in(&phases, &mut pl, k)).flatten() {
            Some(true) => {
                removed += 1;
                continue;
            }
            Some(false) => demolished += 1,
            None => {}
        }
        keep.push(pl);
    }
    f.placements = keep;
    check_building(&s.doc, Some(&id))?;
    Ok(json!({"facade": p.facade, "phase": phases.get(k).map(|ph| ph.id.clone()), "demolished": demolished, "removed_new_in_this_phase": removed}))
}

fn photos(s: &mut Session, p: BuildingParam) -> Result<Value> {
    let site = s.doc.site.as_ref().ok_or_else(|| ModelError::NotFound("no site yet; run site.load_address".into()))?;
    let list = s.provider().photos(site.origin_utm)?;
    let b = s.doc.building(p.building.as_deref())?;
    // A facade facing bearing β is seen by a camera looking towards β + 180°.
    let looks = |dir: &str| match dir {
        "north" => Some(0.0),
        "east" => Some(90.0),
        "south" => Some(180.0),
        "west" => Some(270.0),
        _ => None,
    };
    let per_facade: Vec<Value> = b
        .facades
        .iter()
        .filter_map(|f| {
            let (a, e) = b.edge(f.edge)?;
            let bearing = FacadeFrame::new(a, e)?.bearing();
            let want = (bearing + 180.0) % 360.0;
            let best: Vec<&str> = list
                .iter()
                .filter(|ph| looks(&ph.direction).is_some_and(|d: f64| ((d - want + 540.0) % 360.0 - 180.0).abs() <= 45.0))
                .map(|ph| ph.id.as_str())
                .collect();
            Some(json!({"facade": f.id, "faces": compass(bearing), "photos": best}))
        })
        .collect();
    Ok(json!({"photos": list, "by_facade": per_facade}))
}

fn read_ifc_text(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    if meta.len() as usize > trigon_ifcread::MAX_FILE_BYTES {
        return Err(EngineError::Io(format!("{}: larger than {} MB", path.display(), trigon_ifcread::MAX_FILE_BYTES >> 20)));
    }
    std::fs::read_to_string(path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))
}

fn ifc_err(e: trigon_ifcread::ReadError) -> EngineError {
    EngineError::Io(e.to_string())
}

fn read_summary(r: &trigon_ifcread::ReadResult) -> Value {
    json!({
        "reader": r.reader.name(),
        "schema": r.schema,
        "unit_m": r.unit_m,
        "openings_cut": r.voids_applied,
        "elements": r.elements.len(),
        "triangles": r.triangles(),
        "by_class": r.counts(),
        "failures": r.failures.len(),
        "first_failures": r.failures.iter().take(5).map(|f| json!({"id": f.id, "class": f.class, "message": f.message})).collect::<Vec<_>>(),
        "georef": r.georef.as_ref().map(|g| json!({"crs": g.crs_name, "eastings": g.eastings, "northings": g.northings, "height": g.orthogonal_height})),
    })
}

fn comparison(c: &trigon_ifcread::Comparison) -> Value {
    json!({
        "matched": c.matched,
        "only_in_first": c.only_first.len(),
        "only_in_second": c.only_second.len(),
        "same_triangle_count": c.same_triangles,
        "max_bounds_deviation_mm": (c.max_bounds_mm * 1000.0).round() / 1000.0,
        "worst": c.worst.iter().map(|d| json!({"global_id": d.global_id, "class": d.class, "mm": (d.mm * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
    })
}

/// Bounding boxes of this document's elements, keyed by the GlobalId the IFC export gives them.
fn model_bounds(doc: &Document) -> Result<std::collections::BTreeMap<String, ([f64; 3], [f64; 3])>> {
    let mut out = std::collections::BTreeMap::new();
    let mut add = |id: &str, meshes: Vec<&trigon_model::Mesh>| {
        let mut all = trigon_model::Mesh::default();
        for m in meshes {
            all.append(m);
        }
        if let Some(b) = all.bounds() {
            out.insert(trigon_io::ifc::guid(&format!("trigon:{id}")), b);
        }
    };
    for b in trigon_model::build(doc)? {
        for w in &b.walls {
            add(&w.id, vec![&w.mesh]);
            for e in w.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(&o.attached)).chain(&w.attachments) {
                add(&e.id, e.parts.iter().map(|(_, m)| m).collect());
            }
        }
        for s in &b.slabs {
            add(&s.id, vec![&s.mesh]);
        }
        if let Some(r) = &b.roof {
            add(&r.id, vec![&r.cover]);
            add(&format!("{}.gables", r.id), vec![&r.gables]);
        }
    }
    Ok(out)
}

fn versus_model(doc: &Document, r: &trigon_ifcread::ReadResult) -> Result<Value> {
    let ours = model_bounds(doc)?;
    let mut devs: Vec<(f64, &str, &str)> = r
        .elements
        .iter()
        .filter_map(|e| Some((trigon_ifcread::bounds_deviation(*ours.get(&e.global_id)?, e.mesh.bounds()?), e.global_id.as_str(), e.class.as_str())))
        .collect();
    devs.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(json!({
        "reader": r.reader.name(),
        "matched": devs.len(),
        "model_elements": ours.len(),
        "max_bounds_deviation_mm": devs.first().map_or(0.0, |d| (d.0 * 1000.0).round() / 1000.0),
        "worst": devs.iter().take(3).map(|d| json!({"global_id": d.1, "class": d.2, "mm": (d.0 * 1000.0).round() / 1000.0})).collect::<Vec<_>>(),
    }))
}

fn ifc_check(doc: &Document, path: &Path) -> Result<Value> {
    let text = read_ifc_text(path)?;
    let lite = trigon_ifcread::read_ifc_lite(&text).map_err(ifc_err)?;
    let bim = trigon_ifcread::read_bimifc(&text).map_err(ifc_err)?;
    let mut out = json!({
        "file": path,
        "readers": [read_summary(&lite), read_summary(&bim)],
        "ifc_lite_vs_bimifc": comparison(&trigon_ifcread::compare(&lite, &bim)),
        "note": "bimifc 0.1.0 does not cut openings, so wall triangle counts differ by design; extents should still agree.",
    });
    if !doc.buildings.is_empty() {
        out["versus_this_model"] = json!([versus_model(doc, &lite)?, versus_model(doc, &bim)?]);
    }
    Ok(out)
}

/// How a linked file maps into this document's site frame.
enum Alignment {
    Local,
    Map { origin: [f64; 2], elevation: f64 },
}

fn alignment_for(doc: &Document, r: &trigon_ifcread::ReadResult) -> (Alignment, String) {
    let crs_ok = r.georef.as_ref().and_then(|g| g.crs_name.as_deref()).is_some_and(|c| c.contains("25832"));
    match (&doc.site, crs_ok) {
        (Some(site), true) => (Alignment::Map { origin: site.origin_utm, elevation: site.elevation }, "map".into()),
        (None, true) => (Alignment::Local, "local (no site yet to place it against)".into()),
        (_, false) => (Alignment::Local, "local (no EPSG:25832 georeference in the file)".into()),
    }
}

fn to_site(r: &trigon_ifcread::ReadResult, how: &Alignment, p: [f64; 3]) -> [f64; 3] {
    match how {
        Alignment::Map { origin, elevation } => match r.to_map(p) {
            Some([e, n, h]) => [(e - origin[0]) * 1000.0, (n - origin[1]) * 1000.0, (h - elevation) * 1000.0],
            None => p,
        },
        Alignment::Local => p,
    }
}

fn link_reference(s: &mut Session, p: LinkParams) -> Result<Value> {
    let path = checked_path(&p.path, "ifc")?;
    let r = trigon_ifcread::read_ifc_lite(&read_ifc_text(&path)?).map_err(ifc_err)?;
    let name = p.name.unwrap_or_else(|| path.file_stem().and_then(|n| n.to_str()).unwrap_or("reference").to_string());
    if s.doc.references.iter().any(|x| x.name == name) {
        return Err(ModelError::Invalid(format!("a reference named {name} is already linked")).into());
    }
    let (how, alignment) = alignment_for(&s.doc, &r);
    let mut extent = trigon_model::Mesh::default();
    for e in &r.elements {
        extent.positions.extend(e.mesh.positions.iter().map(|q| to_site(&r, &how, *q)));
    }
    s.doc.references.push(trigon_model::Reference {
        name: name.clone(),
        path: path.to_string_lossy().into_owned(),
        alignment: if matches!(how, Alignment::Map { .. }) { "map".into() } else { "local".into() },
    });
    Ok(json!({
        "linked": name,
        "alignment": alignment,
        "elements": r.elements.len(),
        "failures": r.failures.len(),
        "extent_mm": extent.bounds(),
    }))
}

/// The linked models as one mesh per reference and IFC class, in this document's frame.
fn reference_meshes(doc: &Document) -> Result<Vec<(String, trigon_model::Mesh)>> {
    let mut out = Vec::new();
    for rf in &doc.references {
        let r = trigon_ifcread::read_ifc_lite(&read_ifc_text(Path::new(&rf.path))?).map_err(ifc_err)?;
        let how = if rf.alignment == "map" { alignment_for(doc, &r).0 } else { Alignment::Local };
        let mut by_class: std::collections::BTreeMap<String, trigon_model::Mesh> = Default::default();
        for e in &r.elements {
            let m = by_class.entry(e.class.clone()).or_default();
            let base = m.positions.len() as u32;
            m.positions.extend(e.mesh.positions.iter().map(|q| to_site(&r, &how, *q)));
            m.triangles.extend(e.mesh.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        }
        out.extend(by_class.into_iter().map(|(class, m)| (format!("{}/{class}", rf.name), m)));
    }
    Ok(out)
}
