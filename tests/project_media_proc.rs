//! UsdMedia.SpatialAudio + UsdProc integration test. Asserts that
//! `usd_schemas::media::read_spatial_audio` and
//! `usd_schemas::proc::read_procedural` decode their fixtures, and
//! that the loader attaches `UsdSpatialAudio` / `UsdProcedural`
//! components to the right entities (and only the right entities).

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin, UsdPrimRef, UsdProcedural, UsdSpatialAudio};

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
fn schema_readers_decode_authored_attrs() {
    let stage =
        openusd::Stage::open("tests/stages/media_proc.usda").expect("stage should open");

    let bell = usd_schemas::media::read_spatial_audio(
        &stage,
        &openusd::sdf::Path::new("/World/Bell").expect("valid path"),
    )
    .expect("read ok")
    .expect("Bell should decode");
    println!("\n---- SpatialAudio Bell ----\n  {bell:?}");
    assert_eq!(bell.file_path.as_deref(), Some("sounds/bell.wav"));
    assert_eq!(bell.aural_mode.as_deref(), Some("spatial"));
    assert_eq!(bell.playback_mode.as_deref(), Some("loopFromStart"));
    assert_eq!(bell.gain, Some(0.8));

    let plain = usd_schemas::media::read_spatial_audio(
        &stage,
        &openusd::sdf::Path::new("/World/Plain").expect("valid path"),
    )
    .expect("read ok");
    assert!(plain.is_none());

    let forest = usd_schemas::proc::read_procedural(
        &stage,
        &openusd::sdf::Path::new("/World/Forest").expect("valid path"),
    )
    .expect("read ok")
    .expect("Forest should decode");
    println!("---- Procedural Forest ----\n  {forest:?}");
    assert_eq!(
        forest.procedural_type.as_deref(),
        Some("HoudiniProcedural")
    );
    assert_eq!(forest.procedural_system.as_deref(), Some("houdini"));
}

#[test]
fn loader_attaches_components_to_right_prims() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "media_proc.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();

    let mut audio_paths: Vec<String> = world
        .query::<(&UsdPrimRef, &UsdSpatialAudio)>()
        .iter(world)
        .map(|(p, _)| p.path.clone())
        .collect();
    audio_paths.sort();

    let mut proc_paths: Vec<String> = world
        .query::<(&UsdPrimRef, &UsdProcedural)>()
        .iter(world)
        .map(|(p, _)| p.path.clone())
        .collect();
    proc_paths.sort();

    println!(
        "\n---- Component attach ----\n  audio: {audio_paths:?}\n  procedural: {proc_paths:?}"
    );

    assert_eq!(audio_paths, vec!["/World/Bell", "/World/Click"]);
    assert_eq!(proc_paths, vec!["/World/Forest"]);

    // Plain cube has neither tag.
    let plain_audio: Vec<_> = world
        .query::<(&UsdPrimRef, &UsdSpatialAudio)>()
        .iter(world)
        .filter(|(p, _)| p.path == "/World/Plain")
        .collect();
    assert!(plain_audio.is_empty());
}
