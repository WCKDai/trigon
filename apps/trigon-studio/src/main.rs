//! Trigon Studio: the desktop app where the owner and an AI agent build a model together. The
//! agent drives the same session over the control channel (`trigon mcp` bridges to it), and the
//! owner watches every command land: in the activity feed, and highlighted in the model.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod activity;
mod app;
mod authoring;
mod camera;
mod control;
mod drafting;
mod palette;
mod plan_view;
mod render;
mod scene;
mod schedules;
mod theme;
mod worker;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use trigon_control::auth::{self, Listen};

const USAGE: &str = "\
trigon-studio — co-build a building model with an AI agent

USAGE
  trigon-studio [FILE.trigon] [--control PORT | --no-control]

The control channel listens on 127.0.0.1 only (default port 27900, or the next free one), with a
per-launch token in a file only you can read. Point an MCP client at `trigon mcp` and its agent
works in this window. TRIGON_CONTROL_PORT sets the port too.";

fn parse(args: &[String]) -> Result<app::Launch, String> {
    let mut file = None;
    let mut control = Some(match std::env::var(auth::PORT_ENV).ok().map(|p| p.trim().parse::<u16>()) {
        Some(Ok(p)) if p != 0 => Listen::Exact(p),
        Some(_) => return Err(format!("{} must be a port number", auth::PORT_ENV)),
        None => Listen::Default,
    });
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--control" => {
                let p = it.next().and_then(|p| p.parse::<u16>().ok()).filter(|p| *p != 0).ok_or("--control needs a port number")?;
                control = Some(Listen::Exact(p));
            }
            "--no-control" => control = None,
            "-h" | "--help" => return Err(USAGE.into()),
            f if !f.starts_with('-') && file.is_none() => file = Some(PathBuf::from(f)),
            other => return Err(format!("unknown argument {other}\n\n{USAGE}")),
        }
    }
    if let Some(f) = &file
        && f.extension().and_then(|e| e.to_str()) != Some("trigon")
    {
        return Err(format!("{}: Trigon Studio opens .trigon files", f.display()));
    }
    Ok(app::Launch { file, control })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match parse(&args) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            return if args.iter().any(|a| a == "-h" || a == "--help") { ExitCode::SUCCESS } else { ExitCode::FAILURE };
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("Trigon Studio").with_inner_size([1440.0, 900.0]).with_min_inner_size([820.0, 520.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    match eframe::run_native("Trigon Studio", options, Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, launch))))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("trigon-studio: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_are_checked() {
        let l = parse(&args(&["model.trigon", "--control", "28000"])).unwrap();
        assert_eq!(l.file, Some(PathBuf::from("model.trigon")));
        assert_eq!(l.control, Some(Listen::Exact(28000)));
        assert!(parse(&args(&["--no-control"])).unwrap().control.is_none());
        assert!(parse(&args(&["--control"])).is_err());
        assert!(parse(&args(&["--control", "0"])).is_err());
        assert!(parse(&args(&["--control", "99999"])).is_err());
        assert!(parse(&args(&["model.ifc"])).is_err());
        assert!(parse(&args(&["a.trigon", "b.trigon"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
    }
}
