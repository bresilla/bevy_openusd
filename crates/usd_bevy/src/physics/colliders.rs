//! `UsdCollider` → entries in `PhysicsWorld.colliders`. Bevy ECS
//! adapter; all builder construction lives in `usd_rapier::colliders`.

use bevy::math::DVec3;
use bevy::mesh::Mesh3d;
use bevy::prelude::*;
use openusd::physics::CollisionApprox;
use rapier3d_f64::geometry::{Group, InteractionGroups, InteractionTestMode};
use rapier3d_f64::math::Pose;
use crate::markers::{
    UsdArticulationRoot, UsdCollider, UsdColliderShape, UsdCollisionApprox, UsdPhysicsMaterial,
    UsdRigidBody,
};
use usd_rapier::colliders::{build_collider, ColliderOpinion, ShapeInput};

use super::bodies::BodyAttached;
use super::convert::{quat_to_d, vec3_to_d};
use super::world::PhysicsWorld;

#[derive(Component)]
pub(crate) struct ColliderAttached;

pub fn convert_colliders(
    mut commands: Commands,
    mut world: ResMut<PhysicsWorld>,
    colliders: Query<
        (
            Entity,
            &UsdCollider,
            Option<&UsdRigidBody>,
            Option<&Mesh3d>,
            Option<&GlobalTransform>,
            Option<&bevy::ecs::hierarchy::Children>,
            Option<&ChildOf>,
        ),
        Without<ColliderAttached>,
    >,
    descendant_meshes: Query<&Mesh3d>,
    body_attached: Query<(), With<BodyAttached>>,
    body_globals: Query<&GlobalTransform, With<BodyAttached>>,
    parents: Query<&ChildOf>,
    articulation_roots: Query<(), With<UsdArticulationRoot>>,
    meshes: Res<Assets<Mesh>>,
) {
    for (entity, col, rb_opt, mesh3d, gt, children, child_of) in &colliders {
        // Body-relative scale: scenes with `metersPerUnit != 1` push
        // a uniform scale onto every entity's GlobalTransform; we
        // need the SCALE relative to the parent body so vertices
        // aren't shrunk twice (once by the GlobalTransform chain at
        // render, once by us baking it into the hull).
        let parent_entity = find_body_ancestor(entity, child_of, &body_attached, &parents);
        let body_scale = parent_entity
            .and_then(|e| body_globals.get(e).ok())
            .map(|b| b.compute_transform().scale)
            .unwrap_or(Vec3::ONE);
        let mesh_world_scale = gt
            .map(|g| g.compute_transform().scale)
            .unwrap_or(Vec3::ONE);
        let local_scale = Vec3::new(
            mesh_world_scale.x / body_scale.x,
            mesh_world_scale.y / body_scale.y,
            mesh_world_scale.z / body_scale.z,
        );
        let mesh3d = mesh3d.cloned().or_else(|| {
            children.and_then(|kids| {
                kids.iter()
                    .find_map(|child| descendant_meshes.get(child).ok().cloned())
            })
        });
        // entity_scale used for primitive shape baking (cube/cylinder
        // etc.) — those are world-space dimensions.
        let entity_scale = mesh_world_scale;
        let is_dynamic = rb_opt.is_some_and(|b| b.enabled && !b.kinematic);

        // Build the backend-neutral ShapeInput.
        let shape = match &col.shape {
            UsdColliderShape::Cube { size } => ShapeInput::Cube {
                size: *size as f64,
            },
            UsdColliderShape::Sphere { radius } => ShapeInput::Sphere {
                radius: *radius as f64,
            },
            UsdColliderShape::Capsule { radius, height, axis } => {
                let half = axis.normalize_or_zero() * (*height * 0.5);
                ShapeInput::Capsule {
                    half: DVec3::new(half.x as f64, half.y as f64, half.z as f64),
                    radius: *radius as f64,
                }
            }
            UsdColliderShape::Cylinder { radius, height, axis } => {
                let unit_axis = axis.normalize_or(Vec3::Y);
                let abs_axis = unit_axis.abs();
                let (height_scale, radius_scale) = if abs_axis.x > abs_axis.y && abs_axis.x > abs_axis.z {
                    (entity_scale.x.abs(), entity_scale.y.abs().max(entity_scale.z.abs()))
                } else if abs_axis.z > abs_axis.y {
                    (entity_scale.z.abs(), entity_scale.x.abs().max(entity_scale.y.abs()))
                } else {
                    (entity_scale.y.abs(), entity_scale.x.abs().max(entity_scale.z.abs()))
                };
                ShapeInput::Cylinder {
                    half_height: (*height * 0.5 * height_scale) as f64,
                    radius: (*radius * radius_scale) as f64,
                }
            }
            UsdColliderShape::Plane => ShapeInput::Plane,
            UsdColliderShape::Mesh => {
                let Some(mesh3d) = mesh3d.as_ref() else {
                    continue;
                };
                let Some(mesh) = meshes.get(&mesh3d.0) else {
                    continue;
                };
                info!(
                    "RapierAdapter[mesh-collider]: ent={entity:?} local_scale={:?} approx={:?}",
                    local_scale, col.approximation
                );
                let Some((vertices, indices)) = extract_mesh(mesh, local_scale) else {
                    continue;
                };
                ShapeInput::Mesh {
                    vertices,
                    indices,
                    approx: col.approximation.map(usd_approx_to_openusd),
                    is_dynamic,
                }
            }
        };

        // Compute the collider's body-local pose: includes the mesh's
        // entity-to-body translation/rotation, plus the Y→authored-axis
        // remap for primitive cylinders/capsules so Rapier's Y-default
        // long-axis matches what the mesh xform expects.
        let local_pose = compute_local_pose(parent_entity, &body_globals, gt, &col.shape);

        let groups = find_articulation_root_ancestor(
            entity,
            child_of,
            &articulation_roots,
            &parents,
        )
        .map(|root| {
            let bit = articulation_group_bit(root);
            InteractionGroups::new(
                bit,
                Group::ALL.difference(bit),
                InteractionTestMode::And,
            )
        });

        let parent_handle = parent_entity.and_then(|e| world.entity_to_body.get(&e).copied());

        let op = ColliderOpinion {
            shape,
            local_pose,
            friction: None,
            restitution: None,
            collision_groups: groups,
            user_data: entity.to_bits() as u128,
        };

        let world_mut = world.as_mut();
        match build_collider(
            &mut world_mut.colliders,
            &mut world_mut.bodies,
            parent_handle,
            op,
        ) {
            Ok(Some(handle)) => {
                world_mut.entity_to_collider.insert(entity, handle);
                commands.entity(entity).insert(ColliderAttached);
            }
            _ => {}
        }
    }
}

