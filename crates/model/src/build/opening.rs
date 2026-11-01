//! Opening profiles and hardware. Dimensions here are family presentation defaults in mm,
//! not measurements of an existing building. Nominal opening rectangles remain authoritative.
//!
//! Normal-size windows use a 46 mm outer frame, 32 mm sash and 3 mm glazing gasket;
//! doors use a 60 mm surround, up to 90 mm stiles and a 15 mm threshold. Small lights
//! scale their sightlines down and omit hardware below 200 × 350 mm. Glazing is a 6 mm
//! presentation pane, not a claim about a surveyed insulating-glass build-up. Profiles,
//! seals, panels and hardware share the fill's source identity and phase.

use super::{FacadeFrame, Rect, dividers, fbox};
use crate::{Material, Mesh};

mod historic;
pub(super) use historic::window as historic_window;

#[derive(Default)]
pub(super) struct Parts {
    pub frame: Mesh,
    pub glass: Mesh,
    pub panel: Mesh,
    pub seal: Mesh,
    pub hardware: Mesh,
    pub sill: Mesh,
}

impl Parts {
    pub fn append(&mut self, other: Self) {
        self.frame.append(&other.frame);
        self.glass.append(&other.glass);
        self.panel.append(&other.panel);
        self.seal.append(&other.seal);
        self.hardware.append(&other.hardware);
        self.sill.append(&other.sill);
    }

    pub fn finish(self) -> Vec<(Material, Mesh)> {
        [
            (Material::Frame, self.frame),
            (Material::Glass, self.glass),
            (Material::Door, self.panel),
            (Material::Gasket, self.seal),
            (Material::Steel, self.hardware),
            (Material::Sill, self.sill),
        ]
        .into_iter()
        .filter(|(_, mesh)| !mesh.is_empty())
        .collect()
    }
}

fn inset(r: Rect, d: f64) -> Rect {
    Rect { u0: r.u0 + d, u1: r.u1 - d, v0: r.v0 + d, v1: r.v1 - d }
}

fn small(r: Rect) -> f64 {
    (r.u1 - r.u0).min(r.v1 - r.v0)
}

fn corners(r: Rect) -> [[f64; 2]; 4] {
    [[r.u0, r.v0], [r.u1, r.v0], [r.u1, r.v1], [r.u0, r.v1]]
}

/// A closed rectangular ring, or a U jamb with an open bottom. Each depth station gives
/// the outer and inner boundary; close stations form the rebate shoulders. Sweep
/// the boundary, rather than overlapping four boxes and leaving coincident corner faces.
fn ring(fr: &FacadeFrame, r: Rect, profile: &[(f64, f64, f64)], bottom: bool) -> Mesh {
    let mut m = Mesh::default();
    let section = |outer, inner| {
        let o = inset(r, outer);
        let mut i = inset(r, inner);
        if !bottom {
            i.v0 = o.v0;
        }
        (corners(o), corners(i))
    };
    let contours = |outer, inner| -> Vec<Vec<[f64; 2]>> {
        let (o, i) = section(outer, inner);
        if bottom { vec![o.to_vec(), i.into_iter().rev().collect()] } else { vec![vec![o[0], i[0], i[3], i[2], i[1], o[1], o[2], o[3]]] }
    };
    for pair in profile.windows(2) {
        let ((wa, oa, ia), (wb, ob, ib)) = (pair[0], pair[1]);
        for (a, b) in contours(oa, ia).iter().zip(contours(ob, ib)) {
            for k in 0..a.len() {
                let j = (k + 1) % a.len();
                if wa == wb && a[k] == b[k] && a[j] == b[j] {
                    continue;
                }
                let p = |q: [f64; 2], w| fr.point(q[0], w, q[1]);
                m.quad(p(a[k], wa), p(b[k], wb), p(b[j], wb), p(a[j], wa));
            }
        }
    }
    for (station, reverse) in [(profile.first(), false), (profile.last(), true)] {
        let Some(&(w, outer, inner)) = station else { continue };
        let (o, i) = section(outer, inner);
        let mut cap = Mesh::default();
        if bottom {
            for k in 0..4 {
                let j = (k + 1) % 4;
                let p = |q: [f64; 2]| fr.point(q[0], w, q[1]);
                cap.quad(p(o[k]), p(o[j]), p(i[j]), p(i[k]));
            }
        } else {
            let outline = [o[0], i[0], i[3], i[2], i[1], o[1], o[2], o[3]];
            cap.polygon(&outline, &[], |q| fr.point(q[0], w, q[1]));
        }
        if reverse {
            for t in &mut cap.triangles {
                t.swap(1, 2);
            }
        }
        m.append(&cap);
    }
    m
}

/// Glazing sits in the rebate, backed by the rear stop. The seal overlaps the pane edge by
/// a fraction of a millimetre as a compressible gasket would; exposed faces never coincide. No pane extends
/// across a structural mullion, transom or leaf rail.
fn glazing(parts: &mut Parts, fr: &FacadeFrame, aperture: Rect, mid: f64) {
    let gasket = 3.0_f64.min(small(aperture) / 12.0);
    parts.seal.append(&ring(fr, aperture, &[(mid - 7.0, 0.0, gasket), (mid + 7.0, 0.0, gasket)], true));
    let pane = inset(aperture, gasket * 0.8);
    parts.glass.append(&fbox(fr, (pane.u0, pane.u1), (mid - 3.0, mid + 3.0), (pane.v0, pane.v1)));
}

