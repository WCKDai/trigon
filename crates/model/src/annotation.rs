//! Saved drawing views and drafting annotations. Positions are drawing-plane millimetres;
//! lettering sizes are paper millimetres, independent of the drawing's scale.

use serde::{Deserialize, Serialize};

use crate::{Document, ModelError, Result, checked};

pub const MAX_VIEWS: usize = 256;
pub const MAX_ANNOTATIONS: usize = 5000;
pub const MAX_REGION_POINTS: usize = 256;

/// A bundled, portable font family. Documents cannot request a local font path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontFamily {
    #[default]
    Sans,
    Serif,
    Mono,
}

impl FontFamily {
    pub fn name(self) -> &'static str {
        match self {
            Self::Sans => "Liberation Sans",
            Self::Serif => "Liberation Serif",
            Self::Mono => "Liberation Mono",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextStyle {
    #[serde(default)]
    pub family: FontFamily,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
}

impl TextStyle {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Pattern lengths are paper mm: dash 2, gap 1; dots have round caps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinePattern {
    #[default]
    Solid,
    Dashed,
    Dotted,
    DashDot,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineStyle {
    pub colour: [u8; 3],
    /// Paper mm. DXF rounds to its nearest supported lineweight.
    pub width_mm: f64,
    #[serde(default)]
    pub pattern: LinePattern,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self { colour: [26; 3], width_mm: 0.25, pattern: LinePattern::Solid }
    }
}

/// A portable drawing preset stored with a saved view and reusable via view.apply_template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawingTemplate {
    pub name: String,
    #[serde(default)]
    pub line_weights: LineWeightProfile,
    #[serde(default)]
    pub sizing: ViewSizing,
    /// Drawing-plane crop [x0,y0,x1,y1], in mm. Fit uses this rectangle; fixed scale clips to it.
    pub crop: Option<[f64; 4]>,
    /// Plan only, relative to the selected storey floor. None retains legacy plan symbols.
    pub range: Option<PlanRange>,
    /// Live labels from the selected floor's room definitions; hidden preserves old drawings.
    #[serde(default, skip_serializing_if = "RoomLabels::is_hidden")]
    pub room_labels: RoomLabels,
    /// Facade only. Hide generated balcony size labels and ticks; manual annotations remain.
    #[serde(default, skip_serializing_if = "is_false")]
    pub hide_balcony_dimensions: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RoomLabels {
    #[default]
    Hidden,
    Area,
    NameAndArea,
}

impl RoomLabels {
    fn is_hidden(&self) -> bool {
        *self == Self::Hidden
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineWeightProfile {
    Fine,
    #[default]
    Standard,
    Bold,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewSizing {
    #[default]
    Fit,
    /// Physical 1:denominator on the 338.6667 x 190.5 mm sheet, cropped around drawing centre.
    Fixed { denominator: f64, centre: [f64; 2] },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanRange {
    pub top_mm: f64,
    pub cut_mm: f64,
    pub bottom_mm: f64,
    pub depth_mm: f64,
}

impl Default for PlanRange {
    fn default() -> Self {
        Self { top_mm: 2300.0, cut_mm: 1200.0, bottom_mm: 0.0, depth_mm: -1000.0 }
    }
}
impl Default for DrawingTemplate {
    fn default() -> Self {
        Self {
            name: "Custom".into(),
            line_weights: Default::default(),
            sizing: Default::default(),
            crop: None,
            range: None,
            room_labels: Default::default(),
            hide_balcony_dimensions: false,
        }
    }
}
impl PlanRange {
    pub fn validate(&self) -> Result<()> {
        for value in [self.top_mm, self.cut_mm, self.bottom_mm, self.depth_mm] {
            checked("plan range relative to floor (mm)", value, -100_000.0, 100_000.0)?;
        }
        if !(self.depth_mm <= self.bottom_mm && self.bottom_mm < self.cut_mm && self.cut_mm < self.top_mm) {
            return Err(ModelError::Invalid("plan range requires depth <= bottom < cut < top (relative to floor)".into()));
        }
        Ok(())
    }
}
impl DrawingTemplate {
    pub fn validate(&self) -> Result<()> {
        text(&self.name, false)?;
        if self.name.len() > 80 {
            return Err(ModelError::Invalid("template name is limited to 80 bytes".into()));
        }
        if let ViewSizing::Fixed { denominator, centre } = self.sizing {
            checked("physical scale denominator", denominator, 1.0, 10000.0)?;
            point(centre)?;
        }
        if let Some([x0, y0, x1, y1]) = self.crop {
            point([x0, y0])?;
            point([x1, y1])?;
            checked("crop width (mm)", x1 - x0, 1.0, 1_000_000.0)?;
            checked("crop height (mm)", y1 - y0, 1.0, 1_000_000.0)?;
        }
        if let Some(range) = self.range {
            range.validate()?;
        }
        Ok(())
    }
}

fn default_region_border() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawingView {
    pub id: String,
    pub name: String,
    pub kind: DrawingViewKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<DrawingTemplate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "view", rename_all = "snake_case", deny_unknown_fields)]
pub enum DrawingViewKind {
    /// Site x/y, in mm. The level index follows the plan commands.
    Plan { level: usize },
    /// x along the facade from its start, y = height above terrain, in mm.
    Facade { facade: String },
    /// x along from → to, y = height above terrain, looking left of that line.
    Section {
        from: [f64; 2],
        to: [f64; 2],
        #[serde(default)]
        flip: bool,
        #[serde(default = "default_depth")]
        depth_mm: f64,
    },
}

fn default_depth() -> f64 {
    30_000.0
}

pub fn default_size() -> f64 {
    2.5
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    pub id: String,
    /// The id of a saved drawing view; annotations never leak into another view.
    pub view: String,
    /// Lettering height (em), paper mm, independent of the drawing's scale.
    #[serde(default = "default_size")]
    pub size_mm: f64,
    #[serde(default, skip_serializing_if = "TextStyle::is_default")]
    pub style: TextStyle,
    pub kind: AnnotationKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "annotation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationKind {
    /// Manual drafting geometry, never a model element or an apartment boundary assertion.
    Line {
        from: [f64; 2],
        to: [f64; 2],
        #[serde(default)]
        stroke: LineStyle,
    },
    Text {
        at: [f64; 2],
        text: String,
        #[serde(default)]
        rotation_deg: f64,
    },
    /// An aligned dimension. The signed offset is to the left of from → to.
    /// The label is always calculated from the measured points in millimetres.
    Dimension { from: [f64; 2], to: [f64; 2], offset_mm: f64 },
    /// An arrow at `tip`, a bend at `elbow` and a single-line tag at `at`.
    Leader { tip: [f64; 2], elbow: [f64; 2], at: [f64; 2], text: String },
    /// A vertical datum mark: the value shown is (at.y − datum_mm), in metres.
    Level {
        at: [f64; 2],
        #[serde(default)]
        datum_mm: f64,
        #[serde(default)]
        label: String,
    },
    /// An opaque area fill in drawing-plane mm, without a repeated closing point.
    Region {
        points: Vec<[f64; 2]>,
        fill: [u8; 3],
        #[serde(default = "default_region_border")]
        border: bool,
    },
}

pub fn identifier(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 80 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c)) {
        return Err(ModelError::Invalid("drawing ids need 1 to 80 ASCII letters, digits, dots, underscores or hyphens".into()));
    }
    Ok(())
}

fn text(value: &str, empty: bool) -> Result<()> {
    if value.len() > 512 || (!empty && value.trim().is_empty()) || value.chars().any(char::is_control) {
        return Err(ModelError::Invalid("drawing text needs one line of at most 512 bytes, without control characters".into()));
    }
    Ok(())
}

fn point(p: [f64; 2]) -> Result<()> {
    for v in p {
        checked("drawing coordinate (mm)", v, -1_000_000.0, 1_000_000.0)?;
    }
    Ok(())
}

fn region(points: &[[f64; 2]]) -> Result<()> {
    let invalid =
        || ModelError::Invalid("a filled region needs a simple nonzero ring of 3 to 256 distinct points, without a repeated closing point".into());
    if !(3..=MAX_REGION_POINTS).contains(&points.len()) {
        return Err(invalid());
    }
    for p in points {
        point(*p)?;
    }
    let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    let contains = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        cross(a, b, p).abs() <= 1e-7 && (0..2).all(|axis| p[axis] >= a[axis].min(b[axis]) - 1e-7 && p[axis] <= a[axis].max(b[axis]) + 1e-7)
    };
    let n = points.len();
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let previous = points[(i + n - 1) % n];
        // Adjacent edges may meet at their corner, but cannot double back over each other.
        if cross(previous, a, b).abs() <= 1e-7 && (a[0] - previous[0]) * (b[0] - a[0]) + (a[1] - previous[1]) * (b[1] - a[1]) < 0.0 {
            return Err(invalid());
        }
        for j in i + 1..n {
            if (points[j][0] - a[0]).hypot(points[j][1] - a[1]) <= 1e-6 {
                return Err(invalid());
            }
            if j == i + 1 || (i == 0 && j == n - 1) {
                continue;
            }
            let c = points[j];
            let d = points[(j + 1) % n];
            if (cross(a, b, c) * cross(a, b, d) < 0.0 && cross(c, d, a) * cross(c, d, b) < 0.0)
                || contains(a, b, c)
                || contains(a, b, d)
                || contains(c, d, a)
                || contains(c, d, b)
            {
                return Err(invalid());
            }
        }
    }
    let origin = points[0];
    let twice_area: f64 = (1..n - 1).map(|i| cross(origin, points[i], points[i + 1])).sum();
    if twice_area.abs() < 0.002 {
        return Err(invalid());
    }
    Ok(())
}

impl DrawingView {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        text(&self.name, false)?;
        if let Some(template) = &self.template {
            template.validate()?;
            if template.range.is_some() && !matches!(self.kind, DrawingViewKind::Plan { .. }) {
                return Err(ModelError::Invalid("floor-relative view ranges belong to plan views".into()));
            }
            if template.room_labels != RoomLabels::Hidden && !matches!(self.kind, DrawingViewKind::Plan { .. }) {
                return Err(ModelError::Invalid("room labels belong to plan views".into()));
            }
        }
        if self.name.len() > 200 {
            return Err(ModelError::Invalid("drawing view names are limited to 200 bytes".into()));
        }
        match &self.kind {
            DrawingViewKind::Plan { level } if *level >= 1000 => return Err(ModelError::Invalid("plan level must be below 1000".into())),
            DrawingViewKind::Facade { facade } => identifier(facade)?,
            DrawingViewKind::Section { from, to, depth_mm, .. } => {
                point(*from)?;
                point(*to)?;
                checked("section length (mm)", (to[0] - from[0]).hypot(to[1] - from[1]), 1000.0, 1_000_000.0)?;
                checked("depth_mm", *depth_mm, 100.0, 500_000.0)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Targets may become stale when a building is replaced; list them as invalid so they can
    /// be repaired. Loading a document only requires structural validity.
    pub fn validate_source(&self, doc: &Document) -> Result<()> {
        self.validate()?;
        match &self.kind {
            DrawingViewKind::Plan { level } => {
                if !doc.buildings.iter().any(|b| b.storeys.get(*level).is_some()) {
                    return Err(ModelError::NotFound(format!("plan level {level}")));
                }
            }
            DrawingViewKind::Facade { facade } => {
                if !doc.buildings.iter().any(|b| b.facades.iter().any(|f| f.id == *facade)) {
                    return Err(ModelError::NotFound(format!("facade {facade}")));
                }
            }
            DrawingViewKind::Section { .. } => {
                doc.building(None)?;
            }
        }
        Ok(())
    }
}

impl Annotation {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        identifier(&self.view)?;
        checked("size_mm (paper)", self.size_mm, 0.5, 12.0)?;
        match &self.kind {
            AnnotationKind::Line { from, to, stroke } => {
                point(*from)?;
                point(*to)?;
                checked("drafting line length (mm)", (to[0] - from[0]).hypot(to[1] - from[1]), 0.001, 1_000_000.0)?;
                checked("line width (paper mm)", stroke.width_mm, 0.05, 2.11)?;
            }
            AnnotationKind::Text { at, text: label, rotation_deg } => {
                point(*at)?;
                text(label, false)?;
                checked("rotation_deg", *rotation_deg, -360.0, 360.0)?;
            }
            AnnotationKind::Dimension { from, to, offset_mm } => {
                point(*from)?;
                point(*to)?;
                checked("dimension length (mm)", (to[0] - from[0]).hypot(to[1] - from[1]), 0.001, 1_000_000.0)?;
                checked("offset_mm", *offset_mm, -100_000.0, 100_000.0)?;
            }
            AnnotationKind::Leader { tip, elbow, at, text: label } => {
                for p in [tip, elbow, at] {
                    point(*p)?;
                }
                checked("leader first segment (mm)", (elbow[0] - tip[0]).hypot(elbow[1] - tip[1]), 0.001, 1_000_000.0)?;
                text(label, false)?;
            }
            AnnotationKind::Level { at, datum_mm, label } => {
                point(*at)?;
                checked("datum_mm", *datum_mm, -1_000_000.0, 1_000_000.0)?;
                text(label, true)?;
            }
            AnnotationKind::Region { points, .. } => region(points)?,
        }
        Ok(())
    }
}

/// Structural persistence checks, also used when opening an untrusted document. A stale source
/// view remains repairable; duplicate ids and annotations without a view are refused.
pub fn validate_document(doc: &Document) -> Result<()> {
    use std::collections::BTreeSet;
    if doc.views.len() > MAX_VIEWS || doc.annotations.len() > MAX_ANNOTATIONS {
        return Err(ModelError::Invalid(format!("at most {MAX_VIEWS} saved views and {MAX_ANNOTATIONS} annotations are allowed")));
    }
    let mut views = BTreeSet::new();
    for v in &doc.views {
        v.validate()?;
        if !views.insert(&v.id) {
            return Err(ModelError::Invalid(format!("duplicate drawing view {}", v.id)));
        }
    }
    let mut ids = BTreeSet::new();
    for a in &doc.annotations {
        a.validate()?;
        if !ids.insert(&a.id) {
            return Err(ModelError::Invalid(format!("duplicate annotation {}", a.id)));
        }
        if !views.contains(&a.view) {
            return Err(ModelError::NotFound(format!("annotation {} refers to drawing view {}", a.id, a.view)));
        }
        if matches!(a.kind, AnnotationKind::Level { .. })
            && doc.views.iter().any(|v| v.id == a.view && matches!(v.kind, DrawingViewKind::Plan { .. }))
        {
            return Err(ModelError::Invalid("vertical level marks belong to facade or section views".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_documents_keep_their_shape_and_new_drafting_fields_round_trip() {
        let old = serde_json::to_value(Document::default()).unwrap();
        assert!(old.get("views").is_none());
        assert!(old.get("annotations").is_none());
        let loaded: Document = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(serde_json::to_value(loaded).unwrap(), old);
        let mut payload = old;
        payload["views"] = json!([{"id":"section-a", "name":"A–A", "kind":{"view":"section", "from":[0,0], "to":[10000,0]}}]);
        payload["annotations"] = json!([{"id":"level-a", "view":"section-a", "kind":{"annotation":"level", "at":[2000,3200]}}]);
        let doc: Document = serde_json::from_value(payload).unwrap();
        validate_document(&doc).unwrap();
        let roundtrip: Document = serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        assert_eq!(roundtrip, doc);
        let mut duplicate = doc.clone();
        duplicate.annotations.push(doc.annotations[0].clone());
        assert!(validate_document(&duplicate).is_err());
        let mut orphan = doc;
        orphan.views.clear();
        assert!(validate_document(&orphan).is_err());
    }

    #[test]
    fn annotation_numbers_text_and_variants_are_checked() {
        let base = Annotation {
            id: "a".into(),
            view: "v".into(),
            size_mm: 2.5,
            style: TextStyle::default(),
            kind: AnnotationKind::Text { at: [0.0, 0.0], text: "Æøå & <masonry>".into(), rotation_deg: 0.0 },
        };
        base.validate().unwrap();
        for value in [f64::NAN, f64::INFINITY, f64::MAX, -1.0, 0.0] {
            let mut a = base.clone();
            a.size_mm = value;
            assert!(a.validate().is_err());
        }
        for label in ["", "  ", "one\ntwo", "\u{0}", &"x".repeat(513)] {
            let mut a = base.clone();
            a.kind = AnnotationKind::Text { at: [0.0, 0.0], text: label.into(), rotation_deg: 0.0 };
            assert!(a.validate().is_err());
        }
        for v in [f64::NAN, f64::INFINITY, f64::MAX] {
            let mut a = base.clone();
            a.kind = AnnotationKind::Dimension { from: [0.0, 0.0], to: [v, 100.0], offset_mm: 10.0 };
            assert!(a.validate().is_err());
        }
        assert!(
            serde_json::from_value::<Annotation>(json!({"id":"a", "view":"v", "kind":{"annotation":"text", "at":[0,0], "text":"x", "ignored":1}}))
                .is_err()
        );
    }

    #[test]
    fn legacy_annotations_omit_default_style_and_selected_fonts_round_trip() {
        let old = json!({"id":"note","view":"plan","size_mm":2.5,"kind":{"annotation":"text","at":[50.0,100.0],"text":"Køkken","rotation_deg":0.0}});
        let annotation: Annotation = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(annotation.style, TextStyle::default());
        assert_eq!(serde_json::to_value(&annotation).unwrap(), old);
        for family in [FontFamily::Sans, FontFamily::Serif, FontFamily::Mono] {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                let mut annotation = annotation.clone();
                annotation.style = TextStyle { family, bold, italic };
                annotation.validate().unwrap();
                let serialized = serde_json::to_value(&annotation).unwrap();
                assert_eq!(serialized.get("style").is_none(), annotation.style.is_default());
                assert_eq!(serde_json::from_value::<Annotation>(serialized).unwrap(), annotation);
            }
        }
        for style in [json!({"family":"/local/private/font.ttf"}), json!({"bold":"yes"}), json!({"path":"font.ttf"})] {
            let mut payload = old.clone();
            payload["style"] = style;
            assert!(serde_json::from_value::<Annotation>(payload).is_err());
        }
    }

    #[test]
    fn filled_regions_accept_simple_concave_rings_and_reject_hostile_or_crossing_geometry() {
        let payload = |points| json!({"id":"zone","view":"plan","kind":{"annotation":"region","points":points,"fill":[220,230,240]}});
        for points in [
            vec![[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0]],
            vec![[0.0, 1000.0], [1000.0, 1000.0], [1000.0, 0.0], [0.0, 0.0]],
            vec![[0.0, 0.0], [1000.0, 0.0], [1000.0, 400.0], [400.0, 400.0], [400.0, 1000.0], [0.0, 1000.0]],
            vec![[999998.0, 999998.0], [999999.0, 999998.0], [999999.0, 999999.0], [999998.0, 999999.0]],
        ] {
            let annotation: Annotation = serde_json::from_value(payload(points)).unwrap();
            annotation.validate().unwrap();
            assert!(matches!(annotation.kind, AnnotationKind::Region { border: true, .. }));
            assert_eq!(serde_json::from_str::<Annotation>(&serde_json::to_string(&annotation).unwrap()).unwrap(), annotation);
        }
        for points in [
            vec![],
            vec![[0.0, 0.0], [1000.0, 0.0]],
            vec![[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [0.0, 0.0]],
            vec![[0.0, 0.0], [1000.0, 1000.0], [0.0, 1000.0], [1000.0, 0.0]],
            vec![[0.0, 0.0], [1000.0, 0.0], [1000.0, 1000.0], [500.0, 0.0], [0.0, 1000.0]],
            vec![[0.0, 0.0], [1000.0, 0.0], [500.0, 0.0], [500.0, 1000.0], [0.0, 1000.0]],
            vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]],
            vec![[0.0, 0.0], [0.001, 0.0], [0.0, 0.001]],
            vec![[0.0, 0.0], [1e8, 0.0], [0.0, 1000.0]],
            vec![[0.0, 0.0]; MAX_REGION_POINTS + 1],
        ] {
            let annotation: Annotation = serde_json::from_value(payload(points)).unwrap();
            assert!(annotation.validate().is_err());
        }
        let mut annotation: Annotation = serde_json::from_value(payload(vec![[0.0, 0.0], [1000.0, 0.0], [0.0, 1000.0]])).unwrap();
        if let AnnotationKind::Region { points, .. } = &mut annotation.kind {
            points[1][0] = f64::NAN;
        }
        assert!(annotation.validate().is_err());
        let mut invalid_colour = payload(vec![[0.0, 0.0], [1000.0, 0.0], [0.0, 1000.0]]);
        invalid_colour["kind"]["fill"] = json!([256, 0, 0]);
        assert!(serde_json::from_value::<Annotation>(invalid_colour).is_err());
    }
}
