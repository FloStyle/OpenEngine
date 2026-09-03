//! Headless orbit camera (spec 24). f32/glam — presentation math only.

use glam::{Mat4, Vec3};

/// An orbit camera around a focus point.
#[derive(Clone, Copy, Debug)]
pub struct EditorCamera {
    /// Point the camera looks at / orbits around.
    pub focus: Vec3,
    /// Distance from `focus` to the eye.
    pub distance: f32,
    /// Orbit yaw (radians).
    pub yaw: f32,
    /// Orbit pitch (radians).
    pub pitch: f32,
    /// Vertical field of view (radians).
    pub fov: f32,
}

impl Default for EditorCamera {
    fn default() -> Self {
        EditorCamera {
            focus: Vec3::ZERO,
            distance: 6.0,
            yaw: 0.6,
            pitch: 0.5,
            fov: 45f32.to_radians(),
        }
    }
}

impl EditorCamera {
    /// World-space eye position.
    pub fn eye(&self) -> Vec3 {
        let dir = Vec3::new(
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
            self.pitch.cos() * self.yaw.cos(),
        );
        self.focus + dir * self.distance
    }

    /// The combined view-projection matrix (glam column-major convention).
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = Mat4::perspective_rh(self.fov, aspect, 0.01, 200.0);
        let view = Mat4::look_at_rh(self.eye(), self.focus, Vec3::Y);
        proj * view
    }

    /// Un-project a screen NDC point (`x`, `y` in -1..1) into a world ray.
    pub fn unproject_ray(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let near = inv.transform_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv.transform_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let dir = (far - near).normalize();
        (near, dir)
    }

    /// Orbit by yaw/pitch deltas.
    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw += dyaw;
        self.pitch = (self.pitch + dpitch).clamp(-1.5, 1.5);
    }

    /// Pan the focus point (screen-space right/up in world XZ/up).
    pub fn pan(&mut self, right: Vec3, up: Vec3) {
        self.focus += right * self.distance * 0.001 + up * self.distance * 0.001;
    }

    /// Zoom (positive = closer).
    pub fn zoom(&mut self, delta: f32) {
        self.distance = (self.distance - delta).clamp(0.5, 200.0);
    }
}
