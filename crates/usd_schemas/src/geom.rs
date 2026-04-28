//! UsdGeom authoring: Cube, Sphere, Cylinder, Capsule, Mesh.

use openusd::sdf::{Path, Value};

use anyhow::Result;

use super::Stage;
use super::tokens::*;

/// UsdGeom.Cube with explicit size=1 + an xformOp:scale to hit the URDF box
/// size. Parent is expected to already carry the URDF visual/collision pose.
pub fn define_box(stage: &mut Stage, parent: &Path, name: &str, size: [f64; 3]) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_CUBE)?;
    stage.define_attribute(&p, "size", "double", Value::Double(1.0), false)?;
    super::xform::set_trs(stage, &p, &super::xform::Pose::identity(), Some(size))?;
    Ok(p)
}

pub fn define_sphere(stage: &mut Stage, parent: &Path, name: &str, radius: f64) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_SPHERE)?;
    stage.define_attribute(&p, "radius", "double", Value::Double(radius), false)?;
    Ok(p)
}

pub fn define_cylinder(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    radius: f64,
    length: f64,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_CYLINDER)?;
    stage.define_attribute(&p, "radius", "double", Value::Double(radius), false)?;
    stage.define_attribute(&p, "height", "double", Value::Double(length), false)?;
    stage.define_attribute(&p, "axis", "token", Value::Token("Z".into()), true)?;
    Ok(p)
}

pub fn define_capsule(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    radius: f64,
    length: f64,
) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_CAPSULE)?;
    stage.define_attribute(&p, "radius", "double", Value::Double(radius), false)?;
    stage.define_attribute(&p, "height", "double", Value::Double(length), false)?;
    stage.define_attribute(&p, "axis", "token", Value::Token("Z".into()), true)?;
    Ok(p)
}

pub struct MeshData {
    pub points: Vec<[f32; 3]>,
    pub face_vertex_counts: Vec<i32>,
    pub face_vertex_indices: Vec<i32>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// Per-vertex texture coords (optional). If present its length must equal
    /// `points.len()`.
    pub uvs: Option<Vec<[f32; 2]>>,
}

pub fn define_mesh(stage: &mut Stage, parent: &Path, name: &str, mesh: &MeshData) -> Result<Path> {
    let p = stage.define_prim(parent, name, T_MESH)?;

    stage.define_attribute(
        &p,
        "points",
        "point3f[]",
        Value::Vec3fVec(mesh.points.clone()),
        false,
    )?;
    stage.define_attribute(
        &p,
        "faceVertexCounts",
        "int[]",
        Value::IntVec(mesh.face_vertex_counts.clone()),
        false,
    )?;
    stage.define_attribute(
        &p,
        "faceVertexIndices",
        "int[]",
        Value::IntVec(mesh.face_vertex_indices.clone()),
        false,
    )?;
    if let Some(normals) = &mesh.normals {
        stage.define_attribute(
            &p,
            "normals",
            "normal3f[]",
            Value::Vec3fVec(normals.clone()),
            false,
        )?;
    }
    if let Some(uvs) = &mesh.uvs {
        stage.define_attribute(
            &p,
            "primvars:st",
            "texCoord2f[]",
            Value::Vec2fVec(uvs.clone()),
            false,
        )?;
    }
    Ok(p)
}

/// Set `purpose = "guide"` on a prim (used to tag collision geometry so it
/// doesn't render in the default view).
pub fn set_purpose_guide(stage: &mut Stage, prim: &Path) -> Result<()> {
    stage.define_attribute(prim, "purpose", "token", Value::Token("guide".into()), true)
}

// ── Readers ──────────────────────────────────────────────────────────────
//
// Symmetric to the `define_*` authoring helpers above. Return `None` when
// the prim lacks the defining attributes (openusd's `field::<T>(...)`
// returns `Ok(None)` for unauthored specs — treat that as "skip this prim"
// rather than surfacing an error).

/// Primvar interpolation as authored in USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Constant,
    Uniform,
    Varying,
    Vertex,
    FaceVarying,
}

impl Interpolation {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "constant" => Self::Constant,
            "uniform" => Self::Uniform,
            "varying" => Self::Varying,
            "vertex" => Self::Vertex,
            "faceVarying" => Self::FaceVarying,
            _ => return None,
        })
    }
}

/// Orientation the Mesh was authored with. `RightHanded` is USD's default;
/// `LeftHanded` flips the front face — callers either flip winding or set
/// `CullMode::Back` to account for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    RightHanded,
    LeftHanded,
}

/// `UsdGeomMesh.subdivisionScheme` — how the mesh is supposed to be
/// tessellated at render time. `None` is Pixar's default (polygonal
/// mesh as-authored); `CatmullClark` / `Loop` / `Bilinear` are
/// subdivision surfaces. USD stages author the *intent*; the plugin
/// currently passes the mesh through flat regardless of scheme, but
/// exposes the authored value so downstream consumers can run their
/// own subdivision pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubdivScheme {
    /// Polygonal — render exactly as authored (USD default).
    #[default]
    None,
    /// Catmull-Clark subdivision (quad-friendly).
    CatmullClark,
    /// Loop subdivision (triangle-only).
    Loop,
    /// Bilinear subdivision — rare; treated similarly to CatmullClark.
    Bilinear,
}

