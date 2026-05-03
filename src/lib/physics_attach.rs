//! Stage → ECS projection of UsdPhysics opinions.
//!
//! Called from `build.rs::spawn_prim_subtree` for every prim entity
//! (per-prim attachment), then from `build.rs::stage_to_scene` after
//! the main walk completes (relationship resolution + articulation
//! tree walk).
//!
//! All conversions to SI happen here. The schema reader returns scene
//! units; the marker components ship metres / kilograms / radians.

use std::collections::HashMap;
use std::f32::consts::PI;

use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::Children;
use bevy::ecs::world::World;
use bevy::math::{Quat, Vec3};
use openusd::Stage;
use openusd::sdf::{Path, Value};

use usd_physics_markers::*;
use usd_schemas::physics as ph;

/// Stage-level conversion factors. Read once at the start of
/// `stage_to_scene` and threaded through every per-prim attachment.
#[derive(Debug, Clone, Copy)]
pub struct StageMeta {
    /// Linear unit conversion: scene units × this = metres.
    pub meters_per_unit: f32,
    /// Mass unit conversion: scene units × this = kilograms.
    pub kilograms_per_unit: f32,
    /// Rotation that takes USD-native vectors into Bevy world space
    /// (identity for Y-up stages; -π/2 about X for Z-up).
    pub basis_rotation: Quat,
}

impl Default for StageMeta {
    fn default() -> Self {
        Self {
            meters_per_unit: 1.0,
            kilograms_per_unit: 1.0,
            basis_rotation: Quat::IDENTITY,
        }
    }
}

/// Collected from the pseudo-root before the main walk.
pub fn read_stage_meta(stage: &Stage) -> StageMeta {
    let up_axis = stage
        .field::<String>(Path::abs_root(), "upAxis")
        .ok()
        .flatten();
    // Match `root_basis_transform`'s default-fallback chain so unit
    // conversion stays consistent with the scene-root scale.
    let authored_mpu = stage
        .field::<Value>(Path::abs_root(), "metersPerUnit")
        .ok()
        .flatten()
        .and_then(|v| match v {
            Value::Double(d) => Some(d as f32),
            Value::Float(f) => Some(f),
            Value::Int(i) => Some(i as f32),
            Value::Int64(i) => Some(i as f32),
            _ => None,
        });
    let meters_per_unit = std::env::var("BEVY_OPENUSD_METERS_PER_UNIT")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .or(authored_mpu)
        .unwrap_or(0.01);
    let kilograms_per_unit = stage
        .field::<Value>(Path::abs_root(), "kilogramsPerUnit")
        .ok()
        .flatten()
        .and_then(|v| match v {
            Value::Double(d) => Some(d as f32),
            Value::Float(f) => Some(f),
            Value::Int(i) => Some(i as f32),
            Value::Int64(i) => Some(i as f32),
            _ => None,
        })
        .unwrap_or(1.0);
    let basis_rotation = match up_axis.as_deref() {
        Some("Z") => Quat::from_rotation_x(-core::f32::consts::FRAC_PI_2),
        _ => Quat::IDENTITY,
    };
    StageMeta {
        meters_per_unit,
        kilograms_per_unit,
        basis_rotation,
    }
}

/// Pending relationship resolution state. Filled during the walk;
/// drained in `resolve_pending_physics` once every prim entity exists.
#[derive(Default)]
pub struct PendingPhysics {
    /// Joint entities awaiting body0/body1 path → Entity resolution.
    pub joints: Vec<(Entity, Option<String>, Option<String>)>,
    /// (group entity, member prim paths, filtered group prim paths)
    pub collision_groups: Vec<(Entity, Vec<String>, Vec<String>)>,
    /// (filter entity, filtered prim paths)
    pub filtered_pairs: Vec<(Entity, Vec<String>)>,
    /// (collider entity, bound material prim path)
    pub colliders_with_material: Vec<(Entity, String)>,
    /// ArticulationRoot entities — populated with joint subtree post-pass.
    pub articulation_roots: Vec<Entity>,
}

// ════════════════════════════════════════════════════════════════════════
//                          PER-PRIM ATTACHMENT
// ════════════════════════════════════════════════════════════════════════

