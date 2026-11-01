//! glTF 2.0 binary (GLB) export. One node per element, one primitive per material.
//!
//! glTF is Y-up in metres; Trigon is Z-up in millimetres: (x, y, z) → (x, z, -y) / 1000.
//! Masonry carries embedded original base-colour/normal images and physically scaled UVs.
//! Its per-face normals and tangent frames preserve the model's flat faces. Other materials
//! keep the original position-only representation (glTF 2.0 §3.7.2.1).

use std::collections::{BTreeMap, BTreeSet};

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

    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let mut view = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.views.push(view);
        self.bin.extend_from_slice(bytes);
        self.views.len() - 1
    }

    /// Positions, indices and optional physically scaled material coordinates for one primitive.
    fn mesh(&mut self, source: &Mesh, material: Material) -> (Value, usize) {
        let expanded = material.masonry().then(|| masonry_mesh(source, material));
        let m = expanded.as_ref().map_or(source, |expanded| &expanded.mesh);
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
        let pv = self.view(&pos, Some(34962));
        self.accessors.push(json!({"bufferView": pv, "componentType": 5126, "count": m.positions.len(), "type": "VEC3", "min": lo, "max": hi}));
        let pa = self.accessors.len() - 1;
        let mut attributes = json!({"POSITION":pa});
        let iv = self.view(&idx, Some(34963));
        self.accessors.push(json!({"bufferView": iv, "componentType": 5125, "count": m.triangles.len() * 3, "type": "SCALAR"}));
        let ia = self.accessors.len() - 1;
        if let Some(expanded) = expanded {
            for (name, kind, count, bytes) in [
                ("TEXCOORD_0", "VEC2", expanded.uv.len(), expanded.uv.iter().flatten().flat_map(|value| value.to_le_bytes()).collect::<Vec<_>>()),
                ("NORMAL", "VEC3", expanded.normals.len(), expanded.normals.iter().flatten().flat_map(|value| value.to_le_bytes()).collect()),
                ("TANGENT", "VEC4", expanded.tangents.len(), expanded.tangents.iter().flatten().flat_map(|value| value.to_le_bytes()).collect()),
            ] {
                let view = self.view(&bytes, Some(34962));
                self.accessors.push(json!({"bufferView":view,"componentType":5126,"count":count,"type":kind}));
                attributes[name] = json!(self.accessors.len() - 1);
            }
        }
        (attributes, ia)
    }
}

/// A corner may belong to differently oriented faces. Give every triangle its own UVs while
/// retaining its exact source points and winding; no geometry or element identity is inferred.
#[derive(Default)]
struct MasonryMesh {
    mesh: Mesh,
    uv: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 4]>,
}

fn unit(vector: [f64; 3]) -> [f64; 3] {
    let length = vector.iter().map(|value| value * value).sum::<f64>().sqrt();
    if length.is_finite() && length > 1e-20 { vector.map(|value| value / length) } else { [0.0, 0.0, 1.0] }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn material_frame(ab: [f64; 3], ac: [f64; 3], normal: [f64; 3], uv: [[f32; 2]; 3]) -> ([f32; 3], [f32; 4]) {
    let transform = |p: [f64; 3]| [p[0], p[2], -p[1]];
    let n = unit(transform(normal));
    let (ab, ac) = (transform(ab), transform(ac));
    let duv1 = [f64::from(uv[1][0] - uv[0][0]), f64::from(uv[1][1] - uv[0][1])];
    let duv2 = [f64::from(uv[2][0] - uv[0][0]), f64::from(uv[2][1] - uv[0][1])];
    let determinant = duv1[0] * duv2[1] - duv2[0] * duv1[1];
    let (tangent, bitangent) = if determinant.abs() > 1e-20 {
        (
            std::array::from_fn(|axis| (ab[axis] * duv2[1] - ac[axis] * duv1[1]) / determinant),
            std::array::from_fn(|axis| (ac[axis] * duv1[0] - ab[axis] * duv2[0]) / determinant),
        )
    } else {
        let tangent = cross(if n[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] }, n);
        (tangent, cross(n, tangent))
    };
    // Gram-Schmidt also removes small projection errors from stored f32 texture coordinates.
    let projection: f64 = tangent.iter().zip(n).map(|(a, b)| a * b).sum();
    let tangent = unit(std::array::from_fn(|axis| tangent[axis] - n[axis] * projection));
    let handedness: f64 = cross(n, tangent).iter().zip(bitangent).map(|(a, b)| a * b).sum();
    (n.map(|value| value as f32), [tangent[0] as f32, tangent[1] as f32, tangent[2] as f32, if handedness < 0.0 { -1.0 } else { 1.0 }])
}

