//! Explicit inclined roof-window source. Dimensions are design inputs, not product certification.
use crate::{ModelError, Result, checked};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rooflight {
    pub id: String,
    pub facade: String,
    /// Centre along the original source facade, in mm.
    pub u_mm: f64,
    /// Horizontal inward run from the source wall to the lower frame edge, in mm.
    pub run_mm: f64,
    #[serde(default, skip_serializing_if = "super::is_zero")]
    #[schemars(range(min = -45.0, max = 45.0))]
    pub yaw_deg: f64,
    /// A rectangular Window or FixedWindow family. Its lights/transom controls are reused.
    pub window_type: String,
    /// Overall frame aperture dimensions in the inclined roof plane, in mm.
    pub width_mm: f64,
    pub height_mm: f64,
    /// Normal distance of the family's outer face from the finished roof skin, in mm.
    pub stand_off_mm: f64,
    /// Width of the surrounding roof-plane flashing, in mm.
    pub flashing_mm: f64,
}
impl Rooflight {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [("id", &self.id), ("facade", &self.facade), ("window type", &self.window_type)] {
            if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
                return Err(ModelError::Invalid(format!("rooflight {name} needs 1–128 bytes without control characters")));
            }
        }
        checked("rooflight centre", self.u_mm, 0., 1e8)?;
        checked("rooflight horizontal run", self.run_mm, 0., 1e8)?;
        checked("rooflight yaw", self.yaw_deg, -45., 45.)?;
        checked("rooflight width", self.width_mm, 300., 3000.)?;
        checked("rooflight slope-plane height", self.height_mm, 400., 2400.)?;
        checked("rooflight normal stand-off", self.stand_off_mm, 100., 300.)?;
        checked("rooflight flashing width", self.flashing_mm, 40., 300.)?;
        Ok(())
    }
}
