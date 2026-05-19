//! Phase 4: 4-link chain with PhysicsArticulationRootAPI projects an
//! UsdArticulationRoot whose `joints` list contains every Physics*Joint
//! in the subtree, and joint body0/body1 paths resolve to the link
//! entities.

use std::f32::consts::PI;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{
    UsdArticulationRoot, UsdAsset, UsdJointKind, UsdPhysicsJoint, UsdPlugin, UsdPrimRef,
    UsdRigidBody,
};

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
fn articulation_root_collects_subtree_joints() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_articulation.usda");
    let world = app.world_mut();

    let root_e = entity_for_path(world, "/Robot");
    let link0_e = entity_for_path(world, "/Robot/Link0");
    let link1_e = entity_for_path(world, "/Robot/Link1");
    let link2_e = entity_for_path(world, "/Robot/Link2");
    let link3_e = entity_for_path(world, "/Robot/Link3");
    let j01_e = entity_for_path(world, "/Robot/Joints/J01");
    let j12_e = entity_for_path(world, "/Robot/Joints/J12");
    let j23_e = entity_for_path(world, "/Robot/Joints/J23");

    // ArticulationRoot collects all three joints in subtree (under
    // /Robot/Joints, which is a descendant of /Robot).
    let ar = world
        .get::<UsdArticulationRoot>(root_e)
        .expect("/Robot missing UsdArticulationRoot");
    assert_eq!(
        ar.joints.len(),
        3,
        "expected 3 joints in articulation, got {}: {:?}",
        ar.joints.len(),
        ar.joints
    );
    assert!(ar.joints.contains(&j01_e));
    assert!(ar.joints.contains(&j12_e));
    assert!(ar.joints.contains(&j23_e));

    // Every link has UsdRigidBody.
    for link_e in [link0_e, link1_e, link2_e, link3_e] {
        assert!(
            world.get::<UsdRigidBody>(link_e).is_some(),
            "missing UsdRigidBody on a link"
        );
    }
    // Link0 is kinematic (base of the chain).
    assert!(world.get::<UsdRigidBody>(link0_e).unwrap().kinematic);

    // J01 / J12 use Z axis with degrees; J23 uses Y axis with smaller range.
    let j01 = world.get::<UsdPhysicsJoint>(j01_e).unwrap();
    assert_eq!(j01.kind, UsdJointKind::Revolute);
    assert_eq!(j01.axis, Vec3::Z);
    assert_eq!(j01.body0, Some(link0_e));
    assert_eq!(j01.body1, Some(link1_e));
    let (lo, hi) = j01.built_in_limit.unwrap();
    assert!((lo - (-PI / 2.0)).abs() < 1e-5);
    assert!((hi - (PI / 2.0)).abs() < 1e-5);

    let j23 = world.get::<UsdPhysicsJoint>(j23_e).unwrap();
    assert_eq!(j23.axis, Vec3::Y);
    assert_eq!(j23.body0, Some(link2_e));
    assert_eq!(j23.body1, Some(link3_e));

    println!(
        "articulation OK: ArticulationRoot.joints len={}, all 3 hinges resolved, axes Z/Z/Y",
        ar.joints.len()
    );
}
