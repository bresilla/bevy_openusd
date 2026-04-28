//! `UsdAsset` + `UsdLoader`.
//!
//! The loader parses the stage, walks it once via [`build::stage_to_scene`],
//! and publishes the projected scene as a labeled sub-asset named `"Scene"`.
//! `UsdAsset` holds a strong [`Handle<Scene>`] that users spawn via
//! `SceneRoot(asset.scene.clone())`.
//!
//! openusd only accepts a filesystem path, so bytes from Bevy's `Reader` are
//! spilled to a tempfile before opening. For `.usdz` the loader additionally
//! cracks open the archive (it's a zero-compression ZIP) to surface every
//! non-layer entry (textures, aux files) to the stage walker as an
//! in-memory map keyed by its archive-relative path.

use std::collections::HashMap;
use std::io;
use std::io::Read;
use std::path::{Path, PathBuf};

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, Handle, LoadContext};
use bevy::reflect::TypePath;
use bevy::scene::Scene;
use serde::{Deserialize, Serialize};

use crate::build;

/// ZIP local-file header magic. USDZ is a zero-compression ZIP, so the first
/// four bytes are always `PK\x03\x04`.
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

/// A composed USD stage loaded as a Bevy asset.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct UsdAsset {
    /// Projected scene: one entity per prim, with `Name`, `Transform`, and
    /// `UsdPrimRef`. Basis / unit correction lives on the scene root.
    pub scene: Handle<Scene>,
    /// `defaultPrim` metadata on the root layer, if authored.
    pub default_prim: Option<String>,
    /// Number of layers composed into this stage.
    pub layer_count: usize,
    /// Every prim that authors a `variantSet` + the current selection for
    /// each set. Keyed by prim path. M6 exposes this for UI surfacing;
    /// variant switching (re-opening with a session layer) lands in M6.1.
    pub variants: HashMap<String, Vec<VariantSet>>,
    /// How many UsdLux lights got translated, broken down by Bevy light
    /// type. Populated during `UsdLoader::load`.
    pub light_tally: LightTally,
    /// Every `UsdGeom.Camera` prim in the stage. Prim-path → decoded
    /// camera params. The viewer surfaces this as a dropdown; picking
    /// one mounts its transform + intrinsics onto the active `Camera3d`.
    pub cameras: Vec<StageCamera>,
    /// Raw `UsdGeom.BasisCurves` / `UsdGeom.Points` data keyed by prim
    /// path. The initial mesh handles are baked into the Scene at load
    /// time, but tuning sliders rebuild the mesh bytes in-place (no
    /// asset reload) using this source-of-truth copy.
    pub curves: HashMap<String, usd_schemas::geom::ReadCurves>,
    pub points_clouds: HashMap<String, usd_schemas::geom::ReadPoints>,
    /// How many prims author `instanceable = true`. Surfaced on the
    /// Info panel as a sanity check — and used by the prototype cache
    /// to know when dedup opportunities exist.
    pub instance_prim_count: usize,
    /// How many instance sites the loader dedupped against a previously
    /// built prototype (`0` when no instance site was seen more than
    /// once). `instance_prim_count - instance_prototype_reuses` gives
    /// the number of unique prototypes materialized.
    pub instance_prototype_reuses: usize,
    /// Prim path → animated xform ops. Populated at load time from any
    /// `xformOp:*.timeSamples` on the stage; the viewer's animation
    /// clock reads this + the current time each frame and writes the
    /// resulting `Transform`.
    pub animated_prims: HashMap<String, usd_schemas::anim::AnimatedPrim>,
    /// Stage-level `startTimeCode` / `endTimeCode` (defaults 0..1 when
    /// absent) and `timeCodesPerSecond`/`framesPerSecond` (defaults 24).
    /// The viewer plays `seconds * timeCodesPerSecond` through this
    /// range.
    pub start_time_code: f64,
    pub end_time_code: f64,
    pub time_codes_per_second: f64,
    /// UsdSkel `Skeleton` prims discovered on the stage (M16 read side).
    pub skeletons: Vec<usd_schemas::skel::ReadSkeleton>,
    /// UsdSkel `SkelRoot` container prims with their skeleton /
    /// animationSource relationships.
    pub skel_roots: Vec<usd_schemas::skel::ReadSkelRoot>,
    /// Per-mesh `SkelBindingAPI` bindings (joint indices + weights).
    pub skel_bindings: Vec<usd_schemas::skel::ReadSkelBinding>,
    /// Sidecar-parsed `UsdSkelAnimation` prims keyed by their authored
    /// prim name (e.g. `"SkelAnim"`). Populated at load time when
    /// `UsdLoaderSettings::skel_animation_files` is non-empty (or the
    /// `BEVY_OPENUSD_SKEL_ANIM_FILE` env var is set). Lets us play
    /// SkelAnimation prims authored in `.usda` files that
    /// `openusd-rs` can't parse today (tuple-valued timeSamples).
    pub skel_animations: HashMap<String, usd_schemas::skel_anim_text::ReadSkelAnimText>,
    /// `UsdRender.RenderSettings` prims (M19 read side).
    pub render_settings: Vec<usd_schemas::render::ReadRenderSettings>,
    pub render_products: Vec<usd_schemas::render::ReadRenderProduct>,
    pub render_vars: Vec<usd_schemas::render::ReadRenderVar>,
    /// `UsdPhysics` prim paths that author a `PhysicsRigidBodyAPI`
    /// (M_LAST read side). Paired reader side of the existing authoring
    /// helpers — the plugin doesn't simulate, just surfaces.
    pub rigid_body_prims: Vec<String>,
    /// `PhysicsScene` prim paths.
    pub physics_scene_prims: Vec<String>,
    /// Decoded `Physics*Joint` prims. Authored frames + limits already
    /// resolved — downstream physics backends can consume directly.
    pub joints: Vec<usd_schemas::physics::ReadJoint>,
    /// Authored `custom` attributes + `customData` + `assetInfo` per
    /// prim (M24). Keyed by prim path; prims with NO user-authored
    /// metadata stay out of the map entirely.
    pub custom_attrs: HashMap<String, crate::prim_ref::UsdCustomAttrs>,
    /// Layer-level `customLayerData` dictionary. Omniverse stashes
    /// camera bookmarks, authoring-layer state, render settings
    /// defaults, etc. here. Empty when the root layer didn't author
    /// one.
    pub custom_layer_data: usd_schemas::geom::CustomDict,
    /// Prim paths whose `UsdGeomMesh.subdivisionScheme` is anything
    /// other than `none` (M25). Surfaces the author's intent so
    /// downstream tools know which meshes are *meant* to be
    /// tessellated — the plugin renders them flat as-authored.
    pub subdivision_prims: Vec<(String, usd_schemas::geom::SubdivScheme)>,
    /// UsdLux lights that authored any of `light:link`, `shadow:link`,
    /// or `light:filters` relationships (M26). Bevy's render pipeline
    /// doesn't yet honour the linking — this list just surfaces what
    /// was authored so downstream tools / our future render hook can
    /// act on it.
    pub light_linking_prims: Vec<String>,
    /// `UsdClipsAPI` metadata per prim (M27). Each entry captures the
    /// decoded clip sets authored on that prim. openusd doesn't
    /// compose clip layers yet; this surfaces the authoring so
    /// downstream tools can honour it manually.
    pub clip_sets: std::collections::HashMap<String, Vec<usd_schemas::clips::ReadClipSet>>,
}

