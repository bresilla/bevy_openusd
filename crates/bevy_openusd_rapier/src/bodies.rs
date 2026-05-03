//! `UsdRigidBody` + `UsdMass` → entries in `PhysicsWorld.bodies`.

use bevy::math::DVec3;
use bevy::prelude::*;
use rapier3d_f64::prelude::*;
use usd_physics_markers::{UsdMass, UsdRigidBody};

use crate::convert::{quat_to_d, vec3_to_d};
use crate::world::PhysicsWorld;

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
        let body_type = if !rb.enabled {
            RigidBodyType::Fixed
        } else if rb.kinematic {
            RigidBodyType::KinematicPositionBased
        } else {
            RigidBodyType::Dynamic
        };

        let mut builder = RigidBodyBuilder::new(body_type);

        // Initial pose lifts from the entity's GlobalTransform.
        if let Some(gt) = gt {
            let t = gt.compute_transform();
            builder = builder.position(Pose {
                translation: vec3_to_d(t.translation),
                rotation: quat_to_d(t.rotation),
            });
        }

        builder = builder
            .linvel(vec3_to_d(rb.velocity))
            .angvel(vec3_to_d(rb.angular_velocity));

        if rb.starts_asleep {
            builder = builder.sleeping(true);
        }

        // Body damping for non-articulated chains.
        if matches!(body_type, RigidBodyType::Dynamic) {
            builder = builder.linear_damping(0.1).angular_damping(0.5);
        }

        // Mass: explicit kg → MassProperties on body. Density falls
        // to collider's mass-from-density. None → tiny safety mass.
        match mass {
            Some(m) if m.mass.is_some() => {
                let mass_kg = m.mass.unwrap() as f64;
                let inertia: DVec3 = m
                    .diagonal_inertia
                    .map(vec3_to_d)
                    .unwrap_or(DVec3::splat(0.4 * mass_kg * 0.01));
                let com: DVec3 = m.center_of_mass.map(vec3_to_d).unwrap_or(DVec3::ZERO);
                builder = builder.additional_mass_properties(
                    MassProperties::new(com, mass_kg, inertia),
                );
            }
            _ if matches!(body_type, RigidBodyType::Dynamic) => {
                builder = builder.additional_mass_properties(MassProperties::new(
                    DVec3::ZERO,
                    0.001,
                    DVec3::splat(0.0001),
                ));
            }
            _ => {}
        }

        builder = builder.user_data(entity.to_bits() as u128);
        let handle = world.bodies.insert(builder.build());
        world.entity_to_body.insert(entity, handle);
        commands.entity(entity).insert(BodyAttached);
    }
}
