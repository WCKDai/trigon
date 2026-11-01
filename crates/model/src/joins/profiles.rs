//! Exact material at mixed-width, shared-endpoint continuations. The public join API stays
//! quadrilateral; mesh and exchange consumers use these potentially concave profiles. Ownership is
//! the source union: the continuing run owns its band, and branches own what remains of theirs.
//! No snapping, reach extension, or near-collinear inference is introduced here.

use std::collections::{BTreeMap, BTreeSet};

use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
use i_overlay::float::overlay::FloatOverlay;

use super::{Axis, Join, JoinKind, MIN_SIN, Target, cross, dot, endpoint_key, frame, square};
use crate::build::{FacadeFrame, Rect};
use crate::mesh::Mesh;

type Shapes = super::MaterialProfile;
pub(crate) type Profiled = super::JoinedMaterial;

#[derive(Clone, Copy)]
struct Local {
    origin: [f64; 2],
    t: [f64; 2],
    n: [f64; 2],
}

impl Local {
    fn project(self, p: [f64; 2]) -> [f64; 2] {
        let d = super::sub(p, self.origin);
        // Exactly shared caps must quantize to the same line after a rigid transform.
        [dot(d, self.t), dot(d, self.n)].map(|v| if v.abs() < 1e-8 { 0.0 } else { v })
    }

    fn site(self, p: [f64; 2]) -> [f64; 2] {
        [self.origin[0] + p[0] * self.t[0] + p[1] * self.n[0], self.origin[1] + p[0] * self.t[1] + p[1] * self.n[1]]
    }
}

fn axis_order(a: &Axis, b: &Axis) -> std::cmp::Ordering {
    let ordered = |w: &Axis| {
        if w.a[0].total_cmp(&w.b[0]).then(w.a[1].total_cmp(&w.b[1])).is_le() { [w.a, w.b] } else { [w.b, w.a] }
    };
    let a = ordered(a);
    let b = ordered(b);
    a.into_iter().flatten().zip(b.into_iter().flatten()).map(|(a, b)| a.total_cmp(&b)).find(|c| !c.is_eq()).unwrap_or(std::cmp::Ordering::Equal)
}

