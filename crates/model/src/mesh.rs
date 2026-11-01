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
    /// White render (pudset facade, bands).
    Render,
    /// Red brick, where a facade isn't the building's usual brick.
    BrickRed,
    /// Yellow facing brick, explicitly assigned rather than changing a building's default brick.
    BrickYellow,
    /// Appended site presentation materials; existing indices stay stable.
    Paving,
    Gravel,
    Grass,
    Bark,
    Foliage,
    Soil,
    Planter,
    /// Painted white metal, distinct from white timber window/door frames.
    SteelWhite,
    /// Dark elastomer glazing seals, distinct from painted steel railings.
    Gasket,
    /// Neutral outdoor ground; actual paving/soil composition is project information.
    Ground,
    /// Original slate/shingle roof presentation material.
    Slate,
    /// Bituminous roofing felt (tagpap).
    Felt,
}

impl Material {
    pub const ALL: [Material; 29] = [
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
        Material::Render,
        Material::BrickRed,
        Material::BrickYellow,
        Material::Paving,
        Material::Gravel,
        Material::Grass,
        Material::Bark,
        Material::Foliage,
        Material::Soil,
        Material::Planter,
        Material::SteelWhite,
        Material::Gasket,
        Material::Ground,
        Material::Slate,
        Material::Felt,
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
            Material::Render => "render",
            Material::BrickRed => "brick_red",
            Material::BrickYellow => "brick_yellow",
            Material::Paving => "paving",
            Material::Gravel => "gravel",
            Material::Grass => "grass",
            Material::Bark => "bark",
            Material::Foliage => "foliage",
            Material::Soil => "soil",
            Material::Planter => "planter",
            Material::SteelWhite => "steel_white",
            Material::Gasket => "gasket",
            Material::Ground => "ground",
            Material::Slate => "slate",
            Material::Felt => "felt",
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
            Material::Render => [0.90, 0.90, 0.88, 1.0],
            Material::BrickRed => [0.55, 0.22, 0.14, 1.0],
            Material::BrickYellow => [0.57, 0.43, 0.22, 1.0],
            Material::Paving => [0.52, 0.51, 0.47, 1.0],
            Material::Gravel => [0.62, 0.58, 0.48, 1.0],
            Material::Grass => [0.22, 0.31, 0.12, 1.0],
            Material::Bark => [0.25, 0.16, 0.09, 1.0],
            Material::Foliage => [0.19, 0.32, 0.10, 1.0],
            Material::Soil => [0.20, 0.14, 0.08, 1.0],
            Material::Planter => [0.30, 0.31, 0.28, 1.0],
            Material::SteelWhite => Material::Frame.rgba(),
            Material::Gasket => Material::Railing.rgba(),
            Material::Ground => [0.38, 0.40, 0.35, 1.0],
            Material::Slate => [0.12, 0.14, 0.16, 1.0],
            Material::Felt => [0.065, 0.07, 0.075, 1.0],
        }
    }

    pub fn masonry(self) -> bool {
        matches!(self, Self::Wall | Self::BrickRed | Self::BrickYellow)
    }

    /// Original running-bond material coordinates, anchored to site-local millimetres.
    /// One repeat is 480 × 132 mm: two 240 × 66 modules with 228 × 54 brick faces and
    /// 12 mm joints. This is a presentation template, not a measurement of an existing bond.
    /// `normal` is the triangle cross product in original coordinates; it need not be unit.
    pub fn masonry_uv(self, point: [f64; 3], normal: [f64; 3]) -> Option<[f32; 2]> {
        if !self.masonry() || point.iter().any(|v| !v.is_finite() || v.abs() > 1e9) || normal.iter().any(|v| !v.is_finite() || v.abs() > 1e30) {
            return None;
        }
        let horizontal = normal[0].hypot(normal[1]);
        if normal[2].abs() > horizontal {
            return Some([(point[0] / 480.0) as f32, (-point[1] / 132.0) as f32]);
        }
        if horizontal <= 1e-12 {
            return None;
        }
        let along = point[0] * (normal[1] / horizontal) - point[1] * (normal[0] / horizontal);
        Some([(along / 480.0) as f32, (-point[2] / 132.0) as f32])
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
    #[test]
    fn appended_terrain_and_roof_materials_preserve_the_published_material_prefix() {
        // Published pre-integration inventory from immutable revision 5ff5d3a.
        let legacy = [
            "wall",
            "slab",
            "roof",
            "frame",
            "glass",
            "door",
            "sill",
            "railing",
            "reference",
            "steel",
            "deck",
            "demolished",
            "plinth",
            "plaster",
            "render",
            "brick_red",
            "brick_yellow",
            "paving",
            "gravel",
            "grass",
            "bark",
            "foliage",
            "soil",
            "planter",
            "steel_white",
            "gasket",
        ];
        assert_eq!(super::Material::ALL[..26].iter().map(|m| m.name()).collect::<Vec<_>>(), legacy);
        for (index, material) in super::Material::ALL.iter().enumerate() {
            assert_eq!(*material as usize, index, "GPU/export material indices must agree with the shared table");
        }
    }

    use super::*;

    #[test]
    fn masonry_coordinates_preserve_physical_scale_and_ignore_normal_magnitude() {
        for material in [Material::Wall, Material::BrickRed, Material::BrickYellow] {
            assert_eq!(material.masonry_uv([480.0, 0.0, -132.0], [0.0, 1.0, 0.0]), Some([1.0, 1.0]));
            assert_eq!(material.masonry_uv([480.0, 0.0, -132.0], [0.0, 9000.0, 0.0]), Some([1.0, 1.0]));
            assert_eq!(material.masonry_uv([0.0, 480.0, 132.0], [1.0, 0.0, 0.0]), Some([-1.0, -1.0]));
            assert_eq!(material.masonry_uv([480.0, 132.0, 0.0], [0.0, 0.0, 1.0]), Some([1.0, -1.0]));
            assert!(material.masonry_uv([f64::NAN, 0.0, 0.0], [0.0, 1.0, 0.0]).is_none());
            assert!(material.masonry_uv([0.0; 3], [f64::INFINITY, 0.0, 0.0]).is_none());
            assert!(material.masonry_uv([f64::MAX, 0.0, 0.0], [0.0, 1.0, 0.0]).is_none());
            assert!(material.masonry_uv([0.0; 3], [0.0; 3]).is_none());
        }
        for material in Material::ALL.into_iter().filter(|m| !m.masonry()) {
            assert!(material.masonry_uv([0.0; 3], [0.0, 1.0, 0.0]).is_none());
        }
        assert_ne!(Material::BrickYellow.rgba(), Material::Wall.rgba());
        assert_eq!(serde_json::from_str::<Material>("\"wall\"").unwrap(), Material::Wall);
        assert_eq!(serde_json::from_str::<Material>("\"brick_red\"").unwrap(), Material::BrickRed);
        assert_eq!(serde_json::from_str::<Material>("\"brick_yellow\"").unwrap(), Material::BrickYellow);
    }

    #[test]
    fn polygon_with_hole_triangulates() {
        let mut m = Mesh::default();
        let outer = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let hole = vec![[3.0, 3.0], [3.0, 6.0], [6.0, 6.0], [6.0, 3.0]];
        assert!(m.polygon(&outer, &[hole], |p| [p[0], p[1], 0.0]));
        assert_eq!(m.triangles.len(), 8);
    }
}
