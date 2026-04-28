//! UsdLux → Bevy lights.
//!
//! Translates `usd_schemas::lux::ReadLight` variants into Bevy's
//! `DirectionalLight` / `PointLight` / `SpotLight` bundles. Omniverse
//! scenes that were dark before M9 (we dropped every light on the floor)
//! now pick up the authored lighting.
//!
//! ## Intensity convention
//!
//! UsdLux separates `intensity` (relative scalar) and `exposure` (stops).
//! Bevy's `PointLight.intensity` is lumens, `DirectionalLight.illuminance`
//! is lux. Those aren't the same units as UsdLux — every DCC picks
//! slightly different conventions. We compute a *reasonable guess*:
//!
//!   total = intensity * 2^exposure * light_intensity_scale
//!   PointLight.intensity   = total * 1000.0     // ~1000 lm/unit is a mid-ground
//!   SpotLight.intensity    = total * 1000.0
//!   DirectionalLight.illum = total * 1000.0     // assuming intensity is already lux-like
//!
//! Users dial `UsdLoaderSettings::light_intensity_scale` when a scene
//! looks blown out or too dark.

use bevy::color::{Color, LinearRgba};
use bevy::ecs::entity::Entity;
use bevy::ecs::world::World;
use bevy::light::{DirectionalLight, PointLight, SpotLight};
use usd_schemas::lux::{
    LightCommon, ReadCylinderLight, ReadDiskLight, ReadDistantLight, ReadLight, ReadRectLight,
    ReadSphereLight,
};

/// Compute the "total brightness" — what Bevy lights want as a single
/// number. Combines `intensity` with `2^exposure`, applies the global
/// scale, defaults to 1.0 when nothing is authored.
fn brightness(common: &LightCommon, scale: f32) -> f32 {
    let i = common.intensity.unwrap_or(1.0);
    let e = common.exposure.unwrap_or(0.0);
    i * 2.0f32.powf(e) * scale
}

fn authored_color(common: &LightCommon) -> Color {
    match common.color {
        Some([r, g, b]) => Color::LinearRgba(LinearRgba::rgb(r, g, b)),
        None => Color::WHITE,
    }
}

/// Spawn the right Bevy light component on `entity` based on `read`.
/// Attaches to the existing prim entity rather than creating a new one
/// so the USD prim hierarchy keeps its shape.
pub fn spawn_light(
    world: &mut World,
    entity: Entity,
    read: &ReadLight,
    scale: f32,
    prim_path: &str,
    parent: Entity,
) -> Tally {
    match read {
        ReadLight::Distant(d) => {
            world
                .entity_mut(entity)
                .insert(directional_from_usd(d, scale));
            Tally {
                directional: 1,
                ..Default::default()
            }
        }
        ReadLight::Sphere(s) => {
            // Cone authored → behave as a spotlight (that's what USD's
            // `shaping:cone` semantically means on a SphereLight).
            if s.cone_angle_deg.is_some() {
                world.entity_mut(entity).insert(spot_from_sphere(s, scale));
                Tally {
                    spot: 1,
                    ..Default::default()
                }
            } else {
                world.entity_mut(entity).insert(point_from_sphere(s, scale));
                Tally {
                    point: 1,
                    ..Default::default()
                }
            }
        }
        ReadLight::Rect(r) => {
            // Bevy has no rectangular area light; approximate as a narrow
            // spot pointing -Z (UsdLux convention) scaled by the aspect.
            world.entity_mut(entity).insert(spot_from_rect(r, scale));
            Tally {
                spot: 1,
                ..Default::default()
            }
        }
        ReadLight::Disk(d) => {
            // Same story as Rect — approximate with a spot.
            world.entity_mut(entity).insert(spot_from_disk(d, scale));
            Tally {
                spot: 1,
                ..Default::default()
            }
        }
        ReadLight::Cylinder(c) => {
            // Strip-like; we spawn a row of small point lights along the
            // local +X axis to fake it.
            let count = cylinder_fan(world, entity, c, scale, prim_path, parent);
            Tally {
                point: count,
                ..Default::default()
            }
        }
        ReadLight::Dome(d) => {
            // Proper IBL wiring is deferred to M9.1; log so the user sees
            // that the light *was* authored but we're ignoring the
            // environment map.
            bevy::log::info!(
                "UsdLuxDomeLight {prim_path}: deferred (texture={:?}, format={:?})",
                d.texture_file,
                d.texture_format
            );
            // Fallback: a dim white ambient boost so the scene isn't pitch
            // black in the absence of IBL. Lives on a GlobalAmbientLight
            // resource the viewer already manages; from a loader we only
            // note it in the tally.
            Tally {
                dome: 1,
                ..Default::default()
            }
        }
    }
}

