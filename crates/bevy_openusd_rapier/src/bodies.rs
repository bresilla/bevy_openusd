//! `UsdRigidBody` + `UsdMass` → Rapier `RigidBody` / `Velocity` /
//! mass / `Sleeping`.

use bevy::prelude::*;
use usd_physics_markers::{UsdMass, UsdRigidBody};
use bevy_rapier3d::dynamics::{
    AdditionalMassProperties, Damping, MassProperties, RigidBody, Sleeping, Velocity,
};
use bevy_rapier3d::geometry::ColliderMassProperties;

/// On every newly-spawned `UsdRigidBody`, attach the matching Rapier
/// `RigidBody` enum variant plus initial `Velocity` and (when
/// authored) explicit mass / density / sleep state.
///
/// Mass priority follows USD convention: explicit `mass` wins;
/// otherwise `density` × collider volume; otherwise a tiny fallback
/// mass+inertia is inserted so that the multibody Featherstone solver
/// can't divide by zero before colliders have populated their own
/// mass contribution.
pub fn convert_rigid_bodies(
    mut commands: Commands,
    bodies: Query<
        (Entity, &UsdRigidBody, Option<&UsdMass>),
        (Added<UsdRigidBody>, Without<RigidBody>),
    >,
) {
    for (entity, rb, mass) in &bodies {
        let body = if !rb.enabled {
            // Disabled bodies are left as static (still collide, won't move).
            RigidBody::Fixed
        } else if rb.kinematic {
            RigidBody::KinematicPositionBased
        } else {
            RigidBody::Dynamic
        };

        let mut e = commands.entity(entity);
        e.insert((
            body,
            Velocity {
                linvel: rb.velocity,
                angvel: rb.angular_velocity,
            },
        ));

        let is_dynamic = rb.enabled && !rb.kinematic;

        match mass {
            Some(m) if m.mass.is_some() => {
                // Explicit kg → `AdditionalMassProperties` on the body
                // itself. Using the body component (not
                // `ColliderMassProperties`) means the body has correct
                // mass even before its collider materialises — the
                // multibody Featherstone solver runs on the very next
                // step, and a Dynamic body with zero inertia panics
                // with `min/max NaN` mid-step.
                //
                // To avoid double-counting once colliders attach, we
                // also pin the collider's contribution to zero.
                let mass_kg = m.mass.unwrap();
                // Approximate inertia of a 10 cm sphere when authoring
                // didn't supply a tensor. Real robotics assets author
                // both, but holonomic-ish defaults keep solver stable
                // until they do.
                let inertia = m
                    .diagonal_inertia
                    .unwrap_or_else(|| Vec3::splat(0.4 * mass_kg * 0.01));
                e.insert(AdditionalMassProperties::MassProperties(MassProperties {
                    mass: mass_kg,
                    principal_inertia: inertia,
                    principal_inertia_local_frame: m.principal_axes.unwrap_or(Quat::IDENTITY),
                    local_center_of_mass: m.center_of_mass.unwrap_or(Vec3::ZERO),
                }));
                e.insert(ColliderMassProperties::Mass(0.0));
            }
            Some(m) if m.density.is_some() => {
                e.insert(ColliderMassProperties::Density(m.density.unwrap()));
                if is_dynamic {
                    e.insert(safety_mass());
                }
            }
            _ => {
                if is_dynamic {
                    e.insert(safety_mass());
                }
            }
        }

        if rb.starts_asleep {
            e.insert(Sleeping {
                sleeping: true,
                ..default()
            });
        }

        // Real robot joints have gearbox / bearing friction that
        // dissipates kinetic energy. Without it, an articulation chain
        // under gravity oscillates forever — every link wobbles around
        // the equilibrium pose. USD doesn't have a `body damping`
        // schema, so add a small default. Authored drives still
        // dominate; this only kills free oscillation.
        if is_dynamic {
            // Gentle defaults that settle long articulation chains
            // without locking wheels. Going higher (e.g. 20 on
            // angular) made the Create3's wheels freeze under contact
            // friction and the chassis launched off the ground; rely
            // primarily on `set_contacts_enabled(false)` between
            // jointed bodies (set in joints.rs) to kill jitter.
            // Authored drives still dominate.
            e.insert(Damping {
                linear_damping: 0.1,
                angular_damping: 0.5,
            });
        }
    }
}

/// A negligible mass+inertia floor so dynamic bodies that haven't yet
/// received a collider can still take a simulation step without the
/// Featherstone solver dividing by zero. Engine-authored mass (from
/// `UsdMass` or collider density) dominates this on every realistic
/// asset.
fn safety_mass() -> AdditionalMassProperties {
    AdditionalMassProperties::MassProperties(MassProperties {
        mass: 0.001,
        principal_inertia: Vec3::splat(0.0001),
        principal_inertia_local_frame: Quat::IDENTITY,
        local_center_of_mass: Vec3::ZERO,
    })
}