/// One authored `UsdGeom.Camera` with its prim path + decoded params.
#[derive(Debug, Clone, PartialEq)]
pub struct StageCamera {
    pub path: String,
    pub data: usd_schemas::camera::ReadCamera,
}

/// Counts of UsdLux lights translated into Bevy lights during load.
/// Surfaced on the viewer's Info panel.
#[derive(Debug, Clone, Copy, Default)]
pub struct LightTally {
    pub directional: usize,
    pub point: usize,
    pub spot: usize,
    pub dome: usize,
}

impl From<crate::light::Tally> for LightTally {
    fn from(t: crate::light::Tally) -> Self {
        Self {
            directional: t.directional,
            point: t.point,
            spot: t.spot,
            dome: t.dome,
        }
    }
}

/// One variant set authored on a prim. `selection` is the currently-active
/// opinion (or `None` if only the `variantSetNames` metadata was declared
/// without a default selection). `options` enumerates every variant name
/// declared inside the set so the UI can surface a dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantSet {
    pub name: String,
    pub selection: Option<String>,
    pub options: Vec<String>,
}

/// Per-asset loader settings. Populated via
/// `asset_server.load_with_settings::<UsdAsset, _>(path, |s| {...})`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UsdLoaderSettings {
    /// Filesystem directories openusd searches when resolving relative
    /// asset paths inside the stage (references, payloads, sublayers,
    /// texture `inputs:file`, …).
    ///
    /// The loader always adds the tempfile's own parent directory first,
    /// but that's usually `/tmp`. For any stage with sibling references
    /// (`./materials/foo.usd`, `./greenhouse/front.usdc`, …) callers
    /// should pass the original asset's filesystem parent here.
    pub search_paths: Vec<PathBuf>,

    /// When `false`, payload arcs that fail to resolve are dropped silently
    /// (the prim keeps any local opinions but nothing gets pulled in).
    /// When `true`, unresolved payloads surface as a warning. Has no effect
    /// on payloads that *do* resolve — those always load.
    ///
    /// Use this to open huge staged scenes without their heavy lazy
    /// subtrees. `true` by default so out-of-the-box behaviour matches
    /// Pixar's USD.
    pub load_payloads: bool,

    /// When `true`, subtrees under prims tagged `kind = "component"` or
    /// `"subcomponent"` collapse their intermediate Xforms: every geom
    /// descendant becomes a direct child of the Kind-tagged prim, carrying
    /// the pre-composed world transform. Entity count drops by the tree
    /// depth without touching the mesh / material handles.
    ///
    /// `false` by default. Turning it on pays off on 10k+ prim Omniverse
    /// scenes where most of the depth is organizational rather than
    /// meaningful (`Robot/Chassis/Visual/MeshLink` → just `Robot/<mesh>`).
    pub kind_collapse: bool,

    /// Multiplier applied on top of the UsdLux intensity conversion.
    /// Authored `inputs:intensity` × `2^exposure` × `light_intensity_scale`
    /// feeds into the Bevy light's `intensity`. 1.0 is the identity; drop
    /// to 0.1 or raise to 10 if the scene's authored lights look wildly
    /// off (unit conventions vary wildly between DCCs).
    pub light_intensity_scale: f32,

    /// Radius (metres) used for `UsdGeom.BasisCurves` when the prim
    /// doesn't author `widths`. Ignored when `widths` is set. Smaller
    /// is crisper; larger reads better at distance.
    pub curve_default_radius: f32,

    /// How many tube-ring samples per spine vertex. 3 = triangular
    /// prism (cheapest, facetted), 6 = decent, 12 = smooth. Trivially
    /// scales mesh size: `vertices_per_curve = spine_len × ring_segments`.
    pub curve_ring_segments: u32,

    /// Multiplier applied to every `UsdGeom.Points` cube's half-extent.
    /// 1.0 = use authored `widths` as-is; 0.25 = quarter size; etc. Lets
    /// you dial a too-chunky point cloud down without touching USD.
    pub point_scale: f32,

    /// Per-prim variant overrides. Authored into a temporary session
    /// layer prepended to the stack before composition, so selections
    /// dominate any opinions the asset itself carries. Empty vec =
    /// no override = honour the stage's authored selections.
    pub variant_selections: Vec<VariantSelection>,

    /// Sidecar text-mode parser inputs: extra `.usda` files we scan
    /// for `UsdSkelAnimation` prims. Each scanned animation gets
    /// stashed on `UsdAsset.skel_animations` keyed by prim name.
    /// Workaround for `openusd-rs`'s USDA parser failing on
    /// tuple-valued timeSamples (`Unsupported property metadata
    /// value token: Punctuation('(')`) — Pixar's
    /// `HumanFemale.walk.usd` is the canonical case. Empty vec =
    /// only the env-var override `BEVY_OPENUSD_SKEL_ANIM_FILE` (if
    /// set) takes effect. Files are read but never composed into the
    /// stage.
    pub skel_animation_files: Vec<PathBuf>,
}