/// Casement sash or fixed frame with an integral bevelled glazing bead and rear stop.
fn glazed_ring(parts: &mut Parts, fr: &FacadeFrame, r: Rect, w: (f64, f64), width: f64) {
    let bevel = (width * 0.13).min(4.0);
    parts.frame.append(&ring(
        fr,
        r,
        &[
            (w.0, 0.0, width),
            (w.0 + 10.0, 0.0, width),
            (w.0 + 14.0, 0.0, width - bevel),
            (w.1 - 10.0, 0.0, width - bevel),
            (w.1 - 10.0, 0.0, width + bevel),
            (w.1, 0.0, width + bevel),
        ],
        true,
    ));
    // Leave 1 mm between the pane's back and the stop at w.1 - 10; the gasket spans
    // that seating clearance and embeds into the stop, rather than floating in the cavity.
    glazing(parts, fr, inset(r, width - bevel), w.1 - 14.0);
}

/// Eight-sided barrel, axis vertical; attached straps and the operating lever are simple
/// solid metal sections. Small lights omit hardware instead of filling their clear glazing.
fn barrel(fr: &FacadeFrame, u: f64, w: f64, z: f64, radius: f64, height: f64) -> Mesh {
    let mut m = Mesh::default();
    let points: Vec<_> = (0..8)
        .map(|k| {
            let a = f64::from(k) * std::f64::consts::TAU / 8.0;
            (u + radius * a.cos(), w + radius * a.sin())
        })
        .collect();
    for k in 0..8 {
        let (a, b) = (points[k], points[(k + 1) % 8]);
        m.quad(fr.point(a.0, a.1, z), fr.point(b.0, b.1, z), fr.point(b.0, b.1, z + height), fr.point(a.0, a.1, z + height));
        if k > 0 && k < 7 {
            let base = m.positions.len() as u32;
            m.positions.extend([
                fr.point(points[0].0, points[0].1, z),
                fr.point(b.0, b.1, z),
                fr.point(a.0, a.1, z),
                fr.point(points[0].0, points[0].1, z + height),
                fr.point(a.0, a.1, z + height),
                fr.point(b.0, b.1, z + height),
            ]);
            m.triangles.extend([[base, base + 1, base + 2], [base + 3, base + 4, base + 5]]);
        }
    }
    m
}

fn hardware(parts: &mut Parts, fr: &FacadeFrame, r: Rect, w: (f64, f64), stile: f64, hinge_left: bool, door: bool, handle: bool) {
    if r.u1 - r.u0 < 200.0 || r.v1 - r.v0 < 350.0 {
        return;
    }
    let sign = if hinge_left { 1.0 } else { -1.0 };
    let hinge = if hinge_left { r.u0 } else { r.u1 };
    let radius = if door { 6.0 } else { 4.0 };
    for fraction in [0.18, 0.82] {
        let z = r.v0 + (r.v1 - r.v0) * fraction;
        parts.hardware.append(&barrel(fr, hinge - sign, w.0 + 1.0, z - 20.0, radius, 40.0));
        let end = hinge + sign * (stile * 0.6);
        parts.hardware.append(&fbox(fr, (hinge.min(end), hinge.max(end)), (w.0 - 2.0, w.0 + 2.0), (z - 12.0, z + 12.0)));
    }
    if !handle {
        return;
    }
    let latch = if hinge_left { r.u1 - stile / 2.0 } else { r.u0 + stile / 2.0 };
    let z = if door { r.v0 + 1000.0_f64.min((r.v1 - r.v0) * 0.48) } else { (r.v0 + r.v1) / 2.0 };
    let plate_width = 24.0_f64.min(stile * 0.65);
    let length = if door { 90.0_f64.min((r.u1 - r.u0) * 0.2) } else { 50.0 };
    // Door levers on both sides; a casement's operating lever is inside only.
    for (surface, direction) in [(w.0, -1.0), (w.1, 1.0)] {
        if !door && direction < 0.0 {
            continue;
        }
        let tip = surface + direction * 26.0;
        parts.hardware.append(&fbox(
            fr,
            (latch - plate_width / 2.0, latch + plate_width / 2.0),
            (surface - 1.0, surface + 1.0),
            (z - 32.0, z + 32.0),
        ));
        parts.hardware.append(&fbox(fr, (latch - 4.0, latch + 4.0), (surface.min(tip), surface.max(tip)), (z - 4.0, z + 4.0)));
        if door {
            let end = latch - sign * length;
            parts.hardware.append(&fbox(fr, (latch.min(end) - 4.0, latch.max(end) + 4.0), (tip - 4.0, tip + 4.0), (z - 4.0, z + 4.0)));
        } else {
            parts.hardware.append(&fbox(fr, (latch - 4.0, latch + 4.0), (tip - 4.0, tip + 4.0), (z - length, z + 4.0)));
        }
    }
}

pub(super) fn window(fr: &FacadeFrame, r: Rect, w: (f64, f64), lights: u32, top: f64, operable: bool, mirror: bool) -> Parts {
    let mut parts = Parts::default();
    let lights = lights.clamp(1, 8);
    let fw = 46.0_f64.min((r.u1 - r.u0) / (4.0 * f64::from(lights) + 2.0)).min((r.v1 - r.v0) / 8.0);
    if !operable {
        glazed_ring(&mut parts, fr, r, w, fw);
        return parts;
    }
    parts.frame.append(&ring(fr, r, &[(w.0, 0.0, fw), (w.1 - 10.0, 0.0, fw), (w.1 - 10.0, 0.0, fw * 1.15), (w.1, 0.0, fw * 1.15)], true));
    let clear = inset(r, fw);
    let mullion = fw * 0.75;
    let cuts: Vec<_> = std::iter::once(clear.u0).chain(dividers(clear.u0, clear.u1, lights)).chain(std::iter::once(clear.u1)).collect();
    for &u in cuts.iter().skip(1).take(lights.saturating_sub(1) as usize) {
        parts.frame.append(&fbox(fr, (u - mullion / 2.0, u + mullion / 2.0), w, (clear.v0 - 0.5, clear.v1 + 0.5)));
    }
    for (index, pair) in cuts.windows(2).enumerate() {
        let (u0, u1) =
            (pair[0] + if index > 0 { mullion / 2.0 } else { 0.0 }, pair[1] - if index + 1 < lights as usize { mullion / 2.0 } else { 0.0 });
        let transom = r.v1 - fw - top;
        let mut rows = vec![(clear.v0, clear.v1)];
        if top > 0.0 && r.v1 - r.v0 > top + 4.0 * fw {
            rows = vec![(clear.v0, transom - mullion / 2.0), (transom + mullion / 2.0, clear.v1)];
            parts.frame.append(&fbox(fr, (u0 - 0.5, u1 + 0.5), w, (transom - mullion / 2.0, transom + mullion / 2.0)));
        }
        for (v0, v1) in rows {
            let cell = Rect { u0, u1, v0, v1 };
            let sash = inset(cell, 2.0_f64.min(small(cell) / 30.0));
            let sw = 32.0_f64.min(small(sash) * 0.18);
            let depth = (w.0 + 8.0, w.1 - 12.0);
            glazed_ring(&mut parts, fr, sash, depth, sw);
            let hinge_left = if index * 2 + 1 == lights as usize { !mirror } else { index * 2 < lights as usize };
            hardware(&mut parts, fr, sash, depth, sw, hinge_left, false, true);
        }
    }
    parts
}

