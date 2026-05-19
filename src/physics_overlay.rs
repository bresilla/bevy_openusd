//! Visual debug overlay for the projection's physics marker components.
//!
//! Renders (when `DisplayToggles.show_physics` is on, hotkey `Y`):
//!
//! - **PhysicsScene**: gravity arrow at the scene-root origin, scaled
//!   to the scene extent.
//! - **UsdRigidBody / UsdMass**: small sphere at each body's
//!   center-of-mass (world if explicit `mass`, gray if density-only).
//! - **UsdPhysicsJoint**: per-anchor body frame triads, joint axis
//!   arrow, body0↔body1 connection line **coloured by joint kind**,
//!   plus a per-kind limit shape:
//!     - Revolute → arc spanning lower→upper around the axis
//!     - Prismatic → line segment along the axis from low to high
//!     - Spherical → wireframe cone with cone-angle aperture
//!     - Distance → two concentric spheres (min / max)
//!     - Generic → small per-DOF segments / arcs from `joint.limits`
//! - **UsdArticulationRoot**: a wireframe sphere at the root entity
//!   in the chain colour, plus a thicker chain-coloured connection
//!   line over every joint in the articulation.
//!
//! Independent from any physics engine — pure ECS-data visualisation.
//! Joint shape primitives live in `bevy_glacial::joint_gizmos`.

use bevy::color::palettes::tailwind;
use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bevy_glacial::joint_gizmos::{
    draw_cone_wireframe, draw_distance_envelope, draw_prismatic_limit_segment,
    draw_revolute_limit_arc,
};
use usd_bevy::{
    UsdArticulationRoot, UsdDof, UsdJointKind, UsdMass, UsdPhysicsJoint, UsdPhysicsScene,
    UsdRigidBody,
};

use crate::overlays::{DisplayToggles, SceneExtent};

// Tighter axis-triad palette for joint frames — cool R/G/B that
// reads against typical scene colours without dominating.
const FRAME_COLORS: [Color; 3] = [
    Color::srgb(1.00, 0.35, 0.35),
    Color::srgb(0.35, 1.00, 0.35),
    Color::srgb(0.35, 0.55, 1.00),
];

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct PhysicsGizmos;

pub struct PhysicsOverlayPlugin;

impl Plugin for PhysicsOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_gizmo_group::<PhysicsGizmos>()
            .add_systems(Startup, setup_physics_gizmos_on_top)
            .add_systems(
                Update,
                (
                    draw_joints,
                    draw_articulation_chains,
                    draw_articulation_root_markers,
                    draw_mass_markers,
                    draw_scene_gravity,
                ),
            );
    }
}

/// Render physics gizmos in front of geometry so anchors aren't
/// hidden inside body meshes.
fn setup_physics_gizmos_on_top(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<PhysicsGizmos>();
    cfg.depth_bias = -1.0;
}

// ── Joints ──────────────────────────────────────────────────────────────

