//! MDL → UsdPreviewSurface conversion.
//!
//! Omniverse scenes frequently author materials via MDL shader networks
//! (`Clear_Glass.usd`, `OmniPBR`, `Heavy_Dirt_Dust`, …) that pure-OpenUSD
//! consumers can't render — the OpenUSD shading path understands
//! `UsdPreviewSurface` and nothing more. This module produces a thin
//! **override layer** that sublayers the original scene and, for each
//! Material that lacks a UsdPreviewSurface surface connection, authors a
//! new `PreviewSurface` Shader sibling with best-effort defaults
//! (colour keyed off the Material's prim name — `Glass` → blueish
//! translucent, `Metal` → gray metallic, etc.) and rewires the
//! Material's `outputs:surface` at the new shader.
//!
//! The original file is never touched. Loading the emitted
//! `*.preview.usda` composes the overrides over the source and renders
//! through the existing shade pipeline.
//!
//! Any Material that already has a working UsdPreviewSurface is left
//! alone.
//!
//! # Example
//! ```no_run
//! usd_schemas::third_party::convert::mdl_to_preview(
//!     std::path::Path::new("greenhouse.usda"),
//!     std::path::Path::new("greenhouse.preview.usda"),
//! ).unwrap();
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use openusd::sdf::{path as sdf_path, Path as SdfPath, Value};

use super::resolver::StripMetadataResolver;
use super::strip_metadata::strip_unsupported_prim_metadata;

/// Summary of what the conversion pass did.
#[derive(Debug, Clone, Default)]
pub struct ConversionReport {
    /// Total Material prims seen on the input.
    pub materials_scanned: usize,
    /// Materials that already had a `UsdPreviewSurface` surface
    /// connection — left alone.
    pub already_preview: usize,
    /// Materials for which we authored a `PreviewSurface` fallback.
    pub preview_authored: usize,
    /// Extra binding targets discovered via `material:binding` that
    /// didn't resolve to a proper Material prim with a
    /// UsdPreviewSurface (e.g. Clear_Glass.usd referenced as a raw
    /// Xform-with-Shader in Omniverse exports). Each of these gets a
    /// UsdPreviewSurface override authored at the binding target path.
    pub binding_targets_overridden: usize,
    /// Per-material best-guess colour (linear RGB) keyed off the
    /// material/target prim name; useful for asserting in tests.
    pub authored_colours: Vec<(String, [f32; 3])>,
}

