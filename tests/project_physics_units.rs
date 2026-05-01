//! Phase 4: stage-unit conversion. metersPerUnit=0.01, kilogramsPerUnit=0.001,
//! upAxis=Z. Every physics marker value should land in SI after the
//! read→marker boundary.

use std::f32::consts::PI;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{
    UsdAsset, UsdJointKind, UsdMass, UsdPhysicsJoint, UsdPhysicsScene, UsdPlugin, UsdPrimRef,
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
            app.world().resource::<AssetServer>().get_load_state(&handle),
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

fn approx(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() < eps
}

#[test]
fn unit_conversion_lands_si_in_markers() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_units.usda");
    let world = app.world_mut();

    // PhysicsScene gravity: USD authored (0,0,-1) Z-up + 981 cm/s².
    // After basis fix: (0,-1,0); after meters_per_unit scaling: 9.81 m/s².
    let scene_e = entity_for_path(world, "/World/PhysicsScene");
    let scene = world
        .get::<UsdPhysicsScene>(scene_e)
        .expect("PhysicsScene missing UsdPhysicsScene");
    assert!(
        approx(scene.gravity_direction.y, -1.0, 1e-5)
            && approx(scene.gravity_direction.x, 0.0, 1e-5)
            && approx(scene.gravity_direction.z, 0.0, 1e-5),
        "Z-up gravity didn't rotate to Bevy world space: got {:?}",
        scene.gravity_direction
    );
    assert!(
        approx(scene.gravity_magnitude, 9.81, 1e-3),
        "gravity magnitude SI conversion failed: expected 9.81, got {}",
        scene.gravity_magnitude
    );

    // Body: mass 2500g → 2.5 kg, velocity 100 cm/s → 1 m/s, ang vel 180°/s → π rad/s.
    let body_e = entity_for_path(world, "/World/Body");
    let body_mass = world.get::<UsdMass>(body_e).expect("Body missing UsdMass");
    assert!(
        approx(body_mass.mass.unwrap(), 2.5, 1e-5),
        "mass kgPU conversion failed: expected 2.5, got {:?}",
        body_mass.mass
    );
    let body_rb = world
        .get::<UsdRigidBody>(body_e)
        .expect("Body missing UsdRigidBody");
    assert!(
        approx(body_rb.velocity.x, 1.0, 1e-5),
        "velocity mPU conversion failed: expected (1,0,0), got {:?}",
        body_rb.velocity
    );
    assert!(
        approx(body_rb.angular_velocity.x, PI, 1e-4),
        "angular velocity deg→rad failed: expected (π,0,0), got {:?}",
        body_rb.angular_velocity
    );

    // Joint: localPos 50cm/50cm → 0.5/0.5 m; prismatic limits -50cm/50cm → -0.5/0.5 m.
    let joint_e = entity_for_path(world, "/World/Slider");
    let joint = world
        .get::<UsdPhysicsJoint>(joint_e)
        .expect("Slider missing UsdPhysicsJoint");
    assert_eq!(joint.kind, UsdJointKind::Prismatic);
    assert!(
        approx(joint.local_pos0.x, 0.5, 1e-5) && approx(joint.local_pos0.y, 0.5, 1e-5),
        "joint local_pos0 mPU conversion failed: got {:?}",
        joint.local_pos0
    );
    let (lo, hi) = joint
        .built_in_limit
        .expect("prismatic limits should be authored");
    assert!(
        approx(lo, -0.5, 1e-5) && approx(hi, 0.5, 1e-5),
        "prismatic limits mPU conversion failed: expected (-0.5, 0.5), got ({lo}, {hi})"
    );

    println!(
        "units OK: gravity dir/mag, mass, vel, ang vel, joint anchor, prismatic limits all SI"
    );
}
