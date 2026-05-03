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
//! use bevy_openusd_rapier::RapierAdapterPlugin;
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
            .add_systems(PostUpdate, writeback::writeback_transforms)
            .add_systems(Last, debug::draw_collider_gizmos);
    }
}