pub(crate) fn join(walls: &[Axis], facade: &[[f64; 2]], band: f64) -> Vec<Profiled> {
    let mut joined = super::join(walls, facade, band);
    let frames: Vec<_> = walls.iter().map(|w| frame(w.a, w.b)).collect();
    let mut stations = BTreeMap::<_, Vec<(usize, usize)>>::new();
    for (i, w) in walls.iter().enumerate() {
        // The builder normally receives validated walls. Keep hostile direct model calls out
        // of the overlay as well; the old join path retains their established fallback.
        if ![w.a, w.b].iter().flatten().all(|v| v.is_finite() && v.abs() <= 1e8)
            || !(40.0..=1000.0).contains(&w.thickness)
            || !frames[i].is_some_and(|(_, _, len)| (100.0..=200_000.0).contains(&len))
        {
            continue;
        }
        for (e, p) in [w.a, w.b].into_iter().enumerate() {
            stations.entry(endpoint_key(p)).or_default().push((i, e));
        }
    }
    let mut reset = vec![[false; 2]; walls.len()];
    let mut clips = vec![BTreeSet::new(); walls.len()];
    let mut locals = vec![None; walls.len()];
    let mut contacts = vec![[None, None]; walls.len()];
    let direction = |i: usize, end: usize| frames[i].map(|(t, _, _)| if end == 0 { t } else { [-t[0], -t[1]] });
    for incident in stations.values() {
        let mut pairs = Vec::new();
        for (k, &(i, e)) in incident.iter().enumerate() {
            for &(j, f) in &incident[..k] {
                let (Some(a), Some(b)) = (direction(i, e), direction(j, f)) else { continue };
                if (walls[i].thickness - walls[j].thickness).abs() > 1e-6 && cross(a, b).abs() <= 1e-9 && dot(a, b) < 0.0 {
                    pairs.push([(i, e), (j, f)]);
                }
            }
        }
        // If both crossing runs step here, choose one owner consistently, independently of
        // input order and endpoint direction. Never clip two runs against each other twice.
        let priority = |pair: &[(usize, usize); 2]| {
            (
                walls[pair[0].0].thickness.max(walls[pair[1].0].thickness),
                frames[pair[0].0].map_or(0.0, |f| f.2) + frames[pair[1].0].map_or(0.0, |f| f.2),
            )
        };
        pairs.sort_by(|a, b| {
            let (at, al) = priority(a);
            let (bt, bl) = priority(b);
            bt.total_cmp(&at).then(bl.total_cmp(&al)).then_with(|| {
                let first = |p: &[(usize, usize); 2]| if axis_order(&walls[p[0].0], &walls[p[1].0]).is_le() { p[0].0 } else { p[1].0 };
                axis_order(&walls[first(a)], &walls[first(b)])
            })
        });
        let Some(&[(i, e), (j, _)]) = pairs.first() else { continue };
        let Some(main) = direction(i, e) else { continue };
        let mut branches: Vec<_> =
            incident.iter().copied().filter(|&(k, g)| direction(k, g).is_some_and(|d| cross(main, d).abs() >= MIN_SIN)).collect();
        if branches.is_empty() {
            continue;
        }
        // A short leg can put the next step inside a branch's width. Follow exact,
        // opposite collinear endpoints through every leg, including equal-width seams.
        let mut run = BTreeSet::from([i, j]);
        let mut pending = vec![i, j];
        while let Some(k) = pending.pop() {
            for (end, point) in [walls[k].a, walls[k].b].into_iter().enumerate() {
                let Some(inward) = direction(k, end) else { continue };
                for &(other, other_end) in stations.get(&endpoint_key(point)).into_iter().flatten() {
                    let Some(d) = direction(other, other_end) else { continue };
                    if cross(main, d).abs() <= 1e-9 && dot(inward, d) < 0.0 {
                        // Internal seams belong to the continuous band, even if the legacy
                        // near-face solver mistook a nearby wide branch for an end target.
                        reset[k][end] = true;
                        reset[other][other_end] = true;
                        joined[k].ends[end] = None;
                        joined[other].ends[other_end] = None;
                        if run.insert(other) {
                            pending.push(other);
                        }
                    }
                }
            }
        }
        branches.sort_by(|&(a, _), &(b, _)| walls[b].thickness.total_cmp(&walls[a].thickness).then_with(|| axis_order(&walls[a], &walls[b])));
        for (k, &(b, g)) in branches.iter().enumerate() {
            reset[b][g] = true;
            clips[b].extend(run.iter().copied());
            contacts[b][g] = Some(run.clone());
            locals[b] = Some(Local { origin: if e == 0 { walls[i].a } else { walls[i].b }, t: main, n: [-main[1], main[0]] });
            // Transverse branches on the same side can share material outside the run too.
            // Assign that intersection once, without admitting near-parallel joins.
            for &(a, h) in &branches[..k] {
                if let (Some(x), Some(y)) = (direction(a, h), direction(b, g))
                    && cross(x, y).abs() >= MIN_SIN
                {
                    clips[b].insert(a);
                }
            }
        }
    }
    for ((w, j), ends) in walls.iter().zip(&mut joined).zip(&reset) {
        let raw = square(w, None);
        if ends[0] {
            j.a = w.a;
            j.outline[0] = raw.outline[0];
            j.outline[3] = raw.outline[3];
        }
        if ends[1] {
            j.b = w.b;
            j.outline[1] = raw.outline[1];
            j.outline[2] = raw.outline[2];
        }
    }
    let outlines: Vec<_> = joined.iter().map(|j| j.outline.to_vec()).collect();
    joined
        .into_iter()
        .zip(clips)
        .zip(locals)
        .zip(contacts)
        .map(|(((mut joined, clips), local), contacts)| {
            let material = local.map(|local| {
                let clips: Vec<_> = clips
                    .into_iter()
                    .filter(|&i| intersects(&joined.outline, &outlines[i]))
                    .map(|i| outlines[i].iter().map(|&p| local.project(p)).collect())
                    .collect();
                let subject = vec![vec![joined.outline.iter().map(|&p| local.project(p)).collect()]];
                let mut material = overlay(&subject, &clips, OverlayRule::Difference);
                for p in material.iter_mut().flatten().flatten() {
                    *p = local.site(*p);
                }
                material
            });
            if let Some(material) = &material {
                for (end, targets) in contacts.into_iter().enumerate() {
                    if let Some(targets) = targets {
                        // A branch can contact only the thinner leg, or both legs and the
                        // shoulder. Name a face it actually touches, rather than always
                        // choosing the thicker wall (which can be on the opposite side).
                        joined.ends[end] = targets
                            .into_iter()
                            .map(|i| (i, shared_length(material, &outlines[i])))
                            .filter(|(_, length)| *length > 1e-3)
                            .max_by(|(a, al), (b, bl)| al.total_cmp(bl).then(walls[*a].thickness.total_cmp(&walls[*b].thickness)))
                            .map(|(i, _)| Join { target: Target::Wall(i), kind: JoinKind::Butt });
                    }
                }
            }
            Profiled { joined, material }
        })
        .collect()
}

/// Convex join references share material only when no edge separates them. Excluding
/// remote run members keeps their coordinates out of the local overlay's integer grid.
fn intersects(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    for ring in [a, b] {
        for (&p, &q) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
            let Some((_, n, _)) = frame(p, q) else { continue };
            let interval = |r: &[[f64; 2]]| {
                r.iter().map(|&v| dot(super::sub(v, p), n)).fold([f64::INFINITY, f64::NEG_INFINITY], |[lo, hi], v| [lo.min(v), hi.max(v)])
            };
            let ([al, ah], [bl, bh]) = (interval(a), interval(b));
            if ah < bl || bh < al {
                return false;
            }
        }
    }
    true
}

