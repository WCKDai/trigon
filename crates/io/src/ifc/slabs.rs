//! Floors and roofs. A floor is an `IfcSlab` extruded from its outline with the stair openings
//! as voids in the profile, so the hole is part of the slab's shape everywhere it is read (also
//! where the slab's outline takes a notch at the wall) and needs no boolean at import. A roof is
//! an `IfcRoof`: a flat roof of one slab, a pitched roof of one slab per plane; gables are walls.

use trigon_model::build::BuiltSlab;
use trigon_model::{Built, SlabKind};

use super::catalog::{self, Qty, Val};
use super::part21::{Frame, area, ccw, clean_ring, gid, refs, string};
use super::split::{self, ROOF_PLANE};
use super::{Floors, Ifc};

/// A floor this close to the terrain or below it rests on the ground (a basement or slab on grade), mm.
const ON_GROUND: f64 = 300.0;

impl Ifc<'_> {
    pub(super) fn slabs(&mut self, b: &Built, fl: &mut Floors) {
        for s in &b.slabs {
            match s.kind {
                SlabKind::Floor => {
                    let on_ground = s.level == 0 && b.storeys.first().is_some_and(|st| st.elevation <= ON_GROUND);
                    let up = catalog::floor(s.thickness, on_ground);
                    if let Some(id) = self.slab(s, &up, fl.at(s.level), false) {
                        fl.contain(s.level, id);
                    }
                }
                SlabKind::Roof => {
                    let up = catalog::flat_roof(s.thickness);
                    let key = format!("{}.assembly", s.id);
                    let placed = self.w.place(Some(fl.at(s.level)), fl.at(s.level).frame);
                    let roof = self.w.add(format!("IFCROOF({},$,'Tag',$,$,#{},$,{},.FLAT_ROOF.)", gid(&key), placed.id, string(&key)));
                    self.common("Pset_RoofCommon", roof, &up.name, None, vec![("IsExternal", Val::Bool(true))]);
                    if let Some(id) = self.slab(s, &up, placed, true) {
                        self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{roof},(#{id}))", gid(&format!("{key}.parts"))));
                    }
                    fl.contain(s.level, roof);
                }
            }
        }
    }

    /// A horizontal slab relative to `parent`, with its type, layers, properties and quantities.
    fn slab(&mut self, s: &BuiltSlab, up: &catalog::BuildUp, parent: super::part21::Placed, roof: bool) -> Option<usize> {
        let outer = ccw(clean_ring(&s.polygon));
        let holes: Vec<Vec<[f64; 2]>> = s.holes.iter().map(|h| clean_ring(h)).filter(|h| h.len() >= 3).collect();
        if outer.len() < 3 || !(area(&outer) > 1.0) || !(s.thickness > 0.0) {
            return None;
        }
        let frame = Frame::at([0.0, 0.0, s.z_bottom]);
        let placed = self.w.place(Some(parent), frame);
        let top = up.layers.last().map_or(catalog::CONCRETE, |l| l.mat);
        let local = |r: &[[f64; 2]]| -> Vec<[f64; 2]> { r.iter().map(|p| frame.local2(*p)).collect() };
        let solid = self.extruded(&local(&outer), &holes.iter().map(|h| local(h)).collect::<Vec<_>>(), s.thickness, top);
        let layer = if roof { catalog::L_ROOF } else { catalog::L_FLOOR };
        let shape = self.shape("SweptSolid", &[solid], None, layer)?;
        let id =
            self.w.add(format!("IFCSLAB({},$,{},$,$,#{},#{shape},{},.{}.)", gid(&s.id), string(&up.name), placed.id, string(&s.id), up.predefined));
        self.slab_type(id, up);
        let usage = self.cat.usage(&mut self.w, up, "AXIS3", true, 0.0);
        self.cat.associate(&format!("usage:{}|axis3", up.name), usage, id);
        self.common("Pset_SlabCommon", id, &up.name, None, vec![("IsExternal", Val::Bool(roof)), ("LoadBearing", Val::Bool(true))]);
        let gross = area(&outer);
        let net = (gross - holes.iter().map(|h| area(h)).sum::<f64>()).max(0.0);
        let perimeter: f64 = (0..outer.len())
            .map(|i| {
                let (a, b) = (outer[i], outer[(i + 1) % outer.len()]);
                (b[0] - a[0]).hypot(b[1] - a[1])
            })
            .sum();
        let qs = [
            Qty::Length("Width", s.thickness),
            Qty::Length("Perimeter", perimeter),
            Qty::Area("GrossArea", gross / 1e6),
            Qty::Area("NetArea", net / 1e6),
            Qty::Volume("GrossVolume", gross * s.thickness / 1e9),
            Qty::Volume("NetVolume", net * s.thickness / 1e9),
        ];
        self.cat.quantities(&mut self.w, &s.id, "Qto_SlabBaseQuantities", &qs, id);
        Some(id)
    }

    fn slab_type(&mut self, id: usize, up: &catalog::BuildUp) {
        let key = format!("slab:{}", up.name);
        let created = !self.cat.types.contains_key(&key);
        let ty = self.cat.typed(&mut self.w, &key, id, |w| {
            w.add(format!("IFCSLABTYPE({},$,{},$,$,$,$,$,$,.{}.)", gid(&format!("slabtype:{}", up.name)), string(&up.name), up.predefined))
        });
        if created {
            let set = self.cat.layer_set(&mut self.w, up);
            self.cat.associate(&format!("set:{}", up.name), set, ty);
        }
    }

    /// A pitched roof: an IfcRoof of one slab per roof plane, its gables as walls and its
    /// gutters as gutter segments.
    pub(super) fn roof(&mut self, b: &Built, fl: &mut Floors) {
        let Some(r) = &b.roof else { return };
        let top = b.storeys.len().saturating_sub(1);
        let pieces = split::roof(r, &b.walls);
        let thickness = pieces.iter().find(|p| p.gable.is_none()).map_or(ROOF_PLANE, |p| p.thickness());
        let up = catalog::pitched_roof((thickness * 10.0).round() / 10.0);
        let placed = self.w.place(Some(fl.at(top)), fl.at(top).frame);
        let kind = if r.compound {
            ".MANSARD_ROOF."
        } else if r.gables.is_empty() {
            ".HIP_ROOF."
        } else {
            ".GABLE_ROOF."
        };
        // Mitred slope breaks have different top/soffit boundaries. Export the real shell,
        // rather than reconstructing it as unrelated plane extrusions with overlaps or gaps.
        let shape = if r.compound {
            self.w
                .faceset(&r.cover, &placed.frame)
                .and_then(|item| {
                    self.cat.paint(&mut self.w, item, catalog::TILES);
                    self.shape("Tessellation", &[item], None, catalog::L_ROOF)
                })
                .map_or_else(|| "$".into(), |id| format!("#{id}"))
        } else {
            "$".into()
        };
        let roof = self.w.add(format!(
            "IFCROOF({},$,{},$,$,#{},{shape},{},{kind})",
            gid(&r.id),
            string(&format!("Tag {:.0}°", r.pitch_deg)),
            placed.id,
            string(&r.id)
        ));
        let reference = if r.compound { "Original mansard shell - illustrative build-up; covering is separate" } else { &up.name };
        self.common("Pset_RoofCommon", roof, reference, None, vec![("IsExternal", Val::Bool(true))]);
        fl.contain(top, roof);
        let mut planes = Vec::new();
        for piece in &pieces {
            match &piece.gable {
                None => {
                    if let Some(id) = self.roof_plane(piece, &up, placed, r.pitch_deg) {
                        planes.push(id);
                    }
                }
                Some((wall, t)) => {
                    let Some(w) = b.walls.iter().find(|w| &w.id == wall) else { continue };
                    if let Some(id) = self.gable(piece, w, *t, fl, top) {
                        fl.contain(top, id);
                    }
                }
            }
        }
        if !planes.is_empty() {
            self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{roof},({}))", gid(&format!("{}.parts", r.id)), refs(&planes)));
        }
        for (key, mesh) in split::gutters(r) {
            let frame = fl.at(top).frame;
            let lp = self.w.place(Some(fl.at(top)), frame);
            let Some(item) = self.w.faceset(&mesh, &frame) else { continue };
            self.cat.paint(&mut self.w, item, catalog::ZINC);
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_ROOF) else { continue };
            let id = self.w.add(format!("IFCPIPESEGMENT({},$,'Tagrende',$,$,#{},#{shape},{},.GUTTER.)", gid(&key), lp.id, string(&key)));
            self.made_of(id, catalog::ZINC);
            fl.contain(top, id);
        }
        for (material, mesh) in &r.covering {
            let key = format!("{}.covering.{}", r.id, material.name());
            let frame = fl.at(top).frame;
            let lp = self.w.place(Some(fl.at(top)), frame);
            let Some(item) = self.w.faceset(mesh, &frame) else { continue };
            let mat = catalog::of(*material);
            self.cat.paint(&mut self.w, item, mat);
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_ROOF) else { continue };
            let id = self.w.add(format!(
                "IFCCOVERING({},$,{},$,$,#{},#{shape},{},.ROOFING.)",
                gid(&key),
                string(&format!("{} - original family defaults", material.name())),
                lp.id,
                string(&key)
            ));
            self.made_of(id, mat);
            fl.contain(top, id);
        }
        for d in &r.dormers {
            for (suffix, mesh) in [("roof", &d.roof), ("cheeks", &d.cheeks)] {
                let key = format!("{}.{suffix}", d.id);
                let frame = fl.at(top).frame;
                let lp = self.w.place(Some(fl.at(top)), frame);
                let Some(item) = self.w.faceset(mesh, &frame) else { continue };
                self.cat.paint(&mut self.w, item, catalog::ZINC);
                let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_ROOF) else { continue };
                let id = if suffix == "roof" {
                    self.w.add(format!(
                        "IFCROOF({},$,'Dormer shed roof - family default',$,$,#{},#{shape},{},.SHED_ROOF.)",
                        gid(&key),
                        lp.id,
                        string(&key)
                    ))
                } else {
                    self.w.add(format!(
                        "IFCCOVERING({},$,'Dormer cheek cladding - family default',$,$,#{},#{shape},{},.CLADDING.)",
                        gid(&key),
                        lp.id,
                        string(&key)
                    ))
                };
                fl.contain(top, id);
            }
        }
    }

    /// A roof plane: its underside outline in its own plane, extruded along its rise to the top
    /// face (straight up for a roof with a build-up, square to the plane for a bare surface).
    fn roof_plane(&mut self, piece: &split::RoofPiece, up: &catalog::BuildUp, parent: super::part21::Placed, pitch: f64) -> Option<usize> {
        let frame = split::plane_frame(&piece.polygon)?;
        let ring: Vec<[f64; 2]> = piece.polygon.iter().map(|p| frame.local(*p)).map(|q| [q[0], q[1]]).collect();
        let ring = ccw(clean_ring(&ring));
        let rise = frame.local_dir(piece.rise);
        let depth = (rise[0] * rise[0] + rise[1] * rise[1] + rise[2] * rise[2]).sqrt();
        if ring.len() < 3 || !(area(&ring) > 1.0) || !(depth > 1.0 && rise[2] > 0.0) {
            return None;
        }
        let placed = self.w.place(Some(parent), frame);
        // An extrusion only where it runs square to the plane: a built-up roof's planes rise
        // straight up (vertical fascias), which some readers (bimifc 0.1.0) extrude wrongly, and a
        // plane next to a gable is warped where the ridge was slid onto the gable. Those are
        // closed triangulated solids, as built.
        let square = rise[0].abs() + rise[1].abs() < 1e-6 * depth;
        let shape = if !square || piece.warp() > split::WARP {
            let item = self.w.faceset(&split::plane_solid(piece), &frame)?;
            self.cat.paint(&mut self.w, item, catalog::TILES);
            self.shape("Tessellation", &[item], None, catalog::L_ROOF)?
        } else {
            let profile = self.w.profile(&ring, &[]);
            let position = self.w.axis2([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
            let solid = self.w.extrusion(profile, position, depth);
            self.cat.paint(&mut self.w, solid, catalog::TILES);
            self.shape("SweptSolid", &[solid], None, catalog::L_ROOF)?
        };
        let id = self.w.add(format!("IFCSLAB({},$,{},$,$,#{},#{shape},{},.ROOF.)", gid(&piece.key), string(&up.name), placed.id, string(&piece.key)));
        self.slab_type(id, up);
        // Layers from the underside (the plane's z = 0) up.
        let usage = self.cat.usage(&mut self.w, up, "AXIS3", true, 0.0);
        self.cat.associate(&format!("usage:{}|axis3", up.name), usage, id);
        self.common(
            "Pset_SlabCommon",
            id,
            &up.name,
            None,
            vec![("IsExternal", Val::Bool(true)), ("LoadBearing", Val::Bool(true)), ("PitchAngle", Val::Angle(pitch.to_radians()))],
        );
        let (a, t) = (area(&ring), piece.thickness());
        let qs = [
            Qty::Length("Width", t),
            Qty::Area("GrossArea", a / 1e6),
            Qty::Area("NetArea", a / 1e6),
            Qty::Volume("GrossVolume", a * t / 1e9),
            Qty::Volume("NetVolume", a * t / 1e9),
        ];
        self.cat.quantities(&mut self.w, &piece.key, "Qto_SlabBaseQuantities", &qs, id);
        Some(id)
    }

    /// A gable: the triangle above the eaves in the facade's plane, as thick as the top storey's wall.
    fn gable(&mut self, piece: &split::RoofPiece, wall: &trigon_model::build::BuiltWall, t: f64, fl: &mut Floors, top: usize) -> Option<usize> {
        let frame = split::gable_frame(&piece.polygon, wall)?;
        // In the facade's elevation (x along it, z up); the profile's y is −z (see the position).
        let ring: Vec<[f64; 2]> = piece.polygon.iter().map(|p| frame.local(*p)).map(|q| [q[0], -q[2]]).collect();
        let ring = ccw(clean_ring(&ring));
        if ring.len() < 3 || !(area(&ring) > 1.0) || !(t > 0.0) {
            return None;
        }
        let up = catalog::facade(t, wall.kind == trigon_model::FacadeKind::Party, wall.finish);
        let placed = self.w.place(Some(fl.at(top)), frame);
        let profile = self.w.profile(&ring, &[]);
        // The profile stands in the x–z plane and is extruded into the building (+y).
        let position = self.w.axis2([0.0; 3], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]);
        let solid = self.w.extrusion(profile, position, t);
        self.cat.paint(&mut self.w, solid, up.face());
        let shape = self.shape("SweptSolid", &[solid], None, catalog::L_FACADE)?;
        let id = self.w.add(format!(
            "IFCWALL({},$,{},$,'Gavltrekant',#{},#{shape},{},.{}.)",
            gid(&piece.key),
            string(&up.name),
            placed.id,
            string(&piece.key),
            up.predefined
        ));
        let a = area(&ring);
        let (lo, hi) = piece.polygon.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| (lo.min(p[2]), hi.max(p[2])));
        let base = ring.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max) - ring.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min);
        let qs = [
            Qty::Length("Length", base),
            Qty::Length("Width", t),
            Qty::Length("Height", (hi - lo).max(0.0)),
            Qty::Area("GrossSideArea", a / 1e6),
            Qty::Area("NetSideArea", a / 1e6),
            Qty::Volume("GrossVolume", a * t / 1e9),
            Qty::Volume("NetVolume", a * t / 1e9),
        ];
        self.wall_data(id, &piece.key, &up, None, wall.kind != trigon_model::FacadeKind::Party, true, &qs);
        Some(id)
    }
}