impl SubdivScheme {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(Self::None),
            "catmullClark" => Some(Self::CatmullClark),
            "loop" => Some(Self::Loop),
            "bilinear" => Some(Self::Bilinear),
            _ => None,
        }
    }
    pub fn is_subdivision(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Per-vertex primvar with its authored interpolation. `vertex` and
/// `faceVarying` are the only interpolations that trivially map to a Bevy
/// vertex attribute; anything else needs expansion on the consumer side.
#[derive(Debug, Clone)]
pub struct MeshPrimvar<T> {
    pub values: Vec<T>,
    pub interpolation: Interpolation,
    /// If non-empty, per-face indexed primvar (USD's `indices` attr on
    /// `primvars:foo`). Semantically: effective[i] = values[indices[i]].
    pub indices: Vec<i32>,
}

/// Fully-decoded UsdGeom.Mesh read back from a composed stage.
#[derive(Debug, Clone)]
pub struct ReadMesh {
    pub points: Vec<[f32; 3]>,
    pub face_vertex_counts: Vec<i32>,
    pub face_vertex_indices: Vec<i32>,
    pub normals: Option<MeshPrimvar<[f32; 3]>>,
    pub uvs: Option<MeshPrimvar<[f32; 2]>>,
    pub orientation: Orientation,
    /// `primvars:displayColor` — per-vertex / per-face / constant RGB. When
    /// a material:binding already authors a `diffuseColor` the rendered
    /// colour wins; displayColor is the fallback for Meshes shipped without
    /// a proper shader (nav-graph edges, debug visuals, Isaac mission UI).
    pub display_color: Option<MeshPrimvar<[f32; 3]>>,
    /// `primvars:displayOpacity` — matches displayColor, but scalar. `None`
    /// means "broadcast 1.0".
    pub display_opacity: Option<MeshPrimvar<f32>>,
    /// `GeomSubset` children of this Mesh with `familyName == "materialBind"`.
    /// When non-empty the consumer should split the mesh into one Bevy
    /// mesh per subset so each can bind its own material.
    pub subsets: Vec<ReadSubset>,
    /// `UsdGeomGprim.doubleSided` — `true` means front AND back faces
    /// render. Renderer maps to `StandardMaterial { double_sided: true,
    /// cull_mode: None }`. Unauthored → USD default of `false`.
    pub double_sided: bool,
    /// `UsdGeomBoundable.extent` — authored bounding box as
    /// `[min, max]` in the prim's local space. `None` when unauthored
    /// (consumer falls back to computing from `points`).
    pub extent: Option<[[f32; 3]; 2]>,
    /// `UsdGeomMesh.subdivisionScheme` — authored intent for
    /// subdivision-surface rendering. Plugin treats every mesh as
    /// polygonal today; the field is preserved so downstream tools
    /// can run their own tessellation.
    pub subdivision_scheme: SubdivScheme,
}

/// A GeomSubset face-index partition with an optional material binding.
/// `indices` is a list of face indices into the parent mesh's
/// `faceVertexCounts`.
#[derive(Debug, Clone)]
pub struct ReadSubset {
    pub name: String,
    pub indices: Vec<i32>,
    pub material_binding: Option<Path>,
}

/// Read a `UsdGeom.Mesh` prim. Returns `None` if the required attributes
/// (`points`, `faceVertexCounts`, `faceVertexIndices`) aren't authored.
pub fn read_mesh(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadMesh>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let Some(face_vertex_counts) = read_int_array(stage, prim, "faceVertexCounts")? else {
        return Ok(None);
    };
    let Some(face_vertex_indices) = read_int_array(stage, prim, "faceVertexIndices")? else {
        return Ok(None);
    };

    let normals = read_primvar_vec3f(stage, prim, "normals")?;
    let uvs = read_primvar_vec2f(stage, prim, "primvars:st")?.or(read_primvar_vec2f(
        stage,
        prim,
        "primvars:st0",
    )?);

    let orientation = match read_token(stage, prim, "orientation")?.as_deref() {
        Some("leftHanded") => Orientation::LeftHanded,
        _ => Orientation::RightHanded,
    };

    let display_color = read_primvar_vec3f(stage, prim, "primvars:displayColor")?;
    let display_opacity = read_primvar_float(stage, prim, "primvars:displayOpacity")?;
    let subsets = read_material_subsets(stage, prim)?;
    let double_sided = read_bool(stage, prim, "doubleSided")?.unwrap_or(false);
    let extent = read_extent(stage, prim)?;
    let subdivision_scheme = read_token(stage, prim, "subdivisionScheme")?
        .as_deref()
        .and_then(SubdivScheme::parse)
        .unwrap_or_default();

    Ok(Some(ReadMesh {
        points,
        face_vertex_counts,
        face_vertex_indices,
        normals,
        uvs,
        orientation,
        display_color,
        display_opacity,
        subsets,
        double_sided,
        extent,
        subdivision_scheme,
    }))
}

/// Enumerate GeomSubset children of `mesh_prim` that participate in the
/// `materialBind` family with `face` element type. Other subset families
/// (collision, visibility, custom) surface as empty for now — wiring them
/// up is future work.
fn read_material_subsets(
    stage: &openusd::Stage,
    mesh_prim: &Path,
) -> anyhow::Result<Vec<ReadSubset>> {
    let mut out = Vec::new();
    let children = stage
        .prim_children(mesh_prim.clone())
        .map_err(anyhow::Error::from)?;
    for child_name in children {
        let Ok(child_path) = mesh_prim.append_path(child_name.as_str()) else {
            continue;
        };
        let type_name: Option<String> = stage
            .field::<String>(child_path.clone(), "typeName")
            .ok()
            .flatten();
        if type_name.as_deref() != Some("GeomSubset") {
            continue;
        }
        let family = read_token(stage, &child_path, "familyName")?;
        if family.as_deref() != Some("materialBind") {
            continue;
        }
        let element = read_token(stage, &child_path, "elementType")?;
        if matches!(element.as_deref(), Some(e) if e != "face") {
            continue;
        }
        let indices = read_int_array(stage, &child_path, "indices")?.unwrap_or_default();
        let material_binding = super::shade::read_material_binding(stage, &child_path)?;
        out.push(ReadSubset {
            name: child_name.to_string(),
            indices,
            material_binding,
        });
    }
    Ok(out)
}

/// UsdGeom.Cube `size` attribute (a single scalar — the cube is `size × size × size`).
pub fn read_cube_size(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<f64>> {
    read_double(stage, prim, "size")
}

/// UsdGeom.Sphere `radius`.
pub fn read_sphere_radius(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<f64>> {
    read_double(stage, prim, "radius")
}

/// UsdGeom.Cylinder / Capsule dimensions + axis. Axis is a token `"X"|"Y"|"Z"`
/// (USD default is `"Z"`).
#[derive(Debug, Clone, Copy)]
pub struct ReadCylinder {
    pub radius: f64,
    pub height: f64,
    pub axis: Axis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "X" => Self::X,
            "Y" => Self::Y,
            "Z" => Self::Z,
            _ => return None,
        })
    }
}

