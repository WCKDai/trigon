//! The document: site, buildings, storeys, facades and placements.

use serde::{Deserialize, Serialize};

use crate::component::{ComponentType, default_types};
use crate::footprint;
use crate::{ModelError, Result, checked};

/// Format tag written into every `.trigon` file.
pub const FORMAT: &str = "trigon/1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub format: String,
    pub site: Option<Site>,
    pub buildings: Vec<Building>,
    /// The component library (parametric families with their parameters set).
    pub types: Vec<ComponentType>,
    /// Every model-changing command that produced this document, in order. Replaying it from an
    /// empty document gives the same model (given the same source data).
    #[serde(default)]
    pub journal: Vec<JournalEntry>,
    /// Linked IFC models shown with this one (neighbours, an architect's model, a survey).
    #[serde(default)]
    pub references: Vec<Reference>,
    /// Linked drawings (scanned plans, sections, elevations). Only the link is stored.
    #[serde(default)]
    pub drawings: Vec<Drawing>,
    /// Phases in time order, as in Revit: every placement is created in one and may be
    /// demolished in a later one.
    #[serde(default = "default_phases")]
    pub phases: Vec<Phase>,
    /// The phase new work goes into; the last phase when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_phase: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub id: String,
    pub name: String,
}

pub fn default_phases() -> Vec<Phase> {
    vec![Phase { id: "existing".into(), name: "Eksisterende".into() }]
}

/// A page of a drawing PDF used as an underlay, in its upright orientation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Drawing {
    pub name: String,
    pub path: String,
    /// 1-based page.
    pub page: u32,
    /// Clockwise rotation applied to the scan to make it upright: 0, 90, 180 or 270.
    pub rotate: u32,
    /// Size after rotation, pixels.
    pub width: u32,
    pub height: u32,
    /// Free text: title, archive number, scale.
    #[serde(default)]
    pub note: String,
    /// Elevations on this sheet tied to facades.
    #[serde(default)]
    pub facades: Vec<FacadeRegistration>,
    /// Plans on this sheet tied to the site.
    #[serde(default)]
    pub plans: Vec<PlanRegistration>,
}

/// Ties a drawn plan to the site with a similarity transform (scale, rotation, shift; the
/// drawing's y axis points down): `site = [a −b; b a] · (x, −y) + (tx, ty)`, mm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanRegistration {
    /// Storeys this plan shows (an "Etageplan" usually covers several).
    pub levels: Vec<usize>,
    pub a: f64,
    pub b: f64,
    pub tx: f64,
    pub ty: f64,
}

impl PlanRegistration {
    /// Least-squares fit from (drawing px, site mm) pairs; at least two.
    pub fn fit(levels: Vec<usize>, pairs: &[([f64; 2], [f64; 2])]) -> Result<Self> {
        if pairs.len() < 2 {
            return Err(ModelError::Invalid("give at least two points".into()));
        }
        if pairs.iter().flat_map(|(p, q)| [p[0], p[1], q[0], q[1]]).any(|v| !v.is_finite() || v.abs() > 1e9) {
            return Err(ModelError::Invalid("points must be finite".into()));
        }
        let n = pairs.len() as f64;
        let pc = pairs.iter().fold([0.0, 0.0], |c, (p, _)| [c[0] + p[0] / n, c[1] - p[1] / n]);
        let qc = pairs.iter().fold([0.0, 0.0], |c, (_, q)| [c[0] + q[0] / n, c[1] + q[1] / n]);
        let (mut s, mut sa, mut sb) = (0.0, 0.0, 0.0);
        for (p, q) in pairs {
            let (px, py) = (p[0] - pc[0], -p[1] - pc[1]);
            let (qx, qy) = (q[0] - qc[0], q[1] - qc[1]);
            s += px * px + py * py;
            sa += px * qx + py * qy;
            sb += px * qy - py * qx;
        }
        if !(s > 1.0) {
            return Err(ModelError::Invalid("the drawing points must be apart".into()));
        }
        let (a, b) = (sa / s, sb / s);
        Ok(Self { levels, a, b, tx: qc[0] - (a * pc[0] - b * pc[1]), ty: qc[1] - (b * pc[0] + a * pc[1]) })
    }

    pub fn to_site(&self, p: [f64; 2]) -> [f64; 2] {
        let (x, y) = (p[0], -p[1]);
        [self.a * x - self.b * y + self.tx, self.b * x + self.a * y + self.ty]
    }

