# bevy_openusd

A [Bevy](https://bevy.org) 0.18 plugin that loads
[OpenUSD](https://openusd.org) (`.usda` / `.usdc` / `.usdz`) files as native
Bevy scenes.

> **Status:** M0 — bootstrap. The asset loader parses a stage through
> [`mxpv/openusd`](https://github.com/mxpv/openusd) and materializes a
> `UsdAsset` carrying `defaultPrim` + `layerCount`. Scene graph
> construction, geometry, materials, and physics land in later
> milestones — see [`PLAN.md`](./docs/PLAN.md).

## Usage (M0)

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
    let handle: Handle<UsdAsset> = asset_server.load("robot.usdz");
    commands.insert_resource(Stage(handle));
}

#[derive(Resource)]
struct Stage(Handle<UsdAsset>);
```

Run the smoke example:

```bash
cargo run --example view_simple
```

## Roadmap

See [`PLAN.md`](./docs/PLAN.md) for the full implementation plan. Short version:

| Milestone | Contents |
|-----------|----------|
| **M0** | Workspace bootstrap; `UsdAsset` / `UsdLoader` skeleton. |
| **M1** | Live `Stage` projection: one entity per prim, `UsdPrimRef` component, root basis fix. |
| **M2** | Geometry — `UsdGeom.Mesh` + primitives, xformOp stack, purpose filter. |
| **M3** | Materials — `UsdPreviewSurface` → `StandardMaterial` with textures. |
| **M3.5** | USDZ support with embedded textures. |
| **M4** | Internal references, instancing, Kind-based collapse. |
| **M5** | UsdPhysics → Rapier bindings (optional feature). |
| **M6** | Variant selections, payload control, hot reload polish. |
| **M7+** | UsdSkel / animation. |

## Workspace layout

```
bevy_openusd/
├── src/                 plugin source
├── examples/            view_simple, view_usdz, view_husky (later)
├── tests/
│   └── stages/          curated .usda fixtures for integration tests
└── crates/
    └── usd_schemas/     typed Rust equivalents of Pixar's C++ schemas
                         (UsdGeom, UsdShade, UsdPhysics — shared with urdf2usd)
```

## License

MIT.
