//! UsdGeom → `bevy::render::mesh::Mesh`.
//!
//! Two kinds of input:
//! - Full meshes (`UsdGeom.Mesh`) — converts points / face indices / normals
//!   / uvs, fan-triangulates faces > 3 verts, expands `faceVarying` primvars.
//! - Primitive shapes (`Cube`, `Sphere`, `Cylinder`, `Capsule`) — delegate
//!   to Bevy's built-in `Meshable` primitives with the right dimensions.
//!
//! Orientation (`"leftHanded"` flips winding) and missing-normal fallback
//! (`compute_smooth_normals`) are handled here.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, Meshable, PrimitiveTopology, VertexAttributeValues};
use usd_schemas::geom::{Axis, Interpolation, MeshPrimvar, Orientation, ReadCylinder, ReadMesh};


/// Per-USD-point skinning data, normalised to Bevy's fixed 4-influences-
/// per-vertex layout. Built from a `ReadSkelBinding` via
/// [`skin_attrs_from_binding`]; passed into [`mesh_from_usd_subset`]
/// so the right per-corner copy lands in the emitted mesh.
#[derive(Debug, Clone)]
pub struct SkinAttrs {
    /// Joint index per influence — 4 per USD point.
    pub indices: Vec<[u16; 4]>,
    /// Skin weight per influence — 4 per USD point. Renormalised to
    /// sum to 1 after top-4 truncation.
    pub weights: Vec<[f32; 4]>,
}