/// Per-joint gizmos: anchor frame triads, axis arrow, kind-coloured
/// connection line, kind-specific limit shape, drive targets.
fn draw_joints(
    mut gizmos: Gizmos<PhysicsGizmos>,
    toggles: Res<DisplayToggles>,
    extent: Res<SceneExtent>,
    joints: Query<&UsdPhysicsJoint>,
    transforms: Query<&GlobalTransform>,
) {
    if !toggles.show_physics {
        return;
    }
    let triad = triad_size(&extent);
    let axis_len = triad * 2.0;
    let limit_radius = triad * 4.0;

    for joint in &joints {
        let a0 = joint
            .body0
            .and_then(|e| transforms.get(e).ok())
            .map(|gt| anchor_world(gt, joint.local_pos0, joint.local_rot0));
        let a1 = joint
            .body1
            .and_then(|e| transforms.get(e).ok())
            .map(|gt| anchor_world(gt, joint.local_pos1, joint.local_rot1));

        // Body frame triads — small R/G/B at each anchor.
        if let Some((p0, r0)) = a0 {
            draw_triad(&mut gizmos, p0, r0, triad);
        }
        if let Some((p1, r1)) = a1 {
            draw_triad(&mut gizmos, p1, r1, triad);
        }

        // Joint axis arrow at anchor 0 (world-space rotation of the
        // authored axis through anchor 0's frame).
        let axis_world_at_0 = a0.map(|(_, r0)| (r0 * joint.axis).normalize_or_zero());
        if let (Some((p0, _)), Some(axw)) = (a0, axis_world_at_0)
            && axw.length_squared() > 1e-6
        {
            gizmos.arrow(p0, p0 + axw * axis_len, Color::from(tailwind::FUCHSIA_400));
        }

        // Connection line in the joint's KIND colour.
        let kind_col = joint_kind_color(joint.kind);
        if let (Some((p0, _)), Some((p1, _))) = (a0, a1) {
            gizmos.line(p0, p1, kind_col.with_alpha(0.7));
        }

        // Kind-specific limit visualisation.
        if let (Some((p0, _)), Some(axw)) = (a0, axis_world_at_0) {
            draw_kind_limits(
                &mut gizmos,
                joint,
                p0,
                axw,
                a1.map(|(p, _)| p),
                limit_radius,
            );
        }
    }
}

fn draw_kind_limits(
    gizmos: &mut Gizmos<PhysicsGizmos>,
    joint: &UsdPhysicsJoint,
    anchor0: Vec3,
    axis_world: Vec3,
    anchor1: Option<Vec3>,
    radius: f32,
) {
    let kind_col = joint_kind_color(joint.kind);

    match joint.kind {
        UsdJointKind::Revolute => {
            if let Some((lo, hi)) = joint.built_in_limit {
                draw_revolute_limit_arc(
                    gizmos,
                    anchor0,
                    axis_world,
                    lo,
                    hi,
                    radius,
                    kind_col.with_alpha(0.8),
                    Some(48),
                );
            }
        }
        UsdJointKind::Prismatic => {
            if let Some((lo, hi)) = joint.built_in_limit {
                draw_prismatic_limit_segment(
                    gizmos,
                    anchor0,
                    axis_world,
                    lo,
                    hi,
                    kind_col.with_alpha(0.85),
                );
            }
        }
        UsdJointKind::Spherical => {
            if let Some((c0, c1)) = joint.cone_limit {
                // USD's spherical cone has two angles (swing X / swing Y).
                // Use the larger one for the cone visualisation.
                let half = c0.max(c1).max(0.0);
                if half > 0.0 {
                    draw_cone_wireframe(
                        gizmos,
                        anchor0,
                        axis_world,
                        half,
                        radius,
                        16,
                        kind_col.with_alpha(0.7),
                    );
                }
            }
        }
        UsdJointKind::Distance => {
            if let Some((min, max)) = joint.distance_limit {
                let centre = match anchor1 {
                    Some(p1) => (anchor0 + p1) * 0.5,
                    None => anchor0,
                };
                draw_distance_envelope(
                    gizmos,
                    centre,
                    min.max(0.0),
                    max.max(0.0),
                    kind_col.with_alpha(0.6),
                    kind_col.with_alpha(0.9),
                );
            }
        }
        UsdJointKind::Generic => {
            // Walk the multi-apply LimitAPI list and draw per-DOF
            // shapes. Linear DOFs → small segments along that axis;
            // rotational → small arcs.
            for lim in &joint.limits {
                let (axis_unit, rotational) = dof_world_axis(axis_world, lim.dof);
                if rotational {
                    draw_revolute_limit_arc(
                        gizmos,
                        anchor0,
                        axis_unit,
                        lim.low,
                        lim.high,
                        radius * 0.7,
                        kind_col.with_alpha(0.6),
                        Some(24),
                    );
                } else {
                    draw_prismatic_limit_segment(
                        gizmos,
                        anchor0,
                        axis_unit,
                        lim.low,
                        lim.high,
                        kind_col.with_alpha(0.6),
                    );
                }
            }
        }
        UsdJointKind::Fixed => {
            // No limits — fixed joint locks all DOFs by definition.
        }
    }

    // Drive target indicators: mark the target_position of the first
    // rotational drive on the joint with a tick mark on the arc.
    for d in &joint.drives {
        let (axis_unit, rotational) = dof_world_axis(axis_world, d.dof);
        let Some(target) = d.target_position else {
            continue;
        };
        if rotational {
            // Tick at the target angle along the arc radius.
            let perp_seed = if axis_unit.abs().dot(Vec3::Y) < 0.9 {
                Vec3::Y
            } else {
                Vec3::X
            };
            let perp = (perp_seed - axis_unit * perp_seed.dot(axis_unit)).normalize();
            let dir = Quat::from_axis_angle(axis_unit, target) * perp;
            let outer = anchor0 + dir * radius * 1.15;
            let inner = anchor0 + dir * radius * 0.85;
            gizmos.line(inner, outer, Color::from(tailwind::PINK_300));
        } else {
            let p = anchor0 + axis_unit * target;
            gizmos.sphere(
                Isometry3d::from_translation(p),
                radius * 0.06,
                Color::from(tailwind::PINK_300),
            );
        }
    }
}

