//! UsdShade authoring: `Material` + `UsdPreviewSurface` + per-channel
//! `UsdUVTexture` shaders.
//!
//! We author the **PreviewMaterial interface pattern**: every shader
//! input that can be overridden (diffuseColor, opacity, roughness,
//! metallic, emissiveColor, ior, normal) is promoted up to the
//! `Material` prim as `inputs:<name>`, and the shader's own input is a
//! connection to the Material input. Downstream users can tweak a
//! material's look by overriding `Material.inputs:diffuseColor` without
//! touching the shader graph.
//!
//! - Scalar channels → `Material.inputs:X = <value>` (default value) and
//!   `Shader.inputs:X.connect = </Material.inputs:X>`.
//! - Textured channels → `Material.inputs:X.connect = </Material/Tex.outputs:rgb>`
//!   and `Shader.inputs:X.connect = </Material.inputs:X>`. The texture
//!   output flows through the Material's interface so overriding the
//!   Material input also cuts the texture off cleanly.

use openusd::sdf::{Path, Value};

use anyhow::Result;

use super::Stage;
use super::tokens::*;

pub struct MaterialSpec<'a> {
    pub diffuse_srgb: [f32; 3],
    pub opacity: f32,
    pub roughness: f32,
    pub metallic: f32,
    pub emissive_srgb: [f32; 3],
    /// Zero means "don't author `inputs:ior` at all — let the renderer use
    /// the PreviewSurface default".
    pub ior: f32,

    pub diffuse_texture: Option<&'a str>,
    pub normal_texture: Option<&'a str>,
    pub roughness_texture: Option<&'a str>,
    pub metallic_texture: Option<&'a str>,
    pub opacity_texture: Option<&'a str>,
    pub emissive_texture: Option<&'a str>,
}

impl<'a> Default for MaterialSpec<'a> {
    fn default() -> Self {
        Self {
            diffuse_srgb: [0.8, 0.8, 0.8],
            opacity: 1.0,
            roughness: 0.5,
            metallic: 0.0,
            emissive_srgb: [0.0, 0.0, 0.0],
            ior: 0.0,
            diffuse_texture: None,
            normal_texture: None,
            roughness_texture: None,
            metallic_texture: None,
            opacity_texture: None,
            emissive_texture: None,
        }
    }
}

