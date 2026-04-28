//! NURBS curves integration test: assert that
//! `usd_schemas::geom::read_nurbs_curves` decodes the fixture, that
//! the De Boor sampler in `bevy_openusd::curves::nurbs_to_read_curves`
//! produces a polyline whose endpoints match the first / last CVs
//! (end-clamped property), and that the loader spawns one entity
//! per NurbsCurves prim with a Mesh3d + Material attached.

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
fn de_boor_endpoints_match_clamped_cvs() {
    use usd_schemas::geom::read_nurbs_curves;

    let stage = openusd::Stage::open("tests/stages/nurbs_curves.usda")
        .expect("stage should open");

    let nurbs = read_nurbs_curves(
        &stage,
        &openusd::sdf::Path::new("/World/Cubic").expect("valid path"),
    )
    .expect("read ok")
    .expect("cubic should decode");

    println!(
        "\n---- Cubic NURBS ----\n  \
         points={} order={:?} knots={:?} ranges={:?}",
        nurbs.points.len(),
        nurbs.order,
        nurbs.knots,
        nurbs.ranges,
    );

    let read = bevy_openusd::curves::nurbs_to_read_curves(&nurbs);
    println!(
        "  sampled vertex_counts={:?} (first sample {:?}, last {:?})",
        read.vertex_counts,
        read.points.first(),
        read.points.last(),
    );

    // End-clamped knot vector → curve passes exactly through the
    // first and last CVs at u = umin and u ≈ umax.
    let first = read.points.first().expect("samples present");
    let last = read.points.last().expect("samples present");
    let cv_first = nurbs.points[0];
    let cv_last = nurbs.points[nurbs.points.len() - 1];
    let near = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) < 1e-2
    };
    assert!(
        near(*first, cv_first),
        "first sample {first:?} should match first CV {cv_first:?}"
    );
    assert!(
        near(*last, cv_last),
        "last sample {last:?} should match last CV {cv_last:?}"
    );
}

#[test]
fn loader_spawns_entity_per_nurbs_curve() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "nurbs_curves.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        by_path.insert(prim.path.clone(), m3d.0.clone());
    }
    println!("\n---- NURBS load ----\n  mesh-carrying prims:");
    let mut keys: Vec<_> = by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k}");
    }

    assert!(
        by_path.contains_key("/World/Cubic"),
        "Cubic NurbsCurves prim should have a Mesh3d"
    );
    assert!(
        by_path.contains_key("/World/Quadratic"),
        "Quadratic NurbsCurves prim should have a Mesh3d"
    );
}
