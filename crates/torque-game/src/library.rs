//! The game library table: the arithmetic of the game classes, called through one set of function pointers.
//!
//! Like the collision and scene tables, every entry of [`GameLibrary`] is called through [`get`]. [`PLAIN`] holds this crate's
//! own code: plain `f32` arithmetic in the order Torque's source writes it, and TGE's rules where an entry is a rule (what
//! `setVelocity` keeps, whether a speed picks a moving action). A host installs another table once, before its first object,
//! with [`install`].
//!
//! Credit: this module follows TGE as Torque3D, its MIT-licensed release, keeps it, where the two agree: `math/mMatrix.cpp`,
//! `T3D/camera.cpp` and `ts/tsThread.cpp` (Copyright (c) 2012 GarageGames, LLC); `T3D/player.cpp` and `T3D/shapeBase.cpp`
//! (Copyright (c) 2012 GarageGames, LLC; Copyright (C) 2015 Faust Logic, Inc.). The code here is this crate's own.

use torque_scene::Seam;
use torque_scene::transform::Matrix;

/// A point or a vector.
pub type Point = [f32; 3];

/// The routines the game classes call through the table. Each entry names the Torque routine it stands for.
#[derive(Debug, Clone, Copy)]
pub struct GameLibrary {
    /// `Camera::setPosition`'s matrix: `Rz(yaw) * Rx(pitch)` (`MatrixF::set(EulerF)` twice, then `MatrixF::mul`), at
    /// `position`.
    pub camera_pose: fn(Point, f32, f32) -> Matrix,
    /// `Camera::setTransform`'s rotation of a matrix, `(pitch, yaw)`: `(-atan2(fz, sqrt(fx^2 + fy^2)), -atan2(-fx, fy))` of its
    /// forward column.
    pub camera_rotation: fn(&Matrix) -> (f32, f32),
    /// What `Player::setVelocity` keeps of a velocity (TGE keeps it as given).
    pub set_velocity: fn(Point) -> Point,
    /// `PlaneF::set(point, normal)`'s offset: `-dot(point, normal)`.
    pub plane_offset: fn(Point, Point) -> f32,
    /// `TSThread::getGround`'s key frame and the fraction past it, for a position `t` along `frames` ground frames.
    pub ground_frame: fn(f32, i32) -> (i32, f32),
    /// `TSThread::getGround`'s translation between two key frames: `(b - a) * k + a`.
    pub ground_lerp: fn(f32, f32, f32) -> f32,
    /// `PlayerData::getGroundInfo`'s unit direction and speed of a ground displacement; None below the least speed.
    pub ground_motion: fn(Point) -> Option<(Point, f32)>,
    /// `Player::pickActionAnimation`: whether a velocity (in object space) may pick a moving action (TGE: any velocity).
    pub moving: fn(Point) -> bool,
    /// `Player::pickActionAnimation`'s `mDot(vel, anim.dir)`, the velocity first.
    pub action_dot: fn(Point, Point) -> f32,
    /// `Player::updateAnimation`'s action rate: the velocity along the action's direction over its speed, unsigned.
    pub action_rate: fn(Point, Point, f32) -> f32,
    /// The change of a level a script callback is told (`onDamage`'s delta): `new - old`.
    pub difference: fn(f32, f32) -> f64,
    /// `ShapeBase::processTick`'s fade end: whether the elapsed time is past the fade's time plus its delay.
    pub fade_over: fn(f32, f32, f32) -> bool,
    /// `Player::onNewDataBlock`'s test of the object box `min`..`max` against the datablock's size: whether they differ by more
    /// than 0.1.
    pub box_differs: fn(Point, Point, Point) -> bool,
}

/// This crate's own routines: plain `f32` arithmetic in the order Torque's source writes it, and TGE's rules.
pub static PLAIN: GameLibrary = GameLibrary {
    camera_pose: plain::camera_pose,
    camera_rotation: plain::camera_rotation,
    set_velocity: plain::set_velocity,
    plane_offset: plain::plane_offset,
    ground_frame: plain::ground_frame,
    ground_lerp: plain::ground_lerp,
    ground_motion: plain::ground_motion,
    moving: plain::moving,
    action_dot: plain::action_dot,
    action_rate: plain::action_rate,
    difference: plain::difference,
    fade_over: plain::fade_over,
    box_differs: plain::box_differs,
};

