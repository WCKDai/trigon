//! Saved drawing views and drafting annotations. Positions are drawing-plane millimetres;
//! lettering sizes are paper millimetres, independent of the drawing's scale.

use serde::{Deserialize, Serialize};

use crate::{Document, ModelError, Result, checked};

pub const MAX_VIEWS: usize = 256;
pub const MAX_ANNOTATIONS: usize = 5000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DrawingView {
    pub id: String,
    pub name: String,
    pub kind: DrawingViewKind,
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
    pub kind: AnnotationKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "annotation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationKind {
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

impl DrawingView {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.id)?;
        text(&self.name, false)?;
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
                if doc.building(None)?.storeys.get(*level).is_none() {
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
}
