//! `UsdPhysicsJoint` → entries in `PhysicsWorld.multibody_joints` or
//! `PhysicsWorld.impulse_joints` depending on articulation routing.
//!
//! Routing: when a `UsdArticulationRoot` exists in the scene, all
//! joints default to `MultibodyJoint` (Featherstone reduced-coord
//! solver) unless they're flagged `excludeFromArticulation` —
//! those fall back to `ImpulseJoint`. Joints with no articulation
//! root anywhere are always `ImpulseJoint`.

use bevy::math::{DVec3, Vec3};
use bevy::prelude::*;
use rapier3d_f64::prelude::*;
use usd_physics_markers::{UsdArticulationRoot, UsdJointKind, UsdPhysicsJoint};

use crate::convert::{quat_to_d, vec3_to_d};
use crate::world::PhysicsWorld;

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
                        b.set_body_type(rapier3d_f64::dynamics::RigidBodyType::Fixed, false);
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

        match joint.kind {
            UsdJointKind::Revolute | UsdJointKind::Prismatic => {
                let inserted = build_and_insert_axis_joint(
                    joint, body0, body1, use_multibody, &mut world,
                );
                if !inserted {
                    continue;
                }
            }
            UsdJointKind::Fixed => {
                let typed = FixedJointBuilder::new()
                    .local_anchor1(vec3_to_d(joint.local_pos0))
                    .local_anchor2(vec3_to_d(joint.local_pos1))
                    .build();
                if use_multibody {
                    world.multibody_joints.insert(body0, body1, typed, true);
                } else {
                    world.impulse_joints.insert(body0, body1, typed, true);
                }
            }
            UsdJointKind::Spherical => {
                let typed = SphericalJointBuilder::new()
                    .local_anchor1(vec3_to_d(joint.local_pos0))
                    .local_anchor2(vec3_to_d(joint.local_pos1))
                    .build();
                if use_multibody {
                    world.multibody_joints.insert(body0, body1, typed, true);
                } else {
                    world.impulse_joints.insert(body0, body1, typed, true);
                }
            }
            UsdJointKind::Distance => {
                warn!("RapierAdapter: PhysicsDistanceJoint not yet supported; skipping");
            }
            UsdJointKind::Generic => {
                warn!(
                    "RapierAdapter: generic D6 joint not yet implemented; skipping ({} limits, {} drives)",
                    joint.limits.len(),
                    joint.drives.len()
                );
            }
        }

        commands.entity(joint_entity).insert(JointAttached);
    }
}

