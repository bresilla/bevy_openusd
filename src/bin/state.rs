//! Shared viewer state: which panel is open, stage metadata snapshot,
//! viewer-level UX requests (reload / fly-to / swap-asset).

use bevy::prelude::{Entity, Resource, Vec3};
use std::path::PathBuf;

#[derive(Resource, Default, Debug, Clone)]
pub struct StageInfo {
    pub path: String,
    pub default_prim: Option<String>,
    pub layer_count: usize,
    pub variant_count: usize,
    /// UsdLux totals captured at load time (M9).
    pub lights_directional: usize,
    pub lights_point: usize,
    pub lights_spot: usize,
    pub lights_dome: usize,
    /// Prims marked `instanceable = true` and, of those, how many were
    /// recognised as reuses of a prototype we'd already built (M14).
    pub instance_prim_count: usize,
    pub instance_prototype_reuses: usize,
    /// Prims whose xformOps carry timeSamples (M15).
    pub animated_prim_count: usize,
    /// UsdSkel totals (M16 read side).
    pub skeleton_count: usize,
    pub skel_root_count: usize,
    pub skel_binding_count: usize,
    /// UsdRender totals + primary resolution (M19 read side).
    pub render_settings_count: usize,
    pub render_product_count: usize,
    pub render_var_count: usize,
    pub render_primary_resolution: Option<[i32; 2]>,
    pub render_primary_path: Option<String>,
    /// UsdPhysics totals (M_LAST read side).
    pub rigid_body_count: usize,
    pub physics_scene_count: usize,
    pub joint_count: usize,
    /// Custom-attribute / customData / assetInfo coverage (M24).
    pub custom_attr_prim_count: usize,
    pub custom_layer_data_entries: usize,
    /// Subdivision-surface meshes (M25).
    pub subdivision_prim_count: usize,
    /// Lights with authored `light:link` rels (M26).
    pub light_linked_count: usize,
    /// Prims carrying `UsdClipsAPI` metadata (M27).
    pub clip_prim_count: usize,
}

/// Flipped to `true` by the keyboard handler (R key) or the UI panel to
/// request a force-reload of the current USD asset. The main-loop's
/// `handle_usd_hot_reload` system reacts on the next frame.
#[derive(Resource, Default, Debug, Clone)]
pub struct ReloadRequest {
    pub requested: bool,
}

/// Swap the loaded asset at runtime. Set by the Browse-USD file picker.
/// On the next frame, the viewer despawns current SceneRoots + updates
/// the RequestedAsset + re-registers the AssetPlugin search roots.
#[derive(Resource, Default, Debug, Clone)]
pub struct LoadRequest {
    pub path: Option<PathBuf>,
}

/// Currently-selected prim (clicked in the Tree panel). The highlight
/// system reads this; the fly-to system watches for changes.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct SelectedPrim(pub Option<Entity>);

/// An in-flight camera tween. `remaining` counts down by `delta_time`
/// every frame until zero, at which point the camera settles at
/// `target_focus` / `target_distance`. The yaw / elevation pairs are
/// optional — set them only when restoring a bookmark; tree-click
/// fly-tos leave them unset so the user's current orbit is preserved.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct FlyTo {
    pub target_focus: Vec3,
    pub target_distance: f32,
    pub remaining: f32,
    pub duration: f32,
    pub start_focus: Vec3,
    pub start_distance: f32,
    pub start_yaw: Option<f32>,
    pub target_yaw: Option<f32>,
    pub start_elevation: Option<f32>,
    pub target_elevation: Option<f32>,
}

/// Saved camera viewpoints — `Cameras` panel wires "Save current view"
/// + a list of named bookmarks. Session-only for now; persistence is
/// a future concern.
#[derive(Resource, Default, Debug, Clone)]
pub struct CameraBookmarks {
    pub items: Vec<CameraBookmark>,
    pub next_seq: u32,
}

#[derive(Debug, Clone)]
pub struct CameraBookmark {
    pub name: String,
    pub focus: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub elevation: f32,
}

/// Which camera the viewer is looking through. `Arcball` means our own
/// free camera drives the view; `Mounted` means we've clamped the
/// Camera3d to the transform + projection of a USD `Camera` prim.
#[derive(Resource, Debug, Clone, Default)]
pub enum CameraMount {
    #[default]
    Arcball,
    Mounted {
        /// `UsdPrimRef.path` of the authored camera — identifies the
        /// entity we copy Transform + projection from each frame.
        prim_path: String,
    },
}

/// Live knobs the viewer passes to `UsdLoaderSettings` on every load
/// / reload: curve + point rendering defaults plus any variant-selection
/// overrides authored in the Variants panel. Bundled into one Resource
/// so systems that need both (load_stage, handle_usd_hot_reload,
/// draw_panel) stay under Bevy's 16-param limit.
#[derive(Resource, Debug, Clone, Default)]
pub struct LoaderTuning {
    pub curves: CurveTuning,
    /// `(prim_path, set_name) → selected option`. Empty = honour the
    /// stage's authored selections.
    pub variants: std::collections::HashMap<(String, String), String>,
}

impl LoaderTuning {
    pub fn to_variant_selections(&self) -> Vec<bevy_openusd::VariantSelection> {
        self.variants
            .iter()
            .map(|((prim_path, set_name), option)| bevy_openusd::VariantSelection {
                prim_path: prim_path.clone(),
                set_name: set_name.clone(),
                option: option.clone(),
            })
            .collect()
    }
}

/// Curve / point rendering defaults. Not a Resource on its own — lives
/// inside [`LoaderTuning`]. Split so the rebuild-tuned-meshes system
/// can diff a lightweight `Copy` key without cloning the variant map.
#[derive(Debug, Clone, Copy)]
pub struct CurveTuning {
    pub default_radius: f32,
    pub ring_segments: u32,
    pub point_scale: f32,
}

impl Default for CurveTuning {
    fn default() -> Self {
        Self {
            default_radius: 0.02,
            ring_segments: 6,
            point_scale: 1.0,
        }
    }
}

/// Animation playback clock. Ticks up by `delta_time` every frame when
/// `playing`, wraps back to `start` on reaching `end`. Held values are
/// in SECONDS; the per-frame evaluator converts to timeCodes using the
/// stage's authored `timeCodesPerSecond`.
#[derive(Resource, Debug, Clone, Copy)]
pub struct UsdStageTime {
    pub seconds: f64,
    pub playing: bool,
    pub start_time_code: f64,
    pub end_time_code: f64,
    pub time_codes_per_second: f64,
    /// Latched true on the frame we first sync from `UsdAsset`; avoids
    /// clobbering user scrubs on every reload.
    pub initialized: bool,
}

impl Default for UsdStageTime {
    fn default() -> Self {
        Self {
            seconds: 0.0,
            playing: false,
            start_time_code: 0.0,
            end_time_code: 1.0,
            time_codes_per_second: 24.0,
            initialized: false,
        }
    }
}

impl UsdStageTime {
    pub fn current_time_code(&self) -> f64 {
        self.start_time_code + self.seconds * self.time_codes_per_second
    }
    pub fn duration_seconds(&self) -> f64 {
        (self.end_time_code - self.start_time_code).max(0.0) / self.time_codes_per_second
    }
}
