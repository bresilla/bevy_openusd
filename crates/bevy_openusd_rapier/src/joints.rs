//! `UsdPhysicsJoint` → Rapier `ImpulseJoint` / `MultibodyJoint`
//! routing.
//!
//! Routing rule: a joint is built as a [`MultibodyJoint`] when both
//! its bodies sit inside the same [`UsdArticulationRoot`]'s
//! `joints` list AND the joint isn't `exclude_from_articulation`.
//! Everything else uses [`ImpulseJoint`]. `MultibodyJoint` solves in
//! reduced coordinates (Featherstone), which is the standard for
//! robotics chains — accurate joint constraints, no drift, stable
//! at high mass ratios.

use bevy::prelude::*;
use usd_physics_markers::{UsdArticulationRoot, UsdJointKind, UsdPhysicsJoint};
use bevy_rapier3d::dynamics::{
    FixedJointBuilder, GenericJointBuilder, ImpulseJoint, JointAxesMask, JointAxis,
    MultibodyJoint, RigidBody, SphericalJointBuilder, TypedJoint,
};

/// Marker so we don't re-spawn joints across ticks. Inserted on the
/// joint prim's entity once we've materialised it.
#[derive(Component)]
pub(crate) struct JointConsumed;

pub fn convert_joints(
    mut commands: Commands,
    joints: Query<(Entity, &UsdPhysicsJoint), (Added<UsdPhysicsJoint>, Without<JointConsumed>)>,
    articulations: Query<&UsdArticulationRoot>,
) {
    // Pragmatic routing: if any `UsdArticulationRoot` exists in the
    // scene, treat ALL joints as part of an articulation (MultibodyJoint
    // / reduced-coord solver). Subtree-based joint collection in the
    // projection's `populate_articulation_joints` is unreliable for
    // assets that put `PhysicsArticulationRootAPI` on the Fixed
    // root_joint prim (Isaac Sim convention) instead of an enclosing
    // Xform. The body graph is the real articulation tree anyway, so
    // erring on the side of MultibodyJoint gives the right behaviour
    // for kinematic chains. Loop-closing joints opt out via
    // `physics:excludeFromArticulation`.
    if joints.is_empty() {
        return;
    }
    let any_articulation = !articulations.is_empty();

    for (joint_entity, joint) in &joints {
        if !joint.joint_enabled {
            commands.entity(joint_entity).insert(JointConsumed);
            continue;
        }
        let (body0_opt, body1_opt) = (joint.body0, joint.body1);
        let (Some(body0), Some(body1)) = (body0_opt, body1_opt) else {
            // World-anchored joint (one rel is None). The standard
            // Isaac Sim / URDF convention uses a joint with
            // `body0=None body1=base_link` to pin the base of an
            // articulation to the world. Whether it's authored as
            // Fixed (Carter, agilebot), Generic (Franka), or even
            // Revolute (a fixed pivot to the world), the practical
            // intent is "this body doesn't move". Promote the
            // referenced body to `RigidBody::Fixed`. This loses the
            // joint's degree of freedom — for true world-anchored
            // revolutes we'd need a hidden static anchor body, but
            // the vast majority of authored world-anchored joints
            // are "pin the base," which Fixed handles correctly.
            if let Some(target) = body1_opt.or(body0_opt) {
                commands.entity(target).insert(RigidBody::Fixed);
            }
            commands.entity(joint_entity).insert(JointConsumed);
            continue;
        };

        let Some(typed) = build_typed_joint(joint) else {
            commands.entity(joint_entity).insert(JointConsumed);
            continue;
        };

        // Rapier joint components attach to the CHILD body and reference
        // the parent. Convention: body0 = parent, body1 = child.
        let use_multibody = any_articulation && !joint.exclude_from_articulation;
        if use_multibody {
            commands
                .entity(body1)
                .insert(MultibodyJoint::new(body0, typed));
        } else {
            commands
                .entity(body1)
                .insert(ImpulseJoint::new(body0, typed));
        }
        commands.entity(joint_entity).insert(JointConsumed);
    }
}

