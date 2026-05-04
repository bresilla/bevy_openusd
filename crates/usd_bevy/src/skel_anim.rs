//! Pure-logic evaluation of a `UsdSkelAnimDriver` at a given stage
//! time. Returns one local-`Transform` override per animation channel
//! (in the driver's joint order); the caller writes those into the
//! mapped joint entity's `Transform` component each frame.
//!
//! Why a free function instead of a Bevy system: the lib doesn't own
//! the stage-time resource (the viewer crate does). Keeping
//! evaluation pure means custom apps can wire their own playback
//! pipelines (e.g. driving from a `Time<Virtual>`-derived clock or a
//! network-synced timeline) without forking this code.
//!
//! Interpolation:
//! - Translations: linear lerp between bracketing keyframes.
//! - Rotations:    spherical-linear (slerp) — required for visually
//!   correct quaternion interpolation. Linear lerp on quaternion
//!   components produces normalisation drift and visible "shortcut"
//!   artefacts on rotations greater than ~30°.
//! - Scales:       linear lerp.
//!
//! Out-of-range times hold the first / last keyframe (no
//! extrapolation). Channels with empty sample lists fall through to
//! the joint's authored rest transform (we don't write to those).

use bevy::math::{Quat, Vec3};
use bevy::transform::components::Transform;

use crate::prim_ref::UsdSkelAnimDriver;

/// One per animation channel: the lerp/slerp result, or `None` when
/// no samples were authored for that channel kind. Indexed parallel
/// to `UsdSkelAnimDriver::joint_entities`.
#[derive(Debug, Clone, Default)]
pub struct EvaluatedJoint {
    pub translation: Option<Vec3>,
    pub rotation: Option<Quat>,
    pub scale: Option<Vec3>,
}

impl EvaluatedJoint {
    /// Apply this evaluation to a `Transform`. Channels that produced
    /// no value leave the existing Transform component untouched —
    /// authored rest pose stays in place for those.
    pub fn apply(&self, t: &mut Transform) {
        if let Some(v) = self.translation {
            t.translation = v;
        }
        if let Some(q) = self.rotation {
            t.rotation = q;
        }
        if let Some(s) = self.scale {
            t.scale = s;
        }
    }
}

/// Evaluate every animation channel at `time_code`. Returns one
/// `EvaluatedJoint` per channel; caller maps to entities via
/// `driver.joint_entities`.
pub fn evaluate(driver: &UsdSkelAnimDriver, time_code: f64) -> Vec<EvaluatedJoint> {
    let n = driver.joint_entities.len();
    let mut out = vec![EvaluatedJoint::default(); n];

    // Translations.
    if let Some((lo, hi, u)) = bracket(&driver.translations, time_code) {
        let a = &driver.translations[lo].1;
        let b = &driver.translations[hi].1;
        for i in 0..n {
            let av = a.get(i).copied().unwrap_or([0.0, 0.0, 0.0]);
            let bv = b.get(i).copied().unwrap_or(av);
            let v = lerp_vec3(av, bv, u);
            out[i].translation = Some(Vec3::from(v));
        }
    }

    // Rotations — slerp. Element order depends on the authoring
    // tool: Pixar spec says `(w, x, y, z)`, Apple's USDZ exporter
    // writes `(x, y, z, w)`. The driver's `quat_xyzw_order` flag is
    // auto-detected at load time and tells us which.
    if let Some((lo, hi, u)) = bracket(&driver.rotations, time_code) {
        let a = &driver.rotations[lo].1;
        let b = &driver.rotations[hi].1;
        let identity_default = if driver.quat_xyzw_order {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0, 0.0]
        };
        for i in 0..n {
            let aq = a.get(i).copied().unwrap_or(identity_default);
            let bq = b.get(i).copied().unwrap_or(aq);
            let (qa, qb) = if driver.quat_xyzw_order {
                (
                    Quat::from_xyzw(aq[0], aq[1], aq[2], aq[3]),
                    Quat::from_xyzw(bq[0], bq[1], bq[2], bq[3]),
                )
            } else {
                (
                    Quat::from_xyzw(aq[1], aq[2], aq[3], aq[0]),
                    Quat::from_xyzw(bq[1], bq[2], bq[3], bq[0]),
                )
            };
            let q = qa.slerp(qb, u as f32);
            out[i].rotation = Some(q);
        }
    }

    // Scales.
    if let Some((lo, hi, u)) = bracket(&driver.scales, time_code) {
        let a = &driver.scales[lo].1;
        let b = &driver.scales[hi].1;
        for i in 0..n {
            let av = a.get(i).copied().unwrap_or([1.0, 1.0, 1.0]);
            let bv = b.get(i).copied().unwrap_or(av);
            let v = lerp_vec3(av, bv, u);
            out[i].scale = Some(Vec3::from(v));
        }
    }

    out
}

