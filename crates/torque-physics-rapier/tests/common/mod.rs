//! Map pieces made in code for the tests: a terrain of a height function and an interior of boxes.

#![allow(dead_code)]

use std::rc::Rc;
use std::sync::Arc;

use torque_collide::map::interior::{Face, Hull};
use torque_collide::map::{HeightField, InteriorGeom, InteriorShape, MapShape, TerrainShape};
use torque_collide::math::{Box3, IDENTITY};
use torque_mapcore::budget::HEIGHTFIELD_SIDE as SIDE;
use torque_mapcore::collision::{Heightfield, HeightfieldPlacement, Split};

/// Pays for everything.
pub fn free(_: u64) -> bool {
    true
}

/// A terrain block whose sample (i, j) is `f(i, j)` high, `square` units a square, sample (0, 0) at `origin`, raised by `z`.
pub fn terrain(
    f: impl Fn(usize, usize) -> f32,
    square: i32,
    origin: [f32; 2],
    z: f32,
    repeat: bool,
    holes: Vec<bool>,
) -> Rc<MapShape> {
    let heights = (0..SIDE * SIDE).map(|k| f(k % SIDE, k / SIDE)).collect();
    let field = Heightfield {
        heights,
        layers: Vec::new(),
        split: Split::Alternating,
    };
    let place = HeightfieldPlacement::new(origin, z, square, repeat, holes);
    Rc::new(MapShape::Terrain(TerrainShape::new(
        Arc::new(HeightField::new(&field)),
        &place,
    )))
}

/// A box hull from `lo` to `hi` (the faces as the interior reader makes them; only the corners and the box matter here).
pub fn box_hull(lo: [f32; 3], hi: [f32; 3]) -> Hull {
    let corner = |c: usize| {
        [
            if c & 1 != 0 { hi[0] } else { lo[0] },
            if c & 2 != 0 { hi[1] } else { lo[1] },
            if c & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    let points: Vec<[f32; 3]> = (0..8).map(corner).collect();
    let corners = points.iter().map(|p| p.map(f64::from)).collect();
    let planes = vec![
        [-1.0, 0.0, 0.0, f64::from(lo[0])],
        [1.0, 0.0, 0.0, -f64::from(hi[0])],
        [0.0, -1.0, 0.0, f64::from(lo[1])],
        [0.0, 1.0, 0.0, -f64::from(hi[1])],
        [0.0, 0.0, -1.0, f64::from(lo[2])],
        [0.0, 0.0, 1.0, -f64::from(hi[2])],
    ];
    let faces = [
        [0u32, 4, 6, 2],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 2, 3, 1],
        [4, 5, 7, 6],
    ]
    .iter()
    .zip(&planes)
    .map(|(v, &plane)| Face {
        plane,
        verts: v.to_vec(),
    })
    .collect();
    Hull {
        points,
        corners,
        planes,
        faces,
        bounds: Box3::new(lo, hi),
    }
}

/// An interior of `hulls` at `offset`.
pub fn interior(hulls: Vec<Hull>, offset: [f32; 3]) -> Rc<MapShape> {
    let mut m = IDENTITY;
    m[3] = offset[0];
    m[7] = offset[1];
    m[11] = offset[2];
    let geom = Arc::new(InteriorGeom::from_hulls(hulls));
    Rc::new(MapShape::Interior(
        InteriorShape::new(geom, m, [1.0; 3]).expect("a placeable interior"),
    ))
}

/// A small xorshift generator.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        (self.next() >> 11) % n.max(1)
    }

    /// A float in `lo..hi`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        let t = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        lo + (hi - lo) * t
    }
}
