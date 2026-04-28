//! Smoke-test the text-mode SkelAnimation parser against Pixar's
//! `HumanFemale.walk.usd` — the reference asset openusd-rs's USDA
//! parser can't open. Prints joint/timeline counts and a few sample
//! values so we can spot-check the parsing.

use std::path::Path;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "assets/UsdSkelExamples/HumanFemale/HumanFemale.walk.usd".to_string());
    let text = std::fs::read_to_string(Path::new(&path)).expect("read .usda text");
    let anims = usd_schemas::skel_anim_text::scan_skel_animations(&text);
    println!("scanned {} SkelAnimation prim(s)", anims.len());
    for a in &anims {
        println!(
            "  prim={} joints={} blendShapes={} translation_keys={} rotation_keys={} scale_keys={} blendWeight_keys={}",
            a.prim_name,
            a.joints.len(),
            a.blend_shapes.len(),
            a.translations.len(),
            a.rotations.len(),
            a.scales.len(),
            a.blend_shape_weights.len(),
        );
        if let Some((t, vals)) = a.rotations.iter().next() {
            println!(
                "  first rotation timecode={:.1} count={} head={:?}",
                t.0,
                vals.len(),
                &vals[..3.min(vals.len())]
            );
        }
        if let Some((t, vals)) = a.translations.iter().next() {
            println!(
                "  first translation timecode={:.1} count={} head={:?}",
                t.0,
                vals.len(),
                &vals[..3.min(vals.len())]
            );
        }
        if let Some((t, vals)) = a.scales.iter().next() {
            println!(
                "  first scale timecode={:.1} count={} head={:?}",
                t.0,
                vals.len(),
                &vals[..3.min(vals.len())]
            );
        }
        if let Some((t, vals)) = a.blend_shape_weights.iter().next() {
            let nonzero = vals.iter().filter(|w| w.abs() > 1e-4).count();
            let total: f32 = vals.iter().map(|w| w.abs()).sum();
            let max = vals
                .iter()
                .copied()
                .map(|w| w.abs())
                .fold(0.0f32, f32::max);
            println!(
                "  blend weights at timecode={:.1} count={} nonzero={} sum_abs={:.3} max_abs={:.3}",
                t.0, vals.len(), nonzero, total, max,
            );
            // Show top 5 by absolute value.
            let mut idx: Vec<usize> = (0..vals.len()).collect();
            idx.sort_by(|&a, &b| vals[b].abs().partial_cmp(&vals[a].abs()).unwrap_or(std::cmp::Ordering::Equal));
            for i in idx.iter().take(5) {
                println!(
                    "    [{i}] {} = {:.4}",
                    a.blend_shapes.get(*i).map(|s| s.as_str()).unwrap_or("?"),
                    vals[*i]
                );
            }
        }
    }
}
