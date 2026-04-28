//! M11 integration test: stage with `BasisCurves` + `Points` prims lands
//! a `Mesh3d` on each, with the right primitive topology + vertex count.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d, PrimitiveTopology};
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
fn projects_basis_curves_and_points() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "curves_points.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut per_path = std::collections::HashMap::new();
    for (prim, mesh3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        per_path.insert(prim.path.clone(), mesh3d.0.clone());
    }

    for expected in ["/World/Polyline", "/World/Curve", "/World/Cloud"] {
        assert!(
            per_path.contains_key(expected),
            "expected Mesh3d on {expected}; got {:?}",
            per_path.keys().collect::<Vec<_>>()
        );
    }

    let meshes = app.world().resource::<Assets<Mesh>>();

    // Linear polyline: 5 CVs × 6 ring segments (default) = 30 tube
    // vertices. Topology is TriangleList.
    let polyline = meshes
        .get(&per_path["/World/Polyline"])
        .expect("polyline mesh missing");
    assert_eq!(
        polyline.primitive_topology(),
        PrimitiveTopology::TriangleList
    );
    assert_eq!(polyline.count_vertices(), 5 * 6);

    // Cubic Bézier 7 CVs → 2 spans → 33 spine samples × 6 ring segments.
    let curve = meshes
        .get(&per_path["/World/Curve"])
        .expect("curve mesh missing");
    assert_eq!(curve.primitive_topology(), PrimitiveTopology::TriangleList);
    assert_eq!(curve.count_vertices(), 33 * 6);

    // Points: 6 authored points expanded to 6 × 8-vertex cubes so they
    // actually render (PointList topology is 1-pixel / invisible in
    // Bevy's default pipeline). Triangles → TriangleList.
    let cloud = meshes
        .get(&per_path["/World/Cloud"])
        .expect("cloud mesh missing");
    assert_eq!(cloud.primitive_topology(), PrimitiveTopology::TriangleList);
    assert_eq!(cloud.count_vertices(), 6 * 8);
}
