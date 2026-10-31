//! Walls: facade pieces (one per facade and storey) and interior walls, as standard-case walls
//! with an Axis, a vertical extrusion in their own placement, a material layer set usage, a type,
//! common properties and base quantities; and the path connections between joined walls.

use std::collections::{BTreeMap, BTreeSet};

use trigon_model::build::{BuiltInterior, BuiltWall};
use trigon_model::joins::Target;
use trigon_model::walls::WallSegment;
use trigon_model::{Building, Built, FacadeKind};

use super::catalog::{self, BuildUp, Qty, Val};
use super::part21::{Frame, Placed, area, ccw, clean_ring, gid, string};
use super::{Floors, Ifc};

/// A facade piece as written: which facade and storey, its entity and placement.
pub(crate) struct Piece<'b> {
    pub wall: String,
    pub edge: Option<usize>,
    pub seg: &'b WallSegment,
    pub id: usize,
    pub placed: Placed,
}

/// Slack when deciding whether a profile follows the wall's axis and thickness, mm.
const ON_LINE: f64 = 0.5;

impl Ifc<'_> {
    /// Every wall of a building and the connections between them. Returns the facade pieces.
    pub(super) fn walls<'b>(&mut self, b: &'b Built, bd: Option<&Building>, fl: &mut Floors) -> Vec<Piece<'b>> {
        let mut pieces = Vec::new();
        for wall in &b.walls {
            let edge = bd.and_then(|bd| bd.facade(&wall.id).ok()).map(|f| f.edge);
            for seg in &wall.segments {
                if let Some((id, placed)) = self.facade_piece(wall, seg, fl) {
                    pieces.push(Piece { wall: wall.id.clone(), edge, seg, id, placed });
                }
            }
        }
        let mut interior = BTreeMap::new();
        for iw in &b.interior {
            if let Some(id) = self.interior_wall(iw, fl) {
                interior.insert(iw.id.clone(), id);
            }
        }
        if let Some(bd) = bd {
            self.connections(b, bd, &pieces, &interior);
        }
        pieces
    }

    fn facade_piece(&mut self, wall: &BuiltWall, seg: &WallSegment, fl: &mut Floors) -> Option<(usize, Placed)> {
        let fr = wall.frame;
        // x along the outer face (the reference line), y into the building.
        let frame = Frame::plan([fr.origin[0], fr.origin[1], seg.z0], fr.t)?;
        let p = seg.plan.map(|q| frame.local2(q));
        let (len, t, h) = (fr.length, seg.thickness, seg.z1 - seg.z0);
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < ON_LINE && (a[1] - b[1]).abs() < ON_LINE;
        let standard = near(p[0], [0.0, 0.0])
            && near(p[1], [len, 0.0])
            && (p[2][1] - t).abs() < ON_LINE
            && (p[3][1] - t).abs() < ON_LINE
            && p[2][0] - p[3][0] > 1.0;
        let ring = if standard { p.to_vec() } else { ccw(clean_ring(&p)) };
        if ring.len() < 3 || !(area(&ring) > 1.0) || !(h > 1.0) {
            return None;
        }
        let up = catalog::facade(t, wall.kind == FacadeKind::Party);
        let placed = self.w.place(Some(fl.at(seg.level)), frame);
        let axis = self.w.polyline(&[[0.0, 0.0], [len, 0.0]], false);
        let solid = self.extruded(&ring, &[], h, up.face());
        let shape = self.shape("SweptSolid", &[solid], Some(axis), catalog::L_FACADE)?;
        let class = if standard { "IFCWALLSTANDARDCASE" } else { "IFCWALL" };
        let id = self.w.add(format!(
            "{class}({},$,{},$,$,#{},#{shape},{},.{}.)",
            gid(&seg.id),
            string(&up.name),
            placed.id,
            string(&seg.id),
            up.predefined
        ));
        let cut = opening_area(wall, seg);
        let side = (len + (p[2][0] - p[3][0]).max(0.0)) / 2.0 * h;
        let footprint = area(&ring);
        let qs = [
            Qty::Length("Length", len),
            Qty::Length("Width", t),
            Qty::Length("Height", h),
            Qty::Area("GrossFootprintArea", footprint / 1e6),
            Qty::Area("NetFootprintArea", footprint / 1e6),
            Qty::Area("GrossSideArea", side / 1e6),
            Qty::Area("NetSideArea", (side - cut).max(0.0) / 1e6),
            Qty::Volume("GrossVolume", footprint * h / 1e9),
            Qty::Volume("NetVolume", net_volume(wall, seg, footprint) / 1e9),
        ];
        self.wall_data(id, &seg.id, &up, standard.then_some(0.0), wall.kind != FacadeKind::Party, true, &qs);
        fl.contain(seg.level, id);
        Some((id, placed))
    }

    fn interior_wall(&mut self, iw: &BuiltInterior, fl: &mut Floors) -> Option<usize> {
        let [a, b] = iw.axis;
        let dir = [b[0] - a[0], b[1] - a[1]];
        let len = dir[0].hypot(dir[1]);
        // x along the centre line, y to its left.
        let frame = Frame::plan([a[0], a[1], iw.z_bottom], dir).unwrap_or(Frame::at([a[0], a[1], iw.z_bottom]));
        let p = iw.outline.map(|q| frame.local2(q));
        let (t, h) = (iw.thickness, iw.height);
        let half = t / 2.0;
        let standard = len > 1.0
            && (p[0][1] + half).abs() < ON_LINE
            && (p[1][1] + half).abs() < ON_LINE
            && (p[2][1] - half).abs() < ON_LINE
            && (p[3][1] - half).abs() < ON_LINE
            && p[1][0] - p[0][0] > 1.0
            && p[2][0] - p[3][0] > 1.0;
        let ring = if standard { p.to_vec() } else { ccw(clean_ring(&p)) };
        if ring.len() < 3 || !(area(&ring) > 1.0) || !(h > 1.0) {
            return None;
        }
        let up = catalog::interior(t, iw.load_bearing);
        let placed = self.w.place(Some(fl.at(iw.level)), frame);
        let axis = (len > 1.0).then(|| self.w.polyline(&[[0.0, 0.0], [len, 0.0]], false));
        let solid = self.extruded(&ring, &[], h, up.face());
        let shape = self.shape("SweptSolid", &[solid], axis, catalog::L_INTERIOR)?;
        let class = if standard { "IFCWALLSTANDARDCASE" } else { "IFCWALL" };
        let id =
            self.w.add(format!("{class}({},$,{},$,$,#{},#{shape},{},.{}.)", gid(&iw.id), string(&up.name), placed.id, string(&iw.id), up.predefined));
        let footprint = area(&ring);
        let qs = [
            Qty::Length("Length", len),
            Qty::Length("Width", t),
            Qty::Length("Height", h),
            Qty::Area("GrossFootprintArea", footprint / 1e6),
            Qty::Area("NetFootprintArea", footprint / 1e6),
            Qty::Area("GrossSideArea", len * h / 1e6),
            Qty::Area("NetSideArea", len * h / 1e6),
            Qty::Volume("GrossVolume", footprint * h / 1e9),
            Qty::Volume("NetVolume", footprint * h / 1e9),
        ];
        self.wall_data(id, &iw.id, &up, standard.then_some(-half), false, iw.load_bearing, &qs);
        fl.contain(iw.level, id);
        Some(id)
    }

    /// Type, material, common properties and quantities of a wall. `usage_offset` is the
    /// layers' offset from the axis for a standard-case wall; other walls get the layer set.
    pub(super) fn wall_data(
        &mut self,
        id: usize,
        key: &str,
        up: &BuildUp,
        usage_offset: Option<f64>,
        external: bool,
        load_bearing: bool,
        qs: &[Qty],
    ) {
        let set = self.cat.layer_set(&mut self.w, up);
        let type_key = format!("wall:{}", up.name);
        let created = !self.cat.types.contains_key(&type_key);
        let ty = self.cat.typed(&mut self.w, &type_key, id, |w| {
            w.add(format!("IFCWALLTYPE({},$,{},$,$,$,$,$,$,.{}.)", gid(&format!("walltype:{}", up.name)), string(&up.name), up.predefined))
        });
        if created {
            self.cat.associate(&format!("set:{}", up.name), set, ty);
        }
        match usage_offset {
            Some(offset) => {
                let usage = self.cat.usage(&mut self.w, up, "AXIS2", true, offset);
                self.cat.associate(&format!("usage:{}|{offset}", up.name), usage, id);
            }
            None => self.cat.associate(&format!("set:{}", up.name), set, id),
        }
        self.common("Pset_WallCommon", id, &up.name, None, vec![("IsExternal", Val::Bool(external)), ("LoadBearing", Val::Bool(load_bearing))]);
        self.cat.quantities(&mut self.w, key, "Qto_WallBaseQuantities", qs, id);
    }

    /// Path connections: facade pieces meeting at the footprint's corners, and interior walls
    /// ending on a facade or on another wall.
    fn connections(&mut self, b: &Built, bd: &Building, pieces: &[Piece], interior: &BTreeMap<String, usize>) {
        let n = bd.footprint.len();
        let piece = |edge: usize, level: usize| pieces.iter().find(|p| p.edge == Some(edge) && p.seg.level == level);
        let mut done: BTreeSet<(usize, usize)> = BTreeSet::new();
        let mut connect = |x: &mut Self, (a, ka, ta): (usize, &str, &str), (c, kc, tc): (usize, &str, &str)| {
            if a == c || !done.insert((a.min(c), a.max(c))) {
                return;
            }
            x.w.add(format!("IFCRELCONNECTSPATHELEMENTS({},$,$,$,$,#{a},#{c},(),(),.{tc}.,.{ta}.)", gid(&format!("{ka}.connects.{kc}"))));
        };
        for p in pieces {
            let Some(edge) = p.edge else { continue };
            if let Some(next) = piece((edge + 1) % n.max(1), p.seg.level) {
                connect(self, (p.id, &p.seg.id, "ATEND"), (next.id, &next.seg.id, "ATSTART"));
            }
        }
        for iw in &b.interior {
            let Some(&id) = interior.get(&iw.id) else { continue };
            for (e, end) in iw.ends.iter().enumerate() {
                let Some(join) = end else { continue };
                let at = if e == 0 { "ATSTART" } else { "ATEND" };
                match join.target {
                    Target::Facade(edge) => {
                        if let Some(p) = piece(edge, iw.level) {
                            connect(self, (id, &iw.id, at), (p.id, &p.seg.id, "ATPATH"));
                        }
                    }
                    Target::Wall(j) => {
                        let Some(other) = on_storey(bd, iw.level, j) else { continue };
                        let (Some(&oid), Some(ow)) = (interior.get(&other), b.interior.iter().find(|w| w.id == other)) else { continue };
                        let point = iw.axis[e];
                        let slack = iw.thickness + ow.thickness / 2.0 + 60.0;
                        let target_at = where_on(ow, point, slack);
                        connect(self, (id, &iw.id, at), (oid, &ow.id, target_at));
                    }
                }
            }
        }
    }
}

