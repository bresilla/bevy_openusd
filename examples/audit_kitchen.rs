//! Walk every `.usd*` file under a directory tree, open each as a
//! standalone `openusd::Stage`, and tally what's authored. Goal:
//! find every prim type, every property name, every shader id, and
//! every relationship name that the bevy_openusd loader silently
//! drops. That tells us where the Kitchen_set "still grey" surface
//! area lives.
//!
//! Usage: `cargo run -p bevy_openusd --example audit_kitchen -- <root>`
//! e.g. `… -- assets/Kitchen_set`

use openusd::sdf::{Path as SdfPath, Value};
use std::collections::BTreeMap;

#[derive(Default)]
struct Tally {
    type_names: BTreeMap<String, usize>,
    property_names: BTreeMap<String, usize>,
    shader_ids: BTreeMap<String, usize>,
    api_schemas: BTreeMap<String, usize>,
    /// Counts of mesh prims with non-trivial primvars beyond
    /// `displayColor` / `normals` / `uvs`. Surfaces "primvars:foo"
    /// that point at custom shading data.
    extra_primvars: BTreeMap<String, usize>,
    /// Materials seen + the shader-id they connect to (or "no
    /// surface" when neither outputs:surface nor outputs:mtlx:surface
    /// is wired).
    materials_by_shader: BTreeMap<String, usize>,
    files_walked: usize,
}

fn value_signature(v: &Value) -> String {
    match v {
        Value::Vec3f(c) => format!("color3f({:.2},{:.2},{:.2})", c[0], c[1], c[2]),
        Value::Vec3d(c) => format!("color3d({:.2},{:.2},{:.2})", c[0], c[1], c[2]),
        Value::Vec3fVec(v) => format!("color3f[{}]", v.len()),
        Value::Vec3dVec(v) => format!("color3d[{}]", v.len()),
        Value::Float(x) => format!("f={x}"),
        Value::Double(x) => format!("d={x}"),
        Value::FloatVec(v) => format!("float[{}]", v.len()),
        Value::IntVec(v) => format!("int[{}]", v.len()),
        Value::String(s) | Value::Token(s) | Value::AssetPath(s) => format!("\"{s}\""),
        Value::TokenVec(v) | Value::StringVec(v) => format!("strs[{}]", v.len()),
        other => format!("{other:?}"),
    }
}

