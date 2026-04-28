//! `UsdClipsAPI` reader.
//!
//! Value clips let an animation-heavy stage split its time-sampled data
//! across multiple `.usd` clip files — common in film/VFX pipelines
//! where per-frame data would blow up a single layer. USD exposes this
//! via prim-level metadata; openusd parses the metadata but **doesn't
//! yet compose clip layers into the time-sample stream**. So this
//! module stops at the reader — downstream tools can choose how to
//! honour the authoring.
//!
//! Two metadata layouts exist:
//!
//! - **Legacy**: loose fields `clipAssetPaths`, `clipPrimPath`,
//!   `clipActive`, `clipTimes`, `clipManifestAssetPath` authored
//!   directly on the prim.
//! - **Modern**: a nested `clips` dictionary keyed by clip-set name,
//!   where each set carries the same fields.
//!
//! Both are captured into `ReadClipSet` records.

use anyhow::Result;
use openusd::sdf::{Path, Value};

/// A single authored clip set on a prim. Matches USD's per-set
/// metadata bundle from `UsdClipsAPI`.
#[derive(Debug, Clone, Default)]
pub struct ReadClipSet {
    /// Clip-set name. `"default"` for legacy-style prims that don't
    /// wrap their metadata in a named set.
    pub name: String,
    /// Prim path in each clip layer where the animated data lives.
    pub clip_prim_path: Option<String>,
    /// Ordered list of clip-layer asset paths.
    pub asset_paths: Vec<String>,
    /// `clipActive` — pairs of `(stage_time_code, clip_index)`
    /// describing when each clip is active on the composed timeline.
    pub active: Vec<(f64, i64)>,
    /// `clipTimes` — pairs of `(stage_time_code, clip_time_code)`
    /// remapping composed time into each clip's internal time.
    pub times: Vec<(f64, f64)>,
    /// Optional manifest layer — tells consumers which attributes are
    /// animated across the clip set.
    pub manifest_asset_path: Option<String>,
}

/// Read every clip set authored on `prim`. Covers both layouts:
///   - legacy loose fields → exposed as a single `"default"` set.
///   - modern `clips` dict → one `ReadClipSet` per key.
pub fn read_clips(stage: &openusd::Stage, prim: &Path) -> Result<Vec<ReadClipSet>> {
    let mut out = Vec::new();

    // Modern form: `clips = { ... }` dict, keyed by set name.
    if let Some(Value::Dictionary(clips)) = stage
        .field::<Value>(prim.clone(), "clips")
        .map_err(anyhow::Error::from)?
    {
        for (set_name, set_val) in clips {
            if let Value::Dictionary(set_dict) = set_val {
                out.push(clip_set_from_dict(&set_name, &set_dict));
            }
        }
    }

    // Legacy form: loose fields directly on the prim.
    let legacy = legacy_clip_set(stage, prim)?;
    if !legacy.is_empty() {
        out.push(legacy);
    }

    Ok(out)
}

fn legacy_clip_set(
    stage: &openusd::Stage,
    prim: &Path,
) -> Result<ReadClipSet> {
    let clip_prim_path = string_field(stage, prim, "clipPrimPath")?;
    let asset_paths = asset_path_vec_field(stage, prim, "clipAssetPaths")?;
    let active = f64_i64_pair_vec(stage, prim, "clipActive")?;
    let times = f64_pair_vec(stage, prim, "clipTimes")?;
    let manifest_asset_path = string_field(stage, prim, "clipManifestAssetPath")?;
    Ok(ReadClipSet {
        name: "default".to_string(),
        clip_prim_path,
        asset_paths,
        active,
        times,
        manifest_asset_path,
    })
}

impl ReadClipSet {
    /// `true` when every field is empty/unauthored — used to skip
    /// synthesising a `"default"` set from a prim that has no clip
    /// metadata at all.
    fn is_empty(&self) -> bool {
        self.clip_prim_path.is_none()
            && self.asset_paths.is_empty()
            && self.active.is_empty()
            && self.times.is_empty()
            && self.manifest_asset_path.is_none()
    }
}

// ── helpers ────────────────────────────────────────────────────────

fn clip_set_from_dict(
    name: &str,
    dict: &std::collections::HashMap<String, Value>,
) -> ReadClipSet {
    ReadClipSet {
        name: name.to_string(),
        clip_prim_path: dict_string(dict, "primPath"),
        asset_paths: dict_asset_paths(dict, "assetPaths"),
        active: dict_pair_vec_f64_i64(dict, "active"),
        times: dict_pair_vec_f64(dict, "times"),
        manifest_asset_path: dict_string(dict, "manifestAssetPath"),
    }
}

fn dict_string(
    dict: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Option<String> {
    match dict.get(key)? {
        Value::String(s) | Value::Token(s) | Value::AssetPath(s) => Some(s.clone()),
        _ => None,
    }
}

fn dict_asset_paths(
    dict: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Vec<String> {
    match dict.get(key) {
        Some(Value::StringVec(v)) | Some(Value::TokenVec(v)) => v.clone(),
        _ => Vec::new(),
    }
}

fn dict_pair_vec_f64(
    dict: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Vec<(f64, f64)> {
    match dict.get(key) {
        Some(Value::Vec2dVec(v)) => v.iter().map(|p| (p[0], p[1])).collect(),
        _ => Vec::new(),
    }
}

fn dict_pair_vec_f64_i64(
    dict: &std::collections::HashMap<String, Value>,
    key: &str,
) -> Vec<(f64, i64)> {
    match dict.get(key) {
        Some(Value::Vec2dVec(v)) => v.iter().map(|p| (p[0], p[1] as i64)).collect(),
        _ => Vec::new(),
    }
}

fn string_field(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Option<String>> {
    Ok(match stage
        .field::<Value>(prim.clone(), name)
        .map_err(anyhow::Error::from)?
    {
        Some(Value::String(s)) | Some(Value::Token(s)) | Some(Value::AssetPath(s)) => {
            Some(s)
        }
        _ => None,
    })
}

fn asset_path_vec_field(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Vec<String>> {
    Ok(match stage
        .field::<Value>(prim.clone(), name)
        .map_err(anyhow::Error::from)?
    {
        Some(Value::StringVec(v)) | Some(Value::TokenVec(v)) => v,
        _ => Vec::new(),
    })
}

fn f64_pair_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Vec<(f64, f64)>> {
    Ok(match stage
        .field::<Value>(prim.clone(), name)
        .map_err(anyhow::Error::from)?
    {
        Some(Value::Vec2dVec(v)) => v.into_iter().map(|p| (p[0], p[1])).collect(),
        _ => Vec::new(),
    })
}

fn f64_i64_pair_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> Result<Vec<(f64, i64)>> {
    Ok(match stage
        .field::<Value>(prim.clone(), name)
        .map_err(anyhow::Error::from)?
    {
        Some(Value::Vec2dVec(v)) => v.into_iter().map(|p| (p[0], p[1] as i64)).collect(),
        _ => Vec::new(),
    })
}
