//! What the elements are made of and how they look: materials with colours, layered build-ups
//! (walls, floors, roofs) with their element types, property sets, quantities and presentation
//! layers. Everything here is declared once and shared by the elements that use it.

use std::collections::BTreeMap;

use trigon_model::Material;
use trigon_model::build::Status;

use super::part21::{W, fine, gid, real, refs, string};

/// A material: name, category, colour (sRGB, 0–1) and transparency (0 opaque).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat {
    pub name: &'static str,
    pub category: &'static str,
    pub rgb: [f64; 3],
    pub transparency: f64,
}

const fn mat(name: &'static str, category: &'static str, rgb: [f64; 3]) -> Mat {
    Mat { name, category, rgb, transparency: 0.0 }
}

pub const BRICK: Mat = mat("Tegl, massiv mur", "brick", [0.69, 0.36, 0.25]);
pub const PLASTER: Mat = mat("Kalkpuds", "plaster", [0.93, 0.92, 0.88]);
pub const GYPSUM: Mat = mat("Gipsplade", "gypsum", [0.95, 0.95, 0.93]);
pub const STUDS: Mat = mat("Træskelet med mineraluld", "wood", [0.84, 0.74, 0.55]);
pub const JOISTS: Mat = mat("Træbjælkelag med indskud", "wood", [0.62, 0.47, 0.30]);
pub const FLOORBOARDS: Mat = mat("Gulvbrædder", "wood", [0.78, 0.62, 0.42]);
pub const CEILING: Mat = mat("Puds på forskalling", "plaster", [0.93, 0.92, 0.88]);
pub const CONCRETE: Mat = mat("Beton", "concrete", [0.72, 0.72, 0.70]);
pub const TILES: Mat = mat("Tegltag på lægter", "clay", [0.62, 0.27, 0.18]);
pub const RAFTERS: Mat = mat("Spær og undertag", "wood", [0.70, 0.55, 0.36]);
pub const FELT: Mat = mat("Tagpap", "bitumen", [0.16, 0.16, 0.17]);
pub const FRAME: Mat = mat("Træ, malet hvid", "wood", [0.95, 0.95, 0.93]);
pub const GLASS: Mat = Mat { name: "Glas", category: "glass", rgb: [0.55, 0.70, 0.80], transparency: 0.6 };
pub const DOOR: Mat = mat("Træ, dørblad", "wood", [0.42, 0.26, 0.16]);
pub const SILL: Mat = mat("Sålbænk og beklædning", "metal", [0.80, 0.78, 0.74]);
pub const RAIL: Mat = mat("Stål, malet mørk", "steel", [0.18, 0.18, 0.18]);
pub const STEEL: Mat = mat("Stål, varmforzinket", "steel", [0.66, 0.68, 0.70]);
pub const DECK: Mat = mat("Træ, terrassebrædder", "wood", [0.55, 0.35, 0.20]);
pub const STAIR: Mat = mat("Trappe", "stair", [0.70, 0.62, 0.52]);
pub const PLINTH: Mat = mat("Sokkel, puds", "plaster", [0.55, 0.55, 0.53]);
pub const RENDER: Mat = mat("Facadepuds", "plaster", [0.91, 0.90, 0.87]);
pub const BRICK_RED: Mat = mat("Tegl, røde sten", "brick", [0.62, 0.25, 0.17]);
pub const BRICK_YELLOW: Mat = mat("Tegl, gule sten", "brick", [0.78, 0.68, 0.47]);
pub const ZINC: Mat = mat("Zink", "metal", [0.60, 0.62, 0.63]);
/// Spaces: a translucent volume, no material.
pub const SPACE: Mat = Mat { name: "Rum", category: "space", rgb: [0.55, 0.75, 0.95], transparency: 0.75 };

