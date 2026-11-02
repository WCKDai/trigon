//! Openings and what fills them: an opening element per wall piece an opening cuts, shaped as the
//! opening through that piece; the window or door placed in the opening, typed, with its frame,
//! leaves and glass as separately coloured items; and the railings that belong to the opening.

use trigon_model::build::{BuiltElement, BuiltWall, Status};
use trigon_model::component::Side;
use trigon_model::{ComponentKind, ElementClass, FacadeFrame, Material};

use super::catalog::{self, Qty, Val};
use super::part21::{Frame, Placed, gid, real, refs, string};
use super::walls::Piece;
use super::{Floors, Ifc};

pub(crate) fn window_partitioning(kind: &ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Window { lights: 1, .. } | ComponentKind::HistoricWindow { lights: 1, .. } | ComponentKind::FixedWindow => ".SINGLE_PANEL.",
        ComponentKind::Window { lights: 2, .. } | ComponentKind::HistoricWindow { lights: 2, .. } => ".DOUBLE_PANEL_VERTICAL.",
        ComponentKind::Window { lights: 3, .. } | ComponentKind::HistoricWindow { lights: 3, .. } => ".TRIPLE_PANEL_VERTICAL.",
        _ => ".NOTDEFINED.",
    }
}

pub(crate) fn door_operation(kind: &ComponentKind) -> &'static str {
    match kind {
        ComponentKind::FrenchDoor { leaves: 2 }
        | ComponentKind::FrenchBalcony { leaves: 2, .. }
        | ComponentKind::Balcony { leaves: 2, .. }
        | ComponentKind::PanelDoor { leaves: 2, .. } => ".DOUBLE_DOOR_SINGLE_SWING.",
        ComponentKind::FrenchDoor { leaves } | ComponentKind::FrenchBalcony { leaves, .. } | ComponentKind::Balcony { leaves, .. } if *leaves > 2 => {
            ".NOTDEFINED."
        }
        ComponentKind::DoorWindow { door_side: Side::Right, .. } => ".SINGLE_SWING_RIGHT.",
        _ => ".SINGLE_SWING_LEFT.",
    }
}

/// A frame on the facade (x along it, y into the building, z up) at the lowest, leftmost
/// corner of `meshes`, seen from outside.
pub(crate) fn facade_frame_at<'m>(fr: &FacadeFrame, meshes: impl Iterator<Item = &'m trigon_model::Mesh>) -> Frame {
    let base = Frame::plan([fr.origin[0], fr.origin[1], 0.0], fr.t).unwrap_or(Frame::at([fr.origin[0], fr.origin[1], 0.0]));
    let mut lo = [f64::INFINITY; 3];
    for m in meshes {
        for p in &m.positions {
            let q = base.local(*p);
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
            }
        }
    }
    if !lo.iter().all(|v| v.is_finite()) {
        return base;
    }
    base.shifted([lo[0], 0.0, lo[2]])
}

