//! ISO 10303-21 by hand: entity lines, literals, GlobalIds, and the geometry items every
//! element uses (points, directions, placements, polylines, profiles, extrusions, face sets).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use trigon_model::Mesh;

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

/// A Part 21 real for quantities (m², m³): rounded to 1e-9.
pub fn fine(x: f64) -> String {
    let x = if x.is_finite() { (x * 1e9).round() / 1e9 } else { 0.0 };
    if x == 0.0 {
        return "0.".to_string();
    }
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') { s } else { format!("{s}.") }
}

/// `#1,#2,…`
pub fn refs(ids: &[usize]) -> String {
    ids.iter().map(|i| format!("#{i}")).collect::<Vec<_>>().join(",")
}

/// A quoted GlobalId for a Trigon id.
pub fn gid(key: &str) -> String {
    string(&guid(&format!("trigon:{key}")))
}

/// Degrees as IfcCompoundPlaneAngleMeasure (degrees, minutes, seconds, millionths).
pub fn compound(deg: f64) -> String {
    let sign = if deg < 0.0 { -1 } else { 1 };
    let total = (deg.abs() * 3600.0 * 1e6).round() as i64;
    let (d, rem) = (total / 3_600_000_000, total % 3_600_000_000);
    let (m, rem) = (rem / 60_000_000, rem % 60_000_000);
    let (s, micro) = (rem / 1_000_000, rem % 1_000_000);
    format!("({},{},{},{})", sign * d, sign * m, sign * s, sign * micro)
}

pub fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = dot(v, v).sqrt();
    (l > 1e-9 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// A right-handed coordinate system in site mm: origin and unit axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub o: [f64; 3],
    pub x: [f64; 3],
    pub y: [f64; 3],
    pub z: [f64; 3],
}

impl Frame {
    /// The site's own axes at `o`.
    pub fn at(o: [f64; 3]) -> Frame {
        Frame { o, x: [1.0, 0.0, 0.0], y: [0.0, 1.0, 0.0], z: [0.0, 0.0, 1.0] }
    }

    /// Z up and x along the plan direction `dir`; y is to the left of `dir`.
    pub fn plan(o: [f64; 3], dir: [f64; 2]) -> Option<Frame> {
        Frame::new(o, [0.0, 0.0, 1.0], [dir[0], dir[1], 0.0])
    }

    /// Z along `z`, x along `x` made square to it.
    pub fn new(o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> Option<Frame> {
        let z = unit(z)?;
        let x = unit(sub(x, [z[0] * dot(x, z), z[1] * dot(x, z), z[2] * dot(x, z)]))?;
        Some(Frame { o, x, y: cross(z, x), z })
    }

    /// A site point in this frame's coordinates.
    pub fn local(&self, p: [f64; 3]) -> [f64; 3] {
        let d = sub(p, self.o);
        [dot(d, self.x), dot(d, self.y), dot(d, self.z)]
    }

    /// A site direction in this frame's coordinates.
    pub fn local_dir(&self, d: [f64; 3]) -> [f64; 3] {
        [dot(d, self.x), dot(d, self.y), dot(d, self.z)]
    }

    /// A point given in this frame's coordinates, in site coordinates.
    pub fn world(&self, p: [f64; 3]) -> [f64; 3] {
        let (x, y, z) = (self.x, self.y, self.z);
        [
            self.o[0] + x[0] * p[0] + y[0] * p[1] + z[0] * p[2],
            self.o[1] + x[1] * p[0] + y[1] * p[1] + z[1] * p[2],
            self.o[2] + x[2] * p[0] + y[2] * p[1] + z[2] * p[2],
        ]
    }

    /// The same axes with the origin moved to a point given in this frame.
    pub fn shifted(&self, p: [f64; 3]) -> Frame {
        Frame { o: self.world(p), ..*self }
    }

    /// A site point in this frame's plan (x, y).
    pub fn local2(&self, p: [f64; 2]) -> [f64; 2] {
        let q = self.local([p[0], p[1], self.o[2]]);
        [q[0], q[1]]
    }
}

/// An IfcLocalPlacement and the frame it puts its product in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub id: usize,
    pub frame: Frame,
}

