//! `UsdPrimRef` — the bridge between a Bevy entity and the USD prim path it
//! was projected from.
//!
//! Entities spawned by the USD stage walker carry a `UsdPrimRef` so ECS-side
//! code can ask "what prim was this?" and downstream milestones can re-open
//! the composed stage for live queries, variant switching, or re-projection.

use bevy::ecs::component::Component;
use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::{Reflect, std_traits::ReflectDefault};

/// Stores the USD prim path an entity was projected from.
///
/// The path is the composed absolute `sdf::Path` stringified (e.g.
/// `/World/Robot/base_link`). Kept as a `String` for M1 to stay
/// `Reflect`-friendly; a typed wrapper around `openusd::sdf::Path` can land
/// once we decide how to reflect the openusd types.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq, Hash)]
#[reflect(Component, Default)]
pub struct UsdPrimRef {
    /// Absolute composed prim path, e.g. `"/World/ChildA"`.
    pub path: String,
}

impl UsdPrimRef {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }
}

/// Marker on entities that represent a single joint of a UsdSkel
/// `Skeleton`. `path` is the joint's authored token path
/// (`"Root/Hip/Knee"`); `index` is its position in the parent
/// Skeleton's `joints` array — which is the index every per-joint
/// attribute and every UsdSkelAnimation channel uses. Stored so the
/// future animation playback system can drive the right joint
/// without re-walking the hierarchy.
#[derive(Component, Reflect, Debug, Clone)]
#[reflect(Component)]
pub struct UsdJoint {
    pub path: String,
    pub index: u32,
}

/// Marker on a skinned mesh entity that names the BlendShapes the
/// mesh's `MorphWeights` map to. Each entry is the BlendShape's
/// authored token (matching the SkelAnimation's `blendShapes`).
/// The runtime `drive_blend_shapes` system uses this to look up the
/// current weight for each blend target the mesh references.
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdBlendShapeBinding {
    /// One entry per morph target on the mesh, in the same order
    /// the mesh's `MorphTargetImage` was built. The string is the
    /// blend-shape name; matched against the SkelAnimation's
    /// `blendShapes` to fetch the per-frame weight.
    pub names: Vec<String>,
}

/// Per-skeleton animation driver. Attached to the SkelRoot entity
/// when a sidecar `UsdSkelAnimation` was matched against the SkelRoot's
/// authored `skel:animationSource` (or env-var override). Each frame
/// the `drive_skel_animations` system reads `UsdStageTime`, finds the
/// bracketing keyframes, slerps/lerps to a per-joint local transform,
/// and writes those into the joint entities listed in `joint_entities`.
///
/// `joint_entities[i]` is the Bevy entity for the skeleton joint that
/// corresponds to animation channel index `i` (after remapping by joint
/// name from animation joints → skeleton joints; channels with no
/// matching skeleton joint are stored as `None` and skipped at drive
/// time).
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdSkelAnimDriver {
    /// One entry per animation channel (i.e. animation's `joints`
    /// array length). `None` when the animation references a joint
    /// the bound Skeleton doesn't have. `#[entities]` tells the
    /// scene reloader to remap these on respawn.
    #[entities]
    pub joint_entities: Vec<Option<bevy::ecs::entity::Entity>>,
    /// Sorted-by-time copy of the animation's translations.
    pub translations: Vec<(f64, Vec<[f32; 3]>)>,
    /// Sorted-by-time copy of the animation's rotations. Element
    /// order is the asset's authored convention — see
    /// `quat_xyzw_order` for whether the array is (w, x, y, z)
    /// (Pixar spec) or (x, y, z, w) (Apple AR Quick Look exporter).
    pub rotations: Vec<(f64, Vec<[f32; 4]>)>,
    /// `true` when the asset stores quaternions as
    /// `[x, y, z, w]` (Apple Quick Look convention).
    /// `false` for the canonical Pixar `[w, x, y, z]` order.
    /// Auto-detected at load time by comparing first-keyframe
    /// quaternion magnitudes against rest-pose magnitudes.
    pub quat_xyzw_order: bool,
    /// Sorted-by-time copy of the animation's scales.
    pub scales: Vec<(f64, Vec<[f32; 3]>)>,
    /// SkelAnimation's `blendShapes` token list — names that the
    /// per-time `blend_shape_weights` arrays line up with.
    pub blend_shape_names: Vec<String>,
    /// Sorted-by-time blendShapeWeights — `weights[i]` corresponds
    /// to `blend_shape_names[i]`.
    pub blend_shape_weights: Vec<(f64, Vec<f32>)>,
}

/// Marker on the Bevy entity that represents a UsdSkel `SkelRoot`.
/// Future skinning code uses this to find the joint hierarchy + the
/// authored animationSource rel.
#[derive(Component, Reflect, Debug, Clone, Default)]
#[reflect(Component, Default)]
pub struct UsdSkelRoot {
    /// Composed prim path of the resolved Skeleton (target of
    /// `skel:skeleton` on the SkelRoot or any descendant Mesh).
    pub skeleton_path: String,
    /// Composed prim path of the resolved animationSource (target of
    /// `skel:animationSource`). Empty when none authored.
    pub animation_source_path: String,
}