fn build_typed_joint(j: &UsdPhysicsJoint) -> Option<TypedJoint> {
    match j.kind {
        UsdJointKind::Fixed => {
            let b = FixedJointBuilder::new()
                .local_anchor1(j.local_pos0)
                .local_anchor2(j.local_pos1);
            let mut joint = b.build();
            joint.data.set_contacts_enabled(false);
            Some(joint.into())
        }
        UsdJointKind::Revolute => Some(TypedJoint::GenericJoint(build_axis_joint(
            j,
            AxisJointKind::Revolute,
        ))),
        UsdJointKind::Prismatic => Some(TypedJoint::GenericJoint(build_axis_joint(
            j,
            AxisJointKind::Prismatic,
        ))),
        UsdJointKind::Spherical => {
            // Rapier's SphericalJoint is a ball-and-socket; cone limits
            // (USD's `coneAngle0/1Limit`) aren't directly expressible on
            // the basic spherical joint, so we drop them with a warning.
            // Generic D6 (below) is the workaround when cone limits
            // matter.
            if j.cone_limit.is_some() {
                debug!(
                    "RapierAdapter: spherical joint cone limits not directly supported; \
                     consider authoring a Generic joint with multi-DOF LimitAPI"
                );
            }
            let b = SphericalJointBuilder::new()
                .local_anchor1(j.local_pos0)
                .local_anchor2(j.local_pos1);
            let mut joint = b.build();
            joint.data.set_contacts_enabled(false);
            Some(joint.into())
        }
        UsdJointKind::Distance => {
            // No native distance joint in current bevy_rapier; emulate
            // with a prismatic pinned at the desired range, or skip.
            warn!(
                "RapierAdapter: PhysicsDistanceJoint is not yet supported; skipping joint"
            );
            None
        }
        UsdJointKind::Generic => {
            // Generic joints come from the multi-apply LimitAPI/DriveAPI
            // path. Building the full GenericJointBuilder with per-DOF
            // configuration is a follow-up — until then, skip with a
            // hint.
            warn!(
                "RapierAdapter: generic D6 joint translation is not yet implemented; \
                 the joint has {} limits and {} drives that will be ignored",
                j.limits.len(),
                j.drives.len()
            );
            None
        }
    }
}

#[derive(Copy, Clone)]
enum AxisJointKind {
    Revolute,
    Prismatic,
}

/// Build a Rapier revolute/prismatic joint as a `GenericJoint` with
/// full per-body basis rotations.
///
/// USD authors a joint via `localPos0/Rot0` and `localPos1/Rot1` — the
/// joint's coordinate system expressed in each body. The free axis
/// (`physics:axis`) lives in that joint frame.
///
/// Rapier's native `RevoluteJointBuilder::new(axis)` takes a single
/// axis vector and assumes axis1 == axis2 in body-frames. That holds
/// only when `localRot0 == localRot1`. Isaac Sim's robot chains
/// rotate 90° between consecutive links (the joint frame is shared
/// but reaches each body via different rotations), so passing a
/// single axis makes Rapier solve "twist body1 to align with body0,"
/// scattering the chain on every step.
///
/// Doing it correctly: hand Rapier the FULL local basis on each
/// side. `local_basis1 = localRot0 * axis_remap`, where `axis_remap`
/// is the rotation that maps Rapier's principal X-axis onto USD's
/// `axis`. That keeps Y and Z axes consistent between the two body
/// frames so the locked perpendicular DOFs don't fight at rest.
fn build_axis_joint(j: &UsdPhysicsJoint, kind: AxisJointKind) -> bevy_rapier3d::dynamics::GenericJoint {
    let axis_remap = Quat::from_rotation_arc(Vec3::X, j.axis.normalize_or_zero());
    let basis1 = j.local_rot0 * axis_remap;
    let basis2 = j.local_rot1 * axis_remap;

    let (locked_axes, motor_axis) = match kind {
        AxisJointKind::Revolute => (JointAxesMask::LOCKED_REVOLUTE_AXES, JointAxis::AngX),
        AxisJointKind::Prismatic => (JointAxesMask::LOCKED_PRISMATIC_AXES, JointAxis::LinX),
    };

    let mut b = GenericJointBuilder::new(locked_axes)
        .local_basis1(basis1)
        .local_basis2(basis2)
        .local_anchor1(j.local_pos0)
        .local_anchor2(j.local_pos1);

    if let Some((lo, hi)) = j.built_in_limit {
        b = b.limits(motor_axis, [lo, hi]);
    }

    let dof_matches: fn(usd_physics_markers::UsdDof) -> bool = match kind {
        AxisJointKind::Revolute => dof_matches_revolute,
        AxisJointKind::Prismatic => dof_matches_prismatic,
    };
    if let Some(d) = j.drives.iter().find(|d| dof_matches(d.dof)) {
        if let Some(target) = d.target_position {
            b = b.motor_position(motor_axis, target, d.stiffness, d.damping);
        } else if let Some(vel) = d.target_velocity {
            b = b.motor_velocity(motor_axis, vel, d.damping);
        }
        if let Some(max) = d.max_force {
            b = b.motor_max_force(motor_axis, max);
        }
    }
    // Joint friction lives on the body's `Damping` component, not as
    // a motor. Velocity-target motors push back against the slightest
    // velocity noise from the solver, which can create their own
    // feedback jitter — preferring body damping kept agilebot from
    // jumping at the wrist link.

    let mut joint = b.build();
    // Disable contacts between the two bodies of this joint. Industrial
    // robot collision meshes overlap at the joint pivot (link_n's
    // distal cap and link_{n+1}'s proximal cap occupy the same volume),
    // and without this Rapier generates contact impulses every step
    // that pop the chain links apart in discrete jumps. PhysX
    // (`PxArticulation::setSelfCollisions(false)`) and Newton both
    // disable inter-link collision by default for articulations; do
    // the same here.
    joint.set_contacts_enabled(false);
    joint
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
