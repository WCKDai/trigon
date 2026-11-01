//! Element properties resolve displayed occurrences back to their parametric source. The worker
//! reuses its built view; commands validate a candidate document before one atomic assignment.

use std::collections::{BTreeMap, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::build::{Built, BuiltElement, ElementClass, Status, View, build_view};
use trigon_model::doc::{Building, FacadeKind, WallFinish};
use trigon_model::{ComponentKind, Document, ModelError, checked};

use crate::{Result, Session};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Get {
    pub id: String,
    pub phase: Option<String>,
    pub filter: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Set {
    pub id: String,
    pub phase: Option<String>,
    pub filter: Option<String>,
    pub properties: BTreeMap<String, Value>,
}

#[derive(Clone)]
enum Source {
    Site(usize),
    Terrain,
    Facade(usize, usize),
    Placement(usize, usize, usize),
    Interior(usize, usize, usize),
    InteriorDoor(usize, usize, usize, usize),
    Stair(usize, usize, usize),
    Room(usize, usize, usize),
    Cellar(usize, usize, bool),
    Slab(usize, usize, bool),
    Roof(usize),
    Dormer(usize, usize),
    Building(usize),
}

struct Record {
    data: Value,
    source: Source,
}

fn invalid(message: impl Into<String>) -> crate::EngineError {
    ModelError::Invalid(message.into()).into()
}

fn identifier(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        return Err(invalid("element ids need 1 to 512 bytes without control characters"));
    }
    Ok(())
}

/// Canonical displayed room identity, also used by the IFC space GUID.
pub fn room_element_id(building: &Building, index: usize, level: usize) -> String {
    let root =
        building.rooms.get(index).filter(|room| !room.id.is_empty()).map_or_else(|| format!("{}.room{index}", building.id), |room| room.id.clone());
    format!("{root}.space.L{level}")
}

fn definition_id(building: &Building, kind: &str, index: usize, value: &str) -> String {
    if value.is_empty() { format!("{}.{kind}{index}", building.id) } else { value.to_string() }
}

fn field(key: &str, label: impl Into<String>, value: impl Into<Value>, editable: bool) -> Value {
    json!({"key":key,"label":label.into(),"value":value.into(),"editable":editable})
}

fn number(key: &str, label: impl Into<String>, value: impl Into<Value>, unit: &str, min: f64, max: f64) -> Value {
    let mut value = field(key, label, value, true);
    value["unit"] = json!(unit);
    value["min"] = json!(min);
    value["max"] = json!(max);
    value
}

fn choice(key: &str, label: &str, value: impl Into<Value>, choices: impl Into<Value>) -> Value {
    let mut value = field(key, label, value, true);
    value["choices"] = choices.into();
    value
}

fn handle(key: &str, label: &str, position: [f64; 3], direction: [f64; 3], value: f64, bounds: [f64; 2], gain: f64) -> Value {
    let [min, max] = bounds;
    json!({"key":key,"label":label,"position_mm":position,"direction":direction,"value":value,"min":min,"max":max,"gain":gain})
}

fn record(
    id: &str,
    kind: &str,
    building: &Building,
    occurrence: (Option<usize>, Status),
    source: Source,
    properties: Vec<Value>,
    handles: Vec<Value>,
) -> Record {
    let (level, status) = occurrence;
    Record {
        data: json!({"id":id,"kind":kind,"building":building.id,"level":level,"editable":true,"reason":null,"status":status.name(),"properties":properties,"handles":handles}),
        source,
    }
}

fn element_kind(element: &BuiltElement) -> &'static str {
    match element.class {
        ElementClass::Window => "window",
        ElementClass::Door => "door",
        ElementClass::Railing => "railing",
        ElementClass::BalconySlab => "balcony",
        ElementClass::Stair => "cellar stair",
    }
}

fn cellar_id(building: &Building, index: usize) -> Option<String> {
    let c = building.cellar_stairs.get(index)?;
    Some(if c.id.is_empty() { format!("{}.cellar{index}", c.facade) } else { c.id.clone() })
}

fn component_properties(ty: &trigon_model::ComponentType) -> Vec<Value> {
    let mut result = Vec::new();
    let depth = match ty.kind {
        ComponentKind::GuardRail { projection, handrail, .. } => Some((projection, if handrail { 35.0 } else { 20.0 }, 3000.0)),
        ComponentKind::ShapedBalcony { depth, .. } => Some((depth, 400.0, 3000.0)),
        ComponentKind::Loggia { depth, .. } => Some((depth, 300.0, 4000.0)),
        _ => ty.kind.depth().map(|depth| (depth, 300.0, 3000.0)),
    };
    if let Some((depth, min, max)) = depth {
        let label = if matches!(ty.kind, ComponentKind::GuardRail { .. }) { "Projection to guard centreline" } else { "Depth (this assembly)" };
        result.push(number("depth_mm", label, depth, "mm", min, max));
    }
    match &ty.kind {
        ComponentKind::FrenchBalcony { rail_height, .. } | ComponentKind::Balcony { rail_height, .. } | ComponentKind::Loggia { rail_height, .. } => {
            let min = if matches!(ty.kind, ComponentKind::Balcony { .. }) { 900.0 } else { 600.0 };
            result.push(number("rail_height_mm", "Guard height (this assembly)", *rail_height, "mm", min, 1500.0));
        }
        _ => {}
    }
    if let ComponentKind::Balcony { slab_width, .. } = &ty.kind {
        result.push(number("slab_width_mm", "Slab width (this assembly)", *slab_width, "mm", ty.width, 12000.0));
    }
    if let ComponentKind::BalconySlab { thickness, .. } | ComponentKind::ShapedBalcony { thickness, .. } = &ty.kind {
        let min = if matches!(ty.kind, ComponentKind::ShapedBalcony { .. }) { 80.0 } else { 60.0 };
        result.push(number("slab_thickness_mm", "Slab thickness (this assembly)", *thickness, "mm", min, 400.0));
    }
    if let ComponentKind::ShapedBalcony { plan, .. } = &ty.kind {
        result.push(field("balcony_plan", "Convex parametric plan", json!(plan), true));
    }
    result
}

