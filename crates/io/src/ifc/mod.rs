//! IFC4 (ADD2 TC1) export as an ISO 10303-21 file, for import as building elements in Revit,
//! Vectorworks and other BIM tools.
//!
//! - Walls are `IfcWallStandardCase` (one per facade and storey, one per interior wall and
//!   storey) with an Axis and a vertical SweptSolid in the wall's own placement, a material
//!   layer set usage and a wall type per build-up; wedge-shaped pieces fall back to `IfcWall`.
//!   Joined walls are connected with `IfcRelConnectsPathElements`.
//! - Openings void the wall pieces they cut; windows and doors fill them, placed in the opening.
//! - Floors are `IfcSlab` with stair openings in their profile; a pitched roof is an `IfcRoof`
//!   of roof slabs, its gables are walls; stairs are `IfcStair` of flights and landings; rooms
//!   are `IfcSpace`; balconies are `IfcElementAssembly` of slab, railing and steel members.
//! - Materials carry colours, and every shape is on a presentation layer per category.
//!
//! Lengths are millimetres. The site is georeferenced with `IfcMapConversion` to EPSG:25832,
//! so local millimetres stay small. GlobalIds are derived from Trigon's element ids, so the same
//! element keeps its GlobalId from one export to the next. Written from the public
//! specifications; no other implementation is linked or read.

mod balconies;
mod catalog;
mod fills;
mod part21;
pub mod sample;
mod slabs;
mod spaces;
mod split;
mod stairs;
mod walls;

use std::fmt::Write as _;

use trigon_model::build::Status;
use trigon_model::{Built, Document, Material, Mesh};

use catalog::{Catalog, Mat, Val};
use part21::{Frame, Placed, W, compound, gid, real, refs};

pub use part21::{guid, string};
pub use split::model_shapes;

/// Writes the document's buildings as IFC4. `timestamp` is ISO 8601 (for FILE_NAME).
pub fn write(doc: &Document, built: &[Built], file_name: &str, timestamp: &str) -> String {
    let mut x = Ifc::new(doc);
    let project = x.project();
    let site = x.site(project);
    for terrain in built.iter().filter_map(|b| b.terrain.as_ref()) {
        x.terrain(terrain, site);
    }
    let mut buildings = Vec::new();
    for b in built {
        buildings.push(x.building(b, site));
    }
    if !buildings.is_empty() {
        x.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{},({}))", gid("site-buildings"), site.0, refs(&buildings)));
    }
    x.cat.finish(&mut x.w, project);

    let mut out = String::new();
    // Openings are subtracted at import and walls carry layer set usages: Design Transfer View.
    out.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [DesignTransferView_V1.0]'),'2;1');\n");
    let version = concat!("Trigon ", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "FILE_NAME({},{},(''),(''),'{version}','{version}','');", string(file_name), string(timestamp));
    out.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, l) in x.w.lines.iter().enumerate() {
        let _ = writeln!(out, "#{}={};", i + 1, l);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}

/// The writer's state for one export.
pub(crate) struct Ifc<'a> {
    w: W,
    cat: Catalog,
    doc: &'a Document,
    /// Representation contexts: the model, its Body and Axis subcontexts.
    model: usize,
    body: usize,
    axis: usize,
    /// The document has more than one phase: elements carry Pset_*Common.Status.
    phases: bool,
}

/// A building's storeys: their placements and what each contains.
pub(crate) struct Floors {
    building: Placed,
    placed: Vec<Placed>,
    contained: Vec<Vec<usize>>,
    spaces: Vec<Vec<usize>>,
}

impl Floors {
    fn level(&self, l: usize) -> usize {
        l.min(self.placed.len().saturating_sub(1))
    }

    /// The storey's placement (the building's if it has no storeys).
    fn at(&self, l: usize) -> Placed {
        self.placed.get(self.level(l)).copied().unwrap_or(self.building)
    }

    fn contain(&mut self, l: usize, id: usize) {
        let k = self.level(l);
        if let Some(c) = self.contained.get_mut(k) {
            c.push(id);
        }
    }
}

