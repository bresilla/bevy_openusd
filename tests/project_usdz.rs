//! M3.5 integration test: craft a tiny `.usdz` on the fly (a minimal
//! `.usda` referencing a 1×1 red PNG), feed it through the loader, and
//! assert the material's `base_color_texture` materializes into an Image
//! asset.
//!
//! Generating the fixture at test time keeps the repo free of opaque
//! binary blobs and proves the USDZ path works end-to-end.

use std::io::{Cursor, Write};
use std::path::PathBuf;

use bevy::asset::{AssetServer, Assets, LoadState};
use bevy::image::Image;
use bevy::mesh::Mesh;
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::{Scene, SceneRoot};
use bevy_openusd::{UsdAsset, UsdPlugin};
use zip::write::SimpleFileOptions;

/// A 1×1 red PNG encoded on the fly via the `image` crate. Keeping this
/// dynamic avoids hand-rolling the PNG byte stream (the first attempt
/// tripped a CRC check inside Bevy's decoder).
fn red_1x1_png() -> Vec<u8> {
    use image::{ImageFormat, RgbaImage};
    let mut img = RgbaImage::new(1, 1);
    img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

const USDA: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    upAxis = "Y"
    metersPerUnit = 1.0
)

def Xform "World"
{
    def Scope "Materials"
    {
        def Material "Red"
        {
            token outputs:surface.connect = </World/Materials/Red/Surface.outputs:surface>

            def Shader "Surface"
            {
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor.connect = </World/Materials/Red/DiffuseTex.outputs:rgb>
                float inputs:roughness = 0.5
                token outputs:surface
            }

            def Shader "DiffuseTex"
            {
                uniform token info:id = "UsdUVTexture"
                asset inputs:file = @./textures/red.png@
                token inputs:sourceColorSpace = "sRGB"
                float3 outputs:rgb
            }
        }
    }

    def Cube "RedBox" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        double size = 0.5
        rel material:binding = </World/Materials/Red>
    }
}
"#;

fn build_usdz(dest: &std::path::Path) {
    let mut buf = Vec::<u8>::new();
    {
        let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        // Per USDZ spec: the first entry must be a USD layer.
        zw.start_file("scene.usda", opts).unwrap();
        zw.write_all(USDA.as_bytes()).unwrap();

        let png = red_1x1_png();
        zw.start_file("textures/red.png", opts).unwrap();
        zw.write_all(&png).unwrap();

        zw.finish().unwrap();
    }
    std::fs::write(dest, buf).unwrap();
}

fn fixture_dir() -> PathBuf {
    // Target dir survives across `cargo test` runs — no repo pollution.
    let mut p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    p.push("bevy_openusd_usdz_fixture");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn build_test_app(asset_root: PathBuf) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::asset::AssetPlugin {
            file_path: asset_root.to_string_lossy().into_owned(),
            ..Default::default()
        })
        .init_asset::<Scene>()
        .init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<StandardMaterial>()
        .add_plugins(bevy::scene::ScenePlugin)
        .add_plugins(UsdPlugin)
        .register_type::<bevy::mesh::Mesh3d>()
        .register_type::<bevy::pbr::MeshMaterial3d<StandardMaterial>>();
    app
}

fn load_and_drive(app: &mut App, asset_name: &str) -> Handle<UsdAsset> {
    let handle: Handle<UsdAsset> = app
        .world()
        .resource::<AssetServer>()
        .load(asset_name.to_string());
    for _ in 0..200 {
        app.update();
        let server = app.world().resource::<AssetServer>();
        match server.get_load_state(&handle) {
            Some(LoadState::Loaded) => return handle,
            Some(LoadState::Failed(err)) => panic!("UsdAsset load failed: {err}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    panic!("UsdAsset did not load in time");
}

fn spawn_scene_root(app: &mut App, handle: &Handle<UsdAsset>) {
    let scene_handle = {
        let assets = app.world().resource::<Assets<UsdAsset>>();
        assets.get(handle).expect("asset missing").scene.clone()
    };
    app.world_mut().spawn(SceneRoot(scene_handle));
    // Enough ticks for SceneSpawner to instantiate + sub-assets to settle.
    for _ in 0..30 {
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn loads_usdz_with_embedded_texture() {
    let dir = fixture_dir();
    let archive = dir.join("red_box.usdz");
    build_usdz(&archive);

    let mut app = build_test_app(dir);
    let handle = load_and_drive(&mut app, "red_box.usdz");
    spawn_scene_root(&mut app, &handle);

    // There should be exactly one geom prim (the Cube), and its
    // StandardMaterial should carry a base_color_texture.
    use bevy_openusd::UsdPrimRef;
    let world = app.world_mut();
    let mut mat_for_prim = std::collections::HashMap::new();
    for (prim, mat) in world
        .query::<(&UsdPrimRef, &MeshMaterial3d<StandardMaterial>)>()
        .iter(world)
    {
        mat_for_prim.insert(prim.path.clone(), mat.0.clone());
    }

    let red_box_handle = mat_for_prim
        .get("/World/RedBox")
        .cloned()
        .expect("RedBox should have a material");
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let red = materials.get(&red_box_handle).expect("material missing");
    let tex = red
        .base_color_texture
        .as_ref()
        .expect("base_color_texture should be set from embedded PNG");

    // The texture must resolve to an actual Image in storage.
    let server = app.world().resource::<AssetServer>();
    let tex_load_state = server.get_load_state(tex);
    let images = app.world().resource::<Assets<Image>>();
    let image_count = images.iter().count();
    let image = images.get(tex).unwrap_or_else(|| {
        panic!(
            "embedded PNG should decode. tex load state = {:?}. \
             Assets<Image>::iter().count() = {image_count}.",
            tex_load_state
        )
    });
    // 1×1 PNG → 1×1 Image.
    assert_eq!(image.texture_descriptor.size.width, 1);
    assert_eq!(image.texture_descriptor.size.height, 1);
}