/// One authored override: set `prim_path`'s `set_name` variant to
/// `option`. Collections of these come from the Variants panel.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct VariantSelection {
    pub prim_path: String,
    pub set_name: String,
    pub option: String,
}

impl Default for UsdLoaderSettings {
    fn default() -> Self {
        Self {
            search_paths: Vec::new(),
            load_payloads: true,
            kind_collapse: false,
            light_intensity_scale: 1.0,
            curve_default_radius: 0.02,
            curve_ring_segments: 6,
            point_scale: 1.0,
            variant_selections: Vec::new(),
            skel_animation_files: Vec::new(),
        }
    }
}

/// Errors produced by [`UsdLoader`].
#[derive(thiserror::Error, Debug)]
pub enum UsdLoaderError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("failed to open USD stage: {0}")]
    Stage(String),
    #[error("failed to read USDZ archive: {0}")]
    Usdz(String),
}

/// Bevy `AssetLoader` for `.usda` / `.usdc` / `.usd` / `.usdz` files.
#[derive(Default, TypePath)]
pub struct UsdLoader;

impl AssetLoader for UsdLoader {
    type Asset = UsdAsset;
    type Settings = UsdLoaderSettings;
    type Error = UsdLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &UsdLoaderSettings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<UsdAsset, UsdLoaderError> {
        bevy::log::info!(
            "UsdLoader::load fired for {:?} (curve_radius={:.4}, curve_rings={}, point_scale={:.2})",
            load_context.path(),
            settings.curve_default_radius,
            settings.curve_ring_segments,
            settings.point_scale,
        );
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let asset_path = load_context.path();
        let fs_path: &Path = asset_path.path();
        let ext_hint = fs_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("usd");

        let is_usdz = bytes.starts_with(ZIP_MAGIC) || ext_hint.eq_ignore_ascii_case("usdz");
        let is_usda = !is_usdz
            && (ext_hint.eq_ignore_ascii_case("usda")
                || (ext_hint.eq_ignore_ascii_case("usd") && is_text_usd(&bytes)));

        // For non-USDZ inputs, write the tempfile INTO the first search
        // path (the user's asset root) rather than `/tmp`. openusd's
        // `DefaultResolver::create_identifier` anchors relative references
        // against the parent layer's *directory*; if we land the root
        // layer in `/tmp`, every `./foo.usdc` sibling reference resolves
        // into `/tmp/foo.usdc` regardless of `search_paths`. Writing the
        // tempfile into the actual source dir keeps sibling lookups sane.
        let tmp_dir = if !is_usdz && let Some(first) = settings.search_paths.first() {
            first.clone()
        } else {
            std::env::temp_dir()
        };

        let (tmp, embedded) = if is_usdz {
            extract_usdz(&bytes, fs_path)?
        } else {
            let tmp = tempfile_in(&tmp_dir, fs_path, ext_hint);
            let final_bytes = if is_usda {
                usd_schemas::third_party::strip_metadata::strip_unsupported_prim_metadata(&bytes)
            } else {
                bytes.clone()
            };
            std::fs::write(&tmp, &final_bytes)?;
            (tmp, HashMap::new())
        };

        let tmp_str = tmp
            .to_str()
            .ok_or_else(|| UsdLoaderError::Stage("non-UTF-8 tempfile path".into()))?;
        // Build a resolver that searches the user-supplied dirs first, then
        // the tempfile's parent (so intra-USDZ layers stay resolvable). The
        // DefaultResolver also falls back to `std::env::current_dir()`, but
        // relying on that would make loads CWD-sensitive and flaky.
        let mut search: Vec<PathBuf> = settings.search_paths.clone();
        if let Some(parent) = tmp.parent().map(|p| p.to_path_buf()) {
            search.push(parent);
        }

        // Session layer: if the caller authored variant overrides, write
        // them out as a tiny USDA file with `over` specs carrying
        // `variants = { ... }` metadata, then hand the path to the
        // StageBuilder. Composition puts session opinions ahead of
        // everything else, so the override wins over whatever the stage
        // authored itself.
        let session_layer_path = if !settings.variant_selections.is_empty() {
            let text = author_variant_session_layer(&settings.variant_selections);
            let session_tmp =
                tempfile_session(&tmp_dir, fs_path, &settings.variant_selections, &text);
            std::fs::write(&session_tmp, &text)?;
            bevy::log::info!(
                "usd: wrote {} variant selection(s) to session layer {}",
                settings.variant_selections.len(),
                session_tmp.display()
            );
            Some(session_tmp)
        } else {
            None
        };

        let skip_payloads = !settings.load_payloads;
        let mut builder = openusd::Stage::builder()
            .resolver(
                usd_schemas::third_party::resolver::StripMetadataResolver::with_search_paths(
                    search.clone(),
                ),
            )
            .on_error(move |err| {
                // Demote "unresolved payload" to a silent skip when the
                // caller opted out of payload loading. Everything else
                // keeps the default warn-and-continue behaviour so genuine
                // composition issues stay visible.
                if skip_payloads
                    && let openusd::CompositionError::Layer(
                        openusd::layer::Error::UnresolvedAsset { kind, .. },
                    ) = &err
                    && matches!(kind, openusd::DependencyKind::Payload)
                {
                    return Ok(());
                }
                bevy::log::warn!("usd composition: {err}");
                Ok(())
            });
        if let Some(ref p) = session_layer_path {
            let s = p.to_str().ok_or_else(|| {
                UsdLoaderError::Stage("non-UTF-8 session-layer path".into())
            })?;
            builder = builder.session_layer(s.to_string());
        }
        let stage = builder
            .open(tmp_str)
            .map_err(|e| {
                // Walk the anyhow chain so the real parser error (which
                // layer-open wraps twice) surfaces to the user.
                let mut msg = e.to_string();
                let mut src: Option<&dyn std::error::Error> = e.source();
                while let Some(s) = src {
                    msg.push_str(&format!(" :: {s}"));
                    src = s.source();
                }
                UsdLoaderError::Stage(msg)
            })?;

        let default_prim = stage.default_prim();
        let layer_count = stage.layer_count();
        let variants = collect_variants(&stage);
        let cameras = collect_cameras(&stage);
        let (curves, points_clouds) = collect_curves_and_points(&stage);
        let animated_prims = collect_animated_prims(&stage);
        let (mut start_time_code, mut end_time_code, time_codes_per_second) =
            read_stage_timeline(&stage);
        let (skeletons, skel_roots, skel_bindings) = collect_skel(&stage);
        let (render_settings, render_products, render_vars) = collect_render(&stage);
        let (rigid_body_prims, physics_scene_prims, joints) = collect_physics(&stage);
        let custom_attrs = collect_custom_attrs(&stage);
        let custom_layer_data = usd_schemas::geom::read_custom_layer_data(&stage)
            .ok()
            .flatten()
            .unwrap_or_default();
        let subdivision_prims = collect_subdivision_prims(&stage);
        let light_linking_prims = collect_light_linking_prims(&stage);
        let clip_sets = collect_clip_sets(&stage);

        // Sidecar: scan extra .usda files for UsdSkelAnimation prims.
        // Workaround for openusd-rs USDA parser failing on
        // tuple-valued timeSamples (e.g. Pixar's HumanFemale.walk.usd).
        // Files come from `UsdLoaderSettings::skel_animation_files`
        // and the `BEVY_OPENUSD_SKEL_ANIM_FILE` env var (one path or
        // colon-separated list).
        let mut skel_animations: HashMap<
            String,
            usd_schemas::skel_anim_text::ReadSkelAnimText,
        > = HashMap::new();
        let mut anim_paths: Vec<PathBuf> = settings.skel_animation_files.clone();
        if let Ok(envv) = std::env::var("BEVY_OPENUSD_SKEL_ANIM_FILE") {
            for piece in envv.split(':') {
                if !piece.is_empty() {
                    anim_paths.push(PathBuf::from(piece));
                }
            }
        }
        // For USDZ stages, auto-scan every extracted USDA/USD layer
        // for SkelAnimation prims. Pixar's HumanFemale.walk.usd is the
        // canonical case: openusd-rs's parser rejects its tuple-valued
        // timeSamples, so the file CAN'T contribute via the regular
        // composition path — but our sidecar text scanner handles it
        // fine. Without this, animation gets dropped silently when
        // packing into a USDZ (the loose-file viewer relied on the
        // user setting BEVY_OPENUSD_SKEL_ANIM_FILE manually).
        if is_usdz {
            if let Some(parent) = tmp.parent() {
                collect_text_layers_recursive(parent, &mut anim_paths);
            }
        }
        let stage_authored_timeline = end_time_code > start_time_code;
        let mut anim_min_time: Option<f64> = None;
        let mut anim_max_time: Option<f64> = None;
        let mut record_time = |t: f64, mn: &mut Option<f64>, mx: &mut Option<f64>| {
            *mn = Some(mn.map(|m| m.min(t)).unwrap_or(t));
            *mx = Some(mx.map(|m| m.max(t)).unwrap_or(t));
        };
        for p in anim_paths {
            // Resolve relative paths against the search paths.
            let candidates: Vec<PathBuf> = if p.is_absolute() {
                vec![p.clone()]
            } else {
                let mut v = vec![p.clone()];
                for sp in &search {
                    v.push(sp.join(&p));
                }
                v
            };
            let resolved = candidates.into_iter().find(|c| c.exists());
            let Some(path) = resolved else {
                bevy::log::warn!(
                    "skel anim sidecar: file not found: {}",
                    p.display()
                );
                continue;
            };
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let anims = usd_schemas::skel_anim_text::scan_skel_animations(&text);
                    bevy::log::info!(
                        "skel anim sidecar: parsed {} animation(s) from {}",
                        anims.len(),
                        path.display()
                    );
                    for a in anims {
                        for k in a.translations.keys() {
                            record_time(k.0, &mut anim_min_time, &mut anim_max_time);
                        }
                        for k in a.rotations.keys() {
                            record_time(k.0, &mut anim_min_time, &mut anim_max_time);
                        }
                        for k in a.scales.keys() {
                            record_time(k.0, &mut anim_min_time, &mut anim_max_time);
                        }
                        for k in a.blend_shape_weights.keys() {
                            record_time(k.0, &mut anim_min_time, &mut anim_max_time);
                        }
                        skel_animations.insert(a.prim_name.clone(), a);
                    }
                }
                Err(e) => {
                    bevy::log::warn!(
                        "skel anim sidecar: failed to read {}: {e}",
                        path.display()
                    );
                }
            }
        }
        // When the stage authored no timeline, derive playback range
        // from the sidecar's authored keyframes — exactly matches
        // walk.usd's frame 101..129 for HumanFemale, so the playhead
        // doesn't sit at frame 0 (where the bracket holds the first
        // keyframe and animation appears stalled).
        if !stage_authored_timeline {
            if let (Some(mn), Some(mx)) = (anim_min_time, anim_max_time) {
                start_time_code = mn;
                end_time_code = mx;
            }
        }

        let (scene, light_tally, instance_stats) = build::stage_to_scene(
            &stage,
            load_context,
            &embedded,
            &search,
            settings.kind_collapse,
            settings.light_intensity_scale,
            settings.curve_default_radius,
            settings.curve_ring_segments,
            settings.point_scale,
            &skel_animations,
        );
        bevy::log::info!(
            "usd: translated {} directional + {} point + {} spot lights (+ {} dome deferred)",
            light_tally.directional,
            light_tally.point,
            light_tally.spot,
            light_tally.dome,
        );
        let scene_handle = load_context.add_labeled_asset("Scene".into(), scene);

        let _ = std::fs::remove_file(&tmp);

        Ok(UsdAsset {
            scene: scene_handle,
            default_prim,
            layer_count,
            variants,
            light_tally: light_tally.into(),
            cameras,
            curves,
            points_clouds,
            instance_prim_count: instance_stats.instance_prim_count,
            instance_prototype_reuses: instance_stats.prototype_reuses,
            animated_prims,
            start_time_code,
            end_time_code,
            time_codes_per_second,
            skeletons,
            skel_roots,
            skel_bindings,
            skel_animations,
            render_settings,
            render_products,
            render_vars,
            rigid_body_prims,
            physics_scene_prims,
            joints,
            custom_attrs,
            custom_layer_data,
            subdivision_prims,
            light_linking_prims,
            clip_sets,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["usda", "usdc", "usd", "usdz"]
    }
}