/// Convert a USD `SkelBindingAPI` (variable elementSize jointIndices /
/// jointWeights flat array) into Bevy-shaped 4-wide skin attributes
/// keyed per USD point. `vertex_count` should match `read.points.len()`
/// — i.e. the unexpanded vertex count. When the binding authors more
/// than 4 influences per vertex, top-4 by weight are kept and
/// renormalised to sum to 1.
pub fn skin_attrs_from_binding(
    binding: &usd_schemas::skel::ReadSkelBinding,
    vertex_count: usize,
    max_joint_count: u16,
) -> SkinAttrs {
    let n = binding.elements_per_vertex.max(1) as usize;
    let mut indices = vec![[0u16; 4]; vertex_count];
    let mut weights = vec![[0f32; 4]; vertex_count];
    for v in 0..vertex_count {
        let base = v * n;
        // Top-4 by weight, AFTER filtering out indices that exceed
        // the Skeleton's joint count. Pixar's HumanFemale authors
        // 109-joint binding indices against a composed Skeleton that
        // our 66-joint reference resolves — variants/composition we
        // can't yet flatten introduce the gap. Out-of-range indices
        // referencing unbound `SkinnedMesh.joints` slots produce
        // wild distortion ("elongated brush"). Zeroing the weight
        // collapses the vertex onto its remaining valid influences;
        // when none remain we fall back to the Skeleton root (joint
        // 0) so the vertex at least stays attached to the rig.
        let mut entries: Vec<(u16, f32)> = (0..n)
            .filter_map(|k| {
                let idx = binding
                    .joint_indices
                    .get(base + k)
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u16;
                let w = binding.joint_weights.get(base + k).copied().unwrap_or(0.0);
                if idx < max_joint_count {
                    Some((idx, w.max(0.0)))
                } else {
                    None
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        let take = entries.len().min(4);
        let mut sum = 0.0f32;
        for k in 0..take {
            indices[v][k] = entries[k].0;
            weights[v][k] = entries[k].1;
            sum += weights[v][k];
        }
        if sum > 0.0 {
            for k in 0..4 {
                weights[v][k] /= sum;
            }
        } else {
            // Pin to root joint at full weight when every authored
            // influence was out-of-range. Vertex tracks the rig's
            // origin instead of flying off to infinity.
            indices[v] = [0, 0, 0, 0];
            weights[v] = [1.0, 0.0, 0.0, 0.0];
        }
    }
    SkinAttrs { indices, weights }
}

/// Convert a `usd_schemas::geom::ReadMesh` into a Bevy mesh.
///
/// Steps:
/// 1. Triangulate each face by fan (works for triangles and convex quads;
///    non-convex n-gons need an ear-clip pass we punt to M2.1).
/// 2. Expand per-vertex attributes for `faceVarying` primvars (one vertex
///    per corner) or keep indexed when interpolation is `vertex`.
/// 3. Fall back to `compute_smooth_normals` when normals aren't authored.
/// 4. Flip index winding when `orientation == LeftHanded`.
pub fn mesh_from_usd(read: &ReadMesh) -> Mesh {
    mesh_from_usd_subset_with_skin(read, None, None)
}

/// Same as [`mesh_from_usd`] but bakes the supplied skin attributes
/// into `ATTRIBUTE_JOINT_INDEX` / `ATTRIBUTE_JOINT_WEIGHT` so the result
/// can be used with Bevy's `SkinnedMesh` component.
pub fn mesh_from_usd_with_skin(read: &ReadMesh, skin: &SkinAttrs) -> Mesh {
    mesh_from_usd_subset_with_skin(read, None, Some(skin))
}

/// Same as [`mesh_from_usd`] but emits only the faces in `face_subset` when
/// provided. Used to split a `UsdGeom.Mesh` into one Bevy mesh per
/// `GeomSubset` so each subset can carry its own material binding.
///
/// `face_subset = None` emits every face.
pub fn mesh_from_usd_subset(read: &ReadMesh, face_subset: Option<&[i32]>) -> Mesh {
    mesh_from_usd_subset_with_skin(read, face_subset, None)
}

/// Variant of [`mesh_from_usd_subset`] that also bakes per-vertex
/// skinning data into the resulting mesh. `skin` carries one
/// `[u16; 4]` / `[f32; 4]` pair per USD point (i.e. unexpanded), so
/// the indexed and expanded paths can each look up the right slot via
/// the same `point_ix` they use for positions.
pub fn mesh_from_usd_subset_with_skin(
    read: &ReadMesh,
    face_subset: Option<&[i32]>,
    skin: Option<&SkinAttrs>,
) -> Mesh {
    // Face-Varying or Uniform (per-face) primvars break the indexed
    // point-sharing optimisation — vertex-indexed output can't represent
    // a per-face or per-corner value when a vertex is shared between
    // faces with different authored values. Expand to per-corner layout
    // in those cases.
    let non_indexed = |interp: Interpolation| {
        matches!(interp, Interpolation::FaceVarying | Interpolation::Uniform)
    };
    let expand = read
        .normals
        .as_ref()
        .map(|p| non_indexed(p.interpolation))
        .unwrap_or(false)
        || read
            .uvs
            .as_ref()
            .map(|p| non_indexed(p.interpolation))
            .unwrap_or(false)
        || read
            .display_color
            .as_ref()
            .map(|p| non_indexed(p.interpolation))
            .unwrap_or(false)
        || read
            .display_opacity
            .as_ref()
            .map(|p| non_indexed(p.interpolation))
            .unwrap_or(false);

    let (positions, normals, uvs, colors, indices, skin_per_vertex) = if expand {
        let (p, n, u, c, i) = build_expanded(read, face_subset);
        // Expanded path: each corner is its own vertex, expand
        // per-USD-point skin data along the face_vertex_indices map
        // exactly like positions are expanded.
        let skin_v = skin.map(|s| {
            let mut idx = Vec::with_capacity(p.len());
            let mut wgt = Vec::with_capacity(p.len());
            for face_verts in &read.face_vertex_counts {
                let n = *face_verts as usize;
                let mut consumed = 0usize;
                let _ = n; let _ = consumed; // silence unused if loop empty
                for k in 0..(*face_verts as usize) {
                    let _ = k;
                }
            }
            // Simpler: iterate corners in the same order build_expanded did
            let mut corner_ix = 0usize;
            for face_verts in &read.face_vertex_counts {
                for k in 0..(*face_verts as usize) {
                    let point_ix = read.face_vertex_indices[corner_ix + k] as usize;
                    idx.push(s.indices.get(point_ix).copied().unwrap_or([0u16; 4]));
                    wgt.push(s.weights.get(point_ix).copied().unwrap_or([0.0f32; 4]));
                }
                corner_ix += *face_verts as usize;
            }
            (idx, wgt)
        });
        (p, n, u, c, i, skin_v)
    } else {
        let (p, n, u, c, i) = build_indexed(read, face_subset);
        // Indexed path: positions correspond 1:1 with USD points.
        let skin_v = skin.map(|s| {
            let mut idx = vec![[0u16; 4]; p.len()];
            let mut wgt = vec![[0.0f32; 4]; p.len()];
            for v in 0..p.len() {
                idx[v] = s.indices.get(v).copied().unwrap_or([0u16; 4]);
                wgt[v] = s.weights.get(v).copied().unwrap_or([0.0f32; 4]);
            }
            (idx, wgt)
        });
        (p, n, u, c, i, skin_v)
    };

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    // USD's `primvars:st` convention puts (0,0) at the texture's
    // bottom-left corner. Bevy / glTF / wgpu use top-left, so V is
    // inverted between the two systems. Flip on the way in so the
    // authored texture lands right-side-up — without this, eyes paint
    // on tails, etc.
    let uvs: Vec<[f32; 2]> = uvs.into_iter().map(|[u, v]| [u, 1.0 - v]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    if let Some(cs) = colors {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, cs);
    }
    if let Some((joint_idx, joint_wgt)) = skin_per_vertex {
        // Bevy 0.18 expects Uint16x4 for joint indices and Float32x4
        // for joint weights. There's no `From<Vec<[u16; 4]>>` for
        // VertexAttributeValues so we construct the variant directly.
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_JOINT_INDEX,
            VertexAttributeValues::Uint16x4(joint_idx),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_wgt);
    }
    // Indices first so `compute_smooth_normals` has a topology to
    // average across — it requires an indexed mesh to find adjacent
    // faces.
    mesh.insert_indices(Indices::U32(indices));
    if let Some(ns) = normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, ns);
    } else {
        // `compute_flat_normals` replicates positions so normals are per-face
        // — correct but bloats the mesh. Smooth normals keep the original
        // topology and average adjacent face normals. For plain USD stages
        // without authored normals that's the intuitive default.
        mesh.compute_smooth_normals();
    }
    // MikkT vertex tangents — Bevy's PBR shader needs `ATTRIBUTE_TANGENT`
    // to evaluate normal maps correctly. Without them, normal-mapped
    // surfaces silently fall back to geometric normals and the surface
    // detail (drummer stitching, glove leather, biplane rivets) looks
    // flat. `generate_tangents` requires positions + normals + UV0 — all
    // present at this point; failures are fatal-but-rare and we just log.
    if let Err(e) = mesh.generate_tangents() {
        bevy::log::debug!("mesh: generate_tangents failed: {e}");
    }
    mesh
}

