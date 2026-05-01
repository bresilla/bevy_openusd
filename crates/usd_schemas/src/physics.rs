//! UsdPhysics — authoring + reader helpers.
//!
//! Authoring side stamps the right `TypeName` tokens, `apiSchemas` list ops,
//! relationships, and attribute defaults that Pixar's C++ schema classes
//! would emit for `PhysicsScene`, joints (Fixed / Revolute / Prismatic /
//! Spherical / Distance / Generic), and the `RigidBodyAPI` / `MassAPI` /
//! `CollisionAPI` / `MeshCollisionAPI` / `MaterialAPI` /
//! `ArticulationRootAPI` / `FilteredPairsAPI` API schemas, plus the
//! multi-apply `PhysicsLimitAPI:<dof>` and `PhysicsDriveAPI:<dof>`.
//!
//! Reader side surfaces strongly-typed structs (`ReadPhysicsScene`,
//! `ReadJoint`, `ReadMass`, `ReadCollisionShape`, `ReadPhysicsMaterial`,
//! `ReadCollisionGroup`, `ReadFilteredPairs`) plus a single sweep helper
//! [`find_physics_prims`] that walks the stage once and returns
//! categorised path lists.
//!
//! All values stay in the **scene's authored units** on the reader side —
//! callers apply `metersPerUnit` / `kilogramsPerUnit` at the import
//! boundary. Quaternions stay in USD's `(w, x, y, z)` order. Angles stay
//! in degrees (as USD authors them).

use anyhow::Result;
use openusd::Stage;
use openusd::sdf::{Path, Value};

use super::Stage as AuthorStage;
use super::tokens::*;

// ════════════════════════════════════════════════════════════════════════
//                              AUTHORING
// ════════════════════════════════════════════════════════════════════════

pub fn define_scene(stage: &mut AuthorStage, parent: &Path, name: &str) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_SCENE)?;
    // Match the Python converter: apply NewtonSceneAPI so downstream
    // Newton importers initialize the scene correctly.
    stage.apply_api_schemas(&p, &[API_NEWTON_SCENE])?;
    Ok(p)
}

pub fn apply_rigid_body(stage: &mut AuthorStage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_RIGID_BODY])
}

pub fn apply_articulation_root(stage: &mut AuthorStage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_ARTICULATION_ROOT, API_NEWTON_ARTICULATION_ROOT])
}

pub struct MassProps {
    pub mass: f64,
    pub center_of_mass: [f64; 3],
    pub diagonal_inertia: [f32; 3],
    /// Quaternion `(w, x, y, z)` of the principal-axes frame.
    pub principal_axes: [f32; 4],
}

pub fn apply_mass(stage: &mut AuthorStage, prim: &Path, props: &MassProps) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_MASS])?;
    stage.define_attribute(prim, A_MASS, "float", Value::Float(props.mass as f32), false)?;
    stage.define_attribute(
        prim,
        A_CENTER_OF_MASS,
        "point3f",
        Value::Vec3f([
            props.center_of_mass[0] as f32,
            props.center_of_mass[1] as f32,
            props.center_of_mass[2] as f32,
        ]),
        false,
    )?;
    stage.define_attribute(
        prim,
        A_DIAGONAL_INERTIA,
        "float3",
        Value::Vec3f(props.diagonal_inertia),
        false,
    )?;
    stage.define_attribute(
        prim,
        A_PRINCIPAL_AXES,
        "quatf",
        Value::Quatf(props.principal_axes),
        false,
    )?;
    Ok(())
}

pub fn apply_collision(stage: &mut AuthorStage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_COLLISION])
}

/// For mesh collisions, apply both CollisionAPI and MeshCollisionAPI, and
/// author `physics:approximation = "convexHull"` which matches the Python
/// converter default.
pub fn apply_mesh_collision_convex_hull(stage: &mut AuthorStage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_COLLISION, API_MESH_COLLISION])?;
    stage.define_attribute(
        prim,
        A_APPROXIMATION,
        "token",
        Value::Token(APPROX_CONVEX_HULL.into()),
        true,
    )
}

/// Apply MeshCollisionAPI with an explicit approximation token.
pub fn apply_mesh_collision(
    stage: &mut AuthorStage,
    prim: &Path,
    approximation: CollisionApprox,
) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_COLLISION, API_MESH_COLLISION])?;
    stage.define_attribute(
        prim,
        A_APPROXIMATION,
        "token",
        Value::Token(approximation.as_token().into()),
        true,
    )
}

/// Apply PhysicsMaterialAPI with the four scalar attributes.
pub fn apply_physics_material(
    stage: &mut AuthorStage,
    prim: &Path,
    static_friction: Option<f32>,
    dynamic_friction: Option<f32>,
    restitution: Option<f32>,
    density: Option<f32>,
) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_PHYSICS_MATERIAL])?;
    if let Some(v) = static_friction {
        stage.define_attribute(prim, A_STATIC_FRICTION, "float", Value::Float(v), false)?;
    }
    if let Some(v) = dynamic_friction {
        stage.define_attribute(prim, A_DYNAMIC_FRICTION, "float", Value::Float(v), false)?;
    }
    if let Some(v) = restitution {
        stage.define_attribute(prim, A_RESTITUTION, "float", Value::Float(v), false)?;
    }
    if let Some(v) = density {
        stage.define_attribute(prim, A_DENSITY, "float", Value::Float(v), false)?;
    }
    Ok(())
}

