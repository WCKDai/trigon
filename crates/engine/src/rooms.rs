//! Rooms from walls: the spaces the walls enclose on a storey, with door gaps closed.
//!
//! Each interior wall end is extended along its wall by up to [`DOOR`] mm until it meets another
//! wall or the outer walls' inside face (that closes door openings without inventing walls across
//! rooms); then the free space inside the outer walls is flood-filled on a 50 mm grid.

use trigon_model::{Building, Room};

/// Widest door gap closed, mm.
const DOOR: f64 = 1100.0;
const CELL: f64 = 50.0;

fn seg_hit(p: [f64; 2], d: [f64; 2], a: [f64; 2], b: [f64; 2]) -> Option<f64> {
    let e = [b[0] - a[0], b[1] - a[1]];
    let den = d[0] * e[1] - d[1] * e[0];
    if den.abs() < 1e-9 {
        return None;
    }
    let t = ((a[0] - p[0]) * e[1] - (a[1] - p[1]) * e[0]) / den;
    let s = ((a[0] - p[0]) * d[1] - (a[1] - p[1]) * d[0]) / den;
    (t > 1.0 && (0.0..=1.0).contains(&s)).then_some(t)
}

/// The rooms of storey `level`: polygons in site mm with their areas (m²), largest first.
/// Extra blocking from a drawing: true where the site point (mm) is on drawn line work.
pub(crate) type Ink<'a> = Option<&'a dyn Fn([f64; 2]) -> bool>;

