//! M19 integration test: `UsdRender.*` read side. Fixture authors a
//! RenderSettings pointing at a RenderProduct which orders two
//! RenderVars. Verify all three arrays round-trip onto `UsdAsset`.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};

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
fn reads_render_settings_product_and_vars() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "render_settings.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- UsdRender read ----\n  \
         RenderSettings = {} \n  \
         RenderProducts = {} \n  \
         RenderVars     = {}\n",
        asset.render_settings.len(),
        asset.render_products.len(),
        asset.render_vars.len(),
    );

    assert_eq!(asset.render_settings.len(), 1);
    assert_eq!(asset.render_products.len(), 1);
    assert_eq!(asset.render_vars.len(), 2);

    let s = &asset.render_settings[0];
    println!(
        "  Settings {} → resolution={:?} pixelAspect={:?} policy={:?} products={:?} purposes={:?}",
        s.path,
        s.resolution,
        s.pixel_aspect_ratio,
        s.aspect_ratio_conform_policy,
        s.products,
        s.included_purposes,
    );
    assert_eq!(s.path, "/Render/Primary");
    assert_eq!(s.resolution, Some([1920, 1080]));
    assert_eq!(s.pixel_aspect_ratio, Some(1.0));
    assert_eq!(s.aspect_ratio_conform_policy.as_deref(), Some("expandAperture"));
    assert_eq!(s.products, vec!["/Render/Products/Beauty".to_string()]);
    assert_eq!(
        s.included_purposes,
        vec!["default".to_string(), "render".to_string()]
    );

    let p = &asset.render_products[0];
    println!(
        "  Product {} → type={:?} name={:?} orderedVars={:?}",
        p.path, p.product_type, p.product_name, p.ordered_vars
    );
    assert_eq!(p.path, "/Render/Products/Beauty");
    assert_eq!(p.product_type.as_deref(), Some("raster"));
    assert_eq!(p.product_name.as_deref(), Some("beauty.0001.exr"));
    assert_eq!(
        p.ordered_vars,
        vec![
            "/Render/Vars/Color".to_string(),
            "/Render/Vars/Depth".to_string(),
        ]
    );

    let mut by_path: std::collections::HashMap<&str, &_> = std::collections::HashMap::new();
    for v in &asset.render_vars {
        by_path.insert(v.path.as_str(), v);
    }
    let color = by_path
        .get("/Render/Vars/Color")
        .expect("Color var missing");
    let depth = by_path
        .get("/Render/Vars/Depth")
        .expect("Depth var missing");
    println!(
        "  Var Color → dataType={:?} sourceName={:?} sourceType={:?}",
        color.data_type, color.source_name, color.source_type
    );
    println!(
        "  Var Depth → dataType={:?} sourceName={:?} sourceType={:?}",
        depth.data_type, depth.source_name, depth.source_type
    );
    assert_eq!(color.data_type.as_deref(), Some("color3f"));
    assert_eq!(color.source_name.as_deref(), Some("Ci"));
    assert_eq!(depth.data_type.as_deref(), Some("float"));
    assert_eq!(depth.source_name.as_deref(), Some("a"));
}
