//! UsdPreviewSurface → `bevy::pbr::StandardMaterial`.
//!
//! `usd_schemas::shade::read_preview_material` extracts the authored inputs
//! from the Material prim + its surface Shader; this module turns those
//! inputs into a fully-textured Bevy material. Textures are resolved via
//! [`crate::texture::load_texture`] so colour space is correct per channel.
//!
//! Bevy and USD disagree on some conventions:
//!
//! - `diffuseColor` in USD is linear RGB; `StandardMaterial::base_color`
//!   takes `Color`. We go through `LinearRgba::rgb` so no gamma is reapplied.
//! - `normal` textures in UsdPreviewSurface sample `[0, 1]` and are remapped
//!   to `[-1, 1]` with `inputs:scale = (2,2,2,1)` + `inputs:bias = (-1,-1,-1,0)`.
//!   Bevy expects that remap *inside* the texture (tangent-space normal map),
//!   so we just hand the raw linear image through — the authored scale/bias
//!   is the renderer's responsibility under UsdPreviewSurface semantics.
//! - `opacity` < 1.0 or textured → `AlphaMode::Blend`; `opacityThreshold` > 0
//!   switches to `AlphaMode::Mask(threshold)`.

use bevy::asset::{Handle, LoadContext};
use bevy::color::{Color, LinearRgba};
use bevy::pbr::StandardMaterial;
use usd_schemas::shade::ReadPreviewMaterial;

use crate::build::BuildCtx;
use crate::texture::{TextureChannel, load_texture};

/// Build a Bevy `StandardMaterial` from a decoded UsdPreviewSurface.
///
/// Calls `lc.loader()` to register every referenced texture as a dependent
/// asset; the returned `StandardMaterial` is then registered separately by
/// the caller via `lc.add_labeled_asset`.
pub fn standard_material_from_usd(
    ctx: &mut BuildCtx<'_, '_>,
    read: &ReadPreviewMaterial,
) -> StandardMaterial {
    let mut mat = StandardMaterial {
        base_color: Color::linear_rgb(0.8, 0.8, 0.8),
        perceptual_roughness: 0.5,
        metallic: 0.0,
        ..Default::default()
    };

    if let Some([r, g, b]) = read.diffuse_color {
        mat.base_color = Color::LinearRgba(LinearRgba::rgb(r, g, b));
    }
    if let Some(r) = read.roughness {
        mat.perceptual_roughness = r;
    }
    if let Some(m) = read.metallic {
        mat.metallic = m;
    }
    if let Some([r, g, b]) = read.emissive_color {
        mat.emissive = LinearRgba::rgb(r, g, b);
    }
    if let Some(ior) = read.ior {
        mat.ior = ior;
    }

    // Texture maps. Colour space flows from the channel kind, not from the
    // USD-authored `sourceColorSpace` token — we trust the M3 convention
    // that diffuse/emissive are sRGB and the rest are linear. (M3.1 can
    // read `inputs:sourceColorSpace` directly if authoring gets sloppy.)
    if let Some(path) = read.diffuse_texture.as_deref() {
        mat.base_color_texture = load_texture(ctx, path, TextureChannel::Srgb);
    }
    if let Some(path) = read.normal_texture.as_deref() {
        mat.normal_map_texture = load_texture(ctx, path, TextureChannel::Linear);
    }
    if let Some(path) = read.occlusion_texture.as_deref() {
        mat.occlusion_texture = load_texture(ctx, path, TextureChannel::Linear);
    }
    if let Some(path) = read.emissive_texture.as_deref() {
        mat.emissive_texture = load_texture(ctx, path, TextureChannel::Srgb);
    }
    // Roughness + metallic bind to the same combined texture slot.
    // UsdPreviewSurface allows them separate; Bevy's `StandardMaterial`
    // packs roughness (G) + metallic (B) into `metallic_roughness_texture`.
    // When only one is authored, Bevy still samples from the texture's
    // matching channel — we just hand the same image to both expectations.
    // metallic + roughness packing. Bevy's `metallic_roughness_texture`
    // expects a SINGLE RGBA texture with G = roughness, B = metallic
    // (glTF spec). USD authors them as TWO independent texture assets.
    // When both are authored, composite into one packed image so neither
    // side gets dropped. When only one is authored, fall through to the
    // "shove the single channel into the slot" cheap path — Bevy will
    // sample the right channel at shade time.
    match (
        read.metallic_texture.as_deref(),
        read.roughness_texture.as_deref(),
    ) {
        (Some(m_path), Some(r_path)) if m_path != r_path => {
            mat.metallic_roughness_texture =
                crate::texture::load_metallic_roughness_packed(ctx, r_path, m_path);
        }
        (Some(path), _) | (_, Some(path)) => {
            mat.metallic_roughness_texture = load_texture(ctx, path, TextureChannel::Linear);
        }
        (None, None) => {}
    }

    // Alpha. `opacityThreshold` wins over `opacity` (UsdPreviewSurface says
    // the threshold kicks the shader into opaque-mask mode).
    use bevy::render::alpha::AlphaMode;
    if let Some(thr) = read.opacity_threshold.filter(|t| *t > 0.0) {
        mat.alpha_mode = AlphaMode::Mask(thr);
    } else if read.opacity.map(|o| o < 1.0).unwrap_or(false) || read.opacity_texture.is_some() {
        mat.alpha_mode = AlphaMode::Blend;
        if let Some(o) = read.opacity {
            let LinearRgba {
                red, green, blue, ..
            } = mat.base_color.into();
            mat.base_color = Color::LinearRgba(LinearRgba {
                red,
                green,
                blue,
                alpha: o,
            });
        }
    }

    mat
}

/// Small helper to let callers construct a default "Material:Default"
/// StandardMaterial without going through UsdShade at all.
pub fn default_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::srgb(0.72, 0.72, 0.75),
        perceptual_roughness: 0.8,
        metallic: 0.0,
        ..Default::default()
    }
}

/// Register a `StandardMaterial` as a labeled sub-asset under
/// `"Material:<prim_path>"`. Returns the handle.
pub fn add_material_labeled(
    lc: &mut LoadContext<'_>,
    prim_path: &str,
    mat: StandardMaterial,
) -> Handle<StandardMaterial> {
    lc.add_labeled_asset(format!("Material:{prim_path}"), mat)
}
