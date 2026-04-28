//! One-shot diagnostic: open a Pixar Kitchen_set binary geom file
//! directly via `openusd::Stage` and dump every prim's authored
//! `xformOpOrder` + the raw value of each op. Helps determine
//! whether composition issues are due to mixed-op orderings or
//! `xformOp:transform` matrices that decompose surprisingly.

use openusd::sdf::{Path, Value};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/Kitchen_set/assets/FramePicture/FramePicture.geom.usd".to_string());
    let stage = openusd::Stage::open(&path).unwrap();

    fn walk(stage: &openusd::Stage, prim: &Path, depth: usize) {
        let indent = "  ".repeat(depth);
        let type_name: Option<String> =
            stage.field::<String>(prim.clone(), "typeName").ok().flatten();
        // xformOpOrder
        let order_attr = prim.append_property("xformOpOrder").ok();
        let order: Option<Vec<String>> = order_attr.and_then(|a| {
            stage.field::<Value>(a, "default").ok().flatten().and_then(|v| match v {
                Value::TokenVec(t) | Value::StringVec(t) => Some(t),
                _ => None,
            })
        });
        println!(
            "{}{} ({})",
            indent,
            prim.as_str(),
            type_name.as_deref().unwrap_or("?")
        );
        // Direct probe known shader / material attrs (composed values
        // from references won't show via prim_properties).
        for probe in [
            "info:id",
            "outputs:surface",
            "inputs:diffuseColor",
            "inputs:diffuseColor.connect",
            "inputs:roughness",
            "inputs:metallic",
            "inputs:normal",
            "inputs:file",
            "inputs:st",
            "material:binding",
        ] {
            if let Ok(ap) = prim.append_property(probe) {
                if let Ok(Some(v)) = stage.field::<Value>(ap.clone(), "default") {
                    println!("{}    PROBE .{}: {:?}", indent, probe, v);
                }
                if let Ok(Some(v)) = stage.field::<Value>(ap.clone(), "connectionPaths") {
                    println!("{}    PROBE .{}.conn: {:?}", indent, probe, v);
                }
                if let Ok(Some(v)) = stage.field::<Value>(ap, "targetPaths") {
                    println!("{}    PROBE .{}.targets: {:?}", indent, probe, v);
                }
            }
        }
        // Dump every authored property name + a short signature of its
        // default value so we can see materials, primvars, shaders.
        match stage.prim_properties(prim.clone()) {
            Ok(props) => {
                println!("{}  properties ({}):", indent, props.len());
                for prop_name in props {
                    let prop_str: &str = prop_name.as_str();
                    let Ok(attr) = prim.append_property(prop_str) else {
                        continue;
                    };
                    if let Ok(Some(v)) = stage.field::<Value>(attr, "default") {
                        let sig = match v {
                            Value::Vec3f(_) | Value::Vec3d(_) => "vec3".to_string(),
                            Value::Float(x) => format!("f={x}"),
                            Value::Double(x) => format!("d={x}"),
                            Value::String(s) | Value::Token(s) | Value::AssetPath(s) => {
                                format!("str/asset=\"{s}\"")
                            }
                            Value::Vec3fVec(v) => format!("Vec3f[{}]", v.len()),
                            Value::Vec3dVec(v) => format!("Vec3d[{}]", v.len()),
                            Value::TokenVec(v) | Value::StringVec(v) => format!("strs={:?}", v),
                            Value::IntVec(v) => format!("int[{}]", v.len()),
                            Value::FloatVec(v) => format!("f[{}]", v.len()),
                            other => format!("{other:?}"),
                        };
                        println!("{}    .{}: {}", indent, prop_name, sig);
                    } else {
                        println!("{}    .{}: (rel/no-default)", indent, prop_name);
                    }
                }
            }
            Err(e) => println!("{}  (props err: {})", indent, e),
        }
        // Recurse into children
        if let Ok(children) = stage.prim_children(prim.clone()) {
            for c in children {
                if let Ok(child_path) = prim.append_path(c.as_str()) {
                    walk(stage, &child_path, depth + 1);
                }
            }
        }
    }
    walk(&stage, &Path::abs_root(), 0);

    // Try `read_preview_material` on every Material we find.
    fn walk_mats(stage: &openusd::Stage, prim: &Path) {
        if stage
            .field::<String>(prim.clone(), "typeName")
            .ok()
            .flatten()
            .as_deref()
            == Some("Material")
        {
            println!("\n=== read_preview_material on {} ===", prim.as_str());
            match usd_schemas::shade::read_preview_material(stage, prim) {
                Ok(Some(m)) => println!("  decoded: {:#?}", m),
                Ok(None) => println!("  decoded: None (unrecognised shader)"),
                Err(e) => println!("  err: {e}"),
            }
        }
        if let Ok(children) = stage.prim_children(prim.clone()) {
            for c in children {
                if let Ok(cp) = prim.append_path(c.as_str()) {
                    walk_mats(stage, &cp);
                }
            }
        }
    }
    walk_mats(&stage, &Path::abs_root());
}