/// Inspect `prim` for every UsdPhysics opinion and attach the
/// corresponding marker component(s) to `entity`. Joint and collision-
/// group body refs land as `None` here and get resolved by the
/// post-pass once every prim entity exists.
pub fn attach_physics_to_prim(
    stage: &Stage,
    path: &Path,
    entity: Entity,
    world: &mut World,
    pending: &mut PendingPhysics,
    meta: &StageMeta,
) {
    // PhysicsScene
    if let Ok(Some(scene)) = ph::read_physics_scene(stage, path) {
        let dir = scene
            .gravity_direction
            .map(|d| meta.basis_rotation * Vec3::from_array(d))
            .unwrap_or(Vec3::NEG_Y);
        // Per UsdPhysicsScene spec, `physics:gravityMagnitude` is in
        // scene units / s². But virtually every authored scene writes
        // `9.81` meaning Earth gravity in m/s² regardless of
        // `metersPerUnit`. Multiplying by metersPerUnit on a
        // `metersPerUnit=0.01` scene (Scout V2, Isaac Sim assets)
        // turns 9.81 into 0.0981 m/s² and the whole sim falls in
        // slow-motion. Treat the authored value as already in m/s².
        let mag = scene.gravity_magnitude.unwrap_or(9.81);
        world.entity_mut(entity).insert(UsdPhysicsScene {
            gravity_direction: dir.normalize_or_zero(),
            gravity_magnitude: mag,
        });
    }

    // Most other schemas piggyback on apiSchemas — read once.
    let api_schemas = ph::read_api_schemas(stage, path).unwrap_or_default();

    // RigidBodyAPI
    if api_schemas.iter().any(|s| s == "PhysicsRigidBodyAPI") {
        let kinematic = read_bool(stage, path, "physics:kinematicEnabled").unwrap_or(false);
        let enabled = read_bool(stage, path, "physics:rigidBodyEnabled").unwrap_or(true);
        let starts_asleep = read_bool(stage, path, "physics:startsAsleep").unwrap_or(false);
        let velocity = read_vec3f(stage, path, "physics:velocity")
            .map(|v| Vec3::from_array(v) * meta.meters_per_unit)
            .unwrap_or(Vec3::ZERO);
        // USD authors angularVelocity in DEGREES per second; convert.
        let angular_velocity = read_vec3f(stage, path, "physics:angularVelocity")
            .map(|v| Vec3::from_array(v) * (PI / 180.0))
            .unwrap_or(Vec3::ZERO);
        let simulation_owner = read_rel_first(stage, path, "physics:simulationOwner");
        world.entity_mut(entity).insert(UsdRigidBody {
            kinematic,
            enabled,
            starts_asleep,
            velocity,
            angular_velocity,
            simulation_owner,
        });
    }

    // MassAPI
    if let Ok(Some(mass)) = ph::read_mass(stage, path) {
        world.entity_mut(entity).insert(UsdMass {
            mass: mass.mass.map(|m| m * meta.kilograms_per_unit),
            density: mass
                .density
                .map(|d| d * meta.kilograms_per_unit / meta.meters_per_unit.powi(3)),
            center_of_mass: mass
                .center_of_mass
                .map(|c| Vec3::from_array(c) * meta.meters_per_unit),
            diagonal_inertia: mass.diagonal_inertia.map(|i| {
                Vec3::from_array(i) * meta.kilograms_per_unit * meta.meters_per_unit.powi(2)
            }),
            principal_axes: mass.principal_axes.map(quat_from_usd_wxyz),
        });
    }

    // CollisionAPI (+ optional MeshCollisionAPI)
    if let Ok(Some(coll)) = ph::read_collision_shape(stage, path) {
        let shape = collider_shape_from_prim(stage, path, meta);
        let approximation = coll
            .approximation
            .map(usd_collision_approx_to_marker);
        if let Some(mat_path) = &coll.physics_material_path {
            pending
                .colliders_with_material
                .push((entity, mat_path.clone()));
        }
        world.entity_mut(entity).insert(UsdCollider {
            shape,
            enabled: coll.collision_enabled,
            approximation,
            physics_material: None,
            simulation_owner: coll.simulation_owner,
        });
    }

    // PhysicsMaterialAPI (typically on a Material prim)
    if let Ok(Some(m)) = ph::read_physics_material(stage, path) {
        world.entity_mut(entity).insert(UsdPhysicsMaterial {
            static_friction: m.static_friction,
            dynamic_friction: m.dynamic_friction,
            restitution: m.restitution,
            density: m
                .density
                .map(|d| d * meta.kilograms_per_unit / meta.meters_per_unit.powi(3)),
        });
    }

    // ArticulationRootAPI
    if api_schemas.iter().any(|s| s == "PhysicsArticulationRootAPI") {
        world.entity_mut(entity).insert(UsdArticulationRoot::default());
        pending.articulation_roots.push(entity);
    }

    // FilteredPairsAPI
    if let Ok(Some(fp)) = ph::read_filtered_pairs(stage, path) {
        world.entity_mut(entity).insert(UsdCollisionFilter::default());
        pending.filtered_pairs.push((entity, fp.filtered));
    }

    // Joints (one entity per Physics*Joint prim).
    if let Ok(Some(j)) = ph::read_joint(stage, path) {
        let kind = match j.kind {
            ph::JointKind::Fixed => UsdJointKind::Fixed,
            ph::JointKind::Revolute => UsdJointKind::Revolute,
            ph::JointKind::Prismatic => UsdJointKind::Prismatic,
            ph::JointKind::Spherical => UsdJointKind::Spherical,
            ph::JointKind::Distance => UsdJointKind::Distance,
            ph::JointKind::Generic => UsdJointKind::Generic,
        };
        let axis = match j.axis.as_deref() {
            Some("Y") => Vec3::Y,
            Some("Z") => Vec3::Z,
            _ => Vec3::X, // default per UsdPhysics spec
        };
        let built_in_limit = match (j.kind, j.lower_limit, j.upper_limit) {
            (ph::JointKind::Revolute, Some(lo), Some(hi)) => {
                Some((lo.to_radians(), hi.to_radians()))
            }
            (ph::JointKind::Prismatic, Some(lo), Some(hi)) => {
                Some((lo * meta.meters_per_unit, hi * meta.meters_per_unit))
            }
            _ => None,
        };
        let cone_limit = match (j.cone_angle_0, j.cone_angle_1) {
            (Some(a), Some(b)) => Some((a.to_radians(), b.to_radians())),
            _ => None,
        };
        let distance_limit = match (j.min_distance, j.max_distance) {
            (Some(lo), Some(hi)) => Some((lo * meta.meters_per_unit, hi * meta.meters_per_unit)),
            _ => None,
        };
        let limits = j
            .limits
            .into_iter()
            .map(|l| convert_limit(l, meta))
            .collect();
        let drives = j
            .drives
            .into_iter()
            .map(|d| convert_drive(d, meta))
            .collect();
        let joint = UsdPhysicsJoint {
            kind,
            body0: None,
            body1: None,
            local_pos0: Vec3::from_array(j.local_pos0) * meta.meters_per_unit,
            local_rot0: quat_from_usd_wxyz(j.local_rot0),
            local_pos1: Vec3::from_array(j.local_pos1) * meta.meters_per_unit,
            local_rot1: quat_from_usd_wxyz(j.local_rot1),
            axis,
            joint_enabled: j.joint_enabled,
            collision_enabled: j.collision_enabled,
            exclude_from_articulation: j.exclude_from_articulation,
            break_force: j.break_force,
            break_torque: j.break_torque,
            built_in_limit,
            cone_limit,
            distance_limit,
            limits,
            drives,
        };
        world.entity_mut(entity).insert(joint);
        pending.joints.push((entity, j.body0, j.body1));
    }

    // CollisionGroup
    if let Ok(Some(g)) = ph::read_collision_group(stage, path) {
        world.entity_mut(entity).insert(UsdCollisionGroup {
            members: Vec::new(),
            filtered: Vec::new(),
            merge_group: g.merge_group,
            invert_filtered_groups: g.invert_filtered_groups,
        });
        pending
            .collision_groups
            .push((entity, g.members, g.filtered_groups));
    }
}

