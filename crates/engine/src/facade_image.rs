//! Rectified facade images from Skråfoto: what the agent looks at before it places anything.
//!
//! The facade plane (outer wall face, from the ground to the eaves) is sampled on a regular
//! grid in millimetres, each sample projected into the photo with the photo's camera and read
//! bilinearly. The result is an orthographic elevation of the photo, to scale, with the model's
//! storey lines, bay lines and openings drawn over it.

use std::path::{Path, PathBuf};

use trigon_data::{Camera, Photo};
use trigon_model::build::{FacadeFrame, Status, View, resolve_facade};
use trigon_model::{Building, Document, ModelError, Site};

use crate::{EngineError, Result, Session};

pub struct FacadeImage {
    pub path: PathBuf,
    pub photo: Photo,
    pub width: u32,
    pub height: u32,
    pub mm_per_px: f64,
    pub length_mm: f64,
    pub height_mm: f64,
    /// Share of the image's pixels the photo covered (the rest is outside the photo).
    pub coverage: f64,
}

/// Highest image side, pixels.
const MAX_SIDE: f64 = 4000.0;

fn world(site: &Site, fr: &FacadeFrame, u: f64, v: f64) -> [f64; 3] {
    let p = fr.point(u, 0.0, v);
    [site.origin_utm[0] + p[0] / 1000.0, site.origin_utm[1] + p[1] / 1000.0, site.elevation + p[2] / 1000.0]
}

/// Facades seen at a flatter angle than this (cosine to the wall normal) aren't used.
const MIN_FACING: f64 = 0.15;

/// How squarely the camera sees the wall's outer face: the cosine between the outward normal
/// and the direction to the camera. Negative = the camera is behind the wall.
fn facing(cam: &Camera, fr: &FacadeFrame, mid: [f64; 3]) -> f64 {
    let to = [cam.center[0] - mid[0], cam.center[1] - mid[1], cam.center[2] - mid[2]];
    let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
    if !(len > 0.0) {
        return -1.0;
    }
    (fr.n[0] * to[0] + fr.n[1] * to[1]) / len
}

/// Projected area (px²) of the facade in a photo, or `None` if any corner falls outside it.
fn projected_area(cam: &Camera, corners: &[[f64; 3]; 4]) -> Option<f64> {
    let px: Vec<[f64; 2]> = corners.iter().map(|c| cam.project(*c)).collect::<Option<_>>()?;
    if !px.iter().all(|p| cam.contains(*p)) {
        return None;
    }
    let n = px.len();
    Some((0..n).map(|i| px[i][0] * px[(i + 1) % n][1] - px[(i + 1) % n][0] * px[i][1]).sum::<f64>().abs() / 2.0)
}

pub(crate) fn facade_frame(b: &Building, facade: &str) -> Result<FacadeFrame> {
    let f = b.facade(facade)?;
    let (a, e) = b.edge(f.edge).ok_or_else(|| ModelError::NotFound(format!("facade {facade}")))?;
    Ok(FacadeFrame::new(a, e).ok_or_else(|| ModelError::Invalid(format!("facade {facade} has zero length")))?)
}