/// Build the common case: indexed triangle list, one vertex per USD point.
/// Uses vertex-level or constant interpolation only.
fn build_indexed(
    read: &ReadMesh,
    face_subset: Option<&[i32]>,
) -> (
    Vec<[f32; 3]>,
    Option<Vec<[f32; 3]>>,
    Vec<[f32; 2]>,
    Option<Vec<[f32; 4]>>,
    Vec<u32>,
) {
    let positions = read.points.clone();

    // Normals: pick up vertex-indexed data if present; else None and let
    // `compute_smooth_normals` handle it. `Varying` is semantically
    // per-point for polygonal meshes (USD spec) so we ride the same
    // path as `Vertex` — silently dropping it would force generated
    // smooth normals over authored ones.
    let normals = read.normals.as_ref().and_then(|p| {
        match p.interpolation {
            Interpolation::Vertex | Interpolation::Varying => {
                Some(expand_vertex_primvar(&p, positions.len(), [0.0, 1.0, 0.0]))
            }
            Interpolation::Constant if !p.values.is_empty() => {
                Some(vec![p.values[0]; positions.len()])
            }
            _ => None,
        }
    });

    let uvs = read
        .uvs
        .as_ref()
        .and_then(|p| match p.interpolation {
            Interpolation::Vertex | Interpolation::Varying => {
                Some(expand_vertex_primvar(&p, positions.len(), [0.0, 0.0]))
            }
            _ => None,
        })
        .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

    let colors = build_vertex_colors_indexed(read, positions.len());

    let indices = triangulate_polygon(
        &positions,
        &read.face_vertex_counts,
        &read.face_vertex_indices,
        read.orientation,
        face_subset,
    );
    (positions, normals, uvs, colors, indices)
}

/// For indexed output, displayColor / displayOpacity only contribute when
/// they're vertex- or constant-interpolated (faceVarying/uniform force the
/// expanded path). Returns `None` when there's nothing to emit so the
/// caller can skip writing the attribute at all.
fn build_vertex_colors_indexed(
    read: &ReadMesh,
    vertex_count: usize,
) -> Option<Vec<[f32; 4]>> {
    if read.display_color.is_none() && read.display_opacity.is_none() {
        return None;
    }
    let mut colors = vec![[1.0f32, 1.0, 1.0, 1.0]; vertex_count];
    if let Some(dc) = read.display_color.as_ref() {
        let rgbs = match dc.interpolation {
            Interpolation::Constant if !dc.values.is_empty() => {
                vec![dc.values[0]; vertex_count]
            }
            // Single-value primvar — broadcast regardless of which
            // interpolation token was authored. Pixar's Kitchen_set
            // authors `primvars:displayColor = [(0.5, 0.5, 0.4)]`
            // with no `interpolation` token; the schema reader's
            // default of `Vertex` then fails to expand a 1-element
            // array to vertex_count and falls through to white.
            _ if dc.values.len() == 1 => vec![dc.values[0]; vertex_count],
            // `Varying` is semantically per-vertex for polygonal meshes,
            // so it rides the same indexed path as `Vertex`.
            Interpolation::Vertex | Interpolation::Varying => {
                expand_vertex_primvar(dc, vertex_count, [1.0, 1.0, 1.0])
            }
            _ => vec![[1.0, 1.0, 1.0]; vertex_count],
        };
        for (i, rgb) in rgbs.iter().enumerate() {
            colors[i][0] = rgb[0];
            colors[i][1] = rgb[1];
            colors[i][2] = rgb[2];
        }
    }
    if let Some(dop) = read.display_opacity.as_ref() {
        let alphas = match dop.interpolation {
            Interpolation::Constant if !dop.values.is_empty() => {
                vec![dop.values[0]; vertex_count]
            }
            // Single-value primvar — broadcast regardless of declared
            // interpolation (see `display_color` arm above for the
            // Pixar Kitchen_set rationale).
            _ if dop.values.len() == 1 => vec![dop.values[0]; vertex_count],
            Interpolation::Vertex | Interpolation::Varying => {
                expand_vertex_primvar(dop, vertex_count, 1.0)
            }
            _ => vec![1.0; vertex_count],
        };
        for (i, a) in alphas.iter().enumerate() {
            colors[i][3] = *a;
        }
    }
    Some(colors)
}