/// Common body0/body1 + local frame authoring for any UsdPhysics joint type.
pub struct JointFrame {
    pub body0: Option<Path>,
    pub body1: Option<Path>,
    pub local_pos0: [f32; 3],
    pub local_rot0: [f32; 4], // (w, x, y, z)
    pub local_pos1: [f32; 3],
    pub local_rot1: [f32; 4],
}

pub fn author_joint_frame(stage: &mut AuthorStage, joint: &Path, f: &JointFrame) -> Result<()> {
    if let Some(b0) = &f.body0 {
        stage.define_relationship(joint, A_BODY0, vec![b0.clone()])?;
    }
    if let Some(b1) = &f.body1 {
        stage.define_relationship(joint, A_BODY1, vec![b1.clone()])?;
    }
    stage.define_attribute(joint, A_LOCAL_POS_0, "point3f", Value::Vec3f(f.local_pos0), false)?;
    stage.define_attribute(joint, A_LOCAL_ROT_0, "quatf", Value::Quatf(f.local_rot0), false)?;
    stage.define_attribute(joint, A_LOCAL_POS_1, "point3f", Value::Vec3f(f.local_pos1), false)?;
    stage.define_attribute(joint, A_LOCAL_ROT_1, "quatf", Value::Quatf(f.local_rot1), false)?;
    Ok(())
}

pub fn define_fixed_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_FIXED_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    Ok(p)
}

/// Axis token — one of `"X"`, `"Y"`, `"Z"`. URDF gives a free vector; if
/// the vector is close to a canonical axis we emit that token, otherwise
/// we emit `"X"` and the caller is expected to absorb the actual direction
/// into `localRot` via `quat_from_x_to`.
pub fn axis_token(axis: [f64; 3]) -> &'static str {
    const EPS: f64 = 1e-6;
    let is_axis_x = axis[1].abs() < EPS && axis[2].abs() < EPS && axis[0].abs() > EPS;
    let is_axis_y = axis[0].abs() < EPS && axis[2].abs() < EPS && axis[1].abs() > EPS;
    let is_axis_z = axis[0].abs() < EPS && axis[1].abs() < EPS && axis[2].abs() > EPS;
    if is_axis_y {
        AXIS_Y
    } else if is_axis_z {
        AXIS_Z
    } else if is_axis_x {
        AXIS_X
    } else {
        AXIS_X
    }
}

pub struct JointLimit {
    pub lower: f64,
    pub upper: f64,
}

pub fn define_revolute_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    axis: &str,
    limits: Option<JointLimit>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_REVOLUTE_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    stage.define_attribute(&p, A_AXIS, "token", Value::Token(axis.into()), true)?;
    if let Some(l) = limits {
        // Revolute limits are in DEGREES in UsdPhysics.
        stage.define_attribute(
            &p,
            A_LOWER_LIMIT,
            "float",
            Value::Float(l.lower.to_degrees() as f32),
            false,
        )?;
        stage.define_attribute(
            &p,
            A_UPPER_LIMIT,
            "float",
            Value::Float(l.upper.to_degrees() as f32),
            false,
        )?;
    }
    Ok(p)
}

pub fn define_prismatic_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    axis: &str,
    limits: Option<JointLimit>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_PRISMATIC_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    stage.define_attribute(&p, A_AXIS, "token", Value::Token(axis.into()), true)?;
    if let Some(l) = limits {
        stage.define_attribute(&p, A_LOWER_LIMIT, "float", Value::Float(l.lower as f32), false)?;
        stage.define_attribute(&p, A_UPPER_LIMIT, "float", Value::Float(l.upper as f32), false)?;
    }
    Ok(p)
}

/// Spherical joint with optional cone limits (degrees, `-1` = unlimited).
pub fn define_spherical_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    axis: &str,
    cone_angle_0: Option<f32>,
    cone_angle_1: Option<f32>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_SPHERICAL_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    stage.define_attribute(&p, A_AXIS, "token", Value::Token(axis.into()), true)?;
    if let Some(v) = cone_angle_0 {
        stage.define_attribute(&p, A_CONE_ANGLE_0_LIMIT, "float", Value::Float(v), false)?;
    }
    if let Some(v) = cone_angle_1 {
        stage.define_attribute(&p, A_CONE_ANGLE_1_LIMIT, "float", Value::Float(v), false)?;
    }
    Ok(p)
}

/// Distance joint with optional min/max limits (scene units; `-1` = unlimited).
pub fn define_distance_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    min_distance: Option<f32>,
    max_distance: Option<f32>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_DISTANCE_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    if let Some(v) = min_distance {
        stage.define_attribute(&p, A_MIN_DISTANCE, "float", Value::Float(v), false)?;
    }
    if let Some(v) = max_distance {
        stage.define_attribute(&p, A_MAX_DISTANCE, "float", Value::Float(v), false)?;
    }
    Ok(p)
}