fn masonry_mesh(source: &Mesh, material: Material) -> MasonryMesh {
    let mut expanded = MasonryMesh::default();
    for triangle in &source.triangles {
        let [Some(a), Some(b), Some(c)] = triangle.map(|index| source.positions.get(index as usize)) else { continue };
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let normal = cross(ab, ac);
        let uv = [*a, *b, *c].map(|point| material.masonry_uv(point, normal).unwrap_or([0.0; 2]));
        let (normal, tangent) = material_frame(ab, ac, normal, uv);
        let Ok(base) = u32::try_from(expanded.mesh.positions.len()) else { break };
        let Some(last) = base.checked_add(2) else { break };
        for point in [*a, *b, *c] {
            expanded.mesh.positions.push(point);
            expanded.normals.push(normal);
            expanded.tangents.push(tangent);
        }
        expanded.uv.extend(uv);
        expanded.mesh.triangles.push([base, base + 1, last]);
    }
    expanded
}

/// Writes every built building, plus named reference meshes (linked models), as one GLB scene.
pub fn write(built: &[Built], references: &[(String, Mesh)]) -> Vec<u8> {
    let mut w = Writer { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut materials: Vec<Value> = Material::ALL
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
    let mut masonry = BTreeSet::new();
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
                let (attributes, ia) = w.mesh(mesh, *mat);
                if mat.masonry() {
                    masonry.insert(*mat);
                }
                json!({"attributes":attributes,"indices":ia,"material":mat_index(*mat)})
            })
            .collect();
        if prims.is_empty() {
            return;
        }
        meshes.push(json!({"name": name, "primitives": prims}));
        nodes.push(json!({"name": name, "mesh": meshes.len() - 1,"extras":{"trigon_id":name}}));
    };

    for b in built {
        if let Some(terrain) = &b.terrain {
            add(&mut w, "site.terrain", &[(Material::Ground, terrain)]);
        }
        for wall in b.exterior_walls() {
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
            let mut parts = vec![(Material::Roof, &r.cover), (Material::Steel, &r.gutters)];
            if r.gable_parts.is_empty() {
                parts.insert(1, (Material::Wall, &r.gables));
            } else {
                parts.extend(r.gable_parts.iter().map(|(m, mesh)| (*m, mesh)));
            }
            parts.extend(r.covering.iter().map(|(m, mesh)| (*m, mesh)));
            add(&mut w, &r.id, &parts);
            for d in &r.dormers {
                add(&mut w, &format!("{}.roof", d.id), &[(Material::Steel, &d.roof)]);
                add(&mut w, &format!("{}.cheeks", d.id), &[(Material::Steel, &d.cheeks)]);
            }
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
    let (mut images, mut textures) = (Vec::new(), Vec::new());
    for material in masonry {
        // The infallible public writer predates textures. If encoding cannot complete, retain
        // its existing flat material instead of emitting a broken image or texture reference.
        let Ok(bytes) = crate::masonry_texture::png(material) else { continue };
        let view = w.view(&bytes, None);
        images.push(json!({"name":format!("{} masonry",material.name()),"bufferView":view,"mimeType":"image/png"}));
        textures.push(json!({"source":images.len()-1,"sampler":0}));
        let pbr = &mut materials[mat_index(material)]["pbrMetallicRoughness"];
        pbr["baseColorFactor"] = json!([1.0, 1.0, 1.0, 1.0]);
        pbr["baseColorTexture"] = json!({"index":textures.len()-1,"texCoord":0});
        pbr["roughnessFactor"] = json!(0.92);
    }
    if !textures.is_empty()
        && let Ok(bytes) = crate::masonry_texture::normal_png()
    {
        let view = w.view(&bytes, None);
        images.push(json!({"name":"masonry joint relief","bufferView":view,"mimeType":"image/png"}));
        textures.push(json!({"source":images.len()-1,"sampler":0}));
        let normal = textures.len() - 1;
        for material in &mut materials {
            if material["pbrMetallicRoughness"].get("baseColorTexture").is_some() {
                material["normalTexture"] = json!({"index":normal,"texCoord":0,"scale":1.0});
            }
        }
    }
    w.align();
    let node_ids: Vec<usize> = (0..nodes.len()).collect();
    let mut doc = json!({
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
    if !textures.is_empty() {
        doc["images"] = json!(images);
        doc["textures"] = json!(textures);
        doc["samplers"] = json!([{"magFilter":9729,"minFilter":9987,"wrapS":10497,"wrapT":10497}]);
    }
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

    fn parse(bytes: &[u8]) -> (Value, &[u8]) {
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, bytes.len());
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
        assert_eq!(&bytes[24 + json_len..28 + json_len], b"BIN\0");
        let length = u32::from_le_bytes(bytes[20 + json_len..24 + json_len].try_into().unwrap()) as usize;
        let bin = &bytes[28 + json_len..];
        assert_eq!(length, bin.len());
        (value, bin)
    }

    fn view<'a>(doc: &Value, bin: &'a [u8], index: usize) -> &'a [u8] {
        let view = &doc["bufferViews"][index];
        let start = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        assert_eq!(start % 4, 0);
        &bin[start..start + length]
    }

    fn floats(doc: &Value, bin: &[u8], index: usize) -> Vec<f32> {
        let accessor = &doc["accessors"][index];
        assert_eq!(accessor["componentType"], 5126);
        let bytes = view(doc, bin, accessor["bufferView"].as_u64().unwrap() as usize);
        bytes.as_chunks::<4>().0.iter().map(|bytes| f32::from_le_bytes(*bytes)).collect()
    }

    fn positions_in_triangle_order(doc: &Value, bin: &[u8], primitive: &Value) -> Vec<[f32; 3]> {
        let positions = floats(doc, bin, primitive["attributes"]["POSITION"].as_u64().unwrap() as usize);
        let accessor = &doc["accessors"][primitive["indices"].as_u64().unwrap() as usize];
        assert_eq!(accessor["componentType"], 5125);
        view(doc, bin, accessor["bufferView"].as_u64().unwrap() as usize)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| {
                let index = u32::from_le_bytes(*bytes) as usize;
                [positions[index * 3], positions[index * 3 + 1], positions[index * 3 + 2]]
            })
            .collect()
    }

    #[test]
    fn per_face_masonry_uvs_preserve_shared_corners_and_physical_repeat() {
        let mut source = Mesh::default();
        source.quad([0.0, 0.0, 0.0], [480.0, 0.0, 0.0], [480.0, 0.0, 132.0], [0.0, 0.0, 132.0]);
        source.quad([480.0, 0.0, 0.0], [480.0, 480.0, 0.0], [480.0, 480.0, 132.0], [480.0, 0.0, 132.0]);
        let expanded = masonry_mesh(&source, Material::BrickYellow);
        let (mesh, uv) = (&expanded.mesh, &expanded.uv);
        assert_eq!(mesh.triangles.len(), source.triangles.len());
        assert_eq!(mesh.positions.len(), source.triangles.len() * 3);
        assert_eq!(mesh.positions.len(), uv.len());
        assert_eq!(mesh.bounds(), source.bounds());
        for (index, triangle) in source.triangles.iter().enumerate() {
            for (corner, original) in triangle.iter().enumerate() {
                assert_eq!(mesh.positions[index * 3 + corner], source.positions[*original as usize]);
            }
        }
        assert_eq!((uv[1][0] - uv[0][0]).abs(), 1.0, "480 mm is one repeat along the facade");
        assert_eq!((uv[2][1] - uv[1][1]).abs(), 1.0, "132 mm is one repeat vertically");
        assert_eq!(mesh.positions[1], mesh.positions[6]);
        assert_ne!(uv[1], uv[6], "a shared corner can have different per-face UVs");
        assert!(uv.iter().flatten().all(|coordinate| coordinate.is_finite()));
        assert_eq!(expanded.normals.len(), mesh.positions.len());
        assert_eq!(expanded.tangents.len(), mesh.positions.len());
        for (normal, tangent) in expanded.normals.iter().zip(&expanded.tangents) {
            assert!((normal.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-6);
            assert!((tangent[..3].iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-6);
            assert!(normal.iter().zip(tangent).map(|(a, b)| a * b).sum::<f32>().abs() < 1e-6);
            assert!(matches!(tangent[3], -1.0 | 1.0));
        }
    }

    #[test]
    fn embedded_masonry_textures_keep_nodes_geometry_and_non_masonry_materials() {
        let mut doc = Document::default();
        // An oblique wall also checks that UVs are derived from each original face normal.
        doc.buildings.push(
            Building::new("oblique", "Texture fixture", &[[1000.0, 2000.0], [9000.0, 6000.0], [5000.0, 14000.0], [-3000.0, 10000.0]], 1, 3000.0)
                .unwrap(),
        );
        let mut built = build(&doc).unwrap();
        for (wall, material) in built[0].walls.iter_mut().zip([Material::Wall, Material::BrickRed, Material::BrickYellow, Material::Render]) {
            wall.finish = material;
        }
        let bytes = write(&built, &[]);
        assert_eq!(bytes, write(&built, &[]), "portable material bytes are deterministic");
        let (data, bin) = parse(&bytes);
        assert_eq!(data["images"].as_array().unwrap().len(), 4, "one image per used masonry material plus one shared normal map");
        assert_eq!(data["textures"].as_array().unwrap().len(), 4);
        assert_eq!(data["samplers"][0]["wrapS"], 10497);
        assert_eq!(data["samplers"][0]["wrapT"], 10497);
        for image in data["images"].as_array().unwrap() {
            assert!(image.get("uri").is_none());
            assert_eq!(image["mimeType"], "image/png");
            let index = image["bufferView"].as_u64().unwrap() as usize;
            assert!(data["bufferViews"][index].get("target").is_none(), "image views are not vertex/index buffers");
            let png = view(&data, bin, index);
            let reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
            assert_eq!((reader.info().width, reader.info().height), (960, 264));
        }
        for wall in &built[0].walls {
            let node = data["nodes"].as_array().unwrap().iter().find(|node| node["name"] == wall.id).unwrap();
            let mesh = &data["meshes"][node["mesh"].as_u64().unwrap() as usize];
            let material = Material::ALL.iter().position(|m| *m == wall.finish).unwrap();
            let primitive = mesh["primitives"].as_array().unwrap().iter().find(|primitive| primitive["material"] == material).unwrap();
            let actual = positions_in_triangle_order(&data, bin, primitive);
            let expected: Vec<_> = wall
                .mesh
                .triangles
                .iter()
                .flatten()
                .map(|index| {
                    let p = wall.mesh.positions[*index as usize];
                    [(p[0] / 1000.0) as f32, (p[2] / 1000.0) as f32, (-p[1] / 1000.0) as f32]
                })
                .collect();
            assert_eq!(actual, expected, "{} keeps every triangle point and its winding", wall.id);
            let pbr = &data["materials"][material]["pbrMetallicRoughness"];
            if wall.finish.masonry() {
                let uv = primitive["attributes"]["TEXCOORD_0"].as_u64().unwrap() as usize;
                let position = primitive["attributes"]["POSITION"].as_u64().unwrap() as usize;
                assert_eq!(data["accessors"][uv]["type"], "VEC2");
                assert_eq!(data["accessors"][uv]["count"], data["accessors"][position]["count"]);
                assert!(floats(&data, bin, uv).iter().all(|value| value.is_finite()));
                for attribute in ["NORMAL", "TANGENT"] {
                    let accessor = primitive["attributes"][attribute].as_u64().unwrap() as usize;
                    assert_eq!(data["accessors"][accessor]["count"], data["accessors"][position]["count"]);
                    assert!(floats(&data, bin, accessor).iter().all(|value| value.is_finite()));
                }
                assert_eq!(pbr["baseColorFactor"], json!([1.0, 1.0, 1.0, 1.0]));
                let texture = pbr["baseColorTexture"]["index"].as_u64().unwrap() as usize;
                assert!(data["textures"].as_array().unwrap().get(texture).is_some());
                assert_eq!(data["materials"][material]["normalTexture"]["index"], 3);
                assert_eq!(data["materials"][material]["normalTexture"]["scale"], 1.0);
            } else {
                assert!(primitive["attributes"].get("TEXCOORD_0").is_none());
                assert!(pbr.get("baseColorTexture").is_none());
                assert_eq!(pbr["baseColorFactor"], json!(wall.finish.rgba()));
            }
        }
        let glass = Material::ALL.iter().position(|material| *material == Material::Glass).unwrap();
        assert_eq!(data["materials"][glass]["alphaMode"], "BLEND");
        assert_eq!(data["materials"][glass]["pbrMetallicRoughness"]["baseColorFactor"], json!(Material::Glass.rgba()));
    }

    #[test]
    fn non_masonry_only_exports_have_no_image_or_texture_requirements() {
        let mut reference = Mesh::default();
        reference.quad([0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], [1000.0, 1000.0, 0.0], [0.0, 1000.0, 0.0]);
        let bytes = write(&[], &[("reference".into(), reference)]);
        let (doc, _) = parse(&bytes);
        for key in ["images", "textures", "samplers"] {
            assert!(doc.get(key).is_none());
        }
        assert_eq!(doc["nodes"][0]["name"], "reference");
        assert!(doc["meshes"][0]["primitives"][0]["attributes"].get("TEXCOORD_0").is_none());
    }

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
