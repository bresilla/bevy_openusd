//! Xform composition + readers + writers.
//!
//! Reader half: read `xformOp:translate / orient / scale` and the
//! `rotateXYZ` Euler ops + matrix `xformOp:transform` from a composed
//! [`openusd::Stage`], compose them per `xformOpOrder`, return TRS.
//!
//! Writer half: emit those ops onto a `Stage` prim spec — used by
//! the URDF→USD authoring path.

use anyhow::Result;
use openusd::sdf::{Path, Value};

use crate::Stage as SchemaStage;
use crate::math::rpy_to_quat;

/// URDF-style `<origin xyz="..." rpy="..."/>` pose, in radians.
pub struct Pose {
    pub xyz: [f64; 3],
    pub rpy: [f64; 3],
}

impl Pose {
    pub fn identity() -> Self {
        Self {
            xyz: [0.0; 3],
            rpy: [0.0; 3],
        }
    }

    pub fn new(xyz: [f64; 3], rpy: [f64; 3]) -> Self {
        Self { xyz, rpy }
    }
}

// ── Writers ──────────────────────────────────────────────────────────────

/// Write TRS ops on `prim` from a URDF origin. Omits any op that is identity.
pub fn set_pose(stage: &mut SchemaStage, prim: &Path, pose: &Pose) -> Result<()> {
    set_trs(stage, prim, pose, None)
}

/// Write TRS ops with an optional non-uniform scale.
pub fn set_trs(
    stage: &mut SchemaStage,
    prim: &Path,
    pose: &Pose,
    scale: Option<[f64; 3]>,
) -> Result<()> {
    let mut order: Vec<String> = Vec::new();

    let translate_identity = pose.xyz == [0.0, 0.0, 0.0];
    let rotate_identity = pose.rpy == [0.0, 0.0, 0.0];
    let scale_identity = scale.is_none_or(|s| s == [1.0, 1.0, 1.0]);

    if !translate_identity {
        stage.define_attribute(
            prim,
            "xformOp:translate",
            "double3",
            Value::Vec3d(pose.xyz),
            false,
        )?;
        order.push("xformOp:translate".into());
    }

    if !rotate_identity {
        let q = rpy_to_quat(pose.rpy[0], pose.rpy[1], pose.rpy[2]);
        let q_f: [f32; 4] = [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32];
        stage.define_attribute(prim, "xformOp:orient", "quatf", Value::Quatf(q_f), false)?;
        order.push("xformOp:orient".into());
    }

    if !scale_identity {
        let s = scale.unwrap();
        stage.define_attribute(prim, "xformOp:scale", "double3", Value::Vec3d(s), false)?;
        order.push("xformOp:scale".into());
    }

    if !order.is_empty() {
        stage.define_attribute(
            prim,
            "xformOpOrder",
            "token[]",
            Value::TokenVec(order),
            true,
        )?;
    }
    Ok(())
}

/// Define an `Xform` child of `parent` and apply a pose. Returns the prim path.
pub fn define_xform(
    stage: &mut SchemaStage,
    parent: &Path,
    name: &str,
    pose: &Pose,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, super::tokens::T_XFORM)?;
    set_pose(stage, &p, pose)?;
    Ok(p)
}

// ── Readers ──────────────────────────────────────────────────────────────

/// TRS evaluation of a prim's `xformOp` stack. Translation in metres,
/// rotation as a quaternion in `(x, y, z, w)` layout (USD's native
/// `Quatf` is `(w, x, y, z)` — conversion is done here), non-uniform
/// scale.
#[derive(Debug, Clone, PartialEq)]
pub struct Transform3 {
    pub translate: [f32; 3],
    pub rotate: [f32; 4],
    pub scale: [f32; 3],
}