/// Decompose a USDZ archive. Writes the first USD layer to a tempfile (so
/// `openusd::Stage::open` can parse it the normal way) and returns a map of
/// `archive-relative path -> raw bytes` for every *non-layer* entry —
/// typically PNG / JPEG / KTX textures.
///
/// `openusd` already has its own `usdz::Archive` reader for walking layers,
/// but it doesn't surface the media payload. We go direct through the `zip`
/// crate so textures land under our control.
fn extract_usdz(
    bytes: &[u8],
    asset_path: &Path,
) -> Result<(PathBuf, HashMap<String, Vec<u8>>), UsdLoaderError> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| UsdLoaderError::Usdz(e.to_string()))?;

    // Spill every archive entry to a per-USDZ tempdir so openusd-rs can
    // resolve internal `references = @./other_layer.usd@` arcs the way
    // it would for an unzipped scene. Without this, multi-layer USDZs
    // (Kitchen_set, HumanFemale) compose only the root and the user
    // sees an almost-empty stage.
    //
    // The first `.usda`/`.usdc`/`.usd` entry is the package's root
    // layer per the USDZ spec — we return its tempfile path as the
    // stage opener's input.
    let mut layer_name: Option<String> = None;
    for i in 0..archive.len() {
        let f = archive
            .by_index(i)
            .map_err(|e| UsdLoaderError::Usdz(e.to_string()))?;
        let name = f.name().to_string();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".usda") || lower.ends_with(".usdc") || lower.ends_with(".usd") {
            layer_name = Some(name);
            break;
        }
    }
    let layer_name = layer_name
        .ok_or_else(|| UsdLoaderError::Usdz("USDZ archive contains no USD layer".to_string()))?;

    // Stable tempdir keyed off the source path so reloads overwrite
    // predictably (matches what `tempfile_for` does for plain .usd files).
    let extract_dir = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        asset_path.hash(&mut h);
        std::env::temp_dir().join(format!(".bevy_openusd_usdz_{:016x}", h.finish()))
    };
    let _ = std::fs::remove_dir_all(&extract_dir);
    std::fs::create_dir_all(&extract_dir)?;

    let mut embedded = HashMap::new();
    for i in 0..archive.len() {
        let mut f = archive
            .by_index(i)
            .map_err(|e| UsdLoaderError::Usdz(e.to_string()))?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let mut buf = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut buf)?;

        // Drop any path traversal ahead of joining onto our extract_dir.
        let safe_rel = sanitize_archive_name(&name);
        let dest = extract_dir.join(&safe_rel);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(&dest, &buf) {
            bevy::log::warn!(
                "usdz: failed to spill {name:?} to {}: {err}",
                dest.display()
            );
        }

        // Keep the raw bytes for non-layer entries so the texture
        // loader's USDZ-embedded fast-path still works (textures get
        // decoded once + cached as a labeled sub-asset).
        let lower = name.to_ascii_lowercase();
        let is_layer =
            lower.ends_with(".usda") || lower.ends_with(".usdc") || lower.ends_with(".usd");
        if !is_layer {
            embedded.insert(name, buf);
        }
    }

    let safe_root = sanitize_archive_name(&layer_name);
    let root_path = extract_dir.join(safe_root);
    if !root_path.is_file() {
        return Err(UsdLoaderError::Usdz(format!(
            "root layer {} missing after extract",
            layer_name
        )));
    }
    Ok((root_path, embedded))
}

