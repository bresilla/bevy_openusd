//! M24 integration test: authored `custom` attributes (including
//! `userProperties:*` namespaces) round-trip into `UsdCustomAttrs`.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};
use usd_schemas::geom::CustomAttrValue;

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
fn reads_custom_attrs_and_filters_schema_ones() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "custom_attrs.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!("\n---- UsdAsset.custom_attrs snapshot ----");
    let mut keys: Vec<_> = asset.custom_attrs.keys().collect();
    keys.sort();
    for k in keys {
        let a = &asset.custom_attrs[k];
        println!("  {k}: {} entries", a.entries.len());
        for (name, val) in &a.entries {
            println!("    {name} = {val:?}");
        }
    }

    // /World/Plain has no authored custom attrs → not in the map.
    assert!(
        !asset.custom_attrs.contains_key("/World/Plain"),
        "Plain cube should NOT have an entry in custom_attrs"
    );

    let robot = asset
        .custom_attrs
        .get("/World/Robot")
        .expect("Robot should have an entry in custom_attrs");

    // Five custom attrs authored; `xformOp:translate` is schema and
    // must not appear.
    assert_eq!(robot.entries.len(), 5);
    assert!(
        robot.get("xformOp:translate").is_none(),
        "schema attributes must not leak into UsdCustomAttrs"
    );

    // Per-attribute typed round-trip.
    assert_eq!(
        robot.get("userProperties:max_speed"),
        Some(&CustomAttrValue::Float(2.5))
    );
    assert_eq!(
        robot.get("userProperties:priority"),
        Some(&CustomAttrValue::Int(7))
    );
    assert_eq!(
        robot.get("userProperties:name"),
        Some(&CustomAttrValue::String("cart_01".to_string()))
    );
    assert_eq!(
        robot.get("userProperties:active"),
        Some(&CustomAttrValue::Bool(true))
    );
    match robot.get("arena:tint") {
        Some(CustomAttrValue::Vec3f(rgb)) => {
            assert!((rgb[0] - 0.85).abs() < 1e-4);
            assert!((rgb[1] - 0.55).abs() < 1e-4);
            assert!((rgb[2] - 0.1).abs() < 1e-4);
        }
        other => panic!("arena:tint should decode as Vec3f, got {other:?}"),
    }
}
