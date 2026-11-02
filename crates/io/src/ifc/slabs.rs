//! Floors and roofs. A floor is an `IfcSlab` extruded from its outline with the stair openings
//! as voids in the profile, so the hole is part of the slab's shape everywhere it is read (also
//! where the slab's outline takes a notch at the wall) and needs no boolean at import. A roof is
//! an `IfcRoof`: a flat roof of one slab; unopened pitched roofs use plane slabs, while opened
//! roofs retain their original cover with explicit solid-quantity status. Gables are walls.

use trigon_model::build::BuiltSlab;
use trigon_model::{Built, Mesh, SlabKind};

use super::catalog::{self, Qty, Val};
use super::part21::{Frame, area, ccw, clean_ring, cross, dot, gid, refs, string, sub};
use super::split::{self, ROOF_PLANE};
use super::{Floors, Ifc};

/// A floor this close to the terrain or below it rests on the ground (a basement or slab on grade), mm.
const ON_GROUND: f64 = 300.0;

#[cfg(test)]
#[path = "tests/roof_fragments.rs"]
mod tests;

/// The bounded fallback for a rejected profile: an unchanged triangular source cap and its
/// rise. Do not retriangulate or clean it. In particular, multiple cap triangles can overlap
/// or cancel; a positive sum of absolute triangle areas does not prove a valid cap.
fn roof_fragment_solid(piece: &split::RoofPiece, frame: &Frame) -> Option<Mesh> {
    let [cap] = piece.surface.triangles.as_slice() else { return None };
    if piece.polygon.len() != 3 || piece.surface.positions.len() != 3 {
        return None;
    }
    let cap = [
        *piece.surface.positions.get(cap[0] as usize)?,
        *piece.surface.positions.get(cap[1] as usize)?,
        *piece.surface.positions.get(cap[2] as usize)?,
    ];
    if !piece.polygon.iter().all(|p| cap.contains(p)) || piece.polygon.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return None;
    }
    let n = cross(sub(cap[1], cap[0]), sub(cap[2], cap[0]));
    let cap_area = dot(n, n).sqrt() / 2.0;
    if !cap_area.is_finite() || cap_area < 0.0005 || piece.warp() > 1e-6 {
        return None;
    }
    let mesh = split::plane_solid(piece);
    let local: Vec<_> = mesh.positions.iter().map(|p| frame.local(*p)).collect();
    if local.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return None;
    }
    // Match faceset's real() precision, also welding the duplicated side/cap vertices at
    // that precision. A shell must remain closed and outward after writing, not just before.
    let keys: Vec<_> = local.iter().map(|p| p.map(|v| (v * 1e6).round() as i64)).collect();
    let written: Vec<_> = keys.iter().map(|p| p.map(|v| v as f64 / 1e6)).collect();
    let mut edges = std::collections::BTreeMap::<_, (usize, i32)>::new();
    let (mut source_volume, mut written_volume) = (0.0, 0.0);
    for t in &mesh.triangles {
        let [a, b, c] = [*local.get(t[0] as usize)?, *local.get(t[1] as usize)?, *local.get(t[2] as usize)?];
        let [wa, wb, wc] = [*written.get(t[0] as usize)?, *written.get(t[1] as usize)?, *written.get(t[2] as usize)?];
        let alignment = dot(cross(sub(b, a), sub(c, a)), cross(sub(wb, wa), sub(wc, wa)));
        if !alignment.is_finite() || alignment <= 0.0 {
            return None;
        }
        source_volume += dot(a, cross(b, c)) / 6.0;
        written_volume += dot(wa, cross(wb, wc)) / 6.0;
        for k in 0..3 {
            let (a, b) = (*keys.get(t[k] as usize)?, *keys.get(t[(k + 1) % 3] as usize)?);
            let e = edges.entry((a.min(b), a.max(b))).or_default();
            e.0 += 1;
            e.1 += if a < b { 1 } else { -1 };
        }
    }
    // Require positive quantities at Qto's 0.001 mm² / 1 mm³ resolution, and at most 0.1% change
    // from coordinate rounding. Geometry below those limits stays explicitly uncovered.
    if edges.is_empty()
        || edges.values().any(|e| *e != (2, 0))
        || !source_volume.is_finite()
        || !written_volume.is_finite()
        || source_volume < 0.5
        || written_volume <= 0.0
        || (written_volume - source_volume).abs() > source_volume * 0.001
    {
        return None;
    }
    Some(mesh)
}

