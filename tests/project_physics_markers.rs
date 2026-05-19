//! Phase 3 verification: physics.usda projects every authored
//! UsdPhysics opinion onto the right marker components, and the
//! post-pass resolves joint body0/body1 path strings to entity refs.

use std::f32::consts::PI;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{
    UsdAsset, UsdCollider, UsdJointKind, UsdMass, UsdPhysicsJoint, UsdPhysicsScene, UsdPlugin,
    UsdPrimRef, UsdRigidBody,
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
        match app
            .world()
            .resource::<AssetServer>()
            .get_load_state(&handle)
        {
            Some(LoadState::Loaded) => break,
            Some(LoadState::Failed(err)) => panic!("UsdAsset load failed: {err}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    let scene_handle = {
        let assets = app.world().resource::<Assets<UsdAsset>>();
        assets.get(&handle).expect("asset missing").scene.clone()
    };
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
fn projection_emits_physics_marker_components() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics.usda");
    let world = app.world_mut();

    let scene_e = entity_for_path(world, "/World/PhysicsScene");
    let base_e = entity_for_path(world, "/World/Base");
    let arm_e = entity_for_path(world, "/World/Arm");
    let hinge_e = entity_for_path(world, "/World/Hinge");

    // PhysicsScene → UsdPhysicsScene attached.
    assert!(
        world.get::<UsdPhysicsScene>(scene_e).is_some(),
        "/World/PhysicsScene missing UsdPhysicsScene"
    );

    // /World/Base authors RigidBody + Mass + Collision.
    assert!(
        world.get::<UsdRigidBody>(base_e).is_some(),
        "Base no UsdRigidBody"
    );
    let base_mass = world.get::<UsdMass>(base_e).expect("Base no UsdMass");
    // physics.usda authors mass=2.5 with kilogramsPerUnit=1.0 so SI mass = 2.5 kg.
    assert!(
        (base_mass.mass.unwrap() - 2.5).abs() < 1e-5,
        "Base mass: expected 2.5, got {:?}",
        base_mass.mass
    );
    assert!(
        world.get::<UsdCollider>(base_e).is_some(),
        "Base no UsdCollider"
    );

    // /World/Arm authors RigidBody + Collision but NO Mass.
    assert!(
        world.get::<UsdRigidBody>(arm_e).is_some(),
        "Arm no UsdRigidBody"
    );
    assert!(
        world.get::<UsdMass>(arm_e).is_none(),
        "Arm should not have UsdMass"
    );
    assert!(
        world.get::<UsdCollider>(arm_e).is_some(),
        "Arm no UsdCollider"
    );

    // /World/Hinge → UsdPhysicsJoint with resolved body refs + Vec3::Z axis +
    // radian-converted built-in limits.
    let joint = world
        .get::<UsdPhysicsJoint>(hinge_e)
        .expect("Hinge missing UsdPhysicsJoint");
    assert_eq!(joint.kind, UsdJointKind::Revolute);
    assert_eq!(
        joint.body0,
        Some(base_e),
        "joint body0 should resolve to Base"
    );
    assert_eq!(
        joint.body1,
        Some(arm_e),
        "joint body1 should resolve to Arm"
    );
    assert_eq!(joint.axis, Vec3::Z);
    let (lo, hi) = joint
        .built_in_limit
        .expect("revolute limits should round-trip");
    // physics.usda authors -45° / +45°; markers store radians.
    assert!(
        (lo - (-PI / 4.0)).abs() < 1e-5,
        "lower: expected -π/4, got {lo}"
    );
    assert!(
        (hi - (PI / 4.0)).abs() < 1e-5,
        "upper: expected π/4, got {hi}"
    );

    println!(
        "physics markers OK: PhysicsScene + 2 RigidBody + 1 Mass + 2 Collider + 1 Joint (resolved body0/1)"
    );
}
