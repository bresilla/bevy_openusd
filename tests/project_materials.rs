//! M3 integration test: load a stage with per-prim material bindings and
//! confirm the projection attaches distinct `StandardMaterial` handles with
//! the expected base colours, and that bindings sharing a Material dedup
//! to the same handle.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::Mesh;
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
        .register_type::<bevy::mesh::Mesh3d>()
        .register_type::<bevy::pbr::MeshMaterial3d<StandardMaterial>>();
    app
}

fn load_and_drive(app: &mut App, asset_name: &str) -> Handle<UsdAsset> {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());

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
    for _ in 0..10 {
        app.update();
    }
}

#[test]
fn projects_per_prim_materials() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "materials.usda");
    spawn_scene_root(&mut app, &handle);

    // Every geom should carry a MeshMaterial3d<StandardMaterial>.
    let world = app.world_mut();
    let mut per_path = std::collections::HashMap::new();
    for (prim, mat) in world
        .query::<(&UsdPrimRef, &MeshMaterial3d<StandardMaterial>)>()
        .iter(world)
    {
        per_path.insert(prim.path.clone(), mat.0.clone());
    }

    for expected in [
        "/World/RedBox",
        "/World/GreenBall",
        "/World/GreenBox",
        "/World/EmissiveBall",
    ] {
        assert!(
            per_path.contains_key(expected),
            "expected MeshMaterial3d on {expected}; got {:?}",
            per_path.keys().collect::<Vec<_>>()
        );
    }

    // Two prims bound to /World/Materials/GreenMetal must share a handle.
    assert_eq!(
        per_path["/World/GreenBall"], per_path["/World/GreenBox"],
        "GreenBall + GreenBox should share one StandardMaterial handle"
    );
    // Red vs Green are distinct materials — must *not* share.
    assert_ne!(
        per_path["/World/RedBox"], per_path["/World/GreenBall"],
        "Red + GreenMetal must resolve to distinct handles"
    );

    // Verify base colours survived.
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let red = materials
        .get(&per_path["/World/RedBox"])
        .expect("Red material missing");
    let LinearRgba {
        red: rr,
        green: rg,
        blue: rb,
        ..
    } = red.base_color.into();
    assert!(
        (rr - 0.8).abs() < 1e-5 && (rg - 0.1).abs() < 1e-5 && (rb - 0.1).abs() < 1e-5,
        "RedBox base_color should be (0.8, 0.1, 0.1), got ({rr}, {rg}, {rb})"
    );

    let green = materials
        .get(&per_path["/World/GreenBall"])
        .expect("Green material missing");
    assert!(
        (green.metallic - 0.95).abs() < 1e-4,
        "GreenMetal metallic should be 0.95, got {}",
        green.metallic
    );

    let emissive = materials
        .get(&per_path["/World/EmissiveBall"])
        .expect("Emissive material missing");
    let LinearRgba {
        red: er,
        green: eg,
        blue: eb,
        ..
    } = emissive.emissive;
    assert!(
        (er - 0.1).abs() < 1e-5 && (eg - 0.4).abs() < 1e-5 && (eb - 1.2).abs() < 1e-5,
        "EmissiveBlue emissive should be (0.1, 0.4, 1.2), got ({er}, {eg}, {eb})"
    );
}