    pub fn to_drawing(&self, q: [f64; 2]) -> [f64; 2] {
        let k = self.a * self.a + self.b * self.b;
        let (x, y) = (q[0] - self.tx, q[1] - self.ty);
        [(self.a * x + self.b * y) / k, -((-self.b * x + self.a * y) / k)]
    }

    /// Drawing pixels per metre.
    pub fn px_per_m(&self) -> f64 {
        1000.0 / (self.a * self.a + self.b * self.b).sqrt()
    }
}

/// Ties a drawn elevation to a facade: two x positions (the facade's ends) and two heights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FacadeRegistration {
    pub facade: String,
    /// Drawing x (pixels) of the facade's start and end.
    pub x_start: f64,
    pub x_end: f64,
    /// Two `[drawing y in pixels, height above the terrain in mm]` pairs.
    pub y_levels: [[f64; 2]; 2],
}

impl FacadeRegistration {
    /// Drawing pixel of the facade point `u` mm along it, `v` mm above the terrain; `length` is the facade's length.
    pub fn to_drawing(&self, u: f64, v: f64, length: f64) -> [f64; 2] {
        let x = self.x_start + (self.x_end - self.x_start) * u / length;
        let [[ya, va], [yb, vb]] = self.y_levels;
        [x, ya + (v - va) * (yb - ya) / (vb - va)]
    }

    pub fn validate(&self) -> Result<()> {
        let all = [self.x_start, self.x_end, self.y_levels[0][0], self.y_levels[0][1], self.y_levels[1][0], self.y_levels[1][1]];
        if all.iter().any(|v| !v.is_finite() || v.abs() > 1e7) {
            return Err(ModelError::Invalid("registration values must be finite numbers".into()));
        }
        if (self.x_end - self.x_start).abs() < 10.0 {
            return Err(ModelError::Invalid("x_start and x_end must be at least 10 px apart".into()));
        }
        if (self.y_levels[0][0] - self.y_levels[1][0]).abs() < 10.0 || (self.y_levels[0][1] - self.y_levels[1][1]).abs() < 100.0 {
            return Err(ModelError::Invalid("the two y_levels must be at least 10 px and 100 mm apart".into()));
        }
        Ok(())
    }
}

/// A linked IFC file. Only the link is stored; the file is read again when needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub name: String,
    pub path: String,
    /// `map`: placed through the file's IfcMapConversion relative to the site;
    /// `local`: the file's local origin is put on the site origin.
    pub alignment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub command: String,
    pub params: serde_json::Value,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format: FORMAT.to_string(),
            site: None,
            buildings: Vec::new(),
            types: default_types(),
            journal: Vec::new(),
            references: Vec::new(),
            drawings: Vec::new(),
            phases: default_phases(),
            active_phase: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Site {
    pub name: String,
    /// The point the address resolved to (WGS84).
    pub origin: GeoPoint,
    /// The same point in ETRS89 / UTM 32N (EPSG:25832), metres: easting, northing.
    pub origin_utm: [f64; 2],
    /// Terrain height at the origin, metres (DVR90). Local z = 0 is this height.
    #[serde(default)]
    pub elevation: f64,
    pub evidence: Vec<Evidence>,
}

