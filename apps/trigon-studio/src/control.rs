//! Control requests in the window (docs/control-protocol.md): an agent's commands run through
//! the app's own `run`, so they land in the feed and the viewport like the owner's; `ui.view`
//! changes what the owner sees, `ui.screenshot` shows the agent the same pixels.

use std::time::Instant;

use eframe::egui;
use serde_json::{Value, json};
use trigon_control::{Request, methods};

use crate::activity::Who;
use crate::app::{Mode, SHOT_WAIT, Shot, ShotTarget, StudioApp, filter_name, parse_filter};

/// A checked `ui.view` request; `None` keeps the current setting.
#[derive(Debug, Default, PartialEq)]
pub struct ViewRequest {
    pub mode: Option<Mode>,
    pub drawing: Option<String>,
    /// `Some(None)`: the whole model in 3D.
    pub level: Option<Option<usize>>,
    pub phase: Option<usize>,
    pub filter: Option<trigon_model::build::PhaseFilter>,
    pub phase_colors: Option<bool>,
    pub fit: bool,
    pub focus: Option<String>,
}

fn opt_str<'a>(p: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.len() <= 256 => Ok(Some(s)),
        Some(_) => Err(format!("ui.view: `{key}` must be a short string")),
    }
}

/// Check a `ui.view` request against a model with `levels` levels; `phase_index` finds a phase.
pub fn parse_view(p: &Value, levels: usize, phase_index: &dyn Fn(&str) -> Result<usize, String>) -> Result<ViewRequest, String> {
    if !p.is_object() && !p.is_null() {
        return Err("ui.view: params must be an object".into());
    }
    let mode = match opt_str(p, "mode")? {
        None => None,
        Some("3d") => Some(Mode::ThreeD),
        Some("plan") => Some(Mode::Plan),
        Some("drawing") => Some(Mode::Drawing),
        Some(other) => return Err(format!("ui.view: mode {other}: use 3d, plan or drawing")),
    };
    let drawing = opt_str(p, "drawing")?.map(str::to_string);
    if let Some(id) = &drawing {
        trigon_model::annotation::identifier(id).map_err(|e| format!("ui.view: {e}"))?;
    }
    let mode = mode.or(drawing.as_ref().map(|_| Mode::Drawing));
    let level = match p.get("level") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => match v.as_u64().and_then(|l| usize::try_from(l).ok()).filter(|l| *l < levels) {
            Some(l) => Some(Some(l)),
            None => return Err(format!("ui.view: level {v}: the model has levels 0 to {}", levels.saturating_sub(1))),
        },
    };
    let phase = match opt_str(p, "phase")? {
        None => None,
        Some(id) => Some(phase_index(id).map_err(|e| format!("ui.view: {e}"))?),
    };
    let filter = match opt_str(p, "filter")? {
        None => None,
        Some(f) => Some(parse_filter(f).ok_or_else(|| format!("ui.view: filter {f}: use complete, demolition, new or all"))?),
    };
    let phase_colors = match opt_str(p, "appearance")? {
        None => None,
        Some("materials") => Some(false),
        Some("phases") => Some(true),
        Some(_) => return Err("ui.view: appearance must be materials or phases".into()),
    };
    let fit = match p.get("fit") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("ui.view: `fit` must be true or false".into()),
    };
    let focus = opt_str(p, "focus")?.map(str::to_string);
    Ok(ViewRequest { mode, drawing, level, phase, filter, phase_colors, fit, focus })
}

/// Longest queue of synthetic input.
const MAX_SYNTHETIC: usize = 64;

fn modifiers(p: &Value) -> egui::Modifiers {
    let on = |k: &str| p.get(k).and_then(Value::as_bool).unwrap_or(false);
    egui::Modifiers {
        alt: on("alt"),
        ctrl: on("ctrl"),
        shift: on("shift"),
        mac_cmd: cfg!(target_os = "macos") && on("command"),
        command: on("command"),
    }
}

