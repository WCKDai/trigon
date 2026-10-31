//! Balconies. IFC4 has no balcony entity; a balcony is an `IfcElementAssembly` of the parts it is
//! built from (slab or deck, railing, and for the steel balcony its UPE frame beams and tie rods),
//! so each part keeps its own class and category on import, and the assembly carries the type
//! and dimensions.

use trigon_model::build::{BuiltElement, BuiltWall};
use trigon_model::component::{Deck, Finish, Railing, Screens};
use trigon_model::{ComponentKind, FacadeFrame};

use super::catalog::{self, Val};
use super::fills::{facade_frame_at, status_of};
use super::part21::{gid, refs, string};
use super::split::{self, Class};
use super::{Floors, Ifc};

fn railing_name(r: Railing) -> &'static str {
    match r {
        Railing::Frederiksberg => "Frederiksberg",
        Railing::Norrebro => "Nørrebro",
        Railing::Vesterbro => "Vesterbro",
        Railing::Osterbro => "Østerbro",
    }
}

fn finish_name(f: Finish) -> &'static str {
    match f {
        Finish::Galvanised => "Varmforzinket",
        Finish::White => "Hvid",
        Finish::Anthracite => "Antracit",
    }
}

fn deck_name(d: Deck) -> &'static str {
    match d {
        Deck::Cumaru => "Cumaru",
        Deck::FibreConcrete => "Fiberbeton",
        Deck::Composite => "Komposit",
        Deck::Bamboo => "Bambus",
    }
}

fn screens_name(s: Screens) -> &'static str {
    match s {
        Screens::None => "Ingen",
        Screens::Left => "Venstre",
        Screens::Right => "Højre",
        Screens::Both => "Begge sider",
    }
}

impl Ifc<'_> {
    /// The balconies hung on a facade wall without an opening of their own.
    pub(super) fn wall_balconies(&mut self, wall: &BuiltWall, fl: &mut Floors) {
        for (key, group) in split::attachment_groups(wall) {
            match group.first() {
                Some(e) if e.class == trigon_model::ElementClass::Stair => self.cellar_stair(e, &wall.frame, fl),
                _ => self.balcony(&key, &group, &wall.frame, fl),
            }
        }
    }

    /// One balcony: an element assembly of its parts, contained in its storey.
    pub(super) fn balcony(&mut self, key: &str, elements: &[&BuiltElement], fr: &FacadeFrame, fl: &mut Floors) {
        let Some(first) = elements.first() else { return };
        let level = first.level;
        let ty = self.doc.component(&first.type_id).ok();
        let kind = ty.map(|t| &t.kind);
        let products = split::balcony(elements, kind);
        let frame = facade_frame_at(fr, products.iter().flat_map(|p| p.parts.iter().map(|(_, m)| m)));
        let placed = self.w.place(Some(fl.at(level)), frame);
        let status = status_of(elements);
        let type_name = ty.map_or(first.name.as_str(), |t| t.name.as_str());
        let place = if matches!(kind, Some(ComponentKind::SteelBalcony { .. })) { ".FACTORY." } else { ".NOTDEFINED." };
        let assembly = self.w.add(format!(
            "IFCELEMENTASSEMBLY({},$,{},$,'Altan',#{},$,{},{place},.USERDEFINED.)",
            gid(key),
            string(type_name),
            placed.id,
            string(key)
        ));
        let mut parts = Vec::new();
        for p in &products {
            if p.class == Class::Railing {
                if let Some(e) = elements.iter().find(|e| e.id == p.key) {
                    parts.push(self.railing_in(e, &p.key, placed, frame));
                }
                continue;
            }
            let lp = self.w.place(Some(placed), frame);
            let items = self.tessellated(&p.parts, &frame);
            let Some(shape) = self.shape("Tessellation", &items, None, catalog::L_BALCONY) else { continue };
            let (g, name, tag) = (gid(&p.key), string(&p.name), string(&p.key));
            let main = p.parts.first().map_or(catalog::STEEL, |(m, _)| catalog::of(*m));
            let id = match p.class {
                Class::Railing => continue,
                Class::Slab => {
                    let id = self.w.add(format!("IFCSLAB({g},$,{name},$,'Altandæk',#{},#{shape},{tag},.USERDEFINED.)", lp.id));
                    self.common(
                        "Pset_SlabCommon",
                        id,
                        type_name,
                        Some(status),
                        vec![("IsExternal", Val::Bool(true)), ("LoadBearing", Val::Bool(true))],
                    );
                    self.made_of(id, main);
                    id
                }
                Class::Beam => {
                    let id = self.w.add(format!("IFCBEAM({g},$,{name},$,$,#{},#{shape},{tag},.BEAM.)", lp.id));
                    self.common(
                        "Pset_BeamCommon",
                        id,
                        type_name,
                        Some(status),
                        vec![("IsExternal", Val::Bool(true)), ("LoadBearing", Val::Bool(true))],
                    );
                    self.made_of(id, main);
                    id
                }
                Class::Member => {
                    let id = self.w.add(format!("IFCMEMBER({g},$,{name},$,'Trækstang',#{},#{shape},{tag},.USERDEFINED.)", lp.id));
                    self.common(
                        "Pset_MemberCommon",
                        id,
                        type_name,
                        Some(status),
                        vec![("IsExternal", Val::Bool(true)), ("LoadBearing", Val::Bool(true))],
                    );
                    self.made_of(id, main);
                    id
                }
            };
            parts.push(id);
        }
        if parts.is_empty() {
            return;
        }
        self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{assembly},({}))", gid(&format!("{key}.parts")), refs(&parts)));
        let mut props = vec![("TypeName", Val::Label(type_name.to_string()))];
        if let Some(t) = ty {
            props.push(("TypeId", Val::Id(t.id.clone())));
            props.push(("Width", Val::Length(t.width)));
        }
        match kind {
            Some(ComponentKind::SteelBalcony { depth, railing, finish, deck, handrail, screens, tie_rods }) => {
                props.push(("Depth", Val::Length(*depth)));
                props.push(("RailingHeight", Val::Length(first.height)));
                props.push(("Railing", Val::Label(railing_name(*railing).into())));
                props.push(("Finish", Val::Label(finish_name(*finish).into())));
                props.push(("Deck", Val::Label(deck_name(*deck).into())));
                props.push(("Handrail", Val::Bool(*handrail)));
                props.push(("Screens", Val::Label(screens_name(*screens).into())));
                props.push(("TieRods", Val::Bool(*tie_rods)));
            }
            Some(ComponentKind::Balcony { depth, slab_width, rail_height, .. }) => {
                props.push(("Depth", Val::Length(*depth)));
                props.push(("SlabWidth", Val::Length(*slab_width)));
                props.push(("RailingHeight", Val::Length(*rail_height)));
            }
            Some(ComponentKind::BalconySlab { depth, thickness }) => {
                props.push(("Depth", Val::Length(*depth)));
                props.push(("SlabThickness", Val::Length(*thickness)));
                props.push(("RailingHeight", Val::Length(first.height)));
            }
            _ => {}
        }
        if self.phases {
            props.push(("Status", Val::Status(status)));
        }
        self.cat.pset(&mut self.w, "Trigon_Balcony", &props, assembly);
        fl.contain(level, assembly);
    }
}
