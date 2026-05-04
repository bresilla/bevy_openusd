//! UsdLux readers.
//!
//! Symmetric to the authoring helpers elsewhere in this crate: given a
//! composed `openusd::Stage` and a prim `Path`, return typed Rust
//! descriptions of the authored light. No Bevy dependency — the consumer
//! (`bevy_openusd::light`) handles the Bevy-side mapping.
//!
//! All UsdLux lights share a common set of inputs (`intensity`, `color`,
//! `exposure`, `diffuse`, `specular`, `enableColorTemperature`,
//! `colorTemperature`). Each concrete light type adds shape attributes on
//! top (`radius`, `width`, `height`, `length`, `angle`, `texture:file`).
//!
//! Reference: <https://openusd.org/release/api/usd_lux_page_front.html>.

use anyhow::Result;
use openusd::sdf::{Path, Value};

/// Inputs shared across every UsdLux light. `None` fields mean "inherit
/// the UsdLux default" — consumer decides whether that's 1.0 or the Bevy
/// default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightCommon {
    /// `inputs:intensity`. Relative scalar; combine with `2^exposure`.
    pub intensity: Option<f32>,
    /// `inputs:exposure`. Stops above/below base intensity.
    pub exposure: Option<f32>,
    /// `inputs:color`, sRGB linearised to `[f32; 3]`.
    pub color: Option<[f32; 3]>,
    /// `inputs:diffuse` / `inputs:specular` multipliers. Bevy can't drive
    /// diffuse-specular split independently so consumers typically average.
    pub diffuse: Option<f32>,
    pub specular: Option<f32>,
    /// `inputs:enableColorTemperature` — when true, override the authored
    /// colour with a Kelvin temperature.
    pub enable_color_temperature: Option<bool>,
    /// `inputs:colorTemperature` in Kelvin (USD default ≈ 6500).
    pub color_temperature: Option<f32>,
    /// `inputs:normalize` — when true, UsdLux divides intensity by the
    /// authored surface area. Bevy's point/spot lights author their
    /// intensity in lumens regardless; flag carried through so the
    /// consumer can undo the normalisation.
    pub normalize: Option<bool>,
    /// `UsdLuxLightAPI.light:link` — relationship targets for which
    /// geometry the light illuminates. Empty vec = no linking
    /// authored (implicit all-geometry).
    pub light_link_targets: Vec<String>,
    /// `UsdLuxLightAPI.shadow:link` — geometry that CASTS shadows
    /// from this light. Empty vec = no linking authored.
    pub shadow_link_targets: Vec<String>,
    /// `UsdLuxLightAPI.light:filters` — light-filter prim paths.
    /// Empty vec = no filters.
    pub light_filters: Vec<String>,
}

/// `UsdLuxDistantLight`. Sun-style parallel light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadDistantLight {
    pub common: LightCommon,
    /// Half-angle of the disc subtended by the sun, in degrees (USD default 0.53).
    pub angle_deg: Option<f32>,
}

/// `UsdLuxSphereLight`. Point / spherical area light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadSphereLight {
    pub common: LightCommon,
    pub radius: Option<f32>,
    /// `inputs:shaping:cone:angle` (degrees) — when authored, makes the
    /// sphere light behave as a cone; maps cleanly to Bevy `SpotLight`.
    pub cone_angle_deg: Option<f32>,
    /// `inputs:shaping:cone:softness` [0, 1] — maps to Bevy spot light
    /// inner/outer cone feather.
    pub cone_softness: Option<f32>,
}

/// `UsdLuxRectLight`. Rectangular area light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadRectLight {
    pub common: LightCommon,
    pub width: Option<f32>,
    pub height: Option<f32>,
}

/// `UsdLuxDiskLight`. Circular area light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadDiskLight {
    pub common: LightCommon,
    pub radius: Option<f32>,
}

/// `UsdLuxCylinderLight`. Tube / strip light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadCylinderLight {
    pub common: LightCommon,
    pub length: Option<f32>,
    pub radius: Option<f32>,
}

/// `UsdLuxDomeLight`. Image-based environment light.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadDomeLight {
    pub common: LightCommon,
    /// `inputs:texture:file` — HDR / EXR environment map path.
    pub texture_file: Option<String>,
    /// `inputs:texture:format` — token like "automatic" / "latlong" /
    /// "mirroredBall" / "angular".
    pub texture_format: Option<String>,
}

// ── Dispatcher ───────────────────────────────────────────────────────────

/// Dispatch helper — read the prim's `typeName` and return the matching
/// concrete reader result. Returns `None` when the typeName isn't a
/// recognised UsdLux light.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadLight {
    Distant(ReadDistantLight),
    Sphere(ReadSphereLight),
    Rect(ReadRectLight),
    Disk(ReadDiskLight),
    Cylinder(ReadCylinderLight),
    Dome(ReadDomeLight),
}