/// Where a piece of the model came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    /// Source id, for example `osm:way/174985023` or `nominatim:node/340506214`.
    pub source: String,
    /// What the source supplied, for example `footprint` or `address`.
    pub supplies: String,
    /// Licence or attribution the source requires.
    pub attribution: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Storey {
    pub name: String,
    /// Floor level above the site origin, mm.
    pub elevation: f64,
    /// Floor-to-floor height, mm.
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FacadeKind {
    /// Not classified yet.
    Unknown,
    Street,
    Courtyard,
    /// Shared with a neighbour: no openings.
    Party,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Facade {
    /// `<building>.f<n>`, numbered from 1 along the footprint.
    pub id: String,
    /// Index of the footprint vertex the facade starts at; it runs to the next vertex.
    pub edge: usize,
    pub kind: FacadeKind,
    /// Bay centre lines, mm along the facade from its start.
    pub bays: Vec<f64>,
    pub placements: Vec<Placement>,
    /// This wall's thickness per storey where it differs from the building's schedule (a
    /// fire gable, for example, keeps one thickness all the way up), mm.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_schedule: Vec<f64>,
    /// The wall's outer finish where it isn't the building's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<WallFinish>,
    /// Horizontal rendered bands standing proud of the wall (slab edges, spandrels).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bands: Vec<Band>,
}

/// An outdoor stair down to the basement (kældernedgang): a pit along a facade, `from`–`to` mm
/// along it and `width` mm out from it, between concrete retaining walls with a railing on top;
/// a straight flight runs along the wall from `top` (at the terrain) down to `bottom` (at the
/// pit floor, `floor` mm relative to the terrain); a door `door_width` wide at `door_at` opens
/// into the basement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CellarStair {
    #[serde(default)]
    pub id: String,
    pub facade: String,
    pub from: f64,
    pub to: f64,
    pub width: f64,
    pub top: f64,
    pub bottom: f64,
    pub floor: f64,
    pub door_at: f64,
    #[serde(default = "cellar_door")]
    pub door_width: f64,
}

fn cellar_door() -> f64 {
    900.0
}

impl CellarStair {
    /// Checks the numbers against the facade's length.
    pub fn validate(&self, length: f64) -> Result<()> {
        for (what, v) in [("from", self.from), ("to", self.to), ("top", self.top), ("bottom", self.bottom), ("door_at", self.door_at)] {
            checked(what, v, -1.0, length + 1.0)?;
        }
        checked("pit length", self.to - self.from, 1500.0, 20_000.0)?;
        checked("width", self.width, 700.0, 4000.0)?;
        checked("floor", self.floor, -6000.0, -600.0)?;
        checked("door_width", self.door_width, 600.0, 2000.0)?;
        let inside = |u: f64| u >= self.from - 1.0 && u <= self.to + 1.0;
        if !inside(self.top) || !inside(self.bottom) || (self.top - self.bottom).abs() < 600.0 {
            return Err(ModelError::Invalid("the flight's top and bottom must lie in the pit, at least 600 mm apart".into()));
        }
        if self.door_at - self.door_width / 2.0 < self.from || self.door_at + self.door_width / 2.0 > self.to {
            return Err(ModelError::Invalid("the door must open onto the pit".into()));
        }
        Ok(())
    }
}

/// A facade's outer finish.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WallFinish {
    /// Facing brick in the building's brick (yellow on most Copenhagen blocks of the 1930s).
    #[default]
    Brick,
    /// Red facing brick.
    RedBrick,
    /// White render.
    Render,
}

/// A horizontal band on the listed storeys, from `bottom` to `top` mm relative to each storey's
/// floor (negative reaches down past the floor), standing `projection` mm proud of the wall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Band {
    pub levels: Vec<usize>,
    pub bottom: f64,
    pub top: f64,
    #[serde(default = "band_projection")]
    pub projection: f64,
}

fn band_projection() -> f64 {
    40.0
}

/// A component type placed in one slot (storey × bay) of a facade grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub level: usize,
    pub bay: usize,
    #[serde(rename = "type")]
    pub type_id: String,
    /// Overrides of the type's width, height and sill for this slot, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Sill override, mm above this storey's floor. May be negative (an opening below the floor
    /// level, down to the terrain), for example a door down to a lower courtyard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sill: Option<f64>,
    /// The opening may run past the floor above (stairwell and landing windows). It must still
    /// stay below the eaves.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub spans_storeys: bool,
    /// Phase id the element is created in; the first phase when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// Phase id the element is demolished in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demolished: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub id: String,
    pub name: String,
    /// Outer wall line, counter-clockwise, local mm. No repeated closing vertex.
    pub footprint: Vec<[f64; 2]>,
    /// Facade wall thickness where `wall_schedule` gives none, mm.
    pub wall_thickness: f64,
    /// Facade wall thickness per storey, mm (index = storey). Copenhagen masonry walls thin
    /// going up, stepping on the inside at each floor while the outer face stays flush.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_schedule: Vec<f64>,
    /// Floor level of the ground storey above the terrain, mm (a raised ground floor over a
    /// half-sunken basement is common in Copenhagen). The wall runs from the terrain up.
    #[serde(default)]
    pub ground_floor: f64,
    pub storeys: Vec<Storey>,
    pub facades: Vec<Facade>,
    pub evidence: Vec<Evidence>,
    /// Facts from registers (BBR: year built, use, wall and roof material, areas).
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub roof: crate::roof::Roof,
    /// Walls inside the outer walls, per storey.
    #[serde(default)]
    pub interior_walls: Vec<InteriorWall>,
    #[serde(default)]
    pub stairs: Vec<Stair>,
    #[serde(default)]
    pub rooms: Vec<Room>,
    /// Entrances (staircases) with their address and units, from the registers.
    #[serde(default)]
    pub entrances: Vec<Entrance>,
    /// Index of the ground storey ("stueetage"); storeys below it are basements.
    #[serde(default)]
    pub ground_level: usize,
    /// A plinth (sokkel) along the foot of every facade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plinth: Option<Plinth>,
    /// Outdoor stairs down to the basement (kældernedgange) along facades.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cellar_stairs: Vec<CellarStair>,
    /// The facades' outer finish unless a facade has its own.
    #[serde(default, skip_serializing_if = "is_brick")]
    pub finish: WallFinish,
    /// Below the ground storey, the walls run straight along the inner corners of facade steps
    /// up to this deep (bays and stepped fronts that start at the ground floor), mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basement_straighten_mm: Option<f64>,
}

