//! Parametric terrain height grids in site-local mm. No map coordinates or supplied meshes.

use crate::{Building, Mesh, ModelError, Result, checked};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Terrain {
    /// South-west sample point, site-local mm.
    pub origin: [f64; 2],
    pub spacing_mm: [f64; 2],
    /// Rows run south to north, columns west to east. Heights are relative to site datum.
    pub heights_mm: Vec<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

impl Terrain {
    pub fn validate(&self) -> Result<()> {
        let rows = self.heights_mm.len();
        let cols = self.heights_mm.first().map_or(0, Vec::len);
        if !(2..=65).contains(&rows) || !(2..=65).contains(&cols) || self.heights_mm.iter().any(|row| row.len() != cols) {
            return Err(ModelError::Invalid("terrain requires a rectangular grid of 2–65 rows and columns".into()));
        }
        for k in 0..2 {
            checked("terrain origin", self.origin[k], -1e8, 1e8)?;
            checked("terrain spacing", self.spacing_mm[k], 100.0, 200_000.0)?;
            let count = if k == 0 { cols } else { rows };
            checked("terrain extent", self.origin[k] + self.spacing_mm[k] * (count - 1) as f64, -1e8, 1e8)?;
        }
        for height in self.heights_mm.iter().flatten() {
            checked("terrain height", *height, -1e6, 1e6)?;
        }
        if self.source.iter().chain(&self.attribution).any(|s| s.len() > 2000) {
            return Err(ModelError::Invalid("terrain source text is limited to 2000 bytes".into()));
        }
        Ok(())
    }

    /// Open, upward-facing surface, trimmed at building footprints to keep ground out of
    /// floors and avoid coplanar faces. Every clipped point retains its original triangle plane.
    pub fn mesh(&self, buildings: &[Building]) -> Result<Mesh> {
        self.validate()?;
        let mut mesh = Mesh::default();
        let masks: Vec<_> = buildings.iter().map(|b| crate::footprint::clean(&b.footprint)).collect::<Result<_>>()?;
        let p = |x: usize, y: usize| {
            [self.origin[0] + x as f64 * self.spacing_mm[0], self.origin[1] + y as f64 * self.spacing_mm[1], self.heights_mm[y][x]]
        };
        for y in 0..self.heights_mm.len() - 1 {
            for x in 0..self.heights_mm[0].len() - 1 {
                for t in [[p(x, y), p(x + 1, y), p(x + 1, y + 1)], [p(x, y), p(x + 1, y + 1), p(x, y + 1)]] {
                    let outline = t.map(|p| [p[0], p[1]]);
                    let [a, b, c] = t;
                    let det = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
                    let lift = |q: [f64; 2]| {
                        let u = ((q[0] - a[0]) * (c[1] - a[1]) - (q[1] - a[1]) * (c[0] - a[0])) / det;
                        let v = ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])) / det;
                        [q[0], q[1], a[2] + u * (b[2] - a[2]) + v * (c[2] - a[2])]
                    };
                    for shape in crate::walls::cut(&outline, &masks) {
                        let Some((outer, holes)) = shape.split_first() else { continue };
                        if !mesh.polygon(outer, holes, lift) {
                            return Err(ModelError::Invalid("terrain surface could not be triangulated".into()));
                        }
                    }
                }
            }
        }
        for t in &mut mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            if (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) < 0.0 {
                t.swap(1, 2);
            }
        }
        Ok(mesh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sloping_surface_retains_samples_and_cuts_building_footprints() {
        let terrain = Terrain {
            origin: [-1000., -1000.],
            spacing_mm: [12000., 10000.],
            heights_mm: vec![vec![0., 120.], vec![200., 320.]],
            source: None,
            attribution: None,
        };
        let building = Building::new("b", "Synthetic terrain fixture", &[[0., 0.], [10000., 0.], [10000., 8000.], [0., 8000.]], 1, 3000.).unwrap();
        let mesh = terrain.mesh(&[building]).unwrap();
        let mut area = 0.;
        for t in &mesh.triangles {
            let [a, b, c] = t.map(|i| mesh.positions[i as usize]);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross > 0.0);
            area += cross / 2.;
            let centre = [(a[0] + b[0] + c[0]) / 3., (a[1] + b[1] + c[1]) / 3.];
            assert!(!(centre[0] > 0. && centre[0] < 10000. && centre[1] > 0. && centre[1] < 8000.));
        }
        assert!((area - (12000. * 10000. - 10000. * 8000.)).abs() < 1.);
        for p in &mesh.positions {
            assert!((p[2] - ((p[0] + 1000.) * 0.01 + (p[1] + 1000.) * 0.02)).abs() < 1e-6);
        }
        assert_eq!(terrain.mesh(&[]).unwrap().triangles.len(), 2);
    }
    #[test]
    fn hostile_and_ragged_grids_are_refused() {
        let mut t =
            Terrain { origin: [0., 0.], spacing_mm: [1000., 1000.], heights_mm: vec![vec![0., 0.], vec![0., 0.]], source: None, attribution: None };
        for n in [f64::NAN, f64::INFINITY, f64::MAX, -1e300] {
            t.heights_mm[0][0] = n;
            assert!(t.mesh(&[]).is_err());
        }
        t.heights_mm = vec![vec![0.; 65]; 66];
        assert!(t.validate().is_err());
        t.heights_mm = vec![vec![0., 0.], vec![0.]];
        assert!(t.validate().is_err());
        t.heights_mm = vec![vec![0., 0.], vec![0., 0.]];
        let mut b = Building::new("b", "Bad mask fixture", &[[0., 0.], [10000., 0.], [10000., 8000.], [0., 8000.]], 1, 3000.).unwrap();
        b.footprint[0][0] = f64::NAN;
        assert!(t.mesh(&[b]).is_err());
    }
}