pub fn read_cylinder(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadCylinder>> {
    let Some(radius) = read_double(stage, prim, "radius")? else {
        return Ok(None);
    };
    let Some(height) = read_double(stage, prim, "height")? else {
        return Ok(None);
    };
    let axis = read_token(stage, prim, "axis")?
        .as_deref()
        .and_then(Axis::parse)
        .unwrap_or(Axis::Z);
    Ok(Some(ReadCylinder {
        radius,
        height,
        axis,
    }))
}

pub fn read_capsule(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadCylinder>> {
    read_cylinder(stage, prim)
}

/// Convenience wrapper over `attr_default` that coerces a scalar double /
/// float to f64. Used by shape readers for attributes like
/// `UsdGeom.Plane.width`.
pub fn read_double_attr(stage: &openusd::Stage, prim: &Path, name: &str) -> Option<f64> {
    read_double(stage, prim, name).ok().flatten()
}

/// Decoded `UsdGeom.PointInstancer`. Missing attributes surface as empty
/// vectors; orientations / scales / protoIndices are optional per the spec.
#[derive(Debug, Clone, Default)]
pub struct ReadPointInstancer {
    pub prototypes: Vec<Path>,
    pub positions: Vec<[f32; 3]>,
    /// Quaternions in USD `(w, x, y, z)` layout. Empty when unauthored.
    pub orientations: Vec<[f32; 4]>,
    pub scales: Vec<[f32; 3]>,
    pub proto_indices: Vec<i32>,
}

pub fn read_point_instancer(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadPointInstancer>> {
    let Some(positions) = read_vec3f_array(stage, prim, "positions")? else {
        return Ok(None);
    };
    let Some(proto_indices) = read_int_array(stage, prim, "protoIndices")? else {
        return Ok(None);
    };

    // `prototypes` is a relationship on the instancer — read targets.
    let proto_rel = prim
        .append_property("prototypes")
        .map_err(anyhow::Error::from)?;
    let prototypes = match attr_default(stage, prim, "prototypes")? {
        // Some authoring tools put targets on the default field directly.
        Some(Value::PathListOp(op)) => op.flatten(),
        Some(Value::PathVec(v)) => v,
        _ => {
            // Standard path: `targetPaths` field on the relationship spec.
            match stage
                .field::<Value>(proto_rel, "targetPaths")
                .map_err(anyhow::Error::from)?
            {
                Some(Value::PathListOp(op)) => op.flatten(),
                Some(Value::PathVec(v)) => v,
                _ => Vec::new(),
            }
        }
    };

    let orientations = read_quat_array(stage, prim, "orientations")?.unwrap_or_default();
    let scales = read_vec3f_array(stage, prim, "scales")?.unwrap_or_default();

    Ok(Some(ReadPointInstancer {
        prototypes,
        positions,
        orientations,
        scales,
        proto_indices,
    }))
}

fn read_quat_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<[f32; 4]>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::QuatfVec(v)) => Some(v),
        Some(Value::QuatdVec(v)) => Some(
            v.into_iter()
                .map(|q| [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32])
                .collect(),
        ),
        // QuathVec disabled until we sort out the storage convention —
        // Pixar's PointInstancedMedCity authors them but our naive
        // wxyz mapping produced visually-wrong rotations. Falls
        // through to None → identity rotation per instance, which is
        // what the previous "yess but missing things" state had.
        _ => None,
    })
}

// ── UsdGeom.BasisCurves ───────────────────────────────────────────────────

/// Curve tessellation kind. Matches the `type` attribute on BasisCurves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveType {
    Linear,
    Cubic,
}

impl CurveType {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "linear" => Some(Self::Linear),
            "cubic" => Some(Self::Cubic),
            _ => None,
        }
    }
}

/// Basis function for cubic curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveBasis {
    Bezier,
    Bspline,
    CatmullRom,
    Hermite,
}

impl CurveBasis {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "bezier" => Some(Self::Bezier),
            "bspline" => Some(Self::Bspline),
            "catmullRom" => Some(Self::CatmullRom),
            "hermite" => Some(Self::Hermite),
            _ => None,
        }
    }
}

/// `wrap` token — whether curves are closed loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveWrap {
    Nonperiodic,
    Periodic,
    Pinned,
}

impl CurveWrap {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "nonperiodic" => Some(Self::Nonperiodic),
            "periodic" => Some(Self::Periodic),
            "pinned" => Some(Self::Pinned),
            _ => None,
        }
    }
}

/// Decoded `UsdGeom.BasisCurves`. Multiple curves share one prim — each
/// entry in `vertex_counts` is the point count for one curve; the
/// matching slice of `points` is that curve's CVs.
#[derive(Debug, Clone)]
pub struct ReadCurves {
    pub points: Vec<[f32; 3]>,
    pub vertex_counts: Vec<i32>,
    pub curve_type: CurveType,
    pub basis: CurveBasis,
    pub wrap: CurveWrap,
    /// Per-vertex width, per-curve width, or single width (broadcast).
    /// Empty means "fall back to the consumer's default".
    pub widths: Vec<f32>,
    /// Optional `primvars:displayColor` — consumer uses for line colour.
    pub display_color: Option<Vec<[f32; 3]>>,
}

pub fn read_curves(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadCurves>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let Some(vertex_counts) = read_int_array(stage, prim, "curveVertexCounts")? else {
        return Ok(None);
    };

    let curve_type = read_token(stage, prim, "type")?
        .as_deref()
        .and_then(CurveType::parse)
        .unwrap_or(CurveType::Linear);
    let basis = read_token(stage, prim, "basis")?
        .as_deref()
        .and_then(CurveBasis::parse)
        .unwrap_or(CurveBasis::Bezier);
    let wrap = read_token(stage, prim, "wrap")?
        .as_deref()
        .and_then(CurveWrap::parse)
        .unwrap_or(CurveWrap::Nonperiodic);
    let widths = read_float_array(stage, prim, "widths")?.unwrap_or_default();
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;

    Ok(Some(ReadCurves {
        points,
        vertex_counts,
        curve_type,
        basis,
        wrap,
        widths,
        display_color,
    }))
}

// ── UsdGeom.NurbsCurves ────────────────────────────────────────────────────

/// Decoded `UsdGeom.NurbsCurves`. Several curves can share one prim:
/// `curve_vertex_counts[i]` CVs from `points` belong to curve `i`,
/// the next `curve_vertex_counts[i] + order[i]` knots from `knots`
/// belong to curve `i`, and `ranges[i]` is its parameter span.
#[derive(Debug, Clone)]
pub struct ReadNurbsCurves {
    pub points: Vec<[f32; 3]>,
    pub curve_vertex_counts: Vec<i32>,
    /// Per-curve order (= degree + 1). Defaults to 4 (cubic) per
    /// USD when unauthored.
    pub order: Vec<i32>,
    /// Concatenated knot vectors. Length = Σ(curve_vertex_counts + order).
    pub knots: Vec<f64>,
    /// Per-curve `(uMin, uMax)` parameter range. Defaults to the
    /// inner knot span when unauthored.
    pub ranges: Vec<[f64; 2]>,
    pub widths: Vec<f32>,
    pub display_color: Option<Vec<[f32; 3]>>,
}

pub fn read_nurbs_curves(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadNurbsCurves>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let Some(curve_vertex_counts) = read_int_array(stage, prim, "curveVertexCounts")? else {
        return Ok(None);
    };

    // `order` is optional; default cubic (order = 4) per UsdGeomNurbsCurves.
    let order = read_int_array(stage, prim, "order")?.unwrap_or_else(|| {
        curve_vertex_counts
            .iter()
            .map(|_| 4)
            .collect()
    });

    let knots = read_double_array(stage, prim, "knots")?.unwrap_or_default();

    // `ranges` is `double2[]` — read raw and stride. When unauthored,
    // synthesize from each curve's clamped-knot inner span.
    let ranges = read_vec2d_array(stage, prim, "ranges")?.unwrap_or_else(|| {
        let mut out = Vec::with_capacity(curve_vertex_counts.len());
        let mut k_cursor = 0usize;
        for (i, count) in curve_vertex_counts.iter().enumerate() {
            let n = (*count as usize).max(0);
            let p = order.get(i).copied().unwrap_or(4) as usize;
            let nk = n + p;
            if k_cursor + nk <= knots.len() && p > 0 && n > 0 {
                let umin = knots[k_cursor + p - 1];
                let umax = knots[k_cursor + n];
                out.push([umin, umax]);
            } else {
                out.push([0.0, 1.0]);
            }
            k_cursor += nk;
        }
        out
    });

    let widths = read_float_array(stage, prim, "widths")?.unwrap_or_default();
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;

    Ok(Some(ReadNurbsCurves {
        points,
        curve_vertex_counts,
        order,
        knots,
        ranges,
        widths,
        display_color,
    }))
}