/// A rendered or concrete plinth (sokkel) at the foot of the facades.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plinth {
    /// Its top, mm above the terrain.
    pub height: f64,
    /// How far it stands proud of the brickwork, mm.
    pub projection: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Entrance {
    pub street: String,
    pub number: String,
    /// Access point, site mm.
    pub point: [f64; 2],
    /// The staircase serving it.
    #[serde(default)]
    pub stair: Option<String>,
    pub units: Vec<Unit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Unit {
    /// "st", "1", "2", … or "kl".
    pub floor: String,
    /// "tv", "th", "mf", a number or empty.
    pub door: String,
    #[serde(default)]
    pub area_m2: Option<f64>,
    #[serde(default)]
    pub rooms: Option<i64>,
}

impl Unit {
    /// The storey index of the unit's floor, given the ground storey's index: "st" is the ground
    /// storey, "1" the one above, "kl" the one below.
    pub fn level(&self, ground: usize) -> Option<usize> {
        match self.floor.as_str() {
            "st" => Some(ground),
            "kl" => ground.checked_sub(1),
            f => f.parse::<usize>().ok().map(|n| ground + n),
        }
    }
}

/// A straight interior wall, centre line `a`–`b` in site mm, on the listed storeys (full storey height).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InteriorWall {
    #[serde(default)]
    pub id: String,
    pub levels: Vec<usize>,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub thickness: f64,
    #[serde(default)]
    pub load_bearing: bool,
    /// Doors in the wall, on every storey it stands on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub doors: Vec<WallDoor>,
}

/// A door in an interior wall.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WallDoor {
    /// The opening's centre, mm along the wall from its start `a`.
    pub at: f64,
    /// Clear width of the opening, mm.
    pub width: f64,
    /// Height of the opening above the floor, mm (default 2100).
    #[serde(default = "door_height")]
    pub height: f64,
}

fn is_brick(f: &WallFinish) -> bool {
    *f == WallFinish::Brick
}

pub fn door_height() -> f64 {
    2100.0
}

/// A staircase: its outline in plan (site mm) and the direction it climbs (unit vector), on the listed storeys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stair {
    #[serde(default)]
    pub id: String,
    pub levels: Vec<usize>,
    /// The flights' rectangle `[p0, p1, p2, p3]`: p0→p1 runs along the flights from the floor end to the
    /// landing end, p1→p2 across them.
    pub outline: Vec<[f64; 2]>,
    /// Tread depth, mm (default: the run divided evenly).
    #[serde(default)]
    pub tread: Option<f64>,
    #[serde(default)]
    pub kind: StairKind,
    /// Depth of the half landing beyond the p1–p2 edge, mm (U-turn stairs; default half the width).
    #[serde(default)]
    pub landing: Option<f64>,
    /// Gap between the two flights, mm (U-turn stairs; default 150).
    #[serde(default)]
    pub well: Option<f64>,
    /// Half landing drawn and built as a half circle (default true).
    #[serde(default = "yes")]
    pub rounded: bool,
    /// Turning steps instead of a flat landing (2–16), split between the turns for a double quarter turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winders: Option<usize>,
    /// Depths of the two quarter turns beyond the straight flight, mm; defaults to flight width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_depths: Option<[f64; 2]>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StairKind {
    /// One flight along p0→p1.
    #[default]
    Straight,
    /// Two flights side by side with a half landing at the p1–p2 end: up along p0→p1, back along p2→p3.
    UTurn,
    /// One straight flight with a quarter turn at each end, both turning towards p3.
    DoubleQuarterTurn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Room {
    #[serde(default)]
    pub id: String,
    pub levels: Vec<usize>,
    pub name: String,
    pub polygon: Vec<[f64; 2]>,
}

