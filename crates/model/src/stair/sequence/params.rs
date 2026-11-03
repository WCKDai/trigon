//! Strict, bounded input. Tagged alternatives deserialize directly into typed fields;
//! no intermediate JSON Value or serde tagged-content buffer can erase duplicate keys
//! or allocate an unbounded tread collection before its limit is checked.
use super::{MAX_JSON_BYTES, invalid};
use crate::{Result, checked};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
};
use std::{fmt, marker::PhantomData};

pub type Point = [f64; 2];
pub type Gate = [Point; 2];

fn bounded<'de, D, T, const MIN: usize, const MAX: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T, const MIN: usize, const MAX: usize>(PhantomData<T>);
    struct Refuse;
    impl<'de> DeserializeSeed<'de> for Refuse {
        type Value = ();
        fn deserialize<D: Deserializer<'de>>(self, _: D) -> std::result::Result<(), D::Error> {
            Err(de::Error::custom("collection exceeds its limit"))
        }
    }
    impl<'de, T: Deserialize<'de>, const MIN: usize, const MAX: usize> Visitor<'de> for Items<T, MIN, MAX> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{MIN}..={MAX} items")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
            let mut items = Vec::new();
            while items.len() < MAX {
                match seq.next_element()? {
                    Some(item) => items.push(item),
                    None => {
                        if items.len() < MIN {
                            return Err(de::Error::custom("collection below its minimum"));
                        }
                        return Ok(items);
                    }
                }
            }
            // Reject before visiting/allocating the first excess element.
            seq.next_element_seed(Refuse)?;
            Ok(items)
        }
    }
    d.deserialize_seq(Items::<T, MIN, MAX>(PhantomData))
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> std::result::Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}
fn present_bounded<'de, D: Deserializer<'de>, T: Deserialize<'de>, const MIN: usize, const MAX: usize>(
    d: D,
) -> std::result::Result<Option<Vec<T>>, D::Error> {
    bounded::<D, T, MIN, MAX>(d).map(Some)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SequenceKind {
    #[serde(rename = "explicit_sequence_v1")]
    ExplicitSequenceV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "SequenceInput")]
