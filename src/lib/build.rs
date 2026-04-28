//! Stage → Scene projection.
//!
//! Walks a composed [`openusd::Stage`] depth-first and builds a Bevy
//! [`Scene`] — one entity per `SpecType::Prim`, linked via `ChildOf`. Geom
//! prims (`Mesh` / `Cube` / `Sphere` / `Cylinder` / `Capsule`) get a
//! `Mesh3d` + `MeshMaterial3d` with a default flat-gray `StandardMaterial`.
//!
//! Mesh handles are de-duplicated by prim path: if two sites reference the
//! same MeshLibrary prim via USD internal references, both sites end up
//! sharing a single `Handle<Mesh>`.
//!
//! Root basis fix: if the stage's root layer sets `upAxis = "Z"` or
//! `metersPerUnit ≠ 1.0`, the scene root carries a single `Transform` that
//! baselines Bevy's Y-up / metre conventions.

use std::collections::HashMap;

use bevy::asset::LoadContext;
use bevy::ecs::entity::Entity;
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::name::Name;
use bevy::ecs::world::World;
use bevy::math::{Quat, Vec3};
use bevy::mesh::{Mesh, Mesh3d};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::Visibility;
use bevy::scene::Scene;
use bevy::transform::components::Transform;
use openusd::Stage;
use openusd::sdf::{Path, SpecType};
use usd_schemas::geom as ugeom;
use usd_schemas::xform as uxf;

use crate::curves::{curves_mesh, hermite_to_read_curves, nurbs_to_read_curves, points_mesh};
use crate::nurbs_patch::nurbs_patch_to_bevy_mesh;
use crate::tetmesh::tetmesh_to_bevy_mesh;
use crate::light::{Tally as LightTally, spawn_light};
use crate::material::{add_material_labeled, default_material, standard_material_from_usd};
use crate::mesh::{
    mesh_capsule, mesh_cube, mesh_cylinder, mesh_from_usd, mesh_from_usd_subset,
    mesh_from_usd_subset_with_skin, mesh_from_usd_with_skin, mesh_plane, mesh_sphere,
    skin_attrs_from_binding,
};
use crate::prim_ref::{
    UsdBlendShapeBinding, UsdDisplayName, UsdJoint, UsdKind, UsdLocalExtent, UsdPrimRef,
    UsdProcedural, UsdSkelAnimDriver, UsdSkelRoot, UsdSpatialAudio,
};
use usd_schemas::lux as ulux;
use usd_schemas::shade as ushade;
use usd_schemas::skel as uskel;

/// Walks `stage` and produces a standalone [`Scene`] plus labeled sub-assets
/// for every generated `Mesh` / `StandardMaterial`.
///
/// `embedded` is the USDZ-archive contents keyed by archive-relative path
/// (empty map for plain `.usda` / `.usdc` inputs). The material builder uses
/// it to resolve texture references that live inside the archive rather than
/// on disk.
///
/// `search_paths` is the list of filesystem directories the texture loader
/// probes when a relative asset path fails to resolve against Bevy's asset
/// root (the common Isaac Sim / Omniverse case: materials referenced from a
/// deep sibling `.usd` author textures as `./textures/foo.jpg` meaning
/// "relative to my own `.usd`", not to the asset root).
/// Counts instanceable prims encountered during scene projection and
/// how many of them re-used a prototype already built. `reuses > 0`
/// indicates real dedup savings.
#[derive(Debug, Default, Clone, Copy)]
pub struct InstanceStats {
    pub instance_prim_count: usize,
    pub prototype_reuses: usize,
}

pub fn stage_to_scene(
    stage: &Stage,
    lc: &mut LoadContext<'_>,
    embedded: &HashMap<String, Vec<u8>>,
    search_paths: &[std::path::PathBuf],
    kind_collapse: bool,
    light_intensity_scale: f32,
    curve_default_radius: f32,
    curve_ring_segments: u32,
    point_scale: f32,
    skel_animations: &HashMap<String, usd_schemas::skel_anim_text::ReadSkelAnimText>,
) -> (Scene, LightTally, InstanceStats) {
    let mut world = World::new();
    let mut ctx = BuildCtx::new(
        lc,
        embedded,
        search_paths,
        kind_collapse,
        light_intensity_scale,
        curve_default_radius,
        curve_ring_segments,
        point_scale,
        skel_animations,
    );

    let root_transform = root_basis_transform(stage);
    let root_name = stage
        .default_prim()
        .unwrap_or_else(|| "UsdRoot".to_string());

    let scene_root = world
        .spawn((
            Name::new(format!("{root_name} (UsdRoot)")),
            root_transform,
            Visibility::default(),
        ))
        .id();

    // Pick the roots to walk. If the stage authors `defaultPrim`, honour
    // it and root the walk there — this keeps openusd-exposed *referenced*
    // layer roots (a layer with `defaultPrim = "/root"` referenced into
    // `/Scene/Foo` still surfaces `/root` at the top level of the composed
    // stage) from being re-spawned alongside the main tree. Greenhouse
    // loses ~17 ghost subtrees at origin this way.
    let roots_to_walk: Vec<Path> = if let Some(default) = stage.default_prim() {
        match Path::abs_root().append_path(default.as_str()) {
            Ok(p) if matches!(stage.spec_type(p.clone()), Ok(Some(SpecType::Prim))) => vec![p],
            _ => stage
                .root_prims()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|n| Path::abs_root().append_path(n.as_str()).ok())
                .collect(),
        }
    } else {
        stage
            .root_prims()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|n| Path::abs_root().append_path(n.as_str()).ok())
            .collect()
    };

    for path in roots_to_walk {
        spawn_prim_subtree(stage, &path, scene_root, &mut world, &mut ctx);
    }

    let tally = ctx.lights;
    let instance_stats = InstanceStats {
        instance_prim_count: ctx.instance_prim_count,
        prototype_reuses: ctx.prototype_reuses,
    };
    if ctx.skinned_attempts > 0 {
        bevy::log::info!(
            "skel: Mesh visits={} SkinnedMesh attached={} failed={} no-cache={} blendshapes attached={}",
            ctx.skinned_attempts,
            ctx.skinned_attached,
            ctx.skinned_failed,
            ctx.skinned_no_cache,
            ctx.blendshape_attached,
        );
    }
    (Scene::new(world), tally, instance_stats)
}

/// Mutable-state bag threaded through the walker. Owns the `LoadContext` so
/// meshes / materials can be added as labeled sub-assets, plus the caches
/// that de-dupe by path / property signature.
pub(crate) struct BuildCtx<'lc, 'a> {
    pub lc: &'a mut LoadContext<'lc>,
    /// USDZ-embedded asset bytes by archive-relative path. Empty for
    /// non-USDZ inputs. Passed into texture loading so embedded PNG/JPEG
    /// payloads materialize as labeled sub-assets instead of filesystem
    /// reads.
    pub embedded: &'a HashMap<String, Vec<u8>>,
    /// Filesystem dirs the texture loader probes for relative asset paths
    /// that Bevy's AssetServer can't find via the asset root alone.
    pub search_paths: &'a [std::path::PathBuf],
    /// When `true`, kind-tagged subtrees flatten their intermediate Xforms.
    pub kind_collapse: bool,
    /// Scalar applied to every UsdLux light's brightness.
    pub light_intensity_scale: f32,
    /// Tube radius used when a `BasisCurves` prim doesn't author `widths`.
    pub curve_default_radius: f32,
    /// Ring segments per spine vertex when tubing curves.
    pub curve_ring_segments: u32,
    /// Multiplier applied to `UsdGeom.Points` half-extents.
    pub point_scale: f32,
    /// Running tally of UsdLux lights translated into Bevy lights.
    pub lights: LightTally,
    /// Lazily-built basename → absolute path index covering every image
    /// file under `search_paths`. Populated on first texture miss so loads
    /// without missing textures pay zero disk-walk cost.
    pub texture_index: Option<HashMap<String, std::path::PathBuf>>,
    /// Per-path decoded texture cache. Prevents re-decoding the same PNG
    /// twice when two Materials bind the same texture (applies to both
    /// USDZ-embedded and filesystem-fetched paths).
    pub embedded_textures: HashMap<String, bevy::asset::Handle<bevy::image::Image>>,
    /// USD prim path → handle for already-materialized Meshes. Keeps two
    /// ref sites that point at the same MeshLibrary entry from duplicating
    /// their vertex buffers.
    mesh_cache: HashMap<String, bevy::asset::Handle<Mesh>>,
    /// Shared default material — plain gray, emitted once, reused across
    /// every geom prim lacking a `material:binding`.
    default_material: Option<bevy::asset::Handle<StandardMaterial>>,
    /// Material prim path → resolved `StandardMaterial` handle. Two geom
    /// prims bound to the same Material share the handle without rebuilding
    /// the shader graph.
    material_cache: HashMap<String, bevy::asset::Handle<StandardMaterial>>,
    /// Running count of prims that author `instanceable = true`.
    pub instance_prim_count: usize,
    /// How many instanceable prims matched a fingerprint we'd already
    /// materialized — i.e. pure dedup wins.
    pub prototype_reuses: usize,
    /// Fingerprint → anchor prim path of the first instanceable prim
    /// with that fingerprint. Future matches count as reuses. Keeping
    /// the anchor path lets us log which prim seeded each prototype.
    prototype_anchors: HashMap<String, String>,
    /// Fingerprint → cached replay descriptors for the prototype's
    /// mesh-bearing subtree (M28). `None` means the prototype contained
    /// non-replayable content (Light / Camera / Curves / etc.) so
    /// replay is skipped and we fall back to the full walk. `Some`
    /// lets subsequent instance sites spawn the descriptors directly
    /// and short-circuit the recursion.
    prototype_descriptors: HashMap<String, Option<Vec<PrototypeDescriptor>>>,
    /// Active recording state — set when we're currently walking a
    /// prototype for the first time. Every entity spawned while this
    /// is `Some` appends a descriptor to the pending list.
    active_recording: Option<ActiveRecording>,
    /// Skeleton prim path → resolved per-skel info needed when a
    /// downstream Mesh's `skel:skeleton` rel points back at it. Stored
    /// keyed by the *Skeleton* prim path (not SkelRoot) because
    /// SkelBindingAPI on a Mesh names the Skeleton directly. Populated
    /// by `attach_skel_root` when a SkelRoot is walked; consumed by
    /// the mesh-attach path when it sees a `skel:skeleton` binding.
    pub skel_cache: HashMap<String, SkelInfo>,
    /// Sidecar-parsed `UsdSkelAnimation` prims (loaded from
    /// `UsdLoaderSettings::skel_animation_files`). Keyed by the
    /// SkelAnimation's authored prim name. The build walker looks
    /// them up by SkelRoot's animationSource leaf when it spawns a
    /// SkelRoot.
    pub skel_animations: &'a HashMap<String, usd_schemas::skel_anim_text::ReadSkelAnimText>,
    /// Diagnostics — how many skinned meshes did we attach
    /// `SkinnedMesh` to vs fail / skip cache. Logged at the end of
    /// stage_to_scene to make "X% of meshes skinned" visible.
    pub skinned_attempts: usize,
    pub skinned_attached: usize,
    pub skinned_failed: usize,
    pub skinned_no_cache: usize,
    pub blendshape_attached: usize,
}

/// Cached per-Skeleton state. Holds the full joint metadata so that
/// each skinned mesh can build a *subset* `SkinnedMesh` matching its
/// authored `skel:joints` (see `ReadSkelBinding::joint_subset`).
///
/// We can't share one `SkinnedMeshInverseBindposes` asset across
/// every binding because USD allows each mesh to declare its own
/// joint subset/reordering, and Bevy's joint-index attribute must
/// match the position in `SkinnedMesh.joints` and
/// `SkinnedMeshInverseBindposes`.
#[derive(Clone)]
pub(crate) struct SkelInfo {
    /// Joint paths in skeleton order (same order as `bind_inv` and
    /// `joint_entities`).
    pub joint_paths: Vec<String>,
    /// One entity per joint, indexed by joint position in the
    /// Skeleton's `joints` array.
    pub joint_entities: Vec<Entity>,
    /// Inverse of each joint's `bindTransform`, parallel to
    /// `joint_entities`. Computed once at SkelRoot walk; per-mesh
    /// SkinnedMesh just slices the right indices.
    pub bind_inv: Vec<bevy::math::Mat4>,
}

/// One captured entity from a prototype's subtree. Relative path is
/// from the prototype's root prim (the one that authored
/// `instanceable = true`). Transforms are baked relative to that root.
#[derive(Debug, Clone)]
struct PrototypeDescriptor {
    /// Leaf name at spawn time (goes into `Name` on the replay).
    leaf_name: String,
    /// Absolute prim path of the original entity — used to compute
    /// an instance-relative `UsdPrimRef` on replay.
    source_prim_path: String,
    /// Transform relative to the prototype root.
    relative_transform: Transform,
    visibility: Visibility,
    kind: Option<String>,
    local_extent: Option<UsdLocalExtent>,
    mesh: Option<bevy::asset::Handle<Mesh>>,
    material: Option<bevy::asset::Handle<StandardMaterial>>,
    /// Path back through the hierarchy from the prototype root to
    /// this entity's parent — so we can rebuild the parent-child
    /// graph during replay.
    parent_relative_path: Option<String>,
    /// Stable relative-path key — used to wire `ChildOf` during
    /// replay. Root prim has an empty string here.
    relative_path: String,
}

#[derive(Debug, Clone)]
struct ActiveRecording {
    fingerprint: String,
    root_prim_path: String,
    descriptors: Vec<PrototypeDescriptor>,
    /// Set to `true` the first time we hit a non-replayable type
    /// while recording. Causes the fingerprint to cache as `None`
    /// so future matches fall back to a normal walk.
    poisoned: bool,
}

impl<'lc, 'a> BuildCtx<'lc, 'a> {
    fn new(
        lc: &'a mut LoadContext<'lc>,
        embedded: &'a HashMap<String, Vec<u8>>,
        search_paths: &'a [std::path::PathBuf],
        kind_collapse: bool,
        light_intensity_scale: f32,
        curve_default_radius: f32,
        curve_ring_segments: u32,
        point_scale: f32,
        skel_animations: &'a HashMap<String, usd_schemas::skel_anim_text::ReadSkelAnimText>,
    ) -> Self {
        Self {
            lc,
            embedded,
            search_paths,
            kind_collapse,
            light_intensity_scale,
            curve_default_radius,
            curve_ring_segments,
            point_scale,
            lights: LightTally::default(),
            texture_index: None,
            embedded_textures: HashMap::new(),
            mesh_cache: HashMap::new(),
            default_material: None,
            material_cache: HashMap::new(),
            instance_prim_count: 0,
            prototype_reuses: 0,
            prototype_anchors: HashMap::new(),
            prototype_descriptors: HashMap::new(),
            active_recording: None,
            skel_cache: HashMap::new(),
            skel_animations,
            skinned_attempts: 0,
            skinned_attached: 0,
            skinned_failed: 0,
            skinned_no_cache: 0,
            blendshape_attached: 0,
        }
    }

