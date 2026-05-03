//! `UsdCollider` → entries in `PhysicsWorld.colliders`, parented to
//! the body handle from `entity_to_body`.
//!
//! Approximation fallback table (PLAN.md §2.5):
//!
//! | Authored      | Static body | Dynamic body          |
//! | ------------- | ----------- | --------------------- |
//! | None/default  | TriMesh     | ConvexHull (warn)     |
//! | ConvexHull    | ConvexHull  | ConvexHull            |
//! | ConvexDecomp  | Decomp      | Decomp                |
//! | MeshSimplify  | TriMesh     | ConvexHull (warn)     |

use bevy::math::DVec3;
use bevy::mesh::Mesh3d;
use bevy::prelude::*;
use rapier3d_f64::prelude::*;
use usd_physics_markers::{
    UsdArticulationRoot, UsdCollider, UsdColliderShape, UsdCollisionApprox, UsdPhysicsMaterial,
    UsdRigidBody,
};

use crate::bodies::BodyAttached;
use crate::convert::{quat_to_d, vec3_to_d};
use crate::world::PhysicsWorld;

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
        // Find the body this collider belongs to BEFORE shape-building
        // so we can compute the entity-local-to-body-local scale for
        // mesh vertex baking. Scenes with `metersPerUnit != 1` push
        // a uniform scale onto the scene root that propagates through
        // every entity's GlobalTransform — using the absolute scale
        // would shrink the hull by that factor a second time.
        let parent_entity_pre = find_body_ancestor(entity, child_of, &body_attached, &parents);
        let body_scale = parent_entity_pre
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
        // etc.) where the shape is in world space. Mesh-collider
        // vertex baking uses `local_scale` instead.
        let entity_scale = mesh_world_scale;
        let is_dynamic = rb_opt.is_some_and(|b| b.enabled && !b.kinematic);

        let builder = match &col.shape {
            UsdColliderShape::Cube { size } => {
                let h = (*size as f64) * 0.5;
                Some(ColliderBuilder::cuboid(h, h, h))
            }
            UsdColliderShape::Sphere { radius } => {
                Some(ColliderBuilder::ball(*radius as f64))
            }
            UsdColliderShape::Capsule { radius, height, axis } => {
                let half = axis.normalize_or_zero() * (*height * 0.5);
                let a = DVec3::new(-half.x as f64, -half.y as f64, -half.z as f64);
                let b = DVec3::new(half.x as f64, half.y as f64, half.z as f64);
                Some(ColliderBuilder::capsule_from_endpoints(a, b, *radius as f64))
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
                let baked_radius = (radius * radius_scale) as f64;
                let baked_half_height = (height * 0.5 * height_scale) as f64;
                Some(ColliderBuilder::cylinder(baked_half_height, baked_radius))
            }
            UsdColliderShape::Plane => Some(ColliderBuilder::cuboid(50.0, 0.001, 50.0)),
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
                build_mesh_collider(mesh, col.approximation, is_dynamic, local_scale, entity)
            }
        };

        let Some(mut builder) = builder else {
            continue;
        };

        // Mesh colliders get the entity scale baked into vertices
        // since Rapier doesn't track collider scale separately.
        // Primitives already had their scale baked into shape params.

        // Disable contacts for colliders on bodies that share a joint
        // — handled later in joints.rs by setting contacts_enabled
        // on the joint itself, NOT here.
        builder = builder.user_data(entity.to_bits() as u128);

        // Find the body to parent this collider to. Walk up the
        // entity hierarchy looking for an ancestor with `BodyAttached`.
        let parent_entity = find_body_ancestor(entity, child_of, &body_attached, &parents);
        let parent_handle = parent_entity.and_then(|e| world.entity_to_body.get(&e).copied());

        // Self-collision filter: every collider whose body sits
        // inside an articulation root subtree gets a group bit
        // hashed from that root's entity. Two colliders sharing the
        // same articulation skip mutual contacts (chassis hulls vs
        // wheel hulls within the same vehicle); they still collide
        // with the world (group 0) and other articulations.
        if let Some(art_root) = find_articulation_root_ancestor(
            entity,
            child_of,
            &articulation_roots,
            &parents,
        ) {
            let bit = articulation_group_bit(art_root);
            // Membership = own bit. Filter = ALL bits except own.
            // ALL is `Group::ALL`; remove `bit` to skip self pairs.
            let groups = InteractionGroups::new(
                bit,
                Group::ALL.difference(bit),
                InteractionTestMode::And,
            );
            builder = builder.collision_groups(groups);
        }

        // Apply the collider's local pose relative to its parent
        // body. The mesh entity's GlobalTransform sits somewhere
        // inside the body subtree (intermediate Xforms with
        // compensation translates / scales). Subtract the body's
        // world pose so the collider's translation/rotation are
        // correctly expressed in the body's local frame.
        if let (Some(parent_e), Some(mesh_gt)) = (parent_entity, gt) {
            if let Ok(body_gt) = body_globals.get(parent_e) {
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
                // USD's Cylinder/Capsule default axis is Z (per
                // `UsdGeomCylinder` spec); Rapier's primitive
                // cylinder/capsule has its long axis along Y. The
                // mesh's own xformOp:orient already accounts for
                // wherever the author wanted the cylinder pointing
                // in body-local space — so we need to compose a
                // Y→authored-axis remap so Rapier's Y-default lines
                // up with what the mesh xform expects to consume.
                let axis_remap = match &col.shape {
                    UsdColliderShape::Cylinder { axis, .. }
                    | UsdColliderShape::Capsule { axis, .. } => {
                        Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Z))
                    }
                    _ => Quat::IDENTITY,
                };
                let local_rotation = inv_body_rot * mesh_t.rotation * axis_remap;
                builder = builder.position(Pose {
                    translation: vec3_to_d(local_translation),
                    rotation: quat_to_d(local_rotation),
                });
            }
        }

        let collider = builder.build();
        // Split the borrow so insert_with_parent can take &mut to
        // both colliders and bodies fields simultaneously.
        let world_mut = world.as_mut();
        let handle = if let Some(parent) = parent_handle {
            world_mut.colliders.insert_with_parent(
                collider,
                parent,
                &mut world_mut.bodies,
            )
        } else {
            world_mut.colliders.insert(collider)
        };
        world_mut.entity_to_collider.insert(entity, handle);
        commands.entity(entity).insert(ColliderAttached);
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
/// bits, skipping bit 0 (reserved for "world / unfiltered"). Two
/// articulations may collide-on-self if their bits collide — fine
/// for handfuls of vehicles, problematic at fleet scale (where
/// you'd want a per-pair custom filter via `PhysicsHooks`).
fn articulation_group_bit(entity: Entity) -> Group {
    // Bit index in [1, 31]. Avoid 0 so the world (default group 0)
    // can still see articulated colliders.
    let bit_index = (entity.to_bits() % 31) + 1;
    Group::from_bits_truncate(1 << bit_index)
}