impl<'a> Ifc<'a> {
    fn terrain(&mut self, mesh: &Mesh, site: (usize, Placed)) {
        let placed = self.w.place(Some(site.1), Frame::at([0.; 3]));
        let Some(item) = self.w.faceset(mesh, &placed.frame) else { return };
        self.cat.paint(&mut self.w, item, catalog::GROUND);
        let Some(shape) = self.shape("Tessellation", &[item], None, "L-Terrain") else { return };
        let id = self.w.add(format!(
            "IFCGEOGRAPHICELEMENT({},$,'Terrain surface',$,$,#{},#{shape},'site.terrain',.TERRAIN.)",
            gid("site.terrain"),
            placed.id
        ));
        self.w.add(format!("IFCRELCONTAINEDINSPATIALSTRUCTURE({},$,$,$,(#{id}),#{})", gid("site.terrain.contains"), site.0));
        self.common("Pset_GeographicElementCommon", id, "Terrain surface", None, vec![("IsExternal", catalog::Val::Bool(true))]);
        self.made_of(id, catalog::GROUND);
    }

    fn new(doc: &'a Document) -> Self {
        let mut w = W::default();
        let wcs = w.axis2([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let model = w.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{wcs},$)"));
        let body = w.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#{model},$,.MODEL_VIEW.,$)"));
        let axis = w.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Axis','Model',*,*,*,*,#{model},$,.GRAPH_VIEW.,$)"));
        let mut cat = Catalog::default();
        cat.context = body;
        Ifc { w, cat, doc, model, body, axis, phases: doc.phases.len() > 1 }
    }

    fn project(&mut self) -> usize {
        let w = &mut self.w;
        let u_len = w.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into());
        let u_area = w.add("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)".into());
        let u_vol = w.add("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)".into());
        let u_ang = w.add("IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)".into());
        let units = w.add(format!("IFCUNITASSIGNMENT((#{u_len},#{u_area},#{u_vol},#{u_ang}))"));
        let name = self.doc.site.as_ref().map_or("Trigon project", |s| s.name.as_str());
        w.add(format!("IFCPROJECT({},$,{},$,$,$,$,(#{}),#{units})", gid("project"), string(name), self.model))
    }

