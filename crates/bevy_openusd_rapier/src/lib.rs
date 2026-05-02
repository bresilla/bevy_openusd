//! Rapier physics adapter for [`bevy_openusd`].
//!
//! [`bevy_openusd`] reads USD files and projects them into ECS as a
//! tree of entities carrying backend-neutral marker components
//! (`UsdRigidBody`, `UsdMass`, `UsdCollider`, `UsdPhysicsJoint`,
//! `UsdArticulationRoot`, `UsdPhysicsScene`, `UsdPhysicsMaterial`).
//! This crate adds [`RapierAdapterPlugin`], which translates those
//! markers 1:1 into the corresponding [`bevy_rapier3d`] components so
//! the scene actually simulates.
//!
//! # Conventions inherited from `bevy_openusd`
//!
//! - All values are SI (m, kg, m/s, rad/s) — `bevy_openusd` applied
//!   `metersPerUnit` / `kilogramsPerUnit` / degree→radian conversions
//!   at the read→marker boundary.
//! - Quaternions are Bevy-native `Quat::from_xyzw` order.
//! - `lower > upper` on any limit means a locked DOF.
//!
//! # Routing rules this adapter follows
//!
//! - [`UsdPhysicsScene`]: first one seen sets [`RapierConfiguration::gravity`].
//! - [`UsdRigidBody`]: kinematic bodies become
//!   `RigidBody::KinematicPositionBased`, otherwise `RigidBody::Dynamic`.
//!   Mass priority follows USD: explicit `mass` → `density` → engine default.
//! - [`UsdCollider`]: primitive shapes use Rapier's native constructors;
//!   mesh colliders honour the `MeshCollisionAPI` approximation token,
//!   with the fallback table from the project's PLAN.md (trimesh for
//!   static bodies, convex hull for dynamic when authored as `none`).
//! - [`UsdPhysicsJoint`]: joints inside a [`UsdArticulationRoot`]
//!   subtree become `MultibodyJoint` (reduced-coordinate, stable for
//!   long chains). Joints with `exclude_from_articulation` or those
//!   that would close a loop fall back to `ImpulseJoint`. Joints
//!   outside any articulation are always `ImpulseJoint`.
//!
//! # Usage
//!
//! ```ignore
//! use bevy::prelude::*;
//! use bevy_openusd::UsdPlugin;
//! use bevy_openusd_rapier::RapierAdapterPlugin;
//! use bevy_rapier3d::prelude::*;
//!
//! App::new()
//!     .add_plugins(DefaultPlugins)
//!     .add_plugins(UsdPlugin)
//!     .add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
//!     .add_plugins(RapierAdapterPlugin)
//!     .run();
//! ```

mod bodies;
mod colliders;
mod joints;
mod scene;

use bevy::prelude::*;

/// Wires every USD physics marker → Rapier component conversion
/// system. Run after `RapierPhysicsPlugin` so its resources exist.
pub struct RapierAdapterPlugin;

impl Plugin for RapierAdapterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                scene::sync_gravity_from_usd_scene,
                scene::raise_scaled_shape_subdivision,
                bodies::convert_rigid_bodies,
                colliders::convert_colliders,
                colliders::apply_physics_materials,
                // Runs after rigid-body conversion so world-anchored
                // Fixed joints can override the body's RigidBody enum
                // to ::Fixed (Isaac Sim convention pins the base of
                // an articulation that way).
                joints::convert_joints.after(bodies::convert_rigid_bodies),
            ),
        );
    }
}
