//! Optional external FBX conversion. Fixed script and argument arrays, private scratch space,
//! restricted child environment, timeout, verified round-trip and atomic final file replacement.

use crate::{EngineError, Result, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Export {
    path: String,
    phase: Option<String>,
    filter: Option<String>,
    cut_level: Option<usize>,
    cut_mm: Option<f64>,
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp =
            SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| EngineError::Io("cannot initialise FBX scratch space".into()))?.as_nanos();
        let base = std::path::absolute(std::env::temp_dir()).map_err(io)?;
        for _ in 0..64 {
            let path = base.join(format!("trigon-fbx-{}-{stamp}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(io(e)),
            }
        }
        Err(EngineError::Io("cannot allocate private FBX scratch space".into()))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn io(e: std::io::Error) -> EngineError {
    EngineError::Io(format!("FBX file operation: {e}"))
}

fn executable() -> OsString {
    if let Some(path) = std::env::var_os("TRIGON_BLENDER") {
        return path;
    }
    let mac = Path::new("/Applications/Blender.app/Contents/MacOS/Blender");
    if cfg!(target_os = "macos") && mac.is_file() { mac.as_os_str().to_owned() } else { OsString::from("blender") }
}

fn process(executable: &OsStr, scratch: &Path) -> Command {
    let mut c = Command::new(executable);
    c.env_clear();
    for key in ["PATH", "HOME", "SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
        if let Some(v) = std::env::var_os(key) {
            c.env(key, v);
        }
    }
    for key in ["BLENDER_USER_CONFIG", "BLENDER_USER_SCRIPTS", "BLENDER_USER_EXTENSIONS"] {
        c.env(key, scratch.join("blender-user"));
    }
    c.current_dir(scratch)
        .args(["--background", "--factory-startup", "--disable-autoexec", "--threads", "3", "--python-exit-code", "1", "--python"])
        .arg(scratch.join("convert.py"))
        .args(["--", "--input"])
        .arg(scratch.join("model.glb"))
        .arg("--output")
        .arg(scratch.join("model.fbx"))
        .arg("--report")
        .arg(scratch.join("report.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    c
}

fn convert(scratch: &Path, executable: &OsStr) -> Result<(Vec<u8>, Value)> {
    std::fs::write(scratch.join("convert.py"), include_str!("fbx_convert.py")).map_err(io)?;
    let mut child = process(executable, scratch).spawn().map_err(|_| {
        EngineError::Io("FBX needs an installed Blender with FBX support; set TRIGON_BLENDER to its executable if it is not on PATH".into())
    })?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(EngineError::Io("Blender FBX conversion or round-trip verification failed; the destination was preserved".into()));
                }
                break;
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io(e));
            }
            Ok(None) if start.elapsed() > Duration::from_secs(120) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(EngineError::Io("Blender FBX conversion exceeded 120 seconds; the destination was preserved".into()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let path = scratch.join("model.fbx");
    if std::fs::metadata(&path).map_err(io)?.len() > 1 << 30 {
        return Err(EngineError::Io("FBX output exceeds 1 GiB".into()));
    }
    let bytes = std::fs::read(&path).map_err(io)?;
    if !bytes.starts_with(b"Kaydara FBX Binary  \0\x1a\0") {
        return Err(EngineError::Io("Blender did not produce binary FBX".into()));
    }
    let report_path = scratch.join("report.json");
    if std::fs::metadata(&report_path).map_err(io)?.len() > 1 << 20 {
        return Err(EngineError::Io("FBX verification report exceeds 1 MiB".into()));
    }
    let report = std::fs::read(report_path).map_err(io)?;
    let report: Value = serde_json::from_slice(&report).map_err(|_| EngineError::Io("invalid FBX verification report".into()))?;
    if report["source_ids_preserved"] != true
        || !report["max_vertex_deviation_mm"].as_f64().is_some_and(|v| v.is_finite() && (0.0..=0.1).contains(&v))
    {
        return Err(EngineError::Io("FBX round-trip was not verified".into()));
    }
    Ok((bytes, report))
}

pub(crate) fn export(s: &mut Session, p: Export) -> Result<Value> {
    export_using(s, p, &executable())
}

fn export_using(s: &mut Session, p: Export, executable: &OsStr) -> Result<Value> {
    let output = PathBuf::from(&p.path);
    if output.extension().and_then(OsStr::to_str) != Some("fbx") || p.path.contains('\0') || p.path.contains("://") || p.path.len() > 4096 {
        return Err(EngineError::Io("FBX output needs a local file path ending in .fbx".into()));
    }
    let scratch = Scratch::new()?;
    let mut params = serde_json::to_value(&p).map_err(|_| EngineError::Io("invalid FBX parameters".into()))?;
    params["path"] = json!(scratch.0.join("model.glb"));
    s.execute("export.gltf", params)?;
    let (bytes, report) = convert(&scratch.0, executable)?;
    crate::plan::write_output(&output, &bytes)?;
    Ok(json!({"written":output,"bytes":bytes.len(),"format":"fbx","verification":report}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converter_uses_fixed_arguments_and_does_not_forward_credentials_or_python_hooks() {
        let folder = Path::new("/tmp/space ; $() test");
        let command = process(OsStr::new("blender"), folder);
        let args: Vec<_> = command.get_args().collect();
        assert!(args.contains(&OsStr::new("--disable-autoexec")));
        assert!(args.contains(&folder.join("model.glb").as_os_str()));
        let keys: Vec<_> = command.get_envs().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        assert!(!keys.iter().any(|k| k.contains("TOKEN") || k.contains("KEY") || k.starts_with("PYTHON")));
    }
    #[cfg(unix)]
    #[test]
    fn inherited_test_credentials_and_python_hooks_are_absent_in_the_actual_child() {
        if std::env::var_os("TRIGON_FBX_ENV_PROBE").is_none() {
            let result = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "fbx::tests::inherited_test_credentials_and_python_hooks_are_absent_in_the_actual_child"])
                .env("TRIGON_FBX_ENV_PROBE", "1")
                .env("TRIGON_FBX_TEST_CREDENTIAL", "not-a-real-key")
                .env("PYTHONSTARTUP", "untrusted-startup-marker")
                .output()
                .unwrap();
            assert!(result.status.success(), "child environment probe failed");
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new().unwrap();
        let helper = scratch.0.join("environment-probe");
        std::fs::write(&helper, b"#!/bin/sh\n[ -z \"${TRIGON_FBX_TEST_CREDENTIAL+x}\" ] && [ -z \"${PYTHONSTARTUP+x}\" ]\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(process(helper.as_os_str(), &scratch.0).status().unwrap().success());
    }
    #[test]
    fn failed_converter_cleans_only_its_private_scratch_and_does_not_touch_destination() {
        let scratch = Scratch::new().unwrap();
        let path = scratch.0.clone();
        std::fs::write(path.join("model.glb"), b"invalid fixture").unwrap();
        assert!(convert(&path, path.join("missing-blender").as_os_str()).is_err());
        assert!(path.join("model.glb").exists());
        drop(scratch);
        assert!(!path.exists());
    }
    #[test]
    fn conversion_failure_preserves_the_existing_export_and_document() {
        let directory = Scratch::new().unwrap();
        let output = directory.0.join("existing.fbx");
        std::fs::write(&output, b"previous export").unwrap();
        let mut s = Session::new(Box::new(trigon_data::Live::new(trigon_data::Credentials::default())));
        s.doc = trigon_io::ifc::sample::document().unwrap();
        let before = s.doc.clone();
        let params = Export { path: output.to_string_lossy().into_owned(), phase: None, filter: None, cut_level: None, cut_mm: None };
        assert!(export_using(&mut s, params, directory.0.join("missing-blender").as_os_str()).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous export");
        assert_eq!(s.doc, before);
        assert!(!s.dirty);
    }
}