/// Authored `UsdGeomBoundable.extent` — `[min, max]` corners in the
/// prim's local space. Attached to prim entities when the USD stage
/// provides an explicit bounding box, so downstream systems can skip
/// walking vertex data for scene-extent / culling work.
#[derive(Component, Reflect, Debug, Clone, Copy, PartialEq)]
#[reflect(Component)]
pub struct UsdLocalExtent {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Default for UsdLocalExtent {
    fn default() -> Self {
        Self {
            min: [0.0; 3],
            max: [0.0; 3],
        }
    }
}

/// Authored `UsdModelAPI.kind` — one of `"model" | "group" | "assembly"
/// | "component" | "subcomponent"` (or a custom token). Only attached
/// to prims that explicitly authored `kind`; unauthored prims go
/// without the component.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct UsdKind {
    pub kind: String,
}

/// Authored `ui:displayName` (UsdUISceneGraphPrimAPI). Friendly label
/// the viewer shows in its prim tree instead of the prim's leaf name.
/// Only attached to prims that authored a non-empty `ui:displayName`.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct UsdDisplayName(pub String);

/// Tag on `UsdMediaSpatialAudio` prims. Carries the authored
/// `filePath` + playback / aural-mode tokens; downstream consumers
/// either ignore it (read-side only, like the viewer today) or wire
/// a real audio backend that spawns a `bevy_audio` source positioned
/// at the prim's transform.
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct UsdSpatialAudio {
    pub file_path: Option<String>,
    pub aural_mode: Option<String>,
    pub playback_mode: Option<String>,
    pub gain: Option<f64>,
}

/// Tag on `UsdProcGenerativeProcedural` (and its subclasses) prims.
/// Carries the procedural's identifier so the viewer can show that
/// the prim is procedural even though we can't execute it without
/// the engine (Houdini, Renderman, …).
#[derive(Component, Reflect, Debug, Clone, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct UsdProcedural {
    pub procedural_type: Option<String>,
    pub procedural_system: Option<String>,
}

/// Authored `custom` attributes on a prim (including namespaced
/// `userProperties:*`) plus the prim's `customData` and `assetInfo`
/// dictionaries. Stored on `UsdAsset` keyed by prim path.
///
/// Each authored scalar / vector / array has a typed representation
/// (`CustomAttrValue`) plus convenience accessors (`get_float` /
/// `get_string` / `get_vec3` / `namespaced` / `iter_prefix`).
#[derive(Debug, Clone, Default)]
pub struct UsdCustomAttrs {
    /// Flat `(name, value)` list of authored `custom` attributes.
    /// Names are whatever the author wrote (`userProperties:max_speed`,
    /// `arena:tint`, `my_custom_thing`, …).
    pub entries: Vec<(String, usd_schemas::geom::CustomAttrValue)>,
    /// Authored `customData = { ... }` dictionary on the prim itself.
    /// Empty when the prim didn't author one.
    pub custom_data: usd_schemas::geom::CustomDict,
    /// Authored `assetInfo = { ... }` dictionary — identity metadata
    /// package-management tools stamp onto a prim.
    pub asset_info: usd_schemas::geom::CustomDict,
}

impl UsdCustomAttrs {
    /// Raw by-name lookup on the flat attribute list.
    pub fn get(&self, name: &str) -> Option<&usd_schemas::geom::CustomAttrValue> {
        self.entries
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.custom_data.is_empty() && self.asset_info.is_empty()
    }

    // ── Typed-convenience accessors ────────────────────────────────

    pub fn get_bool(&self, name: &str) -> Option<bool> {
        self.get(name)?.as_bool()
    }
    pub fn get_int(&self, name: &str) -> Option<i64> {
        self.get(name)?.as_int()
    }
    pub fn get_float(&self, name: &str) -> Option<f64> {
        self.get(name)?.as_float()
    }
    pub fn get_string(&self, name: &str) -> Option<&str> {
        self.get(name)?.as_str()
    }
    pub fn get_vec2(&self, name: &str) -> Option<[f32; 2]> {
        self.get(name)?.as_vec2()
    }
    pub fn get_vec3(&self, name: &str) -> Option<[f32; 3]> {
        self.get(name)?.as_vec3()
    }
    pub fn get_vec4(&self, name: &str) -> Option<[f32; 4]> {
        self.get(name)?.as_vec4()
    }
    pub fn get_dict(&self, name: &str) -> Option<&usd_schemas::geom::CustomDict> {
        self.get(name)?.as_dict()
    }

    // ── Namespace queries ──────────────────────────────────────────

    /// Return all attributes whose name starts with `prefix` (typically
    /// a namespace like `"userProperties:"`), with the prefix stripped
    /// so callers see `max_speed` not `userProperties:max_speed`.
    pub fn namespaced<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a usd_schemas::geom::CustomAttrValue)> + 'a {
        self.entries.iter().filter_map(move |(name, val)| {
            name.strip_prefix(prefix).map(|short| (short, val))
        })
    }

    /// Raw prefix iteration (keeps the full name).
    pub fn iter_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Iterator<Item = &'a (String, usd_schemas::geom::CustomAttrValue)> + 'a {
        self.entries.iter().filter(move |(n, _)| n.starts_with(prefix))
    }
}
