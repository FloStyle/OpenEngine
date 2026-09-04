//! Grid + snapping + ground-plane dragging (Unreal-like editor rudiment).
//!
//! A UE user expects to place/move actors on a level that is optionally snapped
//! to a grid. This module holds only the deterministic math (no UI): snap a
//! position to a grid step, and turn a viewport pointer into a grid-snapped
//! point on the ground plane via the camera. Used by the editor gizmo/place UI.

use crate::camera::EditorCamera;
use crate::translate::ray_ground_plane;

/// A square world-space grid the editor snaps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorGrid {
    /// Spacing between grid lines (world units).
    pub step: f32,
}

impl Default for EditorGrid {
    fn default() -> Self {
        EditorGrid { step: 1.0 }
    }
}

impl EditorGrid {
    /// Snap one axis value to the grid.
    pub fn snap_axis(&self, v: f32) -> f32 {
        if self.step <= 0.0 {
            return v;
        }
        (v / self.step).round() * self.step
    }

    /// Snap an XZ position (y preserved) to the grid.
    pub fn snap_xz(&self, p: [f32; 3]) -> [f32; 3] {
        [self.snap_axis(p[0]), p[1], self.snap_axis(p[2])]
    }
}

/// Project a viewport NDC point (-1..1) onto the ground plane (y=0) through the
/// camera, then snap it to `grid`. Returns `None` if the ray is parallel to the
/// ground or points away. This is the Unreal-like "drag an actor across the
/// floor" primitive.
pub fn ground_point_snapped(
    camera: &EditorCamera,
    ndc_x: f32,
    ndc_y: f32,
    aspect: f32,
    grid: &EditorGrid,
) -> Option<[f32; 3]> {
    let (origin, dir) = camera.unproject_ray(ndc_x, ndc_y, aspect);
    let hit = ray_ground_plane(origin, dir)?;
    Some(grid.snap_xz([hit.x, 0.0, hit.z]))
}

/// Snap a full `[x,y,z]` position to the grid (all three axes).
pub fn snap_pos(grid: &EditorGrid, p: [f32; 3]) -> [f32; 3] {
    [
        grid.snap_axis(p[0]),
        grid.snap_axis(p[1]),
        grid.snap_axis(p[2]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn snap_axis_rounds_to_grid() {
        let g = EditorGrid { step: 1.0 };
        assert_eq!(g.snap_axis(0.7), 1.0);
        assert_eq!(g.snap_axis(-0.6), -1.0);
        assert_eq!(g.snap_axis(0.0), 0.0);
        let g2 = EditorGrid { step: 2.0 };
        assert_eq!(g2.snap_axis(2.9), 2.0);
        assert_eq!(g2.snap_axis(1.0), 2.0);
        assert_eq!(g2.snap_axis(-1.4), -2.0);
    }

    #[test]
    fn snap_xz_preserves_y() {
        let g = EditorGrid { step: 1.0 };
        assert_eq!(g.snap_xz([0.4, 5.5, 0.7]), [0.0, 5.5, 1.0]);
    }

    #[test]
    fn ground_point_at_screen_center_tracks_focus() {
        // Camera above the origin looking at it: the center pixel's ground ray
        // passes through the focus, so it hits ~the origin.
        let cam = EditorCamera {
            focus: Vec3::ZERO,
            distance: 20.0,
            yaw: 0.0,
            pitch: 0.4,
            fov: 45f32.to_radians(),
        };
        let g = EditorGrid::default();
        let hit = ground_point_snapped(&cam, 0.0, 0.0, 1.0, &g).expect("hit");
        assert!(
            hit[0].abs() < 0.01,
            "center should hit near origin, got {hit:?}"
        );
        assert!(
            hit[2].abs() < 0.01,
            "center should hit near origin, got {hit:?}"
        );
        assert_eq!(hit[1], 0.0);
    }

    #[test]
    fn off_center_pixels_map_to_symmetric_ground_points() {
        let cam = EditorCamera {
            focus: Vec3::ZERO,
            distance: 20.0,
            yaw: 0.0,
            pitch: 0.4,
            fov: 45f32.to_radians(),
        };
        let g = EditorGrid { step: 0.5 };
        let right = ground_point_snapped(&cam, 0.9, 0.0, 1.0, &g).unwrap();
        let left = ground_point_snapped(&cam, -0.9, 0.0, 1.0, &g).unwrap();
        // Two symmetric pixels must not collapse onto the same snapped point.
        assert!(
            right != left,
            "symmetric off-center pixels must differ on the ground, got {right:?} vs {left:?}"
        );
    }
}
