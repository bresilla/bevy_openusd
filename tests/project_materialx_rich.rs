//! MaterialX-rich integration test: graph traversal beyond
//! standard_surface and UsdPreviewSurface. Asserts that
//! `usd_schemas::shade::read_preview_material` follows connections
//! through `ND_image_*`, `ND_normalmap`, `ND_multiply_*`, and
//! `ND_constant_*` to reach the texture / scalar at the leaf.

use openusd::sdf::Path;
use usd_schemas::shade::read_preview_material;

#[test]
fn rich_network_resolves_textures_and_constants() {
    let stage =
        openusd::Stage::open("tests/stages/materialx_rich.usda").expect("stage should open");

    let m = read_preview_material(
        &stage,
        &Path::new("/World/Materials/Rich").expect("valid path"),
    )
    .expect("read ok")
    .expect("rich material should decode");

    println!(
        "\n---- Rich MaterialX ----\n  \
         diffuse_color={:?} diffuse_texture={:?}\n  \
         metallic={:?} metallic_texture={:?}\n  \
         normal_texture={:?}",
        m.diffuse_color,
        m.diffuse_texture,
        m.metallic,
        m.metallic_texture,
        m.normal_texture,
    );

    // base_color goes: standard_surface.base_color
    //   ← ND_multiply_color3.out (pass-through to in1)
    //   ← ND_image_color3.out (terminal, file = diffuse.png)
    assert_eq!(m.diffuse_texture.as_deref(), Some("textures/diffuse.png"));

    // metalness goes: standard_surface.metalness
    //   ← ND_constant_float.out (terminal, value = 0.85)
    let metal = m.metallic.expect("metalness should decode");
    assert!(
        (metal - 0.85).abs() < 1e-4,
        "metallic should be 0.85, got {metal}"
    );

    // normal goes: standard_surface.normal
    //   ← ND_normalmap.out (descend into inputs:in)
    //   ← ND_image_vector3.out (terminal, file = normal.png)
    assert_eq!(m.normal_texture.as_deref(), Some("textures/normal.png"));
}
