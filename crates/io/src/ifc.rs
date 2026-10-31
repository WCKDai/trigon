//! IFC4 (ADD2 TC1) export as an ISO 10303-21 file.
//!
//! Walls are swept solids (plan outline × height) voided by opening elements; windows and doors
//! fill the openings and carry their types; balcony slabs and railings are tessellated. The
//! site is georeferenced with `IfcMapConversion` to EPSG:25832, so local millimetres stay small.
//! GlobalIds are derived from Trigon's element ids, so the same element keeps its GlobalId
//! from one export to the next.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use trigon_model::build::Status;
use trigon_model::{Built, BuiltElement, ComponentKind, Document, ElementClass, Mesh, SlabKind};

const ALPHABET: &[u8; 64] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$";

/// A 22-character IFC GlobalId derived from `seed` (two FNV-1a 64-bit hashes).
pub fn guid(seed: &str) -> String {
    let fnv = |offset: u64| seed.bytes().fold(offset, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3));
    let x = (u128::from(fnv(0xcbf2_9ce4_8422_2325)) << 64) | u128::from(fnv(0x8422_2325_cbf2_9ce4));
    let mut s = String::with_capacity(22);
    s.push(char::from(ALPHABET[((x >> 126) & 3) as usize]));
    for i in 1..22 {
        s.push(char::from(ALPHABET[((x >> (126 - 6 * i)) & 63) as usize]));
    }
    s
}

/// A Part 21 string literal: quotes doubled, backslashes doubled, non-ASCII as `\X2\…\X0\`.
pub fn string(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            _ => {
                out.push_str("\\X2\\");
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "{unit:04X}");
                }
                out.push_str("\\X0\\");
            }
        }
    }
    out.push('\'');
    out
}

/// A Part 21 real: always has a decimal point, rounded to 1e-6.
pub fn real(x: f64) -> String {
    let x = if x.is_finite() { (x * 1e6).round() / 1e6 } else { 0.0 };
    if x == 0.0 {
        return "0.".to_string();
    }
    let s = format!("{x}");
    if s.contains('.') { s } else { format!("{s}.") }
}

struct W {
    lines: Vec<String>,
}

impl W {
    fn add(&mut self, entity: String) -> usize {
        self.lines.push(entity);
        self.lines.len()
    }

    fn point3(&mut self, p: [f64; 3]) -> usize {
        self.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(p[0]), real(p[1]), real(p[2])))
    }

    fn point2(&mut self, p: [f64; 2]) -> usize {
        self.add(format!("IFCCARTESIANPOINT(({},{}))", real(p[0]), real(p[1])))
    }

    fn dir(&mut self, d: [f64; 3]) -> usize {
        self.add(format!("IFCDIRECTION(({},{},{}))", real(d[0]), real(d[1]), real(d[2])))
    }

    fn polyline(&mut self, pts: &[[f64; 2]]) -> usize {
        let mut ids: Vec<usize> = pts.iter().map(|p| self.point2(*p)).collect();
        if let Some(&first) = ids.first() {
            ids.push(first);
        }
        self.add(format!("IFCPOLYLINE(({}))", refs(&ids)))
    }

    fn shape(&mut self, ctx: usize, kind: &str, items: &[usize]) -> usize {
        let rep = self.add(format!("IFCSHAPEREPRESENTATION(#{ctx},'Body','{kind}',({}))", refs(items)));
        self.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,(#{rep}))"))
    }

    fn faceset(&mut self, m: &Mesh) -> Option<usize> {
        if m.is_empty() {
            return None;
        }
        let coords: Vec<String> = m.positions.iter().map(|p| format!("({},{},{})", real(p[0]), real(p[1]), real(p[2]))).collect();
        let list = self.add(format!("IFCCARTESIANPOINTLIST3D(({}))", coords.join(",")));
        let tris: Vec<String> = m.triangles.iter().map(|t| format!("({},{},{})", t[0] + 1, t[1] + 1, t[2] + 1)).collect();
        Some(self.add(format!("IFCTRIANGULATEDFACESET(#{list},$,$,({}),$)", tris.join(","))))
    }
}

fn refs(ids: &[usize]) -> String {
    ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",")
}

