//! Pure-Rust UsdPhysics → Rapier conversion.
//!
//! Produces `RigidBody` / `Collider` / `MultibodyJoint` / `ImpulseJoint`
//! entries directly into caller-owned Rapier sets. No Bevy dependency.
//!
//! The intent is that **all** USD→Rapier translation logic lives in
//! one place. Bevy hosts (`bevy_openusd_rapier`) wrap each call in an
//! ECS system that pulls authored data off entity components; non-Bevy
//! hosts (gearbox) call the same functions directly when walking a
//! USD stage.
//!
//! # API shape
//!
//! Each top-level function takes:
//! - The Rapier sets it needs to mutate (passed as separate `&mut` so
//!   borrows compose cleanly across multiple inserts).
//! - A pure-Rust description of the authored intent (Pose, mass,
//!   axis, decoded `openusd::physics::Read*` records).
//!
//! It returns the handle inserted, leaving caller-side bookkeeping
//! (entity ↔ handle maps) up to the caller.
//!
//! # Modules
//!
//! - [`bodies`] — `RigidBodyBuilder` from authored rigid-body data.
//! - [`colliders`] — `ColliderBuilder` for primitive shapes and
//!   mesh-derived approximations.
//! - [`joints`] — Revolute / prismatic / fixed / spherical joint
//!   construction with axis remap and same-basis vs differing-basis
//!   routing. Inserts into `MultibodyJointSet` or `ImpulseJointSet`.

pub mod bodies;
pub mod colliders;
pub mod joints;