pub fn render(
    s: &Session,
    facade: &str,
    photo_id: Option<&str>,
    mm_per_px: f64,
    overlay: bool,
    out: Option<PathBuf>,
    view: View,
) -> Result<FacadeImage> {
    let doc: &Document = &s.doc;
    let site = doc.site.as_ref().ok_or_else(|| ModelError::NotFound("no site yet; run site.load_address".into()))?;
    let b =
        doc.buildings.iter().find(|b| b.facades.iter().any(|f| f.id == facade)).ok_or_else(|| ModelError::NotFound(format!("facade {facade}")))?;
    let fr = facade_frame(b, facade)?;
    let h = b.height();
    let mm_per_px = trigon_model::checked("mm_per_px", mm_per_px, 10.0, 500.0)?;
    let corners = [world(site, &fr, 0.0, 0.0), world(site, &fr, fr.length, 0.0), world(site, &fr, fr.length, h), world(site, &fr, 0.0, h)];

    let mid = world(site, &fr, fr.length / 2.0, h / 2.0);
    let photos = s.provider().photos(site.origin_utm)?;
    let photo = match photo_id {
        Some(id) => {
            let p = photos.into_iter().find(|p| p.id == id).ok_or_else(|| ModelError::NotFound(format!("photo {id}")))?;
            if p.camera.as_ref().is_some_and(|c| facing(c, &fr, mid) < MIN_FACING) {
                return Err(ModelError::Invalid(format!("photo {id} sees facade {facade} from behind or edge-on")).into());
            }
            p
        }
        None => photos
            .into_iter()
            .filter_map(|p| {
                let cam = p.camera.as_ref()?;
                if facing(cam, &fr, mid) < MIN_FACING {
                    return None;
                }
                let a = projected_area(cam, &corners)?;
                Some((a, p))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.datetime.cmp(&b.1.datetime)))
            .map(|(_, p)| p)
            .ok_or_else(|| ModelError::NotFound(format!("no photo shows all of facade {facade}")))?,
    };
    let cam = photo.camera.clone().ok_or_else(|| ModelError::NotFound(format!("photo {} has no camera", photo.id)))?;

    // The photo window the facade covers, with a margin for bilinear sampling.
    let px: Vec<[f64; 2]> = corners
        .iter()
        .map(|c| cam.project(*c))
        .collect::<Option<_>>()
        .ok_or_else(|| ModelError::Invalid("the facade is behind the camera".into()))?;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in &px {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let clamp = |v: f64, hi: f64| v.clamp(0.0, hi) as u32;
    let window = [clamp(x0 - 4.0, cam.width), clamp(y0 - 4.0, cam.height), clamp(x1 + 5.0, cam.width), clamp(y1 + 5.0, cam.height)];
    let region = s.provider().photo_region(&photo, window)?;

    let width = (fr.length / mm_per_px).ceil().clamp(1.0, MAX_SIDE) as u32;
    let height = (h / mm_per_px).ceil().clamp(1.0, MAX_SIDE) as u32;
    let (du, dv) = (fr.length / f64::from(width), h / f64::from(height));
    let mut rgb = vec![40u8; (width * height * 3) as usize];
    let mut covered = 0usize;
    for j in 0..height {
        let v = h - (f64::from(j) + 0.5) * dv;
        for i in 0..width {
            let u = (f64::from(i) + 0.5) * du;
            if let Some(c) = cam.project(world(site, &fr, u, v)).and_then(|p| region.sample(p)) {
                let k = ((j * width + i) * 3) as usize;
                if let Some(dst) = rgb.get_mut(k..k + 3) {
                    dst.copy_from_slice(&c);
                    covered += 1;
                }
            }
        }
    }

    if overlay {
        draw_model(doc, b, facade, &fr, &mut Pen { rgb: &mut rgb, width, height, du, dv, h }, view)?;
    }

    let path = out.unwrap_or_else(|| default_path(s.path.as_deref(), facade, &photo.id));
    write_png(&path, width, height, &rgb)?;
    Ok(FacadeImage {
        path,
        photo,
        width,
        height,
        mm_per_px,
        length_mm: fr.length,
        height_mm: h,
        coverage: covered as f64 / f64::from(width * height),
    })
}

/// The model's storey lines (cyan), bay lines (yellow) and, in a phase view, its openings and
/// balconies coloured by status: existing magenta, new green, demolished red.
pub(crate) fn draw_model(doc: &Document, b: &Building, facade: &str, fr: &FacadeFrame, pen: &mut Pen<'_>, view: View) -> Result<()> {
    let f = b.facade(facade)?;
    let h = b.height();
    for st in &b.storeys {
        pen.hline(st.elevation, 0.0, fr.length, [0, 220, 255]);
    }
    for c in &f.bays {
        pen.vline(*c, 0.0, h, [255, 210, 0]);
    }
    if let Ok(items) = resolve_facade(doc, b, f) {
        for o in items {
            let Some(status) = view.status(o.lifetime) else { continue };
            let c = match status {
                Status::Existing => [255, 0, 200],
                Status::New => [120, 200, 20],
                Status::Demolished => [235, 30, 30],
            };
            let r = o.rect;
            pen.hline(r.v0, r.u0, r.u1, c);
            pen.hline(r.v1, r.u0, r.u1, c);
            pen.vline(r.u0, r.v0, r.v1, c);
            pen.vline(r.u1, r.v0, r.v1, c);
            if !o.is_opening() {
                // The deck line, and a cross so balconies read apart from openings.
                pen.hline(r.v0 + 160.0, r.u0, r.u1, c);
                pen.line((r.u0, r.v0), (r.u1, r.v1), c);
                pen.line((r.u0, r.v1), (r.u1, r.v0), c);
            }
        }
    }
    Ok(())
}

pub(crate) fn default_path(doc: Option<&Path>, facade: &str, photo: &str) -> PathBuf {
    let dir = doc.and_then(Path::parent).map_or_else(|| PathBuf::from("trigon-evidence"), |d| d.join("trigon-evidence"));
    let safe: String =
        format!("{facade}-{photo}").chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
    dir.join(format!("{safe}.png"))
}

pub(crate) struct Pen<'a> {
    pub(crate) rgb: &'a mut [u8],
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) du: f64,
    pub(crate) dv: f64,
    pub(crate) h: f64,
}