/// The area of a piece's openings in the wall's elevation, mm².
fn opening_area(wall: &BuiltWall, seg: &WallSegment) -> f64 {
    seg.openings
        .iter()
        .filter_map(|&k| wall.openings.get(k))
        .map(|o| (o.rect.u1 - o.rect.u0).max(0.0) * (o.rect.v1.min(seg.z1) - o.rect.v0.max(seg.z0)).max(0.0))
        .sum()
}

/// A facade piece's volume as exported: its plan outline (`footprint`, mm²) times its height,
/// less its openings through the wall, mm³. Where Trigon's piece is a closed solid this is its
/// volume (tests check it); the export doesn't take the mesh's volume because a piece whose
/// inner corner collapsed (a short jog in a thick wall) is not always closed.
pub(crate) fn net_volume(wall: &BuiltWall, seg: &WallSegment, footprint: f64) -> f64 {
    (footprint * (seg.z1 - seg.z0) - opening_area(wall, seg) * seg.thickness).max(0.0)
}

/// The id of the `j`th interior wall on a storey, as `build` names it.
fn on_storey(bd: &Building, level: usize, j: usize) -> Option<String> {
    let (k, w) = bd.interior_walls.iter().enumerate().filter(|(_, w)| w.levels.contains(&level)).nth(j)?;
    let id = if w.id.is_empty() { format!("{}.iw{k}", bd.id) } else { w.id.clone() };
    Some(format!("{id}.L{level}"))
}

/// Where along wall `w` the point lies: at its start, its end, or along its path.
fn where_on(w: &BuiltInterior, p: [f64; 2], slack: f64) -> &'static str {
    let [a, b] = w.axis;
    let d = [b[0] - a[0], b[1] - a[1]];
    let len = d[0].hypot(d[1]);
    if !(len > 1.0) {
        return "NOTDEFINED";
    }
    let s = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len;
    if s < slack {
        "ATSTART"
    } else if s > len - slack {
        "ATEND"
    } else {
        "ATPATH"
    }
}
