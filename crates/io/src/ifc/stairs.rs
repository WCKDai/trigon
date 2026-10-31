//! Stairs: one `IfcStair` per staircase and storey, aggregating its flights (`IfcStairFlight`,
//! straight or winding) and half landing (`IfcSlab` LANDING), with the step counts in
//! Pset_StairCommon and Pset_StairFlightCommon.

use trigon_model::stair::PartKind;
use trigon_model::{Building, Built, StairKind};

use super::catalog::{self, Val};
use super::part21::{gid, real, refs, string};
use super::split;
use super::{Floors, Ifc};

impl Ifc<'_> {
    pub(super) fn stairs(&mut self, b: &Built, bd: &Building, fl: &mut Floors) {
        for st in split::stairs(bd) {
            if !b.stairs.iter().any(|s| s.id == st.id) {
                continue;
            }
            let frame = fl.at(st.level).frame;
            let placed = self.w.place(Some(fl.at(st.level)), frame);
            let kind = match st.stair.kind {
                StairKind::Straight => "STRAIGHT_RUN_STAIR",
                StairKind::UTurn if st.stair.winders.is_some() => "HALF_WINDING_STAIR",
                StairKind::UTurn => "HALF_TURN_STAIR",
                StairKind::DoubleQuarterTurn => "TWO_QUARTER_WINDING_STAIR",
            };
            let stair = self.w.add(format!("IFCSTAIR({},$,'Trappe',$,$,#{},$,{},.{kind}.)", gid(&st.id), placed.id, string(&st.id)));
            let mut parts = Vec::new();
            for (key, p) in &st.parts {
                let lp = self.w.place(Some(placed), frame);
                let Some(item) = self.w.faceset(&p.mesh, &frame) else { continue };
                self.cat.paint(&mut self.w, item, catalog::STAIR);
                let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_STAIR) else { continue };
                let id = match p.kind {
                    PartKind::Landing => {
                        let id = self.w.add(format!("IFCSLAB({},$,'Repos',$,$,#{},#{shape},{},.LANDING.)", gid(key), lp.id, string(key)));
                        self.common("Pset_SlabCommon", id, "Repos", None, vec![("IsExternal", Val::Bool(false))]);
                        id
                    }
                    PartKind::Flight | PartKind::Winders => {
                        let (name, ty) = if p.kind == PartKind::Flight { ("Trappeløb", "STRAIGHT") } else { ("Spiraltrin", "WINDER") };
                        let going = if p.going > 0.0 { real(p.going) } else { "$".into() };
                        let id = self.w.add(format!(
                            "IFCSTAIRFLIGHT({},$,'{name}',$,$,#{},#{shape},{},{},{},{},{going},.{ty}.)",
                            gid(key),
                            lp.id,
                            string(key),
                            p.risers,
                            p.treads,
                            real(p.rise)
                        ));
                        let mut props = vec![("NumberOfRiser", Val::Count(p.risers as f64)), ("NumberOfTreads", Val::Count(p.treads as f64))];
                        props.push(("RiserHeight", Val::Length(p.rise)));
                        if p.going > 0.0 {
                            props.push(("TreadLength", Val::Length(p.going)));
                        }
                        self.common("Pset_StairFlightCommon", id, name, None, props);
                        id
                    }
                };
                self.made_of(id, catalog::STAIR);
                parts.push(id);
            }
            if !parts.is_empty() {
                self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{stair},({}))", gid(&format!("{}.parts", st.id)), refs(&parts)));
            }
            let risers: usize = st.parts.iter().map(|(_, p)| p.risers).sum();
            let treads: usize = st.parts.iter().map(|(_, p)| p.treads).sum();
            let rise = st.parts.first().map_or(0.0, |(_, p)| p.rise);
            let going = st.parts.iter().map(|(_, p)| p.going).find(|g| *g > 0.0);
            let mut props = vec![
                ("NumberOfRiser", Val::Count(risers as f64)),
                ("NumberOfTreads", Val::Count(treads as f64)),
                ("RiserHeight", Val::Length(rise)),
                ("IsExternal", Val::Bool(false)),
            ];
            if let Some(g) = going {
                props.push(("TreadLength", Val::Length(g)));
            }
            self.common("Pset_StairCommon", stair, "Trappe", None, props);
            fl.contain(st.level, stair);
        }
    }
}