/// Walk `dir` and append every plain-text USD layer (`.usda`, plus
/// `.usd` files whose first bytes are `#usda`) onto `out`. Used to
/// auto-feed extracted USDZ layers into the SkelAnimation sidecar
/// scan — text layers may carry tuple-valued timeSamples openusd-rs
/// rejects, but our sidecar text parser handles them.
fn collect_text_layers_recursive(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(path);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            if ext == "usda" {
                out.push(path);
                continue;
            }
            if ext == "usd" {
                // Sniff first bytes — `.usd` can be either USDA (text)
                // or USDC (binary). The sidecar text parser is text-only.
                if let Ok(mut f) = std::fs::File::open(&path) {
                    let mut head = [0u8; 8];
                    use std::io::Read;
                    let n = f.read(&mut head).unwrap_or(0);
                    if head[..n].starts_with(b"#usda") {
                        out.push(path);
                    }
                }
            }
        }
    }
}

/// Strip any leading `/` and reject `..` segments before joining the
/// archive entry name onto the on-disk extraction directory. Prevents
/// a malicious USDZ from writing outside the tempdir, and normalises
/// Windows-style separators to `/` (`zip` enforces forward slashes
/// for new entries but tolerates legacy archives with backslashes).
fn sanitize_archive_name(name: &str) -> PathBuf {
    let cleaned = name.replace('\\', "/");
    let mut out = PathBuf::new();
    for seg in cleaned.split('/') {
        match seg {
            "" | "." => continue,
            ".." => continue,
            other => out.push(other),
        }
    }
    out
}