/// The IFC material of a part of Trigon's geometry.
pub fn of(m: Material) -> Mat {
    match m {
        Material::Wall => BRICK,
        Material::Slab | Material::Reference | Material::Demolished => CONCRETE,
        Material::Roof => TILES,
        Material::Frame => FRAME,
        Material::Glass => GLASS,
        Material::Door => DOOR,
        Material::Sill => SILL,
        Material::Railing => RAIL,
        Material::Steel => STEEL,
        Material::Deck => DECK,
        Material::Plinth => PLINTH,
        Material::Plaster => PLASTER,
        Material::Render => RENDER,
        Material::BrickRed => BRICK_RED,
        Material::BrickYellow => BRICK_YELLOW,
        Material::Paving => mat("Site paving", "stone", [0.52, 0.51, 0.47]),
        Material::Gravel => mat("Site gravel", "stone", [0.62, 0.58, 0.48]),
        Material::Grass => mat("Site grass", "vegetation", [0.22, 0.31, 0.12]),
        Material::Bark => mat("Tree bark", "wood", [0.25, 0.16, 0.09]),
        Material::Foliage => mat("Tree crown", "vegetation", [0.19, 0.32, 0.10]),
        Material::Soil => mat("Planter soil", "soil", [0.20, 0.14, 0.08]),
        Material::Planter => mat("Planter shell", "concrete", [0.30, 0.31, 0.28]),
    }
}

/// Presentation layers (Vectorworks turns them into classes).
pub const L_FACADE: &str = "A-Ydervægge";
pub const L_INTERIOR: &str = "A-Indervægge";
pub const L_FLOOR: &str = "A-Dæk";
pub const L_ROOF: &str = "A-Tag";
pub const L_WINDOW: &str = "A-Vinduer";
pub const L_DOOR: &str = "A-Døre";
pub const L_STAIR: &str = "A-Trapper";
pub const L_SPACE: &str = "A-Rum";
pub const L_BALCONY: &str = "A-Altaner";
pub const L_OPENING: &str = "A-Åbninger";

/// A layer of a build-up: material, thickness (mm) and what it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub mat: Mat,
    pub thickness: f64,
    pub category: &'static str,
}

/// A layered construction and the element type named after it.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildUp {
    pub name: String,
    /// Wall layers from the reference side (outside, or the right of the axis) in; slab layers from the bottom up.
    pub layers: Vec<Layer>,
    /// IfcWallTypeEnum or IfcSlabTypeEnum.
    pub predefined: &'static str,
}

impl BuildUp {
    #[cfg(test)]
    pub fn thickness(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness).sum()
    }

    /// The material that shows on the element's faces.
    pub fn face(&self) -> Mat {
        self.layers.first().map_or(CONCRETE, |l| l.mat)
    }
}

fn layer(mat: Mat, thickness: f64, category: &'static str) -> Layer {
    Layer { mat, thickness, category }
}

/// A core between two finishes, or the core alone when the element is too thin for them.
fn sandwich(t: f64, finish: (Mat, f64), core: Mat, other: Option<(Mat, f64)>) -> Vec<Layer> {
    let skins = finish.1 + other.map_or(0.0, |o| o.1);
    if !(t > skins + 20.0) {
        return vec![layer(core, t.max(0.0), "LoadBearing")];
    }
    let mut v = vec![layer(finish.0, finish.1, "Finish"), layer(core, t - skins, "LoadBearing")];
    if let Some((m, d)) = other {
        v.push(layer(m, d, "Finish"));
    }
    v
}

/// Danish brick thickness in half bricks (½ sten = 120 mm with its joint), if `t` is one.
fn sten(t: f64) -> Option<String> {
    let k = (t / 120.0).round();
    if !((1.0..=20.0).contains(&k) && (t - k * 120.0).abs() <= 25.0) {
        return None;
    }
    let k = k as u32;
    Some(match (k / 2, k % 2) {
        (0, _) => "½".to_string(),
        (whole, 0) => format!("{whole}"),
        (whole, _) => format!("{whole}½"),
    })
}

fn mm(t: f64) -> String {
    format!("{t:.0} mm")
}

