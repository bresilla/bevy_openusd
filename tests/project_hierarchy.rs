//! M1 integration test: load a three-prim stage, spawn the projected scene,
//! and verify the resulting entity hierarchy matches the composed tree.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::ecs::hierarchy::Children;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin, UsdPrimRef};
use std::collections::HashMap;

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
        .add_plugins(UsdPlugin);
    app
}

fn load_and_drive(app: &mut App, asset_name: &str) -> Handle<UsdAsset> {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());

    // Wait for the asset to resolve.
    for _ in 0..200 {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.get_load_state(&handle) {
            Some(LoadState::Loaded) => return handle,
            Some(LoadState::Failed(err)) => panic!("UsdAsset load failed: {err}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    panic!("UsdAsset did not load in time");
}

fn spawn_scene_root(app: &mut App, handle: &Handle<UsdAsset>) {
    let scene_handle = {
        let assets = app.world().resource::<Assets<UsdAsset>>();
        assets.get(handle).expect("asset missing").scene.clone()
    };
    app.world_mut().spawn(SceneRoot(scene_handle));
    // Give the scene spawner a few ticks to instantiate entities.
    for _ in 0..10 {
        app.update();
    }
}

#[test]
fn projects_three_prim_hierarchy() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "hierarchy.usda");

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("UsdAsset missing")
        .clone();
    assert_eq!(asset.default_prim.as_deref(), Some("World"));
    assert!(asset.layer_count >= 1);

    spawn_scene_root(&mut app, &handle);

    // Collect every entity with a UsdPrimRef and key by path.
    let mut by_path: HashMap<String, Entity> = HashMap::new();
    for (entity, prim_ref) in app
        .world_mut()
        .query::<(Entity, &UsdPrimRef)>()
        .iter(app.world())
    {
        by_path.insert(prim_ref.path.clone(), entity);
    }

    assert!(
        by_path.contains_key("/World"),
        "expected /World; got {:?}",
        by_path.keys().collect::<Vec<_>>()
    );
    assert!(by_path.contains_key("/World/ChildA"));
    assert!(by_path.contains_key("/World/ChildB"));
    assert_eq!(
        by_path.len(),
        3,
        "three prims expected, got {}",
        by_path.len()
    );

    // /World has two children, /World/ChildA + /World/ChildB are leaves.
    let world_entity = by_path["/World"];
    let world_children = app.world().get::<Children>(world_entity);
    let world_child_count = world_children.map(|c| c.len()).unwrap_or(0);
    assert_eq!(
        world_child_count, 2,
        "/World should have 2 children, has {world_child_count}"
    );

    // Sanity: names match the prim leaf.
    let a_name = app
        .world()
        .get::<Name>(by_path["/World/ChildA"])
        .expect("Name missing on ChildA");
    assert_eq!(a_name.as_str(), "ChildA");
}