/// Build the fully-expanded form: one vertex per face corner so `faceVarying`
/// primvars (cube uvs, seams) can be represented.
fn build_expanded(
    read: &ReadMesh,
    face_subset: Option<&[i32]>,
) -> (
    Vec<[f32; 3]>,
    Option<Vec<[f32; 3]>>,
    Vec<[f32; 2]>,
    Option<Vec<[f32; 4]>>,
    Vec<u32>,
) {
    let corner_count: usize = read.face_vertex_counts.iter().map(|c| *c as usize).sum();
    let mut positions = Vec::with_capacity(corner_count);
    let mut normals_out: Vec<[f32; 3]> = Vec::with_capacity(corner_count);
    let mut uvs_out: Vec<[f32; 2]> = Vec::with_capacity(corner_count);
    let mut colors_out: Vec<[f32; 4]> = Vec::with_capacity(corner_count);

    let want_normals = read.normals.is_some();
    let want_uvs = read.uvs.is_some();
    let want_colors = read.display_color.is_some() || read.display_opacity.is_some();

    // When normals aren't authored, compute them on the *unexpanded*
    // point-indexed mesh so vertices shared between faces produce a
    // smoothed (averaged) normal. If we let `Mesh::compute_smooth_normals`
    // run after expansion, every corner is its own vertex (because some
    // other primvar — usually FaceVarying UVs for texture seams — forced
    // expansion), so "smooth" normals collapse to face normals and you
    // see every polygon. Compute once, then index per corner.
    let smooth_per_point: Option<Vec<[f32; 3]>> =
        (!want_normals).then(|| compute_point_smooth_normals(read));

    let mut corner_ix: usize = 0;
    for (face_ix, face_verts) in read.face_vertex_counts.iter().enumerate() {
        for k in 0..(*face_verts as usize) {
            let point_ix = read.face_vertex_indices[corner_ix + k] as usize;
            positions.push(read.points[point_ix]);
            if want_normals {
                normals_out.push(corner_normal(read, face_ix, corner_ix + k, point_ix));
            } else if let Some(ref ns) = smooth_per_point {
                normals_out.push(*ns.get(point_ix).unwrap_or(&[0.0, 1.0, 0.0]));
            }
            if want_uvs {
                uvs_out.push(corner_uv(read, face_ix, corner_ix + k, point_ix));
            } else {
                // Pad UVs so `ATTRIBUTE_UV_0` always has the same
                // length as `ATTRIBUTE_POSITION` — mismatched lengths
                // make Bevy silently drop the mesh.
                uvs_out.push([0.0, 0.0]);
            }
            if want_colors {
                colors_out.push(corner_color(read, face_ix, corner_ix + k, point_ix));
            }
        }
        corner_ix += *face_verts as usize;
    }

    // After expansion, indices become sequential 0..N per face, then
    // fan-triangulated. Re-derive a pseudo `faceVertexIndices` of the form
    // [0,1,2,3, 4,5,6, …] so `triangulate_fan` can do its job.
    let mut sequential = Vec::with_capacity(corner_count);
    let mut running = 0u32;
    for face_verts in &read.face_vertex_counts {
        for _ in 0..*face_verts {
            sequential.push(running as i32);
            running += 1;
        }
    }
    let indices = triangulate_polygon(
        &positions,
        &read.face_vertex_counts,
        &sequential,
        read.orientation,
        face_subset,
    );

    // We emit normals whenever they were authored OR we synthesised
    // them from the point-smooth pass. The latter is the difference
    // between "smooth like Hydra" and "every face is visible".
    let emit_normals = want_normals || smooth_per_point.is_some();
    (
        positions,
        emit_normals.then_some(normals_out),
        uvs_out,
        want_colors.then_some(colors_out),
        indices,
    )
}

