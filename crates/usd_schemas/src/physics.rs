//! UsdPhysics authoring: Scene, joints, and the apiSchemas + per-schema
//! attribute sets for RigidBodyAPI / MassAPI / CollisionAPI / MeshCollisionAPI
//! / ArticulationRootAPI.

use openusd::sdf::{Path, Value};

use anyhow::Result;

use super::Stage;
use super::tokens::*;

pub fn define_scene(stage: &mut Stage, parent: &Path, name: &str) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_SCENE)?;
    // Match the Python converter: apply NewtonSceneAPI so downstream
    // Newton importers initialize the scene correctly.
    stage.apply_api_schemas(&p, &[API_NEWTON_SCENE])?;
    Ok(p)
}

pub fn apply_rigid_body(stage: &mut Stage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_RIGID_BODY])
}

pub fn apply_articulation_root(stage: &mut Stage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_ARTICULATION_ROOT, API_NEWTON_ARTICULATION_ROOT])
}

pub struct MassProps {
    pub mass: f64,
    pub center_of_mass: [f64; 3],
    pub diagonal_inertia: [f32; 3],
    /// Quaternion `(w, x, y, z)` of the principal-axes frame.
    pub principal_axes: [f32; 4],
}

pub fn apply_mass(stage: &mut Stage, prim: &Path, props: &MassProps) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_MASS])?;
    stage.define_attribute(
        prim,
        "physics:mass",
        "float",
        Value::Float(props.mass as f32),
        false,
    )?;
    stage.define_attribute(
        prim,
        "physics:centerOfMass",
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
        "physics:diagonalInertia",
        "float3",
        Value::Vec3f(props.diagonal_inertia),
        false,
    )?;
    stage.define_attribute(
        prim,
        "physics:principalAxes",
        "quatf",
        Value::Quatf(props.principal_axes),
        false,
    )?;
    Ok(())
}

pub fn apply_collision(stage: &mut Stage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_COLLISION])
}

/// For mesh collisions, apply both CollisionAPI and MeshCollisionAPI, and
/// author `physics:approximation = "convexHull"` which matches the Python
/// converter default.
pub fn apply_mesh_collision_convex_hull(stage: &mut Stage, prim: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_COLLISION, API_MESH_COLLISION])?;
    stage.define_attribute(
        prim,
        "physics:approximation",
        "token",
        Value::Token("convexHull".into()),
        true,
    )
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

pub fn author_joint_frame(stage: &mut Stage, joint: &Path, f: &JointFrame) -> Result<()> {
    if let Some(b0) = &f.body0 {
        stage.define_relationship(joint, "physics:body0", vec![b0.clone()])?;
    }
    if let Some(b1) = &f.body1 {
        stage.define_relationship(joint, "physics:body1", vec![b1.clone()])?;
    }
    stage.define_attribute(
        joint,
        "physics:localPos0",
        "point3f",
        Value::Vec3f(f.local_pos0),
        false,
    )?;
    stage.define_attribute(
        joint,
        "physics:localRot0",
        "quatf",
        Value::Quatf(f.local_rot0),
        false,
    )?;
    stage.define_attribute(
        joint,
        "physics:localPos1",
        "point3f",
        Value::Vec3f(f.local_pos1),
        false,
    )?;
    stage.define_attribute(
        joint,
        "physics:localRot1",
        "quatf",
        Value::Quatf(f.local_rot1),
        false,
    )?;
    Ok(())
}

pub fn define_fixed_joint(
    stage: &mut Stage,
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
/// into `localRot` via [`quat_from_x_to`].
pub fn axis_token(axis: [f64; 3]) -> &'static str {
    // Only return a non-X token when the axis is essentially canonical
    // (within 1e-6 of an axis-aligned unit vector, sign-independent). For
    // anything else we return "X" and expect the joint frame to be rotated.
    const EPS: f64 = 1e-6;
    let is_axis_x = axis[1].abs() < EPS && axis[2].abs() < EPS && axis[0].abs() > EPS;
    let is_axis_y = axis[0].abs() < EPS && axis[2].abs() < EPS && axis[1].abs() > EPS;
    let is_axis_z = axis[0].abs() < EPS && axis[1].abs() < EPS && axis[2].abs() > EPS;
    if is_axis_y {
        "Y"
    } else if is_axis_z {
        "Z"
    } else if is_axis_x {
        "X"
    } else {
        "X"
    }
}

pub struct JointLimit {
    pub lower: f64,
    pub upper: f64,
}

