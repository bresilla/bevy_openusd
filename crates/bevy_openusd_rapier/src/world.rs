//! `PhysicsWorld` — the single resource that owns every Rapier set
//! and the integration pipeline. Bevy talks to Rapier exclusively
//! through this type; the rest of the adapter just populates it.
//!
//! **f64 throughout.** Rapier's `Real` is `f64` because we depend on
//! `rapier3d-f64`. Conversion happens at the Bevy boundary
//! (`Transform`/`Vec3`/`Quat` are `f32`).

use std::collections::HashMap;

use bevy::prelude::*;
use rapier3d_f64::prelude::*;

/// All Rapier state for the loaded USD scene. Exactly one of these
/// in the world.
#[derive(Resource)]
pub struct PhysicsWorld {
    pub gravity: Vector,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub islands: IslandManager,
    pub broad_phase: BroadPhaseBvh,
    pub narrow_phase: NarrowPhase,
    pub bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub impulse_joints: ImpulseJointSet,
    pub multibody_joints: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
    /// Bevy entity → Rapier rigid-body handle. Lets writeback look up
    /// which body to copy into the entity's Transform each tick.
    pub entity_to_body: HashMap<Entity, RigidBodyHandle>,
    /// Bevy entity → Rapier collider handle.
    pub entity_to_collider: HashMap<Entity, ColliderHandle>,
}

impl Default for PhysicsWorld {
    fn default() -> Self {
        let mut integration_parameters = IntegrationParameters::default();
        // Soft `ImpulseJoint` constraints converge harder per tick;
        // matters for vehicles whose front-axle joints fall back to
        // impulse when the multibody solver can't take all of them.
        integration_parameters.num_solver_iterations = 16;
        integration_parameters.num_internal_pgs_iterations = 4;
        Self {
            gravity: Vector::new(0.0, -9.81, 0.0),
            integration_parameters,
            physics_pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            entity_to_body: HashMap::new(),
            entity_to_collider: HashMap::new(),
        }
    }
}

impl PhysicsWorld {
    /// One Rapier integration step using the resource's current
    /// gravity + parameters. No event handlers.
    pub fn step(&mut self) {
        self.physics_pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
    }
}

/// Whether `step_physics` ticks this frame. Flipped by the play
/// button on the viewer's ribbon.
#[derive(Resource, Clone, Copy, Debug)]
pub struct PhysicsActive(pub bool);

impl Default for PhysicsActive {
    fn default() -> Self {
        Self(false)
    }
}

/// Step the world once per frame when `PhysicsActive(true)`.
pub fn step_physics(active: Res<PhysicsActive>, mut world: ResMut<PhysicsWorld>) {
    if active.0 {
        world.step();
    }
}