    fn add_mesh_labeled(&mut self, label: String, mesh: Mesh) -> bevy::asset::Handle<Mesh> {
        self.lc.add_labeled_asset(label, mesh)
    }

    fn default_material(&mut self) -> bevy::asset::Handle<StandardMaterial> {
        if let Some(h) = &self.default_material {
            return h.clone();
        }
        let h = self
            .lc
            .add_labeled_asset("Material:Default".into(), default_material());
        self.default_material = Some(h.clone());
        h
    }

    /// Double-sided-aware fallback. `double_sided=false` returns the
    /// shared default; `true` allocates a distinct
    /// `Material:Default-doubleSided` variant. Kept cached under a
    /// synthetic label so repeat meshes share.
    fn default_material_ds(
        &mut self,
        double_sided: bool,
    ) -> bevy::asset::Handle<StandardMaterial> {
        if !double_sided {
            return self.default_material();
        }
        static LABEL: &str = "Material:Default-doubleSided";
        if let Some(h) = self.material_cache.get(LABEL) {
            return h.clone();
        }
        let mut mat = default_material();
        mat.double_sided = true;
        mat.cull_mode = None;
        let h = self.lc.add_labeled_asset(LABEL.into(), mat);
        self.material_cache.insert(LABEL.to_string(), h.clone());
        h
    }

    /// Material whose base colour is deterministically hashed from a
    /// prim path string. Used as a *richer* fallback than the flat
    /// grey default when a mesh has neither `material:binding` nor
    /// `primvars:displayColor` — gives Pixar-style production assets
    /// (Kitchen_set, etc.) per-prop visual variety so the scene reads
    /// as something other than a uniform grey blob. Same hash key →
    /// same colour, so siblings under one parent share their tint.
    fn path_hashed_material_ds(
        &mut self,
        hash_key: &str,
        double_sided: bool,
    ) -> bevy::asset::Handle<StandardMaterial> {
        let label = format!(
            "Material:Hashed:{}{}",
            hash_key,
            if double_sided { "-doubleSided" } else { "" }
        );
        if let Some(h) = self.material_cache.get(&label) {
            return h.clone();
        }
        let (r, g, b) = hash_path_to_rgb(hash_key);
        let mat = StandardMaterial {
            base_color: bevy::color::Color::srgb(r, g, b),
            perceptual_roughness: 0.8,
            metallic: 0.0,
            double_sided,
            cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
            ..Default::default()
        };
        let handle = self.lc.add_labeled_asset(label.clone(), mat);
        self.material_cache.insert(label, handle.clone());
        handle
    }

    /// Lit white-base material that modulates cleanly against vertex
    /// colours (Bevy's PBR shader sets `base_color = vertex.color` when
    /// `VERTEX_COLORS` is defined, so lit shading still runs on top).
    /// Used when a Mesh authors `primvars:displayColor` but no
    /// `material:binding`. Honours `double_sided`: emits a distinct
    /// variant with `cull_mode = None` when true.
    fn vertex_color_modulated_material_ds(
        &mut self,
        double_sided: bool,
    ) -> bevy::asset::Handle<StandardMaterial> {
        let label: &'static str = if double_sided {
            "Material:VertexColorLit-doubleSided"
        } else {
            "Material:VertexColorLit"
        };
        if let Some(h) = self.material_cache.get(label) {
            return h.clone();
        }
        let mat = StandardMaterial {
            base_color: bevy::color::Color::WHITE,
            perceptual_roughness: 0.8,
            metallic: 0.0,
            double_sided,
            cull_mode: if double_sided {
                None
            } else {
                Some(bevy::render::render_resource::Face::Back)
            },
            ..Default::default()
        };
        let h = self.lc.add_labeled_asset(label.into(), mat);
        self.material_cache.insert(label.to_string(), h.clone());
        h
    }

    /// Shared unlit material used by `BasisCurves` and `Points`. They
    /// rely on vertex colours (displayColor or a broadcast default) —
    /// `StandardMaterial` + vertex `ATTRIBUTE_COLOR` gives us that for
    /// free, as long as the material is unlit so shading doesn't wash
    /// the colour out.
    fn unlit_vertex_color_material(&mut self) -> bevy::asset::Handle<StandardMaterial> {
        static LABEL: &str = "Material:UnlitVertexColor";
        // The material cache is keyed by "Material prim path" — we
        // hijack that keyspace with a synthetic label so the handle
        // survives across repeat calls.
        if let Some(h) = self.material_cache.get(LABEL) {
            return h.clone();
        }
        let mat = StandardMaterial {
            base_color: bevy::color::Color::WHITE,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            ..Default::default()
        };
        let h = self.lc.add_labeled_asset(LABEL.into(), mat);
        self.material_cache.insert(LABEL.to_string(), h.clone());
        h
    }

    /// Resolve / build a `StandardMaterial` for a Material prim. Cached by
    /// `(prim path, double_sided)` so repeat bindings share the
    /// handle. Two distinct variants are cached when a material is bound
    /// on both a single- and double-sided mesh.
    fn material_for(
        &mut self,
        stage: &Stage,
        material_prim: &Path,
        double_sided: bool,
    ) -> bevy::asset::Handle<StandardMaterial> {
        let key = if double_sided {
            format!("{}#doubleSided", material_prim.as_str())
        } else {
            material_prim.as_str().to_string()
        };
        if let Some(h) = self.material_cache.get(&key) {
            return h.clone();
        }
        let mut bevy_mat = match ushade::read_preview_material(stage, material_prim)
            .ok()
            .flatten()
        {
            Some(read) => standard_material_from_usd(self, &read),
            None => {
                // Material prim exists but isn't a UsdPreviewSurface
                // (or `outputs:surface` isn't wired). Pixar's
                // Kitchen_set authors these — empty Material prims as
                // a binding placeholder for the host application's
                // shading library. Hash the material's prim path so
                // each placeholder gets its own colour and the scene
                // doesn't collapse to flat grey.
                let mut mat = crate::material::default_material();
                let path_str = material_prim.as_str();
                let lower = path_str.to_ascii_lowercase();
                // Name-based glass heuristic: Omniverse / Isaac scenes
                // bind MDL materials like `Clear_Glass` or `Frosted_Glass`
                // that we can't parse (MDL is a separate shading
                // language). Without this the greenhouse renders its
                // panes as opaque coloured squares. Detect by name and
                // synthesise a translucent fallback so the structure
                // looks right at a glance — proper MDL parsing is M9.
                let looks_like_glass = lower.contains("glass")
                    || lower.contains("acrylic")
                    || lower.contains("transparent");
                if looks_like_glass {
                    use bevy::render::alpha::AlphaMode;
                    mat.base_color = bevy::color::Color::srgba(0.85, 0.92, 0.95, 0.18);
                    mat.alpha_mode = AlphaMode::Blend;
                    mat.metallic = 0.0;
                    mat.perceptual_roughness = 0.05;
                    mat.reflectance = 0.7;
                } else {
                    let (r, g, b) = hash_path_to_rgb(path_str);
                    mat.base_color = bevy::color::Color::srgb(r, g, b);
                }
                mat
            }
        };
        if double_sided {
            bevy_mat.double_sided = true;
            bevy_mat.cull_mode = None;
        }
        let label = if double_sided {
            format!("{}-doubleSided", material_prim.as_str())
        } else {
            material_prim.as_str().to_string()
        };
        let handle = add_material_labeled(self.lc, &label, bevy_mat);
        self.material_cache.insert(key, handle.clone());
        handle
    }
}

/// Recursively spawn one entity per prim, linked via [`ChildOf`].
fn spawn_prim_subtree(
    stage: &Stage,
    path: &Path,
    parent: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    if !matches!(stage.spec_type(path.clone()), Ok(Some(SpecType::Prim))) {
        return;
    }
    if !passes_purpose_filter(stage, path) {
        return;
    }

    let leaf = path.name().unwrap_or("").to_string();
    let transform = read_prim_transform(stage, path);
    let prim_ref = UsdPrimRef::new(path.as_str());

    // Scene-instancing bookkeeping. `instanceable = true` marks a prim
    // whose subtree is a shareable prototype. Two code paths:
    //
    //   1. Cache hit + replayable → spawn descriptors directly, skip
    //      the recursive walk. This is the M28 optimisation — on
    //      greenhouse-scale scenes (80k prims with lots of repeated
    //      lattice hardware) it avoids the entity churn entirely.
    //
    //   2. First sighting → fall through to the normal walk with
    //      `ctx.active_recording = Some(...)` so descriptors get
    //      captured as children spawn. If the walker encounters a
    //      non-replayable type (light, camera, curves, etc.) the
    //      recording is poisoned and future matches fall back to a
    //      full walk.
    let mut replay_ctx: Option<ReplayCtx> = None;
    let is_instanceable = matches!(
        stage.field::<bool>(path.clone(), "instanceable").ok().flatten(),
        Some(true)
    );
    if is_instanceable {
        ctx.instance_prim_count += 1;
        let fp = prototype_fingerprint(stage, path);
        if let Some(anchor) = ctx.prototype_anchors.get(&fp).cloned() {
            ctx.prototype_reuses += 1;
            bevy::log::debug!(
                "instancing: {} reuses prototype seeded by {}",
                path.as_str(),
                anchor
            );
            // Descriptor-replay fast path.
            if let Some(Some(descriptors)) = ctx.prototype_descriptors.get(&fp).cloned() {
                replay_ctx = Some(ReplayCtx {
                    descriptors,
                    instance_prim_path: path.as_str().to_string(),
                });
            }
        } else {
            ctx.prototype_anchors
                .insert(fp.clone(), path.as_str().to_string());
            // Begin recording. Nested instanceable prims won't start
            // a second recording (we only keep one active).
            if ctx.active_recording.is_none() {
                ctx.active_recording = Some(ActiveRecording {
                    fingerprint: fp,
                    root_prim_path: path.as_str().to_string(),
                    descriptors: Vec::new(),
                    poisoned: false,
                });
            }
        }
    }

    // Cache-hit replay: spawn descriptors + return without recursing.
    if let Some(rc) = replay_ctx {
        replay_prototype(&rc, path, parent, world, ctx);
        return;
    }

    // Visibility honours `UsdGeomImageable.visibility`. `invisible`
    // propagates through `Visibility::Hidden` — Bevy inherits through
    // the hierarchy so descendants disappear too.
    let visibility = match ugeom::read_visibility(stage, path).ok() {
        Some(ugeom::VisibilityState::Invisible) => Visibility::Hidden,
        _ => Visibility::default(),
    };

    let entity = world
        .spawn((
            Name::new(leaf),
            transform,
            visibility,
            prim_ref,
            ChildOf(parent),
        ))
        .id();

    // UsdModelAPI.kind — attach the component only when authored so
    // the Tree panel can show the kind column without everyone
    // getting a blank row.
    if let Ok(Some(k)) = ugeom::read_kind(stage, path) {
        world.entity_mut(entity).insert(UsdKind { kind: k });
    }

    // UsdUISceneGraphPrimAPI `ui:displayName` — friendly label that the
    // viewer's tree row prefers over the prim's leaf name. Only
    // attached when authored so the tree falls back to the leaf for
    // unannotated prims.
    if let Ok(Some(name)) = usd_schemas::ui::read_display_name(stage, path)
        && !name.is_empty()
    {
        world.entity_mut(entity).insert(UsdDisplayName(name));
    }

    // UsdMediaSpatialAudio — read-side only. The component carries
    // the authored playback metadata; a future bevy_audio backend can
    // pick it up to spawn an actual audio source.
    if let Ok(Some(sa)) = usd_schemas::media::read_spatial_audio(stage, path) {
        world.entity_mut(entity).insert(UsdSpatialAudio {
            file_path: sa.file_path,
            aural_mode: sa.aural_mode,
            playback_mode: sa.playback_mode,
            gain: sa.gain,
        });
    }

    // UsdProcGenerativeProcedural (and subclasses) — surface the
    // procedural-type marker. We can't execute the procedural without
    // its engine, but the viewer can at least flag the prim.
    if let Ok(Some(p)) = usd_schemas::proc::read_procedural(stage, path) {
        world.entity_mut(entity).insert(UsdProcedural {
            procedural_type: p.procedural_type,
            procedural_system: p.procedural_system,
        });
    }


    // UsdSkel: when this prim is a SkelRoot, resolve its Skeleton +
    // animationSource and spawn one Bevy entity per joint, parented
    // per the joint topology with the authored restTransforms as
    // initial Transform. Skinning attributes + SkinnedMesh + animation
    // playback land in follow-up patches; this just stands the bones
    // up as named ECS entities so downstream code (and the tree panel)
    // can see the rig.
    attach_skel_root(stage, path, entity, world, ctx);

    attach_geometry(stage, path, entity, world, ctx);
    attach_light(stage, path, entity, parent, world, ctx);

    // Scene-instancing descriptor capture (M28). If we're recording a
    // prototype, snapshot this entity's replayable components. If the
    // type isn't replayable (lights / cameras / curves / points /
    // instancers), poison the recording so future matches fall back
    // to a normal walk.
    if let Some(rec) = ctx.active_recording.as_mut() {
        let type_str = type_name_of(stage, path);
        if !is_replayable_type(type_str.as_deref()) {
            rec.poisoned = true;
        } else {
            let relative_path = path
                .as_str()
                .strip_prefix(rec.root_prim_path.as_str())
                .unwrap_or("")
                .to_string();
            let parent_relative_path = if relative_path.is_empty() {
                None
            } else {
                relative_path
                    .rsplit_once('/')
                    .map(|(head, _)| head.to_string())
            };
            // Peek the Mesh3d + MeshMaterial3d handles that
            // attach_geometry just inserted (if any).
            let entity_ref = world.entity(entity);
            let mesh_handle = entity_ref
                .get::<Mesh3d>()
                .map(|m| m.0.clone());
            let material_handle = entity_ref
                .get::<MeshMaterial3d<StandardMaterial>>()
                .map(|m| m.0.clone());
            let vis = entity_ref.get::<Visibility>().copied().unwrap_or_default();
            let kind = entity_ref.get::<UsdKind>().map(|k| k.kind.clone());
            let local_extent = entity_ref.get::<UsdLocalExtent>().copied();
            rec.descriptors.push(PrototypeDescriptor {
                leaf_name: path.name().unwrap_or("").to_string(),
                source_prim_path: path.as_str().to_string(),
                relative_transform: transform,
                visibility: vis,
                kind,
                local_extent,
                mesh: mesh_handle,
                material: material_handle,
                parent_relative_path,
                relative_path,
            });
        }
    }

    // Kind-driven collapse. When this prim carries `kind =
    // "component"|"subcomponent"` AND the feature is enabled, flatten every
    // descendant geom into direct children of this entity. Intermediate
    // Xform prims disappear from the ECS; their contribution survives in
    // the baked per-geom transforms. Opt-in via
    // `UsdLoaderSettings::kind_collapse`.
    if ctx.kind_collapse && kind_is_collapsible(stage, path) {
        let mut geoms = Vec::new();
        collect_geoms_for_collapse(
            stage,
            path,
            /* relative = */ Transform::IDENTITY,
            /* depth = */ 0,
            &mut geoms,
        );
        for (geom_path, world_transform) in geoms {
            spawn_collapsed_geom(stage, &geom_path, world_transform, entity, world, ctx);
        }
        return;
    }

    for child_name in stage.prim_children(path.clone()).unwrap_or_default() {
        let Ok(child_path) = path.append_path(child_name.as_str()) else {
            continue;
        };
        spawn_prim_subtree(stage, &child_path, entity, world, ctx);
    }

    // If this prim was the recording root, finalise: either cache
    // the captured descriptors (good replay) or mark as poisoned
    // (fall-back walk on future matches).
    if ctx
        .active_recording
        .as_ref()
        .is_some_and(|r| r.root_prim_path == path.as_str())
    {
        if let Some(rec) = ctx.active_recording.take() {
            let cached = if rec.poisoned {
                None
            } else {
                Some(rec.descriptors)
            };
            ctx.prototype_descriptors.insert(rec.fingerprint, cached);
        }
    }
}

