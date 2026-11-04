//! A map's heightfield collision: a [`torque_mapcore::collision::Heightfield`] at its
//! [`torque_mapcore::collision::HeightfieldPlacement`] (a `.ter` terrain centred on the origin, repeated as the placement
//! says),
//! each square cut into the two triangles drawing uses ([`torque_mapcore::collision::Split`]), the squares its holes name left out.
//!
//! - **Convexes** ([`TerrainShape::pieces`]): one a triangle a square, the triangle extruded down by [`TERRAIN_DEPTH`] (a
//!   prism): its six corners answer `support`, its top triangle is its polygon list and its feature. This is the crate's own
//!   convex, not Torque's `TerrainConvex` (a square's four surface points, split in two only where the square is concave, with
//!   no depth below them; `TerrainThickness` 0.5 bounds its polygon lists): a prism gives the distance and penetration
//!   queries a volume under the surface, so a mover that ends a tick below a triangle is pushed back up through its top.
//! - **Rays** ([`TerrainShape::cast_ray`]): the segment clipped to the block's height range (and to the block when it does not
//!   repeat), then walked square by square along its path over the grid (each square's two triangles, front faces only), stopping
//!   at the first square that starts beyond the nearest hit.
//!
//! The geometry is the triangles' corners as floats; the rays test them in `f64`.

use std::sync::Arc;

use torque_mapcore::budget::HEIGHTFIELD_SIDE as SIZE;
use torque_mapcore::collision::{Heightfield, HeightfieldPlacement, Split};

use crate::math::{Box3, F3};
use crate::{Exhausted, work};

/// How far a terrain triangle's convex reaches below its lowest corner: a mover that crosses the surface within one 32 ms tick
/// at up to 125 m/s still ends inside the prism.
pub const TERRAIN_DEPTH: f32 = 4.0;
/// The most squares one box query looks at (cap; a mover's query covers a few).
pub const MAP_QUERY_SQUARES: usize = 4_096;
/// The most squares one ray walks (cap: a ray across a repeated block's whole tile is 512 squares).
pub const MAP_RAY_SQUARES: usize = 65_536;
/// The square size a block is placed with, at least 1 and at most this (the layer's cap).
pub use torque_mapcore::budget::MAX_SQUARE_SIZE;
/// The half-extent in x and y of a repeated block's world box.
pub const REPEAT_REACH: f32 = 1.0e7;

/// A heightfield's heights, with their least and greatest, and its cut (shared by every block of the file).
#[derive(Debug, Clone, PartialEq)]
pub struct HeightField {
    heights: Vec<f32>,
    min: f32,
    max: f32,
    split: Split,
}

impl HeightField {
    pub fn new(field: &Heightfield) -> HeightField {
        let heights: Vec<f32> = (0..SIZE * SIZE)
            .map(|i| field.height(i % SIZE, i / SIZE))
            .collect();
        let min = heights.iter().copied().fold(f32::INFINITY, f32::min);
        let max = heights.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        HeightField {
            heights,
            min,
            max,
            split: field.split,
        }
    }

    /// The height at a sample, the grid wrapping.
    pub fn at(&self, x: i64, y: i64) -> f32 {
        let w = |v: i64| v.rem_euclid(SIZE as i64) as usize;
        self.heights[w(x) + w(y) * SIZE]
    }

    pub fn min(&self) -> f32 {
        self.min
    }

    pub fn max(&self) -> f32 {
        self.max
    }
}

/// One placed terrain block.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainShape {
    field: Arc<HeightField>,
    /// Sample (0, 0) and the object's z.
    origin: [f64; 3],
    square: f64,
    repeat: bool,
    empty: Arc<Vec<bool>>,
}

/// A segment's first hit: the fraction along it and the unit normal in the world.
pub type MapHit = (f32, F3);

impl TerrainShape {
    /// A block of `field` at `place` (its square already held to 1..[`MAX_SQUARE_SIZE`], its height finite).
    pub fn new(field: Arc<HeightField>, place: &HeightfieldPlacement) -> TerrainShape {
        let [ox, oy, z] = place.origin;
        TerrainShape {
            field,
            origin: [f64::from(ox), f64::from(oy), f64::from(z)],
            square: f64::from(place.square),
            repeat: place.repeat,
            empty: Arc::clone(&place.holes),
        }
    }

    pub fn field(&self) -> &HeightField {
        &self.field
    }

    pub fn repeats(&self) -> bool {
        self.repeat
    }

    /// The square size in metres.
    pub fn square(&self) -> f32 {
        self.square as f32
    }

    /// The box the block's surface lies in (its convexes' depth below it).
    pub fn world_box(&self) -> Box3 {
        let z0 = (self.origin[2] + f64::from(self.field.min)) as f32 - TERRAIN_DEPTH;
        let z1 = (self.origin[2] + f64::from(self.field.max)) as f32;
        if self.repeat {
            return Box3::new(
                [-REPEAT_REACH, -REPEAT_REACH, z0],
                [REPEAT_REACH, REPEAT_REACH, z1],
            );
        }
        let span = self.square * SIZE as f64;
        Box3::new(
            [self.origin[0] as f32, self.origin[1] as f32, z0],
            [
                (self.origin[0] + span) as f32,
                (self.origin[1] + span) as f32,
                z1,
            ],
        )
    }