/// Produce an override layer that renders MDL-only Materials as
/// UsdPreviewSurface fallbacks. Writes to `output` and returns a
/// summary.
pub fn mdl_to_preview(input: &Path, output: &Path) -> Result<ConversionReport> {
    // Real Omniverse-authored stages carry USDA metadata tokens
    // (`hide_in_stage_window`, `no_delete`) the pinned openusd parser
    // rejects. Run the same preprocess the loader does and use the
    // StripMetadataResolver so sibling references / sublayers under
    // the source dir still resolve.
    let source_dir = input
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let bytes = std::fs::read(input)
        .with_context(|| format!("mdl_to_preview: read input {}", input.display()))?;
    // Sniff the content — `.usd` can be either text USDA or binary
    // USDC. The preprocess + text-scrape fallback only applies to
    // USDA; USDC passes through untouched.
    let is_text_usda = bytes.starts_with(b"#usda")
        || input
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("usda"))
            .unwrap_or(false);
    let clean = if is_text_usda {
        strip_unsupported_prim_metadata(&bytes)
    } else {
        bytes
    };
    let is_usda = is_text_usda;
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or(if is_usda { "usda" } else { "usd" });
    let tmp = std::env::temp_dir().join(format!(
        ".usd_schemas_convert_{:016x}.{ext}",
        {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            input.hash(&mut h);
            h.finish()
        }
    ));
    std::fs::write(&tmp, &clean)
        .with_context(|| format!("mdl_to_preview: tempfile write {}", tmp.display()))?;

    let tmp_str = tmp
        .to_str()
        .context("mdl_to_preview: non-UTF-8 tempfile path")?;
    let stage = openusd::Stage::builder()
        .resolver(StripMetadataResolver::with_search_paths(vec![
            source_dir.clone(),
        ]))
        .on_error(|err| {
            eprintln!("usd_convert: composition error: {err}");
            Ok(())
        })
        .open(tmp_str)
        .map_err(|e| anyhow::anyhow!("mdl_to_preview: open input failed: {e}"))?;
    let _ = std::fs::remove_file(&tmp);

    let materials = collect_materials(&stage);
    let mut report = ConversionReport {
        materials_scanned: materials.len(),
        ..Default::default()
    };

    // Author the override layer from scratch. We use a sublayer that
    // references the original file so composition pulls the source in
    // as weaker opinions under our overrides.
    let mut out = crate::Stage::new_sublayer();
    let sublayer_ref = format!(
        "./{}",
        input.file_name().and_then(|n| n.to_str()).unwrap_or("source.usda")
    );
    out.add_sublayer(sublayer_ref);
    // Mirror the source's `defaultPrim` onto the override so the
    // loader walks only the intended subtree. Without this, loading
    // the override file (whose pseudo-root has no defaultPrim) would
    // expose every root prim — including referenced-layer roots that
    // should stay hidden under the main hierarchy.
    if let Some(default_prim) = stage.default_prim() {
        out.set_layer_metadata("defaultPrim", Value::Token(default_prim));
    }
    out.set_layer_metadata(
        "comment",
        Value::String(
            "Override layer authored by usd_schemas::third_party::convert::mdl_to_preview. \
             Replaces MDL/OmniPBR surfaces with UsdPreviewSurface fallbacks."
                .into(),
        ),
    );

    let mut already_overridden: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for mat in materials {
        if material_has_preview_surface(&stage, &mat.path) {
            report.already_preview += 1;
            continue;
        }
        let rgb = guess_colour_from_name(&mat.leaf_name);
        let opacity = guess_opacity_from_name(&mat.leaf_name);
        author_preview_override(&mut out, &mat.path, rgb, opacity)?;
        report.preview_authored += 1;
        already_overridden.insert(mat.path.as_str().to_string());
        report
            .authored_colours
            .push((mat.path.as_str().to_string(), rgb));
    }

    // Second pass: walk every geom prim's `material:binding`. When the
    // binding target isn't already a Material we've handled, author a
    // UsdPreviewSurface at that path. This catches Omniverse-style
    // scenes that reference MDL shader bundles *without* wrapping
    // them in a typed `Material` prim (e.g. `Clear_Glass.usd` under
    // `Greenhouse/Looks`).
    let mut binding_targets = collect_binding_targets(&stage);
    // Fallback #1: for USDA root layers, text-scrape for
    // `rel material:binding = </path>` lines. Catches greenhouse's
    // over-nested variant/reference subtrees.
    if binding_targets.is_empty() && is_usda {
        binding_targets = scrape_binding_targets_from_text(&clean);
    }
    // Fallback #2: scan each layer's raw AbstractData. Works for
    // both USDA and USDC root layers, and crucially catches bindings
    // authored inside USDC sublayers (Carter's `carter_main.usd` et
    // al.). We pay an extra disk read per layer but this is a one-off
    // conversion pass.
    if binding_targets.is_empty() {
        binding_targets = scan_binding_targets_across_layers(&stage, &source_dir, input);
    }
    for target in binding_targets {
        if already_overridden.contains(&target) {
            continue;
        }
        let Ok(target_path) = openusd::sdf::path(&target) else {
            continue;
        };
        if material_has_preview_surface(&stage, &target_path) {
            continue;
        }
        let leaf = target_path.name().unwrap_or("").to_string();
        let rgb = guess_colour_from_name(&leaf);
        let opacity = guess_opacity_from_name(&leaf);
        author_preview_override(&mut out, &target_path, rgb, opacity)?;
        report.binding_targets_overridden += 1;
        already_overridden.insert(target);
        report
            .authored_colours
            .push((target_path.as_str().to_string(), rgb));
    }

    out.write_usda(output)
        .with_context(|| format!("mdl_to_preview: write output {}", output.display()))?;
    Ok(report)
}

struct MaterialEntry {
    path: SdfPath,
    leaf_name: String,
}