pub fn define_preview_material(
    stage: &mut Stage,
    parent: &Path,
    name: &str,
    spec: &MaterialSpec<'_>,
) -> Result<Path> {
    let mat = stage.define_prim(parent, name, T_MATERIAL)?;
    stage.set_prim_metadata(&mat, "instanceable", Value::Bool(true))?;

    let shader = stage.define_prim(&mat, "Surface", T_SHADER)?;
    stage.define_attribute(
        &shader,
        "info:id",
        "token",
        Value::Token("UsdPreviewSurface".into()),
        true,
    )?;

    let any_texture = spec.diffuse_texture.is_some()
        || spec.normal_texture.is_some()
        || spec.roughness_texture.is_some()
        || spec.metallic_texture.is_some()
        || spec.opacity_texture.is_some()
        || spec.emissive_texture.is_some();

    // Shared `st` primvar reader used by every UsdUVTexture.
    let st_out = if any_texture {
        Some(define_st_reader(stage, &mat)?)
    } else {
        None
    };

    // Author each textured channel first so we can reference the texture's
    // output prim path from the Material interface input.
    let diffuse_linear = srgb_to_linear(spec.diffuse_srgb);
    let diffuse_tex_out = if let (Some(path), Some(st)) = (spec.diffuse_texture, st_out.as_ref()) {
        let fallback = [
            diffuse_linear[0],
            diffuse_linear[1],
            diffuse_linear[2],
            spec.opacity,
        ];
        let t = author_uv_texture(stage, &mat, "DiffuseTex", path, "sRGB", fallback, st)?;
        Some(stage.attribute_path(&t, "outputs:rgb")?)
    } else {
        None
    };
    let normal_tex_out = if let (Some(path), Some(st)) = (spec.normal_texture, st_out.as_ref()) {
        let t = author_uv_texture(
            stage,
            &mat,
            "NormalTex",
            path,
            "raw",
            [0.5, 0.5, 1.0, 1.0],
            st,
        )?;
        stage.define_attribute(
            &t,
            "inputs:scale",
            "float4",
            Value::Vec4f([2.0, 2.0, 2.0, 1.0]),
            false,
        )?;
        stage.define_attribute(
            &t,
            "inputs:bias",
            "float4",
            Value::Vec4f([-1.0, -1.0, -1.0, 0.0]),
            false,
        )?;
        Some(stage.attribute_path(&t, "outputs:rgb")?)
    } else {
        None
    };
    let roughness_tex_out =
        if let (Some(path), Some(st)) = (spec.roughness_texture, st_out.as_ref()) {
            let t = author_uv_texture(
                stage,
                &mat,
                "RoughnessTex",
                path,
                "raw",
                [spec.roughness, spec.roughness, spec.roughness, 1.0],
                st,
            )?;
            Some(stage.attribute_path(&t, "outputs:r")?)
        } else {
            None
        };
    let metallic_tex_out = if let (Some(path), Some(st)) = (spec.metallic_texture, st_out.as_ref())
    {
        let t = author_uv_texture(
            stage,
            &mat,
            "MetallicTex",
            path,
            "raw",
            [spec.metallic, spec.metallic, spec.metallic, 1.0],
            st,
        )?;
        Some(stage.attribute_path(&t, "outputs:r")?)
    } else {
        None
    };
    let opacity_tex_out = if let (Some(path), Some(st)) = (spec.opacity_texture, st_out.as_ref()) {
        let t = author_uv_texture(
            stage,
            &mat,
            "OpacityTex",
            path,
            "raw",
            [spec.opacity, spec.opacity, spec.opacity, 1.0],
            st,
        )?;
        Some(stage.attribute_path(&t, "outputs:r")?)
    } else {
        None
    };
    let emissive_linear = srgb_to_linear(spec.emissive_srgb);
    let emissive_tex_out = if let (Some(path), Some(st)) = (spec.emissive_texture, st_out.as_ref())
    {
        let fallback = [
            emissive_linear[0],
            emissive_linear[1],
            emissive_linear[2],
            1.0,
        ];
        let t = author_uv_texture(stage, &mat, "EmissiveTex", path, "sRGB", fallback, st)?;
        Some(stage.attribute_path(&t, "outputs:rgb")?)
    } else {
        None
    };

    // Now promote each input to the Material interface + wire the shader's
    // input as a connection to the Material interface.
    promote_scalar(
        stage,
        &mat,
        &shader,
        "diffuseColor",
        "color3f",
        Value::Vec3f(diffuse_linear),
        diffuse_tex_out,
    )?;
    promote_scalar(
        stage,
        &mat,
        &shader,
        "opacity",
        "float",
        Value::Float(spec.opacity),
        opacity_tex_out,
    )?;
    promote_scalar(
        stage,
        &mat,
        &shader,
        "roughness",
        "float",
        Value::Float(spec.roughness),
        roughness_tex_out,
    )?;
    promote_scalar(
        stage,
        &mat,
        &shader,
        "metallic",
        "float",
        Value::Float(spec.metallic),
        metallic_tex_out,
    )?;
    if spec.emissive_srgb != [0.0, 0.0, 0.0] || spec.emissive_texture.is_some() {
        promote_scalar(
            stage,
            &mat,
            &shader,
            "emissiveColor",
            "color3f",
            Value::Vec3f(emissive_linear),
            emissive_tex_out,
        )?;
    }
    if spec.ior > 0.0 {
        promote_scalar(
            stage,
            &mat,
            &shader,
            "ior",
            "float",
            Value::Float(spec.ior),
            None,
        )?;
    }
    if let Some(out) = normal_tex_out {
        // Normal map doesn't have a scalar fallback on PreviewSurface — it
        // flows only through the texture. Still promote to Material interface
        // for symmetry.
        let mat_attr = stage.attribute_path(&mat, "inputs:normal")?;
        stage.define_connection(&mat, "inputs:normal", "normal3f", out)?;
        stage.define_connection(&shader, "inputs:normal", "normal3f", mat_attr)?;
    }

    // Shader output + material output wiring.
    stage.define_attribute(
        &shader,
        "outputs:surface",
        "token",
        Value::Token(String::new()),
        false,
    )?;
    let shader_surface = stage.attribute_path(&shader, "outputs:surface")?;
    stage.define_connection(&mat, "outputs:surface", "token", shader_surface)?;

    Ok(mat)
}