/// Degrees as IfcCompoundPlaneAngleMeasure (degrees, minutes, seconds, millionths).
fn compound(deg: f64) -> String {
    let sign = if deg < 0.0 { -1 } else { 1 };
    let total = (deg.abs() * 3600.0 * 1e6).round() as i64;
    let (d, rem) = (total / 3_600_000_000, total % 3_600_000_000);
    let (m, rem) = (rem / 60_000_000, rem % 60_000_000);
    let (s, micro) = (rem / 1_000_000, rem % 1_000_000);
    format!("({},{},{},{})", sign * d, sign * m, sign * s, sign * micro)
}

fn window_partitioning(kind: &ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Window { lights: 1 } | ComponentKind::FixedWindow => ".SINGLE_PANEL.",
        ComponentKind::Window { lights: 2 } => ".DOUBLE_PANEL_VERTICAL.",
        ComponentKind::Window { lights: 3 } => ".TRIPLE_PANEL_VERTICAL.",
        _ => ".NOTDEFINED.",
    }
}

fn door_operation(kind: &ComponentKind) -> &'static str {
    match kind {
        ComponentKind::FrenchDoor { leaves: 2 } | ComponentKind::FrenchBalcony { leaves: 2, .. } | ComponentKind::Balcony { leaves: 2, .. } => {
            ".DOUBLE_DOOR_SINGLE_SWING."
        }
        ComponentKind::DoorWindow { door_side: trigon_model::component::Side::Right, .. } => ".SINGLE_SWING_RIGHT.",
        _ => ".SINGLE_SWING_LEFT.",
    }
}

