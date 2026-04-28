//! M2 integration test: load a stage with one of each UsdGeom primitive
//! plus a tiny Mesh, and assert the projection attaches a `Mesh3d` per geom
//! prim with the expected Bevy shape.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::Mesh;
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
        // Our projected scenes insert these components, so the scene spawner
        // needs them registered — `MinimalPlugins` + `reflect_auto_register`
        // don't wire them up, so do it explicitly.
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
fn projects_primitives_with_meshes() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "primitives.usda");
    spawn_scene_root(&mut app, &handle);

    // Every geom prim should carry a Mesh3d. Tally them by expected path.
    use bevy::mesh::Mesh3d;
    let world = app.world_mut();
    let mut per_path = std::collections::HashMap::new();
    for (prim, mesh3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        per_path.insert(prim.path.clone(), mesh3d.0.clone());
    }

    for expected in [
        "/World/Box",
        "/World/Ball",
        "/World/Pipe",
        "/World/Cap",
        "/World/Tri",
    ] {
        assert!(
            per_path.contains_key(expected),
            "expected Mesh3d on {expected}; got {:?}",
            per_path.keys().collect::<Vec<_>>()
        );
    }
    // /World itself is an Xform with no geometry — must *not* have Mesh3d.
    assert!(
        !per_path.contains_key("/World"),
        "Xform prim should not carry Mesh3d"
    );

    // Sanity-check the triangle mesh has the expected vertex count.
    let tri_handle = per_path.get("/World/Tri").unwrap();
    let meshes = app.world().resource::<Assets<Mesh>>();
    let tri = meshes.get(tri_handle).expect("Tri mesh not in Assets");
    assert_eq!(
        tri.count_vertices(),
        3,
        "Tri should be a single triangle (3 vertices)"
    );
}
