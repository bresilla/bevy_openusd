//! `UsdGeomNurbsPatch` → Bevy `Mesh` via tensor-product De Boor.
//!
//! A NURBS patch is a 2D parametric surface whose control net is a
//! `u_count × v_count` grid of CVs. We sample the surface on a regular
//! grid of `(u, v)` parameter values, triangulate the sample grid into
//! quads (two triangles each), and emit a standard `TriangleList` mesh.
//!
//! Evaluation:
//!
//! 1. For each U-row `i ∈ [0, u_count)`, take the row's V-direction
//!    CVs `points[i*v_count .. i*v_count + v_count]` and evaluate
//!    De Boor at parameter `v` to get a row-collapsed point.
//! 2. The `u_count` row-collapsed points form an effective U-direction
//!    curve at parameter `v`. Evaluate De Boor at parameter `u` to get
//!    the surface point at `(u, v)`.
//!
//! This is a non-rational (weights = 1) sampler. Real-world rational
//! NURBS surfaces with non-uniform weights are uncommon enough that
//! we accept the approximation today; the door's open for a weighted
//! variant later.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use usd_schemas::geom::ReadNurbsPatch;

use crate::curves::eval_nurbs_de_boor;

/// Sample resolution in each parametric direction. 32×32 → 1024
/// vertices, ~1922 triangles. Plenty smooth for typical patches at
/// scene scale; a future setting can scale this if greenhouse-style
/// dense surfaces want to dial it down.
const PATCH_SAMPLES: u32 = 32;

pub fn nurbs_patch_to_bevy_mesh(read: &ReadNurbsPatch) -> Mesh {
    let nu = read.u_vertex_count.max(0) as usize;
    let nv = read.v_vertex_count.max(0) as usize;
    let u_order = read.u_order.max(1) as usize;
    let v_order = read.v_order.max(1) as usize;
    let u_degree = u_order.saturating_sub(1);
    let v_degree = v_order.saturating_sub(1);
    let expected_knots_u = nu + u_order;
    let expected_knots_v = nv + v_order;

    if nu < u_order
        || nv < v_order
        || nu * nv != read.points.len()
        || read.u_knots.len() != expected_knots_u
        || read.v_knots.len() != expected_knots_v
    {
        return empty_mesh();
    }

    let nsamp = PATCH_SAMPLES;
    let [umin, umax] = read.u_range;
    let [vmin, vmax] = read.v_range;

    // Sample on a regular (u, v) grid. Index ordering: row-major in V,
    // i.e. `positions[sv * nsamp + su]`.
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity((nsamp * nsamp) as usize);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity((nsamp * nsamp) as usize);
    for sv in 0..nsamp {
        let tv = if nsamp > 1 { sv as f64 / (nsamp - 1) as f64 } else { 0.0 };
        let v_raw = vmin + (vmax - vmin) * tv;
        // Same trick as the curve sampler: nudge the last sample
        // slightly below the upper bound so the knot-span search still
        // resolves cleanly.
        let v = if tv >= 1.0 {
            vmax - (vmax - vmin).abs() * 1e-6
        } else {
            v_raw
        };
        for su in 0..nsamp {
            let tu = if nsamp > 1 { su as f64 / (nsamp - 1) as f64 } else { 0.0 };
            let u_raw = umin + (umax - umin) * tu;
            let u = if tu >= 1.0 {
                umax - (umax - umin).abs() * 1e-6
            } else {
                u_raw
            };
            let p = eval_patch(
                &read.points,
                nu,
                nv,
                &read.u_knots,
                &read.v_knots,
                u_degree,
                v_degree,
                u,
                v,
            );
            positions.push(p);
            uvs.push([tu as f32, tv as f32]);
        }
    }

    // Triangulate the sample grid: 2 triangles per cell, CCW from
    // outside (i.e. when looking down +V the patch's "front" face
    // shows). We compute geometric normals next; if the patch was
    // authored "inside-out" the area-weighted accumulator just flips
    // every contribution and the surface still shades smoothly.
    let mut indices: Vec<u32> = Vec::with_capacity(((nsamp - 1) * (nsamp - 1) * 6) as usize);
    for sv in 0..nsamp - 1 {
        for su in 0..nsamp - 1 {
            let i00 = sv * nsamp + su;
            let i01 = sv * nsamp + (su + 1);
            let i10 = (sv + 1) * nsamp + su;
            let i11 = (sv + 1) * nsamp + (su + 1);
            indices.extend_from_slice(&[i00, i10, i11, i00, i11, i01]);
        }
    }

    // Per-vertex normals via area-weighted accumulation.
    let mut normals: Vec<Vec3> = vec![Vec3::ZERO; positions.len()];
    for tri in indices.chunks(3) {
        let &[i0, i1, i2] = tri else { continue };
        let p0 = Vec3::from(positions[i0 as usize]);
        let p1 = Vec3::from(positions[i1 as usize]);
        let p2 = Vec3::from(positions[i2 as usize]);
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
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals_arr);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Tensor-product NURBS evaluation: collapse each U-row by evaluating
/// its V-direction curve at `v`, then evaluate the resulting
/// `nu`-CV U-direction curve at `u`. Equivalent to the more familiar
/// `S(u, v) = Σ N_i,p(u) · N_j,q(v) · P_i,j` formulation but uses
/// `nu` 1D evaluations instead of `nu × nv` 2D products.
fn eval_patch(
    cps: &[[f32; 3]],
    nu: usize,
    nv: usize,
    u_knots: &[f64],
    v_knots: &[f64],
    u_degree: usize,
    v_degree: usize,
    u: f64,
    v: f64,
) -> [f32; 3] {
    let mut intermediate: Vec<[f32; 3]> = Vec::with_capacity(nu);
    for i in 0..nu {
        let row_start = i * nv;
        let row = &cps[row_start..row_start + nv];
        intermediate.push(eval_nurbs_de_boor(row, v_knots, v_degree, v));
    }
    eval_nurbs_de_boor(&intermediate, u_knots, u_degree, u)
}

fn empty_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new());
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, Vec::<[f32; 3]>::new());
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new());
    m.insert_indices(Indices::U32(Vec::new()));
    m
}
