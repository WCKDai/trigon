//! One 2D drawing scene for every sheet. Plans, elevations and sections are built as paths and
//! texts in drawing-plane millimetres at 1:1 on named layers, with stroke weights and text sizes in
//! paper millimetres at the sheet's scale (DS/ISO 128 pens). Writers turn a scene into SVG (which
//! feeds the PNG and PDF output) and DXF.

pub(crate) mod cut;
pub(crate) mod display;
pub(crate) mod dxf;
pub(crate) mod project;
pub(crate) mod style;
pub(crate) mod svg;

use crate::views::{DrawingElementKind, DrawingElementSegment};

#[derive(Clone, Debug)]
pub(crate) struct Element {
    pub(crate) id: String,
    pub(crate) kind: DrawingElementKind,
}

impl Element {
    pub(crate) fn new(id: &str, kind: DrawingElementKind) -> Self {
        Self { id: id.into(), kind }
    }

    pub(crate) fn segment(&self, segment: [[f64; 2]; 2]) -> DrawingElementSegment {
        DrawingElementSegment { id: self.id.clone(), kind: self.kind, segment }
    }
}

pub(crate) fn overlap<const DIM: usize>(drawn: [[f64; DIM]; 2], source: [[f64; DIM]; 2], epsilon: f64) -> Option<[[f64; DIM]; 2]> {
    if drawn.iter().chain(source.iter()).flatten().any(|value| !value.is_finite() || value.abs() > 1e8) {
        return None;
    }
    let direction: [f64; DIM] = std::array::from_fn(|axis| drawn[1][axis] - drawn[0][axis]);
    let length2: f64 = direction.iter().map(|value| value * value).sum();
    if length2 <= 1e-16 {
        return None;
    }
    let parameters = source.map(|point| (0..DIM).map(|axis| (point[axis] - drawn[0][axis]) * direction[axis]).sum::<f64>() / length2);
    for (point, parameter) in source.iter().zip(parameters) {
        let distance2: f64 = (0..DIM).map(|axis| (point[axis] - drawn[0][axis] - parameter * direction[axis]).powi(2)).sum();
        if distance2 > epsilon * epsilon {
            return None;
        }
    }
    let start = parameters[0].min(parameters[1]).max(0.0);
    let end = parameters[0].max(parameters[1]).min(1.0);
    if end <= start || (end - start) * length2.sqrt() <= 1e-8 {
        return None;
    }
    Some([start, end].map(|parameter| std::array::from_fn(|axis| drawn[0][axis] + parameter * direction[axis])))
}

/// The shared presentation scene, used both by file exports and live drafting workspaces.
#[derive(Clone)]
pub(crate) struct Sheet {
    pub(crate) scene: Scene,
    pub(crate) vp: Viewport,
    pub(crate) clip: [f64; 4],
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) caption: String,
    pub(crate) note: Option<String>,
    pub(crate) metrics: serde_json::Value,
}

impl Sheet {
    pub(crate) fn svg(&self) -> String {
        svg::sheet(
            &svg::Frame { title: &self.title, project: &self.project, caption: &self.caption, footer: &self.title, note: self.note.clone() },
            &self.scene,
            &self.vp,
            self.clip,
        )
    }

    pub(crate) fn write(&self, path: std::path::PathBuf) -> crate::Result<serde_json::Value> {
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default();
        let mut result = self.metrics.clone();
        result["written"] = serde_json::json!(path);
        let bytes = match ext {
            "dxf" => {
                let text = dxf::write(
                    &self.scene,
                    &dxf::TitleBlock { title: &self.title, project: &self.project, caption: &self.caption, note: self.note.clone() },
                );
                result["dxf"] = dxf::summary(&text);
                dxf::bytes(&text)
            }
            "svg" => self.svg().into_bytes(),
            "png" => crate::plan_output::png(&self.svg())?,
            "pdf" => crate::plan_output::pdf(&[self.svg()], PAGE_MM)?,
            _ => return Err(trigon_model::ModelError::Invalid("use .pdf, .png, .svg or .dxf for a drawing view".into()).into()),
        };
        crate::plan::write_output(&path, &bytes)?;
        if ext == "png" {
            result["image_path"] = serde_json::json!(path);
        }
        Ok(result)
    }
}

/// The 1600 × 900 px sheet prints as 338.67 × 190.5 mm (96 dpi).
pub(crate) const PAGE_MM: [f64; 2] = [338.666_666_7, 190.5];
pub(crate) const MM_PER_PX: f64 = PAGE_MM[0] / 1600.0;

