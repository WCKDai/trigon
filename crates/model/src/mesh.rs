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
}

impl Material {
    pub const ALL: [Material; 8] =
        [Material::Wall, Material::Slab, Material::Roof, Material::Frame, Material::Glass, Material::Door, Material::Sill, Material::Railing];

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
