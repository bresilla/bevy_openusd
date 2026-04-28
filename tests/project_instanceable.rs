//! M14 integration test: instance prim detection + prototype dedup.
//!
//! Two `instanceable = true` Xforms with identical subtrees should
//! surface as `instance_prim_count = 2, instance_prototype_reuses = 1`.
//! The Cubes inside them share the mesh cache, so both instance-site
//! entities carry the same `Handle<Mesh>`.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin, UsdPrimRef};

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

fn load_and_drive(app: &mut App, asset_name: &str) -> Handle<UsdAsset> {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());

    for _ in 0..200 {
        app.update();
        match app
            .world()
            .resource::<AssetServer>()
            .get_load_state(&handle)
        {
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
    for _ in 0..10 {
        app.update();
    }
}

#[test]
fn counts_instance_prims_and_detects_prototype_reuse() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "instanceable.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- instanceable stats ----\n  \
         instance_prim_count     = {} (expected 2)\n  \
         instance_prototype_reuses = {} (expected 1)\n",
        asset.instance_prim_count, asset.instance_prototype_reuses
    );

    assert_eq!(
        asset.instance_prim_count, 2,
        "expected 2 instanceable=true prims, got {}",
        asset.instance_prim_count
    );
    assert_eq!(
        asset.instance_prototype_reuses, 1,
        "InstanceA + InstanceB have identical subtrees → InstanceB should reuse InstanceA's fingerprint, got {}",
        asset.instance_prototype_reuses
    );

    // The two cubes must also share the same Mesh handle (content-based
    // dedup). The Sphere gets its own handle because it's a different
    // primitive type.
    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        by_path.insert(prim.path.clone(), m3d.0.clone());
    }

    let a_handle = by_path
        .get("/World/InstanceA/Body")
        .expect("InstanceA/Body Mesh3d missing");
    let b_handle = by_path
        .get("/World/InstanceB/Body")
        .expect("InstanceB/Body Mesh3d missing");
    assert_eq!(
        a_handle, b_handle,
        "instance bodies should share a Mesh handle via content-hash dedup"
    );
    println!(
        "  /World/InstanceA/Body and /World/InstanceB/Body share Handle<Mesh>: true"
    );

    let unique_handle = by_path
        .get("/World/Unique/Body")
        .expect("Unique/Body Mesh3d missing");
    assert_ne!(
        a_handle, unique_handle,
        "a Sphere shouldn't collide with the Cube prototype"
    );
}