// ── UsdGeom.NurbsPatch ────────────────────────────────────────────────────

/// Decoded `UsdGeom.NurbsPatch`. The control net is `points` laid out
/// row-major: `P[i, j]` lives at `points[i * v_vertex_count + j]`,
/// where `i ∈ [0, u_vertex_count)` is the U index and
/// `j ∈ [0, v_vertex_count)` is the V index. Knot vectors have
/// length `u_vertex_count + u_order` (resp. `v + order`).
#[derive(Debug, Clone)]
pub struct ReadNurbsPatch {
    pub points: Vec<[f32; 3]>,
    pub u_vertex_count: i32,
    pub v_vertex_count: i32,
    pub u_order: i32,
    pub v_order: i32,
    pub u_knots: Vec<f64>,
    pub v_knots: Vec<f64>,
    pub u_range: [f64; 2],
    pub v_range: [f64; 2],
    pub display_color: Option<Vec<[f32; 3]>>,
}

pub fn read_nurbs_patch(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadNurbsPatch>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let u_vertex_count = read_int_scalar(stage, prim, "uVertexCount")?.unwrap_or(0);
    let v_vertex_count = read_int_scalar(stage, prim, "vVertexCount")?.unwrap_or(0);
    let u_order = read_int_scalar(stage, prim, "uOrder")?.unwrap_or(4);
    let v_order = read_int_scalar(stage, prim, "vOrder")?.unwrap_or(4);
    let u_knots = read_double_array(stage, prim, "uKnots")?.unwrap_or_default();
    let v_knots = read_double_array(stage, prim, "vKnots")?.unwrap_or_default();
    let u_range = read_vec2d_scalar(stage, prim, "uRange")?.unwrap_or_else(|| {
        // Default to the inner (clamped) span of the U knot vector.
        let n = u_vertex_count as usize;
        let p = u_order as usize;
        if !u_knots.is_empty() && n > 0 && p > 0 && u_knots.len() >= n + p {
            [u_knots[p - 1], u_knots[n]]
        } else {
            [0.0, 1.0]
        }
    });
    let v_range = read_vec2d_scalar(stage, prim, "vRange")?.unwrap_or_else(|| {
        let n = v_vertex_count as usize;
        let p = v_order as usize;
        if !v_knots.is_empty() && n > 0 && p > 0 && v_knots.len() >= n + p {
            [v_knots[p - 1], v_knots[n]]
        } else {
            [0.0, 1.0]
        }
    });
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;
    Ok(Some(ReadNurbsPatch {
        points,
        u_vertex_count,
        v_vertex_count,
        u_order,
        v_order,
        u_knots,
        v_knots,
        u_range,
        v_range,
        display_color,
    }))
}

// ── UsdGeom.TetMesh ────────────────────────────────────────────────────────

/// Decoded `UsdGeom.TetMesh`. The mesh is defined by N tetrahedra
/// (`tet_vertex_indices` length = 4 × N) sharing a flat point pool.
/// `surface_face_vertex_indices`, when authored, caches the boundary
/// triangulation; otherwise the consumer computes it from the tet
/// connectivity.
#[derive(Debug, Clone)]
pub struct ReadTetMesh {
    pub points: Vec<[f32; 3]>,
    /// Flat — length = 4 × num_tets. Each consecutive 4-tuple is one tet.
    pub tet_vertex_indices: Vec<i32>,
    /// Optional cached surface — flat, length = 3 × num_boundary_faces.
    pub surface_face_vertex_indices: Option<Vec<i32>>,
    pub display_color: Option<Vec<[f32; 3]>>,
}

pub fn read_tetmesh(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadTetMesh>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let Some(tet_vertex_indices) = read_int_array(stage, prim, "tetVertexIndices")? else {
        return Ok(None);
    };
    let surface_face_vertex_indices =
        read_int_array(stage, prim, "surfaceFaceVertexIndices")?;
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;
    Ok(Some(ReadTetMesh {
        points,
        tet_vertex_indices,
        surface_face_vertex_indices,
        display_color,
    }))
}

// ── UsdGeom.HermiteCurves ──────────────────────────────────────────────────

/// Decoded `UsdGeom.HermiteCurves`. Each CV has both a position
/// (`points[i]`) and a tangent (`tangents[i]`). A curve consists of
/// `curve_vertex_counts[i]` consecutive CVs; the cubic-Hermite basis
/// interpolates between adjacent CVs using their tangents.
#[derive(Debug, Clone)]
pub struct ReadHermiteCurves {
    pub points: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub curve_vertex_counts: Vec<i32>,
    pub widths: Vec<f32>,
    pub display_color: Option<Vec<[f32; 3]>>,
}

pub fn read_hermite_curves(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadHermiteCurves>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let Some(curve_vertex_counts) = read_int_array(stage, prim, "curveVertexCounts")? else {
        return Ok(None);
    };
    // Tangents are required by spec — but if absent, fall back to a
    // forward-difference estimate so the curve still renders.
    let tangents = read_vec3f_array(stage, prim, "tangents")?.unwrap_or_else(|| {
        // Forward differences with a copy of the last segment for the
        // tail. Length always matches `points`.
        let n = points.len();
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let a = points[i];
            let b = if i + 1 < n { points[i + 1] } else { points[i] };
            out.push([b[0] - a[0], b[1] - a[1], b[2] - a[2]]);
        }
        out
    });
    let widths = read_float_array(stage, prim, "widths")?.unwrap_or_default();
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;
    Ok(Some(ReadHermiteCurves {
        points,
        tangents,
        curve_vertex_counts,
        widths,
        display_color,
    }))
}

// ── UsdGeom.Points ────────────────────────────────────────────────────────

/// Decoded `UsdGeom.Points` point cloud.
#[derive(Debug, Clone)]
pub struct ReadPoints {
    pub points: Vec<[f32; 3]>,
    /// Per-point radius. Empty = consumer default.
    pub widths: Vec<f32>,
    /// `primvars:displayColor` — per-point colour. When `values.len() == 1`
    /// the single colour applies to every point.
    pub display_color: Option<Vec<[f32; 3]>>,
    /// Stable simulation IDs. Not used for rendering but surfaced so
    /// animation passes can match particles across frames.
    pub ids: Vec<i64>,
}