/// Recessed field panel, bevelled on both faces. Its edge seats 0.5 mm in the surrounding
/// leaf, so exposed seams are shoulders, not coplanar overlays on a solid slab.
fn panel(fr: &FacadeFrame, r: Rect, w: (f64, f64)) -> Mesh {
    let edge = inset(r, -0.5);
    let field = inset(r, 8.0_f64.min(small(r) / 8.0));
    let mut m = Mesh::default();
    let recess = 11.0_f64.min((w.1 - w.0) * 0.25);
    let lip = recess * 0.45;
    let rings = [(edge, w.0 + lip), (field, w.0 + recess), (field, w.1 - recess), (edge, w.1 - lip)];
    // The boundary goes inward along the bevel, across the field, then back out. Close
    // the peripheral edge separately, leaving the two broad field faces recessed.
    for (a, b) in [(rings[0], rings[1]), (rings[2], rings[3]), (rings[3], rings[0])] {
        let (a, b) = (corners(a.0).map(|q| fr.point(q[0], a.1, q[1])), corners(b.0).map(|q| fr.point(q[0], b.1, q[1])));
        for k in 0..4 {
            let j = (k + 1) % 4;
            m.quad(a[k], a[j], b[j], b[k]);
        }
    }
    let face = |r, w| corners(r).map(|q| fr.point(q[0], w, q[1]));
    let a = face(field, w.0 + recess);
    let b = face(field, w.1 - recess);
    m.quad(a[0], a[1], a[2], a[3]);
    m.quad(b[3], b[2], b[1], b[0]);
    m
}

fn leaf(parts: &mut Parts, fr: &FacadeFrame, r: Rect, w: (f64, f64), glazed: bool, hinge_left: bool, handle: bool) {
    let stile = 90.0_f64.min(small(r) / 4.0);
    let material = if glazed { &mut parts.frame } else { &mut parts.panel };
    material.append(&ring(fr, r, &[(w.0, 0.0, stile), (w.1, 0.0, stile)], true));
    let clear = inset(r, stile);
    let rail_z = r.v0 + 0.36 * (r.v1 - r.v0);
    let rail = stile * 0.7;
    material.append(&fbox(fr, (clear.u0 - 0.5, clear.u1 + 0.5), w, (rail_z, rail_z + rail)));
    let lower = Rect { v1: rail_z, ..clear };
    if glazed {
        parts.frame.append(&panel(fr, lower, w));
    } else {
        parts.panel.append(&panel(fr, lower, w));
    }
    let upper = Rect { v0: rail_z + rail, ..clear };
    if glazed {
        let bead = 12.0_f64.min(small(upper) / 10.0);
        // The bead seats into the structural rails/stiles, without duplicate exposed faces.
        glazed_ring(parts, fr, inset(upper, -0.5), (w.0 + 2.0, w.1 - 2.0), bead);
    } else {
        parts.panel.append(&panel(fr, upper, w));
    }
    hardware(parts, fr, r, w, stile, hinge_left, true, handle);
}

/// Door surround with a rebate, separated closed leaves, 15 mm threshold, seated panels,
/// hinges and levers. `mirror` reflects the complete unit, including the active leaf.
pub(super) fn door(fr: &FacadeFrame, r: Rect, w: (f64, f64), leaves: u32, glazed: bool, top: f64, mirror: bool) -> Parts {
    door_layout(fr, r, w, leaves, glazed, top, mirror, None)
}

pub(super) fn panel_door(fr: &FacadeFrame, r: Rect, w: (f64, f64), leaves: u32, columns: u32, rows: u32, glazed_rows: u32) -> Parts {
    door_layout(fr, r, w, leaves, false, 0.0, false, Some((columns, rows, glazed_rows)))
}

