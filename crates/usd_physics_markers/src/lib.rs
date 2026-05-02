//! Backend-neutral UsdPhysics marker components.
//!
//! The projection layer translates `usd_schemas::physics::*` data into
//! these components, one per affected prim entity. Adapter crates
//! (`bevy_openusd_rapier`, future `bevy_openusd_avian`) read them and
//! insert their engine's components in turn — `bevy_openusd` itself
//! never depends on a physics engine.
//!
//! ## Conventions
//! - **Units**: SI throughout (m, kg, m/s, rad/s, N, N·m). The
//!   projection layer applies `metersPerUnit` / `kilogramsPerUnit` once
//!   at the import boundary so adapters never re-scale.
//! - **Quaternion order**: Bevy's `Quat::from_xyzw(x, y, z, w)` (the
//!   schema-reader's USD `(w, x, y, z)` order is converted at the
//!   read→marker boundary).
//! - **Joint axis**: a unit `Vec3` in the joint local frame. Token
//!   `physics:axis` ("X" / "Y" / "Z") is materialised as `Vec3::X` etc;
//!   non-canonical axes are absorbed into `local_rot0/1` by authoring
//!   tools.
//! - **Limits**: rotational limits in radians (USD authors degrees;
//!   converted at projection). Linear limits in metres.
//! - **Lock convention**: `lower > upper` on any limit encodes a locked
//!   DOF. Adapters detect the inversion.
//! - **Mass priority**: `UsdMass` exposes what was authored, not the
//!   resolved value. Adapters apply USD's priority order (explicit mass
//!   → density × volume → material density → 1000 kg/m³).
//! - **Collision approximation fallback**: token preserved verbatim;
//!   adapters apply the per-engine fallback table when the requested
//!   approximation isn't supported (e.g. trimesh on a dynamic body
//!   typically becomes convexHull with a warning).

use bevy::ecs::component::Component;
use bevy::ecs::entity::Entity;
use bevy::ecs::reflect::ReflectComponent;
use bevy::math::{Quat, Vec3};
use bevy::reflect::{Reflect, std_traits::ReflectDefault};

// ── PhysicsScene ────────────────────────────────────────────────────────

/// Tagged on the entity that projects a `UsdPhysics.PhysicsScene` prim.
/// `gravity_direction` is post unit-conversion and post upAxis fix —
/// adapters consume it verbatim as their gravity vector. `gravity_magnitude`
/// is in m/s² (positive scalar).
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdPhysicsScene {
    pub gravity_direction: Vec3,
    pub gravity_magnitude: f32,
}

// ── RigidBody + Mass ────────────────────────────────────────────────────

/// `PhysicsRigidBodyAPI` applied to a prim. Velocities are SI
/// (m/s, rad/s) — the projection layer converts USD's degree-per-second
/// `physics:angularVelocity`.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdRigidBody {
    pub kinematic: bool,
    /// `physics:rigidBodyEnabled`, default `true`.
    pub enabled: bool,
    pub starts_asleep: bool,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    /// `physics:simulationOwner` rel target (composed prim path of the
    /// `PhysicsScene` this body belongs to). `None` when unauthored —
    /// adapters fall back to the default scene.
    pub simulation_owner: Option<String>,
}

/// `PhysicsMassAPI`. Kept separate from `UsdRigidBody` because USD allows
/// `MassAPI` on plain colliders to seed the parent body's mass via
/// aggregation. Adapters that prefer the body-only model can ignore mass
/// authored on non-body prims; adapters that aggregate (matches PhysX /
/// Rapier behaviour) walk descendants and sum.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdMass {
    pub mass: Option<f32>,
    pub density: Option<f32>,
    pub center_of_mass: Option<Vec3>,
    pub diagonal_inertia: Option<Vec3>,
    pub principal_axes: Option<Quat>,
}

// ── Collider ────────────────────────────────────────────────────────────

/// Collision-approximation token authored on `PhysicsMeshCollisionAPI`.
/// Mirrors `usd_schemas::physics::CollisionApprox`. Preserved verbatim;
/// adapters apply per-engine fallbacks (see module-level docs).
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[reflect(Default)]
pub enum UsdCollisionApprox {
    #[default]
    None,
    ConvexHull,
    ConvexDecomposition,
    BoundingSphere,
    BoundingCube,
    MeshSimplification,
}

/// Collider geometry source. Mesh data, when relevant, is on the same
/// entity's `Mesh3d` component — the visual mesh and the collision mesh
/// are the same prim in USD.
#[derive(Reflect, Debug, Clone)]
pub enum UsdColliderShape {
    Cube { size: f32 },
    Sphere { radius: f32 },
    Capsule { radius: f32, height: f32, axis: Vec3 },
    Cylinder { radius: f32, height: f32, axis: Vec3 },
    /// Mesh-derived collider; geometry sits on the entity's `Mesh3d`.
    /// `approximation` on `UsdCollider` selects the engine-side build.
    Mesh,
    Plane,
}

impl Default for UsdColliderShape {
    fn default() -> Self {
        UsdColliderShape::Cube { size: 1.0 }
    }
}

/// `PhysicsCollisionAPI` (+ optional `PhysicsMeshCollisionAPI`).
/// `physics_material` resolves to the entity carrying the bound
/// `UsdPhysicsMaterial` (looked up via `material:binding:physics` first,
/// then plain `material:binding`).
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdCollider {
    pub shape: UsdColliderShape,
    /// `physics:collisionEnabled`, default `true`.
    pub enabled: bool,
    /// `Some` only when MeshCollisionAPI is applied. `None` for primitive
    /// shapes, where the engine uses its native collider.
    pub approximation: Option<UsdCollisionApprox>,
    #[entities]
    pub physics_material: Option<Entity>,
    pub simulation_owner: Option<String>,
}

