//! UsdGeom.Camera reader.
//!
//! USD stores camera geometry in *millimetres* (focal length, aperture
//! width / height) and scene units (clipping range). Consumers decide how
//! to map those to the rendering API of their choice — we just surface
//! the raw, post-composition values.
//!
//! The horizontal + vertical aperture × focal length triangle gives the
//! field of view:
//!
//!   fov_h = 2 · atan(h_aperture / (2 · focal_length))
//!   fov_v = 2 · atan(v_aperture / (2 · focal_length))
//!
//! Reference: <https://openusd.org/release/api/class_usd_geom_camera.html>.

use anyhow::Result;
use openusd::sdf::{Path, Value};

/// Decoded `UsdGeom.Camera`. `None` fields use USD defaults:
///
/// | attribute             | USD default      |
/// |-----------------------|------------------|
/// | `focalLength`         | 50.0 mm          |
/// | `horizontalAperture`  | 20.955 mm (35mm) |
/// | `verticalAperture`    | 15.2908 mm       |
/// | `clippingRange`       | (1.0, 1_000_000) |
/// | `projection`          | "perspective"    |
/// | `focusDistance`       | 0.0 (pinhole)    |
/// | `fStop`               | 0.0 (pinhole)    |
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadCamera {
    /// `focalLength` in millimetres.
    pub focal_length_mm: Option<f32>,
    /// `horizontalAperture` in millimetres.
    pub h_aperture_mm: Option<f32>,
    /// `verticalAperture` in millimetres.
    pub v_aperture_mm: Option<f32>,
    /// `clippingRange.x` / `.y` — near + far, in scene units.
    pub clip_near: Option<f32>,
    pub clip_far: Option<f32>,
    /// `"perspective"` or `"orthographic"`.
    pub projection: Option<Projection>,
    /// `focusDistance` — distance at which a DoF camera focuses.
    pub focus_distance: Option<f32>,
    /// `fStop` — lens aperture. 0 means "pinhole" / no DoF.
    pub f_stop: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    Orthographic,
}

impl ReadCamera {
    /// Vertical field of view in radians, computed from the authored
    /// aperture + focal length. Falls back to a 35 mm / 50 mm preset.
    pub fn vertical_fov_rad(&self) -> f32 {
        let v_aperture = self.v_aperture_mm.unwrap_or(15.2908);
        let focal = self.focal_length_mm.unwrap_or(50.0).max(0.001);
        2.0 * (v_aperture / (2.0 * focal)).atan()
    }

    pub fn horizontal_fov_rad(&self) -> f32 {
        let h_aperture = self.h_aperture_mm.unwrap_or(20.955);
        let focal = self.focal_length_mm.unwrap_or(50.0).max(0.001);
        2.0 * (h_aperture / (2.0 * focal)).atan()
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.h_aperture_mm.unwrap_or(20.955) / self.v_aperture_mm.unwrap_or(15.2908).max(0.001)
    }
}

pub fn read_camera(stage: &openusd::Stage, prim: &Path) -> Result<Option<ReadCamera>> {
    // Guard: only read prims whose typeName is "Camera".
    let type_name: Option<String> = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?;
    if type_name.as_deref() != Some("Camera") {
        return Ok(None);
    }

    let (near, far) = read_clipping_range(stage, prim)?;
    Ok(Some(ReadCamera {
        focal_length_mm: read_f32(stage, prim, "focalLength")?,
        h_aperture_mm: read_f32(stage, prim, "horizontalAperture")?,
        v_aperture_mm: read_f32(stage, prim, "verticalAperture")?,
        clip_near: near,
        clip_far: far,
        projection: read_projection(stage, prim)?,
        focus_distance: read_f32(stage, prim, "focusDistance")?,
        f_stop: read_f32(stage, prim, "fStop")?,
    }))
}

pub fn is_camera_type(type_name: &str) -> bool {
    type_name == "Camera"
}

// ── Attribute helpers ────────────────────────────────────────────────────

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

fn read_projection(stage: &openusd::Stage, prim: &Path) -> Result<Option<Projection>> {
    Ok(match attr_default(stage, prim, "projection")? {
        Some(Value::Token(t)) | Some(Value::String(t)) => match t.as_str() {
            "perspective" => Some(Projection::Perspective),
            "orthographic" => Some(Projection::Orthographic),
            _ => None,
        },
        _ => None,
    })
}

fn read_clipping_range(stage: &openusd::Stage, prim: &Path) -> Result<(Option<f32>, Option<f32>)> {
    Ok(match attr_default(stage, prim, "clippingRange")? {
        Some(Value::Vec2f(v)) => (Some(v[0]), Some(v[1])),
        Some(Value::Vec2d(v)) => (Some(v[0] as f32), Some(v[1] as f32)),
        _ => (None, None),
    })
}