/// `true` if `bytes` look like a text USDA file (starts with `#usda`,
/// ignoring leading BOM / whitespace). Used to decide whether to run the
/// metadata stripper on a `.usd` file with ambiguous extension.
fn is_text_usd(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0xEF | 0xBB | 0xBF))
        .unwrap_or(bytes.len());
    bytes[start..].starts_with(b"#usda")
}

/// Collect every `BasisCurves` + `Points` prim with its decoded data.
/// Used by the viewer's live-tuning system so sliding the radius /
/// ring-segments / point-scale rebuilds meshes in place — no reload.
fn collect_curves_and_points(
    stage: &openusd::Stage,
) -> (
    HashMap<String, usd_schemas::geom::ReadCurves>,
    HashMap<String, usd_schemas::geom::ReadPoints>,
) {
    use openusd::sdf::Path;
    let mut curves = HashMap::new();
    let mut points = HashMap::new();
    let _ = stage.traverse(|path: &Path| {
        let type_name: Option<String> = stage.field::<String>(path.clone(), "typeName").ok().flatten();
        match type_name.as_deref() {
            Some("BasisCurves") => {
                if let Ok(Some(read)) = usd_schemas::geom::read_curves(stage, path) {
                    curves.insert(path.as_str().to_string(), read);
                }
            }
            Some("Points") => {
                if let Ok(Some(read)) = usd_schemas::geom::read_points(stage, path) {
                    points.insert(path.as_str().to_string(), read);
                }
            }
            _ => {}
        }
    });
    (curves, points)
}

/// Scan every prim for `xformOp:*.timeSamples` and preconvert the
/// samples. Static prims (no authored time samples) stay out of the
/// map — the runtime cost is proportional to animated-prim count, not
/// total stage size.
fn collect_animated_prims(
    stage: &openusd::Stage,
) -> HashMap<String, usd_schemas::anim::AnimatedPrim> {
    use openusd::sdf::Path;
    let mut out = HashMap::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(Some(record)) = usd_schemas::anim::read_animated_prim(stage, path) {
            out.insert(path.as_str().to_string(), record);
        }
    });
    out
}

/// Walk the stage and collect every `Skeleton`, `SkelRoot`, and
/// mesh-with-`SkelBindingAPI` prim into three parallel vectors. The
/// readers return `None` for mismatched types so we rely on dispatch
/// order: try each reader on each prim, record what sticks.
fn collect_skel(
    stage: &openusd::Stage,
) -> (
    Vec<usd_schemas::skel::ReadSkeleton>,
    Vec<usd_schemas::skel::ReadSkelRoot>,
    Vec<usd_schemas::skel::ReadSkelBinding>,
) {
    use openusd::sdf::Path;
    let mut skeletons = Vec::new();
    let mut skel_roots = Vec::new();
    let mut skel_bindings = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(Some(s)) = usd_schemas::skel::read_skeleton(stage, path) {
            skeletons.push(s);
            return;
        }
        if let Ok(Some(r)) = usd_schemas::skel::read_skel_root(stage, path) {
            skel_roots.push(r);
            // `SkelRoot` subtrees can contain Meshes that also author
            // `SkelBindingAPI` — don't early-return, let the next
            // traversal step recurse through children.
        }
        if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(stage, path) {
            skel_bindings.push(b);
        }
    });
    (skeletons, skel_roots, skel_bindings)
}

/// Collect `UsdClipsAPI` sets authored on any prim. Empty entries
/// are filtered out so only prims that actually author clip
/// metadata show up in the map.
fn collect_clip_sets(
    stage: &openusd::Stage,
) -> std::collections::HashMap<String, Vec<usd_schemas::clips::ReadClipSet>> {
    use openusd::sdf::Path;
    let mut out = std::collections::HashMap::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(sets) = usd_schemas::clips::read_clips(stage, path) {
            if !sets.is_empty() {
                out.insert(path.as_str().to_string(), sets);
            }
        }
    });
    out
}

/// Collect every UsdLux light prim that authored at least one of the
/// linking relationships (`light:link`, `shadow:link`, or
/// `light:filters`). Surfaces the authoring intent so consumers can
/// decide how to honour it.
fn collect_light_linking_prims(stage: &openusd::Stage) -> Vec<String> {
    use openusd::sdf::Path;
    let mut out = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(Some(read)) = usd_schemas::lux::read_light(stage, path) {
            let common = match &read {
                usd_schemas::lux::ReadLight::Distant(d) => &d.common,
                usd_schemas::lux::ReadLight::Sphere(s) => &s.common,
                usd_schemas::lux::ReadLight::Rect(r) => &r.common,
                usd_schemas::lux::ReadLight::Disk(d) => &d.common,
                usd_schemas::lux::ReadLight::Cylinder(c) => &c.common,
                usd_schemas::lux::ReadLight::Dome(d) => &d.common,
            };
            if !common.light_link_targets.is_empty()
                || !common.shadow_link_targets.is_empty()
                || !common.light_filters.is_empty()
            {
                out.push(path.as_str().to_string());
            }
        }
    });
    out
}