fn records(doc: &Document, built: &[Built], view: View) -> Result<Vec<Record>> {
    let mut result = Vec::new();
    for model in built {
        let bi = doc.buildings.iter().position(|b| b.id == model.building_id).ok_or_else(|| invalid("built building has no document source"))?;
        let building = &doc.buildings[bi];
        if model.terrain.is_some()
            && let Some(t) = &doc.terrain
        {
            let mut terrain = record(
                "site.terrain",
                "terrain",
                building,
                (None, Status::Existing),
                Source::Terrain,
                vec![
                    field("origin_mm", "Grid origin (mm)", json!(t.origin), false),
                    field("spacing_mm", "Sample spacing (mm)", json!(t.spacing_mm), false),
                    field("rows", "Grid rows", t.heights_mm.len(), false),
                    field("columns", "Grid columns", t.heights_mm.first().map_or(0, Vec::len), false),
                    field("source", "Source", json!(t.source), false),
                ],
                Vec::new(),
            );
            terrain.data["editable"] = json!(false);
            terrain.data["reason"] = json!("Terrain is edited as a complete height grid.");
            result.push(terrain);
        }
        for wall in &model.walls {
            let Some(fi) = building.facades.iter().position(|f| f.id == wall.id) else {
                result.push(record(
                    &wall.id,
                    "wall",
                    building,
                    (None, Status::Existing),
                    Source::Building(bi),
                    vec![
                        field("building_name", "Building name (shared)", building.name.clone(), true),
                        number(
                            "basement_straighten_mm",
                            "Basement straightening (all basement walls; null disables)",
                            json!(building.basement_straighten_mm),
                            "mm",
                            0.0,
                            10000.0,
                        ),
                    ],
                    Vec::new(),
                ));
                continue;
            };
            let facade = &building.facades[fi];
            let mut properties = vec![
                field("a", "Start footprint corner (shared with adjoining facade)", json!(building.footprint.get(facade.edge)), true),
                field(
                    "b",
                    "End footprint corner (shared with adjoining facade)",
                    json!(building.footprint.get((facade.edge + 1) % building.footprint.len().max(1))),
                    true,
                ),
                choice(
                    "finish",
                    "Facade finish",
                    serde_json::to_value(facade.finish.unwrap_or(building.finish)).map_err(|e| invalid(e.to_string()))?,
                    json!(["brick", "red_brick", "yellow_brick", "render"]),
                ),
                choice(
                    "facade_kind",
                    "Facade classification",
                    serde_json::to_value(facade.kind).map_err(|e| invalid(e.to_string()))?,
                    json!(["unknown", "street", "courtyard", "party"]),
                ),
                field("length_mm", "Length", wall.frame.length, false),
                field("height_mm", "Eaves height", wall.height, false),
            ];
            let mut handles = Vec::new();
            for segment in &wall.segments {
                let key = format!("thickness_mm.L{}", segment.level);
                let name = building.storeys.get(segment.level).map_or("Storey", |s| s.name.as_str());
                properties.push(number(&key, format!("{name} wall thickness"), segment.thickness, "mm", 100.0, 1500.0));
                handles.push(handle(
                    &key,
                    "Wall thickness",
                    wall.frame.point(wall.frame.length / 2.0, segment.thickness, segment.z0 + (segment.z1 - segment.z0) / 2.0),
                    [-wall.frame.n[0], -wall.frame.n[1], 0.0],
                    segment.thickness,
                    [100.0, 1500.0],
                    1.0,
                ));
            }
            result.push(record(&wall.id, "wall", building, (None, Status::Existing), Source::Facade(bi, fi), properties, handles));
            let mut placements: HashMap<(String, String, String), Vec<usize>> = HashMap::new();
            for (pi, placement) in facade.placements.iter().enumerate() {
                let Some(status) = view.status(doc.lifetime(placement)?) else { continue };
                let opening = doc.component(&placement.type_id)?.kind.is_opening();
                let id = format!("{}.L{}.B{}{}", facade.id, placement.level, placement.bay, if opening { "" } else { ".balcony" });
                placements.entry((id, placement.type_id.clone(), status.name().to_string())).or_default().push(pi);
            }
            for element in wall.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(o.attached.iter())).chain(&wall.attachments) {
                if matches!(element.type_id.as_str(), "door.cellar" | "stair.cellar")
                    && let Some(ci) = building.cellar_stairs.iter().enumerate().find_map(|(i, _)| {
                        let id = cellar_id(building, i)?;
                        (element.id == id || element.id == format!("{id}.door")).then_some(i)
                    })
                {
                    let c = &building.cellar_stairs[ci];
                    let door = element.type_id == "door.cellar";
                    let mut properties = vec![
                        number(
                            "width_mm",
                            if door { "Door width" } else { "Pit width" },
                            if door { c.door_width } else { c.width },
                            "mm",
                            if door { 600.0 } else { 700.0 },
                            if door { 2000.0 } else { 4000.0 },
                        ),
                        number("door_at_mm", "Door centre along facade", c.door_at, "mm", 0.0, wall.frame.length),
                        number("depth_mm", "Depth below terrain (whole cellar stair)", -c.floor, "mm", 600.0, 6000.0),
                    ];
                    if door {
                        properties.push(field("height_mm", "Door height (model default)", element.height, false));
                    } else {
                        for (key, label, value) in [
                            ("from_mm", "Pit start", c.from),
                            ("to_mm", "Pit end", c.to),
                            ("top_mm", "Top tread", c.top),
                            ("bottom_mm", "Bottom tread", c.bottom),
                        ] {
                            properties.push(number(key, label, value, "mm", 0.0, wall.frame.length));
                        }
                    }
                    result.push(record(
                        &element.id,
                        element_kind(element),
                        building,
                        (Some(element.level), element.status),
                        Source::Cellar(bi, ci, door),
                        properties,
                        Vec::new(),
                    ));
                    continue;
                }
                let owner = element.id.strip_suffix(".rail").or_else(|| element.id.strip_suffix(".slab")).unwrap_or(&element.id);
                let Some(indices) = placements.get(&(owner.to_string(), element.type_id.clone(), element.status.name().to_string())) else {
                    continue;
                };
                let Some(&pi) = indices.first() else { continue };
                let placement = &facade.placements[pi];
                let ty = doc.component(&placement.type_id)?;
                let width = placement.width.unwrap_or(ty.width);
                let height = placement.height.unwrap_or(ty.height);
                let sill = placement.sill.unwrap_or(ty.sill);
                let u = placement.u.or_else(|| facade.bays.get(placement.bay).copied()).ok_or_else(|| invalid("placement has no bay centre"))?;
                let floor = building.storeys.get(placement.level).ok_or_else(|| invalid("placement has no storey"))?.elevation;
                let assembly = if owner != element.id { " (host assembly)" } else { "" };
                let (min_width, min_height, max_height) = match ty.kind {
                    ComponentKind::GuardRail { .. } => (300.0, 900.0, 1500.0),
                    ComponentKind::ShapedBalcony { .. } => (600.0, 900.0, 1500.0),
                    _ => (300.0, 300.0, 4000.0),
                };
                let handle_depth = if let ComponentKind::GuardRail { projection, .. } = ty.kind { -projection } else { 120.0 };
                let mut properties = vec![
                    number("width_mm", format!("Width{assembly}"), width, "mm", min_width, 6000.0),
                    number("height_mm", format!("Height{assembly}"), height, "mm", min_height, max_height),
                    number("sill_mm", format!("Sill / base above floor{assembly}"), sill, "mm", -floor, 3000.0),
                    number("u_mm", format!("Centre along facade{assembly}"), u, "mm", 0.0, wall.frame.length),
                    choice(
                        "type",
                        "Component type (this assembly)",
                        ty.id.clone(),
                        json!(doc.types.iter().filter(|t| t.kind.is_opening() == ty.kind.is_opening()).map(|t| t.id.clone()).collect::<Vec<_>>()),
                    ),
                    field("phase_status", "Phase status", element.status.name(), false),
                    field("source_scope", "Edit scope", "This facade assembly only; component-specific changes create a private type variant", false),
                ];
                properties.extend(component_properties(ty));
                let mut handles = vec![
                    handle(
                        "width_mm",
                        "Width",
                        wall.frame.point(u + width / 2.0, handle_depth, floor + sill + height / 2.0),
                        [wall.frame.t[0], wall.frame.t[1], 0.0],
                        width,
                        [min_width, 6000.0],
                        2.0,
                    ),
                    handle(
                        "height_mm",
                        "Height",
                        wall.frame.point(u, handle_depth, floor + sill + height),
                        [0.0, 0.0, 1.0],
                        height,
                        [min_height, max_height],
                        1.0,
                    ),
                    handle("sill_mm", "Sill / base", wall.frame.point(u, handle_depth, floor + sill), [0.0, 0.0, 1.0], sill, [-floor, 3000.0], 1.0),
                    handle(
                        "u_mm",
                        "Along facade",
                        wall.frame.point(u, handle_depth, floor + sill + height / 2.0),
                        [wall.frame.t[0], wall.frame.t[1], 0.0],
                        u,
                        [0.0, wall.frame.length],
                        1.0,
                    ),
                ];
                if let ComponentKind::GuardRail { projection, handrail, .. } = ty.kind {
                    handles.push(handle(
                        "depth_mm",
                        "Guard projection",
                        wall.frame.point(u, -projection, floor + sill + height / 2.0),
                        [wall.frame.n[0], wall.frame.n[1], 0.0],
                        projection,
                        [if handrail { 35.0 } else { 20.0 }, 3000.0],
                        1.0,
                    ));
                }
                result.push(record(
                    &element.id,
                    element_kind(element),
                    building,
                    (Some(element.level), element.status),
                    Source::Placement(bi, fi, pi),
                    properties,
                    handles,
                ));
            }
        }
        for wall in &model.interior {
            let wi = building
                .interior_walls
                .iter()
                .enumerate()
                .find_map(|(i, w)| (format!("{}.L{}", definition_id(building, "iw", i, &w.id), wall.level) == wall.id).then_some(i));
            let Some(wi) = wi else { continue };
            let source = &building.interior_walls[wi];
            let properties = vec![
                field("a", "Axis start", json!(source.a), true),
                field("b", "Axis end", json!(source.b), true),
                number("thickness_mm", "Wall thickness", source.thickness, "mm", 40.0, 1000.0),
                field("load_bearing", "Load bearing", source.load_bearing, true),
                field("scope", "Edit scope", "Selected storey only", false),
            ];
            let centre = [(source.a[0] + source.b[0]) / 2.0, (source.a[1] + source.b[1]) / 2.0, wall.z_bottom + wall.height / 2.0];
            let position = [centre[0] + wall.frame.n[0] * source.thickness / 2.0, centre[1] + wall.frame.n[1] * source.thickness / 2.0, centre[2]];
            let handles = vec![handle(
                "thickness_mm",
                "Wall thickness",
                position,
                [wall.frame.n[0], wall.frame.n[1], 0.0],
                source.thickness,
                [40.0, 1000.0],
                2.0,
            )];
            result.push(record(
                &wall.id,
                "interior wall",
                building,
                (Some(wall.level), Status::Existing),
                Source::Interior(bi, wi, wall.level),
                properties,
                handles,
            ));
            for door in &wall.doors {
                let Some(di) = door.fill.id.strip_prefix(&format!("{}.D", wall.id)).and_then(|x| x.parse::<usize>().ok()) else { continue };
                let Some(d) = source.doors.get(di) else { continue };
                let properties = vec![
                    number("width_mm", "Nominal opening width", d.width, "mm", 300.0, 4000.0),
                    number("height_mm", "Nominal opening height", d.height, "mm", 1000.0, 4000.0),
                    number("at_mm", "Centre along source wall", d.at, "mm", 0.0, (source.b[0] - source.a[0]).hypot(source.b[1] - source.a[1])),
                    field("built_width_mm", "Built width after wall joins", door.fill.width, false),
                    field("scope", "Edit scope", "Door on selected storey only", false),
                ];
                result.push(record(
                    &door.fill.id,
                    "door",
                    building,
                    (Some(wall.level), door.fill.status),
                    Source::InteriorDoor(bi, wi, wall.level, di),
                    properties,
                    Vec::new(),
                ));
            }
        }
        for stair in &model.stairs {
            let si = building
                .stairs
                .iter()
                .enumerate()
                .find_map(|(i, s)| (format!("{}.L{}", definition_id(building, "stair", i, &s.id), stair.level) == stair.id).then_some(i));
            let Some(si) = si else { continue };
            let source = &building.stairs[si];
            let frame = trigon_model::stair::frame(source)?;
            let properties = vec![
                field("outline", "Source rectangle", json!(source.outline), true),
                number("width_mm", "Flight rectangle width", frame.width, "mm", 600.0, 10000.0),
                number(
                    "length_mm",
                    "Straight run length",
                    frame.length,
                    "mm",
                    if source.kind == trigon_model::StairKind::DoubleQuarterTurn { 100.0 } else { 600.0 },
                    20000.0,
                ),
                number("tread_mm", "Tread parameter (null = automatic)", json!(source.tread), "mm", 150.0, 400.0),
                number("landing_mm", "Landing parameter (null = automatic)", json!(source.landing), "mm", 300.0, frame.width * 2.0),
                number("well_mm", "Well parameter (null = automatic)", json!(source.well), "mm", 0.0, frame.width / 3.0),
                field("rounded", "Rounded turn", source.rounded, true),
                choice(
                    "stair_kind",
                    "Stair kind",
                    serde_json::to_value(source.kind).map_err(|e| invalid(e.to_string()))?,
                    json!(["straight", "u_turn", "double_quarter_turn"]),
                ),
                field("scope", "Edit scope", "Selected storey only", false),
            ];
            let p = frame.at(frame.length / 2.0, frame.width);
            let z = building.storeys.get(stair.level).map_or(0.0, |s| s.elevation + 800.0);
            result.push(record(
                &stair.id,
                "stair",
                building,
                (Some(stair.level), Status::Existing),
                Source::Stair(bi, si, stair.level),
                properties,
                vec![handle("width_mm", "Stair width", [p[0], p[1], z], [frame.across[0], frame.across[1], 0.0], frame.width, [600.0, 10000.0], 1.0)],
            ));
        }
        for slab in &model.slabs {
            if slab.kind == trigon_model::SlabKind::Roof {
                result.push(roof_record(building, bi, &slab.id, Some(slab.level))?);
                continue;
            }
            let level = slab.level;
            let Some(storey) = building.storeys.get(level) else { continue };
            let attic = slab.id == format!("{}.attic", building.id);
            let properties = vec![
                number("thickness_mm", if attic { "Attic slab thickness" } else { "Slab thickness" }, slab.thickness, "mm", 20.0, 1000.0),
                field("storey_name", "Storey name (shared)", storey.name.clone(), true),
                number("elevation_mm", "Storey elevation (adjusts adjacent heights)", storey.elevation, "mm", -15000.0, 1000000.0),
                number("storey_height_mm", "Storey height (moves storeys above)", storey.height, "mm", 2000.0, 10000.0),
                field("scope", "Level parameter scope", "Name/elevation/height affect all elements hosted on the storey", false),
            ];
            let c = polygon_centre(&slab.polygon);
            let position = [c[0], c[1], slab.z_bottom];
            result.push(record(
                &slab.id,
                if attic { "attic slab" } else { "floor slab" },
                building,
                (Some(level), Status::Existing),
                Source::Slab(bi, level, attic),
                properties,
                vec![handle("thickness_mm", "Slab thickness", position, [0.0, 0.0, -1.0], slab.thickness, [20.0, 1000.0], 1.0)],
            ));
        }
        if let Some(roof) = &model.roof {
            result.push(roof_record(building, bi, &roof.id, None)?);
            if let trigon_model::Roof::Mansard { parameters } = &building.roof {
                for (di, d) in roof.dormers.iter().enumerate() {
                    let Some(spec) = parameters.dormers.get(di) else { continue };
                    let properties = vec![
                        number("width_mm", "Dormer window width", spec.width_mm, "mm", 300.0, 3000.0),
                        number("height_mm", "Dormer window height", spec.height_mm, "mm", 300.0, 2200.0),
                        number("u_mm", "Centre along host facade", spec.u_mm, "mm", 0.0, 1e8),
                        number("setback_mm", "Front setback from wall", spec.setback_mm, "mm", 100.0, 1500.0),
                        number("cheek_mm", "Side cheek thickness", spec.cheek_mm, "mm", 50.0, 200.0),
                        number("head_mm", "Head above window", spec.head_mm, "mm", 50.0, 400.0),
                        number("sill_clearance_mm", "Sill above host roof at rear lining", spec.sill_clearance_mm, "mm", 20.0, 300.0),
                        number("roof_pitch_deg", "Shed roof pitch", spec.roof_pitch_deg, "deg", 0.0, 15.0),
                        number("roof_thickness_mm", "Shed roof normal thickness", spec.roof_thickness_mm, "mm", 20.0, 200.0),
                        field("window_type", "Window family", spec.window_type.clone(), true),
                        field("source_scope", "Edit scope", "This parametric roof dormer", false),
                    ];
                    for id in [d.front.id.clone(), format!("{}.roof", d.id), format!("{}.cheeks", d.id)]
                        .into_iter()
                        .chain(d.front.openings.iter().map(|o| o.fill.id.clone()))
                    {
                        result.push(record(
                            &id,
                            "roof dormer",
                            building,
                            (None, Status::Existing),
                            Source::Dormer(bi, di),
                            properties.clone(),
                            Vec::new(),
                        ));
                    }
                }
            }
        }
        for (ri, room) in building.rooms.iter().enumerate() {
            for &level in &room.levels {
                if building.storeys.get(level).is_none() {
                    continue;
                }
                let properties = vec![
                    field("name", "Room name", room.name.clone(), true),
                    field("polygon", "Room boundary", json!(room.polygon), true),
                    field("area_m2", "Floor area", trigon_model::doc::polygon_area(&room.polygon) / 1e6, false),
                    field("scope", "Edit scope", "Selected storey only", false),
                ];
                result.push(record(
                    &room_element_id(building, ri, level),
                    "room",
                    building,
                    (Some(level), Status::Existing),
                    Source::Room(bi, ri, level),
                    properties,
                    Vec::new(),
                ));
            }
        }
    }
    let mut counts = HashMap::new();
    for record in &result {
        *counts.entry(record.data["id"].as_str().unwrap_or("").to_string()).or_insert(0usize) += 1;
    }
    for record in &mut result {
        let reason = if counts.get(record.data["id"].as_str().unwrap_or("")).is_some_and(|n| *n > 1) {
            Some("This ID has multiple occurrences in this view. Use a complete view to edit.")
        } else if view.phase != doc.active_index() {
            Some("Historical views are read-only. Edit in the active phase.")
        } else if record.data["status"] == "demolished" {
            Some("Demolished occurrences are read-only. Use the active complete view.")
        } else {
            None
        };
        if let Some(reason) = reason {
            record.data["editable"] = json!(false);
            record.data["reason"] = json!(reason);
            record.data["handles"] = json!([]);
            if let Some(properties) = record.data["properties"].as_array_mut() {
                for p in properties {
                    p["editable"] = json!(false);
                }
            }
        }
    }
    result.extend(crate::site::records(doc, view)?.into_iter().map(|(i, data)| Record { data, source: Source::Site(i) }));
    result.sort_by(|a, b| a.data["id"].as_str().cmp(&b.data["id"].as_str()).then(a.data["status"].as_str().cmp(&b.data["status"].as_str())));
    Ok(result)
}

