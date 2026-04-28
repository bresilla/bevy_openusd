//! `bevy_openusd` viewer — primary dogfood binary.
//!
//! Loads a USD file, projects it into a Bevy Scene, and shows the result in
//! a VS-Code-style UI (left activity bar + floating panels). Used
//! throughout plugin development: each milestone gets dropped into this
//! viewer so we can eyeball the projection.
//!
//!   cargo run                      # loads assets/two_xforms.usda
//!   cargo run -- path/to/robot.usda
//!
//! Mouse: L+R drag orbit · Middle drag pan · Scroll zoom.
//! Keyboard: T I O ? toggle panels · G X P toggle overlays.

mod camera;
mod keyboard;
mod log_panel;
mod overlays;
mod state;
mod ui;

use std::path::PathBuf;

use bevy::asset::{AssetEvent, AssetServer, LoadState};
use bevy::ecs::message::MessageReader;
use bevy::gizmos::config::{GizmoConfigGroup, GizmoConfigStore};
use bevy::prelude::*;
use bevy::reflect::Reflect;
use bevy::scene::SceneRoot;
use bevy_egui::EguiPlugin;
use bevy_openusd::{UsdAsset, UsdLoaderSettings, UsdPlugin, UsdPrimRef};

use crate::camera::{ArcballCamera, ArcballCameraPlugin};
use crate::keyboard::ViewerKeyboardPlugin;
use crate::overlays::{OverlaysPlugin, SceneExtent};
use crate::state::{
    CameraBookmarks, CameraMount, FlyTo, LoadRequest, LoaderTuning, ReloadRequest, SelectedPrim,
    StageInfo, UsdStageTime,
};
use crate::ui::{ViewerUiPlugin, RIBBON_LEFT, RIB_TREE};

#[derive(Resource)]
struct StageHandle(Handle<UsdAsset>);

#[derive(Resource, Default)]
struct Spawned(bool);

/// Tag on the viewer's fallback `DirectionalLight`. Kept alive until
/// `spawn_when_ready` confirms the loaded stage authors at least one
/// of its own directional lights — at which point the default gets
/// despawned so the two don't stack and blow the exposure out.
#[derive(Component)]
struct DefaultSun;

fn main() {
    let (asset_path, asset_root) = resolve_requested_asset();

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: format!("bevy_openusd — {asset_path}"),
                    resolution: (1400u32, 900u32).into(),
                    ..default()
                }),
                ..default()
            })
            .set(bevy::asset::AssetPlugin {
                file_path: asset_root.to_string_lossy().into_owned(),
                ..Default::default()
            })
            .set(bevy::log::LogPlugin {
                custom_layer: crate::log_panel::loader_log_custom_layer,
                ..Default::default()
            }),
    )
    .add_plugins(EguiPlugin::default())
    .add_plugins(bevy::pbr::wireframe::WireframePlugin::default())
    .add_plugins(UsdPlugin)
    .add_plugins(ArcballCameraPlugin)
    .add_plugins(ViewerUiPlugin)
    .add_plugins(ViewerKeyboardPlugin)
    .add_plugins(OverlaysPlugin)
    .init_resource::<Spawned>()
    .init_resource::<ReloadRequest>()
    .init_resource::<LoadRequest>()
    .init_resource::<SelectedPrim>()
    .init_resource::<FlyTo>()
    .init_resource::<CameraMount>()
    .init_resource::<LoaderTuning>()
    .init_resource::<UsdStageTime>()
    .init_resource::<CameraBookmarks>()
    .add_systems(Startup, open_default_panel)
    .insert_resource(StageInfo {
        path: asset_path.clone(),
        ..default()
    })
    .insert_resource(RequestedAsset {
        name: asset_path,
        root: asset_root.clone(),
    })
    .add_systems(
        Startup,
        (sweep_variant_tempfiles, load_stage, spawn_camera_and_ground),
    )
    .add_systems(
        Update,
        (
            spawn_when_ready,
            fit_camera_once,
            debug_origin_prims_once,
            debug_dump_layout_once,
            handle_usd_hot_reload,
            apply_load_request,
            apply_fly_to,
            draw_selected_prim_highlight,
            follow_mounted_camera,
            rebuild_tuned_meshes,
            tick_stage_time,
            evaluate_animated_prims,
            drive_skel_animations,
            drive_blend_shape_weights,
            draw_joint_gizmos,
            hide_meshes_on_startup,
        ),
    );
    let hide_meshes = std::env::var("BEVY_OPENUSD_HIDE_MESHES")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
        .unwrap_or(false);
    app.insert_resource(HideMeshesFlag(hide_meshes));
    let show_joint_gizmos = std::env::var("BEVY_OPENUSD_JOINT_GIZMOS")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
        .unwrap_or(false);
    app.insert_resource(ShowJointGizmosFlag(show_joint_gizmos));

    // Skeleton bones render as part of their own gizmo group with
    // `depth_bias = -1.0` so they always draw on top of geometry —
    // otherwise the rig is hidden inside the skin and the user has
    // no way to verify the joint hierarchy is alive.
    app.init_gizmo_group::<SkeletonGizmos>()
        .add_systems(Startup, setup_skeleton_gizmos_on_top);

    app.run();
}

/// Working out which USD file to load + where to root the AssetServer.
///
/// - `cargo run` with no arg → `assets/two_xforms.usda` next to the
///   top-level `bevy_openusd` crate.
/// - `cargo run -- path/to/file.usda` → that file, with the AssetServer
///   rooted at its parent dir so sublayers resolve.
/// Open the prim-tree panel on startup so the viewer has something
/// populated to show — same default as the old `LeftTab::Tree`.
fn open_default_panel(mut ribbon: ResMut<bevy_frost::RibbonOpen>) {
    ribbon.toggle(RIBBON_LEFT, RIB_TREE);
}

fn resolve_requested_asset() -> (String, PathBuf) {
    let arg = std::env::args().nth(1);
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    match arg {
        None => ("materials.usda".to_string(), workspace_root.join("assets")),
        Some(raw) => {
            let path = PathBuf::from(&raw);
            let abs = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| workspace_root.clone())
                    .join(path)
            };
            let file = abs
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| raw.clone());
            let dir = abs
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| workspace_root.clone());
            (file, dir)
        }
    }
}