/// Locate the bracketing pair of keyframes for `t`. Returns `(lo,
/// hi, u)` where `u ∈ [0, 1]` is the interpolation parameter
/// (`u = 0` at `samples[lo]`, `u = 1` at `samples[hi]`). Returns
/// `None` when `samples` is empty. Holds endpoints when `t` falls
/// outside the authored range.
fn bracket<T>(samples: &[(f64, T)], t: f64) -> Option<(usize, usize, f64)> {
    if samples.is_empty() {
        return None;
    }
    let last = samples.len() - 1;
    if t <= samples[0].0 {
        return Some((0, 0, 0.0));
    }
    if t >= samples[last].0 {
        return Some((last, last, 0.0));
    }
    // Binary search for the first keyframe >= t.
    let hi = samples
        .binary_search_by(|s| {
            s.0.partial_cmp(&t)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or_else(|i| i);
    let lo = hi.saturating_sub(1);
    let span = samples[hi].0 - samples[lo].0;
    let u = if span <= 0.0 {
        0.0
    } else {
        ((t - samples[lo].0) / span).clamp(0.0, 1.0)
    };
    Some((lo, hi, u))
}

/// Linear-interpolate the blend-shape weights from the driver at
/// `time_code`. Returns one weight per blend-shape channel
/// (parallel to `driver.blend_shape_names`). Out-of-range times
/// hold the first / last keyframe.
pub fn evaluate_blend_shapes(driver: &UsdSkelAnimDriver, time_code: f64) -> Vec<f32> {
    let n = driver.blend_shape_names.len();
    if n == 0 || driver.blend_shape_weights.is_empty() {
        return Vec::new();
    }
    let Some((lo, hi, u)) = bracket(&driver.blend_shape_weights, time_code) else {
        return vec![0.0; n];
    };
    let a = &driver.blend_shape_weights[lo].1;
    let b = &driver.blend_shape_weights[hi].1;
    (0..n)
        .map(|i| {
            let av = a.get(i).copied().unwrap_or(0.0);
            let bv = b.get(i).copied().unwrap_or(av);
            av + (bv - av) * u as f32
        })
        .collect()
}

fn lerp_vec3(a: [f32; 3], b: [f32; 3], u: f64) -> [f32; 3] {
    let u = u as f32;
    [
        a[0] + (b[0] - a[0]) * u,
        a[1] + (b[1] - a[1]) * u,
        a[2] + (b[2] - a[2]) * u,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_holds_endpoints_and_interpolates_middle() {
        let s: Vec<(f64, f32)> = vec![(0.0, 10.0), (10.0, 20.0)];
        assert_eq!(bracket(&s, -1.0), Some((0, 0, 0.0)));
        assert_eq!(bracket(&s, 11.0), Some((1, 1, 0.0)));
        let mid = bracket(&s, 5.0).unwrap();
        assert_eq!(mid.0, 0);
        assert_eq!(mid.1, 1);
        assert!((mid.2 - 0.5).abs() < 1e-9);
    }
}