/// `ui.click`, `ui.drag` (window points), or `ui.key`:
/// the egui events a real click or key press makes.
pub fn parse_input(method: &str, p: &Value) -> Result<Vec<egui::Event>, String> {
    let m = modifiers(p);
    if method == "ui.key" {
        let name = p.get("key").and_then(Value::as_str).ok_or("ui.key needs `key`, an egui key name such as \"K\" or \"Escape\"")?;
        let key = egui::Key::from_name(name).ok_or_else(|| format!("ui.key: unknown key {}", name.chars().take(32).collect::<String>()))?;
        let ev = |pressed| egui::Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers: m };
        return Ok(vec![ev(true), ev(false)]);
    }
    if method == "ui.drag" {
        let point = |key: &str| -> Result<egui::Pos2, String> {
            let values = p
                .get(key)
                .and_then(Value::as_array)
                .filter(|v| v.len() == 2)
                .ok_or_else(|| format!("ui.drag: `{key}` must be [x,y] in window points"))?;
            let coord = |v: &Value| v.as_f64().filter(|v| v.is_finite() && (0.0..=20_000.0).contains(v)).map(|v| v as f32);
            match (coord(&values[0]), coord(&values[1])) {
                (Some(x), Some(y)) => Ok(egui::pos2(x, y)),
                _ => Err(format!("ui.drag: `{key}` coordinates must be between 0 and 20000")),
            }
        };
        let from = point("from")?;
        let to = point("to")?;
        let steps = match p.get("steps") {
            None => 8,
            Some(v) => v.as_u64().filter(|n| (1..=60).contains(n)).ok_or("ui.drag: `steps` must be an integer from 1 to 60")? as usize,
        };
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: m };
        let mut events = vec![egui::Event::PointerMoved(from), press(from, true)];
        for step in 1..=steps {
            events.push(egui::Event::PointerMoved(from + (to - from) * (step as f32 / steps as f32)));
        }
        events.push(press(to, false));
        return Ok(events);
    }
    let coord = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && (0.0..=20_000.0).contains(v)).map(|v| v as f32);
    let (Some(x), Some(y)) = (coord("x"), coord("y")) else {
        return Err("ui.click needs `x` and `y`: window points from the top left".into());
    };
    let button = match p.get("button").and_then(Value::as_str).unwrap_or("left") {
        "left" => egui::PointerButton::Primary,
        "right" => egui::PointerButton::Secondary,
        "middle" => egui::PointerButton::Middle,
        other => return Err(format!("ui.click: button {}: use left, right or middle", other.chars().take(16).collect::<String>())),
    };
    let pos = egui::pos2(x, y);
    let press = |pressed| egui::Event::PointerButton { pos, button, pressed, modifiers: m };
    let mut events = vec![egui::Event::PointerMoved(pos), press(true), press(false)];
    if p.get("double").and_then(Value::as_bool) == Some(true) {
        events.extend([press(true), press(false)]);
    }
    events.truncate(MAX_SYNTHETIC);
    Ok(events)
}

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
            "ui.view" => self.ui_view(&params).map(|mut view| {
                view["window_focused"] = json!(ctx.input(|i| i.viewport().focused));
                view
            }),
            "ui.click" | "ui.key" | "ui.drag" => parse_input(&method, &params).and_then(|events| {
                if self.synthetic.len() + events.len() > 4 * MAX_SYNTHETIC {
                    return Err("busy: earlier input is still being delivered; send it again shortly".into());
                }
                let n = events.len();
                self.synthetic.extend(events);
                ctx.request_repaint();
                Ok(json!({"queued": n, "window_hidden": self.window_hidden}))
            }),
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
                // An unfocused window can still report itself as unoccluded. Honour explicit
                // raising in that case too; handle drags intentionally cancel without focus.
                if raise && matches!(target, ShotTarget::Window) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
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

    /// `ui.view`: checked whole, then applied.
    fn ui_view(&mut self, p: &Value) -> Result<Value, String> {
        let levels = self.session.doc.buildings.first().map_or(0, |b| b.storeys.len());
        let doc = &self.session.doc;
        let req = parse_view(p, levels, &|id| doc.phase_index(id).map_err(|e| e.to_string()))?;
        let drawing = req.drawing.as_deref().unwrap_or(&self.drawing);
        if req.mode == Some(Mode::Drawing) || req.drawing.is_some() {
            doc.views
                .iter()
                .find(|v| v.id == drawing)
                .ok_or_else(|| format!("ui.view: no drawing {drawing}"))?
                .validate_source(doc)
                .map_err(|e| format!("ui.view: {e}"))?;
        }
        if req.mode.unwrap_or(self.mode) == Mode::Drawing && req.focus.is_some() {
            return Err("ui.view: use focus in 3D, or drawing to open a saved drawing".into());
        }
        let focus = match &req.focus {
            Some(id) => Some(self.scene.find(id).ok_or_else(|| format!("ui.view: no element {id} in the view"))?),
            None => None,
        };
        let phase = req.phase.unwrap_or(self.phase);
        let filter = req.filter.unwrap_or(self.filter);
        let phase_colors = req.phase_colors.unwrap_or(self.phase_colors);
        let phase_changed = phase != self.phase || filter != self.filter || phase_colors != self.phase_colors;
        let before = (self.mode, self.level, self.plan_level, self.drawing.clone());
        self.mode = req.mode.unwrap_or(self.mode);
        if let Some(id) = req.drawing {
            self.drawing = id;
        }
        if req.mode == Some(Mode::Drawing) {
            self.show_drawing_properties();
        }
        self.phase = phase;
        self.filter = filter;
        self.phase_colors = phase_colors;
        if let Some(l) = req.level {
            self.level = l;
            if let Some(l) = l {
                self.plan_level = l;
            }
        }
        if phase_changed || before != (self.mode, self.level, self.plan_level, self.drawing.clone()) {
            self.view_changed(phase_changed);
        }
        if req.fit {
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
            "drawing": if self.mode==Mode::Drawing {Some(&self.drawing)} else {None},
            "drawing_raster": self.plan.diagnostics(),
            "drawing_mapping": if self.mode!=Mode::ThreeD && !self.drawing_plan() {self.viewport_rect.filter(|r|self.plan.ready(*r,self.ppp)).and_then(|r|self.plan.screen_mapping(r))} else {None},
            "drawings": self.session.doc.views.iter().map(|v|(&v.id,&v.name)).collect::<Vec<_>>(),
            "levels": b.map(|b| b.storeys.iter().map(|s| s.name.as_str()).collect::<Vec<_>>()).unwrap_or_default(),
            "phase": self.session.doc.phases.get(self.phase).map(|p| p.id.as_str()),
            "phases": self.session.doc.phases.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            "filter": filter_name(self.filter),
            "appearance": if self.phase_colors { "phases" } else { "materials" },
            "selection": self.selection.and_then(|i| self.scene.elements.get(i)).map(|e| e.id.as_str()),
            "camera": {
                "target_m": self.camera.target,
                "distance_m": self.camera.distance,
                "yaw_deg": self.camera.yaw.to_degrees(),
                "pitch_deg": self.camera.pitch.to_degrees(),
            },
            "model_box_m": self.scene.bounds,
            "window_hidden": self.window_hidden,
            "viewport_window_points": self.viewport_rect.map(|r|[r.left(),r.top(),r.width(),r.height()]),
            "handles": self.viewport_rect.map(|rect|self.projected_handles(rect).into_iter().filter_map(|handle|{
                handle.mapping.screen(0.0).filter(|point|rect.contains(*point)).map(|point|json!({"key":handle.handle.key,"label":handle.handle.label,"point":[point.x,point.y],"value_mm":handle.handle.value}))
            }).collect::<Vec<_>>()).unwrap_or_default(),
        })
    }

    /// Answer the screenshots whose view is ready: viewport shots once no rebuild is on its way,
    /// window shots once the window has drawn a frame after that.
    pub(crate) fn serve_shots(&mut self, ctx: &egui::Context) {
        if self.shots.is_empty() {
            return;
        }
        let ready = !self.building() && !(self.mode != Mode::ThreeD && self.drawing_plan());
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
                    if self.mode != Mode::ThreeD
                        && self.plan.error.is_none()
                        && !self.viewport_rect.is_some_and(|rect| self.plan.ready(rect, self.ppp))
                    {
                        keep.push(shot);
                        continue;
                    }
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
            Mode::Plan | Mode::Drawing => {
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
    fn appearance_control_is_explicit_checked_and_optional() {
        let phase = |_: &str| Ok(0);
        for (value, expected) in [("materials", false), ("phases", true)] {
            assert_eq!(parse_view(&json!({"appearance": value}), 1, &phase).unwrap().phase_colors, Some(expected));
        }
        for value in [json!(true), json!(23), json!("green")] {
            assert!(parse_view(&json!({"appearance":value}), 1, &phase).is_err());
        }
    }

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
    fn view_requests_are_checked_whole() {
        let phases = |id: &str| if id == "2026" { Ok(1) } else { Err(format!("phase {id} not found")) };
        let ok =
            parse_view(&json!({"mode": "plan", "level": 2, "phase": "2026", "filter": "all", "fit": true, "focus": "b1.f3"}), 6, &phases).unwrap();
        assert_eq!(ok.mode, Some(Mode::Plan));
        let drawing = parse_view(&json!({"mode":"drawing","drawing":"section-a"}), 6, &phases).unwrap();
        assert_eq!((drawing.mode, drawing.drawing.as_deref()), (Some(Mode::Drawing), Some("section-a")));
        assert_eq!(parse_view(&json!({"drawing":"facade-b1.f1"}), 6, &phases).unwrap().mode, Some(Mode::Drawing));
        assert!(parse_view(&json!({"drawing":"../invalid"}), 6, &phases).is_err());
        assert_eq!((ok.level, ok.phase, ok.fit, ok.focus.as_deref()), (Some(Some(2)), Some(1), true, Some("b1.f3")));
        assert_eq!(parse_view(&json!(null), 6, &phases).unwrap(), ViewRequest::default());
        assert_eq!(parse_view(&json!({"level": null}), 6, &phases).unwrap().level, Some(None));
        let long = "x".repeat(10_000);
        for bad in [
            json!([]),
            json!("plan"),
            json!({"mode": 5}),
            json!({"mode": "top"}),
            json!({"level": -1}),
            json!({"level": 6}),
            json!({"level": 1e300}),
            json!({"level": "2"}),
            json!({"level": u64::MAX}),
            json!({"phase": "1066"}),
            json!({"phase": ["2026"]}),
            json!({"filter": "everything"}),
            json!({"fit": "yes"}),
            json!({"focus": 7}),
            json!({"focus": long}),
        ] {
            assert!(parse_view(&bad, 6, &phases).is_err(), "{bad}");
        }
        assert!(parse_view(&json!({"level": 0}), 0, &phases).is_err(), "an empty model has no levels");
    }

    #[test]
    fn synthetic_input_is_checked() {
        let click = parse_input("ui.click", &json!({"x": 10, "y": 20.5})).unwrap();
        assert_eq!(click.len(), 3);
        assert!(matches!(click[1], egui::Event::PointerButton { pressed: true, button: egui::PointerButton::Primary, .. }));
        assert_eq!(parse_input("ui.click", &json!({"x": 1, "y": 1, "double": true})).unwrap().len(), 5);
        let key = parse_input("ui.key", &json!({"key": "K", "command": true})).unwrap();
        assert!(matches!(key[0], egui::Event::Key { key: egui::Key::K, pressed: true, modifiers, .. } if modifiers.command));
        for (m, bad) in [
            ("ui.click", json!({})),
            ("ui.click", json!({"x": -1, "y": 0})),
            ("ui.click", json!({"x": f64::MAX, "y": 0})),
            ("ui.click", json!({"x": "1", "y": 0})),
            ("ui.click", json!({"x": 1, "y": 1, "button": "fourth"})),
            ("ui.key", json!({})),
            ("ui.key", json!({"key": "NotAKey"})),
            ("ui.key", json!({"key": 5})),
        ] {
            assert!(parse_input(m, &bad).is_err(), "{m} {bad}");
        }
    }

    #[test]
    fn drag_input_is_complete_bounded_and_releases_at_the_last_position() {
        let events = parse_input("ui.drag", &json!({"from":[10,20],"to":[110,220],"steps":4,"shift":true})).unwrap();
        assert_eq!(events.len(), 7);
        assert!(matches!(events[1],egui::Event::PointerButton{pos,pressed:true,modifiers,..} if pos==egui::pos2(10.0,20.0) && modifiers.shift));
        assert!(matches!(events[5],egui::Event::PointerMoved(pos) if pos==egui::pos2(110.0,220.0)));
        assert!(matches!(events[6],egui::Event::PointerButton{pos,pressed:false,..} if pos==egui::pos2(110.0,220.0)));
        assert!(parse_input("ui.drag", &json!({"from":[0,0],"to":[20000,20000],"steps":60})).unwrap().len() <= MAX_SYNTHETIC);
        for bad in [
            json!({}),
            json!({"from":[0,0],"to":[1]}),
            json!({"from":[-1,0],"to":[1,1]}),
            json!({"from":[0,0],"to":[f64::MAX,1]}),
            json!({"from":[0,0],"to":[1,1],"steps":0}),
            json!({"from":[0,0],"to":[1,1],"steps":61}),
            json!({"from":[0,0],"to":[1,1],"steps":1.5}),
        ] {
            assert!(parse_input("ui.drag", &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn screenshot_paths_must_be_png() {
        assert!(shot_reply([1, 1], vec![1], Some("/tmp/x.txt"), "viewport").is_err());
        let r = shot_reply([1, 1], vec![1, 2, 3], None, "viewport").unwrap();
        assert_eq!(r["png_base64"], "AQID");
    }
}