/// Apply `NewtonMimicAPI` to a joint that mimics another joint's position.
pub fn apply_mimic(
    stage: &mut AuthorStage,
    joint: &Path,
    target_joint: &Path,
    multiplier: f64,
    offset: f64,
) -> Result<()> {
    stage.apply_api_schemas(joint, &[API_NEWTON_MIMIC])?;
    stage.define_attribute(joint, "newton:mimicCoef0", "float", Value::Float(offset as f32), false)?;
    stage.define_attribute(joint, "newton:mimicCoef1", "float", Value::Float(multiplier as f32), false)?;
    stage.define_relationship(joint, "newton:mimicJoint", vec![target_joint.clone()])?;
    Ok(())
}

/// Generic `PhysicsJoint` — no built-in axis/limit. Used as the base for
/// planar joints, which author their own `LimitAPI` constraints for
/// translation and rotation on specific DOFs.
pub fn define_generic_joint(
    stage: &mut AuthorStage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    Ok(p)
}

/// Apply `PhysicsLimitAPI` to a joint for a specific DOF token
/// (`"transX"`, `"transY"`, `"transZ"`, `"rotX"`, `"rotY"`, `"rotZ"`,
/// `"linear"`, `"angular"`, `"distance"`).
///
/// Passing `lower > upper` encodes a locked DOF (canonical USD convention).
/// The resulting prim shape is:
/// ```text
/// float limit:<dof>:physics:low = <lower>
/// float limit:<dof>:physics:high = <upper>
/// ```
pub fn apply_limit(
    stage: &mut AuthorStage,
    joint: &Path,
    dof: &str,
    lower: f64,
    upper: f64,
) -> Result<()> {
    let applied = format!("{API_LIMIT}:{dof}");
    stage.apply_api_schemas(joint, &[&applied])?;
    stage.define_attribute(
        joint,
        &format!("limit:{dof}:physics:{LIMIT_SUB_LOW}"),
        "float",
        Value::Float(lower as f32),
        false,
    )?;
    stage.define_attribute(
        joint,
        &format!("limit:{dof}:physics:{LIMIT_SUB_HIGH}"),
        "float",
        Value::Float(upper as f32),
        false,
    )?;
    Ok(())
}

/// Drive parameters for `apply_drive`.
#[derive(Debug, Clone, Copy)]
pub struct DriveProps {
    pub drive_type: DriveType,
    pub target_position: Option<f32>,
    pub target_velocity: Option<f32>,
    pub damping: f32,
    pub stiffness: f32,
    pub max_force: Option<f32>,
}

impl Default for DriveProps {
    fn default() -> Self {
        Self {
            drive_type: DriveType::Force,
            target_position: None,
            target_velocity: None,
            damping: 0.0,
            stiffness: 0.0,
            max_force: None,
        }
    }
}

/// Apply `PhysicsDriveAPI:<dof>` to a joint.
///
/// The resulting prim shape is:
/// ```text
/// uniform token  drive:<dof>:physics:type           = "force" | "acceleration"
/// float          drive:<dof>:physics:targetPosition = ...
/// float          drive:<dof>:physics:targetVelocity = ...
/// float          drive:<dof>:physics:damping        = ...
/// float          drive:<dof>:physics:stiffness      = ...
/// float          drive:<dof>:physics:maxForce       = ...
/// ```
pub fn apply_drive(
    stage: &mut AuthorStage,
    joint: &Path,
    dof: &str,
    drive: &DriveProps,
) -> Result<()> {
    let applied = format!("{API_DRIVE}:{dof}");
    stage.apply_api_schemas(joint, &[&applied])?;
    stage.define_attribute(
        joint,
        &format!("drive:{dof}:physics:{DRIVE_SUB_TYPE}"),
        "token",
        Value::Token(drive.drive_type.as_token().into()),
        true,
    )?;
    if let Some(v) = drive.target_position {
        stage.define_attribute(
            joint,
            &format!("drive:{dof}:physics:{DRIVE_SUB_TARGET_POSITION}"),
            "float",
            Value::Float(v),
            false,
        )?;
    }
    if let Some(v) = drive.target_velocity {
        stage.define_attribute(
            joint,
            &format!("drive:{dof}:physics:{DRIVE_SUB_TARGET_VELOCITY}"),
            "float",
            Value::Float(v),
            false,
        )?;
    }
    stage.define_attribute(
        joint,
        &format!("drive:{dof}:physics:{DRIVE_SUB_DAMPING}"),
        "float",
        Value::Float(drive.damping),
        false,
    )?;
    stage.define_attribute(
        joint,
        &format!("drive:{dof}:physics:{DRIVE_SUB_STIFFNESS}"),
        "float",
        Value::Float(drive.stiffness),
        false,
    )?;
    if let Some(v) = drive.max_force {
        stage.define_attribute(
            joint,
            &format!("drive:{dof}:physics:{DRIVE_SUB_MAX_FORCE}"),
            "float",
            Value::Float(v),
            false,
        )?;
    }
    Ok(())
}

// ════════════════════════════════════════════════════════════════════════
//                              READER TYPES
// ════════════════════════════════════════════════════════════════════════

/// Joint prim types we recognise. `Generic` is `PhysicsJoint` (no built-in
/// axis), typically paired with explicit `PhysicsLimitAPI` constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointKind {
    Fixed,
    Revolute,
    Prismatic,
    Spherical,
    Distance,
    Generic,
}