impl InteriorWall {
    pub fn validate(&self, storeys: usize) -> Result<()> {
        let pts = [self.a[0], self.a[1], self.b[0], self.b[1]];
        if pts.iter().any(|v| !v.is_finite() || v.abs() > 1e8) {
            return Err(ModelError::Invalid("wall ends must be finite site coordinates".into()));
        }
        let len = ((self.b[0] - self.a[0]).powi(2) + (self.b[1] - self.a[1]).powi(2)).sqrt();
        checked("wall length", len, 100.0, 200_000.0)?;
        checked("wall thickness", self.thickness, 40.0, 1000.0)?;
        if self.doors.len() > 50 {
            return Err(ModelError::Invalid("at most 50 doors in one wall".into()));
        }
        let mut spans: Vec<(f64, f64)> = Vec::with_capacity(self.doors.len());
        for d in &self.doors {
            checked("door width", d.width, 300.0, 4000.0)?;
            checked("door height", d.height, 1000.0, 4000.0)?;
            let (lo, hi) = (d.at - d.width / 2.0, d.at + d.width / 2.0);
            if !d.at.is_finite() || lo < -1.0 || hi > len + 1.0 {
                return Err(ModelError::Invalid(format!("a {:.0} mm door at {:.0} mm doesn't fit the {len:.0} mm wall", d.width, d.at)));
            }
            if spans.iter().any(|&(a, b)| lo < b && a < hi) {
                return Err(ModelError::Invalid("doors in a wall may not overlap".into()));
            }
            spans.push((lo, hi));
        }
        if self.levels.is_empty() || self.levels.iter().any(|l| *l >= storeys) {
            return Err(ModelError::Invalid(format!("wall levels must be storey indices below {storeys}")));
        }
        Ok(())
    }
}

/// Area of a simple polygon, mm².
pub fn polygon_area(p: &[[f64; 2]]) -> f64 {
    crate::footprint::signed_area2(p).abs() / 2.0
}

pub const MIN_WALL: f64 = 100.0;
pub const MAX_WALL: f64 = 1500.0;
/// Half a Danish brick with its joint (½ sten), mm.
pub const HALF_BRICK: f64 = 120.0;

/// Copenhagen solid masonry facade walls, one thickness per storey (bottom up): 1½ sten (360 mm)
/// on the top two storeys, 2 sten (480) on the two below, 2½ sten (600) below those, and each
/// basement storey half a brick more than the storey above it (3 sten, 700 mm, under a 600 mm
/// ground storey).
pub fn copenhagen_masonry(storeys: usize, ground_level: usize) -> Vec<f64> {
    const LADDER: [f64; 4] = [360.0, 480.0, 600.0, 700.0];
    let step = |k: usize| LADDER.get(k).copied().unwrap_or(700.0 + (k as f64 - 3.0) * HALF_BRICK).min(MAX_WALL);
    let ground = ground_level.min(storeys.saturating_sub(1));
    let above = |level: usize| (storeys - 1 - level) / 2;
    (0..storeys).map(|level| if level >= ground { step(above(level)) } else { step(above(ground) + (ground - level)) }).collect()
}

impl Building {
    /// A building on `footprint` (any orientation; it is cleaned and made counter-clockwise),
    /// with `storeys` storeys of `storey_height` and one facade per footprint edge.
    pub fn new(id: &str, name: &str, footprint: &[[f64; 2]], storeys: usize, storey_height: f64) -> Result<Self> {
        let ring = footprint::clean(footprint)?;
        let mut b = Self {
            id: id.to_string(),
            name: name.to_string(),
            footprint: ring,
            wall_thickness: 400.0,
            wall_schedule: Vec::new(),
            ground_floor: 0.0,
            storeys: Vec::new(),
            facades: Vec::new(),
            evidence: Vec::new(),
            attributes: Default::default(),
            roof: Default::default(),
            interior_walls: Vec::new(),
            stairs: Vec::new(),
            rooms: Vec::new(),
            entrances: Vec::new(),
            ground_level: 0,
            plinth: None,
            cellar_stairs: Vec::new(),
            finish: WallFinish::Brick,
            basement_straighten_mm: None,
        };
        b.set_storeys(storeys, storey_height)?;
        b.facades = (0..b.footprint.len())
            .map(|i| Facade {
                id: format!("{}.f{}", id, i + 1),
                edge: i,
                kind: FacadeKind::Unknown,
                bays: Vec::new(),
                placements: Vec::new(),
                wall_schedule: Vec::new(),
                finish: None,
                bands: Vec::new(),
            })
            .collect();
        Ok(b)
    }

    pub fn set_storeys(&mut self, count: usize, height: f64) -> Result<()> {
        self.set_storeys_from(count, height, self.ground_floor)
    }