fn roof_record(building: &Building, bi: usize, id: &str, level: Option<usize>) -> Result<Record> {
    let mut properties = vec![choice(
        "roof_kind",
        "Roof kind",
        match building.roof {
            trigon_model::Roof::Flat => "flat",
            trigon_model::Roof::Pitched { .. } => "pitched",
            trigon_model::Roof::Mansard { .. } => "mansard",
        },
        json!(["flat", "pitched", "mansard"]),
    )];
    match &building.roof {
        trigon_model::roof::Roof::Flat => {
            properties.push(number("thickness_mm", "Flat roof slab thickness", building.roof_slab_thickness_mm.unwrap_or(300.0), "mm", 20.0, 1000.0));
        }
        trigon_model::roof::Roof::Pitched { pitch_deg, gables, straighten_mm, overhang_mm, verge_mm, thickness_mm, gutters, drainage } => {
            properties.extend([
                number("pitch_deg", "Roof pitch", *pitch_deg, "deg", 5.0, 75.0),
                number("overhang_mm", "Eave overhang", *overhang_mm, "mm", 0.0, 2000.0),
                number("verge_mm", "Verge overhang", *verge_mm, "mm", 0.0, 1500.0),
                number("thickness_mm", "Roof build-up normal to slope", *thickness_mm, "mm", 0.0, 1000.0),
                number("straighten_mm", "Eave straightening", *straighten_mm, "mm", 0.0, 10000.0),
                field("gutters", "Gutters", *gutters, true),
                field("gables", "Gable facade IDs", json!(gables), true),
                field("drainage", "Optional drainage detail / explicit downpipes", json!(drainage), true),
            ]);
        }
        trigon_model::Roof::Mansard { parameters: p } => {
            properties.extend([
                number("lower_pitch_deg", "Lower roof pitch", p.lower_pitch_deg, "deg", 40.0, 80.0),
                number("upper_pitch_deg", "Upper roof pitch", p.upper_pitch_deg, "deg", 5.0, 45.0),
                number("break_run_mm", "Horizontal run to slope break", p.break_run_mm, "mm", 200.0, 10000.0),
                number("overhang_mm", "Eave overhang", p.overhang_mm, "mm", 0.0, 2000.0),
                number("verge_mm", "Verge overhang", p.verge_mm, "mm", 0.0, 1500.0),
                number("thickness_mm", "Roof build-up normal to each slope", p.thickness_mm, "mm", 0.0, 1000.0),
                number("straighten_mm", "Eave straightening", p.straighten_mm, "mm", 0.0, 10000.0),
                field("gutters", "Half-round gutters", p.gutters, true),
                field("gables", "Gable facade IDs", json!(p.gables), true),
                field("dormers", "Parametric roof dormers", json!(p.dormers), true),
                field("covering", "Lower/upper roof covering", json!(p.covering), true),
            ]);
        }
    }
    Ok(record(id, "roof", building, (level, Status::Existing), Source::Roof(bi), properties, Vec::new()))
}

fn polygon_centre(points: &[[f64; 2]]) -> [f64; 2] {
    let n = points.len().max(1) as f64;
    points.iter().fold([0.0; 2], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n])
}

/// One immutable descriptor pass over the geometry the worker already built.
pub fn element_properties_from_built(doc: &Document, built: &[Built], view: View) -> Result<Vec<Value>> {
    Ok(records(doc, built, view)?.into_iter().map(|r| r.data).collect())
}

pub(crate) fn get(session: &Session, params: Get) -> Result<Value> {
    identifier(&params.id)?;
    let view = crate::commands::view_of(&session.doc, params.phase.as_deref(), params.filter.as_deref())?;
    let built = build_view(&session.doc, view)?;
    let mut matches: Vec<_> = records(&session.doc, &built, view)?.into_iter().filter(|r| r.data["id"] == params.id).collect();
    if matches.len() != 1 {
        return Err(invalid(format!("{}: expected one displayed element, found {}", params.id, matches.len())));
    }
    Ok(matches.pop().ok_or_else(|| invalid("missing element"))?.data)
}

pub(crate) fn set(session: &mut Session, params: Set) -> Result<Value> {
    identifier(&params.id)?;
    if params.properties.is_empty() || params.properties.len() > 64 {
        return Err(invalid("give 1 to 64 element properties"));
    }
    let view = crate::commands::view_of(&session.doc, params.phase.as_deref(), params.filter.as_deref())?;
    let built = build_view(&session.doc, view)?;
    let mut found: Vec<_> = records(&session.doc, &built, view)?.into_iter().filter(|r| r.data["id"] == params.id).collect();
    if found.len() != 1 {
        return Err(invalid(format!("{}: expected one displayed element, found {}", params.id, found.len())));
    }
    let selected = found.pop().ok_or_else(|| invalid("missing element"))?;
    if selected.data["editable"] != true {
        return Err(invalid(selected.data["reason"].as_str().unwrap_or("element is read-only")));
    }
    for (key, value) in &params.properties {
        let property = selected.data["properties"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["key"] == *key && p["editable"] == true))
            .ok_or_else(|| invalid(format!("{key}: not an editable property of this element")))?;
        if let Some(choices) = property["choices"].as_array()
            && !choices.contains(value)
        {
            return Err(invalid(format!("{key}: select a listed value")));
        }
        if property.get("min").is_some() && !value.is_null() {
            let n = value.as_f64().ok_or_else(|| invalid(format!("{key}: expected a number")))?;
            checked(key, n, property["min"].as_f64().unwrap_or(f64::MIN), property["max"].as_f64().unwrap_or(f64::MAX))?;
        }
    }
    let mut candidate = session.doc.clone();
    let target = apply(&mut candidate, &selected.source, &params.properties, &params.id)?;
    if let Source::Slab(bi, _, _) = selected.source {
        validate_storeys(candidate.buildings.get(bi).ok_or_else(|| invalid("missing building"))?)?;
    }
    let built = build_view(&candidate, view)?;
    let descriptors = records(&candidate, &built, view)?;
    let target = if matches!(selected.source, Source::Placement(..)) && descriptors.iter().any(|r| r.data["id"] == params.id) {
        params.id
    } else if descriptors.iter().any(|r| r.data["id"] == target) || matches!(selected.source, Source::Site(_)) {
        // A site source keeps its exact ID even when a valid lifetime edit hides it in this view.
        // site::apply has validated the complete source list before reaching this branch.
        target
    } else {
        descriptors
            .iter()
            .find_map(|r| r.data["id"].as_str().filter(|id| id.starts_with(&format!("{target}."))).map(str::to_string))
            .ok_or_else(|| invalid("updated source has no displayed occurrence"))?
    };
    session.doc = candidate;
    Ok(json!({"changed":1,"targets":[target]}))
}

