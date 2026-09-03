//! Click-to-select by ray vs AABB (spec 24), headless.

use glam::Vec3;
use openengine_ecs::World;

/// Current selection state.
#[derive(Clone, Debug, Default)]
pub struct SelectionModel {
    /// Selected row indices (single-archetype editor world).
    pub selected: Vec<u32>,
    /// Hovered row index, if any.
    pub hovered: Option<u32>,
}

/// Intersect a ray against an axis-aligned box (slab method). Returns the
/// entry distance, or `None` on a miss / behind the camera.
fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
    let mut t0 = (min - origin) * inv;
    let mut t1 = (max - origin) * inv;
    if inv.x < 0.0 {
        std::mem::swap(&mut t0.x, &mut t1.x);
    }
    if inv.y < 0.0 {
        std::mem::swap(&mut t0.y, &mut t1.y);
    }
    if inv.z < 0.0 {
        std::mem::swap(&mut t0.z, &mut t1.z);
    }
    let tmin = t0.x.max(t0.y).max(t0.z);
    let tmax = t1.x.min(t1.y).min(t1.z);
    if tmax < tmin || tmax < 0.0 {
        return None;
    }
    Some(tmin.max(0.0))
}

/// Pick the nearest entity whose unit AABB (Transform.position ± half-extent)
/// the ray hits. `half = |scale|/2` clamped to a minimum so identity-scale
/// entities remain pickable.
pub fn pick(ray_origin: Vec3, ray_dir: Vec3, world: &World) -> Option<u32> {
    let transforms = world.get_transforms()?;
    let n = world.entity_count().min(transforms.len());
    let mut best: Option<(f32, u32)> = None;
    for (i, t) in transforms.iter().take(n).enumerate() {
        let center = Vec3::new(
            t.position[0].to_num(),
            t.position[1].to_num(),
            t.position[2].to_num(),
        );
        let half = Vec3::new(
            t.scale[0].to_num::<f32>().abs() * 0.5,
            t.scale[1].to_num::<f32>().abs() * 0.5,
            t.scale[2].to_num::<f32>().abs() * 0.5,
        )
        .max(Vec3::splat(0.5));
        if let Some(dist) = ray_aabb(ray_origin, ray_dir, center - half, center + half) {
            if best.map(|(d, _)| dist < d).unwrap_or(true) {
                best = Some((dist, i as u32));
            }
        }
    }
    best.map(|(_, i)| i)
}
