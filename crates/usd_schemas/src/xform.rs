//! Xform authoring helpers.
//!
//! We emit a TRS op stack: `xformOp:translate`, `xformOp:orient`,
//! (`xformOp:scale` when provided), listed on `xformOpOrder`.

use openusd::sdf::{Path, Value};

use crate::math::rpy_to_quat;
use anyhow::Result;

use super::Stage;

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

/// Write TRS ops on `prim` from a URDF origin. Omits any op that is identity.
pub fn set_pose(stage: &mut Stage, prim: &Path, pose: &Pose) -> Result<()> {
    set_trs(stage, prim, pose, None)
}

/// Write TRS ops with an optional non-uniform scale (e.g. URDF `<box size="...">`
/// baked into the xform).
pub fn set_trs(stage: &mut Stage, prim: &Path, pose: &Pose, scale: Option<[f64; 3]>) -> Result<()> {
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
        // USD Quatf is stored as [real, imag.x, imag.y, imag.z] = [w, x, y, z]
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
pub fn define_xform(stage: &mut Stage, parent: &Path, name: &str, pose: &Pose) -> Result<Path> {
    let p = stage.define_prim(parent, name, super::tokens::T_XFORM)?;
    set_pose(stage, &p, pose)?;
    Ok(p)
}

// ── Readers ──────────────────────────────────────────────────────────────

/// TRS evaluation of a prim's `xformOp` stack. Returned in Bevy-friendly form:
/// translation in metres, rotation as a quaternion in `(x, y, z, w)` layout
/// (USD's native `Quatf` is `(w, x, y, z)` — conversion is done here), and
/// non-uniform scale.
///
/// Extra `xformOp:transform` matrices are post-multiplied into a `residual`
/// column-major 4×4 so callers can assemble a final `Mat4 = T·R·S·residual`
/// if they need full fidelity. M2 leaves `residual` as identity.
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

/// Evaluate the `xformOp` stack on `prim` and return the composed transform.
///
/// Supported op tokens (anything else is skipped with a debug log — the
/// renderer won't surprise the user with misapplied matrices):
/// - `xformOp:translate`              — `double3` / `float3`
/// - `xformOp:orient`                 — `quatf` / `quatd` (native `(w,x,y,z)`)
/// - `xformOp:rotateXYZ` etc.         — `float3` Euler degrees, XYZ order
/// - `xformOp:scale`                  — `double3` / `float3`
///
/// Returns `None` when the prim has no `xformOpOrder` (i.e. a plain prim).
/// Returns `Some(Transform3::default())` when every op evaluates to identity.
///
/// Composes the full xformOp stack into a single 4×4 matrix and
/// decomposes to TRS at the end. Each op contributes its own matrix
/// (translate, scale, rotate, full-matrix); they multiply in
/// `xformOpOrder` order such that the FIRST listed op is OUTERMOST
/// (applied LAST to a column vector) per USD's semantic. This shape
/// is the only one that supports:
///
/// - **Pivoted rotation**: `[translate, translate:pivot, rotateXYZ,
///   !invert!translate:pivot]` — the namespaced pivot ops + the
///   invert prefix produce the canonical "rotate around pivot" idiom
///   Pixar's Kitchen_set authors on every leaf mesh. The previous
///   field-overwrite reader silently dropped the `:pivot` /
///   `!invert!` ops and produced badly-placed geometry.
/// - **Multiple translates** in the same op stack (treated as
///   compositional offsets, not last-wins overwrites).
/// - **Inverted ops** generally — anything prefixed with `!invert!`
///   reads the named attribute and inverts the resulting matrix.
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
        _ => return Ok(None),
    };

    let mut m = Mat4::IDENTITY;
    for op in &order {
        let op_m = build_op_matrix(stage, prim, op)?;
        // Ops listed earlier are OUTER. For a column vector p,
        // composed = M_1 * M_2 * ... * M_n * p.  So we right-multiply
        // each op's matrix in iteration order.
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
/// Handles three concerns the caller doesn't:
///
/// 1. **`!invert!` prefix** — strips the prefix, reads the named
///    attribute, and inverts the resulting matrix (per USD spec).
/// 2. **Namespaced suffixes** — `xformOp:translate:pivot`,
///    `xformOp:rotateZ:foo`, etc. The PROPERTY name is the full
///    token; the OP KIND is determined by the second `:`-separated
///    segment (`translate`, `rotateZ`, `scale`, `transform`,
///    `orient`, `rotateXYZ`, …).
/// 3. **Per-kind value parsing** — translate/scale/orient as vec3
///    or quat, rotateAXIS as scalar degrees, rotateEULER as vec3
///    degrees, transform as matrix4d.
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
        // Attribute missing — treat as identity (per USD: ops with
        // no value act as no-op).
        return Ok(Mat4::IDENTITY);
    };

    // Op kind is the second `:`-separated segment of the property
    // name. `xformOp:translate` → `translate`,
    // `xformOp:translate:pivot` → `translate`.
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
            // USD `Quatf` is (w, x, y, z); glam Quat::from_xyzw expects (x, y, z, w).
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
            // Each token names the order in which the per-axis
            // rotations are applied to a vector. `XYZ` means
            // rotate-X first, then Y, then Z. With column vectors
            // the matrix product is Z * Y * X.
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
        // openusd's `Matrix4d` is a flat `[f64; 16]` in column-major
        // order (matches OpenUSD's authoring convention).
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

