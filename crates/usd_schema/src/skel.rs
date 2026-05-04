//! UsdSkel readers: `Skeleton`, `SkelRoot`, `SkelBindingAPI`.
//!
//! `openusd` ships no UsdSkel awareness of its own — this module reads
//! the raw typed attributes off any composed stage. Downstream consumers
//! can turn the decoded structs into Bevy bone hierarchies +
//! `SkinnedMesh` + `AnimationClip`. That integration is deferred; for
//! now the plugin just surfaces what was authored so the viewer can
//! count skeletons and future work has the data it needs.
//!
//! Unanimated skeletons (`Skeleton` + `bindTransforms` + `restTransforms`)
//! are fully supported. `SkelAnimation` time-sampled joint data will
//! wait until openusd's USDA parser can round-trip tuple-valued
//! timeSamples (same blocker as vec3 xformOp animation).

use openusd::sdf::{Path, Value};

/// A decoded `Skeleton` prim. Joints are authored as tokens naming
/// each bone; the two transform lists must line up with the joints
/// list index-wise.
#[derive(Debug, Clone)]
pub struct ReadSkeleton {
    /// Composed prim path of the Skeleton itself.
    pub path: String,
    /// Joint names/paths — the primary ordering for every
    /// per-joint array on this skeleton and any driving SkelAnimation.
    pub joints: Vec<String>,
    /// Per-joint `bindTransforms` (matrix4d[]). Empty when unauthored.
    pub bind_transforms: Vec<[f32; 16]>,
    /// Per-joint `restTransforms` (matrix4d[]). Empty when unauthored.
    pub rest_transforms: Vec<[f32; 16]>,
}

/// A decoded `SkelRoot` prim. Carries the skel + animationSource
/// relationships but doesn't resolve them — that's the consumer's job.
#[derive(Debug, Clone)]
pub struct ReadSkelRoot {
    pub path: String,
    pub skeleton: Option<String>,
    pub animation_source: Option<String>,
}

/// A decoded `UsdSkelBlendShape` prim: per-vertex position +
/// optional normal offsets that get scaled by the animation's weight
/// and added to the rest mesh. Sparse via `point_indices` —
/// `offsets[i]` applies to vertex `point_indices[i]`. Dense when
/// `point_indices` is empty (offsets[i] applies to vertex i).
#[derive(Debug, Clone)]
pub struct ReadBlendShape {
    /// Composed prim path of the BlendShape itself.
    pub path: String,
    /// Required `offsets` (vector3f[]). One entry per affected
    /// vertex. Length matches `point_indices.len()` when sparse,
    /// or the bound mesh's vertex count when dense.
    pub offsets: Vec<[f32; 3]>,
    /// Required `normalOffsets` (vector3f[]). Same length as
    /// `offsets`. Empty when unauthored.
    pub normal_offsets: Vec<[f32; 3]>,
    /// Optional `pointIndices` (int[]). Maps `offsets[i]` to the
    /// vertex it deforms. Empty for dense blend shapes.
    pub point_indices: Vec<i32>,
}

