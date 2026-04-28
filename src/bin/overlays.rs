//! Debug overlays: **world grid** + world axes + per-prim axis markers.
//!
//! The grid is a three-layer mesh (minor lines, major lines, dots) styled
//! like `../bevy_urdf/src/overlays.rs`. Unlike that URDF viewer we don't
//! pin a fixed 20 m tile — the grid's extent tracks the loaded USD scene
//! so a nav graph and a Kit robot both get a legible reference plane.
//!
//! **Auto-sizing.** On the first frame after the scene materializes we
//! rebuild the grid at `~4 × scene diagonal`, bucket the major/minor
//! spacing to nice decades (1 / 2 / 5 × 10ⁿ), and bake a radial
//! vertex-alpha fade into every vertex so the tiles outside the scene
//! dissolve to fully transparent — reading as if the grid extends
//! forever.
//!
//! **Layering.** Three separate entities (minor / major / dots) with
//! their own `StandardMaterial`s means we can change alpha, colour, or
//! visibility per layer without rebuilding the mesh.

use bevy::asset::{Assets, RenderAssetUsages};
use bevy::color::{Color, LinearRgba};
use bevy::mesh::{Indices, Mesh, Mesh3d, PrimitiveTopology};
use bevy::pbr::{MeshMaterial3d, StandardMaterial};
use bevy::prelude::*;
use bevy::render::alpha::AlphaMode;
use bevy_openusd::UsdPrimRef;

pub struct OverlaysPlugin;

impl Plugin for OverlaysPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DisplayToggles>()
            .init_resource::<SceneExtent>()
            .init_resource::<WorldGridState>()
            .add_systems(
                Update,
                (
                    compute_extent,
                    rebuild_world_grid_when_ready,
                    apply_world_grid_visibility,
                    capture_original_light_levels,
                    apply_light_intensity_scale,
                    apply_wireframe_toggle,
                    draw_axes,
                    draw_prim_markers,
                )
                    .chain(),
            );
    }
}

/// Captured authored DirectionalLight.illuminance, latched on first
/// spawn so a later global scale multiplies the original value, not
/// whatever the last frame drove it to.
#[derive(Component, Debug, Copy, Clone)]
pub struct OriginalIlluminance(pub f32);

/// Same idea for Point / Spot lights, whose authored strength lives on
/// `.intensity` (candela/lumen) rather than `.illuminance` (lux).
#[derive(Component, Debug, Copy, Clone)]
pub struct OriginalLightIntensity(pub f32);

fn capture_original_light_levels(
    mut cmds: Commands,
    dir: Query<(Entity, &DirectionalLight), (Added<DirectionalLight>, Without<OriginalIlluminance>)>,
    pt: Query<(Entity, &PointLight), (Added<PointLight>, Without<OriginalLightIntensity>)>,
    sp: Query<(Entity, &SpotLight), (Added<SpotLight>, Without<OriginalLightIntensity>)>,
) {
    for (e, l) in &dir {
        cmds.entity(e).insert(OriginalIlluminance(l.illuminance));
    }
    for (e, l) in &pt {
        cmds.entity(e).insert(OriginalLightIntensity(l.intensity));
    }
    for (e, l) in &sp {
        cmds.entity(e).insert(OriginalLightIntensity(l.intensity));
    }
}

fn apply_light_intensity_scale(
    toggles: Res<DisplayToggles>,
    mut dir: Query<(&mut DirectionalLight, &OriginalIlluminance)>,
    mut pt: Query<(&mut PointLight, &OriginalLightIntensity)>,
    mut sp: Query<(&mut SpotLight, &OriginalLightIntensity)>,
) {
    let s = toggles.light_intensity_scale;
    for (mut l, o) in &mut dir {
        l.illuminance = o.0 * s;
    }
    for (mut l, o) in &mut pt {
        l.intensity = o.0 * s;
    }
    for (mut l, o) in &mut sp {
        l.intensity = o.0 * s;
    }
}