// ── Articulation chains ────────────────────────────────────────────────

fn draw_articulation_chains(
    mut gizmos: Gizmos<PhysicsGizmos>,
    toggles: Res<DisplayToggles>,
    roots: Query<&UsdArticulationRoot>,
    joints: Query<&UsdPhysicsJoint>,
    transforms: Query<&GlobalTransform>,
) {
    if !toggles.show_physics {
        return;
    }
    let palette = articulation_palette();
    for (idx, ar) in roots.iter().enumerate() {
        let col = palette[idx % palette.len()];
        for joint_e in &ar.joints {
            let Ok(joint) = joints.get(*joint_e) else {
                continue;
            };
            let Some(b0) = joint.body0 else { continue };
            let Some(b1) = joint.body1 else { continue };
            let Ok(gt0) = transforms.get(b0) else {
                continue;
            };
            let Ok(gt1) = transforms.get(b1) else {
                continue;
            };
            let (p0, _) = anchor_world(gt0, joint.local_pos0, joint.local_rot0);
            let (p1, _) = anchor_world(gt1, joint.local_pos1, joint.local_rot1);
            // Triple-draw with small Y offset to fake line thickness.
            for ofs in [-0.0015_f32, 0.0, 0.0015] {
                gizmos.line(p0 + Vec3::Y * ofs, p1 + Vec3::Y * ofs, col);
            }
        }
    }
}

fn draw_articulation_root_markers(
    mut gizmos: Gizmos<PhysicsGizmos>,
    toggles: Res<DisplayToggles>,
    extent: Res<SceneExtent>,
    roots: Query<(&UsdArticulationRoot, &GlobalTransform)>,
) {
    if !toggles.show_physics {
        return;
    }
    let r = triad_size(&extent) * 2.0;
    let palette = articulation_palette();
    for (idx, (_, gt)) in roots.iter().enumerate() {
        let col = palette[idx % palette.len()];
        gizmos.sphere(Isometry3d::from_translation(gt.translation()), r, col);
    }
}

// ── Mass ────────────────────────────────────────────────────────────────

fn draw_mass_markers(
    mut gizmos: Gizmos<PhysicsGizmos>,
    toggles: Res<DisplayToggles>,
    extent: Res<SceneExtent>,
    bodies: Query<(&GlobalTransform, &UsdMass, Option<&UsdRigidBody>)>,
) {
    if !toggles.show_physics {
        return;
    }
    let r = triad_size(&extent) * 0.7;
    for (gt, mass, _rb) in &bodies {
        let local_com = mass.center_of_mass.unwrap_or(Vec3::ZERO);
        let body = gt.compute_transform();
        let world_com = body.transform_point(local_com);
        // Explicit mass = warm white; density-only = cool gray;
        // both unauthored (rare — UsdMass would be absent then) = dim.
        let col = match (mass.mass, mass.density) {
            (Some(_), _) => Color::from(tailwind::AMBER_200),
            (None, Some(_)) => Color::from(tailwind::SLATE_300),
            _ => Color::from(tailwind::SLATE_500),
        };
        gizmos.sphere(Isometry3d::from_translation(world_com), r, col);
    }
}

