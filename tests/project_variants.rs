//! M13 integration test: authored `variants = { look = "red" }` on the
//! fixture, then a second load that overrides the selection to "blue"
//! via a session layer. The Cube's bound material should flip from
//! Red to Blue.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::color::LinearRgba;
use bevy::mesh::Mesh;
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdLoaderSettings, UsdPlugin, UsdPrimRef, VariantSelection};

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
        .register_type::<bevy::mesh::Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>();
    app
}

fn load_with(
    app: &mut App,
    asset_name: &str,
    selections: Vec<VariantSelection>,
) -> Handle<UsdAsset> {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load_with_settings::<UsdAsset, _>(asset_name.to_string(), move |s: &mut UsdLoaderSettings| {
            s.variant_selections = selections.clone();
        });
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

fn box_base_color(app: &mut App) -> [f32; 3] {
    let world = app.world_mut();
    let mat_handle = world
        .query::<(&UsdPrimRef, &MeshMaterial3d<StandardMaterial>)>()
        .iter(world)
        .find(|(pr, _)| pr.path == "/Root/Box")
        .map(|(_, m)| m.0.clone())
        .expect("/Root/Box should carry a MeshMaterial3d");
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let mat = materials.get(&mat_handle).expect("material missing");
    let LinearRgba {
        red, green, blue, ..
    } = mat.base_color.into();
    [red, green, blue]
}

#[test]
fn default_selection_binds_red() {
    let mut app = build_test_app();
    let handle = load_with(&mut app, "variants.usda", Vec::new());
    spawn_scene_root(&mut app, &handle);

    // Inspect what the loader discovered on the stage.
    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();
    let sets = asset
        .variants
        .get("/Root")
        .expect("/Root should author a variant set");
    println!(
        "\n---- DEFAULT LOAD (no override) ----\n\
         authored variant sets on /Root: {} set(s)\n  \
         name={:?} authored_selection={:?} options={:?}",
        sets.len(),
        sets[0].name,
        sets[0].selection,
        sets[0].options,
    );

    // Authored default is `look = "red"` → diffuseColor (0.9, 0.1, 0.1).
    let [r, g, b] = box_base_color(&mut app);
    println!(
        "/Root/Box bound material base_color = ({r:.3}, {g:.3}, {b:.3})  \
         → expected Red (0.900, 0.100, 0.100)\n"
    );
    assert!(
        (r - 0.9).abs() < 1e-4 && (g - 0.1).abs() < 1e-4 && (b - 0.1).abs() < 1e-4,
        "default variant should bind Red, got ({r}, {g}, {b})"
    );

    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].name, "look");
    assert_eq!(sets[0].selection.as_deref(), Some("red"));
    let mut opts = sets[0].options.clone();
    opts.sort();
    assert_eq!(opts, vec!["blue".to_string(), "red".to_string()]);
}

#[test]
fn override_selection_binds_blue() {
    let mut app = build_test_app();
    let selections = vec![VariantSelection {
        prim_path: "/Root".to_string(),
        set_name: "look".to_string(),
        option: "blue".to_string(),
    }];
    println!(
        "\n---- OVERRIDE LOAD ({} selection) ----\n  \
         {} :: variantSet {:?} = {:?}",
        selections.len(),
        selections[0].prim_path,
        selections[0].set_name,
        selections[0].option,
    );
    println!(
        "\nsession layer USDA the loader generates:\n\
         ────────────────────────────────────────\n\
         {}────────────────────────────────────────",
        bevy_openusd::author_variant_session_layer(&selections)
    );
    let handle = load_with(&mut app, "variants.usda", selections);
    spawn_scene_root(&mut app, &handle);

    // Session-layer override → Blue material (0.1, 0.2, 0.9).
    let [r, g, b] = box_base_color(&mut app);
    println!(
        "/Root/Box bound material base_color = ({r:.3}, {g:.3}, {b:.3})  \
         → expected Blue (0.100, 0.200, 0.900)\n"
    );
    assert!(
        (r - 0.1).abs() < 1e-4 && (g - 0.2).abs() < 1e-4 && (b - 0.9).abs() < 1e-4,
        "override should bind Blue, got ({r}, {g}, {b})"
    );
}