/// Joint DOF tokens used by multi-apply `PhysicsLimitAPI` and
/// `PhysicsDriveAPI`. `Linear` / `Angular` / `Distance` are the
/// shorthand instance names some authoring tools emit on single-axis
/// joints; `TransX..RotZ` are the canonical six DOFs on generic joints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dof {
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

impl Dof {
    pub fn as_token(self) -> &'static str {
        match self {
            Dof::TransX => DOF_TRANS_X,
            Dof::TransY => DOF_TRANS_Y,
            Dof::TransZ => DOF_TRANS_Z,
            Dof::RotX => DOF_ROT_X,
            Dof::RotY => DOF_ROT_Y,
            Dof::RotZ => DOF_ROT_Z,
            Dof::Linear => DOF_LINEAR,
            Dof::Angular => DOF_ANGULAR,
            Dof::Distance => DOF_DISTANCE,
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        Some(match s {
            DOF_TRANS_X => Dof::TransX,
            DOF_TRANS_Y => Dof::TransY,
            DOF_TRANS_Z => Dof::TransZ,
            DOF_ROT_X => Dof::RotX,
            DOF_ROT_Y => Dof::RotY,
            DOF_ROT_Z => Dof::RotZ,
            DOF_LINEAR => Dof::Linear,
            DOF_ANGULAR => Dof::Angular,
            DOF_DISTANCE => Dof::Distance,
            _ => return None,
        })
    }
}

/// `PhysicsDriveAPI:<dof>:physics:type` token values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    Force,
    Acceleration,
}

impl DriveType {
    pub fn as_token(self) -> &'static str {
        match self {
            DriveType::Force => DRIVE_TYPE_FORCE,
            DriveType::Acceleration => DRIVE_TYPE_ACCELERATION,
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        Some(match s {
            DRIVE_TYPE_FORCE => DriveType::Force,
            DRIVE_TYPE_ACCELERATION => DriveType::Acceleration,
            _ => return None,
        })
    }
}

/// `physics:approximation` token values on `PhysicsMeshCollisionAPI`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionApprox {
    /// Default — engine-specific fallback. For dynamic bodies an importer
    /// typically substitutes `ConvexHull`; for static bodies, a trimesh.
    None,
    ConvexHull,
    ConvexDecomposition,
    BoundingSphere,
    BoundingCube,
    MeshSimplification,
}

impl CollisionApprox {
    pub fn as_token(self) -> &'static str {
        match self {
            CollisionApprox::None => APPROX_NONE,
            CollisionApprox::ConvexHull => APPROX_CONVEX_HULL,
            CollisionApprox::ConvexDecomposition => APPROX_CONVEX_DECOMPOSITION,
            CollisionApprox::BoundingSphere => APPROX_BOUNDING_SPHERE,
            CollisionApprox::BoundingCube => APPROX_BOUNDING_CUBE,
            CollisionApprox::MeshSimplification => APPROX_MESH_SIMPLIFICATION,
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        Some(match s {
            APPROX_NONE => CollisionApprox::None,
            APPROX_CONVEX_HULL => CollisionApprox::ConvexHull,
            APPROX_CONVEX_DECOMPOSITION => CollisionApprox::ConvexDecomposition,
            APPROX_BOUNDING_SPHERE => CollisionApprox::BoundingSphere,
            APPROX_BOUNDING_CUBE => CollisionApprox::BoundingCube,
            APPROX_MESH_SIMPLIFICATION => CollisionApprox::MeshSimplification,
            _ => return None,
        })
    }
}

/// Decoded inertial properties from a prim with `PhysicsMassAPI` applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadMass {
    pub mass: Option<f32>,
    pub center_of_mass: Option<[f32; 3]>,
    pub diagonal_inertia: Option<[f32; 3]>,
    /// Quaternion `(w, x, y, z)` of the principal-axes frame.
    pub principal_axes: Option<[f32; 4]>,
    /// `physics:density` (optional — used when `mass` is absent).
    pub density: Option<f32>,
}

/// Decoded `UsdPhysicsScene`. `gravity_direction` is a free vector
/// (typically a unit vector pointing in the direction of gravity);
/// `gravity_magnitude` scales it. Both default to `None` when the scene
/// prim authored only the type but no attributes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadPhysicsScene {
    pub path: String,
    pub gravity_direction: Option<[f32; 3]>,
    pub gravity_magnitude: Option<f32>,
}

/// Decoded `PhysicsCollisionAPI` (+ optional `PhysicsMeshCollisionAPI`)
/// state on a prim. `approximation` is `Some` only when MeshCollisionAPI
/// is applied; otherwise the prim is a primitive shape and the engine
/// uses its native collider type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadCollisionShape {
    pub has_collision_api: bool,
    pub has_mesh_collision_api: bool,
    /// `physics:collisionEnabled` — defaults to `true` when unauthored
    /// (per Pixar spec; CollisionAPI being applied implies "on").
    pub collision_enabled: bool,
    pub approximation: Option<CollisionApprox>,
    pub simulation_owner: Option<String>,
    /// Resolved through `material:binding:physics` first, then
    /// `material:binding`. Empty when no material is bound.
    pub physics_material_path: Option<String>,
}

/// Decoded `PhysicsMaterialAPI` on a `Material` prim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadPhysicsMaterial {
    pub path: String,
    pub static_friction: Option<f32>,
    pub dynamic_friction: Option<f32>,
    pub restitution: Option<f32>,
    pub density: Option<f32>,
}

