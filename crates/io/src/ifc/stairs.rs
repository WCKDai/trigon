//! Stairs: one `IfcStair` per staircase and storey, aggregating its flights (`IfcStairFlight`,
//! straight or winding) and half landing (`IfcSlab` LANDING), with the step counts in
//! Pset_StairCommon and Pset_StairFlightCommon.

use trigon_model::build::BuiltElement;
use trigon_model::stair::PartKind;
use trigon_model::{Building, Built, FacadeFrame, Material, StairKind};

use super::catalog::{self, Qty, Val};
use super::fills::facade_frame_at;
use super::part21::{gid, real, refs, string, volume};
use super::split;
use super::{Floors, Ifc};

impl Ifc<'_> {
    pub(super) fn stairs(&mut self, b: &Built, bd: &Building, fl: &mut Floors) {
        for st in split::stairs(bd) {
            let Some(built) = b.stairs.iter().find(|s| s.id == st.id) else { continue };
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
            // The balustrade along the well of a U-turn stair.
            if let Some(item) = self.w.faceset(&built.railing, &frame) {
                self.cat.paint(&mut self.w, item, catalog::RAIL);
                let key = format!("{}.railing", st.id);
                let lp = self.w.place(Some(placed), frame);
                if let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_STAIR) {
                    let id = self.w.add(format!("IFCRAILING({},$,'Trappeværn',$,$,#{},#{shape},{},.BALUSTRADE.)", gid(&key), lp.id, string(&key)));
                    self.common("Pset_RailingCommon", id, "Trappeværn", None, vec![("IsExternal", Val::Bool(false))]);
                    self.made_of(id, catalog::RAIL);
                    parts.push(id);
                }
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

impl Ifc<'_> {
    /// An outdoor cellar stair: an IfcStair of its flight, the pit's retaining walls and the
    /// railing on the kerb, contained in the storey it leads down to.
    pub(super) fn cellar_stair(&mut self, e: &BuiltElement, fr: &FacadeFrame, fl: &mut Floors) {
        let pieces = split::cellar(e);
        let frame = facade_frame_at(fr, pieces.iter().map(|(_, _, m)| m));
        let placed = self.w.place(Some(fl.at(e.level)), frame);
        let stair = self.w.add(format!(
            "IFCSTAIR({},$,{},$,'Kældernedgang',#{},$,{},.STRAIGHT_RUN_STAIR.)",
            gid(&e.id),
            string(&e.name),
            placed.id,
            string(&e.id)
        ));
        let (mut parts, mut steps) = (Vec::new(), (0usize, 0.0, 0.0));
        for (key, m, mesh) in &pieces {
            let lp = self.w.place(Some(placed), frame);
            let Some(item) = self.w.faceset(mesh, &frame) else { continue };
            let mat = if *m == Material::Railing { catalog::RAIL } else { catalog::of(*m) };
            self.cat.paint(&mut self.w, item, mat);
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_STAIR) else { continue };
            let (g, tag) = (gid(key), string(key));
            let id = match m {
                Material::Sill => {
                    // One block per riser, from the pit floor up to the terrain.
                    let risers = split::boxes(mesh).len().max(1);
                    let run = mesh
                        .positions
                        .iter()
                        .map(|p| frame.local(*p)[0])
                        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
                    let (rise, going) = (e.height / risers as f64, (run.1 - run.0).max(0.0) / risers as f64);
                    steps = (risers, rise, going);
                    let id = self.w.add(format!(
                        "IFCSTAIRFLIGHT({g},$,'Trappeløb',$,$,#{},#{shape},{tag},{risers},{},{},{},.STRAIGHT.)",
                        lp.id,
                        risers.saturating_sub(1),
                        real(rise),
                        real(going)
                    ));
                    self.common(
                        "Pset_StairFlightCommon",
                        id,
                        &e.name,
                        Some(e.status),
                        vec![("NumberOfRiser", Val::Count(risers as f64)), ("RiserHeight", Val::Length(rise))],
                    );
                    id
                }
                Material::Railing => {
                    let id = self.w.add(format!("IFCRAILING({g},$,'Rækværk',$,$,#{},#{shape},{tag},.GUARDRAIL.)", lp.id));
                    self.common(
                        "Pset_RailingCommon",
                        id,
                        &e.name,
                        Some(e.status),
                        vec![("IsExternal", Val::Bool(true)), ("Height", Val::Length(900.0))],
                    );
                    id
                }
                _ => {
                    let (name, what) = (string("Støttemur, beton"), string("Støttemur"));
                    let id = self.w.add(format!("IFCWALL({g},$,{name},$,{what},#{},#{shape},{tag},.USERDEFINED.)", lp.id));
                    self.common(
                        "Pset_WallCommon",
                        id,
                        &e.name,
                        Some(e.status),
                        vec![("IsExternal", Val::Bool(true)), ("LoadBearing", Val::Bool(true))],
                    );
                    let v = volume(mesh);
                    self.cat.quantities(
                        &mut self.w,
                        key,
                        "Qto_WallBaseQuantities",
                        &[Qty::Volume("GrossVolume", v / 1e9), Qty::Volume("NetVolume", v / 1e9)],
                        id,
                    );
                    id
                }
            };
            self.made_of(id, mat);
            parts.push(id);
        }
        if !parts.is_empty() {
            self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{stair},({}))", gid(&format!("{}.parts", e.id)), refs(&parts)));
        }
        let (risers, rise, going) = steps;
        let mut props = vec![("IsExternal", Val::Bool(true))];
        if risers > 0 {
            props.extend([
                ("NumberOfRiser", Val::Count(risers as f64)),
                ("NumberOfTreads", Val::Count(risers.saturating_sub(1) as f64)),
                ("RiserHeight", Val::Length(rise)),
            ]);
            if going > 0.0 {
                props.push(("TreadLength", Val::Length(going)));
            }
        }
        self.common("Pset_StairCommon", stair, &e.name, Some(e.status), props);
        fl.contain(e.level, stair);
    }
}
