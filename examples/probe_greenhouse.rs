//! Probe specific known mesh attrs on greenhouse prims to compare a
//! "Block" (which renders) vs "Rachis" plant (which doesn't).

use std::collections::BTreeMap;
use openusd::sdf::{Path, Value};

fn brief(v: &Value) -> String {
    match v {
        Value::Vec3f(x) => format!("vec3f = {:?}", x),
        Value::Vec3d(x) => format!("vec3d = {:?}", x),
        Value::Float(x) => format!("f={x}"),
        Value::Double(x) => format!("d={x}"),
        Value::String(s) | Value::Token(s) | Value::AssetPath(s) => format!("str=\"{}\"", s),
        Value::Vec3fVec(x) => {
            if x.len() <= 4 {
                format!("Vec3f[{}] = {:?}", x.len(), x)
            } else {
                format!("Vec3f[{}] first={:?} last={:?}", x.len(), &x[..2], &x[x.len()-2..])
            }
        }
        Value::Vec3dVec(x) => {
            if x.len() <= 4 {
                format!("Vec3d[{}] = {:?}", x.len(), x)
            } else {
                format!("Vec3d[{}] first={:?} last={:?}", x.len(), &x[..2], &x[x.len()-2..])
            }
        }
        Value::TokenVec(x) | Value::StringVec(x) => format!("strs={:?}", x),
        Value::IntVec(x) => format!("int[{}]", x.len()),
        Value::FloatVec(x) => format!("f[{}]", x.len()),
        Value::Bool(b) => format!("b={b}"),
        other => format!("{other:?}"),
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/bresilla/data/code/other/isaacsim-greenhouse/blender/Exports/export.usdc".into());
    let stage = openusd::Stage::builder()
        .on_error(|_| Ok(()))
        .open(&path)
        .unwrap();

    let probes = [
        "points",
        "faceVertexCounts",
        "faceVertexIndices",
        "extent",
        "purpose",
        "visibility",
        "doubleSided",
        "orientation",
        "subdivisionScheme",
        "primvars:displayColor",
        "primvars:displayOpacity",
        "primvars:st",
        "material:binding",
        "xformOpOrder",
        "xformOp:transform",
        "xformOp:translate",
        "xformOp:scale",
        "xformOp:rotateXYZ",
    ];

    // Stage layer metadata
    println!("\n== Stage layer metadata ==");
    {
        let root = Path::abs_root();
        for k in ["upAxis", "metersPerUnit", "defaultPrim"] {
            if let Ok(Some(v)) = stage.field::<Value>(root.clone(), k) {
                println!("  {} = {}", k, brief(&v));
            }
        }
    }
    println!();

    let mut groups: BTreeMap<String, (usize, [f32; 3], [f32; 3])> = BTreeMap::new();
    fn walk_groups(stage: &openusd::Stage, prim: &Path, groups: &mut BTreeMap<String, (usize, [f32; 3], [f32; 3])>) {
        let type_name: Option<String> = stage.field(prim.clone(), "typeName").ok().flatten();
        if type_name.as_deref() == Some("Mesh") {
            let name_str = prim.name().unwrap_or("").to_string();
            let prefix: String = name_str.chars().take_while(|c| !c.is_ascii_digit() && *c != '_').collect();
            let prefix = if prefix.is_empty() { name_str.split('_').next().unwrap_or("?").to_string() } else { prefix };
            // Get extent
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            if let Ok(ap) = prim.append_property("extent") {
                if let Ok(Some(Value::Vec3fVec(ext))) = stage.field::<Value>(ap, "default") {
                    if ext.len() == 2 {
                        mn = ext[0]; mx = ext[1];
                    }
                }
            }
            let entry = groups.entry(prefix).or_insert((0, [f32::INFINITY; 3], [f32::NEG_INFINITY; 3]));
            entry.0 += 1;
            for i in 0..3 {
                if mn[i] < entry.1[i] { entry.1[i] = mn[i]; }
                if mx[i] > entry.2[i] { entry.2[i] = mx[i]; }
            }
        }
        if let Ok(children) = stage.prim_children(prim.clone()) {
            for c in children {
                if let Ok(child_path) = prim.append_path(c.as_str()) {
                    walk_groups(stage, &child_path, groups);
                }
            }
        }
    }
    walk_groups(&stage, &Path::abs_root(), &mut groups);
    println!("== Mesh groups by prefix ==");
    for (k, (n, mn, mx)) in &groups {
        let dx = mx[0] - mn[0];
        let dy = mx[1] - mn[1];
        let dz = mx[2] - mn[2];
        println!("  {:30}  n={:4}  extent_max=[{:8.4} {:8.4} {:8.4}]  size=[{:8.4} {:8.4} {:8.4}]", k, n, mx[0], mx[1], mx[2], dx, dy, dz);
    }

    // Walk the GreenMaterial subtree to see how the green leaves are bound.
    println!("\n== GreenMaterial subtree ==");
    fn dump_subtree(stage: &openusd::Stage, prim: &Path, depth: usize) {
        let indent = "  ".repeat(depth);
        let type_name: Option<String> = stage.field(prim.clone(), "typeName").ok().flatten();
        println!("{}{} ({})", indent, prim.as_str(), type_name.as_deref().unwrap_or("?"));
        if let Ok(props) = stage.prim_properties(prim.clone()) {
            for prop in props {
                let prop_str: &str = prop.as_str();
                let Ok(attr) = prim.append_property(prop_str) else { continue };
                if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "default") {
                    println!("{}  .{}: {}", indent, prop_str, brief(&v));
                } else if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "connectionPaths") {
                    println!("{}  .{} <- {:?}", indent, prop_str, v);
                } else if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "targetPaths") {
                    println!("{}  .{} -> {:?}", indent, prop_str, v);
                }
            }
        }
        if let Ok(children) = stage.prim_children(prim.clone()) {
            for c in children {
                if let Ok(child) = prim.append_path(c.as_str()) {
                    dump_subtree(stage, &child, depth + 1);
                }
            }
        }
    }
    dump_subtree(&stage, &Path::new("/root/_materials/GreenMaterial").unwrap(), 0);

    println!("\n== material:binding + preview material resolution ==");
    for sample in [
        "/root/Block_1_001/SM_RockwoolBlock",
        "/root/Rachis_main_spline_0_21/Rachis_main_spline_0_21_curve",
    ] {
        let prim = Path::new(sample).unwrap();
        let bind = usd_schemas::shade::read_material_binding(&stage, &prim);
        match &bind {
            Ok(Some(p)) => println!("  {} -> binding={}", sample, p.as_str()),
            Ok(None) => println!("  {} -> binding=None", sample),
            Err(e) => println!("  {} -> binding ERR {}", sample, e),
        }
        if let Ok(Some(mat_path)) = bind {
            match usd_schemas::shade::read_preview_material(&stage, &mat_path) {
                Ok(Some(mat)) => println!(
                    "        diffuse={:?} opacity={:?} roughness={:?} metallic={:?} diffuse_tex={:?} normal_tex={:?}",
                    mat.diffuse_color, mat.opacity, mat.roughness, mat.metallic,
                    mat.diffuse_texture, mat.normal_texture,
                ),
                Ok(None) => println!("        read_preview_material = None"),
                Err(e) => println!("        read_preview_material ERR {}", e),
            }
        }
    }

    println!("\n== usd_schemas::geom::read_mesh on plant prims ==");
    for sample in [
        "/root/Block_1_001/SM_RockwoolBlock",
        "/root/Rachis_main_spline_0_21/Rachis_main_spline_0_21_curve",
        "/root/Rachis_main_spline_0_21/Rachis_branch_spline_0_21_11/Rachis_branch_spline_0_21_11",
    ] {
        let prim = Path::new(sample).unwrap();
        match usd_schemas::geom::read_mesh(&stage, &prim) {
            Ok(Some(rm)) => println!(
                "  {}: points={} face_counts={} indices={} subsets={} normals={}",
                sample,
                rm.points.len(),
                rm.face_vertex_counts.len(),
                rm.face_vertex_indices.len(),
                rm.subsets.len(),
                rm.normals.as_ref().map(|n| n.values.len()).unwrap_or(0),
            ),
            Ok(None) => println!("  {}: read_mesh returned None", sample),
            Err(e) => println!("  {}: read_mesh ERR {}", sample, e),
        }
    }

    println!("\n== Direct shader probes ==");
    for shader in [
        "/root/_materials/GreenMaterial/Principled_BSDF",
        "/root/_materials/GreenMaterial/Image_Texture",
        "/root/_materials/GreenMaterial/Mapping",
        "/root/_materials/GreenMaterial/uvmap",
        "/root/_materials/GreenMaterial",
    ] {
        let prim = Path::new(shader).unwrap();
        println!("--- {} ---", shader);
        for p in [
            "info:id", "info:implementationSource",
            "inputs:diffuseColor", "inputs:opacity", "inputs:roughness", "inputs:metallic",
            "inputs:emissiveColor", "inputs:normal", "inputs:file", "inputs:st",
            "outputs:surface", "outputs:displacement", "outputs:rgb", "outputs:r", "outputs:g", "outputs:b",
            "inputs:varname", "inputs:scale", "inputs:bias",
            "outputs:result",
        ] {
            let Ok(attr) = prim.append_property(p) else { continue };
            let mut shown = false;
            if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "default") {
                println!("  .{}: {}", p, brief(&v));
                shown = true;
            }
            if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "connectionPaths") {
                println!("  .{} <- {:?}", p, v);
                shown = true;
            }
            if !shown {
                // Don't spam
            }
        }
    }


    println!("\n== Walk /root visibility/purpose ==");
    fn walk_vis(stage: &openusd::Stage, prim: &Path, depth: usize) {
        if depth > 2 { return; }
        let indent = "  ".repeat(depth);
        let type_name: Option<String> = stage.field(prim.clone(), "typeName").ok().flatten();
        let vis = prim.append_property("visibility").ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let pur = prim.append_property("purpose").ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let inst = stage.field::<bool>(prim.clone(), "instanceable").ok().flatten();
        if vis.is_some() || pur.is_some() || inst.is_some() {
            println!(
                "{}{} ({}) vis={:?} purpose={:?} instanceable={:?}",
                indent, prim.as_str(), type_name.as_deref().unwrap_or("?"),
                vis.as_ref().map(brief), pur.as_ref().map(brief), inst,
            );
        }
        if let Ok(children) = stage.prim_children(prim.clone()) {
            for c in children {
                if let Ok(child) = prim.append_path(c.as_str()) {
                    walk_vis(stage, &child, depth + 1);
                }
            }
        }
    }
    walk_vis(&stage, &Path::abs_root(), 0);

    for sample in [
        "/root",
        "/root/Block_1_001",
        "/root/Block_1_001/SM_RockwoolBlock",
        "/root/Rachis_main_spline_0_21",
        "/root/Rachis_main_spline_0_21/Rachis_main_spline_0_21_curve",
    ] {
        println!("=== {} ===", sample);
        let prim = Path::new(sample).unwrap();
        let type_name: Option<String> = stage.field(prim.clone(), "typeName").ok().flatten();
        println!("typeName: {:?}", type_name);
        for p in probes {
            let Ok(attr) = prim.append_property(p) else { continue };
            if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "default") {
                println!("  .{}: {}", p, brief(&v));
            } else if let Ok(Some(v)) = stage.field::<Value>(attr.clone(), "targetPaths") {
                println!("  .{} -> {:?}", p, v);
            }
        }
        println!();
    }
}