/// Author `Material.inputs:<channel>` (scalar default *or* connection to
/// the texture output) and `Shader.inputs:<channel>.connect = </mat.inputs:<channel>>`.
fn promote_scalar(
    stage: &mut Stage,
    mat: &Path,
    shader: &Path,
    channel: &str,
    type_name: &str,
    scalar_default: Value,
    texture_out: Option<Path>,
) -> Result<()> {
    let mat_attr_name = format!("inputs:{channel}");

    if let Some(tex_out) = texture_out {
        // Material.inputs:X connects to the texture output; it carries no
        // scalar default (the texture is the value).
        stage.define_connection(mat, &mat_attr_name, type_name, tex_out)?;
    } else {
        // Material.inputs:X holds the scalar default directly.
        stage.define_attribute(mat, &mat_attr_name, type_name, scalar_default, false)?;
    }

    // Shader.inputs:X always reads from the Material interface input.
    let mat_input_path = stage.attribute_path(mat, &mat_attr_name)?;
    stage.define_connection(shader, &mat_attr_name, type_name, mat_input_path)?;
    Ok(())
}

fn define_st_reader(stage: &mut Stage, mat: &Path) -> Result<Path> {
    let st_reader = stage.define_prim(mat, "stReader", T_SHADER)?;
    stage.define_attribute(
        &st_reader,
        "info:id",
        "token",
        Value::Token("UsdPrimvarReader_float2".into()),
        true,
    )?;
    stage.define_attribute(
        &st_reader,
        "inputs:varname",
        "token",
        Value::Token("st".into()),
        false,
    )?;
    stage.define_attribute(
        &st_reader,
        "outputs:result",
        "float2",
        Value::Vec2f([0.0, 0.0]),
        false,
    )?;
    stage.attribute_path(&st_reader, "outputs:result")
}

fn author_uv_texture(
    stage: &mut Stage,
    material: &Path,
    name: &str,
    asset_path: &str,
    source_color_space: &str,
    fallback: [f32; 4],
    st_out: &Path,
) -> Result<Path> {
    let tex = stage.define_prim(material, name, T_SHADER)?;
    stage.define_attribute(
        &tex,
        "info:id",
        "token",
        Value::Token("UsdUVTexture".into()),
        true,
    )?;
    stage.define_attribute(
        &tex,
        "inputs:file",
        "asset",
        Value::AssetPath(asset_path.to_string()),
        false,
    )?;
    stage.define_attribute(
        &tex,
        "inputs:sourceColorSpace",
        "token",
        Value::Token(source_color_space.into()),
        false,
    )?;
    stage.define_attribute(
        &tex,
        "inputs:wrapS",
        "token",
        Value::Token("repeat".into()),
        false,
    )?;
    stage.define_attribute(
        &tex,
        "inputs:wrapT",
        "token",
        Value::Token("repeat".into()),
        false,
    )?;
    stage.define_attribute(
        &tex,
        "inputs:fallback",
        "float4",
        Value::Vec4f(fallback),
        false,
    )?;
    stage.define_attribute(
        &tex,
        "outputs:rgb",
        "float3",
        Value::Vec3f([0.0, 0.0, 0.0]),
        false,
    )?;
    stage.define_attribute(&tex, "outputs:r", "float", Value::Float(0.0), false)?;
    stage.define_connection(&tex, "inputs:st", "float2", st_out.clone())?;
    Ok(tex)
}

pub fn bind_material(stage: &mut Stage, prim: &Path, material: &Path) -> Result<()> {
    stage.apply_api_schemas(prim, &[API_MATERIAL_BINDING])?;
    stage.define_relationship(prim, "material:binding", vec![material.clone()])?;
    Ok(())
}

// ── Readers ──────────────────────────────────────────────────────────────
//
// Walks UsdShade authoring back into typed Rust data:
//
//  - `read_material_binding` — consume `material:binding` on a geom prim
//    and return the bound Material prim path.
//  - `read_preview_material` — given a Material prim, resolve its surface
//    shader and harvest every `UsdPreviewSurface` input, following
//    connections through the PreviewMaterial interface pattern to the
//    underlying scalar defaults or `UsdUVTexture` sources.

