//! `UsdUI` schemas — purely cosmetic metadata that authoring tools
//! use to label outliners and lay out shading-network editors.
//!
//! What we surface today:
//!
//! - **`ui:displayName`** (`UsdUISceneGraphPrimAPI`) — friendly label
//!   displayed in the viewer's prim tree instead of the prim leaf
//!   name. The single most useful piece of UsdUI for downstream
//!   viewer UX.
//! - **`ui:displayGroup`** — grouping token; surfaced via
//!   [`read_display_group`] for consumers that want to bin prims
//!   into folders.
//!
//! Not yet surfaced:
//!
//! - `UsdUINodeGraphNodeAPI` (node-editor layout / colour / icon) —
//!   only meaningful once we ship a shader-network editor panel.
//! - `UsdUIBackdrop` — visual grouping of node-graph nodes; same
//!   prerequisite.

use anyhow::Result;
use openusd::sdf::{Path, Value};

/// Read `ui:displayName` for the given prim. Returns `None` when the
/// attribute is unauthored or its default is missing.
pub fn read_display_name(stage: &openusd::Stage, prim: &Path) -> Result<Option<String>> {
    read_token_or_string(stage, prim, "ui:displayName")
}

/// Read `ui:displayGroup` — a token that downstream tools use to
/// group prims under named folders in their outliner.
pub fn read_display_group(stage: &openusd::Stage, prim: &Path) -> Result<Option<String>> {
    read_token_or_string(stage, prim, "ui:displayGroup")
}

fn read_token_or_string(
    stage: &openusd::Stage,
    prim: &Path,
    attr_name: &str,
) -> Result<Option<String>> {
    let attr = prim
        .append_property(attr_name)
        .map_err(anyhow::Error::from)?;
    let v = stage
        .field::<Value>(attr, "default")
        .map_err(anyhow::Error::from)?;
    Ok(match v {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}
