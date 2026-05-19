//! Regression test for the proxy-purpose collision drop. Production
//! USD assets author colliders as `purpose = "proxy"` prims under the
//! same parent as the visual mesh — the loader used to silently
//! discard those prims, losing both geometry and PhysicsCollisionAPI
//! opinions. This fixture covers the canonical pattern.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdCollider, UsdPlugin, UsdPrimRef, UsdPurpose, UsdRigidBody};

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

fn load_and_spawn(app: &mut App, asset_name: &str) {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());
    for _ in 0..200 {
        app.update();
        if matches!(
            app.world()
                .resource::<AssetServer>()
                .get_load_state(&handle),
            Some(LoadState::Loaded)
        ) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let scene_handle = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .scene
        .clone();
    app.world_mut().spawn(SceneRoot(scene_handle));
    for _ in 0..10 {
        app.update();
    }
}

fn entity_for_path(world: &mut World, path: &str) -> Entity {
    let mut q = world.query::<(Entity, &UsdPrimRef)>();
    q.iter(world)
        .find(|(_, p)| p.path == path)
        .map(|(e, _)| e)
        .unwrap_or_else(|| panic!("no entity for prim path {path}"))
}

#[test]
fn proxy_purpose_collider_survives_and_carries_physics() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_proxy.usda");
    let world = app.world_mut();

    // The render mesh, the proxy collider, and the guide marker all
    // become entities (regression — they used to be dropped).
    let render_e = entity_for_path(world, "/World/Robot/RenderMesh");
    let proxy_e = entity_for_path(world, "/World/Robot/ProxyCollider");
    let guide_e = entity_for_path(world, "/World/Robot/DebugMarker");

    // Purpose components on the non-default prims.
    assert_eq!(
        world.get::<UsdPurpose>(render_e),
        Some(&UsdPurpose::Render),
        "render mesh should carry UsdPurpose::Render"
    );
    assert_eq!(
        world.get::<UsdPurpose>(proxy_e),
        Some(&UsdPurpose::Proxy),
        "proxy collider should carry UsdPurpose::Proxy"
    );
    assert_eq!(
        world.get::<UsdPurpose>(guide_e),
        Some(&UsdPurpose::Guide),
        "guide marker should carry UsdPurpose::Guide"
    );

    // The proxy carries the physics opinions (this is the regression
    // we're guarding against — pre-fix it was silently dropped).
    assert!(
        world.get::<UsdRigidBody>(proxy_e).is_some(),
        "proxy collider lost PhysicsRigidBodyAPI"
    );
    assert!(
        world.get::<UsdCollider>(proxy_e).is_some(),
        "proxy collider lost PhysicsCollisionAPI"
    );

    // Visibility: render mesh visible, proxy + guide hidden by default.
    assert_eq!(
        world.get::<Visibility>(render_e),
        Some(&Visibility::Inherited),
        "render mesh should be visible by default"
    );
    assert_eq!(
        world.get::<Visibility>(proxy_e),
        Some(&Visibility::Hidden),
        "proxy collider should be Hidden by default"
    );
    assert_eq!(
        world.get::<Visibility>(guide_e),
        Some(&Visibility::Hidden),
        "guide marker should be Hidden by default"
    );

    println!(
        "proxy fix OK: render visible, proxy + guide hidden, proxy retains UsdRigidBody + UsdCollider"
    );
}