// ── Scene gravity ──────────────────────────────────────────────────────

fn draw_scene_gravity(
    mut gizmos: Gizmos<PhysicsGizmos>,
    toggles: Res<DisplayToggles>,
    extent: Res<SceneExtent>,
    scenes: Query<(&UsdPhysicsScene, &GlobalTransform)>,
) {
    if !toggles.show_physics {
        return;
    }
    let len = extent.diag().max(1.0) * 0.15;
    for (scene, gt) in &scenes {
        if scene.gravity_magnitude <= 0.0 || scene.gravity_direction.length_squared() < 1e-6 {
            continue;
        }
        let origin = gt.translation();
        let dir = scene.gravity_direction.normalize_or_zero();
        gizmos.arrow(origin, origin + dir * len, Color::from(tailwind::SLATE_100));
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn anchor_world(body_gt: &GlobalTransform, local_pos: Vec3, local_rot: Quat) -> (Vec3, Quat) {
    let body = body_gt.compute_transform();
    let pos = body.transform_point(local_pos);
    let rot = body.rotation * local_rot;
    (pos, rot)
}

/// R/G/B triad at a world position + rotation. Inlined here (instead
/// of using `bevy_glacial::draw_axis_triad_with_colors`) so we draw
/// into our own gizmo group with the depth-bias in front.
fn draw_triad(gizmos: &mut Gizmos<PhysicsGizmos>, origin: Vec3, rotation: Quat, length: f32) {
    let tip_x = origin + rotation * Vec3::X * length;
    let tip_y = origin + rotation * Vec3::Y * length;
    let tip_z = origin + rotation * Vec3::Z * length;
    gizmos.arrow(origin, tip_x, FRAME_COLORS[0]);
    gizmos.arrow(origin, tip_y, FRAME_COLORS[1]);
    gizmos.arrow(origin, tip_z, FRAME_COLORS[2]);
}

fn triad_size(extent: &SceneExtent) -> f32 {
    (extent.diag() * 0.012).clamp(0.01, 0.12)
}

fn joint_kind_color(kind: UsdJointKind) -> Color {
    match kind {
        UsdJointKind::Fixed => Color::from(tailwind::SLATE_400),
        UsdJointKind::Revolute => Color::from(tailwind::AMBER_400),
        UsdJointKind::Prismatic => Color::from(tailwind::CYAN_400),
        UsdJointKind::Spherical => Color::from(tailwind::EMERALD_400),
        UsdJointKind::Distance => Color::from(tailwind::YELLOW_400),
        UsdJointKind::Generic => Color::from(tailwind::FUCHSIA_400),
    }
}

fn articulation_palette() -> [Color; 4] {
    [
        Color::from(tailwind::EMERALD_400),
        Color::from(tailwind::ROSE_400),
        Color::from(tailwind::INDIGO_400),
        Color::from(tailwind::ORANGE_400),
    ]
}

/// Map a multi-apply DOF token to a world-space axis at the joint's
/// local frame. Returns `(axis_unit, is_rotational)`. For generic
/// joints we use the joint's authored `axis` as the X reference;
/// trans/rotY/Z fall on the perpendicular basis.
fn dof_world_axis(axis_world: Vec3, dof: UsdDof) -> (Vec3, bool) {
    // Build a basis around `axis_world` (treated as local X).
    let x = axis_world.normalize_or_zero();
    let perp_seed = if x.abs().dot(Vec3::Y) < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let y = (perp_seed - x * perp_seed.dot(x)).normalize();
    let z = x.cross(y).normalize();
    match dof {
        UsdDof::TransX | UsdDof::Linear => (x, false),
        UsdDof::TransY => (y, false),
        UsdDof::TransZ | UsdDof::Distance => (z, false),
        UsdDof::RotX | UsdDof::Angular => (x, true),
        UsdDof::RotY => (y, true),
        UsdDof::RotZ => (z, true),
    }
}
