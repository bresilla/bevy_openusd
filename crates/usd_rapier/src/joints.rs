//! Build a Rapier `MultibodyJoint` or `ImpulseJoint` from authored
//! UsdPhysics joint data.
//!
//! Routing:
//! - `use_multibody = true` → insert into `MultibodyJointSet` (Featherstone
//!   reduced-coordinate solver — what's used for robotic chains).
//! - Otherwise insert into `ImpulseJointSet` (soft constraint).
//! - On a multibody insert that fails (loop closure), the joint
//!   automatically falls back to `ImpulseJoint`.
//!
//! Same-basis vs differing-basis routing for revolute / prismatic:
//! - `localRot0 == localRot1` (the common case) → native typed
//!   `RevoluteJointBuilder` / `PrismaticJointBuilder`. Rapier's
//!   articulation solver expects these (the GenericJoint-only path
//!   tickled an `index out of bounds` panic in 0.31).
//! - `localRot0 != localRot1` → fall back to `GenericJoint` with
//!   full per-body bases (Isaac Sim's 90°-rotated chains).

use anyhow::Result;
use glam::{DQuat, DVec3};
use openusd::physics::{Dof, JointKind, ReadDrive, ReadJoint};
use rapier3d_f64::prelude::*;

/// Insert the joint into the appropriate Rapier set. `body0`/`body1`
/// are already-resolved Rapier handles; the caller has converted
/// USD prim path relations into handles via its own map.
///
/// Returns:
/// - `Ok(Some(MultibodyJointHandle))` when inserted into `multibody_joints`
/// - `Ok(None)` for `ImpulseJoint` insertions (host can re-query if needed)
pub fn build_and_insert_joint(
    multibody_joints: &mut MultibodyJointSet,
    impulse_joints: &mut ImpulseJointSet,
    joint: &ReadJoint,
    body0: RigidBodyHandle,
    body1: RigidBodyHandle,
    use_multibody: bool,
) -> Result<Option<MultibodyJointHandle>> {
    if !joint.joint_enabled {
        return Ok(None);
    }
    let local_pos0 = vec3_array_to_d(joint.local_pos0);
    let local_pos1 = vec3_array_to_d(joint.local_pos1);
    let local_rot0 = quat_wxyz_to_d(joint.local_rot0);
    let local_rot1 = quat_wxyz_to_d(joint.local_rot1);

    match joint.kind {
        JointKind::Revolute | JointKind::Prismatic => insert_axis_joint(
            multibody_joints,
            impulse_joints,
            joint,
            body0,
            body1,
            use_multibody,
            local_pos0,
            local_pos1,
            local_rot0,
            local_rot1,
        ),
        JointKind::Fixed => {
            let joint = FixedJointBuilder::new()
                .local_anchor1(local_pos0)
                .local_anchor2(local_pos1)
                .build();
            Ok(insert_generic(
                multibody_joints,
                impulse_joints,
                body0,
                body1,
                joint.into(),
                use_multibody,
            ))
        }
        JointKind::Spherical => {
            let joint = SphericalJointBuilder::new()
                .local_anchor1(local_pos0)
                .local_anchor2(local_pos1)
                .build();
            Ok(insert_generic(
                multibody_joints,
                impulse_joints,
                body0,
                body1,
                joint.into(),
                use_multibody,
            ))
        }
        JointKind::Distance => {
            log::warn!("usd_rapier: PhysicsDistanceJoint not yet supported; skipping");
            Ok(None)
        }
        JointKind::Generic => {
            log::warn!(
                "usd_rapier: generic D6 joint not yet implemented; skipping ({} limits, {} drives)",
                joint.limits.len(),
                joint.drives.len()
            );
            Ok(None)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_axis_joint(
    multibody_joints: &mut MultibodyJointSet,
    impulse_joints: &mut ImpulseJointSet,
    j: &ReadJoint,
    body0: RigidBodyHandle,
    body1: RigidBodyHandle,
    use_multibody: bool,
    local_pos0: DVec3,
    local_pos1: DVec3,
    local_rot0: DQuat,
    local_rot1: DQuat,
) -> Result<Option<MultibodyJointHandle>> {
    let axis_str = j.axis.as_deref().unwrap_or("X");
    let axis = match axis_str {
        "Y" => DVec3::Y,
        "Z" => DVec3::Z,
        _ => DVec3::X,
    };
    let same_basis = local_rot0.abs_diff_eq(local_rot1, 1e-4);
    let world_axis: DVec3 = (local_rot0 * axis).normalize();

    // Build a `GenericJoint` regardless of branch — every typed joint
    // converts via `Into<GenericJoint>` and Rapier's `JointSet::insert`
    // accepts either, so we don't need a wrapper enum like
    // bevy_rapier3d's `TypedJoint`.
    let generic: GenericJoint = if same_basis {
        match j.kind {
            JointKind::Revolute => {
                let mut b = RevoluteJointBuilder::new(world_axis)
                    .local_anchor1(local_pos0)
                    .local_anchor2(local_pos1);
                if let (Some(lo), Some(hi)) = (j.lower_limit, j.upper_limit) {
                    b = b.limits([lo as f64, hi as f64]);
                }
                if let Some(d) = j.drives.iter().find(|d| dof_matches_revolute(d.dof)) {
                    b = b.motor_model(MotorModel::ForceBased);
                    b = apply_drive_revolute(b, d);
                }
                let mut joint = b.build();
                joint.set_contacts_enabled(false);
                joint.into()
            }
            JointKind::Prismatic => {
                let mut b = PrismaticJointBuilder::new(world_axis)
                    .local_anchor1(local_pos0)
                    .local_anchor2(local_pos1);
                if let (Some(lo), Some(hi)) = (j.lower_limit, j.upper_limit) {
                    b = b.limits([lo as f64, hi as f64]);
                }
                if let Some(d) = j.drives.iter().find(|d| dof_matches_prismatic(d.dof)) {
                    b = b.motor_model(MotorModel::ForceBased);
                    b = apply_drive_prismatic(b, d);
                }
                let mut joint = b.build();
                joint.set_contacts_enabled(false);
                joint.into()
            }
            _ => unreachable!(),
        }
    } else {
        // Generic-D6 fallback for chains where local_rot0 != local_rot1.
        let axis_remap_quat = DQuat::from_rotation_arc(DVec3::X, axis);
        let basis1 = local_rot0 * axis_remap_quat;
        let basis2 = local_rot1 * axis_remap_quat;
        let (locked_axes, motor_axis) = match j.kind {
            JointKind::Revolute => (JointAxesMask::LOCKED_REVOLUTE_AXES, JointAxis::AngX),
            JointKind::Prismatic => (JointAxesMask::LOCKED_PRISMATIC_AXES, JointAxis::LinX),
            _ => unreachable!(),
        };
        let frame1 = Pose {
            rotation: basis1,
            translation: local_pos0,
        };
        let frame2 = Pose {
            rotation: basis2,
            translation: local_pos1,
        };
        let mut b = GenericJointBuilder::new(locked_axes)
            .local_frame1(frame1)
            .local_frame2(frame2);
        if let (Some(lo), Some(hi)) = (j.lower_limit, j.upper_limit) {
            b = b.limits(motor_axis, [lo as f64, hi as f64]);
        }
        let dof_match: fn(Dof) -> bool = match j.kind {
            JointKind::Revolute => dof_matches_revolute,
            _ => dof_matches_prismatic,
        };
        if let Some(d) = j.drives.iter().find(|d| dof_match(d.dof)) {
            if let Some(target) = d.target_position {
                b = b.motor_position(
                    motor_axis,
                    target as f64,
                    d.stiffness as f64,
                    d.damping as f64,
                );
            } else if let Some(vel) = d.target_velocity {
                b = b.motor_velocity(motor_axis, vel as f64, d.damping as f64);
            }
            if let Some(max) = d.max_force {
                b = b.motor_max_force(motor_axis, max as f64);
            }
        }
        let mut joint = b.build();
        joint.set_contacts_enabled(false);
        joint
    };

    Ok(insert_generic(
        multibody_joints,
        impulse_joints,
        body0,
        body1,
        generic,
        use_multibody,
    ))
}

fn insert_generic(
    multibody_joints: &mut MultibodyJointSet,
    impulse_joints: &mut ImpulseJointSet,
    body0: RigidBodyHandle,
    body1: RigidBodyHandle,
    joint: GenericJoint,
    use_multibody: bool,
) -> Option<MultibodyJointHandle> {
    if use_multibody {
        if let Some(h) = multibody_joints.insert(body0, body1, joint, true) {
            return Some(h);
        }
        log::warn!("usd_rapier: multibody insert failed (loop?); falling to impulse");
        impulse_joints.insert(body0, body1, joint, true);
        None
    } else {
        impulse_joints.insert(body0, body1, joint, true);
        None
    }
}

fn apply_drive_revolute(b: RevoluteJointBuilder, d: &ReadDrive) -> RevoluteJointBuilder {
    let mut b = b;
    if let Some(target) = d.target_position {
        b = b.motor_position(target as f64, d.stiffness as f64, d.damping as f64);
    } else if let Some(vel) = d.target_velocity {
        b = b.motor_velocity(vel as f64, d.damping as f64);
    }
    if let Some(max) = d.max_force {
        b = b.motor_max_force(max as f64);
    }
    b
}

fn apply_drive_prismatic(b: PrismaticJointBuilder, d: &ReadDrive) -> PrismaticJointBuilder {
    let mut b = b;
    if let Some(target) = d.target_position {
        b = b.motor_position(target as f64, d.stiffness as f64, d.damping as f64);
    } else if let Some(vel) = d.target_velocity {
        b = b.motor_velocity(vel as f64, d.damping as f64);
    }
    if let Some(max) = d.max_force {
        b = b.motor_max_force(max as f64);
    }
    b
}

fn dof_matches_revolute(dof: Dof) -> bool {
    matches!(dof, Dof::Angular | Dof::RotX | Dof::RotY | Dof::RotZ)
}

fn dof_matches_prismatic(dof: Dof) -> bool {
    matches!(dof, Dof::Linear | Dof::TransX | Dof::TransY | Dof::TransZ)
}

fn vec3_array_to_d(v: [f32; 3]) -> DVec3 {
    DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)
}

fn quat_wxyz_to_d(q: [f32; 4]) -> DQuat {
    // USD authors as (w, x, y, z); glam DQuat::from_xyzw expects (x, y, z, w).
    DQuat::from_xyzw(q[1] as f64, q[2] as f64, q[3] as f64, q[0] as f64)
}