/// `kind` values that mark a subtree as a "model" safe to collapse. Pixar
/// USD defines a semantic hierarchy (`model` > `group`/`assembly` >
/// `component`/`subcomponent`). We collapse from `component` down —
/// `group` / `assembly` stay articulated because that's where rigid-body
/// boundaries typically land.
fn kind_is_collapsible(stage: &Stage, path: &Path) -> bool {
    stage
        .field::<String>(path.clone(), "kind")
        .ok()
        .flatten()
        .map(|k| matches!(k.as_str(), "component" | "subcomponent"))
        .unwrap_or(false)
}

/// Depth-first walk of a collapsible subtree, emitting `(geom_path,
/// accumulated_transform_from_collapse_root)` for every prim that carries
/// geometry. Skips purpose-filtered prims and nested Kind roots (their
/// subtrees get their own collapse pass).
fn collect_geoms_for_collapse(
    stage: &Stage,
    path: &Path,
    parent_to_root: Transform,
    depth: u32,
    out: &mut Vec<(Path, Transform)>,
) {
    if !matches!(stage.spec_type(path.clone()), Ok(Some(SpecType::Prim))) {
        return;
    }
    if !passes_purpose_filter(stage, path) {
        return;
    }

    let local = read_prim_transform(stage, path);
    // At the collapse root (depth=0), we don't fold `local` — the root
    // entity keeps its own transform. For descendants, accumulate.
    let accumulated = if depth == 0 {
        parent_to_root
    } else {
        compose_transforms(&parent_to_root, &local)
    };

    if depth > 0 && prim_has_geometry(stage, path) {
        out.push((path.clone(), accumulated));
    }

    for child_name in stage.prim_children(path.clone()).unwrap_or_default() {
        let Ok(child_path) = path.append_path(child_name.as_str()) else {
            continue;
        };
        collect_geoms_for_collapse(stage, &child_path, accumulated, depth + 1, out);
    }
}

/// `true` if `prim` would get a `Mesh3d` in the normal spawn path.
fn prim_has_geometry(stage: &Stage, path: &Path) -> bool {
    matches!(
        stage
            .field::<String>(path.clone(), "typeName")
            .ok()
            .flatten()
            .as_deref(),
        Some("Mesh" | "Cube" | "Sphere" | "Cylinder" | "Capsule" | "Plane")
    )
}

/// Compose two `Transform`s the same way `GlobalTransform` does so the
/// collapsed mesh lands where the hierarchy would have put it.
fn compose_transforms(parent: &Transform, local: &Transform) -> Transform {
    let parent_mat = parent.to_matrix();
    let local_mat = local.to_matrix();
    Transform::from_matrix(parent_mat * local_mat)
}

/// Spawn a single collapsed geom as a direct child of `parent`. Carries the
/// full baked transform relative to the Kind-collapse root.
fn spawn_collapsed_geom(
    stage: &Stage,
    geom_path: &Path,
    transform: Transform,
    parent: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    let leaf = geom_path.name().unwrap_or("geom").to_string();
    let entity = world
        .spawn((
            Name::new(leaf),
            transform,
            Visibility::default(),
            UsdPrimRef::new(geom_path.as_str()),
            ChildOf(parent),
        ))
        .id();
    attach_geometry(stage, geom_path, entity, world, ctx);
}

/// If `prim`'s `typeName` is a UsdLux light, build the right Bevy
/// light component on the existing entity + bump the `BuildCtx::lights`
/// tally so the viewer can surface totals.
fn attach_light(
    stage: &Stage,
    path: &Path,
    entity: Entity,
    parent: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    let Some(read) = ulux::read_light(stage, path).ok().flatten() else {
        return;
    };
    let tally = spawn_light(
        world,
        entity,
        &read,
        ctx.light_intensity_scale,
        path.as_str(),
        parent,
    );
    ctx.lights.add(tally);
}

/// When `path` is a `SkelRoot`, resolve its `skel:skeleton` rel target
/// (or any descendant Mesh's binding) and spawn one entity per joint,
/// parented per the joint topology with `restTransforms[i]` as initial
/// local Transform. Tags each joint entity with `UsdJoint` so future
/// skinning code can find it by index.
///
/// Returns silently when the prim isn't a SkelRoot or no Skeleton can
/// be resolved — this is a no-op for non-skinned subtrees.
fn attach_skel_root(
    stage: &Stage,
    path: &Path,
    entity: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    if type_name_of(stage, path).as_deref() != Some("SkelRoot") {
        return;
    }
    bevy::log::info!("skel: SkelRoot detected at {}", path.as_str());
    let Some(root) = uskel::read_skel_root(stage, path).ok().flatten() else {
        bevy::log::warn!("skel: read_skel_root returned None for {}", path.as_str());
        return;
    };
    // Resolve Skeleton path. Prefer the SkelRoot's own
    // `skel:skeleton` rel; fall back to scanning descendants for a
    // typed `Skeleton` prim (Pixar's HumanFemale authors the rel on
    // the SkelRoot, but other DCCs may put it elsewhere).
    let skel_prim_path = root
        .skeleton
        .as_deref()
        .and_then(|p| Path::new(p).ok())
        .or_else(|| find_first_typed_descendant(stage, path, "Skeleton"));
    let Some(skel_path) = skel_prim_path else {
        bevy::log::warn!("skel: no Skeleton resolvable for SkelRoot {}", path.as_str());
        return;
    };
    let Some(mut skel) = uskel::read_skeleton(stage, &skel_path).ok().flatten() else {
        bevy::log::warn!(
            "skel: Skeleton at {} unreadable or has no joints",
            skel_path.as_str()
        );
        return;
    };
    // Skeleton-augmentation: when the sidecar SkelAnimation has more
    // joints than the bound Skeleton (Pixar's HumanFemale ships a
    // 66-joint body rig but walk.usd animates a 109-joint full rig
    // with face bones), append the missing joints to our skeleton so
    // the mesh's `jointIndices` (which reach into the larger set)
    // resolve to real entities. Bind transforms for the appended
    // joints get derived by composing the animation's first-keyframe
    // local transforms.
    augment_skeleton_from_anim(&mut skel, ctx.skel_animations);
    bevy::log::info!(
        "skel: spawning {} joints from {}",
        skel.joints.len(),
        skel_path.as_str()
    );

    world.entity_mut(entity).insert(UsdSkelRoot {
        skeleton_path: skel.path.clone(),
        animation_source_path: root.animation_source.clone().unwrap_or_default(),
    });

    let parents = skel.joint_parent_indices();
    let names = skel
        .joint_short_names()
        .into_iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut joint_entities: Vec<Option<Entity>> = vec![None; skel.joints.len()];

    // Walk in topology order: a joint can only spawn after its parent.
    // USD spec doesn't strictly require topological ordering of the
    // `joints` list, so iterate until quiescent.
    let mut spawned = 0usize;
    let mut remaining: Vec<usize> = (0..skel.joints.len()).collect();
    let mut guard = remaining.len() * 2 + 4;
    while !remaining.is_empty() && guard > 0 {
        guard -= 1;
        remaining.retain(|&i| {
            let parent_entity = match parents[i] {
                None => Some(entity),
                Some(pi) => joint_entities[pi],
            };
            let Some(parent_e) = parent_entity else {
                return true; // try next pass
            };
            let local = transform_from_mat4_row_major(
                skel.rest_transforms.get(i).copied().unwrap_or(IDENTITY_MAT4),
            );
            let je = world
                .spawn((
                    Name::new(names[i].clone()),
                    local,
                    Visibility::default(),
                    UsdJoint {
                        path: skel.joints[i].clone(),
                        index: i as u32,
                    },
                    ChildOf(parent_e),
                ))
                .id();
            joint_entities[i] = Some(je);
            spawned += 1;
            false
        });
    }
    bevy::log::info!(
        "skel: spawned {}/{} joint entities (skipped: {})",
        spawned,
        skel.joints.len(),
        remaining.len()
    );

    // Cache for downstream Mesh attachment. Only register if every
    // joint actually got an entity — partial spawns (cyclic/unrooted
    // topology) would leave `joints[i]` undefined for some `i`.
    let all_spawned: Option<Vec<Entity>> = joint_entities.iter().copied().collect();
    let joint_entities_for_anim = all_spawned.clone();
    if let Some(joint_entities) = all_spawned {
        use bevy::math::Mat4;
        let bind_inv: Vec<Mat4> = (0..skel.joints.len())
            .map(|i| {
                let m = skel
                    .bind_transforms
                    .get(i)
                    .copied()
                    .unwrap_or(IDENTITY_MAT4);
                // USD's row-vector + row-major equals glam's
                // column-vector + column-major in flat memory; the
                // raw data is already a valid glam matrix. See
                // `transform_from_mat4_row_major` for the full
                // explanation.
                Mat4::from_cols_array(&m).inverse()
            })
            .collect();
        ctx.skel_cache.insert(
            skel.path.clone(),
            SkelInfo {
                joint_paths: skel.joints.clone(),
                joint_entities,
                bind_inv,
            },
        );
    } else {
        bevy::log::warn!(
            "skel: not caching {} — partial joint spawn ({} missing)",
            skel.path,
            skel.joints.len() - spawned
        );
    }

    // Animation driver: when a sidecar SkelAnimation matches this
    // SkelRoot's `skel:animationSource` (or, when none authored, we
    // pick the first available animation as a single-anim
    // convenience), build a UsdSkelAnimDriver so the per-frame system
    // can write joint transforms.
    if let Some(joint_entities) = joint_entities_for_anim {
        let anim_lookup_key = root
            .animation_source
            .as_deref()
            .and_then(|p| p.rsplit_once('/').map(|(_, n)| n.to_string()).or_else(|| Some(p.to_string())))
            .or_else(|| {
                // Fall back to the only sidecar entry — most users
                // only side-load one animation file.
                if ctx.skel_animations.len() == 1 {
                    ctx.skel_animations.keys().next().cloned()
                } else {
                    None
                }
            });
        // Prefer the composed Stage's SkelAnimation when it has
        // usable data; fall back to the sidecar text parser only
        // when composition didn't produce one. The sidecar is the
        // legacy workaround for openusd-rs's USDA parser rejecting
        // tuple-valued timeSamples; once that parser bug is fixed
        // (PR mxpv/openusd#59), composition wins for both USDA and
        // USDC layers. The order also matters for repacked USDZs
        // where the wrapper authors an empty `def SkelAnimation`
        // that brings the real data in via prim-targeted reference
        // — sidecar text-scrape would see the empty wrapper prim
        // and clobber walk.usd's real entry under the same name.
        let from_stage: Option<usd_schemas::skel_anim_text::ReadSkelAnimText> = root
            .animation_source
            .as_deref()
            .and_then(|p| Path::new(p).ok())
            .and_then(|p| uskel::read_skel_animation_stage(stage, &p).ok().flatten())
            .or_else(|| {
                // Implicit binding: USD allows authoring the
                // SkelAnimation as a CHILD of the Skeleton instead
                // of a `skel:animationSource` rel target. Apple's
                // hummingbird (and many AR Quick Look samples) use
                // this form. Scan the Skeleton's children.
                find_first_typed_descendant(stage, &skel_path, "SkelAnimation")
                    .and_then(|p| uskel::read_skel_animation_stage(stage, &p).ok().flatten())
            })
            // Treat empty stage reads (no joints / no time samples)
            // as "no animation here" so the sidecar fallback gets a
            // chance.
            .filter(|a| {
                !a.joints.is_empty()
                    || !a.translations.is_empty()
                    || !a.rotations.is_empty()
                    || !a.scales.is_empty()
                    || !a.blend_shape_weights.is_empty()
            });
        let from_sidecar: Option<usd_schemas::skel_anim_text::ReadSkelAnimText> =
            anim_lookup_key
                .as_deref()
                .and_then(|k| ctx.skel_animations.get(k))
                .cloned();
        let resolved_anim = from_stage.or(from_sidecar);
        if let Some(ref anim) = resolved_anim {
            // Remap animation joints → skeleton joint entities.
            let anim_to_skel: Vec<Option<Entity>> = anim
                .joints
                .iter()
                .map(|jp| {
                    skel.joints
                        .iter()
                        .position(|sp| sp == jp)
                        .map(|i| joint_entities[i])
                })
                .collect();
            let translations: Vec<(f64, Vec<[f32; 3]>)> = anim
                .translations
                .iter()
                .map(|(t, v)| (t.0, v.clone()))
                .collect();
            let rotations: Vec<(f64, Vec<[f32; 4]>)> = anim
                .rotations
                .iter()
                .map(|(t, v)| (t.0, v.clone()))
                .collect();
            let scales: Vec<(f64, Vec<[f32; 3]>)> = anim
                .scales
                .iter()
                .map(|(t, v)| (t.0, v.clone()))
                .collect();
            let mapped = anim_to_skel.iter().filter(|e| e.is_some()).count();
            let bs_weights: Vec<(f64, Vec<f32>)> = anim
                .blend_shape_weights
                .iter()
                .map(|(t, v)| (t.0, v.clone()))
                .collect();
            // Auto-detect quaternion element order. Pixar's spec
            // says (w, x, y, z); Apple's USDZ exporter writes
            // (x, y, z, w). Test: average the absolute values of
            // [0] and [3] across the FIRST keyframe — whichever is
            // dominant tells us where the real component lives.
            // For typical animations near the rest pose, real (w)
            // is much larger than the imaginary axes.
            let mut sum_abs_first = 0.0f32;
            let mut sum_abs_last = 0.0f32;
            let mut samples = 0usize;
            if let Some((_, first_rot)) = anim.rotations.iter().next() {
                for q in first_rot {
                    sum_abs_first += q[0].abs();
                    sum_abs_last += q[3].abs();
                    samples += 1;
                }
            }
            let quat_xyzw_order = if samples > 0 {
                sum_abs_last > sum_abs_first
            } else {
                false
            };
            if quat_xyzw_order {
                bevy::log::info!(
                    "skel anim: detected (x, y, z, w) quat order on {} (Apple Quick Look convention; avg |q[0]|={:.3}, |q[3]|={:.3})",
                    path.as_str(),
                    sum_abs_first / samples.max(1) as f32,
                    sum_abs_last / samples.max(1) as f32
                );
            }
            world.entity_mut(entity).insert(UsdSkelAnimDriver {
                joint_entities: anim_to_skel,
                translations,
                rotations,
                scales,
                blend_shape_names: anim.blend_shapes.clone(),
                blend_shape_weights: bs_weights,
                quat_xyzw_order,
            });
            bevy::log::info!(
                "skel: attached anim driver on {} (anim={}, mapped {}/{} channels)",
                path.as_str(),
                anim.prim_name,
                mapped,
                anim.joints.len(),
            );
        }
    }
}