pub fn define_revolute_joint(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    axis: &str,
    limits: Option<JointLimit>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_REVOLUTE_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    stage.define_attribute(&p, "physics:axis", "token", Value::Token(axis.into()), true)?;
    if let Some(l) = limits {
        // Revolute limits are in DEGREES in UsdPhysics.
        stage.define_attribute(
            &p,
            "physics:lowerLimit",
            "float",
            Value::Float(l.lower.to_degrees() as f32),
            false,
        )?;
        stage.define_attribute(
            &p,
            "physics:upperLimit",
            "float",
            Value::Float(l.upper.to_degrees() as f32),
            false,
        )?;
    }
    Ok(p)
}

pub fn define_prismatic_joint(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
    axis: &str,
    limits: Option<JointLimit>,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_PRISMATIC_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    stage.define_attribute(&p, "physics:axis", "token", Value::Token(axis.into()), true)?;
    if let Some(l) = limits {
        stage.define_attribute(
            &p,
            "physics:lowerLimit",
            "float",
            Value::Float(l.lower as f32),
            false,
        )?;
        stage.define_attribute(
            &p,
            "physics:upperLimit",
            "float",
            Value::Float(l.upper as f32),
            false,
        )?;
    }
    Ok(p)
}

/// Apply `NewtonMimicAPI` to a joint that mimics another joint's position.
/// `target_joint` is the prim path of the joint being followed. Encodes the
/// URDF `<mimic>` relationship as `newton:mimicCoef0` (offset),
/// `newton:mimicCoef1` (multiplier), and `rel newton:mimicJoint`.
pub fn apply_mimic(
    stage: &mut Stage,
    joint: &Path,
    target_joint: &Path,
    multiplier: f64,
    offset: f64,
) -> Result<()> {
    stage.apply_api_schemas(joint, &[API_NEWTON_MIMIC])?;
    stage.define_attribute(
        joint,
        "newton:mimicCoef0",
        "float",
        Value::Float(offset as f32),
        false,
    )?;
    stage.define_attribute(
        joint,
        "newton:mimicCoef1",
        "float",
        Value::Float(multiplier as f32),
        false,
    )?;
    stage.define_relationship(joint, "newton:mimicJoint", vec![target_joint.clone()])?;
    Ok(())
}

/// Generic `PhysicsJoint` — no built-in axis/limit. Used as the base for
/// planar joints, which author their own `LimitAPI` constraints for
/// translation and rotation on specific DOFs.
pub fn define_generic_joint(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    frame: &JointFrame,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_PHYSICS_JOINT)?;
    author_joint_frame(stage, &p, frame)?;
    Ok(p)
}

/// Apply `PhysicsLimitAPI` to a joint for a specific DOF token
/// (`"transX"`, `"transY"`, `"transZ"`, `"rotX"`, `"rotY"`, `"rotZ"`).
///
/// Passing `lower > upper` encodes a locked DOF (canonical USD convention).
/// The resulting prim shape is:
/// ```text
/// float limit:<dof>:physics:low = <lower>
/// float limit:<dof>:physics:high = <upper>
/// ```
pub fn apply_limit(
    stage: &mut Stage,
    joint: &Path,
    dof: &str,
    lower: f64,
    upper: f64,
) -> Result<()> {
    // LimitAPI is a multi-apply schema; its applied name encodes the DOF.
    // We emit `apiSchemas += "PhysicsLimitAPI:<dof>"` and the per-dof attrs.
    let applied = format!("{API_LIMIT}:{dof}");
    stage.apply_api_schemas(joint, &[&applied])?;
    stage.define_attribute(
        joint,
        &format!("limit:{dof}:physics:low"),
        "float",
        Value::Float(lower as f32),
        false,
    )?;
    stage.define_attribute(
        joint,
        &format!("limit:{dof}:physics:high"),
        "float",
        Value::Float(upper as f32),
        false,
    )?;
    Ok(())
}

// ── Readers ──────────────────────────────────────────────────────────

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

/// Joint prim types we recognise. `Generic` is `PhysicsJoint` (no axis),
/// typically paired with explicit `PhysicsLimitAPI` constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointKind {
    Fixed,
    Revolute,
    Prismatic,
    Spherical,
    Generic,
}

/// Decoded joint. `axis` is `"X" | "Y" | "Z"` when set; `limit` is
/// `Some` when the prim authored built-in `physics:lowerLimit` /
/// `upperLimit` (not the multi-apply LimitAPI).
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
    /// `physics:collisionEnabled` — defaults to false when unauthored.
    pub collision_enabled: bool,
}

/// Flattened list of applied API schemas on `prim`.
pub fn read_api_schemas(stage: &openusd::Stage, prim: &Path) -> Result<Vec<String>> {
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
pub fn read_has_rigid_body(stage: &openusd::Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?
        .iter()
        .any(|s| s == API_RIGID_BODY))
}