fn apply_wireframe_toggle(
    toggles: Res<DisplayToggles>,
    mut cfg: ResMut<bevy::pbr::wireframe::WireframeConfig>,
) {
    if cfg.global != toggles.wireframe {
        cfg.global = toggles.wireframe;
    }
}


/// Persistent overlay state, mutated by the Overlays panel + keyboard.
#[derive(Resource, Debug, Clone)]
pub struct DisplayToggles {
    /// Ground grid — auto-sized + radially faded. On by default; anchors
    /// the eye and doubles as a reference plane since we don't draw a
    /// solid ground plate.
    pub show_world_grid: bool,
    /// R/G/B axis triad at world origin.
    pub show_world_axes: bool,
    /// Tiny axis gizmo at every geom-bearing prim — invaluable on sparse
    /// M1 scenes and a compact debug view for dense M2+ scenes.
    pub show_prim_markers: bool,
    /// User bias on top of the auto-computed prim-marker length. 1.0 =
    /// follow the scene, 0.5 = half as long, 2.0 = twice.
    pub prim_marker_bias: f32,
    /// Bone overlay for UsdSkel skeletons — line segments between each
    /// joint and its parent. Useful for verifying the rig is animating
    /// even when the skinned mesh hides what's happening.
    pub show_skeleton: bool,
    /// Global wireframe mode — drives `WireframeConfig.global`.
    pub wireframe: bool,
    /// Multiplier applied to every authored light's intensity. Captured
    /// originals live on `OriginalIlluminance` / `OriginalLightIntensity`
    /// components so the scale is stable across stage reloads.
    pub light_intensity_scale: f32,
}

impl Default for DisplayToggles {
    fn default() -> Self {
        // Grid stays on as the reference plane. Axes + per-prim triads
        // are off by default — they're useful for debugging M1-era
        // wireframe-only scenes but clutter up a real lit scene. User
        // turns them on via the Overlays panel (O) or the G/X/P hotkeys.
        Self {
            show_world_grid: true,
            show_world_axes: false,
            show_prim_markers: false,
            prim_marker_bias: 1.0,
            show_skeleton: false,
            wireframe: false,
            light_intensity_scale: 1.0,
        }
    }
}

/// Axis-aligned bounds of everything the USD projection spawned. Updated
/// every frame by [`compute_extent`]; zero-sized before any prims land.
#[derive(Resource, Debug, Clone, Copy)]
pub struct SceneExtent {
    pub min: Vec3,
    pub max: Vec3,
    pub count: u32,
}

impl Default for SceneExtent {
    fn default() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
            count: 0,
        }
    }
}

impl SceneExtent {
    /// Diagonal length, clamped to 1 m when empty so overlays still render
    /// at sensible defaults before the scene materializes.
    pub fn diag(&self) -> f32 {
        if self.count == 0 {
            1.0
        } else {
            (self.max - self.min).length().max(0.01)
        }
    }

    pub fn centre(&self) -> Vec3 {
        if self.count == 0 {
            Vec3::ZERO
        } else {
            (self.min + self.max) * 0.5
        }
    }
}

