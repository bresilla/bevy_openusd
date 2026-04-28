//! M22 integration test: `UsdGeomImageable.visibility` + `UsdModelAPI.kind`.
//!
//! - `Hidden` cube → `Visibility::Hidden` on its entity.
//! - `HiddenParent` Xform → `Visibility::Hidden`; the child cube
//!   inherits hiddenness through Bevy's visibility propagation.
//! - `World` carries `kind = "assembly"`, `Robot` carries
//!   `kind = "component"` — both round-trip into `UsdKind` components.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdKind, UsdPlugin, UsdPrimRef};

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
fn applies_visibility_and_surfaces_kind() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "visibility_kind.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, (Visibility, Option<String>)> =
        std::collections::HashMap::new();
    for (prim, vis, kind) in world
        .query::<(&UsdPrimRef, &Visibility, Option<&UsdKind>)>()
        .iter(world)
    {
        by_path.insert(
            prim.path.clone(),
            (*vis, kind.map(|k| k.kind.clone())),
        );
    }

    println!("\n---- visibility + kind ----");
    let mut entries: Vec<_> = by_path.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (path, (vis, kind)) in &entries {
        println!("  {path:<35}  vis={vis:?}  kind={kind:?}");
    }

    // Visibility assertions.
    assert_eq!(
        by_path["/World/Visible"].0,
        Visibility::Inherited,
        "unauthored visibility should default to Inherited"
    );
    assert_eq!(
        by_path["/World/Hidden"].0,
        Visibility::Hidden,
        "authored invisible should flip to Hidden"
    );
    assert_eq!(
        by_path["/World/HiddenParent"].0,
        Visibility::Hidden,
        "Xform with invisible should be Hidden too"
    );
    // Inheritance: the child Cube keeps Inherited locally but Bevy's
    // propagation will suppress rendering via InheritedVisibility.
    assert_eq!(
        by_path["/World/HiddenParent/InheritsHidden"].0,
        Visibility::Inherited,
        "unauthored child keeps Inherited; Bevy propagation hides it at render time"
    );

    // Kind assertions.
    assert_eq!(by_path["/World"].1.as_deref(), Some("assembly"));
    assert_eq!(by_path["/World/Robot"].1.as_deref(), Some("component"));
    assert!(
        by_path["/World/Visible"].1.is_none(),
        "Visible cube has no authored kind"
    );
}