    /// Storeys of equal height with the ground storey's floor `ground_floor` mm above the terrain.
    pub fn set_storeys_from(&mut self, count: usize, height: f64, ground_floor: f64) -> Result<()> {
        if count == 0 || count > 60 {
            return Err(ModelError::Invalid(format!("storey count must be 1 to 60 (got {count})")));
        }
        let height = checked("storey height", height, 2000.0, 10000.0)?;
        let ground_floor = checked("ground floor level", ground_floor, 0.0, 5000.0)?;
        self.ground_floor = ground_floor;
        self.storeys = (0..count).map(|i| Storey { name: storey_name(i), elevation: ground_floor + i as f64 * height, height }).collect();
        self.fit_schedule();
        // Placements on storeys that no longer exist go away.
        for f in &mut self.facades {
            f.placements.retain(|p| p.level < count);
        }
        Ok(())
    }

    /// Storeys from explicit floor levels (mm above the terrain, rising), the top one ending at
    /// `top` (the eaves or attic floor). For buildings whose storeys differ in height, as read
    /// from a section drawing.
    pub fn set_levels(&mut self, floors: &[f64], top: f64) -> Result<()> {
        if floors.is_empty() || floors.len() > 60 {
            return Err(ModelError::Invalid(format!("give 1 to 60 floor levels (got {})", floors.len())));
        }
        let mut levels: Vec<f64> = floors.to_vec();
        levels.push(top);
        for (i, w) in levels.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            checked("floor level", a, 0.0, 300_000.0)?;
            let h = b - a;
            if !(2000.0..=10000.0).contains(&h) {
                return Err(ModelError::Invalid(format!("storey {i} would be {h} mm high (from {a} to {b}); allowed 2000 to 10000")));
            }
        }
        let count = floors.len();
        self.ground_floor = levels[0];
        self.storeys = levels.windows(2).enumerate().map(|(i, w)| Storey { name: storey_name(i), elevation: w[0], height: w[1] - w[0] }).collect();
        self.fit_schedule();
        for f in &mut self.facades {
            f.placements.retain(|p| p.level < count);
        }
        Ok(())
    }

    /// The facade wall's thickness on a storey by the building's schedule, mm.
    pub fn facade_thickness(&self, level: usize) -> f64 {
        self.wall_schedule.get(level).copied().unwrap_or(self.wall_thickness)
    }

    /// The thickness of the wall on footprint edge `edge` on a storey: its facade's own schedule
    /// if it has one, else the building's, mm.
    pub fn edge_thickness(&self, edge: usize, level: usize) -> f64 {
        let own = self.facades.iter().find(|f| f.edge == edge).and_then(|f| f.wall_schedule.get(level).copied());
        own.unwrap_or_else(|| self.facade_thickness(level))
    }

    /// The thickest facade wall on a storey, mm.
    pub fn thickest_wall(&self, level: usize) -> f64 {
        (0..self.footprint.len()).map(|e| self.edge_thickness(e, level)).fold(self.facade_thickness(level), f64::max)
    }

    /// The inner face of the facade walls on a storey: one vertex per footprint corner, each wall
    /// at its own thickness.
    pub fn inner_line(&self, level: usize) -> Vec<[f64; 2]> {
        crate::build::inner_ring_by(&self.footprint, |e| self.edge_thickness(e, level))
    }

    /// The wall line on a storey and, for each of its edges, the footprint edge it starts at: the
    /// footprint, or below the ground storey with `basement_straighten_mm` the footprint with
    /// outward steps up to that deep cut off.
    pub fn storey_ring(&self, level: usize) -> (Vec<[f64; 2]>, Vec<usize>) {
        let n = self.footprint.len();
        // Only steps in street fronts: gables and the corners of wings keep their basement walls.
        let street = |edge: usize| self.facades.iter().any(|f| f.edge == edge && f.kind == FacadeKind::Street);
        let kept = match self.basement_straighten_mm {
            Some(d) if level < self.ground_level && n > 0 => footprint::inner_outline(&self.footprint, d, |k| street(k) || street((k + n - 1) % n)),
            _ => (0..n).collect(),
        };
        (kept.iter().filter_map(|&i| self.footprint.get(i).copied()).collect(), kept)
    }

    /// The inner face of the walls on a storey, one vertex per corner of `storey_ring(level)`.
    pub fn storey_inner(&self, level: usize) -> Vec<[f64; 2]> {
        let (ring, kept) = self.storey_ring(level);
        crate::build::inner_ring_by(&ring, |i| kept.get(i).map_or(self.wall_thickness, |&e| self.edge_thickness(e, level)))
    }

    /// Whether footprint edge `edge` has its own wall on a storey (not where a straight basement
    /// wall cuts a step off).
    pub fn edge_on_storey(&self, edge: usize, level: usize) -> bool {
        let n = self.footprint.len();
        let (_, kept) = self.storey_ring(level);
        kept.iter().enumerate().any(|(i, &k)| k == edge && kept.get((i + 1) % kept.len()) == Some(&((edge + 1) % n)))
    }

    /// Gives facades their own thickness per storey (one value per storey), or with an empty
    /// list makes them follow the building's schedule again.
    pub fn set_facade_schedule(&mut self, facades: &[String], thickness: &[f64]) -> Result<()> {
        if !thickness.is_empty() && thickness.len() != self.storeys.len() {
            return Err(ModelError::Invalid(format!("give one thickness per storey: {} storeys, {} values", self.storeys.len(), thickness.len())));
        }
        for t in thickness {
            checked("wall thickness", *t, MIN_WALL, MAX_WALL)?;
        }
        for id in facades {
            self.facade_mut(id)?.wall_schedule = thickness.to_vec();
        }
        Ok(())
    }

    /// Sets the facade wall thickness per storey (one value per storey, bottom up).
    pub fn set_wall_schedule(&mut self, thickness: &[f64]) -> Result<()> {
        if thickness.len() != self.storeys.len() {
            return Err(ModelError::Invalid(format!("give one thickness per storey: {} storeys, {} values", self.storeys.len(), thickness.len())));
        }
        for t in thickness {
            checked("wall thickness", *t, MIN_WALL, MAX_WALL)?;
        }
        self.wall_schedule = thickness.to_vec();
        Ok(())
    }

    /// A schedule that no longer matches the storeys keeps its values from the bottom and
    /// repeats its top value on added storeys.
    fn fit_schedule(&mut self) {
        let count = self.storeys.len();
        for schedule in std::iter::once(&mut self.wall_schedule).chain(self.facades.iter_mut().map(|f| &mut f.wall_schedule)) {
            if let Some(&top) = schedule.last() {
                schedule.resize(count, top);
            }
        }
    }

    /// Bottom of the outer walls, mm (below the terrain when there is a basement).
    pub fn bottom(&self) -> f64 {
        self.storeys.first().map_or(0.0, |s| (s.elevation - 200.0).min(0.0))
    }

    /// Adds a basement storey below the lowest one, with its floor at `floor` mm (below the terrain),
    /// and shifts every storey reference (placements, interior, plans) up by one.
    pub fn add_basement(&mut self, floor: f64, name: &str) -> Result<()> {
        let first = self.storeys.first().cloned().ok_or_else(|| ModelError::Invalid("the building has no storeys".into()))?;
        let floor = checked("basement floor", floor, -15_000.0, first.elevation - 2000.0)?;
        if self.storeys.len() >= 60 {
            return Err(ModelError::Invalid("at most 60 storeys".into()));
        }
        self.storeys.insert(0, Storey { name: name.to_string(), elevation: floor, height: first.elevation - floor });
        if let Some(&lowest) = self.wall_schedule.first() {
            self.wall_schedule.insert(0, (lowest + HALF_BRICK).min(MAX_WALL));
        }
        for f in &mut self.facades {
            if let Some(&lowest) = f.wall_schedule.first() {
                f.wall_schedule.insert(0, lowest);
            }
        }
        self.ground_level += 1;
        for f in &mut self.facades {
            for p in &mut f.placements {
                p.level += 1;
            }
        }
        for w in &mut self.interior_walls {
            w.levels.iter_mut().for_each(|l| *l += 1);
        }
        for s in &mut self.stairs {
            s.levels.iter_mut().for_each(|l| *l += 1);
        }
        for r in &mut self.rooms {
            r.levels.iter_mut().for_each(|l| *l += 1);
        }
        Ok(())
    }

    /// Wall height from the terrain to the eaves, mm.
    pub fn height(&self) -> f64 {
        self.storeys.last().map_or(self.ground_floor, |s| s.elevation + s.height)
    }

    pub fn facade(&self, id: &str) -> Result<&Facade> {
        self.facades.iter().find(|f| f.id == id).ok_or_else(|| ModelError::NotFound(format!("facade {id}")))
    }

    pub fn facade_mut(&mut self, id: &str) -> Result<&mut Facade> {
        self.facades.iter_mut().find(|f| f.id == id).ok_or_else(|| ModelError::NotFound(format!("facade {id}")))
    }

    /// Start and end of a facade's outer wall line.
    pub fn edge(&self, edge: usize) -> Option<([f64; 2], [f64; 2])> {
        let n = self.footprint.len();
        let a = *self.footprint.get(edge)?;
        let b = *self.footprint.get((edge + 1) % n)?;
        Some((a, b))
    }
}

