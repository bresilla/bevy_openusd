# bevy_openusd

> This project was supported by **[Wageningen University and Research (WUR)](https://www.wur.nl/)**.
> A lot of the code was carved out of an internal repo to be open-sourced. Special thanks
> to the team for letting it ship.

A [Bevy](https://bevy.org) 0.18 plugin that loads [OpenUSD](https://openusd.org)
(`.usda` / `.usdc` / `.usdz`) files as native Bevy scenes, plus an interactive
viewer/editor binary that ships in the same package.

The loader composes a stage through [`mxpv/openusd`](https://github.com/mxpv/openusd)
(pure-Rust USD reader — composition / sublayers / variants / payloads / references
all handled upstream) and projects the result into ECS: one entity per composed
prim, geometry + materials + skinning + animation attached as Bevy components.
Stage units (`metersPerUnit` / `kilogramsPerUnit`) and basis (`upAxis = Z` →
Bevy Y-up) are applied once at the import boundary so consumers always see SI.

The focus is **simulation-grade fidelity for robotics assets** (Isaac Sim,
Omniverse, URDF→USD pipelines), not just rendering — physics, articulations,
collision groups, materials, and reference compositions all decode end-to-end
on real production scenes.

![Agilebot GBT-C5A 6-DOF cobot loaded from an Isaac Sim USDC asset, with the physics overlay (Y) showing per-joint frame triads, axis arrows, and revolute limit arcs](docs/agilebot.jpeg)

The screenshot above is the [Agilebot GBT-C5A](https://github.com/sh-agilebot/agilebot_isaac_usd_assets)
6-DOF collaborative arm — a real Isaac Sim production asset (binary USDC,
5 layers composed, 165 prims, 7 rigid bodies, 7 joints, 1 articulation root)
loaded with `make run ARGS="path/to/gbt-c5a.usd"`. The physics overlay (toggle
with `Y`) draws per-joint body frame triads, fuchsia axis arrows, kind-coloured
connection lines, and the revolute limit arcs (orange ellipses). Backend-neutral:
any downstream Bevy physics engine (Rapier, Avian, …) can consume the marker
components without `bevy_openusd` taking a dep on a specific solver.

![Pixar's reference Kitchen_set scene loaded into bevy_openusd, with the Stage Info panel showing 229 composed layers, 2745 prims, 1788 meshes, and 418 variants](docs/kitchen_set.png)

The kitchen scene is Pixar's reference [Kitchen_set](https://openusd.org/release/dl_kitchen_set.html)
asset — the canonical USD test set used across the industry for loader
correctness. Bundled USDZ archive composing **229 sublayers / references**,
**418 variants**, **1,788 meshes**, **2,745 ECS entities** after projection,
and an **11.91 m scene diagonal** at the spec-default 0.01 metersPerUnit.
Loaded with `make run ARGS="assets/external/Kitchen_set.usdz"`. The Stage
Info panel (`I`) shows the composition tally + per-domain counters (lights,
instances, skel, render settings, physics, custom attrs, subdivisions, light
linking, clips, spatial audio, procedurals) so you can sanity-check what the
loader actually decoded. The auto-framed bounding box in the screenshot is the
white wireframe.

![Animated hummingbird from Apple's AR Quick Look gallery — UsdSkel skeleton + skinned mesh + bone animation playing back in the bevy_openusd viewer](docs/hummingbird.gif)

The hummingbird is one of the [AR Quick Look sample assets](https://developer.apple.com/augmented-reality/quick-look/)
Apple ships for testing — a small USDZ with a UsdSkel skeleton, a skinned
mesh bound to that skeleton, and a baked bone animation. The viewer's
animation clock auto-starts when the loaded asset has any animated content,
plays back through the bone hierarchy each frame (`drive_skel_animations`
system), and the result is the wing flap + body bob you see above. Toggle
the bone overlay with `B` to see the skeleton's joint chain rendered as
gizmo lines while the skin animates around it.

## Run the viewer

```bash
cargo run -- path/to/scene.usd[abz]
# or via the Makefile (handles DISPLAY + nix shell wrapping)
make run ARGS="path/to/scene.usd[abz]"
```

`cargo run` with no args opens the bundled `assets/materials.usda` demo. The
viewer is the dogfood target during plugin development — file picker, prim
tree, info panel, gizmo overlays, animation scrub, variant selection, hot
reload, command palette (`Ctrl+K`).

Keyboard cheat-sheet:

| Key | Action |
|---|---|
| `T` | Prim tree panel |
| `I` | Info panel (composition stats, per-domain counters) |
| `O` | Overlays panel |
| `?` | Keys panel |
| `G` | Toggle ground grid |
| `X` | Toggle world axes |
| `P` | Toggle per-prim markers |
| `B` | Toggle skeleton bones |
| `Y` | Toggle physics gizmos (joint anchors / axes / limits / articulations / gravity) |
| `R` | Reload current stage |
| `Ctrl+K` / `Ctrl+P` | Command palette |

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

After `SceneRoot(asset.scene.clone())` spawns, the loader's projection appears
in ECS — every composed prim is an entity carrying `UsdPrimRef { path }` plus
domain-specific markers (`Mesh3d`, `MeshMaterial3d`, `Light`, `UsdSkelRoot`,
`UsdRigidBody`, `UsdPhysicsJoint`, `UsdArticulationRoot`, `UsdCustomAttrs`, …).
Adapter crates pick up the marker components and translate them to engine-side
representations (Rapier / Avian / your own).

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
├── docs/                screenshots + design notes
└── xtra/                external checkouts (openusd-rs fork, Pixar OpenUSD reference, …)
```

## License

MIT.
