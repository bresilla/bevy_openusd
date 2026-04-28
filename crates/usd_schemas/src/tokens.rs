//! String constants for USD schema type names and attribute names we emit.
//!
//! Not exhaustive — only what this converter actually writes.

// Geometry prim type names
pub const T_XFORM: &str = "Xform";
pub const T_SCOPE: &str = "Scope";
pub const T_MESH: &str = "Mesh";
pub const T_CUBE: &str = "Cube";
pub const T_SPHERE: &str = "Sphere";
pub const T_CYLINDER: &str = "Cylinder";
pub const T_CAPSULE: &str = "Capsule";
pub const T_GEOM_SUBSET: &str = "GeomSubset";

// Physics prim type names
pub const T_PHYSICS_SCENE: &str = "PhysicsScene";
pub const T_PHYSICS_FIXED_JOINT: &str = "PhysicsFixedJoint";
pub const T_PHYSICS_REVOLUTE_JOINT: &str = "PhysicsRevoluteJoint";
pub const T_PHYSICS_PRISMATIC_JOINT: &str = "PhysicsPrismaticJoint";
pub const T_PHYSICS_JOINT: &str = "PhysicsJoint";
pub const T_PHYSICS_SPHERICAL_JOINT: &str = "PhysicsSphericalJoint";

// Shade prim type names
pub const T_MATERIAL: &str = "Material";
pub const T_SHADER: &str = "Shader";

// API schemas
pub const API_RIGID_BODY: &str = "PhysicsRigidBodyAPI";
pub const API_MASS: &str = "PhysicsMassAPI";
pub const API_COLLISION: &str = "PhysicsCollisionAPI";
pub const API_MESH_COLLISION: &str = "PhysicsMeshCollisionAPI";
pub const API_ARTICULATION_ROOT: &str = "PhysicsArticulationRootAPI";
pub const API_MATERIAL_BINDING: &str = "MaterialBindingAPI";
pub const API_LIMIT: &str = "PhysicsLimitAPI";
pub const API_NEWTON_MIMIC: &str = "NewtonMimicAPI";
pub const API_NEWTON_SCENE: &str = "NewtonSceneAPI";
pub const API_NEWTON_ARTICULATION_ROOT: &str = "NewtonArticulationRootAPI";