/// Per-point area-weighted smooth normals on the *unexpanded* mesh.
/// USD's faceVertexIndices is a flat per-corner list; we accumulate each
/// face's plane normal (scaled by 2× area) into all its corner points.
/// Ear-/fan-decompose larger faces just like the renderer does — using
/// (a, b, c) for k=1..n-1 keeps the contribution proportional to face
/// area for convex polygons and is good enough for concave ones since
/// a missing area cancels symmetrically.
///
/// Returns `read.points.len()` normals, normalised. Vertices unreferenced
/// by any face fall back to (0,1,0).
fn compute_point_smooth_normals(read: &ReadMesh) -> Vec<[f32; 3]> {
    let mut accum = vec![Vec3::ZERO; read.points.len()];
    let mut corner_ix = 0usize;
    for face_verts in &read.face_vertex_counts {
        let n = *face_verts as usize;
        if n >= 3 {
            let i0 = read.face_vertex_indices[corner_ix] as usize;
            let p0 = Vec3::from_array(read.points[i0]);
            for k in 1..(n - 1) {
                let i1 = read.face_vertex_indices[corner_ix + k] as usize;
                let i2 = read.face_vertex_indices[corner_ix + k + 1] as usize;
                let p1 = Vec3::from_array(read.points[i1]);
                let p2 = Vec3::from_array(read.points[i2]);
                let face_n = match read.orientation {
                    Orientation::RightHanded => (p1 - p0).cross(p2 - p0),
                    Orientation::LeftHanded => (p2 - p0).cross(p1 - p0),
                };
                accum[i0] += face_n;
                accum[i1] += face_n;
                accum[i2] += face_n;
            }
        }
        corner_ix += n;
    }
    accum
        .into_iter()
        .map(|v| {
            if v.length_squared() > 1e-20 {
                let n = v.normalize();
                [n.x, n.y, n.z]
            } else {
                [0.0, 1.0, 0.0]
            }
        })
        .collect()
}

fn corner_normal(read: &ReadMesh, face: usize, corner: usize, point: usize) -> [f32; 3] {
    let p = read.normals.as_ref().unwrap();
    sample_primvar_3(p, face, corner, point, [0.0, 1.0, 0.0])
}

fn corner_color(read: &ReadMesh, face: usize, corner: usize, point: usize) -> [f32; 4] {
    let rgb_fallback = [1.0_f32, 1.0, 1.0];
    let rgb = read
        .display_color
        .as_ref()
        .map(|dc| sample_primvar_3(dc, face, corner, point, rgb_fallback))
        .unwrap_or(rgb_fallback);
    let a = read
        .display_opacity
        .as_ref()
        .map(|dop| sample_primvar_1(dop, face, corner, point, 1.0))
        .unwrap_or(1.0);
    [rgb[0], rgb[1], rgb[2], a]
}

/// Sample a vec3 primvar at a specific corner. Single-value primvars
/// broadcast regardless of declared interpolation — Pixar's
/// Kitchen_set authors `primvars:displayColor = [(0.5, 0.5, 0.4)]`
/// without an `interpolation` token; the schema reader's default of
/// `Vertex` then fails to expand the 1-element array to vertex_count
/// and falls back to white.
fn sample_primvar_3(
    p: &MeshPrimvar<[f32; 3]>,
    face: usize,
    corner: usize,
    point: usize,
    fallback: [f32; 3],
) -> [f32; 3] {
    if p.values.len() == 1 {
        return p.values[0];
    }
    let lookup = |slot: usize| -> [f32; 3] {
        let ix = if !p.indices.is_empty() {
            *p.indices.get(slot).unwrap_or(&0) as usize
        } else {
            slot
        };
        p.values.get(ix).copied().unwrap_or(fallback)
    };
    match p.interpolation {
        Interpolation::Constant => p.values.first().copied().unwrap_or(fallback),
        Interpolation::Uniform => lookup(face),
        Interpolation::Vertex | Interpolation::Varying => lookup(point),
        Interpolation::FaceVarying => lookup(corner),
    }
}

fn sample_primvar_1(
    p: &MeshPrimvar<f32>,
    face: usize,
    corner: usize,
    point: usize,
    fallback: f32,
) -> f32 {
    if p.values.len() == 1 {
        return p.values[0];
    }
    let lookup = |slot: usize| -> f32 {
        let ix = if !p.indices.is_empty() {
            *p.indices.get(slot).unwrap_or(&0) as usize
        } else {
            slot
        };
        p.values.get(ix).copied().unwrap_or(fallback)
    };
    match p.interpolation {
        Interpolation::Constant => p.values.first().copied().unwrap_or(fallback),
        Interpolation::Uniform => lookup(face),
        Interpolation::Vertex | Interpolation::Varying => lookup(point),
        Interpolation::FaceVarying => lookup(corner),
    }
}

