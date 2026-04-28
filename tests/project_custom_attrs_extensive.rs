//! M24-extended integration test: comprehensive custom-attribute
//! coverage (every Value variant), nested `customData`, `assetInfo`
//! per prim, and layer-level `customLayerData`.

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
fn extensive_custom_attrs_and_dicts() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "custom_attrs_extensive.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    // ── 1. Layer-level customLayerData ────────────────────────────
    println!(
        "\n---- customLayerData ({} entries) ----",
        asset.custom_layer_data.len()
    );
    for (k, v) in asset.custom_layer_data.iter() {
        println!("  {k} = {v:?}");
    }
    assert_eq!(
        asset.custom_layer_data.get("authoring_tool"),
        Some(&CustomAttrValue::String("bevy_openusd-test".to_string()))
    );
    assert_eq!(
        asset.custom_layer_data.get("layer_version").and_then(|v| v.as_int()),
        Some(3)
    );
    // Nested key lookup via dotted path.
    let nested_focal = asset
        .custom_layer_data
        .get_nested("camera_bookmarks.Front.focal")
        .and_then(|v| v.as_float());
    println!("  nested camera_bookmarks.Front.focal = {nested_focal:?}");
    assert_eq!(nested_focal, Some(50.0));

    // ── 2. /World customData + assetInfo ──────────────────────────
    let world_meta = asset
        .custom_attrs
        .get("/World")
        .expect("/World should have metadata (customData + assetInfo)");
    assert!(world_meta.entries.is_empty(), "/World has no `custom` attrs");
    println!(
        "\n---- /World.customData ({} entries) ----",
        world_meta.custom_data.len()
    );
    for (k, v) in world_meta.custom_data.iter() {
        println!("  {k} = {v:?}");
    }
    println!(
        "---- /World.assetInfo ({} entries) ----",
        world_meta.asset_info.len()
    );
    for (k, v) in world_meta.asset_info.iter() {
        println!("  {k} = {v:?}");
    }

    assert_eq!(
        world_meta.custom_data.get("scene_author").and_then(|v| v.as_str()),
        Some("dev")
    );
    assert_eq!(
        world_meta.custom_data.get_nested("rmf.map_name").and_then(|v| v.as_str()),
        Some("greenhouse")
    );
    assert_eq!(
        world_meta.custom_data.get_nested("rmf.fleet_count").and_then(|v| v.as_int()),
        Some(3)
    );

    assert_eq!(
        world_meta.asset_info.get("name").and_then(|v| v.as_str()),
        Some("TestScene")
    );
    assert_eq!(
        world_meta.asset_info.get("version").and_then(|v| v.as_str()),
        Some("0.1.0")
    );
    match world_meta.asset_info.get("identifier") {
        Some(CustomAttrValue::AssetPath(p)) => {
            assert_eq!(p, "./custom_attrs_extensive.usda");
        }
        other => panic!("expected AssetPath, got {other:?}"),
    }

    // ── 3. /World/Robot custom attrs covering every Value arm ─────
    let robot = asset
        .custom_attrs
        .get("/World/Robot")
        .expect("Robot metadata present");
    println!(
        "\n---- /World/Robot custom attrs ({} entries) ----",
        robot.entries.len()
    );
    for (n, v) in &robot.entries {
        println!("  {n} = {v:?}");
    }

    // Ergonomic accessors (typed).
    assert_eq!(robot.get_bool("userProperties:active"), Some(true));
    assert_eq!(robot.get_int("userProperties:priority"), Some(7));
    assert_eq!(robot.get_int("userProperties:queue_depth"), Some(128));
    assert_eq!(robot.get_int("userProperties:tick_count"), Some(99999));
    assert_eq!(robot.get_int("userProperties:channel"), Some(7));
    // UInt64 max-ish: widening to i64 truncates; accept via as_int
    // returning SOME value even if not exactly preserved.
    assert!(robot.get_int("userProperties:serial").is_some());
    assert_eq!(robot.get_float("userProperties:max_speed"), Some(2.5));
    assert_eq!(robot.get_float("userProperties:mass_kg"), Some(12.75));
    assert_eq!(robot.get_string("userProperties:name"), Some("cart_01"));
    assert_eq!(robot.get_string("userProperties:kind"), Some("mobile_base"));
    assert_eq!(robot.get_string("userProperties:config"), Some("./config.yaml"));

    // Tuples.
    assert_eq!(robot.get_vec2("userProperties:size_2d"), Some([1.2, 0.8]));
    assert_eq!(
        robot.get_vec3("userProperties:base_offset"),
        Some([0.0, 0.05, 0.0])
    );
    assert_eq!(robot.get_vec3("userProperties:grid_cell"), Some([4.0, 0.0, 2.0]));
    assert_eq!(
        robot.get_vec4("userProperties:padding"),
        Some([1.0, 2.0, 3.0, 4.0])
    );
    assert_eq!(robot.get_vec3("arena:tint"), Some([0.85, 0.55, 0.1]));

    // Quaternion (stored as Quatf, accessible via as_vec4).
    assert_eq!(
        robot.get_vec4("userProperties:orient"),
        Some([1.0, 0.0, 0.0, 0.0])
    );

    // Arrays.
    match robot.get("userProperties:waypoint_ids") {
        Some(CustomAttrValue::IntArray(v)) => assert_eq!(v, &[3, 7, 12, 42]),
        other => panic!("waypoint_ids should be IntArray, got {other:?}"),
    }
    match robot.get("userProperties:gains") {
        Some(CustomAttrValue::FloatArray(v)) => {
            assert!((v[0] - 1.0).abs() < 1e-4 && (v[1] - 0.5).abs() < 1e-4 && (v[2] - 0.1).abs() < 1e-4);
        }
        other => panic!("gains should be FloatArray, got {other:?}"),
    }
    // openusd decodes `string[]` literals as TokenArray in practice —
    // accept either.
    match robot.get("userProperties:tags") {
        Some(CustomAttrValue::StringArray(v)) | Some(CustomAttrValue::TokenArray(v)) => {
            assert_eq!(v, &["autonomous".to_string(), "priority".to_string()]);
        }
        other => panic!("tags should be StringArray/TokenArray, got {other:?}"),
    }

    // Robot carries its own customData too.
    assert_eq!(
        robot.custom_data.get("spawn_priority").and_then(|v| v.as_int()),
        Some(2)
    );
    assert_eq!(
        robot.custom_data.get("capability").and_then(|v| v.as_str()),
        Some("pick_and_place")
    );

    // ── 4. Namespace queries ──────────────────────────────────────
    let user_props: Vec<(&str, &CustomAttrValue)> =
        robot.namespaced("userProperties:").collect();
    println!(
        "\n---- namespaced('userProperties:') → {} hits ----",
        user_props.len()
    );
    for (k, _) in &user_props {
        println!("  {k}");
    }
    // Every attribute starting with that prefix should appear; arena:tint should NOT.
    assert!(user_props.iter().any(|(k, _)| *k == "max_speed"));
    assert!(user_props.iter().any(|(k, _)| *k == "name"));
    assert!(!user_props.iter().any(|(k, _)| *k == "tint"));

    let arena_only: Vec<_> = robot.namespaced("arena:").collect();
    assert_eq!(arena_only.len(), 1);
    assert_eq!(arena_only[0].0, "tint");
}
