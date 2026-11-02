//! Original convex balcony outlines, reused by model geometry and plan exports.

use crate::{ModelError, Result, checked};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum Plan {
    /// Straight returns followed by a half-ellipse, tessellated into 24 front segments.
    Bowed {
        return_fraction: f64,
    },
    Chamfered {
        corner_mm: f64,
    },
    /// Convex points in width/depth fractions: (-0.5,0) to (+0.5,0) around the front.
    Polygon {
        points: Vec<[f64; 2]>,
    },
}

impl Plan {
    /// Coordinates are along the facade from centre, and positive outward, in mm.
    pub fn outline(&self, width: f64, depth: f64) -> Result<Vec<[f64; 2]>> {
        checked("balcony plan width", width, 600.0, 6000.0)?;
        checked("balcony plan depth", depth, 400.0, 3000.0)?;
        let points = match self {
            Self::Bowed { return_fraction } => {
                checked("bowed return fraction", *return_fraction, 0.1, 0.7)?;
                let mut p = vec![[-width / 2.0, 0.0]];
                for k in 0..=24 {
                    let a = std::f64::consts::PI * (1.0 - f64::from(k) / 24.0);
                    p.push([width / 2.0 * a.cos(), depth * (*return_fraction + (1.0 - return_fraction) * a.sin())]);
                }
                p.push([width / 2.0, 0.0]);
                p
            }
            Self::Chamfered { corner_mm } => {
                let c = checked("balcony corner", *corner_mm, 50.0, (width / 3.0).min(depth * 0.8))?;
                vec![
                    [-width / 2.0, 0.0],
                    [-width / 2.0, depth - c],
                    [-width / 2.0 + c, depth],
                    [width / 2.0 - c, depth],
                    [width / 2.0, depth - c],
                    [width / 2.0, 0.0],
                ]
            }
            Self::Polygon { points } => {
                if !(3..=32).contains(&points.len()) {
                    return Err(ModelError::Invalid("balcony plan needs 3–32 convex points".into()));
                }
                for p in points {
                    checked("balcony plan width fraction", p[0], -0.5, 0.5)?;
                    checked("balcony plan depth fraction", p[1], 0.0, 1.0)?;
                }
                if points.first() != Some(&[-0.5, 0.0]) || points.last() != Some(&[0.5, 0.0]) {
                    return Err(ModelError::Invalid("balcony plan must start at [-0.5,0] and end at [0.5,0] on the host wall".into()));
                }
                if !points.iter().any(|p| p[1] == 1.0) {
                    return Err(ModelError::Invalid("balcony polygon must reach depth fraction 1".into()));
                }
                points.iter().map(|p| [p[0] * width, p[1] * depth]).collect()
            }
        };
        for k in 0..points.len() {
            let (a, b, c) = (points[k], points[(k + 1) % points.len()], points[(k + 2) % points.len()]);
            if (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0]) >= -1e-6 || (b[0] - a[0]).hypot(b[1] - a[1]) < 1.0 {
                return Err(ModelError::Invalid("balcony outline must be strictly convex, clockwise, with edges at least 1 mm".into()));
            }
            if points.iter().any(|p| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]) > 1e-6) {
                return Err(ModelError::Invalid("balcony outline must be a simple convex polygon".into()));
            }
        }
        // Check the 50 mm guard setback can be contained, including acute custom corners.
        let inset = guard_inset(&points);
        let sound = inset.len() >= 3 && crate::footprint::signed_area2(&inset) < -20000.0 && inset.iter().flatten().all(|p| p.is_finite());
        if !sound {
            return Err(ModelError::Invalid("balcony plan is too narrow for its inset guard".into()));
        }
        Ok(points)
    }

    /// Guard centreline, starting at the left wall return and ending at the right return.
    /// Half-plane clipping removes consumed offset edges on narrow/elongated curved plans.
    pub fn guard_path(&self, width: f64, depth: f64) -> Result<Vec<[f64; 2]>> {
        Ok(guard_inset(&self.outline(width, depth)?))
    }
}

fn guard_inset(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut poly = points.to_vec();
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)).take(points.len()) {
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        let distance = |p: [f64; 2]| ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])) / length + 50.0;
        let mut clipped = Vec::new();
        for (&p, &q) in poly.iter().zip(poly.iter().cycle().skip(1)).take(poly.len()) {
            let (da, db) = (distance(p), distance(q));
            let (ina, inb) = (da <= 1e-8, db <= 1e-8);
            if ina {
                clipped.push(p);
            }
            if ina != inb {
                let t = da / (da - db);
                clipped.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            }
        }
        clipped.dedup_by(|a, b| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-7);
        poly = clipped;
    }
    if let Some((start, _)) = poly.iter().enumerate().filter(|(_, p)| (p[1] - 50.0).abs() < 1e-6).min_by(|(_, a), (_, b)| a[0].total_cmp(&b[0])) {
        poly.rotate_left(start);
    }
    poly
}

pub fn copenhagen_types() -> Vec<crate::ComponentType> {
    [
        ("bowed", "Bowed Copenhagen balcony", Plan::Bowed { return_fraction: 0.35 }),
        ("chamfered", "Chamfered Copenhagen balcony", Plan::Chamfered { corner_mm: 400.0 }),
    ]
    .into_iter()
    .map(|(id, name, plan)| crate::ComponentType {
        id: format!("dk.balcony.{id}"),
        name: name.into(),
        width: 3000.0,
        height: 1100.0,
        sill: 0.0,
        kind: crate::ComponentKind::ShapedBalcony { depth: 1500.0, thickness: 180.0, plan },
    })
    .collect()
}
