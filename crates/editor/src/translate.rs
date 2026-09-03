//! Drag-translate helpers (spec 24 gizmo stretch). Headless math only.

use glam::Vec3;

/// Intersect a ray with the ground plane y=0. Returns the hit point, or `None`
/// for a parallel/backward ray.
pub fn ray_ground_plane(origin: Vec3, dir: Vec3) -> Option<Vec3> {
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = -origin.y / dir.y;
    if t < 0.0 {
        return None;
    }
    Some(origin + dir * t)
}
