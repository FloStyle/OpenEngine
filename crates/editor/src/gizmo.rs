//! 3D gizmo math (Unreal-like translate gizmo) — headless, no UI.
//!
//! Standard screen-plane translation: to drag an actor along a world axis, we
//! intersect the viewport ray with the plane that passes through the actor and
//! faces the camera, then read the world coordinate of that point on the chosen
//! axis. The editor keeps a grab offset so the actor does not jump; snapping is
//! applied on top by the caller.

use glam::Vec3;

use crate::camera::EditorCamera;

/// The camera's forward (view) direction.
pub fn camera_forward(camera: &EditorCamera) -> Vec3 {
    (camera.focus - camera.eye()).normalize()
}

/// Intersect the viewport ray through NDC `(-1..1)` with the plane that passes
/// through `anchor` and faces the camera. Returns the world point, or `None`
/// when the ray is (near-)parallel to that plane.
pub fn screen_plane_point(
    camera: &EditorCamera,
    ndc_x: f32,
    ndc_y: f32,
    aspect: f32,
    anchor: Vec3,
) -> Option<Vec3> {
    let (origin, dir) = camera.unproject_ray(ndc_x, ndc_y, aspect);
    let n = camera_forward(camera);
    let denom = n.dot(dir);
    if denom.abs() < 1e-5 {
        return None;
    }
    let t = (anchor - origin).dot(n) / denom;
    if t < 0.0 {
        return None;
    }
    Some(origin + dir * t)
}

/// The world coordinate of the cursor's screen-plane point on the given axis
/// (`0`=X, `1`=Y, `2`=Z). This is what a translate gizmo drags that axis to.
pub fn axis_coord(
    camera: &EditorCamera,
    ndc_x: f32,
    ndc_y: f32,
    aspect: f32,
    anchor: Vec3,
    axis: usize,
) -> Option<f32> {
    screen_plane_point(camera, ndc_x, ndc_y, aspect, anchor).map(|p| p[axis])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(focus: Vec3, pitch: f32) -> EditorCamera {
        EditorCamera {
            focus,
            distance: 20.0,
            yaw: 0.0,
            pitch,
            fov: 45f32.to_radians(),
        }
    }

    #[test]
    fn dragging_along_x_changes_only_x() {
        let c = cam(Vec3::ZERO, 0.4);
        let anchor = Vec3::ZERO;
        let near = axis_coord(&c, 0.0, 0.0, 1.0, anchor, 0).unwrap();
        let far = axis_coord(&c, 0.7, 0.0, 1.0, anchor, 0).unwrap();
        assert!(
            (far - near).abs() > 0.01,
            "moving the cursor horizontally must change the X coordinate, got {near} -> {far}"
        );
        // Y and Z must be unchanged at this drag height.
        let ya = axis_coord(&c, 0.0, 0.0, 1.0, anchor, 1).unwrap();
        let yb = axis_coord(&c, 0.7, 0.0, 1.0, anchor, 1).unwrap();
        assert!(
            (ya - yb).abs() < 0.01,
            "pure horizontal drag should keep Y constant"
        );
    }

    #[test]
    fn dragging_vertically_changes_y_not_x() {
        let c = cam(Vec3::ZERO, 0.4);
        let anchor = Vec3::ZERO;
        let low = axis_coord(&c, 0.0, -0.3, 1.0, anchor, 1).unwrap();
        let high = axis_coord(&c, 0.0, 0.3, 1.0, anchor, 1).unwrap();
        assert!(
            (high - low).abs() > 0.01,
            "vertical cursor motion must change the Y coordinate, got {low} -> {high}"
        );
        let xa = axis_coord(&c, 0.0, -0.3, 1.0, anchor, 0).unwrap();
        let xb = axis_coord(&c, 0.0, 0.3, 1.0, anchor, 0).unwrap();
        assert!(
            (xa - xb).abs() < 0.01,
            "pure vertical drag should keep X constant"
        );
    }

    #[test]
    fn screen_point_lies_on_the_plane_through_the_anchor() {
        let c = cam(Vec3::new(0.0, 1.0, 2.0), 0.4);
        let anchor = Vec3::new(1.0, 0.0, -3.0);
        let p = screen_plane_point(&c, 0.2, -0.1, 1.6, anchor).expect("intersection");
        let n = camera_forward(&c);
        let d = (p - anchor).dot(n);
        assert!(
            d.abs() < 1e-3,
            "point must lie on the screen plane through the anchor, dot={d}"
        );
    }
}
