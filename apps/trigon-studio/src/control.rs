//! Control requests in the window (docs/control-protocol.md): an agent's commands run through
//! the app's own `run`, so they land in the feed and the viewport like the owner's; `ui.view`
//! changes what the owner sees, `ui.screenshot` shows the agent the same pixels.

use std::time::Instant;

use eframe::egui;
use serde_json::{Value, json};
use trigon_control::{Request, methods};

use crate::activity::Who;
use crate::app::{Mode, SHOT_WAIT, Shot, ShotTarget, StudioApp, filter_name, parse_filter};

/// Widest window screenshot sent back; wider ones are halved (Retina windows), so replies stay
/// well inside the channel's line limit.
const MAX_SHOT_WIDTH: usize = 2400;

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            out.push(if i <= c.len() { char::from(T[((n >> shift) & 63) as usize]) } else { '=' });
        }
    }
    out
}

/// Halve an RGBA image (2×2 box filter).
pub fn halve(size: [usize; 2], rgba: &[u8]) -> ([usize; 2], Vec<u8>) {
    let (w, h) = (size[0] / 2, size[1] / 2);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            for c in 0..4 {
                let at = |xx: usize, yy: usize| u32::from(rgba.get((yy * size[0] + xx) * 4 + c).copied().unwrap_or(0));
                out.push(((at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1)) / 4) as u8);
            }
        }
    }
    ([w, h], out)
}

/// The reply to a screenshot: the PNG written to `path`, or inline as base64.
fn shot_reply(size: [u32; 2], png: Vec<u8>, path: Option<&str>, target: &str) -> Result<Value, String> {
    match path {
        Some(p) => {
            if !p.ends_with(".png") {
                return Err("ui.screenshot: `path` must end in .png".into());
            }
            std::fs::write(p, &png).map_err(|e| format!("ui.screenshot: {p}: {e}"))?;
            Ok(json!({"path": p, "width": size[0], "height": size[1], "target": target}))
        }
        None => Ok(json!({"width": size[0], "height": size[1], "target": target, "png_base64": base64(&png)})),
    }
}

impl StudioApp {
    /// The name the owner sees for the agent on `connection`.
    fn agent_name(&self, connection: u64) -> String {
        self.presence.as_ref().and_then(|p| p.clients().get(&connection).cloned()).filter(|n| n != "trigon-mcp").unwrap_or_else(|| "agent".into())
    }

    pub(crate) fn handle_request(&mut self, req: Request, ctx: &egui::Context) {
        let (connection, method, params, responder) = req.into_parts();
        let reply = match method.as_str() {
            "command.run" => methods::command_run_params(&params).and_then(|(id, p)| {
                let who = Who::Agent(self.agent_name(connection));
                self.run(who, &id, p)
            }),
            "ui.view" => self.ui_view(&params),
            "ui.screenshot" => {
                let target = match params.get("target").and_then(Value::as_str).unwrap_or("viewport") {
                    "viewport" => ShotTarget::Viewport,
                    "window" => ShotTarget::Window,
                    other => {
                        responder.send(Err(format!("ui.screenshot: target {other}: use viewport or window")));
                        return;
                    }
                };
                let path = params.get("path").and_then(Value::as_str).map(str::to_string);
                let raise = params.get("raise").and_then(Value::as_bool).unwrap_or(false);
                // Answered once the view shows the latest change (serve_shots).
                self.shots.push(Shot { responder, target, path, deadline: Instant::now() + SHOT_WAIT, requested: false, raise });
                ctx.request_repaint();
                self.serve_shots(ctx);
                return;
            }
            m => methods::session_method(&mut self.session, m, &params).unwrap_or_else(|| Err(methods::unknown_method(m))),
        };
        responder.send(reply);
        ctx.request_repaint();
    }