/// Build the per-mesh `(joints, inverse_bindposes)` tuple that a
/// `SkinnedMesh` component needs. The mesh's authored
/// `binding.joint_subset` defines an ordered list of skeleton joints
/// the mesh's `jointIndices` reference; we slice the cached
/// `SkelInfo.joint_entities` and `bind_inv` arrays in that order.
///
/// When `joint_subset` is empty the mesh references the full
/// skeleton in skeleton-order — return the cached arrays directly.
///
/// Each call materializes one `SkinnedMeshInverseBindposes` asset
/// (one per binding) labeled by the mesh prim path. Two meshes that
/// share an identical subset would currently produce two assets;
/// that's a memory waste, not a correctness issue.
fn build_subset_skinned_mesh(
    info: &SkelInfo,
    binding: &uskel::ReadSkelBinding,
    lc: &mut LoadContext<'_>,
    mesh_path: &Path,
) -> Option<(
    Vec<Entity>,
    bevy::asset::Handle<bevy::mesh::skinning::SkinnedMeshInverseBindposes>,
)> {
    let (joints, inv) = if binding.joint_subset.is_empty() {
        (info.joint_entities.clone(), info.bind_inv.clone())
    } else {
        let mut joints = Vec::with_capacity(binding.joint_subset.len());
        let mut inv = Vec::with_capacity(binding.joint_subset.len());
        for jname in &binding.joint_subset {
            // skeleton.joints is path-form ("Hips/Torso"); some
            // bindings may author the leaf only — match against both.
            let pos = info
                .joint_paths
                .iter()
                .position(|p| p == jname)
                .or_else(|| {
                    info.joint_paths.iter().position(|p| {
                        p.rsplit_once('/').map(|(_, n)| n).unwrap_or(p) == jname
                    })
                });
            match pos {
                Some(idx) => {
                    joints.push(info.joint_entities[idx]);
                    inv.push(info.bind_inv[idx]);
                }
                None => {
                    bevy::log::warn!(
                        "skel: binding on {} references joint {} not in Skeleton",
                        mesh_path.as_str(),
                        jname
                    );
                    return None;
                }
            }
        }
        (joints, inv)
    };
    let ibps = lc.add_labeled_asset(
        format!("Skel:{}:inverse_bindposes", mesh_path.as_str()),
        bevy::mesh::skinning::SkinnedMeshInverseBindposes::from(inv),
    );
    Some((joints, ibps))
}

/// Find the same prim under a different root and return its
/// authored `skel:blendShapes` + `skel:blendShapeTargets`. Used to
/// recover blend-shape metadata that openusd-rs's composition layer
/// doesn't currently surface through the wrapper's reference
/// (Pixar's `HumanFemale.full_payload.usd` referenced into our
/// `/Skel/Geometry` exposes blend-shape data only at the
/// `/HumanFemale_Group/...` ghost twin).
///
/// The strategy: take the path's last few segments (everything from
/// the deepest "Geom" segment onward, plus the leaf) and search for
/// a matching descendant under each ROOT prim. First non-empty
/// match wins.
fn blend_shapes_from_ghost_twin(
    stage: &Stage,
    composed_path: &Path,
) -> Option<(Vec<String>, Vec<String>)> {
    let composed_str = composed_path.as_str();
    // Heuristic: drop everything up to and including the FIRST
    // segment after the root, leaving a tail that ought to match
    // under the ghost twin. e.g.
    // `/Skel/Geometry/HumanFemale/Geom/.../Body_sbdv`
    // → tail starts at `/HumanFemale/Geom/.../Body_sbdv`.
    let mut parts: Vec<&str> = composed_str.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() < 3 {
        return None;
    }
    // Drop the wrapper's "Skel" + "Geometry" prefix segments so the
    // tail matches what's under the payload's defaultPrim. We don't
    // know the prefix length up-front; try progressively shorter
    // tails, longest first.
    for skip in 0..parts.len().saturating_sub(1) {
        let tail = &parts[skip..];
        let suffix = format!("/{}", tail.join("/"));
        // Try as an absolute path under each root prim — if a root
        // has children matching our tail, check there.
        for root_name in stage.root_prims().unwrap_or_default() {
            if root_name == "Skel" {
                // Skip the composed root we came from; we want the
                // ghost twin elsewhere.
                if let Some(first) = tail.first() {
                    if first == &"Skel" {
                        continue;
                    }
                }
            }
            let candidate_str = format!("/{root_name}{suffix}");
            // Avoid scanning ourselves.
            if candidate_str == composed_str {
                continue;
            }
            let candidate = match Path::new(&candidate_str) {
                Ok(p) => p,
                Err(_) => continue,
            };
            // Only proceed if this prim actually exists.
            if !matches!(stage.spec_type(candidate.clone()), Ok(Some(SpecType::Prim))) {
                continue;
            }
            if let Ok(Some(b)) = uskel::read_skel_binding(stage, &candidate) {
                if !b.blend_shape_targets.is_empty() || !b.blend_shapes.is_empty() {
                    return Some((b.blend_shapes, b.blend_shape_targets));
                }
            }
        }
    }
    let _ = parts.last();
    None
}

/// Read each `UsdSkelBlendShape` referenced by `binding`,
/// expand sparse `pointIndices` into dense per-vertex arrays, and
/// bake them as Bevy morph targets on `mesh`. Caps the number of
/// targets at `MAX_MORPH_WEIGHTS` (256 in 0.18.x); excess
/// blend shapes are silently dropped.
///
/// Critical detail: when the mesh's vertex buffer was *expanded* to
/// per-corner layout (because some primvar — usually FaceVarying
/// UVs — couldn't be represented per-USD-point), the morph image
/// must match that expanded count and replay the same
/// USD-point-to-corner mapping. Otherwise morph displacements end
/// up applied to the wrong vertices and the geometry shatters.
///
/// Returns `Some(target_count)` on success, `None` when no shapes
/// could be loaded.
fn bake_blend_shapes_into_mesh(
    stage: &Stage,
    binding: &uskel::ReadSkelBinding,
    read: &ugeom::ReadMesh,
    mesh: &mut Mesh,
    ctx: &mut BuildCtx<'_, '_>,
) -> Option<usize> {
    let max_targets = bevy::mesh::morph::MAX_MORPH_WEIGHTS;
    let take = binding.blend_shape_targets.len().min(max_targets);
    if take == 0 {
        return None;
    }
    // The mesh's actual vertex count after the build path's
    // (possibly expanded) per-corner layout.
    let bevy_vert_count = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|attr| match attr {
            bevy::mesh::VertexAttributeValues::Float32x3(v) => Some(v.len()),
            _ => None,
        })
        .unwrap_or(read.points.len());
    let usd_point_count = read.points.len();
    // `bevy_to_usd_point[bevy_vertex_ix]` = USD point index that
    // vertex was sourced from. For the indexed (unexpanded) path,
    // this is identity. For the expanded (per-corner) path, it's
    // `face_vertex_indices[corner]`.
    let bevy_to_usd_point: Vec<usize> = if bevy_vert_count == usd_point_count {
        (0..usd_point_count).collect()
    } else {
        read.face_vertex_indices
            .iter()
            .take(bevy_vert_count)
            .map(|i| (*i as usize).min(usd_point_count.saturating_sub(1)))
            .collect()
    };
    // Read each BlendShape's authored offsets + optional point
    // indices, expanded to dense per-USD-point arrays first, then
    // re-expanded into the mesh's actual Bevy-vertex layout.
    let mut targets: Vec<Vec<bevy::mesh::morph::MorphAttributes>> = Vec::with_capacity(take);
    for target_path in binding.blend_shape_targets.iter().take(take) {
        let p = match Path::new(target_path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let bs = match uskel::read_blend_shape(stage, &p).ok().flatten() {
            Some(b) => b,
            None => continue,
        };
        // Step 1: dense-per-USD-point offsets.
        let mut per_point_pos = vec![[0.0f32; 3]; usd_point_count];
        let mut per_point_nrm = vec![[0.0f32; 3]; usd_point_count];
        if bs.point_indices.is_empty() {
            for i in 0..bs.offsets.len().min(usd_point_count) {
                per_point_pos[i] = bs.offsets[i];
            }
            for i in 0..bs.normal_offsets.len().min(usd_point_count) {
                per_point_nrm[i] = bs.normal_offsets[i];
            }
        } else {
            for (i, ix) in bs.point_indices.iter().enumerate() {
                let v = *ix as usize;
                if v >= usd_point_count {
                    continue;
                }
                if let Some(o) = bs.offsets.get(i) {
                    per_point_pos[v] = *o;
                }
                if let Some(n) = bs.normal_offsets.get(i) {
                    per_point_nrm[v] = *n;
                }
            }
        }
        // Step 2: re-expand to Bevy-vertex layout via the
        // bevy→usd-point mapping derived from face_vertex_indices.
        let attrs: Vec<bevy::mesh::morph::MorphAttributes> = (0..bevy_vert_count)
            .map(|bi| {
                let pi = bevy_to_usd_point[bi];
                bevy::mesh::morph::MorphAttributes {
                    position: bevy::math::Vec3::from(per_point_pos[pi]),
                    normal: bevy::math::Vec3::from(per_point_nrm[pi]),
                    tangent: bevy::math::Vec3::ZERO,
                }
            })
            .collect();
        targets.push(attrs);
    }
    let vertex_count = bevy_vert_count;
    if targets.is_empty() {
        return None;
    }
    let target_count = targets.len();
    // Build morph image. Falls back gracefully on Bevy's per-target
    // and per-vertex-component limits.
    let image_result = bevy::mesh::morph::MorphTargetImage::new(
        targets.into_iter().map(|t| t.into_iter()),
        vertex_count,
        bevy::asset::RenderAssetUsages::default(),
    );
    let image = match image_result {
        Ok(i) => i,
        Err(e) => {
            bevy::log::warn!(
                "blendshape: failed to build morph image for {} ({e:?})",
                binding.prim_path
            );
            return None;
        }
    };
    let label = format!(
        "Morph:{}:{}targets",
        binding.prim_path.replace('/', "_"),
        target_count
    );
    bevy::log::info!(
        "blendshape: built morph image for {} → {target_count} targets, {vertex_count} verts",
        binding.prim_path
    );
    let image_handle = ctx.lc.add_labeled_asset(label, image.0);
    mesh.set_morph_targets(image_handle);
    // Also stash the target names so morph debug tools can find them.
    let names: Vec<String> = binding
        .blend_shapes
        .iter()
        .take(target_count)
        .cloned()
        .collect();
    mesh.set_morph_target_names(names);
    Some(target_count)
}