fn extract_mesh(mesh: &Mesh, scale: Vec3) -> Option<(Vec<DVec3>, Option<Vec<[u32; 3]>>)> {
    let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?.as_float3()?;
    let sx = scale.x as f64;
    let sy = scale.y as f64;
    let sz = scale.z as f64;
    let vertices: Vec<DVec3> = positions
        .iter()
        .map(|p| DVec3::new(p[0] as f64 * sx, p[1] as f64 * sy, p[2] as f64 * sz))
        .collect();
    let indices: Option<Vec<[u32; 3]>> = mesh.indices().map(|i| {
        let raw: Vec<u32> = i.iter().map(|x| x as u32).collect();
        raw.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
    });
    Some((vertices, indices))
}

fn compute_local_pose(
    parent_entity: Option<Entity>,
    body_globals: &Query<&GlobalTransform, With<BodyAttached>>,
    gt: Option<&GlobalTransform>,
    shape: &UsdColliderShape,
) -> Pose {
    let (Some(parent_e), Some(mesh_gt)) = (parent_entity, gt) else {
        return Pose {
            translation: DVec3::ZERO,
            rotation: glam::DQuat::IDENTITY,
        };
    };
    let Ok(body_gt) = body_globals.get(parent_e) else {
        return Pose {
            translation: DVec3::ZERO,
            rotation: glam::DQuat::IDENTITY,
        };
    };
    let body_t = body_gt.compute_transform();
    let mesh_t = mesh_gt.compute_transform();
    let inv_body_rot = body_t.rotation.inverse();
    let world_delta = mesh_t.translation - body_t.translation;
    let local_delta = inv_body_rot * world_delta;
    let local_translation = Vec3::new(
        local_delta.x / body_t.scale.x,
        local_delta.y / body_t.scale.y,
        local_delta.z / body_t.scale.z,
    );
    let axis_remap = match shape {
        UsdColliderShape::Cylinder { axis, .. } | UsdColliderShape::Capsule { axis, .. } => {
            Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Z))
        }
        _ => Quat::IDENTITY,
    };
    let local_rotation = inv_body_rot * mesh_t.rotation * axis_remap;
    Pose {
        translation: vec3_to_d(local_translation),
        rotation: quat_to_d(local_rotation),
    }
}

fn usd_approx_to_openusd(a: UsdCollisionApprox) -> CollisionApprox {
    match a {
        UsdCollisionApprox::None => CollisionApprox::None,
        UsdCollisionApprox::ConvexHull => CollisionApprox::ConvexHull,
        UsdCollisionApprox::ConvexDecomposition => CollisionApprox::ConvexDecomposition,
        UsdCollisionApprox::BoundingSphere => CollisionApprox::BoundingSphere,
        UsdCollisionApprox::BoundingCube => CollisionApprox::BoundingCube,
        UsdCollisionApprox::MeshSimplification => CollisionApprox::MeshSimplification,
    }
}

fn find_articulation_root_ancestor(
    start: Entity,
    own_parent: Option<&ChildOf>,
    articulation_roots: &Query<(), With<UsdArticulationRoot>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    if articulation_roots.get(start).is_ok() {
        return Some(start);
    }
    let mut current = own_parent.map(|p| p.parent());
    while let Some(e) = current {
        if articulation_roots.get(e).is_ok() {
            return Some(e);
        }
        current = parents.get(e).ok().map(|p| p.parent());
    }
    None
}

/// Hash an articulation-root entity to one of Rapier's 32 group
/// bits, skipping bit 0 (reserved for "world / unfiltered").
fn articulation_group_bit(entity: Entity) -> Group {
    let bit_index = (entity.to_bits() % 31) + 1;
    Group::from_bits_truncate(1 << bit_index)
}

fn find_body_ancestor(
    start: Entity,
    own_parent: Option<&ChildOf>,
    body_attached: &Query<(), With<BodyAttached>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    if body_attached.get(start).is_ok() {
        return Some(start);
    }
    let mut current = own_parent.map(|p| p.parent());
    while let Some(e) = current {
        if body_attached.get(e).is_ok() {
            return Some(e);
        }
        current = parents.get(e).ok().map(|p| p.parent());
    }
    None
}

pub fn apply_physics_materials(
    mut world: ResMut<PhysicsWorld>,
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
        let Some(handle) = world.entity_to_collider.get(&entity).copied() else {
            continue;
        };
        let friction_coef = mat.dynamic_friction.or(mat.static_friction).unwrap_or(0.5);
        if let Some(c) = world.colliders.get_mut(handle) {
            c.set_friction(friction_coef as f64);
            if let Some(r) = mat.restitution {
                c.set_restitution(r as f64);
            }
        }
    }
}