/// Entity lines, numbered from 1 in the order they are added.
#[derive(Default)]
pub struct W {
    pub lines: Vec<String>,
    dirs: BTreeMap<String, usize>,
    origin3: Option<usize>,
}

impl W {
    pub fn add(&mut self, entity: String) -> usize {
        self.lines.push(entity);
        self.lines.len()
    }

    pub fn point3(&mut self, p: [f64; 3]) -> usize {
        if p == [0.0; 3] {
            if let Some(o) = self.origin3 {
                return o;
            }
            let o = self.add("IFCCARTESIANPOINT((0.,0.,0.))".into());
            self.origin3 = Some(o);
            return o;
        }
        self.add(format!("IFCCARTESIANPOINT(({},{},{}))", real(p[0]), real(p[1]), real(p[2])))
    }

    pub fn point2(&mut self, p: [f64; 2]) -> usize {
        self.add(format!("IFCCARTESIANPOINT(({},{}))", real(p[0]), real(p[1])))
    }

    /// An IfcDirection, shared between everything that points the same way.
    pub fn dir(&mut self, d: [f64; 3]) -> usize {
        let text = format!("IFCDIRECTION(({},{},{}))", real(d[0]), real(d[1]), real(d[2]));
        if let Some(&id) = self.dirs.get(&text) {
            return id;
        }
        let id = self.add(text.clone());
        self.dirs.insert(text, id);
        id
    }

    /// IfcAxis2Placement3D; the axes are left out when they are the parent's.
    pub fn axis2(&mut self, o: [f64; 3], z: [f64; 3], x: [f64; 3]) -> usize {
        let p = self.point3(o);
        let same = |a: [f64; 3], b: [f64; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-9);
        if same(z, [0.0, 0.0, 1.0]) && same(x, [1.0, 0.0, 0.0]) {
            return self.add(format!("IFCAXIS2PLACEMENT3D(#{p},$,$)"));
        }
        let (z, x) = (self.dir(z), self.dir(x));
        self.add(format!("IFCAXIS2PLACEMENT3D(#{p},#{z},#{x})"))
    }

    /// An IfcLocalPlacement of `frame` relative to `parent`.
    pub fn place(&mut self, parent: Option<Placed>, frame: Frame) -> Placed {
        let (rel, o, z, x) = match parent {
            Some(p) => (format!("#{}", p.id), p.frame.local(frame.o), p.frame.local_dir(frame.z), p.frame.local_dir(frame.x)),
            None => ("$".to_string(), frame.o, frame.z, frame.x),
        };
        let a = self.axis2(o, z, x);
        Placed { id: self.add(format!("IFCLOCALPLACEMENT({rel},#{a})")), frame }
    }

    /// An IfcPolyline through 2D points; closed repeats the first point at the end.
    pub fn polyline(&mut self, pts: &[[f64; 2]], closed: bool) -> usize {
        let mut ids: Vec<usize> = pts.iter().map(|p| self.point2(*p)).collect();
        if closed && let Some(&first) = ids.first() {
            ids.push(first);
        }
        self.add(format!("IFCPOLYLINE(({}))", refs(&ids)))
    }