/// A closed shell's volume and complete boundary area at the writer's coordinate precision.
/// This is not an outer-roof-skin area: soffits, fascia and reveals are included in the area.
/// Check every edge-connected component, so an outward shell cannot hide an inverted one.
fn roof_shell_quantities(mesh: &Mesh, frame: &Frame) -> Option<(f64, f64)> {
    if mesh.is_empty() {
        return None;
    }
    let local: Vec<_> = mesh.positions.iter().map(|&p| frame.local(p)).collect();
    if local.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1e9) {
        return None;
    }
    let keys: Vec<_> = local.iter().map(|p| p.map(|v| (v * 1e6).round() as i64)).collect();
    let written: Vec<_> = keys.iter().map(|p| p.map(|v| v as f64 / 1e6)).collect();
    let mut edges = std::collections::BTreeMap::<_, (usize, i32, usize)>::new();
    let mut neighbours = vec![Vec::new(); mesh.triangles.len()];
    let (mut source_area, mut written_area) = (0., 0.);
    for (index, t) in mesh.triangles.iter().enumerate() {
        let [a, b, c] = [*local.get(t[0] as usize)?, *local.get(t[1] as usize)?, *local.get(t[2] as usize)?];
        let [wa, wb, wc] = [*written.get(t[0] as usize)?, *written.get(t[1] as usize)?, *written.get(t[2] as usize)?];
        let (n, wn) = (cross(sub(b, a), sub(c, a)), cross(sub(wb, wa), sub(wc, wa)));
        if !dot(n, wn).is_finite() || dot(n, wn) <= 0. {
            return None;
        }
        source_area += dot(n, n).sqrt() / 2.;
        written_area += dot(wn, wn).sqrt() / 2.;
        for k in 0..3 {
            let (a, b) = (*keys.get(t[k] as usize)?, *keys.get(t[(k + 1) % 3] as usize)?);
            let entry = edges.entry((a.min(b), a.max(b))).or_insert((0, 0, index));
            if entry.0 > 0 {
                neighbours[index].push(entry.2);
                neighbours[entry.2].push(index);
            }
            entry.0 += 1;
            entry.1 += if a < b { 1 } else { -1 };
        }
    }
    if edges.is_empty()
        || edges.values().any(|e| e.0 != 2 || e.1 != 0)
        || !source_area.is_finite()
        || !written_area.is_finite()
        || source_area <= 0.
        || (written_area - source_area).abs() > source_area * 0.001
    {
        return None;
    }
    let mut seen = vec![false; mesh.triangles.len()];
    let mut volume = 0.;
    for first in 0..mesh.triangles.len() {
        if seen[first] {
            continue;
        }
        let vertex = mesh.triangles[first][0] as usize;
        let (origin, written_origin) = (*local.get(vertex)?, *written.get(vertex)?);
        let (mut source_volume, mut written_volume) = (0., 0.);
        let mut pending = vec![first];
        while let Some(index) = pending.pop() {
            if seen[index] {
                continue;
            }
            seen[index] = true;
            let t = mesh.triangles[index];
            let [a, b, c] = t.map(|i| sub(local[i as usize], origin));
            let [wa, wb, wc] = t.map(|i| sub(written[i as usize], written_origin));
            source_volume += dot(a, cross(b, c)) / 6.;
            written_volume += dot(wa, cross(wb, wc)) / 6.;
            pending.extend(neighbours[index].iter().copied().filter(|&next| !seen[next]));
        }
        // Never use abs(): an inverted component is not a verified outward solid.
        if !source_volume.is_finite()
            || !written_volume.is_finite()
            || source_volume < 0.5
            || written_volume <= 0.
            || (written_volume - source_volume).abs() > source_volume * 0.001
        {
            return None;
        }
        volume += source_volume;
    }
    volume.is_finite().then_some((volume, source_area))
}

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

    /// A multi-plane shell has no single layer-usage axis. Preserve the illustrative
    /// construction on a proper roof type and layer set without inventing that orientation.
    fn roof_shell_metadata(&mut self, b: &Built, roof: usize, frame: &Frame, kind: &str, up: &catalog::BuildUp, represented: bool) {
        let Some(r) = &b.roof else { return };
        let mut props = vec![
            ("QuantityBasis", Val::Text("Original parametric cover only; boundary area includes soffits, fascia and reveals; separate coverings and dormer assemblies excluded".into())),
            ("OuterSkinAreas", Val::Text("Unavailable: no certified gross/net outer-skin partition is retained; no Qto_RoofBaseQuantities area is inferred from volume or upward reveals".into())),
            ("Representation", Val::Text(if represented { "Original parametric cover" } else { "Unavailable: empty or unrepresentable source coordinates/indices; no substitute geometry emitted" }.into())),
        ];
        if let Some(thickness) = split::source_roof_thickness(self.doc, b) {
            let key = format!("roof:{}:{kind}", up.name);
            let created = !self.cat.types.contains_key(&key);
            let ty = self.cat.typed(&mut self.w, &key, roof, |w| {
                w.add(format!("IFCROOFTYPE({},$,{},$,$,$,$,$,$,{kind})", gid(&format!("rooftype:{}:{kind}", up.name)), string(&up.name)))
            });
            let layers = self.cat.layer_set(&mut self.w, up);
            if created {
                self.cat.associate(&format!("roofset:{}", up.name), layers, ty);
            }
            self.cat.associate(&format!("roofset:{}", up.name), layers, roof);
            props.push(("SourceNormalThickness", Val::Length(thickness)));
            props.push(("ConstructionBasis", Val::Text("Illustrative family build-up at declared normal source thickness; layer positions within this multi-plane shell are not resolved".into())));
        } else {
            props.push((
                "ConstructionBasis",
                Val::Text("Unverified: no positive declared source build-up representable at written precision; no normal thickness or material-layer construction is inferred".into()),
            ));
        }
        if let Some((volume, boundary)) = roof_shell_quantities(&r.cover, frame) {
            props.push((
                "SolidCoverage",
                Val::Text(
                    "Verified closed, consistently oriented positive components at written precision; self-intersection is not certified".into(),
                ),
            ));
            self.cat.quantities(
                &mut self.w,
                &format!("{}.geometry", r.id),
                "Trigon_RoofQuantities",
                &[Qty::Volume("SolidVolume", volume / 1e9), Qty::Area("TotalBoundaryArea", boundary / 1e6)],
                roof,
            );
        } else {
            // Preserve the actual cover; never replace an unsupported source shell with
            // fictitious prisms or publish a successful solid-quantity certificate.
            props.push(("SolidCoverage", Val::Text("Unverified: source cover or written shell fails finite, closure, orientation, positive-volume or rounding checks; solid quantities omitted".into())));
        }
        self.cat.pset(&mut self.w, "Trigon_RoofGeometry", &props, roof);
    }

    /// A pitched roof: an IfcRoof with an exact opened cover or unopened plane slabs,
    /// its gables as walls and its gutters as gutter segments.
    pub(super) fn roof(&mut self, b: &Built, fl: &mut Floors) {
        let Some(r) = &b.roof else { return };
        let top = b.storeys.len().saturating_sub(1);
        let pieces = split::roof(r, &b.walls);
        let exact_openings = split::opening_shell(self.doc, b);
        let thickness = if r.compound || exact_openings {
            split::source_roof_thickness(self.doc, b)
                .unwrap_or_else(|| pieces.iter().find(|p| p.gable.is_none()).map_or(ROOF_PLANE, |p| p.thickness()))
        } else {
            pieces.iter().find(|p| p.gable.is_none()).map_or(ROOF_PLANE, |p| p.thickness())
        };
        let up = if (r.compound || exact_openings) && split::source_roof_thickness(self.doc, b).is_some() {
            let mut up = catalog::pitched_roof(thickness);
            // Catalog layer sets are cached by name. Distinct fractional source thicknesses
            // must not collapse into the legacy display name rounded to whole millimetres.
            let dimension = super::part21::real(thickness);
            up.name = format!("Tag, tegl på spær {} mm (illustrative shell)", dimension.trim_end_matches('.'));
            up
        } else {
            catalog::pitched_roof((thickness * 10.0).round() / 10.0)
        };
        let placed = self.w.place(Some(fl.at(top)), fl.at(top).frame);
        let half_hipped = self.doc.buildings.iter().any(|source| source.id == b.building_id && !source.roof.half_hips().is_empty());
        let kind = if half_hipped {
            ".HIPPED_GABLE_ROOF."
        } else if r.compound {
            ".MANSARD_ROOF."
        } else if r.gables.is_empty() {
            ".HIP_ROOF."
        } else {
            ".GABLE_ROOF."
        };
        // Mitred slope breaks have different top/soffit boundaries. Export the real shell,
        // rather than reconstructing it as unrelated plane extrusions with overlaps or gaps.
        let shape = if r.compound || exact_openings {
            // The generic number writer replaces nonfinite numbers with zero. An exact
            // roof must not claim that fabricated shape, or emit out-of-range face indices.
            let representable = r.cover.positions.iter().all(|&p| placed.frame.local(p).iter().all(|v| v.is_finite() && v.abs() <= 1e9))
                && r.cover.triangles.iter().flatten().all(|&i| i < u32::MAX && (i as usize) < r.cover.positions.len());
            representable
                .then(|| {
                    self.w.faceset(&r.cover, &placed.frame).and_then(|item| {
                        self.cat.paint(&mut self.w, item, catalog::TILES);
                        self.shape("Tessellation", &[item], None, catalog::L_ROOF)
                    })
                })
                .flatten()
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
        let reference = if half_hipped {
            "Original half-hipped shell - illustrative build-up"
        } else if r.compound {
            "Original mansard shell - illustrative build-up; covering is separate"
        } else if exact_openings {
            "Original pitched shell with exact parametric roof openings - illustrative build-up"
        } else {
            &up.name
        };
        self.common("Pset_RoofCommon", roof, reference, None, vec![("IsExternal", Val::Bool(true))]);
        if r.compound || exact_openings {
            self.roof_shell_metadata(b, roof, &placed.frame, kind, &up, shape != "$");
        }
        fl.contain(top, roof);
        self.rooflights(r, roof, fl);
        let mut planes = Vec::new();
        for piece in &pieces {
            match &piece.gable {
                None => {
                    // Openings split top and underside tessellations differently. The
                    // legacy centroid pairing / fallback extrusion cannot reproduce that shell.
                    if exact_openings {
                        continue;
                    }
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
        for gable in &r.gable_walls {
            if let Some(id) = self.hosted_gable(gable, fl) {
                fl.contain(gable.level, id);
            }
        }
        if !planes.is_empty() {
            self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{roof},({}))", gid(&format!("{}.parts", r.id)), refs(&planes)));
        }
        for (key, mesh, pipe) in
            split::gutters(r).into_iter().map(|(k, m)| (k, m, false)).chain(split::downpipes(r).into_iter().map(|(k, m)| (k, m, true)))
        {
            let frame = fl.at(top).frame;
            let lp = self.w.place(Some(fl.at(top)), frame);
            let Some(item) = self.w.faceset(&mesh, &frame) else { continue };
            self.cat.paint(&mut self.w, item, catalog::ZINC);
            let Some(shape) = self.shape("Tessellation", &[item], None, catalog::L_ROOF) else { continue };
            let kind = if pipe { ".RIGIDSEGMENT." } else { ".GUTTER." };
            let name = if pipe {
                "Downpipe assembly (geometric)"
            } else if r.drainage.is_empty() {
                "Tagrende"
            } else {
                "Half-round gutter"
            };
            let id =
                self.w.add(format!("IFCPIPESEGMENT({},$,{name},$,$,#{},#{shape},{},{kind})", gid(&key), lp.id, string(&key), name = string(name)));
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
                let common = if suffix == "roof" { "Pset_RoofCommon" } else { "Pset_CoveringCommon" };
                self.common(common, id, &key, None, vec![("IsExternal", Val::Bool(true))]);
                self.made_of(id, catalog::ZINC);
                fl.contain(top, id);
            }
        }
    }

    /// A roof plane: its underside outline in its own plane, extruded along its rise to the top
    /// face (straight up for a roof with a build-up, square to the plane for a bare surface).
    fn roof_plane(&mut self, piece: &split::RoofPiece, up: &catalog::BuildUp, parent: super::part21::Placed, pitch: f64) -> Option<usize> {
        let frame = split::plane_frame(&piece.polygon)?;
        let ring: Vec<[f64; 2]> = piece.polygon.iter().map(|p| frame.local(*p)).map(|q| [q[0], q[1]]).collect();
        let profile = ccw(clean_ring(&ring));
        let rise = frame.local_dir(piece.rise);
        let depth = (rise[0] * rise[0] + rise[1] * rise[1] + rise[2] * rise[2]).sqrt();
        if !depth.is_finite() || !(rise[2] > 0.0) {
            return None;
        }
        let profile_safe = profile.len() >= 3 && area(&profile) > 1.0 && depth > 1.0;
        let fallback = if profile_safe { None } else { Some(roof_fragment_solid(piece, &frame)?) };
        let placed = self.w.place(Some(parent), frame);
        // An extrusion only where it runs square to the plane: a built-up roof's planes rise
        // straight up (vertical fascias), which some readers (bimifc 0.1.0) extrude wrongly, and a
        // plane next to a gable is warped where the ridge was slid onto the gable. Those are
        // closed triangulated solids, as built.
        let square = rise[0].abs() + rise[1].abs() < 1e-6 * depth;
        let shape = if fallback.is_some() || !square || piece.warp() > split::WARP {
            let solid = match fallback {
                Some(solid) => solid,
                None => split::plane_solid(piece),
            };
            let item = self.w.faceset(&solid, &frame)?;
            self.cat.paint(&mut self.w, item, catalog::TILES);
            self.shape("Tessellation", &[item], None, catalog::L_ROOF)?
        } else {
            let profile = self.w.profile(&profile, &[]);
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
        let (a, t) = (area(if profile_safe { &profile } else { &ring }), piece.thickness());
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

    /// A real opened gable follows the roof underside through its mitred wall footprint.
    /// Its net solid is authoritative; a rectangular storey extrusion cannot describe it.
    fn hosted_gable(&mut self, g: &trigon_model::build::BuiltGable, fl: &mut Floors) -> Option<usize> {
        let wall = &g.front;
        if !g.thickness.is_finite() || g.thickness <= 0. {
            return None;
        }
        let frame = Frame::plan([wall.frame.origin[0], wall.frame.origin[1], wall.bottom], wall.frame.t)?;
        let placed = self.w.place(Some(fl.at(g.level)), frame);
        let up = catalog::facade(g.thickness, wall.kind == trigon_model::FacadeKind::Party, wall.finish);
        let representable = wall.mesh.positions.iter().all(|&p| frame.local(p).iter().all(|v| v.is_finite() && v.abs() <= 1e9))
            && wall.mesh.triangles.iter().flatten().all(|&i| (i as usize) < wall.mesh.positions.len());
        let definition = representable
            .then(|| {
                self.w.faceset(&wall.mesh, &frame).and_then(|item| {
                    self.cat.paint(&mut self.w, item, up.face());
                    self.shape("Tessellation", &[item], None, catalog::L_FACADE)
                })
            })
            .flatten();
        let shape = definition.map_or_else(|| "$".to_string(), |id| format!("#{id}"));
        let id = self.w.add(format!(
            "IFCWALL({},$,{},$,'Gavltrekant',#{},{shape},{},.{}.)",
            gid(&wall.id),
            string(&up.name),
            placed.id,
            string(&wall.id),
            up.predefined
        ));
        let certified =
            if definition.is_some() { roof_shell_quantities(&g.gross, &frame).zip(roof_shell_quantities(&wall.mesh, &frame)) } else { None };
        let mut qs = Vec::new();
        if let Some(((gross, _), (net, _))) = certified {
            let ring: Vec<_> = g.profile.iter().map(|p| [p[0], p[1] - wall.bottom]).collect();
            let gross_side = area(&ring).abs();
            // Phase overlays can contain overlapping nominal openings. Measure the
            // actual net front rather than double-counting their nominal areas.
            let net_side = wall
                .mesh
                .triangles
                .iter()
                .filter_map(|t| {
                    let [a, b, c] = [
                        frame.local(*wall.mesh.positions.get(t[0] as usize)?),
                        frame.local(*wall.mesh.positions.get(t[1] as usize)?),
                        frame.local(*wall.mesh.positions.get(t[2] as usize)?),
                    ];
                    [a, b, c].iter().all(|p| p[1].abs() <= 1e-6).then_some(((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() / 2.)
                })
                .sum::<f64>();
            let top = g.gross.positions.iter().map(|p| p[2]).fold(wall.bottom, f64::max);
            qs.extend([
                Qty::Length("Length", wall.frame.length),
                Qty::Length("Width", g.thickness),
                Qty::Length("Height", top - wall.bottom),
                Qty::Area("GrossSideArea", gross_side / 1e6),
                Qty::Area("NetSideArea", net_side / 1e6),
                Qty::Volume("GrossVolume", gross / 1e9),
                Qty::Volume("NetVolume", net / 1e9),
            ]);
        }
        self.wall_data(id, &wall.id, &up, None, wall.kind != trigon_model::FacadeKind::Party, true, &qs);
        self.cat.pset(&mut self.w,"Trigon_GableGeometry",&[
            ("SourceFacade",Val::Text(g.source_facade.clone())),
            ("Representation",Val::Text(if definition.is_some(){"Original opened gable solid below roof underside"}else{"Unavailable: unrepresentable source geometry; no substitute wall emitted"}.into())),
            ("SolidCoverage",Val::Text(if certified.is_some(){"Verified closed, consistently oriented positive gross/net components at written precision; self-intersection is not certified"}else{"Unverified gross/net shell; solid quantities omitted"}.into())),
        ],id);
        if definition.is_some() {
            self.gable_openings(g, id, placed, fl);
        }
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