/// One entry from a multi-apply `PhysicsLimitAPI:<dof>` schema.
/// `lower > upper` encodes a locked DOF (canonical USD convention).
#[derive(Debug, Clone, PartialEq)]
pub struct ReadLimit {
    pub dof: Dof,
    pub low: f32,
    pub high: f32,
}

/// One entry from a multi-apply `PhysicsDriveAPI:<dof>` schema.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadDrive {
    pub dof: Dof,
    pub drive_type: DriveType,
    pub target_position: Option<f32>,
    pub target_velocity: Option<f32>,
    pub damping: f32,
    pub stiffness: f32,
    pub max_force: Option<f32>,
}

/// Decoded `Physics*Joint` prim. `axis` is `"X" | "Y" | "Z"` when set;
/// `lower_limit` / `upper_limit` are the **built-in** single-axis limits
/// authored on `PhysicsRevoluteJoint` / `PhysicsPrismaticJoint` (revolute
/// in degrees, prismatic in scene distance units).
///
/// `limits` and `drives` carry **multi-apply** `PhysicsLimitAPI:<dof>` /
/// `PhysicsDriveAPI:<dof>` opinions, used on generic joints to lock /
/// limit / drive specific DOFs.
#[derive(Debug, Clone)]
pub struct ReadJoint {
    pub path: String,
    pub kind: JointKind,
    pub body0: Option<String>,
    pub body1: Option<String>,
    pub local_pos0: [f32; 3],
    pub local_rot0: [f32; 4],
    pub local_pos1: [f32; 3],
    pub local_rot1: [f32; 4],
    pub axis: Option<String>,
    pub lower_limit: Option<f32>,
    pub upper_limit: Option<f32>,
    pub collision_enabled: bool,
    pub joint_enabled: bool,
    pub exclude_from_articulation: bool,
    pub break_force: Option<f32>,
    pub break_torque: Option<f32>,
    /// `physics:minDistance` / `maxDistance` on `PhysicsDistanceJoint`.
    pub min_distance: Option<f32>,
    pub max_distance: Option<f32>,
    /// `physics:coneAngle0Limit` / `coneAngle1Limit` on `PhysicsSphericalJoint`
    /// (degrees, `-1.0` = unlimited).
    pub cone_angle_0: Option<f32>,
    pub cone_angle_1: Option<f32>,
    pub limits: Vec<ReadLimit>,
    pub drives: Vec<ReadDrive>,
}

/// Decoded `PhysicsCollisionGroup`. `members` is the raw list of
/// `CollectionAPI:colliders.includes` targets (full collection-rule
/// flattening is a follow-up; v1 reads the explicit list only).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadCollisionGroup {
    pub path: String,
    pub members: Vec<String>,
    pub filtered_groups: Vec<String>,
    pub merge_group: Option<String>,
    pub invert_filtered_groups: bool,
}

/// Decoded `PhysicsFilteredPairsAPI` on a body prim.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadFilteredPairs {
    pub path: String,
    pub filtered: Vec<String>,
}

/// Result of a single top-of-stage sweep. Lets the projection layer
/// avoid re-walking the stage once for each schema family.
#[derive(Debug, Clone, Default)]
pub struct PhysicsPrims {
    pub scenes: Vec<String>,
    pub rigid_bodies: Vec<String>,
    pub articulation_roots: Vec<String>,
    pub colliders: Vec<String>,
    pub joints: Vec<String>,
    pub materials: Vec<String>,
    pub collision_groups: Vec<String>,
    pub filtered_pairs: Vec<String>,
}

// ════════════════════════════════════════════════════════════════════════
//                                READERS
// ════════════════════════════════════════════════════════════════════════

/// Flattened list of applied API schemas on `prim`.
pub fn read_api_schemas(stage: &Stage, prim: &Path) -> Result<Vec<String>> {
    let raw = stage
        .field::<Value>(prim.clone(), "apiSchemas")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::TokenListOp(op)) => op.flatten(),
        Some(Value::TokenVec(v)) => v,
        _ => Vec::new(),
    })
}

/// `true` when `PhysicsRigidBodyAPI` is applied to the prim.
pub fn read_has_rigid_body(stage: &Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?.iter().any(|s| s == API_RIGID_BODY))
}

/// `true` when `PhysicsCollisionAPI` is applied.
pub fn read_has_collision(stage: &Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?.iter().any(|s| s == API_COLLISION))
}

/// `true` when `PhysicsArticulationRootAPI` is applied.
pub fn read_has_articulation_root(stage: &Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?.iter().any(|s| s == API_ARTICULATION_ROOT))
}

/// Read `PhysicsMassAPI` attributes. Returns `None` when the prim hasn't
/// applied `MassAPI` (so callers can distinguish "unauthored" from "zero
/// mass").
pub fn read_mass(stage: &Stage, prim: &Path) -> Result<Option<ReadMass>> {
    if !read_api_schemas(stage, prim)?.iter().any(|s| s == API_MASS) {
        return Ok(None);
    }
    let mass = read_scalar_f32(stage, prim, A_MASS)?;
    let center_of_mass = read_attr_value(stage, prim, A_CENTER_OF_MASS)?.and_then(value_to_vec3f);
    let diagonal_inertia =
        read_attr_value(stage, prim, A_DIAGONAL_INERTIA)?.and_then(value_to_vec3f);
    let principal_axes =
        read_attr_value(stage, prim, A_PRINCIPAL_AXES)?.and_then(value_to_quatf);
    let density = read_scalar_f32(stage, prim, A_DENSITY)?;
    Ok(Some(ReadMass {
        mass,
        center_of_mass,
        diagonal_inertia,
        principal_axes,
        density,
    }))
}