/// Per-mesh `SkelBindingAPI` data — maps mesh vertices to joint
/// indices and weights. `elements_per_vertex` tells the consumer how
/// many (index, weight) pairs belong to each vertex; the two arrays
/// are flattened `vertex × elements_per_vertex` buffers.
#[derive(Debug, Clone)]
pub struct ReadSkelBinding {
    /// The mesh prim carrying the binding.
    pub prim_path: String,
    /// `skel:skeleton` rel target (absolute prim path).
    pub skeleton: Option<String>,
    pub joint_indices: Vec<i32>,
    pub joint_weights: Vec<f32>,
    /// Authored `elementSize` metadata on `primvars:skel:jointIndices`
    /// — how many (joint, weight) pairs per vertex. Defaults to 1.
    pub elements_per_vertex: i32,
    /// Authored `skel:joints` token array — when present, this is the
    /// per-mesh subset/reordering of the bound Skeleton's joints. The
    /// `joint_indices` then index into THIS list, not the Skeleton's
    /// `joints`. When absent, indices target the Skeleton directly.
    /// Pixar's HumanFemale authors per-mesh `skel:joints` on every
    /// skinned mesh; without remapping, vertices reach for the wrong
    /// bones (huge shoes, infinite-long fingers).
    pub joint_subset: Vec<String>,
    /// Authored `skel:blendShapes` token array — names of the
    /// blend shapes this mesh uses. Parallel-indexed to
    /// `blend_shape_targets`: `blend_shapes[i]` is driven by the
    /// SkelAnimation's `blendShapeWeights[k]` where the
    /// SkelAnimation's `blendShapes[k]` matches `blend_shapes[i]`.
    pub blend_shapes: Vec<String>,
    /// Authored `skel:blendShapeTargets` (rel) — absolute prim
    /// paths of the BlendShape prims. Parallel to `blend_shapes`.
    pub blend_shape_targets: Vec<String>,
}

impl ReadSkeleton {
    /// USD's `joints` token-array encodes the skeleton topology by
    /// path: e.g. `["Root", "Root/Hip", "Root/Hip/Knee"]`. Recover
    /// the parent index per joint by looking up the path prefix in
    /// the same array. Joints with no parent (root joints) get
    /// `None`. Indices match the order in `joints`.
    pub fn joint_parent_indices(&self) -> Vec<Option<usize>> {
        let by_path: std::collections::HashMap<&str, usize> = self
            .joints
            .iter()
            .enumerate()
            .map(|(i, p)| (p.as_str(), i))
            .collect();
        self.joints
            .iter()
            .map(|p| {
                p.rsplit_once('/')
                    .map(|(parent, _)| parent)
                    .and_then(|parent_path| by_path.get(parent_path).copied())
            })
            .collect()
    }

    /// Last segment of each joint path — the convention USDSkel uses
    /// for the bone's display name. Returns the full path if the
    /// joint has no `/`.
    pub fn joint_short_names(&self) -> Vec<&str> {
        self.joints
            .iter()
            .map(|p| p.rsplit_once('/').map(|(_, n)| n).unwrap_or(p.as_str()))
            .collect()
    }
}

/// Read a `Skeleton` prim. Returns `None` when the prim isn't typed
/// `Skeleton` or has no joints authored.
pub fn read_skeleton(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadSkeleton>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "Skeleton" {
        return Ok(None);
    }
    let joints = read_token_vec(stage, prim, "joints")?;
    if joints.is_empty() {
        return Ok(None);
    }
    let bind_transforms = read_mat4f_vec(stage, prim, "bindTransforms")?;
    let rest_transforms = read_mat4f_vec(stage, prim, "restTransforms")?;
    Ok(Some(ReadSkeleton {
        path: prim.as_str().to_string(),
        joints,
        bind_transforms,
        rest_transforms,
    }))
}

/// Read a `SkelRoot` prim. Returns `None` when the prim isn't typed
/// `SkelRoot`.
pub fn read_skel_root(stage: &openusd::Stage, prim: &Path) -> anyhow::Result<Option<ReadSkelRoot>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "SkelRoot" {
        return Ok(None);
    }
    let skeleton = read_rel_first_target(stage, prim, "skel:skeleton")?;
    let animation_source = read_rel_first_target(stage, prim, "skel:animationSource")?;
    Ok(Some(ReadSkelRoot {
        path: prim.as_str().to_string(),
        skeleton,
        animation_source,
    }))
}

