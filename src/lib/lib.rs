//! `bevy_openusd` — load OpenUSD (`.usda`, `.usdc`, `.usdz`) as native Bevy
//! scenes.
//!
//! M1 deliverable: the loader parses a stage and projects it into a
//! [`bevy::scene::Scene`] — one entity per composed prim, linked via
//! `ChildOf`, each carrying `Name`, `Transform::IDENTITY`, and a
//! [`UsdPrimRef`] component. Scene root applies the upAxis / metersPerUnit
//! basis fix.
//!
mod asset;
mod build;
pub mod curves;
pub mod nurbs_patch;
pub mod tetmesh;
mod light;
mod material;
pub mod mesh;
pub mod prim_ref;
mod texture;

pub use mesh::{mesh_from_usd, mesh_from_usd_subset};

pub use asset::{
    LightTally, StageCamera, UsdAsset, UsdLoader, UsdLoaderError, UsdLoaderSettings,
    VariantSelection, VariantSet, author_variant_session_layer,
};
pub use prim_ref::{
    UsdCustomAttrs, UsdDisplayName, UsdKind, UsdLocalExtent, UsdPrimRef, UsdProcedural,
    UsdSpatialAudio,
};

use bevy::app::{App, Plugin};
use bevy::asset::AssetApp;
use bevy::scene::Scene;

/// Registers the [`UsdAsset`] type, the [`UsdLoader`], and the `UsdPrimRef`
/// reflect registration so projected scenes clone through `SceneRoot`.
///
/// Also `init_asset::<Scene>()` so the labeled sub-scene the loader emits
/// resolves even in app setups that skip `ScenePlugin` (e.g. headless
/// unit tests). Actually *spawning* a scene still needs `ScenePlugin`
/// (which `DefaultPlugins` adds automatically).
#[derive(Default)]
pub struct UsdPlugin;

impl Plugin for UsdPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<Scene>()
            .init_asset::<UsdAsset>()
            .init_asset_loader::<UsdLoader>()
            .register_type::<UsdPrimRef>()
            .register_type::<UsdLocalExtent>()
            .register_type::<UsdKind>()
            .register_type::<prim_ref::UsdSkelAnimDriver>()
            .register_type::<prim_ref::UsdBlendShapeBinding>();
    }
}

/// Pure-logic helpers for evaluating a `UsdSkelAnimDriver` at a
/// given stage time. Produced here so the viewer (or any custom app)
/// can wire its own `Update` system without re-implementing
/// keyframe-bracket lookup + lerp/slerp.
pub mod skel_anim;