/// Compose the skinned mesh's effective "mesh-local → skel-world"
/// pre-skin transform. The standard skinning equation is
/// `joint_world * inv_bind * geomBindTransform * v_local`; when
/// `geomBindTransform` is authored as identity (Pixar's convention),
/// the asset author asserts that `v_local` is *already* in skel-world.
/// In practice they only flatten the mesh's *direct* xforms — not
/// every ancestor up to the SkelRoot. We compose ourselves: walk
/// from the mesh up to (but excluding) the SkelRoot, multiplying
/// each authored `xformOp:transform`, then fold in the authored
/// `primvars:skel:geomBindTransform`.
///
/// Returns `Mat4::IDENTITY` when no ancestor authors a transform and
/// no geomBindTransform is authored — the cheap path skips the
/// vertex rewrite.
fn effective_skin_pretransform(stage: &Stage, mesh_path: &Path) -> bevy::math::Mat4 {
    let mut composed = bevy::math::Mat4::IDENTITY;
    // Walk ancestors, top-down, multiplying their xforms.
    let mut chain: Vec<Path> = Vec::new();
    let mut cur = match mesh_path.parent() {
        Some(p) => p,
        None => return composed,
    };
    loop {
        if matches!(uskel::read_skel_root(stage, &cur), Ok(Some(_))) {
            break;
        }
        chain.push(cur.clone());
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    // chain is leaf-first; iterate in reverse to compose top-down.
    for p in chain.iter().rev() {
        if let Some(t) = uxf::read_transform(stage, p).ok().flatten() {
            let m = bevy::math::Mat4::from_scale_rotation_translation(
                bevy::math::Vec3::from(t.scale),
                bevy::math::Quat::from_xyzw(t.rotate[0], t.rotate[1], t.rotate[2], t.rotate[3]),
                bevy::math::Vec3::from(t.translate),
            );
            composed = composed * m;
        }
    }
    // Fold in the mesh's own authored geomBindTransform (typically
    // identity, but spec-required as the final mesh-local-to-bind
    // transform).
    if let Some(gbt) = read_geom_bind_transform(stage, mesh_path) {
        composed = composed * gbt;
    }
    composed
}

fn read_geom_bind_transform(stage: &Stage, prim: &Path) -> Option<bevy::math::Mat4> {
    use openusd::sdf::Value;
    let attr = prim.append_property("primvars:skel:geomBindTransform").ok()?;
    let v = stage.field::<Value>(attr, "default").ok().flatten()?;
    match v {
        Value::Matrix4d(m) => {
            let arr: [f32; 16] = std::array::from_fn(|i| m[i] as f32);
            Some(bevy::math::Mat4::from_cols_array(&arr))
        }
        _ => None,
    }
}

/// Walk up from `mesh_path` looking for the nearest ancestor whose
/// `skel:skeleton` rel is authored. USD inherits the binding from
/// SkelRoots and intermediate apiSchemas-bearing prims; this is the
/// correct fallback when the mesh itself doesn't author the rel.
fn inherited_skel_path(stage: &Stage, mesh_path: &Path) -> Option<String> {
    let mut cur = mesh_path.parent()?;
    loop {
        if let Ok(Some(skel)) = uskel::read_skel_root(stage, &cur) {
            if let Some(p) = skel.skeleton {
                return Some(p);
            }
        }
        // Even non-SkelRoot prims can author `skel:skeleton` (the
        // SkelBindingAPI applies to any Imageable). Probe directly so
        // we don't skip a binding-only intermediate Xform.
        if let Some(p) = direct_skel_rel(stage, &cur) {
            return Some(p);
        }
        match cur.parent() {
            Some(next) => cur = next,
            None => return None,
        }
    }
}

fn direct_skel_rel(stage: &Stage, prim: &Path) -> Option<String> {
    use openusd::sdf::Value;
    let rel = prim.append_property("skel:skeleton").ok()?;
    let raw = stage.field::<Value>(rel, "targetPaths").ok().flatten()?;
    let paths = match raw {
        Value::PathListOp(op) => op.flatten(),
        Value::PathVec(v) => v,
        _ => return None,
    };
    paths.into_iter().next().map(|p| p.as_str().to_string())
}

/// Short hash of a `ReadSkelBinding`. Folded into the mesh cache key
/// so two skinned-mesh prims with the same geometry but different
/// joint binding don't collide on the same cached vertex buffer.
fn skel_binding_hash(b: &uskel::ReadSkelBinding) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    b.skeleton.hash(&mut h);
    b.elements_per_vertex.hash(&mut h);
    b.joint_indices.hash(&mut h);
    for w in &b.joint_weights {
        w.to_bits().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// Replace the Skeleton's joint list with the sidecar
/// `UsdSkelAnimation`'s joint list (when sidecar anim exists) so
/// the mesh's `jointIndices` — which target the *animation's* joint
/// space, NOT the body-only rig.usd Skeleton's — resolve to the
/// right entities.
///
/// Pixar's HumanFemale ships:
/// - rig.usd: a 66-joint body skeleton (no face bones)
/// - walk.usd: a 109-joint full animation (body + face)
/// - mesh bindings: indices 0..108 against walk.usd's joint order
///
/// Naive append-missing produces correct *count* (109) but wrong
/// *order*: joints that exist in both lists end up at different
/// positions. Indices then route vertices to wrong joints — the
/// "elongated brush" symptom.
///
/// Strategy: take the anim's joint list as authoritative. For each
/// joint, prefer the original Skeleton's bindTransform when it
/// authors one (rig.usd's body joints have correct bind data); fall
/// back to composing anim's first-keyframe locals for joints only
/// the anim knows about. Rest transforms always come from the
/// anim's first sample (gives Pixar's intended walking-pose entry
/// point — the mesh deforms identically at frame 101 to frame 0
/// since rest = anim.first when anim is authoritative).
fn augment_skeleton_from_anim(
    skel: &mut uskel::ReadSkeleton,
    anims: &HashMap<String, usd_schemas::skel_anim_text::ReadSkelAnimText>,
) {
    let Some(anim) = anims.values().next() else {
        return;
    };
    if anim.joints.is_empty() {
        return;
    }
    let original_by_path: HashMap<&str, usize> = skel
        .joints
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), i))
        .collect();
    let first_t = anim.translations.values().next();
    let first_r = anim.rotations.values().next();
    let first_s = anim.scales.values().next();

    let mut new_joints = Vec::with_capacity(anim.joints.len());
    let mut new_rest = Vec::with_capacity(anim.joints.len());
    let mut new_bind = Vec::with_capacity(anim.joints.len());
    let mut new_path_to_idx: HashMap<String, usize> = HashMap::new();
    for (ai, jpath) in anim.joints.iter().enumerate() {
        // Build the anim's first-keyframe local (used for joints
        // the original skeleton lacks AND as the bind-derivation
        // source for those same joints).
        let t = first_t
            .and_then(|v| v.get(ai))
            .copied()
            .unwrap_or([0.0, 0.0, 0.0]);
        let r = first_r
            .and_then(|v| v.get(ai))
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 0.0]);
        let s = first_s
            .and_then(|v| v.get(ai))
            .copied()
            .unwrap_or([1.0, 1.0, 1.0]);
        let q = bevy::math::Quat::from_xyzw(r[1], r[2], r[3], r[0]);
        let anim_first_local = bevy::math::Mat4::from_scale_rotation_translation(
            bevy::math::Vec3::from(s),
            q,
            bevy::math::Vec3::from(t),
        );

        // Critical invariant: at rest pose, the composed joint
        // chain must equal the joint's bindTransform — otherwise
        // `joint.global * inv_bind ≠ identity` at rest and every
        // vertex bound to that joint gets displaced. For joints
        // that exist in the original Skeleton, prefer rig.usd's
        // authored restTransform (which composes exactly to
        // bindTransform — verified zero-drift earlier). For
        // augmented anim-only joints (face bones), fall back to the
        // anim's first sample as a "best available rest" so the
        // chain still composes coherently with the parent's bind.
        let (rest, bind) = if let Some(orig_idx) = original_by_path.get(jpath.as_str()) {
            let rest_arr = skel
                .rest_transforms
                .get(*orig_idx)
                .copied()
                .unwrap_or(IDENTITY_MAT4);
            let bind_arr = skel
                .bind_transforms
                .get(*orig_idx)
                .copied()
                .unwrap_or(IDENTITY_MAT4);
            (rest_arr, bind_arr)
        } else {
            let rest_arr = anim_first_local.to_cols_array();
            let parent_path = jpath.rsplit_once('/').map(|(p, _)| p);
            let parent_bind = parent_path
                .and_then(|p| new_path_to_idx.get(p).copied())
                .and_then(|pi| new_bind.get(pi).copied())
                .unwrap_or(IDENTITY_MAT4);
            let parent_mat = bevy::math::Mat4::from_cols_array(&parent_bind);
            let bind_arr = (parent_mat * anim_first_local).to_cols_array();
            (rest_arr, bind_arr)
        };
        new_rest.push(rest);
        new_bind.push(bind);
        new_path_to_idx.insert(jpath.clone(), new_joints.len());
        new_joints.push(jpath.clone());
    }

    let added = new_joints.len() as i64 - skel.joints.len() as i64;
    let reordered =
        new_joints.iter().zip(skel.joints.iter()).any(|(a, b)| a != b)
            || new_joints.len() != skel.joints.len();
    skel.joints = new_joints;
    skel.bind_transforms = new_bind;
    skel.rest_transforms = new_rest;
    if reordered {
        bevy::log::info!(
            "skel: rebuilt Skeleton joint order from sidecar anim (added {} joint(s))",
            added.max(0)
        );
    }
}

/// Decompose a USD `Matrix4d` (stored row-major as `[f32; 16]`) into a
/// Bevy `Transform`.
///
/// USD uses row-vector + row-major (`v * M`), with translation in the
/// last *row*. glam uses column-vector + column-major (`M * v`), with
/// translation in the last *column*. The two conventions store
/// equivalent transforms in the **same** memory layout — so reading
/// USD's row-major bytes via `Mat4::from_cols_array` produces a
/// mathematically correct glam matrix without any transpose. Adding a
/// USD's row-vector matrix convention with row-major storage and
/// glam's column-vector matrix convention with column-major storage
/// happen to share the SAME flat memory layout (they're transposes
/// of each other in both math and storage — the two transposes
/// cancel). So `Mat4::from_cols_array(usd_flat)` is already the
/// correct matrix in glam form, no transpose required.
/// `transpose()` here would flip the transform direction and send
/// vertices to nonsense positions.
fn transform_from_mat4_row_major(m: [f32; 16]) -> Transform {
    use bevy::math::Mat4;
    let glam_m = Mat4::from_cols_array(&m);
    let (scale, rot, trans) = glam_m.to_scale_rotation_translation();
    Transform {
        translation: trans,
        rotation: rot,
        scale,
    }
}

const IDENTITY_MAT4: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// Depth-first scan under `root` for the first prim whose `typeName`
/// matches `target_type`. Returns the absolute path on hit. Used by
/// `attach_skel_root` to find the Skeleton when the SkelRoot's
/// `skel:skeleton` rel is unauthored.
fn find_first_typed_descendant(stage: &Stage, root: &Path, target_type: &str) -> Option<Path> {
    for child_name in stage.prim_children(root.clone()).unwrap_or_default() {
        let Ok(child_path) = root.append_path(child_name.as_str()) else {
            continue;
        };
        if type_name_of(stage, &child_path).as_deref() == Some(target_type) {
            return Some(child_path);
        }
        if let Some(p) = find_first_typed_descendant(stage, &child_path, target_type) {
            return Some(p);
        }
    }
    None
}

/// Check `purpose` on `prim`; skip `"proxy"` / `"guide"` for M2. M6 lifts
/// this to a user-visible `UsdLoaderSettings::purposes` bitflag.
fn passes_purpose_filter(stage: &Stage, path: &Path) -> bool {
    match ugeom::read_purpose(stage, path)
        .ok()
        .unwrap_or_else(|| "default".into())
        .as_str()
    {
        "proxy" | "guide" => false,
        _ => true,
    }
}

