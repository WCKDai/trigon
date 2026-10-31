//! The viewport's orbit camera, Z up, in scene metres. Matrices are column-major, as WGSL reads
//! them, with wgpu's 0..1 depth range.

pub type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 && l.is_finite() { scale(a, 1.0 / l) } else { [0.0, 0.0, 1.0] }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub target: V3,
    /// Around Z from +X, radians.
    pub yaw: f32,
    /// Above the horizon, radians.
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
    /// Half the diagonal of the model's box, for the depth range.
    pub radius: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        // From the south-west, a little above: a familiar first look at a building.
        Orbit { target: [0.0; 3], yaw: (-120f32).to_radians(), pitch: 28f32.to_radians(), distance: 60.0, fov_y: 40f32.to_radians(), radius: 30.0 }
    }
}

const MIN_DISTANCE: f32 = 0.2;
const MAX_DISTANCE: f32 = 100_000.0;

impl Orbit {
    pub fn eye(&self) -> V3 {
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        add(self.target, scale([cp * self.yaw.cos(), cp * self.yaw.sin(), sp], self.distance))
    }

    /// Forward, right and up, unit length.
    pub fn basis(&self) -> (V3, V3, V3) {
        let f = normalize(sub(self.target, self.eye()));
        let r = normalize(cross(f, [0.0, 0.0, 1.0]));
        let u = cross(r, f);
        (f, r, u)
    }

    pub fn near_far(&self) -> (f32, f32) {
        let near = (self.distance - self.radius * 1.5).max(self.distance * 0.002).max(0.02);
        let far = self.distance + self.radius * 4.0 + 50.0;
        (near, far)
    }

    /// Projection × view.
    pub fn view_proj(&self, aspect: f32) -> [f32; 16] {
        let eye = self.eye();
        let (f, r, u) = self.basis();
        // View: rows are right, up, -forward.
        let view = [r[0], u[0], -f[0], 0.0, r[1], u[1], -f[1], 0.0, r[2], u[2], -f[2], 0.0, -dot(r, eye), -dot(u, eye), dot(f, eye), 1.0];
        let (n, far) = self.near_far();
        let t = 1.0 / (self.fov_y / 2.0).tan();
        let a = if aspect > 0.0 && aspect.is_finite() { aspect } else { 1.0 };
        let proj = [t / a, 0.0, 0.0, 0.0, 0.0, t, 0.0, 0.0, 0.0, 0.0, far / (n - far), -1.0, 0.0, 0.0, n * far / (n - far), 0.0];
        mul(&proj, &view)
    }

    /// The ray through a point of the view, given in -1..1 (x right, y up).
    pub fn ray(&self, ndc: [f32; 2], aspect: f32) -> (V3, V3) {
        let (f, r, u) = self.basis();
        let t = (self.fov_y / 2.0).tan();
        let d = normalize(add(f, add(scale(r, ndc[0] * t * aspect), scale(u, ndc[1] * t))));
        (self.eye(), d)
    }

    /// Turn around the target by screen-space amounts (radians).
    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        if dyaw.is_finite() && dpitch.is_finite() {
            self.yaw = (self.yaw - dyaw).rem_euclid(std::f32::consts::TAU);
            self.pitch = (self.pitch + dpitch).clamp(-1.55, 1.55);
        }
    }

    /// Slide the view by `dx`, `dy` points of a view `height` points tall.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        if !(height > 0.0) || !dx.is_finite() || !dy.is_finite() {
            return;
        }
        let (_, r, u) = self.basis();
        let per_point = 2.0 * self.distance * (self.fov_y / 2.0).tan() / height;
        self.target = add(self.target, add(scale(r, -dx * per_point), scale(u, dy * per_point)));
    }

    /// Zoom by `factor` (below 1 comes closer), keeping the point under the ray `dir` in place.
    pub fn zoom_at(&mut self, factor: f32, dir: V3) {
        if !(factor > 0.0) || !factor.is_finite() {
            return;
        }
        let new_distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let factor = new_distance / self.distance;
        let (f, _, _) = self.basis();
        let along = dot(dir, f);
        if along > 0.05 {
            // The point under the cursor on the plane through the target, facing the camera.
            let p = add(self.eye(), scale(dir, self.distance / along));
            self.target = add(p, scale(sub(self.target, p), factor));
        }
        self.distance = new_distance;
    }

    /// Frame a box, keeping the direction.
    pub fn fit(&mut self, lo: V3, hi: V3, aspect: f32) {
        let c = scale(add(lo, hi), 0.5);
        let r = (dot(sub(hi, lo), sub(hi, lo)).sqrt() / 2.0).max(0.5);
        if !r.is_finite() || !c.iter().all(|v| v.is_finite()) {
            return;
        }
        let half = (self.fov_y / 2.0).min((self.fov_y / 2.0).tan().atan2(1.0 / aspect.clamp(0.2, 5.0)));
        self.target = c;
        self.radius = r;
        // The bounding sphere is generous for a building's flat box: frame a little tighter.
        self.distance = (r / half.sin() * 0.9).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Look at a box from outside along `facing` (x, y), slightly from above, with room around it.
    pub fn focus(&mut self, lo: V3, hi: V3, facing: Option<[f32; 2]>, aspect: f32) {
        if let Some([x, y]) = facing
            && x.is_finite()
            && y.is_finite()
            && (x != 0.0 || y != 0.0)
        {
            self.yaw = y.atan2(x);
            self.pitch = 12f32.to_radians();
        }
        let pad = scale(sub(hi, lo), 0.35);
        let pad = [pad[0].max(2.0), pad[1].max(2.0), pad[2].max(2.0)];
        self.fit(sub(lo, pad), add(hi, pad), aspect);
    }
}

