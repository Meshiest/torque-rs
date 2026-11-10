//! The shapes as line segments: each function pushes the segments of one shape, in world space.

use std::f32::consts::TAU;

use glam::{Quat, Vec3};

/// A segment's two ends.
pub type Segment = (Vec3, Vec3);

/// Segments of a circle of a sphere's outline.
pub const CIRCLE_SEGMENTS: usize = 32;

/// The segments of a box's twelve edges: the corners `min` and `max`.
pub fn aabb(min: Vec3, max: Vec3, out: &mut Vec<Segment>) {
    obb(
        (min + max) * 0.5,
        (max - min).abs() * 0.5,
        Quat::IDENTITY,
        out,
    );
}

/// The twelve edges of a box of half extents `half` turned by `rotation` about its center.
pub fn obb(center: Vec3, half: Vec3, rotation: Quat, out: &mut Vec<Segment>) {
    let corner = |i: usize| {
        let sign = |bit: usize| if i & bit == 0 { -1.0 } else { 1.0 };
        center + rotation * (half * Vec3::new(sign(1), sign(2), sign(4)))
    };
    for i in 0..8 {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                out.push((corner(i), corner(i | bit)));
            }
        }
    }
}

/// A circle of `radius` about `center` in the plane spanned by the unit vectors `u` and `v`.
pub fn circle(center: Vec3, radius: f32, u: Vec3, v: Vec3, out: &mut Vec<Segment>) {
    let point = |i: usize| {
        let angle = TAU * i as f32 / CIRCLE_SEGMENTS as f32;
        center + (u * angle.cos() + v * angle.sin()) * radius
    };
    for i in 0..CIRCLE_SEGMENTS {
        out.push((point(i), point(i + 1)));
    }
}

/// A sphere's outline: three circles, one in each plane of the axes.
pub fn sphere(center: Vec3, radius: f32, out: &mut Vec<Segment>) {
    circle(center, radius, Vec3::X, Vec3::Y, out);
    circle(center, radius, Vec3::X, Vec3::Z, out);
    circle(center, radius, Vec3::Y, Vec3::Z, out);
}

/// Two unit vectors at right angles to the unit vector `n` and each other.
fn perpendicular(n: Vec3) -> (Vec3, Vec3) {
    let helper = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = n.cross(helper).normalize();
    (u, n.cross(u))
}

/// An arrow from `from` to `to`: the shaft and a head of four fins at `to`, a fifth of the length long (none for a point).
pub fn arrow(from: Vec3, to: Vec3, out: &mut Vec<Segment>) {
    out.push((from, to));
    let shaft = to - from;
    let length = shaft.length();
    if length < 1.0e-6 {
        return;
    }
    let direction = shaft / length;
    let (u, v) = perpendicular(direction);
    let head = length * 0.2;
    let base = to - direction * head;
    for fin in [u, -u, v, -v] {
        out.push((to, base + fin * head * 0.4));
    }
}

/// A cross: a line through `center` along each axis, `size` long on each side.
pub fn cross(center: Vec3, size: f32, out: &mut Vec<Segment>) {
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        out.push((center - axis * size, center + axis * size));
    }
}