fn numeric(key: &str, value: &Value) -> Result<f64> {
    value.as_f64().filter(|x| x.is_finite()).ok_or_else(|| invalid(format!("{key}: expected a finite number")))
}
fn text(key: &str, value: &Value) -> Result<String> {
    let s = value.as_str().ok_or_else(|| invalid(format!("{key}: expected text")))?;
    if s.is_empty() || s.len() > 512 || s.chars().any(char::is_control) {
        return Err(invalid(format!("{key}: use 1 to 512 bytes without control characters")));
    }
    Ok(s.to_string())
}
fn boolean(key: &str, value: &Value) -> Result<bool> {
    value.as_bool().ok_or_else(|| invalid(format!("{key}: expected true or false")))
}
fn point(key: &str, value: &Value) -> Result<[f64; 2]> {
    let p: [f64; 2] = serde_json::from_value(value.clone()).map_err(|_| invalid(format!("{key}: expected [x,y] millimetres")))?;
    for n in p {
        checked("site coordinate", n, -1e8, 1e8)?;
    }
    Ok(p)
}
fn polygon(key: &str, value: &Value, exact: Option<usize>) -> Result<Vec<[f64; 2]>> {
    let values = value.as_array().ok_or_else(|| invalid(format!("{key}: expected a point array")))?;
    if !(3..=2000).contains(&values.len()) || exact.is_some_and(|n| values.len() != n) {
        return Err(invalid(format!("{key}: invalid point count")));
    }
    let points = values.iter().map(|v| point(key, v)).collect::<Result<Vec<_>>>()?;
    let n = points.len();
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1.0 {
            return Err(invalid("polygon edges must be at least 1 mm"));
        }
        for j in i + 1..n {
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let (c, d) = (points[j], points[(j + 1) % n]);
            let (ab_c, ab_d, cd_a, cd_b) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
            if ab_c * ab_d <= 0.0
                && cd_a * cd_b <= 0.0
                && a[0].min(b[0]) <= c[0].max(d[0])
                && c[0].min(d[0]) <= a[0].max(b[0])
                && a[1].min(b[1]) <= c[1].max(d[1])
                && c[1].min(d[1]) <= a[1].max(b[1])
            {
                return Err(invalid("polygon must not intersect itself"));
            }
        }
    }
    if trigon_model::doc::polygon_area(&points) < 10000.0 {
        return Err(invalid("polygon area must be at least 0.01 m2"));
    }
    Ok(points)
}

fn ensure_definition_id(doc: &mut Document, bi: usize, index: usize, kind: &str) -> Result<String> {
    let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing building"))?;
    let fallback = format!("{}.{kind}{index}", b.id);
    let id = match kind {
        "iw" => &mut b.interior_walls.get_mut(index).ok_or_else(|| invalid("missing wall"))?.id,
        "stair" => &mut b.stairs.get_mut(index).ok_or_else(|| invalid("missing stair"))?.id,
        "room" => &mut b.rooms.get_mut(index).ok_or_else(|| invalid("missing room"))?.id,
        _ => return Err(invalid("unknown source kind")),
    };
    if id.is_empty() {
        *id = fallback;
    }
    Ok(id.clone())
}