pub fn read_light(stage: &openusd::Stage, prim: &Path) -> Result<Option<ReadLight>> {
    let type_name: Option<String> = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?;
    let Some(type_name) = type_name else {
        return Ok(None);
    };
    Ok(match type_name.as_str() {
        "DistantLight" => Some(ReadLight::Distant(read_distant_light(stage, prim)?)),
        "SphereLight" => Some(ReadLight::Sphere(read_sphere_light(stage, prim)?)),
        "RectLight" => Some(ReadLight::Rect(read_rect_light(stage, prim)?)),
        "DiskLight" => Some(ReadLight::Disk(read_disk_light(stage, prim)?)),
        "CylinderLight" => Some(ReadLight::Cylinder(read_cylinder_light(stage, prim)?)),
        "DomeLight" => Some(ReadLight::Dome(read_dome_light(stage, prim)?)),
        _ => None,
    })
}

pub fn is_light_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "DistantLight"
            | "SphereLight"
            | "RectLight"
            | "DiskLight"
            | "CylinderLight"
            | "DomeLight"
            | "GeometryLight"
            | "PortalLight"
    )
}

// ── Per-type readers ─────────────────────────────────────────────────────

pub fn read_distant_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadDistantLight> {
    Ok(ReadDistantLight {
        common: read_common(stage, prim)?,
        angle_deg: read_f32(stage, prim, "inputs:angle")?,
    })
}

pub fn read_sphere_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadSphereLight> {
    Ok(ReadSphereLight {
        common: read_common(stage, prim)?,
        radius: read_f32(stage, prim, "inputs:radius")?,
        cone_angle_deg: read_f32(stage, prim, "inputs:shaping:cone:angle")?,
        cone_softness: read_f32(stage, prim, "inputs:shaping:cone:softness")?,
    })
}

pub fn read_rect_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadRectLight> {
    Ok(ReadRectLight {
        common: read_common(stage, prim)?,
        width: read_f32(stage, prim, "inputs:width")?,
        height: read_f32(stage, prim, "inputs:height")?,
    })
}

pub fn read_disk_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadDiskLight> {
    Ok(ReadDiskLight {
        common: read_common(stage, prim)?,
        radius: read_f32(stage, prim, "inputs:radius")?,
    })
}

pub fn read_cylinder_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadCylinderLight> {
    Ok(ReadCylinderLight {
        common: read_common(stage, prim)?,
        length: read_f32(stage, prim, "inputs:length")?,
        radius: read_f32(stage, prim, "inputs:radius")?,
    })
}

pub fn read_dome_light(stage: &openusd::Stage, prim: &Path) -> Result<ReadDomeLight> {
    Ok(ReadDomeLight {
        common: read_common(stage, prim)?,
        texture_file: read_asset_path(stage, prim, "inputs:texture:file")?,
        texture_format: read_token_or_string(stage, prim, "inputs:texture:format")?,
    })
}

// ── Attribute plumbing ───────────────────────────────────────────────────

fn read_common(stage: &openusd::Stage, prim: &Path) -> Result<LightCommon> {
    Ok(LightCommon {
        intensity: read_f32(stage, prim, "inputs:intensity")?,
        exposure: read_f32(stage, prim, "inputs:exposure")?,
        color: read_vec3f(stage, prim, "inputs:color")?,
        diffuse: read_f32(stage, prim, "inputs:diffuse")?,
        specular: read_f32(stage, prim, "inputs:specular")?,
        enable_color_temperature: read_bool(stage, prim, "inputs:enableColorTemperature")?,
        color_temperature: read_f32(stage, prim, "inputs:colorTemperature")?,
        normalize: read_bool(stage, prim, "inputs:normalize")?,
        light_link_targets: read_rel_targets(stage, prim, "light:link")?,
        shadow_link_targets: read_rel_targets(stage, prim, "shadow:link")?,
        light_filters: read_rel_targets(stage, prim, "light:filters")?,
    })
}

fn read_rel_targets(
    stage: &openusd::Stage,
    prim: &Path,
    rel_name: &str,
) -> Result<Vec<String>> {
    let rel_path = prim.append_property(rel_name).map_err(anyhow::Error::from)?;
    let raw = stage
        .field::<Value>(rel_path, "targetPaths")
        .map_err(anyhow::Error::from)?;
    let paths = match raw {
        Some(Value::PathListOp(op)) => op.flatten(),
        Some(Value::PathVec(v)) => v,
        _ => Vec::new(),
    };
    Ok(paths.into_iter().map(|p| p.as_str().to_string()).collect())
}

fn attr_default(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<Value>> {
    let attr = prim.append_property(name).map_err(anyhow::Error::from)?;
    stage
        .field::<Value>(attr, "default")
        .map_err(anyhow::Error::from)
}

fn read_f32(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<f32>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Float(v)) => Some(v),
        Some(Value::Double(v)) => Some(v as f32),
        Some(Value::Int(v)) => Some(v as f32),
        _ => None,
    })
}

fn read_bool(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<bool>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Bool(v)) => Some(v),
        _ => None,
    })
}

fn read_vec3f(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<[f32; 3]>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Vec3f(v)) => Some(v),
        Some(Value::Vec3d(v)) => Some([v[0] as f32, v[1] as f32, v[2] as f32]),
        _ => None,
    })
}

fn read_token_or_string(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_asset_path(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    Ok(match attr_default(stage, prim, name)? {
        Some(Value::AssetPath(s)) | Some(Value::String(s)) | Some(Value::Token(s)) => Some(s),
        _ => None,
    })
}
