//! M25 integration test: `UsdGeomMesh.subdivisionScheme` round-trips
//! into ReadMesh, and `UsdAsset.subdivision_prims` lists the subset
//! that asks for subdivision.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};
use usd_schemas::geom::SubdivScheme;

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
fn reads_subdivision_scheme_and_tallies_subsurface_prims() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "subdivision.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!("\n---- subdivision_prims ({} entries) ----", asset.subdivision_prims.len());
    for (path, scheme) in &asset.subdivision_prims {
        println!("  {path} → {scheme:?}");
    }

    assert_eq!(asset.subdivision_prims.len(), 2);
    let mut by_path: std::collections::HashMap<&str, SubdivScheme> =
        asset.subdivision_prims.iter().map(|(p, s)| (p.as_str(), *s)).collect();

    assert_eq!(by_path.remove("/World/CatmullClark"), Some(SubdivScheme::CatmullClark));
    assert_eq!(by_path.remove("/World/LoopTri"), Some(SubdivScheme::Loop));
    assert!(by_path.is_empty());

    // `Flat` authored scheme=none and `Unauthored` left it blank —
    // neither shows up in the tally.
    for (path, _) in &asset.subdivision_prims {
        assert_ne!(path, "/World/Flat");
        assert_ne!(path, "/World/Unauthored");
    }

    // Direct reader check.
    let stage = openusd::Stage::open("tests/stages/subdivision.usda").unwrap();
    use openusd::sdf::Path;
    let flat = usd_schemas::geom::read_mesh(&stage, &Path::new("/World/Flat").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(flat.subdivision_scheme, SubdivScheme::None);
    let cc = usd_schemas::geom::read_mesh(&stage, &Path::new("/World/CatmullClark").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(cc.subdivision_scheme, SubdivScheme::CatmullClark);
    let un = usd_schemas::geom::read_mesh(&stage, &Path::new("/World/Unauthored").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(un.subdivision_scheme, SubdivScheme::None);
}