fn corner_uv(read: &ReadMesh, face: usize, corner: usize, point: usize) -> [f32; 2] {
    let p = read.uvs.as_ref().unwrap();
    let fallback = [0.0, 0.0];
    if p.values.len() == 1 {
        return p.values[0];
    }
    match p.interpolation {
        Interpolation::Constant => p.values.first().copied().unwrap_or(fallback),
        Interpolation::Uniform => {
            let ix = if !p.indices.is_empty() {
                *p.indices.get(face).unwrap_or(&0) as usize
            } else {
                face
            };
            p.values.get(ix).copied().unwrap_or(fallback)
        }
        Interpolation::Vertex | Interpolation::Varying => {
            let ix = if !p.indices.is_empty() {
                *p.indices.get(point).unwrap_or(&0) as usize
            } else {
                point
            };
            p.values.get(ix).copied().unwrap_or(fallback)
        }
        Interpolation::FaceVarying => {
            let ix = if !p.indices.is_empty() {
                *p.indices.get(corner).unwrap_or(&0) as usize
            } else {
                corner
            };
            p.values.get(ix).copied().unwrap_or(fallback)
        }
    }
}

fn expand_vertex_primvar<T: Copy>(
    primvar: &MeshPrimvar<T>,
    expected_len: usize,
    fallback: T,
) -> Vec<T> {
    // Always emit exactly `expected_len` entries — Bevy 0.18 silently
    // drops the mesh if attribute lengths don't match
    // `ATTRIBUTE_POSITION`. Pad with `fallback` if the authored data
    // is short, truncate if it's long.
    let mut out = vec![fallback; expected_len];
    if primvar.indices.is_empty() {
        for (i, v) in primvar.values.iter().take(expected_len).enumerate() {
            out[i] = *v;
        }
    } else {
        for (i, ix) in primvar.indices.iter().take(expected_len).enumerate() {
            if let Some(v) = primvar.values.get(*ix as usize) {
                out[i] = *v;
            }
        }
    }
    out
}

/// Triangulate each face into a triangle list. Smart enough for the three
/// cases real USD assets throw at us:
///
/// - **n = 3**: emit as-is.
/// - **n = 4** (the dominant case in production assets): pick the *shorter*
///   diagonal. Non-planar quads — almost universal in subdivided cages and
///   imported FBX — produce a visible crease along whichever diagonal a fan
///   triangulator picks. Choosing the diagonal that minimises the triangle
///   pair's perimeter aligns the crease with the surface curvature, which
///   is what every offline renderer (and Maya/Blender's default) does.
/// - **n ≥ 5**: ear-clip. Fan triangulation of a concave n-gon emits
///   triangles *outside* the polygon (showing through to the back) and
///   misses parts inside it — which is exactly the "spiky / missing
///   triangles" symptom on production-asset n-gons. Ear clipping handles
///   concave faces correctly. We compute the polygon normal via Newell's
///   method (works on non-planar polygons too) and pick ears in 2D after
///   projecting onto the plane perpendicular to that normal.
///
/// Falls back to fan triangulation if the polygon is degenerate (all colinear
/// points) — emitting *something* matches USD's permissive behaviour.
///
/// `LeftHanded` orientation flips the winding so Bevy's default back-face
/// culling shows the right side.
///
/// `face_subset = Some(&[face_ix])` emits only the listed faces — used by
/// the GeomSubset per-material split.
fn triangulate_polygon(
    positions: &[[f32; 3]],
    counts: &[i32],
    indices: &[i32],
    orientation: Orientation,
    face_subset: Option<&[i32]>,
) -> Vec<u32> {
    // Precompute each face's starting corner so a subset by face index
    // jumps straight to the right slice without rewalking the counts.
    let mut face_starts = Vec::with_capacity(counts.len());
    let mut running = 0usize;
    for c in counts {
        face_starts.push(running);
        running += *c as usize;
    }

    let face_iter: Box<dyn Iterator<Item = usize>> = match face_subset {
        None => Box::new(0..counts.len()),
        Some(sub) => Box::new(
            sub.iter()
                .map(|i| *i as usize)
                .filter(|i| *i < counts.len()),
        ),
    };

    let mut out = Vec::new();
    let emit = |out: &mut Vec<u32>, a: u32, b: u32, c: u32| match orientation {
        Orientation::RightHanded => out.extend_from_slice(&[a, b, c]),
        Orientation::LeftHanded => out.extend_from_slice(&[a, c, b]),
    };
    let pos_of = |idx: i32| -> Vec3 {
        let p = positions[idx as usize];
        Vec3::new(p[0], p[1], p[2])
    };

    for face_ix in face_iter {
        let face_start = face_starts[face_ix];
        let n = counts[face_ix] as usize;
        if n < 3 {
            continue;
        }
        if n == 3 {
            let a = indices[face_start] as u32;
            let b = indices[face_start + 1] as u32;
            let c = indices[face_start + 2] as u32;
            emit(&mut out, a, b, c);
            continue;
        }
        if n == 4 {
            let i0 = indices[face_start];
            let i1 = indices[face_start + 1];
            let i2 = indices[face_start + 2];
            let i3 = indices[face_start + 3];
            // Pick the shorter diagonal: 0–2 vs 1–3.
            let p0 = pos_of(i0);
            let p1 = pos_of(i1);
            let p2 = pos_of(i2);
            let p3 = pos_of(i3);
            let d02 = (p2 - p0).length_squared();
            let d13 = (p3 - p1).length_squared();
            if d02 <= d13 {
                emit(&mut out, i0 as u32, i1 as u32, i2 as u32);
                emit(&mut out, i0 as u32, i2 as u32, i3 as u32);
            } else {
                emit(&mut out, i1 as u32, i2 as u32, i3 as u32);
                emit(&mut out, i1 as u32, i3 as u32, i0 as u32);
            }
            continue;
        }
        // n >= 5: ear clip. Collect corner positions and indices.
        let face_indices: Vec<i32> = indices[face_start..face_start + n].to_vec();
        let face_positions: Vec<Vec3> = face_indices.iter().map(|i| pos_of(*i)).collect();
        ear_clip_into(&face_positions, &face_indices, &mut out, orientation);
    }
    out
}

