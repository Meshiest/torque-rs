//! `Player::setPosition` and what it calls: `set_euler`, the affine inverse, `SceneObject::resetWorldBox`; the scaled box; and the eye,
//! mount and muzzle transforms over the host's [`Skeleton`].
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `T3D/player.cpp` and `T3D/shapeBase.cpp`
//! (each Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.); the code here is this crate's own.

use torque_collide::math::{Box3, F3, Matrix, matrix_product, mul_box, set_euler};

use super::PlayerState;
use super::host::{PlayerHost, Skeleton};

/// The affine inverse (through the [`library`](crate::library) table): the 3x3 part transposed, the translation turned back
/// by it; row 3 untouched.
pub fn affine_inverse(m: &Matrix) -> Matrix {
    (crate::library::get().affine_inverse)(m)
}

/// `MatrixF::affineInverse` in plain arithmetic ([`torque_collide::math::matrix::affine_inverse`]):
/// [`PLAIN`](crate::library::PLAIN)'s entry.
pub use torque_collide::math::matrix::affine_inverse as plain_affine_inverse;

impl PlayerState {
    /// `Player::onScaleChanged`: the scaled box is the object box times the scale per axis. A subclass that sizes its
    /// collision box differently writes `scaled_box` itself after a scale or box change.
    pub fn update_scaled_box(&mut self) {
        self.scaled_box = super::ext::scaled(self.obj_box, self.scale);
    }

    /// `SceneObject::resetWorldBox`: the object box times the scale, moved by the transform; a box with any max below its
    /// min (or unordered with it: a NaN) becomes `(-1, -1, -1)..(1, 1, 1)`; then the sphere around it
    /// (`torque_collide::math::world_sphere`).
    pub fn reset_world_box(&mut self) {
        let mut w = mul_box(
            &self.obj_to_world,
            &super::ext::scaled(self.obj_box, self.scale),
        );
        let invalid =
            |w: &Box3| !(w.max[0] >= w.min[0] && w.max[1] >= w.min[1] && w.max[2] >= w.min[2]);
        if invalid(&w) {
            w = Box3::new([-1.0; 3], [1.0; 3]);
        }
        self.world_box = w;
        (self.world_center, self.world_radius) = torque_collide::math::world_sphere(&w);
    }

    /// The not-mounted body of `setPosition` without the host: the matrix `set_euler(0, 0, rot.z)` with the translation, the
    /// inverse, the world box, the rotation stored.
    pub fn set_transform_raw(&mut self, pos: F3, rot: F3) {
        let mut m = set_euler([0.0, 0.0, rot[2]]);
        m[3] = pos[0];
        m[7] = pos[1];
        m[11] = pos[2];
        self.apply_matrix(&m, rot);
    }

    /// `setPosition(pos, rot)` without the host's rebin, for the host's `setTransform`: mounted (with the
    /// mount's `getMountTransform(node)` given), that transform times `set_euler(0, 0, rot.z)` and `pos` unused; else the position.
    pub fn place(&mut self, pos: F3, rot: F3, mount_transform: Option<Matrix>) {
        match (self.mount, mount_transform) {
            (Some(_), Some(mm)) => {
                let r = set_euler([0.0, 0.0, rot[2]]);
                let m = matrix_product(&mm, &r);
                self.apply_matrix(&m, rot);
            }
            _ => self.set_transform_raw(pos, rot),
        }
    }

    fn apply_matrix(&mut self, m: &Matrix, rot: F3) {
        self.world_to_obj = affine_inverse(m);
        self.obj_to_world = *m;
        self.reset_world_box();
        self.rot = rot;
    }
}

/// `Player::setPosition(pos, rot)`. Mounted: the mount's node transform (`getMountTransform`, of
/// the mount, which the host answers through `mount_transform`) times `set_euler(0, 0, rot.z)` and `pos` is ignored.
pub fn set_position(
    s: &mut PlayerState,
    pos: F3,
    rot: F3,
    mount_transform: Option<Matrix>,
    host: &mut dyn PlayerHost,
) {
    s.place(pos, rot, mount_transform);
    host.moved(s);
}

/// The node transform with its translation scaled by the object's scale.
fn scaled_node(n: &Matrix, scale: F3) -> Matrix {
    let mut t = *n;
    t[3] = n[3] * scale[0];
    t[7] = n[7] * scale[1];
    t[11] = n[11] * scale[2];
    t
}

/// `Player::getEyeTransform`: `T = set_euler(0,0,head.z) * set_euler(head.x,0,0)` (a single-precision product), the eye node's
/// translation scaled into `T`'s, then `objToWorld * T`. A server animates the nodes first (Torque3D's `getEyeBaseTransform`).
pub fn eye_transform(s: &PlayerState, skel: &mut dyn Skeleton) -> Matrix {
    let p = set_euler([s.head[0], 0.0, 0.0]);
    let y = set_euler([0.0, 0.0, s.head[2]]);
    let mut t = matrix_product(&y, &p);
    if skel.has_shape() {
        skel.animate();
        let eye = skel.eye_node();
        if eye != -1
            && let Some(n) = skel.node_transform(eye)
        {
            let n = scaled_node(&n, s.scale);
            t[3] = n[3];
            t[7] = n[7];
            t[11] = n[11];
        }
    }
    matrix_product(&s.obj_to_world, &t)
}

/// `ShapeBase::getMountTransform(point)`: with a node for `point` (< 32) and a shape, `objToWorld * N` where `N` is the
/// node's transform with its translation scaled; else `objToWorld`.
pub fn mount_transform(s: &PlayerState, skel: &dyn Skeleton, point: u32) -> Matrix {
    if point < 32 && skel.has_shape() {
        let node = skel.mount_node(point);
        if node != -1
            && let Some(n) = skel.node_transform(node)
        {
            let n = scaled_node(&n, s.scale);
            return matrix_product(&s.obj_to_world, &n);
        }
    }
    s.obj_to_world
}

/// What `getMuzzleTransform` needs of an image: its mount point, the muzzle node's transform in the image
/// shape, and the first-person case with its eye offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageMuzzle {
    pub mount_point: u32,
    pub muzzle: Matrix,
    pub first_person: Option<Matrix>,
}

/// `ShapeBase::getMuzzleTransform(slot)`: no image: `objToWorld`; first person: `eye * L` with `L`'s translation scaled;
/// else the mount transform, its first three columns scaled by the object scale, times the muzzle node.
pub fn muzzle_transform(
    s: &PlayerState,
    skel: &mut dyn Skeleton,
    image: Option<&ImageMuzzle>,
) -> Matrix {
    let Some(img) = image else {
        return s.obj_to_world;
    };
    if let Some(l) = &img.first_person {
        let e = eye_transform(s, skel);
        let l = scaled_node(l, s.scale);
        return matrix_product(&e, &l);
    }
    let mut m = mount_transform(s, skel, img.mount_point);
    for r in 0..4 {
        for c in 0..3 {
            m[4 * r + c] *= s.scale[c];
        }
    }
    matrix_product(&m, &img.muzzle)
}
