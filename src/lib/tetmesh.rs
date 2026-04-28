//! UsdGeom.TetMesh → Bevy `Mesh` (boundary surface).
//!
//! TetMesh is a volumetric prim type: N tetrahedra sharing a flat
//! point pool. We render the boundary surface — i.e. every triangular
//! face that belongs to exactly one tet (interior faces touch two
//! tets and cancel out).
//!
//! When the authoring tool caches `surfaceFaceVertexIndices`, we trust
//! it. Otherwise we compute it on load:
//!
//! 1. For each tet `(a,b,c,d)`, emit its four faces — `(a,b,c)`,
//!    `(a,c,d)`, `(a,d,b)`, `(b,d,c)`.
//! 2. Sort each face's three indices to get a canonical key.
//! 3. Faces seen exactly once are boundary; faces seen twice are
//!    interior and dropped.
//!
//! Original (unsorted) indices are kept so the winding the tet author
//! intended survives — important for backface culling.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use std::collections::HashMap;
use usd_schemas::geom::ReadTetMesh;

/// Build a `TriangleList` mesh from a `ReadTetMesh`. Uses the cached
/// `surface_face_vertex_indices` when authored, otherwise computes
/// the boundary from `tet_vertex_indices`.
pub fn tetmesh_to_bevy_mesh(read: &ReadTetMesh) -> Mesh {
    let triangles: Vec<u32> = match &read.surface_face_vertex_indices {
        Some(faces) => faces.iter().map(|i| *i as u32).collect(),
        None => extract_boundary(&read.tet_vertex_indices, &read.points),
    };

    // Per-vertex normals via area-weighted accumulation across the
    // triangles each vertex belongs to. Area-weighting mimics
    // `Mesh::compute_smooth_normals` — same look as a regular Bevy mesh.
    let mut normals: Vec<Vec3> = vec![Vec3::ZERO; read.points.len()];
    for tri in triangles.chunks(3) {
        let &[i0, i1, i2] = tri else { continue };
        let p0 = Vec3::from(read.points[i0 as usize]);
        let p1 = Vec3::from(read.points[i1 as usize]);
        let p2 = Vec3::from(read.points[i2 as usize]);
        let n = (p1 - p0).cross(p2 - p0);
        normals[i0 as usize] += n;
        normals[i1 as usize] += n;
        normals[i2 as usize] += n;
    }
    let normals_arr: Vec<[f32; 3]> = normals
        .into_iter()
        .map(|n| n.normalize_or_zero().to_array())
        .collect();

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, read.points.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals_arr);
    // UVs aren't authored on a TetMesh; emit zeros so the StandardMaterial
    // shader's UV-attribute slot stays defined.
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0_f32, 0.0]; read.points.len()],
    );
    mesh.insert_indices(Indices::U32(triangles));
    mesh
}

/// Walk every tet's four triangular faces and emit each face that
/// appears exactly once across the whole mesh — that's the boundary.
/// Interior faces appear twice (once per neighbouring tet) and cancel.
///
/// Each face's winding is **geometrically oriented** so the surface
/// normal points away from the tet's interior — i.e. away from the
/// 4th (opposite) vertex. This makes the extractor robust to whichever
/// vertex order the tet authoring tool emitted; the user's tet need
/// not have a particular sign of `det((b−a),(c−a),(d−a))`. Without
/// this fix, tets authored with mixed-sign volumes produce mixed
/// boundary winding and Bevy backface-culls half the surface, giving
/// a "portal-like" flicker as the camera orbits.
///
/// Returns flat indices: length = 3 × num_boundary_faces.
fn extract_boundary(tet_indices: &[i32], points: &[[f32; 3]]) -> Vec<u32> {
    let mut faces: HashMap<[i32; 3], (i32, [i32; 3])> = HashMap::new();
    for tet in tet_indices.chunks(4) {
        let &[a, b, c, d] = tet else { continue };
        // Each tuple: (face triangle, the 4th tet vertex used as the
        // "interior reference"). The face's normal will be flipped if
        // it points toward that reference instead of away.
        let four = [
            ([a, b, c], d),
            ([a, b, d], c),
            ([a, c, d], b),
            ([b, c, d], a),
        ];
        for (face, opposite) in four {
            let oriented = orient_outward(face, opposite, points);
            let mut sorted = face;
            sorted.sort();
            faces
                .entry(sorted)
                .and_modify(|e| e.0 += 1)
                .or_insert((1, oriented));
        }
    }
    let mut out = Vec::new();
    for (_key, (count, oriented)) in faces {
        if count == 1 {
            out.push(oriented[0] as u32);
            out.push(oriented[1] as u32);
            out.push(oriented[2] as u32);
        }
    }
    out
}

/// Reorder `face` (a, b, c) so its right-hand-rule normal points
/// away from `opposite` (the tet's 4th vertex). Returns the original
/// triple when already correctly oriented; otherwise swaps b/c.
fn orient_outward(face: [i32; 3], opposite: i32, points: &[[f32; 3]]) -> [i32; 3] {
    let a = Vec3::from(points[face[0] as usize]);
    let b = Vec3::from(points[face[1] as usize]);
    let c = Vec3::from(points[face[2] as usize]);
    let d = Vec3::from(points[opposite as usize]);
    let n = (b - a).cross(c - a);
    let to_outside = a - d;
    if n.dot(to_outside) >= 0.0 {
        face
    } else {
        [face[0], face[2], face[1]]
    }
}