static SEAM: Seam<GameLibrary> = Seam::new(&PLAIN);

/// The table in use: the installed one, else [`PLAIN`].
#[inline]
pub fn get() -> &'static GameLibrary {
    SEAM.get()
}

/// Installs `library` for the rest of the process. Only the first install takes; installing the same table again is a
/// no-op. Returns false when another table was installed first.
pub fn install(library: &'static GameLibrary) -> bool {
    SEAM.install(library)
}

/// The rotation about x of `MatrixF::set(EulerF(angle, 0, 0))`, or the identity for an angle of 0.
pub fn euler_x(angle: f32) -> Matrix {
    let mut m = IDENTITY;
    if angle != 0.0 {
        let a = f64::from(angle);
        let (s, c) = (a.sin() as f32, a.cos() as f32);
        m[5] = c;
        m[6] = s;
        m[9] = -s;
        m[10] = c;
    }
    m
}

/// The rotation about z of `MatrixF::set(EulerF(0, 0, angle))`, or the identity for an angle of 0.
pub fn euler_z(angle: f32) -> Matrix {
    let mut m = IDENTITY;
    if angle != 0.0 {
        let a = f64::from(angle);
        let (s, c) = (a.sin() as f32, a.cos() as f32);
        m[0] = c;
        m[1] = s;
        m[4] = -s;
        m[5] = c;
    }
    m
}

/// The identity matrix.
pub const IDENTITY: Matrix = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// [`PLAIN`]'s routines.
pub mod plain {
    use super::{Matrix, Point, euler_x, euler_z};

    fn dot(a: Point, b: Point) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn len(v: Point) -> f32 {
        dot(v, v).sqrt()
    }

    /// `MatrixF::mul(a, b)`.
    pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
        std::array::from_fn(|i| {
            let (row, col) = (i / 4, i % 4);
            (0..4).fold(0.0, |sum, k| sum + a[row * 4 + k] * b[k * 4 + col])
        })
    }

    pub fn camera_pose(position: Point, pitch: f32, yaw: f32) -> Matrix {
        let mut m = mul(&euler_z(yaw), &euler_x(pitch));
        (m[3], m[7], m[11]) = (position[0], position[1], position[2]);
        m
    }

    pub fn camera_rotation(m: &Matrix) -> (f32, f32) {
        let (fx, fy, fz) = (m[1], m[5], m[9]);
        let length = (fx * fx + fy * fy).sqrt();
        let pitch = -(f64::from(fz).atan2(f64::from(length)) as f32);
        let yaw = -(f64::from(-fx).atan2(f64::from(fy)) as f32);
        (pitch, yaw)
    }

    pub fn set_velocity(v: Point) -> Point {
        v
    }

    pub fn plane_offset(point: Point, normal: Point) -> f32 {
        -dot(point, normal)
    }

    pub fn ground_frame(t: f32, frames: i32) -> (i32, f32) {
        let kf = 0.999_999_f32 * t * frames as f32;
        let frame = kf as i32;
        (frame, kf - frame as f32)
    }

    pub fn ground_lerp(a: f32, b: f32, k: f32) -> f32 {
        (b - a) * k + a
    }

    pub fn ground_motion(dir: Point) -> Option<(Point, f32)> {
        let speed = len(dir);
        if speed < 0.01 {
            return None;
        }
        let inv = 1.0 / speed;
        Some((dir.map(|c| c * inv), speed))
    }

    pub fn moving(_velocity: Point) -> bool {
        true
    }

    pub fn action_dot(velocity: Point, dir: Point) -> f32 {
        dot(velocity, dir)
    }

    pub fn action_rate(velocity: Point, dir: Point, speed: f32) -> f32 {
        (dot(velocity, dir) / speed).abs()
    }

    pub fn difference(new: f32, old: f32) -> f64 {
        f64::from(new - old)
    }

    pub fn fade_over(time: f32, delay: f32, elapsed: f32) -> bool {
        time + delay < elapsed
    }

    pub fn box_differs(min: Point, max: Point, size: Point) -> bool {
        let d: Point = std::array::from_fn(|i| max[i] - min[i] - size[i]);
        len(d) > 0.1
    }
}

#[cfg(test)]
#[path = "library_tests.rs"]
mod tests;
