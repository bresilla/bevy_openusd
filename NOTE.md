# Gotchas

Things that bit us and we don't want to rediscover.

## bevy_rapier3d 0.33 writeback operator precedence

`writeback_rigid_bodies` in `plugin/systems/rigid_body.rs:481` writes
`Quat * Vec3 * Vec3`, which Rust parses left-to-right as
`(Quat * scale) * translation` — the rotation lands on the scale vector
first, then a component-wise multiply mangles the translation. Even at
scale `(1,1,1)`, rotating that vector yields mixed signs that flip
translation axes. Symptom: any dynamic body whose parent has a
non-identity rotation (e.g. every Isaac Sim / URDF asset, where the
Z-up→Y-up basis rotation sits on the asset root) ends up with its world
pose rotated through that parent rotation an extra time once physics
starts. Visuals stay where Bevy puts them, simulated bodies drift to
wrong poses. We carry a one-line patch in
`xtra/bevy_rapier3d-patched/` (wired via `[patch.crates-io]`) that
fixes it to `Quat * (scale * translation)`. Drop the patch the moment
upstream merges the fix.

## USD joint frames have separate `localRot0` / `localRot1`

A USD revolute/prismatic joint's `physics:axis` lives in the joint's
local frame, and each body reaches that frame via its own
`localRot0` / `localRot1`. Rapier's `RevoluteJointBuilder::new(axis)`
assumes axis1 == axis2 in body frames, which only holds when those two
rotations agree. Isaac Sim chains rotate 90° between consecutive links,
so the convenience builder splits the chain on every step. Build the
joint as a `GenericJoint` with `local_basis1` / `local_basis2` set to
`localRot0 * axis_remap` and `localRot1 * axis_remap` (where
`axis_remap` rotates Rapier's X onto USD's `axis`) so the locked Y/Z
perpendicular DOFs share a consistent reference frame.

## Dynamic body without mass → multibody NaN

A `RigidBody::Dynamic` with zero inertia (no `UsdMass`, no collider yet
materialised) makes the Featherstone solver compute `1/I = inf` →
panic in step with `min/max NaN`. Bodies in the agilebot get their
collider asynchronously (mesh asset loads on a later tick), and the
joint is attached the same frame the body appears, so the first
simulation step runs with mass = 0. We insert a tiny safety
`AdditionalMassProperties::MassProperties { mass: 0.001, principal_inertia: 0.0001, .. }`
on every dynamic body that lacks a `UsdMass.mass`. Authored mass /
collider density still dominates on every realistic asset.

## Pixar `GfQuat` byte order in USDC

USDA text writes quaternions as `(w, x, y, z)` but Pixar's `GfQuat<T>`
stores them in memory as `[x, y, z, w]`. The mxpv/openusd USDC reader
copied bytes in source order, producing scrambled rotations on every
asset loaded from a `.usdc` (which is most of them, including agilebot
and Isaac Sim robots). We carry the reorder in
`xtra/openusd-rs-patched/` (PR #64 merged upstream — drop the patch
once the rebased crate is published). Symptom was *all* meshes
scattered across the scene, not just rotated; the reader returned the
wrong field as the real component, so every Quat-based xform was
garbage.

## Rapier joints don't disable self-collision (PhysX/Newton do)

Industrial robot collision meshes overlap at the joint pivot by
design — `link_n`'s distal cap and `link_{n+1}`'s proximal cap
occupy the same volume so the joint sits inside material. PhysX
(`PxArticulationFlag::eDISABLE_SELF_COLLISION`) and Newton both
default to disabling collision between articulation-linked bodies for
exactly this reason. Rapier's `MultibodyJoint` does *not* — the
contact solver generates impulses every step trying to separate the
overlapping meshes while the joint constraint pulls them back, and
the chain links visibly jump every frame. Call
`joint.set_contacts_enabled(false)` (or `joint.data.set_contacts_enabled(false)`
for the typed-joint wrappers) on every joint we build. Doesn't affect
collisions with non-linked bodies (table, ground, other objects).

## bevy_rapier `Compound::raw_scale_by` ignores inner rotation

`bevy_rapier3d/src/geometry/shape_views/collider_view.rs:393-403` —
when a `Collider::compound` containing a cylinder gets scaled by
`apply_collider_scale` (every frame, reading `GlobalTransform.scale`),
the inner shape gets scaled in **world axes** applied to its **raw
(un-rotated) frame**. So a Y-aligned cylinder wrapped in a compound
that rotates it to point along Z gets its HEIGHT (still Y in raw
frame) multiplied by the world-Y scale, not the world-Z scale.
Symptom: the Rapier debug-render wireframe of a wheel cylinder
doesn't match the visual cylinder mesh — collider is huge along the
wrong axis. NVIDIA PhysX dodges this by baking per-axis scale into
the cylinder shape at parse time and never auto-scaling later
(`omni.usdphysics/plugins/Collision.cpp`). We do the same in
`crates/bevy_openusd_rapier/src/colliders.rs`: read the entity
scale, pick `scale[axis_index]` for height and `max(other two)` for
radius, build `Collider::cylinder(baked_half_height, baked_radius)`,
and insert `ColliderScale::Absolute(Vec3::ONE)` to suppress
bevy_rapier's broken auto-scale path.