/// Build a Revolute or Prismatic. Same-basis case uses the native
/// typed builder; differing-basis case falls back to GenericJoint
/// with full per-body bases (Isaac Sim 90°-rotated chains).
///
/// Returns `false` if the joint couldn't be constructed.
fn build_and_insert_axis_joint(
    j: &UsdPhysicsJoint,
    body0: rapier3d_f64::dynamics::RigidBodyHandle,
    body1: rapier3d_f64::dynamics::RigidBodyHandle,
    use_multibody: bool,
    world: &mut PhysicsWorld,
) -> bool {
    let axis = j.axis.normalize_or(Vec3::X);
    let same_basis = j.local_rot0.abs_diff_eq(j.local_rot1, 1e-4);
    // Rapier-f64 takes axes as DVec3 (no UnitVector wrapper).
    let world_axis: DVec3 = vec3_to_d((j.local_rot0 * axis).normalize());

    if same_basis {
        match j.kind {
            UsdJointKind::Revolute => {
                let mut b = RevoluteJointBuilder::new(world_axis)
                    .local_anchor1(vec3_to_d(j.local_pos0))
                    .local_anchor2(vec3_to_d(j.local_pos1));
                if let Some((lo, hi)) = j.built_in_limit {
                    b = b.limits([lo as f64, hi as f64]);
                }
                if let Some(d) = j.drives.iter().find(|d| dof_matches_revolute(d.dof)) {
                    b = b.motor_model(MotorModel::ForceBased);
                    if let Some(target) = d.target_position {
                        b = b.motor_position(target as f64, d.stiffness as f64, d.damping as f64);
                    } else if let Some(vel) = d.target_velocity {
                        b = b.motor_velocity(vel as f64, d.damping as f64);
                    }
                    if let Some(max) = d.max_force {
                        b = b.motor_max_force(max as f64);
                    }
                }
                let mut joint = b.build();
                joint.set_contacts_enabled(false);
                if use_multibody {
                    if world.multibody_joints.insert(body0, body1, joint, true).is_none() {
                        warn!("RapierAdapter: multibody joint insert failed (loop?); falling to impulse");
                        world.impulse_joints.insert(body0, body1, joint, true);
                    }
                } else {
                    world.impulse_joints.insert(body0, body1, joint, true);
                }
                true
            }
            UsdJointKind::Prismatic => {
                let mut b = PrismaticJointBuilder::new(world_axis)
                    .local_anchor1(vec3_to_d(j.local_pos0))
                    .local_anchor2(vec3_to_d(j.local_pos1));
                if let Some((lo, hi)) = j.built_in_limit {
                    b = b.limits([lo as f64, hi as f64]);
                }
                if let Some(d) = j.drives.iter().find(|d| dof_matches_prismatic(d.dof)) {
                    b = b.motor_model(MotorModel::ForceBased);
                    if let Some(target) = d.target_position {
                        b = b.motor_position(target as f64, d.stiffness as f64, d.damping as f64);
                    } else if let Some(vel) = d.target_velocity {
                        b = b.motor_velocity(vel as f64, d.damping as f64);
                    }
                    if let Some(max) = d.max_force {
                        b = b.motor_max_force(max as f64);
                    }
                }
                let mut joint = b.build();
                joint.set_contacts_enabled(false);
                if use_multibody {
                    if world.multibody_joints.insert(body0, body1, joint, true).is_none() {
                        world.impulse_joints.insert(body0, body1, joint, true);
                    }
                } else {
                    world.impulse_joints.insert(body0, body1, joint, true);
                }
                true
            }
            _ => false,
        }
    } else {
        // Generic-D6 fallback for chains where local_rot0 != local_rot1.
        let axis_remap_quat = Quat::from_rotation_arc(Vec3::X, axis);
        let basis1 = quat_to_d(j.local_rot0 * axis_remap_quat);
        let basis2 = quat_to_d(j.local_rot1 * axis_remap_quat);
        let (locked_axes, motor_axis) = match j.kind {
            UsdJointKind::Revolute => (JointAxesMask::LOCKED_REVOLUTE_AXES, JointAxis::AngX),
            UsdJointKind::Prismatic => (JointAxesMask::LOCKED_PRISMATIC_AXES, JointAxis::LinX),
            _ => return false,
        };
        let frame1 = Pose {
            rotation: basis1,
            translation: vec3_to_d(j.local_pos0),
        };
        let frame2 = Pose {
            rotation: basis2,
            translation: vec3_to_d(j.local_pos1),
        };
        let mut b = GenericJointBuilder::new(locked_axes)
            .local_frame1(frame1)
            .local_frame2(frame2);
        if let Some((lo, hi)) = j.built_in_limit {
            b = b.limits(motor_axis, [lo as f64, hi as f64]);
        }
        let dof_match: fn(usd_physics_markers::UsdDof) -> bool = match j.kind {
            UsdJointKind::Revolute => dof_matches_revolute,
            _ => dof_matches_prismatic,
        };
        if let Some(d) = j.drives.iter().find(|d| dof_match(d.dof)) {
            if let Some(target) = d.target_position {
                b = b.motor_position(motor_axis, target as f64, d.stiffness as f64, d.damping as f64);
            } else if let Some(vel) = d.target_velocity {
                b = b.motor_velocity(motor_axis, vel as f64, d.damping as f64);
            }
            if let Some(max) = d.max_force {
                b = b.motor_max_force(motor_axis, max as f64);
            }
        }
        let mut joint = b.build();
        joint.set_contacts_enabled(false);
        if use_multibody {
            if world.multibody_joints.insert(body0, body1, joint, true).is_none() {
                world.impulse_joints.insert(body0, body1, joint, true);
            }
        } else {
            world.impulse_joints.insert(body0, body1, joint, true);
        }
        true
    }
}

fn dof_matches_revolute(dof: usd_physics_markers::UsdDof) -> bool {
    matches!(
        dof,
        usd_physics_markers::UsdDof::Angular
            | usd_physics_markers::UsdDof::RotX
            | usd_physics_markers::UsdDof::RotY
            | usd_physics_markers::UsdDof::RotZ
    )
}

fn dof_matches_prismatic(dof: usd_physics_markers::UsdDof) -> bool {
    matches!(
        dof,
        usd_physics_markers::UsdDof::Linear
            | usd_physics_markers::UsdDof::TransX
            | usd_physics_markers::UsdDof::TransY
            | usd_physics_markers::UsdDof::TransZ
    )
}