/// Ear-clip a polygon (n ≥ 4 in practice) into triangles, appending into
/// `out`. Robust against concave polygons; for non-planar polygons we
/// project onto the plane perpendicular to the Newell normal so the 2D
/// containment test is meaningful.
///
/// Falls back to fan triangulation if no ears can be found (e.g. fully
/// degenerate / self-intersecting input). That matches USD's "translate
/// what you can, drop nothing" expectation.
fn ear_clip_into(
    positions: &[Vec3],
    indices: &[i32],
    out: &mut Vec<u32>,
    orientation: Orientation,
) {
    let n = positions.len();
    let emit = |out: &mut Vec<u32>, a: u32, b: u32, c: u32| match orientation {
        Orientation::RightHanded => out.extend_from_slice(&[a, b, c]),
        Orientation::LeftHanded => out.extend_from_slice(&[a, c, b]),
    };

    // Newell's method: robust normal even for non-planar polygons. Sums
    // per-edge cross-products of the projected components.
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let cur = positions[i];
        let nxt = positions[(i + 1) % n];
        normal.x += (cur.y - nxt.y) * (cur.z + nxt.z);
        normal.y += (cur.z - nxt.z) * (cur.x + nxt.x);
        normal.z += (cur.x - nxt.x) * (cur.y + nxt.y);
    }
    if normal.length_squared() < 1e-20 {
        // Degenerate polygon — fall back to fan.
        for k in 1..(n - 1) {
            emit(out, indices[0] as u32, indices[k] as u32, indices[k + 1] as u32);
        }
        return;
    }
    let normal = normal.normalize();

    // Build orthonormal basis (u, v) on the polygon plane to project into
    // 2D. Pick the smallest absolute component of the normal as the
    // helper axis to avoid degeneracy.
    let helper = if normal.x.abs() < normal.y.abs() && normal.x.abs() < normal.z.abs() {
        Vec3::X
    } else if normal.y.abs() < normal.z.abs() {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let u = normal.cross(helper).normalize();
    let v = normal.cross(u);
    let project = |p: Vec3| -> [f32; 2] { [p.dot(u), p.dot(v)] };

    let pts2: Vec<[f32; 2]> = positions.iter().map(|p| project(*p)).collect();

    // Determine polygon winding in 2D. Signed area > 0 → CCW.
    let mut signed_area = 0.0f32;
    for i in 0..n {
        let a = pts2[i];
        let b = pts2[(i + 1) % n];
        signed_area += a[0] * b[1] - b[0] * a[1];
    }
    let ccw = signed_area > 0.0;

    // Active vertex list (linked-list-style via Vec).
    let mut remaining: Vec<usize> = (0..n).collect();
    // Worst-case ear clipping is O(n²) but n is tiny (≤ ~10 in practice).
    let max_iters = n * n + 8;
    let mut iters = 0;
    while remaining.len() > 3 && iters < max_iters {
        iters += 1;
        let m = remaining.len();
        let mut clipped = false;
        for i in 0..m {
            let i_prev = remaining[(i + m - 1) % m];
            let i_cur = remaining[i];
            let i_next = remaining[(i + 1) % m];
            let a = pts2[i_prev];
            let b = pts2[i_cur];
            let c = pts2[i_next];
            // Convex test in chosen winding.
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            let convex = if ccw { cross > 0.0 } else { cross < 0.0 };
            if !convex {
                continue;
            }
            // Ear test: no other remaining vertex inside triangle (a,b,c).
            let mut contains_other = false;
            for j in 0..m {
                let idx = remaining[j];
                if idx == i_prev || idx == i_cur || idx == i_next {
                    continue;
                }
                let p = pts2[idx];
                if point_in_triangle_2d(p, a, b, c) {
                    contains_other = true;
                    break;
                }
            }
            if contains_other {
                continue;
            }
            // Emit and clip.
            emit(out, indices[i_prev] as u32, indices[i_cur] as u32, indices[i_next] as u32);
            remaining.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // No ear found — bail out and fan-triangulate the rest.
            break;
        }
    }
    if remaining.len() == 3 {
        emit(out, indices[remaining[0]] as u32, indices[remaining[1]] as u32, indices[remaining[2]] as u32);
    } else if remaining.len() > 3 {
        // Fallback: fan over what's left.
        let r0 = remaining[0];
        for k in 1..(remaining.len() - 1) {
            emit(out, indices[r0] as u32, indices[remaining[k]] as u32, indices[remaining[k + 1]] as u32);
        }
    }
}