impl Default for Transform3 {
    fn default() -> Self {
        Self {
            translate: [0.0; 3],
            rotate: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

/// Read `xformOpOrder` and compose every listed op into a single 4×4,
/// then decompose to TRS. Returns `None` when no `xformOpOrder` is
/// authored on the prim.
pub fn read_transform(stage: &openusd::Stage, prim: &Path) -> Result<Option<Transform3>> {
    use glam::Mat4;

    let order_attr = prim
        .append_property("xformOpOrder")
        .map_err(anyhow::Error::from)?;
    let Some(raw) = stage
        .field::<Value>(order_attr, "default")
        .map_err(anyhow::Error::from)?
    else {
        return Ok(None);
    };
    let order: Vec<String> = match raw {
        Value::TokenVec(v) | Value::StringVec(v) => v,
        // URDF→USD exports (Agilex Scout V2, Isaac Sim assets) author
        // `xformOpOrder` as a list-op rather than a plain vec.
        Value::TokenListOp(op) => op.flatten(),
        _ => return Ok(None),
    };

    let mut m = Mat4::IDENTITY;
    for op in &order {
        let op_m = build_op_matrix(stage, prim, op)?;
        m = m * op_m;
    }

    let (s, r, t) = m.to_scale_rotation_translation();
    Ok(Some(Transform3 {
        translate: [t.x, t.y, t.z],
        rotate: [r.x, r.y, r.z, r.w],
        scale: [s.x, s.y, s.z],
    }))
}

/// Build the 4×4 matrix that this single xformOp contributes.
/// Handles `!invert!` prefix, namespaced suffixes, and per-kind
/// value parsing (translate / scale / orient / rotateAXIS / rotateEULER /
/// transform).
fn build_op_matrix(stage: &openusd::Stage, prim: &Path, op_token: &str) -> Result<glam::Mat4> {
    use glam::{Mat4, Quat, Vec3};

    const INVERT: &str = "!invert!";
    let (inverted, base) = if let Some(stripped) = op_token.strip_prefix(INVERT) {
        (true, stripped)
    } else {
        (false, op_token)
    };

    let attr_path = prim.append_property(base).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?;
    let Some(raw) = raw else {
        return Ok(Mat4::IDENTITY);
    };

    let kind = base.strip_prefix("xformOp:").unwrap_or(base);
    let kind = kind.split(':').next().unwrap_or(kind);

    let m = match kind {
        "translate" => {
            let v = value_to_vec3f(&raw).unwrap_or([0.0, 0.0, 0.0]);
            Mat4::from_translation(Vec3::from(v))
        }
        "scale" => {
            let v = value_to_vec3f(&raw).unwrap_or([1.0, 1.0, 1.0]);
            Mat4::from_scale(Vec3::from(v))
        }
        "orient" => {
            let q = value_to_quat_wxyz(&raw).unwrap_or([1.0, 0.0, 0.0, 0.0]);
            Mat4::from_quat(Quat::from_xyzw(q[1], q[2], q[3], q[0]))
        }
        "rotateX" => {
            let deg = value_to_scalar_f32(&raw).unwrap_or(0.0);
            Mat4::from_rotation_x(deg.to_radians())
        }
        "rotateY" => {
            let deg = value_to_scalar_f32(&raw).unwrap_or(0.0);
            Mat4::from_rotation_y(deg.to_radians())
        }
        "rotateZ" => {
            let deg = value_to_scalar_f32(&raw).unwrap_or(0.0);
            Mat4::from_rotation_z(deg.to_radians())
        }
        "rotateXYZ" | "rotateYXZ" | "rotateZXY" | "rotateXZY" | "rotateYZX" | "rotateZYX" => {
            let v = value_to_vec3f(&raw).unwrap_or([0.0, 0.0, 0.0]);
            let rx = v[0].to_radians();
            let ry = v[1].to_radians();
            let rz = v[2].to_radians();
            let rx_m = Mat4::from_rotation_x(rx);
            let ry_m = Mat4::from_rotation_y(ry);
            let rz_m = Mat4::from_rotation_z(rz);
            match kind {
                "rotateXYZ" => rz_m * ry_m * rx_m,
                "rotateYXZ" => rz_m * rx_m * ry_m,
                "rotateZXY" => ry_m * rx_m * rz_m,
                "rotateXZY" => ry_m * rz_m * rx_m,
                "rotateYZX" => rx_m * rz_m * ry_m,
                "rotateZYX" => rx_m * ry_m * rz_m,
                _ => unreachable!(),
            }
        }
        "transform" => value_to_mat4_glam(&raw).unwrap_or(Mat4::IDENTITY),
        _ => Mat4::IDENTITY,
    };

    Ok(if inverted { m.inverse() } else { m })
}

fn value_to_mat4_glam(v: &Value) -> Option<glam::Mat4> {
    use glam::Mat4;
    match v {
        Value::Matrix4d(m) => {
            let cols: [f32; 16] = std::array::from_fn(|i| m[i] as f32);
            Some(Mat4::from_cols_array(&cols))
        }
        _ => None,
    }
}

fn value_to_vec3f(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::Vec3f(a) => Some(*a),
        Value::Vec3d(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32]),
        _ => None,
    }
}

fn value_to_scalar_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Float(f) => Some(*f),
        Value::Double(d) => Some(*d as f32),
        Value::Int(i) => Some(*i as f32),
        Value::Int64(i) => Some(*i as f32),
        _ => None,
    }
}

fn value_to_quat_wxyz(v: &Value) -> Option<[f32; 4]> {
    match v {
        Value::Quatf(q) => Some(*q),
        Value::Quatd(q) => Some([q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]),
        _ => None,
    }
}