fn compute_extent(
    prims: Query<
        (
            &GlobalTransform,
            Option<&bevy_openusd::UsdLocalExtent>,
            Option<&bevy::camera::primitives::Aabb>,
        ),
        With<UsdPrimRef>,
    >,
    mut extent: ResMut<SceneExtent>,
) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0u32;
    for (gt, local, aabb) in prims.iter() {
        if let Some(le) = local {
            // Project all 8 corners of the authored local AABB through
            // the prim's world transform, then fold min/max. Gives an
            // upper-bound world AABB without needing to walk mesh
            // vertices.
            let m = gt.to_matrix();
            for i in 0..8 {
                let c = Vec3::new(
                    if i & 1 == 0 { le.min[0] } else { le.max[0] },
                    if i & 2 == 0 { le.min[1] } else { le.max[1] },
                    if i & 4 == 0 { le.min[2] } else { le.max[2] },
                );
                let w = m.transform_point3(c);
                min = min.min(w);
                max = max.max(w);
            }
        } else if let Some(aabb) = aabb {
            // Bevy auto-computes an `Aabb` component for any mesh
            // entity from its vertex buffer. Project its 8 corners
            // through the prim's world transform — same as the USD
            // local-extent path, just sourced from rendered geometry
            // instead of authored metadata. Without this, assets like
            // Apple's chameleon (which doesn't author per-prim
            // extents) would report a 0.01m scene diagonal because we
            // fell straight through to the prim-origin fallback below.
            let m = gt.to_matrix();
            let center = Vec3::from(aabb.center);
            let half = Vec3::from(aabb.half_extents);
            for i in 0..8 {
                let local = Vec3::new(
                    if i & 1 == 0 { center.x - half.x } else { center.x + half.x },
                    if i & 2 == 0 { center.y - half.y } else { center.y + half.y },
                    if i & 4 == 0 { center.z - half.z } else { center.z + half.z },
                );
                let w = m.transform_point3(local);
                min = min.min(w);
                max = max.max(w);
            }
        } else {
            // Fallback: just the prim's origin. Coarser but cheap and
            // stable for prims without authored extent.
            let p = gt.translation();
            min = min.min(p);
            max = max.max(p);
        }
        count += 1;
    }
    *extent = SceneExtent { min, max, count };
}

// ── World grid ───────────────────────────────────────────────────────────

/// Line thickness is authored as a *fraction of grid spacing* so lines stay
/// visible regardless of scene scale. Ratios mirror the reference numbers
/// in `../bevy_urdf/src/overlays.rs` (minor 0.005 m at 0.25 m spacing,
/// major 0.012 m at 1.0 m spacing, dot radius 0.025 m at 1.0 m spacing).
const GRID_MINOR_THICK_FRAC: f32 = 0.020; // 0.005 / 0.25
const GRID_MAJOR_THICK_FRAC: f32 = 0.012; // 0.012 / 1.0
const GRID_DOT_RADIUS_FRAC: f32 = 0.025; // 0.025 / 1.0
const GRID_DOT_SEGMENTS: u32 = 12;

/// Marker split by layer so visibility / material tweaks don't need a
/// mesh rebuild.
#[derive(Component, Copy, Clone, PartialEq, Eq)]
enum WorldGridKind {
    Minor,
    Major,
    Dots,
}

/// Remembers whether the grid has been built and at which extent diagonal.
/// Scene diagonal needs to change by > `diag * 0.1` before we rebuild, so
/// camera panning + incremental prim additions don't thrash the mesh
/// buffers.
#[derive(Resource, Default)]
struct WorldGridState {
    built_for_diag: f32,
    entities: Vec<Entity>,
}

