//! Phase 4: multi-apply LimitAPI + DriveAPI on generic joints,
//! spherical cone limits, distance joint min/max — all with the right
//! per-DOF unit conversions (rotational → radians, linear → metres).

use std::f32::consts::PI;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{
    UsdAsset, UsdDof, UsdDriveType, UsdJointKind, UsdPhysicsJoint, UsdPlugin, UsdPrimRef,
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
fn generic_joint_decodes_multi_apply_limits_and_drives() {
    let mut app = build_test_app();
    load_and_spawn(&mut app, "physics_drives_limits.usda");
    let world = app.world_mut();

    // Generic joint: 2 LimitAPI entries + 1 DriveAPI entry.
    let g_e = entity_for_path(world, "/World/GenericAB");
    let g = world
        .get::<UsdPhysicsJoint>(g_e)
        .expect("GenericAB missing UsdPhysicsJoint");
    assert_eq!(g.kind, UsdJointKind::Generic);
    assert_eq!(g.limits.len(), 2, "expected 2 LimitAPI entries, got {:?}", g.limits);

    let trans_x = g.limits.iter().find(|l| l.dof == UsdDof::TransX).expect("missing transX limit");
    // transX low > high → locked; values are in metres (× metersPerUnit, here 1.0).
    assert!(
        trans_x.low > trans_x.high,
        "transX should preserve lock convention low>high; got low={} high={}",
        trans_x.low, trans_x.high
    );
    assert!(approx(trans_x.low, 1.0, 1e-5));
    assert!(approx(trans_x.high, 0.0, 1e-5));

    let rot_z = g.limits.iter().find(|l| l.dof == UsdDof::RotZ).expect("missing rotZ limit");
    // rotZ -30/30 degrees → -π/6 / π/6 radians.
    assert!(
        approx(rot_z.low, -PI / 6.0, 1e-5),
        "rotZ low: expected -π/6, got {}",
        rot_z.low
    );
    assert!(
        approx(rot_z.high, PI / 6.0, 1e-5),
        "rotZ high: expected π/6, got {}",
        rot_z.high
    );

    // DriveAPI on rotZ: target_velocity 90 deg/s → π/2 rad/s.
    // stiffness 100 N·m/deg → 100 × 180/π N·m/rad ≈ 5729.578.
    assert_eq!(g.drives.len(), 1, "expected 1 DriveAPI entry");
    let d = &g.drives[0];
    assert_eq!(d.dof, UsdDof::RotZ);
    assert_eq!(d.drive_type, UsdDriveType::Force);
    let tv = d.target_velocity.expect("targetVelocity should be authored");
    assert!(
        approx(tv, PI / 2.0, 1e-5),
        "drive target_velocity: expected π/2 rad/s, got {tv}"
    );
    assert!(
        approx(d.stiffness, 100.0 * 180.0 / PI, 1e-1),
        "drive stiffness rotational conversion: got {}",
        d.stiffness
    );
    assert!(
        approx(d.damping, 10.0 * 180.0 / PI, 1e-1),
        "drive damping rotational conversion: got {}",
        d.damping
    );
    assert_eq!(d.max_force, Some(50.0));

    // Spherical joint cone_limit (radians).
    let s_e = entity_for_path(world, "/World/Ball");
    let s = world
        .get::<UsdPhysicsJoint>(s_e)
        .expect("Ball missing UsdPhysicsJoint");
    assert_eq!(s.kind, UsdJointKind::Spherical);
    let (c0, c1) = s.cone_limit.expect("spherical cone limits should be authored");
    assert!(approx(c0, PI / 4.0, 1e-5), "cone0: expected π/4, got {c0}");
    assert!(approx(c1, PI / 6.0, 1e-5), "cone1: expected π/6, got {c1}");

    // Distance joint distance_limit (metres).
    let t_e = entity_for_path(world, "/World/Tether");
    let t = world
        .get::<UsdPhysicsJoint>(t_e)
        .expect("Tether missing UsdPhysicsJoint");
    assert_eq!(t.kind, UsdJointKind::Distance);
    let (mn, mx) = t.distance_limit.expect("distance_limit should be authored");
    assert!(approx(mn, 0.5, 1e-5));
    assert!(approx(mx, 1.5, 1e-5));

    println!(
        "drives+limits OK: 2 multi-LimitAPI, 1 DriveAPI w/ unit conv, sphere cone, distance min/max"
    );
}
