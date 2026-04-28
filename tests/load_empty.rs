//! M0 integration test: load the smallest viable stage through the Bevy
//! asset pipeline and confirm the resulting [`UsdAsset`] actually materializes
//! in `Assets<UsdAsset>`.

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::prelude::*;
use bevy_openusd::{UsdAsset, UsdPlugin};

#[derive(Resource)]
struct Stage(Handle<UsdAsset>);

#[test]
fn loads_empty_stage() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: "tests/stages".into(),
            ..Default::default()
        })
        .add_plugins(UsdPlugin);

    let handle: Handle<UsdAsset> = app.world().resource::<AssetServer>().load("empty.usda");
    app.world_mut().insert_resource(Stage(handle));

    // Drive the app forward until the asset finishes loading (or we give up).
    let mut loaded = false;
    for _ in 0..200 {
        app.update();
        let world = app.world();
        let server = world.resource::<AssetServer>();
        let stage = world.resource::<Stage>();
        match server.get_load_state(&stage.0) {
            Some(LoadState::Loaded) => {
                loaded = true;
                break;
            }
            Some(LoadState::Failed(err)) => {
                panic!("UsdAsset load failed: {err}");
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    assert!(loaded, "UsdAsset did not finish loading in time");

    let world = app.world();
    let assets = world.resource::<Assets<UsdAsset>>();
    let handle = &world.resource::<Stage>().0;
    let asset = assets.get(handle).expect("UsdAsset not in storage");
    assert_eq!(
        asset.default_prim.as_deref(),
        Some("World"),
        "empty.usda declares defaultPrim = \"World\""
    );
    assert!(asset.layer_count >= 1, "stage has at least one layer");
}