/// `true` when `PhysicsCollisionAPI` is applied.
pub fn read_has_collision(stage: &openusd::Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?
        .iter()
        .any(|s| s == API_COLLISION))
}

/// `true` when `PhysicsArticulationRootAPI` is applied.
pub fn read_has_articulation_root(stage: &openusd::Stage, prim: &Path) -> Result<bool> {
    Ok(read_api_schemas(stage, prim)?
        .iter()
        .any(|s| s == API_ARTICULATION_ROOT))
}

/// Read `PhysicsMassAPI` attributes. Returns `None` when the prim hasn't
/// applied `MassAPI` (so callers can distinguish "unauthored" from "zero
/// mass").
pub fn read_mass(stage: &openusd::Stage, prim: &Path) -> Result<Option<ReadMass>> {
    if !read_api_schemas(stage, prim)?.iter().any(|s| s == API_MASS) {
        return Ok(None);
    }
    let mass = read_scalar_f32(stage, prim, "physics:mass")?;
    let center_of_mass = read_attr_value(stage, prim, "physics:centerOfMass")?.and_then(value_to_vec3f);
    let diagonal_inertia =
        read_attr_value(stage, prim, "physics:diagonalInertia")?.and_then(value_to_vec3f);
    let principal_axes =
        read_attr_value(stage, prim, "physics:principalAxes")?.and_then(value_to_quatf);
    let density = read_scalar_f32(stage, prim, "physics:density")?;
    Ok(Some(ReadMass {
        mass,
        center_of_mass,
        diagonal_inertia,
        principal_axes,
        density,
    }))
}

/// Read any `Physics*Joint` prim. Returns `None` when the prim's
/// typeName isn't a known joint type.
pub fn read_joint(stage: &openusd::Stage, prim: &Path) -> Result<Option<ReadJoint>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    let kind = match type_name.as_str() {
        T_PHYSICS_FIXED_JOINT => JointKind::Fixed,
        T_PHYSICS_REVOLUTE_JOINT => JointKind::Revolute,
        T_PHYSICS_PRISMATIC_JOINT => JointKind::Prismatic,
        T_PHYSICS_SPHERICAL_JOINT => JointKind::Spherical,
        T_PHYSICS_JOINT => JointKind::Generic,
        _ => return Ok(None),
    };
    let body0 = read_rel_first_target(stage, prim, "physics:body0")?;
    let body1 = read_rel_first_target(stage, prim, "physics:body1")?;
    let local_pos0 =
        read_attr_value(stage, prim, "physics:localPos0")?.and_then(value_to_vec3f).unwrap_or([0.0; 3]);
    let local_pos1 =
        read_attr_value(stage, prim, "physics:localPos1")?.and_then(value_to_vec3f).unwrap_or([0.0; 3]);
    let local_rot0 = read_attr_value(stage, prim, "physics:localRot0")?
        .and_then(value_to_quatf)
        .unwrap_or([1.0, 0.0, 0.0, 0.0]);
    let local_rot1 = read_attr_value(stage, prim, "physics:localRot1")?
        .and_then(value_to_quatf)
        .unwrap_or([1.0, 0.0, 0.0, 0.0]);
    let axis = read_token(stage, prim, "physics:axis")?;
    let lower_limit = read_scalar_f32(stage, prim, "physics:lowerLimit")?;
    let upper_limit = read_scalar_f32(stage, prim, "physics:upperLimit")?;
    let collision_enabled = matches!(
        read_attr_value(stage, prim, "physics:collisionEnabled")?,
        Some(Value::Bool(true))
    );
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
    }))
}

/// `true` when the prim is typed `PhysicsScene`.
pub fn read_is_physics_scene(stage: &openusd::Stage, prim: &Path) -> Result<bool> {
    Ok(stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .as_deref()
        == Some(T_PHYSICS_SCENE))
}

// ── reader helpers ─────────────────────────────────────────────────

fn read_attr_value(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Option<Value>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)
}

fn read_scalar_f32(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<f32>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Float(f)) => Some(f),
        Some(Value::Double(d)) => Some(d as f32),
        _ => None,
    })
}

fn read_token(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_rel_first_target(
    stage: &openusd::Stage,
    prim: &Path,
    rel_name: &str,
) -> Result<Option<String>> {
    let rel_path = prim.append_property(rel_name).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(rel_path, "targetPaths")
        .map_err(anyhow::Error::from)?;
    let paths = match raw {
        Some(Value::PathListOp(op)) => op.flatten(),
        Some(Value::PathVec(v)) => v,
        _ => Vec::new(),
    };
    Ok(paths.into_iter().next().map(|p| p.as_str().to_string()))
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