/// Danish storey names: stueetage (ground floor), then 1. sal, 2. sal…
pub fn storey_name(i: usize) -> String {
    if i == 0 { "Stueetage".to_string() } else { format!("{i}. sal") }
}

impl Document {
    pub fn building(&self, id: Option<&str>) -> Result<&Building> {
        match id {
            Some(id) => self.buildings.iter().find(|b| b.id == id).ok_or_else(|| ModelError::NotFound(format!("building {id}"))),
            None => self.buildings.first().ok_or_else(|| ModelError::NotFound("no building in the document yet".to_string())),
        }
    }

    pub fn building_mut(&mut self, id: Option<&str>) -> Result<&mut Building> {
        match id {
            Some(id) => self.buildings.iter_mut().find(|b| b.id == id).ok_or_else(|| ModelError::NotFound(format!("building {id}"))),
            None => self.buildings.first_mut().ok_or_else(|| ModelError::NotFound("no building in the document yet".to_string())),
        }
    }

    /// The building that owns a facade id (`b1.f3` belongs to `b1`).
    pub fn building_of_facade_mut(&mut self, facade: &str) -> Result<&mut Building> {
        self.buildings.iter_mut().find(|b| b.facades.iter().any(|f| f.id == facade)).ok_or_else(|| ModelError::NotFound(format!("facade {facade}")))
    }

