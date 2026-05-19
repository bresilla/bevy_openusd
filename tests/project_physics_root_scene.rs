//! Regression test for the Isaac-Sim-style "PhysicsScene at root,
//! defaultPrim is the robot subtree" pattern. Pre-fix our loader
//! only walked defaultPrim → root-level PhysicsScene was silently
//! dropped → no UsdPhysicsScene marker → adapters had no gravity
//! source. The Agilebot GBT-C5A asset triggers this exact case.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPhysicsScene, UsdPlugin, UsdPrimRef, UsdRigidBody};

fn build_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: "tests/stages".into(),
            ..Default::default()
        })
        .init_asset::<Scene>()
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(UsdPlugin)
        .register_type::<Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>();
    app
}

fn load_and_spawn(app: &mut App, asset_name: &str) {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());
    for _ in 0..200 {
        app.update();
        if matches!(
            app.world()
                .resource::<AssetServer>()
                .get_load_state(&handle),
            Some(LoadState::Loaded)
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let scene_handle = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .scene
        .clone();
    app.world_mut().spawn(SceneRoot(scene_handle));
    for _ in 0..10 {
        app.update();
    }
}

#[test]
fn root_level_physics_scene_survives_default_prim_walk() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_root_scene.usda");
    let world = app.world_mut();

    // PhysicsScene at /physicsScene — peer of defaultPrim Robot. Must
    // become an entity even though our walker is rooted at /Robot.
    let mut q_scene = world.query::<(Entity, &UsdPrimRef, &UsdPhysicsScene)>();
    let scenes: Vec<_> = q_scene
        .iter(world)
        .filter(|(_, p, _)| p.path == "/physicsScene")
        .collect();
    assert_eq!(
        scenes.len(),
        1,
        "expected 1 UsdPhysicsScene at /physicsScene; got {}",
        scenes.len()
    );

    // The Robot subtree's body still works.
    let mut q_rb = world.query::<(&UsdPrimRef, &UsdRigidBody)>();
    let bodies: Vec<_> = q_rb
        .iter(world)
        .filter(|(p, _)| p.path == "/Robot/Body")
        .collect();
    assert_eq!(
        bodies.len(),
        1,
        "Body in defaultPrim subtree should still load"
    );

    println!("root-physics fix OK: /physicsScene survived defaultPrim-only walk");
}
