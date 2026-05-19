//! Rapier physics adapter for [`bevy_openusd`].
//!
//! Owns its own Rapier f64 world (`PhysicsWorld` resource) and steps
//! it from a Bevy system. Translates the projection's backend-neutral
//! marker components (`UsdRigidBody`, `UsdMass`, `UsdCollider`,
//! `UsdPhysicsJoint`, `UsdArticulationRoot`, `UsdPhysicsScene`,
//! `UsdPhysicsMaterial`) into Rapier `RigidBodySet` / `ColliderSet` /
//! `MultibodyJointSet` / `ImpulseJointSet` entries. Pose writeback
//! into Bevy `Transform` runs in `PostUpdate`.
//!
//! No `bevy_rapier3d` dependency. This crate wraps `rapier3d-f64`
//! directly so precision matches gearbox / other f64 robotics
//! pipelines and there's no Bevy-Component coupling on the physics
//! state.
//!
//! # Conventions inherited from `bevy_openusd`
//!
//! - All values are SI (m, kg, m/s, rad/s) — `bevy_openusd` applied
//!   `metersPerUnit` / `kilogramsPerUnit` / degree→radian conversions
//!   at the read→marker boundary.
//! - Quaternions are Bevy-native `Quat::from_xyzw` order. Conversions
//!   to nalgebra at the Rapier boundary live in `convert.rs`.
//! - `lower > upper` on any limit means a locked DOF.
//!
//! # Routing rules
//!
//! - [`UsdPhysicsScene`]: first one seen sets `PhysicsWorld.gravity`.
//! - [`UsdRigidBody`]: kinematic bodies become
//!   `RigidBodyType::KinematicPositionBased`, otherwise `Dynamic`.
//!   Mass priority: explicit `mass` → `density` → tiny safety mass.
//! - [`UsdCollider`]: primitive shapes via Rapier's native builders;
//!   mesh colliders honour the `MeshCollisionAPI` approximation token.
//! - [`UsdPhysicsJoint`]: joints in a scene with any
//!   [`UsdArticulationRoot`] become `MultibodyJoint` (Featherstone)
//!   unless flagged `excludeFromArticulation`. Otherwise `ImpulseJoint`.
//!   Same-basis revolute/prismatic joints use the native typed
//!   builder; differing-basis chains fall back to Generic-D6 with full
//!   per-body bases.
//!
//! # Usage
//!
//! ```ignore
//! use bevy::prelude::*;
//! use bevy_openusd::UsdPlugin;
//! use crate::physics::RapierAdapterPlugin;
//!
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_plugins(UsdPlugin)
//!     .add_plugins(RapierAdapterPlugin)
//!     .run();
//! ```

mod bodies;
mod colliders;
mod convert;
mod debug;
mod joints;
mod scene;
mod world;
mod writeback;

pub use debug::ColliderDebugEnabled;
pub use world::{PhysicsActive, PhysicsWorld};

use bevy::prelude::*;

/// Wires the Rapier f64 world + every USD-marker → Rapier conversion
/// system + the writeback path. Adds `PhysicsWorld`, `PhysicsActive`,
/// and `ColliderDebugEnabled` resources.
pub struct RapierAdapterPlugin;

impl Plugin for RapierAdapterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsWorld>()
            .init_resource::<PhysicsActive>()
            .init_resource::<ColliderDebugEnabled>()
            .add_systems(
                Update,
                (
                    scene::sync_gravity_from_usd_scene,
                    bodies::convert_rigid_bodies,
                    colliders::convert_colliders.after(bodies::convert_rigid_bodies),
                    colliders::apply_physics_materials.after(colliders::convert_colliders),
                    joints::convert_joints.after(bodies::convert_rigid_bodies),
                    world::step_physics
                        .after(joints::convert_joints)
                        .after(colliders::convert_colliders),
                ),
            )
            // Writeback only runs while physics is stepping. With it
            // ungated, every frame we copy each body's *initial* pose
            // back over any external Transform mutation (e.g. a user
            // dragging the LoadedAsset root with a transform gizmo),
            // which makes the visual snap back to its mount point even
            // though the gizmo itself has moved.
            .add_systems(
                PostUpdate,
                writeback::writeback_transforms.run_if(physics_is_active),
            )
            // On the OFF→ON edge of `PhysicsActive`, sync every rapier
            // body's pose to the entity's current `GlobalTransform`.
            // Without this, dragging an asset around with a gizmo while
            // physics is paused leaves rapier holding the body's stale
            // load-time pose; resuming physics then snaps writeback to
            // that stale pose and the visual jumps back to where it was
            // before the drag.
            .add_systems(
                Update,
                sync_bodies_to_transforms_on_resume.before(world::step_physics),
            )
            .add_systems(Last, debug::draw_collider_gizmos);
    }
}

fn physics_is_active(active: bevy::prelude::Res<PhysicsActive>) -> bool {
    active.0
}

fn sync_bodies_to_transforms_on_resume(
    active: Res<PhysicsActive>,
    mut prev_active: Local<bool>,
    mut world: ResMut<PhysicsWorld>,
    transforms: Query<&GlobalTransform>,
) {
    let was = *prev_active;
    *prev_active = active.0;
    if !active.0 || was {
        return;
    }
    use rapier3d_f64::prelude::*;
    let world = world.as_mut();
    let pairs: Vec<(Entity, RigidBodyHandle)> =
        world.entity_to_body.iter().map(|(e, h)| (*e, *h)).collect();
    for (entity, handle) in pairs {
        let Ok(gt) = transforms.get(entity) else {
            continue;
        };
        let t = gt.compute_transform();
        let Some(rb) = world.bodies.get_mut(handle) else {
            continue;
        };
        let pose = Pose {
            translation: convert::vec3_to_d(t.translation),
            rotation: convert::quat_to_d(t.rotation),
        };
        rb.set_position(pose, true);
        // Linear & angular velocities reset so the body doesn't
        // inherit whatever it had before the user paused.
        rb.set_linvel(glam::DVec3::ZERO, true);
        rb.set_angvel(glam::DVec3::ZERO, true);
    }
}
