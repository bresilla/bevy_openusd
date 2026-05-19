//! Phase 4: PhysicsMaterialAPI projection + material:binding:physics
//! resolution. Verifies UsdPhysicsMaterial lands on Material prims and
//! UsdCollider.physics_material resolves to the bound Material entity.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdCollider, UsdPhysicsMaterial, UsdPlugin, UsdPrimRef};

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
fn physics_material_binding_resolves_to_material_entity() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_material.usda");
    let world = app.world_mut();

    let rubber_e = entity_for_path(world, "/World/Rubber");
    let ice_e = entity_for_path(world, "/World/Ice");
    let ball_e = entity_for_path(world, "/World/Ball");
    let slider_e = entity_for_path(world, "/World/Slider");

    // PhysicsMaterialAPI lands on Material prims with the four scalars.
    let rubber = world
        .get::<UsdPhysicsMaterial>(rubber_e)
        .expect("/World/Rubber missing UsdPhysicsMaterial");
    assert_eq!(rubber.dynamic_friction, Some(0.8));
    assert_eq!(rubber.static_friction, Some(0.9));
    assert_eq!(rubber.restitution, Some(0.6));
    assert_eq!(rubber.density, Some(1100.0));

    let ice = world
        .get::<UsdPhysicsMaterial>(ice_e)
        .expect("/World/Ice missing UsdPhysicsMaterial");
    assert_eq!(ice.dynamic_friction, Some(0.05));
    assert!(ice.density.is_none(), "Ice did not author density");

    // material:binding:physics on each collider resolves to the
    // Material entity in the post-pass.
    let ball_col = world
        .get::<UsdCollider>(ball_e)
        .expect("/World/Ball missing UsdCollider");
    assert_eq!(
        ball_col.physics_material,
        Some(rubber_e),
        "Ball.physics_material should resolve to Rubber"
    );

    let slider_col = world
        .get::<UsdCollider>(slider_e)
        .expect("/World/Slider missing UsdCollider");
    assert_eq!(
        slider_col.physics_material,
        Some(ice_e),
        "Slider.physics_material should resolve to Ice"
    );

    println!(
        "material binding OK: 2 PhysicsMaterials authored, both colliders resolve to right entity"
    );
}