/// Collect every `UsdGeomMesh` whose `subdivisionScheme` is not
/// `"none"`. Downstream consumers that run their own subdivision pass
/// (Bevy CPU tessellator, offline exporter) can query this list to
/// know which meshes to tesselate.
fn collect_subdivision_prims(
    stage: &openusd::Stage,
) -> Vec<(String, usd_schemas::geom::SubdivScheme)> {
    use openusd::sdf::Path;
    let mut out = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        let type_name: Option<String> = stage
            .field::<String>(path.clone(), "typeName")
            .ok()
            .flatten();
        if type_name.as_deref() != Some("Mesh") {
            return;
        }
        if let Ok(Some(read)) = usd_schemas::geom::read_mesh(stage, path) {
            if read.subdivision_scheme.is_subdivision() {
                out.push((path.as_str().to_string(), read.subdivision_scheme));
            }
        }
    });
    out
}

/// Scan every prim for user-authored metadata:
///   - `custom` attributes (including `userProperties:*` namespaces).
///   - `customData = { ... }` dictionary on the prim.
///   - `assetInfo = { ... }` dictionary on the prim.
/// A prim ends up in the output map only when at least ONE of those
/// three channels has content.
fn collect_custom_attrs(
    stage: &openusd::Stage,
) -> HashMap<String, crate::prim_ref::UsdCustomAttrs> {
    use openusd::sdf::Path;
    let mut out = HashMap::new();
    let _ = stage.traverse(|path: &Path| {
        let entries = usd_schemas::geom::read_custom_attrs(stage, path).unwrap_or_default();
        let custom_data = usd_schemas::geom::read_custom_data(stage, path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let asset_info = usd_schemas::geom::read_asset_info(stage, path)
            .ok()
            .flatten()
            .unwrap_or_default();
        let record = crate::prim_ref::UsdCustomAttrs {
            entries,
            custom_data,
            asset_info,
        };
        if !record.is_empty() {
            out.insert(path.as_str().to_string(), record);
        }
    });
    out
}

/// Walk the stage for UsdPhysics content. Collect every prim that
/// applies `PhysicsRigidBodyAPI`, every `PhysicsScene`, and every
/// recognised `Physics*Joint`. The plugin doesn't simulate — these
/// surfaces let downstream physics backends consume authored data
/// without rewalking the stage themselves.
fn collect_physics(
    stage: &openusd::Stage,
) -> (
    Vec<String>,
    Vec<String>,
    Vec<usd_schemas::physics::ReadJoint>,
) {
    use openusd::sdf::Path;
    let mut rigid = Vec::new();
    let mut scenes = Vec::new();
    let mut joints = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        if matches!(
            usd_schemas::physics::read_is_physics_scene(stage, path),
            Ok(true)
        ) {
            scenes.push(path.as_str().to_string());
            return;
        }
        if let Ok(Some(j)) = usd_schemas::physics::read_joint(stage, path) {
            joints.push(j);
            return;
        }
        if matches!(
            usd_schemas::physics::read_has_rigid_body(stage, path),
            Ok(true)
        ) {
            rigid.push(path.as_str().to_string());
        }
    });
    (rigid, scenes, joints)
}

/// Walk the stage and collect every `UsdRender.*` prim into three
/// parallel vectors. Readers return `None` on type mismatch so we try
/// each reader on each prim.
fn collect_render(
    stage: &openusd::Stage,
) -> (
    Vec<usd_schemas::render::ReadRenderSettings>,
    Vec<usd_schemas::render::ReadRenderProduct>,
    Vec<usd_schemas::render::ReadRenderVar>,
) {
    use openusd::sdf::Path;
    let mut settings = Vec::new();
    let mut products = Vec::new();
    let mut vars = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(Some(s)) = usd_schemas::render::read_render_settings(stage, path) {
            settings.push(s);
            return;
        }
        if let Ok(Some(p)) = usd_schemas::render::read_render_product(stage, path) {
            products.push(p);
            return;
        }
        if let Ok(Some(v)) = usd_schemas::render::read_render_var(stage, path) {
            vars.push(v);
        }
    });
    (settings, products, vars)
}

/// Stage-level timeline metadata. `startTimeCode` / `endTimeCode` default
/// to `0..1` (a single frame) when not authored, matching Pixar's USD.
/// `timeCodesPerSecond` defaults to 24 fps (or falls back to
/// `framesPerSecond` which some authoring tools use instead).
fn read_stage_timeline(stage: &openusd::Stage) -> (f64, f64, f64) {
    use openusd::sdf::{Path, Value};
    let read_f64 = |key: &str| -> Option<f64> {
        match stage.field::<Value>(Path::abs_root(), key).ok().flatten() {
            Some(Value::Double(d)) => Some(d),
            Some(Value::Float(f)) => Some(f as f64),
            Some(Value::Int(i)) => Some(i as f64),
            Some(Value::TimeCode(d)) => Some(d),
            _ => None,
        }
    };
    let start = read_f64("startTimeCode").unwrap_or(0.0);
    let end = read_f64("endTimeCode").unwrap_or(start.max(1.0));
    let tcps = read_f64("timeCodesPerSecond")
        .or_else(|| read_f64("framesPerSecond"))
        .unwrap_or(24.0)
        .max(1e-3);
    (start, end, tcps)
}

/// Walk the composed stage and collect every `UsdGeom.Camera` prim. The
/// viewer surfaces these as a mount-able dropdown.
fn collect_cameras(stage: &openusd::Stage) -> Vec<StageCamera> {
    use openusd::sdf::Path;
    let mut out = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        if let Ok(Some(read)) = usd_schemas::camera::read_camera(stage, path) {
            out.push(StageCamera {
                path: path.as_str().to_string(),
                data: read,
            });
        }
    });
    out
}