/// Decoded UsdPreviewSurface material. Every channel is either `None` (not
/// authored — let the renderer use its default), a scalar, or a texture
/// asset path (caller resolves via Bevy's AssetServer).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadPreviewMaterial {
    pub diffuse_color: Option<[f32; 3]>,
    pub opacity: Option<f32>,
    pub opacity_threshold: Option<f32>,
    pub roughness: Option<f32>,
    pub metallic: Option<f32>,
    pub emissive_color: Option<[f32; 3]>,
    pub ior: Option<f32>,

    pub diffuse_texture: Option<String>,
    pub normal_texture: Option<String>,
    pub roughness_texture: Option<String>,
    pub metallic_texture: Option<String>,
    pub opacity_texture: Option<String>,
    pub emissive_texture: Option<String>,
    pub occlusion_texture: Option<String>,
}

/// Read `material:binding` on a geom prim and return the bound Material prim
/// path. `None` if no binding is authored.
pub fn read_material_binding(stage: &openusd::Stage, prim: &Path) -> Result<Option<Path>> {
    let rel_path = prim
        .append_property("material:binding")
        .map_err(anyhow::Error::from)?;
    Ok(read_path_list(stage, &rel_path, "targetPaths")?
        .into_iter()
        .next())
}

/// Read a `Material` prim and return its decoded surface inputs as a
/// `ReadPreviewMaterial`.
///
/// Recognised shader dialects:
/// - `UsdPreviewSurface` — the native OpenUSD shader. Inputs map
///   straight across.
/// - `ND_UsdPreviewSurface_surfaceshader` — MaterialX wrapper around
///   UsdPreviewSurface. Same input names; treated identically.
/// - `ND_standard_surface_surfaceshader` — MaterialX's reference
///   standard_surface. Inputs use different names (`base_color`,
///   `metalness`, `specular_roughness`, etc.) so they're mapped to the
///   UsdPreviewSurface equivalents below.
///
/// For MaterialX materials the surface connection is usually authored
/// on `outputs:mtlx:surface` instead of `outputs:surface` — we fall
/// back to it automatically.
///
/// Returns `None` when no surface shader is connected or the shader's
/// dialect is unrecognised.
pub fn read_preview_material(
    stage: &openusd::Stage,
    material: &Path,
) -> Result<Option<ReadPreviewMaterial>> {
    let Some(shader) = resolve_surface_shader(stage, material)? else {
        return Ok(None);
    };

    // Dispatch by `info:id` so we pick the right input-name mapping.
    let shader_id = read_scalar_token(stage, &shader, "info:id")?;
    let channels: &[(&str, ColourSetter, ScalarSetter, TextureSetter)] = match shader_id
        .as_deref()
    {
        Some("UsdPreviewSurface") | Some("ND_UsdPreviewSurface_surfaceshader") => {
            PREVIEW_CHANNELS
        }
        Some("ND_standard_surface_surfaceshader") => MATERIALX_STD_SURFACE_CHANNELS,
        Some("OmniPBR") => OMNIPBR_CHANNELS,
        Some("OmniSurface") | Some("OmniSurfaceLite") => OMNISURFACE_CHANNELS,
        _ => return Ok(None),
    };

    let mut out = ReadPreviewMaterial::default();
    for (channel, bind_colour, bind_scalar, bind_texture) in channels {
        let (value, texture) = resolve_channel(stage, material, &shader, channel)?;
        if let Some(tex) = texture {
            bind_texture(&mut out, tex);
        }
        match value {
            Some(ResolvedValue::Color3(c)) => bind_colour(&mut out, c),
            Some(ResolvedValue::Scalar(s)) => bind_scalar(&mut out, s),
            None => {}
        }
    }
    Ok(Some(out))
}

/// Follow the Material's surface connection. Tries `outputs:surface`
/// (native OpenUSD), then `outputs:mtlx:surface` (MaterialX), then
/// `outputs:mdl:surface` (NVIDIA Omniverse MDL pipeline). The
/// last is what Isaac Sim, Kit and other Omniverse-authored stages
/// emit; without recognising it our material reader couldn't see
/// any of those scenes' shaders.
fn resolve_surface_shader(
    stage: &openusd::Stage,
    material: &Path,
) -> Result<Option<Path>> {
    for attr_name in ["outputs:surface", "outputs:mtlx:surface", "outputs:mdl:surface"] {
        let attr_path = material
            .append_property(attr_name)
            .map_err(anyhow::Error::from)?;
        let targets = read_path_list(stage, &attr_path, "connectionPaths")?;
        if let Some(t) = targets.into_iter().next() {
            return Ok(Some(t.prim_path()));
        }
    }
    Ok(None)
}

