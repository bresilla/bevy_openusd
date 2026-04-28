//! UsdGeom.BasisCurves + UsdGeom.Points → Bevy meshes.
//!
//! - **BasisCurves** → a `TriangleList` tube. Every cubic span is
//!   tessellated into `CURVE_SEGMENTS_PER_SPAN` spine samples, then a
//!   ring of `ring_segments` vertices is extruded around each sample
//!   using a *parallel-transport frame* so the tube doesn't twist on
//!   curves that cross the Y-up singularity. Per-vertex `widths` set
//!   the ring radius; unauthored falls back to
//!   [`UsdLoaderSettings::curve_default_radius`].
//! - **Points** → each USD point bakes to an axis-aligned cube (12
//!   triangles). Correct-per-spec `PrimitiveTopology::PointList`
//!   renders as 1-pixel hardware dots — invisible at any real scene
//!   scale. Billboard sprites are a later milestone.
//!
//! Both use unlit materials so `primvars:displayColor` reads directly.

use bevy::asset::RenderAssetUsages;
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use usd_schemas::geom::{
    CurveBasis, CurveType, CurveWrap, ReadCurves, ReadHermiteCurves, ReadNurbsCurves, ReadPoints,
};

/// How many line segments per cubic span. 16 is enough to hide the
/// piecewise-ness of short curves at viewer scale; CPU cost is trivial.
const CURVE_SEGMENTS_PER_SPAN: u32 = 16;