pub fn read_points(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadPoints>> {
    let Some(points) = read_vec3f_array(stage, prim, "points")? else {
        return Ok(None);
    };
    let widths = read_float_array(stage, prim, "widths")?.unwrap_or_default();
    let display_color = read_vec3f_array(stage, prim, "primvars:displayColor")?;
    let ids = read_int64_array(stage, prim, "ids")?.unwrap_or_default();
    Ok(Some(ReadPoints {
        points,
        widths,
        display_color,
        ids,
    }))
}

fn read_float_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<f32>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::FloatVec(v)) => Some(v),
        Some(Value::DoubleVec(v)) => Some(v.into_iter().map(|d| d as f32).collect()),
        Some(Value::Float(v)) => Some(vec![v]),
        _ => None,
    })
}

fn read_int64_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<i64>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Int64Vec(v)) => Some(v),
        Some(Value::IntVec(v)) => Some(v.into_iter().map(|i| i as i64).collect()),
        _ => None,
    })
}

/// `purpose` token on any Imageable prim. USD default (unauthored) = `"default"`.
pub fn read_purpose(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<String> {
    Ok(read_token(stage, prim, "purpose")?.unwrap_or_else(|| "default".to_string()))
}

/// `UsdGeomImageable.visibility` authored state. The USD default
/// (unauthored) is `Inherited` — children inherit their parent's
/// effective visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    Inherited,
    Invisible,
}

/// Read `visibility` on any Imageable prim. Returns `Inherited` when
/// the attribute is unauthored (USD default). Does **not** yet read
/// `timeSamples` — static only for now; animated visibility is a
/// future M22.x.
pub fn read_visibility(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<VisibilityState> {
    Ok(match read_token(stage, prim, "visibility")?.as_deref() {
        Some("invisible") => VisibilityState::Invisible,
        _ => VisibilityState::Inherited,
    })
}

/// `kind` metadata on any model prim. USD authors one of
/// `"model" | "group" | "assembly" | "component" | "subcomponent"` (or
/// a custom token). Returns `None` when unauthored — only "model"
/// prims and their ancestors conventionally carry it.
pub fn read_kind(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<String>> {
    stage
        .field::<String>(prim.clone(), "kind")
        .map_err(anyhow::Error::from)
}

/// A typed snapshot of a single custom attribute / custom-data entry.
/// Covers every scalar / vec / matrix / quat variant openusd surfaces,
/// plus arrays, nested dictionaries, and time-sampled snapshots.
/// Unrecognised / exotic payloads fall through to `Other`.
///
/// Half-precision floats (`Half`, `QuathVec`, `Vec2h`, …) are widened
/// to `f32` at read time so consumers don't need to depend on the
/// `half` crate.
#[derive(Debug, Clone, PartialEq)]
pub enum CustomAttrValue {
    // ── Scalars ────────────────────────────────────────────────────
    Bool(bool),
    Uchar(u8),
    Int(i32),
    UInt(u32),
    Int64(i64),
    UInt64(u64),
    Float(f32),
    Double(f64),
    TimeCode(f64),

    // ── String-like ────────────────────────────────────────────────
    String(String),
    Token(String),
    AssetPath(String),
    PathExpression(String),

    // ── 2/3/4-tuples (half-precision folded into f32) ──────────────
    Vec2f([f32; 2]),
    Vec2d([f64; 2]),
    Vec2i([i32; 2]),
    Vec3f([f32; 3]),
    Vec3d([f64; 3]),
    Vec3i([i32; 3]),
    Vec4f([f32; 4]),
    Vec4d([f64; 4]),
    Vec4i([i32; 4]),

    // ── Quaternions (w, x, y, z) ───────────────────────────────────
    Quatf([f32; 4]),
    Quatd([f64; 4]),

    // ── Matrices (row-major flattened) ─────────────────────────────
    Matrix2d([f64; 4]),
    Matrix3d([f64; 9]),
    Matrix4d([f64; 16]),

    // ── Arrays ─────────────────────────────────────────────────────
    BoolArray(Vec<bool>),
    UcharArray(Vec<u8>),
    IntArray(Vec<i32>),
    UIntArray(Vec<u32>),
    Int64Array(Vec<i64>),
    UInt64Array(Vec<u64>),
    FloatArray(Vec<f32>),
    DoubleArray(Vec<f64>),
    StringArray(Vec<String>),
    TokenArray(Vec<String>),
    PathArray(Vec<String>),
    Vec2fArray(Vec<[f32; 2]>),
    Vec3fArray(Vec<[f32; 3]>),
    Vec4fArray(Vec<[f32; 4]>),
    Vec2dArray(Vec<[f64; 2]>),
    Vec3dArray(Vec<[f64; 3]>),
    Vec4dArray(Vec<[f64; 4]>),
    QuatfArray(Vec<[f32; 4]>),
    Matrix4dArray(Vec<[f64; 16]>),

    // ── Compound ───────────────────────────────────────────────────
    /// Nested dictionary (authored as `dictionary foo = { ... }`).
    Dict(CustomDict),
    /// TimeSample snapshot — one entry per authored timeCode. Useful
    /// for animated custom attrs (rare but legal).
    TimeSamples(Vec<(f64, Box<CustomAttrValue>)>),

    /// Fallback: anything we don't explicitly enumerate lands here as
    /// a debug-formatted string of the original `Value`.
    Other(String),
}

impl CustomAttrValue {
    /// Try to read this value as a bool.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// Numeric coercion that widens any integer variant to i64.
    pub fn as_int(&self) -> Option<i64> {
        Some(match self {
            Self::Uchar(u) => *u as i64,
            Self::Int(i) => *i as i64,
            Self::UInt(u) => *u as i64,
            Self::Int64(i) => *i,
            Self::UInt64(u) => *u as i64,
            _ => return None,
        })
    }
    /// Numeric coercion that widens Float/Double/TimeCode/Half to f64.
    pub fn as_float(&self) -> Option<f64> {
        Some(match self {
            Self::Float(f) => *f as f64,
            Self::Double(d) => *d,
            Self::TimeCode(t) => *t,
            Self::Int(i) => *i as f64,
            Self::Int64(i) => *i as f64,
            Self::UInt(u) => *u as f64,
            Self::UInt64(u) => *u as f64,
            Self::Uchar(u) => *u as f64,
            _ => return None,
        })
    }
    /// Borrow-friendly string accessor covering String/Token/AssetPath/PathExpression.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) | Self::Token(s) | Self::AssetPath(s) | Self::PathExpression(s) => {
                Some(s.as_str())
            }
            _ => None,
        }
    }
    /// 2-tuple projection.
    pub fn as_vec2(&self) -> Option<[f32; 2]> {
        match self {
            Self::Vec2f(a) => Some(*a),
            Self::Vec2d(a) => Some([a[0] as f32, a[1] as f32]),
            Self::Vec2i(a) => Some([a[0] as f32, a[1] as f32]),
            _ => None,
        }
    }
    /// 3-tuple projection (f/d/i all widen to f32).
    pub fn as_vec3(&self) -> Option<[f32; 3]> {
        match self {
            Self::Vec3f(a) => Some(*a),
            Self::Vec3d(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32]),
            Self::Vec3i(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32]),
            _ => None,
        }
    }
    /// 4-tuple projection.
    pub fn as_vec4(&self) -> Option<[f32; 4]> {
        match self {
            Self::Vec4f(a) | Self::Quatf(a) => Some(*a),
            Self::Vec4d(a) | Self::Quatd(a) => Some([
                a[0] as f32,
                a[1] as f32,
                a[2] as f32,
                a[3] as f32,
            ]),
            Self::Vec4i(a) => Some([a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32]),
            _ => None,
        }
    }
    /// Matrix4 projection — decodes row-major `[f32; 16]` for any
    /// Matrix2d/3d/4d authoring (2d/3d get padded with identity rows).
    pub fn as_matrix4(&self) -> Option<[f32; 16]> {
        match self {
            Self::Matrix4d(m) => {
                let mut out = [0.0f32; 16];
                for i in 0..16 {
                    out[i] = m[i] as f32;
                }
                Some(out)
            }
            _ => None,
        }
    }
    /// If this is a `Dict`, return the inner dictionary.
    pub fn as_dict(&self) -> Option<&CustomDict> {
        match self {
            Self::Dict(d) => Some(d),
            _ => None,
        }
    }
    /// Return `true` if this value carries an array-shaped payload.
    pub fn is_array(&self) -> bool {
        matches!(
            self,
            Self::BoolArray(_)
                | Self::UcharArray(_)
                | Self::IntArray(_)
                | Self::UIntArray(_)
                | Self::Int64Array(_)
                | Self::UInt64Array(_)
                | Self::FloatArray(_)
                | Self::DoubleArray(_)
                | Self::StringArray(_)
                | Self::TokenArray(_)
                | Self::PathArray(_)
                | Self::Vec2fArray(_)
                | Self::Vec3fArray(_)
                | Self::Vec4fArray(_)
                | Self::Vec2dArray(_)
                | Self::Vec3dArray(_)
                | Self::Vec4dArray(_)
                | Self::QuatfArray(_)
                | Self::Matrix4dArray(_)
        )
    }
}