/// Standard barycentric inside-triangle test. Includes points exactly on
/// edges (we still emit ears even if a vertex sits on an edge — the
/// alternative is endless retries on collinear data).
fn point_in_triangle_2d(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d_x = p[0] - c[0];
    let d_y = p[1] - c[1];
    let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denom.abs() < 1e-20 {
        return false;
    }
    let s = ((b[1] - c[1]) * d_x + (c[0] - b[0]) * d_y) / denom;
    let t = ((c[1] - a[1]) * d_x + (a[0] - c[0]) * d_y) / denom;
    s > 0.0 && t > 0.0 && (s + t) < 1.0
}

// ── Primitive shapes ────────────────────────────────────────────────────

/// Build a Bevy mesh from a UsdGeom.Cube's `size`. The USD cube is
/// size × size × size centred at the prim origin.
pub fn mesh_cube(size: f64) -> Mesh {
    Mesh::from(bevy::math::primitives::Cuboid::new(
        size as f32,
        size as f32,
        size as f32,
    ))
}

/// UsdGeom.Sphere radius → Bevy's UV sphere.
pub fn mesh_sphere(radius: f64) -> Mesh {
    Mesh::from(bevy::math::primitives::Sphere::new(radius as f32))
}

/// UsdGeom.Cylinder dimensions + axis. Bevy's `Cylinder` points up the Y
/// axis by convention, so we apply an axis rotation for X/Z cases.
pub fn mesh_cylinder(params: ReadCylinder) -> Mesh {
    let mut mesh = Mesh::from(bevy::math::primitives::Cylinder::new(
        params.radius as f32,
        params.height as f32,
    ));
    apply_axis(&mut mesh, params.axis);
    mesh
}

/// UsdGeom.Plane `width` × `length`. Y-normal plane centred at the origin.
pub fn mesh_plane(width: f64, length: f64) -> Mesh {
    Mesh::from(
        bevy::math::primitives::Plane3d::default()
            .mesh()
            .size(width as f32, length as f32),
    )
}

/// UsdGeom.Capsule dimensions + axis. Bevy's `Capsule3d` is Y-axis aligned.
pub fn mesh_capsule(params: ReadCylinder) -> Mesh {
    // UsdGeom.Capsule's `height` is the cylinder portion length (hemispheres
    // add `2*radius` to the total). Bevy's Capsule3d takes `half_length` =
    // half the cylinder portion.
    let mut mesh = Mesh::from(bevy::math::primitives::Capsule3d::new(
        params.radius as f32,
        params.height as f32,
    ));
    apply_axis(&mut mesh, params.axis);
    mesh
}

/// Rotate vertices so a Y-up primitive faces the requested axis.
fn apply_axis(mesh: &mut Mesh, axis: Axis) {
    let rot = match axis {
        Axis::Y => return,
        Axis::X => bevy::math::Quat::from_rotation_z(-core::f32::consts::FRAC_PI_2),
        Axis::Z => bevy::math::Quat::from_rotation_x(core::f32::consts::FRAC_PI_2),
    };
    rotate_mesh(mesh, rot);
}

pub fn rotate_mesh(mesh: &mut Mesh, rot: bevy::math::Quat) {
    if let Some(VertexAttributeValues::Float32x3(ps)) = mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for p in ps.iter_mut() {
            let v = rot * Vec3::new(p[0], p[1], p[2]);
            *p = [v.x, v.y, v.z];
        }
    }
    if let Some(VertexAttributeValues::Float32x3(ns)) = mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL) {
        for n in ns.iter_mut() {
            let v = rot * Vec3::new(n[0], n[1], n[2]);
            *n = [v.x, v.y, v.z];
        }
    }
}
