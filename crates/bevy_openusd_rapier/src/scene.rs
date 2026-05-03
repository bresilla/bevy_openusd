//! `UsdPhysicsScene` → `PhysicsWorld.gravity`. First scene wins;
//! subsequent ones warn (Rapier currently runs one world).

use bevy::math::DVec3;
use bevy::prelude::*;
use usd_physics_markers::UsdPhysicsScene;

use crate::world::PhysicsWorld;

pub fn sync_gravity_from_usd_scene(
    scenes: Query<&UsdPhysicsScene, Added<UsdPhysicsScene>>,
    mut world: ResMut<PhysicsWorld>,
    mut applied: Local<bool>,
) {
    for scene in &scenes {
        if *applied {
            warn!(
                "RapierAdapter: multiple UsdPhysicsScene prims; ignoring extras (gravity already applied)"
            );
            continue;
        }
        let g = scene.gravity_direction.normalize_or_zero() * scene.gravity_magnitude;
        world.gravity = DVec3::new(g.x as f64, g.y as f64, g.z as f64);
        info!("RapierAdapter: gravity set to {:?} m/s² (from UsdPhysicsScene)", world.gravity);
        *applied = true;
    }
}