    /// The site, georeferenced when the document has one. Returns its entity and placement.
    fn site(&mut self, project: usize) -> (usize, Placed) {
        let placed = self.w.place(None, Frame::at([0.0; 3]));
        let w = &mut self.w;
        let site = match &self.doc.site {
            Some(s) => {
                let map_unit = w.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)".into());
                let crs = w.add(format!("IFCPROJECTEDCRS('EPSG:25832','ETRS89 / UTM zone 32N','ETRS89','DVR90',$,$,#{map_unit})"));
                w.add(format!(
                    "IFCMAPCONVERSION(#{},#{crs},{},{},{},1.,0.,0.001)",
                    self.model,
                    real(s.origin_utm[0]),
                    real(s.origin_utm[1]),
                    real(s.elevation)
                ));
                w.add(format!(
                    "IFCSITE({},$,{},$,$,#{},$,$,.ELEMENT.,{},{},{},$,$)",
                    gid("site"),
                    string(&s.name),
                    placed.id,
                    compound(s.origin.lat),
                    compound(s.origin.lon),
                    real(s.elevation * 1000.0)
                ))
            }
            None => w.add(format!("IFCSITE({},$,'Site',$,$,#{},$,$,.ELEMENT.,$,$,$,$,$)", gid("site"), placed.id)),
        };
        w.add(format!("IFCRELAGGREGATES({},$,$,$,#{project},(#{site}))", gid("project-site")));
        (site, placed)
    }

    /// A building, its storeys and everything on them.
    fn building(&mut self, b: &Built, (_, site): (usize, Placed)) -> usize {
        let placed = self.w.place(Some(site), Frame::at([0.0; 3]));
        let building = self.w.add(format!("IFCBUILDING({},$,{},$,$,#{},$,$,.ELEMENT.,$,$,$)", gid(&b.building_id), string(&b.name), placed.id));
        let mut fl =
            Floors { building: placed, placed: Vec::new(), contained: vec![Vec::new(); b.storeys.len()], spaces: vec![Vec::new(); b.storeys.len()] };
        let mut storeys = Vec::new();
        for (i, s) in b.storeys.iter().enumerate() {
            let p = self.w.place(Some(placed), Frame::at([0.0, 0.0, s.elevation]));
            fl.placed.push(p);
            storeys.push(self.w.add(format!(
                "IFCBUILDINGSTOREY({},$,{},$,$,#{},$,$,.ELEMENT.,{})",
                gid(&format!("{}.storey.{i}", b.building_id)),
                string(&s.name),
                p.id,
                real(s.elevation)
            )));
        }
        let doc = self.doc;
        let bd = doc.buildings.iter().find(|x| x.id == b.building_id);

        let pieces = self.walls(b, bd, &mut fl);
        for wall in &b.walls {
            let mine: Vec<&walls::Piece> = pieces.iter().filter(|p| p.wall == wall.id).collect();
            self.openings(wall, &mine, &mut fl);
            self.wall_balconies(wall, &mut fl);
        }
        self.slabs(b, &mut fl);
        self.roof(b, &mut fl);
        if let Some(bd) = bd {
            self.stairs(b, bd, &mut fl);
            self.spaces(bd, &mut fl);
        }

        self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{building},({}))", gid(&format!("{}.storeys", b.building_id)), refs(&storeys)));
        for (i, storey) in storeys.iter().enumerate() {
            if let Some(elems) = fl.contained.get(i).filter(|e| !e.is_empty()) {
                self.w.add(format!(
                    "IFCRELCONTAINEDINSPATIALSTRUCTURE({},$,$,$,({}),#{storey})",
                    gid(&format!("{}.storey.{i}.contains", b.building_id)),
                    refs(elems)
                ));
            }
            if let Some(spaces) = fl.spaces.get(i).filter(|e| !e.is_empty()) {
                self.w.add(format!("IFCRELAGGREGATES({},$,$,$,#{storey},({}))", gid(&format!("{}.storey.{i}.spaces", b.building_id)), refs(spaces)));
            }
        }
        building
    }

    /// A product's shape: an optional Axis and the Body, the Body on a presentation layer.
    fn shape(&mut self, body_kind: &str, items: &[usize], axis: Option<usize>, layer: &'static str) -> Option<usize> {
        if items.is_empty() {
            return None;
        }
        let mut reps = Vec::with_capacity(2);
        if let Some(a) = axis {
            reps.push(self.w.representation(self.axis, "Axis", "Curve2D", &[a]));
        }
        let body = self.w.representation(self.body, "Body", body_kind, items);
        self.cat.layer(layer, body);
        reps.push(body);
        Some(self.w.product_shape(&reps))
    }

    /// Tessellated parts in `frame`'s coordinates, each in its material's colour.
    fn tessellated(&mut self, parts: &[(Material, Mesh)], frame: &Frame) -> Vec<usize> {
        let mut items = Vec::with_capacity(parts.len());
        for (m, mesh) in parts {
            if let Some(item) = self.w.faceset(mesh, frame) {
                self.cat.paint(&mut self.w, item, catalog::of(*m));
                items.push(item);
            }
        }
        items
    }

    /// An extruded profile in its product's frame, in a material's colour.
    fn extruded(&mut self, outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>], depth: f64, mat: Mat) -> usize {
        let profile = self.w.profile(outer, holes);
        let position = self.w.axis2([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let solid = self.w.extrusion(profile, position, depth);
        self.cat.paint(&mut self.w, solid, mat);
        solid
    }

    /// The element's Pset_*Common: reference (its type's name) and the given properties, plus
    /// its phase status when the document has phases (elements Trigon doesn't phase stand in
    /// every phase, so they are existing).
    fn common(&mut self, pset: &str, object: usize, reference: &str, status: Option<Status>, mut props: Vec<(&str, Val)>) {
        props.insert(0, ("Reference", Val::Id(reference.to_string())));
        if self.phases {
            props.push(("Status", Val::Status(status.unwrap_or(Status::Existing))));
        }
        self.cat.pset(&mut self.w, pset, &props, object);
    }

    /// Associates a single material with an element.
    fn made_of(&mut self, object: usize, m: Mat) {
        let id = self.cat.material(&mut self.w, m);
        self.cat.associate(&format!("mat:{}", m.name), id, object);
    }
}

#[cfg(test)]
mod tests;
