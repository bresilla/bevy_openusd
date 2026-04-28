//! Probe a composed skel asset: walk the prim tree, find every Skeleton
//! and SkelRoot, dump joint count + first few joint paths so we can see
//! whether our wrapper exposes the rig.

use openusd::sdf::Path;
use bevy::math::Mat4;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/skel_human.usda".to_string());
    let stage = openusd::Stage::open(&path).unwrap();

    fn walk(stage: &openusd::Stage, prim: &Path) {
        let tn: String = stage
            .field::<String>(prim.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if tn == "Skeleton" {
            if let Ok(Some(s)) = usd_schemas::skel::read_skeleton(stage, prim) {
                println!(
                    "Skeleton {} joints={} bind={} rest={}",
                    prim.as_str(),
                    s.joints.len(),
                    s.bind_transforms.len(),
                    s.rest_transforms.len(),
                );
                let parents = s.joint_parent_indices();
                // For first few joints: print restTransform translation,
                // bindTransform translation, and computed rest world
                // (composed restTransforms up the chain).
                let mut rest_world = vec![Mat4::IDENTITY; s.joints.len()];
                for i in 0..s.joints.len() {
                    let local = Mat4::from_cols_array(
                        &s.rest_transforms.get(i).copied().unwrap_or(IDENTITY),
                    );
                    rest_world[i] = match parents[i] {
                        Some(pi) => rest_world[pi] * local,
                        None => local,
                    };
                }
                // Check if every joint's full matrix matches bind. Sum the
                // L2 norm of (rest_world[i] - bind[i]).
                let mut max_diff = 0.0f32;
                let mut worst = 0usize;
                for i in 0..s.joints.len() {
                    let bind_m = Mat4::from_cols_array(
                        &s.bind_transforms.get(i).copied().unwrap_or(IDENTITY),
                    );
                    let diff = (rest_world[i].to_cols_array_2d().iter().zip(
                        bind_m.to_cols_array_2d().iter(),
                    )).map(|(a, b)| {
                        a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum::<f32>()
                    }).sum::<f32>().sqrt();
                    if diff > max_diff { max_diff = diff; worst = i; }
                }
                println!("  max rest_world vs bind matrix diff: {max_diff:.6} at joint [{worst}] {}", s.joints[worst]);

                // Check decompose-recompose round-trip on every joint.
                let mut decomp_max = 0.0f32;
                let mut decomp_worst = 0usize;
                for i in 0..s.joints.len() {
                    let bind_m = Mat4::from_cols_array(
                        &s.bind_transforms.get(i).copied().unwrap_or(IDENTITY),
                    );
                    let (s_, r_, t_) = bind_m.to_scale_rotation_translation();
                    let recomposed = Mat4::from_scale_rotation_translation(s_, r_, t_);
                    let diff = (bind_m.to_cols_array().iter().zip(
                        recomposed.to_cols_array().iter(),
                    )).map(|(a, b)| (a - b).powi(2)).sum::<f32>().sqrt();
                    if diff > decomp_max {
                        decomp_max = diff;
                        decomp_worst = i;
                    }
                }
                println!(
                    "  decompose-recompose max drift: {decomp_max:.6} at joint [{decomp_worst}] {}",
                    s.joints[decomp_worst],
                );
                if decomp_max > 1e-3 {
                    let bind_m = Mat4::from_cols_array(
                        &s.bind_transforms.get(decomp_worst).copied().unwrap_or(IDENTITY),
                    );
                    let det = bind_m.determinant();
                    println!("    determinant: {det:.6}");
                    let (s_, r_, t_) = bind_m.to_scale_rotation_translation();
                    println!("    decomposed: scale={s_:?} rot={r_:?} trans={t_:?}");
                }
                if max_diff > 1e-3 {
                    let bind_m = Mat4::from_cols_array(
                        &s.bind_transforms.get(worst).copied().unwrap_or(IDENTITY),
                    );
                    println!("  worst rest_world: {:?}", rest_world[worst].to_cols_array());
                    println!("  worst bind:       {:?}", bind_m.to_cols_array());
                }
            }
        } else if tn == "SkelRoot" {
            if let Ok(Some(r)) = usd_schemas::skel::read_skel_root(stage, prim) {
                println!(
                    "SkelRoot {} skel={:?} animSrc={:?}",
                    prim.as_str(),
                    r.skeleton,
                    r.animation_source,
                );
            }
        }
        for child in stage.prim_children(prim.clone()).unwrap_or_default() {
            if let Ok(child_path) = prim.append_path(child.as_str()) {
                walk(stage, &child_path);
            }
        }
    }

    const IDENTITY: [f32; 16] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = Path::abs_root().append_path(n.as_str()) {
            walk(&stage, &p);
        }
    }

    // Per-skinned-mesh diagnostic: count subsets, bound joint subset
    // length, points count, and what the inherited skeleton resolves
    // to. Tells us whether the build path even reaches our skin-attr
    // baking.
    println!();
    println!("== skinned meshes ==");
    let mut count_skinned = 0;
    let mut count_with_subsets = 0;
    fn walk_skin(stage: &openusd::Stage, prim: &Path, count_skinned: &mut usize, count_with_subsets: &mut usize) {
        let tn: String = stage.field::<String>(prim.clone(), "typeName").ok().flatten().unwrap_or_default();
        if tn == "Mesh" {
            if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(stage, prim) {
                *count_skinned += 1;
                let mesh_data = usd_schemas::geom::read_mesh(stage, prim).ok().flatten();
                let subset_count = mesh_data.as_ref().map(|m| m.subsets.len()).unwrap_or(0);
                let pt_count = mesh_data.as_ref().map(|m| m.points.len()).unwrap_or(0);
                if subset_count > 0 {
                    *count_with_subsets += 1;
                }
                let max_idx = b.joint_indices.iter().max().copied().unwrap_or(0);
                let min_idx = b.joint_indices.iter().min().copied().unwrap_or(0);
                let weight_sum_per_vert: f32 = if b.elements_per_vertex > 0 {
                    let n = b.elements_per_vertex as usize;
                    let chunks = b.joint_weights.chunks(n);
                    let count = chunks.len();
                    if count > 0 {
                        b.joint_weights
                            .chunks(n)
                            .map(|c| c.iter().sum::<f32>())
                            .sum::<f32>()
                            / count as f32
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                println!(
                    "  {} subsets={} pts={} subset_joints={} per_vert={} idx_range={}..{} avg_wsum={:.3} skel={:?}",
                    prim.as_str(),
                    subset_count,
                    pt_count,
                    b.joint_subset.len(),
                    b.elements_per_vertex,
                    min_idx,
                    max_idx,
                    weight_sum_per_vert,
                    b.skeleton,
                );
            }
        }
        for child in stage.prim_children(prim.clone()).unwrap_or_default() {
            if let Ok(c) = prim.append_path(child.as_str()) {
                walk_skin(stage, &c, count_skinned, count_with_subsets);
            }
        }
    }
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = Path::abs_root().append_path(n.as_str()) {
            walk_skin(&stage, &p, &mut count_skinned, &mut count_with_subsets);
        }
    }
    println!("total skinned: {count_skinned}, of which with GeomSubset: {count_with_subsets}");

    // Sidecar-parse walk.usd and compare anim translations at first
    // keyframe vs the skeleton's restTransforms translations. If the
    // anim values are in a different unit, you'll see a massive
    // ratio.
    println!();
    println!("== anim vs bind translation scale check ==");
    let walk_path =
        "assets/UsdSkelExamples/HumanFemale/HumanFemale.walk.usd";
    if let Ok(walk_text) = std::fs::read_to_string(walk_path) {
        let anims = usd_schemas::skel_anim_text::scan_skel_animations(&walk_text);
        if let Some(anim) = anims.first() {
            // Find Hips in both anim and skeleton.
            let anim_hips = anim.joints.iter().position(|j| j == "Hips");
            // The Skeleton scan above already printed joints; re-walk to get rest.
            let mut skel_hips_rest = None;
            fn find_skel(
                stage: &openusd::Stage,
                p: &openusd::sdf::Path,
            ) -> Option<usd_schemas::skel::ReadSkeleton> {
                let tn = stage
                    .field::<String>(p.clone(), "typeName")
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                if tn == "Skeleton" {
                    return usd_schemas::skel::read_skeleton(stage, p).ok().flatten();
                }
                for c in stage.prim_children(p.clone()).unwrap_or_default() {
                    if let Ok(cp) = p.append_path(c.as_str()) {
                        if let Some(s) = find_skel(stage, &cp) {
                            return Some(s);
                        }
                    }
                }
                None
            }
            for n in stage.root_prims().unwrap_or_default() {
                if let Ok(p) = openusd::sdf::Path::abs_root().append_path(n.as_str()) {
                    if let Some(s) = find_skel(&stage, &p) {
                        skel_hips_rest = Some(s);
                        break;
                    }
                }
            }
            if let (Some(ai), Some(skel)) = (anim_hips, skel_hips_rest) {
                let skel_hips_local = bevy::math::Mat4::from_cols_array(
                    &skel.rest_transforms[0],
                );
                let (_, _, sk_t) = skel_hips_local.to_scale_rotation_translation();
                let anim_first = anim.translations.iter().next();
                if let Some((_, vals)) = anim_first {
                    let an_t = vals[ai];
                    println!(
                        "  Hips skel rest_local_t = {sk_t:?} | anim translation_at_first = {an_t:?}"
                    );
                }
                if let Some((_, rot_vals)) = anim.rotations.iter().next() {
                    let ar = rot_vals[ai];
                    let aq = bevy::math::Quat::from_xyzw(ar[1], ar[2], ar[3], ar[0]);
                    let (_, sk_r, _) = skel_hips_local.to_scale_rotation_translation();
                    println!(
                        "  Hips skel rest_local_rot = {sk_r:?} | anim rotation_at_first (wxyz) = {ar:?} → quat {aq:?}"
                    );
                    let dot = sk_r.dot(aq).abs();
                    println!("  Hips dot(rest_rot, anim_rot) = {dot:.4} (1.0 = same; far from 1 = unit/order mismatch)");
                }
                if let Some((_, sc_vals)) = anim.scales.iter().next() {
                    let ascale = sc_vals[ai];
                    println!("  Hips anim scale_at_first = {ascale:?}");
                }
            }
        }
    }
    println!();
    println!("== geomBindTransform probe ==");
    // Walk up from Body_sbdv looking for `skel:joints`.
    println!();
    // Identify which anim-order joints the nail and shoe bindings reach.
    println!();
    println!("== anim-order joint lookup ==");
    if let Ok(walk_text) = std::fs::read_to_string("assets/UsdSkelExamples/HumanFemale/HumanFemale.walk.usd") {
        let anims = usd_schemas::skel_anim_text::scan_skel_animations(&walk_text);
        if let Some(a) = anims.first() {
            println!("anim joint count: {}", a.joints.len());
            for ix in [55, 56, 57, 58, 59, 60, 100, 101, 102, 103, 104, 105, 106, 107, 108] {
                if ix < a.joints.len() {
                    println!("  [{ix}] {}", a.joints[ix]);
                }
            }
        }
    }

    // Probe hair meshes' bindings.
    println!();
    println!("== hair mesh binding probe ==");
    for mp in [
        "/Skel/Geometry/HumanFemaleHair/Geom/Hair/Layers/HeadHair/BetaLeft_HairLayer/Standin/Shell_sbdv",
        "/Skel/Geometry/HumanFemaleHair/Geom/Hair/Layers/EyeHair/BrowL_HairLayer/Standin/Shell_sbdv",
    ] {
        let prim = match openusd::sdf::Path::new(mp) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(&stage, &prim) {
            let max_idx = b.joint_indices.iter().max().copied().unwrap_or(0);
            let min_idx = b.joint_indices.iter().min().copied().unwrap_or(0);
            // Distinct indices used.
            let mut distinct: std::collections::BTreeSet<i32> =
                b.joint_indices.iter().copied().collect();
            let _ = distinct.split_off(&i32::MAX);
            let m = usd_schemas::geom::read_mesh(&stage, &prim).ok().flatten();
            let mut center = [0.0; 3];
            if let Some(ref m) = m {
                let mut mn = [f32::INFINITY; 3];
                let mut mx = [f32::NEG_INFINITY; 3];
                for p in &m.points {
                    for i in 0..3 {
                        if p[i] < mn[i] { mn[i] = p[i]; }
                        if p[i] > mx[i] { mx[i] = p[i]; }
                    }
                }
                for i in 0..3 { center[i] = (mn[i] + mx[i]) * 0.5; }
            }
            println!(
                "  {mp}\n    points center=({:.1}, {:.1}, {:.1})\n    per_vert={} idx_range={}..{} distinct={:?}",
                center[0], center[1], center[2],
                b.elements_per_vertex,
                min_idx, max_idx,
                distinct.iter().take(8).copied().collect::<Vec<_>>(),
            );
        }
    }

    // Probe a few nail / shoe meshes' bindings.
    println!();
    println!("== nail/shoe binding probe ==");
    for mp in [
        "/Skel/Geometry/HumanFemale/Geom/Body/Nails/LFingerNails/ThumbNail_sbdv",
        "/Skel/Geometry/ShoesHumanFlats/Geom/LShoe/Body/ShoeBody_sbdv",
        "/Skel/Geometry/ShoesHumanFlats/Geom/LShoe/Sole/Sole_sbdv",
        "/Skel/Geometry/HumanFemaleHair/Geom/Hair/Hair_sbdv",
    ] {
        let prim = match openusd::sdf::Path::new(mp) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(&stage, &prim) {
            let mut idx_set: std::collections::BTreeSet<i32> =
                b.joint_indices.iter().copied().collect();
            let summary: Vec<i32> = idx_set.iter().take(8).copied().collect();
            let _ = idx_set.split_off(&i32::MAX);
            println!(
                "  {mp} per_vert={} joints={} skel:joints_authored={}",
                b.elements_per_vertex,
                summary.len(),
                b.joint_subset.len()
            );
            println!("    indices used: {:?}", summary);
        } else {
            println!("  {mp} → no binding");
        }
    }

    // Walk up from the hair mesh probing for a binding anywhere in the chain.
    println!();
    println!("== hair inheritance walk ==");
    let mut cur =
        openusd::sdf::Path::new("/Skel/Geometry/HumanFemaleHair/Geom/Hair/Hair_sbdv").unwrap();
    loop {
        let attr = cur.append_property("primvars:skel:jointIndices").unwrap();
        let v = stage
            .field::<openusd::sdf::Value>(attr, "default")
            .ok()
            .flatten();
        let count = match v {
            Some(openusd::sdf::Value::IntVec(ref v)) => Some(v.len()),
            _ => None,
        };
        let attr2 = cur.append_property("xformOp:transform").unwrap();
        let xf = stage
            .field::<openusd::sdf::Value>(attr2, "default")
            .ok()
            .flatten();
        println!(
            "  {} → jointIndices {:?} xform {}",
            cur.as_str(),
            count,
            if xf.is_some() { "yes" } else { "no" }
        );
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }

    // Walk up from a shoe mesh dumping authored xforms.
    println!();
    println!("== shoe ancestor xforms ==");
    let mut cur =
        openusd::sdf::Path::new("/Skel/Geometry/ShoesHumanFlats/Geom/LShoe/Body/ShoeBody_sbdv").unwrap();
    loop {
        let order_attr = cur.append_property("xformOpOrder").unwrap();
        let order = stage
            .field::<openusd::sdf::Value>(order_attr, "default")
            .ok()
            .flatten();
        let scale_attr = cur.append_property("xformOp:scale").unwrap();
        let scale = stage
            .field::<openusd::sdf::Value>(scale_attr, "default")
            .ok()
            .flatten();
        let xform_attr = cur.append_property("xformOp:transform").unwrap();
        let xform = stage
            .field::<openusd::sdf::Value>(xform_attr, "default")
            .ok()
            .flatten();
        println!(
            "  {} order={:?} scale={:?} transform={}",
            cur.as_str(),
            order.is_some(),
            scale,
            if xform.is_some() { "yes" } else { "no" }
        );
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }

    // Read ShoesHumanFlats's authored xform.
    println!();
    println!("== ShoesHumanFlats xform ==");
    if let Ok(Some(t)) = usd_schemas::xform::read_transform(
        &stage,
        &openusd::sdf::Path::new("/Skel/Geometry/ShoesHumanFlats").unwrap(),
    ) {
        println!("  translate {:?}", t.translate);
        println!("  rotate {:?}", t.rotate);
        println!("  scale {:?}", t.scale);
    }

    // Bind translation lookup for the shoe's bound joints (0, 100..103).
    println!();
    println!("== bound joint world bind translations ==");
    fn find_first_skel(
        stage: &openusd::Stage,
        p: &openusd::sdf::Path,
    ) -> Option<usd_schemas::skel::ReadSkeleton> {
        let tn = stage
            .field::<String>(p.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if tn == "Skeleton" {
            return usd_schemas::skel::read_skeleton(stage, p).ok().flatten();
        }
        for c in stage.prim_children(p.clone()).unwrap_or_default() {
            if let Ok(cp) = p.append_path(c.as_str()) {
                if let Some(s) = find_first_skel(stage, &cp) {
                    return Some(s);
                }
            }
        }
        None
    }
    let mut skel_for_probe = None;
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = openusd::sdf::Path::abs_root().append_path(n.as_str()) {
            if let Some(s) = find_first_skel(&stage, &p) {
                skel_for_probe = Some(s);
                break;
            }
        }
    }
    if let Some(skel) = skel_for_probe {
        for ix in [0_usize, 100, 101, 102, 103] {
            if let Some(m) = skel.bind_transforms.get(ix) {
                let mat = bevy::math::Mat4::from_cols_array(m);
                let (_, _, t) = mat.to_scale_rotation_translation();
                let name = skel.joints.get(ix).cloned().unwrap_or_default();
                println!("  [{ix}] {name} bind_t = {t:?}");
            }
        }
    }

    // Cross-check: compare blend_shapes / blend_shape_targets read via
    // /Skel/... vs /HumanFemale_Group/... composed paths.
    println!();
    println!("== blend_shapes via /Skel composition ==");
    for mp in [
        "/Skel/Geometry/HumanFemale/Geom/Body/Body_sbdv",
        "/Skel/Geometry/HumanFemale/Geom/Face/Mouth/LowerMouth/LowerTeeth/LLowerTooth1_sbdv",
    ] {
        if let Ok(p) = openusd::sdf::Path::new(mp) {
            if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(&stage, &p) {
                println!(
                    "  {mp} → {} blend_shape_targets, {} blend_shapes (names)",
                    b.blend_shape_targets.len(),
                    b.blend_shapes.len()
                );
            } else {
                println!("  {mp} → no binding");
            }
        }
    }

    // Body mesh point range — tells us whether points are in
    // skel-world (centered around hips at z≈70cm) or mesh-local
    // (centered around origin).
    // Blendshape census.
    println!();
    println!("== blendshape census ==");
    let mut bs_meshes = 0usize;
    let mut bs_total = 0usize;
    let mut bs_max_per_mesh = 0usize;
    let mut bs_sparse = 0usize;
    let mut bs_dense = 0usize;
    let mut bs_max_offsets = 0usize;
    let mut printed_examples = 0;
    fn probe_bs(
        stage: &openusd::Stage,
        prim: &Path,
        bs_meshes: &mut usize,
        bs_total: &mut usize,
        bs_max_per_mesh: &mut usize,
        bs_sparse: &mut usize,
        bs_dense: &mut usize,
        bs_max_offsets: &mut usize,
        printed_examples: &mut usize,
    ) {
        let tn = stage.field::<String>(prim.clone(), "typeName").ok().flatten().unwrap_or_default();
        if tn == "Mesh" {
            if let Ok(Some(b)) = usd_schemas::skel::read_skel_binding(stage, prim) {
                if !b.blend_shape_targets.is_empty() {
                    *bs_meshes += 1;
                    *bs_total += b.blend_shape_targets.len();
                    *bs_max_per_mesh = (*bs_max_per_mesh).max(b.blend_shape_targets.len());
                    if *printed_examples < 2 {
                        println!(
                            "  {} → {} blend_shape_targets, {} blend_shapes (names)",
                            prim.as_str(),
                            b.blend_shape_targets.len(),
                            b.blend_shapes.len(),
                        );
                        *printed_examples += 1;
                    }
                    // Sample a few targets.
                    for t in b.blend_shape_targets.iter().take(3) {
                        let bs_path = match openusd::sdf::Path::new(t) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if let Ok(Some(bs)) = usd_schemas::skel::read_blend_shape(stage, &bs_path) {
                            if bs.point_indices.is_empty() {
                                *bs_dense += 1;
                            } else {
                                *bs_sparse += 1;
                            }
                            *bs_max_offsets = (*bs_max_offsets).max(bs.offsets.len());
                        }
                    }
                }
            }
        }
        for c in stage.prim_children(prim.clone()).unwrap_or_default() {
            if let Ok(cp) = prim.append_path(c.as_str()) {
                probe_bs(stage, &cp, bs_meshes, bs_total, bs_max_per_mesh, bs_sparse, bs_dense, bs_max_offsets, printed_examples);
            }
        }
    }
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = openusd::sdf::Path::abs_root().append_path(n.as_str()) {
            probe_bs(&stage, &p, &mut bs_meshes, &mut bs_total, &mut bs_max_per_mesh, &mut bs_sparse, &mut bs_dense, &mut bs_max_offsets, &mut printed_examples);
        }
    }
    println!(
        "blendshape summary: {bs_meshes} meshes have blendshapes, total target refs={bs_total}, max-per-mesh={bs_max_per_mesh}"
    );
    println!("sampled targets: dense={bs_dense} sparse={bs_sparse} max_offsets={bs_max_offsets}");

    println!();
    println!("== mesh point space probe ==");
    for mp in [
        "/Skel/Geometry/HumanFemale/Geom/Body/Body_sbdv",
        "/Skel/Geometry/HumanFemale/Geom/Body/Nails/LFingerNails/ThumbNail_sbdv",
        "/Skel/Geometry/ShoesHumanFlats/Geom/LShoe/Body/ShoeBody_sbdv",
    ] {
        let prim = match openusd::sdf::Path::new(mp) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if let Ok(Some(m)) = usd_schemas::geom::read_mesh(&stage, &prim) {
            let mut mn = [f32::INFINITY; 3];
            let mut mx = [f32::NEG_INFINITY; 3];
            for p in &m.points {
                for i in 0..3 {
                    if p[i] < mn[i] { mn[i] = p[i]; }
                    if p[i] > mx[i] { mx[i] = p[i]; }
                }
            }
            let cx = (mn[0] + mx[0]) * 0.5;
            let cy = (mn[1] + mx[1]) * 0.5;
            let cz = (mn[2] + mx[2]) * 0.5;
            println!(
                "  {mp} center=({cx:.2}, {cy:.2}, {cz:.2}) extent=({:.2}, {:.2}, {:.2})",
                mx[0] - mn[0],
                mx[1] - mn[1],
                mx[2] - mn[2],
            );
        }
    }

    // Probe hair, nails properly with full discovery.
    println!();
    println!("== full skinned-mesh census ==");
    fn census(
        stage: &openusd::Stage,
        prim: &Path,
        out: &mut Vec<(Path, Option<usd_schemas::skel::ReadSkelBinding>, usize)>,
    ) {
        let tn = stage
            .field::<String>(prim.clone(), "typeName")
            .ok()
            .flatten()
            .unwrap_or_default();
        if tn == "Mesh" {
            let binding = usd_schemas::skel::read_skel_binding(stage, prim).ok().flatten();
            let pts = usd_schemas::geom::read_mesh(stage, prim)
                .ok()
                .flatten()
                .map(|m| m.points.len())
                .unwrap_or(0);
            out.push((prim.clone(), binding, pts));
        }
        for c in stage.prim_children(prim.clone()).unwrap_or_default() {
            if let Ok(cp) = prim.append_path(c.as_str()) {
                census(stage, &cp, out);
            }
        }
    }
    let mut all = Vec::new();
    for n in stage.root_prims().unwrap_or_default() {
        if let Ok(p) = openusd::sdf::Path::abs_root().append_path(n.as_str()) {
            census(&stage, &p, &mut all);
        }
    }
    let total = all.len();
    let with_binding = all.iter().filter(|(_, b, _)| b.is_some()).count();
    // Purpose filter — what % go through proxy/guide?
    let mut by_purpose: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();
    for (path, _, _) in &all {
        let purpose = usd_schemas::geom::read_purpose(&stage, path)
            .ok()
            .unwrap_or_else(|| "default".into());
        *by_purpose.entry(purpose).or_insert(0) += 1;
    }
    println!("mesh purpose distribution: {by_purpose:?}");
    let _ = std::fs::write(
        "/tmp/all_meshes.txt",
        all.iter()
            .map(|(p, _, _)| p.as_str().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    println!("total mesh prims: {total}, with SkelBindingAPI: {with_binding}, without: {}", total - with_binding);
    println!("first 8 unbound meshes:");
    for (path, b, pts) in all.iter().filter(|(_, b, _)| b.is_none()).take(8) {
        println!("  {} ({pts} pts) {b:?}", path.as_str());
    }

    // Check how many bound meshes have GeomSubsets (which our build
    // routes to spawn_mesh_with_subsets, BYPASSING the skin path).
    let mut subsetted_skinned = 0usize;
    let mut subsetted_paths = Vec::new();
    for (path, _, _) in all.iter().filter(|(_, b, _)| b.is_some()) {
        if let Ok(Some(m)) = usd_schemas::geom::read_mesh(&stage, path) {
            if !m.subsets.is_empty() {
                subsetted_skinned += 1;
                if subsetted_paths.len() < 5 {
                    subsetted_paths.push(path.clone());
                }
            }
        }
    }
    println!("bound meshes with subsets (skin gets bypassed): {subsetted_skinned}");
    for p in &subsetted_paths {
        println!("  {}", p.as_str());
    }

    println!();
    println!("== inherited skel:joints walk ==");
    use openusd::sdf::Value as V;
    let mut cur = Path::new("/Skel/Geometry/HumanFemale/Geom/Body/Body_sbdv").unwrap();
    loop {
        let attr = cur.append_property("skel:joints").unwrap();
        let v = stage.field::<V>(attr, "default").ok().flatten();
        let count = match v {
            Some(V::TokenVec(ref t)) | Some(V::StringVec(ref t)) => Some(t.len()),
            _ => None,
        };
        println!("  {} → skel:joints {:?}", cur.as_str(), count);
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    println!();
    println!("== geomBindTransform probe ==");
    use openusd::sdf::Value;
    for mp in [
        "/Skel/Geometry/HumanFemale/Geom/Body/Body_sbdv",
        "/Skel/Geometry/HumanFemale/Geom/Body/Nails/LFingerNails/ThumbNail_sbdv",
    ] {
        let prim = Path::new(mp).unwrap();
        let attr = prim.append_property("primvars:skel:geomBindTransform").unwrap();
        let v = stage.field::<Value>(attr, "default").ok().flatten();
        println!("  {mp} → primvars:skel:geomBindTransform = {:?}", v.is_some());
        if let Some(val) = v {
            match val {
                Value::Matrix4d(m) => {
                    println!("    Matrix4d row0: {:?}", &m[0..4]);
                    println!("    Matrix4d row1: {:?}", &m[4..8]);
                    println!("    Matrix4d row2: {:?}", &m[8..12]);
                    println!("    Matrix4d row3: {:?}", &m[12..16]);
                }
                other => println!("    other variant: {other:?}"),
            }
        }
    }
}
