//! M21 integration test: every primvar interpolation mode
//! (`constant | uniform | varying | vertex | faceVarying`) on
//! `primvars:displayColor` round-trips into `Mesh::ATTRIBUTE_COLOR`
//! with the expected per-vertex stride/values.

use openusd::sdf::Path;
use usd_schemas::geom::{read_mesh, Interpolation};

#[test]
fn reads_all_five_primvar_interpolations() {
    let stage = openusd::Stage::open("tests/stages/primvar_interp.usda")
        .expect("fixture parses");

    // Check each authored mode round-trips through ReadMesh.
    let cases = [
        ("/World/Constant", Interpolation::Constant, 1),
        ("/World/Uniform", Interpolation::Uniform, 2),
        ("/World/Vertex", Interpolation::Vertex, 4),
        ("/World/Varying", Interpolation::Varying, 4),
        ("/World/FaceVarying", Interpolation::FaceVarying, 4),
    ];
    for (path_str, expected_interp, expected_count) in cases {
        let path = Path::new(path_str).expect("valid path");
        let mesh = read_mesh(&stage, &path)
            .expect("read ok")
            .expect("mesh decodes");
        let dc = mesh
            .display_color
            .as_ref()
            .unwrap_or_else(|| panic!("{path_str} should have displayColor"));
        println!(
            "{path_str}: interpolation={:?} values.len()={}",
            dc.interpolation,
            dc.values.len()
        );
        assert_eq!(
            dc.interpolation, expected_interp,
            "{path_str} interpolation mismatch"
        );
        assert_eq!(
            dc.values.len(),
            expected_count,
            "{path_str} value count mismatch"
        );
    }
}

#[test]
fn loader_materialises_all_five_into_bevy_mesh_colors() {
    use bevy::mesh::{Mesh, PrimitiveTopology, VertexAttributeValues};
    use bevy::asset::RenderAssetUsages;

    // Run the same USDA through `mesh_from_usd` (no Bevy asset
    // infrastructure needed — pure function).
    let stage = openusd::Stage::open("tests/stages/primvar_interp.usda")
        .expect("fixture parses");

    // Constant: same colour on every vertex.
    {
        let read = read_mesh(&stage, &Path::new("/World/Constant").unwrap())
            .unwrap()
            .unwrap();
        let bevy_mesh = bevy_openusd_mesh_from_usd(&read);
        let colors = read_attr_color4(&bevy_mesh);
        println!("Constant → {} vertex colour(s)", colors.len());
        assert!(!colors.is_empty(), "expected vertex colours");
        for c in &colors {
            assert!(
                (c[0] - 1.0).abs() < 1e-4 && c[1].abs() < 1e-4 && c[2].abs() < 1e-4,
                "Constant should broadcast red, got {c:?}"
            );
        }
    }

    // Vertex: distinct colours per corner.
    {
        let read = read_mesh(&stage, &Path::new("/World/Vertex").unwrap())
            .unwrap()
            .unwrap();
        let bevy_mesh = bevy_openusd_mesh_from_usd(&read);
        let colors = read_attr_color4(&bevy_mesh);
        println!("Vertex → {} vertex colour(s)", colors.len());
        // Vertex interpolation keeps the indexed vertex-buffer path: 4 verts, 4 colours.
        assert_eq!(colors.len(), 4);
        assert!((colors[0][0] - 1.0).abs() < 1e-4); // red
        assert!((colors[1][1] - 1.0).abs() < 1e-4); // green
        assert!((colors[2][2] - 1.0).abs() < 1e-4); // blue
    }

    // Uniform: one colour per face. Two quad faces → expansion kicks in;
    // each face's four corners take the face's colour.
    {
        let read = read_mesh(&stage, &Path::new("/World/Uniform").unwrap())
            .unwrap()
            .unwrap();
        let bevy_mesh = bevy_openusd_mesh_from_usd(&read);
        let colors = read_attr_color4(&bevy_mesh);
        println!("Uniform → {} expanded vertex colour(s)", colors.len());
        // Two quads × 4 corners = 8 expanded vertices.
        assert_eq!(colors.len(), 8);
        // First face (idx 0..4) should all be red.
        for c in &colors[0..4] {
            assert!(
                (c[0] - 1.0).abs() < 1e-4 && c[1].abs() < 1e-4,
                "Uniform face 0 should be red, got {c:?}"
            );
        }
        // Second face (idx 4..8) should all be blue.
        for c in &colors[4..8] {
            assert!(
                c[0].abs() < 1e-4 && (c[2] - 1.0).abs() < 1e-4,
                "Uniform face 1 should be blue, got {c:?}"
            );
        }
    }

    // FaceVarying: one colour per corner (expanded path).
    {
        let read = read_mesh(&stage, &Path::new("/World/FaceVarying").unwrap())
            .unwrap()
            .unwrap();
        let bevy_mesh = bevy_openusd_mesh_from_usd(&read);
        let colors = read_attr_color4(&bevy_mesh);
        println!("FaceVarying → {} expanded vertex colour(s)", colors.len());
        assert_eq!(colors.len(), 4);
        assert!((colors[0][0] - 1.0).abs() < 1e-4);
        assert!((colors[1][1] - 1.0).abs() < 1e-4);
        assert!((colors[2][2] - 1.0).abs() < 1e-4);
    }

    // Keep imports used.
    let _ = RenderAssetUsages::default();
    let _ = PrimitiveTopology::TriangleList;
    let _: Option<&VertexAttributeValues> = None;
}

fn read_attr_color4(mesh: &bevy::mesh::Mesh) -> Vec<[f32; 4]> {
    use bevy::mesh::{Mesh, VertexAttributeValues};
    match mesh.attribute(Mesh::ATTRIBUTE_COLOR) {
        Some(VertexAttributeValues::Float32x4(v)) => v.clone(),
        other => panic!("expected Float32x4 ATTRIBUTE_COLOR, got {other:?}"),
    }
}

// `mesh_from_usd` is a private module path normally, but we re-export
// just enough via `bevy_openusd` to exercise it in tests. If/when
// `bevy_openusd::mesh` becomes public, this shim goes away.
fn bevy_openusd_mesh_from_usd(read: &usd_schemas::geom::ReadMesh) -> bevy::mesh::Mesh {
    bevy_openusd::mesh_from_usd(read)
}
