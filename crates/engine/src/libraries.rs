//! Portable libraries contain parametric type definitions, never opaque meshes or external assets.

use std::collections::BTreeSet;
use std::io::Read;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trigon_model::{ComponentType, ModelError};

use crate::{EngineError, Result, Session};

const FORMAT: &str = "trigon/library/1";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TYPES: usize = 1024;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Export {
    pub path: String,
    pub ids: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Import {
    pub path: String,
    #[serde(default)]
    pub replace: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Library {
    format: String,
    types: Vec<ComponentType>,
}

fn invalid(message: &str) -> EngineError {
    ModelError::Invalid(message.into()).into()
}

fn path(value: &str) -> Result<std::path::PathBuf> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(invalid("library path must be at most 4096 bytes without control characters"));
    }
    crate::commands::checked_path(value, "trigonlib")
}

impl Library {
    fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(invalid("unsupported component library format; expected trigon/library/1"));
        }
        if self.types.is_empty() || self.types.len() > MAX_TYPES {
            return Err(invalid("a component library needs 1 to 1024 types"));
        }
        let mut ids = BTreeSet::new();
        for ty in &self.types {
            ty.validate()?;
            if !ids.insert(&ty.id) {
                return Err(invalid("a component library must not contain duplicate type IDs"));
            }
        }
        Ok(())
    }
}

pub(crate) fn export(session: &Session, params: Export) -> Result<Value> {
    let path = path(&params.path)?;
    if let Some(ids) = &params.ids {
        if ids.is_empty() || ids.len() > MAX_TYPES || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(invalid("select 1 to 1024 unique component type IDs, or omit ids for all types"));
        }
        if ids.iter().any(|id| !session.doc.types.iter().any(|ty| &ty.id == id)) {
            return Err(invalid("a selected component type does not exist"));
        }
    }
    let library = Library {
        format: FORMAT.into(),
        types: session.doc.types.iter().filter(|ty| params.ids.as_ref().is_none_or(|ids| ids.contains(&ty.id))).cloned().collect(),
    };
    library.validate()?;
    let mut bytes = serde_json::to_vec_pretty(&library).map_err(|_| invalid("component library could not be encoded"))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("component library exceeds 4 MiB; export a smaller selection"));
    }
    crate::commands::write_file(&path, &bytes)?;
    Ok(json!({"path":path,"format":FORMAT,"exported":library.types.iter().map(|ty|&ty.id).collect::<Vec<_>>()}))
}