pub(crate) fn detect(b: &Building, level: usize, min_area_m2: f64, ink: Ink<'_>) -> Vec<(Vec<[f64; 2]>, f64)> {
    let inner = b.inner_line(level);
    let walls: Vec<&trigon_model::InteriorWall> = b.interior_walls.iter().filter(|w| w.levels.contains(&level)).collect();
    // Lines that block: outer faces, interior walls as their two faces, door closers.
    let n = inner.len();
    let mut edges: Vec<([f64; 2], [f64; 2])> = (0..n).map(|i| (inner[i], inner[(i + 1) % n])).collect();
    let mut solids: Vec<[[f64; 2]; 4]> = Vec::new();
    for w in &walls {
        let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        let (nx, ny) = (-dy / l * w.thickness.max(CELL * 2.0) / 2.0, dx / l * w.thickness.max(CELL * 2.0) / 2.0);
        let q = [[w.a[0] + nx, w.a[1] + ny], [w.b[0] + nx, w.b[1] + ny], [w.b[0] - nx, w.b[1] - ny], [w.a[0] - nx, w.a[1] - ny]];
        for k in 0..4 {
            edges.push((q[k], q[(k + 1) % 4]));
        }
        solids.push(q);
    }
    let mut closers: Vec<[[f64; 2]; 4]> = Vec::new();
    for w in &walls {
        let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        let u = [dx / l, dy / l];
        for (p, d) in [(w.b, u), (w.a, [-u[0], -u[1]])] {
            let hit = edges.iter().filter_map(|(a, e)| seg_hit(p, d, *a, *e)).fold(f64::INFINITY, f64::min);
            if hit.is_finite() && hit <= DOOR {
                let end = [p[0] + d[0] * hit, p[1] + d[1] * hit];
                let (nx, ny) = (-d[1] * CELL, d[0] * CELL);
                closers.push([[p[0] + nx, p[1] + ny], [end[0] + nx, end[1] + ny], [end[0] - nx, end[1] - ny], [p[0] - nx, p[1] - ny]]);
            }
        }
    }
    solids.extend(closers);
    // Rasterise.
    let first = match inner.first() {
        Some(p) => *p,
        None => return Vec::new(),
    };
    let (lo, hi) = inner.iter().fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]));
    let (w, h) = (((hi[0] - lo[0]) / CELL).ceil() as usize + 2, ((hi[1] - lo[1]) / CELL).ceil() as usize + 2);
    if w * h > 30_000_000 {
        return Vec::new();
    }
    let centre = |i: usize, j: usize| [lo[0] + (i as f64 - 0.5) * CELL, lo[1] + (j as f64 - 0.5) * CELL];
    let mut blocked = vec![true; w * h];
    // Inside the outer walls: scanline fill of the inner ring.
    for j in 0..h {
        let y = centre(0, j)[1];
        let mut xs: Vec<f64> = Vec::new();
        for k in 0..n {
            let (a, e) = (inner[k], inner[(k + 1) % n]);
            if (a[1] > y) != (e[1] > y) {
                xs.push(a[0] + (y - a[1]) * (e[0] - a[0]) / (e[1] - a[1]));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks(2) {
            if let [x0, x1] = pair {
                let (i0, i1) = (((x0 - lo[0]) / CELL + 0.5).ceil().max(0.0) as usize, ((x1 - lo[0]) / CELL + 0.5).floor().max(0.0) as usize);
                for i in i0..=i1.min(w - 1) {
                    blocked[j * w + i] = false;
                }
            }
        }
    }
    for q in &solids {
        let (ymin, ymax) = q.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        let (j0, j1) = (((ymin - lo[1]) / CELL).floor().max(0.0) as usize, (((ymax - lo[1]) / CELL).ceil() as usize + 1).min(h - 1));
        for j in j0..=j1 {
            let y = centre(0, j)[1];
            let mut xs: Vec<f64> = Vec::new();
            for k in 0..4 {
                let (a, e) = (q[k], q[(k + 1) % 4]);
                if (a[1] > y) != (e[1] > y) {
                    xs.push(a[0] + (y - a[1]) * (e[0] - a[0]) / (e[1] - a[1]));
                }
            }
            xs.sort_by(f64::total_cmp);
            if let [x0, .., x1] = xs.as_slice() {
                let (i0, i1) = (((x0 - lo[0]) / CELL).floor().max(0.0) as usize, (((x1 - lo[0]) / CELL).ceil() as usize + 1).min(w - 1));
                for i in i0..=i1 {
                    blocked[j * w + i] = true;
                }
            }
        }
    }
    // Drawn line work blocks the first fill only; the rooms grow back over it afterwards.
    let mut inked = vec![false; w * h];
    if let Some(ink) = ink {
        for j in 0..h {
            for i in 0..w {
                let k = j * w + i;
                if !blocked[k] && ink(centre(i, j)) {
                    inked[k] = true;
                }
            }
        }
    }
    // Flood fill the free cells.
    let mut label = vec![0u32; w * h];
    let mut sizes: Vec<usize> = vec![0];
    let mut next = 1u32;
    for start in 0..w * h {
        if blocked[start] || inked[start] || label[start] != 0 {
            continue;
        }
        let mut stack = vec![start];
        label[start] = next;
        let mut count = 0usize;
        while let Some(c) = stack.pop() {
            count += 1;
            let (i, j) = (c % w, c / w);
            for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
                let (ni, nj) = (i as i64 + di, j as i64 + dj);
                if ni < 0 || nj < 0 || ni >= w as i64 || nj >= h as i64 {
                    continue;
                }
                let k = nj as usize * w + ni as usize;
                if !blocked[k] && !inked[k] && label[k] == 0 {
                    label[k] = next;
                    stack.push(k);
                }
            }
        }
        sizes.push(count);
        next += 1;
    }
    // Rooms are the regions of at least the minimum area; they grow over ink, text and the small
    // pockets between lines (never through modelled walls) until they meet halfway across each line.
    let min_cells = (min_area_m2 * 1e6 / (CELL * CELL)) as usize;
    let mut queue = std::collections::VecDeque::new();
    for (k, l) in label.iter_mut().enumerate() {
        if *l != 0 && sizes.get(*l as usize).copied().unwrap_or(0) < min_cells {
            *l = 0;
        }
        if *l != 0 {
            queue.push_back(k);
        }
    }
    while let Some(c) = queue.pop_front() {
        let (i, j) = (c % w, c / w);
        for (di, dj) in [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)] {
            let (ni, nj) = (i as i64 + di, j as i64 + dj);
            if ni < 0 || nj < 0 || ni >= w as i64 || nj >= h as i64 {
                continue;
            }
            let k = nj as usize * w + ni as usize;
            if !blocked[k] && label[k] == 0 {
                label[k] = label[c];
                queue.push_back(k);
            }
        }
    }
    // Neighbouring rooms whose shared edge doesn't follow drawn lines are one room split by text,
    // a dimension line or a gap in the scan: merge them.
    let mut edge: std::collections::BTreeMap<(u32, u32), (usize, usize)> = std::collections::BTreeMap::new();
    for j in 0..h {
        for i in 0..w {
            let k = j * w + i;
            for n in [k + 1, k + w] {
                if (n == k + 1 && i + 1 >= w) || n >= w * h {
                    continue;
                }
                let (a, b) = (label[k], label[n]);
                if a != 0 && b != 0 && a != b {
                    let e = edge.entry((a.min(b), a.max(b))).or_insert((0, 0));
                    e.0 += 1;
                    if inked[k] || inked[n] {
                        e.1 += 1;
                    }
                }
            }
        }
    }
    let mut parent: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    fn root(p: &mut std::collections::BTreeMap<u32, u32>, x: u32) -> u32 {
        let mut r = x;
        while let Some(&q) = p.get(&r) {
            if q == r {
                break;
            }
            r = q;
        }
        r
    }
    for ((a, b), (total, inked_n)) in &edge {
        // At least 1 m of shared edge, hardly any of it on a drawn line.
        if *total >= 20 && (*inked_n as f64) < 0.3 * *total as f64 {
            let (ra, rb) = (root(&mut parent, *a), root(&mut parent, *b));
            if ra != rb {
                parent.insert(ra.max(rb), ra.min(rb));
            }
        }
    }
    for l in label.iter_mut() {
        if *l != 0 {
            *l = root(&mut parent, *l);
        }
    }
    let mut cells_of: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (k, l) in label.iter().enumerate() {
        if *l != 0 {
            cells_of.entry(*l).or_default().push(k);
        }
    }
    let mut rooms = Vec::new();
    for (id, cells) in cells_of {
        let area = cells.len() as f64 * CELL * CELL / 1e6;
        if let Some(poly) = outline(&label, w, h, id, &cells) {
            let poly: Vec<[f64; 2]> = poly.into_iter().map(|(i, j)| [lo[0] + (i as f64 - 1.0) * CELL, lo[1] + (j as f64 - 1.0) * CELL]).collect();
            rooms.push((simplify(&poly, 60.0), area));
        }
    }
    rooms.sort_by(|a, b| b.1.total_cmp(&a.1));
    rooms
}

