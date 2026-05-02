//! End-to-end smoke test: load the Agilebot GBT-C5A USD asset, wire
//! `bevy_openusd` → `bevy_openusd_rapier`, and let the chain swing /
//! settle under simulated gravity.
//!
//! Run with:
//! ```bash
//! DISPLAY=:1 cargo run -p bevy_openusd_rapier --example agilebot_drop
//! ```
//!
//! Expects the Agilebot asset checkout at
//! `assets/external/agilebot/gbt-c5a/gbt-c5a.usd` (gitignored — clone
//! `sh-agilebot/agilebot_isaac_usd_assets` next to the workspace root).

use std::path::PathBuf;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::prelude::*;
use bevy::scene::SceneRoot;
use bevy_openusd::{UsdAsset, UsdPlugin};
use bevy_openusd_rapier::RapierAdapterPlugin;
use bevy_rapier3d::plugin::{NoUserData, RapierPhysicsPlugin};
use bevy_rapier3d::render::RapierDebugRenderPlugin;

const ASSET: &str = "external/agilebot/gbt-c5a/gbt-c5a.usd";

#[derive(Resource)]
struct StageHandle(Handle<UsdAsset>);

#[derive(Resource, Default)]
struct Spawned(bool);

fn main() {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf();
    let asset_root = workspace_root.join("assets");
    if !asset_root.join(ASSET).exists() {
        eprintln!(
            "agilebot_drop: asset not found at {}; clone sh-agilebot/agilebot_isaac_usd_assets first",
            asset_root.join(ASSET).display()
        );
        return;
    }

    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "agilebot drops with bevy_openusd_rapier".into(),
                        resolution: (1400u32, 900u32).into(),
                        ..default()
                    }),
                    ..default()
                })
                .set(bevy::asset::AssetPlugin {
                    file_path: asset_root.to_string_lossy().into_owned(),
                    ..Default::default()
                }),
        )
        .add_plugins(UsdPlugin)
        .add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
        .add_plugins(RapierDebugRenderPlugin::default())
        .add_plugins(RapierAdapterPlugin)
        .init_resource::<Spawned>()
        .add_systems(Startup, (load_stage, spawn_camera_ground))
        .add_systems(Update, spawn_when_ready)
        .run();
}

fn load_stage(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handle: Handle<UsdAsset> = asset_server.load(ASSET);
    commands.insert_resource(StageHandle(handle));
}

fn spawn_camera_ground(mut commands: Commands) {
    use bevy_rapier3d::geometry::Collider;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(2.0, 1.5, 2.5).looking_at(Vec3::new(0.0, 0.3, 0.0), Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 6_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 6.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    // Static ground so the falling chain has something to hit.
    commands.spawn((
        Transform::from_xyz(0.0, -0.5, 0.0),
        Collider::cuboid(20.0, 0.05, 20.0),
    ));
}

fn spawn_when_ready(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    assets: Res<Assets<UsdAsset>>,
    stage: Res<StageHandle>,
    mut spawned: ResMut<Spawned>,
) {
    if spawned.0 {
        return;
    }
    match asset_server.get_load_state(&stage.0) {
        Some(LoadState::Loaded) => {
            let Some(asset) = assets.get(&stage.0) else {
                return;
            };
            info!(
                "loaded UsdAsset: {} rigid bodies, {} joints, {} articulation roots, {} colliders",
                asset.rigid_body_prims.len(),
                asset.joints.len(),
                asset.articulation_root_prims.len(),
                asset.collider_prims.len(),
            );
            commands.spawn(SceneRoot(asset.scene.clone()));
            spawned.0 = true;
        }
        Some(LoadState::Failed(err)) => {
            error!("UsdAsset load failed: {err}");
            spawned.0 = true;
        }
        _ => {}
    }
}