/// A facade wall: brick with interior plaster, its outside in the facade's finish (`Wall` for the
/// building's facing brick, `BrickRed` or `Render`). Party walls are fire gables.
pub fn facade(t: f64, party: bool, finish: Material) -> BuildUp {
    let what = if party { "Brandgavl" } else { "Ydervæg" };
    let (wall, brick, render) = match finish {
        Material::Render => ("pudset mur", BRICK, true),
        Material::BrickRed => ("massiv mur, røde sten", BRICK_RED, false),
        Material::BrickYellow => ("massiv mur, gule sten", BRICK_YELLOW, false),
        _ => ("massiv mur", BRICK, false),
    };
    let name = match sten(t) {
        Some(s) => format!("{what}, {wall} {s} sten, {}", mm(t)),
        None => format!("{what}, {wall}, {}", mm(t)),
    };
    let outside = render.then_some((RENDER, 15.0));
    // From the outside in: render if any, brick, then the interior plaster.
    let mut layers = sandwich(t, (PLASTER, 15.0), brick, outside);
    layers.reverse();
    BuildUp { name, layers, predefined: "SOLIDWALL" }
}

/// An interior wall: plastered brick when load-bearing, a light partition otherwise.
pub fn interior(t: f64, load_bearing: bool) -> BuildUp {
    if load_bearing {
        let name = match sten(t) {
            Some(s) => format!("Bærende indervæg, massiv mur {s} sten, {}", mm(t)),
            None => format!("Bærende indervæg, massiv mur, {}", mm(t)),
        };
        BuildUp { name, layers: sandwich(t, (PLASTER, 10.0), BRICK, Some((PLASTER, 10.0))), predefined: "SOLIDWALL" }
    } else {
        BuildUp {
            name: format!("Let skillevæg, {}", mm(t)),
            layers: sandwich(t, (GYPSUM, 13.0), STUDS, Some((GYPSUM, 13.0))),
            predefined: "PARTITIONING",
        }
    }
}

/// A floor: a timber joist floor (etagedæk), or a concrete slab on the ground.
pub fn floor(t: f64, on_ground: bool) -> BuildUp {
    if on_ground {
        return BuildUp { name: format!("Terrændæk, beton {}", mm(t)), layers: vec![layer(CONCRETE, t, "LoadBearing")], predefined: "BASESLAB" };
    }
    BuildUp {
        name: format!("Etagedæk, træbjælkelag {}", mm(t)),
        // Bottom up: ceiling plaster, joists with pugging, floorboards.
        layers: sandwich(t, (CEILING, 20.0), JOISTS, Some((FLOORBOARDS, 22.0))),
        predefined: "FLOOR",
    }
}

/// A flat roof deck: joists and felt.
pub fn flat_roof(t: f64) -> BuildUp {
    let layers = if t > 30.0 { vec![layer(JOISTS, t - 10.0, "LoadBearing"), layer(FELT, 10.0, "Finish")] } else { vec![layer(FELT, t, "Finish")] };
    BuildUp { name: format!("Tagdæk, fladt {}", mm(t)), layers, predefined: "ROOF" }
}

/// A pitched roof plane, from its underside up: rafters, then tiles on battens.
pub fn pitched_roof(t: f64) -> BuildUp {
    let layers = if t > 80.0 { vec![layer(RAFTERS, t - 50.0, "LoadBearing"), layer(TILES, 50.0, "Finish")] } else { vec![layer(TILES, t, "Finish")] };
    BuildUp { name: format!("Tag, tegl på spær {}", mm(t)), layers, predefined: "ROOF" }
}

/// A property value.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Label(String),
    Text(String),
    Id(String),
    Bool(bool),
    Length(f64),
    Count(f64),
    Angle(f64),
    /// Pset_*Common.Status, an enumerated value.
    Status(Status),
}

