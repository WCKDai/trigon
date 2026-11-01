//! A made-up building for checking the IFC export and for `examples/sample-building.ifc`: an
//! L-shaped block of three storeys on a raised ground floor, masonry walls stepping from 480 to
//! 360 mm, a fire gable, a pitched roof with two gables, a U-turn staircase with a stairwell
//! window across the floors, windows and doors, a steel balcony with tie rods (new in the
//! proposal phase) and a cantilever balcony, interior walls with doors, a plinth, a rendered
//! courtyard facade, rendered bands on the street front, gutters and rooms. Not any real address.

use trigon_model::component::{Deck, Finish, Railing, Screens};
use trigon_model::doc::{Band, Plinth, WallFinish};
use trigon_model::{
    Building, ComponentKind, ComponentType, Document, FacadeKind, GeoPoint, InteriorWall, Phase, Placement, Result, Roof, Room, Site, Stair,
    StairKind, WallDoor,
};

fn put<'b>(b: &'b mut Building, facade: usize, level: usize, bay: usize, ty: &str) -> Option<&'b mut Placement> {
    let f = b.facades.get_mut(facade)?;
    f.placements.push(Placement {
        u: None,
        level,
        bay,
        type_id: ty.into(),
        width: None,
        height: None,
        sill: None,
        spans_storeys: false,
        created: None,
        demolished: None,
    });
    f.placements.last_mut()
}

fn wall(id: &str, levels: &[usize], a: [f64; 2], b: [f64; 2], thickness: f64, load_bearing: bool, doors: &[f64]) -> InteriorWall {
    let doors = doors.iter().map(|&at| WallDoor { at, width: 900.0, height: 2100.0 }).collect();
    InteriorWall { id: id.into(), levels: levels.to_vec(), a, b, thickness, load_bearing, doors }
}

fn room(id: &str, levels: &[usize], name: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Room {
    Room { id: id.into(), levels: levels.to_vec(), name: name.into(), polygon: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]] }
}