/// Read a `PhysicsScene` prim. Returns `None` when the prim is not typed
/// `PhysicsScene`.
pub fn read_physics_scene(stage: &Stage, prim: &Path) -> Result<Option<ReadPhysicsScene>> {
    if !read_is_physics_scene(stage, prim)? {
        return Ok(None);
    }
    let gravity_direction =
        read_attr_value(stage, prim, A_GRAVITY_DIRECTION)?.and_then(value_to_vec3f);
    let gravity_magnitude = read_scalar_f32(stage, prim, A_GRAVITY_MAGNITUDE)?;
    Ok(Some(ReadPhysicsScene {
        path: prim.as_str().to_string(),
        gravity_direction,
        gravity_magnitude,
    }))
}

/// `true` when the prim is typed `PhysicsScene`. Predicate kept for
/// callers that don't need the gravity attributes.
pub fn read_is_physics_scene(stage: &Stage, prim: &Path) -> Result<bool> {
    Ok(stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .as_deref()
        == Some(T_PHYSICS_SCENE))
}

/// Read `PhysicsCollisionAPI` (+ optional `PhysicsMeshCollisionAPI`)
/// state on a prim. Returns `None` when CollisionAPI isn't applied.
pub fn read_collision_shape(stage: &Stage, prim: &Path) -> Result<Option<ReadCollisionShape>> {
    let api = read_api_schemas(stage, prim)?;
    if !api.iter().any(|s| s == API_COLLISION) {
        return Ok(None);
    }
    let has_mesh_collision = api.iter().any(|s| s == API_MESH_COLLISION);
    let collision_enabled = match read_attr_value(stage, prim, A_COLLISION_ENABLED)? {
        Some(Value::Bool(b)) => b,
        _ => true, // Pixar spec: CollisionAPI applied implies enabled when unauthored.
    };
    let approximation = if has_mesh_collision {
        read_token(stage, prim, A_APPROXIMATION)?.and_then(|s| CollisionApprox::from_token(&s))
    } else {
        None
    };
    let simulation_owner = read_rel_first_target(stage, prim, A_SIMULATION_OWNER)?;
    // Resolve material binding: physics-purpose first, plain second.
    let physics_material_path = read_rel_first_target(stage, prim, REL_MATERIAL_BINDING_PHYSICS)?
        .or(read_rel_first_target(stage, prim, REL_MATERIAL_BINDING)?);
    Ok(Some(ReadCollisionShape {
        has_collision_api: true,
        has_mesh_collision_api: has_mesh_collision,
        collision_enabled,
        approximation,
        simulation_owner,
        physics_material_path,
    }))
}

/// Read `PhysicsMaterialAPI` on a `Material` prim. Returns `None` unless
/// the prim has `PhysicsMaterialAPI` applied (regardless of typeName, so
/// non-Material prims can carry it too if the author so chose).
pub fn read_physics_material(stage: &Stage, prim: &Path) -> Result<Option<ReadPhysicsMaterial>> {
    if !read_api_schemas(stage, prim)?.iter().any(|s| s == API_PHYSICS_MATERIAL) {
        return Ok(None);
    }
    Ok(Some(ReadPhysicsMaterial {
        path: prim.as_str().to_string(),
        static_friction: read_scalar_f32(stage, prim, A_STATIC_FRICTION)?,
        dynamic_friction: read_scalar_f32(stage, prim, A_DYNAMIC_FRICTION)?,
        restitution: read_scalar_f32(stage, prim, A_RESTITUTION)?,
        density: read_scalar_f32(stage, prim, A_DENSITY)?,
    }))
}

/// Decode every multi-apply `PhysicsLimitAPI:<dof>` instance on a joint.
pub fn read_joint_limits(stage: &Stage, prim: &Path) -> Result<Vec<ReadLimit>> {
    let api = read_api_schemas(stage, prim)?;
    let mut out = Vec::new();
    for name in api {
        let Some(rest) = name.strip_prefix(API_LIMIT) else {
            continue;
        };
        let Some(dof_token) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(dof) = Dof::from_token(dof_token) else {
            continue;
        };
        let low = read_scalar_f32(stage, prim, &format!("limit:{dof_token}:physics:{LIMIT_SUB_LOW}"))?
            .unwrap_or(0.0);
        let high = read_scalar_f32(stage, prim, &format!("limit:{dof_token}:physics:{LIMIT_SUB_HIGH}"))?
            .unwrap_or(0.0);
        out.push(ReadLimit { dof, low, high });
    }
    Ok(out)
}

