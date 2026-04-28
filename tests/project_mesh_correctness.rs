//! M20 integration test: `doubleSided`, `orientation`, and `extent`
//! round-trip correctly.
//!
//! - `SingleSided` + `DoubleSided` share the same `material:binding`
//!   but must resolve to DIFFERENT `StandardMaterial` handles (the
//!   double-sided variant has `cull_mode = None` and `double_sided =
//!   true`).
//! - `LeftHanded` triangle: the reader surfaces the orientation
//!   correctly so `mesh_from_usd` flips winding.
//! - Authored `extent` round-trips.

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
fn reads_double_sided_orientation_and_extent() {
    // 1. Reader side — verify ReadMesh fields round-trip via openusd
    //    directly (no Bevy involvement).
    let stage = openusd::Stage::open("tests/stages/mesh_correctness.usda")
        .expect("fixture parses");
    use openusd::sdf::Path;
    use usd_schemas::geom::{read_mesh, Orientation};

    let single = read_mesh(&stage, &Path::new("/World/SingleSided").unwrap())
        .expect("read ok")
        .expect("mesh decodes");
    let double = read_mesh(&stage, &Path::new("/World/DoubleSided").unwrap())
        .expect("read ok")
        .expect("mesh decodes");
    let left = read_mesh(&stage, &Path::new("/World/LeftHanded").unwrap())
        .expect("read ok")
        .expect("mesh decodes");

    println!(
        "\n---- ReadMesh round-trip ----\n  \
         SingleSided: double_sided={} orientation={:?} extent={:?}\n  \
         DoubleSided: double_sided={} orientation={:?} extent={:?}\n  \
         LeftHanded:  double_sided={} orientation={:?} extent={:?}\n",
        single.double_sided, single.orientation, single.extent,
        double.double_sided, double.orientation, double.extent,
        left.double_sided, left.orientation, left.extent,
    );

    assert!(!single.double_sided);
    assert!(double.double_sided);
    assert!(!left.double_sided);
    assert_eq!(single.orientation, Orientation::RightHanded);
    assert_eq!(double.orientation, Orientation::RightHanded);
    assert_eq!(left.orientation, Orientation::LeftHanded);
    assert_eq!(single.extent, Some([[-0.5, 0.0, -0.5], [0.5, 0.0, 0.5]]));
    assert_eq!(double.extent, Some([[-0.5, 0.0, -0.5], [0.5, 0.0, 0.5]]));
    assert_eq!(left.extent, Some([[0.0, 0.0, 0.0], [1.0, 1.0, 0.0]]));

    // 2. Loader side — the same Material binding on single- and
    //    double-sided meshes must resolve to DIFFERENT StandardMaterial
    //    handles, with the double-sided variant having
    //    `double_sided = true` and `cull_mode = None`.
    let mut app = build_test_app();
    let handle = load_and_drive(&mut app, "mesh_correctness.usda");
    spawn_scene_root(&mut app, &handle);

    let world = app.world_mut();
    let mut by_path: std::collections::HashMap<
        String,
        bevy::asset::Handle<StandardMaterial>,
    > = std::collections::HashMap::new();
    for (prim, mat) in world
        .query::<(&UsdPrimRef, &MeshMaterial3d<StandardMaterial>)>()
        .iter(world)
    {
        by_path.insert(prim.path.clone(), mat.0.clone());
    }

    let single_mat = by_path
        .get("/World/SingleSided")
        .expect("SingleSided material missing");
    let double_mat = by_path
        .get("/World/DoubleSided")
        .expect("DoubleSided material missing");
    println!(
        "  single_mat handle = {single_mat:?}\n  double_mat handle = {double_mat:?}"
    );
    assert_ne!(
        single_mat, double_mat,
        "SingleSided and DoubleSided must resolve to distinct StandardMaterial handles"
    );

    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let s = materials.get(single_mat).expect("single material");
    let d = materials.get(double_mat).expect("double material");
    println!(
        "  SingleSided material: double_sided={} cull_mode={:?}\n  \
         DoubleSided material: double_sided={} cull_mode={:?}",
        s.double_sided, s.cull_mode, d.double_sided, d.cull_mode
    );
    assert!(!s.double_sided);
    assert!(d.double_sided);
    assert_eq!(d.cull_mode, None);
}