/// Nested dictionary container. USD's `customData` / `assetInfo` fields
/// are dictionaries by design, and they're often deeply nested (DCCs
/// stash structured config trees here). Preserves insertion order so
/// downstream YAML/JSON emitters look sane.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CustomDict {
    pub entries: Vec<(String, CustomAttrValue)>,
}

impl CustomDict {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn get(&self, name: &str) -> Option<&CustomAttrValue> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }
    pub fn iter(&self) -> impl Iterator<Item = &(String, CustomAttrValue)> {
        self.entries.iter()
    }
    /// Walk a dotted key path — `get_nested("foo.bar.baz")` descends
    /// through nested `Dict` entries. Returns `None` at the first
    /// missing key or non-dict intermediate.
    pub fn get_nested(&self, dotted: &str) -> Option<&CustomAttrValue> {
        let mut cur: &CustomAttrValue = self.get(dotted.split('.').next()?)?;
        for part in dotted.split('.').skip(1) {
            match cur {
                CustomAttrValue::Dict(d) => cur = d.get(part)?,
                _ => return None,
            }
        }
        Some(cur)
    }
}

/// Read every authored `custom` attribute on `prim` (including
/// namespaced ones like `userProperties:*`). Schema-defined attributes
/// are skipped — this is strictly for user-authored pass-through.
pub fn read_custom_attrs(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Vec<(String, CustomAttrValue)>> {
    let prop_names = stage
        .prim_properties(prim.clone())
        .map_err(anyhow::Error::from)?;
    let mut out = Vec::new();
    for name in prop_names {
        let Ok(attr_path) = prim.append_property(name.as_str()) else {
            continue;
        };
        // `custom = true` is the marker that distinguishes user-authored
        // attributes from schema-defined ones.
        let is_custom = matches!(
            stage.field::<bool>(attr_path.clone(), "custom").ok().flatten(),
            Some(true)
        );
        if !is_custom {
            continue;
        }
        let Some(raw) = stage
            .field::<Value>(attr_path, "default")
            .map_err(anyhow::Error::from)?
        else {
            continue;
        };
        out.push((name.to_string(), value_to_custom(raw)));
    }
    Ok(out)
}

/// Exhaustive `Value` → `CustomAttrValue` conversion. Widens half-float
/// to f32, flattens matrices row-major, unpacks nested dictionaries
/// recursively. Anything exotic (list ops, payload, relocates) falls
/// through to `Other` so downstream consumers at least see the type.
fn value_to_custom(v: Value) -> CustomAttrValue {
    match v {
        // ── Scalars ────────────────────────────────────────────────
        Value::Bool(b) => CustomAttrValue::Bool(b),
        Value::Uchar(u) => CustomAttrValue::Uchar(u),
        Value::Int(i) => CustomAttrValue::Int(i),
        Value::Uint(u) => CustomAttrValue::UInt(u),
        Value::Int64(i) => CustomAttrValue::Int64(i),
        Value::Uint64(u) => CustomAttrValue::UInt64(u),
        Value::Half(h) => CustomAttrValue::Float(f32::from(h)),
        Value::Float(f) => CustomAttrValue::Float(f),
        Value::Double(d) => CustomAttrValue::Double(d),
        Value::TimeCode(t) => CustomAttrValue::TimeCode(t),

        // ── String-like ────────────────────────────────────────────
        Value::String(s) => CustomAttrValue::String(s),
        Value::Token(s) => CustomAttrValue::Token(s),
        Value::AssetPath(s) => CustomAttrValue::AssetPath(s),
        Value::PathExpression(s) => CustomAttrValue::PathExpression(s),

        // ── Vec2/3/4 ───────────────────────────────────────────────
        Value::Vec2h(a) => CustomAttrValue::Vec2f([f32::from(a[0]), f32::from(a[1])]),
        Value::Vec2f(a) => CustomAttrValue::Vec2f(a),
        Value::Vec2d(a) => CustomAttrValue::Vec2d(a),
        Value::Vec2i(a) => CustomAttrValue::Vec2i(a),
        Value::Vec3h(a) => CustomAttrValue::Vec3f([
            f32::from(a[0]),
            f32::from(a[1]),
            f32::from(a[2]),
        ]),
        Value::Vec3f(a) => CustomAttrValue::Vec3f(a),
        Value::Vec3d(a) => CustomAttrValue::Vec3d(a),
        Value::Vec3i(a) => CustomAttrValue::Vec3i(a),
        Value::Vec4h(a) => CustomAttrValue::Vec4f([
            f32::from(a[0]),
            f32::from(a[1]),
            f32::from(a[2]),
            f32::from(a[3]),
        ]),
        Value::Vec4f(a) => CustomAttrValue::Vec4f(a),
        Value::Vec4d(a) => CustomAttrValue::Vec4d(a),
        Value::Vec4i(a) => CustomAttrValue::Vec4i(a),

        // ── Quaternions ────────────────────────────────────────────
        Value::Quath(q) => CustomAttrValue::Quatf([
            f32::from(q[0]),
            f32::from(q[1]),
            f32::from(q[2]),
            f32::from(q[3]),
        ]),
        Value::Quatf(q) => CustomAttrValue::Quatf(q),
        Value::Quatd(q) => CustomAttrValue::Quatd(q),

        // ── Matrices ───────────────────────────────────────────────
        Value::Matrix2d(m) => CustomAttrValue::Matrix2d(m),
        Value::Matrix3d(m) => CustomAttrValue::Matrix3d(m),
        Value::Matrix4d(m) => CustomAttrValue::Matrix4d(m),

        // ── Arrays ─────────────────────────────────────────────────
        Value::BoolVec(v) => CustomAttrValue::BoolArray(v),
        Value::UcharVec(v) => CustomAttrValue::UcharArray(v),
        Value::IntVec(v) => CustomAttrValue::IntArray(v),
        Value::UintVec(v) => CustomAttrValue::UIntArray(v),
        Value::Int64Vec(v) => CustomAttrValue::Int64Array(v),
        Value::Uint64Vec(v) => CustomAttrValue::UInt64Array(v),
        Value::HalfVec(v) => {
            CustomAttrValue::FloatArray(v.into_iter().map(f32::from).collect())
        }
        Value::FloatVec(v) => CustomAttrValue::FloatArray(v),
        Value::DoubleVec(v) => CustomAttrValue::DoubleArray(v),
        Value::StringVec(v) => CustomAttrValue::StringArray(v),
        Value::TokenVec(v) => CustomAttrValue::TokenArray(v),
        Value::PathVec(v) => {
            CustomAttrValue::PathArray(v.into_iter().map(|p| p.as_str().to_string()).collect())
        }
        Value::Vec2hVec(v) => CustomAttrValue::Vec2fArray(
            v.into_iter()
                .map(|a| [f32::from(a[0]), f32::from(a[1])])
                .collect(),
        ),
        Value::Vec2fVec(v) => CustomAttrValue::Vec2fArray(v),
        Value::Vec2dVec(v) => CustomAttrValue::Vec2dArray(v),
        Value::Vec3hVec(v) => CustomAttrValue::Vec3fArray(
            v.into_iter()
                .map(|a| [f32::from(a[0]), f32::from(a[1]), f32::from(a[2])])
                .collect(),
        ),
        Value::Vec3fVec(v) => CustomAttrValue::Vec3fArray(v),
        Value::Vec3dVec(v) => CustomAttrValue::Vec3dArray(v),
        Value::Vec4hVec(v) => CustomAttrValue::Vec4fArray(
            v.into_iter()
                .map(|a| {
                    [
                        f32::from(a[0]),
                        f32::from(a[1]),
                        f32::from(a[2]),
                        f32::from(a[3]),
                    ]
                })
                .collect(),
        ),
        Value::Vec4fVec(v) => CustomAttrValue::Vec4fArray(v),
        Value::Vec4dVec(v) => CustomAttrValue::Vec4dArray(v),
        Value::QuatfVec(v) => CustomAttrValue::QuatfArray(v),
        Value::Matrix4dVec(v) => CustomAttrValue::Matrix4dArray(v),

        // ── Compound ───────────────────────────────────────────────
        Value::Dictionary(dict) => CustomAttrValue::Dict(dict_from_value_map(dict)),
        Value::TimeSamples(samples) => CustomAttrValue::TimeSamples(
            samples
                .into_iter()
                .map(|(t, v)| (t, Box::new(value_to_custom(v))))
                .collect(),
        ),

        other => CustomAttrValue::Other(format!("{other:?}")),
    }
}

/// Convert an openusd `HashMap<String, Value>` dictionary payload into
/// our ordered `CustomDict`. USD doesn't guarantee authoring order
/// through HashMap, but we sort alphabetically to keep test output
/// + YAML emission stable.
fn dict_from_value_map(
    map: std::collections::HashMap<String, Value>,
) -> CustomDict {
    let mut entries: Vec<(String, CustomAttrValue)> = map
        .into_iter()
        .map(|(k, v)| (k, value_to_custom(v)))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    CustomDict { entries }
}

/// Read the `customData` dictionary authored on a prim. Returns
/// `None` when unauthored (most prims don't carry one).
pub fn read_custom_data(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<CustomDict>> {
    let raw = stage
        .field::<Value>(prim.clone(), "customData")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::Dictionary(d)) => {
            let dict = dict_from_value_map(d);
            (!dict.is_empty()).then_some(dict)
        }
        _ => None,
    })
}