fn rotation_to_axis_angle(q: Quat) -> DVec3 {
    let (axis, angle) = q.to_axis_angle();
    let v = axis.normalize_or_zero() * angle;
    DVec3::new(v.x as f64, v.y as f64, v.z as f64)
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

fn build_mesh_collider(
    mesh: &Mesh,
    approx: Option<UsdCollisionApprox>,
    is_dynamic: bool,
    scale: Vec3,
    entity: Entity,
) -> Option<ColliderBuilder> {
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

    let approx = approx.unwrap_or(UsdCollisionApprox::None);
    match approx {
        UsdCollisionApprox::ConvexHull => {
            ColliderBuilder::convex_hull(&vertices)
                .or_else(|| {
                    warn!("RapierAdapter: convex_hull failed for {entity:?}, falling back to bounding cuboid");
                    None
                })
        }
        UsdCollisionApprox::ConvexDecomposition => {
            let Some(idx) = indices else {
                warn!("RapierAdapter: convex decomposition needs indexed mesh; skipping {entity:?}");
                return None;
            };
            Some(ColliderBuilder::convex_decomposition(&vertices, &idx))
        }
        UsdCollisionApprox::None | UsdCollisionApprox::MeshSimplification => {
            if is_dynamic {
                warn!(
                    "RapierAdapter: mesh collider on dynamic body {entity:?} approx={approx:?}; falling back to ConvexHull (Rapier rejects trimesh dynamic)"
                );
                ColliderBuilder::convex_hull(&vertices)
            } else {
                let idx = indices.unwrap_or_else(|| {
                    (0..vertices.len() / 3).map(|i| [(i*3) as u32, (i*3+1) as u32, (i*3+2) as u32]).collect()
                });
                Some(ColliderBuilder::trimesh(vertices, idx).expect("trimesh build"))
            }
        }
        UsdCollisionApprox::BoundingSphere | UsdCollisionApprox::BoundingCube => {
            ColliderBuilder::convex_hull(&vertices)
        }
    }
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