/// Layer-walking fallback: reopen every layer the stage composed and
/// walk the raw `AbstractData` for any `<prim>.material:binding`
/// relationship spec. Catches bindings authored on over-only prims
/// (openusd's `Stage::traverse` skips those) and — unlike the
/// text-scrape fallback — works on USDC layers too. One disk read
/// per layer, paid once during conversion.
fn scan_binding_targets_across_layers(
    _stage: &openusd::Stage,
    anchor_dir: &std::path::Path,
    root_path: &std::path::Path,
) -> Vec<String> {
    use openusd::sdf::{Path as SdfP, SpecType, Value};
    use std::collections::HashSet;

    let resolver = StripMetadataResolver::with_search_paths(vec![anchor_dir.to_path_buf()]);
    let Some(root_str) = root_path.to_str() else {
        return Vec::new();
    };
    // `collect_layers` walks **every** layer reachable from the root
    // via sublayers, references, and payloads — this is the piece
    // `stage.layer_identifiers()` omits. Unresolved deps are swallowed
    // so conversion still produces output on partially-missing scenes.
    let layers = match openusd::layer::collect_layers_with_handler(
        &resolver,
        root_str,
        |_e| Ok(()),
    ) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("usd_convert: collect_layers failed: {e}");
            return Vec::new();
        }
    };
    eprintln!(
        "usd_convert: scanning {} layer(s) for material:binding",
        layers.len()
    );
    let mut out: HashSet<String> = HashSet::new();
    for layer in &layers {
        let data = &layer.data;
        let prim_paths = collect_prim_paths_abstract(&**data);
        let mut hits = 0usize;
        for prim_path in &prim_paths {
            // `<prim>.material:binding` is a Relationship spec path.
            let Ok(rel_path) = prim_path.append_property("material:binding") else {
                continue;
            };
            if !matches!(data.spec_type(&rel_path), Some(SpecType::Relationship)) {
                continue;
            }
            // Read `targetPaths`; fall back to `targetChildren` if the
            // layer encoded targets via the children-list field.
            let raw = data
                .get(&rel_path, "targetPaths")
                .ok()
                .or_else(|| data.get(&rel_path, "targetChildren").ok());
            let Some(raw) = raw else { continue };
            let paths: Vec<SdfP> = match raw.into_owned() {
                Value::PathListOp(op) => op.flatten(),
                Value::PathVec(v) => v,
                _ => continue,
            };
            for p in paths {
                out.insert(p.as_str().to_string());
                hits += 1;
            }
        }
        eprintln!(
            "  {} → {} prim spec(s), {hits} material:binding hit(s)",
            layer.identifier,
            prim_paths.len()
        );
    }
    out.into_iter().collect()
}

/// Mirror of openusd's private `collect_prim_paths` over the public
/// `AbstractData` trait. Walks `primChildren`, `variantSetChildren`,
/// and `variantChildren` — producing every prim spec path authored in
/// the layer, including overs on variant branches.
fn collect_prim_paths_abstract(data: &dyn openusd::sdf::AbstractData) -> Vec<openusd::sdf::Path> {
    use openusd::sdf::{ChildrenKey, Path as SdfP, Value};

    let mut result = Vec::new();
    let mut queue = vec![SdfP::abs_root()];
    while let Some(path) = queue.pop() {
        if !data.has_spec(&path) {
            continue;
        }
        if path != SdfP::abs_root() {
            result.push(path.clone());
        }
        if let Ok(value) = data.get(&path, ChildrenKey::PrimChildren.as_str())
            && let Value::TokenVec(children) = value.into_owned()
        {
            for name in children.iter().rev() {
                if let Ok(child) = path.append_path(name.as_str()) {
                    queue.push(child);
                }
            }
        }
        if let Ok(value) = data.get(&path, ChildrenKey::VariantSetChildren.as_str())
            && let Value::TokenVec(set_names) = value.into_owned()
        {
            for set_name in &set_names {
                let set_path = path.append_variant_selection(set_name, "");
                if let Ok(value) = data.get(&set_path, ChildrenKey::VariantChildren.as_str())
                    && let Value::TokenVec(variant_names) = value.into_owned()
                {
                    for variant_name in &variant_names {
                        queue.push(path.append_variant_selection(set_name, variant_name));
                    }
                }
            }
        }
    }
    result
}