/// Read `SkelBindingAPI` primvars off any mesh prim. Returns `None` when
/// the mesh authors no joint indices — which is how USD signals "not
/// skinned". When it DOES author indices, weights must also be present
/// or we return `None` (the binding is malformed).
pub fn read_skel_binding(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadSkelBinding>> {
    let Some(joint_indices) = read_int_vec(stage, prim, "primvars:skel:jointIndices")? else {
        return Ok(None);
    };
    let Some(joint_weights) = read_float_vec(stage, prim, "primvars:skel:jointWeights")? else {
        return Ok(None);
    };
    let skeleton = read_rel_first_target(stage, prim, "skel:skeleton")?;
    // `elementSize` is metadata on the primvar attribute, not a field
    // on the prim. It's stored as Int/Int64 Value at the attr's
    // `elementSize` field.
    let elements_per_vertex = {
        let attr_path = prim
            .append_property("primvars:skel:jointIndices")
            .map_err(anyhow::Error::from)?;
        match stage
            .field::<Value>(attr_path, "elementSize")
            .map_err(anyhow::Error::from)?
        {
            Some(Value::Int(n)) => n,
            Some(Value::Int64(n)) => n as i32,
            _ => 1,
        }
    };
    let joint_subset = read_token_vec(stage, prim, "skel:joints")?;
    let blend_shapes = read_token_vec(stage, prim, "skel:blendShapes")?;
    let blend_shape_targets = read_rel_targets(stage, prim, "skel:blendShapeTargets")?;
    Ok(Some(ReadSkelBinding {
        prim_path: prim.as_str().to_string(),
        skeleton,
        joint_indices,
        joint_weights,
        elements_per_vertex,
        joint_subset,
        blend_shapes,
        blend_shape_targets,
    }))
}

/// Read a `UsdSkelAnimation` prim from an openusd-rs Stage. Returns
/// the same `ReadSkelAnimText` shape produced by the sidecar text
/// parser so downstream code can consume both transparently. Used
/// when the SkelAnimation lives inside a USDZ (USDC binary) where
/// openusd-rs CAN parse it via the standard time-samples path,
/// instead of needing the .usda sidecar workaround.
pub fn read_skel_animation_stage(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<crate::skel_anim_text::ReadSkelAnimText>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "SkelAnimation" {
        return Ok(None);
    }
    let joints = read_token_vec(stage, prim, "joints")?;
    let blend_shapes = read_token_vec(stage, prim, "blendShapes")?;
    if joints.is_empty() && blend_shapes.is_empty() {
        return Ok(None);
    }
    let prim_name = prim
        .as_str()
        .rsplit_once('/')
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| prim.as_str().to_string());

    use crate::skel_anim_text::{OrdF64, ReadSkelAnimText};
    let mut anim = ReadSkelAnimText {
        prim_name,
        joints,
        blend_shapes,
        ..Default::default()
    };

    // translations: float3[] timeSamples — USD Value::Vec3fVec.
    for (t, val) in read_time_samples(stage, prim, "translations")? {
        if let Value::Vec3fVec(v) = val {
            anim.translations.insert(OrdF64(t), v);
        } else if let Value::Vec3dVec(v) = val {
            let cast: Vec<[f32; 3]> = v
                .into_iter()
                .map(|a| [a[0] as f32, a[1] as f32, a[2] as f32])
                .collect();
            anim.translations.insert(OrdF64(t), cast);
        }
    }
    // rotations: quatf[] / quath[] / quatd[] timeSamples.
    for (t, val) in read_time_samples(stage, prim, "rotations")? {
        let v: Option<Vec<[f32; 4]>> = match val {
            Value::QuatfVec(v) => Some(v),
            Value::QuatdVec(v) => Some(
                v.into_iter()
                    .map(|q| [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32])
                    .collect(),
            ),
            Value::QuathVec(v) => Some(
                v.into_iter()
                    .map(|q| {
                        [
                            q[0].to_f32(),
                            q[1].to_f32(),
                            q[2].to_f32(),
                            q[3].to_f32(),
                        ]
                    })
                    .collect(),
            ),
            _ => None,
        };
        if let Some(v) = v {
            anim.rotations.insert(OrdF64(t), v);
        }
    }
    // scales: float3[] / half3[] timeSamples.
    for (t, val) in read_time_samples(stage, prim, "scales")? {
        let v: Option<Vec<[f32; 3]>> = match val {
            Value::Vec3fVec(v) => Some(v),
            Value::Vec3dVec(v) => Some(
                v.into_iter()
                    .map(|a| [a[0] as f32, a[1] as f32, a[2] as f32])
                    .collect(),
            ),
            Value::Vec3hVec(v) => Some(
                v.into_iter()
                    .map(|a| [a[0].to_f32(), a[1].to_f32(), a[2].to_f32()])
                    .collect(),
            ),
            _ => None,
        };
        if let Some(v) = v {
            anim.scales.insert(OrdF64(t), v);
        }
    }
    // blendShapeWeights: float[] timeSamples.
    for (t, val) in read_time_samples(stage, prim, "blendShapeWeights")? {
        if let Value::FloatVec(v) = val {
            anim.blend_shape_weights.insert(OrdF64(t), v);
        }
    }
    Ok(Some(anim))
}