impl Val {
    fn text(&self) -> String {
        match self {
            Val::Text(s) => format!("IFCTEXT({})", string(s)),
            Val::Label(s) => format!("IFCLABEL({})", string(s)),
            Val::Id(s) => format!("IFCIDENTIFIER({})", string(s)),
            Val::Bool(b) => format!("IFCBOOLEAN({})", if *b { ".T." } else { ".F." }),
            Val::Length(v) => format!("IFCPOSITIVELENGTHMEASURE({})", real(v.max(1e-6))),
            Val::Count(v) => format!("IFCCOUNTMEASURE({})", real(v.max(0.0))),
            Val::Angle(v) => format!("IFCPLANEANGLEMEASURE({})", real(*v)),
            Val::Status(s) => format!("IFCLABEL('{}')", status(*s)),
        }
    }
}

/// The IFC name of a phase status.
pub fn status(s: Status) -> &'static str {
    match s {
        Status::Existing => "EXISTING",
        Status::New => "NEW",
        Status::Demolished => "DEMOLISH",
    }
}

/// A quantity: name and value in the project's units (mm, m², m³).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Qty {
    Length(&'static str, f64),
    Area(&'static str, f64),
    Volume(&'static str, f64),
}

/// Shared declarations, written once and related to their users at the end.
#[derive(Default)]
pub struct Catalog {
    styles: BTreeMap<&'static str, usize>,
    materials: BTreeMap<&'static str, usize>,
    layer_sets: BTreeMap<String, usize>,
    usages: BTreeMap<String, usize>,
    /// What each material definition (layer set, usage, material, constituent set) is associated with.
    associations: BTreeMap<String, (usize, Vec<usize>)>,
    /// Element types by key: entity and the occurrences typed by it.
    pub types: BTreeMap<String, (usize, Vec<usize>)>,
    pub fill_quantity_counts: BTreeMap<(String, String), usize>,
    psets: BTreeMap<String, (usize, Vec<usize>)>,
    status_enum: Option<usize>,
    layers: BTreeMap<&'static str, Vec<usize>>,
    /// The Body context, for styled material representations.
    pub context: usize,
}

impl Catalog {
    /// The surface style of a material.
    pub fn style(&mut self, w: &mut W, m: Mat) -> usize {
        if let Some(&s) = self.styles.get(m.name) {
            return s;
        }
        let [r, g, b] = m.rgb;
        let colour = w.add(format!("IFCCOLOURRGB($,{},{},{})", real(r), real(g), real(b)));
        let rendering = w.add(format!("IFCSURFACESTYLERENDERING(#{colour},{},$,$,$,$,$,$,.NOTDEFINED.)", real(m.transparency)));
        let s = w.add(format!("IFCSURFACESTYLE({},.BOTH.,(#{rendering}))", string(m.name)));
        self.styles.insert(m.name, s);
        s
    }

    /// Gives a representation item the material's colour.
    pub fn paint(&mut self, w: &mut W, item: usize, m: Mat) {
        let s = self.style(w, m);
        w.add(format!("IFCSTYLEDITEM(#{item},(#{s}),$)"));
    }

    /// The IfcMaterial, with its colour as a styled representation.
    pub fn material(&mut self, w: &mut W, m: Mat) -> usize {
        if let Some(&id) = self.materials.get(m.name) {
            return id;
        }
        let id = w.add(format!("IFCMATERIAL({},$,{})", string(m.name), string(m.category)));
        let s = self.style(w, m);
        let item = w.add(format!("IFCSTYLEDITEM($,(#{s}),$)"));
        let rep = w.add(format!("IFCSTYLEDREPRESENTATION(#{},'Style','Material',(#{item}))", self.context));
        w.add(format!("IFCMATERIALDEFINITIONREPRESENTATION($,$,(#{rep}),#{id})"));
        self.materials.insert(m.name, id);
        id
    }

    /// The IfcMaterialLayerSet of a build-up.
    pub fn layer_set(&mut self, w: &mut W, b: &BuildUp) -> usize {
        if let Some(&id) = self.layer_sets.get(&b.name) {
            return id;
        }
        let layers: Vec<usize> = b
            .layers
            .iter()
            .map(|l| {
                let m = self.material(w, l.mat);
                w.add(format!("IFCMATERIALLAYER(#{m},{},.F.,{},$,'{}',$)", real(l.thickness), string(l.mat.name), l.category))
            })
            .collect();
        let id = w.add(format!("IFCMATERIALLAYERSET(({}),{},$)", refs(&layers), string(&b.name)));
        self.layer_sets.insert(b.name.clone(), id);
        id
    }

    /// An IfcMaterialLayerSetUsage: layers along `direction` (AXIS2 for walls, AXIS3 for slabs)
    /// from `offset` off the reference line, towards positive or negative.
    pub fn usage(&mut self, w: &mut W, b: &BuildUp, direction: &str, positive: bool, offset: f64) -> usize {
        let key = format!("{}|{direction}|{positive}|{}", b.name, real(offset));
        if let Some(&id) = self.usages.get(&key) {
            return id;
        }
        let set = self.layer_set(w, b);
        let sense = if positive { ".POSITIVE." } else { ".NEGATIVE." };
        let id = w.add(format!("IFCMATERIALLAYERSETUSAGE(#{set},.{direction}.,{sense},{},$)", real(offset)));
        self.usages.insert(key, id);
        id
    }

    /// Associates a material definition (`key` names it) with an object or type.
    pub fn associate(&mut self, key: &str, definition: usize, object: usize) {
        self.associations.entry(key.to_string()).or_insert((definition, Vec::new())).1.push(object);
    }

    /// The element type for `key`, made by `make` the first time; `object` becomes one of its occurrences.
    pub fn typed(&mut self, w: &mut W, key: &str, object: usize, make: impl FnOnce(&mut W) -> usize) -> usize {
        if let Some(entry) = self.types.get_mut(key) {
            entry.1.push(object);
            return entry.0;
        }
        let id = make(w);
        self.types.insert(key.to_string(), (id, vec![object]));
        id
    }

    /// Puts `object` in a property set; identical sets are written once and shared.
    pub fn pset(&mut self, w: &mut W, name: &str, props: &[(&str, Val)], object: usize) {
        if props.is_empty() {
            return;
        }
        let mut text: Vec<String> = Vec::with_capacity(props.len());
        for (k, v) in props {
            text.push(match v {
                Val::Status(_) => {
                    let e = self.status_enum(w);
                    format!("IFCPROPERTYENUMERATEDVALUE({},$,({}),#{e})", string(k), v.text())
                }
                _ => format!("IFCPROPERTYSINGLEVALUE({},$,{},$)", string(k), v.text()),
            });
        }
        let key = format!("{name}|{}", text.join("|"));
        if let Some(entry) = self.psets.get_mut(&key) {
            entry.1.push(object);
            return;
        }
        let ids: Vec<usize> = text.into_iter().map(|t| w.add(t)).collect();
        let id = w.add(format!("IFCPROPERTYSET({},$,{},$,({}))", gid(&format!("pset:{key}")), string(name), refs(&ids)));
        self.psets.insert(key, (id, vec![object]));
    }

    fn status_enum(&mut self, w: &mut W) -> usize {
        if let Some(e) = self.status_enum {
            return e;
        }
        let values = ["NEW", "EXISTING", "DEMOLISH", "TEMPORARY", "OTHER", "NOTKNOWN", "UNSET"].map(|v| format!("IFCLABEL('{v}')")).join(",");
        let e = w.add(format!("IFCPROPERTYENUMERATION('PEnum_ElementStatus',({values}),$)"));
        self.status_enum = Some(e);
        e
    }

    /// An element's base quantities (`Qto_…BaseQuantities`).
    pub fn quantities(&mut self, w: &mut W, key: &str, name: &str, qs: &[Qty], object: usize) {
        let ids: Vec<usize> = qs
            .iter()
            .map(|q| match *q {
                Qty::Length(n, v) => w.add(format!("IFCQUANTITYLENGTH('{n}',$,$,{},$)", real(finite(v)))),
                Qty::Area(n, v) => w.add(format!("IFCQUANTITYAREA('{n}',$,$,{},$)", fine(finite(v)))),
                Qty::Volume(n, v) => w.add(format!("IFCQUANTITYVOLUME('{n}',$,$,{},$)", fine(finite(v)))),
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        let q = w.add(format!("IFCELEMENTQUANTITY({},$,'{name}',$,$,({}))", gid(&format!("{key}.qto")), refs(&ids)));
        w.add(format!("IFCRELDEFINESBYPROPERTIES({},$,$,$,(#{object}),#{q})", gid(&format!("{key}.qto.rel"))));
    }

    /// Puts a shape representation on a presentation layer.
    pub fn layer(&mut self, name: &'static str, representation: usize) {
        self.layers.entry(name).or_default().push(representation);
    }

    /// Writes the relationships that collect everything declared: types and their occurrences,
    /// material associations, shared property sets and presentation layers.
    pub fn finish(&mut self, w: &mut W, project: usize) {
        let mut declared = Vec::new();
        for (key, (t, objects)) in &self.types {
            w.add(format!("IFCRELDEFINESBYTYPE({},$,$,$,({}),#{t})", gid(&format!("type:{key}.objects")), refs(objects)));
            declared.push(*t);
        }
        if !declared.is_empty() {
            w.add(format!("IFCRELDECLARES({},$,$,$,#{project},({}))", gid("declares"), refs(&declared)));
        }
        for (key, (definition, objects)) in &self.associations {
            w.add(format!("IFCRELASSOCIATESMATERIAL({},$,$,$,({}),#{definition})", gid(&format!("material:{key}")), refs(objects)));
        }
        for (key, (pset, objects)) in &self.psets {
            w.add(format!("IFCRELDEFINESBYPROPERTIES({},$,$,$,({}),#{pset})", gid(&format!("pset:{key}.rel")), refs(objects)));
        }
        for (name, reps) in &self.layers {
            w.add(format!("IFCPRESENTATIONLAYERASSIGNMENT({},$,({}),$)", string(name), refs(reps)));
        }
    }
}

fn finite(v: f64) -> f64 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masonry_is_named_in_bricks_and_layers_add_up() {
        let brick = Material::Wall;
        assert_eq!(facade(600.0, false, brick).name, "Ydervæg, massiv mur 2½ sten, 600 mm");
        assert_eq!(facade(700.0, false, brick).name, "Ydervæg, massiv mur 3 sten, 700 mm");
        assert_eq!(facade(360.0, true, brick).name, "Brandgavl, massiv mur 1½ sten, 360 mm");
        assert_eq!(facade(555.0, false, brick).name, "Ydervæg, massiv mur, 555 mm");
        assert_eq!(facade(480.0, false, Material::BrickRed).name, "Ydervæg, massiv mur, røde sten 2 sten, 480 mm");
        let rendered = facade(480.0, false, Material::Render);
        assert_eq!(rendered.name, "Ydervæg, pudset mur 2 sten, 480 mm");
        assert_eq!(rendered.layers.iter().map(|l| l.mat).collect::<Vec<_>>(), vec![RENDER, BRICK, PLASTER]);
        assert_eq!(interior(120.0, true).name, "Bærende indervæg, massiv mur ½ sten, 120 mm");
        for b in [
            facade(600.0, false, brick),
            facade(45.0, false, brick),
            interior(100.0, false),
            interior(40.0, false),
            floor(200.0, false),
            flat_roof(300.0),
        ] {
            let t = b.layers.iter().map(|l| l.thickness).sum::<f64>();
            assert!((t - b.thickness()).abs() < 1e-9 && b.layers.iter().all(|l| l.thickness > 0.0), "{}", b.name);
        }
        // Brick on the outside, plaster on the inside.
        let f = facade(480.0, false, brick);
        assert_eq!((f.layers[0].mat, f.layers[1].mat), (BRICK, PLASTER));
        assert!((f.thickness() - 480.0).abs() < 1e-9);
    }
}