    /// Sample (x, y)'s corner in the world.
    pub fn corner(&self, x: i64, y: i64) -> F3 {
        [
            (self.origin[0] + x as f64 * self.square) as f32,
            (self.origin[1] + y as f64 * self.square) as f32,
            (self.origin[2] + f64::from(self.field.at(x, y))) as f32,
        ]
    }

    /// Whether square (x, y) has ground: inside the block or the block repeats, and not an empty square.
    pub fn has_square(&self, x: i64, y: i64) -> bool {
        let inside = (0..SIZE as i64).contains(&x) && (0..SIZE as i64).contains(&y);
        if !self.repeat && !inside {
            return false;
        }
        let w = |v: i64| v.rem_euclid(SIZE as i64) as usize;
        !self.empty[w(x) + w(y) * SIZE]
    }

    /// Triangle `half` (0 or 1) of square (x, y), counter-clockwise seen from above (its normal up).
    pub fn triangle(&self, x: i64, y: i64, half: u8) -> [F3; 3] {
        let a = self.corner(x, y);
        let b = self.corner(x + 1, y);
        let c = self.corner(x + 1, y + 1);
        let d = self.corner(x, y + 1);
        let w = |v: i64| v.rem_euclid(SIZE as i64) as usize;
        match (self.field.split.main_diagonal(w(x), w(y)), half) {
            (true, 0) => [a, b, c],
            (true, _) => [a, c, d],
            (false, 0) => [a, b, d],
            (false, _) => [b, c, d],
        }
    }

    /// The bottom of a triangle's convex.
    pub fn bottom(tri: &[F3; 3]) -> f32 {
        tri[0][2].min(tri[1][2]).min(tri[2][2]) - TERRAIN_DEPTH
    }

    /// A triangle's convex's box.
    pub fn triangle_box(tri: &[F3; 3]) -> Box3 {
        let lo = |k: usize| tri[0][k].min(tri[1][k]).min(tri[2][k]);
        let hi = |k: usize| tri[0][k].max(tri[1][k]).max(tri[2][k]);
        Box3::new([lo(0), lo(1), Self::bottom(tri)], [hi(0), hi(1), hi(2)])
    }

    /// The grid cell range of `[lo, hi]` metres along axis `k` (0 or 1), held to the block when it does not repeat; None when empty.
    fn cells(&self, lo: f32, hi: f32, k: usize) -> Option<(i64, i64)> {
        if !(lo.is_finite() && hi.is_finite()) || lo > hi {
            return None;
        }
        let a = ((f64::from(lo) - self.origin[k]) / self.square).floor();
        let b = ((f64::from(hi) - self.origin[k]) / self.square).floor();
        let (mut a, mut b) = (a.max(-1e15) as i64, b.min(1e15) as i64);
        if !self.repeat {
            a = a.max(0);
            b = b.min(SIZE as i64 - 1);
        }
        (a <= b).then_some((a, b))
    }

    /// The triangles whose convexes meet `query`, square by square (x fastest), each as `(x, y, half)`; at most
    /// [`MAP_QUERY_SQUARES`] squares are looked at (`capped` is set past them). Charged a convex a square.
    pub fn pieces(
        &self,
        query: &Box3,
        charge: &mut dyn FnMut(u64) -> bool,
        out: &mut Vec<(i32, i32, u8)>,
        capped: &mut bool,
    ) -> Result<(), Exhausted> {
        let wb = self.world_box();
        if !query.overlaps(&wb) {
            return Ok(());
        }
        let (Some((x0, x1)), Some((y0, y1))) = (
            self.cells(query.min[0], query.max[0], 0),
            self.cells(query.min[1], query.max[1], 1),
        ) else {
            return Ok(());
        };
        let mut looked = 0usize;
        for y in y0..=y1 {
            for x in x0..=x1 {
                if looked >= MAP_QUERY_SQUARES {
                    *capped = true;
                    return Ok(());
                }
                looked += 1;
                if !charge(work::CONVEX) {
                    return Err(Exhausted);
                }
                let (Ok(xi), Ok(yi)) = (i32::try_from(x), i32::try_from(y)) else {
                    continue;
                };
                if !self.has_square(x, y) {
                    continue;
                }
                for half in 0..2u8 {
                    if Self::triangle_box(&self.triangle(x, y, half)).overlaps(query) {
                        out.push((xi, yi, half));
                    }
                }
            }
        }
        Ok(())
    }

