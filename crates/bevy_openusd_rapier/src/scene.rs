//! `UsdPhysicsScene` → `RapierConfiguration` gravity sync, plus
//! a higher-fidelity collider-shape subdivision setting.

use bevy::prelude::*;
use usd_physics_markers::UsdPhysicsScene;
use bevy_rapier3d::plugin::{DefaultRapierContext, RapierConfiguration};

/// Bump bevy_rapier's `scaled_shape_subdivision` from its default 10
/// to 32 once the Rapier configuration appears. Cylinders / cones with
/// a non-uniform `GlobalTransform.scale` (typical of robotics wheels:
/// `(D, D, w)` for diameter D and tread width w) get approximated as
/// convex polyhedra with this many sides — 10 looks visibly faceted on
/// a flat tyre, 32 reads as smooth.
pub fn raise_scaled_shape_subdivision(
    mut config: Query<&mut RapierConfiguration, (With<DefaultRapierContext>, Added<RapierConfiguration>)>,
) {
    if let Ok(mut cfg) = config.single_mut() {
        cfg.scaled_shape_subdivision = 32;
    }
}

/// First `UsdPhysicsScene` we see wins — its gravity vector and
/// magnitude get pushed into the Rapier world's gravity. Subsequent
/// scenes log a warning (Rapier currently runs one world; per-scene
/// `simulationOwner` routing is a follow-up).
pub fn sync_gravity_from_usd_scene(
    scenes: Query<&UsdPhysicsScene, Added<UsdPhysicsScene>>,
    mut config: Query<&mut RapierConfiguration, With<DefaultRapierContext>>,
) {
    let mut applied = false;
    for scene in &scenes {
        if applied {
            warn!(
                "RapierAdapter: multiple UsdPhysicsScene prims; ignoring extras (gravity already applied)"
            );
            continue;
        }
        let Ok(mut cfg) = config.single_mut() else {
            return;
        };
        cfg.gravity = scene.gravity_direction.normalize_or_zero() * scene.gravity_magnitude;
        info!(
            "RapierAdapter: gravity set to {:?} m/s² (from UsdPhysicsScene)",
            cfg.gravity
        );
        applied = true;
    }
}