    /// An area profile: the outline, and holes if there are any.
    pub fn profile(&mut self, outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>]) -> usize {
        let o = self.polyline(outer, true);
        if holes.is_empty() {
            return self.add(format!("IFCARBITRARYCLOSEDPROFILEDEF(.AREA.,$,#{o})"));
        }
        let inner: Vec<usize> = holes.iter().map(|h| self.polyline(h, true)).collect();
        self.add(format!("IFCARBITRARYPROFILEDEFWITHVOIDS(.AREA.,$,#{o},({}))", refs(&inner)))
    }

    /// A profile extruded `depth` along the z axis of `position` (a placement in the product's frame).
    pub fn extrusion(&mut self, profile: usize, position: usize, depth: f64) -> usize {
        let z = self.dir([0.0, 0.0, 1.0]);
        self.add(format!("IFCEXTRUDEDAREASOLID(#{profile},#{position},#{z},{})", real(depth)))
    }

    /// A mesh as an IfcTriangulatedFaceSet in `frame`'s coordinates.
    pub fn faceset(&mut self, m: &Mesh, frame: &Frame) -> Option<usize> {
        if m.is_empty() {
            return None;
        }
        let coords: Vec<String> = m
            .positions
            .iter()
            .map(|p| {
                let q = frame.local(*p);
                format!("({},{},{})", real(q[0]), real(q[1]), real(q[2]))
            })
            .collect();
        let list = self.add(format!("IFCCARTESIANPOINTLIST3D(({}))", coords.join(",")));
        let tris: Vec<String> = m.triangles.iter().map(|t| format!("({},{},{})", t[0] + 1, t[1] + 1, t[2] + 1)).collect();
        Some(self.add(format!("IFCTRIANGULATEDFACESET(#{list},$,$,({}),$)", tris.join(","))))
    }

    /// An IfcShapeRepresentation in `context`.
    pub fn representation(&mut self, context: usize, identifier: &str, kind: &str, items: &[usize]) -> usize {
        self.add(format!("IFCSHAPEREPRESENTATION(#{context},'{identifier}','{kind}',({}))", refs(items)))
    }

    /// An IfcProductDefinitionShape of representations.
    pub fn product_shape(&mut self, reps: &[usize]) -> usize {
        self.add(format!("IFCPRODUCTDEFINITIONSHAPE($,$,({}))", refs(reps)))
    }
}

/// Volume of a closed, consistently oriented mesh (divergence theorem), mm³.
#[cfg(test)]
pub fn volume(m: &Mesh) -> f64 {
    let v: f64 = m
        .triangles
        .iter()
        .map(|t| {
            let p = |i: u32| m.positions.get(i as usize).copied().unwrap_or_default();
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            dot(a, cross(b, c)) / 6.0
        })
        .sum();
    v.abs()
}

/// Area of a plan polygon, mm².
pub fn area(p: &[[f64; 2]]) -> f64 {
    trigon_model::doc::polygon_area(p)
}

/// A plan polygon without repeated corners (consecutive, or the first repeated at the end).
pub fn clean_ring(p: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(p.len());
    for q in p {
        if !q.iter().all(|v| v.is_finite()) {
            continue;
        }
        if out.last().is_none_or(|l| (l[0] - q[0]).hypot(l[1] - q[1]) > 0.01) {
            out.push(*q);
        }
    }
    while out.len() > 1 && out.first().zip(out.last()).is_some_and(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) <= 0.01) {
        out.pop();
    }
    out
}

/// The polygon counter-clockwise.
pub fn ccw(mut p: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if trigon_model::footprint::signed_area2(&p) < 0.0 {
        p.reverse();
    }
    p
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

    #[test]
    fn frames_round_trip_and_stay_right_handed() {
        let f = Frame::plan([1000.0, 2000.0, 300.0], [3.0, 4.0]).unwrap();
        let p = [1234.0, -567.0, 89.0];
        let q = f.world(f.local(p));
        assert!((0..3).all(|k| (p[k] - q[k]).abs() < 1e-9));
        assert!((0..3).all(|k| (cross(f.x, f.y)[k] - f.z[k]).abs() < 1e-12));
        // Left of the direction is +y.
        assert!(f.local([1000.0 - 4.0, 2000.0 + 3.0, 300.0])[1] > 0.0);
        assert!(Frame::plan([0.0; 3], [0.0, 0.0]).is_none());
        assert!(Frame::plan([0.0; 3], [f64::NAN, 1.0]).is_none());
    }

    #[test]
    fn rings_lose_repeated_corners() {
        let r = clean_ring(&[[0.0, 0.0], [0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 0.0]]);
        assert_eq!(r, vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]);
        assert!(trigon_model::footprint::signed_area2(&ccw(vec![[0.0, 0.0], [0.0, 1.0], [1.0, 0.0]])) > 0.0);
    }
}