/// DS/ISO 128 pens, paper mm.
pub(crate) const PENS: [f64; 7] = [0.13, 0.18, 0.25, 0.35, 0.5, 0.7, 1.0];

pub(crate) type Rgb = [u8; 3];
/// Line work: near black.
pub(crate) const INK: Rgb = [26, 26, 26];
/// Lettering, as on the existing sheets.
pub(crate) const LETTERING: Rgb = [0x24, 0x24, 0x24];

/// The pen `steps` sizes thinner than `pen` (never below the finest).
pub(crate) fn thinner(pen: f64, steps: usize) -> f64 {
    let k = PENS.iter().position(|p| *p >= pen - 1e-9).unwrap_or(PENS.len() - 1);
    PENS[k.saturating_sub(steps)]
}

/// Drawing layers, named after the AIA/NCS convention CAD users expect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Layer {
    /// Cut walls, slabs and roofs: the heaviest outline.
    WallCut,
    /// Poche and hatching inside cut outlines.
    WallPattern,
    /// Walls in projection.
    Wall,
    Floor,
    Roof,
    Glazing,
    Door,
    Balcony,
    Stair,
    /// Light tonal fills of surfaces in projection.
    Tone,
    Annotation,
    /// Title, caption and scale.
    Title,
    Ground,
    Level,
}

