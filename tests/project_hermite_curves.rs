//! HermiteCurves integration test: assert that
//! `usd_schemas::geom::read_hermite_curves` decodes the fixture, that
//! the cubic-Hermite sampler in
//! `bevy_openusd::curves::hermite_to_read_curves` matches each authored
//! CV exactly at segment endpoints (h00(0)=h01(1)=1, all other basis
//! functions vanish), and that the loader spawns one entity per
//! HermiteCurves prim with a Mesh3d + Material attached.

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
fn hermite_sampler_matches_cv_endpoints() {
    use usd_schemas::geom::read_hermite_curves;
    let stage =
        openusd::Stage::open("tests/stages/hermite_curves.usda").expect("stage should open");
    let h = read_hermite_curves(
        &stage,
        &openusd::sdf::Path::new("/World/Bend").expect("valid path"),
    )
    .expect("read ok")
    .expect("hermite should decode");

    println!(
        "\n---- Hermite Bend ----\n  points={:?}\n  tangents={:?}",
        h.points, h.tangents
    );

    let read = bevy_openusd::curves::hermite_to_read_curves(&h);
    let total = read.points.len();
    println!(
        "  sampled vertex_counts={:?} total={total} first={:?} last={:?}",
        read.vertex_counts,
        read.points.first(),
        read.points.last(),
    );

    let near = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) < 1e-4
    };

    let first = read.points.first().expect("samples present");
    let last = read.points.last().expect("samples present");
    assert!(
        near(*first, h.points[0]),
        "first sample {first:?} should equal first CV {:?}",
        h.points[0]
    );
    assert!(
        near(*last, h.points[h.points.len() - 1]),
        "last sample {last:?} should equal last CV {:?}",
        h.points[h.points.len() - 1]
    );

    // The middle CV (peak at index 1) should appear exactly at the
    // sample boundary between the two segments, i.e. at index
    // `HERMITE_SEGMENTS_PER_SPAN = 16`.
    let mid_sample = read.points[16];
    assert!(
        near(mid_sample, h.points[1]),
        "sample 16 {mid_sample:?} should equal middle CV {:?}",
        h.points[1]
    );
}

#[test]
fn loader_spawns_entity_per_hermite_curve() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "hermite_curves.usda");
    spawn_scene_root(&mut app, &handle);
    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        by_path.insert(prim.path.clone(), m3d.0.clone());
    }
    println!("\n---- Hermite load ----\n  mesh-carrying prims:");
    let mut keys: Vec<_> = by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k}");
    }
    assert!(
        by_path.contains_key("/World/Bend"),
        "Bend HermiteCurves prim should have a Mesh3d"
    );
    assert!(
        by_path.contains_key("/World/SCurve"),
        "SCurve HermiteCurves prim should have a Mesh3d"
    );
}
