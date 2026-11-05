//! The kit's interiors: brushes made in code, built into a `.dif` by `torque-interior-builder` ([`builder`]'s sibling).
//!
//! **The arena** ([`arena`]): a floor of 40 units a side, a wall round it 3 high, a block in the middle, eight pieces of cover in
//! a ring 9 out (2.4 high, a figure's head tops out near 2), two platforms with two steps each on opposite corners and stacks of
//! crates in the other two. Every brush is a box; the floor is `arena_floor`, the rest `arena_wall` ([`crate::textures`]),
//! each face one texture repeat every 4 units along the world axes across its normal (one 16-texel tile a unit).
//!
//! [`builder`]: crate::builder

use std::io;

use torque_interior_builder::{Brush, Options, TexGen, build};

/// The floor's material, and every other brush's.
pub const FLOOR: &str = "arena_floor";
pub const WALL: &str = "arena_wall";

fn cube(center: [f64; 3], size: [f64; 3], material: &str) -> Brush {
    let min = [0, 1, 2].map(|k| center[k] - size[k] * 0.5);
    let max = [0, 1, 2].map(|k| center[k] + size[k] * 0.5);
    Brush::cuboid(min, max, material, |n| TexGen::world(n, 4.0))
}

/// The arena's brushes.
pub fn arena() -> Vec<Brush> {
    let mut out = vec![
        cube([0.0, 0.0, -0.5], [40.0, 40.0, 1.0], FLOOR),
        cube([0.0, 20.5, 1.5], [42.0, 1.0, 3.0], WALL),
        cube([0.0, -20.5, 1.5], [42.0, 1.0, 3.0], WALL),
        cube([20.5, 0.0, 1.5], [1.0, 40.0, 3.0], WALL),
        cube([-20.5, 0.0, 1.5], [1.0, 40.0, 3.0], WALL),
        cube([0.0, 0.0, 1.3], [3.0, 3.0, 2.6], WALL),
    ];
    for i in 0..8 {
        let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
        let size = if i % 2 == 0 {
            [1.0, 4.0, 2.4]
        } else {
            [2.5, 2.5, 2.4]
        };
        out.push(cube([a.cos() * 9.0, a.sin() * 9.0, 1.2], size, WALL));
    }
    for side in [-1.0, 1.0] {
        let c = 13.0 * side;
        out.push(cube([c, c, 0.75], [6.0, 6.0, 1.5], WALL));
        out.push(cube([c - 4.0 * side, c, 0.25], [2.0, 4.0, 0.5], WALL));
        out.push(cube([c - 3.5 * side, c, 0.75], [1.0, 4.0, 0.5], WALL));
    }
    for side in [-1.0, 1.0] {
        out.push(cube([14.0 * side, -14.0 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([15.2 * side, -13.4 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([14.6 * side, -13.7 * side, 1.8], [1.2; 3], WALL));
    }
    out
}

/// The arena as a `.dif`.
pub fn arena_dif() -> io::Result<Vec<u8>> {
    build(&arena(), &Options::default())
        .and_then(|b| b.bytes())
        .map_err(io::Error::other)
}
