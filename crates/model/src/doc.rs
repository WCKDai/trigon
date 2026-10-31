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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub command: String,
    pub params: serde_json::Value,
}

impl Default for Document {
    fn default() -> Self {
        Self { format: FORMAT.to_string(), site: None, buildings: Vec::new(), types: default_types(), journal: Vec::new() }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sill: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Building {
    pub id: String,
    pub name: String,
    /// Outer wall line, counter-clockwise, local mm. No repeated closing vertex.
    pub footprint: Vec<[f64; 2]>,
    pub wall_thickness: f64,
    pub storeys: Vec<Storey>,
    pub facades: Vec<Facade>,
    pub evidence: Vec<Evidence>,
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
            storeys: Vec::new(),
            facades: Vec::new(),
            evidence: Vec::new(),
        };
        b.set_storeys(storeys, storey_height)?;
        b.facades = (0..b.footprint.len())
            .map(|i| Facade { id: format!("{}.f{}", id, i + 1), edge: i, kind: FacadeKind::Unknown, bays: Vec::new(), placements: Vec::new() })
            .collect();
        Ok(b)
    }

    pub fn set_storeys(&mut self, count: usize, height: f64) -> Result<()> {
        if count == 0 || count > 60 {
            return Err(ModelError::Invalid(format!("storey count must be 1 to 60 (got {count})")));
        }
        let height = checked("storey height", height, 2000.0, 10000.0)?;
        self.storeys = (0..count).map(|i| Storey { name: storey_name(i), elevation: i as f64 * height, height }).collect();
        // Placements on storeys that no longer exist go away.
        for f in &mut self.facades {
            f.placements.retain(|p| p.level < count);
        }
        Ok(())
    }

    pub fn height(&self) -> f64 {
        self.storeys.iter().map(|s| s.height).sum()
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

    pub fn component(&self, id: &str) -> Result<&ComponentType> {
        self.types.iter().find(|t| t.id == id).ok_or_else(|| {
            let known: Vec<&str> = self.types.iter().map(|t| t.id.as_str()).collect();
            ModelError::NotFound(format!("component type {id} (known: {})", known.join(", ")))
        })
    }
}
