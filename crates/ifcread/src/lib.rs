//! IFC import through two independent readers, and a comparison between them.
//!
//! - **ifc-lite** (LTplus AG, MPL-2.0): the import reader. Openings are cut into their hosts.
//! - **bimifc** 0.1.0 (MPL-2.0): a second, independent reader for cross-checking. It does not
//!   cut openings, so host elements are compared by extent, not by triangle count.
//!
//! Both are used unmodified, so their MPL-2.0 terms cover only their own files. Everything this
//! crate returns is in millimetres in the file's own local frame (Z up).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;

use ifc_lite_core::{EntityDecoder, EntityScanner, GeoRefExtractor, GeoReference};
use rustc_hash::FxHashMap;

/// Largest file read, so a hostile file can't exhaust memory.
pub const MAX_FILE_BYTES: usize = 1 << 30;

/// Products that have a body but aren't physical elements.
const NOT_ELEMENTS: [&str; 9] = [
    "IFCOPENINGELEMENT",
    "IFCOPENINGSTANDARDCASE",
    "IFCSPACE",
    "IFCSITE",
    "IFCBUILDING",
    "IFCBUILDINGSTOREY",
    "IFCANNOTATION",
    "IFCVIRTUALELEMENT",
    "IFCGRID",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reader {
    IfcLite,
    Bimifc,
}

impl Reader {
    pub fn name(self) -> &'static str {
        match self {
            Reader::IfcLite => "ifc-lite 27.1",
            Reader::Bimifc => "bimifc 0.1.0",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn bounds(&self) -> Option<([f64; 3], [f64; 3])> {
        let first = *self.positions.first()?;
        Some(self.positions.iter().fold((first, first), |(mut lo, mut hi), p| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            (lo, hi)
        }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub id: u32,
    pub global_id: String,
    /// Upper-case IFC class, for example `IFCWINDOW`.
    pub class: String,
    pub name: String,
    pub mesh: Mesh,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub id: u32,
    pub class: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ReadResult {
    pub reader: Reader,
    pub schema: String,
    /// Metres per file length unit.
    pub unit_m: f64,
    pub voids_applied: bool,
    pub elements: Vec<Element>,
    pub failures: Vec<Failure>,
    /// The file's georeference (from ifc-lite; both readers share it).
    pub georef: Option<GeoReference>,
}

impl ReadResult {
    /// Map coordinates (CRS units, usually metres) of a point given in file-local millimetres.
    pub fn to_map(&self, p: [f64; 3]) -> Option<[f64; 3]> {
        let g = self.georef.as_ref()?;
        let to_file = 1.0 / (self.unit_m * 1000.0);
        let (e, n, h) = g.local_to_map(p[0] * to_file, p[1] * to_file, p[2] * to_file);
        Some([e, n, h])
    }

    pub fn counts(&self) -> BTreeMap<String, usize> {
        let mut c = BTreeMap::new();
        for e in &self.elements {
            *c.entry(e.class.clone()).or_insert(0) += 1;
        }
        c
    }

    pub fn triangles(&self) -> usize {
        self.elements.iter().map(|e| e.mesh.triangles.len()).sum()
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ReadError {
    #[error("{0}")]
    Invalid(String),
}

fn tris(indices: &[u32]) -> Vec<[u32; 3]> {
    indices.as_chunks::<3>().0.to_vec()
}

struct Scan {
    schema: String,
    products: Vec<(u32, String, usize, usize)>,
    voids: FxHashMap<u32, Vec<u32>>,
    types: Vec<(u32, ifc_lite_core::IfcType)>,
}

fn scan(text: &str, dec: &mut EntityDecoder) -> Result<Scan, ReadError> {
    if text.len() > MAX_FILE_BYTES {
        return Err(ReadError::Invalid(format!("the file is larger than {} MB", MAX_FILE_BYTES >> 20)));
    }
    let schema = ifc_lite_core::declared_schema_bounded(text.as_bytes())
        .ok_or_else(|| ReadError::Invalid("not an IFC file: no FILE_SCHEMA in the header".into()))?;
    let mut s = Scan { schema, products: Vec::new(), voids: FxHashMap::default(), types: Vec::new() };
    let mut scanner = EntityScanner::new(text);
    while let Some((id, ty, start, end)) = scanner.next_entity() {
        s.types.push((id, ifc_lite_core::ifc_type_from_keyword(ty)));
        if ty == "IFCRELVOIDSELEMENT" {
            if let Ok(e) = dec.decode_at(start, end)
                && let (Some(host), Some(opening)) = (e.get_ref(4), e.get_ref(5))
            {
                s.voids.entry(host).or_default().push(opening);
            }
        } else if ifc_lite_core::has_geometry_by_name(ty) && !NOT_ELEMENTS.contains(&ty) {
            s.products.push((id, ty.to_string(), start, end));
        }
    }
    Ok(s)
}

fn text_attr(e: &ifc_lite_core::DecodedEntity, i: usize) -> String {
    e.get(i).and_then(|a| a.as_string()).unwrap_or_default().to_string()
}

/// Reads with ifc-lite: every physical element, openings cut, in millimetres.
pub fn read_ifc_lite(text: &str) -> Result<ReadResult, ReadError> {
    let mut dec = EntityDecoder::new(text);
    let s = scan(text, &mut dec)?;
    let unit_m = dec.length_unit_scale();
    let georef = GeoRefExtractor::extract(&mut dec, &s.types).ok().flatten().filter(GeoReference::has_georef);
    let router = ifc_lite_geometry::GeometryRouter::with_units(text, &mut dec);
    let mut elements = Vec::with_capacity(s.products.len());
    let mut failures = Vec::new();
    for (id, class, start, end) in &s.products {
        let e = match dec.decode_at(*start, *end) {
            Ok(e) => e,
            Err(err) => {
                failures.push(Failure { id: *id, class: class.clone(), message: err.to_string() });
                continue;
            }
        };
        match router.process_element_with_voids(&e, &mut dec, &s.voids) {
            Ok(m) if m.positions.is_empty() => {}
            Ok(m) => {
                let o = m.origin;
                let positions = m
                    .positions
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|p| [(o[0] + f64::from(p[0])) * 1000.0, (o[1] + f64::from(p[1])) * 1000.0, (o[2] + f64::from(p[2])) * 1000.0])
                    .collect();
                elements.push(Element {
                    id: *id,
                    global_id: text_attr(&e, 0),
                    class: class.clone(),
                    name: text_attr(&e, 2),
                    mesh: Mesh { positions, triangles: tris(&m.indices) },
                });
            }
            Err(err) => failures.push(Failure { id: *id, class: class.clone(), message: err.to_string() }),
        }
    }
    Ok(ReadResult { reader: Reader::IfcLite, schema: s.schema, unit_m, voids_applied: true, elements, failures, georef })
}

/// Reads with bimifc 0.1.0: every physical element, openings not cut, in millimetres.
pub fn read_bimifc(text: &str) -> Result<ReadResult, ReadError> {
    // Units and georeference come from ifc-lite's header and unit readers: bimifc 0.1.0 returns
    // raw file coordinates.
    let mut dec = EntityDecoder::new(text);
    let s = scan(text, &mut dec)?;
    let unit_m = dec.length_unit_scale();
    let georef = GeoRefExtractor::extract(&mut dec, &s.types).ok().flatten().filter(GeoReference::has_georef);
    let model = bimifc_parser::parse(text).map_err(|e| ReadError::Invalid(format!("bimifc: {e}")))?;
    let resolver = model.resolver();
    let router = bimifc_geometry::GeometryRouter::with_default_processors();
    let to_mm = unit_m * 1000.0;
    let mut classes: Vec<&str> = s.products.iter().map(|p| p.1.as_str()).collect();
    classes.sort_unstable();
    classes.dedup();
    let mut elements = Vec::new();
    let mut failures = Vec::new();
    for class in classes {
        for e in resolver.find_by_type_name(class) {
            let id = e.id.0;
            let attr = |i: usize| e.get(i).and_then(|a| a.as_string()).unwrap_or_default().to_string();
            match router.process_element(&e, resolver) {
                Ok(m) if m.positions.is_empty() => {}
                Ok(m) => {
                    let positions = m
                        .positions
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|p| [f64::from(p[0]) * to_mm, f64::from(p[1]) * to_mm, f64::from(p[2]) * to_mm])
                        .collect();
                    elements.push(Element {
                        id,
                        global_id: attr(0),
                        class: class.to_string(),
                        name: attr(2),
                        mesh: Mesh { positions, triangles: tris(&m.indices) },
                    });
                }
                Err(err) => failures.push(Failure { id, class: class.to_string(), message: err.to_string() }),
            }
        }
    }
    Ok(ReadResult { reader: Reader::Bimifc, schema: s.schema, unit_m, voids_applied: false, elements, failures, georef })
}

/// How far two bounding boxes are apart: the largest corner distance along any axis, mm.
pub fn bounds_deviation(a: ([f64; 3], [f64; 3]), b: ([f64; 3], [f64; 3])) -> f64 {
    (0..3).map(|k| (a.0[k] - b.0[k]).abs().max((a.1[k] - b.1[k]).abs())).fold(0.0, f64::max)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Deviation {
    pub global_id: String,
    pub class: String,
    pub mm: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    /// Elements found by both readers (matched by GlobalId).
    pub matched: usize,
    pub only_first: Vec<String>,
    pub only_second: Vec<String>,
    /// Largest bounding-box deviation over matched elements, mm.
    pub max_bounds_mm: f64,
    /// The worst few matched elements.
    pub worst: Vec<Deviation>,
    /// Matched elements whose triangle counts are equal.
    pub same_triangles: usize,
}

/// Compares two reads element by element (GlobalId).
pub fn compare(a: &ReadResult, b: &ReadResult) -> Comparison {
    let by_id: BTreeMap<&str, &Element> = b.elements.iter().map(|e| (e.global_id.as_str(), e)).collect();
    let ids_a: std::collections::BTreeSet<&str> = a.elements.iter().map(|e| e.global_id.as_str()).collect();
    let mut devs = Vec::new();
    let mut same_triangles = 0;
    let mut only_first = Vec::new();
    for e in &a.elements {
        match by_id.get(e.global_id.as_str()) {
            Some(o) => {
                if e.mesh.triangles.len() == o.mesh.triangles.len() {
                    same_triangles += 1;
                }
                if let (Some(x), Some(y)) = (e.mesh.bounds(), o.mesh.bounds()) {
                    devs.push(Deviation { global_id: e.global_id.clone(), class: e.class.clone(), mm: bounds_deviation(x, y) });
                }
            }
            None => only_first.push(e.global_id.clone()),
        }
    }
    let only_second = b.elements.iter().filter(|e| !ids_a.contains(e.global_id.as_str())).map(|e| e.global_id.clone()).collect();
    devs.sort_by(|x, y| y.mm.total_cmp(&x.mm));
    Comparison {
        matched: devs.len(),
        only_first,
        only_second,
        max_bounds_mm: devs.first().map_or(0.0, |d| d.mm),
        worst: devs.into_iter().take(5).collect(),
        same_triangles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ifc_input_is_refused() {
        assert!(read_ifc_lite("hello").is_err());
        assert!(read_bimifc("").is_err());
    }

    #[test]
    fn bounds_deviation_is_the_worst_axis() {
        let a = ([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let b = ([0.5, 0.0, 0.0], [10.0, 12.0, 10.0]);
        assert_eq!(bounds_deviation(a, b), 2.0);
    }
}