fn panel_leaf(
    parts: &mut Parts,
    fr: &FacadeFrame,
    r: Rect,
    w: (f64, f64),
    columns: u32,
    rows: u32,
    glazed_rows: u32,
    hinge_left: bool,
    handle: bool,
) {
    let stile = 90.0_f64.min((r.u1 - r.u0) / (4.0 * f64::from(columns) + 2.0)).min((r.v1 - r.v0) / (4.0 * f64::from(rows) + 2.0));
    parts.panel.append(&ring(fr, r, &[(w.0, 0.0, stile), (w.1, 0.0, stile)], true));
    let clear = inset(r, stile);
    let rail = stile * 0.65;
    let xs: Vec<_> = (0..=columns).map(|i| clear.u0 + (clear.u1 - clear.u0) * f64::from(i) / f64::from(columns)).collect();
    let zs: Vec<_> = (0..=rows).map(|i| clear.v0 + (clear.v1 - clear.v0) * f64::from(i) / f64::from(rows)).collect();
    for &x in xs.iter().skip(1).take(columns.saturating_sub(1) as usize) {
        parts.panel.append(&fbox(fr, (x - rail / 2.0, x + rail / 2.0), w, (clear.v0 - 0.5, clear.v1 + 0.5)));
    }
    for &z in zs.iter().skip(1).take(rows.saturating_sub(1) as usize) {
        for (i, x) in xs.windows(2).enumerate() {
            parts.panel.append(&fbox(
                fr,
                (x[0] + if i > 0 { rail / 2.0 - 0.5 } else { -0.5 }, x[1] - if i + 1 < columns as usize { rail / 2.0 - 0.5 } else { -0.5 }),
                w,
                (z - rail / 2.0, z + rail / 2.0),
            ));
        }
    }
    for (i, x) in xs.windows(2).enumerate() {
        for (j, z) in zs.windows(2).enumerate() {
            let aperture = Rect {
                u0: x[0] + if i > 0 { rail / 2.0 } else { 0.0 },
                u1: x[1] - if i + 1 < columns as usize { rail / 2.0 } else { 0.0 },
                v0: z[0] + if j > 0 { rail / 2.0 } else { 0.0 },
                v1: z[1] - if j + 1 < rows as usize { rail / 2.0 } else { 0.0 },
            };
            if j >= rows.saturating_sub(glazed_rows) as usize {
                let mut bead = Parts::default();
                glazed_ring(&mut bead, fr, inset(aperture, -0.5), (w.0 + 2.0, w.1 - 2.0), 12.0_f64.min(small(aperture) / 10.0));
                parts.panel.append(&bead.frame);
                parts.glass.append(&bead.glass);
                parts.seal.append(&bead.seal);
            } else {
                parts.panel.append(&panel(fr, aperture, w));
            }
        }
    }
    hardware(parts, fr, r, w, stile, hinge_left, true, handle);
}

fn door_layout(fr: &FacadeFrame, r: Rect, w: (f64, f64), leaves: u32, glazed: bool, top: f64, mirror: bool, grid: Option<(u32, u32, u32)>) -> Parts {
    let mut parts = Parts::default();
    let leaves = leaves.clamp(1, 4);
    let fw = 60.0_f64.min((r.u1 - r.u0) / (4.0 * f64::from(leaves) + 2.0)).min((r.v1 - r.v0) / 8.0);
    parts.frame.append(&ring(fr, r, &[(w.0, 0.0, fw), (w.1 - 10.0, 0.0, fw), (w.1 - 9.0, 0.0, fw * 1.15), (w.1, 0.0, fw * 1.15)], false));
    let mut clear = inset(r, fw);
    clear.v0 = r.v0;
    if top > 2.0 * fw && r.v1 - r.v0 > top + 4.0 * fw {
        let transom = clear.v1 - top;
        parts.frame.append(&fbox(fr, (clear.u0 - 0.5, clear.u1 + 0.5), w, (transom - fw, transom)));
        glazed_ring(&mut parts, fr, inset(Rect { v0: transom, ..clear }, -0.5), (w.0 + 2.0, w.1 - 2.0), fw * 0.45);
        clear.v1 = transom - fw;
    }
    let threshold = 15.0_f64.min((r.v1 - r.v0) / 20.0);
    let lo = [fr.point(clear.u0, w.0, r.v0), fr.point(clear.u1, w.0, r.v0), fr.point(clear.u1, w.1, r.v0), fr.point(clear.u0, w.1, r.v0)];
    let hi = [
        fr.point(clear.u0, w.0, r.v0 + threshold * 0.6),
        fr.point(clear.u1, w.0, r.v0 + threshold * 0.6),
        fr.point(clear.u1, w.1, r.v0 + threshold),
        fr.point(clear.u0, w.1, r.v0 + threshold),
    ];
    parts.sill.hexahedron(lo, hi);
    clear.v0 += threshold + 4.0;
    let cuts: Vec<_> = std::iter::once(clear.u0).chain(dividers(clear.u0, clear.u1, leaves)).chain(std::iter::once(clear.u1)).collect();
    for (index, pair) in cuts.windows(2).enumerate() {
        let r = inset(Rect { u0: pair[0], u1: pair[1], ..clear }, 2.0_f64.min(small(clear) / (f64::from(leaves) * 30.0)));
        let hinge_left = if leaves == 1 { !mirror } else { index * 2 < leaves as usize };
        let active = if mirror { index == 0 } else { index + 1 == leaves as usize };
        if let Some((columns, rows, glazed_rows)) = grid {
            panel_leaf(&mut parts, fr, r, (w.0 + 8.0, w.1 - 12.0), columns, rows, glazed_rows, hinge_left, active);
        } else {
            leaf(&mut parts, fr, r, (w.0 + 8.0, w.1 - 12.0), glazed, hinge_left, active);
        }
    }
    parts
}

pub(super) fn interior(fr: &FacadeFrame, r: Rect, depth: f64) -> Parts {
    let mut parts = Parts::default();
    let fw = 45.0_f64.min((r.u1 - r.u0) / 4.0);
    let thickness = 40.0_f64.min(depth / 2.0);
    let w = ((depth - thickness) / 2.0, (depth + thickness) / 2.0);
    parts.frame.append(&ring(fr, r, &[(0.0, 0.0, fw), (w.1 + 2.0, 0.0, fw), (w.1 + 3.0, 0.0, fw + 8.0), (depth, 0.0, fw + 8.0)], false));
    let clear = Rect { u0: r.u0 + fw + 2.0, u1: r.u1 - fw - 2.0, v0: r.v0 + 10.0, v1: r.v1 - fw - 2.0 };
    leaf(&mut parts, fr, clear, w, false, true, true);
    parts
}