/// Walk the composed stage looking for prims that author `variantSetNames`
/// and collect the current selection per set. Exposed on `UsdAsset` for UI
/// surfacing; switching lands in M6.1 via a session layer.
fn collect_variants(stage: &openusd::Stage) -> HashMap<String, Vec<VariantSet>> {
    use openusd::sdf::{Path, Value};

    let mut out: HashMap<String, Vec<VariantSet>> = HashMap::new();

    let _ = stage.traverse(|path: &Path| {
        // `variantSetNames` (TokenListOp) holds the set names authored here.
        let names: Vec<String> = match stage
            .field::<Value>(path.clone(), "variantSetNames")
            .ok()
            .flatten()
        {
            Some(Value::TokenListOp(op)) => op.flatten(),
            Some(Value::TokenVec(v)) => v,
            _ => return,
        };
        if names.is_empty() {
            return;
        }

        // `variantSelection` is a HashMap<set_name, selection_value>.
        let selections = match stage
            .field::<Value>(path.clone(), "variantSelection")
            .ok()
            .flatten()
        {
            Some(Value::VariantSelectionMap(m)) => m,
            _ => Default::default(),
        };

        let sets: Vec<VariantSet> = names
            .into_iter()
            .map(|name| {
                let selection = selections.get(&name).cloned();
                // Enumerate this set's variant options. They're stored as
                // `variantChildren` (TokenVec) on the variant-set path
                // `/Prim{setName=}` (empty selection = the container).
                let set_path = path.append_variant_selection(&name, "");
                let options: Vec<String> = match stage
                    .field::<Value>(set_path, "variantChildren")
                    .ok()
                    .flatten()
                {
                    Some(Value::TokenVec(v)) => v,
                    _ => Vec::new(),
                };
                VariantSet {
                    name,
                    selection,
                    options,
                }
            })
            .collect();

        if !sets.is_empty() {
            out.insert(path.as_str().to_string(), sets);
        }
    });

    out
}

/// Build a unique tempfile path for an asset. The filename embeds a hash of
/// the asset path so concurrent loads don't collide and so hot-reload reuses
/// the same slot.
fn tempfile_for(asset_path: &Path, ext: &str) -> PathBuf {
    tempfile_in(&std::env::temp_dir(), asset_path, ext)
}

/// Tempfile slot for the per-load session layer. Hash covers the asset
/// path, every authored selection, and the emitted USDA text so two
/// concurrent loads with different variant choices get different
/// filenames and never clobber each other's session layer mid-compose.
fn tempfile_session(
    dir: &Path,
    asset_path: &Path,
    selections: &[VariantSelection],
    text: &str,
) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    asset_path.hash(&mut h);
    for sel in selections {
        sel.prim_path.hash(&mut h);
        sel.set_name.hash(&mut h);
        sel.option.hash(&mut h);
    }
    text.hash(&mut h);
    let mut out = dir.to_path_buf();
    out.push(format!(".bevy_openusd_session_{:016x}.usda", h.finish()));
    out
}

/// Trie node used by the session-layer emitter. Leafs carry variant
/// selections; inner nodes just nest into deeper `over` blocks.
#[derive(Default)]
struct OverNode<'a> {
    children: std::collections::BTreeMap<String, OverNode<'a>>,
    selections: Vec<&'a VariantSelection>,
}

/// Emit a minimal USDA session layer that authors `variants = { ... }`
/// metadata under one `over` spec per prim that received a selection.
/// Prim paths are broken into segments and emitted as nested `over`
/// blocks so the file is syntactically valid.
pub fn author_variant_session_layer(selections: &[VariantSelection]) -> String {
    use std::collections::BTreeMap;

    // Group selections by full prim path so multiple sets on the same
    // prim fold into one `variants = { … }` map.
    let mut by_prim: BTreeMap<&str, Vec<&VariantSelection>> = BTreeMap::new();
    for sel in selections {
        by_prim.entry(sel.prim_path.as_str()).or_default().push(sel);
    }

    let mut root: OverNode = OverNode::default();
    for (path, sels) in by_prim {
        let trimmed = path.trim_start_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let mut cur = &mut root;
        for segment in trimmed.split('/') {
            cur = cur.children.entry(segment.to_string()).or_default();
        }
        cur.selections = sels;
    }

    let mut out = String::new();
    out.push_str("#usda 1.0\n\n");
    for (name, child) in &root.children {
        emit_over(&mut out, name, child, 0);
    }
    out
}

fn emit_over(buf: &mut String, name: &str, node: &OverNode<'_>, depth: usize) {
    use std::fmt::Write;
    let pad = "    ".repeat(depth);
    if node.selections.is_empty() {
        let _ = writeln!(buf, "{pad}over \"{name}\"");
    } else {
        let _ = writeln!(buf, "{pad}over \"{name}\" (");
        let _ = writeln!(buf, "{pad}    variants = {{");
        for sel in &node.selections {
            let _ = writeln!(
                buf,
                "{pad}        string {} = \"{}\"",
                sel.set_name, sel.option
            );
        }
        let _ = writeln!(buf, "{pad}    }}");
        let _ = writeln!(buf, "{pad})");
    }
    let _ = writeln!(buf, "{pad}{{");
    for (child_name, child) in &node.children {
        emit_over(buf, child_name, child, depth + 1);
    }
    let _ = writeln!(buf, "{pad}}}");
}

/// Same as [`tempfile_for`] but the caller chooses the directory. Used to
/// drop non-USDZ tempfiles into the user's asset root so openusd's
/// reference anchoring finds sibling layers.
fn tempfile_in(dir: &Path, asset_path: &Path, ext: &str) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    asset_path.hash(&mut hasher);
    let hash = hasher.finish();

    let mut out = dir.to_path_buf();
    out.push(format!(".bevy_openusd_tmp_{hash:016x}.{ext}"));
    out
}
