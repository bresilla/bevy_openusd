//! Probe the PointInstancedMedCity city — dump every authored attribute on
//! the instancer + one prototype mesh so we can see why our reader bailed
//! with "no readable geometry".

use openusd::sdf::{Path, Value};

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/PointInstancedMedCity/PointInstancedMedCity.usd".to_string());
    let stage = openusd::Stage::open(&path).unwrap();
    let up = stage.field::<String>(Path::abs_root(), "upAxis").ok().flatten();
    let mpu = stage.field::<Value>(Path::abs_root(), "metersPerUnit").ok().flatten();
    println!("ROOT upAxis={up:?} metersPerUnit={mpu:?}");
    let dp = stage.default_prim();
    println!("default_prim={dp:?}");
    // Check the bounds of the first 10 positions to estimate scene size.

    // Exercise the schema readers directly so we see what our fallback
    // produces, not just what's in the raw default field.
    let inst_path = Path::new("/MediterraneanHills/Buildings").unwrap();
    match usd_schemas::geom::read_point_instancer(&stage, &inst_path) {
        Ok(Some(d)) => println!(
            "read_point_instancer OK: positions={} protoIndices={} prototypes={}",
            d.positions.len(),
            d.proto_indices.len(),
            d.prototypes.len(),
        ),
        Ok(None) => println!("read_point_instancer: None (missing required attrs)"),
        Err(e) => println!("read_point_instancer ERROR: {e}"),
    }
    for pi in 0..8 {
        let proto = Path::new(&format!("/MediterraneanHills/Buildings/Prototypes/prototype_{pi}")).unwrap();
        let children = stage.prim_children(proto.clone()).unwrap_or_default();
        print!("prototype_{pi}: children={children:?}");
        for ch in &children {
            if let Ok(child_path) = proto.append_path(ch.as_str()) {
                if let Ok(Some(m)) = usd_schemas::geom::read_mesh(&stage, &child_path) {
                    print!(" {ch}=(p{},f{})", m.points.len(), m.face_vertex_counts.len());
                } else {
                    let tn: String = stage.field::<String>(child_path.clone(), "typeName").ok().flatten().unwrap_or_default();
                    print!(" {ch}=(?type={tn})");
                }
            }
        }
        println!();
    }
    if let Ok(Some(d)) = usd_schemas::geom::read_point_instancer(&stage, &inst_path) {
        let mut counts = std::collections::BTreeMap::<i32, usize>::new();
        for p in &d.proto_indices {
            *counts.entry(*p).or_insert(0) += 1;
        }
        println!("protoIndices distribution: {counts:?}");
        println!("positions={} orientations={} scales={}", d.positions.len(), d.orientations.len(), d.scales.len());
        println!("first 3 positions: {:?}", &d.positions[..3.min(d.positions.len())]);
        println!("first 3 orientations: {:?}", &d.orientations[..3.min(d.orientations.len())]);
        println!("first 3 scales: {:?}", &d.scales[..3.min(d.scales.len())]);
        // Range of positions to gauge real-world size of the asset.
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &d.positions {
            for i in 0..3 {
                if p[i] < mn[i] { mn[i] = p[i]; }
                if p[i] > mx[i] { mx[i] = p[i]; }
            }
        }
        println!("positions range: min={mn:?} max={mx:?} extent={:?}", [mx[0]-mn[0], mx[1]-mn[1], mx[2]-mn[2]]);
    }
    // Dump raw type variants of the timeSampled attrs.
    for attr in ["positions", "orientations", "scales"] {
        let ap = inst_path.append_property(attr).unwrap();
        if let Ok(Some(Value::TimeSamples(ts))) = stage.field::<Value>(ap, "timeSamples") {
            if let Some((_, v)) = ts.first() {
                let kind = match v {
                    Value::Vec3fVec(a) => format!("Vec3fVec(n={})", a.len()),
                    Value::Vec3hVec(a) => format!("Vec3hVec(n={})", a.len()),
                    Value::Vec3dVec(a) => format!("Vec3dVec(n={})", a.len()),
                    Value::QuatfVec(a) => format!("QuatfVec(n={})", a.len()),
                    Value::QuathVec(a) => format!("QuathVec(n={}) head={:?}", a.len(), a.iter().take(3).map(|q| [q[0].to_f32(), q[1].to_f32(), q[2].to_f32(), q[3].to_f32()]).collect::<Vec<_>>()),
                    Value::QuatdVec(a) => format!("QuatdVec(n={})", a.len()),
                    other => format!("{other:?}"),
                };
                println!("instancer.{attr} timeSample[0] kind = {kind}");
            }
        }
    }
    let mesh_path = Path::new("/MediterraneanHills/Buildings/Prototypes/prototype_0/mesh_0").unwrap();
    if let Ok(Some(m)) = usd_schemas::geom::read_mesh(&stage, &mesh_path) {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        for p in &m.points {
            for i in 0..3 {
                if p[i] < mn[i] { mn[i] = p[i]; }
                if p[i] > mx[i] { mx[i] = p[i]; }
            }
        }
        println!("prototype_0 mesh extent: min={mn:?} max={mx:?} dims={:?}", [mx[0]-mn[0], mx[1]-mn[1], mx[2]-mn[2]]);
    }
    let mp = mesh_path.append_property("normals").unwrap();
    if let Ok(Some(Value::TimeSamples(ts))) = stage.field::<Value>(mp, "timeSamples") {
        if let Some((_, v)) = ts.first() {
            let kind = match v {
                Value::Vec3fVec(a) => format!("Vec3fVec(n={})", a.len()),
                Value::Vec3hVec(a) => format!("Vec3hVec(n={})", a.len()),
                other => format!("{other:?}"),
            };
            println!("normals timeSample[0] kind = {kind}");
        }
    }
    println!();

    for probe_path in [
        "/MediterraneanHills/Buildings",
        "/MediterraneanHills/Buildings/Prototypes/prototype_0",
        "/MediterraneanHills/Buildings/Prototypes/prototype_0/mesh_0",
    ] {
        println!("=== {probe_path} ===");
        let prim = Path::new(probe_path).unwrap();
        for attr in [
            "points",
            "faceVertexCounts",
            "faceVertexIndices",
            "normals",
            "extent",
            "primvars:displayColor",
            "primvars:st",
            // PointInstancer-specific:
            "positions",
            "orientations",
            "scales",
            "protoIndices",
            "prototypes",
            "ids",
            "velocities",
            // Generic xform:
            "xformOpOrder",
        ] {
            let ap = prim.append_property(attr).unwrap();
            match stage.field::<Value>(ap.clone(), "default") {
                Ok(Some(v)) => {
                    let preview = match &v {
                        Value::FloatVec(a) => format!("FloatVec(len={})", a.len()),
                        Value::IntVec(a) => format!("IntVec(len={})", a.len()),
                        Value::Vec3fVec(a) => format!("Vec3fVec(len={})", a.len()),
                        Value::Vec2fVec(a) => format!("Vec2fVec(len={})", a.len()),
                        Value::Vec4fVec(a) => format!("Vec4fVec(len={})", a.len()),
                        Value::TokenVec(a) => format!("TokenVec({a:?})"),
                        Value::StringVec(a) => format!("StringVec({a:?})"),
                        Value::PathListOp(a) => format!("PathListOp({a:?})"),
                        other => format!("{other:?}"),
                    };
                    println!("  {attr} = {preview}");
                }
                Ok(None) => {}
                Err(e) => println!("  {attr} ERROR: {e}"),
            }
            // Also try timeSamples for animated attrs.
            match stage.field::<Value>(ap.clone(), "timeSamples") {
                Ok(Some(_)) => println!("  {attr} has timeSamples"),
                _ => {}
            }
        }
        println!();
    }
}
