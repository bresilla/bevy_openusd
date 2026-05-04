//! `UsdRigidBody` + `UsdMass` → entries in `PhysicsWorld.bodies`,
//! using `usd_rapier::bodies::build_rigid_body` for the actual
//! Rapier construction. This file is the Bevy ECS adapter only.

use bevy::prelude::*;
use crate::markers::{UsdMass, UsdRigidBody};
use usd_rapier::bodies::{build_rigid_body, RigidBodyOpinion};

use super::convert::{quat_to_d, vec3_to_d};
use super::world::PhysicsWorld;

/// Marker on entities whose `UsdRigidBody` we've already inserted
/// into `PhysicsWorld.bodies`. Lets the system stay idempotent.
#[derive(Component)]
pub(crate) struct BodyAttached;

pub fn convert_rigid_bodies(
    mut commands: Commands,
    mut world: ResMut<PhysicsWorld>,
    bodies: Query<
        (Entity, &UsdRigidBody, Option<&UsdMass>, Option<&GlobalTransform>),
        (Added<UsdRigidBody>, Without<BodyAttached>),
    >,
) {
    for (entity, rb, mass, gt) in &bodies {
        let (world_translation, world_rotation) = match gt {
            Some(g) => {
                let t = g.compute_transform();
                (vec3_to_d(t.translation), quat_to_d(t.rotation))
            }
            None => (Default::default(), Default::default()),
        };

        let op = RigidBodyOpinion {
            kinematic: rb.kinematic,
            enabled: rb.enabled,
            starts_asleep: rb.starts_asleep,
            world_translation,
            world_rotation,
            linvel: vec3_to_d(rb.velocity),
            angvel: vec3_to_d(rb.angular_velocity),
            mass: mass.and_then(|m| m.mass).map(|m| m as f64),
            center_of_mass: mass.and_then(|m| m.center_of_mass).map(vec3_to_d),
            diagonal_inertia: mass.and_then(|m| m.diagonal_inertia).map(vec3_to_d),
            principal_axes: mass.and_then(|m| m.principal_axes).map(quat_to_d),
        };

        if let Ok(handle) = build_rigid_body(&mut world.bodies, &op, entity.to_bits() as u128) {
            world.entity_to_body.insert(entity, handle);
            commands.entity(entity).insert(BodyAttached);
        }
    }
}
