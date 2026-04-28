//! M15 integration test: time-sampled `xformOp:rotateY` → animated prim
//! map on `UsdAsset`. Verify samples decode correctly and the
//! interpolator produces the expected midpoint angles. Static prims stay
//! out of the map.
//!
//! (Vec3-valued timeSamples — `xformOp:translate`, `xformOp:scale`,
//! `xformOp:rotateXYZ` — are blocked on an openusd parser fix. Scalar
//! single-axis rotates go through unchanged.)

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};
use usd_schemas::anim::sample_scalar_concrete;

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
fn collects_animated_rotate_and_interpolates() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "animated_translate.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- timeline ----\n  \
         startTimeCode = {}\n  \
         endTimeCode   = {}\n  \
         fps           = {}\n  \
         animated prim count = {}\n",
        asset.start_time_code,
        asset.end_time_code,
        asset.time_codes_per_second,
        asset.animated_prims.len(),
    );

    assert!((asset.start_time_code - 0.0).abs() < 1e-6);
    assert!((asset.end_time_code - 48.0).abs() < 1e-6);
    assert!((asset.time_codes_per_second - 24.0).abs() < 1e-6);

    // The Static cube should NOT be in the animated_prims map.
    assert!(!asset.animated_prims.contains_key("/World/Static"));
    // The Spinner cube SHOULD be.
    let spinner = asset
        .animated_prims
        .get("/World/Spinner")
        .expect("/World/Spinner should have animated rotateY");
    let track = spinner
        .rotate_y
        .as_ref()
        .expect("Spinner should have rotateY samples");
    let samples = &track.samples;
    assert_eq!(samples.len(), 3);

    // Interpolation midpoints: 0 → 180 → 360 degrees over t=0..48.
    let at_0 = sample_scalar_concrete(samples, 0.0).unwrap();
    let at_12 = sample_scalar_concrete(samples, 12.0).unwrap();
    let at_24 = sample_scalar_concrete(samples, 24.0).unwrap();
    let at_36 = sample_scalar_concrete(samples, 36.0).unwrap();
    let at_48 = sample_scalar_concrete(samples, 48.0).unwrap();

    println!(
        "  rotateY curve (degrees):\n    \
         t=0  → {:>7.2}  (expected   0.00)\n    \
         t=12 → {:>7.2}  (expected  90.00)\n    \
         t=24 → {:>7.2}  (expected 180.00)\n    \
         t=36 → {:>7.2}  (expected 270.00)\n    \
         t=48 → {:>7.2}  (expected 360.00)\n",
        at_0, at_12, at_24, at_36, at_48
    );

    assert!(at_0.abs() < 1e-3);
    assert!((at_12 - 90.0).abs() < 1e-3);
    assert!((at_24 - 180.0).abs() < 1e-3);
    assert!((at_36 - 270.0).abs() < 1e-3);
    assert!((at_48 - 360.0).abs() < 1e-3);
}