/// Writes the document's buildings as IFC4. `timestamp` is ISO 8601 (for FILE_NAME).
pub fn write(doc: &Document, built: &[Built], file_name: &str, timestamp: &str) -> String {
    let mut w = W { lines: Vec::new() };
    let project_name = doc.site.as_ref().map_or("Trigon project", |s| s.name.as_str());

    let u_len = w.add("IFCSIUNIT(*,.LENGTHUNIT.,.MILLI.,.METRE.)".into());
    let u_area = w.add("IFCSIUNIT(*,.AREAUNIT.,$,.SQUARE_METRE.)".into());
    let u_vol = w.add("IFCSIUNIT(*,.VOLUMEUNIT.,$,.CUBIC_METRE.)".into());
    let u_ang = w.add("IFCSIUNIT(*,.PLANEANGLEUNIT.,$,.RADIAN.)".into());
    let units = w.add(format!("IFCUNITASSIGNMENT((#{u_len},#{u_area},#{u_vol},#{u_ang}))"));
    let origin = w.point3([0.0, 0.0, 0.0]);
    let dz = w.dir([0.0, 0.0, 1.0]);
    let wcs = w.add(format!("IFCAXIS2PLACEMENT3D(#{origin},$,$)"));
    let ctx = w.add(format!("IFCGEOMETRICREPRESENTATIONCONTEXT($,'Model',3,1.E-05,#{wcs},$)"));
    let body = w.add(format!("IFCGEOMETRICREPRESENTATIONSUBCONTEXT('Body','Model',*,*,*,*,#{ctx},$,.MODEL_VIEW.,$)"));
    let project = w.add(format!("IFCPROJECT({},$,{},$,$,$,$,(#{ctx}),#{units})", string(&guid("trigon:project")), string(project_name)));

    let site_lp = w.add(format!("IFCLOCALPLACEMENT($,#{wcs})"));
    let site = match &doc.site {
        Some(s) => {
            let map_unit = w.add("IFCSIUNIT(*,.LENGTHUNIT.,$,.METRE.)".into());
            let crs = w.add(format!("IFCPROJECTEDCRS('EPSG:25832','ETRS89 / UTM zone 32N','ETRS89','DVR90',$,$,#{map_unit})"));
            w.add(format!("IFCMAPCONVERSION(#{ctx},#{crs},{},{},{},1.,0.,0.001)", real(s.origin_utm[0]), real(s.origin_utm[1]), real(s.elevation)));
            w.add(format!(
                "IFCSITE({},$,{},$,$,#{site_lp},$,$,.ELEMENT.,{},{},{},$,$)",
                string(&guid("trigon:site")),
                string(&s.name),
                compound(s.origin.lat),
                compound(s.origin.lon),
                real(s.elevation * 1000.0)
            ))
        }
        None => w.add(format!("IFCSITE({},$,'Site',$,$,#{site_lp},$,$,.ELEMENT.,$,$,$,$,$)", string(&guid("trigon:site")))),
    };
    w.add(format!("IFCRELAGGREGATES({},$,$,$,#{project},(#{site}))", string(&guid("trigon:project-site"))));

    // Types used by windows and doors, declared once.
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    let mut typed: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut buildings = Vec::new();

    for b in built {
        let b_lp = w.add(format!("IFCLOCALPLACEMENT(#{site_lp},#{wcs})"));
        let building = w.add(format!(
            "IFCBUILDING({},$,{},$,$,#{b_lp},$,$,.ELEMENT.,$,$,$)",
            string(&guid(&format!("trigon:{}", b.building_id))),
            string(&b.name)
        ));
        buildings.push(building);
        let mut storeys = Vec::new();
        let mut contained: Vec<Vec<usize>> = vec![Vec::new(); b.storeys.len().max(1)];
        let mut storey_lps = Vec::new();
        for (i, s) in b.storeys.iter().enumerate() {
            let lp = w.add(format!("IFCLOCALPLACEMENT(#{b_lp},#{wcs})"));
            storey_lps.push(lp);
            storeys.push(w.add(format!(
                "IFCBUILDINGSTOREY({},$,{},$,$,#{lp},$,$,.ELEMENT.,{})",
                string(&guid(&format!("trigon:{}.storey.{i}", b.building_id))),
                string(&s.name),
                real(s.elevation)
            )));
        }
        let lp_of = |level: usize| storey_lps.get(level).or(storey_lps.first()).copied().unwrap_or(b_lp);

        for wall in &b.walls {
            let lp = w.add(format!("IFCLOCALPLACEMENT(#{},#{wcs})", lp_of(0)));
            let outline = w.polyline(&wall.plan);
            let profile = w.add(format!("IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#{outline})"));
            let solid = w.add(format!("IFCEXTRUDEDAREASOLID(#{profile},#{wcs},#{dz},{})", real(wall.height)));
            let shape = w.shape(body, "SweptSolid", &[solid]);
            let wall_id = w.add(format!(
                "IFCWALL({},$,{},$,$,#{lp},#{shape},{},.SOLIDWALL.)",
                string(&guid(&format!("trigon:{}", wall.id))),
                string(&format!("Facade {}", wall.id)),
                string(&wall.id)
            ));
            if let Some(c) = contained.first_mut() {
                c.push(wall_id);
            }
            let fr = wall.frame;
            for o in &wall.openings {
                let r = o.rect;
                let centre = w.point3(fr.point((r.u0 + r.u1) / 2.0, -50.0, (r.v0 + r.v1) / 2.0));
                let axis = w.dir([-fr.n[0], -fr.n[1], 0.0]);
                let refd = w.dir([fr.t[0], fr.t[1], 0.0]);
                let place = w.add(format!("IFCAXIS2PLACEMENT3D(#{centre},#{axis},#{refd})"));
                let rect = w.add(format!("IFCRECTANGLEPROFILEDEF(.AREA.,$,$,{},{})", real(r.u1 - r.u0), real(r.v1 - r.v0)));
                let solid = w.add(format!("IFCEXTRUDEDAREASOLID(#{rect},#{place},#{dz},{})", real(wall.thickness + 100.0)));
                let shape = w.shape(body, "SweptSolid", &[solid]);
                let olp = w.add(format!("IFCLOCALPLACEMENT(#{lp},#{wcs})"));
                let opening =
                    w.add(format!("IFCOPENINGELEMENT({},$,'Opening',$,$,#{olp},#{shape},$,.OPENING.)", string(&guid(&format!("trigon:{}", o.id)))));
                w.add(format!("IFCRELVOIDSELEMENT({},$,$,$,#{wall_id},#{opening})", string(&guid(&format!("trigon:{}.voids", o.id)))));

                let level = o.level.min(contained.len().saturating_sub(1));
                let fill = element(&mut w, body, wcs, lp_of(level), &o.fill, doc);
                w.add(format!("IFCRELFILLSELEMENT({},$,$,$,#{opening},#{fill})", string(&guid(&format!("trigon:{}.fills", o.id)))));
                if let Some(c) = contained.get_mut(level) {
                    c.push(fill);
                }
                typed.entry(o.fill.type_id.clone()).or_default().push(fill);
                if !types.contains_key(&o.fill.type_id)
                    && let Ok(t) = doc.component(&o.fill.type_id)
                {
                    {
                        let g = string(&guid(&format!("trigon:type:{}", t.id)));
                        let id = if t.kind.is_door() {
                            w.add(format!(
                                "IFCDOORTYPE({g},$,{},$,$,$,$,{},$,.DOOR.,{},.F.,$)",
                                string(&t.name),
                                string(&t.id),
                                door_operation(&t.kind)
                            ))
                        } else {
                            w.add(format!(
                                "IFCWINDOWTYPE({g},$,{},$,$,$,$,{},$,.WINDOW.,{},.F.,$)",
                                string(&t.name),
                                string(&t.id),
                                window_partitioning(&t.kind)
                            ))
                        };
                        types.insert(t.id.clone(), id);
                    }
                }
                for a in &o.attached {
                    let id = element(&mut w, body, wcs, lp_of(level), a, doc);
                    if let Some(c) = contained.get_mut(level) {
                        c.push(id);
                    }
                }
            }
            for a in &wall.attachments {
                let level = a.level.min(contained.len().saturating_sub(1));
                let id = element(&mut w, body, wcs, lp_of(level), a, doc);
                if let Some(c) = contained.get_mut(level) {
                    c.push(id);
                }
            }
        }

        for s in &b.slabs {
            let lp = w.add(format!("IFCLOCALPLACEMENT(#{},#{wcs})", lp_of(s.level)));
            let outline = w.polyline(&s.polygon);
            let profile = w.add(format!("IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#{outline})"));
            let base = w.point3([0.0, 0.0, s.z_bottom]);
            let place = w.add(format!("IFCAXIS2PLACEMENT3D(#{base},$,$)"));
            let solid = w.add(format!("IFCEXTRUDEDAREASOLID(#{profile},#{place},#{dz},{})", real(s.thickness)));
            let shape = w.shape(body, "SweptSolid", &[solid]);
            let (name, kind) = match s.kind {
                SlabKind::Floor => ("Floor slab", ".FLOOR."),
                SlabKind::Roof => ("Roof slab", ".ROOF."),
            };
            let id = w.add(format!(
                "IFCSLAB({},$,{},$,$,#{lp},#{shape},{},{kind})",
                string(&guid(&format!("trigon:{}", s.id))),
                string(name),
                string(&s.id)
            ));
            if let Some(c) = contained.get_mut(s.level) {
                c.push(id);
            }
        }

        if let Some(r) = &b.roof {
            let top = contained.len().saturating_sub(1);
            let lp = w.add(format!("IFCLOCALPLACEMENT(#{},#{wcs})", lp_of(top)));
            if let Some(fs) = w.faceset(&r.cover) {
                let shape = w.shape(body, "Tessellation", &[fs]);
                let id = w.add(format!(
                    "IFCSLAB({},$,{},$,$,#{lp},#{shape},{},.ROOF.)",
                    string(&guid(&format!("trigon:{}", r.id))),
                    string(&format!("Roof {:.0} deg", r.pitch_deg)),
                    string(&r.id)
                ));
                if let Some(c) = contained.get_mut(top) {
                    c.push(id);
                }
            }
            if let Some(fs) = w.faceset(&r.gables) {
                let shape = w.shape(body, "Tessellation", &[fs]);
                let id = w.add(format!(
                    "IFCWALL({},$,'Gables',$,$,#{lp},#{shape},{},.SOLIDWALL.)",
                    string(&guid(&format!("trigon:{}.gables", r.id))),
                    string(&format!("{}.gables", r.id))
                ));
                if let Some(c) = contained.get_mut(top) {
                    c.push(id);
                }
            }
        }
        w.add(format!("IFCRELAGGREGATES({},$,$,$,#{building},({}))", string(&guid(&format!("trigon:{}.storeys", b.building_id))), refs(&storeys)));
        for (i, (storey, elems)) in storeys.iter().zip(&contained).enumerate() {
            if !elems.is_empty() {
                w.add(format!(
                    "IFCRELCONTAINEDINSPATIALSTRUCTURE({},$,$,$,({}),#{storey})",
                    string(&guid(&format!("trigon:{}.storey.{i}.contains", b.building_id))),
                    refs(elems)
                ));
            }
        }
    }
    if !buildings.is_empty() {
        w.add(format!("IFCRELAGGREGATES({},$,$,$,#{site},({}))", string(&guid("trigon:site-buildings")), refs(&buildings)));
    }
    for (type_id, objects) in &typed {
        if let Some(t) = types.get(type_id) {
            w.add(format!("IFCRELDEFINESBYTYPE({},$,$,$,({}),#{t})", string(&guid(&format!("trigon:type:{type_id}.objects"))), refs(objects)));
        }
    }
    if !types.is_empty() {
        let ids: Vec<usize> = types.values().copied().collect();
        w.add(format!("IFCRELDECLARES({},$,$,$,#{project},({}))", string(&guid("trigon:declares")), refs(&ids)));
    }

    let mut out = String::new();
    out.push_str("ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION(('ViewDefinition [ReferenceView_V1.2]'),'2;1');\n");
    let version = concat!("Trigon ", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "FILE_NAME({},{},(''),(''),'{version}','{version}','');", string(file_name), string(timestamp));
    out.push_str("FILE_SCHEMA(('IFC4'));\nENDSEC;\nDATA;\n");
    for (i, l) in w.lines.iter().enumerate() {
        let _ = writeln!(out, "#{}={};", i + 1, l);
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}

/// A window, door, railing or balcony slab with a tessellated body.
fn element(w: &mut W, body: usize, wcs: usize, storey_lp: usize, e: &BuiltElement, doc: &Document) -> usize {
    let items: Vec<usize> = e.parts.iter().filter_map(|(_, m)| w.faceset(m)).collect();
    let shape = w.shape(body, "Tessellation", &items);
    let lp = w.add(format!("IFCLOCALPLACEMENT(#{storey_lp},#{wcs})"));
    let g = string(&guid(&format!("trigon:{}", e.id)));
    let name = string(&e.name);
    let tag = string(&e.id);
    let kind = doc.component(&e.type_id).map(|t| t.kind.clone()).ok();
    let id = match e.class {
        ElementClass::Window => w.add(format!(
            "IFCWINDOW({g},$,{name},$,$,#{lp},#{shape},{tag},{},{},.WINDOW.,{},$)",
            real(e.height),
            real(e.width),
            kind.as_ref().map_or(".NOTDEFINED.", window_partitioning)
        )),
        ElementClass::Door => w.add(format!(
            "IFCDOOR({g},$,{name},$,$,#{lp},#{shape},{tag},{},{},.DOOR.,{},$)",
            real(e.height),
            real(e.width),
            kind.as_ref().map_or(".NOTDEFINED.", door_operation)
        )),
        ElementClass::Railing => w.add(format!("IFCRAILING({g},$,{name},$,$,#{lp},#{shape},{tag},.GUARDRAIL.)")),
        ElementClass::BalconySlab => w.add(format!("IFCSLAB({g},$,{name},$,'Balcony',#{lp},#{shape},{tag},.USERDEFINED.)")),
    };
    // With phases, the IFC4 common property sets carry the element's status.
    if doc.phases.len() > 1 {
        let pset = match e.class {
            ElementClass::Window => "Pset_WindowCommon",
            ElementClass::Door => "Pset_DoorCommon",
            ElementClass::Railing => "Pset_RailingCommon",
            ElementClass::BalconySlab => "Pset_SlabCommon",
        };
        let value = match e.status {
            Status::Existing => "EXISTING",
            Status::New => "NEW",
            Status::Demolished => "DEMOLISH",
        };
        let prop = w.add(format!("IFCPROPERTYSINGLEVALUE('Status',$,IFCLABEL('{value}'),$)"));
        let ps = w.add(format!("IFCPROPERTYSET({},$,'{pset}',$,(#{prop}))", string(&guid(&format!("trigon:{}.status", e.id)))));
        w.add(format!("IFCRELDEFINESBYPROPERTIES({},$,$,$,(#{id}),#{ps})", string(&guid(&format!("trigon:{}.status.rel", e.id)))));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_are_22_chars_stable_and_distinct() {
        let a = guid("trigon:b1.f1");
        assert_eq!(a.len(), 22);
        assert!(matches!(a.as_bytes()[0], b'0'..=b'3'));
        assert_eq!(a, guid("trigon:b1.f1"));
        assert_ne!(a, guid("trigon:b1.f2"));
    }

    #[test]
    fn strings_and_reals_follow_part_21() {
        assert_eq!(string("Frederikssundsvej 28, 2400 København NV"), "'Frederikssundsvej 28, 2400 K\\X2\\00F8\\X0\\benhavn NV'");
        assert_eq!(string("it's"), "'it''s'");
        assert_eq!(real(1200.0), "1200.");
        assert_eq!(real(-0.0), "0.");
        assert_eq!(real(0.5), "0.5");
        assert_eq!(real(f64::NAN), "0.");
    }

    #[test]
    fn compound_angles() {
        assert_eq!(compound(55.5), "(55,30,0,0)");
        assert_eq!(compound(-12.25), "(-12,-15,0,0)");
    }
}