/// The outer boundary of a labelled region as grid corners (square tracing along cell edges).
fn outline(label: &[u32], w: usize, h: usize, id: u32, cells: &[usize]) -> Option<Vec<(usize, usize)>> {
    let inside = |i: i64, j: i64| i >= 0 && j >= 0 && (i as usize) < w && (j as usize) < h && label[j as usize * w + i as usize] == id;
    // Start at the lowest, then leftmost cell's bottom-left corner, walking with the region on the left.
    let start = *cells.iter().min_by_key(|&&c| (c / w, c % w))?;
    let (si, sj) = ((start % w) as i64, (start / w) as i64);
    let mut pts = Vec::new();
    let (mut x, mut y) = (si, sj); // corner coordinates
    let mut dir = 0usize; // 0 = +x, 1 = +y, 2 = −x, 3 = −y
    let steps = [(1i64, 0i64), (0, 1), (-1, 0), (0, -1)];
    for _ in 0..(4 * cells.len() + 8) {
        pts.push((x as usize, y as usize));
        // Cells to the left and right of the edge ahead (region kept on the left).
        let (l, r) = match dir {
            0 => ((x, y), (x, y - 1)),
            1 => ((x - 1, y), (x, y)),
            2 => ((x - 1, y - 1), (x - 1, y)),
            _ => ((x, y - 1), (x - 1, y - 1)),
        };
        let (li, ri) = (inside(l.0, l.1), inside(r.0, r.1));
        if li && !ri {
            let (dx, dy) = steps[dir];
            x += dx;
            y += dy;
        } else if li && ri {
            dir = (dir + 3) % 4; // turn right
            continue;
        } else {
            dir = (dir + 1) % 4; // turn left
            continue;
        }
        if (x, y) == (si, sj) {
            break;
        }
    }
    (pts.len() >= 4).then_some(pts)
}

/// Douglas–Peucker on a closed ring.
fn simplify(ring: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    fn dp(p: &[[f64; 2]], tol: f64, out: &mut Vec<[f64; 2]>) {
        let (a, b) = (p[0], p[p.len() - 1]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt().max(1e-9);
        let (mut best, mut idx) = (0.0, 0);
        for (k, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
            let d = ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / l;
            if d > best {
                best = d;
                idx = k;
            }
        }
        if best > tol && idx > 0 {
            dp(&p[..=idx], tol, out);
            dp(&p[idx..], tol, out);
        } else {
            out.push(a);
        }
    }
    if ring.len() < 4 {
        return ring.to_vec();
    }
    // Split at the point farthest from the first so both halves are open polylines.
    let far = (1..ring.len()).max_by(|&i, &j| {
        let d = |k: usize| (ring[k][0] - ring[0][0]).powi(2) + (ring[k][1] - ring[0][1]).powi(2);
        d(i).total_cmp(&d(j))
    });
    let Some(far) = far else { return ring.to_vec() };
    let mut out = Vec::new();
    dp(&ring[..=far], tol, &mut out);
    let mut tail: Vec<[f64; 2]> = ring[far..].to_vec();
    tail.push(ring[0]);
    dp(&tail, tol, &mut out);
    out
}

pub(crate) fn rooms_for(b: &Building, level: usize, min_area_m2: f64, ink: Ink<'_>) -> Vec<Room> {
    let mut out = Vec::new();
    {
        for (k, (poly, _)) in detect(b, level, min_area_m2, ink).into_iter().enumerate() {
            out.push(Room { id: format!("{}.L{level}.r{k}", b.id), levels: vec![level], name: String::new(), polygon: poly });
        }
    }
    out
}
