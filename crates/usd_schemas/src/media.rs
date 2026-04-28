//! `UsdMedia` schemas — currently just `SpatialAudio`.
//!
//! `UsdMediaSpatialAudio` is an Imageable, Xformable prim that
//! describes a sound source. We expose its fields as
//! [`ReadSpatialAudio`]; downstream consumers either ignore it (read
//! side only) or wire a real audio backend (bevy_audio etc.) on top.

use anyhow::Result;
use openusd::sdf::{Path, Value};

#[derive(Debug, Clone, Default)]
pub struct ReadSpatialAudio {
    /// Authored `filePath` (`asset` type). Empty when unauthored.
    pub file_path: Option<String>,
    /// Authored `auralMode` token: `"spatial"` (default) or
    /// `"nonSpatial"`.
    pub aural_mode: Option<String>,
    /// Authored `playbackMode` token: `"onceFromStart"` /
    /// `"onceFromStartToEnd"` / `"loopFromStart"` /
    /// `"loopFromStartToEnd"` / `"loopFromStage"`.
    pub playback_mode: Option<String>,
    /// `startTime`, `endTime`, `mediaOffset` are authored as
    /// `timecode`s — kept as `f64` here for the consumer's per-stage
    /// timeCodesPerSecond conversion.
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
    pub media_offset: Option<f64>,
    /// Linear gain multiplier (default 1.0).
    pub gain: Option<f64>,
}

pub fn read_spatial_audio(
    stage: &openusd::Stage,
    prim: &Path,
) -> Result<Option<ReadSpatialAudio>> {
    let file_path = read_asset_or_string(stage, prim, "filePath")?;
    let aural_mode = read_token(stage, prim, "auralMode")?;
    let playback_mode = read_token(stage, prim, "playbackMode")?;
    let start_time = read_double_or_timecode(stage, prim, "startTime")?;
    let end_time = read_double_or_timecode(stage, prim, "endTime")?;
    let media_offset = read_double_or_timecode(stage, prim, "mediaOffset")?;
    let gain = read_double_or_timecode(stage, prim, "gain")?;

    // If absolutely nothing useful was authored, treat as no spatial
    // audio so we don't spawn empty entries on every Xform.
    if file_path.is_none()
        && aural_mode.is_none()
        && playback_mode.is_none()
        && start_time.is_none()
        && end_time.is_none()
        && gain.is_none()
    {
        return Ok(None);
    }

    Ok(Some(ReadSpatialAudio {
        file_path,
        aural_mode,
        playback_mode,
        start_time,
        end_time,
        media_offset,
        gain,
    }))
}

fn attr_default_value(stage: &openusd::Stage, attr: &Path) -> Result<Option<Value>> {
    stage
        .field::<Value>(attr.clone(), "default")
        .map_err(anyhow::Error::from)
}

fn read_asset_or_string(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Option<String>> {
    let attr = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match attr_default_value(stage, &attr)? {
        Some(Value::AssetPath(s)) | Some(Value::String(s)) | Some(Value::Token(s)) => Some(s),
        _ => None,
    })
}

fn read_token(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    let attr = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match attr_default_value(stage, &attr)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_double_or_timecode(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Option<f64>> {
    let attr = prim.append_property(name).map_err(anyhow::Error::from)?;
    Ok(match attr_default_value(stage, &attr)? {
        Some(Value::Double(v)) => Some(v),
        Some(Value::Float(v)) => Some(v as f64),
        Some(Value::TimeCode(v)) => Some(v),
        _ => None,
    })
}
