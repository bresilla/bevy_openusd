//! `usd_bevy` — load OpenUSD into Bevy + drive a Rapier f64 world.
//!
//! Single Bevy crate that subsumes:
//! - the asset loader (`UsdAsset` / `UsdLoader`) and scene projection,
//! - the marker components (`markers::*` — internal module),
//! - the Rapier physics adapter (`physics::*` — wraps `usd_rapier`).

mod asset;
mod build;
pub mod curves;
pub mod mesh;
pub mod nurbs_patch;
pub mod tetmesh;
mod light;
pub mod markers;
mod material;
pub(crate) mod physics_attach;
pub mod physics;
pub mod prim_ref;
pub mod skel_anim;
mod texture;

pub use asset::{
    LightTally, StageCamera, UsdAsset, UsdLoader, UsdLoaderError, UsdLoaderSettings,
    VariantSelection, VariantSet, author_variant_session_layer, parse_variant_label,
    variant_label,
};
pub use mesh::{mesh_from_usd, mesh_from_usd_subset};
// Marker components are part of this crate's public API. Living in
// `markers` keeps the source organised; `pub use` here flattens the
// import path for downstream callers (intra-crate API surface, not a
// shim around an upstream crate).
pub use markers::*;
pub use prim_ref::{
    UsdCustomAttrs, UsdDisplayName, UsdKind, UsdLocalExtent, UsdPrimRef, UsdProcedural,
    UsdPurpose, UsdSpatialAudio,
};

use bevy::app::{App, Plugin};
use bevy::asset::AssetApp;
use bevy::scene::Scene;

/// Registers the [`UsdAsset`] type, the [`UsdLoader`], the
/// `UsdPrimRef` reflect registration, and every marker component
/// from [`markers`] so projected scenes clone through `SceneRoot`.
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
            .register_type::<UsdPurpose>()
            .register_type::<prim_ref::UsdSkelAnimDriver>()
            .register_type::<prim_ref::UsdBlendShapeBinding>()
            .register_type::<markers::UsdPhysicsScene>()
            .register_type::<markers::UsdRigidBody>()
            .register_type::<markers::UsdMass>()
            .register_type::<markers::UsdCollider>()
            .register_type::<markers::UsdColliderShape>()
            .register_type::<markers::UsdCollisionApprox>()
            .register_type::<markers::UsdPhysicsMaterial>()
            .register_type::<markers::UsdArticulationRoot>()
            .register_type::<markers::UsdPhysicsJoint>()
            .register_type::<markers::UsdJointKind>()
            .register_type::<markers::UsdJointLimit>()
            .register_type::<markers::UsdJointDrive>()
            .register_type::<markers::UsdDof>()
            .register_type::<markers::UsdDriveType>()
            .register_type::<markers::UsdCollisionGroup>()
            .register_type::<markers::UsdCollisionFilter>();
    }
}
