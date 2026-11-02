//! Public, command-authored fixtures and an independent copy of the pre-index attribution path.
//! Set TRIGON_OWNERSHIP_PROFILE to an ignored output directory for a paired stage profile.

use std::{hint::black_box, path::Path, time::Instant};

use serde_json::{Value, json};
use trigon_model::build::{View, build_view};

use super::*;
use crate::sheet::{DrawingElementSegment, Item, Sheet, Viewport, dxf};
use crate::views::{DrawingPreview, DrawingTransform};

thread_local! {
    pub(super) static USE_FULL_SCAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

// Keep the complete original loop independent of the optimized path, including source cloning,
// merge, exact predicate and append order. This is also the same-binary profiling control.
pub(super) fn full_scan_edges(scene: &mut Scene, p: &Projection) {
    let mut styled = Vec::with_capacity(p.edges.len());
    for e in &p.edges {
        let b = band(e.depth);
        let base = if e.outline { pen(e.class).max(0.5) } else { pen(e.class) };
        let b = if e.class == Class::Railing { (b + 1).min(2) } else { b };
        let colour = [INK, [0x60, 0x60, 0x60], [0x96, 0x96, 0x96]][b];
        let pen = if e.class == Class::Railing { base } else { thinner(base, b) };
        styled.push((e.a, e.b, layer(e.class), Stroke { pen, colour, dashed: false, pattern: Default::default() }));
    }
    let elements: Vec<_> = p.edges.iter().flat_map(|edge| edge.element_segments.iter().cloned()).collect();
    for (a, b, layer, stroke) in merge(styled) {
        scene.line(layer, a, b, stroke);
        scene.attribute_last(&elements);
    }
}

fn public_fixture(columns: usize, storeys: usize) -> trigon_model::Document {
    let mut session = crate::Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
    let width = columns as f64 * 2400.0 + 1000.0;
    session
        .execute(
            "building.add",
            json!({
                "id":"public", "name":"Original public ownership profile", "footprint":[[0,0],[width,0],[width,8000],[0,8000]],
                "storeys":storeys, "storey_height_mm":3400
            }),
        )
        .unwrap();
    session.execute("types.install_copenhagen", json!({})).unwrap();
    for facade in ["public.f1", "public.f3"] {
        session.execute("facade.set_bays", json!({"facade":facade,"count":columns})).unwrap();
        for (offset, family) in ["dk.window.dannebrog", "dk.window.small-panes", "dk.window.segmental"].into_iter().enumerate() {
            let bays: Vec<_> = (offset..columns).step_by(3).collect();
            if !bays.is_empty() {
                session
                    .execute("facade.place", json!({"facade":facade,"type":family,"levels":(0..storeys).collect::<Vec<_>>(),"bays":bays}))
                    .unwrap();
            }
        }
    }
    session.doc
}

fn projections(doc: &trigon_model::Document) -> Vec<(&'static str, Projection, u128)> {
    use crate::sheet::project::{Look, Near, project};
    let built = build_view(doc, View::latest(doc)).unwrap();
    let width = doc.buildings[0].footprint[1][0];
    [
        ("facade", Look { origin: [0.0, 0.0], t: [1.0, 0.0], d: [0.0, 1.0] }, false, -3000.0),
        ("section", Look { origin: [width, 4000.0], t: [-1.0, 0.0], d: [0.0, -1.0] }, true, 0.0),
    ]
    .into_iter()
    .map(|(name, look, inside, near)| {
        let parts = parts(&built, inside);
        let sources = seen(&parts);
        let start = Instant::now();
        let projection = project(&sources, &look, (near, 20_000.0), Near::Cut);
        (name, projection, start.elapsed().as_nanos())
    })
    .collect()
}

fn render(p: &Projection, baseline: bool) -> (Scene, u128, usize) {
    crate::sheet::ATTRIBUTION_SOURCE_VISITS.with(|count| count.set(0));
    let mut scene = Scene::new(100.0);
    let start = Instant::now();
    if baseline {
        full_scan_edges(&mut scene, black_box(p));
    } else {
        edges(&mut scene, black_box(p));
    }
    let elapsed = start.elapsed().as_nanos();
    let visits = crate::sheet::ATTRIBUTION_SOURCE_VISITS.with(|count| count.get());
    (black_box(scene), elapsed, visits)
}

fn sheet(scene: Scene) -> Sheet {
    let vp = Viewport::fit(scene.bounds().unwrap(), [80.0, 100.0, 1440.0, 700.0]);
    Sheet {
        scene,
        vp,
        clip: [0.0, 0.0, 1600.0, 900.0],
        title: "Public ownership fixture".into(),
        project: "Original synthetic geometry".into(),
        caption: "".into(),
        note: None,
        metrics: json!({}),
    }
}

fn preview(sheet: &Sheet) -> DrawingPreview {
    DrawingPreview {
        svg: sheet.svg(),
        transform: DrawingTransform { x0: sheet.vp.x0, y1: sheet.vp.y1, mm_per_px: sheet.vp.mm },
        segments: sheet
            .scene
            .items
            .iter()
            .flat_map(|item| match item {
                Item::Path(path) => path.segments().collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect(),
        element_segments: sheet
            .scene
            .items
            .iter()
            .flat_map(|item| match item {
                Item::Path(path) => path.element_segments.clone(),
                _ => vec![],
            })
            .collect(),
        element_regions: sheet.scene.element_regions.clone(),
    }
}

fn dxf_bytes(sheet: &Sheet) -> Vec<u8> {
    dxf::bytes(&dxf::write(&sheet.scene, &dxf::TitleBlock { title: &sheet.title, project: &sheet.project, caption: &sheet.caption, note: None }))
}

fn compare_and_write(p: &Projection, path: Option<&Path>) {
    let (mut before, _, _) = render(p, true);
    let (mut after, _, _) = render(p, false);
    assert_eq!(after, before, "every ordered ownership record (including duplicates), coordinate and pen must match");
    fills(&mut before, p);
    fills(&mut after, p);
    assert_eq!(after, before);
    let (before, after) = (sheet(before), sheet(after));
    compare_sheets(&before, &after, path);
    if let Some(path) = path {
        let source: Vec<_> = p
            .edges
            .iter()
            .map(|e| json!({"a":e.a,"b":e.b,"class":format!("{:?}",e.class),"depth":e.depth,"outline":e.outline,"owners":e.element_segments}))
            .collect();
        std::fs::write(path.join("projection.json"), serde_json::to_vec(&source).unwrap()).unwrap();
    }
}

fn compare_sheets(before: &Sheet, after: &Sheet, path: Option<&Path>) {
    assert_eq!(after.scene, before.scene);
    assert_eq!(after.vp, before.vp);
    assert_eq!(after.clip, before.clip);
    assert_eq!(after.svg().as_bytes(), before.svg().as_bytes());
    assert_eq!(dxf_bytes(after), dxf_bytes(before));
    let (a, b) = (preview(before), preview(after));
    assert_eq!(a.element_segments, b.element_segments);
    let mut picks = Vec::new();
    let stride = (a.segments.len() / 128).max(1);
    for segment in a.segments.iter().step_by(stride) {
        for point in [segment[0], segment[1], [(segment[0][0] + segment[1][0]) / 2.0, (segment[0][1] + segment[1][1]) / 2.0]] {
            for tolerance in [0.0, 0.5, 5.0] {
                let hit = a.pick_element(point, tolerance);
                assert_eq!(b.pick_element(point, tolerance), hit);
                picks.push(json!({"point":point,"tolerance":tolerance,"hit":hit}));
            }
        }
    }
    if let Some(path) = path {
        std::fs::create_dir_all(path).unwrap();
        for (label, sheet) in [("baseline", before), ("candidate", after)] {
            std::fs::write(path.join(format!("{label}.svg")), sheet.svg()).unwrap();
            std::fs::write(path.join(format!("{label}.dxf")), dxf_bytes(sheet)).unwrap();
            let preview = preview(sheet);
            std::fs::write(
                path.join(format!("{label}.ownership.json")),
                serde_json::to_vec(&json!({"segments":preview.element_segments,"regions":preview.element_regions})).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(path.join("picks.json"), serde_json::to_vec(&picks).unwrap()).unwrap();
    }
}

fn full_sheet(doc: &trigon_model::Document, name: &str, baseline: bool) -> (Sheet, u128) {
    let previous = USE_FULL_SCAN.with(|flag| flag.replace(baseline));
    let start = Instant::now();
    let phase = View::latest(doc);
    let sheet = if name == "facade" {
        crate::elevation::scene(doc, crate::elevation::Side::Facade("public.f1"), phase, (None, None), "Public facade", "", true)
    } else {
        let width = doc.buildings[0].footprint[1][0];
        crate::section::scene(doc, [[width + 1000.0, 4000.0], [-1000.0, 4000.0]], false, 20_000.0, phase, "Public section", "", None)
    };
    let elapsed = start.elapsed().as_nanos();
    USE_FULL_SCAN.with(|flag| flag.set(previous));
    (black_box(sheet.unwrap()), elapsed)
}

fn full_sheet_profile(doc: &trigon_model::Document, name: &str, path: Option<&Path>) -> Vec<Value> {
    let (before, _) = full_sheet(doc, name, true);
    let (after, _) = full_sheet(doc, name, false);
    compare_sheets(&before, &after, path);
    if path.is_none() {
        return Vec::new();
    }
    // The comparison above also warms each complete sheet path. Timed generation includes
    // model build, projection, fills, merged strokes, clipping, poche and annotations. SVG/DXF
    // encoding, picking probes, rasterization, upload and pan remain outside these timings.
    let mut samples = Vec::new();
    let mut rng = 0x27ab_12cd_u32;
    for pair in 0..9 {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let baseline_first = rng & 0x8000_0000 != 0;
        let (first, second) = (full_sheet(doc, name, baseline_first), full_sheet(doc, name, !baseline_first));
        let (baseline, candidate) = if baseline_first { (first, second) } else { (second, first) };
        assert_eq!(candidate.0.scene, baseline.0.scene);
        assert_eq!(candidate.0.vp, baseline.0.vp);
        samples.push(json!({"pair":pair,"baseline_first":baseline_first,"baseline_ns":baseline.1,"candidate_ns":candidate.1}));
    }
    samples
}

fn split_baseline(p: &Projection) -> Value {
    let stripped = Projection {
        edges: p
            .edges
            .iter()
            .cloned()
            .map(|mut e| {
                e.element_segments.clear();
                e
            })
            .collect(),
        ..Default::default()
    };
    let owners: Vec<_> = p.edges.iter().flat_map(|e| e.element_segments.iter().cloned()).collect();
    let mut scene = Scene::new(100.0);
    let start = Instant::now();
    full_scan_edges(&mut scene, &stripped);
    let style_merge_ns = start.elapsed().as_nanos();
    let start = Instant::now();
    for item in &mut scene.items {
        if let Item::Path(path) = item {
            path.attribute(owners.iter());
        }
    }
    let attribution_ns = start.elapsed().as_nanos();
    black_box(scene);
    json!({"style_merge_ns":style_merge_ns,"attribution_ns":attribution_ns})
}

fn index_cost(p: &Projection) -> Value {
    let sources: Vec<_> = p.edges.iter().flat_map(|e| e.element_segments.iter().cloned()).collect();
    let start = Instant::now();
    let index = OwnershipIndex::new(&sources);
    let build_ns = start.elapsed().as_nanos();
    let (scene, _, _) = render(p, false);
    let mut candidates = Vec::new();
    let mut total_candidates = 0;
    let mut max_candidates = 0;
    let mut full_scan_fallbacks = 0;
    let start = Instant::now();
    for item in &scene.items {
        let Item::Path(path) = item else { continue };
        for segment in path.segments() {
            index.query(segment, &mut candidates);
            total_candidates += candidates.len();
            max_candidates = max_candidates.max(candidates.len());
            full_scan_fallbacks += usize::from(candidates.len() == sources.len());
        }
    }
    let query_ns = start.elapsed().as_nanos();
    let allocated_bytes = index.items.capacity() * std::mem::size_of::<(OwnerBounds, usize)>()
        + index.nodes.capacity() * std::mem::size_of::<OwnerNode>()
        + candidates.capacity() * std::mem::size_of::<usize>();
    json!({"build_ns":build_ns,"all_queries_ns":query_ns,"valid_sources":index.items.len(),"nodes":index.nodes.len(),"allocated_index_and_query_bytes":allocated_bytes,"total_candidates":total_candidates,"max_candidates":max_candidates,"full_scan_fallbacks":full_scan_fallbacks})
}

#[test]
fn public_synthetic_ownership_matches_full_scan_and_profiles() {
    let directory = std::env::var_os("TRIGON_OWNERSHIP_PROFILE").map(std::path::PathBuf::from);
    let dimensions = if directory.is_some() { vec![(4, 3), (8, 5)] } else { vec![(3, 2)] };
    let mut reports = Vec::new();
    for (columns, storeys) in dimensions {
        let start = Instant::now();
        let doc = public_fixture(columns, storeys);
        let authoring_ns = start.elapsed().as_nanos();
        if let Some(directory) = &directory {
            std::fs::create_dir_all(directory).unwrap();
            std::fs::write(directory.join(format!("{columns}x{storeys}.trigon.json")), serde_json::to_vec(&doc).unwrap()).unwrap();
        }
        let start = Instant::now();
        let cases = projections(&doc);
        let build_and_projection_ns = start.elapsed().as_nanos();
        for (name, projection, projection_ns) in cases {
            let path = directory.as_ref().map(|directory| directory.join(format!("{columns}x{storeys}-{name}")));
            compare_and_write(&projection, path.as_deref());
            let full_path = path.as_ref().map(|path| path.join("full-sheet"));
            let full_sheet_samples = full_sheet_profile(&doc, name, full_path.as_deref());
            if directory.is_none() {
                continue;
            }
            // Warm both paths before collecting paired samples. Each pair shares the same data
            // and binary; deterministic pseudo-random order reduces thermal/cache/load bias.
            black_box(render(&projection, true));
            black_box(render(&projection, false));
            let mut samples = Vec::new();
            let mut rng = 0x9e37_79b9_u32;
            for pair in 0..9 {
                rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
                let baseline_first = rng & 0x8000_0000 != 0;
                let (first, second) = (render(&projection, baseline_first), render(&projection, !baseline_first));
                let (baseline, candidate) = if baseline_first { (first, second) } else { (second, first) };
                assert_eq!(candidate.0, baseline.0);
                samples.push(json!({"pair":pair,"baseline_first":baseline_first,"baseline_ns":baseline.1,"candidate_ns":candidate.1,"baseline_source_visits":baseline.2,"candidate_source_visits":candidate.2,"split_baseline":split_baseline(&projection)}));
            }
            let source_segments: usize = projection.edges.iter().map(|e| e.element_segments.len()).sum();
            reports.push(json!({"columns":columns,"storeys":storeys,"view":name,"authoring_ns":authoring_ns,"build_and_projection_ns":build_and_projection_ns,"projection_ns":projection_ns,"edges":projection.edges.len(),"source_segments":source_segments,"merged_lines":render(&projection,false).0.items.len(),"index_cost":index_cost(&projection),"samples":samples,"full_sheet_generation_samples":full_sheet_samples}));
        }
    }
    if let Some(directory) = &directory {
        std::fs::write(directory.join("samples.json"),serde_json::to_vec_pretty(&json!({"debug_assertions":cfg!(debug_assertions),"warmups_per_strategy":1,"repetitions":9,"upload_pan_native_fps_measured":false,"reports":reports})).unwrap()).unwrap();
    }
}

#[test]
fn detailed_public_facade_avoids_cartesian_owner_scans() {
    let doc = public_fixture(4, 3);
    let (_, projection, _) = projections(&doc).remove(0);
    let owners: usize = projection.edges.iter().map(|e| e.element_segments.len()).sum();
    let (scene, _, visits) = render(&projection, false);
    let full_scan = owners * scene.items.len();
    assert!(full_scan > 100_000, "fixture must exercise a substantial ownership scan");
    assert!(visits < full_scan / 10, "attribution visited {visits} sources of {full_scan} full-scan candidates");
}

#[test]
fn small_views_keep_the_original_scan_and_invalid_owners_never_match() {
    use crate::sheet::project::Edge;
    let mut projection = Projection::default();
    for i in 0..16 {
        let (a, b) = ([0.0, f64::from(i)], [100.0, f64::from(i)]);
        let owner = DrawingElementSegment { id: format!("small-{i}"), kind: DrawingElementKind::FacadeWall, segment: [a, b] };
        projection.edges.push(Edge { a, b, class: Class::Wall, depth: 0.0, end_depths: [0.0; 2], outline: false, element_segments: vec![owner] });
    }
    let (expected, _, full_scan) = render(&projection, true);
    let (actual, _, visits) = render(&projection, false);
    assert_eq!(actual, expected);
    assert_eq!(visits, full_scan);
    let sources = vec![DrawingElementSegment { id: "invalid".into(), kind: DrawingElementKind::Opening, segment: [[f64::MAX; 2]; 2] }; 1024];
    let index = OwnershipIndex::new(&sources);
    let mut candidates = vec![123];
    index.query([[0.0; 2], [1.0; 2]], &mut candidates);
    assert!(candidates.is_empty());
    assert!(index.items.is_empty());
    assert!(index.nodes.is_empty());
}

fn adversarial_segments() -> Vec<[[f64; 2]; 2]> {
    let mut segments = Vec::new();
    for a in [[0.0, 0.0], [256.0, -512.0], [1e8 - 4096.0, 1e8 - 4096.0], [-1e8 + 4096.0, -1e8 + 4096.0]] {
        for theta in [
            0.0_f64,
            1e-6,
            std::f64::consts::FRAC_PI_6,
            std::f64::consts::FRAC_PI_4,
            std::f64::consts::FRAC_PI_2 - 1e-6,
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::PI - 1e-6,
            -0.31,
        ] {
            for length in [0.0, 1e-12, 1e-8, 1.1e-8, 0.25, 1024.0, 1e7] {
                let b = [a[0] + length * theta.cos(), a[1] + length * theta.sin()];
                segments.push([a, b]);
                segments.push([b, a]);
            }
        }
    }
    segments.extend([
        [[-1e8, -1e8], [1e8, 1e8]],
        [[1e8, -1e8], [-1e8, 1e8]],
        [[0.0, 0.0], [1.0, 0.0]],
        [[0.25, -0.49], [0.75, 0.49]],
        [[0.5, -0.49], [0.500001, 0.49]],
        [[-0.0, 0.0], [f64::from_bits(1), f64::MIN_POSITIVE]],
    ]);
    segments
}

#[test]
fn spatial_candidates_preserve_exact_attribution_at_tolerance_and_extreme_bounds() {
    let mut queries = adversarial_segments();
    let mut sources = Vec::new();
    for (i, segment) in queries.iter().copied().enumerate() {
        let [a, b] = segment;
        let theta = (b[1] - a[1]).atan2(b[0] - a[0]);
        for offset in [-0.5000001, -0.5, -0.4999999, 0.0, 0.4999999, 0.5, 0.5000001] {
            let normal = [-theta.sin() * offset, theta.cos() * offset];
            let segment = segment.map(|p| [p[0] + normal[0], p[1] + normal[1]]);
            // Repeated identities and exact duplicate records must survive the index.
            sources.push(DrawingElementSegment { id: format!("owner-{}", i % 13), kind: DrawingElementKind::Opening, segment });
            if i % 7 == 0 {
                sources.push(sources.last().unwrap().clone());
            }
        }
    }
    assert!(
        crate::sheet::overlap([[0.0, 0.0], [1.0, 0.0]], [[0.5, -0.49], [0.500001, 0.49]], 0.5).is_some(),
        "short nonparallel sources really can satisfy the old predicate"
    );
    for x in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, f64::MAX, -f64::MAX, 1e8_f64.next_up(), (-1e8_f64).next_down()] {
        let segment = [[0.0, 0.0], [x, 0.0]];
        queries.push(segment);
        sources.push(DrawingElementSegment { id: "hostile".into(), kind: DrawingElementKind::FacadeWall, segment });
    }
    let index = OwnershipIndex::new(&sources);
    let mut candidates = Vec::new();
    let mut matches = 0;
    for drawn in queries {
        index.query(drawn, &mut candidates);
        assert!(candidates.windows(2).all(|w| w[0] < w[1]), "original source order, without deduplicating equal records");
        let mut before = Scene::new(100.0);
        before.line(Layer::Wall, drawn[0], drawn[1], Stroke::new(0.25));
        before.attribute_last(&sources);
        let mut after = Scene::new(100.0);
        after.line(Layer::Wall, drawn[0], drawn[1], Stroke::new(0.25));
        after.attribute_last_from(candidates.iter().map(|&i| &sources[i]));
        let Item::Path(before) = &before.items[0] else { panic!("missing baseline line") };
        let Item::Path(after) = &after.items[0] else { panic!("missing candidate line") };
        assert_eq!(after.element_segments, before.element_segments, "query {drawn:?}");
        matches += before.element_segments.len();
    }
    assert!(matches > 1000, "nonvacuous exact ownership comparison");
    assert!(index.items.len() <= sources.len(), "no source replication for long/diagonal segments");
    assert!(index.nodes.len() <= 2 * index.items.len(), "linear memory bound");
    OwnershipIndex::new(&[]).query([[0.0; 2]; 2], &mut candidates);
    assert!(candidates.is_empty());
}

#[test]
fn dense_overlapping_owners_fall_back_in_original_order_without_losing_duplicates() {
    let drawn = [[-1e8, 0.0], [1e8, 0.0]];
    let sources = vec![DrawingElementSegment { id: "duplicate".into(), kind: DrawingElementKind::Opening, segment: drawn }; 1024];
    let index = OwnershipIndex::new(&sources);
    let mut candidates = Vec::new();
    index.query(drawn, &mut candidates);
    assert_eq!(candidates, (0..sources.len()).collect::<Vec<_>>());
    let mut scene = Scene::new(100.0);
    scene.line(Layer::Glazing, drawn[0], drawn[1], Stroke::new(0.18));
    scene.attribute_last_from(candidates.iter().map(|&i| &sources[i]));
    let Item::Path(path) = &scene.items[0] else { panic!("missing line") };
    assert_eq!(path.element_segments, sources);
}

#[test]
fn merged_adversarial_scenes_keep_exact_exports_ownership_and_picks() {
    use crate::sheet::project::Edge;
    let mut projection = Projection::default();
    for (i, [a, b]) in adversarial_segments().into_iter().enumerate().filter(|(_, s)| s.iter().flatten().all(|v| v.is_finite() && v.abs() <= 1e8)) {
        let owner = DrawingElementSegment { id: format!("element-{}", i % 17), kind: DrawingElementKind::Opening, segment: [a, b] };
        projection.edges.push(Edge {
            a,
            b,
            class: Class::Frame,
            depth: (i % 3) as f64 * 9000.0,
            end_depths: [0.0; 2],
            outline: i % 2 == 0,
            element_segments: vec![owner.clone(), DrawingElementSegment { segment: [b, a], ..owner.clone() }, owner],
        });
    }
    assert!(projection.edges.iter().map(|e| e.element_segments.len()).sum::<usize>() >= 256);
    compare_and_write(&projection, None);
}