/// Build a TriangleList *tube mesh* from a `UsdGeom.BasisCurves`. Each
/// curve is tessellated to a spine polyline; around every spine vertex
/// we extrude a ring of `ring_segments` vertices with a
/// *parallel-transport* frame so the tube doesn't twist on curves that
/// cross axis singularities.
///
/// Per-curve-vertex width is pulled from `read.widths`:
/// - `len == 0` → every ring uses `default_radius`
/// - `len == 1` → single width / 2 broadcast to every vertex
/// - `len == total_cvs` → per-CV, linearly interpolated across
///   tessellation subsegments
pub fn curves_mesh(read: &ReadCurves, default_radius: f32, ring_segments: u32) -> Mesh {
    let rings = ring_segments.max(3);
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let base_color = read
        .display_color
        .as_ref()
        .and_then(|c| c.first())
        .copied()
        .unwrap_or([0.85, 0.85, 0.9]);
    let default_rgba = [base_color[0], base_color[1], base_color[2], 1.0];

    let mut cv_cursor = 0usize;
    for (curve_ix, count) in read.vertex_counts.iter().enumerate() {
        let count = (*count as usize).max(0);
        if count == 0 {
            continue;
        }
        let cvs = &read.points[cv_cursor..cv_cursor + count];
        let cv_widths: Option<Vec<f32>> = match read.widths.len() {
            0 => None,
            1 => Some(vec![read.widths[0]; count]),
            n if n >= cv_cursor + count => Some(read.widths[cv_cursor..cv_cursor + count].to_vec()),
            _ => None,
        };
        cv_cursor += count;

        let spine: Vec<[f32; 3]> = match (read.curve_type, read.basis) {
            (CurveType::Linear, _) => cvs.to_vec(),
            (CurveType::Cubic, CurveBasis::Bezier) => tess_bezier(cvs, read.wrap),
            (CurveType::Cubic, CurveBasis::Bspline) => tess_bspline(cvs, read.wrap),
            (CurveType::Cubic, CurveBasis::CatmullRom) => tess_catmull_rom(cvs, read.wrap),
            (CurveType::Cubic, CurveBasis::Hermite) => tess_hermite(cvs, read.wrap),
        };
        if spine.len() < 2 {
            continue;
        }

        // Per-spine-vertex radius. If per-CV widths were authored we
        // linearly interpolate across the tessellated spine.
        let radii: Vec<f32> = match cv_widths {
            None => vec![default_radius; spine.len()],
            Some(ws) => {
                let mut out = Vec::with_capacity(spine.len());
                let last_cv = (ws.len() - 1) as f32;
                for i in 0..spine.len() {
                    let t = (i as f32 / (spine.len() - 1) as f32) * last_cv;
                    let lo = t.floor() as usize;
                    let hi = (lo + 1).min(ws.len() - 1);
                    let f = t - lo as f32;
                    let w = ws[lo] * (1.0 - f) + ws[hi] * f;
                    out.push((w * 0.5).max(0.001));
                }
                out
            }
        };

        let curve_rgba = read
            .display_color
            .as_ref()
            .filter(|c| c.len() == read.vertex_counts.len())
            .and_then(|c| c.get(curve_ix))
            .map(|c| [c[0], c[1], c[2], 1.0])
            .unwrap_or(default_rgba);

        build_tube(
            &spine,
            &radii,
            rings,
            curve_rgba,
            &mut positions,
            &mut normals,
            &mut colors,
            &mut indices,
        );
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Sweep a ring of `rings` vertices around each spine point using a
/// parallel-transported Frenet frame. Each segment between spine i and
/// i+1 emits `rings × 2` triangles. Endpoint caps aren't drawn — curves
/// are typically viewed from outside so the open ends are invisible at
/// viewer scale, and adding caps doubles the vertex count.
fn build_tube(
    spine: &[[f32; 3]],
    radii: &[f32],
    rings: u32,
    rgba: [f32; 4],
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
) {
    if spine.len() < 2 {
        return;
    }
    // Build a running (normal, binormal) frame. At each step we rotate
    // the previous frame's normal into the plane orthogonal to the new
    // tangent — classic parallel transport.
    let first_tangent = normalize(sub(spine[1], spine[0]));
    let mut prev_normal = perpendicular(first_tangent);
    let mut prev_binormal;

    let start_index = positions.len() as u32;

    for i in 0..spine.len() {
        let tangent = if i == 0 {
            first_tangent
        } else if i == spine.len() - 1 {
            normalize(sub(spine[i], spine[i - 1]))
        } else {
            normalize(sub(spine[i + 1], spine[i - 1]))
        };
        // Rotate prev_normal into the new tangent's perpendicular plane.
        prev_normal = normalize(sub(prev_normal, scale(tangent, dot(prev_normal, tangent))));
        if len_sq(prev_normal) < 1e-6 {
            prev_normal = perpendicular(tangent);
        }
        prev_binormal = normalize(cross(tangent, prev_normal));

        let r = radii[i];
        for s in 0..rings {
            let theta = (s as f32 / rings as f32) * core::f32::consts::TAU;
            let (sin_t, cos_t) = theta.sin_cos();
            let normal = add(scale(prev_normal, cos_t), scale(prev_binormal, sin_t));
            let p = add(spine[i], scale(normal, r));
            positions.push(p);
            normals.push(normal);
            colors.push(rgba);
        }
    }

    let r = rings;
    for i in 0..(spine.len() as u32 - 1) {
        for s in 0..r {
            let s_next = (s + 1) % r;
            let a = start_index + i * r + s;
            let b = start_index + i * r + s_next;
            let c = start_index + (i + 1) * r + s_next;
            let d = start_index + (i + 1) * r + s;
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }
}

// ── Vec3 helpers ─────────────────────────────────────────────────────────
// Inlined vec ops keep the tube builder dep-free of `glam::Vec3`; Bevy's
// Vec3 is glam under the hood but using plain arrays makes the
// tessellators above match.

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len_sq(a: V3) -> f32 {
    dot(a, a)
}
fn normalize(a: V3) -> V3 {
    let l = len_sq(a).sqrt();
    if l < 1e-8 {
        [0.0, 0.0, 1.0]
    } else {
        scale(a, 1.0 / l)
    }
}
fn perpendicular(t: V3) -> V3 {
    // Any vector not parallel to t. Pick world Y unless the tangent is
    // basically aligned with it, in which case fall back to world X.
    let up = if t[1].abs() < 0.9 {
        [0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    normalize(sub(up, scale(t, dot(up, t))))
}

/// Build a visible point-cloud mesh. Each USD point becomes a small
/// axis-aligned cube (8 verts / 12 tris) at the authored position,
/// sized from the point's `widths` entry.
///
/// We used `PrimitiveTopology::PointList` originally — correct per the
/// USD spec but renders as 1-pixel hardware dots that are invisible at
/// any real scene scale. Baking into triangles costs ~12 tris per point
/// which is still fine for tens of thousands of points and actually
/// shows up in the viewport. A proper billboard shader is an M11.1
/// task.
pub fn points_mesh(read: &ReadPoints, scale: f32) -> Mesh {
    let n = read.points.len();
    let default_rgba = read
        .display_color
        .as_ref()
        .and_then(|c| c.first())
        .map(|c| [c[0], c[1], c[2], 1.0])
        .unwrap_or([0.9, 0.9, 1.0, 1.0]);

    // Size fallback: if neither `widths` nor a scene hint is available,
    // pick something scene-relative-ish. Consumers who need exact
    // authored widths will have them in `read.widths`.
    let default_half = {
        let w = read.widths.first().copied().unwrap_or(0.05);
        (w * 0.5 * scale).max(0.001)
    };

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n * 8);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(n * 8);
    let mut colors: Vec<[f32; 4]> = Vec::with_capacity(n * 8);
    let mut indices: Vec<u32> = Vec::with_capacity(n * 36);

    for (i, p) in read.points.iter().enumerate() {
        let raw_half = match read.widths.len() {
            0 => default_half,
            1 => (read.widths[0] * 0.5 * scale).max(0.001),
            _ => {
                (read.widths.get(i).copied().unwrap_or(default_half * 2.0) * 0.5 * scale).max(0.001)
            }
        };
        let half = raw_half;
        let rgba = match read.display_color.as_ref() {
            Some(dc) if dc.len() == n => [dc[i][0], dc[i][1], dc[i][2], 1.0],
            _ => default_rgba,
        };
        let base = positions.len() as u32;

        // Eight cube corners — normals point radially outward (unlit
        // material doesn't use them, but they make the mesh reusable if
        // someone later swaps in a lit material).
        let centre = Vec3::new(p[0], p[1], p[2]);
        let corners = [
            Vec3::new(-half, -half, -half),
            Vec3::new(half, -half, -half),
            Vec3::new(half, half, -half),
            Vec3::new(-half, half, -half),
            Vec3::new(-half, -half, half),
            Vec3::new(half, -half, half),
            Vec3::new(half, half, half),
            Vec3::new(-half, half, half),
        ];
        for c in corners {
            let pos = centre + c;
            positions.push([pos.x, pos.y, pos.z]);
            normals.push([c.x.signum(), c.y.signum(), c.z.signum()]);
            colors.push(rgba);
        }

        // 12 triangles per cube.
        let b = base;
        for &(a, b_, c) in &[
            (0, 2, 1),
            (0, 3, 2), // -Z
            (4, 5, 6),
            (4, 6, 7), // +Z
            (0, 1, 5),
            (0, 5, 4), // -Y
            (3, 6, 2),
            (3, 7, 6), // +Y
            (0, 4, 7),
            (0, 7, 3), // -X
            (1, 2, 6),
            (1, 6, 5), // +X
        ] {
            indices.push(b + a);
            indices.push(b + b_);
            indices.push(b + c);
        }
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// ── Cubic tessellation helpers ───────────────────────────────────────────
//
// USD's basis curves author *control vertices*; the parametric curve
// evaluates within consecutive spans. Span count depends on basis +
// wrap. We evaluate CURVE_SEGMENTS_PER_SPAN points per span and emit a
// polyline.

fn tess_bezier(cvs: &[[f32; 3]], _wrap: CurveWrap) -> Vec<[f32; 3]> {
    // Bezier spans = (N - 1) / 3 segments of 4 CVs each (cubic).
    if cvs.len() < 4 {
        return cvs.to_vec();
    }
    let mut out = Vec::new();
    out.push(cvs[0]);
    for span in (0..cvs.len() - 3).step_by(3) {
        let p0 = v(cvs[span]);
        let p1 = v(cvs[span + 1]);
        let p2 = v(cvs[span + 2]);
        let p3 = v(cvs[span + 3]);
        for i in 1..=CURVE_SEGMENTS_PER_SPAN {
            let t = i as f32 / CURVE_SEGMENTS_PER_SPAN as f32;
            let omt = 1.0 - t;
            let b = p0 * (omt * omt * omt)
                + p1 * (3.0 * omt * omt * t)
                + p2 * (3.0 * omt * t * t)
                + p3 * (t * t * t);
            out.push([b.x, b.y, b.z]);
        }
    }
    out
}

fn tess_bspline(cvs: &[[f32; 3]], wrap: CurveWrap) -> Vec<[f32; 3]> {
    // Uniform cubic B-spline. Every group of 4 consecutive CVs defines
    // one span. Periodic wrapping extends by 3 CVs.
    let extended: Vec<[f32; 3]> = if wrap == CurveWrap::Periodic && cvs.len() >= 3 {
        let mut v = cvs.to_vec();
        v.extend_from_slice(&cvs[..3]);
        v
    } else {
        cvs.to_vec()
    };
    if extended.len() < 4 {
        return extended;
    }
    let mut out = Vec::new();
    for span in 0..=extended.len() - 4 {
        let p0 = v(extended[span]);
        let p1 = v(extended[span + 1]);
        let p2 = v(extended[span + 2]);
        let p3 = v(extended[span + 3]);
        let steps = if span == 0 { 0 } else { 1 };
        for i in steps..=CURVE_SEGMENTS_PER_SPAN {
            let t = i as f32 / CURVE_SEGMENTS_PER_SPAN as f32;
            let one_sixth = 1.0 / 6.0;
            let b0 = one_sixth * (1.0 - t).powi(3);
            let b1 = one_sixth * (3.0 * t.powi(3) - 6.0 * t.powi(2) + 4.0);
            let b2 = one_sixth * (-3.0 * t.powi(3) + 3.0 * t.powi(2) + 3.0 * t + 1.0);
            let b3 = one_sixth * t.powi(3);
            let b = p0 * b0 + p1 * b1 + p2 * b2 + p3 * b3;
            out.push([b.x, b.y, b.z]);
        }
    }
    out
}

fn tess_catmull_rom(cvs: &[[f32; 3]], wrap: CurveWrap) -> Vec<[f32; 3]> {
    // Catmull-Rom through N CVs: N-3 interior spans (each span uses 4 CVs
    // and interpolates between CV1 and CV2). Periodic wraps ends.
    let extended: Vec<[f32; 3]> = if wrap == CurveWrap::Periodic && cvs.len() >= 3 {
        let mut v = Vec::with_capacity(cvs.len() + 3);
        v.push(cvs[cvs.len() - 1]);
        v.extend_from_slice(cvs);
        v.extend_from_slice(&cvs[..2]);
        v
    } else if cvs.len() >= 2 {
        let mut v = Vec::with_capacity(cvs.len() + 2);
        v.push(cvs[0]);
        v.extend_from_slice(cvs);
        v.push(cvs[cvs.len() - 1]);
        v
    } else {
        cvs.to_vec()
    };
    if extended.len() < 4 {
        return extended;
    }
    let mut out = Vec::new();
    out.push(extended[1]);
    for span in 0..=extended.len() - 4 {
        let p0 = v(extended[span]);
        let p1 = v(extended[span + 1]);
        let p2 = v(extended[span + 2]);
        let p3 = v(extended[span + 3]);
        for i in 1..=CURVE_SEGMENTS_PER_SPAN {
            let t = i as f32 / CURVE_SEGMENTS_PER_SPAN as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let b = p0 * (-0.5 * t3 + t2 - 0.5 * t)
                + p1 * (1.5 * t3 - 2.5 * t2 + 1.0)
                + p2 * (-1.5 * t3 + 2.0 * t2 + 0.5 * t)
                + p3 * (0.5 * t3 - 0.5 * t2);
            out.push([b.x, b.y, b.z]);
        }
    }
    out
}

fn tess_hermite(cvs: &[[f32; 3]], _wrap: CurveWrap) -> Vec<[f32; 3]> {
    // Hermite is authored as (position, tangent) pairs — span i uses
    // (p_{2i}, t_{2i+1}, p_{2i+2}, t_{2i+3}).
    if cvs.len() < 4 || cvs.len() % 2 != 0 {
        return cvs.to_vec();
    }
    let mut out = Vec::new();
    out.push(cvs[0]);
    for span in (0..cvs.len() - 2).step_by(2) {
        let p0 = v(cvs[span]);
        let m0 = v(cvs[span + 1]);
        let p1 = v(cvs[span + 2]);
        let m1 = v(cvs[span + 3]);
        for i in 1..=CURVE_SEGMENTS_PER_SPAN {
            let t = i as f32 / CURVE_SEGMENTS_PER_SPAN as f32;
            let t2 = t * t;
            let t3 = t2 * t;
            let b = p0 * (2.0 * t3 - 3.0 * t2 + 1.0)
                + m0 * (t3 - 2.0 * t2 + t)
                + p1 * (-2.0 * t3 + 3.0 * t2)
                + m1 * (t3 - t2);
            out.push([b.x, b.y, b.z]);
        }
    }
    out
}

#[inline]
fn v(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

// ─── NURBS curves ───────────────────────────────────────────────────

/// Default sample count per NURBS curve when converting to a polyline.
/// 32 is a reasonable trade-off between smoothness and CPU cost for
/// typical scene-scale curves.
const NURBS_SAMPLES_PER_CURVE: u32 = 32;

/// Convert `ReadNurbsCurves` into a `ReadCurves` whose curves are
/// linear polylines sampled from the underlying NURBS via De Boor's
/// algorithm. The downstream `curves_mesh` builds tubes from those
/// polylines exactly the same way it does for authored linear
/// curves — so we get the parallel-transport tube + per-vertex
/// width pipeline for free.
///
/// Per-CV widths are interpolated to the sampled vertex count;
/// per-curve / single widths are forwarded through. Display-color
/// is forwarded unchanged (the consumer's slot-arity heuristic
/// still works).
pub fn nurbs_to_read_curves(nurbs: &ReadNurbsCurves) -> ReadCurves {
    let mut out_points: Vec<[f32; 3]> = Vec::new();
    let mut out_counts: Vec<i32> = Vec::with_capacity(nurbs.curve_vertex_counts.len());
    let mut out_widths: Vec<f32> = Vec::new();
    let widths_per_cv = nurbs.widths.len() == nurbs.points.len();

    let mut cv_cursor = 0usize;
    let mut k_cursor = 0usize;
    for (i, count) in nurbs.curve_vertex_counts.iter().enumerate() {
        let n = (*count as usize).max(0);
        let p_order = nurbs.order.get(i).copied().unwrap_or(4) as usize;
        let nk = n + p_order;

        if n == 0 || p_order == 0 || k_cursor + nk > nurbs.knots.len() {
            cv_cursor += n;
            k_cursor += nk;
            out_counts.push(0);
            continue;
        }

        let cps = &nurbs.points[cv_cursor..cv_cursor + n];
        let knots = &nurbs.knots[k_cursor..k_cursor + nk];
        let (umin, umax) = nurbs
            .ranges
            .get(i)
            .copied()
            .map(|r| (r[0], r[1]))
            .unwrap_or((knots[p_order - 1], knots[n]));

        let samples = NURBS_SAMPLES_PER_CURVE;
        let mut sampled: Vec<[f32; 3]> = Vec::with_capacity(samples as usize);
        for s in 0..samples {
            // Clamp the last sample slightly below umax so the
            // knot-span search still finds a span; mathematically
            // the curve at u=umax equals the last CV when end-clamped.
            let t = if samples > 1 {
                s as f64 / (samples - 1) as f64
            } else {
                0.0
            };
            let u = umin + (umax - umin) * t;
            let u = if t >= 1.0 {
                umax - (umax - umin) * 1e-6
            } else {
                u
            };
            sampled.push(eval_nurbs_de_boor(cps, knots, p_order - 1, u));
        }

        // Forward widths.
        if widths_per_cv {
            // Interpolate per-CV widths across the sample count.
            let last_cv = (n - 1) as f32;
            for s in 0..samples {
                let t = if samples > 1 {
                    s as f32 / (samples - 1) as f32
                } else {
                    0.0
                };
                let f = t * last_cv;
                let lo = f.floor() as usize;
                let hi = (lo + 1).min(n - 1);
                let frac = f - lo as f32;
                let w = nurbs.widths[cv_cursor + lo] * (1.0 - frac)
                    + nurbs.widths[cv_cursor + hi] * frac;
                out_widths.push(w);
            }
        }

        out_points.extend_from_slice(&sampled);
        out_counts.push(samples as i32);

        cv_cursor += n;
        k_cursor += nk;
    }

    // Constant width or per-curve width: forward as-is. The downstream
    // builder broadcasts a single width; for per-curve widths, expand
    // here so the per-sample buffer above stays consistent.
    if !widths_per_cv {
        out_widths = nurbs.widths.clone();
    }

    ReadCurves {
        points: out_points,
        vertex_counts: out_counts,
        curve_type: CurveType::Linear,
        basis: CurveBasis::Bezier,
        wrap: CurveWrap::Nonperiodic,
        widths: out_widths,
        display_color: nurbs.display_color.clone(),
    }
}

/// Evaluate a non-rational B-spline at parameter `u` using De Boor's
/// algorithm. `cps` are the curve's control points, `knots` is its
/// knot vector (length `cps.len() + degree + 1`), and `degree` is
/// the polynomial degree (= order − 1).
///
/// Caller guarantees `cps.len() >= degree + 1` and that
/// `knots.len() == cps.len() + degree + 1`. `pub` so the NURBS-patch
/// builder in `nurbs_patch.rs` can drive a tensor-product evaluator
/// off the same primitive.
pub fn eval_nurbs_de_boor(cps: &[[f32; 3]], knots: &[f64], degree: usize, u: f64) -> [f32; 3] {
    let n = cps.len() - 1;
    let p = degree;

    // Find knot span k such that `knots[k] <= u < knots[k+1]`.
    // For the special case `u >= knots[n+1]` (end of curve on a
    // clamped knot vector) we clamp k to `n`.
    let k = if u >= knots[n + 1] {
        n
    } else {
        let mut k = p;
        while k <= n && knots[k + 1] <= u {
            k += 1;
        }
        k.min(n)
    };

    // De Boor's recursion: working set d[0..=p] starts at
    // `cps[k - p ..= k]`, then is iteratively refined.
    let mut d: Vec<[f32; 3]> = (0..=p).map(|j| cps[k - p + j]).collect();
    for r in 1..=p {
        for j in (r..=p).rev() {
            let i = k - p + j;
            let denom = knots[i + p - r + 1] - knots[i];
            let alpha = if denom.abs() < 1e-12 {
                0.0
            } else {
                ((u - knots[i]) / denom) as f32
            };
            d[j] = [
                d[j - 1][0] * (1.0 - alpha) + d[j][0] * alpha,
                d[j - 1][1] * (1.0 - alpha) + d[j][1] * alpha,
                d[j - 1][2] * (1.0 - alpha) + d[j][2] * alpha,
            ];
        }
    }
    d[p]
}

// ─── Hermite curves ─────────────────────────────────────────────────

/// How many segments to subdivide each Hermite span into. 16 matches
/// `CURVE_SEGMENTS_PER_SPAN` so visually a Hermite curve and a cubic
/// BasisCurves at the same scale read the same.
const HERMITE_SEGMENTS_PER_SPAN: u32 = 16;

/// Convert `ReadHermiteCurves` into a sampled `ReadCurves` (linear
/// polyline) so the existing tube builder can render it. The cubic
/// Hermite basis interpolates between two CVs using their authored
/// tangents.
pub fn hermite_to_read_curves(read: &ReadHermiteCurves) -> ReadCurves {
    let mut out_points: Vec<[f32; 3]> = Vec::new();
    let mut out_counts: Vec<i32> = Vec::with_capacity(read.curve_vertex_counts.len());
    let mut out_widths: Vec<f32> = Vec::new();
    let widths_per_cv = read.widths.len() == read.points.len();

    let mut cv_cursor = 0usize;
    for count in &read.curve_vertex_counts {
        let n = (*count as usize).max(0);
        if n < 2 {
            cv_cursor += n;
            out_counts.push(0);
            continue;
        }
        let pts = &read.points[cv_cursor..cv_cursor + n];
        let tans = &read.tangents[cv_cursor..cv_cursor + n];
        let cv_widths: Option<&[f32]> = if widths_per_cv {
            Some(&read.widths[cv_cursor..cv_cursor + n])
        } else {
            None
        };

        let segs = HERMITE_SEGMENTS_PER_SPAN;
        let total_samples = (n - 1) * segs as usize + 1;
        let mut sampled: Vec<[f32; 3]> = Vec::with_capacity(total_samples);
        for i in 0..n - 1 {
            let p0 = pts[i];
            let p1 = pts[i + 1];
            let m0 = tans[i];
            let m1 = tans[i + 1];
            let inclusive_end = i == n - 2;
            let span_samples = if inclusive_end {
                segs + 1
            } else {
                segs
            };
            for s in 0..span_samples {
                let t = s as f32 / segs as f32;
                let h00 = 2.0 * t * t * t - 3.0 * t * t + 1.0;
                let h10 = t * t * t - 2.0 * t * t + t;
                let h01 = -2.0 * t * t * t + 3.0 * t * t;
                let h11 = t * t * t - t * t;
                let x = h00 * p0[0] + h10 * m0[0] + h01 * p1[0] + h11 * m1[0];
                let y = h00 * p0[1] + h10 * m0[1] + h01 * p1[1] + h11 * m1[1];
                let z = h00 * p0[2] + h10 * m0[2] + h01 * p1[2] + h11 * m1[2];
                sampled.push([x, y, z]);
            }
        }

        if let Some(ws) = cv_widths {
            // Per-CV widths interpolate linearly across the sample
            // count. Mirrors the same-named codepath in the BasisCurves
            // tube builder so both authoring styles read consistently.
            let last_cv = (n - 1) as f32;
            for s in 0..sampled.len() {
                let f = s as f32 / (sampled.len() - 1) as f32 * last_cv;
                let lo = f.floor() as usize;
                let hi = (lo + 1).min(n - 1);
                let frac = f - lo as f32;
                let w = ws[lo] * (1.0 - frac) + ws[hi] * frac;
                out_widths.push(w);
            }
        }

        out_counts.push(sampled.len() as i32);
        out_points.extend_from_slice(&sampled);
        cv_cursor += n;
    }

    if !widths_per_cv {
        out_widths = read.widths.clone();
    }

    ReadCurves {
        points: out_points,
        vertex_counts: out_counts,
        curve_type: CurveType::Linear,
        basis: CurveBasis::Bezier,
        wrap: CurveWrap::Nonperiodic,
        widths: out_widths,
        display_color: read.display_color.clone(),
    }
}
