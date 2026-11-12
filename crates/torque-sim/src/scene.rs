//! What Item and Projectile share of `SceneObject`: the transform, its inverse and the world box (`resetWorldBox`), the ray
//! result both cast, the 250 ms collision queue (`queueCollision`, `notifyCollision`) and the squared length their code calls.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `scene/sceneObject.cpp` and
//! `T3D/shapeBase.cpp` (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this
//! crate's own.

use torque_collide::ObjectId;
use torque_collide::math::{Box3, F3, IDENTITY, Matrix, affine_inverse, mul_box};

// `DT` is the tick (`TickSec`), in seconds.
pub use torque_collide::types::TICK_SECONDS as DT;
pub use torque_collide::{MAX_COLLISION_QUEUE, QueuedCollision, queue_collision};

/// What `Container::castRay` answers (`RayInfo`: the object, the point and the normal) and the type mask the callers read from
/// the object at the time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub object: ObjectId,
    pub point: F3,
    pub normal: F3,
    /// `mTypeMask` of the object.
    pub type_mask: u32,
}

/// The `SceneObject` fields both classes keep: `mObjToWorld`, `mWorldToObj`, the scale, the object box, the world box and the
/// bounding sphere.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    pub obj_to_world: Matrix,
    pub world_to_obj: Matrix,
    pub scale: F3,
    pub obj_box: Box3,
    pub world_box: Box3,
    pub world_center: F3,
    pub world_radius: f32,
}

impl SceneObject {
    pub fn new(obj_box: Box3) -> SceneObject {
        let mut o = SceneObject {
            obj_to_world: IDENTITY,
            world_to_obj: IDENTITY,
            scale: [1.0; 3],
            obj_box,
            world_box: Box3::default(),
            world_center: [0.0; 3],
            world_radius: 0.0,
        };
        o.reset_world_box();
        o
    }

    /// The translation (`getPosition`: elements 3, 7 and 11).
    pub fn position(&self) -> F3 {
        [
            self.obj_to_world[3],
            self.obj_to_world[7],
            self.obj_to_world[11],
        ]
    }

    /// Stores `m` and its affine inverse, then `resetWorldBox`.
    pub fn set_matrix(&mut self, m: &Matrix) {
        self.world_to_obj = affine_inverse(m);
        self.obj_to_world = *m;
        self.reset_world_box();
    }

    /// `SceneObject::resetWorldBox`: the object box times the scale, moved by the transform; a box with any max below its min
    /// becomes `(-1, -1, -1)..(1, 1, 1)`; then the sphere around it (`torque_collide::math::world_sphere`).
    pub fn reset_world_box(&mut self) {
        let s = self.scale;
        let b = self.obj_box;
        let scaled = Box3::new(
            std::array::from_fn(|i| b.min[i] * s[i]),
            std::array::from_fn(|i| b.max[i] * s[i]),
        );
        let mut w = mul_box(&self.obj_to_world, &scaled);
        let invalid = |w: &Box3| w.max[0] < w.min[0] || w.max[1] < w.min[1] || w.max[2] < w.min[2];
        if invalid(&w) {
            w = Box3::new([-1.0; 3], [1.0; 3]);
        }
        self.world_box = w;
        (self.world_center, self.world_radius) = torque_collide::math::world_sphere(&w);
    }

    /// The view `collide` reads of this object (for the host's [`torque_collide::World`]).
    pub fn view(&self, type_mask: u32) -> torque_collide::ObjectView {
        torque_collide::ObjectView {
            obj_to_world: self.obj_to_world,
            world_to_obj: self.world_to_obj,
            scale: self.scale,
            obj_box: self.obj_box,
            world_box: self.world_box,
            world_radius: self.world_radius,
            type_mask,
            collision_disabled: false,
        }
    }
}