/// Decode `matrix4d` / `matrix4f` (USD's column-major 4×4) to `[f32; 16]`.
fn value_to_mat4f_col_major(v: &Value) -> Option<[f32; 16]> {
    match v {
        Value::Matrix4d(m) => {
            let mut out = [0.0f32; 16];
            for i in 0..16 {
                out[i] = m[i] as f32;
            }
            Some(out)
        }
        _ => None,
    }
}

/// Decompose a column-major 4×4 matrix into (translation, rotation (x,y,z,w),
/// scale). Follows the glam / Bevy convention:
///
/// - Translation = column 3 (`m[12], m[13], m[14]`).
/// - Scale = lengths of columns 0..2.
/// - Rotation = the remaining orthogonal basis after dividing each column by
///   its scale, converted to a quaternion.
///
/// Handles the flip case: if `det(R) < 0`, negates one column's scale so the
/// rotation stays proper.
fn decompose_mat4_cm(m: &[f32; 16]) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let c0 = [m[0], m[1], m[2]];
    let c1 = [m[4], m[5], m[6]];
    let c2 = [m[8], m[9], m[10]];
    let translate = [m[12], m[13], m[14]];

    let len = |c: &[f32; 3]| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let mut sx = len(&c0);
    let sy = len(&c1);
    let sz = len(&c2);

    // det(3x3) to detect mirrored scale; keep it on sx (arbitrary).
    let det = c0[0] * (c1[1] * c2[2] - c1[2] * c2[1]) - c0[1] * (c1[0] * c2[2] - c1[2] * c2[0])
        + c0[2] * (c1[0] * c2[1] - c1[1] * c2[0]);
    if det < 0.0 {
        sx = -sx;
    }

    let n = |c: [f32; 3], s: f32| {
        if s.abs() < 1e-8 {
            [0.0, 0.0, 0.0]
        } else {
            [c[0] / s, c[1] / s, c[2] / s]
        }
    };
    let r0 = n(c0, sx);
    let r1 = n(c1, sy);
    let r2 = n(c2, sz);

    let rotate = quat_from_rot_columns(&r0, &r1, &r2);
    (translate, rotate, [sx, sy, sz])
}

/// Build a quaternion `(x, y, z, w)` from an orthonormal column-major basis
/// `[r0 r1 r2]`. Uses the standard branching trace algorithm.
///
/// With `r0, r1, r2` as the columns, the matrix entries are
/// `m[row][col]`: `m00=r0[0], m10=r0[1], m20=r0[2]`,
/// `m01=r1[0], m11=r1[1], m21=r1[2]`, `m02=r2[0], m12=r2[1], m22=r2[2]`.
fn quat_from_rot_columns(r0: &[f32; 3], r1: &[f32; 3], r2: &[f32; 3]) -> [f32; 4] {
    let m00 = r0[0];
    let m10 = r0[1];
    let m20 = r0[2];
    let m01 = r1[0];
    let m11 = r1[1];
    let m21 = r1[2];
    let m02 = r2[0];
    let m12 = r2[1];
    let m22 = r2[2];

    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        let w = 0.25 * s;
        let x = (m21 - m12) / s;
        let y = (m02 - m20) / s;
        let z = (m10 - m01) / s;
        return [x, y, z, w];
    }

    // Pick the largest diagonal to keep numerics stable.
    if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        let w = (m21 - m12) / s;
        let x = 0.25 * s;
        let y = (m01 + m10) / s;
        let z = (m02 + m20) / s;
        [x, y, z, w]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        let w = (m02 - m20) / s;
        let x = (m01 + m10) / s;
        let y = 0.25 * s;
        let z = (m12 + m21) / s;
        [x, y, z, w]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        let w = (m10 - m01) / s;
        let x = (m02 + m20) / s;
        let y = (m12 + m21) / s;
        let z = 0.25 * s;
        [x, y, z, w]
    }
}

fn value_to_quat_wxyz(v: &Value) -> Option<[f32; 4]> {
    match v {
        Value::Quatf(q) => Some(*q),
        Value::Quatd(q) => Some([q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]),
        _ => None,
    }
}

/// Intrinsic XYZ Euler → quaternion, returned in `(x, y, z, w)` layout.
fn quat_from_euler_xyz(rx: f32, ry: f32, rz: f32) -> [f32; 4] {
    let (sx, cx) = (rx * 0.5).sin_cos();
    let (sy, cy) = (ry * 0.5).sin_cos();
    let (sz, cz) = (rz * 0.5).sin_cos();
    // Qx · Qy · Qz, Hamilton product.
    let w = cx * cy * cz - sx * sy * sz;
    let x = sx * cy * cz + cx * sy * sz;
    let y = cx * sy * cz - sx * cy * sz;
    let z = cx * cy * sz + sx * sy * cz;
    [x, y, z, w]
}
