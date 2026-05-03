//! Gizmo-based collider debug renderer — replaces `RapierDebugRenderPlugin`.
//! Iterates `PhysicsWorld.colliders` and draws each shape's
//! wireframe via `bevy::gizmos`. Driven by `ColliderDebugEnabled`.

use bevy::math::primitives::{Cuboid, Cylinder, Sphere};
use bevy::prelude::*;
use rapier3d_f64::parry::shape::TypedShape;

use crate::convert::{quat_from_d, vec3_from_d};
use crate::world::PhysicsWorld;

#[derive(Resource, Clone, Copy, Debug)]
pub struct ColliderDebugEnabled(pub bool);

impl Default for ColliderDebugEnabled {
    fn default() -> Self {
        Self(true)
    }
}

const DEBUG_COLOR: Color = Color::srgb(0.0, 0.9, 0.6);

pub fn draw_collider_gizmos(
    enabled: Res<ColliderDebugEnabled>,
    world: Res<PhysicsWorld>,
    mut gizmos: Gizmos,
) {
    if !enabled.0 {
        return;
    }
    for (_handle, collider) in world.colliders.iter() {
        let pose = collider.position();
        let translation = vec3_from_d(pose.translation);
        let rotation = quat_from_d(pose.rotation);
        let iso = Isometry3d::new(translation, rotation);

        match collider.shape().as_typed_shape() {
            TypedShape::Cuboid(c) => {
                let h = c.half_extents;
                gizmos.primitive_3d(
                    &Cuboid::new(h.x as f32 * 2.0, h.y as f32 * 2.0, h.z as f32 * 2.0),
                    iso,
                    DEBUG_COLOR,
                );
            }
            TypedShape::Ball(b) => {
                gizmos
                    .primitive_3d(&Sphere::new(b.radius as f32), iso, DEBUG_COLOR)
                    .resolution(32);
            }
            TypedShape::Cylinder(c) => {
                gizmos
                    .primitive_3d(
                        &Cylinder::new(c.radius as f32, c.half_height as f32 * 2.0),
                        iso,
                        DEBUG_COLOR,
                    )
                    .resolution(32);
            }
            TypedShape::Capsule(c) => {
                let a = vec3_from_d(c.segment.a);
                let b = vec3_from_d(c.segment.b);
                gizmos.line(translation + rotation * a, translation + rotation * b, DEBUG_COLOR);
            }
            TypedShape::ConvexPolyhedron(poly) => {
                let points = poly.points();
                for edge in poly.edges() {
                    let a = vec3_from_d(points[edge.vertices[0] as usize]);
                    let b = vec3_from_d(points[edge.vertices[1] as usize]);
                    gizmos.line(translation + rotation * a, translation + rotation * b, DEBUG_COLOR);
                }
            }
            // Trimeshes / heightfields fall through to AABB outline.
            _ => {
                let aabb = collider.shape().compute_local_aabb();
                let h = aabb.half_extents();
                gizmos.primitive_3d(
                    &Cuboid::new(h.x as f32 * 2.0, h.y as f32 * 2.0, h.z as f32 * 2.0),
                    iso,
                    DEBUG_COLOR.with_alpha(0.4),
                );
            }
        }
    }
}