fn rebuild_world_grid_when_ready(
    mut commands: Commands,
    extent: Res<SceneExtent>,
    mut state: ResMut<WorldGridState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if extent.count == 0 {
        return;
    }
    let diag = extent.diag();
    let prev = state.built_for_diag;
    // Rebuild when the extent grows / shrinks by >10 % so we don't thrash
    // on tiny transform updates. First build always passes the check.
    if prev > 0.0 && (diag / prev - 1.0).abs() < 0.1 {
        return;
    }

    // Tear down the old layers.
    for ent in state.entities.drain(..) {
        if let Ok(mut e) = commands.get_entity(ent) {
            e.despawn();
        }
    }

    // Grid spans ~4× the scene diagonal; clamp so tiny scenes still get a
    // useful visible area and huge scenes don't tank the triangle count.
    let half = (diag * 2.0).clamp(5.0, 500.0);
    let major = bucket_step(half / 20.0);
    let minor = (major / 4.0).max(major * 0.1);
    let minor_thick = minor * GRID_MINOR_THICK_FRAC;
    let major_thick = major * GRID_MAJOR_THICK_FRAC;
    let dot_radius = major * GRID_DOT_RADIUS_FRAC;
    let centre = Vec3::new(extent.centre().x, 0.0, extent.centre().z);

    // Three meshes: minor lines, major lines, dots at every major
    // intersection. Separate entities for independent visibility + to
    // avoid z-fighting via a tiny Y offset per layer.
    let minor_mesh = meshes.add(build_grid_lines_mesh(
        half,
        minor,
        major,
        minor_thick,
        /* skip_major */ true,
    ));
    let major_mesh = meshes.add(build_grid_lines_mesh(
        half,
        major,
        major,
        major_thick,
        /* skip_major */ false,
    ));
    let dots_mesh = meshes.add(build_grid_dots_mesh(half, major, dot_radius));

    let make_mat = |c: Color, materials: &mut Assets<StandardMaterial>| {
        materials.add(StandardMaterial {
            base_color: c,
            emissive: c.to_linear(),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            ..default()
        })
    };

    let minor_color = Color::linear_rgba(0.45, 0.48, 0.55, 0.18);
    let major_color = Color::linear_rgba(0.70, 0.75, 0.85, 0.55);
    let dot_color = Color::linear_rgba(0.95, 0.95, 1.00, 0.80);

    let minor_ent = commands
        .spawn((
            Name::new("WorldGrid:Minor"),
            WorldGridKind::Minor,
            Mesh3d(minor_mesh),
            MeshMaterial3d(make_mat(minor_color, &mut materials)),
            Transform::from_translation(centre + Vec3::new(0.0, 0.000, 0.0)),
            bevy::light::NotShadowCaster,
        ))
        .id();
    let major_ent = commands
        .spawn((
            Name::new("WorldGrid:Major"),
            WorldGridKind::Major,
            Mesh3d(major_mesh),
            MeshMaterial3d(make_mat(major_color, &mut materials)),
            Transform::from_translation(centre + Vec3::new(0.0, 0.001, 0.0)),
            bevy::light::NotShadowCaster,
        ))
        .id();
    let dots_ent = commands
        .spawn((
            Name::new("WorldGrid:Dots"),
            WorldGridKind::Dots,
            Mesh3d(dots_mesh),
            MeshMaterial3d(make_mat(dot_color, &mut materials)),
            Transform::from_translation(centre + Vec3::new(0.0, 0.002, 0.0)),
            bevy::light::NotShadowCaster,
        ))
        .id();

    state.built_for_diag = diag;
    state.entities = vec![minor_ent, major_ent, dots_ent];
}

fn apply_world_grid_visibility(
    toggles: Res<DisplayToggles>,
    mut q: Query<(&WorldGridKind, &mut Visibility)>,
) {
    let want = if toggles.show_world_grid {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    for (_kind, mut vis) in q.iter_mut() {
        if *vis != want {
            *vis = want;
        }
    }
}

/// Build a mesh of thin horizontal quads forming a grid of lines.
/// `spacing` is the line step; `major_every` defines the "major line"
/// boundary — when `skip_major` is true, lines on that boundary are
/// omitted (so the minor layer doesn't overlap the major layer).
///
/// **Each line is subdivided into `LINE_SEGMENTS` sub-quads** so the
/// per-vertex radial alpha-fade samples interior points, not just the
/// boundary corners. Without this split, every line vertex sits at the
/// clip edge where `fade_alpha = 0` → all lines invisible.
fn build_grid_lines_mesh(
    half: f32,
    spacing: f32,
    major_every: f32,
    thickness: f32,
    skip_major: bool,
) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut next_index: u32 = 0;

    let lines = (half / spacing) as i32;
    for lz in -lines..=lines {
        let z = lz as f32 * spacing;
        if skip_major && is_major(z, major_every) {
            continue;
        }
        push_line_segmented(
            &mut positions,
            &mut normals,
            &mut colors,
            &mut indices,
            &mut next_index,
            /* start */ Vec3::new(-half, 0.0, z),
            /* end */ Vec3::new(half, 0.0, z),
            thickness,
            half,
        );
    }
    for lx in -lines..=lines {
        let x = lx as f32 * spacing;
        if skip_major && is_major(x, major_every) {
            continue;
        }
        push_line_segmented(
            &mut positions,
            &mut normals,
            &mut colors,
            &mut indices,
            &mut next_index,
            /* start */ Vec3::new(x, 0.0, -half),
            /* end */ Vec3::new(x, 0.0, half),
            thickness,
            half,
        );
    }
    finalize_mesh(positions, normals, colors, indices)
}