impl Pen<'_> {
    /// A straight line between two facade points (u, v) in mm.
    pub(crate) fn line(&mut self, a: (f64, f64), b: (f64, f64), c: [u8; 3]) {
        let (ia, ja) = (a.0 / self.du, (self.h - a.1) / self.dv);
        let (ib, jb) = (b.0 / self.du, (self.h - b.1) / self.dv);
        let n = (ib - ia).abs().max((jb - ja).abs()).ceil().clamp(1.0, 20_000.0) as i64;
        for s in 0..=n {
            let t = s as f64 / n as f64;
            self.dot((ia + (ib - ia) * t).round() as i64, (ja + (jb - ja) * t).round() as i64, c);
        }
    }

    pub(crate) fn dot(&mut self, i: i64, j: i64, c: [u8; 3]) {
        if i < 0 || j < 0 || i >= i64::from(self.width) || j >= i64::from(self.height) {
            return;
        }
        let k = ((j as usize) * self.width as usize + i as usize) * 3;
        if let Some(dst) = self.rgb.get_mut(k..k + 3) {
            dst.copy_from_slice(&c);
        }
    }

    fn hline(&mut self, v: f64, u0: f64, u1: f64, c: [u8; 3]) {
        let j = ((self.h - v) / self.dv).round() as i64;
        let (a, b) = ((u0 / self.du).floor() as i64, (u1 / self.du).ceil() as i64);
        for i in a..=b {
            self.dot(i, j, c);
            self.dot(i, j - 1, c);
        }
    }

    fn vline(&mut self, u: f64, v0: f64, v1: f64, c: [u8; 3]) {
        let i = (u / self.du).round() as i64;
        let (a, b) = (((self.h - v1) / self.dv).floor() as i64, ((self.h - v0) / self.dv).ceil() as i64);
        for j in a..=b {
            self.dot(i, j, c);
            self.dot(i - 1, j, c);
        }
    }
}

pub(crate) fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::Io(format!("{}: {e}", dir.display())))?;
    }
    let file = std::fs::File::create(path).map_err(|e| EngineError::Io(format!("{}: {e}", path.display())))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| EngineError::Io(e.to_string()))?;
    w.write_image_data(rgb).map_err(|e| EngineError::Io(e.to_string()))
}
