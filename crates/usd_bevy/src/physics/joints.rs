//! `UsdPhysicsJoint` → entries in `PhysicsWorld.{multibody,impulse}_joints`.
//! Bevy ECS adapter; all joint construction lives in `usd_rapier::joints`.

use bevy::prelude::*;
use openusd::physics::{Dof, ReadDrive, ReadJoint, ReadLimit, JointKind};
use rapier3d_f64::prelude::*;
use crate::markers::{
    UsdArticulationRoot, UsdDof, UsdDriveType, UsdJointKind, UsdPhysicsJoint,
};
use usd_rapier::joints::build_and_insert_joint;

use super::world::PhysicsWorld;

#[derive(Component)]
pub(crate) struct JointAttached;

pub fn convert_joints(
    mut commands: Commands,
    mut world: ResMut<PhysicsWorld>,
    joints: Query<(Entity, &UsdPhysicsJoint), Without<JointAttached>>,
    articulations: Query<&UsdArticulationRoot>,
) {
    if joints.is_empty() {
        return;
    }
    let any_articulation = !articulations.is_empty();

    for (joint_entity, joint) in &joints {
        if !joint.joint_enabled {
            commands.entity(joint_entity).insert(JointAttached);
            continue;
        }
        let (Some(body0_e), Some(body1_e)) = (joint.body0, joint.body1) else {
            // World-anchored joint — pin the referenced body to fixed.
            if let Some(target) = joint.body1.or(joint.body0) {
                if let Some(handle) = world.entity_to_body.get(&target).copied() {
                    if let Some(b) = world.bodies.get_mut(handle) {
                        b.set_body_type(RigidBodyType::Fixed, false);
                    }
                }
            }
            commands.entity(joint_entity).insert(JointAttached);
            continue;
        };
        let (Some(body0), Some(body1)) = (
            world.entity_to_body.get(&body0_e).copied(),
            world.entity_to_body.get(&body1_e).copied(),
        ) else {
            // Body entities not yet materialised; try again next frame.
            continue;
        };

        let use_multibody = any_articulation && !joint.exclude_from_articulation;
        let read_joint = bridge_to_read_joint(joint);

        let world_mut = world.as_mut();
        if let Ok(_) = build_and_insert_joint(
            &mut world_mut.multibody_joints,
            &mut world_mut.impulse_joints,
            &read_joint,
            body0,
            body1,
            use_multibody,
        ) {
            commands.entity(joint_entity).insert(JointAttached);
        }
    }
}

/// Bridge a Bevy-component `UsdPhysicsJoint` (Vec3/Quat fields) to
/// the upstream `openusd::physics::ReadJoint` ([f32; 3] / [f32; 4]
/// fields) the `usd_rapier` builder expects.
fn bridge_to_read_joint(j: &UsdPhysicsJoint) -> ReadJoint {
    let (lower, upper) = match j.built_in_limit {
        Some((lo, hi)) => (Some(lo), Some(hi)),
        None => (None, None),
    };
    let axis_token = if j.axis.x.abs() > 0.9 {
        Some("X".into())
    } else if j.axis.y.abs() > 0.9 {
        Some("Y".into())
    } else if j.axis.z.abs() > 0.9 {
        Some("Z".into())
    } else {
        None
    };
    ReadJoint {
        path: String::new(),
        kind: kind_to_openusd(j.kind),
        body0: None,
        body1: None,
        local_pos0: [j.local_pos0.x, j.local_pos0.y, j.local_pos0.z],
        // USD authors quat as (w, x, y, z); Bevy Quat is (x, y, z, w).
        local_rot0: [j.local_rot0.w, j.local_rot0.x, j.local_rot0.y, j.local_rot0.z],
        local_pos1: [j.local_pos1.x, j.local_pos1.y, j.local_pos1.z],
        local_rot1: [j.local_rot1.w, j.local_rot1.x, j.local_rot1.y, j.local_rot1.z],
        axis: axis_token,
        lower_limit: lower,
        upper_limit: upper,
        collision_enabled: j.collision_enabled,
        joint_enabled: j.joint_enabled,
        exclude_from_articulation: j.exclude_from_articulation,
        break_force: j.break_force,
        break_torque: j.break_torque,
        min_distance: j.distance_limit.map(|(lo, _)| lo),
        max_distance: j.distance_limit.map(|(_, hi)| hi),
        cone_angle_0: j.cone_limit.map(|(a, _)| a),
        cone_angle_1: j.cone_limit.map(|(_, b)| b),
        limits: j
            .limits
            .iter()
            .map(|l| ReadLimit {
                dof: dof_to_openusd(l.dof),
                low: l.low,
                high: l.high,
            })
            .collect(),
        drives: j
            .drives
            .iter()
            .map(|d| ReadDrive {
                dof: dof_to_openusd(d.dof),
                drive_type: drive_type_to_openusd(d.drive_type),
                target_position: d.target_position,
                target_velocity: d.target_velocity,
                stiffness: d.stiffness,
                damping: d.damping,
                max_force: d.max_force,
            })
            .collect(),
    }
}

fn kind_to_openusd(k: UsdJointKind) -> JointKind {
    match k {
        UsdJointKind::Fixed => JointKind::Fixed,
        UsdJointKind::Revolute => JointKind::Revolute,
        UsdJointKind::Prismatic => JointKind::Prismatic,
        UsdJointKind::Spherical => JointKind::Spherical,
        UsdJointKind::Distance => JointKind::Distance,
        UsdJointKind::Generic => JointKind::Generic,
    }
}

fn dof_to_openusd(d: UsdDof) -> Dof {
    match d {
        UsdDof::TransX => Dof::TransX,
        UsdDof::TransY => Dof::TransY,
        UsdDof::TransZ => Dof::TransZ,
        UsdDof::RotX => Dof::RotX,
        UsdDof::RotY => Dof::RotY,
        UsdDof::RotZ => Dof::RotZ,
        UsdDof::Linear => Dof::Linear,
        UsdDof::Angular => Dof::Angular,
        UsdDof::Distance => Dof::Distance,
    }
}

fn drive_type_to_openusd(t: UsdDriveType) -> openusd::physics::DriveType {
    match t {
        UsdDriveType::Acceleration => openusd::physics::DriveType::Acceleration,
        UsdDriveType::Force => openusd::physics::DriveType::Force,
    }
}