/// For each channel: (token, colour-setter, scalar-setter, texture-setter).
/// `None` setters mean "channel doesn't accept this kind of value".
type ColourSetter = fn(&mut ReadPreviewMaterial, [f32; 3]);
type ScalarSetter = fn(&mut ReadPreviewMaterial, f32);
type TextureSetter = fn(&mut ReadPreviewMaterial, String);

fn set_diffuse_c(o: &mut ReadPreviewMaterial, c: [f32; 3]) {
    o.diffuse_color = Some(c);
}
fn set_diffuse_s(_: &mut ReadPreviewMaterial, _: f32) {}
fn set_diffuse_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.diffuse_texture = Some(s);
}
fn set_opacity_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_opacity_s(o: &mut ReadPreviewMaterial, s: f32) {
    o.opacity = Some(s);
}
fn set_opacity_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.opacity_texture = Some(s);
}
fn set_opacity_threshold_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_opacity_threshold_s(o: &mut ReadPreviewMaterial, s: f32) {
    o.opacity_threshold = Some(s);
}
fn set_opacity_threshold_tex(_: &mut ReadPreviewMaterial, _: String) {}
fn set_rough_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_rough_s(o: &mut ReadPreviewMaterial, s: f32) {
    o.roughness = Some(s);
}
fn set_rough_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.roughness_texture = Some(s);
}
fn set_metal_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_metal_s(o: &mut ReadPreviewMaterial, s: f32) {
    o.metallic = Some(s);
}
fn set_metal_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.metallic_texture = Some(s);
}
fn set_emissive_c(o: &mut ReadPreviewMaterial, c: [f32; 3]) {
    o.emissive_color = Some(c);
}
fn set_emissive_s(_: &mut ReadPreviewMaterial, _: f32) {}
fn set_emissive_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.emissive_texture = Some(s);
}
fn set_ior_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_ior_s(o: &mut ReadPreviewMaterial, s: f32) {
    o.ior = Some(s);
}
fn set_ior_tex(_: &mut ReadPreviewMaterial, _: String) {}
fn set_normal_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_normal_s(_: &mut ReadPreviewMaterial, _: f32) {}
fn set_normal_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.normal_texture = Some(s);
}
fn set_occlusion_c(_: &mut ReadPreviewMaterial, _: [f32; 3]) {}
fn set_occlusion_s(_: &mut ReadPreviewMaterial, _: f32) {}
fn set_occlusion_tex(o: &mut ReadPreviewMaterial, s: String) {
    o.occlusion_texture = Some(s);
}

const PREVIEW_CHANNELS: &[(&str, ColourSetter, ScalarSetter, TextureSetter)] = &[
    (
        "diffuseColor",
        set_diffuse_c,
        set_diffuse_s,
        set_diffuse_tex,
    ),
    ("opacity", set_opacity_c, set_opacity_s, set_opacity_tex),
    (
        "opacityThreshold",
        set_opacity_threshold_c,
        set_opacity_threshold_s,
        set_opacity_threshold_tex,
    ),
    ("roughness", set_rough_c, set_rough_s, set_rough_tex),
    ("metallic", set_metal_c, set_metal_s, set_metal_tex),
    (
        "emissiveColor",
        set_emissive_c,
        set_emissive_s,
        set_emissive_tex,
    ),
    ("ior", set_ior_c, set_ior_s, set_ior_tex),
    ("normal", set_normal_c, set_normal_s, set_normal_tex),
    (
        "occlusion",
        set_occlusion_c,
        set_occlusion_s,
        set_occlusion_tex,
    ),
];