/// Number of sub-quads per grid line. Enough sampling points along a line
/// so the radial alpha-fade reads as a smooth curve rather than an abrupt
/// cutoff. 20 segments = 21 spine vertices × 2 sides = 42 verts / line;
/// at ~40 lines per layer on a 200 m grid that's ~1.7k verts — trivial.
const LINE_SEGMENTS: u32 = 20;

/// Emit a thick line from `start` to `end` as a chain of `LINE_SEGMENTS`
/// quads. The line is offset perpendicular to its direction (in the XZ
/// plane) by `thickness / 2` on each side. Per-vertex alpha is the
/// radial fade evaluated at each spine sample, so mid-span vertices
/// stay visible even when endpoints sit at the boundary.
fn push_line_segmented(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    next_index: &mut u32,
    start: Vec3,
    end: Vec3,
    thickness: f32,
    half: f32,
) {
    // Perpendicular in the XZ plane: rotate direction by 90°. Sufficient
    // for the axis-aligned lines the grid draws (direction is always ±X
    // or ±Z); would need renormalisation for diagonals.
    let dir = (end - start).normalize_or_zero();
    let perp = Vec3::new(dir.z, 0.0, -dir.x) * (thickness * 0.5);

    let segments = LINE_SEGMENTS.max(1);
    for s in 0..=segments {
        let t = s as f32 / segments as f32;
        let spine = start.lerp(end, t);
        let left = spine - perp;
        let right = spine + perp;
        let alpha = fade_alpha(spine, half);
        positions.push([left.x, 0.0, left.z]);
        positions.push([right.x, 0.0, right.z]);
        normals.extend([[0.0, 1.0, 0.0]; 2]);
        colors.push([1.0, 1.0, 1.0, alpha]);
        colors.push([1.0, 1.0, 1.0, alpha]);
    }
    // Stitch consecutive pairs (L_i, R_i, L_{i+1}, R_{i+1}) into two tris.
    for s in 0..segments {
        let base = *next_index + s * 2;
        indices.extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
    }
    *next_index += (segments + 1) * 2;
}

fn build_grid_dots_mesh(half: f32, spacing: f32, radius: f32) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut next_index: u32 = 0;
    let lines = (half / spacing) as i32;
    for lx in -lines..=lines {
        for lz in -lines..=lines {
            let x = lx as f32 * spacing;
            let z = lz as f32 * spacing;
            push_disc_faded(
                &mut positions,
                &mut normals,
                &mut colors,
                &mut indices,
                &mut next_index,
                Vec3::new(x, 0.0, z),
                radius,
                GRID_DOT_SEGMENTS,
                half,
            );
        }
    }
    finalize_mesh(positions, normals, colors, indices)
}

fn is_major(coord: f32, major_every: f32) -> bool {
    let q = (coord / major_every).round();
    (coord - q * major_every).abs() < 1e-3
}

/// `(1 - (r/half)^2)` then clamped; r = max(|x|, |z|) so the fade matches
/// the square grid rather than a disc. Baked into vertex alpha.
fn fade_alpha(pos: Vec3, half: f32) -> f32 {
    let r = pos.x.abs().max(pos.z.abs()) / half;
    let a = 1.0 - r * r;
    a.clamp(0.0, 1.0)
}

