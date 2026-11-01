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
        id: "site.add",
        summary: "Add a typed ground area, tree, bench or planter in site-local mm. Explicit design intent; validated, journaled and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::site::Add>,
        run: |s, v| crate::site::add(s, parse("site.add", v)?),
    },
    CommandSpec {
        id: "site.set",
        summary: "Replace one site's typed source while preserving its ID. Validated, atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::site::Set>,
        run: |s, v| crate::site::set(s, parse("site.set", v)?),
    },
    CommandSpec {
        id: "site.delete",
        summary: "Delete a site source, in every phase. Undo restores its exact parameters and lifetime.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::site::Id>,
        run: |s, v| crate::site::delete(s, parse("site.delete", v)?),
    },
    CommandSpec {
        id: "site.list",
        summary: "List exact site sources and design intent; these are not building opening or room quantities.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("site.list", v)?;
            Ok(json!({"objects":s.doc.site_objects}))
        },
    },
    CommandSpec {
        id: "export.fbx",
        summary: "Export binary FBX through an optional locally installed Blender, for Lumion and rendering tools. Pure Rust model geometry passes through GLB; an isolated public-API converter embeds textures and verifies source IDs, triangle counts, UVs and vertices within 0.1 mm on re-import before atomic file replacement. No account or GUI required. TRIGON_BLENDER selects an operator-configured executable.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::fbx::Export>,
        run: |s, v| crate::fbx::export(s, parse("export.fbx", v)?),
    },
    CommandSpec {
        id: "site.set_terrain",
        summary: "Set/remove a parametric ground height grid in site-local mm. Buildings trim the surface, keeping ground out of floors. Bounded, validated, atomic and undoable; unmeasured flat ground is a presentation assumption.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::terrain::Set>,
        run: |s, v| crate::terrain::set(s, parse("site.set_terrain", v)?),
    },
    CommandSpec {
        id: "site.sample_terrain",
        summary: "Sample a bounded ground grid from Danmarks Højdemodel DTM using configured Dataforsyningen access. Converts DVR90 metres to site-local mm, records source attribution, rejects missing data. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::terrain::Sample>,
        run: |s, v| crate::terrain::sample(s, parse("site.sample_terrain", v)?),
    },
    CommandSpec {
        id: "element.get",
        summary: "Inspect a semantic model element and its editable source properties and dimensional handles in the requested phase view. Read-only; exact built IDs.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::elements::Get>,
        run: |s, v| crate::elements::get(s, parse("element.get", v)?),
    },
    CommandSpec {
        id: "element.set",
        summary: "Update the selected element's advertised source properties. Shared parametric model for plans and 3D; validated, atomic and undoable. Ambiguous and historical edits are refused.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::elements::Set>,
        run: |s, v| crate::elements::set(s, parse("element.set", v)?),
    },
    CommandSpec {
        id: "schedule.export",
        summary: "Write an opening schedule to .csv or .json with the same building/storey/phase filters as schedule.openings. CSV uses UTF-8, model millimetres, m2 areas and an apostrophe prefix for formula-like text. Atomic file output; document and history are unchanged.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::schedule_export::Export>,
        run: |s, v| crate::schedule_export::export(s, parse("schedule.export", v)?),
    },
    CommandSpec {
        id: "schedule.openings",
        summary: "List actual built windows and doors with semantic IDs, building/host/storey/type, phase status and dimensions. Areas are outer-lining profile m2 (width × height for rectangular families); loggia facade-hole areas are excluded. Optional building, levels and phase/filter. Read-only and deterministic; does not change history.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::schedules::OpeningScheduleRequest>,
        run: |s, v| crate::schedules::openings(s, parse("schedule.openings", v)?),
    },
    CommandSpec {
        id: "view.define",
        summary: "Create or replace a named plan, facade or section drawing view. Coordinates are drawing-plane mm. A view with annotations may be renamed, but its coordinate frame cannot change. Undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<trigon_model::annotation::DrawingView>,
        run: |s, v| crate::views::define_view(s, parse("view.define", v)?),
    },
    CommandSpec {
        id: "view.configure",
        summary: "Set or clear a saved view's persistent template: line weights, physical scale or fit/crop, and optional plan-only floor-relative cut/top/bottom/depth. Null template restores legacy drawing defaults. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::view_config::Configure>,
        run: |s, v| crate::view_config::configure(s, parse("view.configure", v)?),
    },
    CommandSpec {
        id: "view.apply_template",
        summary: "Copy a configured saved view's template to 1 to 256 distinct target views, preserving their coordinate frames and annotations. All targets validate before any change; atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::view_config::Apply>,
        run: |s, v| crate::view_config::apply(s, parse("view.apply_template", v)?),
    },
    CommandSpec {
        id: "view.list",
        summary: "List saved drawing views with validity and annotation counts. Stale model references remain repairable.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| parse::<NoParams>("view.list", v).map(|_| crate::views::list_views(s)),
    },
    CommandSpec {
        id: "view.remove",
        summary: "Remove an empty saved drawing view. Remove its annotations first. Undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::views::Id>,
        run: |s, v| {
            let p: crate::views::Id = parse("view.remove", v)?;
            crate::views::remove_view(s, &p.id)
        },
    },
    CommandSpec {
        id: "view.snap",
        summary: "Find the closest endpoint, midpoint, intersection or grid point within a tolerance in drawing mm in a saved view. Read-only; returns the original point when no candidate is near.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::views::SnapRequest>,
        run: |s, v| {
            let p: crate::views::SnapRequest = parse("view.snap", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::views::snap(&s.doc, p, view)
        },
    },
    CommandSpec {
        id: "view.pick",
        summary: "Find the nearest visible model-element line within tolerance_mm in a saved drawing view. Drawing-plane mm; returns hit with the existing semantic id, kind, nearest point and distance_mm, or null. Exact ties prefer openings, interior walls, facade walls, then lexical id. Read-only.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::views::PickRequest>,
        run: |s, v| {
            let p: crate::views::PickRequest = parse("view.pick", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::views::pick(&s.doc, p, view)
        },
    },
    CommandSpec {
        id: "view.export",
        summary: "Export one saved drawing view and its annotations to SVG, PNG, vector PDF or DXF at 1:1 in drawing-plane mm. Phase/filter defaults match other sheet exports.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::views::Export>,
        run: |s, v| {
            let p: crate::views::Export = parse("view.export", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::views::sheet(&s.doc, &p.id, view)?.write(PathBuf::from(p.path))
        },
    },
    CommandSpec {
        id: "annotation.define",
        summary: "Create or replace text, an aligned measured dimension, a leader/tag or a vertical level mark in a saved view. Positions/offsets are drawing mm; size_mm is paper mm. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<trigon_model::annotation::Annotation>,
        run: |s, v| crate::views::define_annotation(s, parse("annotation.define", v)?),
    },
    CommandSpec {
        id: "annotation.list",
        summary: "List drafting annotations, optionally filtered to one saved drawing view.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::views::ListAnnotations>,
        run: |s, v| {
            let p: crate::views::ListAnnotations = parse("annotation.list", v)?;
            crate::views::list_annotations(s, p.view.as_deref())
        },
    },
    CommandSpec {
        id: "annotation.remove",
        summary: "Remove one drafting annotation. Undoable; model geometry is unchanged.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::views::Id>,
        run: |s, v| {
            let p: crate::views::Id = parse("annotation.remove", v)?;
            crate::views::remove_annotation(s, &p.id)
        },
    },
    CommandSpec {
        id: "group.define",
        summary: "Save a named group of up to 256 editable source definitions/instances. Groups may be targets of edit commands (group:<id>); references to existing groups are flattened. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<trigon_model::doc::ModelGroup>,
        run: |s, v| crate::edit::define_group(s, parse("group.define", v)?),
    },
    CommandSpec {
        id: "group.list",
        summary: "List saved selection groups and report stale or invalid member references.",
        mutates: false,
        journaled: false,
        schema: schema::<NoParams>,
        run: |s, v| parse::<NoParams>("group.list", v).map(|_| crate::edit::list_groups(s)),
    },
    CommandSpec {
        id: "group.remove",
        summary: "Remove a saved group without deleting its model elements. Undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::GroupId>,
        run: |s, v| crate::edit::remove_group(s, parse("group.remove", v)?),
    },
    CommandSpec {
        id: "document.from_template",
        summary: "Start an unsaved copy of a .trigon project template, with its families, groups and model. The source file stays unchanged and the copy has no save path.",
        mutates: true,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("document.from_template", v)?;
            let path = checked_path(&p.path, "trigon")?;
            let doc = read_document(&path)?;
            s.doc = doc;
            s.path = None;
            s.dirty = true;
            s.reset_history();
            Ok(info(&s.doc))
        },
    },
    CommandSpec {
        id: "document.save_template",
        summary: "Save a reusable .trigon project template without changing the current document's path, dirty state or undo history.",
        mutates: false,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("document.save_template", v)?;
            let path = checked_path(&p.path, "trigon")?;
            let text = serde_json::to_string_pretty(&s.doc).map_err(|e| EngineError::Io(e.to_string()))?;
            write_file(&path, text.as_bytes())?;
            Ok(json!({"saved":path,"template":true}))
        },
    },
    CommandSpec {
        id: "edit.move",
        summary: "Move selected walls, stairs, rooms or facade instances by a precise [dx,dy,dz] offset in mm. Studio ids select one storey; wall:<id>, stair:<id>, room:<id> select all its storeys. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::Move>,
        run: |s, v| crate::edit::move_elements(s, parse("edit.move", v)?),
    },
    CommandSpec {
        id: "edit.copy",
        summary: "Copy selected elements at a precise offset, optionally as an array (count 1–100). Every copy has a distinct id; originals stay in place. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::Copy>,
        run: |s, v| crate::edit::copy_elements(s, parse("edit.copy", v)?),
    },
    CommandSpec {
        id: "edit.copy_to_levels",
        summary: "Copy selected geometry onto destination storeys, keeping its position in plan or along the facade. Source storeys are skipped. replace copies over matching destination geometry/slots; otherwise a collision is an error. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::CopyToLevels>,
        run: |s, v| crate::edit::copy_to_levels(s, parse("edit.copy_to_levels", v)?),
    },
    CommandSpec {
        id: "edit.align",
        summary: "Align selected elements' start, centre or end to a coordinate, reference element or arbitrary plan line. X/Y are site axes, U is along a facade, Z is absolute opening height. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::Align>,
        run: |s, v| crate::edit::align(s, parse("edit.align", v)?),
    },
    CommandSpec {
        id: "edit.align_walls",
        summary: "Make nearly parallel interior walls exactly collinear, or align their left/right faces to a picked reference wall. Preserves length, door distances and unselected storeys. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::AlignWalls>,
        run: |s, v| crate::edit::align_walls(s, parse("edit.align_walls", v)?),
    },
    CommandSpec {
        id: "edit.delete",
        summary: "Delete selected source elements or just the picked storey instances, preserving their other storeys. This removes them from every phase; use facade.demolish for phased demolition. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::edit::Selection>,
        run: |s, v| crate::edit::delete(s, parse("edit.delete", v)?),
    },
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
        id: "building.add_basement",
        summary: "Add a basement storey below the lowest storey, its floor `floor` mm relative to the terrain (negative, e.g. \
                  read from a section). Every storey reference (placements, interior, registered plans) moves up by one; \
                  the outer walls then reach down to the basement floor. `source` is recorded as evidence.",
        mutates: true,
        journaled: true,
        schema: schema::<AddBasement>,
        run: |s, v| {
            let p: AddBasement = parse("building.add_basement", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.add_basement(p.floor, p.name.as_deref().unwrap_or("Kælder"))?;
            if let Some(src) = p.source.filter(|x| !x.trim().is_empty()) {
                b.evidence.push(trigon_model::Evidence { source: src, supplies: "levels".into(), attribution: p.attribution.unwrap_or_default() });
            }
            for d in &mut s.doc.drawings {
                for r in &mut d.plans {
                    r.levels.iter_mut().for_each(|l| *l += 1);
                }
            }
            Ok(info(&s.doc))
        },
    },
    CommandSpec {
        id: "building.set_roof",
        summary: "Set the roof: flat, or pitched from the eaves (the top of the walls) at `pitch_deg`, or at the pitch that puts \
                  the ridge at `ridge` (mm above the terrain, e.g. read from a section). `gables` lists the facades that end in a \
                  gable (wing ends, walls against neighbours). A pitched roof can carry on past the walls (`overhang_mm` at the \
                  eaves, `verge_mm` at the gables), have a build-up (`thickness_mm`, with fascia, soffit and verge boards) and \
                  `gutters`. `source` records where the values came from.",
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
        id: "building.set_wall_schedule",
        summary: "Set the facade wall thickness per storey (mm, one value per storey from the bottom up), or `preset: \
                  copenhagen_masonry`: 1½ sten (360) on the top two storeys, 2 sten (480) on the next two, 2½ sten (600) \
                  below those and each basement half a brick more (700 under a 600 ground storey). The wall steps on the \
                  inside at every floor and the floors rest on the steps; the outer face stays flush. With `facades`, \
                  `thickness` applies to those walls only (fire gables and party walls usually keep one thickness all \
                  the way up); an empty `thickness` makes them follow the building's schedule again. Check the \
                  thickness against the archive plans (`drawing.view`) before settling on it.",
        mutates: true,
        journaled: true,
        schema: schema::<SetWallSchedule>,
        run: |s, v| {
            let p: SetWallSchedule = parse("building.set_wall_schedule", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            if let Some(facades) = &p.facades {
                if p.preset.is_some() || facades.is_empty() {
                    return Err(ModelError::Invalid("with `facades`, give `thickness` (one per storey, or empty), not `preset`".into()).into());
                }
                b.set_facade_schedule(facades, p.thickness.as_deref().unwrap_or_default())?;
                check_building(&s.doc, p.building.as_deref())?;
                trigon_model::build(&s.doc)?;
                let b = s.doc.building(p.building.as_deref())?;
                return Ok(json!({
                    "facades": facades.iter().filter_map(|id| b.facade(id).ok()).map(|f| json!({
                        "facade": f.id,
                        "thickness_mm": (0..b.storeys.len()).map(|l| b.edge_thickness(f.edge, l)).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>()
                }));
            }
            let values = match (p.thickness, p.preset) {
                (Some(t), None) => t,
                (None, Some(WallPreset::CopenhagenMasonry)) => trigon_model::doc::copenhagen_masonry(b.storeys.len(), b.ground_level),
                _ => return Err(ModelError::Invalid("give either `thickness` (one value per storey) or `preset`".into()).into()),
            };
            b.set_wall_schedule(&values)?;
            check_building(&s.doc, p.building.as_deref())?;
            trigon_model::build(&s.doc)?;
            let b = s.doc.building(p.building.as_deref())?;
            Ok(json!({
                "schedule": b.storeys.iter().enumerate().map(|(i, st)| json!({"storey": st.name, "thickness_mm": b.facade_thickness(i)})).collect::<Vec<_>>()
            }))
        },
    },
    CommandSpec {
        id: "building.set_basement_line",
        summary: "Below the ground storey, run the walls straight along the inner corners of facade steps up to \
                  `straighten_mm` deep (bays and stepped fronts that begin at the ground floor stand on a straight basement \
                  wall in most Copenhagen blocks; check the basement plan). The steps above run down into the ground on \
                  their own foundation. `straighten_mm: 0` makes the basement follow the footprint again.",
        mutates: true,
        journaled: true,
        schema: schema::<SetBasementLine>,
        run: |s, v| {
            let p: SetBasementLine = parse("building.set_basement_line", v)?;
            let d = trigon_model::checked("straighten_mm", p.straighten_mm, 0.0, 5000.0)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.basement_straighten_mm = (d > 0.0).then_some(d);
            check_building(&s.doc, p.building.as_deref())?;
            trigon_model::build(&s.doc)?;
            let b = s.doc.building(p.building.as_deref())?;
            let cut: Vec<String> =
                b.facades.iter().filter(|f| (0..b.ground_level).any(|l| !b.edge_on_storey(f.edge, l))).map(|f| f.id.clone()).collect();
            Ok(json!({"straighten_mm": b.basement_straighten_mm, "facades_without_basement_wall": cut}))
        },
    },
    CommandSpec {
        id: "building.set_cellar_stairs",
        summary: "Set the outdoor cellar stairs (kældernedgange), replacing any there were: each is a pit along a facade \
                  (`from`–`to` mm along it, `width` mm out from it) between concrete retaining walls with a railing, a \
                  straight flight along the wall from `top` (at the terrain) down to `bottom` (the pit floor, `floor` mm \
                  relative to the terrain, e.g. the basement floor), and a door `door_width` wide (default 900) at `door_at` \
                  into the basement. Read them off the basement plan. `stairs: []` removes them.",
        mutates: true,
        journaled: true,
        schema: schema::<SetCellarStairs>,
        run: |s, v| {
            let p: SetCellarStairs = parse("building.set_cellar_stairs", v)?;
            if p.stairs.len() > 100 {
                return Err(ModelError::Invalid("at most 100 cellar stairs".into()).into());
            }
            let b = s.doc.building_mut(p.building.as_deref())?;
            for c in &p.stairs {
                let f = b.facade(&c.facade)?;
                let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::NotFound(format!("facade {}", c.facade)))?;
                c.validate(((e[0] - a[0]).powi(2) + (e[1] - a[1]).powi(2)).sqrt())?;
            }
            b.cellar_stairs = p.stairs;
            trigon_model::build(&s.doc)?;
            Ok(json!({"cellar_stairs": s.doc.building(p.building.as_deref())?.cellar_stairs.len()}))
        },
    },
    CommandSpec {
        id: "building.set_plinth",
        summary: "Give the facades a plinth (sokkel): rendered or concrete, from below the terrain up to `height_mm` above it, \
                  standing `projection_mm` proud of the brickwork (default 20). Openings in the plinth band (basement windows) \
                  are cut through it. `remove: true` takes it away.",
        mutates: true,
        journaled: true,
        schema: schema::<SetPlinth>,
        run: |s, v| {
            let p: SetPlinth = parse("building.set_plinth", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.plinth = match (p.remove.unwrap_or(false), p.height_mm) {
                (true, None) => None,
                (false, Some(h)) => Some(trigon_model::doc::Plinth {
                    height: trigon_model::checked("height_mm", h, 100.0, 5000.0)?,
                    projection: trigon_model::checked("projection_mm", p.projection_mm.unwrap_or(20.0), 0.0, 300.0)?,
                }),
                _ => return Err(ModelError::Invalid("give height_mm, or remove: true".into()).into()),
            };
            trigon_model::build(&s.doc)?;
            Ok(json!({"plinth": s.doc.building(p.building.as_deref())?.plinth}))
        },
    },
    CommandSpec {
        id: "facade.set_finish",
        summary: "Set the outer finish of facades: brick (the building's facing brick), red_brick, yellow_brick or render (white). Without \
                  `facades` it sets the building's default; `clear: true` makes the listed facades follow the building again.",
        mutates: true,
        journaled: true,
        schema: schema::<SetFinish>,
        run: |s, v| {
            let p: SetFinish = parse("facade.set_finish", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            match (&p.facades, p.finish, p.clear.unwrap_or(false)) {
                (None, Some(f), false) => b.finish = f,
                (Some(ids), Some(f), false) if !ids.is_empty() => {
                    for id in ids {
                        b.facade_mut(id)?.finish = Some(f);
                    }
                }
                (Some(ids), None, true) if !ids.is_empty() => {
                    for id in ids {
                        b.facade_mut(id)?.finish = None;
                    }
                }
                _ => return Err(ModelError::Invalid("give `finish` (with or without `facades`), or `facades` with `clear: true`".into()).into()),
            }
            let b = s.doc.building(p.building.as_deref())?;
            Ok(
                json!({"building": b.finish, "facades": b.facades.iter().filter_map(|f| f.finish.map(|x| json!({"facade": f.id, "finish": x}))).collect::<Vec<_>>()}),
            )
        },
    },
    CommandSpec {
        id: "facade.set_bands",
        summary: "Give facades horizontal rendered bands standing proud of the wall (slab edges, spandrels, cornices): each band \
                  runs on `levels` from `bottom` to `top` mm relative to each storey's floor (negative reaches below it), \
                  `projection` mm proud (default 40). Openings are cut through. Replaces the facades' bands; `bands: []` clears.",
        mutates: true,
        journaled: true,
        schema: schema::<SetBands>,
        run: |s, v| {
            let p: SetBands = parse("facade.set_bands", v)?;
            if p.facades.is_empty() || p.bands.len() > 50 {
                return Err(ModelError::Invalid("give one or more facades and at most 50 bands".into()).into());
            }
            let b = s.doc.building_mut(p.building.as_deref())?;
            let storeys = b.storeys.len();
            for band in &p.bands {
                trigon_model::checked("band bottom", band.bottom, -10_000.0, 10_000.0)?;
                trigon_model::checked("band top", band.top, band.bottom + 10.0, 10_000.0)?;
                trigon_model::checked("band projection", band.projection, 1.0, 500.0)?;
                if band.levels.is_empty() || band.levels.iter().any(|l| *l >= storeys) {
                    return Err(ModelError::Invalid(format!("band levels must be storey indices below {storeys}")).into());
                }
            }
            for id in &p.facades {
                b.facade_mut(id)?.bands = p.bands.clone();
            }
            trigon_model::build(&s.doc)?;
            Ok(json!({"facades": p.facades, "bands": p.bands.len()}))
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
        id: "types.install_copenhagen",
        summary: "Install original parametric Danish Dannebrog, crosspost, segmental/round, true/applied muntin and panel-door families. Explicit style defaults, not surveyed or certified products. Existing definitions are never overwritten. Atomic and undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<NoParams>,
        run: |s, v| {
            parse::<NoParams>("types.install_copenhagen", v)?;
            let mut installed = Vec::new();
            for ty in trigon_model::component::copenhagen_types() {
                ty.validate()?;
                if let Some(existing) = s.doc.types.iter().find(|t| t.id == ty.id) {
                    if existing != &ty {
                        return Err(ModelError::Invalid(format!(
                            "type {} already has different parameters; rename it or install in a new document",
                            ty.id
                        ))
                        .into());
                    }
                } else {
                    installed.push(ty.id.clone());
                    s.doc.types.push(ty);
                }
            }
            Ok(json!({"installed":installed,"family_defaults":true}))
        },
    },
    CommandSpec {
        id: "types.define",
        summary: "Add a component type or replace one with the same id. Facade uses are re-checked and all hosted geometry, including dormer windows, is built before committing.",
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
            trigon_model::build(&s.doc)?;
            Ok(json!({"defined": t.id}))
        },
    },
    CommandSpec {
        id: "types.export",
        summary: "Export selected component types (or all) to a portable, parametric .trigonlib library.",
        mutates: false,
        journaled: false,
        schema: schema::<crate::libraries::Export>,
        run: |s, v| crate::libraries::export(s, parse("types.export", v)?),
    },
    CommandSpec {
        id: "types.import",
        summary: "Import a parametric .trigonlib library; replace explicitly updates all linked instances as one undoable change.",
        mutates: true,
        journaled: true,
        schema: schema::<crate::libraries::Import>,
        run: |s, v| crate::libraries::import(s, parse("types.import", v)?),
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
        id: "drawing.trace_walls",
        summary: "Read candidate interior walls off a registered plan for one storey: dark bands as thick as walls (min_mm to \
                  max_mm, default 70–450) running at least min_len (default 600) along the building's wall directions, inside \
                  the outer walls, and stair flights (evenly spaced treads; walls inside a flight are dropped). Returns them; `add: true` \
                  also adds them for the given `levels` (default: the traced storey).",
        mutates: true,
        journaled: true,
        schema: schema::<TraceWalls>,
        run: |s, v| {
            let p: TraceWalls = parse("drawing.trace_walls", v)?;
            let dark = trigon_model::checked("contrast", p.contrast.unwrap_or(60.0), 5.0, 250.0)?;
            let (mn, mx) = (
                trigon_model::checked("min_mm", p.min_mm.unwrap_or(70.0), 30.0, 500.0)?,
                trigon_model::checked("max_mm", p.max_mm.unwrap_or(450.0), 60.0, 1500.0)?,
            );
            let min_len = trigon_model::checked("min_len", p.min_len.unwrap_or(600.0), 200.0, 10_000.0)?;
            let (mut walls, mut stairs) = crate::drawings::trace_walls(s, &p.name, p.level, dark, mn, mx, min_len)?;
            let total_m: f64 = walls.iter().map(|w| ((w.b[0] - w.a[0]).powi(2) + (w.b[1] - w.a[1]).powi(2)).sqrt()).sum::<f64>() / 1000.0;
            let summary = json!({
                "walls": walls.len(),
                "load_bearing": walls.iter().filter(|w| w.load_bearing).count(),
                "total_length_m": total_m.round(),
                "stair_flights": stairs.len(),
            });
            if p.add.unwrap_or(false) {
                let levels = p.levels.clone().unwrap_or_else(|| vec![p.level]);
                let b = s.doc.building_mut(None)?;
                let start = b.interior_walls.len();
                for (k, w) in walls.iter_mut().enumerate() {
                    w.levels = levels.clone();
                    w.id = format!("{}.iw{}", b.id, start + k);
                    w.validate(b.storeys.len())?;
                }
                b.interior_walls.extend(walls);
                let added = b.interior_walls.len() - start;
                let mut stairs_added = 0;
                if p.stairs.unwrap_or(true) {
                    let s0 = b.stairs.len();
                    for (k, st) in stairs.iter_mut().enumerate() {
                        st.levels = levels.clone();
                        st.id = format!("{}.stair{}", b.id, s0 + k);
                    }
                    stairs_added = stairs.len();
                    b.stairs.extend(stairs);
                }
                return Ok(json!({"traced": summary, "added": added, "stairs_added": stairs_added}));
            }
            Ok(json!({"traced": summary, "walls": walls, "stairs": stairs}))
        },
    },
    CommandSpec {
        id: "drawing.to_site",
        summary: "Convert drawing pixels on a registered plan to site mm (x east, y north), to place walls, stairs and rooms read \
                  off the drawing.",
        mutates: false,
        journaled: false,
        schema: schema::<ToSite>,
        run: |s, v| {
            let p: ToSite = parse("drawing.to_site", v)?;
            let d = s.doc.drawings.iter().find(|d| d.name == p.name).ok_or_else(|| ModelError::NotFound(format!("drawing {}", p.name)))?;
            let reg = d
                .plans
                .iter()
                .find(|r| r.levels.contains(&p.level))
                .ok_or_else(|| ModelError::NotFound(format!("drawing {} has no plan registered for level {}", p.name, p.level)))?;
            if p.points.len() > 10_000 || p.points.iter().flatten().any(|v| !v.is_finite()) {
                return Err(ModelError::Invalid("give up to 10000 finite points".into()).into());
            }
            Ok(json!(
                p.points
                    .iter()
                    .map(|q| {
                        let r = reg.to_site(*q);
                        [r[0].round(), r[1].round()]
                    })
                    .collect::<Vec<_>>()
            ))
        },
    },
    CommandSpec {
        id: "drawing.fit_stair",
        summary: "Measure a U-turn staircase on a registered plan from a rough box around it (four drawing pixels: p0→p1 along \
                  the flights towards the half landing, p1→p2 across): finds the tread lines, stringers and well and returns the \
                  exact stair in site mm. `add: true` adds it on `levels` (default: this storey) and removes traced walls inside it.",
        mutates: true,
        journaled: true,
        schema: schema::<FitStair>,
        run: |s, v| {
            let p: FitStair = parse("drawing.fit_stair", v)?;
            let fit = crate::drawings::fit_stair(s, &p.name, p.level, p.rough, p.template)?;
            let report = json!({
                "treads_found": fit.treads,
                "tread_mm": fit.tread_mm.round(),
                "run_mm": fit.length_mm.round(),
                "width_mm": fit.width_mm.round(),
                "well_mm": fit.well_mm.round(),
                "lines_across_mm": fit.lines_across.iter().map(|(t, v)| [t.round(), (v * 100.0).round() / 100.0]).collect::<Vec<_>>(),
                "outline": fit.stair.outline.iter().map(|p| [p[0].round(), p[1].round()]).collect::<Vec<_>>(),
            });
            if p.add.unwrap_or(false) {
                let mut st = fit.stair;
                st.levels = p.levels.unwrap_or_else(|| vec![p.level]);
                let b = s.doc.building_mut(None)?;
                st.id = format!("{}.stair{}", b.id, b.stairs.len());
                if st.levels.iter().any(|l| *l >= b.storeys.len()) {
                    return Err(ModelError::Invalid("levels must be existing storeys".into()).into());
                }
                let sym = trigon_model::stair::symbol(&st, 3000.0)?;
                let mut area: Vec<[f64; 2]> = vec![st.outline[0], st.outline[1]];
                area.extend(sym.landing.iter().copied());
                area.push(st.outline[2]);
                area.push(st.outline[3]);
                let before = b.interior_walls.len();
                b.interior_walls.retain_mut(|w| {
                    let m = [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
                    if crate::drawings::inside(&area, m) {
                        w.levels.retain(|l| !st.levels.contains(l));
                    }
                    !w.levels.is_empty()
                });
                let removed = before - b.interior_walls.len();
                b.stairs.push(st);
                trigon_model::build(&s.doc)?;
                return Ok(json!({"measured": report, "added": true, "walls_removed": removed}));
            }
            Ok(json!({"measured": report}))
        },
    },
    CommandSpec {
        id: "interior.add_walls",
        summary: "Add interior walls: centre line a–b (site mm), thickness, storeys, load-bearing or partition. Leave gaps for \
                  doors. Use drawing.trace_walls to read candidates off a registered plan.",
        mutates: true,
        journaled: true,
        schema: schema::<AddWalls>,
        run: |s, v| {
            let p: AddWalls = parse("interior.add_walls", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let n = b.storeys.len();
            if p.walls.len() > 5000 {
                return Err(ModelError::Invalid("at most 5000 walls at a time".into()).into());
            }
            for w in &p.walls {
                w.validate(n)?;
            }
            let start = b.interior_walls.len();
            for (k, mut w) in p.walls.into_iter().enumerate() {
                if w.id.is_empty() {
                    w.id = format!("{}.iw{}", b.id, start + k);
                }
                b.interior_walls.push(w);
            }
            Ok(json!({"added": b.interior_walls.len() - start, "interior_walls": b.interior_walls.len()}))
        },
    },
    CommandSpec {
        id: "interior.doors_from_gaps",
        summary: "Turn door-sized gaps in interior walls into doors (traced plans show doors as gaps): two collinear pieces \
                  of one wall either side of a `min_mm`–`max_mm` gap (default 600–1300) become one wall with a door, and a \
                  wall that stops a door's width short of another wall runs on to it with the door at its end. Gaps \
                  crossed by another wall stay. Doors are drawn as a closed leaf (no swing) and exported as doors in their \
                  wall. `levels` limits it to walls on those storeys. Undoable.",
        mutates: true,
        journaled: true,
        schema: schema::<DoorsFromGaps>,
        run: |s, v| {
            let p: DoorsFromGaps = parse("interior.doors_from_gaps", v)?;
            let rules = trigon_model::doors::Rules {
                min: trigon_model::checked("min_mm", p.min_mm.unwrap_or(600.0), 300.0, 4000.0)?,
                max: trigon_model::checked("max_mm", p.max_mm.unwrap_or(1300.0), 300.0, 4000.0)?,
                height: trigon_model::checked("height_mm", p.height_mm.unwrap_or(trigon_model::doc::door_height()), 1000.0, 4000.0)?,
            };
            if rules.max < rules.min {
                return Err(ModelError::Invalid("max_mm must be at least min_mm".into()).into());
            }
            let b = s.doc.building_mut(p.building.as_deref())?;
            let storeys = b.storeys.len();
            let before = b.interior_walls.len();
            let found = trigon_model::doors::doors_from_gaps(&mut b.interior_walls, p.levels.as_deref(), &rules);
            for w in &b.interior_walls {
                w.validate(storeys)?;
            }
            let doors: usize = b.interior_walls.iter().map(|w| w.doors.len()).sum();
            trigon_model::build(&s.doc)?;
            Ok(json!({
                "doors_between_pieces": found.between,
                "doors_at_wall_ends": found.at_ends,
                "walls_before": before,
                "walls_after": s.doc.building(p.building.as_deref())?.interior_walls.len(),
                "doors_in_walls": doors,
            }))
        },
    },
    CommandSpec {
        id: "interior.regularize",
        summary: "Straighten measured interior walls onto explicit building axes, co-align fragments, merge duplicates and \
                  repair small junction gaps. Preserves door-size gaps and storey boundaries. Undoable; refresh derived room \
                  boundaries afterwards.",
        mutates: true,
        journaled: true,
        schema: schema::<RegularizeWalls>,
        run: |s, v| {
            let p: RegularizeWalls = parse("interior.regularize", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let before = b.interior_walls.clone();
            let inner = trigon_model::build::inner_ring(&b.footprint, b.wall_thickness);
            let walls = trigon_model::wall_cleanup::regularize(
                &before,
                &p.axes_deg,
                p.tolerance_mm.unwrap_or(100.0),
                p.angle_tolerance_deg.unwrap_or(3.0),
                &inner,
            )?;
            for wall in &walls {
                wall.validate(b.storeys.len())?;
            }
            let retired: Vec<_> = before.iter().filter(|old| !walls.iter().any(|w| w.id == old.id)).map(|w| w.id.clone()).collect();
            let moved = walls.iter().filter(|w| before.iter().find(|old| old.id == w.id).is_some_and(|old| old.a != w.a || old.b != w.b)).count();
            let result = json!({"before": before.len(), "after": walls.len(), "moved": moved, "retired_ids": retired, "room_boundaries_need_refresh": !b.rooms.is_empty()});
            b.interior_walls = walls;
            trigon_model::build(&s.doc)?;
            Ok(result)
        },
    },
    CommandSpec {
        id: "interior.add_stairs",
        summary: "Add staircases: four rectangle corners (site mm) on the listed departure storeys, drawn with treads in plans.",
        mutates: true,
        journaled: true,
        schema: schema::<AddStairs>,
        run: |s, v| {
            let p: AddStairs = parse("interior.add_stairs", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let n = b.storeys.len();
            let start = b.stairs.len();
            for (k, mut st) in p.stairs.into_iter().enumerate() {
                if st.levels.is_empty() || st.levels.iter().any(|l| *l >= n) {
                    return Err(ModelError::Invalid(format!("stair {k}: give at least one existing storey below {n}")).into());
                }
                trigon_model::stair::frame(&st)?;
                if st.id.is_empty() {
                    st.id = format!("{}.stair{}", b.id, start + k);
                }
                b.stairs.push(st);
            }
            let mut removed = 0;
            if p.clear_walls_inside.unwrap_or(false) {
                let stairs: Vec<_> = b.stairs[start..]
                    .iter()
                    .map(|st| Ok((st.levels.clone(), trigon_model::stair::opening(st)?)))
                    .collect::<trigon_model::Result<_>>()?;
                let before = b.interior_walls.len();
                b.interior_walls.retain_mut(|w| {
                    let m = [(w.a[0] + w.b[0]) / 2.0, (w.a[1] + w.b[1]) / 2.0];
                    for (levels, area) in &stairs {
                        if crate::drawings::inside(area, m) {
                            w.levels.retain(|l| !levels.contains(l));
                        }
                    }
                    !w.levels.is_empty()
                });
                removed = before - b.interior_walls.len();
            }
            let count = b.stairs.len();
            trigon_model::build(&s.doc)?;
            Ok(json!({"stairs": count, "walls_removed": removed}))
        },
    },
    CommandSpec {
        id: "interior.add_rooms",
        summary: "Add rooms: a name and a polygon (site mm) on the listed storeys; plans label them with name and area.",
        mutates: true,
        journaled: true,
        schema: schema::<AddRooms>,
        run: |s, v| {
            let p: AddRooms = parse("interior.add_rooms", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let n = b.storeys.len();
            let start = b.rooms.len();
            for (k, mut r) in p.rooms.into_iter().enumerate() {
                if r.polygon.len() < 3
                    || r.polygon.len() > 256
                    || r.polygon.iter().flatten().any(|v| !v.is_finite())
                    || r.levels.iter().any(|l| *l >= n)
                {
                    return Err(ModelError::Invalid(format!("room {k}: give 3 to 256 finite corners and storeys below {n}")).into());
                }
                if r.id.is_empty() {
                    r.id = format!("{}.room{}", b.id, start + k);
                }
                b.rooms.push(r);
            }
            Ok(json!({"rooms": b.rooms.len()}))
        },
    },
    CommandSpec {
        id: "interior.detect_rooms",
        summary: "Find the rooms on the given storeys: the spaces enclosed by the modelled walls and, where a plan is \
                  registered, the drawn lines (partitions, door leaves); door gaps up to 1.1 m are closed. Stores them, replacing earlier detected rooms on those storeys. Rooms smaller than min_area_m2 \
                  (default 1.0) are ignored. Plans label each room with its area.",
        mutates: true,
        journaled: true,
        schema: schema::<DetectRooms>,
        run: |s, v| {
            let p: DetectRooms = parse("interior.detect_rooms", v)?;
            let b = s.doc.building(p.building.as_deref())?;
            let levels = p.levels.clone().unwrap_or_else(|| (0..b.storeys.len()).collect());
            if levels.iter().any(|l| *l >= b.storeys.len()) {
                return Err(ModelError::Invalid("levels must be existing storeys".into()).into());
            }
            let min = trigon_model::checked("min_area_m2", p.min_area_m2.unwrap_or(1.0), 0.2, 100.0)?;
            let contrast = trigon_model::checked("contrast", p.contrast.unwrap_or(40.0), 5.0, 250.0)?;
            let use_scan = p.use_scan.unwrap_or(true);
            let mut found = Vec::new();
            for &l in &levels {
                let ink = if use_scan { crate::drawings::ink_for(s, l, contrast)? } else { None };
                let b = s.doc.building(p.building.as_deref())?;
                found.extend(crate::rooms::rooms_for(b, l, min, ink.as_ref().map(|f| f as &dyn Fn([f64; 2]) -> bool)));
            }
            let b = s.doc.building_mut(p.building.as_deref())?;
            b.rooms.retain_mut(|r| {
                if r.name.is_empty() {
                    r.levels.retain(|l| !levels.contains(l));
                }
                !r.levels.is_empty()
            });
            let per: Vec<Value> = levels
                .iter()
                .map(|l| {
                    let rs: Vec<&trigon_model::Room> = found.iter().filter(|r| r.levels.contains(l)).collect();
                    let total: f64 = rs.iter().map(|r| trigon_model::doc::polygon_area(&r.polygon)).sum::<f64>() / 1e6;
                    json!({"level": l, "rooms": rs.len(), "area_m2": total.round()})
                })
                .collect();
            b.rooms.extend(found);
            Ok(json!(per))
        },
    },
    CommandSpec {
        id: "interior.clear",
        summary: "Remove interior walls, stairs and rooms (`what`: walls, stairs, rooms or all; default all).",
        mutates: true,
        journaled: true,
        schema: schema::<InteriorClear>,
        run: |s, v| {
            let p: InteriorClear = parse("interior.clear", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let what = p.what.as_deref().unwrap_or("all");
            if !matches!(what, "all" | "walls" | "stairs" | "rooms") {
                return Err(ModelError::Invalid(format!("what {what}: use walls, stairs, rooms or all")).into());
            }
            if matches!(what, "all" | "walls") {
                b.interior_walls.clear();
            }
            if matches!(what, "all" | "stairs") {
                b.stairs.clear();
            }
            if matches!(what, "all" | "rooms") {
                b.rooms.clear();
            }
            Ok(json!({"cleared": what}))
        },
    },
    CommandSpec {
        id: "interior.list",
        summary: "Count the interior walls (load-bearing and partitions), stairs and rooms per storey, with room areas.",
        mutates: false,
        journaled: false,
        schema: schema::<BuildingParam>,
        run: |s, v| {
            let p: BuildingParam = parse("interior.list", v)?;
            let b = s.doc.building(p.building.as_deref())?;
            let per: Vec<Value> = (0..b.storeys.len())
                .map(|l| {
                    let walls: Vec<&trigon_model::InteriorWall> = b.interior_walls.iter().filter(|w| w.levels.contains(&l)).collect();
                    json!({
                        "level": l,
                        "load_bearing": walls.iter().filter(|w| w.load_bearing).count(),
                        "partitions": walls.iter().filter(|w| !w.load_bearing).count(),
                        "stairs": b.stairs.iter().filter(|x| x.levels.contains(&l)).count(),
                        "rooms": b.rooms.iter().filter(|r| r.levels.contains(&l)).map(|r| json!({"name": r.name, "area_m2": (trigon_model::doc::polygon_area(&r.polygon) / 1e4).round() / 100.0})).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(json!(per))
        },
    },
    CommandSpec {
        id: "building.load_units",
        summary: "Load the building's entrances and apartments from the registers (BBR staircases and units, DAR addresses: \
                  street, house number, access point, floor and door) and tie each entrance to the nearest staircase. Plans then \
                  label each staircase \"TV 7 TH\" per floor.",
        mutates: true,
        journaled: true,
        schema: schema::<BuildingParam>,
        run: |s, v| {
            let p: BuildingParam = parse("building.load_units", v)?;
            let origin = s.doc.site.as_ref().map(|x| x.origin_utm).ok_or_else(|| ModelError::NotFound("no site yet".into()))?;
            let bbr = s
                .doc
                .building(p.building.as_deref())?
                .attributes
                .get("bbr_id")
                .cloned()
                .ok_or_else(|| ModelError::NotFound("the building has no BBR id (load it with a Datafordeler key)".into()))?;
            let data = s.provider().entrances(&bbr)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let mut entrances: Vec<trigon_model::Entrance> = data
                .into_iter()
                .map(|e| trigon_model::Entrance {
                    street: e.street,
                    number: e.number,
                    point: [(e.point_utm[0] - origin[0]) * 1000.0, (e.point_utm[1] - origin[1]) * 1000.0],
                    stair: None,
                    units: e
                        .units
                        .into_iter()
                        .map(|u| trigon_model::Unit { floor: u.floor, door: u.door, area_m2: u.area_m2, rooms: u.rooms })
                        .collect(),
                })
                .collect();
            // Nearest staircase to each access point, each staircase used once (closest pairs first).
            let centre = |st: &trigon_model::Stair| {
                let n = st.outline.len().max(1) as f64;
                st.outline.iter().fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n])
            };
            let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
            for (i, e) in entrances.iter().enumerate() {
                for (k, st) in b.stairs.iter().enumerate() {
                    let c = centre(st);
                    pairs.push((((c[0] - e.point[0]).powi(2) + (c[1] - e.point[1]).powi(2)).sqrt(), i, k));
                }
            }
            pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (mut used_e, mut used_s) = (vec![false; entrances.len()], vec![false; b.stairs.len()]);
            let mut matched = Vec::new();
            for (d, i, k) in pairs {
                if used_e[i] || used_s[k] || d > 25_000.0 {
                    continue;
                }
                used_e[i] = true;
                used_s[k] = true;
                entrances[i].stair = Some(b.stairs[k].id.clone());
                matched.push(json!({"entrance": format!("{} {}", entrances[i].street, entrances[i].number), "stair": b.stairs[k].id, "distance_m": (d / 100.0).round() / 10.0}));
            }
            let units: usize = entrances.iter().map(|e| e.units.len()).sum();
            b.entrances = entrances;
            Ok(json!({"entrances": b.entrances.len(), "units": units, "matched": matched}))
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
        summary: "Draw a floor plan of a storey, cut 1 m above its floor: walls, windows, closed door leaves and balconies with their \
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
        id: "plan.sheet",
        summary: "Compose clean landscape presentation plans: joined wall outlines, masonry hatching, balcony dimensions, \
                  entrance labels and a floor title. No scan underlay. PDF accepts several levels; SVG/PNG/DXF one. An optional \
                  site-coordinate region crops the viewport without changing the model. DXF (AutoCAD 2000, layers A-WALL-CUT, \
                  A-GLAZ, A-DOOR, A-BALC, A-ANNO, …) holds the whole storey in model space at 1:1 in mm: site coordinates, \
                  turned by rotate_deg about the site origin when one is given.",
        mutates: false,
        journaled: false,
        schema: schema::<PlanSheet>,
        run: |s, v| {
            let p: PlanSheet = parse("plan.sheet", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::plan::sheet(
                &s.doc,
                &p.levels,
                view,
                p.rotate_deg.unwrap_or(0.0),
                p.region.as_deref(),
                p.caption.as_deref().unwrap_or_default(),
                PathBuf::from(p.path),
            )
        },
    },
    CommandSpec {
        id: "elevation.sheet",
        summary: "Draw a facade elevation as a presentation sheet: the model seen square-on to one facade (walls, windows, doors, \
                  balconies, roof behind the eaves), each balcony labelled with its size (\"Ca. depth x width\") and each staircase \
                  with its entrance (\"TV 7 TH\", mirrored when seen from behind). Give `facade`, or `along` (two site points) for a \
                  stepped front. from_mm/to_mm crop along it. A line drawing: hidden lines removed, feature edges only, the \
                  outline heavier, lighter and thinner further back, light tones. .pdf, .png, .svg or .dxf (model space 1:1 in mm: \
                  x along the facade from its start, y = height above the terrain).",
        mutates: false,
        journaled: false,
        schema: schema::<ElevationSheet>,
        run: |s, v| {
            let p: ElevationSheet = parse("elevation.sheet", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let side = match (&p.facade, p.along) {
                (Some(f), None) => crate::elevation::Side::Facade(f),
                (None, Some(line)) => crate::elevation::Side::Along(line),
                _ => return Err(ModelError::Invalid("give either `facade` or `along`".into()).into()),
            };
            crate::elevation::sheet(
                &s.doc,
                side,
                view,
                (p.from_mm, p.to_mm),
                p.title.as_deref().unwrap_or("Facade"),
                p.caption.as_deref().unwrap_or_default(),
                PathBuf::from(p.path),
            )
        },
    },
    CommandSpec {
        id: "section.sheet",
        summary: "Draw a vertical section as a sheet: the model cut along a line `from` → `to` (site mm), looking to the left of \
                  that direction (`flip` looks to the right; the drawing then runs from `to` on the left). Cut walls, slabs, \
                  stairs and roofs are filled (masonry hatched, concrete grey) and outlined heavy; what lies up to `depth_mm` \
                  (default 30000) beyond the cut is drawn with hidden lines removed, lighter further back. Storey levels \
                  (+0,00 at the ground storey floor) and names at the sides, the ground line outside the building. .pdf, \
                  .png, .svg or .dxf (model space 1:1 in mm: x along the line from its left end, y = height above the terrain).",
        mutates: false,
        journaled: false,
        schema: schema::<SectionSheet>,
        run: |s, v| {
            let p: SectionSheet = parse("section.sheet", v)?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            crate::section::sheet(
                &s.doc,
                [p.from, p.to],
                p.flip.unwrap_or(false),
                p.depth_mm.unwrap_or(30_000.0),
                view,
                p.title.as_deref().unwrap_or("Snit"),
                p.caption.as_deref().unwrap_or_default(),
                PathBuf::from(p.path),
            )
        },
    },
    CommandSpec {
        id: "export.gltf",
        summary: "Export the model as glTF 2.0 binary (.glb), metres, Y up. `phase` and `filter` choose the view (default: the \
                  active phase, complete); demolished elements are translucent red. `cut_level` makes a 3D plan cut: everything \
                  above `cut_mm` (default 1200) over that storey's floor is left out, walls are closed at the cut, so the \
                  storey's rooms, doors and stairs can be seen from above.",
        mutates: false,
        journaled: false,
        schema: schema::<ExportGltf>,
        run: |s, v| {
            let p: ExportGltf = parse("export.gltf", v)?;
            let path = checked_path(&p.path, "glb")?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let refs = reference_meshes(&s.doc)?;
            let cut = match p.cut_level {
                Some(level) => {
                    let b = s.doc.building(None)?;
                    let storey = b.storeys.get(level).ok_or_else(|| ModelError::Invalid(format!("level {level} doesn't exist")))?;
                    Some(storey.elevation + trigon_model::checked("cut_mm", p.cut_mm.unwrap_or(1200.0), 100.0, storey.height)?)
                }
                None if p.cut_mm.is_some() => return Err(ModelError::Invalid("cut_mm needs cut_level".into()).into()),
                None => None,
            };
            let bytes = trigon_io::glb::write_with_site(
                &trigon_model::build::build_cut(&s.doc, view, cut)?,
                &trigon_model::site::build(&s.doc, view, cut)?,
                &refs,
            );
            write_file(&path, &bytes)?;
            Ok(json!({"written": path, "bytes": bytes.len()}))
        },
    },
    CommandSpec {
        id: "export.ifc",
        summary: "Export the model as IFC4 (.ifc) building elements for Revit, Vectorworks and other BIM tools, georeferenced to \
                  EPSG:25832: layered walls with axes and wall types, openings with typed windows and doors, floor slabs with stair \
                  openings, the roof, stairs of flights and landings, rooms as spaces, balconies as assemblies, with materials, \
                  colours, quantities and presentation layers. `phase` and `filter` choose the view; with phases, elements carry \
                  Pset_*Common.Status (EXISTING, NEW, DEMOLISH).",
        mutates: false,
        journaled: false,
        schema: schema::<ExportParams>,
        run: |s, v| {
            let p: ExportParams = parse("export.ifc", v)?;
            let path = checked_path(&p.path, "ifc")?;
            let view = view_of(&s.doc, p.phase.as_deref(), p.filter.as_deref())?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("model.ifc").to_string();
            let text = trigon_io::ifc::write_with_site(
                &s.doc,
                &trigon_model::build::build_view(&s.doc, view)?,
                &trigon_model::site::build(&s.doc, view, None)?,
                &name,
                &time::now_utc(),
            );
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
struct ToSite {
    name: String,
    /// A storey the plan is registered for.
    level: usize,
    /// Drawing pixels [x, y].
    points: Vec<[f64; 2]>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FitStair {
    name: String,
    level: usize,
    /// Rough box in drawing pixels: p0→p1 along the flights towards the half landing, p1→p2 across.
    rough: [[f64; 2]; 4],
    /// A known design to match instead of measuring freely (e.g. one stair fitted already).
    template: Option<crate::drawings::StairTemplate>,
    add: Option<bool>,
    levels: Option<Vec<usize>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TraceWalls {
    name: String,
    level: usize,
    /// How much darker than the surrounding paper a pixel must be to count as wall (0–255, default 60).
    contrast: Option<f64>,
    min_mm: Option<f64>,
    max_mm: Option<f64>,
    min_len: Option<f64>,
    /// Add the traced walls to the model.
    add: Option<bool>,
    /// Storeys the added walls stand on (default: the traced one).
    levels: Option<Vec<usize>>,
    /// Also add the stair flights found (default true).
    stairs: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddWalls {
    building: Option<String>,
    walls: Vec<trigon_model::InteriorWall>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RegularizeWalls {
    building: Option<String>,
    /// Building grids in degrees from site X, modulo 90. Supply each wing's grid explicitly.
    axes_deg: Vec<f64>,
    /// Maximum local adjustment in millimetres (default 100).
    tolerance_mm: Option<f64>,
    /// Maximum angular discrepancy accepted for snapping (default 3 degrees).
    angle_tolerance_deg: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddStairs {
    building: Option<String>,
    stairs: Vec<trigon_model::Stair>,
    /// Remove interior walls on the same storeys whose middle lies inside a staircase (traced rails and stringers).
    clear_walls_inside: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddRooms {
    building: Option<String>,
    rooms: Vec<trigon_model::Room>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DetectRooms {
    building: Option<String>,
    levels: Option<Vec<usize>>,
    min_area_m2: Option<f64>,
    /// Use the registered plan's drawn lines as room boundaries too (default true).
    use_scan: Option<bool>,
    /// How much darker than the paper counts as a drawn line (default 40).
    contrast: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InteriorClear {
    building: Option<String>,
    what: Option<String>,
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
struct PlanSheet {
    levels: Vec<usize>,
    /// .pdf for multiple vector pages; .svg, .png or .dxf for one sheet.
    path: String,
    /// Site points bounding the desired viewport. The model is never clipped or altered.
    region: Option<Vec<[f64; 2]>>,
    rotate_deg: Option<f64>,
    /// Street or drawing caption; defaults to the building name.
    caption: Option<String>,
    phase: Option<String>,
    filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ElevationSheet {
    /// Facade id, e.g. "b1.f31".
    facade: Option<String>,
    /// Or a line along a stepped front, from a to b in site mm with the outside on the right: the view is square-on to it.
    along: Option<[[f64; 2]; 2]>,
    /// .pdf, .png, .svg or .dxf.
    path: String,
    /// Start and end along the facade, mm from its start (default: all of it plus 1.5 m either side).
    from_mm: Option<f64>,
    to_mm: Option<f64>,
    /// Title in the box at the top left (default "Facade").
    title: Option<String>,
    /// Caption under the drawing.
    caption: Option<String>,
    phase: Option<String>,
    filter: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SectionSheet {
    /// Start of the cut line, site mm. The section looks to the left of from → to.
    from: [f64; 2],
    /// End of the cut line, site mm.
    to: [f64; 2],
    /// Look to the right of from → to instead (the drawing then runs from `to` on the left).
    flip: Option<bool>,
    /// How far beyond the cut to draw, mm (default 30000).
    depth_mm: Option<f64>,
    /// .pdf, .png, .svg or .dxf.
    path: String,
    /// Title in the box at the top left (default "Snit").
    title: Option<String>,
    /// Caption under the drawing.
    caption: Option<String>,
    phase: Option<String>,
    filter: Option<String>,
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
struct AddBasement {
    building: Option<String>,
    /// Basement floor, mm relative to the terrain (negative).
    floor: f64,
    name: Option<String>,
    source: Option<String>,
    attribution: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRoof {
    building: Option<String>,
    /// flat, pitched or mansard.
    kind: String,
    /// Roof pitch in degrees.
    pitch_deg: Option<f64>,
    /// Mansard lower slope (default 68°).
    lower_pitch_deg: Option<f64>,
    /// Mansard upper slope (default 25°).
    upper_pitch_deg: Option<f64>,
    /// Mansard underside break, horizontal inward run from the exterior wall (default 1300 mm).
    break_run_mm: Option<f64>,
    /// Shed dormers with coordinated roof voids, on one straight mansard face each.
    dormers: Option<Vec<trigon_model::roof::Dormer>>,
    /// Optional lower/upper slate or tagpap construction, with typed family dimensions.
    covering: Option<trigon_model::roof::Covering>,
    /// Ridge height, mm above the terrain; the pitch is worked out from it when `pitch_deg` is omitted.
    ridge: Option<f64>,
    /// Facades that end in a gable.
    gables: Option<Vec<String>>,
    /// Facade steps up to this deep (mm) get no roof faces of their own; the eave runs straight (default 1200, 0 = follow every step).
    straighten_mm: Option<f64>,
    /// How far the roof runs past the walls at the eaves, mm (default 0).
    overhang_mm: Option<f64>,
    /// How far it runs past the gable walls, mm (default 0).
    verge_mm: Option<f64>,
    /// Build-up square to the roof (rafters, battens, tiles), mm (default 0: a surface). Its underside rests on the
    /// wall line at the eaves.
    thickness_mm: Option<f64>,
    /// Gutters along the eaves (default false).
    gutters: Option<bool>,
    /// Opt-in channel detail and explicit pipe positions; requires gutters.
    drainage: Option<trigon_model::roof::drainage::Drainage>,
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
struct ExportGltf {
    /// .glb file to write.
    path: String,
    phase: Option<String>,
    filter: Option<String>,
    /// Cut the model above this storey (a 3D plan cut).
    cut_level: Option<usize>,
    /// Height of the cut above that storey's floor, mm (default 1200).
    cut_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetBasementLine {
    building: Option<String>,
    /// Deepest facade step the basement wall runs straight past, mm (0: follow the footprint).
    straighten_mm: f64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetFinish {
    building: Option<String>,
    facades: Option<Vec<String>>,
    finish: Option<trigon_model::WallFinish>,
    clear: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetBands {
    building: Option<String>,
    facades: Vec<String>,
    bands: Vec<trigon_model::doc::Band>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetCellarStairs {
    building: Option<String>,
    stairs: Vec<trigon_model::CellarStair>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPlinth {
    building: Option<String>,
    /// Top of the plinth, mm above the terrain.
    height_mm: Option<f64>,
    /// How far it stands proud of the brickwork, mm (default 20).
    projection_mm: Option<f64>,
    remove: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DoorsFromGaps {
    building: Option<String>,
    /// Only walls standing on one of these storeys.
    levels: Option<Vec<usize>>,
    /// Narrowest gap taken for a door, mm (default 600).
    min_mm: Option<f64>,
    /// Widest gap taken for a door, mm (default 1300).
    max_mm: Option<f64>,
    /// Door height, mm (default 2100).
    height_mm: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetWallSchedule {
    building: Option<String>,
    /// Facade wall thickness per storey, mm, from the lowest storey up.
    thickness: Option<Vec<f64>>,
    /// Only these facades' walls (ids from `facade.list`).
    facades: Option<Vec<String>>,
    preset: Option<WallPreset>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum WallPreset {
    /// Solid brick facades thinning by half a brick every two storeys going up.
    CopenhagenMasonry,
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
pub(crate) fn checked_path(p: &str, ext: &str) -> Result<PathBuf> {
    let path = PathBuf::from(p);
    if path.extension().and_then(|e| e.to_str()) != Some(ext) {
        return Err(EngineError::Io(format!("{p}: the file name must end in .{ext}")));
    }
    Ok(path)
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
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
    trigon_model::annotation::validate_document(&doc)?;
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
            "ground_level": b.ground_level,
            "height_mm": b.height(),
            "wall_thickness_mm": b.wall_thickness,
            "wall_schedule_mm": (0..b.storeys.len()).map(|i| b.facade_thickness(i)).collect::<Vec<_>>(),
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
    if p.kind != "mansard"
        && (p.lower_pitch_deg.is_some() || p.upper_pitch_deg.is_some() || p.break_run_mm.is_some() || p.dormers.is_some() || p.covering.is_some())
    {
        return Err(ModelError::Invalid("lower_pitch_deg, upper_pitch_deg, break_run_mm, dormers and covering require kind mansard".into()).into());
    }
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
            trigon_model::Roof::Pitched {
                pitch_deg: pitch,
                gables,
                straighten_mm: p.straighten_mm.unwrap_or(1200.0),
                overhang_mm: trigon_model::checked("overhang_mm", p.overhang_mm.unwrap_or(0.0), 0.0, 2000.0)?,
                verge_mm: trigon_model::checked("verge_mm", p.verge_mm.unwrap_or(0.0), 0.0, 1500.0)?,
                thickness_mm: trigon_model::checked("thickness_mm", p.thickness_mm.unwrap_or(0.0), 0.0, 1000.0)?,
                gutters: p.gutters.unwrap_or(false),
                drainage: p.drainage,
            }
        }
        "mansard" => {
            if p.drainage.is_some() {
                return Err(
                    ModelError::Invalid("detailed drainage is supported only on pitched roofs; mansard drainage is not implemented".into()).into()
                );
            }
            if p.pitch_deg.is_some() || p.ridge.is_some() {
                return Err(ModelError::Invalid("mansard roofs use lower_pitch_deg, upper_pitch_deg and break_run_mm".into()).into());
            }
            trigon_model::Roof::Mansard {
                parameters: trigon_model::roof::Mansard {
                    lower_pitch_deg: p.lower_pitch_deg.unwrap_or(68.0),
                    upper_pitch_deg: p.upper_pitch_deg.unwrap_or(25.0),
                    break_run_mm: p.break_run_mm.unwrap_or(1300.0),
                    gables: p.gables.unwrap_or_default(),
                    straighten_mm: p.straighten_mm.unwrap_or(1200.0),
                    overhang_mm: p.overhang_mm.unwrap_or(0.0),
                    verge_mm: p.verge_mm.unwrap_or(0.0),
                    thickness_mm: p.thickness_mm.unwrap_or(0.0),
                    gutters: p.gutters.unwrap_or(false),
                    dormers: p.dormers.unwrap_or_default(),
                    covering: p.covering,
                },
            }
        }
        other => return Err(ModelError::Invalid(format!("roof kind {other}: use flat, pitched or mansard")).into()),
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
    let mut level_list: Vec<usize> = p.levels.clone().unwrap_or_else(|| (0..levels).collect());
    let mut bay_list: Vec<usize> = p.bays.clone().unwrap_or_else(|| (0..f.bays.len()).collect());
    level_list.sort_unstable();
    level_list.dedup();
    bay_list.sort_unstable();
    bay_list.dedup();
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
                u: None,
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
    let built = trigon_model::build(doc)?;
    let site = trigon_model::site::build(doc, trigon_model::build::View::latest(doc), None)?;
    Ok(trigon_io::ifc::model_shapes(doc, &built)
        .into_iter()
        .chain(site.into_iter().map(|o| {
            let mut m = trigon_model::Mesh::default();
            for (_, part) in o.parts {
                m.append(&part);
            }
            (o.id, m)
        }))
        .filter_map(|(id, mesh)| Some((trigon_io::ifc::guid(&format!("trigon:{id}")), mesh.bounds()?)))
        .collect())
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