    /// The segment `start` to `end` against the block: the nearest front face hit (module docs). Charged a polygon a square.
    pub fn cast_ray(
        &self,
        start: F3,
        end: F3,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<Option<MapHit>, Exhausted> {
        if !start.iter().chain(&end).all(|v| v.is_finite()) {
            return Ok(None);
        }
        let s = start.map(f64::from);
        let e = end.map(f64::from);
        let dir = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        let mut clip = |p: f64, dp: f64, lo: f64, hi: f64| -> bool {
            if dp == 0.0 {
                return p >= lo && p <= hi;
            }
            let (a, b) = ((lo - p) / dp, (hi - p) / dp);
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            t0 = t0.max(a);
            t1 = t1.min(b);
            t0 <= t1
        };
        let z0 = self.origin[2] + f64::from(self.field.min) - 1e-3;
        let z1 = self.origin[2] + f64::from(self.field.max) + 1e-3;
        if !clip(s[2], dir[2], z0, z1) {
            return Ok(None);
        }
        if !self.repeat {
            let span = self.square * SIZE as f64;
            for k in 0..2 {
                if !clip(s[k], dir[k], self.origin[k], self.origin[k] + span) {
                    return Ok(None);
                }
            }
        }
        // The grid position at `t`, in squares.
        let g = |k: usize, t: f64| (s[k] + dir[k] * t - self.origin[k]) / self.square;
        let at0 = [g(0, t0), g(1, t0)];
        let mut cell = [at0[0].floor(), at0[1].floor()];
        // Where the walk leaves the start cell in x and y, and the step between cell walls (in t).
        let mut next = [f64::INFINITY; 2];
        let mut delta = [f64::INFINITY; 2];
        let mut step = [0i64; 2];
        for k in 0..2 {
            let dg = dir[k] / self.square;
            if dg > 0.0 {
                step[k] = 1;
                delta[k] = 1.0 / dg;
                next[k] = t0 + (cell[k] + 1.0 - at0[k]) / dg;
            } else if dg < 0.0 {
                step[k] = -1;
                delta[k] = -1.0 / dg;
                next[k] = t0 + (cell[k] - at0[k]) / dg;
            }
        }
        let mut best: Option<(f64, [f64; 3])> = None;
        let mut entered = t0;
        for _ in 0..MAP_RAY_SQUARES {
            if entered > t1 || best.is_some_and(|(bt, _)| entered > bt) {
                break;
            }
            if !charge(work::POLY * 2) {
                return Err(Exhausted);
            }
            if !(cell[0].abs() < 1e15 && cell[1].abs() < 1e15) {
                break;
            }
            let (x, y) = (cell[0] as i64, cell[1] as i64);
            if self.has_square(x, y) {
                for half in 0..2u8 {
                    let tri = self.triangle(x, y, half);
                    if let Some((t, n)) = ray_triangle(s, dir, &tri)
                        && t >= t0 - 1e-9
                        && t <= t1 + 1e-9
                        && best.is_none_or(|(bt, _)| t < bt)
                    {
                        best = Some((t, n));
                    }
                }
            }
            let k = if next[0] < next[1] { 0 } else { 1 };
            if !next[k].is_finite() {
                break;
            }
            entered = next[k];
            next[k] += delta[k];
            cell[k] += step[k] as f64;
        }
        Ok(best.map(|(t, n)| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (
                (t.clamp(0.0, 1.0)) as f32,
                [
                    (n[0] / len) as f32,
                    (n[1] / len) as f32,
                    (n[2] / len) as f32,
                ],
            )
        }))
    }

    /// The surface height at (x, y) in the world, from the two triangles of its square (None off the block or over a hole).
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        let gx = (f64::from(x) - self.origin[0]) / self.square;
        let gy = (f64::from(y) - self.origin[1]) / self.square;
        if !gx.is_finite() || !gy.is_finite() {
            return None;
        }
        let (ix, iy) = (gx.floor() as i64, gy.floor() as i64);
        if !self.has_square(ix, iy) {
            return None;
        }
        let top = [f64::from(x), f64::from(y), 1e9];
        for half in 0..2u8 {
            let tri = self.triangle(ix, iy, half);
            if let Some((t, _)) = ray_triangle(top, [0.0, 0.0, -2e9], &tri) {
                return Some((1e9 - t * 2e9) as f32);
            }
        }
        None
    }
}

/// The segment `s + t * dir` (t in 0..1) against triangle `tri` (counter-clockwise seen from its front): `t` and the
/// unnormalised front normal for a front-face hit, edges included.
pub(crate) fn ray_triangle(s: [f64; 3], dir: [f64; 3], tri: &[F3; 3]) -> Option<(f64, [f64; 3])> {
    let p = tri.map(|v| v.map(f64::from));
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
    let denom = dot(n, dir);
    if !(denom < 0.0) {
        return None;
    }
    let t = dot(n, sub(p[0], s)) / denom;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let q = [s[0] + dir[0] * t, s[1] + dir[1] * t, s[2] + dir[2] * t];
    // Inside when q is on the inner side of each edge (a relative tolerance keeps shared edges watertight).
    let scale = dot(n, n).sqrt();
    let tol = -1e-9 * scale * scale.sqrt().max(1.0);
    for i in 0..3 {
        let a = p[i];
        let b = p[(i + 1) % 3];
        if dot(cross(sub(b, a), sub(q, a)), n) < tol {
            return None;
        }
    }
    Some((t, n))
}

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod tests;
