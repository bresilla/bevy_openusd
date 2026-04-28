//! M23 integration test: `interpolation = "held"` metadata on a
//! time-sampled attribute produces step-function evaluation.

use openusd::sdf::Path;
use usd_schemas::anim::{eval_scalar_track, read_animated_prim, InterpMode};

#[test]
fn held_interpolation_snaps_to_lower_sample() {
    let stage = openusd::Stage::open("tests/stages/animated_held.usda")
        .expect("fixture parses");

    let linear = read_animated_prim(&stage, &Path::new("/World/LinearSpinner").unwrap())
        .expect("read ok")
        .expect("LinearSpinner animated");
    let held = read_animated_prim(&stage, &Path::new("/World/HeldSpinner").unwrap())
        .expect("read ok")
        .expect("HeldSpinner animated");

    let linear_track = linear.rotate_y.as_ref().expect("linear rotateY track");
    let held_track = held.rotate_y.as_ref().expect("held rotateY track");

    println!(
        "\n---- authored InterpMode ----\n  \
         LinearSpinner.rotateY → {:?}\n  \
         HeldSpinner.rotateY   → {:?}\n",
        linear_track.mode, held_track.mode
    );

    assert_eq!(linear_track.mode, InterpMode::Linear);
    assert_eq!(held_track.mode, InterpMode::Held);

    // Linear @ t=12 → midpoint between 0 and 90 = 45.
    // Held   @ t=12 → stays at 0 until t=24 flips to 90.
    let linear_12 = eval_scalar_track(linear_track, 12.0).unwrap();
    let held_12 = eval_scalar_track(held_track, 12.0).unwrap();
    println!("  t=12: linear={linear_12:.2}, held={held_12:.2}");
    assert!((linear_12 - 45.0).abs() < 1e-3);
    assert!(held_12.abs() < 1e-3);

    // t=24 sample boundary.
    assert!((eval_scalar_track(linear_track, 24.0).unwrap() - 90.0).abs() < 1e-3);
    assert!((eval_scalar_track(held_track, 24.0).unwrap() - 90.0).abs() < 1e-3);

    // t=36 → linear midway between 90 and 180 = 135; held still 90.
    let linear_36 = eval_scalar_track(linear_track, 36.0).unwrap();
    let held_36 = eval_scalar_track(held_track, 36.0).unwrap();
    println!("  t=36: linear={linear_36:.2}, held={held_36:.2}");
    assert!((linear_36 - 135.0).abs() < 1e-3);
    assert!((held_36 - 90.0).abs() < 1e-3);

    // Beyond end: both hold-at-last.
    let linear_100 = eval_scalar_track(linear_track, 100.0).unwrap();
    let held_100 = eval_scalar_track(held_track, 100.0).unwrap();
    assert!((linear_100 - 180.0).abs() < 1e-3);
    assert!((held_100 - 180.0).abs() < 1e-3);
}
