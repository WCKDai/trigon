//! Inclined windows keep real IfcWindow/IfcOpeningElement identities and host relationships.
use super::catalog::Qty;
use super::*;
impl Ifc<'_> {
    pub(super) fn rooflights(&mut self, r: &trigon_model::build::BuiltRoof, roof: usize, fl: &mut Floors) {
        for light in &r.rooflights {
            let level = light.fill.level;
            let Some(frame) = Frame::new(light.frame.origin, light.frame.up, light.frame.x) else { continue };
            let placed = self.w.place(Some(fl.at(level)), frame);
            let Some(item) = self.w.faceset(&light.opening, &frame) else { continue };
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_OPENING) else { continue };
            let opening = self.w.add(format!(
                "IFCOPENINGELEMENT({},$,'Rooflight through-roof opening',$,$,#{},#{shape},{},.OPENING.)",
                gid(&light.opening_id),
                placed.id,
                string(&light.opening_id)
            ));
            self.w.add(format!("IFCRELVOIDSELEMENT({},$,$,$,#{roof},#{opening})", gid(&format!("{}.voids", light.opening_id))));
            let fill_frame = frame.shifted([0., -light.stand_off, 0.]);
            let fill = self.skylight(&light.fill, placed, fill_frame, fl);
            self.w.add(format!("IFCRELFILLSELEMENT({},$,$,$,#{opening},#{fill})", gid(&format!("{}.fills", light.opening_id))));
            self.cat.quantities(
                &mut self.w,
                &light.opening_id,
                "Qto_OpeningElementBaseQuantities",
                &[
                    Qty::Length("Width", light.fill.width),
                    Qty::Length("Height", light.fill.height),
                    Qty::Area("Area", light.fill.width * light.fill.height / 1e6),
                    Qty::Volume("Volume", part21::volume(&light.opening) / 1e9),
                ],
                opening,
            );
            let key = format!("{}.flashing", light.id);
            let Some(item) = self.w.faceset(&light.flashing, &frame) else { continue };
            self.cat.paint(&mut self.w, item, catalog::ZINC);
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_ROOF) else { continue };
            let id = self.w.add(format!(
                "IFCCOVERING({},$,'Rooflight curb and flashing - family detail',$,$,#{},#{shape},{},.ROOFING.)",
                gid(&key),
                placed.id,
                string(&key)
            ));
            self.common("Pset_CoveringCommon", id, "Rooflight curb and flashing", None, vec![("IsExternal", Val::Bool(true))]);
            self.made_of(id, catalog::ZINC);
            fl.contain(level, id);
        }
    }
}
