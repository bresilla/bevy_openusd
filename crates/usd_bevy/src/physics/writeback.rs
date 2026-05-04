//! Copy Rapier rigid-body world poses back into Bevy `Transform`
//! after each physics step. Runs in `PostUpdate` so subsequent
//! transform-propagation runs see the updated values.
//!
//! Rapier writes WORLD-space pose for each body. Bevy entities have
//! local transforms (relative to parent). For bodies whose entity
//! has a parent, we factor out the parent's GlobalTransform so the
//! local Transform we write produces the right world position.

use bevy::prelude::*;

use super::convert::{quat_from_d, vec3_from_d};
use super::world::PhysicsWorld;

pub fn writeback_transforms(
    world: Res<PhysicsWorld>,
    mut q_targets: Query<(Entity, &mut Transform, Option<&ChildOf>)>,
    q_parent_gt: Query<&GlobalTransform>,
) {
    for (entity, mut tr, parent) in &mut q_targets {
        let Some(handle) = world.entity_to_body.get(&entity).copied() else {
            continue;
        };
        let Some(rb) = world.bodies.get(handle) else {
            continue;
        };
        let pose = rb.position();
        let world_translation = vec3_from_d(pose.translation);
        let world_rotation = quat_from_d(pose.rotation);

        if let Some(parent_link) = parent {
            if let Ok(parent_gt) = q_parent_gt.get(parent_link.parent()) {
                let parent_iso = parent_gt.compute_transform();
                let inv_rot = parent_iso.rotation.inverse();
                // Factor out parent's world scale too — UsdRoot
                // applies `metersPerUnit` (typically 0.01 on Isaac
                // assets) as a scale, and forgetting to divide here
                // collapses every body toward origin frame-by-frame
                // because Bevy then re-multiplies local × parent.scale.
                let local_delta = inv_rot * (world_translation - parent_iso.translation);
                tr.translation = Vec3::new(
                    local_delta.x / parent_iso.scale.x,
                    local_delta.y / parent_iso.scale.y,
                    local_delta.z / parent_iso.scale.z,
                );
                tr.rotation = inv_rot * world_rotation;
                continue;
            }
        }
        tr.translation = world_translation;
        tr.rotation = world_rotation;
    }
}
