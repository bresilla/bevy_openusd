//! M26 integration test: `UsdLuxLightAPI.light:link`, `shadow:link`,
//! `light:filters` relationships round-trip into `LightCommon`.

use openusd::sdf::Path;
use usd_schemas::lux::{read_light, ReadLight};

fn common_of(l: &ReadLight) -> &usd_schemas::lux::LightCommon {
    match l {
        ReadLight::Distant(d) => &d.common,
        ReadLight::Sphere(s) => &s.common,
        ReadLight::Rect(r) => &r.common,
        ReadLight::Disk(d) => &d.common,
        ReadLight::Cylinder(c) => &c.common,
        ReadLight::Dome(d) => &d.common,
    }
}

#[test]
fn reads_light_linking_rels() {
    let stage = openusd::Stage::open("tests/stages/light_linking.usda")
        .expect("fixture parses");

    let key = read_light(&stage, &Path::new("/World/KeyLight").unwrap())
        .unwrap()
        .unwrap();
    let fill = read_light(&stage, &Path::new("/World/FillLight").unwrap())
        .unwrap()
        .unwrap();

    let kc = common_of(&key);
    let fc = common_of(&fill);

    println!(
        "\n---- KeyLight linking ----\n  \
         light_link_targets = {:?}\n  \
         shadow_link_targets = {:?}\n  \
         light_filters = {:?}",
        kc.light_link_targets, kc.shadow_link_targets, kc.light_filters
    );
    println!(
        "\n---- FillLight linking ----\n  \
         light_link_targets = {:?}\n  \
         shadow_link_targets = {:?}\n  \
         light_filters = {:?}",
        fc.light_link_targets, fc.shadow_link_targets, fc.light_filters
    );

    assert_eq!(kc.light_link_targets, vec!["/World/StageLeft".to_string()]);
    assert_eq!(kc.shadow_link_targets, vec!["/World/StageRight".to_string()]);
    assert!(kc.light_filters.is_empty());

    // FillLight authored no linking — everything empty.
    assert!(fc.light_link_targets.is_empty());
    assert!(fc.shadow_link_targets.is_empty());
    assert!(fc.light_filters.is_empty());
}
