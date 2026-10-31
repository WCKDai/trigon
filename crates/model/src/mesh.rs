//! Triangle meshes for display and exchange. Positions are site-local mm, Z up.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Material {
    Wall,
    Slab,
    Roof,
    Frame,
    Glass,
    Door,
    Sill,
    Railing,
    /// Linked reference models.
    Reference,
    /// Galvanised steel.
    Steel,
    /// Hardwood, composite or bamboo decking.
    Deck,
    /// Shown in a demolition view: what a phase tears down.
    Demolished,
    /// Rendered or concrete plinth (sokkel) at the foot of the facade.
    Plinth,
    /// Plastered interior walls.
    Plaster,
}

impl Material {
    pub const ALL: [Material; 14] = [
        Material::Wall,
        Material::Slab,
        Material::Roof,
        Material::Frame,
        Material::Glass,
        Material::Door,
        Material::Sill,
        Material::Railing,
        Material::Reference,
        Material::Steel,
        Material::Deck,
        Material::Demolished,
        Material::Plinth,
        Material::Plaster,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Material::Wall => "wall",
            Material::Slab => "slab",
            Material::Roof => "roof",
            Material::Frame => "frame",
            Material::Glass => "glass",
            Material::Door => "door",
            Material::Sill => "sill",
            Material::Railing => "railing",
            Material::Reference => "reference",
            Material::Steel => "steel",
            Material::Deck => "deck",
            Material::Demolished => "demolished",
            Material::Plinth => "plinth",
            Material::Plaster => "plaster",
        }
    }

    /// Linear RGBA used by the glTF export.
    pub fn rgba(self) -> [f32; 4] {
        match self {
            Material::Wall => [0.62, 0.30, 0.20, 1.0],
            Material::Slab => [0.70, 0.70, 0.68, 1.0],
            Material::Roof => [0.25, 0.25, 0.27, 1.0],
            Material::Frame => [0.92, 0.92, 0.90, 1.0],
            Material::Glass => [0.35, 0.50, 0.60, 0.45],
            Material::Door => [0.30, 0.18, 0.10, 1.0],
            Material::Sill => [0.80, 0.78, 0.74, 1.0],
            Material::Railing => [0.12, 0.12, 0.12, 1.0],
            Material::Reference => [0.78, 0.80, 0.84, 1.0],
            Material::Steel => [0.62, 0.64, 0.66, 1.0],
            Material::Deck => [0.55, 0.33, 0.18, 1.0],
            Material::Demolished => [0.90, 0.10, 0.10, 0.45],
            Material::Plinth => [0.42, 0.42, 0.41, 1.0],
            Material::Plaster => [0.93, 0.92, 0.89, 1.0],
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    pub fn append(&mut self, other: &Mesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.triangles.extend(other.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }

    /// Adds a planar quad `a b c d` (in order around its edge).
    pub fn quad(&mut self, a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) {
        let i = self.positions.len() as u32;
        self.positions.extend([a, b, c, d]);
        self.triangles.push([i, i + 1, i + 2]);
        self.triangles.push([i, i + 2, i + 3]);
    }

    /// Adds a box given its 8 corners: `lo[k]` is the bottom face, `hi[k]` the top, same order.
    pub fn hexahedron(&mut self, lo: [[f64; 3]; 4], hi: [[f64; 3]; 4]) {
        self.quad(lo[3], lo[2], lo[1], lo[0]);
        self.quad(hi[0], hi[1], hi[2], hi[3]);
        for k in 0..4 {
            let j = (k + 1) % 4;
            self.quad(lo[k], lo[j], hi[j], hi[k]);
        }
    }

    /// Triangulates a planar polygon with holes given in 2D and maps it to 3D with `to3d`.
    /// Returns false (adding nothing) if the polygon can't be triangulated.
    pub fn polygon(&mut self, outer: &[[f64; 2]], holes: &[Vec<[f64; 2]>], to3d: impl Fn([f64; 2]) -> [f64; 3]) -> bool {
        let mut flat: Vec<f64> = outer.iter().flat_map(|p| [p[0], p[1]]).collect();
        let mut hole_starts = Vec::with_capacity(holes.len());
        let mut all: Vec<[f64; 2]> = outer.to_vec();
        for h in holes {
            hole_starts.push(all.len());
            all.extend_from_slice(h);
            flat.extend(h.iter().flat_map(|p| [p[0], p[1]]));
        }
        let Ok(idx) = earcutr::earcut(&flat, &hole_starts, 2) else {
            return false;
        };
        let base = self.positions.len() as u32;
        self.positions.extend(all.iter().map(|&p| to3d(p)));
        self.triangles.extend(idx.as_chunks::<3>().0.iter().map(|t| [base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]));
        true
    }

    /// The part of the mesh at or below height `z` (triangles crossing it are clipped; no caps).
    pub fn below(&self, z: f64) -> Mesh {
        let mut out = Mesh::default();
        for t in &self.triangles {
            let pts: Vec<[f64; 3]> = t.iter().filter_map(|&i| self.positions.get(i as usize).copied()).collect();
            if pts.len() != 3 {
                continue;
            }
            if pts.iter().all(|p| p[2] <= z) {
                let base = out.positions.len() as u32;
                out.positions.extend_from_slice(&pts);
                out.triangles.push([base, base + 1, base + 2]);
                continue;
            }
            if pts.iter().all(|p| p[2] > z) {
                continue;
            }
            // Sutherland–Hodgman against the half-space below z, then a fan.
            let mut poly: Vec<[f64; 3]> = Vec::with_capacity(4);
            for k in 0..3 {
                let (a, b) = (pts[k], pts[(k + 1) % 3]);
                if a[2] <= z {
                    poly.push(a);
                }
                if (a[2] <= z) != (b[2] <= z) {
                    let s = (z - a[2]) / (b[2] - a[2]);
                    poly.push([a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, z]);
                }
            }
            if poly.len() < 3 {
                continue;
            }
            let base = out.positions.len() as u32;
            out.positions.extend_from_slice(&poly);
            for k in 1..poly.len() as u32 - 1 {
                out.triangles.push([base, base + k, base + k + 1]);
            }
        }
        out
    }

    /// Bounding box `(min, max)`, or `None` if empty.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_with_hole_triangulates() {
        let mut m = Mesh::default();
        let outer = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[3.0, 3.0], [3.0, 6.0], [6.0, 6.0], [6.0, 3.0]];
        assert!(m.polygon(&outer, &[hole], |p| [p[0], p[1], 0.0]));
        assert_eq!(m.triangles.len(), 8);
    }
}