fn apply(doc: &mut Document, source: &Source, properties: &BTreeMap<String, Value>, selected_id: &str) -> Result<String> {
    match *source {
        Source::Site(index) => crate::site::apply(doc, index, properties),
        Source::Terrain => Err(invalid("terrain is edited as a complete height grid")),
        Source::Facade(bi, fi) => {
            let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing building"))?;
            let edge = b.facades.get(fi).ok_or_else(|| invalid("missing facade"))?.edge;
            for (key, value) in properties {
                if let Some(level) = key.strip_prefix("thickness_mm.L").and_then(|x| x.parse::<usize>().ok()) {
                    if level >= b.storeys.len() {
                        return Err(invalid("wall thickness storey does not exist"));
                    }
                    let schedule: Vec<_> = (0..b.storeys.len()).map(|l| b.edge_thickness(edge, l)).collect();
                    b.facades[fi].wall_schedule = schedule;
                    b.facades[fi].wall_schedule[level] = checked(key, numeric(key, value)?, 100.0, 1500.0)?;
                } else {
                    match key.as_str() {
                        "a" | "b" => {
                            let i = if key == "a" { edge } else { (edge + 1) % b.footprint.len().max(1) };
                            let p = point(key, value)?;
                            *b.footprint.get_mut(i).ok_or_else(|| invalid("missing footprint corner"))? = p;
                        }
                        "finish" => {
                            b.facades[fi].finish =
                                Some(serde_json::from_value::<WallFinish>(value.clone()).map_err(|_| invalid("unknown facade finish"))?)
                        }
                        "facade_kind" => {
                            b.facades[fi].kind =
                                serde_json::from_value::<FacadeKind>(value.clone()).map_err(|_| invalid("unknown facade classification"))?
                        }
                        _ => return Err(invalid("unknown facade property")),
                    }
                }
            }
            let ring = polygon("footprint", &json!(b.footprint), Some(b.footprint.len()))?;
            if trigon_model::footprint::signed_area2(&ring) <= 0.0 {
                return Err(invalid("footprint corner edits must preserve counter-clockwise order"));
            }
            if trigon_model::doc::polygon_area(&ring) < 4_000_000.0 {
                return Err(invalid("building footprint must be at least 4 m2"));
            }
            Ok(selected_id.to_string())
        }
        Source::Placement(bi, fi, pi) => {
            let building = doc.buildings.get(bi).ok_or_else(|| invalid("missing building"))?;
            let facade = building.facades.get(fi).ok_or_else(|| invalid("missing facade"))?;
            let placement = facade.placements.get(pi).ok_or_else(|| invalid("missing placement"))?;
            let id = format!("placement:{}:{pi}", facade.id);
            let original_type = doc.component(&placement.type_id)?.clone();
            let custom = properties
                .keys()
                .any(|k| matches!(k.as_str(), "depth_mm" | "rail_height_mm" | "slab_width_mm" | "slab_thickness_mm" | "balcony_plan"));
            if custom && properties.contains_key("type") {
                return Err(invalid("change the component type separately from its assembly parameters"));
            }
            let new_type = if custom {
                let mut ty = original_type;
                for (key, value) in properties {
                    let n = || numeric(key, value);
                    match (key.as_str(), &mut ty.kind) {
                        (
                            "depth_mm",
                            ComponentKind::Balcony { depth, .. }
                            | ComponentKind::BalconySlab { depth, .. }
                            | ComponentKind::SteelBalcony { depth, .. }
                            | ComponentKind::ShapedBalcony { depth, .. }
                            | ComponentKind::Loggia { depth, .. }
                            | ComponentKind::GuardRail { projection: depth, .. },
                        ) => *depth = n()?,
                        (
                            "rail_height_mm",
                            ComponentKind::Balcony { rail_height, .. }
                            | ComponentKind::FrenchBalcony { rail_height, .. }
                            | ComponentKind::Loggia { rail_height, .. },
                        ) => *rail_height = n()?,
                        ("slab_width_mm", ComponentKind::Balcony { slab_width, .. }) => *slab_width = n()?,
                        ("slab_thickness_mm", ComponentKind::BalconySlab { thickness, .. } | ComponentKind::ShapedBalcony { thickness, .. }) => {
                            *thickness = n()?
                        }
                        ("balcony_plan", ComponentKind::ShapedBalcony { plan, .. }) => {
                            *plan = serde_json::from_value(value.clone()).map_err(|_| invalid("balcony_plan requires a typed convex balcony plan"))?
                        }
                        ("balcony_plan", _) => return Err(invalid("balcony_plan requires a shaped balcony")),
                        _ => {}
                    }
                }
                let base = ty.id.clone();
                let new = (1..=10000)
                    .map(|i| format!("{base}.instance{i}"))
                    .find(|id| !doc.types.iter().any(|t| t.id == *id))
                    .ok_or_else(|| invalid("too many private component variants"))?;
                identifier(&new)?;
                ty.id = new.clone();
                ty.name = format!("{} (instance)", ty.name);
                ty.validate()?;
                doc.types.push(ty);
                Some(new)
            } else {
                None
            };
            let result = crate::edit::update_parameters(doc, &id, |item| {
                let crate::edit::Item::Placement(p) = item else { return Err(invalid("wrong placement source")) };
                if let Some(id) = new_type {
                    p.type_id = id;
                }
                for (key, value) in properties {
                    match key.as_str() {
                        "width_mm" => p.width = Some(numeric(key, value)?),
                        "height_mm" => p.height = Some(numeric(key, value)?),
                        "sill_mm" => p.sill = Some(numeric(key, value)?),
                        "u_mm" => p.u = Some(numeric(key, value)?),
                        "type" => p.type_id = text(key, value)?,
                        "depth_mm" | "rail_height_mm" | "slab_width_mm" | "slab_thickness_mm" | "balcony_plan" => {}
                        _ => return Err(invalid("unknown placement property")),
                    }
                }
                Ok(())
            })?;
            Ok(result)
        }
        Source::Interior(bi, wi, level) | Source::InteriorDoor(bi, wi, level, _) => {
            let root = ensure_definition_id(doc, bi, wi, "iw")?;
            let target = format!("{root}.L{level}");
            let di = match *source {
                Source::InteriorDoor(_, _, _, di) => Some(di),
                _ => None,
            };
            let next = crate::edit::update_parameters(doc, &target, |item| {
                let crate::edit::Item::Wall(w) = item else { return Err(invalid("wrong wall source")) };
                if let Some(di) = di {
                    let d = w.doors.get_mut(di).ok_or_else(|| invalid("missing interior door"))?;
                    for (key, value) in properties {
                        match key.as_str() {
                            "width_mm" => d.width = numeric(key, value)?,
                            "height_mm" => d.height = numeric(key, value)?,
                            "at_mm" => d.at = numeric(key, value)?,
                            _ => return Err(invalid("unknown interior door property")),
                        }
                    }
                } else {
                    for (key, value) in properties {
                        match key.as_str() {
                            "a" => w.a = point(key, value)?,
                            "b" => w.b = point(key, value)?,
                            "thickness_mm" => w.thickness = numeric(key, value)?,
                            "load_bearing" => w.load_bearing = boolean(key, value)?,
                            _ => return Err(invalid("unknown wall property")),
                        }
                    }
                }
                Ok(())
            })?;
            Ok(di.map_or(next.clone(), |di| format!("{next}.D{di}")))
        }
        Source::Stair(bi, si, level) => {
            let root = ensure_definition_id(doc, bi, si, "stair")?;
            if properties.contains_key("outline") && (properties.contains_key("width_mm") || properties.contains_key("length_mm")) {
                return Err(invalid("edit the outline separately from width/length"));
            }
            crate::edit::update_parameters(doc, &format!("{root}.L{level}"), |item| {
                let crate::edit::Item::Stair(stair) = item else { return Err(invalid("wrong stair source")) };
                let frame = trigon_model::stair::frame(stair)?;
                let width = properties.get("width_mm").map(|v| numeric("width_mm", v)).transpose()?.unwrap_or(frame.width);
                let length = properties.get("length_mm").map(|v| numeric("length_mm", v)).transpose()?.unwrap_or(frame.length);
                if properties.contains_key("width_mm") || properties.contains_key("length_mm") {
                    stair.outline = vec![frame.at(0.0, 0.0), frame.at(length, 0.0), frame.at(length, width), frame.at(0.0, width)];
                }
                for (key, value) in properties {
                    match key.as_str() {
                        "outline" => stair.outline = polygon(key, value, Some(4))?,
                        "width_mm" | "length_mm" => {}
                        "tread_mm" => stair.tread = if value.is_null() { None } else { Some(numeric(key, value)?) },
                        "landing_mm" => stair.landing = if value.is_null() { None } else { Some(numeric(key, value)?) },
                        "well_mm" => stair.well = if value.is_null() { None } else { Some(numeric(key, value)?) },
                        "rounded" => stair.rounded = boolean(key, value)?,
                        "stair_kind" => stair.kind = serde_json::from_value(value.clone()).map_err(|_| invalid("unknown stair kind"))?,
                        _ => return Err(invalid("unknown stair property")),
                    }
                }
                trigon_model::stair::frame(stair)?;
                Ok(())
            })
        }
        Source::Room(bi, ri, level) => {
            let root = ensure_definition_id(doc, bi, ri, "room")?;
            let next = crate::edit::update_parameters(doc, &format!("{root}.L{level}"), |item| {
                let crate::edit::Item::Room(room) = item else { return Err(invalid("wrong room source")) };
                for (key, value) in properties {
                    match key.as_str() {
                        "name" => room.name = text(key, value)?,
                        "polygon" => room.polygon = polygon(key, value, None)?,
                        _ => return Err(invalid("unknown room property")),
                    }
                }
                Ok(())
            })?;
            let (root, l) = next.rsplit_once(".L").ok_or_else(|| invalid("updated room has no storey"))?;
            Ok(format!("{root}.space.L{l}"))
        }
        Source::Cellar(bi, ci, door) => {
            let c = doc.buildings.get_mut(bi).and_then(|b| b.cellar_stairs.get_mut(ci)).ok_or_else(|| invalid("missing cellar stair"))?;
            for (key, value) in properties {
                let n = numeric(key, value)?;
                match key.as_str() {
                    "width_mm" => {
                        if door {
                            c.door_width = n
                        } else {
                            c.width = n
                        }
                    }
                    "door_at_mm" => c.door_at = n,
                    "depth_mm" => c.floor = -n,
                    "from_mm" => c.from = n,
                    "to_mm" => c.to = n,
                    "top_mm" => c.top = n,
                    "bottom_mm" => c.bottom = n,
                    _ => return Err(invalid("unknown cellar stair property")),
                }
            }
            Ok(selected_id.to_string())
        }
        Source::Slab(bi, level, attic) => {
            let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing building"))?;
            if properties.contains_key("elevation_mm") && properties.contains_key("storey_height_mm") {
                return Err(invalid("change elevation and storey height separately"));
            }
            for (key, value) in properties {
                match key.as_str() {
                    "thickness_mm" => {
                        let n = checked(key, numeric(key, value)?, 20.0, 1000.0)?;
                        if attic {
                            b.attic_thickness_mm = Some(n)
                        } else {
                            b.floor_thicknesses.resize(b.storeys.len(), 200.0);
                            *b.floor_thicknesses.get_mut(level).ok_or_else(|| invalid("missing slab storey"))? = n;
                        }
                    }
                    "storey_name" => b.storeys.get_mut(level).ok_or_else(|| invalid("missing storey"))?.name = text(key, value)?,
                    "elevation_mm" => {
                        let n = numeric(key, value)?;
                        let next = b.storeys.get(level + 1).map(|s| s.elevation);
                        let previous = level.checked_sub(1).and_then(|l| b.storeys.get(l)).map(|s| s.elevation);
                        let s = b.storeys.get_mut(level).ok_or_else(|| invalid("missing storey"))?;
                        s.elevation = n;
                        if let Some(next) = next {
                            s.height = next - n;
                        }
                        if let (Some(previous), Some(l)) = (previous, level.checked_sub(1)) {
                            b.storeys[l].height = n - previous;
                        }
                    }
                    "storey_height_mm" => {
                        let n = numeric(key, value)?;
                        let current = b.storeys.get_mut(level).ok_or_else(|| invalid("missing storey"))?;
                        let delta = n - current.height;
                        current.height = n;
                        for s in b.storeys.iter_mut().skip(level + 1) {
                            s.elevation += delta;
                        }
                    }
                    _ => return Err(invalid("unknown slab/storey property")),
                }
            }
            b.ground_floor = checked("ground floor elevation", b.storeys.get(b.ground_level).map_or(b.ground_floor, |s| s.elevation), 0.0, 300000.0)?;
            Ok(selected_id.to_string())
        }
        Source::Dormer(bi, di) => {
            let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing dormer building"))?;
            let trigon_model::Roof::Mansard { parameters } = &mut b.roof else { return Err(invalid("dormers require a mansard roof")) };
            let d = parameters.dormers.get_mut(di).ok_or_else(|| invalid("missing roof dormer"))?;
            for (key, value) in properties {
                match key.as_str() {
                    "width_mm" => d.width_mm = numeric(key, value)?,
                    "height_mm" => d.height_mm = numeric(key, value)?,
                    "u_mm" => d.u_mm = numeric(key, value)?,
                    "setback_mm" => d.setback_mm = numeric(key, value)?,
                    "cheek_mm" => d.cheek_mm = numeric(key, value)?,
                    "head_mm" => d.head_mm = numeric(key, value)?,
                    "sill_clearance_mm" => d.sill_clearance_mm = numeric(key, value)?,
                    "roof_pitch_deg" => d.roof_pitch_deg = numeric(key, value)?,
                    "roof_thickness_mm" => d.roof_thickness_mm = numeric(key, value)?,
                    "window_type" => d.window_type = text(key, value)?,
                    _ => return Err(invalid("unknown roof dormer property")),
                }
            }
            d.validate()?;
            Ok(selected_id.to_string())
        }
        Source::Roof(bi) => {
            let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing building"))?;
            if let Some(kind) = properties.get("roof_kind") {
                match kind.as_str() {
                    Some("flat") => b.roof = trigon_model::roof::Roof::Flat,
                    Some("pitched") if !matches!(b.roof, trigon_model::roof::Roof::Pitched { .. }) => {
                        b.roof = trigon_model::roof::Roof::Pitched {
                            pitch_deg: 35.0,
                            gables: Vec::new(),
                            straighten_mm: 1200.0,
                            overhang_mm: 0.0,
                            verge_mm: 0.0,
                            thickness_mm: 0.0,
                            gutters: false,
                            drainage: None,
                        }
                    }
                    Some("pitched") => {}
                    Some("mansard") if !matches!(b.roof, trigon_model::Roof::Mansard { .. }) => {
                        b.roof = trigon_model::Roof::Mansard { parameters: trigon_model::roof::Mansard::default() };
                    }
                    Some("mansard") => {}
                    _ => return Err(invalid("unknown roof kind")),
                }
            }
            for (key, value) in properties {
                if key == "roof_kind" {
                    continue;
                }
                match &mut b.roof {
                    trigon_model::roof::Roof::Flat => {
                        if key == "thickness_mm" {
                            b.roof_slab_thickness_mm = Some(numeric(key, value)?)
                        } else {
                            return Err(invalid("this roof property requires a pitched roof"));
                        }
                    }
                    trigon_model::roof::Roof::Pitched {
                        pitch_deg,
                        gables,
                        straighten_mm,
                        overhang_mm,
                        verge_mm,
                        thickness_mm,
                        gutters,
                        drainage,
                    } => match key.as_str() {
                        "pitch_deg" => *pitch_deg = numeric(key, value)?,
                        "gables" => {
                            let ids: Vec<String> = serde_json::from_value(value.clone()).map_err(|_| invalid("gables must be facade IDs"))?;
                            if ids.len() > 200 {
                                return Err(invalid("at most 200 gables"));
                            }
                            *gables = ids;
                        }
                        "straighten_mm" => *straighten_mm = numeric(key, value)?,
                        "overhang_mm" => *overhang_mm = numeric(key, value)?,
                        "verge_mm" => *verge_mm = numeric(key, value)?,
                        "thickness_mm" => *thickness_mm = numeric(key, value)?,
                        "gutters" => *gutters = boolean(key, value)?,
                        "drainage" => *drainage = serde_json::from_value(value.clone()).map_err(|e| invalid(format!("drainage: {e}")))?,
                        _ => return Err(invalid("unknown roof property")),
                    },
                    trigon_model::Roof::Mansard { parameters: p } => match key.as_str() {
                        "covering" => {
                            p.covering =
                                serde_json::from_value(value.clone()).map_err(|_| invalid("covering requires typed lower and upper roof surfaces"))?
                        }
                        "lower_pitch_deg" => p.lower_pitch_deg = numeric(key, value)?,
                        "upper_pitch_deg" => p.upper_pitch_deg = numeric(key, value)?,
                        "break_run_mm" => p.break_run_mm = numeric(key, value)?,
                        "straighten_mm" => p.straighten_mm = numeric(key, value)?,
                        "overhang_mm" => p.overhang_mm = numeric(key, value)?,
                        "verge_mm" => p.verge_mm = numeric(key, value)?,
                        "thickness_mm" => p.thickness_mm = numeric(key, value)?,
                        "gutters" => p.gutters = boolean(key, value)?,
                        "dormers" => {
                            p.dormers = serde_json::from_value(value.clone()).map_err(|_| invalid("dormers requires typed dormer parameters"))?
                        }
                        "gables" => {
                            let ids: Vec<String> = serde_json::from_value(value.clone()).map_err(|_| invalid("gables must be facade IDs"))?;
                            if ids.len() > 200 {
                                return Err(invalid("at most 200 gables"));
                            }
                            p.gables = ids;
                        }
                        _ => return Err(invalid("unknown mansard roof property")),
                    },
                }
            }
            Ok(selected_id.to_string())
        }
        Source::Building(bi) => {
            let b = doc.buildings.get_mut(bi).ok_or_else(|| invalid("missing building"))?;
            for (key, value) in properties {
                match key.as_str() {
                    "building_name" => b.name = text(key, value)?,
                    "basement_straighten_mm" => b.basement_straighten_mm = if value.is_null() { None } else { Some(numeric(key, value)?) },
                    _ => return Err(invalid("unknown generated wall property")),
                }
            }
            Ok(selected_id.to_string())
        }
    }
}