/// Decode every multi-apply `PhysicsDriveAPI:<dof>` instance on a joint.
pub fn read_joint_drives(stage: &Stage, prim: &Path) -> Result<Vec<ReadDrive>> {
    let api = read_api_schemas(stage, prim)?;
    let mut out = Vec::new();
    for name in api {
        let Some(rest) = name.strip_prefix(API_DRIVE) else {
            continue;
        };
        let Some(dof_token) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(dof) = Dof::from_token(dof_token) else {
            continue;
        };
        let drive_type = read_token(stage, prim, &format!("drive:{dof_token}:physics:{DRIVE_SUB_TYPE}"))?
            .and_then(|s| DriveType::from_token(&s))
            .unwrap_or(DriveType::Force);
        let target_position = read_scalar_f32(
            stage,
            prim,
            &format!("drive:{dof_token}:physics:{DRIVE_SUB_TARGET_POSITION}"),
        )?;
        let target_velocity = read_scalar_f32(
            stage,
            prim,
            &format!("drive:{dof_token}:physics:{DRIVE_SUB_TARGET_VELOCITY}"),
        )?;
        let damping = read_scalar_f32(stage, prim, &format!("drive:{dof_token}:physics:{DRIVE_SUB_DAMPING}"))?
            .unwrap_or(0.0);
        let stiffness = read_scalar_f32(stage, prim, &format!("drive:{dof_token}:physics:{DRIVE_SUB_STIFFNESS}"))?
            .unwrap_or(0.0);
        let max_force = read_scalar_f32(stage, prim, &format!("drive:{dof_token}:physics:{DRIVE_SUB_MAX_FORCE}"))?;
        out.push(ReadDrive {
            dof,
            drive_type,
            target_position,
            target_velocity,
            damping,
            stiffness,
            max_force,
        });
    }
    Ok(out)
}

/// Read any `Physics*Joint` prim. Returns `None` when the prim's
/// typeName isn't a known joint type.
pub fn read_joint(stage: &Stage, prim: &Path) -> Result<Option<ReadJoint>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    let kind = match type_name.as_str() {
        T_PHYSICS_FIXED_JOINT => JointKind::Fixed,
        T_PHYSICS_REVOLUTE_JOINT => JointKind::Revolute,
        T_PHYSICS_PRISMATIC_JOINT => JointKind::Prismatic,
        T_PHYSICS_SPHERICAL_JOINT => JointKind::Spherical,
        T_PHYSICS_DISTANCE_JOINT => JointKind::Distance,
        T_PHYSICS_JOINT => JointKind::Generic,
        _ => return Ok(None),
    };
    let body0 = read_rel_first_target(stage, prim, A_BODY0)?;
    let body1 = read_rel_first_target(stage, prim, A_BODY1)?;
    let local_pos0 = read_attr_value(stage, prim, A_LOCAL_POS_0)?
        .and_then(value_to_vec3f)
        .unwrap_or([0.0; 3]);
    let local_pos1 = read_attr_value(stage, prim, A_LOCAL_POS_1)?
        .and_then(value_to_vec3f)
        .unwrap_or([0.0; 3]);
    let local_rot0 = read_attr_value(stage, prim, A_LOCAL_ROT_0)?
        .and_then(value_to_quatf)
        .unwrap_or([1.0, 0.0, 0.0, 0.0]);
    let local_rot1 = read_attr_value(stage, prim, A_LOCAL_ROT_1)?
        .and_then(value_to_quatf)
        .unwrap_or([1.0, 0.0, 0.0, 0.0]);
    let axis = read_token(stage, prim, A_AXIS)?;
    let lower_limit = read_scalar_f32(stage, prim, A_LOWER_LIMIT)?;
    let upper_limit = read_scalar_f32(stage, prim, A_UPPER_LIMIT)?;
    let collision_enabled = matches!(
        read_attr_value(stage, prim, A_JOINT_COLLISION_ENABLED)?,
        Some(Value::Bool(true))
    );
    let joint_enabled = match read_attr_value(stage, prim, A_JOINT_ENABLED)? {
        Some(Value::Bool(b)) => b,
        _ => true, // Pixar default
    };
    let exclude_from_articulation = matches!(
        read_attr_value(stage, prim, A_EXCLUDE_FROM_ARTICULATION)?,
        Some(Value::Bool(true))
    );
    let break_force = read_scalar_f32(stage, prim, A_BREAK_FORCE)?;
    let break_torque = read_scalar_f32(stage, prim, A_BREAK_TORQUE)?;
    let min_distance = read_scalar_f32(stage, prim, A_MIN_DISTANCE)?;
    let max_distance = read_scalar_f32(stage, prim, A_MAX_DISTANCE)?;
    let cone_angle_0 = read_scalar_f32(stage, prim, A_CONE_ANGLE_0_LIMIT)?;
    let cone_angle_1 = read_scalar_f32(stage, prim, A_CONE_ANGLE_1_LIMIT)?;
    let limits = read_joint_limits(stage, prim)?;
    let drives = read_joint_drives(stage, prim)?;

    Ok(Some(ReadJoint {
        path: prim.as_str().to_string(),
        kind,
        body0,
        body1,
        local_pos0,
        local_rot0,
        local_pos1,
        local_rot1,
        axis,
        lower_limit,
        upper_limit,
        collision_enabled,
        joint_enabled,
        exclude_from_articulation,
        break_force,
        break_torque,
        min_distance,
        max_distance,
        cone_angle_0,
        cone_angle_1,
        limits,
        drives,
    }))
}