    /// `ui.view {mode?, level?, phase?, filter?, fit?}`: checked whole, then applied.
    fn ui_view(&mut self, p: &Value) -> Result<Value, String> {
        if !p.is_object() && !p.is_null() {
            return Err("ui.view: params must be an object".into());
        }
        let mode = match p.get("mode").and_then(Value::as_str) {
            None => self.mode,
            Some("3d") => Mode::ThreeD,
            Some("plan") => Mode::Plan,
            Some(other) => return Err(format!("ui.view: mode {other}: use 3d or plan")),
        };
        let levels = self.session.doc.buildings.first().map_or(0, |b| b.storeys.len());
        let level = match p.get("level") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(v) => match v.as_u64().and_then(|l| usize::try_from(l).ok()).filter(|l| *l < levels) {
                Some(l) => Some(Some(l)),
                None => return Err(format!("ui.view: level {v}: the model has levels 0 to {}", levels.saturating_sub(1))),
            },
        };
        let phase = match p.get("phase").and_then(Value::as_str) {
            None => self.phase,
            Some(id) => self.session.doc.phase_index(id).map_err(|e| format!("ui.view: {e}"))?,
        };
        let filter = match p.get("filter").and_then(Value::as_str) {
            None => self.filter,
            Some(f) => parse_filter(f).ok_or_else(|| format!("ui.view: filter {f}: use complete, demolition, new or all"))?,
        };
        let focus = match p.get("focus") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let id = v.as_str().ok_or("ui.view: focus must be an element id")?;
                let i = self.scene.find(id).ok_or_else(|| format!("ui.view: no element {} in the view", id.chars().take(128).collect::<String>()))?;
                Some(i)
            }
        };
        let phase_changed = phase != self.phase || filter != self.filter;
        let before = (self.mode, self.level, self.plan_level);
        self.mode = mode;
        self.phase = phase;
        self.filter = filter;
        if let Some(l) = level {
            self.level = l;
            if let Some(l) = l {
                self.plan_level = l;
            }
        }
        if phase_changed || before != (self.mode, self.level, self.plan_level) {
            self.view_changed(phase_changed);
        }
        if p.get("fit").and_then(Value::as_bool) == Some(true) {
            self.fit();
        }
        if let Some(i) = focus {
            self.focus(i);
        }
        Ok(self.view_state())
    }

    pub(crate) fn view_state(&self) -> Value {
        let b = self.session.doc.buildings.first();
        json!({
            "mode": self.mode.name(),
            "level": self.level,
            "plan_level": self.plan_level,
            "levels": b.map(|b| b.storeys.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
            "phase": self.session.doc.phases.get(self.phase).map(|p| p.id.as_str()),
            "phases": self.session.doc.phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            "filter": filter_name(self.filter),
            "selection": self.selection.and_then(|i| self.scene.elements.get(i)).map(|e| e.id.as_str()),
            "camera": {
                "target_m": self.camera.target,
                "distance_m": self.camera.distance,
                "yaw_deg": self.camera.yaw.to_degrees(),
                "pitch_deg": self.camera.pitch.to_degrees(),
            },
            "model_box_m": self.scene.bounds,
            "window_hidden": self.window_hidden,
        })
    }

    /// Answer the screenshots whose view is ready: viewport shots once no rebuild is on its way,
    /// window shots once the window has drawn a frame after that.
    pub(crate) fn serve_shots(&mut self, ctx: &egui::Context) {
        if self.shots.is_empty() {
            return;
        }
        let ready = !self.building() && !(self.mode == Mode::Plan && self.drawing_plan());
        let now = Instant::now();
        let mut keep = Vec::new();
        for mut shot in std::mem::take(&mut self.shots) {
            if now > shot.deadline {
                shot.responder.send(Err("ui.screenshot: the view did not get ready in time (window hidden or a rebuild still running)".into()));
                continue;
            }
            if !ready {
                keep.push(shot);
                continue;
            }
            match shot.target {
                ShotTarget::Viewport => {
                    let r = self.viewport_png().and_then(|(size, png)| shot_reply(size, png, shot.path.as_deref(), "viewport"));
                    shot.responder.send(r);
                }
                ShotTarget::Window if self.window_hidden && !shot.raise => {
                    shot.responder.send(Err("ui.screenshot: the window is minimised or covered, so it draws nothing to capture; use \
                                             target viewport (drawn offscreen), or raise: true to bring the window to the front"
                        .into()));
                }
                ShotTarget::Window if self.window_hidden => {
                    // Raised first; captured once it draws again.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    keep.push(shot);
                }
                ShotTarget::Window => {
                    if !shot.requested {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                        shot.requested = true;
                    }
                    keep.push(shot);
                }
            }
        }
        self.shots = keep;
        if !self.shots.is_empty() {
            ctx.request_repaint();
        }
    }

    fn viewport_png(&mut self) -> Result<([u32; 2], Vec<u8>), String> {
        let ppp = self.ppp;
        let rect = self.viewport_rect.unwrap_or(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0)));
        let size = [(rect.width() * ppp).round().max(16.0) as u32, (rect.height() * ppp).round().max(16.0) as u32];
        match self.mode {
            Mode::ThreeD => {
                let rs = self.render_state.clone().ok_or("no GPU renderer: the 3D view is unavailable")?;
                self.capture_3d(&rs, size, ppp)
            }
            Mode::Plan => {
                let img = self.plan.snapshot(rect, ppp).ok_or("the plan is not drawn yet")?;
                let size = [img.size[0] as u32, img.size[1] as u32];
                Ok((size, crate::render::png(size, img.as_raw())?))
            }
        }
    }

    /// The window screenshot egui delivers a frame after it was asked for.
    pub(crate) fn collect_window_shots(&mut self, ctx: &egui::Context) {
        let image = ctx.input(|i| {
            i.raw.events.iter().rev().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else { return };
        let (mut size, mut rgba) = (image.size, image.as_raw().to_vec());
        while size[0] > MAX_SHOT_WIDTH {
            (size, rgba) = halve(size, &rgba);
        }
        let size32 = [size[0] as u32, size[1] as u32];
        let png = crate::render::png(size32, &rgba);
        let mut keep = Vec::new();
        for shot in std::mem::take(&mut self.shots) {
            if matches!(shot.target, ShotTarget::Window) && shot.requested {
                let r = png.clone().and_then(|png| shot_reply(size32, png, shot.path.as_deref(), "window"));
                shot.responder.send(r);
            } else {
                keep.push(shot);
            }
        }
        self.shots = keep;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving_averages_blocks() {
        let rgba = [[0u8, 0, 0, 255], [255, 255, 255, 255], [255, 255, 255, 255], [0, 0, 0, 255]].concat();
        let (size, out) = halve([2, 2], &rgba);
        assert_eq!(size, [1, 1]);
        assert_eq!(out, vec![127, 127, 127, 255]);
        // Odd sizes and short buffers never panic.
        let (size, out) = halve([5, 3], &[1, 2, 3]);
        assert_eq!((size, out.len()), ([2, 1], 8));
    }

    #[test]
    fn base64_matches_rfc_4648() {
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fo"), "Zm8=");
    }

    #[test]
    fn screenshot_paths_must_be_png() {
        assert!(shot_reply([1, 1], vec![1], Some("/tmp/x.txt"), "viewport").is_err());
        let r = shot_reply([1, 1], vec![1, 2, 3], None, "viewport").unwrap();
        assert_eq!(r["png_base64"], "AQID");
    }
}
