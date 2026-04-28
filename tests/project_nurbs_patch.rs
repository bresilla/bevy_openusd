//! NURBS patch integration test: assert that
//! `usd_schemas::geom::read_nurbs_patch` decodes a 4×4 cubic patch,
//! that the tensor-product evaluator in
//! `bevy_openusd::nurbs_patch::nurbs_patch_to_bevy_mesh` produces a
//! 32×32 sample grid (1024 verts / 1922 tris), and that the corner
//! samples land on the corner CVs (end-clamped property in 2D).

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::mesh::{Indices, Mesh, Mesh3d, VertexAttributeValues};
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
fn corners_match_clamped_cvs() {
    use usd_schemas::geom::read_nurbs_patch;
    let stage =
        openusd::Stage::open("tests/stages/nurbs_patch.usda").expect("stage should open");
    let p = read_nurbs_patch(
        &stage,
        &openusd::sdf::Path::new("/World/Arch").expect("valid path"),
    )
    .expect("read ok")
    .expect("patch should decode");

    let mesh = bevy_openusd::nurbs_patch::nurbs_patch_to_bevy_mesh(&p);

    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => panic!("expected Float32x3 positions"),
    };
    let tri_count = match mesh.indices() {
        Some(Indices::U32(v)) => v.len() / 3,
        _ => 0,
    };
    println!(
        "\n---- NurbsPatch Arch ----\n  cps={} positions={} triangles={}",
        p.points.len(),
        positions.len(),
        tri_count,
    );

    // 32×32 sample grid → 1024 verts, 31×31×2 triangles.
    assert_eq!(positions.len(), 32 * 32);
    assert_eq!(tri_count, 31 * 31 * 2);

    // First sample (su=0, sv=0) → P[0, 0]
    // Last sample (su=31, sv=31) → P[3, 3]
    // (Row-major in V means our index ordering is (sv * nsamp + su),
    // so corner indices are 0, 31, 31*32, 31*32+31.)
    let near = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) < 1e-2
    };
    let p00 = positions[0];
    let p_last = positions[32 * 32 - 1];
    let cv00 = p.points[0];
    let cv33 = p.points[p.points.len() - 1];
    println!(
        "  corners: first={p00:?} (CV[0,0]={cv00:?}), last={p_last:?} (CV[3,3]={cv33:?})"
    );
    assert!(near(p00, cv00), "first sample should match CV[0,0]");
    assert!(near(p_last, cv33), "last sample should match CV[3,3]");
}

#[test]
fn loader_spawns_entity_per_nurbs_patch() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "nurbs_patch.usda");
    spawn_scene_root(&mut app, &handle);
    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        by_path.insert(prim.path.clone(), m3d.0.clone());
    }
    println!("\n---- NURBS-patch load ----\n  mesh-carrying prims:");
    let mut keys: Vec<_> = by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k}");
    }
    assert!(by_path.contains_key("/World/Arch"));
}
