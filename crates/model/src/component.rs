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
    /// Casement window divided into `lights` side-hung sashes (1 = single sash), with a row of
    /// top lights (overvinduer) `top_light` mm high across the top when it isn't 0.
    Window {
        lights: u32,
        #[serde(default, skip_serializing_if = "is_zero")]
        top_light: f64,
    },
    /// Fixed glazing, no sash.
    FixedWindow,
    /// Original, parametric traditional joinery. The opening follows the declared profile;
    /// muntins are either real glazing divisions or explicitly applied presentation bars.
    HistoricWindow {
        #[serde(default)]
        shape: WindowShape,
        #[serde(default = "one")]
        lights: u32,
        #[serde(default)]
        top_light: f64,
        #[serde(default = "one")]
        panes_across: u32,
        #[serde(default = "one")]
        panes_down: u32,
        #[serde(default)]
        muntins: Muntins,
        #[serde(default = "muntin_width")]
        muntin_width_mm: f64,
        #[serde(default = "yes")]
        operable: bool,
    },
    /// Fyldningsdør: a parameterised grid of recessed panels; the upper rows may be glazed.
    PanelDoor {
        leaves: u32,
        panel_columns: u32,
        panel_rows: u32,
        #[serde(default)]
        glazed_rows: u32,
    },
    /// Solid entrance door.
    EntranceDoor,
    /// Glazed door with `leaves` leaves opening onto nothing (ground floor, or into a balcony).
    FrenchDoor { leaves: u32 },
    /// French (Juliet) balcony: a French door with a guard rail in front of it, no slab.
    FrenchBalcony { leaves: u32, rail_height: f64 },
    /// Cantilever balcony: a French door, a slab of `depth` × `slab_width` and a guard rail.
    Balcony { leaves: u32, depth: f64, slab_width: f64, rail_height: f64 },
    /// A door and a window in one opening (a balcony door unit): the door leaf `door_width` wide on
    /// `door_side` (left/right seen from outside), the window over a panel `window_sill` above the
    /// opening's bottom, `lights` sashes.
    DoorWindow { door_width: f64, door_side: Side, window_sill: f64, lights: u32 },
    /// A balcony on its own, hung on the wall in front of an opening: a slab `depth` deep and the
    /// type's `width` wide, with a plain guard rail. Doesn't cut the wall. For existing balconies.
    BalconySlab { depth: f64, thickness: f64 },
    /// Convex bowed, chamfered or custom-plan slab and guard. Does not cut the host wall.
    ShapedBalcony { depth: f64, thickness: f64, plan: crate::balcony::Plan },
    /// A loggia: the facade is opened from the sill up and the glazing (a door and a window) is set
    /// back `depth` mm from the outer face, between brick returns, under a soffit, with a guard
    /// rail across the opening. The parapet below the sill is the facade wall.
    Loggia { depth: f64, rail_height: f64, door_width: f64, door_side: Side },
    /// A MinAltan steel balcony (stålaltan): hot-dip galvanised frame on a 160 mm UPE profile, a
    /// sunken deck, an Ibond underside and a railing style named after the districts. The type's
    /// `width` is the balcony width and its `height` the railing height. Doesn't cut the wall.
    SteelBalcony {
        depth: f64,
        railing: Railing,
        finish: Finish,
        deck: Deck,
        /// Hardwood handrail on top of the railing.
        #[serde(default)]
        handrail: bool,
        /// Matt-glass privacy screens, 1800 mm high, the full depth.
        #[serde(default)]
        screens: Screens,
        /// Tie rods from the front corners up to the wall; otherwise wall anchoring.
        #[serde(default)]
        tie_rods: bool,
    },
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}
fn muntin_width() -> f64 {
    22.0
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowShape {
    #[default]
    Rectangular,
    /// Circular segment over vertical jambs. Rise is measured from spring line to crown.
    Segmental { rise_mm: f64 },
    /// A circular fixed light; nominal width and height must be equal.
    Round,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Muntins {
    #[default]
    TrueDivided,
    Applied,
}

/// Left or right, seen from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

/// MinAltan railing styles (balusters, rails).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Railing {
    /// Ø12 mm balusters, two 50 × 10 mm rails.
    Frederiksberg,
    /// 30 × 6 mm balusters, one 60 × 15 mm rail.
    Norrebro,
    /// 30 × 6 mm balusters, two 50 × 10 mm rails.
    Vesterbro,
    /// 30 × 6 mm balusters only.
    Osterbro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    Galvanised,
    White,
    Anthracite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Deck {
    Cumaru,
    FibreConcrete,
    Composite,
    Bamboo,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Screens {
    #[default]
    None,
    Left,
    Right,
    Both,
}

impl ComponentKind {
    pub fn is_door(&self) -> bool {
        matches!(
            self,
            Self::EntranceDoor
                | Self::PanelDoor { .. }
                | Self::FrenchDoor { .. }
                | Self::FrenchBalcony { .. }
                | Self::Balcony { .. }
                | Self::DoorWindow { .. }
                | Self::Loggia { .. }
        )
    }

    /// Cuts an opening in the wall (windows and doors). Attachments (balconies hung on the
    /// wall) don't, and a slot can hold one opening and one attachment.
    pub fn is_opening(&self) -> bool {
        !matches!(self, Self::BalconySlab { .. } | Self::SteelBalcony { .. } | Self::ShapedBalcony { .. })
    }

    /// Projection from the wall, mm, for attachments.
    pub fn depth(&self) -> Option<f64> {
        match self {
            Self::BalconySlab { depth, .. } | Self::SteelBalcony { depth, .. } | Self::Balcony { depth, .. } | Self::ShapedBalcony { depth, .. } => {
                Some(*depth)
            }
            _ => None,
        }
    }
}

impl ComponentType {
    /// Checks every number is in a buildable range.
    pub fn validate(&self) -> Result<()> {
        checked("width", self.width, 300.0, 6000.0)?;
        checked("height", self.height, 300.0, 4000.0)?;
        checked("sill", self.sill, 0.0, 3000.0)?;
        match &self.kind {
            ComponentKind::HistoricWindow { shape, lights, top_light, panes_across, panes_down, muntin_width_mm, operable, .. } => {
                if !(1..=4).contains(lights)
                    || !(1..=6).contains(panes_across)
                    || !(1..=8).contains(panes_down)
                    || u64::from(*lights) * u64::from(*panes_across) * u64::from(*panes_down) * if *top_light > 0.0 { 2 } else { 1 } > 96
                {
                    return Err(crate::ModelError::Invalid("historic window: 1–4 lights, 1–6 pane columns, 1–8 pane rows, at most 96 panes".into()));
                }
                checked("top_light", *top_light, 0.0, self.height / 2.0)?;
                checked("muntin_width_mm", *muntin_width_mm, 8.0, 60.0)?;
                match shape {
                    WindowShape::Rectangular => {}
                    WindowShape::Segmental { rise_mm } => {
                        checked("arch rise", *rise_mm, 20.0, (self.width / 2.0).min(self.height * 0.45))?;
                    }
                    WindowShape::Round => {
                        if (self.width - self.height).abs() > 1e-6 || *operable || *lights != 1 || *top_light != 0.0 {
                            return Err(crate::ModelError::Invalid(
                                "round windows require equal width/height, one fixed light and no top_light".into(),
                            ));
                        }
                    }
                }
                Ok(())
            }
            ComponentKind::PanelDoor { leaves, panel_columns, panel_rows, glazed_rows } => {
                if !(1..=2).contains(leaves) || !(1..=3).contains(panel_columns) || !(1..=6).contains(panel_rows) || *glazed_rows > *panel_rows {
                    return Err(crate::ModelError::Invalid(
                        "panel door: 1–2 leaves, 1–3 panel columns, 1–6 panel rows; glazed_rows cannot exceed panel_rows".into(),
                    ));
                }
                Ok(())
            }
            ComponentKind::Window { lights, .. } if *lights == 0 || *lights > 8 => {
                Err(crate::ModelError::Invalid(format!("lights must be 1 to 8 (got {lights})")))
            }
            ComponentKind::Window { top_light, .. } => checked("top_light", *top_light, 0.0, self.height / 2.0).map(|_| ()),
            ComponentKind::Loggia { depth, rail_height, door_width, .. } => {
                checked("depth", *depth, 300.0, 4000.0)?;
                checked("rail_height", *rail_height, 600.0, 1500.0)?;
                checked("door_width", *door_width, 500.0, (self.width - 300.0).max(500.0)).map(|_| ())
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
            ComponentKind::DoorWindow { door_width, window_sill, lights, .. } => {
                checked("door_width", *door_width, 500.0, self.width - 300.0)?;
                checked("window_sill", *window_sill, 0.0, self.height - 400.0)?;
                if *lights == 0 || *lights > 4 {
                    return Err(crate::ModelError::Invalid(format!("lights must be 1 to 4 (got {lights})")));
                }
                Ok(())
            }
            ComponentKind::BalconySlab { depth, thickness } => {
                checked("depth", *depth, 300.0, 3000.0)?;
                checked("thickness", *thickness, 60.0, 400.0)?;
                checked("rail height (the type's height)", self.height, 900.0, 1500.0).map(|_| ())
            }
            ComponentKind::ShapedBalcony { depth, thickness, plan } => {
                checked("shaped balcony width", self.width, 600.0, 6000.0)?;
                checked("shaped balcony depth", *depth, 400.0, 3000.0)?;
                checked("shaped balcony slab", *thickness, 80.0, 400.0)?;
                checked("shaped balcony guard", self.height, 900.0, 1500.0)?;
                plan.outline(self.width, *depth).map(|_| ())
            }
            ComponentKind::SteelBalcony { depth, .. } => {
                checked("depth", *depth, 300.0, 3000.0)?;
                checked("railing height (the type's height)", self.height, 900.0, 1500.0).map(|_| ())
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
        t("window.casement.1", "Casement window, 1 sash", 700.0, 1400.0, 850.0, ComponentKind::Window { lights: 1, top_light: 0.0 }),
        t("window.casement.2", "Casement window, 2 sashes", 1200.0, 1500.0, 850.0, ComponentKind::Window { lights: 2, top_light: 0.0 }),
        t("window.casement.3", "Casement window, 3 sashes", 1800.0, 1500.0, 850.0, ComponentKind::Window { lights: 3, top_light: 0.0 }),
        t("window.fixed", "Fixed window", 1200.0, 1500.0, 850.0, ComponentKind::FixedWindow),
        t("door.entrance", "Entrance door", 1200.0, 2300.0, 0.0, ComponentKind::EntranceDoor),
        t("door.french", "French door, 2 leaves", 1200.0, 2250.0, 0.0, ComponentKind::FrenchDoor { leaves: 2 }),
        t("balcony.french", "French balcony", 1200.0, 2250.0, 0.0, ComponentKind::FrenchBalcony { leaves: 2, rail_height: 1000.0 }),
        t(
            "balcony.minaltan",
            "MinAltan steel balcony, Vesterbro railing",
            2700.0,
            1000.0,
            0.0,
            ComponentKind::SteelBalcony {
                depth: 1300.0,
                railing: Railing::Vesterbro,
                finish: Finish::Galvanised,
                deck: Deck::Cumaru,
                handrail: false,
                screens: Screens::None,
                tie_rods: false,
            },
        ),
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

/// Original configurable style templates, installed explicitly so old projects and their
/// default libraries remain unchanged. Dimensions are family defaults, not surveyed data.
pub fn copenhagen_types() -> Vec<ComponentType> {
    let window = |id: &str, name: &str, width, height, shape, lights, top_light, panes_across, panes_down, muntins, operable| ComponentType {
        id: format!("dk.window.{id}"),
        name: name.into(),
        width,
        height,
        sill: 850.0,
        kind: ComponentKind::HistoricWindow { shape, lights, top_light, panes_across, panes_down, muntins, muntin_width_mm: 22.0, operable },
    };
    let mut types = vec![
        window("dannebrog", "Dannebrog – four casements", 1200.0, 1900.0, WindowShape::Rectangular, 2, 450.0, 1, 1, Muntins::TrueDivided, true),
        window("crosspost", "Krydspost – balanced cross", 1400.0, 1900.0, WindowShape::Rectangular, 2, 700.0, 1, 1, Muntins::TrueDivided, true),
        window(
            "three-light",
            "Three-light Copenhagen casement",
            1800.0,
            1900.0,
            WindowShape::Rectangular,
            3,
            450.0,
            1,
            1,
            Muntins::TrueDivided,
            true,
        ),
        window(
            "small-panes",
            "Småsprosset – true divided glazing",
            1100.0,
            1700.0,
            WindowShape::Rectangular,
            2,
            0.0,
            1,
            3,
            Muntins::TrueDivided,
            true,
        ),
        window(
            "applied-bars",
            "Pyntesprosser – explicitly applied bars",
            1100.0,
            1700.0,
            WindowShape::Rectangular,
            2,
            0.0,
            1,
            3,
            Muntins::Applied,
            true,
        ),
        window(
            "segmental",
            "Segmentbuet Dannebrog",
            1200.0,
            2000.0,
            WindowShape::Segmental { rise_mm: 180.0 },
            2,
            450.0,
            1,
            1,
            Muntins::TrueDivided,
            true,
        ),
        window(
            "arched-fixed",
            "Arched fixed light",
            1000.0,
            1800.0,
            WindowShape::Segmental { rise_mm: 450.0 },
            1,
            0.0,
            1,
            1,
            Muntins::TrueDivided,
            false,
        ),
        window("round", "Round oculus – fixed", 700.0, 700.0, WindowShape::Round, 1, 0.0, 2, 2, Muntins::TrueDivided, false),
    ];
    for (id, name, width, height, leaves, columns, rows, glazed_rows) in [
        ("six-panel", "Six-panel entrance", 1100.0, 2350.0, 1, 2, 3, 0),
        ("glazed-panel", "Glazed-panel entrance", 1100.0, 2350.0, 1, 2, 3, 1),
        ("double-panel", "Double Copenhagen entrance", 1600.0, 2500.0, 2, 1, 3, 1),
    ] {
        types.push(ComponentType {
            id: format!("dk.door.{id}"),
            name: name.into(),
            width,
            height,
            sill: 0.0,
            kind: ComponentKind::PanelDoor { leaves, panel_columns: columns, panel_rows: rows, glazed_rows },
        });
    }
    types
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

    #[test]
    fn historic_family_parameters_round_trip_without_rewriting_old_families() {
        for kind in [
            serde_json::json!({"family":"historic_window","shape":{"kind":"segmental","rise_mm":180},"lights":2,"top_light":450,"panes_across":1,"panes_down":2,"muntins":"true_divided","muntin_width_mm":22,"operable":true}),
            serde_json::json!({"family":"historic_window","shape":{"kind":"round"},"lights":1,"operable":false}),
            serde_json::json!({"family":"panel_door","leaves":2,"panel_columns":2,"panel_rows":3,"glazed_rows":1}),
        ] {
            let parsed: ComponentKind = serde_json::from_value(kind).unwrap();
            let round_trip: ComponentKind = serde_json::from_value(serde_json::to_value(&parsed).unwrap()).unwrap();
            assert_eq!(parsed, round_trip);
        }
        let old: ComponentKind = serde_json::from_str(r#"{"family":"window","lights":2}"#).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(), serde_json::json!({"family":"window","lights":2}));
    }

    #[test]
    fn copenhagen_templates_are_valid_and_have_distinct_ids() {
        let types = copenhagen_types();
        let ids: std::collections::BTreeSet<_> = types.iter().map(|t| &t.id).collect();
        assert_eq!(ids.len(), types.len());
        for t in types {
            t.validate().unwrap();
        }
    }
}