/// Sloped sill with a 5 mm underside drip groove. The back seats at the nominal opening
/// bottom; no part rises into the opening. Composite sills stop at their door jamb.
pub(super) fn sill(fr: &FacadeFrame, u: (f64, f64), z: f64, back: f64) -> Mesh {
    let profile = [
        [-40.0, z - 12.0],
        [back, z],
        [back, z - 32.0],
        [-18.0, z - 44.0],
        [-18.0, z - 39.0],
        [-23.0, z - 39.0],
        [-23.0, z - 44.0],
        [-40.0, z - 44.0],
    ];
    let mut m = Mesh::default();
    for k in 0..profile.len() {
        let j = (k + 1) % profile.len();
        let (a, b) = (profile[k], profile[j]);
        m.quad(fr.point(u.0, a[0], a[1]), fr.point(u.1, a[0], a[1]), fr.point(u.1, b[0], b[1]), fr.point(u.0, b[0], b[1]));
    }
    for (u, reverse) in [(u.0, true), (u.1, false)] {
        let mut cap = Mesh::default();
        cap.polygon(&profile, &[], |q| fr.point(u, q[0], q[1]));
        if reverse {
            for t in &mut cap.triangles {
                t.swap(1, 2);
            }
        }
        m.append(&cap);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn facade() -> FacadeFrame {
        FacadeFrame::new([0.0, 0.0], [12000.0, 0.0]).unwrap()
    }

    fn fill(rect: Rect, kind: &ComponentKind) -> Vec<(Material, Mesh)> {
        fill_parts(&facade(), rect, kind, Fill { wall: 400.0, finish: Material::Wall, floor: 0.0, ceiling: 4500.0 })
    }

    // An inward ray through the facade: exclude triangles edge-on to the facade, and test
    // barycentric coordinates in its (u, v) plane. This also checks glass hidden inside bars.
    fn covers(mesh: &Mesh, [u, v]: [f64; 2]) -> bool {
        mesh.triangles.iter().any(|tri| {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let cross = |x: [f64; 2], y: [f64; 2]| x[0] * y[1] - x[1] * y[0];
            let ab = [b[0] - a[0], b[2] - a[2]];
            let ac = [c[0] - a[0], c[2] - a[2]];
            let ap = [u - a[0], v - a[2]];
            let d = cross(ab, ac);
            if d.abs() < 1e-9 {
                return false;
            }
            let (s, t) = (cross(ap, ac) / d, cross(ab, ap) / d);
            s >= -1e-9 && t >= -1e-9 && s + t <= 1.0 + 1e-9
        })
    }

    #[test]
    fn glazing_is_seated_per_light_instead_of_intersecting_mullions_and_transoms() {
        let parts = fill(Rect { u0: 0.0, u1: 1200.0, v0: 850.0, v1: 2350.0 }, &ComponentKind::Window { lights: 2, top_light: 300.0 });
        let glass = &parts.iter().find(|(material, _)| *material == Material::Glass).unwrap().1;
        let (lo, hi) = glass.bounds().unwrap();
        let sash_back = REVEAL + FRAME_DEPTH - 12.0;
        assert_eq!(hi[1] - lo[1], 6.0, "presentation pane thickness");
        assert_eq!(hi[1], sash_back - 11.0, "pane must seat 1 mm before the rear stop");
        assert!(!covers(glass, [600.0, 1400.0]), "glass must not run through the vertical mullion");
        // Check the whole middle of the cross rail, allowing family profile widths to change.
        assert!((1975..=2025).any(|v| !covers(glass, [300.0, f64::from(v)])), "top lights need separate panes either side of the transom");
        assert!(covers(glass, [300.0, 1400.0]) && covers(glass, [900.0, 2200.0]), "clear lights must remain glazed");
    }

    #[test]
    fn operable_fills_have_attached_hardware_and_low_thresholds() {
        for kind in [ComponentKind::Window { lights: 2, top_light: 0.0 }, ComponentKind::EntranceDoor, ComponentKind::FrenchDoor { leaves: 2 }] {
            let parts = fill(Rect { u0: 0.0, u1: 1200.0, v0: 0.0, v1: 2300.0 }, &kind);
            let hardware =
                &parts.iter().find(|(material, mesh)| *material == Material::Steel && !mesh.is_empty()).expect("operable fill needs hardware").1;
            let (lo, hi) = hardware.bounds().unwrap();
            assert!(lo[0] > 0.0 && hi[0] < 1200.0 && lo[1] >= REVEAL - 30.0 && hi[1] <= REVEAL + FRAME_DEPTH + 30.0);
            if kind.is_door() {
                let threshold = &parts.iter().find(|(material, _)| *material == Material::Sill).expect("door needs a threshold").1;
                let (lo, hi) = threshold.bounds().unwrap();
                assert!(lo[2] >= 0.0 && hi[2] <= 20.0, "threshold must not obstruct the clear opening");
            }
        }
    }

    #[test]
    fn opening_mounts_fit_the_host_wall_without_moving_the_nominal_rectangle() {
        use crate::component::{Muntins, WindowShape};
        let r = Rect { u0: 100.0, u1: 2000.0, v0: 0.0, v1: 2300.0 };
        for wall in [100.0, 150.0, 190.0, 360.0, 1500.0] {
            for kind in [
                ComponentKind::Window { lights: 2, top_light: 350.0 },
                ComponentKind::FixedWindow,
                ComponentKind::EntranceDoor,
                ComponentKind::FrenchDoor { leaves: 2 },
                ComponentKind::PanelDoor { leaves: 2, panel_columns: 2, panel_rows: 3, glazed_rows: 1 },
                ComponentKind::HistoricWindow {
                    shape: WindowShape::Segmental { rise_mm: 250.0 },
                    lights: 2,
                    top_light: 350.0,
                    panes_across: 2,
                    panes_down: 2,
                    muntins: Muntins::TrueDivided,
                    muntin_width_mm: 24.0,
                    operable: true,
                },
                ComponentKind::DoorWindow { door_width: 850.0, door_side: Side::Left, window_sill: 800.0, lights: 2 },
                ComponentKind::DoorWindow { door_width: 850.0, door_side: Side::Right, window_sill: 800.0, lights: 2 },
            ] {
                let parts = fill_parts(&facade(), r, &kind, Fill { wall, finish: Material::Wall, floor: 0.0, ceiling: 3000.0 });
                let frame = &parts.iter().find(|(m, _)| *m == Material::Frame).unwrap().1;
                let (lo, hi) = frame.bounds().unwrap();
                let reveal = REVEAL.min(wall - FRAME_DEPTH);
                assert_eq!((lo[1], hi[1]), (reveal, reveal + FRAME_DEPTH), "{wall} mm host, {kind:?}");
                assert_eq!((lo[0], hi[0], lo[2], hi[2]), (r.u0, r.u1, r.v0, r.v1), "nominal opening must remain unchanged");
                for (material, mesh) in parts {
                    sound(&mesh);
                    let (lo, hi) = mesh.bounds().unwrap();
                    match material {
                        Material::Sill => assert!(hi[1] <= wall, "sill/threshold must seat in its host"),
                        Material::Steel => assert!(lo[1] >= reveal - 30.0 && hi[1] <= reveal + FRAME_DEPTH + 30.0, "hardware stays attached"),
                        _ => assert!(lo[1] >= 0.0 && hi[1] <= wall, "{kind:?} {material:?} is outside {wall} mm host: {lo:?}, {hi:?}"),
                    }
                }
            }
        }
    }

    fn sound(mesh: &Mesh) {
        use std::collections::BTreeMap;
        assert!(mesh.positions.iter().flatten().all(|v| v.is_finite()));
        let key = |p: [f64; 3]| p.map(|v| (v * 1e6).round() as i64);
        let mut edges = BTreeMap::<_, (usize, i32)>::new();
        let mut volume = 0.0;
        for tri in &mesh.triangles {
            let [a, b, c] = tri.map(|i| mesh.positions[i as usize]);
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let cross = [ab[1] * ac[2] - ab[2] * ac[1], ab[2] * ac[0] - ab[0] * ac[2], ab[0] * ac[1] - ab[1] * ac[0]];
            assert!(cross.iter().map(|v| v * v).sum::<f64>() > 1e-10, "degenerate triangle {a:?}, {b:?}, {c:?}");
            volume += a.iter().zip(cross).map(|(v, n)| v * n).sum::<f64>() / 6.0;
            for (a, b) in [(key(a), key(b)), (key(b), key(c)), (key(c), key(a))] {
                let (edge, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let entry = edges.entry(edge).or_default();
                entry.0 += 1;
                entry.1 += sign;
            }
        }
        assert!(volume > 0.0, "outward closed mesh must have positive volume, got {volume}");
        assert!(
            edges.values().all(|&(count, winding)| count == 2 && winding == 0),
            "mesh has an open, duplicate or reversed edge: {:?}",
            edges.iter().find(|(_, (count, winding))| *count != 2 || *winding != 0)
        );
    }

    #[test]
    fn profiles_panels_and_sill_are_closed_outward_solids_without_corner_overlays() {
        let fr = facade();
        let r = Rect { u0: 0.0, u1: 900.0, v0: 0.0, v1: 2100.0 };
        for bottom in [true, false] {
            sound(&super::ring(&fr, r, &[(120.0, 0.0, 45.0), (180.0, 0.0, 45.0), (181.0, 0.0, 53.0), (190.0, 0.0, 53.0)], bottom));
        }
        sound(&super::panel(&fr, r, (40.0, 80.0)));
        sound(&super::barrel(&fr, 40.0, 120.0, 300.0, 6.0, 40.0));
        sound(&super::sill(&fr, (0.0, 900.0), 850.0, 120.0));
    }

    #[test]
    fn supported_extremes_keep_finite_positive_detail_and_bounded_mesh_cost() {
        for width in [300.0, 1200.0, 6000.0] {
            for height in [300.0, 1500.0, 4000.0] {
                let r = Rect { u0: 100.0, u1: 100.0 + width, v0: 850.0, v1: 850.0 + height };
                for kind in [
                    ComponentKind::Window { lights: 1, top_light: 0.0 },
                    ComponentKind::Window { lights: 8, top_light: height / 2.0 },
                    ComponentKind::FixedWindow,
                    ComponentKind::EntranceDoor,
                    ComponentKind::FrenchDoor { leaves: 4 },
                    ComponentKind::FrenchBalcony { leaves: 2, rail_height: 1000.0 },
                    ComponentKind::Balcony { leaves: 1, depth: 1200.0, slab_width: 6000.0, rail_height: 1000.0 },
                ] {
                    let parts = fill(r, &kind);
                    let count: usize = parts.iter().map(|(_, mesh)| mesh.triangles.len()).sum();
                    assert!(count < 7000, "{kind:?} has {count} triangles");
                    for (material, mesh) in parts {
                        sound(&mesh);
                        let (lo, hi) = mesh.bounds().unwrap();
                        let margin = if material == Material::Sill { 30.0 } else { 0.0 };
                        assert!(lo[0] >= r.u0 - margin - 1e-6 && hi[0] <= r.u1 + margin + 1e-6, "{kind:?} {material:?} escaped width: {lo:?} {hi:?}");
                        assert!(hi[2] <= r.v1 && lo[2] >= r.v0 - 44.0);
                    }
                }
            }
        }
        for depth in [40.0, 150.0, 1000.0] {
            for width in [300.0, 900.0, 4000.0] {
                for (_, mesh) in super::interior(&facade(), Rect { u0: 0.0, u1: width, v0: 0.0, v1: 2100.0 }, depth).finish() {
                    sound(&mesh);
                }
            }
        }
    }

    #[test]
    fn door_window_mirrors_the_entire_assembly_and_preserves_the_requested_split() {
        let r = Rect { u0: 0.0, u1: 1900.0, v0: 0.0, v1: 2300.0 };
        for lights in [1, 2, 4] {
            let kind = |door_side| ComponentKind::DoorWindow { door_width: 850.0, door_side, window_sill: 800.0, lights };
            let left = fill(r, &kind(Side::Left));
            let right = fill(r, &kind(Side::Right));
            // Compare geometric vertices, independent of triangulation order and winding.
            let points = |parts: Vec<(Material, Mesh)>, mirror| {
                let mut all: Vec<_> = parts
                    .into_iter()
                    .flat_map(|(m, mesh)| {
                        mesh.positions.into_iter().map(move |p| {
                            (
                                m,
                                [
                                    (if mirror { r.u1 - p[0] } else { p[0] } * 1e6).round() as i64,
                                    (p[1] * 1e6).round() as i64,
                                    (p[2] * 1e6).round() as i64,
                                ],
                            )
                        })
                    })
                    .collect();
                all.sort_unstable();
                all.dedup();
                all
            };
            assert_eq!(points(left, true), points(right, false));
        }
        for width in [800.0, 6000.0] {
            let r = Rect { u1: width, ..r };
            let (door, window) = opening_split(r, 500.0, Side::Left);
            assert_eq!(door.u1 - door.u0, 500.0);
            assert_eq!(window.u0, door.u1);
        }
    }

    #[test]
    fn wall_voids_clear_the_glazing_without_changing_opening_identity_or_placement() {
        // A public-safe reproduction of the facade/window intersection report. Probe both
        // panes and the mullion through thin and thick masonry, retaining the 120 mm reveal.
        for thickness in [100.0, 360.0, 700.0, 1500.0] {
            let mut doc = Document::default();
            let mut b = Building::new("b", "Opening clearance", &[[0.0, 0.0], [12000.0, 0.0], [12000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap();
            b.wall_thickness = thickness;
            b.facades[0].bays = vec![2400.0];
            b.facades[0].placements.push(serde_json::from_value(serde_json::json!({"level":0,"bay":0,"type":"window.casement.2"})).unwrap());
            doc.buildings.push(b);
            let before = doc.clone();
            let built = build(&doc).unwrap();
            let wall = &built[0].walls[0];
            let opening = &wall.openings[0];
            assert_eq!(opening.rect, Rect { u0: 1800.0, u1: 3000.0, v0: 850.0, v1: 2350.0 });
            assert_eq!(opening.fill.id, "b.f1.L0.B0");
            assert_eq!(opening.fill.type_id, "window.casement.2");
            assert_eq!((opening.fill.width, opening.fill.height, opening.fill.status), (1200.0, 1500.0, Status::Existing));
            for u in [2000.0, 2400.0, 2800.0] {
                for v in [1100.0, 1800.0, 2200.0] {
                    assert!(!covers(&wall.mesh, [u, v]), "wall skin crosses the opening at {u}, {v}, thickness {thickness}");
                    for segment in &wall.segments {
                        assert!(!covers(&segment.mesh, [u, v]), "wall segment crosses the opening");
                    }
                }
            }
            for (_, mesh) in opening.fill.parts.iter().filter(|(m, _)| *m == Material::Glass) {
                assert!(
                    mesh.positions
                        .iter()
                        .all(|p| p[0] > opening.rect.u0 && p[0] < opening.rect.u1 && p[2] > opening.rect.v0 && p[2] < opening.rect.v1)
                );
            }
            assert_eq!(build(&doc).unwrap(), built, "profiles and hardware must be deterministic");
            assert_eq!(doc, before, "building geometry is read-only");
            assert_eq!(serde_json::from_str::<Document>(&serde_json::to_string(&doc).unwrap()).unwrap(), before);
        }
    }

    #[test]
    fn composite_extremes_do_not_invert_panels_or_close_the_door_with_masonry() {
        for (width, height, door_width) in [(800.0, 400.0, 500.0), (1900.0, 2300.0, 850.0), (6000.0, 4000.0, 500.0)] {
            for side in [Side::Left, Side::Right] {
                for sill in [0.0, height - 400.0] {
                    let kind = ComponentKind::DoorWindow { door_width, door_side: side, window_sill: sill, lights: 4 };
                    let r = Rect { u0: 0.0, u1: width, v0: 0.0, v1: height };
                    for (material, mesh) in fill(r, &kind) {
                        sound(&mesh);
                        if material == Material::Wall {
                            let door_centre = if side == Side::Left { door_width / 2.0 } else { width - door_width / 2.0 };
                            assert!(!covers(&mesh, [door_centre, height * 0.5]), "composite infill obstructed its door");
                        }
                        for triangle in &mesh.triangles {
                            let [a, b, c] = triangle.map(|i| mesh.positions[i as usize]);
                            assert!(a != b && b != c && a != c, "collapsed composite triangle");
                        }
                    }
                }
            }
        }
        // Old validation permits an oversized loggia door; a positive fallback subdivision
        // must still build safely, without rewriting its stored type or nominal rectangle.
        let kind = ComponentKind::Loggia { depth: 1400.0, rail_height: 1000.0, door_width: 500.0, door_side: Side::Left };
        for (_, mesh) in fill(Rect { u0: 0.0, u1: 300.0, v0: 0.0, v1: 300.0 }, &kind) {
            sound(&mesh);
        }
    }

    #[test]
    fn historic_families_stay_sound_at_supported_dimension_extremes() {
        use crate::component::{Muntins, WindowShape};
        for (width, height) in [(300.0, 300.0), (300.0, 4000.0), (6000.0, 300.0), (6000.0, 4000.0)] {
            for shape in [
                WindowShape::Rectangular,
                WindowShape::Segmental { rise_mm: 20.0 },
                WindowShape::Segmental { rise_mm: (width / 2.0_f64).min(height * 0.45) },
            ] {
                for mode in [Muntins::Applied, Muntins::TrueDivided] {
                    let kind = ComponentKind::HistoricWindow {
                        shape: shape.clone(),
                        lights: 4,
                        top_light: height / 2.0,
                        panes_across: 3,
                        panes_down: 4,
                        muntins: mode,
                        muntin_width_mm: 60.0,
                        operable: true,
                    };
                    for (material, mesh) in fill(Rect { u0: 0.0, u1: width, v0: 0.0, v1: height }, &kind) {
                        assert!(std::panic::catch_unwind(|| sound(&mesh)).is_ok(), "{width} × {height}, {kind:?}, {material:?}");
                    }
                }
            }
            for (_, mesh) in fill(
                Rect { u0: 0.0, u1: width, v0: 0.0, v1: height },
                &ComponentKind::PanelDoor { leaves: 2, panel_columns: 3, panel_rows: 6, glazed_rows: 3 },
            ) {
                sound(&mesh);
            }
        }
        for size in [300.0, 4000.0] {
            let kind = ComponentKind::HistoricWindow {
                shape: WindowShape::Round,
                lights: 1,
                top_light: 0.0,
                panes_across: 6,
                panes_down: 8,
                muntins: Muntins::TrueDivided,
                muntin_width_mm: 60.0,
                operable: false,
            };
            for (_, mesh) in fill(Rect { u0: 0.0, u1: size, v0: 0.0, v1: size }, &kind) {
                sound(&mesh);
            }
        }
    }

    #[test]
    fn historic_muntins_distinguish_real_panes_from_applied_bars() {
        use crate::component::{Muntins, WindowShape};
        let r = Rect { u0: 0.0, u1: 1200.0, v0: 0.0, v1: 1400.0 };
        for mode in [Muntins::Applied, Muntins::TrueDivided] {
            let kind = ComponentKind::HistoricWindow {
                shape: WindowShape::Rectangular,
                lights: 1,
                top_light: 0.0,
                panes_across: 2,
                panes_down: 2,
                muntins: mode,
                muntin_width_mm: 22.0,
                operable: false,
            };
            let parts = fill(r, &kind);
            let glass = &parts.iter().find(|(m, _)| *m == Material::Glass).unwrap().1;
            assert_eq!(covers(glass, [600.0, 700.0]), mode == Muntins::Applied, "applied bars retain one pane; true bars divide glazing");
            for (_, mesh) in &parts {
                sound(mesh);
            }
        }
    }

    #[test]
    fn round_and_segmental_voids_are_real_and_preserve_nominal_extents() {
        use crate::component::{Muntins, WindowShape};
        for shape in [WindowShape::Round, WindowShape::Segmental { rise_mm: 220.0 }] {
            let mut doc = Document::default();
            let kind = ComponentKind::HistoricWindow {
                shape,
                lights: 1,
                top_light: 0.0,
                panes_across: 1,
                panes_down: 1,
                muntins: Muntins::TrueDivided,
                muntin_width_mm: 22.0,
                operable: false,
            };
            doc.types.push(ComponentType {
                id: "shape".into(),
                name: "Original shaped fixture".into(),
                width: 1200.0,
                height: 1200.0,
                sill: 1000.0,
                kind,
            });
            let mut b = Building::new("b", "Profile fixture", &[[0.0, 0.0], [6000.0, 0.0], [6000.0, 6000.0], [0.0, 6000.0]], 2, 3000.0).unwrap();
            b.plinth = Some(crate::doc::Plinth { height: 2200.0, projection: 20.0 });
            b.facades[0].bands.push(crate::doc::Band { levels: vec![0], bottom: 2000.0, top: 2200.0, projection: 40.0 });
            b.facades[0].bays = vec![3000.0];
            b.facades[0].placements.push(serde_json::from_value(serde_json::json!({"level":0,"bay":0,"type":"shape"})).unwrap());
            doc.buildings.push(b);
            let built = build(&doc).unwrap();
            let wall = &built[0].walls[0];
            let opening = &wall.openings[0];
            assert_eq!(opening.rect, Rect { u0: 2400.0, u1: 3600.0, v0: 1000.0, v1: 2200.0 });
            assert!(covers(&wall.mesh, [2420.0, 2180.0]), "masonry remains outside the curved head");
            assert!(!covers(&wall.mesh, [3000.0, 1600.0]), "the centre is an actual wall void");
            for finish in [&wall.plinth, &wall.trim] {
                assert!(covers(finish, [2420.0, 2180.0]), "facing retains material outside the curved head");
                assert!(!covers(finish, [3000.0, 2100.0]), "facing must not obstruct the curved light");
            }
            for (_, mesh) in &opening.fill.parts {
                sound(mesh);
            }
            let volume = |m: &Mesh| {
                m.triangles
                    .iter()
                    .map(|t| {
                        let [a, b, c] = t.map(|i| m.positions[i as usize]);
                        (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6.0
                    })
                    .sum::<f64>()
            };
            let expected = wall.segments.iter().map(|s| crate::doc::polygon_area(&s.plan) * (s.z1 - s.z0)).sum::<f64>()
                - crate::doc::polygon_area(&opening.profile) * 400.0;
            assert!((volume(&wall.mesh) - expected).abs() < expected * 1e-6, "curved wall volume {} vs {expected}", volume(&wall.mesh));
        }
    }
}
