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
    pub wall_thickness: f64,
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
}

pub const MIN_WALL: f64 = 100.0;
pub const MAX_WALL: f64 = 1500.0;

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
            ground_floor: 0.0,
            storeys: Vec::new(),
            facades: Vec::new(),
            evidence: Vec::new(),
            attributes: Default::default(),
        };
        b.set_storeys(storeys, storey_height)?;
        b.facades = (0..b.footprint.len())
            .map(|i| Facade { id: format!("{}.f{}", id, i + 1), edge: i, kind: FacadeKind::Unknown, bays: Vec::new(), placements: Vec::new() })
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
        for f in &mut self.facades {
            f.placements.retain(|p| p.level < count);
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
