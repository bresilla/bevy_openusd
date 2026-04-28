//! M16 integration test: `UsdSkel` read side. Load a fixture with a
//! `SkelRoot` containing a 2-joint `Skeleton` and a `Mesh` bound via
//! `SkelBindingAPI`. Verify the plugin picks all three up and exposes
//! them on `UsdAsset`.

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
fn reads_skel_root_skeleton_and_binding() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "skel.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- UsdSkel read ----\n  \
         skeletons     = {}\n  \
         skel_roots    = {}\n  \
         skel_bindings = {}\n",
        asset.skeletons.len(),
        asset.skel_roots.len(),
        asset.skel_bindings.len(),
    );

    assert_eq!(asset.skeletons.len(), 1);
    assert_eq!(asset.skel_roots.len(), 1);
    assert_eq!(asset.skel_bindings.len(), 1);

    let skel = &asset.skeletons[0];
    println!(
        "  Skeleton {} joints = {:?}",
        skel.path, skel.joints
    );
    assert_eq!(skel.path, "/World/Rig/Skel");
    assert_eq!(skel.joints, vec!["root".to_string(), "root/tip".to_string()]);

    let root = &asset.skel_roots[0];
    println!(
        "  SkelRoot {} → skeleton={:?} animationSource={:?}",
        root.path, root.skeleton, root.animation_source
    );
    assert_eq!(root.path, "/World/Rig");
    assert_eq!(root.skeleton.as_deref(), Some("/World/Rig/Skel"));
    assert_eq!(root.animation_source.as_deref(), Some("/World/Rig/Anim"));

    let binding = &asset.skel_bindings[0];
    println!(
        "  SkelBinding on {} → skel={:?} elementsPerVertex={} indices={} weights={}",
        binding.prim_path,
        binding.skeleton,
        binding.elements_per_vertex,
        binding.joint_indices.len(),
        binding.joint_weights.len()
    );
    assert_eq!(binding.prim_path, "/World/Rig/Arm");
    assert_eq!(binding.skeleton.as_deref(), Some("/World/Rig/Skel"));
    assert_eq!(binding.elements_per_vertex, 2);
    assert_eq!(binding.joint_indices.len(), 6); // 3 verts × 2 elements
    assert_eq!(binding.joint_weights.len(), 6);
    // Vertex 0 fully bound to joint 0.
    assert_eq!(binding.joint_indices[0], 0);
    assert!((binding.joint_weights[0] - 1.0).abs() < 1e-5);
    // Vertex 1 split 50/50.
    assert!((binding.joint_weights[2] - 0.5).abs() < 1e-5);
    assert!((binding.joint_weights[3] - 0.5).abs() < 1e-5);
}
