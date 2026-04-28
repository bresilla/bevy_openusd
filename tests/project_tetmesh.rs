//! TetMesh integration test: assert that
//! `usd_schemas::geom::read_tetmesh` decodes a fixture, that the
//! boundary extractor in `bevy_openusd::tetmesh::tetmesh_to_bevy_mesh`
//! produces the right number of triangles for a known topology,
//! and that the loader spawns a mesh-carrying entity per TetMesh prim.

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

fn triangle_count(mesh: &Mesh) -> usize {
    use bevy::mesh::Indices;
    match mesh.indices() {
        Some(Indices::U16(v)) => v.len() / 3,
        Some(Indices::U32(v)) => v.len() / 3,
        None => 0,
    }
}

#[test]
fn boundary_face_count_matches_topology() {
    use usd_schemas::geom::read_tetmesh;
    let stage = openusd::Stage::open("tests/stages/tetmesh.usda")
        .expect("stage should open");

    // A single tet has exactly 4 boundary faces.
    let single = read_tetmesh(
        &stage,
        &openusd::sdf::Path::new("/World/Single").expect("valid path"),
    )
    .expect("read ok")
    .expect("single tet should decode");
    let single_mesh = bevy_openusd::tetmesh::tetmesh_to_bevy_mesh(&single);
    println!(
        "\n---- TetMesh Single ----\n  points={} tets={} boundary tris={}",
        single.points.len(),
        single.tet_vertex_indices.len() / 4,
        triangle_count(&single_mesh),
    );
    assert_eq!(triangle_count(&single_mesh), 4);

    // Four tets meeting at a central axis form an octahedron whose
    // boundary is exactly 8 triangles (all 4 axial faces cancel).
    let octa = read_tetmesh(
        &stage,
        &openusd::sdf::Path::new("/World/Octa").expect("valid path"),
    )
    .expect("read ok")
    .expect("octa should decode");
    let octa_mesh = bevy_openusd::tetmesh::tetmesh_to_bevy_mesh(&octa);
    println!(
        "---- TetMesh Octa ----\n  points={} tets={} boundary tris={}",
        octa.points.len(),
        octa.tet_vertex_indices.len() / 4,
        triangle_count(&octa_mesh),
    );
    assert_eq!(triangle_count(&octa_mesh), 8);
}

#[test]
fn loader_spawns_entity_per_tetmesh() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "tetmesh.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<String, bevy::asset::Handle<Mesh>> =
        std::collections::HashMap::new();
    for (prim, m3d) in world.query::<(&UsdPrimRef, &Mesh3d)>().iter(world) {
        by_path.insert(prim.path.clone(), m3d.0.clone());
    }
    println!("\n---- TetMesh load ----\n  mesh-carrying prims:");
    let mut keys: Vec<_> = by_path.keys().collect();
    keys.sort();
    for k in &keys {
        println!("    {k}");
    }
    assert!(by_path.contains_key("/World/Single"));
    assert!(by_path.contains_key("/World/Octa"));
}