impl Ifc<'_> {
    /// An above-eaves window belongs to the actual shaped gable wall, not a
    /// fabricated rectangular storey piece. Preserve ordinary opening/fill identities.
    pub(super) fn gable_openings(&mut self, g: &trigon_model::build::BuiltGable, host: usize, parent: Placed, fl: &mut Floors) {
        for o in &g.front.openings {
            let r = o.rect;
            let (w, h) = (r.u1 - r.u0, r.v1 - r.v0);
            let frame = parent.frame.shifted([r.u0, 0., r.v0 - g.front.bottom]);
            let placed = self.w.place(Some(parent), frame);
            let profile = self.w.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,$,{}, {})", real(w), real(h)));
            let position = self.w.axis2([w / 2., 0., h / 2.], [0., 1., 0.], [1., 0., 0.]);
            let solid = self.w.extrusion(profile, position, g.thickness);
            let Some(shape) = self.shape("SweptSolid", &[solid], None, catalog::L_OPENING) else { continue };
            let opening =
                self.w.add(format!("IFCOPENINGELEMENT({},$,'Opening',$,$,#{},#{shape},{},.OPENING.)", gid(&o.id), placed.id, string(&o.id)));
            self.w.add(format!("IFCRELVOIDSELEMENT({},$,$,$,#{host},#{opening})", gid(&format!("{}.voids", o.id))));
            let fill = self.fill(&o.fill, placed, frame, true, fl);
            self.w.add(format!("IFCRELFILLSELEMENT({},$,$,$,#{opening},#{fill})", gid(&format!("{}.fills", o.id))));
        }
    }

    /// The openings of one facade wall, their fills and what is attached to them.
    pub(super) fn openings(&mut self, wall: &BuiltWall, pieces: &[&Piece], fl: &mut Floors) {
        for (k, o) in wall.openings.iter().enumerate() {
            let r = o.rect;
            // The opening on its own storey's piece hosts the fill (or the first piece it cuts).
            let mut host: Option<(Placed, f64, usize)> = None;
            for p in pieces.iter().filter(|p| p.seg.openings.contains(&k)) {
                let seg = p.seg;
                let (v0, v1) = (r.v0.max(seg.z0), r.v1.min(seg.z1));
                let (w, h) = (r.u1 - r.u0, v1 - v0);
                if !(w > 1.0 && h > 1.0) {
                    continue;
                }
                // At the opening's outer bottom-left corner, seen from outside; through the piece.
                let frame = p.placed.frame.shifted([r.u0, 0.0, v0 - seg.z0]);
                let placed = self.w.place(Some(p.placed), frame);
                let (profile, position) = if o.profile.is_empty() {
                    (
                        self.w.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,$,{},{})", real(w), real(h))),
                        self.w.axis2([w / 2.0, 0.0, h / 2.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
                    )
                } else {
                    let poly = trigon_model::profile::clip(&o.profile, trigon_model::Rect { v0, v1, ..r });
                    // The extrusion axis is +Y into the wall and profile X is +X along it;
                    // their cross product makes profile Y point down (-Z). Negate V so
                    // the real opening rises above its bottom, like the centred rectangle.
                    let poly: Vec<_> = poly.into_iter().map(|p| [p[0] - r.u0, v0 - p[1]]).collect();
                    (self.w.profile(&poly, &[]), self.w.axis2([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]))
                };
                let solid = self.w.extrusion(profile, position, seg.thickness);
                let Some(shape) = self.shape("SweptSolid", &[solid], None, catalog::L_OPENING) else { continue };
                let key = if seg.level == o.level { o.id.clone() } else { format!("{}.L{}", o.id, seg.level) };
                let opening =
                    self.w.add(format!("IFCOPENINGELEMENT({},$,'Opening',$,$,#{},#{shape},{},.OPENING.)", gid(&key), placed.id, string(&key)));
                self.w.add(format!("IFCRELVOIDSELEMENT({},$,$,$,#{},#{opening})", gid(&format!("{key}.voids")), p.id));
                if seg.level == o.level || host.is_none() {
                    host = Some((placed, v0, opening));
                }
            }
            let Some((placed, v0, opening)) = host else { continue };
            // The window or door: placed in its opening at the opening's real bottom.
            let frame = placed.frame.shifted([0.0, 0.0, r.v0 - v0]);
            let fill = self.fill(&o.fill, placed, frame, true, fl);
            self.w.add(format!("IFCRELFILLSELEMENT({},$,$,$,#{opening},#{fill})", gid(&format!("{}.fills", o.id))));
            // A cantilever balcony belongs to its door; a French balcony is only a rail.
            if o.attached.iter().any(|a| a.class == ElementClass::BalconySlab) {
                let parts: Vec<&BuiltElement> = o.attached.iter().collect();
                self.balcony(&format!("{}.assembly", o.fill.id), &parts, &wall.frame, fl);
            } else {
                for a in &o.attached {
                    self.railing(a, &wall.frame, fl);
                }
            }
        }
    }

    /// A window or door in `frame`, placed relative to its opening.
    pub(super) fn fill(&mut self, e: &BuiltElement, opening: Placed, frame: Frame, external: bool, fl: &mut Floors) -> usize {
        self.fill_as(e, opening, frame, external, fl, false)
    }
    pub(super) fn skylight(&mut self, e: &BuiltElement, opening: Placed, frame: Frame, fl: &mut Floors) -> usize {
        self.fill_as(e, opening, frame, true, fl, true)
    }
    fn fill_as(&mut self, e: &BuiltElement, opening: Placed, frame: Frame, external: bool, fl: &mut Floors, skylight: bool) -> usize {
        let placed = self.w.place(Some(opening), frame);
        let material_items = self.tessellated_parts(&e.parts, &frame);
        let items: Vec<_> = material_items.iter().map(|(_, item)| *item).collect();
        let door = e.class == ElementClass::Door;
        let layer = if door { catalog::L_DOOR } else { catalog::L_WINDOW };
        let definition = self.shape("Tessellation", &items, None, layer);
        let shape = definition.map_or("$".to_string(), |s| format!("#{s}"));
        let ty = self.doc.component(&e.type_id).ok();
        let kind = ty.map(|t| &t.kind);
        let (g, name, tag) = (gid(&e.id), string(&e.name), string(&e.id));
        let (h, wd) = (real(e.height), real(e.width));
        let predefined = if skylight { ".SKYLIGHT." } else { ".WINDOW." };
        let id = if door {
            let op = kind.map_or(".SINGLE_SWING_LEFT.", door_operation);
            self.w.add(format!("IFCDOOR({g},$,{name},$,$,#{},{shape},{tag},{h},{wd},.DOOR.,{op},$)", placed.id))
        } else {
            let part = kind.map_or(".NOTDEFINED.", window_partitioning);
            self.w.add(format!("IFCWINDOW({g},$,{name},$,$,#{},{shape},{tag},{h},{wd},{predefined},{part},$)", placed.id))
        };
        if let Some(definition) = definition {
            self.opening_materials(id, definition, &material_items);
        }
        // The type: the component type, or for doors Trigon makes itself (interior doors) one by name.
        let reference = ty.map_or(e.name.as_str(), |t| t.name.as_str());
        let type_key = if skylight {
            // Encode the source-key boundary and avoid every real facade-family key.
            // Repeated namespace prefixes cannot equal another encoded rooflight key.
            let mut key = format!("rooflight:{}:{}", e.type_id.len(), e.type_id);
            for _ in 0..=self.doc.types.len() {
                if !self.doc.types.iter().any(|t| t.id == key) {
                    break;
                }
                key = format!("rooflight:{key}");
            }
            key
        } else {
            e.type_id.clone()
        };
        let tg = gid(&format!("type:{type_key}"));
        let (tn, tt) = (string(reference), string(&type_key));
        let operation = kind.map_or(".SINGLE_SWING_LEFT.", door_operation);
        let partitioning = kind.map_or(".NOTDEFINED.", window_partitioning);
        self.cat.typed(&mut self.w, &type_key, id, |w| {
            if door {
                w.add(format!("IFCDOORTYPE({tg},$,{tn},$,$,$,$,{tt},$,.DOOR.,{operation},.F.,$)"))
            } else {
                w.add(format!("IFCWINDOWTYPE({tg},$,{tn},$,$,$,$,{tt},$,{predefined},{partitioning},.F.,$)"))
            }
        });
        let pset = if door { "Pset_DoorCommon" } else { "Pset_WindowCommon" };
        self.common(pset, id, reference, Some(e.status), vec![("IsExternal", Val::Bool(external))]);
        if !matches!(kind, Some(ComponentKind::Loggia { .. })) {
            let occurrence = self.cat.fill_quantity_counts.entry((e.id.clone(), e.type_id.clone())).or_default();
            let quantity_key = format!("fill:{}:{}:{}:{}:{occurrence}", e.id.len(), e.id, e.type_id.len(), e.type_id);
            *occurrence += 1;
            self.cat.quantities(
                &mut self.w,
                &quantity_key,
                if door { "Qto_DoorBaseQuantities" } else { "Qto_WindowBaseQuantities" },
                &[
                    Qty::Length("Height", e.height),
                    Qty::Length("Width", e.width),
                    Qty::Area("Area", kind.map_or(e.width * e.height, |kind| trigon_model::profile::area(kind, e.width, e.height)) / 1e6),
                ],
                id,
            );
        }
        fl.contain(e.level, id);
        id
    }

    /// IFC4 constituents and matching named shape aspects identify the actual material parts.
    /// Fractions remain unset: these presentation meshes do not establish composition quantities.
    fn opening_materials(&mut self, object: usize, definition: usize, items: &[(Material, usize)]) {
        let mut groups = std::collections::BTreeMap::<Material, Vec<usize>>::new();
        for &(material, item) in items {
            groups.entry(material).or_default().push(item);
        }
        let mut constituents = Vec::new();
        for (material, items) in groups {
            let role = match material {
                Material::Frame => "Framing",
                Material::Glass => "Glazing",
                Material::Door => "Panel",
                Material::Gasket => "Gasket",
                Material::Steel => "Hardware",
                Material::SteelWhite => "Guard",
                Material::Sill => "Sill",
                Material::Render => "Lining",
                _ => material.name(),
            };
            let mat = self.cat.material(&mut self.w, catalog::of(material));
            constituents.push(self.w.add(format!("IFCMATERIALCONSTITUENT({},$,#{mat},$,$)", string(role))));
            let rep = self.w.add(format!("IFCSHAPEREPRESENTATION(#{},'Body','Tessellation',({}))", self.cat.context, refs(&items)));
            self.w.add(format!("IFCSHAPEASPECT((#{rep}),{},$,.T.,#{definition})", string(role)));
        }
        if !constituents.is_empty() {
            let set = self.w.add(format!("IFCMATERIALCONSTITUENTSET('Opening materials',$,({}))", refs(&constituents)));
            self.cat.associate(&format!("fill-materials:{object}"), set, object);
        }
    }

    /// A railing on its own (a French balcony's rail), contained in its storey.
    pub(super) fn railing(&mut self, e: &BuiltElement, fr: &FacadeFrame, fl: &mut Floors) -> usize {
        let frame = facade_frame_at(fr, e.parts.iter().map(|(_, m)| m));
        let id = self.railing_in(e, &e.id, fl.at(e.level), frame);
        fl.contain(e.level, id);
        id
    }

    /// An IfcRailing placed relative to `parent`.
    pub(super) fn railing_in(&mut self, e: &BuiltElement, key: &str, parent: Placed, frame: Frame) -> usize {
        let placed = self.w.place(Some(parent), frame);
        let items = self.tessellated(&e.parts, &frame);
        let shape = self.shape("Tessellation", &items, None, catalog::L_BALCONY).map_or("$".to_string(), |s| format!("#{s}"));
        let id = self.w.add(format!("IFCRAILING({},$,{},$,$,#{},{shape},{},.GUARDRAIL.)", gid(key), string(&e.name), placed.id, string(key)));
        let ty = self.doc.component(&e.type_id).ok();
        let reference = ty.map_or(e.name.as_str(), |t| t.name.as_str());
        let height = match ty.map(|t| &t.kind) {
            Some(ComponentKind::FrenchBalcony { rail_height, .. } | ComponentKind::Balcony { rail_height, .. }) => *rail_height,
            _ => e.height,
        };
        self.common("Pset_RailingCommon", id, reference, Some(e.status), vec![("IsExternal", Val::Bool(true)), ("Height", Val::Length(height))]);
        let main = e.parts.first().map_or(catalog::RAIL, |(m, _)| catalog::of(*m));
        self.made_of(id, main);
        id
    }
}

/// The phase status of a group of elements: the first one's.
pub(crate) fn status_of(es: &[&BuiltElement]) -> Status {
    es.first().map_or(Status::Existing, |e| e.status)
}