/// The sample document.
pub fn document() -> Result<Document> {
    let mut doc = Document {
        // A made-up point in a park in Copenhagen (no building there).
        site: Some(Site {
            name: "Trigon eksempel, Prøvevej 1".into(),
            origin: GeoPoint { lat: 55.7, lon: 12.57 },
            origin_utm: [724321.791, 6178467.512],
            elevation: 10.0,
            evidence: Vec::new(),
        }),
        phases: vec![Phase { id: "1940".into(), name: "Opført".into() }, Phase { id: "forslag".into(), name: "Forslag".into() }],
        ..Document::default()
    };
    doc.types.push(ComponentType {
        id: "altan.staal".into(),
        name: "Stålaltan med trækstænger".into(),
        width: 2400.0,
        height: 1000.0,
        sill: 0.0,
        kind: ComponentKind::SteelBalcony {
            depth: 1300.0,
            railing: Railing::Frederiksberg,
            finish: Finish::Galvanised,
            deck: Deck::Cumaru,
            handrail: true,
            screens: Screens::Left,
            tie_rods: true,
        },
    });
    // Facades in order: 0 south (street), 1 east (gable), 2 north of the main block, 3 east of the
    // wing, 4 north end of the wing (gable), 5 west (fire gable against a neighbour).
    let footprint = [[0.0, 0.0], [18000.0, 0.0], [18000.0, 10000.0], [8000.0, 10000.0], [8000.0, 16000.0], [0.0, 16000.0]];
    let mut b = Building::new("b1", "Eksempelhus", &footprint, 3, 3000.0)?;
    b.set_levels(&[300.0, 3300.0, 6100.0], 8900.0)?;
    b.set_wall_schedule(&[480.0, 360.0, 360.0])?;
    let ids: Vec<String> = b.facades.iter().map(|f| f.id.clone()).collect();
    let id = |k: usize| ids.get(k).cloned().unwrap_or_default();
    b.set_facade_schedule(&[id(5)], &[360.0, 360.0, 360.0])?;
    for (k, kind) in
        [FacadeKind::Street, FacadeKind::Courtyard, FacadeKind::Courtyard, FacadeKind::Courtyard, FacadeKind::Courtyard, FacadeKind::Party]
            .into_iter()
            .enumerate()
    {
        if let Some(f) = b.facades.get_mut(k) {
            f.kind = kind;
        }
    }
    b.roof = Roof::Pitched {
        pitch_deg: 40.0,
        gables: vec![id(1), id(4)],
        straighten_mm: 1200.0,
        overhang_mm: 300.0,
        verge_mm: 100.0,
        thickness_mm: 250.0,
        gutters: true,
        drainage: None,
        dormers: Vec::new(),
    };
    b.plinth = Some(Plinth { height: 500.0, projection: 30.0 });
    // A rendered courtyard side, and rendered bands at the floors of the street front.
    if let Some(f) = b.facades.get_mut(2) {
        f.finish = Some(WallFinish::Render);
    }
    if let Some(f) = b.facades.get_mut(0) {
        f.bands = vec![Band { levels: vec![1, 2], bottom: -300.0, top: 100.0, projection: 40.0, finish: None }];
    }
    let bays: [Vec<f64>; 5] = [
        trigon_model::build::even_bays(18000.0, 5, 900.0),
        trigon_model::build::even_bays(10000.0, 3, 900.0),
        trigon_model::build::even_bays(10000.0, 3, 900.0),
        vec![1500.0, 4150.0],
        trigon_model::build::even_bays(8000.0, 2, 900.0),
    ];
    for (k, centres) in bays.into_iter().enumerate() {
        if let Some(f) = b.facades.get_mut(k) {
            f.bays = centres;
        }
    }
    // Street: an entrance, windows, and in the proposal French doors onto a steel balcony
    // where a window was.
    for level in 0..3 {
        for bay in 0..5 {
            let ty = if level == 0 && bay == 2 { "door.entrance" } else { "window.casement.2" };
            if let Some(p) = put(&mut b, 0, level, bay, ty)
                && level > 0
                && bay == 2
            {
                p.demolished = Some("forslag".into());
            }
        }
    }
    for level in [1, 2] {
        if let Some(p) = put(&mut b, 0, level, 2, "door.french") {
            p.created = Some("forslag".into());
        }
        if let Some(p) = put(&mut b, 0, level, 2, "altan.staal") {
            p.created = Some("forslag".into());
        }
    }
    // Courtyard and gables: windows; a cantilever balcony and a French balcony at the back.
    for (facade, count) in [(1, 3), (2, 3), (4, 2)] {
        for level in 0..3 {
            for bay in 0..count {
                let ty = match (facade, level, bay) {
                    (2, 1, 1) => "balcony.cantilever",
                    (2, 2, 1) => "balcony.french",
                    _ => "window.casement.2",
                };
                put(&mut b, facade, level, bay, ty);
            }
        }
    }
    // The wing's east side: windows, and a stairwell window across each floor the stair climbs.
    for level in 0..3 {
        put(&mut b, 3, level, 0, "window.casement.1");
    }
    for level in [0, 1] {
        if let Some(p) = put(&mut b, 3, level, 1, "window.fixed") {
            (p.width, p.height, p.sill, p.spans_storeys) = (Some(900.0), Some(1500.0), Some(2200.0), true);
        }
    }
    b.interior_walls = vec![
        wall("spine", &[0, 1, 2], [400.0, 5000.0], [17600.0, 5000.0], 240.0, true, &[3600.0, 12600.0]),
        wall("wing.a", &[0, 1, 2], [400.0, 10000.0], [3000.0, 10000.0], 100.0, false, &[]),
        wall("wing.b", &[0, 1, 2], [3900.0, 10000.0], [7400.0, 10000.0], 100.0, false, &[]),
        wall("stairwell", &[0, 1, 2], [5100.0, 10500.0], [5100.0, 15400.0], 120.0, true, &[1000.0]),
        wall("room", &[1, 2], [9000.0, 400.0], [9000.0, 4850.0], 100.0, false, &[3500.0]),
    ];
    b.stairs = vec![Stair {
        id: "trappe".into(),
        levels: vec![0, 1],
        outline: vec![[7400.0, 10800.0], [7400.0, 13600.0], [5200.0, 13600.0], [5200.0, 10800.0]],
        tread: None,
        kind: StairKind::UTurn,
        landing: None,
        well: None,
        rounded: false,
        winders: None,
        turn_depths: None,
    }];
    b.rooms = vec![
        room("butik", &[0], "Butik", 480.0, 480.0, 17520.0, 4880.0),
        room("lager", &[0], "Lager", 480.0, 5120.0, 17520.0, 9520.0),
        room("kontor", &[0], "Kontor", 360.0, 10050.0, 5040.0, 15520.0),
        room("stue", &[1, 2], "Stue", 360.0, 360.0, 8950.0, 4880.0),
        room("vaerelse", &[1, 2], "Værelse", 9050.0, 360.0, 17640.0, 4880.0),
        room("koekken", &[1, 2], "Køkken", 360.0, 5120.0, 17640.0, 9640.0),
        room("bad", &[1, 2], "Bad", 360.0, 10050.0, 5040.0, 15640.0),
        room("trapperum", &[0, 1, 2], "Trapperum", 5160.0, 10050.0, 7520.0, 15520.0),
    ];
    doc.buildings.push(b);
    Ok(doc)
}
