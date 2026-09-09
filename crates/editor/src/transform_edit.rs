//! Transform-tool math for the editor (Unreal Rotate E / Scale R) — headless.
//!
//! Produces new fixed-point `Transform`s from drag intent. The UI layer maps a
//! pixel drag to a yaw/scale amount and calls these; snapping is the caller's
//! concern.

use openengine_contracts::Transform;
use openengine_math::I16F16 as F;

fn f(v: f32) -> F {
    F::from_num(v)
}

/// Map a horizontal pixel delta to a yaw angle (radians) with a sensitivity.
/// Monotonic so dragging left/right maps cleanly.
pub fn drag_to_yaw(ndc_dx: f32, sensitivity: f32) -> f32 {
    ndc_dx * sensitivity
}

/// Map a drag to a *factor* for uniform scale, clamped to stay positive.
pub fn drag_to_scale(ndc_dx: f32, per_unit: f32) -> f32 {
    (1.0 + ndc_dx * per_unit).max(0.05)
}

/// Rotate `t` by `yaw` radians about the world Y axis (replaces rotation with a
/// pure Y-axis quaternion). `yaw` is in radians.
pub fn rotate_yaw(mut t: Transform, yaw: f32) -> Transform {
    let (s, c) = (0.5f32 * yaw).sin_cos();
    t.rotation = [f(s), f(0.0), f(0.0), f(c)];
    t
}

/// Uniformly scale `t` by `factor` (clamped positive).
pub fn scale_uniform(mut t: Transform, factor: f32) -> Transform {
    let k = factor.max(0.05);
    for s in t.scale.iter_mut() {
        let cur = s.to_num::<f32>();
        *s = f(cur * k);
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_math::I16F16 as F;

    fn t_at() -> Transform {
        let mut t = Transform::at(F::from_num(0), F::from_num(0), F::from_num(0));
        // default scale is 1; reset rotation to identity explicitly.
        t.rotation = [
            F::from_num(0),
            F::from_num(0),
            F::from_num(0),
            F::from_num(1),
        ];
        t
    }

    #[test]
    fn drag_to_yaw_is_monotonic() {
        assert!(drag_to_yaw(1.0, 1.0) > drag_to_yaw(0.0, 1.0));
        assert!(drag_to_yaw(-1.0, 1.0) < 0.0);
    }

    #[test]
    fn drag_to_scale_stays_positive() {
        assert!(drag_to_scale(-100.0, 0.1) > 0.0);
        assert!(drag_to_scale(0.0, 0.1).abs() - 1.0 < 1e-6);
    }

    #[test]
    fn rotate_yaw_changes_only_rotation() {
        let t0 = t_at();
        let t1 = rotate_yaw(t0, 0.5);
        assert!(t1.rotation[3].to_num::<f32>() < 1.0, "cos(0.25) < 1");
        assert!(
            t1.rotation[0].to_num::<f32>().abs() > 0.0,
            "sin component set"
        );
        assert_eq!(t1.position, t0.position, "rotation must not move position");
    }

    #[test]
    fn scale_uniform_multiplies() {
        let t0 = t_at();
        let t1 = scale_uniform(t0, 2.0);
        for i in 0..3 {
            assert!(
                (t1.scale[i].to_num::<f32>() - 2.0).abs() < 1e-3,
                "scale axis {i} = 2"
            );
        }
    }
}