fn read_time_samples(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Vec<(f64, Value)>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(attr_path, "timeSamples")
        .map_err(anyhow::Error::from)?;
    Ok(match raw {
        Some(Value::TimeSamples(v)) => v,
        _ => Vec::new(),
    })
}

/// Read a `BlendShape` prim. Returns `None` when the prim isn't
/// typed `BlendShape` or has no `offsets` authored.
pub fn read_blend_shape(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadBlendShape>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "BlendShape" {
        return Ok(None);
    }
    let offsets = read_vec3f_vec(stage, prim, "offsets")?;
    if offsets.is_empty() {
        return Ok(None);
    }
    let normal_offsets = read_vec3f_vec(stage, prim, "normalOffsets")?;
    let point_indices = read_int_vec(stage, prim, "pointIndices")?.unwrap_or_default();
    Ok(Some(ReadBlendShape {
        path: prim.as_str().to_string(),
        offsets,
        normal_offsets,
        point_indices,
    }))
}

fn read_vec3f_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Vec<[f32; 3]>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?
    {
        Some(Value::Vec3fVec(v)) => v,
        Some(Value::Vec3dVec(v)) => v
            .into_iter()
            .map(|a| [a[0] as f32, a[1] as f32, a[2] as f32])
            .collect(),
        _ => Vec::new(),
    })
}

fn read_rel_targets(
    stage: &openusd::Stage,
    prim: &Path,
    rel_name: &str,
) -> anyhow::Result<Vec<String>> {
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

// ── attribute helpers ────────────────────────────────────────────────

fn read_token_vec(stage: &openusd::Stage, prim: &Path, name: &str) -> anyhow::Result<Vec<String>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?
    {
        Some(Value::TokenVec(v)) | Some(Value::StringVec(v)) => v,
        _ => Vec::new(),
    })
}

fn read_int_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<i32>>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?
    {
        Some(Value::IntVec(v)) => Some(v),
        _ => None,
    })
}

fn read_float_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Vec<f32>>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?
    {
        Some(Value::FloatVec(v)) => Some(v),
        Some(Value::DoubleVec(v)) => Some(v.into_iter().map(|d| d as f32).collect()),
        _ => None,
    })
}

fn read_rel_first_target(
    stage: &openusd::Stage,
    prim: &Path,
    rel_name: &str,
) -> anyhow::Result<Option<String>> {
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

fn read_mat4f_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Vec<[f32; 16]>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)?
    {
        Some(Value::Matrix4dVec(v)) => v
            .into_iter()
            .map(|m| {
                let mut out = [0.0f32; 16];
                for i in 0..16 {
                    out[i] = m[i] as f32;
                }
                out
            })
            .collect(),
        _ => Vec::new(),
    })
}
