# bevy_openusd

A [Bevy](https://bevy.org) 0.18 plugin that loads
[OpenUSD](https://openusd.org) (`.usda` / `.usdc` / `.usdz`) files as native
Bevy scenes, plus an interactive viewer/editor binary that ships in the
same package.

The loader composes a stage through [`mxpv/openusd`](https://github.com/mxpv/openusd)
and projects it into ECS — one entity per composed prim, geometry +
materials + skinning + animation attached as Bevy components.

![Agilebot GBT-C5A 6-DOF cobot loaded from an Isaac Sim USDC asset, with the physics overlay (Y) showing per-joint frame triads, axis arrows, and revolute limit arcs](docs/agilebot.jpeg)

The screenshot above is the [Agilebot GBT-C5A](https://github.com/sh-agilebot/agilebot_isaac_usd_assets)
6-DOF collaborative arm — a real Isaac Sim production asset (binary
USDC, 5 layers composed, 165 prims, 7 rigid bodies, 7 joints, 1
articulation root) loaded with `make run ARGS="path/to/gbt-c5a.usd"`.
The physics overlay (toggle with `Y`) draws per-joint body frame triads,
fuchsia axis arrows, kind-coloured connection lines, and the revolute
limit arcs (orange ellipses). Backend-neutral: any downstream Bevy
physics engine (Rapier, Avian, …) can consume the marker components
without `bevy_openusd` taking a dep on a specific solver.

![Pixar's reference Kitchen_set scene loaded into bevy_openusd, with the Stage Info panel showing 229 composed layers, 2745 prims, 1788 meshes, and 418 variants](docs/kitchen_set.png)

The kitchen scene is Pixar's reference [Kitchen_set](https://openusd.org/release/dl_kitchen_set.html)
asset — the canonical USD test set used across the industry for loader
correctness. Bundled USDZ archive composing **229 sublayers /
references**, **418 variants**, **1,788 meshes**, **2,745 ECS entities**
after projection, and an **11.91 m scene diagonal** at the spec-default
0.01 metersPerUnit. Loaded with `make run ARGS="assets/external/Kitchen_set.usdz"`.
The Stage Info panel (`I`) shows the composition tally + per-domain
counters (lights, instances, skel, render settings, physics, custom
attrs, subdivisions, light linking, clips, spatial audio, procedurals)
so you can sanity-check what the loader actually decoded. The
auto-framed bounding box in the screenshot is the white wireframe.

THIS WAS A PROJECT SUPORTED BY WUR (WAGENINGEN UNIVERSITY AND RESEARCH). A LOT OF CODE WAS COPIED FROM THE ORIGINAL REPO TO BE OPEND SOURCED.

## Run the viewer

```bash
cargo run -- path/to/scene.usd[abz]
```

`cargo run` (no args) opens the bundled `assets/external/usdz_sample.usdz`
demo. The viewer is the dogfood target during plugin development —
file-picker, tree panel, gizmos, animation scrub, variant selection.

## Use the plugin

```rust
use bevy::prelude::*;
use bevy_openusd::{UsdAsset, UsdPlugin};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(UsdPlugin)
        .add_systems(Startup, load)
        .run();
}

fn load(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handle: Handle<UsdAsset> = asset_server.load("scene.usdz");
    commands.insert_resource(Stage(handle));
}

#[derive(Resource)]
struct Stage(Handle<UsdAsset>);
```

## Layout

```
bevy_openusd/
├── src/
│   ├── lib/             plugin: asset loader, scene projection, schema readers
│   └── bin/             viewer binary (camera, ui, overlays, …)
├── crates/
│   └── usd_schemas/     typed schema readers — slated for upstreaming into
│                        openusd-rs, so kept as a sibling crate
├── examples/            standalone tools + probe scripts
├── tests/
│   └── stages/          curated .usda fixtures for integration tests
├── assets/              hand-authored .usda demos
│   └── external/        bundled USDZ archives (Kitchen_set, HumanFemale, …)
└── xtra/                external checkouts (openusd-rs, etc.)
```

## License

MIT.
