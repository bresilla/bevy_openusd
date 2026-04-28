//! M28 integration test: instance dedup replays a cached descriptor
//! list for prototype-matching instance sites, producing the right
//! subtree without re-walking the USD stage.
//!
//! Uses the existing `tests/stages/instanceable.usda` fixture (two
//! `instanceable = true` Xforms with identical Cube subtrees + a
//! non-instanceable Sphere control).

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin, UsdPrimRef};

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
fn replay_produces_full_subtree_and_shares_handles() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "instanceable.usda");
    spawn_scene_root(&mut app, &handle);

    let asset = app
        .world()
        .resource::<Assets<UsdAsset>>()
        .get(&handle)
        .expect("asset missing")
        .clone();

    println!(
        "\n---- M28 dedup ----\n  \
         instance_prim_count       = {} (expected 2)\n  \
         instance_prototype_reuses = {} (expected 1)\n",
        asset.instance_prim_count, asset.instance_prototype_reuses
    );
    assert_eq!(asset.instance_prim_count, 2);
    assert_eq!(asset.instance_prototype_reuses, 1);

    let world = app.world_mut();

    // Gather mesh handles by UsdPrimRef path.
    let mut meshes_by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        meshes_by_path.insert(prim.path.clone(), m3d.0.clone());
    }
    println!("  mesh-carrying prims:");
    let mut keys: Vec<_> = meshes_by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k}");
    }

    // Both instance sites' Body cubes must exist.
    let a = meshes_by_path
        .get("/World/InstanceA/Body")
        .expect("InstanceA/Body missing — replay dropped it?");
    let b = meshes_by_path
        .get("/World/InstanceB/Body")
        .expect("InstanceB/Body missing — replay dropped it?");

    // Both must share the same Mesh handle (content-hash dedup on the
    // first walk's cache; replay reuses the same handle).
    assert_eq!(
        a, b,
        "InstanceA/Body and InstanceB/Body must share a Mesh handle"
    );

    // The non-instanceable Sphere must still exist.
    assert!(
        meshes_by_path.contains_key("/World/Unique/Body"),
        "non-instanceable Sphere must not be affected by M28"
    );

    // Hierarchy check: InstanceB's Body entity must have ChildOf
    // pointing at an entity whose UsdPrimRef is "/World/InstanceB".
    let world = app.world_mut();
    let mut body_b_entity = None;
    for (e, prim) in world.query::<(Entity, &UsdPrimRef)>().iter(world) {
        if prim.path == "/World/InstanceB/Body" {
            body_b_entity = Some(e);
            break;
        }
    }
    let body_b_entity = body_b_entity.expect("/World/InstanceB/Body entity missing");
    let parent = world
        .get::<ChildOf>(body_b_entity)
        .expect("Body must have a ChildOf");
    let parent_entity = parent.0;
    let parent_prim = world
        .get::<UsdPrimRef>(parent_entity)
        .expect("parent must have UsdPrimRef");
    println!(
        "  /World/InstanceB/Body parent → {} (expected /World/InstanceB)",
        parent_prim.path
    );
    assert_eq!(parent_prim.path, "/World/InstanceB");
}