/// Read the `assetInfo` dictionary authored on a prim. Package
/// management tools (Omniverse, Houdini, Maya) stash identifier /
/// version metadata here.
pub fn read_asset_info(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<CustomDict>> {
    let raw = stage
        .field::<Value>(prim.clone(), "assetInfo")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::Dictionary(d)) => {
            let dict = dict_from_value_map(d);
            (!dict.is_empty()).then_some(dict)
        }
        _ => None,
    })
}

/// Read the pseudo-root `customLayerData` dictionary — layer-level
/// freeform metadata. Omniverse authors camera bookmarks, layer
/// authoring state, render settings defaults, etc. here.
pub fn read_custom_layer_data(
    stage: &openusd::Stage,
) -> anyhow::Result<Option<CustomDict>> {
    let raw = stage
        .field::<Value>(Path::abs_root(), "customLayerData")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::Dictionary(d)) => {
            let dict = dict_from_value_map(d);
            (!dict.is_empty()).then_some(dict)
        }
        _ => None,
    })
}

// ── Low-level attribute helpers ──────────────────────────────────────────

fn attr_default(stage: &openusd::Stage, prim: &Path, name: &str) -> anyhow::Result<Option<Value>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    let default = stage
        .field::<Value>(attr_path.clone(), "default")
        .map_err(anyhow::Error::from)?;
    // If `default` carries real data, use it. Production assets
    // (Pixar PointInstancedMedCity, FX caches, etc.) routinely author
    // every per-frame attribute as `timeSamples` only, *with an empty
    // default array as a placeholder* — `points = []` but
    // `points.timeSamples = { 184: [...real points...] }`. Treat an
    // empty array container as "no default" and fall through to the
    // first time sample. Stage-time animation is handled separately
    // in `anim.rs`; this fallback just gives the static schema reader
    // a non-empty value to translate at load time.
    if let Some(v) = default {
        if !is_empty_array_value(&v) {
            return Ok(Some(v));
        }
    }
    let raw = stage
        .field::<Value>(attr_path, "timeSamples")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::TimeSamples(samples)) => samples.into_iter().next().map(|(_, v)| v),
        _ => None,
    })
}

