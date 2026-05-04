//! Boundary conversions between Bevy's f32 glam types and Rapier's
//! f64 glam types. `rapier3d-f64` exposes its math as `glam::DVec3` /
//! `glam::DQuat`, so this is just an f32 ↔ f64 cast.

use bevy::math::{DQuat, DVec3, Quat, Vec3};

#[inline]
pub fn vec3_to_d(v: Vec3) -> DVec3 {
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
}

#[inline]
pub fn vec3_from_d(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

#[inline]
pub fn quat_to_d(q: Quat) -> DQuat {
    DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64)
}

#[inline]
pub fn quat_from_d(q: DQuat) -> Quat {
    Quat::from_xyzw(q.x as f32, q.y as f32, q.z as f32, q.w as f32)
}
