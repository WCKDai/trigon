//! Rooms as `IfcSpace`: the floor polygon extruded to the underside of the floor above,
//! aggregated to its storey, numbered per storey (`<storey>.<nn>`, Revit's room number) with the
//! room's name as the long name.

use trigon_model::Building;

use super::catalog::{self, Qty, Val};
use super::part21::{area, ccw, clean_ring, gid, string};
use super::{Floors, Ifc};

impl Ifc<'_> {
    pub(super) fn spaces(&mut self, bd: &Building, fl: &mut Floors) {
        for (level, s) in bd.storeys.iter().enumerate() {
            // Knee height is not room height. Do not fabricate unclipped roof spaces/areas,
            // even if this writer is called with an externally assembled built model.
            if s.roof_level {
                continue;
            }
            let mut n = 0;
            for (k, r) in bd.rooms.iter().enumerate().filter(|(_, r)| r.levels.contains(&level)) {
                let ring = ccw(clean_ring(&r.polygon));
                let floor = area(&ring);
                let height = (s.height - bd.ceiling_thickness(level)).max(100.0);
                if ring.len() < 3 || !(floor > 10_000.0) || !height.is_finite() {
                    continue;
                }
                n += 1;
                let rid = if r.id.is_empty() { format!("{}.room{k}", bd.id) } else { r.id.clone() };
                let key = format!("{rid}.space.L{level}");
                let frame = fl.at(level).frame;
                let placed = self.w.place(Some(fl.at(level)), frame);
                let local: Vec<[f64; 2]> = ring.iter().map(|p| frame.local2(*p)).collect();
                let solid = self.extruded(&local, &[], height, catalog::SPACE);
                let Some(shape) = self.shape("SweptSolid", &[solid], None, catalog::L_SPACE) else { continue };
                let long = if r.name.is_empty() { "Rum" } else { r.name.as_str() };
                let number = format!("{level}.{n:02}");
                let id = self.w.add(format!(
                    "IFCSPACE({},$,{},{},$,#{},#{shape},{},.ELEMENT.,.INTERNAL.,$)",
                    gid(&key),
                    string(&number),
                    string(&rid),
                    placed.id,
                    string(long)
                ));
                let perimeter: f64 = (0..ring.len())
                    .map(|i| {
                        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                        (b[0] - a[0]).hypot(b[1] - a[1])
                    })
                    .sum();
                let qs = [
                    Qty::Length("Height", height),
                    Qty::Length("GrossPerimeter", perimeter),
                    Qty::Area("GrossFloorArea", floor / 1e6),
                    Qty::Area("NetFloorArea", floor / 1e6),
                    Qty::Volume("GrossVolume", floor * height / 1e9),
                    Qty::Volume("NetVolume", floor * height / 1e9),
                ];
                self.cat.quantities(&mut self.w, &key, "Qto_SpaceBaseQuantities", &qs, id);
                self.cat.pset(&mut self.w, "Pset_SpaceCommon", &[("Reference", Val::Id(number.clone())), ("IsExternal", Val::Bool(false))], id);
                if let Some(v) = fl.spaces.get_mut(level) {
                    v.push(id);
                }
            }
        }
    }
}