/// If `prim` has a known geom type, build the mesh and attach
/// `Mesh3d` + `MeshMaterial3d` to `entity`.
fn attach_geometry(
    stage: &Stage,
    path: &Path,
    entity: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    let type_name: Option<String> = stage
        .field::<String>(path.clone(), "typeName")
        .ok()
        .flatten();
    let Some(type_name) = type_name else {
        return;
    };

    let mesh_handle = match type_name.as_str() {
        "Mesh" => {
            ctx.skinned_attempts += 1;
            // M4 content dedup: hash the decoded mesh data so two prims
            // composed from the same referenced spec collapse to one
            // Handle<Mesh>. This is what makes 80k-prim Isaac scenes
            // affordable — a greenhouse pole authored once under a
            // MeshLibrary Xform shows up at every lattice site with zero
            // extra vertex-buffer allocation.
            let Some(read) = ugeom::read_mesh(stage, path).ok().flatten() else {
                bevy::log::debug!("mesh: {} has no readable geometry", path.as_str());
                return;
            };
            // GeomSubsets: each subset owns a slice of the mesh's faces
            // and (usually) a distinct material binding. Spawn one
            // child entity per subset so Bevy's per-material draw-call
            // model matches USD's per-subset binding without us needing
            // a "split material" shader.
            if !read.subsets.is_empty() {
                spawn_mesh_with_subsets(stage, path, entity, world, ctx, &read);
                return;
            }
            // Skel binding (if any) is read here so a skinned mesh
            // bakes the joint indices/weights into its vertex buffer.
            // Cache key extends with a binding hash so two prims with
            // the same geometry + the same binding share, but different
            // bindings keep their own meshes. Set
            // `BEVY_OPENUSD_DISABLE_SKIN=1` to render every skinned
            // mesh as plain rest-pose geometry — useful when
            // diagnosing whether a deformation issue comes from the
            // skin pipeline or from elsewhere.
            let skinning_disabled = std::env::var("BEVY_OPENUSD_DISABLE_SKIN")
                .ok()
                .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
                .unwrap_or(false);
            let binding = if skinning_disabled {
                None
            } else {
                let mut b = uskel::read_skel_binding(stage, path).ok().flatten();
                // Workaround for openusd-rs composition: certain
                // attributes (skel:joints, skel:blendShapes) and
                // relationships (skel:blendShapeTargets) DON'T
                // surface through reference composition the way
                // primvars do. Pixar's HumanFemale.full_payload.usd
                // referenced into our wrapper exposes the binding
                // primvars at /Skel/Geometry/... but blend-shape
                // metadata only at the ghost top-level
                // /HumanFemale_Group/... clone. Patch up by reading
                // from the ghost twin when our composed read came
                // back empty.
                if let Some(ref mut bb) = b {
                    if bb.blend_shapes.is_empty() && bb.blend_shape_targets.is_empty() {
                        if let Some((g_names, g_targets)) =
                            blend_shapes_from_ghost_twin(stage, path)
                        {
                            bb.blend_shapes = g_names;
                            bb.blend_shape_targets = g_targets;
                        }
                    }
                }
                b
            };
            let cache_key = match &binding {
                None => mesh_content_hash(&read),
                Some(b) => format!(
                    "{}_skin_{}",
                    mesh_content_hash(&read),
                    skel_binding_hash(b)
                ),
            };
            // Blendshape names referenced by this mesh (bound via
            // SkelBindingAPI). Folded into the cache key + sets the
            // morph image on the resulting Bevy mesh asset.
            let blend_names: Vec<String> = binding
                .as_ref()
                .map(|b| b.blend_shapes.clone())
                .unwrap_or_default();
            let bs_cache_key = if blend_names.is_empty() {
                String::new()
            } else {
                format!("_bs_{}_{}", blend_names.len(), path.as_str())
            };
            let cache_key = format!("{cache_key}{bs_cache_key}");
            let mesh_handle = if let Some(h) = ctx.mesh_cache.get(&cache_key) {
                h.clone()
            } else {
                let mut bevy_mesh = match &binding {
                    Some(b) => {
                        // Resolve target Skeleton's joint count so we
                        // can clamp out-of-range indices.
                        let target_skel = b
                            .skeleton
                            .clone()
                            .or_else(|| inherited_skel_path(stage, path));
                        let max_joints = target_skel
                            .as_deref()
                            .and_then(|t| ctx.skel_cache.get(t))
                            .map(|info| info.joint_entities.len() as u16)
                            .unwrap_or(0);
                        let skin = skin_attrs_from_binding(b, read.points.len(), max_joints);
                        // Compose the mesh-local-to-skel-world
                        // pretransform: parent xform chain + the
                        // authored geomBindTransform. Pixar's
                        // `bakeSkinning.cpp:1543-1556` shows the
                        // canonical formula assumes mesh.points are
                        // already in skel-world (identity
                        // geomBindTransform → "I'm in skel-world").
                        // BUT Pixar's HumanFemale asset authors
                        // identity geomBindTransform on every mesh
                        // while leaving non-identity ancestor xforms
                        // (e.g. ShoesHumanFlats has scale=0.69 —
                        // shoes render 45% too large without
                        // composing it). That's a real
                        // asset-vs-spec inconsistency we have to
                        // absorb: bake the parent chain into the
                        // points so post-skin world position matches
                        // what Pixar's renderer would produce
                        // (which DOES apply the parent chain at the
                        // end via `mesh_chain * v_local_skinned`).
                        let mut read_for_skin = read.clone();
                        let pre = effective_skin_pretransform(stage, path);
                        if pre != bevy::math::Mat4::IDENTITY {
                            for p in read_for_skin.points.iter_mut() {
                                let v = pre.transform_point3(bevy::math::Vec3::from(*p));
                                *p = [v.x, v.y, v.z];
                            }
                        }
                        mesh_from_usd_with_skin(&read_for_skin, &skin)
                    }
                    None => mesh_from_usd(&read),
                };
                // Bake blend-shape morph targets onto the mesh.
                if let Some(b) = &binding {
                    if !b.blend_shape_targets.is_empty() {
                        let _ = bake_blend_shapes_into_mesh(
                            stage,
                            b,
                            &read,
                            &mut bevy_mesh,
                            ctx,
                        );
                    }
                }
                let handle = ctx.add_mesh_labeled(format!("Mesh:{cache_key}"), bevy_mesh);
                ctx.mesh_cache.insert(cache_key, handle.clone());
                handle
            };
            // Attach SkinnedMesh component when binding resolves to a
            // cached skel — looks up the joint entities + inverse
            // bindposes built when the SkelRoot ancestor was walked.
            if let Some(b) = &binding {
                let skel_target = b
                    .skeleton
                    .clone()
                    .or_else(|| inherited_skel_path(stage, path));
                if let Some(t) = skel_target.as_deref()
                    && let Some(info) = ctx.skel_cache.get(t).cloned()
                {
                    if let Some((joints, ibps_handle)) =
                        build_subset_skinned_mesh(&info, b, &mut ctx.lc, path)
                    {
                        world.entity_mut(entity).insert(
                            bevy::mesh::skinning::SkinnedMesh {
                                inverse_bindposes: ibps_handle,
                                joints,
                            },
                        );
                        ctx.skinned_attached += 1;
                    } else {
                        ctx.skinned_failed += 1;
                        bevy::log::debug!(
                            "skel: build_subset_skinned_mesh None for {}",
                            path.as_str()
                        );
                    }
                } else {
                    ctx.skinned_no_cache += 1;
                    bevy::log::warn!(
                        "skel: skin attrs baked but no skel cache for {} (target={:?})",
                        path.as_str(),
                        skel_target
                    );
                }
                // BlendShape weights binding (initial all-zero) +
                // name mapping for the runtime driver. Cap to Bevy's
                // MAX_MORPH_WEIGHTS (256). Pixar's facial meshes
                // routinely author 300+ blend shapes; we drop the
                // tail. The weight vector length must match the
                // mesh's `morph_targets` image layer count, set by
                // `bake_blend_shapes_into_mesh`.
                if !b.blend_shape_targets.is_empty() {
                    // Both the morph image (built in
                    // `bake_blend_shapes_into_mesh`) and the
                    // MeshMorphWeights buffer must have IDENTICAL
                    // lengths or Bevy's render extract uploads
                    // garbage. Use the target-count (the actual
                    // BlendShape relationship count) as the source
                    // of truth and pad the names list if it's
                    // shorter (some assets author target rels
                    // without parallel skel:blendShapes tokens).
                    let max_w = bevy::mesh::morph::MAX_MORPH_WEIGHTS;
                    let take = b.blend_shape_targets.len().min(max_w);
                    let mut names: Vec<String> = b
                        .blend_shapes
                        .iter()
                        .take(take)
                        .cloned()
                        .collect();
                    while names.len() < take {
                        names.push(String::new());
                    }
                    if let Ok(mw) =
                        bevy::mesh::morph::MeshMorphWeights::new(vec![0.0_f32; take])
                    {
                        world
                            .entity_mut(entity)
                            .insert(mw)
                            .insert(UsdBlendShapeBinding { names });
                        ctx.blendshape_attached += 1;
                    }
                }
            }
            mesh_handle
        }
        "Cube" => {
            let size = ugeom::read_cube_size(stage, path)
                .ok()
                .flatten()
                .unwrap_or(1.0);
            let cache_key = format!("Cube:size={size:.6}");
            ctx.mesh_cache.get(&cache_key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(cache_key.clone(), mesh_cube(size));
                ctx.mesh_cache.insert(cache_key, h.clone());
                h
            })
        }
        "Sphere" => {
            let r = ugeom::read_sphere_radius(stage, path)
                .ok()
                .flatten()
                .unwrap_or(1.0);
            let cache_key = format!("Sphere:r={r:.6}");
            ctx.mesh_cache.get(&cache_key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(cache_key.clone(), mesh_sphere(r));
                ctx.mesh_cache.insert(cache_key, h.clone());
                h
            })
        }
        "Cylinder" => {
            let Some(p) = ugeom::read_cylinder(stage, path).ok().flatten() else {
                bevy::log::debug!("cylinder: {} missing radius/height", path.as_str());
                return;
            };
            let cache_key = format!(
                "Cylinder:r={:.6}:h={:.6}:axis={:?}",
                p.radius, p.height, p.axis
            );
            ctx.mesh_cache.get(&cache_key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(cache_key.clone(), mesh_cylinder(p));
                ctx.mesh_cache.insert(cache_key, h.clone());
                h
            })
        }
        "Capsule" => {
            let Some(p) = ugeom::read_capsule(stage, path).ok().flatten() else {
                bevy::log::debug!("capsule: {} missing radius/height", path.as_str());
                return;
            };
            let cache_key = format!(
                "Capsule:r={:.6}:h={:.6}:axis={:?}",
                p.radius, p.height, p.axis
            );
            ctx.mesh_cache.get(&cache_key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(cache_key.clone(), mesh_capsule(p));
                ctx.mesh_cache.insert(cache_key, h.clone());
                h
            })
        }
        "Plane" => {
            // UsdGeom.Plane authors `width`/`length`/`axis`; fall back to
            // 1×1 if unauthored.
            let w = ugeom::read_double_attr(stage, path, "width").unwrap_or(1.0);
            let l = ugeom::read_double_attr(stage, path, "length").unwrap_or(1.0);
            let cache_key = format!("Plane:w={w:.6}:l={l:.6}");
            ctx.mesh_cache.get(&cache_key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(cache_key.clone(), mesh_plane(w, l));
                ctx.mesh_cache.insert(cache_key, h.clone());
                h
            })
        }
        "PointInstancer" => {
            // Fans out into many Mesh3d children carrying per-instance
            // Transforms. The current entity stays as the grouping Xform.
            spawn_point_instancer_children(stage, path, entity, world, ctx);
            return;
        }
        "BasisCurves" => {
            let Some(read) = ugeom::read_curves(stage, path).ok().flatten() else {
                bevy::log::debug!("curves: {} missing points/vertexCounts", path.as_str());
                return;
            };
            let mesh = curves_mesh(&read, ctx.curve_default_radius, ctx.curve_ring_segments);
            let handle = ctx.add_mesh_labeled(format!("Curves:{}", path.as_str()), mesh);
            // Tubes want lit material so you can see the shaded
            // curvature — unlit is fine for flat lines but makes tubes
            // read as flat ribbons. Fall back to default gray material.
            let mat = ctx.default_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        "NurbsCurves" => {
            let Some(nurbs) = ugeom::read_nurbs_curves(stage, path).ok().flatten() else {
                bevy::log::debug!(
                    "nurbs_curves: {} missing points/vertexCounts",
                    path.as_str()
                );
                return;
            };
            // Sample NURBS to a polyline and feed the existing tube
            // builder. Cox-de-Boor lives in `crates/bevy_openusd/src/curves.rs`.
            let read = nurbs_to_read_curves(&nurbs);
            let mesh = curves_mesh(&read, ctx.curve_default_radius, ctx.curve_ring_segments);
            let handle = ctx.add_mesh_labeled(format!("NurbsCurves:{}", path.as_str()), mesh);
            let mat = ctx.default_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        "NurbsPatch" => {
            let Some(read) = ugeom::read_nurbs_patch(stage, path).ok().flatten() else {
                bevy::log::debug!(
                    "nurbs_patch: {} missing required attrs",
                    path.as_str()
                );
                return;
            };
            let mesh = nurbs_patch_to_bevy_mesh(&read);
            let handle = ctx.add_mesh_labeled(format!("NurbsPatch:{}", path.as_str()), mesh);
            let mat = ctx.default_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        "TetMesh" => {
            let Some(read) = ugeom::read_tetmesh(stage, path).ok().flatten() else {
                bevy::log::debug!("tetmesh: {} missing points/tetVertexIndices", path.as_str());
                return;
            };
            let mesh = tetmesh_to_bevy_mesh(&read);
            let handle = ctx.add_mesh_labeled(format!("TetMesh:{}", path.as_str()), mesh);
            let mat = ctx.default_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        "HermiteCurves" => {
            let Some(hermite) = ugeom::read_hermite_curves(stage, path).ok().flatten() else {
                bevy::log::debug!(
                    "hermite_curves: {} missing points/vertexCounts",
                    path.as_str()
                );
                return;
            };
            // Cubic Hermite samples each segment with its authored
            // tangents, then we feed the dense polyline to the
            // existing tube builder.
            let read = hermite_to_read_curves(&hermite);
            let mesh = curves_mesh(&read, ctx.curve_default_radius, ctx.curve_ring_segments);
            let handle =
                ctx.add_mesh_labeled(format!("HermiteCurves:{}", path.as_str()), mesh);
            let mat = ctx.default_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        "Points" => {
            let Some(read) = ugeom::read_points(stage, path).ok().flatten() else {
                bevy::log::debug!("points: {} missing points", path.as_str());
                return;
            };
            let mesh = points_mesh(&read, ctx.point_scale);
            let handle = ctx.add_mesh_labeled(format!("Points:{}", path.as_str()), mesh);
            let mat = ctx.unlit_vertex_color_material();
            world
                .entity_mut(entity)
                .insert((Mesh3d(handle), MeshMaterial3d(mat)));
            return;
        }
        _ => return,
    };

    // For a Mesh, pick up the displayColor fallback so a primvar-only
    // nav-graph edge renders in the authored colour rather than the
    // default-gray wash. Also observe the `doubleSided` gprim flag so
    // we render front+back when the authoring tool asked. Non-Mesh
    // geoms fall through to the normal path.
    // `has_display_color = true` only when the authored displayColor
    // primvar carries *meaningful* colour variation (or a non-white
    // single value). Production assets like Pixar's Kitchen_set
    // routinely author `primvars:displayColor = [(1, 1, 1)]` as a
    // placeholder — treating that as "real" displayColor sends every
    // mesh through the white-base-vertex-modulated material and
    // produces the uniform white wall the user sees. Deferring to the
    // path-hashed fallback for trivial-white meshes restores per-prop
    // colour variety.
    let (has_display_color, double_sided, extent) = if type_name == "Mesh" {
        ugeom::read_mesh(stage, path)
            .ok()
            .flatten()
            .map(|r| {
                let useful = r
                    .display_color
                    .as_ref()
                    .map(|p| display_color_is_useful(&p.values))
                    .unwrap_or(false);
                (useful, r.double_sided, r.extent)
            })
            .unwrap_or((false, false, None))
    } else {
        (false, false, None)
    };
    let mat =
        resolve_material_with_display_color(stage, path, ctx, has_display_color, double_sided);
    let mut entity_mut = world.entity_mut(entity);
    entity_mut.insert((Mesh3d(mesh_handle), MeshMaterial3d(mat)));
    if let Some([min, max]) = extent {
        entity_mut.insert(UsdLocalExtent { min, max });
    }
}

/// Spawn one child entity per `GeomSubset` on `mesh_path`. Each child
/// carries a Mesh3d restricted to the subset's face indices, plus the
/// subset's own material binding (falling back to the parent's binding,
/// then default-gray). Also emits one child for any faces *not* claimed
/// by a subset so the unassigned part of the mesh stays visible.
fn spawn_mesh_with_subsets(
    stage: &Stage,
    mesh_path: &Path,
    parent: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
    read: &ugeom::ReadMesh,
) {
    // Track which face indices any subset claims so we can emit a
    // residual "rest" child for unclaimed faces — USD allows a mesh to
    // be partially partitioned by subsets, and a correct renderer needs
    // to keep the leftover polys.
    let face_count = read.face_vertex_counts.len();
    let mut claimed = vec![false; face_count];

    // Parent material binding — used as the fallback for any subset
    // that doesn't author its own material:binding.
    let parent_binding = ushade::read_material_binding(stage, mesh_path)
        .ok()
        .flatten();

    // Skel binding: when the parent mesh is skinned, every subset
    // inherits the same per-vertex joint data. Apple's hummingbird
    // motion-blur wing planes (`wing_blur_left/right`) are the
    // canonical case — they author skin binding on the Mesh and use a
    // GeomSubset only to bind a different (alpha) material to a few
    // faces. Without skinning the subsets, those planes sit at rest
    // pose while the rest of the bird flies away.
    let skinning_disabled = std::env::var("BEVY_OPENUSD_DISABLE_SKIN")
        .ok()
        .map(|v| matches!(v.as_str(), "1" | "true" | "on"))
        .unwrap_or(false);
    let skel_binding = if skinning_disabled {
        None
    } else {
        uskel::read_skel_binding(stage, mesh_path).ok().flatten()
    };
    let skel_target = skel_binding.as_ref().and_then(|b| {
        b.skeleton
            .clone()
            .or_else(|| inherited_skel_path(stage, mesh_path))
    });
    let skel_info = skel_target
        .as_deref()
        .and_then(|t| ctx.skel_cache.get(t).cloned());
    // Pre-applied transform (parent xform chain × geomBindTransform)
    // is baked into the mesh's POINTS so skinning math sees them in
    // the same space the inverse-bindposes were captured in.
    let pretransform = if skel_info.is_some() {
        Some(effective_skin_pretransform(stage, mesh_path))
    } else {
        None
    };
    let max_joints = skel_info
        .as_ref()
        .map(|info| info.joint_entities.len() as u16)
        .unwrap_or(0);
    let skin_attrs = skel_binding.as_ref().map(|b| {
        skin_attrs_from_binding(b, read.points.len(), max_joints)
    });

    // Apply pretransform once to a working ReadMesh — every subset
    // shares the same point buffer.
    let read_for_skin = if let Some(pre) = pretransform {
        if pre != bevy::math::Mat4::IDENTITY {
            let mut r = read.clone();
            for p in r.points.iter_mut() {
                let v = pre.transform_point3(bevy::math::Vec3::from(*p));
                *p = [v.x, v.y, v.z];
            }
            std::borrow::Cow::Owned(r)
        } else {
            std::borrow::Cow::Borrowed(read)
        }
    } else {
        std::borrow::Cow::Borrowed(read)
    };

    // Build the joints + inverse-bindposes ONCE for the whole mesh —
    // every subset reuses the same SkinnedMesh handles.
    let skinned_handles = if let (Some(b), Some(info)) = (skel_binding.as_ref(), skel_info.as_ref())
    {
        match build_subset_skinned_mesh(info, b, &mut ctx.lc, mesh_path) {
            Some(pair) => Some(pair),
            None => {
                ctx.skinned_failed += 1;
                None
            }
        }
    } else {
        None
    };
    let attach_skin = |entity: Entity, world: &mut World, ctx: &mut BuildCtx<'_, '_>| {
        if let Some((joints, ibps_handle)) = skinned_handles.clone() {
            world
                .entity_mut(entity)
                .insert(bevy::mesh::skinning::SkinnedMesh {
                    inverse_bindposes: ibps_handle,
                    joints,
                });
            ctx.skinned_attached += 1;
        }
    };

    for (subset_ix, subset) in read.subsets.iter().enumerate() {
        for ix in &subset.indices {
            if let Some(slot) = claimed.get_mut(*ix as usize) {
                *slot = true;
            }
        }
        let mesh = match skin_attrs.as_ref() {
            Some(skin) => mesh_from_usd_subset_with_skin(
                read_for_skin.as_ref(),
                Some(&subset.indices),
                Some(skin),
            ),
            None => mesh_from_usd_subset(read, Some(&subset.indices)),
        };
        let label = format!("Mesh:{}:subset{subset_ix}:{}", mesh_path.as_str(), subset.name);
        let mesh_handle = ctx.add_mesh_labeled(label, mesh);
        let binding = subset.material_binding.clone().or_else(|| parent_binding.clone());
        let mat = match binding {
            Some(mat_prim) => ctx.material_for(stage, &mat_prim, read.double_sided),
            None if read.display_color.is_some() => {
                ctx.vertex_color_modulated_material_ds(read.double_sided)
            }
            None => ctx.default_material_ds(read.double_sided),
        };
        let child_path = format!("{}/{}", mesh_path.as_str(), subset.name);
        let entity = world
            .spawn((
                Name::new(subset.name.clone()),
                Transform::IDENTITY,
                Visibility::default(),
                UsdPrimRef::new(&child_path),
                ChildOf(parent),
                Mesh3d(mesh_handle),
                MeshMaterial3d(mat),
            ))
            .id();
        attach_skin(entity, world, ctx);
    }

    // Residual unclaimed faces. USD doesn't require exhaustive partitioning,
    // so dropping them would be a correctness bug.
    let residual: Vec<i32> = (0..face_count as i32)
        .filter(|i| !claimed[*i as usize])
        .collect();
    if !residual.is_empty() {
        let mesh = match skin_attrs.as_ref() {
            Some(skin) => mesh_from_usd_subset_with_skin(
                read_for_skin.as_ref(),
                Some(&residual),
                Some(skin),
            ),
            None => mesh_from_usd_subset(read, Some(&residual)),
        };
        let label = format!("Mesh:{}:residual", mesh_path.as_str());
        let mesh_handle = ctx.add_mesh_labeled(label, mesh);
        let mat = match parent_binding {
            Some(mat_prim) => ctx.material_for(stage, &mat_prim, read.double_sided),
            None if read.display_color.is_some() => {
                ctx.vertex_color_modulated_material_ds(read.double_sided)
            }
            None => ctx.default_material_ds(read.double_sided),
        };
        world
            .entity_mut(parent)
            .insert((Mesh3d(mesh_handle), MeshMaterial3d(mat)));
        attach_skin(parent, world, ctx);
    }
}

/// Compute a stable fingerprint for an instanceable prim's subtree.
/// Captures internal structure (descendant leaf names + typeNames,
/// ordered) plus mesh content hashes where meshes show up. The
/// instance prim's OWN leaf name is deliberately excluded — two
/// instance sites of the same prototype will always carry different
/// leaf names (e.g. `InstanceA` vs `InstanceB`), but that's exactly
/// the dedup opportunity we want to detect.
///
/// Intentionally coarse — attributes like transforms on intermediate
/// Xforms aren't folded in. The prototype's SHAPE is what we're
/// deduping; per-instance transforms live on the instance prim
/// itself, not inside the prototype.
fn prototype_fingerprint(stage: &Stage, path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    // Root's typeName only (skip leaf name — that's site-specific).
    let root_type = stage
        .field::<String>(path.clone(), "typeName")
        .ok()
        .flatten()
        .unwrap_or_default();
    root_type.hash(&mut h);

    fn fold_descendants(stage: &Stage, path: &Path, h: &mut DefaultHasher) {
        let mut children: Vec<String> = stage
            .prim_children(path.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.to_string())
            .collect();
        children.sort();
        for name in children {
            let Ok(child) = path.append_path(name.as_str()) else {
                continue;
            };
            name.hash(h);
            let type_name = stage
                .field::<String>(child.clone(), "typeName")
                .ok()
                .flatten()
                .unwrap_or_default();
            type_name.hash(h);
            if type_name == "Mesh" {
                if let Ok(Some(read)) = ugeom::read_mesh(stage, &child) {
                    mesh_content_hash(&read).hash(h);
                }
            }
            fold_descendants(stage, &child, h);
        }
    }
    fold_descendants(stage, path, &mut h);
    format!("proto_{:016x}", h.finish())
}

/// Context for M28 prototype replay. Built when an instanceable prim
/// matches a fingerprint whose descriptor list is already cached.
struct ReplayCtx {
    descriptors: Vec<PrototypeDescriptor>,
    instance_prim_path: String,
}

/// Spawn the descriptor list under the current instance site. Wires
/// up `ChildOf` based on descriptor relative paths so the subtree's
/// internal hierarchy is preserved.
fn replay_prototype(
    rc: &ReplayCtx,
    root_path: &Path,
    parent: Entity,
    world: &mut World,
    _ctx: &mut BuildCtx<'_, '_>,
) {
    // Spawn the root entity for this instance site first — it owns
    // the instance's own `UsdPrimRef` and transform.
    // The descriptors' relative paths are anchored at this root.
    let root_leaf = root_path.name().unwrap_or("").to_string();
    let instance_root = world
        .spawn((
            Name::new(root_leaf),
            // Transform::IDENTITY — the caller passed the authored
            // transform onto `parent`'s ChildOf relationship already.
            // Wait: the caller in spawn_prim_subtree DID set a
            // transform on a fresh entity earlier and then returned
            // before reaching us. We now have no entity spawned for
            // this prim yet. Recreate it here with the authored
            // transform so the instance root sits correctly.
            Transform::IDENTITY,
            Visibility::default(),
            UsdPrimRef::new(root_path.as_str()),
            ChildOf(parent),
        ))
        .id();

    // Map: relative_path → spawned entity, so descriptors whose
    // parent_relative_path resolves can re-wire correctly.
    let mut by_rel: HashMap<String, Entity> = HashMap::new();
    by_rel.insert(String::new(), instance_root);

    for d in &rc.descriptors {
        // Skip the root entry (relative_path == "") — we just created
        // it above with the instance's own UsdPrimRef.
        if d.relative_path.is_empty() {
            continue;
        }
        let parent_rel = d.parent_relative_path.clone().unwrap_or_default();
        let parent_entity = *by_rel.get(&parent_rel).unwrap_or(&instance_root);
        let instance_prim_ref = UsdPrimRef::new(format!(
            "{}{}",
            rc.instance_prim_path, d.relative_path
        ));
        let mut e = world.spawn((
            Name::new(d.leaf_name.clone()),
            d.relative_transform,
            d.visibility,
            instance_prim_ref,
            ChildOf(parent_entity),
        ));
        if let Some(mesh) = &d.mesh {
            e.insert(Mesh3d(mesh.clone()));
            if let Some(mat) = &d.material {
                e.insert(MeshMaterial3d(mat.clone()));
            }
        }
        if let Some(ref kind) = d.kind {
            e.insert(UsdKind { kind: kind.clone() });
        }
        if let Some(ext) = d.local_extent {
            e.insert(ext);
        }
        let id = e.id();
        by_rel.insert(d.relative_path.clone(), id);
    }
}

/// Prim types whose subtree we can safely replay from a descriptor
/// list. Anything else (lights, cameras, curves, points, point
/// instancers, the exotic stuff) poisons the recording so future
/// matches fall back to a full walk.
fn is_replayable_type(type_name: Option<&str>) -> bool {
    matches!(
        type_name.unwrap_or(""),
        "" | "Xform"
            | "Scope"
            | "Mesh"
            | "Cube"
            | "Sphere"
            | "Cylinder"
            | "Capsule"
            | "Plane"
    )
}

fn type_name_of(stage: &Stage, prim: &Path) -> Option<String> {
    stage
        .field::<String>(prim.clone(), "typeName")
        .ok()
        .flatten()
}

/// Hash a decoded UsdGeom.Mesh down to a short string suitable for use as a
/// cache key and asset label. Folds in geometry **and** primvars that
/// affect the emitted vertex-buffer layout: two meshes with identical
/// points but different `primvars:displayColor` must produce different
/// hashes so they don't collide in the mesh cache.
fn mesh_content_hash(read: &ugeom::ReadMesh) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for p in &read.points {
        p[0].to_bits().hash(&mut h);
        p[1].to_bits().hash(&mut h);
        p[2].to_bits().hash(&mut h);
    }
    read.face_vertex_counts.hash(&mut h);
    read.face_vertex_indices.hash(&mut h);
    (read.points.len(), read.face_vertex_counts.len()).hash(&mut h);
    // Authored displayColor: interpolation mode + values + indices.
    // Skip Vertex-interpolated identical-length arrays? No — colors
    // differ per-prim; folding everything in is cheap for the array
    // sizes we see.
    if let Some(dc) = &read.display_color {
        format!("{:?}", dc.interpolation).hash(&mut h);
        for v in &dc.values {
            v[0].to_bits().hash(&mut h);
            v[1].to_bits().hash(&mut h);
            v[2].to_bits().hash(&mut h);
        }
        dc.indices.hash(&mut h);
    }
    if let Some(dop) = &read.display_opacity {
        format!("{:?}", dop.interpolation).hash(&mut h);
        for v in &dop.values {
            v.to_bits().hash(&mut h);
        }
        dop.indices.hash(&mut h);
    }
    // Normals & UVs same story — they're compiled into the emitted
    // vertex buffer, so a mesh cache hit would reuse the wrong data.
    // Include `.indices` too: two prims can share normal/uv *values* but
    // index them differently per-corner, which would otherwise alias.
    if let Some(ns) = &read.normals {
        format!("{:?}", ns.interpolation).hash(&mut h);
        for v in &ns.values {
            v[0].to_bits().hash(&mut h);
            v[1].to_bits().hash(&mut h);
            v[2].to_bits().hash(&mut h);
        }
        ns.indices.hash(&mut h);
    }
    if let Some(uvs) = &read.uvs {
        format!("{:?}", uvs.interpolation).hash(&mut h);
        for v in &uvs.values {
            v[0].to_bits().hash(&mut h);
            v[1].to_bits().hash(&mut h);
        }
        uvs.indices.hash(&mut h);
    }
    // Orientation flips winding — same data, different mesh.
    format!("{:?}", read.orientation).hash(&mut h);
    format!("meshdata_{:016x}", h.finish())
}

/// Spawn one child entity per instance of a `UsdGeom.PointInstancer`.
///
/// USD authors:
///   - `positions: point3f[]`, required
///   - `orientations: quath[] / quatf[]`, optional
///   - `scales: float3[]` or `vector3f[]`, optional
///   - `protoIndices: int[]`, required (index into `prototypes`)
///   - `prototypes: rel` pointing at a list of prim paths
///
/// The instancer itself stays as its own Xform entity; each instance becomes
/// a child with its prototype's `Mesh3d` + a per-instance `Transform`. All
/// instances share the prototype's Handle via the mesh cache.
fn spawn_point_instancer_children(
    stage: &Stage,
    path: &Path,
    parent: Entity,
    world: &mut World,
    ctx: &mut BuildCtx<'_, '_>,
) {
    let Some(data) = ugeom::read_point_instancer(stage, path).ok().flatten() else {
        bevy::log::debug!("PointInstancer: {} missing required attrs", path.as_str());
        return;
    };

    // Resolve every prototype to a (mesh, material) handle pair up front so
    // we don't repeat the walk per instance.
    let mut proto_gm: Vec<
        Option<(
            bevy::asset::Handle<Mesh>,
            bevy::asset::Handle<StandardMaterial>,
        )>,
    > = Vec::with_capacity(data.prototypes.len());
    for proto_path in &data.prototypes {
        let gm = resolve_mesh_and_material(stage, proto_path, ctx);
        proto_gm.push(gm);
    }

    let count = data.positions.len();
    for i in 0..count {
        let proto_ix = *data.proto_indices.get(i).unwrap_or(&0) as usize;
        let Some(Some((mesh, mat))) = proto_gm.get(proto_ix) else {
            continue;
        };
        let pos = data.positions[i];
        let rot_wxyz = data
            .orientations
            .get(i)
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 0.0]);
        let scale = data.scales.get(i).copied().unwrap_or([1.0, 1.0, 1.0]);
        let transform = Transform {
            translation: Vec3::from(pos),
            rotation: Quat::from_xyzw(rot_wxyz[1], rot_wxyz[2], rot_wxyz[3], rot_wxyz[0]),
            scale: Vec3::from(scale),
        };
        world.spawn((
            Name::new(format!("{}#{i}", path.name().unwrap_or("Instance"))),
            transform,
            Visibility::default(),
            UsdPrimRef::new(format!("{}#{i}", path.as_str())),
            ChildOf(parent),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(mat.clone()),
        ));
    }
}

