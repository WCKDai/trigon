//! glTF 2.0 binary (GLB) export. One node per element, one primitive per material.
//!
//! glTF is Y-up in metres; Trigon is Z-up in millimetres: (x, y, z) → (x, z, -y) / 1000.
//! No normals are written, so viewers compute flat normals (glTF 2.0 §3.7.2.1).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use trigon_model::{Built, Material, Mesh};

struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Writer {
    fn align(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn view(&mut self, bytes: &[u8], target: u32) -> usize {
        self.align();
        self.views.push(json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len(), "target": target}));
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    /// Positions and indices for one mesh; returns (position accessor, index accessor).
    fn mesh(&mut self, m: &Mesh) -> (usize, usize) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut pos = Vec::with_capacity(m.positions.len() * 12);
        for p in &m.positions {
            let q = [(p[0] / 1000.0) as f32, (p[2] / 1000.0) as f32, (-p[1] / 1000.0) as f32];
            for k in 0..3 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
                pos.extend_from_slice(&q[k].to_le_bytes());
            }
        }
        let idx: Vec<u8> = m.triangles.iter().flatten().flat_map(|i| i.to_le_bytes()).collect();
        let pv = self.view(&pos, 34962);
        self.accessors.push(json!({"bufferView": pv, "componentType": 5126, "count": m.positions.len(), "type": "VEC3", "min": lo, "max": hi}));
        let pa = self.accessors.len() - 1;
        let iv = self.view(&idx, 34963);
        self.accessors.push(json!({"bufferView": iv, "componentType": 5125, "count": m.triangles.len() * 3, "type": "SCALAR"}));
        (pa, self.accessors.len() - 1)
    }
}

/// Writes every built building, plus named reference meshes (linked models), as one GLB scene.
pub fn write(built: &[Built], references: &[(String, Mesh)]) -> Vec<u8> {
    let mut w = Writer { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let materials: Vec<Value> = Material::ALL
        .iter()
        .map(|m| {
            let c = m.rgba();
            let mut v = json!({
                "name": m.name(),
                "doubleSided": true,
                "pbrMetallicRoughness": {"baseColorFactor": c, "metallicFactor": 0.0, "roughnessFactor": 0.85}
            });
            if c[3] < 1.0 {
                v["alphaMode"] = json!("BLEND");
            }
            v
        })
        .collect();
    let mat_index = |m: Material| Material::ALL.iter().position(|x| *x == m).unwrap_or(0);

    let mut meshes = Vec::new();
    let mut nodes = Vec::new();
    let mut add = |w: &mut Writer, name: &str, parts: &[(Material, &Mesh)]| {
        // Merge parts that share a material into one primitive.
        let mut by_mat: BTreeMap<Material, Mesh> = BTreeMap::new();
        for (m, mesh) in parts {
            by_mat.entry(*m).or_default().append(mesh);
        }
        let prims: Vec<Value> = by_mat
            .iter()
            .filter(|(_, m)| !m.is_empty())
            .map(|(mat, mesh)| {
                let (pa, ia) = w.mesh(mesh);
                json!({"attributes": {"POSITION": pa}, "indices": ia, "material": mat_index(*mat)})
            })
            .collect();
        if prims.is_empty() {
            return;
        }
        meshes.push(json!({"name": name, "primitives": prims}));
        nodes.push(json!({"name": name, "mesh": meshes.len() - 1}));
    };

    for b in built {
        for wall in &b.walls {
            add(&mut w, &wall.id, &[(wall.finish, &wall.mesh), (Material::Plinth, &wall.plinth), (Material::Render, &wall.trim)]);
            let elements = wall.openings.iter().flat_map(|o| std::iter::once(&o.fill).chain(&o.attached)).chain(&wall.attachments);
            for e in elements {
                // What a phase tears down is drawn in translucent red, as in Revit's demolition views.
                let demolished = e.status == trigon_model::build::Status::Demolished;
                let parts: Vec<(Material, &Mesh)> =
                    e.parts.iter().map(|(m, mesh)| (if demolished { Material::Demolished } else { *m }, mesh)).collect();
                add(&mut w, &e.id, &parts);
            }
        }
        for s in &b.slabs {
            let m = if s.kind == trigon_model::SlabKind::Roof { Material::Roof } else { Material::Slab };
            add(&mut w, &s.id, &[(m, &s.mesh)]);
        }
        if let Some(r) = &b.roof {
            add(&mut w, &r.id, &[(Material::Roof, &r.cover), (Material::Wall, &r.gables), (Material::Steel, &r.gutters)]);
        }
        for iw in &b.interior {
            add(&mut w, &iw.id, &[(Material::Plaster, &iw.mesh)]);
            for d in &iw.doors {
                let parts: Vec<(Material, &Mesh)> = d.fill.parts.iter().map(|(m, mesh)| (*m, mesh)).collect();
                add(&mut w, &d.fill.id, &parts);
            }
        }
        for st in &b.stairs {
            add(&mut w, &st.id, &[(Material::Sill, &st.mesh), (Material::Railing, &st.railing)]);
        }
    }
    for (name, mesh) in references {
        add(&mut w, name, &[(Material::Reference, mesh)]);
    }
    w.align();
    let node_ids: Vec<usize> = (0..nodes.len()).collect();
    let doc = json!({
        "asset": {"version": "2.0", "generator": concat!("Trigon ", env!("CARGO_PKG_VERSION"))},
        "scene": 0,
        "scenes": [{"nodes": node_ids}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "accessors": w.accessors,
        "bufferViews": w.views,
        "buffers": [{"byteLength": w.bin.len()}],
    });
    let mut js = doc.to_string().into_bytes();
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let total = 12 + 8 + js.len() + 8 + w.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&js);
    out.extend_from_slice(&(w.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&w.bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_model::{Building, Document, build};

    #[test]
    fn glb_header_and_json_are_consistent() {
        let mut doc = Document::default();
        doc.buildings.push(Building::new("b1", "T", &[[0.0, 0.0], [9000.0, 0.0], [9000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap());
        let glb = write(&build(&doc).unwrap(), &[]);
        assert_eq!(&glb[0..4], b"glTF");
        let total = u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize;
        assert_eq!(total, glb.len());
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let v: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
        assert_eq!(v["nodes"].as_array().unwrap().len(), 4 + 3); // 4 walls, 2 floors, 1 roof
        let blen = u32::from_le_bytes(glb[20 + jlen..24 + jlen].try_into().unwrap()) as usize;
        assert_eq!(v["buffers"][0]["byteLength"].as_u64().unwrap() as usize, blen);
    }
}