/// MaterialX `standard_surface` uses different input names than
/// UsdPreviewSurface. Map the subset we can translate cleanly:
///
/// | MaterialX input     | UsdPreviewSurface equivalent |
/// |---------------------|------------------------------|
/// | `base_color`        | `diffuseColor` (base × base_color) |
/// | `metalness`         | `metallic`                   |
/// | `specular_roughness`| `roughness`                  |
/// | `emission_color`    | `emissiveColor`              |
/// | `opacity` (color3)  | `opacity` (we take the first channel) |
/// | `normal`            | `normal`                     |
///
/// `base` (a scalar brightness multiplier), `specular_IOR`, coat
/// terms, anisotropy, sheen and subsurface are MaterialX-only and
/// don't have a clean UsdPreviewSurface landing — they get dropped
/// silently. The mapped subset is what the open-standard viewers all
/// agree on.
const MATERIALX_STD_SURFACE_CHANNELS: &[(&str, ColourSetter, ScalarSetter, TextureSetter)] = &[
    (
        "base_color",
        set_diffuse_c,
        set_diffuse_s,
        set_diffuse_tex,
    ),
    (
        "metalness",
        set_metal_c,
        set_metal_s,
        set_metal_tex,
    ),
    (
        "specular_roughness",
        set_rough_c,
        set_rough_s,
        set_rough_tex,
    ),
    (
        "emission_color",
        set_emissive_c,
        set_emissive_s,
        set_emissive_tex,
    ),
    (
        "opacity",
        set_opacity_mtlx_c,
        set_opacity_s,
        set_opacity_tex,
    ),
    ("normal", set_normal_c, set_normal_s, set_normal_tex),
];

/// MaterialX authors `opacity` as a `color3` (one channel per RGB
/// transparency). UsdPreviewSurface wants a scalar — fold the channels
/// into a luminance-weighted average so the result still looks right
/// for the common "uniform opacity" case.
fn set_opacity_mtlx_c(o: &mut ReadPreviewMaterial, c: [f32; 3]) {
    let lum = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    o.opacity = Some(lum);
}

/// Omniverse `OmniPBR` shader inputs. Maps the MDL/OmniPBR input
/// names to UsdPreviewSurface equivalents:
///
/// | OmniPBR input                      | UsdPreviewSurface  |
/// |------------------------------------|--------------------|
/// | `diffuse_color_constant`           | `diffuseColor`     |
/// | `diffuse_texture`                  | diffuse texture    |
/// | `reflection_roughness_constant`    | `roughness`        |
/// | `reflectionroughness_texture`      | roughness texture  |
/// | `metallic_constant`                | `metallic`         |
/// | `metallic_texture`                 | metallic texture   |
/// | `emissive_color`                   | `emissiveColor`    |
/// | `emissive_color_texture`           | emissive texture   |
/// | `opacity_constant`                 | `opacity`          |
/// | `opacity_texture`                  | opacity texture    |
/// | `normalmap_texture`                | normal texture     |
const OMNIPBR_CHANNELS: &[(&str, ColourSetter, ScalarSetter, TextureSetter)] = &[
    ("diffuse_color_constant", set_diffuse_c, set_diffuse_s, set_diffuse_tex),
    ("diffuse_texture", set_diffuse_c, set_diffuse_s, set_diffuse_tex),
    ("reflection_roughness_constant", set_rough_c, set_rough_s, set_rough_tex),
    ("reflectionroughness_texture", set_rough_c, set_rough_s, set_rough_tex),
    ("metallic_constant", set_metal_c, set_metal_s, set_metal_tex),
    ("metallic_texture", set_metal_c, set_metal_s, set_metal_tex),
    ("emissive_color", set_emissive_c, set_emissive_s, set_emissive_tex),
    ("emissive_color_texture", set_emissive_c, set_emissive_s, set_emissive_tex),
    ("opacity_constant", set_opacity_c, set_opacity_s, set_opacity_tex),
    ("opacity_texture", set_opacity_c, set_opacity_s, set_opacity_tex),
    ("normalmap_texture", set_normal_c, set_normal_s, set_normal_tex),
];

/// Omniverse `OmniSurface` (and `OmniSurfaceLite`) shader inputs.
/// Different vocabulary than OmniPBR — closer to MaterialX
/// standard_surface but with `_image` suffixes for textures.
const OMNISURFACE_CHANNELS: &[(&str, ColourSetter, ScalarSetter, TextureSetter)] = &[
    ("diffuse_reflection_color", set_diffuse_c, set_diffuse_s, set_diffuse_tex),
    ("diffuse_reflection_color_image", set_diffuse_c, set_diffuse_s, set_diffuse_tex),
    ("geometry_normal_image", set_normal_c, set_normal_s, set_normal_tex),
    ("geometry_opacity_image", set_opacity_c, set_opacity_s, set_opacity_tex),
    ("geometry_opacity", set_opacity_c, set_opacity_s, set_opacity_tex),
    ("roughness", set_rough_c, set_rough_s, set_rough_tex),
    ("metalness", set_metal_c, set_metal_s, set_metal_tex),
    ("emission_color", set_emissive_c, set_emissive_s, set_emissive_tex),
    ("emission_color_image", set_emissive_c, set_emissive_s, set_emissive_tex),
];