fn push_disc_faded(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    colors: &mut Vec<[f32; 4]>,
    indices: &mut Vec<u32>,
    next_index: &mut u32,
    centre: Vec3,
    radius: f32,
    segments: u32,
    half: f32,
) {
    let centre_alpha = fade_alpha(centre, half);
    let centre_index = *next_index;
    positions.push([centre.x, 0.0, centre.z]);
    normals.push([0.0, 1.0, 0.0]);
    colors.push([1.0, 1.0, 1.0, centre_alpha]);
    *next_index += 1;
    for step in 0..segments {
        let angle = (step as f32 / segments as f32) * core::f32::consts::TAU;
        let pos = Vec3::new(
            centre.x + angle.cos() * radius,
            0.0,
            centre.z + angle.sin() * radius,
        );
        positions.push([pos.x, 0.0, pos.z]);
        normals.push([0.0, 1.0, 0.0]);
        colors.push([1.0, 1.0, 1.0, centre_alpha]);
        *next_index += 1;
    }
    for step in 0..segments {
        indices.extend_from_slice(&[
            centre_index,
            centre_index + 1 + step,
            centre_index + 1 + ((step + 1) % segments),
        ]);
    }
}

fn finalize_mesh(
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Bucket a candidate grid-line spacing to the nearest "nice" round value
/// so the grid always reads as decades-of-10 × {1, 2, 5}. Covers sub-mm
/// to kilometre scenes.
fn bucket_step(raw: f32) -> f32 {
    if raw <= 0.0 {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let mantissa = raw / 10f32.powf(exp);
    let m = if mantissa < 2.0 {
        1.0
    } else if mantissa < 5.0 {
        2.0
    } else {
        5.0
    };
    m * 10f32.powf(exp)
}

// ── World axes ───────────────────────────────────────────────────────────

fn draw_axes(toggles: Res<DisplayToggles>, extent: Res<SceneExtent>, mut gizmos: Gizmos) {
    if !toggles.show_world_axes {
        return;
    }
    let l = (extent.diag() * 0.05).clamp(0.1, 10.0);
    gizmos.line(
        Vec3::ZERO,
        Vec3::new(l, 0.0, 0.0),
        Color::srgb(1.0, 0.2, 0.2),
    );
    gizmos.line(
        Vec3::ZERO,
        Vec3::new(0.0, l, 0.0),
        Color::srgb(0.2, 1.0, 0.2),
    );
    gizmos.line(
        Vec3::ZERO,
        Vec3::new(0.0, 0.0, l),
        Color::srgb(0.4, 0.5, 1.0),
    );
    let _ = LinearRgba::BLACK; // silence unused imports on some profiles
}

// ── Per-prim markers ─────────────────────────────────────────────────────

fn draw_prim_markers(
    toggles: Res<DisplayToggles>,
    extent: Res<SceneExtent>,
    prims: Query<&GlobalTransform, (With<UsdPrimRef>, With<Mesh3d>)>,
    mut gizmos: Gizmos,
) {
    if !toggles.show_prim_markers {
        return;
    }
    let base = (extent.diag() * 0.015).clamp(0.005, extent.diag().max(0.01));
    let l = (base * toggles.prim_marker_bias).max(0.0);
    if l < 1e-4 {
        return;
    }
    for gt in prims.iter() {
        let origin = gt.translation();
        let rot = gt.rotation();
        gizmos.line(
            origin,
            origin + rot * Vec3::X * l,
            Color::srgb(1.0, 0.35, 0.35),
        );
        gizmos.line(
            origin,
            origin + rot * Vec3::Y * l,
            Color::srgb(0.35, 1.0, 0.35),
        );
        gizmos.line(
            origin,
            origin + rot * Vec3::Z * l,
            Color::srgb(0.5, 0.6, 1.0),
        );
    }
}