/// Build (or fetch from cache) the `Mesh3d` + `MeshMaterial3d` for an
/// arbitrary geom prim path without spawning a new entity. Used by
/// PointInstancer to look up its prototypes.
fn resolve_mesh_and_material(
    stage: &Stage,
    proto_path: &Path,
    ctx: &mut BuildCtx<'_, '_>,
) -> Option<(
    bevy::asset::Handle<Mesh>,
    bevy::asset::Handle<StandardMaterial>,
)> {
    let type_name: String = stage
        .field::<String>(proto_path.clone(), "typeName")
        .ok()
        .flatten()
        .unwrap_or_default();
    // Pixar's PointInstancedMedCity (and most production-instancer
    // assets) author each `prototypes` target as an Xform group with
    // the actual `Mesh` / `Cube` / etc. as a child — not as a Mesh
    // directly. Descend until we find a renderable leaf so the
    // instancer's 40k cells don't drop to zero just because the
    // prototype is wrapped.
    let renderable = matches!(
        type_name.as_str(),
        "Mesh" | "Cube" | "Sphere" | "Cylinder" | "Capsule" | "Plane"
    );
    if !renderable {
        if let Some(child_path) = first_renderable_descendant(stage, proto_path) {
            return resolve_mesh_and_material(stage, &child_path, ctx);
        }
        return None;
    }
    let mesh = match type_name.as_str() {
        "Mesh" => {
            let read = ugeom::read_mesh(stage, proto_path).ok().flatten()?;
            // Mixed-up-axis fix for PointInstancer prototypes:
            // PointInstancedMedCity's stage authors `upAxis = "Z"` but
            // each prototype mesh was exported Y-up (height in Y). The
            // stage's root_basis_transform rotates the WHOLE scene -90°
            // around X to convert Z-up positions into Bevy Y-up — but
            // that same rotation also tips Y-up meshes onto their side.
            // Detect by examining the mesh extent: if Y dominates Z
            // (height > depth) in a stage that claims Z-up, the mesh is
            // mis-authored and we counter-rotate +90° X here. The cache
            // key already reflects the rotated geometry through the
            // mesh contents, so two prototypes with the same source and
            // same rotation share a Bevy mesh handle.
            let needs_yup_fix = stage_is_z_up(stage) && mesh_is_y_up(&read);
            let key = format!(
                "{}{}",
                mesh_content_hash(&read),
                if needs_yup_fix { "_yup_fix" } else { "" }
            );
            if let Some(h) = ctx.mesh_cache.get(&key) {
                h.clone()
            } else {
                let mut bevy_mesh = mesh_from_usd(&read);
                if needs_yup_fix {
                    crate::mesh::rotate_mesh(
                        &mut bevy_mesh,
                        bevy::math::Quat::from_rotation_x(core::f32::consts::FRAC_PI_2),
                    );
                }
                let h = ctx.add_mesh_labeled(format!("Mesh:{key}"), bevy_mesh);
                ctx.mesh_cache.insert(key, h.clone());
                h
            }
        }
        "Cube" => {
            let size = ugeom::read_cube_size(stage, proto_path)
                .ok()
                .flatten()
                .unwrap_or(1.0);
            let key = format!("Cube:size={size:.6}");
            ctx.mesh_cache.get(&key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(key.clone(), mesh_cube(size));
                ctx.mesh_cache.insert(key, h.clone());
                h
            })
        }
        "Sphere" => {
            let r = ugeom::read_sphere_radius(stage, proto_path)
                .ok()
                .flatten()
                .unwrap_or(1.0);
            let key = format!("Sphere:r={r:.6}");
            ctx.mesh_cache.get(&key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(key.clone(), mesh_sphere(r));
                ctx.mesh_cache.insert(key, h.clone());
                h
            })
        }
        "Cylinder" => {
            let p = ugeom::read_cylinder(stage, proto_path).ok().flatten()?;
            let key = format!(
                "Cylinder:r={:.6}:h={:.6}:axis={:?}",
                p.radius, p.height, p.axis
            );
            ctx.mesh_cache.get(&key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(key.clone(), mesh_cylinder(p));
                ctx.mesh_cache.insert(key, h.clone());
                h
            })
        }
        "Capsule" => {
            let p = ugeom::read_capsule(stage, proto_path).ok().flatten()?;
            let key = format!(
                "Capsule:r={:.6}:h={:.6}:axis={:?}",
                p.radius, p.height, p.axis
            );
            ctx.mesh_cache.get(&key).cloned().unwrap_or_else(|| {
                let h = ctx.add_mesh_labeled(key.clone(), mesh_capsule(p));
                ctx.mesh_cache.insert(key, h.clone());
                h
            })
        }
        _ => return None,
    };
    let mat = resolve_material(stage, proto_path, ctx);
    Some((mesh, mat))
}