// ════════════════════════════════════════════════════════════════════════
//                       POST-WALK RELATIONSHIP RESOLVE
// ════════════════════════════════════════════════════════════════════════

/// Drain `pending` and substitute every USD prim path string with the
/// corresponding `Entity` from `prim_paths`. Joints, collision groups,
/// filtered pairs, and collider material bindings all get patched here.
pub fn resolve_pending_physics(
    world: &mut World,
    pending: &PendingPhysics,
    prim_paths: &HashMap<String, Entity>,
) {
    for (joint_entity, body0_path, body1_path) in &pending.joints {
        let body0 = body0_path.as_ref().and_then(|p| prim_paths.get(p).copied());
        let body1 = body1_path.as_ref().and_then(|p| prim_paths.get(p).copied());
        if let Some(mut entity_mut) = world.get_entity_mut(*joint_entity).ok()
            && let Some(mut joint) = entity_mut.get_mut::<UsdPhysicsJoint>()
        {
            joint.body0 = body0;
            joint.body1 = body1;
        }
    }

    for (group_entity, members, filtered) in &pending.collision_groups {
        let members: Vec<Entity> = members
            .iter()
            .filter_map(|p| prim_paths.get(p).copied())
            .collect();
        let filtered: Vec<Entity> = filtered
            .iter()
            .filter_map(|p| prim_paths.get(p).copied())
            .collect();
        if let Some(mut entity_mut) = world.get_entity_mut(*group_entity).ok()
            && let Some(mut group) = entity_mut.get_mut::<UsdCollisionGroup>()
        {
            group.members = members;
            group.filtered = filtered;
        }
    }

    for (filter_entity, paths) in &pending.filtered_pairs {
        let resolved: Vec<Entity> = paths
            .iter()
            .filter_map(|p| prim_paths.get(p).copied())
            .collect();
        if let Some(mut entity_mut) = world.get_entity_mut(*filter_entity).ok()
            && let Some(mut filt) = entity_mut.get_mut::<UsdCollisionFilter>()
        {
            filt.filtered = resolved;
        }
    }

    for (collider_entity, mat_path) in &pending.colliders_with_material {
        let mat_entity = prim_paths.get(mat_path).copied();
        if let Some(mut entity_mut) = world.get_entity_mut(*collider_entity).ok()
            && let Some(mut col) = entity_mut.get_mut::<UsdCollider>()
        {
            col.physics_material = mat_entity;
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//                         ARTICULATION SUBTREE WALK
// ════════════════════════════════════════════════════════════════════════

/// For each ArticulationRoot, walk descendants and collect every
/// `UsdPhysicsJoint` entity (filtered by `exclude_from_articulation`).
/// Loop-closing detection (proper DAG check) is left to the engine
/// adapter — it has more context (which joints map to MultibodyJoint
/// vs ImpulseJoint in Rapier's case).
pub fn populate_articulation_joints(world: &mut World, articulation_roots: &[Entity]) {
    for root in articulation_roots {
        let mut joints = Vec::new();
        collect_joints_recursive(world, *root, &mut joints);
        if let Some(mut entity_mut) = world.get_entity_mut(*root).ok()
            && let Some(mut ar) = entity_mut.get_mut::<UsdArticulationRoot>()
        {
            ar.joints = joints;
        }
    }
}

fn collect_joints_recursive(world: &World, entity: Entity, out: &mut Vec<Entity>) {
    if let Some(joint) = world.get::<UsdPhysicsJoint>(entity)
        && !joint.exclude_from_articulation
        && joint.joint_enabled
    {
        out.push(entity);
    }
    if let Some(children) = world.get::<Children>(entity) {
        let child_entities: Vec<Entity> = children.iter().copied().collect();
        for child in child_entities {
            collect_joints_recursive(world, child, out);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
//                          conversion helpers
// ════════════════════════════════════════════════════════════════════════

fn quat_from_usd_wxyz(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[1], q[2], q[3], q[0])
}

fn usd_collision_approx_to_marker(a: ph::CollisionApprox) -> UsdCollisionApprox {
    match a {
        ph::CollisionApprox::None => UsdCollisionApprox::None,
        ph::CollisionApprox::ConvexHull => UsdCollisionApprox::ConvexHull,
        ph::CollisionApprox::ConvexDecomposition => UsdCollisionApprox::ConvexDecomposition,
        ph::CollisionApprox::BoundingSphere => UsdCollisionApprox::BoundingSphere,
        ph::CollisionApprox::BoundingCube => UsdCollisionApprox::BoundingCube,
        ph::CollisionApprox::MeshSimplification => UsdCollisionApprox::MeshSimplification,
    }
}

fn dof_to_marker(d: ph::Dof) -> UsdDof {
    match d {
        ph::Dof::TransX => UsdDof::TransX,
        ph::Dof::TransY => UsdDof::TransY,
        ph::Dof::TransZ => UsdDof::TransZ,
        ph::Dof::RotX => UsdDof::RotX,
        ph::Dof::RotY => UsdDof::RotY,
        ph::Dof::RotZ => UsdDof::RotZ,
        ph::Dof::Linear => UsdDof::Linear,
        ph::Dof::Angular => UsdDof::Angular,
        ph::Dof::Distance => UsdDof::Distance,
    }
}

fn drive_type_to_marker(t: ph::DriveType) -> UsdDriveType {
    match t {
        ph::DriveType::Force => UsdDriveType::Force,
        ph::DriveType::Acceleration => UsdDriveType::Acceleration,
    }
}

fn dof_is_rotational(d: ph::Dof) -> bool {
    matches!(
        d,
        ph::Dof::RotX | ph::Dof::RotY | ph::Dof::RotZ | ph::Dof::Angular
    )
}

fn convert_limit(l: ph::ReadLimit, meta: &StageMeta) -> UsdJointLimit {
    let (low, high) = if dof_is_rotational(l.dof) {
        (l.low.to_radians(), l.high.to_radians())
    } else {
        (l.low * meta.meters_per_unit, l.high * meta.meters_per_unit)
    };
    UsdJointLimit {
        dof: dof_to_marker(l.dof),
        low,
        high,
    }
}

fn convert_drive(d: ph::ReadDrive, meta: &StageMeta) -> UsdJointDrive {
    let rot = dof_is_rotational(d.dof);
    let pos_scale = if rot { PI / 180.0 } else { meta.meters_per_unit };
    let target_position = d.target_position.map(|v| v * pos_scale);
    let target_velocity = d.target_velocity.map(|v| v * pos_scale);
    // Stiffness/damping are authored in scene-unit-per-author-angle;
    // convert by the same per-DOF factor (USD spec: stiffness scales
    // (target - current), so its units are inverse of position).
    let dyn_scale = if rot { 180.0 / PI } else { 1.0 / meta.meters_per_unit };
    UsdJointDrive {
        dof: dof_to_marker(d.dof),
        drive_type: drive_type_to_marker(d.drive_type),
        target_position,
        target_velocity,
        damping: d.damping * dyn_scale,
        stiffness: d.stiffness * dyn_scale,
        max_force: d.max_force,
    }
}

fn collider_shape_from_prim(stage: &Stage, path: &Path, _meta: &StageMeta) -> UsdColliderShape {
    // Primitive collider dimensions stay in **scene units**, NOT
    // pre-multiplied by `metersPerUnit`. The downstream Rapier adapter
    // hands the raw shape to bevy_rapier; bevy_rapier's own
    // `apply_collider_scale` system multiplies by the entity's
    // `GlobalTransform.scale` every frame, and the scene-root scale
    // we author already encodes the unit conversion. Pre-multiplying
    // here would double-count it (Scout V2 reproduced this:
    // `metersPerUnit=0.01` × scene-root scale `0.01` × shape `0.01`
    // collapsed every collider to millimetre size).
    let type_name = stage
        .field::<String>(path.clone(), "typeName")
        .ok()
        .flatten()
        .unwrap_or_default();
    match type_name.as_str() {
        "Cube" => {
            let size = read_double(stage, path, "size").unwrap_or(2.0) as f32;
            UsdColliderShape::Cube { size }
        }
        "Sphere" => {
            let radius = read_double(stage, path, "radius").unwrap_or(1.0) as f32;
            UsdColliderShape::Sphere { radius }
        }
        "Capsule" => {
            let radius = read_double(stage, path, "radius").unwrap_or(0.5) as f32;
            let height = read_double(stage, path, "height").unwrap_or(1.0) as f32;
            let axis = capsule_axis(stage, path);
            UsdColliderShape::Capsule { radius, height, axis }
        }
        "Cylinder" => {
            let radius = read_double(stage, path, "radius").unwrap_or(1.0) as f32;
            let height = read_double(stage, path, "height").unwrap_or(2.0) as f32;
            let axis = capsule_axis(stage, path);
            UsdColliderShape::Cylinder { radius, height, axis }
        }
        "Mesh" => UsdColliderShape::Mesh,
        "Plane" => UsdColliderShape::Plane,
        // Unknown geom type with CollisionAPI applied. Vendor USDs
        // (Agilebot, Carter) commonly put `PhysicsCollisionAPI` on an
        // Xform/Scope whose Mesh is a child rather than on the Mesh
        // itself. Look for any Mesh in the descendant subtree and
        // treat as a mesh collider in that case; only fall back to a
        // unit cube if the prim subtree truly has no geometry.
        _ => {
            if has_mesh_descendant(stage, path) {
                UsdColliderShape::Mesh
            } else {
                UsdColliderShape::Cube { size: 1.0 }
            }
        }
    }
}

/// Walk the prim subtree looking for any `def Mesh` prim. Used by
/// `collider_shape_from_prim` to recognise Xform-with-CollisionAPI as
/// a mesh collider when the actual geometry sits one level deeper.
fn has_mesh_descendant(stage: &Stage, root: &Path) -> bool {
    let Ok(children) = stage.prim_children(root.clone()) else {
        return false;
    };
    for child_name in children {
        let Ok(child_path) = root.append_path(child_name.as_str()) else {
            continue;
        };
        let type_name = stage
            .field::<String>(child_path.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if type_name == "Mesh" {
            return true;
        }
        if has_mesh_descendant(stage, &child_path) {
            return true;
        }
    }
    false
}

fn capsule_axis(stage: &Stage, path: &Path) -> Vec3 {
    match read_token_attr(stage, path, "axis").as_deref() {
        Some("X") => Vec3::X,
        Some("Y") => Vec3::Y,
        _ => Vec3::Z, // UsdGeom.{Capsule, Cylinder} default
    }
}

// ── primitive reads ─────────────────────────────────────────────────────

fn read_attr(stage: &Stage, prim: &Path, name: &str) -> Option<Value> {
    let attr = prim.append_property(name).ok()?;
    stage.field::<Value>(attr, "default").ok().flatten()
}

fn read_bool(stage: &Stage, prim: &Path, name: &str) -> Option<bool> {
    match read_attr(stage, prim, name)? {
        Value::Bool(b) => Some(b),
        _ => None,
    }
}

fn read_double(stage: &Stage, prim: &Path, name: &str) -> Option<f64> {
    match read_attr(stage, prim, name)? {
        Value::Double(d) => Some(d),
        Value::Float(f) => Some(f as f64),
        _ => None,
    }
}

fn read_vec3f(stage: &Stage, prim: &Path, name: &str) -> Option<[f32; 3]> {
    match read_attr(stage, prim, name)? {
        Value::Vec3f(v) => Some(v),
        Value::Vec3d(v) => Some([v[0] as f32, v[1] as f32, v[2] as f32]),
        _ => None,
    }
}

fn read_token_attr(stage: &Stage, prim: &Path, name: &str) -> Option<String> {
    match read_attr(stage, prim, name)? {
        Value::Token(s) | Value::String(s) => Some(s),
        _ => None,
    }
}

fn read_rel_first(stage: &Stage, prim: &Path, rel_name: &str) -> Option<String> {
    let rel = prim.append_property(rel_name).ok()?;
    let raw = stage.field::<Value>(rel, "targetPaths").ok().flatten()?;
    let paths = match raw {
        Value::PathListOp(op) => op.flatten(),
        Value::PathVec(v) => v,
        _ => return None,
    };
    paths.into_iter().next().map(|p| p.as_str().to_string())
}
