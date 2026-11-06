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
    Brush::cuboid(min.into(), max.into(), material, |n| TexGen::world(n, 4.0))
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

/// A flight of `steps` box steps, each `rise` higher and `run` deeper, climbing from `foot` (the floor at the first step's
/// front edge) along `dir` (one of the four axis directions), `width` wide. Each step is a slab under the ones above it, so
/// no two brushes overlap.
fn stairs(
    foot: [f64; 3],
    dir: [f64; 2],
    width: f64,
    steps: u32,
    rise: f64,
    run: f64,
) -> Vec<Brush> {
    let total = run * f64::from(steps);
    (0..steps)
        .map(|k| {
            let k = f64::from(k);
            // The slab from this step's front edge to the top of the flight.
            let from = run * k;
            let len = total - from;
            let mid = from + len * 0.5;
            let along = [foot[0] + dir[0] * mid, foot[1] + dir[1] * mid];
            let size = if dir[0] != 0.0 {
                [len, width, rise]
            } else {
                [width, len, rise]
            };
            cube([along[0], along[1], foot[2] + rise * (k + 0.5)], size, WALL)
        })
        .collect()
}

/// The keep: a tight court of 32 units a side inside walls 6.5 high, with a deck 3 up along the north and the south walls
/// (each over a low hall held up by pillars), a bridge across the middle joining them, stairs up at the four corners, low
/// parapets along the decks' edges and the bridge, and cover in the court and the halls.
pub fn keep() -> Vec<Brush> {
    let (half, wall_h) = (16.0, 6.5);
    let deck = 3.0;
    let mut out = vec![
        cube([0.0, 0.0, -0.5], [2.0 * half, 2.0 * half, 1.0], FLOOR),
        cube(
            [0.0, half + 0.5, wall_h * 0.5],
            [2.0 * half + 2.0, 1.0, wall_h],
            WALL,
        ),
        cube(
            [0.0, -half - 0.5, wall_h * 0.5],
            [2.0 * half + 2.0, 1.0, wall_h],
            WALL,
        ),
        cube(
            [half + 0.5, 0.0, wall_h * 0.5],
            [1.0, 2.0 * half, wall_h],
            WALL,
        ),
        cube(
            [-half - 0.5, 0.0, wall_h * 0.5],
            [1.0, 2.0 * half, wall_h],
            WALL,
        ),
    ];
    for side in [-1.0, 1.0] {
        // The deck: y from 10 to the wall, its slab 0.5 thick (a hall 2.5 high under it).
        out.push(cube(
            [0.0, 13.0 * side, deck - 0.25],
            [2.0 * half, 6.0, 0.5],
            WALL,
        ));
        for x in [-12.0, -4.0, 4.0, 12.0] {
            out.push(cube([x, 10.6 * side, 1.25], [0.8, 0.8, 2.5], WALL));
        }
        // Parapets along the deck's edge, open where the stairs and the bridge come up.
        for (x0, x1) in [(-13.5, -1.5), (1.5, 13.5)] {
            out.push(cube(
                [(x0 + x1) * 0.5, 10.2 * side, deck + 0.55],
                [x1 - x0, 0.4, 1.1],
                WALL,
            ));
        }
        // Stairs up to the deck by the east and west walls: six steps of 0.5.
        for x in [-15.0, 15.0] {
            out.extend(stairs([x, 5.2 * side, 0.0], [0.0, side], 2.0, 6, 0.5, 0.8));
        }
        // Cover in the hall under the deck: crates against the wall and a low block between the pillars.
        out.push(cube([-8.0 * side, 14.6 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([-6.8 * side, 14.6 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([8.0 * side, 13.5 * side, 0.7], [3.0, 1.0, 1.4], WALL));
        // Cover in the court: tall blocks either side of the bridge and crates by the stairs.
        out.push(cube([6.5 * side, 4.5 * side, 1.2], [1.0, 3.5, 2.4], WALL));
        out.push(cube([-6.5 * side, 4.5 * side, 1.2], [3.0, 1.0, 2.4], WALL));
        out.push(cube([10.5 * side, -side, 0.6], [1.2; 3], WALL));
        out.push(cube([10.5 * side, 0.2 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([10.5 * side, -0.4 * side, 1.8], [1.2; 3], WALL));
    }
    // The bridge: 2.5 wide over the court from deck to deck, low parapets along both sides, a pier under its middle.
    out.push(cube([0.0, 0.0, deck - 0.25], [2.5, 20.0, 0.5], WALL));
    for x in [-1.05, 1.05] {
        out.push(cube([x, 0.0, deck + 0.45], [0.4, 20.0, 0.9], WALL));
    }
    out.push(cube([0.0, 0.0, 1.25], [1.6, 1.6, 2.5], WALL));
    out
}

/// The keep as a `.dif`.
pub fn keep_dif() -> io::Result<Vec<u8>> {
    build(&keep(), &Options::default())
        .and_then(|b| b.bytes())
        .map_err(io::Error::other)
}

/// The yard: an open field of 72 by 48 units inside walls 3 high, with long sight lines down its length: a raised stand at
/// each end (1.5 up, steps at the front and low parapets), two long low walls broken by gaps along the lanes, tall pillars and
/// a wall in the middle for cover, and a few crates.
pub fn yard() -> Vec<Brush> {
    let (hx, hy, wall_h) = (36.0, 24.0, 3.0);
    let stand = 1.5;
    let mut out = vec![
        cube([0.0, 0.0, -0.5], [2.0 * hx, 2.0 * hy, 1.0], FLOOR),
        cube(
            [0.0, hy + 0.5, wall_h * 0.5],
            [2.0 * hx + 2.0, 1.0, wall_h],
            WALL,
        ),
        cube(
            [0.0, -hy - 0.5, wall_h * 0.5],
            [2.0 * hx + 2.0, 1.0, wall_h],
            WALL,
        ),
        cube([hx + 0.5, 0.0, wall_h * 0.5], [1.0, 2.0 * hy, wall_h], WALL),
        cube(
            [-hx - 0.5, 0.0, wall_h * 0.5],
            [1.0, 2.0 * hy, wall_h],
            WALL,
        ),
        // The middle: a wall across the long lane, tall enough to hide behind.
        cube([0.0, 0.0, 1.3], [1.5, 5.0, 2.6], WALL),
    ];
    for side in [-1.0, 1.0] {
        // The stand: x from 30 to the wall, 18 wide, with three steps up at the front and parapets either side of them.
        out.push(cube(
            [33.0 * side, 0.0, stand * 0.5],
            [6.0, 18.0, stand],
            WALL,
        ));
        out.extend(stairs(
            [28.5 * side, 0.0, 0.0],
            [side, 0.0],
            3.0,
            3,
            0.5,
            0.5,
        ));
        for (y0, y1) in [(-9.0, -2.0), (2.0, 9.0)] {
            out.push(cube(
                [30.2 * side, (y0 + y1) * 0.5, stand + 0.5],
                [0.4, y1 - y0, 1.0],
                WALL,
            ));
        }
        // The long low walls along the lanes, with gaps to run through.
        for (x0, x1) in [(-26.0, -15.0), (-8.0, 8.0), (15.0, 26.0)] {
            out.push(cube(
                [(x0 + x1) * 0.5, 9.0 * side, 0.55],
                [x1 - x0, 0.5, 1.1],
                WALL,
            ));
        }
        // Tall pillars: in the long lane and out on the flanks.
        out.push(cube([19.0 * side, 0.0, 1.3], [2.0, 2.0, 2.6], WALL));
        out.push(cube([0.0, 17.0 * side, 1.3], [2.0, 2.0, 2.6], WALL));
        out.push(cube(
            [-12.0 * side, 17.0 * side, 1.3],
            [2.0, 2.0, 2.6],
            WALL,
        ));
        out.push(cube([12.0 * side, 4.5 * side, 1.3], [1.0, 3.0, 2.6], WALL));
        // Crates on the flanks.
        out.push(cube([24.0 * side, -18.0 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([25.2 * side, -18.0 * side, 0.6], [1.2; 3], WALL));
        out.push(cube([24.6 * side, -18.0 * side, 1.8], [1.2; 3], WALL));
    }
    out
}

/// The yard as a `.dif`.
pub fn yard_dif() -> io::Result<Vec<u8>> {
    build(&yard(), &Options::default())
        .and_then(|b| b.bytes())
        .map_err(io::Error::other)
}
