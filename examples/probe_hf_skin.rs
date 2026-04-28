//! Inspect a HumanFemale mesh's skel binding inside the composed
//! USDZ stage to see whether the variant override took effect.

use openusd::sdf::{Path, Value};

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        "/tmp/hf_check/HumanFemale_wrapper.usda".to_string()
    });
    let stage = openusd::Stage::open(&arg).unwrap();

    println!("=== /Skel/Rig/Skel skeleton ===");
    let skel = Path::new("/Skel/Rig/Skel").unwrap();
    println!("  spec_type = {:?}", stage.spec_type(skel.clone()));
    println!(
        "  typeName = {:?}",
        stage.field::<String>(skel.clone(), "typeName").ok().flatten()
    );
    for attr in ["joints", "bindTransforms", "restTransforms"] {
        if let Ok(p) = skel.append_property(attr) {
            if let Ok(Some(v)) = stage.field::<Value>(p, "default") {
                let s = match &v {
                    Value::TokenVec(t) => format!("TokenVec[{}]", t.len()),
                    Value::Matrix4dVec(m) => format!("Matrix4dVec[{}]", m.len()),
                    other => format!("{other:?}"),
                };
                println!("  {attr}: {s}");
            } else {
                println!("  {attr}: <none>");
            }
        }
    }

    println!("\n=== /Skel/SkelAnim ===");
    let anim = Path::new("/Skel/SkelAnim").unwrap();
    if let Ok(Some(v)) = stage.field::<Value>(anim.append_property("joints").unwrap(), "default") {
        if let Value::TokenVec(j) = v {
            println!("  joints: {} entries", j.len());
        }
    }

    println!("\n=== full traverse ===");
    let mut count = 0;
    let _ = stage.traverse(|p: &Path| {
        let tn: String = stage.field::<String>(p.clone(), "typeName").ok().flatten().unwrap_or_default();
        if tn == "Mesh" {
            let joints = match stage.field::<Value>(p.append_property("skel:joints").unwrap(), "default") {
                Ok(Some(Value::TokenVec(j))) => format!("{}", j.len()),
                _ => "-".to_string(),
            };
            let elt_size = match stage.field::<Value>(p.append_property("primvars:skel:jointIndices").unwrap(), "elementSize") {
                Ok(Some(Value::Int(n))) => n,
                _ => 0,
            };
            let max_idx = match stage.field::<Value>(p.append_property("primvars:skel:jointIndices").unwrap(), "default") {
                Ok(Some(Value::IntVec(v))) => v.iter().copied().max().unwrap_or(-1),
                _ => -1,
            };
            count += 1;
            if count <= 5 {
                println!("  Mesh {}: skel:joints={joints}, max_idx={max_idx}, elementSize={elt_size}", p.as_str());
            }
        }
    });
    println!("  total meshes: {count}");
}

fn walk(stage: &openusd::Stage, prim: &Path, depth: usize) {
    let pad = "  ".repeat(depth);
    let tn: String = stage.field::<String>(prim.clone(), "typeName").ok().flatten().unwrap_or_default();
    if tn == "Mesh" {
        // Per-mesh skel:joints (the joint subset this mesh's indices reference)
        let joints = match stage.field::<Value>(prim.append_property("skel:joints").unwrap(), "default") {
            Ok(Some(Value::TokenVec(j))) => format!("{} subset entries", j.len()),
            _ => "(none)".to_string(),
        };
        // jointIndices range
        let max_idx = match stage.field::<Value>(prim.append_property("primvars:skel:jointIndices").unwrap(), "default") {
            Ok(Some(Value::IntVec(v))) => v.iter().copied().max().unwrap_or(-1),
            _ => -1,
        };
        let elt_size = match stage.field::<Value>(prim.append_property("primvars:skel:jointIndices").unwrap(), "elementSize") {
            Ok(Some(Value::Int(n))) => n,
            _ => 0,
        };
        println!(
            "{pad}{}: skel:joints={joints}, max_index={max_idx}, elementSize={elt_size}",
            prim.as_str()
        );
    } else if !tn.is_empty() {
        println!("{pad}{tn}: {}", prim.as_str());
    }
    for child in stage.prim_children(prim.clone()).unwrap_or_default() {
        if let Ok(c) = prim.append_path(child.as_str()) {
            walk(stage, &c, depth + 1);
        }
    }
}