pub(crate) fn import(session: &mut Session, params: Import) -> Result<Value> {
    let path = path(&params.path)?;
    let file = std::fs::File::open(&path).map_err(|e| EngineError::Io(format!("component library: {e}")))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| EngineError::Io(format!("component library: {e}")))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("component library exceeds 4 MiB"));
    }
    // Do not echo arbitrary file contents or unknown enum/field values in diagnostics.
    let library: Library = serde_json::from_slice(&bytes)
        .map_err(|e| EngineError::Io(format!("invalid component library JSON at line {}, column {}", e.line(), e.column())))?;
    library.validate()?;
    let mut candidate = session.doc.clone();
    let mut replaced = Vec::new();
    for ty in &library.types {
        match candidate.types.iter_mut().find(|old| old.id == ty.id) {
            Some(_) if !params.replace => return Err(invalid("component IDs already exist; use replace to update their linked instances")),
            Some(old) => {
                *old = ty.clone();
                replaced.push(ty.id.clone());
            }
            None => candidate.types.push(ty.clone()),
        }
    }
    // Source dimensions and material choices are checked in their real hosted occurrences before
    // assigning anything. One import is one outer command and one history entry.
    trigon_model::build(&candidate)?;
    let ids: Vec<_> = library.types.iter().map(|ty| ty.id.clone()).collect();
    session.doc = candidate;
    Ok(json!({"format":FORMAT,"imported":ids,"replaced":replaced}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use trigon_model::build::ElementClass;
    use trigon_model::mesh::Material;

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            Self(std::env::temp_dir().join(format!("trigon-library-{}-{nonce}.trigonlib", std::process::id())))
        }
        fn text(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn session() -> Session {
        Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())))
    }
    fn write(file: &Temp, types: Vec<ComponentType>) {
        std::fs::write(&file.0, serde_json::to_vec(&Library { format: FORMAT.into(), types }).unwrap()).unwrap();
    }

    #[test]
    fn selected_library_round_trips_exact_types_without_document_or_history_side_effects() {
        let mut source = session();
        let before = source.doc.clone();
        let file = Temp::new();
        let ids = ["balcony.french", "balcony.minaltan"];
        source.execute("types.export", json!({"path":file.text(),"ids":ids})).unwrap();
        assert_eq!(source.doc, before);
        assert!(!source.dirty && source.undo.is_empty());
        let first_bytes = std::fs::read(&file.0).unwrap();
        let mut destination = session();
        destination.doc.types.clear();
        let initial = destination.doc.clone();
        destination.execute("types.import", json!({"path":file.text()})).unwrap();
        assert_eq!(destination.doc.types, before.types.iter().filter(|ty| ids.contains(&ty.id.as_str())).cloned().collect::<Vec<_>>());
        assert_eq!(destination.doc.journal.len(), initial.journal.len() + 1);
        destination.execute("types.export", json!({"path":file.text()})).unwrap();
        assert_eq!(std::fs::read(&file.0).unwrap(), first_bytes);
        let reopened: trigon_model::Document = serde_json::from_value(serde_json::to_value(&destination.doc).unwrap()).unwrap();
        assert_eq!(reopened.types, destination.doc.types);
        destination.execute("edit.undo", json!({})).unwrap();
        assert_eq!(destination.doc, initial);
    }

    #[test]
    fn replacing_a_library_updates_linked_dimensions_and_finish_but_keeps_instance_overrides() {
        let mut s = session();
        s.doc.buildings.push(
            trigon_model::Building::new("b", "Library fixture", &[[0.0, 0.0], [14000.0, 0.0], [14000.0, 8000.0], [0.0, 8000.0]], 2, 3000.0).unwrap(),
        );
        s.execute("facade.set_bays", json!({"facade":"b.f1","count":2})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"balcony.minaltan","levels":[1],"bays":[0]})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"balcony.minaltan","levels":[1],"bays":[1],"width":2300})).unwrap();
        let before = s.doc.clone();
        let initial = trigon_model::build(&s.doc).unwrap();
        let file = Temp::new();
        let mut ty = s.doc.component("balcony.minaltan").unwrap().clone();
        ty.width = 3200.0;
        let trigon_model::ComponentKind::SteelBalcony { finish, .. } = &mut ty.kind else { panic!("steel balcony fixture") };
        *finish = trigon_model::component::Finish::White;
        write(&file, vec![ty]);
        assert!(s.execute("types.import", json!({"path":file.text()})).is_err());
        assert_eq!(s.doc, before);
        s.execute("types.import", json!({"path":file.text(),"replace":true})).unwrap();
        let built = trigon_model::build(&s.doc).unwrap();
        let rails: Vec<_> = built[0].walls[0].attachments.iter().filter(|e| e.class == ElementClass::Railing).collect();
        assert_eq!(rails.len(), 2);
        assert_eq!([rails[0].width, rails[1].width], [3200.0, 2300.0]);
        assert!(rails.iter().all(|e| e.parts.iter().any(|(material, _)| *material == Material::SteelWhite)));
        assert_ne!(built, initial);
        assert_eq!(s.doc.journal.len(), before.journal.len() + 1);
        s.execute("edit.undo", json!({})).unwrap();
        assert_eq!(s.doc, before);
        assert_eq!(trigon_model::build(&s.doc).unwrap(), initial);
    }

    #[test]
    fn invalid_library_batches_and_host_geometry_never_partially_change_types() {
        let mut s = session();
        let file = Temp::new();
        let before = s.doc.clone();
        let one = s.doc.types[0].clone();
        let mut bad = one.clone();
        bad.id = "invalid".into();
        bad.width = f64::MAX;
        write(&file, vec![one.clone(), bad]);
        assert!(s.execute("types.import", json!({"path":file.text(),"replace":true})).is_err());
        assert_eq!(s.doc, before);
        write(&file, vec![one.clone(), one]);
        assert!(s.execute("types.import", json!({"path":file.text(),"replace":true})).is_err());
        assert_eq!(s.doc, before);
        for text in [
            r#"{"format":"trigon/library/2","types":[]}"#,
            r#"{"format":"trigon/library/1","types":[],"assets":["https://example.test/model.glb"]}"#,
            "not json",
        ] {
            std::fs::write(&file.0, text).unwrap();
            assert!(s.execute("types.import", json!({"path":file.text()})).is_err());
            assert_eq!(s.doc, before);
        }
        std::fs::write(&file.0, vec![b' '; MAX_BYTES as usize + 1]).unwrap();
        assert!(s.execute("types.import", json!({"path":file.text()})).is_err());
        assert_eq!(s.doc, before);
        assert!(s.undo.is_empty() && !s.dirty);
    }

    #[test]
    fn import_rechecks_real_hosts_and_export_refuses_missing_ids_or_unrelated_paths() {
        let mut s = session();
        s.doc
            .buildings
            .push(trigon_model::Building::new("b", "Host", &[[0.0, 0.0], [5000.0, 0.0], [5000.0, 5000.0], [0.0, 5000.0]], 1, 3000.0).unwrap());
        s.execute("facade.set_bays", json!({"facade":"b.f1","count":2})).unwrap();
        s.execute("facade.place", json!({"facade":"b.f1","type":"window.fixed","levels":[0]})).unwrap();
        let before = s.doc.clone();
        let file = Temp::new();
        let mut ty = s.doc.component("window.fixed").unwrap().clone();
        ty.width = 6000.0;
        write(&file, vec![ty]);
        assert!(s.execute("types.import", json!({"path":file.text(),"replace":true})).is_err());
        assert_eq!(s.doc, before);
        for params in [
            json!({"path":file.text(),"ids":["missing"]}),
            json!({"path":file.text(),"ids":[]}),
            json!({"path":file.text(),"ids":["window.fixed","window.fixed"]}),
            json!({"path":"do-not-overwrite.txt"}),
        ] {
            assert!(s.execute("types.export", params).is_err());
            assert_eq!(s.doc, before);
        }
    }
}