fn is_empty_array_value(v: &Value) -> bool {
    match v {
        Value::IntVec(a) => a.is_empty(),
        Value::FloatVec(a) => a.is_empty(),
        Value::DoubleVec(a) => a.is_empty(),
        Value::TokenVec(a) | Value::StringVec(a) => a.is_empty(),
        Value::Vec2fVec(a) => a.is_empty(),
        Value::Vec2dVec(a) => a.is_empty(),
        Value::Vec3fVec(a) => a.is_empty(),
        Value::Vec3dVec(a) => a.is_empty(),
        Value::Vec4fVec(a) => a.is_empty(),
        _ => false,
    }
}

fn read_token(stage: &openusd::Stage, prim: &Path, name: &str) -> anyhow::Result<Option<String>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_double(stage: &openusd::Stage, prim: &Path, name: &str) -> anyhow::Result<Option<f64>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Double(v)) => Some(v),
        Some(Value::Float(v)) => Some(v as f64),
        _ => None,
    })
}

fn read_bool(stage: &openusd::Stage, prim: &Path, name: &str) -> anyhow::Result<Option<bool>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Bool(b)) => Some(b),
        _ => None,
    })
}

/// Read `UsdGeomBoundable.extent` — authored as `float3[2]` (min / max
/// in prim-local space). Returns `None` when unauthored; consumer is
/// expected to compute from vertex data in that case.
fn read_extent(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<[[f32; 3]; 2]>> {
    let arr = read_vec3f_array(stage, prim, "extent")?;
    Ok(match arr {
        Some(v) if v.len() >= 2 => Some([v[0], v[1]]),
        _ => None,
    })
}

fn read_int_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<i32>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::IntVec(v)) => Some(v),
        _ => None,
    })
}

fn read_double_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<f64>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::DoubleVec(v)) => Some(v),
        Some(Value::FloatVec(v)) => Some(v.into_iter().map(|f| f as f64).collect()),
        _ => None,
    })
}

fn read_int_scalar(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<i32>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Int(v)) => Some(v),
        _ => None,
    })
}

fn read_vec2d_scalar(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<[f64; 2]>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Vec2d(v)) => Some(v),
        Some(Value::Vec2f(v)) => Some([v[0] as f64, v[1] as f64]),
        _ => None,
    })
}

fn read_vec2d_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<[f64; 2]>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Vec2dVec(v)) => Some(v),
        Some(Value::Vec2fVec(v)) => Some(
            v.into_iter()
                .map(|a| [a[0] as f64, a[1] as f64])
                .collect(),
        ),
        _ => None,
    })
}

fn read_vec3f_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<[f32; 3]>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Vec3fVec(v)) => Some(v),
        Some(Value::Vec3dVec(v)) => Some(
            v.into_iter()
                .map(|a| [a[0] as f32, a[1] as f32, a[2] as f32])
                .collect(),
        ),
        _ => None,
    })
}

fn read_vec2f_array(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<[f32; 2]>>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Vec2fVec(v)) => Some(v),
        Some(Value::Vec2dVec(v)) => {
            Some(v.into_iter().map(|a| [a[0] as f32, a[1] as f32]).collect())
        }
        _ => None,
    })
}

/// Read the `interpolation` metadata field authored on a primvar
/// attribute spec (i.e. the value inside the parens block:
/// `color3f[] primvars:displayColor = [...] (interpolation = "uniform")`).
/// Tries attribute metadata first; falls back to the legacy sibling
/// `{name}:interpolation` attribute some older authoring tools emit.
fn read_primvar_interpolation(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Interpolation>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    if let Some(v) = stage
        .field::<Value>(attr_path, "interpolation")
        .map_err(anyhow::Error::from)?
    {
        if let Value::Token(s) | Value::String(s) = v
            && let Some(i) = Interpolation::parse(&s)
        {
            return Ok(Some(i));
        }
    }
    // Legacy fallback: some tools author a sibling `{name}:interpolation`
    // attribute's default value instead of the metadata field.
    let fallback_name = format!("{name}:interpolation");
    Ok(read_token(stage, prim, &fallback_name)?
        .as_deref()
        .and_then(Interpolation::parse))
}

fn read_primvar_vec3f(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<MeshPrimvar<[f32; 3]>>> {
    let Some(values) = read_vec3f_array(stage, prim, name)? else {
        return Ok(None);
    };
    let interpolation =
        read_primvar_interpolation(stage, prim, name)?.unwrap_or(Interpolation::Vertex);
    let indices = read_int_array(stage, prim, &format!("{name}:indices"))?.unwrap_or_default();
    Ok(Some(MeshPrimvar {
        values,
        interpolation,
        indices,
    }))
}

fn read_primvar_float(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<MeshPrimvar<f32>>> {
    let Some(values) = read_float_array(stage, prim, name)? else {
        return Ok(None);
    };
    let interpolation =
        read_primvar_interpolation(stage, prim, name)?.unwrap_or(Interpolation::Vertex);
    let indices = read_int_array(stage, prim, &format!("{name}:indices"))?.unwrap_or_default();
    Ok(Some(MeshPrimvar {
        values,
        interpolation,
        indices,
    }))
}

fn read_primvar_vec2f(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<MeshPrimvar<[f32; 2]>>> {
    let Some(values) = read_vec2f_array(stage, prim, name)? else {
        return Ok(None);
    };
    let interpolation =
        read_primvar_interpolation(stage, prim, name)?.unwrap_or(Interpolation::FaceVarying);
    let indices = read_int_array(stage, prim, &format!("{name}:indices"))?.unwrap_or_default();
    Ok(Some(MeshPrimvar {
        values,
        interpolation,
        indices,
    }))
}

/// Author a `GeomSubset` child of `parent_mesh` that selects a set of face
/// indices for per-subset material binding (family `"materialBind"`).
pub fn define_geom_subset_face(
    stage: &mut Stage,
    parent_mesh: &Path,
    name: &str,
    face_indices: &[i32],
) -> Result<Path> {
    let p = stage.define_prim(parent_mesh, name, T_GEOM_SUBSET)?;
    stage.define_attribute(
        &p,
        "familyName",
        "token",
        Value::Token("materialBind".into()),
        true,
    )?;
    stage.define_attribute(
        &p,
        "elementType",
        "token",
        Value::Token("face".into()),
        true,
    )?;
    stage.define_attribute(
        &p,
        "indices",
        "int[]",
        Value::IntVec(face_indices.to_vec()),
        false,
    )?;
    Ok(p)
}