/// Text-scrape fallback for Omniverse-style greenhouses whose
/// `material:binding` rels live inside deeply-nested over/variant
/// subtrees openusd's `Stage::traverse` doesn't yet surface. Matches
/// `rel material:binding = </absolute/path>` lines in the raw USDA.
/// Works on preprocessed bytes (stripped of unknown metadata), so the
/// paths are canonical.
fn scrape_binding_targets_from_text(bytes: &[u8]) -> Vec<String> {
    use std::collections::HashSet;
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    let mut out = HashSet::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("rel material:binding") {
            continue;
        }
        let Some((_lhs, rhs)) = trimmed.split_once('=') else {
            continue;
        };
        // Single target: `</path>`. List: `[</a>, </b>, …]`.
        for chunk in rhs.split(|c| c == '[' || c == ']' || c == ',') {
            let t = chunk.trim();
            if let Some(stripped) = t.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
                let path = stripped.trim();
                if path.starts_with('/') {
                    out.insert(path.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Walk every prim on the stage and collect the set of paths that
/// something else binds to via `material:binding`. Dedup on the way
/// so the caller doesn't author the same override N times.
fn collect_binding_targets(stage: &openusd::Stage) -> Vec<String> {
    use openusd::sdf::Path;
    use std::collections::HashSet;
    let mut out = HashSet::new();
    let _ = stage.traverse(|path: &Path| {
        let Ok(rel_path) = path.append_property("material:binding") else {
            return;
        };
        let raw = stage
            .field::<Value>(rel_path.clone(), "targetPaths")
            .ok()
            .flatten()
            .or_else(|| {
                stage
                    .field::<Value>(rel_path, "targetChildren")
                    .ok()
                    .flatten()
            });
        let paths = match raw {
            Some(Value::PathListOp(op)) => op.flatten(),
            Some(Value::PathVec(v)) => v,
            _ => return,
        };
        for p in paths {
            out.insert(p.as_str().to_string());
        }
    });
    out.into_iter().collect()
}

fn collect_materials(stage: &openusd::Stage) -> Vec<MaterialEntry> {
    use openusd::sdf::Path;
    let mut out = Vec::new();
    let _ = stage.traverse(|path: &Path| {
        let type_name = stage
            .field::<String>(path.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if type_name == "Material" {
            let leaf = path.name().unwrap_or("").to_string();
            out.push(MaterialEntry {
                path: path.clone(),
                leaf_name: leaf,
            });
        }
    });
    out
}

/// `true` when the Material's `outputs:surface.connect` targets a Shader
/// whose `info:id` is `UsdPreviewSurface` (not MDL, not
/// OmniPBR-via-MaterialX).
fn material_has_preview_surface(stage: &openusd::Stage, material: &SdfPath) -> bool {
    let Ok(attr_path) = material.append_property("outputs:surface") else {
        return false;
    };
    let connections: Vec<SdfPath> = match stage
        .field::<Value>(attr_path, "connectionPaths")
        .ok()
        .flatten()
    {
        Some(Value::PathListOp(op)) => op.flatten(),
        Some(Value::PathVec(v)) => v,
        _ => return false,
    };
    for conn in connections {
        let shader = conn.prim_path();
        let info_id = stage
            .field::<String>(
                shader.append_property("info:id").ok().unwrap_or(shader),
                "default",
            )
            .ok()
            .flatten();
        if info_id.as_deref() == Some("UsdPreviewSurface") {
            return true;
        }
    }
    false
}

/// Author an `over` chain to `material_path` in `out`, adding a
/// `PreviewSurface` child Shader typed `UsdPreviewSurface` with the
/// supplied colour, and redirecting the material's `outputs:surface`
/// connection at it.
fn author_preview_override(
    out: &mut crate::Stage,
    material_path: &SdfPath,
    rgb: [f32; 3],
    opacity: f32,
) -> Result<()> {
    out.define_over(material_path)?;

    // Shader child path: `<material>/PreviewSurface`.
    let shader_path = sdf_path(&format!("{}/PreviewSurface", material_path.as_str()))
        .map_err(anyhow::Error::from)?;
    // Define the shader prim fresh (it's new — the source file doesn't
    // have one under this name).
    out.define_prim(material_path, "PreviewSurface", "Shader")?;
    out.define_attribute(
        &shader_path,
        "info:id",
        "token",
        Value::Token("UsdPreviewSurface".into()),
        true,
    )?;
    out.define_attribute(
        &shader_path,
        "inputs:diffuseColor",
        "color3f",
        Value::Vec3f(rgb),
        false,
    )?;
    out.define_attribute(
        &shader_path,
        "inputs:roughness",
        "float",
        Value::Float(roughness_for(rgb)),
        false,
    )?;
    out.define_attribute(
        &shader_path,
        "inputs:metallic",
        "float",
        Value::Float(0.0),
        false,
    )?;
    if opacity < 0.999 {
        // Any sub-unity opacity → author both `inputs:opacity` and
        // `inputs:opacityThreshold` so UsdPreviewSurface blends the
        // material properly. The threshold of 0.0 disables alpha-mask
        // cutoff and forces full blending.
        out.define_attribute(
            &shader_path,
            "inputs:opacity",
            "float",
            Value::Float(opacity),
            false,
        )?;
        out.define_attribute(
            &shader_path,
            "inputs:opacityThreshold",
            "float",
            Value::Float(0.0),
            false,
        )?;
    }
    out.define_attribute(
        &shader_path,
        "outputs:surface",
        "token",
        Value::Token(String::new()),
        false,
    )?;

    // Redirect the Material's outputs:surface connection to our new
    // shader. `define_connection` authors `outputs:surface` with a
    // single `connectionPaths` target — since this comes from the
    // override layer it wins over whatever MDL target the source
    // authored.
    let shader_surface = sdf_path(&format!(
        "{}/PreviewSurface.outputs:surface",
        material_path.as_str()
    ))
    .map_err(anyhow::Error::from)?;
    out.define_connection(material_path, "outputs:surface", "token", shader_surface)?;

    Ok(())
}

/// Heuristic: pick a plausible linear-RGB diffuse colour from a
/// material's leaf name. Keywords beat exact matches so
/// `"Heavy_Dirt_Dust"` still triggers "dirt".
fn guess_colour_from_name(name: &str) -> [f32; 3] {
    let lower = name.to_lowercase();
    let contains = |kw: &str| lower.contains(kw);
    if contains("glass") || contains("window") {
        // Pale blue-gray translucent-look (kept opaque for simplicity).
        return [0.70, 0.80, 0.90];
    }
    if contains("dust") || contains("dirt") || contains("sand") {
        return [0.55, 0.45, 0.30];
    }
    if contains("wood") || contains("bark") || contains("log") {
        return [0.45, 0.28, 0.15];
    }
    if contains("leaf") || contains("leaves") || contains("plant") || contains("green") {
        return [0.25, 0.55, 0.20];
    }
    if contains("water") {
        return [0.20, 0.45, 0.70];
    }
    if contains("rust") {
        return [0.55, 0.25, 0.15];
    }
    if contains("metal") || contains("steel") || contains("iron") || contains("alum") {
        return [0.75, 0.75, 0.78];
    }
    if contains("plastic") {
        return [0.85, 0.85, 0.85];
    }
    if contains("concrete") || contains("stone") || contains("rock") {
        return [0.60, 0.60, 0.60];
    }
    if contains("black") {
        return [0.05, 0.05, 0.05];
    }
    if contains("white") {
        return [0.95, 0.95, 0.95];
    }
    if contains("red") {
        return [0.85, 0.15, 0.15];
    }
    if contains("blue") {
        return [0.15, 0.30, 0.85];
    }
    if contains("yellow") {
        return [0.95, 0.85, 0.20];
    }
    // Fallback: a mid-neutral beige that looks natural under daylight.
    [0.72, 0.68, 0.62]
}

/// Keyword heuristic: materials whose names suggest transparent media
/// (glass, window, water) get a sub-unity opacity so the plugin's
/// StandardMaterial blends them correctly. Defaults to fully opaque.
fn guess_opacity_from_name(name: &str) -> f32 {
    let lower = name.to_lowercase();
    let contains = |kw: &str| lower.contains(kw);
    if contains("glass") || contains("window") {
        return 0.25;
    }
    if contains("water") {
        return 0.55;
    }
    if contains("translucent") || contains("transparent") {
        return 0.35;
    }
    1.0
}

fn roughness_for(rgb: [f32; 3]) -> f32 {
    // Slightly darker diffuse → slightly shinier; this tends to look
    // right for the kinds of materials we bucket above. Clamped 0.2..0.9.
    let lum = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
    (1.0 - lum * 0.5).clamp(0.2, 0.9)
}
