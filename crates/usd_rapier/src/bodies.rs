//! Build a Rapier `RigidBody` from authored UsdPhysics rigid-body
//! data + an initial world pose.
//!
//! Mass priority follows the UsdPhysics spec:
//! 1. Explicit `physics:mass` → `MassProperties` on the body itself
//!    so the multibody Featherstone solver has a valid inertia tensor
//!    BEFORE colliders attach (Dynamic body with zero inertia panics
//!    mid-step on the next solver tick).
//! 2. `physics:density` → falls through to the collider's own
//!    mass-from-density (caller wires that on the `ColliderBuilder`).
//! 3. None authored → tiny safety mass + inertia so the body can take
//!    a step before a collider materialises.
//!
//! Returns the inserted `RigidBodyHandle`. Caller maintains any
//! per-host map (entity ↔ handle) externally.

use anyhow::Result;
use glam::{DQuat, DVec3};
use rapier3d_f64::prelude::*;

/// Authored rigid-body opinion ready for Rapier insertion. A subset
/// of `openusd::physics::ReadRigidBody` plus per-body decoded mass.
pub struct RigidBodyOpinion {
    pub kinematic: bool,
    pub enabled: bool,
    pub starts_asleep: bool,
    /// Initial world position.
    pub world_translation: DVec3,
    pub world_rotation: DQuat,
    pub linvel: DVec3,
    pub angvel: DVec3,
    pub mass: Option<f64>,
    pub center_of_mass: Option<DVec3>,
    pub diagonal_inertia: Option<DVec3>,
    pub principal_axes: Option<DQuat>,
}

impl RigidBodyOpinion {
    /// Identity-pose dynamic body with no authored mass / velocity —
    /// useful starting point when the host has only the world pose.
    pub fn dynamic_at(world_translation: DVec3, world_rotation: DQuat) -> Self {
        Self {
            kinematic: false,
            enabled: true,
            starts_asleep: false,
            world_translation,
            world_rotation,
            linvel: DVec3::ZERO,
            angvel: DVec3::ZERO,
            mass: None,
            center_of_mass: None,
            diagonal_inertia: None,
            principal_axes: None,
        }
    }
}

/// Insert a rigid body into `bodies` from an authored opinion.
///
/// The principal-axes quat is currently ignored by Rapier's mass
/// properties (Rapier expects the body's local frame to be the
/// principal-axes frame); upstream USD authoring rarely needs it.
/// `user_data` is forwarded to `RigidBodyBuilder::user_data` so the
/// host can tag the body with anything addressable as a `u128`
/// (Bevy entity bits, gearbox part id, …).
pub fn build_rigid_body(
    bodies: &mut RigidBodySet,
    op: &RigidBodyOpinion,
    user_data: u128,
) -> Result<RigidBodyHandle> {
    let body_type = if !op.enabled {
        RigidBodyType::Fixed
    } else if op.kinematic {
        RigidBodyType::KinematicPositionBased
    } else {
        RigidBodyType::Dynamic
    };

    let mut builder = RigidBodyBuilder::new(body_type)
        .position(Pose {
            translation: op.world_translation,
            rotation: op.world_rotation,
        })
        .linvel(op.linvel)
        .angvel(op.angvel);

    if op.starts_asleep {
        builder = builder.sleeping(true);
    }

    // Real robot joints have gearbox / bearing friction. Without
    // damping a hanging articulation chain oscillates forever; USD
    // has no body-damping schema so apply sane defaults that don't
    // freeze drive wheels.
    if matches!(body_type, RigidBodyType::Dynamic) {
        builder = builder.linear_damping(0.1).angular_damping(0.5);
    }

    match op.mass {
        Some(mass_kg) => {
            let inertia = op
                .diagonal_inertia
                .unwrap_or(DVec3::splat(0.4 * mass_kg * 0.01));
            let com = op.center_of_mass.unwrap_or(DVec3::ZERO);
            builder = builder.additional_mass_properties(MassProperties::new(com, mass_kg, inertia));
        }
        None if matches!(body_type, RigidBodyType::Dynamic) => {
            // Tiny safety mass + inertia so a dynamic body without a
            // collider can step before any collider materialises.
            builder = builder.additional_mass_properties(MassProperties::new(
                DVec3::ZERO,
                0.001,
                DVec3::splat(0.0001),
            ));
        }
        _ => {}
    }

    builder = builder.user_data(user_data);
    Ok(bodies.insert(builder.build()))
}