fn walk_prim(stage: &openusd::Stage, prim: &SdfPath, t: &mut Tally) {
    let type_name: Option<String> = stage
        .field::<String>(prim.clone(), "typeName")
        .ok()
        .flatten();
    if let Some(tn) = &type_name {
        *t.type_names.entry(tn.clone()).or_insert(0) += 1;
    }

    // apiSchemas list-op
    if let Ok(Some(v)) = stage.field::<Value>(prim.clone(), "apiSchemas") {
        if let Value::TokenListOp(op) = v {
            for s in op
                .prepended_items
                .iter()
                .chain(op.appended_items.iter())
                .chain(op.explicit_items.iter())
            {
                *t.api_schemas.entry(s.clone()).or_insert(0) += 1;
            }
        }
    }

    // Properties via `prim_properties`. Some binary files don't
    // expose this — silently skip those.
    if let Ok(props) = stage.prim_properties(prim.clone()) {
        for prop_name in props {
            *t.property_names
                .entry(prop_name.as_str().to_string())
                .or_insert(0) += 1;
            if let Some(rest) = prop_name.as_str().strip_prefix("primvars:") {
                if rest != "displayColor"
                    && rest != "displayOpacity"
                    && rest != "normals"
                    && rest != "st"
                    && !rest.contains(":indices")
                    && !rest.contains(":interpolation")
                {
                    *t.extra_primvars.entry(rest.to_string()).or_insert(0) += 1;
                }
            }
        }
    }

    // Direct probe: try a handful of well-known attributes by name
    // to catch values composed-in from references that
    // `prim_properties` doesn't enumerate.
    for probe in [
        "primvars:displayColor",
        "primvars:displayOpacity",
        "material:binding",
        "points",
        "faceVertexIndices",
    ] {
        if let Ok(ap) = prim.append_property(probe) {
            if let Ok(Some(v)) = stage.field::<Value>(ap, "default") {
                let key = format!("(probe) {probe}: {}", value_signature(&v));
                *t.property_names.entry(key).or_insert(0) += 1;

                // For displayColor specifically, tally distinct
                // colour values so we know whether Pixar's meshes
                // share one shade or have per-prop variety.
                if probe == "primvars:displayColor" {
                    if let Value::Vec3fVec(v) = &v {
                        if !v.is_empty() {
                            let c = v[0];
                            let bucket = format!(
                                "({:.2},{:.2},{:.2})",
                                (c[0] * 10.0).round() / 10.0,
                                (c[1] * 10.0).round() / 10.0,
                                (c[2] * 10.0).round() / 10.0
                            );
                            *t.shader_ids.entry(bucket).or_insert(0) += 1;
                        }
                    } else if let Value::Vec3dVec(v) = &v {
                        if !v.is_empty() {
                            let c = v[0];
                            let bucket = format!(
                                "({:.2},{:.2},{:.2})",
                                (c[0] * 10.0).round() / 10.0,
                                (c[1] * 10.0).round() / 10.0,
                                (c[2] * 10.0).round() / 10.0
                            );
                            *t.shader_ids.entry(bucket).or_insert(0) += 1;
                        }
                    }
                }
            }
        }
    }

    // Shader info:id
    if type_name.as_deref() == Some("Shader") {
        if let Ok(attr) = prim.append_property("info:id") {
            if let Ok(Some(v)) = stage.field::<Value>(attr, "default") {
                if let Value::Token(s) | Value::String(s) = v {
                    *t.shader_ids.entry(s).or_insert(0) += 1;
                }
            }
        }
    }

    // Materials: which shader id is reachable via outputs:surface or
    // outputs:mtlx:surface?
    if type_name.as_deref() == Some("Material") {
        let mut found = "(no surface)".to_string();
        for out in ["outputs:surface", "outputs:mtlx:surface"] {
            if let Ok(attr) = prim.append_property(out) {
                if let Ok(Some(Value::PathListOp(op))) = stage.field::<Value>(attr, "connectionPaths") {
                    if let Some(target) = op.flatten().into_iter().next() {
                        // target is a property path; chase to its prim
                        // and grab info:id.
                        let shader_prim = target.prim_path();
                        if let Ok(id_attr) = shader_prim.append_property("info:id") {
                            if let Ok(Some(v)) =
                                stage.field::<Value>(id_attr, "default")
                            {
                                if let Value::Token(s) | Value::String(s) = v {
                                    found = s;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        *t.materials_by_shader.entry(found).or_insert(0) += 1;
    }

    // Recurse
    if let Ok(children) = stage.prim_children(prim.clone()) {
        for c in children {
            if let Ok(child_path) = prim.append_path(c.as_str()) {
                walk_prim(stage, &child_path, t);
            }
        }
    }
}

fn walk_dir(dir: &std::path::Path, t: &mut Tally) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_dir(&p, t);
        } else {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(ext.as_str(), "usd" | "usda" | "usdc" | "usdz") {
                t.files_walked += 1;
                if let Some(p_str) = p.to_str()
                    && let Ok(stage) = openusd::Stage::open(p_str)
                {
                    walk_prim(&stage, &SdfPath::abs_root(), t);
                }
            }
        }
    }
}

fn print_top(label: &str, m: &BTreeMap<String, usize>, n: usize) {
    let mut v: Vec<_> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    println!("\n=== {label} ({} unique) ===", v.len());
    for (k, count) in v.iter().take(n) {
        println!("  {count:>6} × {k}");
    }
    if v.len() > n {
        println!("  … {} more", v.len() - n);
    }
}

fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/Kitchen_set".to_string());
    let mut t = Tally::default();
    walk_dir(std::path::Path::new(&root), &mut t);
    println!("Walked {} files under {root}", t.files_walked);
    print_top("Prim type names", &t.type_names, 40);
    print_top("API schemas", &t.api_schemas, 20);
    print_top("Materials by shader id", &t.materials_by_shader, 20);
    print_top("displayColor first-value buckets (rounded to 0.1)", &t.shader_ids, 30);
    print_top("Property names", &t.property_names, 50);
    print_top("Extra primvars (beyond displayColor / normals / st)", &t.extra_primvars, 30);

    // Composed-stage check: open the top-level `Kitchen_set.usd` and
    // probe a few mesh prims via direct `stage.field` queries (not
    // `prim_properties`, which only enumerates spec-level fields and
    // doesn't reach values composed in via references / payloads).
    println!("\n=== Composed-stage probe ===");
    let composed = std::path::Path::new(&root).join("Kitchen_set.usd");
    if let Some(s) = composed.to_str() {
        if let Ok(stage) = openusd::Stage::open(s) {
            let probes = [
                "/Kitchen_set/Arch_grp/Kitchen_1/Geom/Cabinets/Body/pCube251",
                "/Kitchen_set/Props_grp/West_grp/WestWall_grp/FramePictureD_1/Geom/FramePicture",
                "/Kitchen_set/Props_grp/North_grp/NorthWall_grp/MeasuringSpoon_1",
                "/Kitchen_set/Arch_grp/Kitchen_1",
                "/Kitchen_set",
            ];
            for path_str in probes {
                let Ok(p) = SdfPath::new(path_str) else {
                    println!("  {path_str}: invalid path");
                    continue;
                };
                let type_name: Option<String> =
                    stage.field::<String>(p.clone(), "typeName").ok().flatten();
                println!("  {path_str}: typeName={:?}", type_name);
                if let Ok(children) = stage.prim_children(p.clone()) {
                    println!(
                        "    children: {:?}",
                        children.iter().take(8).collect::<Vec<_>>()
                    );
                }
                for attr in [
                    "primvars:displayColor",
                    "primvars:displayOpacity",
                    "material:binding",
                    "purpose",
                    "points",
                    "faceVertexCounts",
                ] {
                    if let Ok(ap) = p.append_property(attr) {
                        if let Ok(Some(v)) = stage.field::<Value>(ap, "default") {
                            println!("    .{attr}: {:?}", v);
                        }
                    }
                }
            }
        }
    }
}
