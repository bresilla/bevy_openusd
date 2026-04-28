//! M12 integration test: vertex `primvars:displayColor` produces
//! `Mesh::ATTRIBUTE_COLOR` on the Bevy mesh, and `GeomSubset` children
//! of a Mesh split into per-subset child entities each carrying the
//! subset's bound material.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::color::LinearRgba;
use bevy::mesh::{Mesh, Mesh3d, VertexAttributeValues};
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
        let server = app.world().resource::<AssetServer>();
        match server.get_load_state(&handle) {
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
fn projects_displaycolor_and_subsets() {
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "displaycolor_subsets.usda");
    spawn_scene_root(&mut app, &handle);

    // ── 1. ColoredQuad: Mesh::ATTRIBUTE_COLOR populated from vertex displayColor.
    let (world_ref, quad_mesh_handle, split_child_paths) = {
        let world = app.world_mut();
        let mut by_path: std::collections::HashMap<
            String,
            (bevy::asset::Handle<Mesh>, bevy::asset::Handle<StandardMaterial>),
        > = std::collections::HashMap::new();
        for (prim, m3d, mat) in world
            .query::<(&UsdPrimRef, &Mesh3d, &MeshMaterial3d<StandardMaterial>)>()
            .iter(world)
        {
            by_path.insert(prim.path.clone(), (m3d.0.clone(), mat.0.clone()));
        }

        let quad = by_path
            .get("/World/ColoredQuad")
            .expect("ColoredQuad Mesh3d missing");

        let split_children: Vec<String> = by_path
            .keys()
            .filter(|p| p.starts_with("/World/SplitMesh/"))
            .cloned()
            .collect();

        (by_path.clone(), quad.0.clone(), split_children)
    };

    let meshes = app.world().resource::<Assets<Mesh>>();
    let quad_mesh = meshes.get(&quad_mesh_handle).expect("ColoredQuad mesh missing");
    let colors = quad_mesh
        .attribute(Mesh::ATTRIBUTE_COLOR)
        .expect("ATTRIBUTE_COLOR missing on ColoredQuad — displayColor wasn't projected");
    println!(
        "\n---- /World/ColoredQuad: primvars:displayColor → Mesh::ATTRIBUTE_COLOR ----"
    );
    match colors {
        VertexAttributeValues::Float32x4(values) => {
            for (i, v) in values.iter().enumerate() {
                println!(
                    "  vertex[{i}] = ({:.2}, {:.2}, {:.2}, {:.2})",
                    v[0], v[1], v[2], v[3]
                );
            }
            assert_eq!(values.len(), 4, "expected 4 vertex colors on the quad, got {}", values.len());
            // Vertex 0 → red; vertex 2 → blue.
            let v0 = values[0];
            let v2 = values[2];
            assert!(
                (v0[0] - 1.0).abs() < 1e-5 && v0[1].abs() < 1e-5 && v0[2].abs() < 1e-5,
                "vertex 0 should be (1,0,0,*), got {v0:?}"
            );
            assert!(
                v2[0].abs() < 1e-5 && v2[1].abs() < 1e-5 && (v2[2] - 1.0).abs() < 1e-5,
                "vertex 2 should be (0,0,1,*), got {v2:?}"
            );
        }
        other => panic!("ATTRIBUTE_COLOR should be Float32x4, got {other:?}"),
    }

    // ── 2. SplitMesh: should spawn 2 child entities (one per subset), each
    //     binding a distinct material. The SplitMesh entity itself must
    //     NOT carry a Mesh3d (every face was claimed, no residual).
    assert!(
        !world_ref.contains_key("/World/SplitMesh"),
        "SplitMesh shouldn't carry its own Mesh3d when subsets fully partition the faces"
    );
    let mut subset_paths = split_child_paths.clone();
    subset_paths.sort();
    assert_eq!(
        subset_paths,
        vec![
            "/World/SplitMesh/FaceA".to_string(),
            "/World/SplitMesh/FaceB".to_string()
        ],
        "expected exactly two subset children"
    );

    println!(
        "\n---- /World/SplitMesh GeomSubsets → one child entity per subset ----"
    );
    for p in &subset_paths {
        println!("  child: {p}");
    }

    let mat_a_handle = world_ref["/World/SplitMesh/FaceA"].1.clone();
    let mat_b_handle = world_ref["/World/SplitMesh/FaceB"].1.clone();
    assert_ne!(
        mat_a_handle, mat_b_handle,
        "the two subsets should bind distinct materials"
    );

    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let mat_a = materials.get(&mat_a_handle).expect("MatA missing");
    let mat_b = materials.get(&mat_b_handle).expect("MatB missing");

    let LinearRgba {
        red: ar,
        green: ag,
        blue: ab,
        ..
    } = mat_a.base_color.into();
    println!("  FaceA base_color = ({ar:.3}, {ag:.3}, {ab:.3})  → expected MatA red-ish");
    assert!(
        (ar - 0.9).abs() < 1e-4 && (ag - 0.1).abs() < 1e-4 && (ab - 0.1).abs() < 1e-4,
        "FaceA should bind MatA (red-ish), got ({ar}, {ag}, {ab})"
    );
    let LinearRgba {
        red: br,
        green: bg,
        blue: bb,
        ..
    } = mat_b.base_color.into();
    println!("  FaceB base_color = ({br:.3}, {bg:.3}, {bb:.3})  → expected MatB blue-ish\n");
    assert!(
        (br - 0.1).abs() < 1e-4 && (bg - 0.3).abs() < 1e-4 && (bb - 0.9).abs() < 1e-4,
        "FaceB should bind MatB (blue-ish), got ({br}, {bg}, {bb})"
    );
}
