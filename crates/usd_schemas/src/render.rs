//! `UsdRender` read side: `RenderSettings`, `RenderProduct`, `RenderVar`.
//!
//! These prims carry render-config metadata that DCC tools author onto
//! the stage (resolution, pixel aspect, per-product outputs, etc.).
//! The plugin doesn't actually render anything to those settings — it
//! just surfaces them so the viewer can display what was authored and
//! downstream tools (offline renderers) can pick them up.

use openusd::sdf::{Path, Value};

/// A `UsdRender.RenderSettings` prim.
#[derive(Debug, Clone)]
pub struct ReadRenderSettings {
    pub path: String,
    /// `resolution` — `int2` width × height. `None` when unauthored.
    pub resolution: Option<[i32; 2]>,
    /// `pixelAspectRatio` — defaults to 1.0.
    pub pixel_aspect_ratio: Option<f32>,
    /// `aspectRatioConformPolicy` token.
    pub aspect_ratio_conform_policy: Option<String>,
    /// `products` rel — targets `/Render/Products/<product>` prim paths.
    pub products: Vec<String>,
    /// `includedPurposes` tokens (e.g. `["default", "render"]`).
    pub included_purposes: Vec<String>,
    /// `materialBindingPurposes` tokens.
    pub material_binding_purposes: Vec<String>,
}

/// A `UsdRender.RenderProduct` prim.
#[derive(Debug, Clone)]
pub struct ReadRenderProduct {
    pub path: String,
    /// `productType` token — typically `"raster"`.
    pub product_type: Option<String>,
    /// `productName` — the output filename / URL.
    pub product_name: Option<String>,
    /// `camera` rel target path.
    pub camera: Option<String>,
    /// `orderedVars` rel — list of `RenderVar` prim paths in output order.
    pub ordered_vars: Vec<String>,
}

/// A `UsdRender.RenderVar` prim.
#[derive(Debug, Clone)]
pub struct ReadRenderVar {
    pub path: String,
    /// `dataType` token (e.g. `"color3f"`).
    pub data_type: Option<String>,
    /// `sourceName` — AOV / primvar / LPE name.
    pub source_name: Option<String>,
    /// `sourceType` token (`"raw"` / `"primvar"` / `"lpe"` / `"intrinsic"`).
    pub source_type: Option<String>,
}

pub fn read_render_settings(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadRenderSettings>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "RenderSettings" {
        return Ok(None);
    }
    let resolution = read_attr_value(stage, prim, "resolution")?.and_then(|v| match v {
        Value::Vec2i(a) => Some(a),
        _ => None,
    });
    let pixel_aspect_ratio = read_scalar_f32(stage, prim, "pixelAspectRatio")?;
    let aspect_ratio_conform_policy = read_token(stage, prim, "aspectRatioConformPolicy")?;
    let products = read_rel_targets(stage, prim, "products")?;
    let included_purposes = read_token_vec(stage, prim, "includedPurposes")?;
    let material_binding_purposes = read_token_vec(stage, prim, "materialBindingPurposes")?;
    Ok(Some(ReadRenderSettings {
        path: prim.as_str().to_string(),
        resolution,
        pixel_aspect_ratio,
        aspect_ratio_conform_policy,
        products,
        included_purposes,
        material_binding_purposes,
    }))
}

pub fn read_render_product(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadRenderProduct>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "RenderProduct" {
        return Ok(None);
    }
    let product_type = read_token(stage, prim, "productType")?;
    let product_name = read_scalar_string(stage, prim, "productName")?;
    let camera = read_rel_targets(stage, prim, "camera")?.into_iter().next();
    let ordered_vars = read_rel_targets(stage, prim, "orderedVars")?;
    Ok(Some(ReadRenderProduct {
        path: prim.as_str().to_string(),
        product_type,
        product_name,
        camera,
        ordered_vars,
    }))
}

pub fn read_render_var(
    stage: &openusd::Stage,
    prim: &Path,
) -> anyhow::Result<Option<ReadRenderVar>> {
    let type_name = stage
        .field::<String>(prim.clone(), "typeName")
        .map_err(anyhow::Error::from)?
        .unwrap_or_default();
    if type_name != "RenderVar" {
        return Ok(None);
    }
    let data_type = read_token(stage, prim, "dataType")?;
    let source_name = read_scalar_string(stage, prim, "sourceName")?;
    let source_type = read_token(stage, prim, "sourceType")?;
    Ok(Some(ReadRenderVar {
        path: prim.as_str().to_string(),
        data_type,
        source_name,
        source_type,
    }))
}

// ── helpers ─────────────────────────────────────────────────────────

fn read_attr_value(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<Value>> {
    let attr_path = prim.append_property(name).map_err(anyhow::Error::from)?;
    stage
        .field::<Value>(attr_path, "default")
        .map_err(anyhow::Error::from)
}

fn read_scalar_f32(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<f32>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Float(f)) => Some(f),
        Some(Value::Double(d)) => Some(d as f32),
        _ => None,
    })
}

fn read_scalar_string(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<String>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::String(s)) | Some(Value::Token(s)) | Some(Value::AssetPath(s)) => Some(s),
        _ => None,
    })
}

fn read_token(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Option<String>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn read_token_vec(
    stage: &openusd::Stage,
    prim: &Path,
    name: &str,
) -> anyhow::Result<Vec<String>> {
    Ok(match read_attr_value(stage, prim, name)? {
        Some(Value::TokenVec(v)) | Some(Value::StringVec(v)) => v,
        _ => Vec::new(),
    })
}

fn read_rel_targets(
    stage: &openusd::Stage,
    prim: &Path,
    rel_name: &str,
) -> anyhow::Result<Vec<String>> {
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