/// Authored value as resolved through the interface pattern.
enum ResolvedValue {
    Color3([f32; 3]),
    Scalar(f32),
}

/// Resolve one channel. Returns `(value, texture_path)` — either half may be
/// `None`. Follows this chain, stopping at the first useful value:
///
/// 1. `<material>.inputs:<channel>` — interface pattern's "most authoritative"
///    slot. Scalar default ⇒ value. Connection to `UsdUVTexture` ⇒ texture.
///    Connection to anything else (e.g. a `PrimvarReader`) is ignored for M3.
/// 2. `<shader>.inputs:<channel>` — older pattern with inputs authored on the
///    shader directly. Same scalar/texture discrimination.
fn resolve_channel(
    stage: &openusd::Stage,
    material: &Path,
    shader: &Path,
    channel: &str,
) -> Result<(Option<ResolvedValue>, Option<String>)> {
    let mat_attr = format!("inputs:{channel}");
    let mat_path = material
        .append_property(&mat_attr)
        .map_err(anyhow::Error::from)?;
    let (v, t) = resolve_attr_chain(stage, &mat_path)?;
    if v.is_some() || t.is_some() {
        return Ok((v, t));
    }
    let sh_path = shader
        .append_property(&mat_attr)
        .map_err(anyhow::Error::from)?;
    resolve_attr_chain(stage, &sh_path)
}

/// Read an attribute: if it has `connectionPaths`, follow them recursively;
/// otherwise return its `default` value.
///
/// Recognised terminal / pass-through nodes (M17 + MaterialX-rich
/// extension):
///
/// - **`UsdUVTexture`** + **`ND_image_*`** — terminal. Extract the
///   asset path from `inputs:file`.
/// - **`ND_normalmap`** — pass-through. Walk into `inputs:in` (the
///   normal-map texture lives there).
/// - **`ND_constant_*`** — terminal. Return `inputs:value`'s default
///   as a resolved value.
/// - **`ND_multiply_*`** / **`ND_mix_*`** / **`ND_add_*`** /
///   **`ND_subtract_*`** — pass-through to the principal input
///   (`in1` for multiply/add/subtract, `fg` for mix). We don't try
///   to model the operation — we just chase the texture.
/// - **Anything else** — keep walking the connection.
fn resolve_attr_chain(
    stage: &openusd::Stage,
    attr_path: &Path,
) -> Result<(Option<ResolvedValue>, Option<String>)> {
    // Follow connections up to 16 hops to handle deeper MaterialX
    // graphs (image → normalmap → standard_surface input is 3 alone).
    let mut cur = attr_path.clone();
    for _ in 0..16 {
        let connections = read_path_list(stage, &cur, "connectionPaths")?;
        if let Some(next) = connections.into_iter().next() {
            let prim = next.prim_path();
            match shader_kind(stage, &prim)? {
                ShaderKind::Texture => {
                    let file = read_texture_file(stage, &prim)?;
                    return Ok((None, file));
                }
                ShaderKind::NormalMap => {
                    cur = prim
                        .append_property("inputs:in")
                        .map_err(anyhow::Error::from)?;
                    continue;
                }
                ShaderKind::Constant => {
                    let v_path = prim
                        .append_property("inputs:value")
                        .map_err(anyhow::Error::from)?;
                    let v = attr_default_value(stage, &v_path)?;
                    return Ok((v.and_then(value_to_preview), None));
                }
                ShaderKind::Multiply | ShaderKind::AddOrSubtract => {
                    cur = prim
                        .append_property("inputs:in1")
                        .map_err(anyhow::Error::from)?;
                    continue;
                }
                ShaderKind::Mix => {
                    cur = prim
                        .append_property("inputs:fg")
                        .map_err(anyhow::Error::from)?;
                    continue;
                }
                ShaderKind::Unknown => {
                    cur = next;
                    continue;
                }
            }
        }
        // No connection — is there a default value?
        let default = attr_default_value(stage, &cur)?;
        // Asset-path / string defaults are common on Omniverse MDL
        // shaders (`OmniPBR.inputs:diffuse_texture = @file.png@`,
        // `OmniSurface.inputs:diffuse_reflection_color_image = @...@`),
        // where the texture is bound DIRECTLY on the input rather
        // than through a `UsdUVTexture` intermediate. Surface those
        // as textures so the material reader picks them up.
        if let Some(Value::AssetPath(s) | Value::String(s)) = default.clone() {
            return Ok((None, Some(s)));
        }
        let resolved = default.and_then(value_to_preview);
        return Ok((resolved, None));
    }
    Ok((None, None))
}

