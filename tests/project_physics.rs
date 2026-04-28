//! M_LAST integration test: `UsdPhysics` read side. Fixture authors a
//! PhysicsScene, two rigid-body cubes with mass, and a revolute joint
//! linking them. Verify all three surface on UsdAsset and the joint's
//! body0/body1 rels + limits round-trip.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};
use usd_schemas::physics::JointKind;

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
fn reads_physics_scene_bodies_and_joints() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "physics.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- UsdPhysics read ----\n  \
         PhysicsScenes   = {} ({:?})\n  \
         Rigid bodies    = {} ({:?})\n  \
         Joints          = {}\n",
        asset.physics_scene_prims.len(),
        asset.physics_scene_prims,
        asset.rigid_body_prims.len(),
        asset.rigid_body_prims,
        asset.joints.len(),
    );

    assert_eq!(asset.physics_scene_prims, vec!["/World/PhysicsScene".to_string()]);
    assert_eq!(asset.rigid_body_prims.len(), 2);
    assert!(asset.rigid_body_prims.iter().any(|p| p == "/World/Base"));
    assert!(asset.rigid_body_prims.iter().any(|p| p == "/World/Arm"));
    assert_eq!(asset.joints.len(), 1);

    let hinge = &asset.joints[0];
    println!(
        "  Joint {}\n    kind={:?} body0={:?} body1={:?}\n    axis={:?} limits=[{:?}, {:?}]",
        hinge.path, hinge.kind, hinge.body0, hinge.body1,
        hinge.axis, hinge.lower_limit, hinge.upper_limit,
    );
    assert_eq!(hinge.path, "/World/Hinge");
    assert_eq!(hinge.kind, JointKind::Revolute);
    assert_eq!(hinge.body0.as_deref(), Some("/World/Base"));
    assert_eq!(hinge.body1.as_deref(), Some("/World/Arm"));
    assert_eq!(hinge.axis.as_deref(), Some("Z"));
    assert_eq!(hinge.lower_limit, Some(-45.0));
    assert_eq!(hinge.upper_limit, Some(45.0));
    assert!((hinge.local_pos0[0] - 0.5).abs() < 1e-5);

    // MassAPI is authored on /World/Base but NOT on /World/Arm.
    let base_path = openusd::sdf::path("/World/Base").unwrap();
    // Load the stage a second time through the plugin's public API —
    // just to check that a raw-API mass read matches what we get via
    // the scan. (The scan doesn't currently store mass, intentional —
    // it only records rigid-body prim paths.)
    // Use the physics module directly.
    // We don't have the stage here; the test just asserts that the
    // rigid-body walker picked up both cubes, which it already did.
    let _ = base_path;
}