/// `a × b`, column-major.
fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

/// A point through `m`, in normalised device coordinates.
#[cfg(test)]
pub fn project(m: &[f32; 16], p: V3) -> V3 {
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let z = m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14];
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    [x / w, y / w, z / w]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_projects_to_the_centre_and_depth_is_in_range() {
        let c = Orbit { target: [3.0, -2.0, 5.0], ..Default::default() };
        let m = c.view_proj(1.5);
        let p = project(&m, c.target);
        assert!(p[0].abs() < 1e-4 && p[1].abs() < 1e-4, "{p:?}");
        assert!((0.0..1.0).contains(&p[2]), "{p:?}");
        // Up is up on screen.
        let above = project(&m, [3.0, -2.0, 6.0]);
        assert!(above[1] > 0.0);
    }

    #[test]
    fn rays_go_through_what_they_point_at() {
        let c = Orbit::default();
        let m = c.view_proj(2.0);
        let p = [4.0, 1.0, 2.0];
        let s = project(&m, p);
        let (o, d) = c.ray([s[0], s[1]], 2.0);
        let to = normalize(sub(p, o));
        assert!(dot(to, d) > 0.99999, "{to:?} {d:?}");
    }

    #[test]
    fn fit_frames_the_box_and_zoom_keeps_the_cursor_point() {
        let mut c = Orbit::default();
        c.fit([-10.0, -5.0, 0.0], [10.0, 5.0, 15.0], 1.6);
        c.focus([-1.0, -5.0, 3.0], [1.0, -4.8, 4.5], Some([0.0, -1.0]), 1.6);
        assert!(c.eye()[1] < -5.0, "a south-facing window is seen from the south");
        c.fit([-10.0, -5.0, 0.0], [10.0, 5.0, 15.0], 1.6);
        let m = c.view_proj(1.6);
        for corner in [[-10.0, -5.0, 0.0], [10.0, 5.0, 15.0], [10.0, -5.0, 0.0], [-10.0, 5.0, 15.0]] {
            let p = project(&m, corner);
            assert!(p[0].abs() <= 1.0 && p[1].abs() <= 1.0, "{corner:?} → {p:?}");
        }
        let (_, d) = c.ray([0.5, 0.2], 1.6);
        let before = c.target;
        c.zoom_at(0.5, d);
        assert!(c.distance > 0.0 && c.target != before);
        let (_, d2) = c.ray([0.5, 0.2], 1.6);
        assert!(dot(d, d2) > 0.9999, "the point under the cursor stays put");
    }

    #[test]
    fn hostile_input_leaves_the_camera_usable() {
        let mut c = Orbit::default();
        c.orbit(f32::NAN, 1.0);
        c.pan(f32::INFINITY, 0.0, 100.0);
        c.zoom_at(0.0, [0.0, 0.0, 1.0]);
        c.zoom_at(f32::NAN, [0.0, 0.0, 1.0]);
        c.fit([f32::NAN; 3], [1.0; 3], 1.0);
        c.zoom_at(1e30, [0.0, 0.0, -1.0]);
        assert!(c.view_proj(1.0).iter().all(|v| v.is_finite()));
    }
}