fn validate_storeys(b: &Building) -> Result<()> {
    let mut previous = None;
    for s in &b.storeys {
        checked("storey elevation", s.elevation, -15000.0, 1000000.0)?;
        checked("storey height", s.height, 2000.0, 10000.0)?;
        if previous.is_some_and(|p| s.elevation <= p) {
            return Err(invalid("storey elevations must increase"));
        }
        previous = Some(s.elevation);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn session() -> Session {
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        let mut b = Building::new("b", "Property fixture", &[[0.0, 0.0], [20000.0, 0.0], [20000.0, 12000.0], [0.0, 12000.0]], 3, 3000.0).unwrap();
        b.facades[0].bays = vec![5000.0, 10000.0, 15000.0];
        b.facades[0].placements = serde_json::from_value(json!([
            {"level":0,"bay":0,"type":"window.casement.2"},
            {"level":1,"bay":0,"type":"window.casement.2"},
            {"level":0,"bay":1,"type":"balcony.cantilever"},
            {"level":1,"bay":1,"type":"balcony.cantilever"},
            {"level":1,"bay":2,"type":"balcony.minaltan"}
        ]))
        .unwrap();
        b.interior_walls = serde_json::from_value(json!([{
            "id":"partition","levels":[0,1],"a":[2000,6000],"b":[18000,6000],
            "thickness":150,"doors":[{"at":4000,"width":900}]
        }]))
        .unwrap();
        b.stairs = serde_json::from_value(json!([{
            "id":"stairs","levels":[0,1],"outline":[[2000,8000],[5500,8000],[5500,10000],[2000,10000]],
            "kind":"u_turn","rounded":false
        }]))
        .unwrap();
        b.rooms = serde_json::from_value(json!([{
            "id":"room","name":"Living room","levels":[0,1],
            "polygon":[[1000,1000],[8000,1000],[8000,5000],[1000,5000]]
        }]))
        .unwrap();
        s.doc.buildings.push(b);
        trigon_model::build(&s.doc).unwrap();
        s
    }

    fn descriptor(s: &mut Session, id: &str) -> Value {
        s.execute("element.get", json!({"id":id})).unwrap()
    }

    fn property(value: &Value, key: &str) -> Value {
        value["properties"].as_array().unwrap().iter().find(|p| p["key"] == key).unwrap()["value"].clone()
    }

    fn update(s: &mut Session, id: &str, properties: Value) -> String {
        s.execute("element.set", json!({"id":id,"properties":properties})).unwrap()["targets"][0].as_str().unwrap().to_string()
    }

    fn guard_session() -> Session {
        let mut s = session();
        s.doc.buildings[0].facades[0].placements.retain(|p| p.type_id.starts_with("window."));
        s.execute(
            "types.define",
            json!({
                "id":"guard.flat","name":"Independent flat-bar guard","width":1500,"height":1100,"sill":0,
                "kind":{"family":"guard_rail","projection":150,"railing":"norrebro","finish":"galvanised","handrail":false}
            }),
        )
        .unwrap();
        s
    }

    #[test]
    fn standalone_guards_preserve_openings_and_walls_and_are_owned_by_the_actual_railing() {
        let mut s = guard_session();
        let initial = trigon_model::build(&s.doc).unwrap();
        for (bay, width) in [(0, 1500), (2, 1800)] {
            s.execute("facade.place", json!({"facade":"b.f1","type":"guard.flat","levels":[0],"bays":[bay],"width":width})).unwrap();
        }
        let built = trigon_model::build(&s.doc).unwrap();
        let wall = &built[0].walls[0];
        assert_eq!(wall.mesh, initial[0].walls[0].mesh, "an independent guard never cuts the wall");
        assert_eq!(wall.openings, initial[0].walls[0].openings, "a guard must coexist with its separate window without creating a fill");
        assert_eq!(wall.attachments.len(), 2);
        assert!(wall.attachments.iter().all(|e| e.class == ElementClass::Railing));
        assert_eq!(wall.attachments[0].id, "b.f1.L0.B0.balcony.rail");
        let drawing = crate::plan_drawing(&s.doc, 0, View::latest(&s.doc)).unwrap();
        for element in &wall.attachments {
            let segments: Vec<_> = drawing.element_segments.iter().filter(|line| line.id == element.id).collect();
            assert!(!segments.is_empty(), "{} has no plan ownership", element.id);
            let line = segments
                .iter()
                .max_by(|a, b| (a.segment[0][0] - a.segment[1][0]).abs().total_cmp(&(b.segment[0][0] - b.segment[1][0]).abs()))
                .unwrap();
            let [a, b] = line.segment;
            let point = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            assert_eq!(drawing.pick_element(point, 0.1).unwrap().id, element.id);
            assert_eq!(descriptor(&mut s, &element.id)["kind"], "railing");
        }
        assert_eq!(trigon_model::build(&s.doc).unwrap(), built, "guard geometry is deterministic");
    }

    #[test]
    fn guard_type_changes_preserve_overrides_and_instance_handles_edit_once_with_exact_undo() {
        let mut s = guard_session();
        s.execute("facade.place", json!({"facade":"b.f1","type":"guard.flat","levels":[0],"bays":[0]})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"guard.flat","levels":[0],"bays":[2],"width":1800})).unwrap();
        let before = s.doc.clone();
        let initial = trigon_model::build(&s.doc).unwrap();
        let mut ty = serde_json::to_value(s.doc.component("guard.flat").unwrap()).unwrap();
        ty["width"] = json!(1700);
        ty["kind"]["finish"] = json!("white");
        s.execute("types.define", ty).unwrap();
        let built = trigon_model::build(&s.doc).unwrap();
        let guards = &built[0].walls[0].attachments;
        assert_eq!([guards[0].width, guards[1].width], [1700.0, 1800.0]);
        assert!(guards.iter().all(|e| e.parts.iter().any(|(m, _)| *m == trigon_model::Material::SteelWhite)));
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(trigon_model::build(&s.doc).unwrap(), initial);

        let id = "b.f1.L0.B0.balcony.rail";
        let data = descriptor(&mut s, id);
        let handles = data["handles"].as_array().unwrap();
        let projection = handles.iter().find(|h| h["key"] == "depth_mm").unwrap();
        assert_eq!(projection["direction"], json!([0.0, -1.0, 0.0]));
        assert_eq!(projection["position_mm"], json!([5000.0, -150.0, 550.0]));
        assert_eq!(projection["min"], 20.0);
        let height = handles.iter().find(|h| h["key"] == "height_mm").unwrap();
        assert_eq!((height["min"].clone(), height["max"].clone()), (json!(900.0), json!(1500.0)));
        assert_eq!(update(&mut s, id, json!({"depth_mm":300,"height_mm":1200,"width_mm":1600})), id);
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        let edited = trigon_model::build(&s.doc).unwrap();
        let neighbour = "b.f1.L0.B2.balcony.rail";
        assert_eq!(
            edited[0].walls[0].attachments.iter().find(|e| e.id == neighbour),
            initial[0].walls[0].attachments.iter().find(|e| e.id == neighbour)
        );
        assert_eq!(property(&descriptor(&mut s, id), "depth_mm"), 300.0);
        assert_eq!(property(&descriptor(&mut s, id), "height_mm"), 1200.0);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(trigon_model::build(&s.doc).unwrap(), initial);
    }

    #[test]
    fn guards_reject_invalid_type_or_instance_parameters_without_document_or_history_changes() {
        let mut s = guard_session();
        s.execute("facade.place", json!({"facade":"b.f1","type":"guard.flat","levels":[0],"bays":[0]})).unwrap();
        let before = s.doc.clone();
        let history = s.undo.len();
        for (key, value) in [("width", json!(299)), ("height", json!(899)), ("height", json!(1501)), ("sill", json!(-1)), ("height", json!(f64::NAN))]
        {
            let mut ty = serde_json::to_value(s.doc.component("guard.flat").unwrap()).unwrap();
            ty[key] = value;
            assert!(s.execute("types.define", ty).is_err());
            assert_eq!(s.doc, before);
        }
        for (handrail, projection) in [(false, 19.0), (true, 34.0), (false, 3001.0), (false, f64::MAX), (false, f64::NEG_INFINITY)] {
            let mut ty = serde_json::to_value(s.doc.component("guard.flat").unwrap()).unwrap();
            ty["kind"]["handrail"] = json!(handrail);
            ty["kind"]["projection"] = json!(projection);
            assert!(s.execute("types.define", ty).is_err());
            assert_eq!(s.doc, before);
        }
        for properties in [json!({"depth_mm":19}), json!({"height_mm":1501}), json!({"width_mm":f64::MAX}), json!({"u_mm":0})] {
            assert!(s.execute("element.set", json!({"id":"b.f1.L0.B0.balcony.rail","properties":properties})).is_err());
            assert_eq!(s.doc, before);
            assert_eq!(s.undo.len(), history);
        }
    }

    #[test]
    fn guard_occurrence_ids_support_copy_alignment_and_storey_copy_with_exact_undo() {
        let mut s = guard_session();
        s.execute("facade.place", json!({"facade":"b.f1","type":"guard.flat","levels":[0],"bays":[0]})).unwrap();
        let before = s.doc.clone();
        let copied = s.execute("edit.copy", json!({"targets":["b.f1.L0.B0.balcony.rail"],"delta":[3000,0,0]})).unwrap();
        let copy_id = copied["targets"][0].as_str().unwrap();
        s.execute("edit.align", json!({"targets":[copy_id],"axis":"u","coordinate":9000,"anchor":"center"})).unwrap();
        let upper = s.execute("edit.copy_to_levels", json!({"targets":[copy_id],"levels":[1]})).unwrap();
        assert_eq!(upper["targets"].as_array().unwrap().len(), 1);
        assert_eq!(s.doc.buildings[0].facades[0].placements.iter().filter(|p| p.type_id == "guard.flat").count(), 3);
        trigon_model::build(&s.doc).unwrap();
        for _ in 0..3 {
            s.execute("edit.undo", json!({})).unwrap();
        }
        assert_eq!(s.doc, before);
    }

    #[test]
    fn shared_descriptors_cover_every_built_occurrence_and_query_is_immutable() {
        let mut s = session();
        let before = s.doc.clone();
        let built = trigon_model::build(&s.doc).unwrap();
        let values = element_properties_from_built(&s.doc, &built, View::latest(&s.doc)).unwrap();
        let ids: BTreeSet<_> = values.iter().map(|v| v["id"].as_str().unwrap().to_string()).collect();
        for b in &built {
            for w in &b.walls {
                assert!(ids.contains(&w.id));
                for e in w.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(o.attached.iter())).chain(&w.attachments) {
                    assert!(ids.contains(&e.id), "{}", e.id);
                }
            }
            for w in &b.interior {
                assert!(ids.contains(&w.id));
                for d in &w.doors {
                    assert!(ids.contains(&d.fill.id));
                }
            }
            for st in &b.stairs {
                assert!(ids.contains(&st.id));
            }
            for slab in &b.slabs {
                assert!(ids.contains(&slab.id));
            }
        }
        assert!(ids.contains("room.space.L0"));
        assert_eq!(property(&descriptor(&mut s, "b.f1.L0.B0"), "width_mm"), 1200.0);
        assert_eq!(property(&descriptor(&mut s, "room.space.L0"), "area_m2"), 28.0);
        for value in &values {
            assert_eq!(value["editable"], true, "{value}");
            for h in value["handles"].as_array().unwrap() {
                let d = h["direction"].as_array().unwrap();
                let norm = d.iter().map(|n| n.as_f64().unwrap().powi(2)).sum::<f64>().sqrt();
                assert!((norm - 1.0).abs() < 1e-9);
                assert!(h["position_mm"].as_array().unwrap().iter().all(|n| n.as_f64().unwrap().is_finite()));
            }
        }
        assert_eq!(s.doc, before);
        assert!(!s.dirty);
        assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], false);
    }

    #[test]
    fn opening_dimensions_regenerate_the_same_model_in_2d_and_3d_and_undo_once() {
        let mut s = session();
        let before = s.doc.clone();
        let drawing = crate::plan_drawing(&s.doc, 0, View::latest(&s.doc)).unwrap();
        let initial: Vec<_> = drawing.element_segments.iter().filter(|e| e.id == "b.f1.L0.B0").cloned().collect();
        let query = descriptor(&mut s, "b.f1.L0.B0");
        let width = query["handles"].as_array().unwrap().iter().find(|h| h["key"] == "width_mm").unwrap();
        assert_eq!(width["position_mm"], json!([5600.0, 120.0, 1600.0]));
        assert_eq!(width["direction"], json!([1.0, 0.0, 0.0]));
        assert_eq!(width["gain"], 2.0);
        assert_eq!(update(&mut s, "b.f1.L0.B0", json!({"width_mm":1600,"height_mm":1700,"sill_mm":700,"u_mm":5200})), "b.f1.L0.B0");
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1, "one property edit is one journal entry");
        let built = trigon_model::build(&s.doc).unwrap();
        let w = &built[0].walls[0];
        let changed = w.openings.iter().find(|o| o.fill.id == "b.f1.L0.B0").unwrap();
        assert_eq!((changed.fill.width, changed.fill.height), (1600.0, 1700.0));
        assert_eq!((changed.rect.u0, changed.rect.u1, changed.rect.v0), (4400.0, 6000.0, 700.0));
        assert_eq!(w.openings.iter().find(|o| o.fill.id == "b.f1.L1.B0").unwrap().fill.width, 1200.0);
        assert_eq!(s.doc.buildings[0].facades[0].bays, before.buildings[0].facades[0].bays);
        let next = crate::plan_drawing(&s.doc, 0, View::latest(&s.doc)).unwrap();
        let changed: Vec<_> = next.element_segments.iter().filter(|e| e.id == "b.f1.L0.B0").cloned().collect();
        assert!(!initial.is_empty() && !changed.is_empty());
        assert_ne!(initial, changed);
        let after = s.doc.clone();
        assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], true);
        assert_eq!(s.doc, before);
        assert_eq!(s.execute("edit.redo", json!({})).unwrap()["redone"], true);
        assert_eq!(s.doc, after);
    }

    #[test]
    fn properties_split_shared_walls_doors_stairs_and_spaces_at_the_picked_storey() {
        let mut s = session();
        let before = trigon_model::build(&s.doc).unwrap();
        let wall = update(&mut s, "partition.L1", json!({"thickness_mm":240,"b":[17500,6000]}));
        assert_ne!(wall, "partition.L1");
        let built = trigon_model::build(&s.doc).unwrap();
        assert_eq!(built[0].interior.iter().find(|w| w.id == "partition.L0"), before[0].interior.iter().find(|w| w.id == "partition.L0"));
        assert_eq!(built[0].interior.iter().find(|w| w.id == wall).unwrap().thickness, 240.0);
        let door = update(&mut s, "partition.L0.D0", json!({"width_mm":1100,"at_mm":4500}));
        assert_eq!(property(&descriptor(&mut s, &door), "width_mm"), 1100.0);
        let stair = update(&mut s, "stairs.L1", json!({"width_mm":2300,"tread_mm":275,"well_mm":180,"landing_mm":1100}));
        assert_ne!(stair, "stairs.L1");
        assert_eq!(property(&descriptor(&mut s, "stairs.L0"), "width_mm"), 2000.0);
        assert_eq!(property(&descriptor(&mut s, &stair), "width_mm"), 2300.0);
        let space = update(&mut s, "room.space.L1", json!({"name":"Dining room","polygon":[[1000,1000],[9000,1000],[9000,5000],[1000,5000]]}));
        assert!(space.ends_with(".space.L1"));
        assert_eq!(property(&descriptor(&mut s, "room.space.L0"), "name"), "Living room");
        assert_eq!(property(&descriptor(&mut s, &space), "area_m2"), 32.0);
        // The canonical room identity also works with the pre-existing transform commands.
        s.execute("edit.move", json!({"targets":[space],"delta":[100,0,0]})).unwrap();
        assert_eq!(property(&descriptor(&mut s, "room.space.L0"), "area_m2"), 28.0);
    }

    #[test]
    fn blank_source_ids_resolve_to_builder_ids_and_split_without_renaming_other_levels() {
        let mut s = session();
        s.doc.buildings[0].interior_walls[0].id.clear();
        s.doc.buildings[0].stairs[0].id.clear();
        s.doc.buildings[0].rooms[0].id.clear();
        assert_eq!(property(&descriptor(&mut s, "b.iw0.L0"), "thickness_mm"), 150.0);
        update(&mut s, "b.iw0.L1", json!({"thickness_mm":200}));
        update(&mut s, "b.stair0.L1", json!({"width_mm":2200}));
        update(&mut s, "b.room0.space.L1", json!({"name":"Renamed"}));
        assert_eq!(property(&descriptor(&mut s, "b.iw0.L0"), "thickness_mm"), 150.0);
        assert_eq!(property(&descriptor(&mut s, "b.stair0.L0"), "width_mm"), 2000.0);
        assert_eq!(property(&descriptor(&mut s, "b.room0.space.L0"), "name"), "Living room");
    }

    #[test]
    fn scoped_component_parameters_and_facade_schedules_preserve_other_occurrences() {
        let mut s = session();
        let before = trigon_model::build(&s.doc).unwrap();
        let old_types = s.doc.types.clone();
        let slab = update(&mut s, "b.f1.L0.B1.slab", json!({"depth_mm":1600,"slab_width_mm":3400,"rail_height_mm":1100}));
        assert_eq!(slab, "b.f1.L0.B1.slab");
        assert_eq!(&s.doc.types[..old_types.len()], &old_types);
        assert_eq!(s.doc.types.len(), old_types.len() + 1);
        assert_eq!(property(&descriptor(&mut s, "b.f1.L0.B1.rail"), "depth_mm"), 1600.0);
        assert_eq!(property(&descriptor(&mut s, "b.f1.L1.B1.slab"), "depth_mm"), 1200.0);
        update(&mut s, "b.f1", json!({"thickness_mm.L1":500,"finish":"yellow_brick"}));
        let built = trigon_model::build(&s.doc).unwrap();
        let a = &built[0].walls[0];
        let original = &before[0].walls[0];
        assert_eq!(a.segment(0).unwrap().thickness, original.segment(0).unwrap().thickness);
        assert_eq!(a.segment(1).unwrap().thickness, 500.0);
        assert_eq!(
            built[0].walls[1].segments.iter().map(|s| s.thickness).collect::<Vec<_>>(),
            before[0].walls[1].segments.iter().map(|s| s.thickness).collect::<Vec<_>>()
        );
        assert_eq!(s.doc.buildings[0].facades[0].finish, Some(WallFinish::YellowBrick));
        // Changing an assembly type can remove the previously selected derived part.
        let target = update(&mut s, "b.f1.L0.B1.slab", json!({"type":"door.french"}));
        assert_eq!(target, "b.f1.L0.B1");
        assert_eq!(property(&descriptor(&mut s, &target), "type"), "door.french");
    }

    #[test]
    fn slab_parameters_preserve_old_defaults_and_regenerate_adjacent_clearances() {
        let mut s = session();
        let original = serde_json::to_value(&s.doc).unwrap();
        for key in ["floor_thicknesses", "attic_thickness_mm", "roof_slab_thickness_mm"] {
            assert!(original["buildings"][0].get(key).is_none());
        }
        let legacy: Document = serde_json::from_value(original).unwrap();
        assert_eq!(trigon_model::build(&legacy).unwrap(), trigon_model::build(&s.doc).unwrap());
        update(&mut s, "b.slab.L1", json!({"thickness_mm":350,"storey_name":"First floor"}));
        let built = trigon_model::build(&s.doc).unwrap();
        let slab = built[0].slabs.iter().find(|slab| slab.id == "b.slab.L1").unwrap();
        assert_eq!((slab.thickness, slab.z_bottom), (350.0, 2650.0));
        assert_eq!(built[0].interior.iter().find(|w| w.level == 0).unwrap().height, 2650.0);
        assert_eq!(built[0].interior.iter().find(|w| w.level == 1).unwrap().height, 2800.0);
        assert_eq!(built[0].walls[0].segment(0).unwrap().z1, 2650.0);
        assert_eq!(s.doc.buildings[0].floor_thickness(0), 200.0);
        assert_eq!(s.doc.buildings[0].floor_thickness(2), 200.0);
        update(&mut s, "b.slab.L1", json!({"storey_height_mm":3200}));
        assert_eq!(s.doc.buildings[0].storeys[2].elevation, 6200.0);
        let copy: Document = serde_json::from_value(serde_json::to_value(&s.doc).unwrap()).unwrap();
        assert_eq!(copy, s.doc);
        let mut b = copy.buildings[0].clone();
        b.set_storeys(1, 3000.0).unwrap();
        assert_eq!(b.floor_thicknesses.len(), 1);
        b.add_basement(-2500.0, "Basement").unwrap();
        assert_eq!(b.floor_thicknesses, vec![200.0, 200.0]);
    }

    #[test]
    fn roof_and_cellar_elements_resolve_to_real_source_parameters() {
        let mut s = session();
        s.doc.buildings[0].interior_walls[0].levels.push(2);
        update(&mut s, "b.roof", json!({"thickness_mm":450}));
        assert_eq!(s.doc.buildings[0].roof_slab_thickness_mm, Some(450.0));
        update(&mut s, "b.roof", json!({"roof_kind":"pitched"}));
        update(&mut s, "b.roof", json!({"pitch_deg":40,"overhang_mm":300,"thickness_mm":180,"gutters":true}));
        let built = trigon_model::build(&s.doc).unwrap();
        assert_eq!(built[0].roof.as_ref().unwrap().pitch_deg, 40.0);
        update(&mut s, "b.attic", json!({"thickness_mm":320}));
        assert_eq!(s.doc.buildings[0].attic_thickness(), 320.0);
        assert_eq!(s.doc.buildings[0].ceiling_thickness(2), 320.0);
        update(&mut s, "b.roof", json!({"roof_kind":"flat"}));
        assert_eq!(s.doc.buildings[0].ceiling_thickness(2), 200.0, "a saved pitched-roof parameter must not change flat-roof rooms");
        assert_eq!(trigon_model::build(&s.doc).unwrap()[0].interior.iter().find(|wall| wall.level == 2).unwrap().height, 2800.0);
        s.doc.buildings[0].add_basement(-2480.0, "Basement").unwrap();
        s.doc.buildings[0].cellar_stairs = serde_json::from_value(json!([{
            "id":"cellar","facade":"b.f1","from":1000,"to":6000,"width":1500,
            "top":1200,"bottom":4000,"floor":-2480,"door_at":5000
        }]))
        .unwrap();
        assert_eq!(property(&descriptor(&mut s, "cellar.door"), "width_mm"), 900.0);
        update(&mut s, "cellar.door", json!({"width_mm":1000}));
        update(&mut s, "cellar", json!({"width_mm":1600}));
        assert_eq!(s.doc.buildings[0].cellar_stairs[0].door_width, 1000.0);
        assert_eq!(s.doc.buildings[0].cellar_stairs[0].width, 1600.0);
        let built = trigon_model::build(&s.doc).unwrap();
        assert_eq!(built[0].walls[0].openings.iter().find(|o| o.fill.id == "cellar.door").unwrap().fill.width, 1000.0);
    }

    #[test]
    fn phase_queries_refuse_ambiguous_historical_and_demolished_edits() {
        let mut s = session();
        s.execute("phase.set", json!({"phases":[{"id":"old","name":"Existing"},{"id":"new","name":"Proposal"}]})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"window.fixed","levels":[0],"bays":[0]})).unwrap();
        let view = crate::commands::view_of(&s.doc, Some("new"), Some("all")).unwrap();
        let built = build_view(&s.doc, view).unwrap();
        let all = element_properties_from_built(&s.doc, &built, view).unwrap();
        let duplicates: Vec<_> = all.iter().filter(|v| v["id"] == "b.f1.L0.B0").collect();
        assert_eq!(duplicates.len(), 2);
        assert!(duplicates.iter().all(|v| v["editable"] == false && v["handles"] == json!([])));
        let historical = s.execute("element.get", json!({"id":"b.f1.L0.B0","phase":"old"})).unwrap();
        assert_eq!(historical["editable"], false);
        s.execute("facade.demolish", json!({"facade":"b.f1","levels":[1],"bays":[0],"what":"openings"})).unwrap();
        let demolished = s.execute("element.get", json!({"id":"b.f1.L1.B0","filter":"demolition"})).unwrap();
        assert_eq!(demolished["editable"], false);
        let before = s.doc.clone();
        for request in [
            json!({"id":"b.f1.L0.B0","filter":"all","properties":{"width_mm":1400}}),
            json!({"id":"b.f1.L0.B0","phase":"old","properties":{"width_mm":1400}}),
            json!({"id":"b.f1.L1.B0","filter":"demolition","properties":{"width_mm":1400}}),
        ] {
            assert!(s.execute("element.set", request).is_err());
            assert_eq!(s.doc, before);
        }
        update(&mut s, "b.f1.L0.B0", json!({"width_mm":1400}));
        assert_eq!(property(&descriptor(&mut s, "b.f1.L0.B0"), "width_mm"), 1400.0);
    }

    #[test]
    fn editing_one_anonymous_definition_keeps_other_fallback_ids_and_sources() {
        for named_first in [false, true] {
            let mut s = session();
            let b = &mut s.doc.buildings[0];
            b.interior_walls[0].id = if named_first { "first-wall".into() } else { String::new() };
            b.interior_walls[0].levels = vec![0];
            let mut other = b.interior_walls[0].clone();
            other.id.clear();
            other.a[1] += 1000.0;
            other.b[1] += 1000.0;
            b.interior_walls.push(other.clone());
            b.stairs[0].id = if named_first { "first-stair".into() } else { String::new() };
            b.stairs[0].levels = vec![0];
            let mut other_stair = b.stairs[0].clone();
            other_stair.id.clear();
            for p in &mut other_stair.outline {
                p[0] += 8000.0;
            }
            b.stairs.push(other_stair.clone());
            b.rooms[0].id = if named_first { "first-room".into() } else { String::new() };
            b.rooms[0].levels = vec![0];
            let mut other_room = b.rooms[0].clone();
            other_room.id.clear();
            for p in &mut other_room.polygon {
                p[0] += 9000.0;
            }
            b.rooms.push(other_room.clone());
            let before = s.doc.clone();
            for (id, values) in [
                (if named_first { "first-wall.L0" } else { "b.iw0.L0" }, json!({"thickness_mm":180})),
                (if named_first { "first-stair.L0" } else { "b.stair0.L0" }, json!({"width_mm":2200})),
                (if named_first { "first-room.space.L0" } else { "b.room0.space.L0" }, json!({"name":"Edited room"})),
            ] {
                update(&mut s, id, values);
            }
            let b = &s.doc.buildings[0];
            assert_eq!(&b.interior_walls[1], &other);
            assert_eq!(&b.stairs[1], &other_stair);
            assert_eq!(&b.rooms[1], &other_room);
            for id in ["b.iw1.L0", "b.stair1.L0", "b.room1.space.L0"] {
                let built = trigon_model::build(&s.doc).unwrap();
                let values = element_properties_from_built(&s.doc, &built, View::latest(&s.doc)).unwrap();
                assert_eq!(values.iter().filter(|v| v["id"] == id).count(), 1, "{id}");
            }
            for _ in 0..3 {
                s.execute("edit.undo", json!({})).unwrap();
            }
            assert_eq!(s.doc, before);
        }
    }

    #[test]
    fn invalid_property_batches_are_atomic_and_never_report_internal_failure() {
        let mut s = session();
        let before = s.doc.clone();
        for (id, properties) in [
            ("b.f1.L0.B0", json!({"unknown":2})),
            ("b.f1.L0.B0", json!({"width_mm":"wide"})),
            ("b.f1.L0.B0", json!({"width_mm":f64::MAX})),
            ("b.f1.L0.B0", json!({"width_mm":null})),
            ("b.f1.L0.B0", json!({"width_mm":1400,"u_mm":100})),
            ("partition.L0", json!({"a":[f64::MAX,0]})),
            ("partition.L0", json!({"a":[2000,6000],"b":[2000,6000]})),
            ("stairs.L0", json!({"outline":[[0,0],[1000,0],[1500,1000],[0,1000]]})),
            ("room.space.L0", json!({"name":"Changed","polygon":[[1000,1000],[5000,5000],[1000,5000],[5000,1000]]})),
            ("b.slab.L1", json!({"elevation_mm":200})),
            ("b.slab.L0", json!({"thickness_mm":0})),
            ("b.roof", json!({"roof_kind":"spiky"})),
            ("b.f1", json!({"b":[0,0]})),
            ("b.f1.L0.B1.slab", json!({"depth_mm":1700,"type":"door.french"})),
        ] {
            let outcome = s.execute("element.set", json!({"id":id,"properties":properties}));
            assert!(outcome.is_err(), "{id} {properties}");
            assert!(!matches!(outcome, Err(crate::EngineError::Internal(_))), "{id} {properties}");
            assert_eq!(s.doc, before, "{id} {properties}");
        }
        assert!(!s.dirty);
        assert_eq!(s.execute("edit.undo", json!({})).unwrap()["undone"], false);
    }
}