// ── PhysicsMaterial ─────────────────────────────────────────────────────

/// `PhysicsMaterialAPI` applied to a `UsdShade.Material` prim. Adapters
/// read these scalars when wiring `Friction` / `Restitution` /
/// density on consumer colliders.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdPhysicsMaterial {
    pub static_friction: Option<f32>,
    pub dynamic_friction: Option<f32>,
    pub restitution: Option<f32>,
    pub density: Option<f32>,
}

// ── ArticulationRoot ────────────────────────────────────────────────────

/// `PhysicsArticulationRootAPI` marker. No attributes; presence is the
/// signal. The Rapier adapter realises this as a `MultibodyJoint`
/// subtree (reduced-coordinate solve); the future Avian adapter
/// degrades to chained constraints with a warning.
///
/// `joints` is populated by the projection post-pass with every
/// `UsdPhysicsJoint` entity in this articulation's subtree (filtered by
/// `exclude_from_articulation` and de-cycled). Adapters read this
/// instead of re-walking the tree.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdArticulationRoot {
    #[entities]
    pub joints: Vec<Entity>,
}

// ── Joints ──────────────────────────────────────────────────────────────

#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[reflect(Default)]
pub enum UsdJointKind {
    #[default]
    Fixed,
    Revolute,
    Prismatic,
    Spherical,
    Distance,
    Generic,
}

/// USD DOF tokens (Pixar canonical six + the linear/angular/distance
/// shorthands some authoring tools emit on single-axis joints).
#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[reflect(Default)]
pub enum UsdDof {
    #[default]
    TransX,
    TransY,
    TransZ,
    RotX,
    RotY,
    RotZ,
    Linear,
    Angular,
    Distance,
}

#[derive(Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[reflect(Default)]
pub enum UsdDriveType {
    #[default]
    Force,
    Acceleration,
}

/// One entry from a multi-apply `PhysicsLimitAPI:<dof>`. Rotational DOFs
/// are in radians (converted from USD's degrees at projection); linear
/// DOFs in metres. `low > high` encodes a locked DOF.
#[derive(Reflect, Debug, Clone, Default)]
#[reflect(Default)]
pub struct UsdJointLimit {
    pub dof: UsdDof,
    pub low: f32,
    pub high: f32,
}

/// One entry from a multi-apply `PhysicsDriveAPI:<dof>`. Stiffness /
/// damping in SI (rotational: N·m/rad and N·m·s/rad; linear: N/m and
/// N·s/m). Targets and max force in matching SI units.
#[derive(Reflect, Debug, Clone, Default)]
#[reflect(Default)]
pub struct UsdJointDrive {
    pub dof: UsdDof,
    pub drive_type: UsdDriveType,
    pub target_position: Option<f32>,
    pub target_velocity: Option<f32>,
    pub damping: f32,
    pub stiffness: f32,
    pub max_force: Option<f32>,
}

/// One joint entity per `Physics*Joint` prim. `body0` / `body1`
/// resolve to entities at projection time; either may be `None` when
/// the joint anchors against world (USD allows one body rel empty —
/// adapters either spawn / reuse a static body or use the engine's
/// world-anchor primitive).
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdPhysicsJoint {
    pub kind: UsdJointKind,
    #[entities]
    pub body0: Option<Entity>,
    #[entities]
    pub body1: Option<Entity>,
    /// Local frames in each body's space (post `metersPerUnit` conversion).
    /// Constraint: `G0 · L0 · J = G1 · L1` where `G*` are body world
    /// poses, `L*` are these local frames, `J` is the relative joint
    /// pose (zero for fixed; rotation about axis for revolute; etc).
    pub local_pos0: Vec3,
    pub local_rot0: Quat,
    pub local_pos1: Vec3,
    pub local_rot1: Quat,
    /// Unit axis vector in the joint local frame.
    pub axis: Vec3,
    /// `physics:jointEnabled`, default `true`.
    pub joint_enabled: bool,
    /// `physics:collisionEnabled` on the joint (allow collision between
    /// the two attached bodies).
    pub collision_enabled: bool,
    pub exclude_from_articulation: bool,
    pub break_force: Option<f32>,
    pub break_torque: Option<f32>,
    /// Built-in single-DOF limits (revolute / prismatic). Revolute is
    /// rad, prismatic is m. `None` when unauthored or unlimited.
    pub built_in_limit: Option<(f32, f32)>,
    /// Spherical cone limits (rad, `-1.0` = unlimited).
    pub cone_limit: Option<(f32, f32)>,
    /// Distance joint min / max (m).
    pub distance_limit: Option<(f32, f32)>,
    pub limits: Vec<UsdJointLimit>,
    pub drives: Vec<UsdJointDrive>,
}

// ── Collision filtering ─────────────────────────────────────────────────

/// `PhysicsCollisionGroup` prim. `members` and `filtered` are resolved
/// to entity references at projection time. `members` is the raw
/// `collection:colliders:includes` list — full UsdCollectionAPI rule
/// evaluation is a v2 follow-up.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdCollisionGroup {
    #[entities]
    pub members: Vec<Entity>,
    #[entities]
    pub filtered: Vec<Entity>,
    pub merge_group: Option<String>,
    pub invert_filtered_groups: bool,
}

/// `PhysicsFilteredPairsAPI` on a body prim. Lists entities this body
/// should never collide with regardless of group membership.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdCollisionFilter {
    #[entities]
    pub filtered: Vec<Entity>,
}
