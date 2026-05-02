//! `UsdCollider` → Rapier `Collider`, plus `Friction` / `Restitution`
//! from a bound `UsdPhysicsMaterial` entity.

use bevy::mesh::Mesh3d;
use bevy::prelude::*;
use usd_physics_markers::{
    UsdCollider, UsdColliderShape, UsdCollisionApprox, UsdPhysicsMaterial, UsdRigidBody,
};
use bevy_rapier3d::geometry::{
    Collider, ColliderScale, ComputedColliderShape, Friction, Restitution,
};

/// Build a Rapier `Collider` from each `UsdCollider`. Primitive
/// shapes (Cube/Sphere/Capsule/Cylinder/Plane) use Rapier's native
/// constructors; mesh colliders look up the entity's `Mesh3d` and
/// honour the `MeshCollisionAPI` approximation token.
///
/// Approximation fallback table (PLAN.md §2.5):
///
/// | Authored      | Static body | Dynamic body          |
/// | ------------- | ----------- | --------------------- |
/// | None/default  | TriMesh     | ConvexHull (warn)     |
/// | ConvexHull    | ConvexHull  | ConvexHull            |
/// | ConvexDecomp  | Decomp      | Decomp                |
/// | MeshSimplify  | TriMesh     | ConvexHull (warn)     |
pub fn convert_colliders(
    mut commands: Commands,
    colliders: Query<
        (
            Entity,
            &UsdCollider,
            Option<&UsdRigidBody>,
            Option<&Mesh3d>,
            Option<&GlobalTransform>,
            Option<&bevy::ecs::hierarchy::Children>,
        ),
        Without<Collider>,
    >,
    descendant_meshes: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
) {
    for (entity, col, rb, mesh3d, gt, children) in &colliders {
        // Some asset patterns put `PhysicsCollisionAPI` on an Xform
        // that references a Mesh prim. Bevy then spawns the referenced
        // mesh as a CHILD of this entity, with `Mesh3d` only on the
        // child. Search descendants if the entity itself has none.
        let mesh3d = mesh3d.cloned().or_else(|| {
            children.and_then(|kids| {
                kids.iter()
                    .find_map(|child| descendant_meshes.get(child).ok().cloned())
            })
        });
        let mesh3d = mesh3d.as_ref();
        let entity_scale = gt
            .map(|g| g.compute_transform().scale)
            .unwrap_or(Vec3::ONE);
        let is_dynamic = rb.is_some_and(|b| b.enabled && !b.kinematic);

        let collider = match &col.shape {
            UsdColliderShape::Cube { size } => {
                let h = size * 0.5;
                Some(Collider::cuboid(h, h, h))
            }
            UsdColliderShape::Sphere { radius } => Some(Collider::ball(*radius)),
            UsdColliderShape::Capsule { radius, height, axis } => {
                let half = axis.normalize_or_zero() * (height * 0.5);
                Some(Collider::capsule(-half, half, *radius))
            }
            UsdColliderShape::Cylinder { radius, height, axis } => {
                // Match NVIDIA PhysX's `Collision.cpp` algorithm: bake
                // the entity's GlobalTransform.scale into the cylinder
                // dimensions per the authored axis (height-scale =
                // scale along the axis, radius-scale = max of the
                // OTHER two). Bevy_rapier's `apply_collider_scale`
                // can't do this — it scales the inner shape's RAW
                // (un-rotated) frame in world axes, so a compound-
                // wrapped cylinder ends up with the wrong axis's
                // scale on its height. Suppress the auto-scale via
                // `ColliderScale::Absolute(Vec3::ONE)` and create the
                // cylinder at its already-scaled dimensions.
                let unit_axis = axis.normalize_or(Vec3::Y);
                let abs_axis = unit_axis.abs();
                let (height_scale, radius_scale) = if abs_axis.x > abs_axis.y
                    && abs_axis.x > abs_axis.z
                {
                    (
                        entity_scale.x.abs(),
                        entity_scale.y.abs().max(entity_scale.z.abs()),
                    )
                } else if abs_axis.z > abs_axis.y {
                    (
                        entity_scale.z.abs(),
                        entity_scale.x.abs().max(entity_scale.y.abs()),
                    )
                } else {
                    (
                        entity_scale.y.abs(),
                        entity_scale.x.abs().max(entity_scale.z.abs()),
                    )
                };
                let baked_radius = radius * radius_scale;
                let baked_half_height = height * 0.5 * height_scale;
                let cyl = Collider::cylinder(baked_half_height, baked_radius);
                commands
                    .entity(entity)
                    .insert(ColliderScale::Absolute(Vec3::ONE));
                if unit_axis.abs_diff_eq(Vec3::Y, 1e-4) {
                    Some(cyl)
                } else {
                    let rot = Quat::from_rotation_arc(Vec3::Y, unit_axis);
                    Some(Collider::compound(vec![(Vec3::ZERO, rot, cyl)]))
                }
            }
            UsdColliderShape::Plane => {
                // No native `plane` collider; use a thin slab.
                Some(Collider::cuboid(50.0, 0.001, 50.0))
            }
            UsdColliderShape::Mesh => {
                let Some(mesh3d) = mesh3d else {
                    // No Mesh3d on this entity.
                    info!("RapierAdapter[mesh-collider]: ent={entity:?} waiting for Mesh3d");
                    continue;
                };
                let Some(mesh) = meshes.get(&mesh3d.0) else {
                    info!("RapierAdapter[mesh-collider]: ent={entity:?} waiting for mesh asset");
                    continue;
                };
                info!("RapierAdapter[mesh-collider]: ent={entity:?} building (approx={:?})", col.approximation);
                let approx = col.approximation.unwrap_or(UsdCollisionApprox::None);
                let computed = match approx {
                    UsdCollisionApprox::ConvexHull => ComputedColliderShape::ConvexHull,
                    UsdCollisionApprox::ConvexDecomposition => {
                        ComputedColliderShape::ConvexDecomposition(default())
                    }
                    UsdCollisionApprox::None | UsdCollisionApprox::MeshSimplification => {
                        if is_dynamic {
                            warn!(
                                "RapierAdapter: mesh collider on dynamic body {entity:?} has \
                                 approximation={approx:?}; falling back to ConvexHull (trimesh \
                                 dynamic colliders are rejected by Rapier)"
                            );
                            ComputedColliderShape::ConvexHull
                        } else {
                            ComputedColliderShape::TriMesh(default())
                        }
                    }
                    UsdCollisionApprox::BoundingSphere | UsdCollisionApprox::BoundingCube => {
                        // No native equivalent; engineers usually want
                        // these as primitive Sphere/Cube authored on the
                        // collider prim, not a mesh approximation. Fall
                        // back to convex hull which is the next-cheapest
                        // shape Rapier can produce.
                        ComputedColliderShape::ConvexHull
                    }
                };
                Collider::from_bevy_mesh(mesh, &computed)
            }
        };

        let Some(collider) = collider else {
            continue;
        };
        // bevy_rapier3d's `apply_collider_scale` system (in
        // `plugin::systems::collider`) already takes the entity's
        // GlobalTransform.scale and bakes it into the collider shape
        // each frame. So even for raw-mm USD mesh data under a 0.001
        // mm→m parent xform, the final collider lands in metres
        // automatically — we don't need (and must not) call set_scale
        // ourselves. Doing so would double-apply the scale.
        commands.entity(entity).insert(collider);
    }
}

/// For colliders that resolved a `physics_material` rel to an entity
/// carrying `UsdPhysicsMaterial`, copy the friction / restitution
/// scalars onto the collider.
pub fn apply_physics_materials(
    mut commands: Commands,
    colliders: Query<(Entity, &UsdCollider), Added<UsdCollider>>,
    materials: Query<&UsdPhysicsMaterial>,
) {
    for (entity, col) in &colliders {
        let Some(mat_e) = col.physics_material else {
            continue;
        };
        let Ok(mat) = materials.get(mat_e) else {
            continue;
        };
        // Rapier doesn't distinguish static / dynamic friction at the
        // material level — use dynamic when present, else static.
        let friction_coef = mat
            .dynamic_friction
            .or(mat.static_friction)
            .unwrap_or(0.5);
        commands.entity(entity).insert(Friction {
            coefficient: friction_coef,
            ..default()
        });
        if let Some(r) = mat.restitution {
            commands.entity(entity).insert(Restitution {
                coefficient: r,
                ..default()
            });
        }
    }
}
