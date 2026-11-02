//! Trigon's building model.
//!
//! Units are millimetres, Z is up. Plan coordinates are in the site's local frame: x east and
//! y north of [`Site::origin_utm`], so the numbers stay small enough for 1e-6 mm resolution
//! (map coordinates in EPSG:25832 are about 6.1e9 mm, where one f64 step is already 1.4e-6 mm).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod annotation;
pub mod balcony;
pub mod build;
pub mod component;
pub mod doc;
pub mod doors;
pub mod footprint;
pub mod joins;
pub mod mesh;
pub mod profile;
pub mod roof;
pub mod site;
pub mod stair;
pub mod terrain;
pub mod wall_cleanup;
pub mod walls;

pub use build::{Built, BuiltElement, BuiltOpening, BuiltSlab, BuiltWall, ElementClass, FacadeFrame, Rect, SlabKind, build};
pub use component::{ComponentKind, ComponentType, default_types};
pub use doc::{
    Building, CellarStair, Document, Drawing, Entrance, Evidence, FORMAT, Facade, FacadeKind, FacadeRegistration, GeoPoint, InteriorWall, Phase,
    Placement, PlanRegistration, Reference, Room, Site, Stair, StairKind, Storey, Unit, WallDoor, WallFinish,
};
pub use mesh::{Material, Mesh};
pub use roof::Roof;

/// Model errors. Every message names the thing that is wrong and what is allowed.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ModelError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ModelError>;

/// A finite number within `lo..=hi`, or an `Invalid` error naming `what`.
pub fn checked(what: &str, value: f64, lo: f64, hi: f64) -> Result<f64> {
    if !value.is_finite() || value < lo || value > hi {
        return Err(ModelError::Invalid(format!("{what} must be between {lo} and {hi} (got {value})")));
    }
    Ok(value)
}