pub struct SequenceV1 {
    pub kind: SequenceKind,
    pub id: String,
    pub name: String,
    pub landings: Vec<Landing>,
    pub flights: Vec<Flight>,
    pub openings: Vec<Opening>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SequenceInput {
    kind: SequenceKind,
    id: String,
    name: String,
    #[serde(deserialize_with = "bounded::<_, _, 2, 33>")]
    landings: Vec<Landing>,
    #[serde(deserialize_with = "bounded::<_, _, 1, 32>")]
    flights: Vec<Flight>,
    #[serde(deserialize_with = "bounded::<_, _, 0, 64>")]
    openings: Vec<Opening>,
}
impl TryFrom<SequenceInput> for SequenceV1 {
    type Error = crate::ModelError;
    fn try_from(v: SequenceInput) -> Result<Self> {
        let s = Self { kind: v.kind, id: v.id, name: v.name, landings: v.landings, flights: v.flights, openings: v.openings };
        s.check_bounds()?;
        Ok(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "FloorInput")]
pub enum FloorRef {
    Storey { level: usize },
    Attic,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FloorKind {
    Storey,
    Attic,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FloorInput {
    kind: FloorKind,
    #[serde(default, deserialize_with = "present")]
    level: Option<usize>,
}
impl TryFrom<FloorInput> for FloorRef {
    type Error = &'static str;
    fn try_from(v: FloorInput) -> std::result::Result<Self, Self::Error> {
        match (v.kind, v.level) {
            (FloorKind::Storey, Some(level @ 0..=59)) => Ok(Self::Storey { level }),
            (FloorKind::Attic, None) => Ok(Self::Attic),
            _ => Err("floor requires storey level 0..59, or attic without level"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Elevation {
    pub floor: FloorRef,
    pub offset_mm: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    pub id: String,
    pub edge: Gate,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub landing: String,
    pub port: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "SurfaceInput")]
pub enum Surface {
    Slab { thickness_mm: f64 },
    Floor { floor: FloorRef },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SurfaceKind {
    Slab,
    Floor,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SurfaceInput {
    kind: SurfaceKind,
    #[serde(default, deserialize_with = "present")]
    thickness_mm: Option<f64>,
    #[serde(default, deserialize_with = "present")]
    floor: Option<FloorRef>,
}
impl TryFrom<SurfaceInput> for Surface {
    type Error = &'static str;
    fn try_from(v: SurfaceInput) -> std::result::Result<Self, Self::Error> {
        match (v.kind, v.thickness_mm, v.floor) {
            (SurfaceKind::Slab, Some(thickness_mm), None) => Ok(Self::Slab { thickness_mm }),
            (SurfaceKind::Floor, None, Some(floor)) => Ok(Self::Floor { floor }),
            _ => Err("surface requires only slab thickness_mm or only floor reference"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub id: String,
    pub height_mm: f64,
    pub thickness_mm: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandingGuard {
    pub id: String,
    pub edge: Gate,
    pub height_mm: f64,
    pub thickness_mm: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlightGuards {
    #[serde(deserialize_with = "nullable")]
    pub left: Option<Guard>,
    #[serde(deserialize_with = "nullable")]
    pub right: Option<Guard>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tread {
    pub id: String,
    #[serde(deserialize_with = "bounded::<_, _, 3, 16>")]
    pub outline: Vec<Point>,
    pub entry_edge: [usize; 2],
    pub exit_edge: [usize; 2],
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", try_from = "ProfileInput")]
pub enum Profile {
    Straight { tread_ids: Vec<String> },
    TreadChain { treads: Vec<Tread> },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProfileKind {
    Straight,
    TreadChain,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    kind: ProfileKind,
    #[serde(default, deserialize_with = "present_bounded::<_, _, 1, 80>")]
    tread_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_bounded::<_, _, 1, 80>")]
    treads: Option<Vec<Tread>>,
}
impl TryFrom<ProfileInput> for Profile {
    type Error = &'static str;
    fn try_from(v: ProfileInput) -> std::result::Result<Self, Self::Error> {
        match (v.kind, v.tread_ids, v.treads) {
            (ProfileKind::Straight, Some(tread_ids), None) => Ok(Self::Straight { tread_ids }),
            (ProfileKind::TreadChain, None, Some(treads)) => Ok(Self::TreadChain { treads }),
            _ => Err("profile requires only straight tread_ids or only tread_chain treads"),
        }
    }
}
impl Profile {
    pub fn tread_count(&self) -> usize {
        match self {
            Self::Straight { tread_ids } => tread_ids.len(),
            Self::TreadChain { treads } => treads.len(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Landing {
    pub id: String,
    pub name: String,
    pub elevation: Elevation,
    #[serde(deserialize_with = "nullable")]
    pub access_level: Option<usize>,
    #[serde(deserialize_with = "bounded::<_, _, 3, 64>")]
    pub outline: Vec<Point>,
    #[serde(deserialize_with = "bounded::<_, _, 1, 2>")]
    pub ports: Vec<Port>,
    #[serde(deserialize_with = "bounded::<_, _, 0, 8>")]
    pub walk_via: Vec<Point>,
    pub surface: Surface,
    #[serde(deserialize_with = "bounded::<_, _, 0, 16>")]
    pub guards: Vec<LandingGuard>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flight {
    pub id: String,
    pub name: String,
    pub from: Endpoint,
    pub to: Endpoint,
    pub soffit_mm: f64,
    pub profile: Profile,
    pub guards: FlightGuards,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opening {
    pub id: String,
    pub floor: FloorRef,
    #[serde(deserialize_with = "bounded::<_, _, 3, 64>")]
    pub outline: Vec<Point>,
}

pub(super) fn count(what: &str, n: usize, min: usize, max: usize) -> Result<()> {
    if !(min..=max).contains(&n) {
        return Err(invalid(format!("{what}: count must be {min}..={max}")));
    }
    Ok(())
}
pub(super) fn floor_bounds(floor: FloorRef) -> Result<()> {
    if let FloorRef::Storey { level } = floor {
        count("storey level", level, 0, 59)?;
    }
    Ok(())
}
fn id(s: &str, source: bool) -> Result<()> {
    count("ID bytes", s.len(), 1, if source { 256 } else { 64 })?;
    if !s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || (source && c == b'.')) {
        return Err(invalid("ID must contain only ASCII letters, digits, underscore, hyphen (source may contain dots)"));
    }
    Ok(())
}
fn name(s: &str) -> Result<()> {
    count("name bytes", s.len(), 1, 200)?;
    if s.chars().any(char::is_control) {
        return Err(invalid("name contains control characters"));
    }
    Ok(())
}
pub(super) fn point(p: Point) -> Result<()> {
    checked("sequence X", p[0], -1e6, 1e6)?;
    checked("sequence Y", p[1], -1e6, 1e6)?;
    Ok(())
}
fn ring_bounds(r: &[Point], max: usize) -> Result<()> {
    count("ring vertices", r.len(), 3, max)?;
    for &p in r {
        point(p)?;
    }
    Ok(())
}
fn guard(id_value: &str, height: f64, thickness: f64) -> Result<()> {
    id(id_value, false)?;
    checked("guard height", height, 100., 2000.)?;
    checked("guard thickness", thickness, 5., 100.)?;
    Ok(())
}
impl SequenceV1 {
    /// Size-bounded JSON entry point. Generic serde callers still get bounded collections
    /// but must bound their own input bytes (including strings and whitespace).
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        count("sequence JSON bytes", bytes.len(), 1, MAX_JSON_BYTES)?;
        serde_json::from_slice(bytes).map_err(|e| invalid(format!("JSON: {e}")))
    }
    /// Cheap complete scan before any geometry or input-derived index arithmetic.
    pub(super) fn check_bounds(&self) -> Result<()> {
        id(&self.id, true)?;
        name(&self.name)?;
        count("landings", self.landings.len(), 2, 33)?;
        count("flights", self.flights.len(), 1, 32)?;
        count("openings", self.openings.len(), 0, 64)?;
        let mut vertices = 0usize;
        let mut treads_total = 0usize;
        for l in &self.landings {
            id(&l.id, false)?;
            name(&l.name)?;
            floor_bounds(l.elevation.floor)?;
            checked("landing offset", l.elevation.offset_mm, -20000., 20000.)?;
            if let Some(level) = l.access_level {
                count("access level", level, 0, 59)?;
            }
            ring_bounds(&l.outline, 64)?;
            vertices += l.outline.len();
            count("landing ports", l.ports.len(), 1, 2)?;
            for p in &l.ports {
                id(&p.id, false)?;
                for q in p.edge {
                    point(q)?;
                }
            }
            count("walk_via", l.walk_via.len(), 0, 8)?;
            for &p in &l.walk_via {
                point(p)?;
            }
            match l.surface {
                Surface::Slab { thickness_mm } => {
                    checked("landing thickness", thickness_mm, 50., 1000.)?;
                }
                Surface::Floor { floor } => floor_bounds(floor)?,
            }
            count("landing guards", l.guards.len(), 0, 16)?;
            for g in &l.guards {
                guard(&g.id, g.height_mm, g.thickness_mm)?;
                for p in g.edge {
                    point(p)?;
                }
            }
        }
        for f in &self.flights {
            id(&f.id, false)?;
            name(&f.name)?;
            for e in [&f.from, &f.to] {
                id(&e.landing, false)?;
                id(&e.port, false)?;
            }
            checked("flight soffit", f.soffit_mm, 20., 500.)?;
            count("flight treads", f.profile.tread_count(), 1, 80)?;
            treads_total += f.profile.tread_count();
            match &f.profile {
                Profile::Straight { tread_ids } => {
                    for t in tread_ids {
                        id(t, false)?;
                    }
                    vertices += tread_ids.len() * 4;
                }
                Profile::TreadChain { treads } => {
                    for t in treads {
                        id(&t.id, false)?;
                        ring_bounds(&t.outline, 16)?;
                        vertices += t.outline.len();
                        for i in t.entry_edge.into_iter().chain(t.exit_edge) {
                            if i >= t.outline.len() {
                                return Err(invalid(format!("tread {} edge index out of range", t.id)));
                            }
                        }
                    }
                }
            }
            for g in f.guards.left.iter().chain(&f.guards.right) {
                guard(&g.id, g.height_mm, g.thickness_mm)?;
            }
        }
        for o in &self.openings {
            id(&o.id, false)?;
            floor_bounds(o.floor)?;
            ring_bounds(&o.outline, 64)?;
            vertices += o.outline.len();
        }
        count("sequence treads", treads_total, 1, 1024)?;
        count("sequence vertices", vertices, 0, 20000)?;
        Ok(())
    }
}
