//! Parametric component families: windows, doors and balconies.
//!
//! A [`ComponentType`] is a family with its parameters set (a "type" in BIM terms). Placements
//! refer to a type by id and may override width, height and sill per slot. Geometry is always
//! generated from these numbers by [`crate::build`]; nothing here holds a mesh.

use serde::{Deserialize, Serialize};

use crate::{Result, checked};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComponentType {
    pub id: String,
    pub name: String,
    /// Opening width, mm.
    pub width: f64,
    /// Opening height, mm.
    pub height: f64,
    /// Bottom of the opening above the storey's floor, mm.
    pub sill: f64,
    pub kind: ComponentKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "family", rename_all = "snake_case")]
pub enum ComponentKind {
    /// Casement window divided into `lights` side-hung sashes (1 = single sash).
    Window { lights: u32 },
    /// Fixed glazing, no sash.
    FixedWindow,
    /// Solid entrance door.
    EntranceDoor,
    /// Glazed door with `leaves` leaves opening onto nothing (ground floor, or into a balcony).
    FrenchDoor { leaves: u32 },
    /// French (Juliet) balcony: a French door with a guard rail in front of it, no slab.
    FrenchBalcony { leaves: u32, rail_height: f64 },
    /// Cantilever balcony: a French door, a slab of `depth` × `slab_width` and a guard rail.
    Balcony { leaves: u32, depth: f64, slab_width: f64, rail_height: f64 },
}

impl ComponentKind {
    pub fn is_door(&self) -> bool {
        !matches!(self, Self::Window { .. } | Self::FixedWindow)
    }
}

impl ComponentType {
    /// Checks every number is in a buildable range.
    pub fn validate(&self) -> Result<()> {
        checked("width", self.width, 300.0, 6000.0)?;
        checked("height", self.height, 300.0, 4000.0)?;
        checked("sill", self.sill, 0.0, 3000.0)?;
        match &self.kind {
            ComponentKind::Window { lights } if *lights == 0 || *lights > 8 => {
                Err(crate::ModelError::Invalid(format!("lights must be 1 to 8 (got {lights})")))
            }
            ComponentKind::FrenchDoor { leaves } | ComponentKind::FrenchBalcony { leaves, .. } | ComponentKind::Balcony { leaves, .. }
                if *leaves == 0 || *leaves > 4 =>
            {
                Err(crate::ModelError::Invalid(format!("leaves must be 1 to 4 (got {leaves})")))
            }
            ComponentKind::FrenchBalcony { rail_height, .. } => checked("rail_height", *rail_height, 600.0, 1500.0).map(|_| ()),
            ComponentKind::Balcony { depth, slab_width, rail_height, .. } => {
                checked("depth", *depth, 300.0, 3000.0)?;
                checked("slab_width", *slab_width, self.width, 12000.0)?;
                checked("rail_height", *rail_height, 900.0, 1500.0).map(|_| ())
            }
            _ => Ok(()),
        }
    }
}

/// The starting library: common types on Copenhagen apartment blocks. Guard rails are 1.0 m.
pub fn default_types() -> Vec<ComponentType> {
    let t = |id: &str, name: &str, width: f64, height: f64, sill: f64, kind: ComponentKind| ComponentType {
        id: id.to_string(),
        name: name.to_string(),
        width,
        height,
        sill,
        kind,
    };
    vec![
        t("window.casement.1", "Casement window, 1 sash", 700.0, 1400.0, 850.0, ComponentKind::Window { lights: 1 }),
        t("window.casement.2", "Casement window, 2 sashes", 1200.0, 1500.0, 850.0, ComponentKind::Window { lights: 2 }),
        t("window.casement.3", "Casement window, 3 sashes", 1800.0, 1500.0, 850.0, ComponentKind::Window { lights: 3 }),
        t("window.fixed", "Fixed window", 1200.0, 1500.0, 850.0, ComponentKind::FixedWindow),
        t("door.entrance", "Entrance door", 1200.0, 2300.0, 0.0, ComponentKind::EntranceDoor),
        t("door.french", "French door, 2 leaves", 1200.0, 2250.0, 0.0, ComponentKind::FrenchDoor { leaves: 2 }),
        t("balcony.french", "French balcony", 1200.0, 2250.0, 0.0, ComponentKind::FrenchBalcony { leaves: 2, rail_height: 1000.0 }),
        t(
            "balcony.cantilever",
            "Cantilever balcony",
            1000.0,
            2250.0,
            0.0,
            ComponentKind::Balcony { leaves: 1, depth: 1200.0, slab_width: 3000.0, rail_height: 1000.0 },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_types_are_valid() {
        for t in default_types() {
            t.validate().unwrap();
        }
    }

    #[test]
    fn hostile_numbers_are_refused() {
        let mut t = default_types().remove(0);
        for w in [f64::NAN, f64::INFINITY, -1.0, 1e9] {
            t.width = w;
            assert!(t.validate().is_err(), "width {w}");
        }
    }
}