    pub fn phase_index(&self, id: &str) -> Result<usize> {
        self.phases.iter().position(|p| p.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.phases.iter().map(|p| p.id.as_str()).collect();
            ModelError::NotFound(format!("phase {id} (phases: {})", known.join(", ")))
        })
    }

    /// The phase new work goes into (the last phase unless one is activated).
    pub fn active_index(&self) -> usize {
        self.active_phase.as_deref().and_then(|a| self.phases.iter().position(|p| p.id == a)).unwrap_or(self.phases.len().saturating_sub(1))
    }

    /// The phases a placement exists in: from its creation up to (not including) its demolition.
    pub fn lifetime(&self, p: &Placement) -> Result<(usize, usize)> {
        let c = match &p.created {
            Some(id) => self.phase_index(id)?,
            None => 0,
        };
        let d = match &p.demolished {
            Some(id) => self.phase_index(id)?,
            None => self.phases.len(),
        };
        if d <= c {
            return Err(ModelError::Invalid(format!(
                "level {} bay {}: demolished in a phase before or equal to the one it's created in",
                p.level, p.bay
            )));
        }
        Ok((c, d))
    }

    pub fn component(&self, id: &str) -> Result<&ComponentType> {
        self.types.iter().find(|t| t.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.types.iter().map(|t| t.id.as_str()).collect();
            ModelError::NotFound(format!("component type {id} (known: {})", known.join(", ")))
        })
    }
}

#[cfg(test)]
mod plan_tests {
    use super::PlanRegistration;

    #[test]
    fn similarity_round_trips_and_fits_exactly() {
        // A sheet at 100 px/m, rotated 30°, shifted.
        let (s, t) = (10.0, 30f64.to_radians());
        let truth = |p: [f64; 2]| {
            let (x, y) = (p[0], -p[1]);
            [s * (t.cos() * x - t.sin() * y) + 5000.0, s * (t.sin() * x + t.cos() * y) - 2000.0]
        };
        let pts = [[100.0, 200.0], [900.0, 250.0], [400.0, 800.0]];
        let pairs: Vec<_> = pts.iter().map(|p| (*p, truth(*p))).collect();
        let r = PlanRegistration::fit(vec![1], &pairs).unwrap();
        assert!((r.px_per_m() - 100.0).abs() < 1e-9);
        for p in pts {
            let q = r.to_site(p);
            assert!((q[0] - truth(p)[0]).abs() < 1e-6 && (q[1] - truth(p)[1]).abs() < 1e-6);
            let back = r.to_drawing(q);
            assert!((back[0] - p[0]).abs() < 1e-9 && (back[1] - p[1]).abs() < 1e-9);
        }
        assert!(PlanRegistration::fit(vec![1], &pairs[..1]).is_err());
    }
}