impl Layer {
    pub(crate) const ALL: [Layer; 14] = [
        Layer::WallCut,
        Layer::WallPattern,
        Layer::Wall,
        Layer::Floor,
        Layer::Roof,
        Layer::Glazing,
        Layer::Door,
        Layer::Balcony,
        Layer::Stair,
        Layer::Tone,
        Layer::Annotation,
        Layer::Title,
        Layer::Ground,
        Layer::Level,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Layer::WallCut => "A-WALL-CUT",
            Layer::WallPattern => "A-WALL-PATT",
            Layer::Wall => "A-WALL",
            Layer::Floor => "A-FLOR",
            Layer::Roof => "A-ROOF",
            Layer::Glazing => "A-GLAZ",
            Layer::Door => "A-DOOR",
            Layer::Balcony => "A-BALC",
            Layer::Stair => "A-FLOR-STRS",
            Layer::Tone => "A-ELEV-PATT",
            Layer::Annotation => "A-ANNO",
            Layer::Title => "A-ANNO-TTLB",
            Layer::Ground => "A-GRND",
            Layer::Level => "A-LEVL",
        }
    }

    /// AutoCAD colour index: 7 prints black (white on a dark screen), 8 and 9 are greys.
    pub(crate) fn aci(self) -> u8 {
        match self {
            Layer::WallPattern => 8,
            Layer::Tone => 9,
            _ => 7,
        }
    }

    /// The layer's pen, paper mm.
    pub(crate) fn pen(self) -> f64 {
        match self {
            Layer::WallCut => 0.5,
            Layer::Ground => 0.7,
            Layer::Wall | Layer::Roof | Layer::Door | Layer::Balcony | Layer::Floor | Layer::Title => 0.25,
            Layer::Glazing | Layer::Stair | Layer::Annotation | Layer::Level => 0.18,
            Layer::WallPattern | Layer::Tone => 0.13,
        }
    }

    pub(crate) fn dashed(self) -> bool {
        self == Layer::Level
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stroke {
    /// Paper mm.
    pub(crate) pen: f64,
    pub(crate) colour: Rgb,
    pub(crate) dashed: bool,
    pub(crate) pattern: trigon_model::annotation::LinePattern,
}

impl Stroke {
    pub(crate) fn pattern(&self) -> trigon_model::annotation::LinePattern {
        if self.dashed { trigon_model::annotation::LinePattern::Dashed } else { self.pattern }
    }

    pub(crate) fn new(pen: f64) -> Self {
        Stroke { pen, colour: INK, dashed: false, pattern: Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fill {
    Solid(Rgb),
    /// Masonry: 45° lines over white.
    Masonry,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Path {
    pub(crate) layer: Layer,
    /// One or more rings (closed paths, filled even-odd) or polylines.
    pub(crate) rings: Vec<Vec<[f64; 2]>>,
    pub(crate) closed: bool,
    pub(crate) fill: Option<Fill>,
    pub(crate) stroke: Option<Stroke>,
    pub(crate) element_segments: Vec<DrawingElementSegment>,
}

impl Path {
    pub(crate) fn segments(&self) -> impl Iterator<Item = [[f64; 2]; 2]> + '_ {
        self.rings.iter().flat_map(|ring| {
            ring.windows(2).map(|pair| [pair[0], pair[1]]).chain((self.closed && ring.len() > 2).then(|| [ring[ring.len() - 1], ring[0]]))
        })
    }

    pub(crate) fn attribute(&mut self, sources: &[DrawingElementSegment]) {
        let elements: Vec<_> = self
            .segments()
            .flat_map(|drawn| {
                sources
                    .iter()
                    .filter_map(move |source| overlap(drawn, source.segment, 0.5).map(|segment| DrawingElementSegment { segment, ..source.clone() }))
            })
            .collect();
        self.element_segments.extend(elements);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Anchor {
    Start,
    Middle,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Halo {
    None,
    /// A white outline around the glyphs.
    Outline,
    /// A white box behind the text.
    Box,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Text {
    pub(crate) layer: Layer,
    pub(crate) at: [f64; 2],
    pub(crate) text: String,
    /// Font size (em), paper mm.
    pub(crate) size: f64,
    /// Counter-clockwise, degrees.
    pub(crate) deg: f64,
    pub(crate) anchor: Anchor,
    /// Centred on `at` vertically; otherwise `at` is on the baseline.
    pub(crate) middle: bool,
    pub(crate) colour: Rgb,
    pub(crate) halo: Halo,
    pub(crate) style: trigon_model::annotation::TextStyle,
}

impl Text {
    pub(crate) fn new(layer: Layer, at: [f64; 2], text: impl Into<String>, size: f64) -> Self {
        Text {
            layer,
            at,
            text: text.into(),
            size,
            deg: 0.0,
            anchor: Anchor::Middle,
            middle: true,
            colour: LETTERING,
            halo: Halo::None,
            style: Default::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Path(Path),
    Text(Text),
}

/// A drawing in drawing-plane mm at 1:1, meant to be printed at 1:`scale`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Scene {
    pub(crate) scale: f64,
    pub(crate) items: Vec<Item>,
    pub(crate) element_regions: Vec<crate::views::DrawingElementRegion>,
}

impl Scene {
    pub(crate) fn new(scale: f64) -> Self {
        Scene { scale, items: Vec::new(), element_regions: Vec::new() }
    }

    pub(crate) fn polygon(&mut self, layer: Layer, rings: Vec<Vec<[f64; 2]>>, fill: Option<Fill>, stroke: Option<Stroke>) {
        if !rings.is_empty() {
            self.items.push(Item::Path(Path { layer, rings, closed: true, fill, stroke, element_segments: Vec::new() }));
        }
    }

    pub(crate) fn polyline(&mut self, layer: Layer, points: Vec<[f64; 2]>, stroke: Stroke) {
        if points.len() >= 2 {
            self.items.push(Item::Path(Path {
                layer,
                rings: vec![points],
                closed: false,
                fill: None,
                stroke: Some(stroke),
                element_segments: Vec::new(),
            }));
        }
    }

    pub(crate) fn line(&mut self, layer: Layer, a: [f64; 2], b: [f64; 2], stroke: Stroke) {
        self.polyline(layer, vec![a, b], stroke);
    }

    pub(crate) fn attribute_last(&mut self, sources: &[DrawingElementSegment]) {
        if let Some(Item::Path(path)) = self.items.last_mut() {
            path.attribute(sources);
        }
    }

    pub(crate) fn text(&mut self, t: Text) {
        if !t.text.is_empty() {
            self.items.push(Item::Text(t));
        }
    }

    /// Bounds of the paths' points (texts excluded), `[x0, y0, x1, y1]`.
    pub(crate) fn bounds(&self) -> Option<[f64; 4]> {
        let mut b: Option<[f64; 4]> = None;
        for item in &self.items {
            let Item::Path(p) = item else { continue };
            for q in p.rings.iter().flatten().filter(|q| q[0].is_finite() && q[1].is_finite()) {
                let r = b.get_or_insert([q[0], q[1], q[0], q[1]]);
                *r = [r[0].min(q[0]), r[1].min(q[1]), r[2].max(q[0]), r[3].max(q[1])];
            }
        }
        b
    }

    /// Keeps what lies inside `rect` (`[x0, y0, x1, y1]`): rings are clipped as areas, polylines as
    /// lines. Texts are kept as they are.
    pub(crate) fn clip(&mut self, rect: [f64; 4]) {
        let [x0, y0, x1, y1] = rect.map(|v| v.clamp(-1e8, 1e8));
        let clip = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        self.element_regions = self
            .element_regions
            .iter()
            .flat_map(|region| {
                cut::intersect_rings(&region.rings, &clip).into_iter().map(|rings| crate::views::DrawingElementRegion { rings, ..region.clone() })
            })
            .collect();
        let items = std::mem::take(&mut self.items);
        for item in items {
            match item {
                Item::Path(mut p) if p.closed => {
                    p.rings = p.rings.iter().map(|r| clip_ring(r, rect)).filter(|r| r.len() >= 3).collect();
                    if !p.rings.is_empty() {
                        self.items.push(Item::Path(p));
                    }
                }
                Item::Path(p) => {
                    let mut pieces: Vec<Vec<[f64; 2]>> = Vec::new();
                    for ring in &p.rings {
                        let mut current: Vec<[f64; 2]> = Vec::new();
                        for w in ring.windows(2) {
                            match clip_segment(w[0], w[1], rect) {
                                Some((a, b)) => {
                                    if current.last().is_some_and(|l| (l[0] - a[0]).hypot(l[1] - a[1]) < 1e-9) {
                                        current.push(b);
                                    } else {
                                        if current.len() >= 2 {
                                            pieces.push(std::mem::take(&mut current));
                                        }
                                        current = vec![a, b];
                                    }
                                }
                                None => {
                                    if current.len() >= 2 {
                                        pieces.push(std::mem::take(&mut current));
                                    }
                                    current.clear();
                                }
                            }
                        }
                        if current.len() >= 2 {
                            pieces.push(current);
                        }
                    }
                    if !pieces.is_empty() {
                        self.items.push(Item::Path(Path { rings: pieces, ..p }));
                    }
                }
                text => self.items.push(text),
            }
        }
    }
}

/// The part of segment `a b` inside `rect` (Liang–Barsky).
pub(crate) fn clip_segment(a: [f64; 2], b: [f64; 2], rect: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (p, q) in [(-d[0], a[0] - rect[0]), (d[0], rect[2] - a[0]), (-d[1], a[1] - rect[1]), (d[1], rect[3] - a[1])] {
        if p.abs() < 1e-15 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
        }
    }
    (t0 < t1).then(|| ([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
}

/// A polygon ring clipped to `rect` (Sutherland–Hodgman).
pub(crate) fn clip_ring(ring: &[[f64; 2]], rect: [f64; 4]) -> Vec<[f64; 2]> {
    let mut out = ring.to_vec();
    // (axis, bound, keep the side above the bound)
    for (axis, bound, above) in [(0, rect[0], true), (0, rect[2], false), (1, rect[1], true), (1, rect[3], false)] {
        let input = std::mem::take(&mut out);
        let n = input.len();
        let inside = |p: &[f64; 2]| if above { p[axis] >= bound } else { p[axis] <= bound };
        for i in 0..n {
            let (p, q) = (input[i], input[(i + 1) % n]);
            let (pi, qi) = (inside(&p), inside(&q));
            if pi {
                out.push(p);
            }
            if pi != qi {
                let t = (bound - p[axis]) / (q[axis] - p[axis]);
                out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
            }
        }
    }
    out
}

/// The smallest ISO 5455 scale (plus 1:25 and 1:250, common in Danish drawings) at which a
/// drawing that needs 1:`fit` still fits; beyond 1:10000 the fit itself.
pub(crate) fn standard_scale(fit: f64) -> f64 {
    const SCALES: [f64; 16] = [1.0, 2.0, 5.0, 10.0, 20.0, 25.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 2500.0, 5000.0, 10000.0];
    SCALES.iter().copied().find(|s| *s >= fit * (1.0 - 1e-9)).unwrap_or(fit)
}

/// The scale a sheet is drawn at: the standard scale that fits, when that keeps the drawing at
/// least 80 % of the size that would fill the sheet; otherwise the fitted scale itself.
pub(crate) fn sheet_scale(fit: f64) -> f64 {
    let s = standard_scale(fit);
    if s <= fit * 1.25 { s } else { fit }
}

/// "1:200", or "ca. 1:263" for a scale that isn't a standard one.
pub(crate) fn scale_text(scale: f64) -> String {
    if (standard_scale(scale) - scale).abs() < 1e-6 { format!("1:{}", scale.round()) } else { format!("ca. 1:{}", scale.round()) }
}

/// A number with `places` decimals and a decimal comma.
pub(crate) fn decimal(v: f64, places: usize) -> String {
    format!("{v:.places$}").replace('.', ",")
}

/// A level in metres from a height in mm: "+3,61", "−2,83", "+0,00".
pub(crate) fn level_text(mm: f64) -> String {
    let m = (mm / 10.0).round() / 100.0 + 0.0;
    if m < 0.0 { format!("−{}", decimal(-m, 2)) } else { format!("+{}", decimal(m, 2)) }
}

/// Maps drawing mm to sheet pixels: `x` to the right, `y` up on paper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Viewport {
    /// Drawing x at pixel 0.
    pub(crate) x0: f64,
    /// Drawing y at pixel 0 (the top).
    pub(crate) y1: f64,
    /// Drawing mm per pixel.
    pub(crate) mm: f64,
}

impl Viewport {
    pub(crate) fn px(&self, p: [f64; 2]) -> [f64; 2] {
        [(p[0] - self.x0) / self.mm, (self.y1 - p[1]) / self.mm]
    }

    pub(crate) fn at(&self, q: [f64; 2]) -> [f64; 2] {
        [self.x0 + q[0] * self.mm, self.y1 - q[1] * self.mm]
    }

    /// The scale this viewport prints at on the sheet.
    pub(crate) fn scale(&self) -> f64 {
        self.mm / MM_PER_PX
    }

    /// Centres `bounds` (drawing mm) in `area` (sheet px `[x, y, w, h]`) at [`sheet_scale`].
    pub(crate) fn fit(bounds: [f64; 4], area: [f64; 4]) -> Viewport {
        let (w, h) = ((bounds[2] - bounds[0]).max(1.0), (bounds[3] - bounds[1]).max(1.0));
        let fit = (w / area[2]).max(h / area[3]) / MM_PER_PX;
        let mm = sheet_scale(fit) * MM_PER_PX;
        let c = [(bounds[0] + bounds[2]) / 2.0, (bounds[1] + bounds[3]) / 2.0];
        Viewport { x0: c[0] - (area[0] + area[2] / 2.0) * mm, y1: c[1] + (area[1] + area[3] / 2.0) * mm, mm }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_round_up_to_a_standard_one() {
        assert_eq!(standard_scale(180.0), 200.0);
        assert_eq!(standard_scale(200.0), 200.0);
        assert_eq!(standard_scale(201.0), 250.0);
        assert_eq!(standard_scale(0.3), 1.0);
        assert_eq!(standard_scale(20_000.0), 20_000.0);
        assert_eq!(scale_text(200.0), "1:200");
        assert_eq!(scale_text(263.44), "ca. 1:263");
        assert_eq!(sheet_scale(180.0), 200.0);
        assert_eq!(sheet_scale(263.44), 263.44, "1:500 would halve the drawing");
    }

    #[test]
    fn levels_use_a_decimal_comma_and_a_sign() {
        assert_eq!(level_text(0.0), "+0,00");
        assert_eq!(level_text(3610.0), "+3,61");
        assert_eq!(level_text(-2830.0), "−2,83");
        assert_eq!(level_text(-3.0), "+0,00", "no negative zero");
        assert_eq!(thinner(0.35, 1), 0.25);
        assert_eq!(thinner(0.13, 3), 0.13);
    }

    #[test]
    fn clipping_keeps_what_is_inside() {
        let rect = [0.0, 0.0, 10.0, 10.0];
        assert_eq!(clip_segment([-5.0, 5.0], [15.0, 5.0], rect), Some(([0.0, 5.0], [10.0, 5.0])));
        assert_eq!(clip_segment([-5.0, -5.0], [-1.0, 20.0], rect), None);
        let ring = clip_ring(&[[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]], rect);
        let area: f64 = (0..ring.len()).map(|i| ring[i][0] * ring[(i + 1) % ring.len()][1] - ring[(i + 1) % ring.len()][0] * ring[i][1]).sum();
        assert!((area / 2.0 - 25.0).abs() < 1e-9);
        let mut s = Scene::new(100.0);
        s.polyline(Layer::Wall, vec![[-5.0, 1.0], [5.0, 1.0], [5.0, 20.0], [6.0, 5.0]], Stroke::new(0.25));
        s.clip(rect);
        let Item::Path(p) = &s.items[0] else { panic!() };
        assert_eq!(p.rings.len(), 2, "leaving and re-entering splits the line");
    }

    #[test]
    fn viewport_fits_at_a_standard_scale() {
        // 50 m wide in 1440 px: 1:164 fits, so 1:200.
        let vp = Viewport::fit([0.0, 0.0, 50_000.0, 10_000.0], [80.0, 190.0, 1440.0, 560.0]);
        assert!((vp.scale() - 200.0).abs() < 1e-9);
        let c = vp.px([25_000.0, 5_000.0]);
        assert!((c[0] - 800.0).abs() < 1e-6 && (c[1] - 470.0).abs() < 1e-6);
        let back = vp.at(c);
        assert!((back[0] - 25_000.0).abs() < 1e-6 && (back[1] - 5_000.0).abs() < 1e-6);
    }
}