#[derive(Resource)]
pub struct RequestedAsset {
    pub name: String,
    pub root: PathBuf,
}

fn load_stage(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    requested: Res<RequestedAsset>,
    tuning: Res<LoaderTuning>,
) {
    // Pass the absolute asset-root directory as a search path so openusd
    // can chase sibling references like `@./greenhouse/front.usdc@`.
    let search = vec![requested.root.clone()];
    let kind_collapse = std::env::var("BEVY_OPENUSD_KIND_COLLAPSE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
        .unwrap_or(false);
    let curve_radius = tuning.curves.default_radius;
    let curve_rings = tuning.curves.ring_segments;
    let point_scale = tuning.curves.point_scale;
    let variant_selections = tuning.to_variant_selections();
    let handle: Handle<UsdAsset> = asset_server.load_with_settings::<UsdAsset, _>(
        requested.name.clone(),
        move |s: &mut UsdLoaderSettings| {
            s.search_paths = search.clone();
            s.kind_collapse = kind_collapse;
            s.curve_default_radius = curve_radius;
            s.curve_ring_segments = curve_rings;
            s.point_scale = point_scale;
            s.variant_selections = variant_selections.clone();
        },
    );
    commands.insert_resource(StageHandle(handle));
    info!(
        "queued asset load: {} (search paths: {:?}, kind_collapse={}, curve_radius={}, curve_rings={}, point_scale={}, variants={})",
        requested.name,
        requested.root,
        kind_collapse,
        curve_radius,
        curve_rings,
        point_scale,
        tuning.variants.len()
    );
}

fn spawn_camera_and_ground(mut commands: Commands) {
    use bevy::core_pipeline::tonemapping::Tonemapping;
    use bevy::post_process::bloom::Bloom;
    use bevy::render::view::Hdr;

    // Arcball camera targeting origin; focus/distance get tuned once the
    // stage lands (stretch goal: fit-to-bounds).
    //
    // **HDR + ACES tone mapping + bloom** are essential for PBR
    // materials to look right. In Bevy 0.18 HDR is a `Hdr` marker
    // component (was a `hdr: bool` field on `Camera` previously), and
    // bloom lives in `bevy::post_process`. Without HDR, emissive
    // textures and metallic specular highlights clamp to LDR and look
    // chalky; ACES tone mapping (the curve usdview / Quick Look apply)
    // restores the filmic falloff. Bloom adds the soft edge around
    // light sources and bright reflections.
    commands.spawn((
        Camera3d::default(),
        Hdr,
        // AgX is the modern filmic curve Blender / Krita default to.
        // ACES is more contrasty + clips highlights harder; with a
        // single 50k-lux sun ACES turned the teapot into a pure-white
        // blob. AgX rolls highlights gently, reproduces albedo more
        // faithfully, and tolerates wider exposure ranges before
        // clipping.
        Tonemapping::AgX,
        // Bloom is OFF by default — turn it on with `Bloom::default()`
        // once the user wants it. With strong direct lighting + HDR +
        // ACES, the default bloom radius blew the highlights out into
        // halos that overlapped the entire silhouette.
        // (Re-enable: add `Bloom::default()` here.)
        Transform::from_xyz(3.0, 2.5, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        ArcballCamera {
            focus: Vec3::new(0.0, 0.4, 0.0),
            distance: 4.0,
            ..default()
        },
    ));
    // Indoor-overcast lux. With HDR + AgX a single 5k-lux sun reads
    // closer to a quick-look studio render than 50k did — an order of
    // magnitude lower because HDR + AgX preserve dynamic range above
    // 1.0 instead of clipping; we don't need to push raw lux.
    // Ambient stays modest so PBR keeps its contrast.
    commands.spawn((
        DirectionalLight {
            illuminance: 5_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(4.0, 6.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
        DefaultSun,
    ));
    commands.insert_resource(bevy::light::GlobalAmbientLight {
        brightness: 200.0,
        ..default()
    });
    // Ground plate intentionally gone — the WorldGrid overlay provides the
    // reference plane now, sized and faded to match the scene extent.
}

/// Dump geom-bearing prims that landed within ~1 % of the scene diagonal
/// from the world origin. Fires once on the first frame after the scene
/// materializes — a quick diagnostic for the "stuff stuck at origin" class
/// of bugs (missing xform ops, broken basis fix, etc.). Set
/// `BEVY_OPENUSD_DEBUG_ORIGIN=1` to enable.
/// One-shot dump of every prim entity's path, world translation, and
/// whether it carries a Mesh3d. Written to `/tmp/kitchen_layout.txt`
/// when `BEVY_OPENUSD_DEBUG_LAYOUT=1`. Used for offline analysis when
/// a real production asset (Pixar Kitchen_set, etc.) loads with
/// scattered geometry — clustering by parent path quickly shows
/// whether transforms are off, payloads failed, or a specific prop
/// landed at the wrong scale.
fn debug_dump_layout_once(
    prims: Query<(&UsdPrimRef, &GlobalTransform, &Transform, Option<&bevy::mesh::Mesh3d>)>,
    extent: Res<SceneExtent>,
    mut done: Local<bool>,
) {
    if *done || extent.count == 0 {
        return;
    }
    if std::env::var("BEVY_OPENUSD_DEBUG_LAYOUT").ok().as_deref() != Some("1") {
        *done = true;
        return;
    }
    let mut rows: Vec<(String, Vec3, Vec3, Quat, Vec3, bool)> = prims
        .iter()
        .map(|(p, gt, t, mesh)| {
            (
                p.path.clone(),
                gt.translation(),
                t.translation,
                t.rotation,
                t.scale,
                mesh.is_some(),
            )
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    out.push_str(&format!(
        "# {} prims · {} with Mesh3d · diag {:.2} m\n# path | world_xyz | local_t | local_quat(xyzw) | local_scale | has_mesh\n",
        rows.len(),
        rows.iter().filter(|r| r.5).count(),
        extent.diag()
    ));
    for (path, w, lt, lq, ls, has_mesh) in &rows {
        out.push_str(&format!(
            "{path} | {:.3},{:.3},{:.3} | {:.3},{:.3},{:.3} | {:.3},{:.3},{:.3},{:.3} | {:.3},{:.3},{:.3} | {}\n",
            w.x, w.y, w.z,
            lt.x, lt.y, lt.z,
            lq.x, lq.y, lq.z, lq.w,
            ls.x, ls.y, ls.z,
            has_mesh
        ));
    }
    let path = "/tmp/kitchen_layout.txt";
    if let Err(e) = std::fs::write(path, &out) {
        info!("layout dump failed: {e}");
    } else {
        info!("layout dump: wrote {} rows to {path}", rows.len());
    }
    *done = true;
}

fn debug_origin_prims_once(
    prims: Query<(&UsdPrimRef, &GlobalTransform), With<bevy::mesh::Mesh3d>>,
    extent: Res<SceneExtent>,
    mut done: Local<bool>,
) {
    if *done || extent.count == 0 {
        return;
    }
    if std::env::var("BEVY_OPENUSD_DEBUG_ORIGIN").ok().as_deref() != Some("1") {
        *done = true;
        return;
    }
    let diag = extent.diag();
    let threshold = (diag * 0.01).max(0.05);
    let mut near_origin: Vec<(String, Vec3)> = Vec::new();
    for (prim, gt) in prims.iter() {
        let p = gt.translation();
        if p.length() < threshold {
            near_origin.push((prim.path.clone(), p));
        }
    }
    if near_origin.is_empty() {
        info!(
            "origin debug: no geom prims within {threshold:.3} m of (0,0,0) — \
             origin extrusion bug not reproduced"
        );
    } else {
        info!(
            "origin debug: {} geom prim(s) within {threshold:.3} m of (0,0,0):",
            near_origin.len()
        );
        for (path, pos) in near_origin.iter().take(40) {
            info!(
                "    {path}  @  ({:+.4}, {:+.4}, {:+.4})",
                pos.x, pos.y, pos.z
            );
        }
        if near_origin.len() > 40 {
            info!("    … and {} more", near_origin.len() - 40);
        }
    }
    *done = true;
}

/// Recenter the arcball on whatever the USD projection spawned the moment
/// enough prims show up to have a valid bounding box. Runs exactly once so
/// the user can still orbit / pan afterwards.
fn fit_camera_once(
    extent: Res<SceneExtent>,
    mut cameras: Query<&mut ArcballCamera>,
    mut done: Local<bool>,
    mut wait_ticks: Local<u32>,
    mut last_diag: Local<f32>,
    prims: Query<(), With<UsdPrimRef>>,
) {
    if *done || extent.count == 0 || prims.iter().count() == 0 {
        return;
    }
    // Wait for the extent to stabilize. Bevy populates `Aabb` components
    // for skinned meshes only after the mesh asset is uploaded, which
    // happens a few frames after the prim entities first spawn. If we
    // frame the camera on the first available extent, assets that
    // don't author per-prim `extent` metadata (Apple's chameleon is
    // the canonical case) compute a 1cm scene diagonal from prim
    // origins alone — the camera zooms in to a point and the actual
    // mesh sits behind the camera.
    let diag = extent.diag();
    *wait_ticks += 1;
    if *wait_ticks < 60 && diag > *last_diag * 1.05 {
        // Extent is still growing — keep waiting.
        *last_diag = diag;
        return;
    }
    let Ok(mut cam) = cameras.single_mut() else {
        return;
    };
    cam.focus = extent.centre();
    // Skinned scenes (Apple AR / UsdSkel chameleon) don't author
    // per-prim extent metadata and Bevy doesn't populate `Aabb` for
    // skinned meshes (skinning happens in render, after the
    // CPU-side extent compute), so the diag we see can be ~0
    // even after waiting. Use a 2m fallback radius so the camera
    // at least frames a region the mesh likely fits in — the user
    // can scroll-out from there if the actual asset is bigger.
    let effective = diag.max(2.0);
    cam.distance = effective * 1.1;
    cam.max_distance = cam.distance.max(cam.max_distance) * 4.0;
    // Scale the zoom-in clamp to the scene size: 0.1% of the diagonal
    // floors at 1mm so a 100m greenhouse can still be inspected at
    // sub-cm detail and a 30cm asset doesn't refuse to zoom past 20cm
    // (the original 0.2m default). Matches the camera-distance scaling
    // above so dolly stops just before the mesh.
    cam.min_distance = (effective * 0.001).max(0.001);
    *done = true;
    info!(
        "camera framed on scene: focus={:?}, diag={:.2} m (effective={:.2} m), {} prims (waited {} ticks)",
        cam.focus,
        diag,
        effective,
        extent.count,
        *wait_ticks
    );
}

/// React to `AssetEvent::<UsdAsset>::Modified` (fired by Bevy's file
/// watcher when the source USD changes on disk). Despawn the existing
/// SceneRoot(s) and rerun the spawn path — the new scene handle inside
/// UsdAsset will differ, so `Spawned` gets reset.
fn handle_usd_hot_reload(
    mut events: MessageReader<AssetEvent<UsdAsset>>,
    mut commands: Commands,
    mut stage: Option<ResMut<StageHandle>>,
    scene_roots: Query<Entity, With<SceneRoot>>,
    mut spawned: ResMut<Spawned>,
    mut reload: ResMut<ReloadRequest>,
    asset_server: Res<AssetServer>,
    requested: Res<RequestedAsset>,
    tuning: Res<LoaderTuning>,
    mut usd_assets: ResMut<bevy::asset::Assets<UsdAsset>>,
) {
    let Some(stage) = stage.as_deref_mut() else {
        return;
    };

    // Automatic path: fired by Bevy's file watcher (when re-enabled).
    for event in events.read() {
        if matches!(event, AssetEvent::Modified { id } if *id == stage.0.id()) {
            info!("hot-reload: UsdAsset modified, respawning scene");
            for entity in &scene_roots {
                commands.entity(entity).despawn();
            }
            spawned.0 = false;
        }
    }

    // Manual path: R keypress or UI button flipped ReloadRequest.
    if reload.requested {
        reload.requested = false;
        for entity in &scene_roots {
            commands.entity(entity).despawn();
        }
        // Drop the previously-loaded UsdAsset so its handles can be
        // freed when the new load replaces stage.0.
        let old_id = stage.0.id();
        usd_assets.remove(old_id);

        let search = vec![requested.root.clone()];
        let kind_collapse = std::env::var("BEVY_OPENUSD_KIND_COLLAPSE")
            .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
            .unwrap_or(false);
        let radius = tuning.curves.default_radius;
        let rings = tuning.curves.ring_segments;
        let point_scale = tuning.curves.point_scale;
        let variant_selections = tuning.to_variant_selections();

        // Bevy's AssetServer caches handles by asset path. Calling
        // `load_with_settings` a second time with the same path just
        // returns the prior handle without re-running the loader
        // closure — even when our closure captures different settings.
        // Route each reload through a variant-keyed copy sitting NEXT
        // TO the source (so the asset-root gate Bevy enforces still
        // passes, and openusd's sibling-reference resolution keeps
        // working). Per-selection hashing makes the asset path unique,
        // forcing a fresh loader run.
        let source_path = requested.root.join(&requested.name);
        let variant_basename = unique_variant_basename(&source_path, &variant_selections);
        let variant_fs_path = requested.root.join(&variant_basename);
        if let Err(err) = ensure_variant_copy(&source_path, &variant_fs_path) {
            error!(
                "hot-reload: failed to materialize variant-keyed copy {}: {err}",
                variant_fs_path.display()
            );
            return;
        }

        info!(
            "hot-reload: manual reload of {} via {} (curve_radius={radius:.4}, curve_rings={rings}, point_scale={point_scale:.2}, variants={})",
            requested.name,
            variant_basename,
            variant_selections.len()
        );
        let handle: Handle<UsdAsset> = asset_server.load_with_settings::<UsdAsset, _>(
            // Relative asset name so Bevy's asset-root gate accepts it.
            variant_basename.clone(),
            move |s: &mut UsdLoaderSettings| {
                s.search_paths = search.clone();
                s.kind_collapse = kind_collapse;
                s.curve_default_radius = radius;
                s.curve_ring_segments = rings;
                s.point_scale = point_scale;
                s.variant_selections = variant_selections.clone();
            },
        );
        stage.0 = handle;
        spawned.0 = false;
    }
}

fn spawn_when_ready(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    assets: Res<Assets<UsdAsset>>,
    stage: Res<StageHandle>,
    mut spawned: ResMut<Spawned>,
    mut info: ResMut<StageInfo>,
    default_suns: Query<Entity, With<DefaultSun>>,
) {
    if spawned.0 {
        return;
    }
    match asset_server.get_load_state(&stage.0) {
        Some(LoadState::Loaded) => {
            // During a variant reload we remove the previous UsdAsset
            // from `Assets<UsdAsset>` to force the loader to re-run.
            // The AssetServer's load-state tracker can still report
            // `Loaded` for a frame or two before the new load actually
            // populates storage — treat that gap as "still loading".
            let Some(asset) = assets.get(&stage.0) else {
                return;
            };
            info!(
                "loaded UsdAsset: default_prim={:?}, layer_count={}, variants={} prims",
                asset.default_prim,
                asset.layer_count,
                asset.variants.len()
            );
            info.default_prim = asset.default_prim.clone();
            info.layer_count = asset.layer_count;
            info.variant_count = asset.variants.values().map(|sets| sets.len()).sum();
            info.lights_directional = asset.light_tally.directional;
            info.lights_point = asset.light_tally.point;
            info.lights_spot = asset.light_tally.spot;
            info.lights_dome = asset.light_tally.dome;
            info.instance_prim_count = asset.instance_prim_count;
            info.instance_prototype_reuses = asset.instance_prototype_reuses;
            // Fallback sun stays — we don't despawn based on USD
            // directional lights because authored intensities vary
            // by orders of magnitude across tools and often blow out
            // PBR exposure. The viewer's fixed 10k-illuminance sun
            // is the reliable baseline.
            let _ = &default_suns;
            info.animated_prim_count = asset.animated_prims.len();
            info.skeleton_count = asset.skeletons.len();
            info.skel_root_count = asset.skel_roots.len();
            info.skel_binding_count = asset.skel_bindings.len();
            info.render_settings_count = asset.render_settings.len();
            info.render_product_count = asset.render_products.len();
            info.render_var_count = asset.render_vars.len();
            let primary = asset.render_settings.first();
            info.render_primary_resolution = primary.and_then(|s| s.resolution);
            info.render_primary_path = primary.map(|s| s.path.clone());
            info.rigid_body_count = asset.rigid_body_prims.len();
            info.physics_scene_count = asset.physics_scene_prims.len();
            info.joint_count = asset.joints.len();
            info.custom_attr_prim_count = asset.custom_attrs.len();
            info.custom_layer_data_entries = asset.custom_layer_data.len();
            info.subdivision_prim_count = asset.subdivision_prims.len();
            info.light_linked_count = asset.light_linking_prims.len();
            info.clip_prim_count = asset.clip_sets.len();
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

/// Handle the Browse-USD file picker result. Reconfigures the running
/// AssetServer to the new root directory + requests a reload against
/// the new filename. Full AssetPlugin reconfiguration isn't supported
/// at runtime, so we instead swap the asset name + search paths
/// passed into `load_with_settings` — effective as long as the new
/// file's parent happens to fall under the existing default source.
/// For arbitrary paths, window title + behaviour update but the
/// AssetServer may resolve against the original root; cleanest
/// workaround is documenting that Browse-USD works best on files
/// under the initial root dir. Full dynamic-source swap lands with
/// the main-loop restart approach when needed.
fn apply_load_request(
    mut commands: Commands,
    mut req: ResMut<LoadRequest>,
    asset_server: Res<AssetServer>,
    scene_roots: Query<Entity, With<SceneRoot>>,
    mut stage: ResMut<StageHandle>,
    mut spawned: ResMut<Spawned>,
    mut info: ResMut<StageInfo>,
    mut requested: ResMut<RequestedAsset>,
    mut window: Query<&mut Window, With<bevy::window::PrimaryWindow>>,
) {
    let Some(new_path) = req.path.take() else {
        return;
    };

    let file = new_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| new_path.display().to_string());
    let root = new_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    info!("Browse: switching to {file} under {}", root.display());

    // Tear down current scene + reset load bookkeeping.
    for e in &scene_roots {
        commands.entity(e).despawn();
    }
    spawned.0 = false;
    info.path = file.clone();
    info.default_prim = None;
    info.layer_count = 0;
    info.variant_count = 0;

    // Update requested resource + re-issue load. Search paths include
    // the new root so sibling references work.
    requested.name = file.clone();
    requested.root = root.clone();
    let search = vec![root];

    let handle: Handle<UsdAsset> = asset_server.load_with_settings::<UsdAsset, _>(
        // Absolute path works here because Bevy's default AssetReader
        // falls back to an absolute-path source when the name
        // resolves to an existing file.
        new_path.to_string_lossy().into_owned(),
        move |s: &mut UsdLoaderSettings| {
            s.search_paths = search.clone();
        },
    );
    stage.0 = handle;

    if let Ok(mut w) = window.single_mut() {
        w.title = format!("bevy_openusd — {file}");
    }
}

/// Lerp the arcball's focus + distance toward the last-requested
/// target. Zero `remaining` is the sentinel "no tween in flight".
fn apply_fly_to(time: Res<Time>, mut fly: ResMut<FlyTo>, mut cameras: Query<&mut ArcballCamera>) {
    if fly.remaining <= 0.0 {
        return;
    }
    let Ok(mut cam) = cameras.single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(1.0 / 30.0);
    fly.remaining = (fly.remaining - dt).max(0.0);
    let progress = if fly.duration > 0.0 {
        1.0 - (fly.remaining / fly.duration).clamp(0.0, 1.0)
    } else {
        1.0
    };
    // Cosine ease-out.
    let eased = 1.0 - ((1.0 - progress) * core::f32::consts::FRAC_PI_2).cos();

    cam.focus = fly.start_focus.lerp(fly.target_focus, eased);
    cam.distance = fly
        .start_distance
        .lerp(fly.target_distance, eased)
        .max(cam.min_distance);

    // Bookmark restores set start/target yaw + elevation; pick the
    // shortest angular path so a 359° → 1° tween doesn't sweep the
    // long way round.
    if let (Some(sy), Some(ty)) = (fly.start_yaw, fly.target_yaw) {
        cam.yaw = lerp_angle(sy, ty, eased);
    }
    if let (Some(se), Some(te)) = (fly.start_elevation, fly.target_elevation) {
        cam.elevation = se + (te - se) * eased;
    }
}

fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let two_pi = core::f32::consts::TAU;
    let mut delta = (b - a) % two_pi;
    if delta > core::f32::consts::PI {
        delta -= two_pi;
    } else if delta < -core::f32::consts::PI {
        delta += two_pi;
    }
    a + delta * t
}

/// When `CameraMount::Mounted { prim_path }` is set, copy the USD camera
/// prim's `GlobalTransform` + projection onto the live `Camera3d` every
/// frame. Goes quiet in `CameraMount::Arcball` mode so the arcball runs
/// unopposed.
fn follow_mounted_camera(
    mount: Res<CameraMount>,
    usd_assets: Res<Assets<UsdAsset>>,
    stage: Option<Res<StageHandle>>,
    prims: Query<(&UsdPrimRef, &GlobalTransform)>,
    mut cameras: Query<(&mut Transform, &mut bevy::camera::Projection), With<Camera3d>>,
) {
    let CameraMount::Mounted { prim_path } = &*mount else {
        return;
    };
    let Some(stage) = stage else { return };
    let Some(asset) = usd_assets.get(&stage.0) else {
        return;
    };
    let Some(cam_data) = asset
        .cameras
        .iter()
        .find(|c| &c.path == prim_path)
        .map(|c| c.data.clone())
    else {
        return;
    };

    // Find the entity whose UsdPrimRef matches the mounted camera so we
    // can read its world transform. Every geom + xform prim gets a
    // UsdPrimRef, including Camera prims.
    let Some(gt) = prims
        .iter()
        .find(|(pr, _)| pr.path == *prim_path)
        .map(|(_, gt)| gt)
    else {
        return;
    };

    let Ok((mut tr, mut proj)) = cameras.single_mut() else {
        return;
    };

    let world = gt.compute_transform();
    tr.translation = world.translation;
    tr.rotation = world.rotation;
    // Leave scale alone — distorting cameras via scale is a footgun.

    // Swap projection to match the authored camera. `OrthographicProjection`
    // defaults tend to be too narrow for scene scale, so size from the
    // authored aperture.
    use bevy::camera::{OrthographicProjection, PerspectiveProjection, Projection};
    use usd_schemas::camera::Projection as UsdProj;
    match cam_data.projection.unwrap_or(UsdProj::Perspective) {
        UsdProj::Perspective => {
            let fov = cam_data
                .vertical_fov_rad()
                .clamp(0.1, core::f32::consts::PI - 0.1);
            let mut persp = PerspectiveProjection::default();
            persp.fov = fov;
            persp.aspect_ratio = cam_data.aspect_ratio().max(0.01);
            persp.near = cam_data.clip_near.unwrap_or(0.1).max(0.001);
            persp.far = cam_data.clip_far.unwrap_or(1.0e6);
            *proj = Projection::Perspective(persp);
        }
        UsdProj::Orthographic => {
            let mut ortho = OrthographicProjection::default_3d();
            ortho.near = cam_data.clip_near.unwrap_or(0.1);
            ortho.far = cam_data.clip_far.unwrap_or(1.0e6);
            *proj = Projection::Orthographic(ortho);
        }
    }
}

/// Live curve / point tuning. On every `CurveTuning` change (slider
/// move is enough), iterate the prim tree, look up each curve/points
/// prim's raw data on the loaded `UsdAsset`, and rebuild the mesh's
/// vertex buffers in place via `Assets<Mesh>::get_mut`. No asset
/// reload, no AssetServer-cache fight.
fn rebuild_tuned_meshes(
    tuning: Res<LoaderTuning>,
    stage: Option<Res<StageHandle>>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    prims: Query<(&UsdPrimRef, &bevy::mesh::Mesh3d)>,
    mut last: Local<Option<(f32, u32, f32)>>,
) {
    let Some(stage) = stage else { return };
    let Some(asset) = usd_assets.get(&stage.0) else {
        return;
    };
    let radius = tuning.curves.default_radius;
    let rings = tuning.curves.ring_segments;
    let point_scale = tuning.curves.point_scale;
    // egui's ResMut access fires `is_changed` every frame, so compare
    // the actual slider values — only rebuild when they really move.
    let key = (radius, rings, point_scale);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let mut rebuilt = 0usize;

    for (prim, mesh3d) in prims.iter() {
        if let Some(read) = asset.curves.get(&prim.path) {
            let new_mesh = bevy_openusd::curves::curves_mesh(read, radius, rings);
            if let Some(slot) = meshes.get_mut(&mesh3d.0) {
                *slot = new_mesh;
                rebuilt += 1;
            }
        } else if let Some(read) = asset.points_clouds.get(&prim.path) {
            let new_mesh = bevy_openusd::curves::points_mesh(read, point_scale);
            if let Some(slot) = meshes.get_mut(&mesh3d.0) {
                *slot = new_mesh;
                rebuilt += 1;
            }
        }
    }
    if rebuilt > 0 {
        info!(
            "tuning: rebuilt {rebuilt} curve/point mesh(es) (radius={radius:.4}, rings={rings}, point_scale={point_scale:.2})"
        );
    }
}

/// Draw a bright yellow AABB around the currently selected prim so the
/// user can visually locate the entity they clicked in the tree panel.
fn draw_selected_prim_highlight(
    selected: Res<SelectedPrim>,
    xforms: Query<&GlobalTransform>,
    aabbs: Query<&bevy::camera::primitives::Aabb>,
    mut gizmos: Gizmos,
) {
    let Some(entity) = selected.0 else {
        return;
    };
    let Ok(gt) = xforms.get(entity) else {
        return;
    };
    let origin = gt.translation();
    let color = Color::srgb(1.0, 0.9, 0.2);

    if let Ok(aabb) = aabbs.get(entity) {
        // Mesh AABB is in local space; transform corners into world.
        let half = Vec3::new(
            aabb.half_extents.x,
            aabb.half_extents.y,
            aabb.half_extents.z,
        );
        let centre_local = Vec3::new(aabb.center.x, aabb.center.y, aabb.center.z);
        let iso = gt.compute_transform();
        let corners = [
            Vec3::new(-half.x, -half.y, -half.z),
            Vec3::new(half.x, -half.y, -half.z),
            Vec3::new(half.x, half.y, -half.z),
            Vec3::new(-half.x, half.y, -half.z),
            Vec3::new(-half.x, -half.y, half.z),
            Vec3::new(half.x, -half.y, half.z),
            Vec3::new(half.x, half.y, half.z),
            Vec3::new(-half.x, half.y, half.z),
        ];
        let worldify = |v: Vec3| iso.translation + iso.rotation * ((v + centre_local) * iso.scale);
        let c: [Vec3; 8] = std::array::from_fn(|i| worldify(corners[i]));
        // 12 edges of the box.
        let edges = [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0), // bottom
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4), // top
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7), // sides
        ];
        for (a, b) in edges {
            gizmos.line(c[a], c[b], color);
        }
    } else {
        // No AABB (no Mesh3d): fall back to a small cross.
        let l = 0.2;
        gizmos.line(origin - Vec3::X * l, origin + Vec3::X * l, color);
        gizmos.line(origin - Vec3::Y * l, origin + Vec3::Y * l, color);
        gizmos.line(origin - Vec3::Z * l, origin + Vec3::Z * l, color);
    }
}

/// Wipe stale `.bevy_openusd_variant_<hash>.usda` copies left in the
/// asset root by prior viewer runs. Fires once at startup before
/// `load_stage` queues the initial load, so the subsequent fresh
/// copies are the only ones on disk.
fn sweep_variant_tempfiles(requested: Res<RequestedAsset>) {
    let Ok(entries) = std::fs::read_dir(&requested.root) else {
        return;
    };
    for entry in entries.flatten() {
        let name_os = entry.file_name();
        let Some(name) = name_os.to_str() else { continue };
        if name.starts_with(".bevy_openusd_variant_") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Build a per-variant-selection asset basename. Bevy's AssetServer
/// caches by asset path alone, so each distinct selection set needs a
/// distinct path to force a fresh loader run. The basename sits
/// alongside the real asset file so Bevy's asset-root gate accepts it.
fn unique_variant_basename(
    source: &std::path::Path,
    selections: &[bevy_openusd::VariantSelection],
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    source.hash(&mut h);
    for sel in selections {
        sel.prim_path.hash(&mut h);
        sel.set_name.hash(&mut h);
        sel.option.hash(&mut h);
    }
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("usda");
    format!(".bevy_openusd_variant_{:016x}.{ext}", h.finish())
}

/// Ensure `dest` exists and mirrors `source`'s bytes. We re-copy only
/// when the destination is missing or stale compared to the source's
/// modification time.
fn ensure_variant_copy(
    source: &std::path::Path,
    dest: &std::path::Path,
) -> std::io::Result<()> {
    let needs_copy = match (source.metadata(), dest.metadata()) {
        (Ok(s), Ok(d)) => match (s.modified().ok(), d.modified().ok()) {
            (Some(s_mt), Some(d_mt)) => s_mt > d_mt,
            _ => true,
        },
        _ => true,
    };
    if needs_copy {
        std::fs::copy(source, dest)?;
    }
    Ok(())
}

/// Advance `UsdStageTime.seconds` by the frame delta when `playing`.
/// Wraps back to the start on reaching the end so the animation loops
/// — most authored scenes are short cycles and the user can pause on
/// any frame with the timeline panel.
///
/// Also syncs `start`/`end`/`fps` from the loaded UsdAsset on first
/// sight so a fresh stage populates the clock's bounds.
fn tick_stage_time(
    time: Res<Time>,
    mut clock: ResMut<UsdStageTime>,
    stage: Option<Res<StageHandle>>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut end_hold_elapsed: Local<f64>,
) {
    // Pull stage timeline metadata once after the asset lands.
    if !clock.initialized
        && let Some(stage) = stage
        && let Some(asset) = usd_assets.get(&stage.0)
    {
        clock.start_time_code = asset.start_time_code;
        clock.end_time_code = asset.end_time_code;
        clock.time_codes_per_second = asset.time_codes_per_second;
        clock.seconds = 0.0;
        // Anything that animates over time → start with the clock
        // playing. Stage-resident SkelAnimations don't go through the
        // sidecar (USDC USDZs author them inline), so also key off
        // the authored stage timeline being non-trivial.
        clock.playing = asset.animated_prims.iter().next().is_some()
            || !asset.skel_animations.is_empty()
            || asset.end_time_code > asset.start_time_code;
        clock.initialized = true;
        info!(
            "stage time clock: start={:.2} end={:.2} fps={:.2} (duration {:.2}s) — {} animated prim(s), {} skel anim(s)",
            clock.start_time_code,
            clock.end_time_code,
            clock.time_codes_per_second,
            clock.duration_seconds(),
            asset.animated_prims.len(),
            asset.skel_animations.len()
        );
    }

    /// How long to sit at `endTimeCode` before wrapping back to
    /// `startTimeCode`. Held-interpolation samples authored at the
    /// endpoint would otherwise never render (linear sampling doesn't
    /// care). Matched to a typical keyframe interval so the last
    /// sample value looks on par with the others.
    const END_HOLD_SECONDS: f64 = 1.0;

    if clock.playing {
        clock.seconds += time.delta_secs_f64();
        let dur = clock.duration_seconds();
        if dur > 0.0 {
            if clock.seconds >= dur {
                *end_hold_elapsed += time.delta_secs_f64();
                if *end_hold_elapsed >= END_HOLD_SECONDS {
                    // Hold window over — wrap to the start.
                    clock.seconds = 0.0;
                    *end_hold_elapsed = 0.0;
                } else {
                    // Still within the hold — pin at the endpoint.
                    clock.seconds = dur;
                }
            } else {
                *end_hold_elapsed = 0.0;
            }
        }
    }
}

/// Re-evaluate animated xformOps for every prim in
/// `UsdAsset::animated_prims` and write the resulting `Transform`. Runs
/// every frame — cheap because only prims with authored timeSamples
/// are touched (the rest stay static at their load-time Transform).
fn evaluate_animated_prims(
    clock: Res<UsdStageTime>,
    stage: Option<Res<StageHandle>>,
    usd_assets: Res<Assets<UsdAsset>>,
    mut prims: Query<(&UsdPrimRef, &mut Transform)>,
) {
    let Some(stage) = stage else { return };
    let Some(asset) = usd_assets.get(&stage.0) else {
        return;
    };
    if asset.animated_prims.is_empty() {
        return;
    }
    let tc = clock.current_time_code();
    use usd_schemas::anim::eval_scalar_track;

    for (prim_ref, mut tr) in prims.iter_mut() {
        let Some(record) = asset.animated_prims.get(&prim_ref.path) else {
            continue;
        };
        // Single-axis rotates: if present, overwrite Transform.rotation
        // with a quat for the sampled degree value. We scope to the one
        // axis the prim authored (non-overlapping per USD spec).
        // `eval_scalar_track` dispatches linear vs held based on the
        // authored `interpolation` metadata.
        if let Some(track) = &record.rotate_y
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_y(deg.to_radians());
        }
        if let Some(track) = &record.rotate_x
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_x(deg.to_radians());
        }
        if let Some(track) = &record.rotate_z
            && let Some(deg) = eval_scalar_track(track, tc)
        {
            tr.rotation = bevy::math::Quat::from_rotation_z(deg.to_radians());
        }
    }
}

/// Per-frame driver for `UsdSkelAnimation`. For each `UsdSkelAnimDriver`
/// component (one per SkelRoot with a matched sidecar animation):
///
/// 1. Evaluate the animation at the current stage time → one
///    `EvaluatedJoint` per channel.
/// 2. Map each channel to its skeleton joint entity (already
///    pre-resolved at load time and stored in
///    `driver.joint_entities[i]`) and apply the evaluated translation
///    / rotation / scale to that joint's local `Transform`.
///
/// Skips the eval when no driver is present (no animation loaded), so
/// the cost on non-animated scenes is one `Query::iter()` per frame.
fn drive_skel_animations(
    clock: Res<UsdStageTime>,
    drivers: Query<&bevy_openusd::prim_ref::UsdSkelAnimDriver>,
    mut joints: Query<&mut Transform>,
    mut diag_emitted: Local<bool>,
    mut tick: Local<u32>,
) {
    let tc = clock.current_time_code();
    *tick += 1;
    for driver in drivers.iter() {
        let evaluated = bevy_openusd::skel_anim::evaluate(driver, tc);
        let mut hits = 0usize;
        let mut misses = 0usize;
        // Sample joint 0's local Transform translation BEFORE applying
        // — to verify driver actually changes values across frames.
        let probe_je = driver
            .joint_entities
            .iter()
            .skip(10)
            .find_map(|e| *e);
        let before: Option<(bevy::math::Vec3, bevy::math::Quat)> = probe_je.and_then(|je| {
            joints.get(je).ok().map(|t| (t.translation, t.rotation))
        });
        for (channel_ix, joint_entity) in driver.joint_entities.iter().enumerate() {
            let Some(je) = joint_entity else {
                misses += 1;
                continue;
            };
            let Ok(mut tr) = joints.get_mut(*je) else {
                misses += 1;
                continue;
            };
            evaluated[channel_ix].apply(&mut tr);
            hits += 1;
        }
        let after: Option<(bevy::math::Vec3, bevy::math::Quat)> = probe_je.and_then(|je| {
            joints.get(je).ok().map(|t| (t.translation, t.rotation))
        });
        if !*diag_emitted && (hits > 0 || misses > 0) {
            info!(
                "skel anim: first-tick wrote {hits}/{} joints (missed {misses}); probe before={before:?} after={after:?} tc={tc:.2}",
                driver.joint_entities.len()
            );
            *diag_emitted = true;
        } else if *tick % 30 == 0 {
            info!(
                "skel anim tick={} tc={tc:.2} probe={after:?}",
                *tick
            );
        }
    }
}

/// Custom gizmo group for the skeleton overlay. Configured at
/// startup with `depth_bias = -1.0` so bone lines render in front of
/// the skin mesh — without that, a hummingbird rig is invisible
/// inside its own body.
#[derive(Default, Reflect, GizmoConfigGroup)]
struct SkeletonGizmos;

fn setup_skeleton_gizmos_on_top(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<SkeletonGizmos>();
    cfg.depth_bias = -1.0;
}


/// Draw bone gizmos for every UsdSkel skeleton in the scene: a line
/// from each `UsdJoint` to each of its `UsdJoint` children. Drives
/// off `DisplayToggles.show_skeleton` (UI toggle + B hotkey) — and
/// also stays on whenever the legacy `BEVY_OPENUSD_JOINT_GIZMOS`
/// env var is set, in which case we additionally drop a green
/// sphere at every joint origin (the env-var path is the engine
/// debug view; the toggle path is what users actually want).
fn draw_joint_gizmos(
    mut gizmos: Gizmos<SkeletonGizmos>,
    joints: Query<(&GlobalTransform, Option<&Children>), With<bevy_openusd::prim_ref::UsdJoint>>,
    children_q: Query<&GlobalTransform, With<bevy_openusd::prim_ref::UsdJoint>>,
    flag: Res<ShowJointGizmosFlag>,
    toggles: Res<crate::overlays::DisplayToggles>,
) {
    let want_bones = toggles.show_skeleton || flag.0;
    let want_spheres = flag.0;
    if !want_bones {
        return;
    }
    for (parent_gt, children) in joints.iter() {
        let parent_pos = parent_gt.translation();
        if want_spheres {
            gizmos.sphere(parent_pos, 0.01, bevy::color::palettes::tailwind::LIME_400);
        }
        if let Some(children) = children {
            for child in children.iter() {
                if let Ok(child_gt) = children_q.get(child) {
                    let child_pos = child_gt.translation();
                    gizmos.line(
                        parent_pos,
                        child_pos,
                        bevy::color::palettes::tailwind::CYAN_400,
                    );
                }
            }
        }
    }
}

/// Diagnostic: when `BEVY_OPENUSD_HIDE_MESHES=1` is set, hide every
/// mesh entity so the user only sees the skeleton via the gizmos
/// system. Lets us answer "is the rig animating?" without the
/// visual noise of broken skinning.
fn hide_meshes_on_startup(
    flag: Res<HideMeshesFlag>,
    mut q: Query<&mut Visibility, With<bevy::mesh::Mesh3d>>,
    mut done: Local<bool>,
) {
    if !flag.0 || *done {
        return;
    }
    let mut count = 0;
    for mut v in q.iter_mut() {
        *v = Visibility::Hidden;
        count += 1;
    }
    if count > 0 {
        info!("hide_meshes: hid {count} mesh entities (BEVY_OPENUSD_HIDE_MESHES=1)");
        *done = true;
    }
}

#[derive(Resource)]
struct HideMeshesFlag(bool);

#[derive(Resource)]
struct ShowJointGizmosFlag(bool);

/// Per-frame driver for blend-shape weights. Reads
/// `UsdSkelAnimDriver`'s blendShapeWeights, looks up each mesh's
/// per-target name in the anim's `blend_shape_names`, and writes
/// the matching weight into `MeshMorphWeights`. Missing names get
/// weight 0.
///
/// For multi-skel scenes this picks the first driver — single-rig
/// is the common case (HumanFemale, etc.).
fn drive_blend_shape_weights(
    clock: Res<UsdStageTime>,
    drivers: Query<&bevy_openusd::prim_ref::UsdSkelAnimDriver>,
    mut meshes: Query<(
        &bevy_openusd::prim_ref::UsdBlendShapeBinding,
        &mut bevy::mesh::morph::MeshMorphWeights,
    )>,
    mut diag_emitted: Local<bool>,
) {
    let Some(driver) = drivers.iter().next() else {
        return;
    };
    let tc = clock.current_time_code();
    let evaluated = bevy_openusd::skel_anim::evaluate_blend_shapes(driver, tc);
    if !*diag_emitted {
        let nz: usize = evaluated.iter().filter(|w| w.abs() > 1e-4).count();
        let mx: f32 = evaluated.iter().map(|w| w.abs()).fold(0.0_f32, f32::max);
        info!(
            "blend anim: evaluated {} weights at tc={:.1}, nonzero={nz}, max={mx:.3}",
            evaluated.len(),
            tc
        );
    }
    if evaluated.is_empty() {
        return;
    }
    // Build a lookup table from blend-shape name → weight index
    // (one-time per-frame cost; the names list is short relative to
    // mesh count).
    let name_to_ix: std::collections::HashMap<&str, usize> = driver
        .blend_shape_names
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    // Debug override: BEVY_OPENUSD_BLEND_DEBUG=1 forces all morph
    // weights to 1.0 so we can verify the GPU morph path is wired
    // independently of the anim's weight values. If meshes change
    // shape with this set but not without, the issue is the anim →
    // weight mapping; if they don't change either, the morph
    // rendering itself isn't applying.
    let force_all = std::env::var("BEVY_OPENUSD_BLEND_DEBUG")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
        .unwrap_or(false);
    let mut total_meshes = 0usize;
    let mut total_targets = 0usize;
    for (binding, mut weights) in meshes.iter_mut() {
        total_meshes += 1;
        let buf = weights.weights_mut();
        for (slot, name) in binding.names.iter().enumerate() {
            if slot >= buf.len() {
                break;
            }
            buf[slot] = if force_all {
                1.0
            } else {
                name_to_ix
                    .get(name.as_str())
                    .and_then(|i| evaluated.get(*i))
                    .copied()
                    .unwrap_or(0.0)
            };
            total_targets += 1;
        }
    }
    if !*diag_emitted && total_meshes > 0 {
        // Sample first weight buffer to confirm we're writing
        // non-zero values + the underlying mesh asset has morph
        // targets attached.
        let mut sample_nonzero = 0;
        let mut sample_max = 0.0f32;
        let mut sample_buf_len = 0;
        for (_, weights) in meshes.iter().take(1) {
            sample_buf_len = weights.weights().len();
            for w in weights.weights() {
                if w.abs() > 1e-4 {
                    sample_nonzero += 1;
                }
                sample_max = sample_max.max(w.abs());
            }
        }
        info!(
            "blend anim: drove {total_meshes} meshes, {total_targets} targets across {} anim channels; sample buf len={sample_buf_len} nonzero={sample_nonzero} max={sample_max:.3}",
            evaluated.len()
        );
        *diag_emitted = true;
    }
}
