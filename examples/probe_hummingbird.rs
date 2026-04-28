//! Specialized probe for hummingbird USDZ asset skeleton and animation structure.
//! Focuses on skel:joints, joint indices/weights, geomBindTransform, and animation keyframe alignment.

use openusd::sdf::{Path, Value};
use bevy::math::Mat4;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/bresilla/data/code/other/bevy_openusd/assets/external/hummingbird.usdz".to_string());
    let stage = openusd::Stage::open(&path).unwrap();

    println!("=== SKELETON STRUCTURE ===");
    let skel_path = Path::new("/hummingbird_anim_hover_idle_long/hummingbird_rig/hummingbird_skinned_mesh/hummingbird_bind/root_1").unwrap();
    
    if let Ok(Some(s)) = usd_schemas::skel::read_skeleton(&stage, &skel_path) {
        println!("Skeleton at {}", skel_path.as_str());
        println!("  Joint count: {}", s.joints.len());
        println!("  Bind transforms: {}", s.bind_transforms.len());
        println!("  Rest transforms: {}", s.rest_transforms.len());
        
        println!("\n  First 10 joints:");
        for (i, j) in s.joints.iter().take(10).enumerate() {
            println!("    [{}] {}", i, j);
        }
        
        // Check parent indices
        let parents = s.joint_parent_indices();
        println!("\n  Parent structure (first 10):");
        for (i, p) in parents.iter().take(10).enumerate() {
            println!("    [{}] parent={:?}", i, p);
        }
    }

    println!("\n=== MESH BINDINGS ===");
    
    // List all meshes and their skel bindings
    fn walk_meshes(stage: &openusd::Stage, prim: &Path) {
        let tn: String = stage
            .field::<String>(prim.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        
        if tn == "Mesh" {
            if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(stage, prim) {
                let max_idx = b.joint_indices.iter().max().copied().unwrap_or(0);
                let min_idx = b.joint_indices.iter().min().copied().unwrap_or(0);
                
                // Count distinct indices
                let mut distinct: std::collections::BTreeSet<i32> = 
                    b.joint_indices.iter().copied().collect();
                let _ = distinct.split_off(&i32::MAX);
                
                println!("  {} (elements_per_vert={})", prim.as_str(), b.elements_per_vertex);
                println!("    indices: range {}..{}, {} distinct", min_idx, max_idx, distinct.len());
                println!("    joint_subset: {} joints authored", b.joint_subset.len());
                
                // Check sparse vs dense: if joint_indices is short, it's sparse
                let is_sparse = b.joint_indices.len() < b.joint_weights.len();
                println!("    storage: {} (indices={}, weights={})", 
                    if is_sparse { "SPARSE" } else { "DENSE" },
                    b.joint_indices.len(),
                    b.joint_weights.len()
                );
                
                // Check if first mesh, also dump geomBindTransform
                if prim.as_str().contains("body") {
                    let gbind_attr = prim.append_property("primvars:skel:geomBindTransform").unwrap();
                    let gbind_val = stage.field::<Value>(gbind_attr, "default").ok().flatten();
                    if let Some(val) = gbind_val {
                        match val {
                            Value::Matrix4d(m) => {
                                let m_f32: [f32; 16] = m.iter().map(|&x| x as f32).collect::<Vec<_>>().try_into().unwrap();
                                let mat = Mat4::from_cols_array(&m_f32);
                                let (s, _, t) = mat.to_scale_rotation_translation();
                                let is_identity = (mat - Mat4::IDENTITY).abs_diff_eq(Mat4::ZERO, 1e-4);
                                println!("    geomBindTransform: scale={:?}, trans={:?}, identity={}", s, t, is_identity);
                            }
                            _ => println!("    geomBindTransform: present but unexpected type"),
                        }
                    } else {
                        println!("    geomBindTransform: NOT AUTHORED");
                    }
                }
            }
        }
        
        for child in stage.prim_children(prim.clone()).unwrap_or_default() {
            if let Ok(child_path) = prim.append_path(child.as_str()) {
                walk_meshes(stage, &child_path);
            }
        }
    }
    
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = Path::abs_root().append_path(n.as_str()) {
            walk_meshes(&stage, &p);
        }
    }

    println!("\n=== SKEL ANIMATION ===");
    
    let anim_path = Path::new("/hummingbird_anim_hover_idle_long/hummingbird_rig/hummingbird_skinned_mesh/hummingbird_bind/root_1/Animation").unwrap();
    
    println!("Animation prim at {}", anim_path.as_str());
    
    // Probe joints attribute
    if let Ok(jattr) = anim_path.append_property("joints") {
        if let Ok(Some(v)) = stage.field::<Value>(jattr, "default") {
            match v {
                Value::TokenVec(tokens) | Value::StringVec(tokens) => {
                    println!("  joints.default: {} entries", tokens.len());
                    println!("    First 5: {:?}", &tokens[..5.min(tokens.len())]);
                }
                _ => println!("  joints: unexpected type"),
            }
        }
    }
    
    // Check translations timeSamples
    if let Ok(tattr) = anim_path.append_property("translations") {
        match stage.field::<Value>(tattr.clone(), "timeSamples") {
            Ok(Some(Value::Vec3fVec(vals))) => {
                println!("  translations.timeSamples: {} vector3f values (1 keyframe = {} vals / joint_count)", 
                    vals.len(), vals.len());
            }
            _ => {
                // Try timeSampleIndices
                if let Ok(Some(indices)) = stage.field::<Value>(tattr, "timeSampleIndices") {
                    println!("  translations: has timeSampleIndices {:?}", indices);
                } else {
                    println!("  translations: no timeSamples found");
                }
            }
        }
    }
    
    // Check rotations timeSamples
    if let Ok(rattr) = anim_path.append_property("rotations") {
        match stage.field::<Value>(rattr.clone(), "timeSamples") {
            Ok(Some(Value::QuatfVec(vals))) => {
                println!("  rotations.timeSamples: {} quatf values", vals.len());
            }
            Ok(Some(Value::Vec4fVec(vals))) => {
                println!("  rotations.timeSamples: {} vec4f values (as quats)", vals.len());
            }
            _ => println!("  rotations: no timeSamples found"),
        }
    }

    println!("\n=== INHERITED skel:joints ===");
    
    // Walk from body mesh upward looking for skel:joints declarations
    let body_path = Path::new("/hummingbird_anim_hover_idle_long/hummingbird_rig/hummingbird_skinned_mesh/geom/body").unwrap();
    let mut cur = body_path.clone();
    loop {
        if let Ok(jattr) = cur.append_property("skel:joints") {
            if let Ok(Some(v)) = stage.field::<Value>(jattr, "default") {
                match v {
                    Value::TokenVec(tokens) | Value::StringVec(tokens) => {
                        println!("  {} → skel:joints: {} entries", cur.as_str(), tokens.len());
                        if tokens.len() <= 20 {
                            for (i, t) in tokens.iter().enumerate() {
                                println!("    [{}] {}", i, t);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }

    println!("\n=== SKEL ROOT REFERENCE ===");
    let skelroot = Path::new("/hummingbird_anim_hover_idle_long").unwrap();
    if let Ok(Some(r)) = usd_schemas::skel::read_skel_root(&stage, &skelroot) {
        println!("SkelRoot at {}", skelroot.as_str());
        println!("  skeleton: {:?}", r.skeleton);
        println!("  animation_source: {:?}", r.animation_source);
    }
}
