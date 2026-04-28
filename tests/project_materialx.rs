//! M17 integration test: `usd_schemas::shade::read_preview_material`
//! handles MaterialX dialects (ND_UsdPreviewSurface_surfaceshader and
//! ND_standard_surface_surfaceshader) in addition to native
//! UsdPreviewSurface. Fixture authors all three variants on one stage.

use openusd::sdf::Path;
use usd_schemas::shade::read_preview_material;

#[test]
fn reads_native_preview_materialx_wrapper_and_standard_surface() {
    let stage = openusd::Stage::open("tests/stages/materialx.usda")
        .expect("stage should open");

    // Native UsdPreviewSurface.
    let native = read_preview_material(
        &stage,
        &Path::new("/World/Materials/Native").expect("valid path"),
    )
    .expect("read ok")
    .expect("native should decode");
    println!(
        "\n---- Native UsdPreviewSurface ----\n  \
         diffuse={:?} roughness={:?} metallic={:?}",
        native.diffuse_color, native.roughness, native.metallic
    );
    assert_eq!(native.diffuse_color, Some([0.9, 0.1, 0.1]));
    assert_eq!(native.roughness, Some(0.4));
    assert_eq!(native.metallic, Some(0.0));

    // MaterialX wrapper (ND_UsdPreviewSurface_surfaceshader).
    let wrapped = read_preview_material(
        &stage,
        &Path::new("/World/Materials/MtlxWrapped").expect("valid path"),
    )
    .expect("read ok")
    .expect("wrapped should decode");
    println!(
        "---- MaterialX-wrapped UsdPreviewSurface ----\n  \
         diffuse={:?} roughness={:?} metallic={:?}",
        wrapped.diffuse_color, wrapped.roughness, wrapped.metallic
    );
    assert_eq!(wrapped.diffuse_color, Some([0.1, 0.8, 0.2]));
    assert_eq!(wrapped.roughness, Some(0.25));
    assert_eq!(wrapped.metallic, Some(0.0));

    // MaterialX standard_surface — input names remapped.
    let std = read_preview_material(
        &stage,
        &Path::new("/World/Materials/MtlxStandard").expect("valid path"),
    )
    .expect("read ok")
    .expect("standard_surface should decode");
    println!(
        "---- MaterialX standard_surface ----\n  \
         diffuse={:?} roughness={:?} metallic={:?} emissive={:?} opacity={:?}",
        std.diffuse_color, std.roughness, std.metallic, std.emissive_color, std.opacity
    );
    assert_eq!(std.diffuse_color, Some([0.1, 0.25, 0.85]));
    // specular_roughness → roughness
    assert_eq!(std.roughness, Some(0.15));
    // metalness → metallic
    assert_eq!(std.metallic, Some(0.9));
    // emission_color → emissiveColor
    assert_eq!(std.emissive_color, Some([0.0, 0.0, 0.5]));
    // opacity folded luminance-weighted; (0.4, 0.4, 0.4) → 0.4
    let op = std.opacity.expect("opacity should decode");
    assert!((op - 0.4).abs() < 1e-4, "opacity fold got {op}");
}