/// Classify the shader at `prim` so `resolve_attr_chain` knows
/// whether to terminate (texture / constant), walk a specific child
/// input (normalmap / multiply / mix / add), or just chase the
/// connection.
enum ShaderKind {
    /// Terminal — texture asset readable from `inputs:file`.
    Texture,
    /// Pass-through — descend into `inputs:in`.
    NormalMap,
    /// Terminal — the value lives at `inputs:value`'s default.
    Constant,
    /// Pass-through — descend into `inputs:in1`.
    Multiply,
    /// Pass-through — descend into `inputs:in1`.
    AddOrSubtract,
    /// Pass-through — descend into `inputs:fg`.
    Mix,
    /// Treat as a connection chain link; keep walking.
    Unknown,
}

fn shader_kind(stage: &openusd::Stage, prim: &Path) -> Result<ShaderKind> {
    let id = read_scalar_token(stage, prim, "info:id")?;
    Ok(match id.as_deref() {
        Some("UsdUVTexture") => ShaderKind::Texture,
        // MaterialX `ND_image_color3`, `ND_image_color4`,
        // `ND_image_vector3`, `ND_image_float`, etc.
        Some(s) if s.starts_with("ND_image_") => ShaderKind::Texture,
        Some("ND_normalmap") => ShaderKind::NormalMap,
        Some(s) if s.starts_with("ND_constant_") => ShaderKind::Constant,
        Some(s) if s.starts_with("ND_multiply_") => ShaderKind::Multiply,
        Some(s) if s.starts_with("ND_add_") || s.starts_with("ND_subtract_") => {
            ShaderKind::AddOrSubtract
        }
        Some(s) if s.starts_with("ND_mix_") => ShaderKind::Mix,
        _ => ShaderKind::Unknown,
    })
}

fn read_texture_file(stage: &openusd::Stage, tex_prim: &Path) -> Result<Option<String>> {
    let path = tex_prim
        .append_property("inputs:file")
        .map_err(anyhow::Error::from)?;
    match attr_default_value(stage, &path)? {
        Some(Value::AssetPath(s)) | Some(Value::String(s)) | Some(Value::Token(s)) => Ok(Some(s)),
        _ => Ok(None),
    }
}

fn value_to_preview(v: Value) -> Option<ResolvedValue> {
    match v {
        Value::Float(f) => Some(ResolvedValue::Scalar(f)),
        Value::Double(d) => Some(ResolvedValue::Scalar(d as f32)),
        Value::Vec3f(c) => Some(ResolvedValue::Color3(c)),
        Value::Vec3d(c) => Some(ResolvedValue::Color3([
            c[0] as f32,
            c[1] as f32,
            c[2] as f32,
        ])),
        _ => None,
    }
}

fn attr_default_value(stage: &openusd::Stage, attr: &Path) -> Result<Option<Value>> {
    stage
        .field::<Value>(attr.clone(), "default")
        .map_err(anyhow::Error::from)
}

fn read_path_list(stage: &openusd::Stage, attr: &Path, field: &str) -> Result<Vec<Path>> {
    match stage
        .field::<Value>(attr.clone(), field)
        .map_err(anyhow::Error::from)?
    {
        Some(Value::PathListOp(op)) => Ok(op.flatten()),
        Some(Value::PathVec(v)) => Ok(v),
        _ => Ok(Vec::new()),
    }
}

fn read_scalar_token(stage: &openusd::Stage, prim: &Path, attr: &str) -> Result<Option<String>> {
    let attr_path = prim.append_property(attr).map_err(anyhow::Error::from)?;
    Ok(match attr_default_value(stage, &attr_path)? {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2])]
}
