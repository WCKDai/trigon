//! The command list. Add a command by adding a params struct, a function and one entry in
//! [`COMMANDS`]; the CLI and MCP pick it up from there.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use trigon_model::build::{FacadeFrame, even_bays, resolve_facade};
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
        summary: "Set the number of storeys and the floor-to-floor height (mm). Placements above the new top storey are removed.",
        mutates: true,
        journaled: true,
        schema: schema::<SetStoreys>,
        run: |s, v| {
            let p: SetStoreys = parse("building.set_storeys", v)?;
            let b = s.doc.building_mut(p.building.as_deref())?;
            let height = p.height.unwrap_or_else(|| b.storeys.first().map_or(3000.0, |s| s.height));
            b.set_storeys(p.count, height)?;
            check_building(&s.doc, p.building.as_deref())?;
            Ok(info(&s.doc))
        },
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
        id: "export.gltf",
        summary: "Export the model as glTF 2.0 binary (.glb), metres, Y up.",
        mutates: false,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("export.gltf", v)?;
            let path = checked_path(&p.path, "glb")?;
            let bytes = trigon_io::glb::write(&trigon_model::build(&s.doc)?);
            write_file(&path, &bytes)?;
            Ok(json!({"written": path, "bytes": bytes.len()}))
        },
    },
    CommandSpec {
        id: "export.ifc",
        summary: "Export the model as IFC4 (.ifc), georeferenced to EPSG:25832.",
        mutates: false,
        journaled: false,
        schema: schema::<PathParams>,
        run: |s, v| {
            let p: PathParams = parse("export.ifc", v)?;
            let path = checked_path(&p.path, "ifc")?;
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("model.ifc").to_string();
            let text = trigon_io::ifc::write(&s.doc, &trigon_model::build(&s.doc)?, &name, &time::now_utc());
            write_file(&path, text.as_bytes())?;
            Ok(json!({"written": path, "bytes": text.len()}))
        },
    },
];

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

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
    sill: Option<f64>,
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
            "height_mm": b.height(),
            "wall_thickness_mm": b.wall_thickness,
            "facades": facades(doc, b),
            "evidence": b.evidence,
        })).collect::<Vec<_>>(),
        "types": doc.types.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
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
    // Storeys from the median roof height; the agent or the user corrects it with building.set_storeys.
    let estimated = heights.as_ref().map(|h| ((h.median * 1000.0) / storey_height).round().max(1.0) as usize);
    let storeys = p.storeys.or(estimated).unwrap_or(4);
    let local: Vec<[f64; 2]> = fp.ring_utm.iter().map(|q| [(q[0] - addr.utm[0]) * 1000.0, (q[1] - addr.utm[1]) * 1000.0]).collect();
    let mut b = Building::new("b1", &addr.text, &local, storeys, storey_height)?;
    if let Some(t) = p.wall_thickness {
        b.wall_thickness = trigon_model::checked("wall thickness", t, trigon_model::doc::MIN_WALL, trigon_model::doc::MAX_WALL)?;
    }
    b.evidence.push(fp.evidence.clone());
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
        "heights_m": heights.as_ref().map(|h| json!({"terrain": h.terrain, "median_above_terrain": h.median, "top_above_terrain": h.top, "samples": h.samples})),
        "storeys": storeys,
        "storeys_source": if p.storeys.is_some() { "given" } else if estimated.is_some() { "estimated from median height" } else { "default" },
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

fn place(s: &mut Session, p: Place) -> Result<Value> {
    s.doc.component(&p.type_id)?;
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
    let mut count = 0;
    for &level in &level_list {
        for &bay in &bay_list {
            f.placements.retain(|pl| !(pl.level == level && pl.bay == bay));
            f.placements.push(Placement { level, bay, type_id: p.type_id.clone(), width: p.width, height: p.height, sill: p.sill });
            count += 1;
        }
    }
    f.placements.sort_by_key(|pl| (pl.level, pl.bay));
    check_building(&s.doc, Some(&id))?;
    Ok(json!({"facade": p.facade, "placed": count, "type": p.type_id}))
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