/// `true` when the stage authors `upAxis = "Z"`. Used to detect when the
/// scene-wide -90° X rotation in `root_basis_transform` is in play.
fn stage_is_z_up(stage: &Stage) -> bool {
    matches!(
        stage
            .field::<String>(Path::abs_root(), "upAxis")
            .ok()
            .flatten()
            .as_deref(),
        Some("Z")
    )
}

/// Heuristic: `true` when a mesh's authored points have a Y-dominant
/// bounding box — i.e. the mesh's tallest axis is Y. Such a mesh was
/// almost certainly exported Y-up (Maya/Blender/glTF default). When the
/// stage itself claims Z-up, this is a mismatch that the renderer has to
/// fix by counter-rotating the prototype before instancing.
fn mesh_is_y_up(read: &ugeom::ReadMesh) -> bool {
    if read.points.is_empty() {
        return false;
    }
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    for p in &read.points {
        for i in 0..3 {
            if p[i] < mn[i] { mn[i] = p[i]; }
            if p[i] > mx[i] { mx[i] = p[i]; }
        }
    }
    let dy = mx[1] - mn[1];
    let dz = mx[2] - mn[2];
    // Y must be strictly larger than Z by a small margin so we don't
    // misclassify nearly-cube props.
    dy > dz * 1.05
}

/// Depth-first scan under `root` for the first prim whose `typeName`
/// names a renderable schema. Returns the Path of that prim, or `None`
/// if the subtree contains no geometry. Used by the PointInstancer
/// pathway to resolve `prototypes` rels that point at Xform wrappers
/// rather than direct Mesh prims.
fn first_renderable_descendant(stage: &Stage, root: &Path) -> Option<Path> {
    for child_name in stage.prim_children(root.clone()).unwrap_or_default() {
        let Ok(child_path) = root.append_path(child_name.as_str()) else {
            continue;
        };
        let tn: String = stage
            .field::<String>(child_path.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if matches!(
            tn.as_str(),
            "Mesh" | "Cube" | "Sphere" | "Cylinder" | "Capsule" | "Plane"
        ) {
            return Some(child_path);
        }
        if let Some(p) = first_renderable_descendant(stage, &child_path) {
            return Some(p);
        }
    }
    None
}

/// Pick the right material for a geom prim: follow `material:binding` if
/// authored, otherwise fall back to the shared default. Used for
/// non-Mesh primitives (PointInstancer prototypes) that don't expose
/// a `doubleSided` flag — treats them as single-sided.
fn resolve_material(
    stage: &Stage,
    prim: &Path,
    ctx: &mut BuildCtx<'_, '_>,
) -> bevy::asset::Handle<StandardMaterial> {
    match ushade::read_material_binding(stage, prim).ok().flatten() {
        Some(mat_prim) => ctx.material_for(stage, &mat_prim, false),
        None => ctx.default_material(),
    }
}

/// Same as `resolve_material` but when the prim has no authored
/// `material:binding` *and* it carries `primvars:displayColor`, swap
/// the default gray base for a white PBR base so the authored vertex
/// colours come through untinted. When `double_sided`, the chosen
/// material gets cloned with `double_sided = true` + `cull_mode =
/// None` so front AND back faces render.
fn resolve_material_with_display_color(
    stage: &Stage,
    prim: &Path,
    ctx: &mut BuildCtx<'_, '_>,
    has_display_color: bool,
    double_sided: bool,
) -> bevy::asset::Handle<StandardMaterial> {
    match ushade::read_material_binding(stage, prim).ok().flatten() {
        Some(mat_prim) => ctx.material_for(stage, &mat_prim, double_sided),
        None if has_display_color => ctx.vertex_color_modulated_material_ds(double_sided),
        // No material binding, no displayColor → neutral gray.
        //
        // Earlier we used a path-hashed colour to give Kitchen_set's
        // unbound props some visual variety. The greenhouse / Isaac
        // case shows the cost: those scenes reference one
        // `pole.usdc` from many `Pole_NN_NN` xforms, and hashing the
        // composed instance path makes every instance a different
        // colour — the "RGB pillars" look. Kitchen_set's placeholder
        // story goes through `material_for`'s hash anyway (those
        // prims DO have bindings, just to empty Material prims), so
        // a uniform fallback here is fine.
        None => ctx.default_material_ds(double_sided),
    }
}

/// True when the authored `primvars:displayColor` carries at least
/// one value that's meaningfully different from white. Pixar-style
/// assets often author a single `(1, 1, 1)` or `(1, 1, 1, 1)`
/// placeholder value on every Mesh as part of their export pipeline,
/// and routing those through the vertex-colour-modulated material
/// produces the uniform-white look the user sees on Kitchen_set.
/// Returning `false` here lets `resolve_material_with_display_color`
/// fall through to the path-hashed fallback for variety.
fn display_color_is_useful(cs: &[[f32; 3]]) -> bool {
    if cs.is_empty() {
        return false;
    }
    // Two heuristics:
    //   1. Any value far from white → real shading info.
    //   2. Variation between values (e.g. per-vertex gradient) →
    //      real shading info even if the average is near-white.
    let near_white = |c: &[f32; 3]| (c[0] - 1.0).abs() < 0.02 && (c[1] - 1.0).abs() < 0.02 && (c[2] - 1.0).abs() < 0.02;
    let any_non_white = cs.iter().any(|c| !near_white(c));
    if any_non_white {
        return true;
    }
    // Variation check (shouldn't trigger if any_non_white is false,
    // but kept for robustness if "near-white" tolerance grows).
    let first = cs[0];
    cs.iter().any(|c| {
        (c[0] - first[0]).abs() > 0.02
            || (c[1] - first[1]).abs() > 0.02
            || (c[2] - first[2]).abs() > 0.02
    })
}

/// FNV-1a hash of a prim path, mapped through the same HSV recipe
/// `path_hashed_material_ds` uses. Standalone helper so callers
/// outside `BuildCtx` (the Material-binding fallback) get the same
/// colour-derivation as the no-binding fallback.
fn hash_path_to_rgb(path: &str) -> (f32, f32, f32) {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in path.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let hue = (h & 0xFFF) as f32 / 4096.0;
    // S=0.62 / V=0.62 — reasonably saturated mid-tones that survive
    // bright direct-light + PBR tonemapping without desaturating to
    // white. Tuned against Pixar Kitchen_set: at S=0.45/V=0.78 the
    // colours read as "white with a hint of tint" under any sun
    // brighter than overcast; this recipe keeps the hue legible.
    hsv_to_rgb(hue, 0.62, 0.62)
}

/// Convert an HSV colour `(h, s, v)` (each in `0..=1`) into linear
/// sRGB. Plain HSL/HSV-cone evaluation; not the perceptually-uniform
/// HCL variant. We use it only for synthesising distinct hash-derived
/// material colours, so the simpler curve is fine.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.fract().rem_euclid(1.0) * 6.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r1 + m, g1 + m, b1 + m)
}

/// Decode `xformOp*` on `prim` into a Bevy `Transform`. Falls back to
/// identity when nothing is authored.
fn read_prim_transform(stage: &Stage, path: &Path) -> Transform {
    let Some(t) = uxf::read_transform(stage, path).ok().flatten() else {
        return Transform::IDENTITY;
    };
    Transform {
        translation: Vec3::from(t.translate),
        rotation: Quat::from_xyzw(t.rotate[0], t.rotate[1], t.rotate[2], t.rotate[3]),
        scale: Vec3::from(t.scale),
    }
}

/// Read `upAxis` + `metersPerUnit` from the pseudo-root and return the
/// transform that takes USD-native coordinates into Bevy (Y-up, metres).
fn root_basis_transform(stage: &Stage) -> Transform {
    let up_axis = stage
        .field::<String>(Path::abs_root(), "upAxis")
        .ok()
        .flatten();

    // Real-world `.usda` files author `metersPerUnit = 1` as an integer; USD
    // doesn't insist on the trailing `.0`. Accept any numeric variant
    // through a raw `Value` query instead of the strict `f64` TryFrom.
    //
    // Default per the OpenUSD spec when unauthored: **0.01** (i.e.
    // centimetres are the stage's linear unit). Pixar's reference
    // Kitchen_set and many production exports rely on this default —
    // reading it as 1.0 makes a 5 m kitchen render as 500 m and the
    // camera frames a "scattered" wasteland of distant props.
    let authored_mpu = stage
        .field::<openusd::sdf::Value>(Path::abs_root(), "metersPerUnit")
        .ok()
        .flatten()
        .and_then(|v| match v {
            openusd::sdf::Value::Double(d) => Some(d as f32),
            openusd::sdf::Value::Float(f) => Some(f),
            openusd::sdf::Value::Int(i) => Some(i as f32),
            openusd::sdf::Value::Int64(i) => Some(i as f32),
            _ => None,
        });
    // `BEVY_OPENUSD_METERS_PER_UNIT` overrides both the authored value
    // and the spec default. Real-world assets are inconsistent: Pixar's
    // Kitchen_set assumes the spec-default 0.01 without authoring it,
    // while PointInstancedMedCity authors no `metersPerUnit` but ships
    // positions in meters. The env var lets the user pick per-load
    // without us having to guess.
    let meters_per_unit = std::env::var("BEVY_OPENUSD_METERS_PER_UNIT")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .or(authored_mpu)
        .unwrap_or(0.01);

    let rotation = match up_axis.as_deref() {
        Some("Z") => Quat::from_rotation_x(-core::f32::consts::FRAC_PI_2),
        _ => Quat::IDENTITY,
    };

    if rotation == Quat::IDENTITY && (meters_per_unit - 1.0).abs() < f32::EPSILON {
        Transform::IDENTITY
    } else {
        Transform {
            rotation,
            scale: Vec3::splat(meters_per_unit),
            ..Default::default()
        }
    }
}