/// Read a `PhysicsCollisionGroup` prim. Returns `None` when the typeName
/// doesn't match.
///
/// Note: full UsdCollectionAPI rule evaluation (includes / excludes /
/// expansion-rule semantics) is a v2 follow-up. For v1 we read the
/// explicit `collection:colliders:includes` target list only — adequate
/// for the common authoring pattern.
pub fn read_collision_group(stage: &Stage, prim: &Path) -> Result<Option<ReadCollisionGroup>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != T_PHYSICS_COLLISION_GROUP {
        return Ok(None);
    }
    let members = read_rel_all_targets(stage, prim, "collection:colliders:includes")?;
    let filtered_groups = read_rel_all_targets(stage, prim, A_FILTERED_GROUPS)?;
    let merge_group = read_token(stage, prim, A_MERGE_GROUP)?;
    let invert_filtered_groups = matches!(
        read_attr_value(stage, prim, A_INVERT_FILTERED_GROUPS)?,
        Some(Value::Bool(true))
    );
    Ok(Some(ReadCollisionGroup {
        path: prim.as_str().to_string(),
        members,
        filtered_groups,
        merge_group,
        invert_filtered_groups,
    }))
}

/// Read `PhysicsFilteredPairsAPI` on a body prim. Returns `None` unless
/// the API is applied.
pub fn read_filtered_pairs(stage: &Stage, prim: &Path) -> Result<Option<ReadFilteredPairs>> {
    if !read_api_schemas(stage, prim)?.iter().any(|s| s == API_FILTERED_PAIRS) {
        return Ok(None);
    }
    let filtered = read_rel_all_targets(stage, prim, A_FILTERED_PAIRS)?;
    Ok(Some(ReadFilteredPairs {
        path: prim.as_str().to_string(),
        filtered,
    }))
}

/// Walk the entire stage once and return categorised path lists.
/// Saves the projection layer from re-walking for every schema family.
pub fn find_physics_prims(stage: &Stage) -> Result<PhysicsPrims> {
    let mut out = PhysicsPrims::default();
    stage
        .traverse(|path| {
            if let Ok(Some(type_name)) = stage.field::<String>(path.clone(), "typeName") {
                match type_name.as_str() {
                    T_PHYSICS_SCENE => out.scenes.push(path.as_str().to_string()),
                    T_PHYSICS_JOINT
                    | T_PHYSICS_FIXED_JOINT
                    | T_PHYSICS_REVOLUTE_JOINT
                    | T_PHYSICS_PRISMATIC_JOINT
                    | T_PHYSICS_SPHERICAL_JOINT
                    | T_PHYSICS_DISTANCE_JOINT => out.joints.push(path.as_str().to_string()),
                    T_PHYSICS_COLLISION_GROUP => {
                        out.collision_groups.push(path.as_str().to_string())
                    }
                    _ => {}
                }
            }
            if let Ok(api) = read_api_schemas(stage, path) {
                let p = path.as_str().to_string();
                if api.iter().any(|s| s == API_RIGID_BODY) {
                    out.rigid_bodies.push(p.clone());
                }
                if api.iter().any(|s| s == API_ARTICULATION_ROOT) {
                    out.articulation_roots.push(p.clone());
                }
                if api.iter().any(|s| s == API_COLLISION) {
                    out.colliders.push(p.clone());
                }
                if api.iter().any(|s| s == API_PHYSICS_MATERIAL) {
                    out.materials.push(p.clone());
                }
                if api.iter().any(|s| s == API_FILTERED_PAIRS) {
                    out.filtered_pairs.push(p);
                }
            }
        })
        .map_err(anyhow::Error::from)?;
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════
//                            reader helpers
// ════════════════════════════════════════════════════════════════════════

fn read_attr_value(stage: &Stage, prim: &Path, name: &str) -> Result<Option<Value>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)
}

fn read_scalar_f32(stage: &Stage, prim: &Path, name: &str) -> Result<Option<f32>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Float(f)) => Some(f),
        Some(Value::Double(d)) => Some(d as f32),
        _ => None,
    })
}

fn read_token(stage: &Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_rel_first_target(stage: &Stage, prim: &Path, rel_name: &str) -> Result<Option<String>> {
    Ok(read_rel_all_targets(stage, prim, rel_name)?.into_iter().next())
}

fn read_rel_all_targets(stage: &Stage, prim: &Path, rel_name: &str) -> Result<Vec<String>> {
    let rel_path = prim.append_property(rel_name).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(rel_path, "targetPaths")
        .map_err(anyhow::Error::from)?;
    let paths = match raw {
        Some(Value::PathListOp(op)) => op.flatten(),
        Some(Value::PathVec(v)) => v,
        _ => Vec::new(),
    };
    Ok(paths.into_iter().map(|p| p.as_str().to_string()).collect())
}

fn value_to_vec3f(v: Value) -> Option<[f32; 3]> {
    match v {
        Value::Vec3f(a) => Some(a),
        Value::Vec3d(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32]),
        _ => None,
    }
}

fn value_to_quatf(v: Value) -> Option<[f32; 4]> {
    match v {
        Value::Quatf(q) => Some(q),
        Value::Quatd(q) => Some([q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]),
        _ => None,
    }
}