## Rapier `Collider::cylinder` is hard-coded Y-aligned

`Collider::cylinder(half_height, radius)` always produces a Y-axis
cylinder; there's no per-shape axis parameter. USD/URDF wheels are
typically authored with `axis = X` (the wheel rotates around X), and
naively dropping the cylinder in makes the wheel lie flat on the
ground instead of standing on its tread. Wrap the cylinder in a
`Collider::compound([(Vec3::ZERO, Quat::from_rotation_arc(Vec3::Y, axis), cyl)])`
when the authored axis isn't already Y. Capsules don't have this
problem — `Collider::capsule(p1, p2, r)` takes two endpoints and
naturally aligns to the axis vector.

## Omniverse shaders lie about `info:id` — pick the dialect from the connection

`UsdShade` materials have three possible surface outputs:
`outputs:surface` (UsdPreviewSurface), `outputs:mtlx:surface`
(MaterialX), `outputs:mdl:surface` (NVIDIA MDL). The reader's
instinct is to dispatch the input vocabulary by the shader's
`info:id`, but Omniverse exporters set `info:id="UsdPreviewSurface"`
on shaders whose actual inputs are pure OmniPBR
(`diffuse_color_constant`, `diffuse_texture`, …) for "compatibility."
Trusting `info:id` then reads the wrong attribute names, every
channel returns `None`, and the material falls through to default
white. Pick the input vocabulary by **which output dialect carried
the connection**, not by `info:id`. AgileX Scout and Jackal both
have this dual-stack pattern — three of Jackal's six materials
silently rendered with no color before the fix.

## Isaac Sim shaders declare themselves via `info:mdl:sourceAsset`

Standard USD shaders set `info:id = "UsdPreviewSurface"` and that's
how dispatch chooses the input mapping. Isaac Sim's exported assets
often skip `info:id` entirely and only set
`info:mdl:sourceAsset = @OmniPBR.mdl@` plus
`info:mdl:sourceAsset:subIdentifier = "OmniPBR"`. Match on either
`info:id`, the `subIdentifier`, or the basename of the MDL source
asset path so OmniPBR-style shaders aren't silently skipped (which
falls all the way through to a default-color StandardMaterial).
Isaac Sim's S3 mirror also doesn't ship texture files for most
single-file robots (Jackal/Dingo/Jetbot/Create3/Turtlebot all have
zero `.png/.jpg` files on disk) — those textures live in the paid
Omniverse Nucleus server, so the assets render with authored solid
colors and that's the best we can do without an Omniverse account.

## Default body damping must stay low for wheeled robots

`Damping { angular_damping: 20.0 }` settles an industrial-arm chain
nicely but locks the wheels of a mobile robot — contact friction
then grips and the chassis launches off the ground. Keep defaults
gentle (`linear=0.1, angular=0.5`) and rely on
`set_contacts_enabled(false)` between jointed bodies for jitter
control. Don't try to substitute joint friction for joint motors
on revolute axes either: a `motor_velocity(0.0, factor)` "friction"
motor pushes back against solver velocity noise and creates its own
feedback jitter at the wrist link.

## Don't pre-multiply primitive collider sizes by `metersPerUnit`

USD primitive geometry attributes (`Cube.size`, `Sphere.radius`,
`Cylinder.radius`/`height`, `Capsule.radius`/`height`) live in scene
units and must NOT be pre-multiplied by `metersPerUnit` in the
shape constructor. The scene root carries a Transform with scale =
`(mPU, mPU, mPU)` so vertex positions land in metres after
propagation, and bevy_rapier's `apply_collider_scale` system
multiplies the entity's `GlobalTransform.scale` into the collider
shape every frame. Pre-multiplying in our reader stacks both, so a
Scout V2 chassis (`mPU=0.01`, authored cube `size=1`, parent xform
scale `92.5`) collapses to `1 × 0.01 × 0.01 × 92.5 ≈ 9 mm` — every
collider winds up roughly `mPU²` of its real size. Translations,
joint anchors, mass-related quantities (kg, kg·m²) etc. still need
the explicit mPU multiply because they're not on the scene-root
xform path; only the shape-primitive `radius`/`height`/`size`
attributes get the auto-scale.

## bevy_rapier auto-applies collider scale

`apply_collider_scale` in bevy_rapier reads each entity's
`GlobalTransform.scale` and bakes it into the Rapier collider shape
every frame. Don't call `Collider::set_scale` ourselves — doing both
double-applies the scale (mm→m parent xforms turn into 1e-6× colliders).
