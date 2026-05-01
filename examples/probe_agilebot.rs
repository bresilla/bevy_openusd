//! One-off probe for Agilebot GBT-C5A. Reports the loaded stage's
//! up-axis / metersPerUnit, the prim hierarchy, every prim that
//! authors physics opinions, and any unresolved references that
//! would cause "scattered" rendering.

use openusd::Stage;
use openusd::sdf::{Path, SpecType, Value};
use usd_schemas::physics as ph;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        "/home/bresilla/data/code/other/OUSD/bevy_openusd/assets/external/agilebot/gbt-c5a/gbt-c5a.usd".into()
    });
    println!("# Loading: {path}");
    let stage = Stage::builder()
        .on_error(|e| {
            eprintln!("(soft error) {e}");
            Ok(())
        })
        .open(&path)
        .expect("open stage");

    // Stage metadata
    let up_axis = stage
        .field::<String>(Path::abs_root(), "upAxis")
        .ok()
        .flatten();
    let mpu = stage
        .field::<Value>(Path::abs_root(), "metersPerUnit")
        .ok()
        .flatten();
    let kpu = stage
        .field::<Value>(Path::abs_root(), "kilogramsPerUnit")
        .ok()
        .flatten();
    let default_prim = stage.default_prim();
    println!("# upAxis = {up_axis:?}");
    println!("# metersPerUnit = {mpu:?}");
    println!("# kilogramsPerUnit = {kpu:?}");
    println!("# defaultPrim = {default_prim:?}");
    println!("# layer count = {}", stage.layer_count());

    // Walk prims, count types + authored xform op orders + transforms
    let mut total = 0usize;
    let mut by_type = std::collections::BTreeMap::<String, usize>::new();
    let mut with_xform_ops = 0usize;
    let mut with_translate = 0usize;
    let mut with_orient = 0usize;
    let mut at_origin = 0usize;
    stage
        .traverse(|p: &Path| {
            if matches!(stage.spec_type(p.clone()), Ok(Some(SpecType::Prim))) {
                total += 1;
                let tn: String = stage
                    .field::<String>(p.clone(), "typeName")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "(no typeName)".into());
                *by_type.entry(tn.clone()).or_insert(0) += 1;
                if let Ok(Some(_)) = stage
                    .field::<Value>(p.clone(), "xformOpOrder")
                {
                    with_xform_ops += 1;
                }
                if let Ok(attr_p) = p.append_property("xformOp:translate") {
                    if let Ok(Some(v)) = stage.field::<Value>(attr_p, "default") {
                        with_translate += 1;
                        let zeroish = match v {
                            Value::Vec3d(a) => a.iter().all(|x| x.abs() < 1e-9),
                            Value::Vec3f(a) => a.iter().all(|x| x.abs() < 1e-9),
                            _ => false,
                        };
                        if zeroish {
                            at_origin += 1;
                        }
                    }
                }
                if let Ok(attr_p) = p.append_property("xformOp:orient") {
                    if let Ok(Some(_)) = stage.field::<Value>(attr_p, "default") {
                        with_orient += 1;
                    }
                }
            }
        })
        .ok();
    println!("\n# Prim totals: {total} prims");
    for (t, c) in &by_type {
        println!("  {c:>4}  {t}");
    }
    println!(
        "\n# Transform stats: {with_xform_ops} prims author xformOpOrder, {with_translate} have xformOp:translate ({at_origin} at origin), {with_orient} have xformOp:orient"
    );

    // Find every Mesh prim and dump its prim path + transform info.
    println!("\n# Mesh prims (path · authored xformOpOrder · authored translate):");
    let mut mesh_paths = Vec::new();
    stage
        .traverse(|p: &Path| {
            if let Ok(Some(t)) = stage.field::<String>(p.clone(), "typeName")
                && t == "Mesh"
            {
                mesh_paths.push(p.as_str().to_string());
            }
        })
        .ok();
    for mp in mesh_paths.iter().take(15) {
        let p = openusd::sdf::path(mp).unwrap();
        let order = p
            .append_property("xformOpOrder")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let tr = p
            .append_property("xformOp:translate")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        // Try our actual reader
        let computed = usd_schemas::xform::read_transform(&stage, &p).ok().flatten();
        println!(
            "  {mp}\n    order = {order:?}\n    translate = {tr:?}\n    computed = {computed:?}"
        );
    }
    if mesh_paths.len() > 15 {
        println!("  ... and {} more meshes", mesh_paths.len() - 15);
    }

    // Probe each link Xform's transform.
    println!("\n# Link Xform transforms:");
    for link in [
        "/GBT_C5A",
        "/GBT_C5A/base_link",
        "/GBT_C5A/link1",
        "/GBT_C5A/link2",
        "/GBT_C5A/link3",
        "/GBT_C5A/link4",
        "/GBT_C5A/link5",
        "/GBT_C5A/link6",
        "/GBT_C5A/link1/visuals",
        "/GBT_C5A/link1/visuals/link1",
        "/GBT_C5A/link1/collisions",
    ] {
        let p = openusd::sdf::path(link).unwrap();
        let order = p
            .append_property("xformOpOrder")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let tr = p
            .append_property("xformOp:translate")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let or = p
            .append_property("xformOp:orient")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let computed = usd_schemas::xform::read_transform(&stage, &p).ok().flatten();
        println!("  {link}");
        println!("    order={order:?}");
        println!("    translate={tr:?}");
        println!("    orient={or:?}");
        println!("    computed={computed:?}");
    }

    // Root-level prims (peers of defaultPrim).
    println!("\n# Root prims (peers of defaultPrim):");
    if let Ok(roots) = stage.root_prims() {
        for r in roots.iter().take(20) {
            println!("  {r}");
        }
    }

    // Probe purpose on collision vs visual paths.
    println!("\n# Purpose of visuals vs collisions:");
    for path_str in [
        "/GBT_C5A/link1/visuals",
        "/GBT_C5A/link1/visuals/link1",
        "/GBT_C5A/link1/visuals/link1/mesh",
        "/GBT_C5A/link1/collisions",
        "/GBT_C5A/link1/collisions/link1",
        "/GBT_C5A/link1/collisions/link1/mesh",
    ] {
        let p = openusd::sdf::path(path_str).unwrap();
        let purpose = p
            .append_property("purpose")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        println!("  {path_str:<55} purpose = {purpose:?}");
    }

    // Probe whether visual meshes have actual point data after composition.
    println!("\n# Mesh point counts (after composition):");
    for path_str in [
        "/GBT_C5A/base_link/visuals/base_link/mesh",
        "/GBT_C5A/link1/visuals/link1/mesh",
        "/GBT_C5A/link6/visuals/link6/mesh",
    ] {
        let p = openusd::sdf::path(path_str).unwrap();
        let pts = p
            .append_property("points")
            .ok()
            .and_then(|a| stage.field::<Value>(a, "default").ok().flatten());
        let count = match &pts {
            Some(Value::Vec3fVec(v)) => v.len(),
            Some(Value::Vec3dVec(v)) => v.len(),
            _ => 0,
        };
        println!("  {path_str:<55} points={count} verts");
    }

    // Probe one specific link prim — every authored field + child property.
    let link1 = openusd::sdf::path("/GBT_C5A/link1").unwrap();
    println!("\n# Probing /GBT_C5A/link1 fields:");
    for field in [
        "typeName",
        "specifier",
        "xformOpOrder",
        "xformOp:translate",
        "xformOp:orient",
        "primChildren",
        "propertyChildren",
        "apiSchemas",
    ] {
        let raw = stage.field::<Value>(link1.clone(), field).ok().flatten();
        println!("  field {field:<20} = {raw:?}");
    }
    // Also try reading xformOpOrder as a PROPERTY (with its own default value).
    if let Ok(prop) = link1.append_property("xformOpOrder") {
        let raw = stage.field::<Value>(prop, "default").ok().flatten();
        println!("  prop  xformOpOrder.default = {raw:?}");
    }

    // Physics summary via our find_physics_prims helper
    match ph::find_physics_prims(&stage) {
        Ok(p) => {
            println!("\n# Physics prims (via find_physics_prims):");
            println!("  scenes              = {}", p.scenes.len());
            for s in &p.scenes {
                println!("    {s}");
            }
            println!("  rigid bodies        = {}", p.rigid_bodies.len());
            for s in p.rigid_bodies.iter().take(20) {
                println!("    {s}");
            }
            if p.rigid_bodies.len() > 20 {
                println!("    ... and {} more", p.rigid_bodies.len() - 20);
            }
            println!("  articulation roots  = {}", p.articulation_roots.len());
            for s in &p.articulation_roots {
                println!("    {s}");
            }
            println!("  colliders           = {}", p.colliders.len());
            println!("  joints              = {}", p.joints.len());
            for jp in p.joints.iter().take(20) {
                let pp = openusd::sdf::path(jp).unwrap();
                if let Ok(Some(j)) = ph::read_joint(&stage, &pp) {
                    println!(
                        "    {jp}  kind={:?}  body0={:?}  body1={:?}  axis={:?}  lim=({:?},{:?})",
                        j.kind, j.body0, j.body1, j.axis, j.lower_limit, j.upper_limit
                    );
                }
            }
            if p.joints.len() > 20 {
                println!("    ... and {} more", p.joints.len() - 20);
            }
            println!("  materials           = {}", p.materials.len());
            println!("  collision groups    = {}", p.collision_groups.len());
            println!("  filtered pairs      = {}", p.filtered_pairs.len());
        }
        Err(e) => println!("# physics scan failed: {e}"),
    }
}