/// Running tally of what we translated — surfaced on the viewer's Info
/// panel so users can see how the authored lighting got mapped.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tally {
    pub directional: usize,
    pub point: usize,
    pub spot: usize,
    pub dome: usize,
}

impl Tally {
    pub fn add(&mut self, other: Tally) {
        self.directional += other.directional;
        self.point += other.point;
        self.spot += other.spot;
        self.dome += other.dome;
    }
}

// ── Per-type builders ────────────────────────────────────────────────────

fn directional_from_usd(d: &ReadDistantLight, _scale: f32) -> DirectionalLight {
    // USD's DistantLight points along -Z by convention; the prim's
    // transform handles orientation — we just emit a DirectionalLight and
    // let the parent entity's `Transform` aim it.
    //
    // Intensity is deliberately low (5k lux ≈ overcast daylight) and
    // ignores the authored value because USD doesn't standardise
    // DistantLight units and authoring tools disagree by 3+ orders
    // of magnitude (intensity=3 through intensity=1500 both show up
    // in real files). The viewer's own fallback sun is the stable
    // baseline; the authored light just adds a secondary fill at a
    // direction the stage wanted.
    DirectionalLight {
        color: authored_color(&d.common),
        illuminance: 5_000.0,
        shadows_enabled: true,
        ..Default::default()
    }
}

fn point_from_sphere(s: &ReadSphereLight, scale: f32) -> PointLight {
    let radius = s.radius.unwrap_or(0.0);
    PointLight {
        color: authored_color(&s.common),
        intensity: brightness(&s.common, scale) * 1000.0,
        range: (radius * 10.0).max(5.0),
        shadows_enabled: true,
        ..Default::default()
    }
}

fn spot_from_sphere(s: &ReadSphereLight, scale: f32) -> SpotLight {
    let outer = s.cone_angle_deg.unwrap_or(30.0).to_radians();
    let softness = s.cone_softness.unwrap_or(0.0).clamp(0.0, 1.0);
    let inner = outer * (1.0 - softness);
    SpotLight {
        color: authored_color(&s.common),
        intensity: brightness(&s.common, scale) * 1000.0,
        range: (s.radius.unwrap_or(0.0) * 10.0).max(5.0),
        outer_angle: outer,
        inner_angle: inner.min(outer - 1.0e-3),
        shadows_enabled: true,
        ..Default::default()
    }
}

fn spot_from_rect(r: &ReadRectLight, scale: f32) -> SpotLight {
    let w = r.width.unwrap_or(1.0).max(0.01);
    let h = r.height.unwrap_or(1.0).max(0.01);
    let outer = (w.max(h) * 0.5).atan().max(0.1);
    SpotLight {
        color: authored_color(&r.common),
        intensity: brightness(&r.common, scale) * 1000.0,
        range: 20.0,
        outer_angle: outer,
        inner_angle: outer * 0.85,
        shadows_enabled: true,
        ..Default::default()
    }
}

fn spot_from_disk(d: &ReadDiskLight, scale: f32) -> SpotLight {
    let radius = d.radius.unwrap_or(0.5).max(0.01);
    let outer = radius.atan().max(0.1);
    SpotLight {
        color: authored_color(&d.common),
        intensity: brightness(&d.common, scale) * 1000.0,
        range: 20.0,
        outer_angle: outer,
        inner_angle: outer * 0.85,
        shadows_enabled: true,
        ..Default::default()
    }
}

fn cylinder_fan(
    world: &mut World,
    parent_entity: Entity,
    c: &ReadCylinderLight,
    scale: f32,
    _prim_path: &str,
    _parent_world_entity: Entity,
) -> usize {
    // Collapsed to a single PointLight — real scenes author hundreds of
    // CylinderLights (510 in the greenhouse), and Bevy's forward renderer
    // caps well below that. If/when we gain a proper area-light shader
    // (M17 or so) we switch back to a multi-sample or native strip.
    //
    // The 0.05 factor accounts for the "every grow lamp in a greenhouse
    // is authored at full production brightness" reality — at full scale
    // a scene with hundreds of them blows out every surface. Dial it back
    // via `UsdLoaderSettings::light_intensity_scale` if you want it hot.
    const CYLINDER_DIM: f32 = 0.05;
    world.entity_mut(parent_entity).insert(PointLight {
        color: authored_color(&c.common),
        intensity: brightness(&c.common, scale) * 1000.0 * CYLINDER_DIM,
        range: (c.length.unwrap_or(0.5) + c.radius.unwrap_or(0.1) * 10.0).max(2.0),
        shadows_enabled: false,
        ..Default::default()
    });
    1
}