fn shared_length(material: &Shapes, outline: &[[f64; 2]]) -> f64 {
    let mut length = 0.0;
    for ring in material.iter().flatten() {
        for (&p, &q) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
            let Some((t, _, len)) = frame(p, q) else { continue };
            for (&r, &s) in outline.iter().zip(outline.iter().cycle().skip(1)).take(outline.len()) {
                if cross(super::sub(r, p), t).abs() < 1e-3 && cross(super::sub(s, p), t).abs() < 1e-3 {
                    let (a, b) = (dot(super::sub(r, p), t), dot(super::sub(s, p), t));
                    length += (a.max(b).min(len) - a.min(b).max(0.0)).max(0.0);
                }
            }
        }
    }
    length
}

fn overlay(subject: &Shapes, clips: &[Vec<[f64; 2]>], rule: OverlayRule) -> Shapes {
    if clips.is_empty() && rule == OverlayRule::Difference {
        return subject.clone();
    }
    let mut shapes = FloatOverlay::with_subj_and_clip(subject, &clips.to_vec()).overlay(rule, FillRule::NonZero);
    for shape in &mut shapes {
        for (i, ring) in shape.iter_mut().enumerate() {
            if (crate::footprint::signed_area2(ring) > 0.0) != (i == 0) {
                ring.reverse();
            }
        }
    }
    shapes
}

fn door_band(fr: &FacadeFrame, r: &Rect, depth: f64) -> Vec<[f64; 2]> {
    [[r.u0, -1.0], [r.u1, -1.0], [r.u1, depth + 1.0], [r.u0, depth + 1.0]]
        .map(|[u, w]| {
            let p = fr.point(u, w, 0.0);
            [p[0], p[1]]
        })
        .to_vec()
}

pub(crate) fn cut(material: &Shapes, footprints: &[Vec<[f64; 2]>]) -> Vec<Vec<[f64; 2]>> {
    overlay(material, footprints, OverlayRule::Difference).into_iter().filter_map(|s| s.into_iter().next()).collect()
}

fn fill(mesh: &mut Mesh, shapes: &Shapes, to3d: impl Fn([f64; 2]) -> [f64; 3], reverse: bool) -> Option<()> {
    let mut part = Mesh::default();
    for shape in shapes {
        let (outer, holes) = shape.split_first()?;
        if !part.polygon(outer, holes, &to3d) {
            return None;
        }
    }
    if reverse {
        for t in &mut part.triangles {
            t.swap(1, 2);
        }
    }
    mesh.append(&part);
    Some(())
}

/// Extrude a profile, cutting each vertical boundary and the door reveals in its own plane.
/// This avoids overlapping closed prisms or internal horizontal faces at door-head heights.
pub(crate) fn mesh(material: &Shapes, fr: &FacadeFrame, holes: &[Rect], depth: f64, z0: f64, height: f64) -> Option<Mesh> {
    let mut mesh = Mesh::default();
    let z1 = z0 + height;
    let u = |p: [f64; 2]| (p[0] - fr.origin[0]) * fr.t[0] + (p[1] - fr.origin[1]) * fr.t[1];
    for (z, reverse) in [(z0, true), (z1, false)] {
        let clips: Vec<_> = holes.iter().filter(|r| r.v0 <= z && r.v1 >= z).map(|r| door_band(fr, r, depth)).collect();
        fill(&mut mesh, &overlay(material, &clips, OverlayRule::Difference), |p| [p[0], p[1], z], reverse)?;
    }
    for ring in material.iter().flatten() {
        for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
            let Some((t, _, len)) = frame(*a, *b) else { continue };
            let (ua, ub) = (u(*a), u(*b));
            let mut cuts = Vec::new();
            for r in holes {
                let (lo, hi) = if (ub - ua).abs() < 1e-9 {
                    if ua < r.u0 || ua > r.u1 {
                        continue;
                    }
                    (0.0, len)
                } else {
                    let (s, t) = ((r.u0 - ua) * len / (ub - ua), (r.u1 - ua) * len / (ub - ua));
                    (s.min(t).max(0.0), s.max(t).min(len))
                };
                if hi - lo > 1e-8 {
                    cuts.push(vec![[lo, r.v0], [hi, r.v0], [hi, r.v1], [lo, r.v1]]);
                }
            }
            let face = vec![vec![vec![[0.0, z0], [len, z0], [len, z1], [0.0, z1]]]];
            fill(&mut mesh, &overlay(&face, &cuts, OverlayRule::Difference), |[s, z]| [a[0] + t[0] * s, a[1] + t[1] * s, z], false)?;
        }
    }
    for r in holes {
        let opening = overlay(material, &[door_band(fr, r, depth)], OverlayRule::Intersect);
        if r.v1 < z1 {
            fill(&mut mesh, &opening, |p| [p[0], p[1], r.v1], true)?;
        }
        for ring in opening.iter().flatten() {
            for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)).take(ring.len()) {
                if [r.u0, r.u1].iter().any(|s| (u(*a) - s).abs() < 1e-3 && (u(*b) - s).abs() < 1e-3) {
                    // Reverse the void's boundary: these faces look into the opening.
                    let top = r.v1.min(z1);
                    mesh.quad([b[0], b[1], z0], [a[0], a[1], z0], [a[0], a[1], top], [b[0], b[1], top]);
                }
            }
        }
    }
    Some(mesh)
}
