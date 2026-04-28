//! UsdUI integration test: assert that authored `ui:displayName`
//! tokens are read by `usd_schemas::ui::read_display_name` and that
//! the loader attaches a `UsdDisplayName` component to the prim
//! entity. Prims without an authored display name don't get the
//! component (so the tree falls back to the prim leaf name).

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdDisplayName, UsdPlugin, UsdPrimRef};

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
fn schema_reader_decodes_display_name() {
    let stage = openusd::Stage::open("tests/stages/usdui.usda").expect("stage should open");
    let dn = usd_schemas::ui::read_display_name(
        &stage,
        &openusd::sdf::Path::new("/World/robot_v3_baseLink_geom_0").expect("valid path"),
    )
    .expect("read ok");
    println!("\n---- ui:displayName ----\n  {:?}", dn);
    assert_eq!(dn.as_deref(), Some("Main Body"));

    let dg = usd_schemas::ui::read_display_group(
        &stage,
        &openusd::sdf::Path::new("/World/robot_v3_baseLink_geom_0").expect("valid path"),
    )
    .expect("read ok");
    assert_eq!(dg.as_deref(), Some("robot"));

    let no_dn = usd_schemas::ui::read_display_name(
        &stage,
        &openusd::sdf::Path::new("/World/Plain").expect("valid path"),
    )
    .expect("read ok");
    assert!(no_dn.is_none());
}

#[test]
fn loader_attaches_display_name_component() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "usdui.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for (prim, dn) in world
        .query::<(&UsdPrimRef, Option<&UsdDisplayName>)>()
        .iter(world)
    {
        by_path.insert(prim.path.clone(), dn.map(|d| d.0.clone()));
    }
    println!("\n---- UsdUI load ----\n  prim → display name:");
    let mut keys: Vec<_> = by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k} -> {:?}", by_path[*k]);
    }

    assert_eq!(
        by_path.get("/World").and_then(|o| o.as_deref()),
        Some("Top of the world")
    );
    assert_eq!(
        by_path
            .get("/World/robot_v3_baseLink_geom_0")
            .and_then(|o| o.as_deref()),
        Some("Main Body")
    );
    assert_eq!(by_path.get("/World/Plain").and_then(|o| o.as_deref()), None);
}
