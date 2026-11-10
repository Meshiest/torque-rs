//! The map's pieces as fixed colliders, built by region on first use.
//!
//! The world's xy plane is cut into [`CELL`]-unit cells. The first time a body comes near a cell ([`crate::World::step`] asks
//! for the cells round every awake body, [`crate::World::ensure_region`] for any box), the cell is built: for each map piece
//! in id order whose box meets the cell's column,
//! - a terrain gives one triangle mesh of the squares meeting the cell, each square cut into the two triangles the engine's
//!   terrain collision uses ([`TerrainShape::triangle`]: the file's diagonal, counter-clockwise seen from above), holes left
//!   out. A height field would cut every square along one diagonal, so it would not match the engine's own collision.
//! - an interior gives one convex polyhedron for each of its hulls meeting the cell (made from the hull's corners).
//!
//! A square or hull that reaches into several cells is made once, by the first of its cells to be built. Cells are built row by
//! row (y, then x) within a query and never dropped; adding or removing a map piece clears them all, to be built again.
//!
//! Every point is checked (finite, within [`Limits::max_coordinate`]); a piece that fails is left out and counted. Each cell
//! is charged for its squares, triangles and hull points before they are made.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rapier3d::prelude::{
    ColliderBuilder, ColliderHandle, PhysicsWorld, SharedShape, TriMeshFlags, Vector,
};
use torque_collide::ObjectId;
use torque_collide::map::interior::MAP_QUERY_HULLS;
use torque_collide::map::terrain::MAP_QUERY_SQUARES;
use torque_collide::map::{InteriorShape, MapShape, TerrainShape};
use torque_collide::math::Box3;

use crate::limits::Limits;
use crate::shape::{self, Shape};
use crate::work;
use crate::world::{Cap, Error};

/// A region cell's side (units).
pub const CELL: f32 = 64.0;

/// The terrain's sample grid side.
const SIDE: i64 = torque_mapcore::budget::HEIGHTFIELD_SIDE as i64;

/// What the map pieces cost and left out so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MapStats {
    /// Cells built.
    pub cells: usize,
    /// Map colliders made.
    pub colliders: usize,
    /// Terrain triangles made.
    pub triangles: usize,
    /// Interior hulls made, and those that became their box.
    pub hulls: usize,
    pub hulls_replaced: usize,
    /// Pieces left out: a point not finite or out of range, or past a per-cell cap.
    pub dropped: usize,
}

/// The map pieces and the cells built from them.
#[derive(Debug, Default)]
pub(crate) struct Maps {
    pieces: BTreeMap<ObjectId, Rc<MapShape>>,
    built: BTreeSet<(i32, i32)>,
    /// Terrain squares (piece, x, y) that reach past their cell, made already; and interior hulls (piece, hull).
    squares: BTreeSet<(ObjectId, i64, i64)>,
    hulls: BTreeSet<(ObjectId, u32)>,
    colliders: Vec<ColliderHandle>,
    pub stats: MapStats,
    pub friction: f32,
}

/// A cell's column: its square in xy, every height.
fn column(cx: i32, cy: i32, reach: f32) -> Box3 {
    let x = cx as f32 * CELL;
    let y = cy as f32 * CELL;
    Box3::new([x, y, -reach], [x + CELL, y + CELL, reach])
}

impl Maps {
    /// Adds or replaces a map piece; every cell is built again on next use.
    pub fn insert(&mut self, id: ObjectId, shape: Rc<MapShape>, rw: &mut PhysicsWorld) {
        self.pieces.insert(id, shape);
        self.clear(rw);
    }

    /// Removes a map piece (true when it was there); every cell is built again on next use.
    pub fn remove(&mut self, id: ObjectId, rw: &mut PhysicsWorld) -> bool {
        let had = self.pieces.remove(&id).is_some();
        if had {
            self.clear(rw);
        }
        had
    }

    /// Takes every map collider out.
    fn clear(&mut self, rw: &mut PhysicsWorld) {
        for h in self.colliders.drain(..) {
            rw.remove_collider(h);
        }
        self.built.clear();
        self.squares.clear();
        self.hulls.clear();
    }

    pub fn is_built(&self, cell: (i32, i32)) -> bool {
        self.built.contains(&cell)
    }

    /// The cells of `b`'s xy extent, row by row; at most `limits.max_cells_per_query`, the ones nearest the box's middle when
    /// it covers more. None for a box that is not finite.
    pub fn cells_of(b: &Box3, limits: &Limits) -> Option<Vec<(i32, i32)>> {
        if !(crate::guard::finite(&b.min) && crate::guard::finite(&b.max)) {
            return None;
        }
        let reach = limits.max_coordinate;
        let cell = |v: f32| (v.clamp(-reach, reach) / CELL).floor() as i32;
        let (x0, x1) = (cell(b.min[0]), cell(b.max[0]));
        let (y0, y1) = (cell(b.min[1]), cell(b.max[1]));
        if x0 > x1 || y0 > y1 {
            return Some(Vec::new());
        }
        // The window kept: a side of at most the square root of the cap, about the middle.
        let side = (limits.max_cells_per_query as f64).sqrt().floor().max(1.0) as i64;
        let window = |a: i32, b: i32| {
            let (a, b) = (i64::from(a), i64::from(b));
            if b - a < side {
                return (a, b);
            }
            let mid = a + (b - a) / 2;
            let lo = mid - (side - 1) / 2;
            (lo, lo + side - 1)
        };
        let (x0, x1) = window(x0, x1);
        let (y0, y1) = window(y0, y1);
        let mut out = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.push((x as i32, y as i32));
            }
        }
        out.truncate(limits.max_cells_per_query);
        Some(out)
    }

    /// Builds `cell` if it is not built (module docs). On [`Error::Exhausted`] or a cap the pieces made so far stay (and are
    /// not made again), and the cell is built again on the next call.
    pub fn build_cell(
        &mut self,
        cell: (i32, i32),
        rw: &mut PhysicsWorld,
        limits: &Limits,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<bool, Error> {
        if self.built.contains(&cell) {
            return Ok(false);
        }
        let col = column(cell.0, cell.1, limits.max_coordinate);
        let pieces: Vec<(ObjectId, Rc<MapShape>)> = self
            .pieces
            .iter()
            .filter(|(_, s)| s.world_box().overlaps(&col))
            .map(|(&id, s)| (id, Rc::clone(s)))
            .collect();
        for (id, piece) in pieces {
            match &*piece {
                MapShape::Terrain(t) => self.terrain(id, t, cell, rw, limits, charge)?,
                MapShape::Interior(i) => self.interior(id, i, &col, rw, limits, charge)?,
            }
        }
        self.built.insert(cell);
        self.stats.cells += 1;
        Ok(true)
    }

    fn insert_collider(
        &mut self,
        shape: SharedShape,
        at: [f32; 3],
        rw: &mut PhysicsWorld,
        limits: &Limits,
    ) -> Result<(), Error> {
        if rw.colliders.len() >= limits.max_colliders {
            return Err(Error::Cap(Cap::Colliders));
        }
        let c = ColliderBuilder::new(shape)
            .translation(Vector::new(at[0], at[1], at[2]))
            .friction(self.friction)
            .restitution(0.0)
            .build();
        let h = shape::caught(|| rw.insert_collider(c, None))?;
        self.colliders.push(h);
        self.stats.colliders += 1;
        Ok(())
    }

    /// The squares of terrain `t` meeting `cell`, as one triangle mesh.
    fn terrain(
        &mut self,
        id: ObjectId,
        t: &TerrainShape,
        cell: (i32, i32),
        rw: &mut PhysicsWorld,
        limits: &Limits,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Error> {
        let sq = f64::from(t.square());
        let o = t.corner(0, 0);
        if !(sq >= 1e-3) || !sq.is_finite() || !crate::guard::finite(&o) {
            self.stats.dropped += 1;
            return Ok(());
        }
        let lo = [
            f64::from(cell.0) * f64::from(CELL),
            f64::from(cell.1) * f64::from(CELL),
        ];
        let hi = [lo[0] + f64::from(CELL), lo[1] + f64::from(CELL)];
        // Squares meeting the cell along one axis: `o + i sq` below `hi` and `o + (i + 1) sq` above `lo`.
        let range = |k: usize| {
            let a = ((lo[k] - f64::from(o[k])) / sq).floor();
            let b = ((hi[k] - f64::from(o[k])) / sq).ceil() - 1.0;
            let (mut a, mut b) = (a.clamp(-1e15, 1e15) as i64, b.clamp(-1e15, 1e15) as i64);
            if !t.repeats() {
                a = a.max(0);
                b = b.min(SIDE - 1);
            }
            (a, b)
        };
        let ((x0, x1), (y0, y1)) = (range(0), range(1));
        if x0 > x1 || y0 > y1 {
            return Ok(());
        }
        let count = (x1 - x0 + 1).saturating_mul(y1 - y0 + 1);
        if !charge(work::CANDIDATE.saturating_mul(count.min(MAP_QUERY_SQUARES as i64 + 1) as u64)) {
            return Err(Error::Exhausted);
        }
        let reaches_out = |x: i64, y: i64| {
            let s = [
                f64::from(o[0]) + x as f64 * sq,
                f64::from(o[1]) + y as f64 * sq,
            ];
            (0..2).any(|k| s[k] < lo[k] || s[k] + sq > hi[k])
        };
        let origin = [lo[0] as f32, lo[1] as f32, 0.0];
        let mut verts: Vec<Vector> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut looked = 0usize;
        let mut shared = Vec::new();
        'squares: for y in y0..=y1 {
            for x in x0..=x1 {
                looked += 1;
                if looked > MAP_QUERY_SQUARES {
                    self.stats.dropped += 1;
                    break 'squares;
                }
                if !t.has_square(x, y) {
                    continue;
                }
                let wide = reaches_out(x, y);
                if wide && self.squares.contains(&(id, x, y)) {
                    continue;
                }
                for half in 0..2u8 {
                    let tri = t.triangle(x, y, half);
                    let ok = tri.iter().all(|p| {
                        crate::guard::finite(p)
                            && p.iter().all(|v| v.abs() <= limits.max_coordinate)
                    });
                    if !ok {
                        self.stats.dropped += 1;
                        continue;
                    }
                    let base = verts.len() as u32;
                    for p in tri {
                        verts.push(Vector::new(p[0] - origin[0], p[1] - origin[1], p[2]));
                    }
                    tris.push([base, base + 1, base + 2]);
                }
                if wide {
                    shared.push((id, x, y));
                }
            }
        }
        if tris.is_empty() {
            return Ok(());
        }
        let cost = work::POLY * tris.len() as u64 + work::VERTEX * verts.len() as u64;
        if !charge(cost) {
            return Err(Error::Exhausted);
        }
        let n = tris.len();
        let mesh = shape::caught(|| {
            SharedShape::trimesh_with_flags(verts, tris, TriMeshFlags::FIX_INTERNAL_EDGES)
        })?;
        let Ok(mesh) = mesh else {
            self.stats.dropped += n;
            return Ok(());
        };
        self.insert_collider(mesh, origin, rw, limits)?;
        self.stats.triangles += n;
        self.squares.extend(shared);
        Ok(())
    }

    /// The hulls of interior `i` meeting the cell's column `col`, each a convex polyhedron.
    fn interior(
        &mut self,
        id: ObjectId,
        i: &InteriorShape,
        col: &Box3,
        rw: &mut PhysicsWorld,
        limits: &Limits,
        charge: &mut dyn FnMut(u64) -> bool,
    ) -> Result<(), Error> {
        let mut list = Vec::new();
        i.pieces(col, charge, &mut list)
            .map_err(|_| Error::Exhausted)?;
        if list.len() > MAP_QUERY_HULLS {
            self.stats.dropped += list.len() - MAP_QUERY_HULLS;
            list.truncate(MAP_QUERY_HULLS);
        }
        // Map hulls may be as large as the map: their points are held to the world's reach, not a body's.
        let held = Limits {
            max_half_extent: limits.max_coordinate,
            ..*limits
        };
        for k in list {
            if self.hulls.contains(&(id, k)) {
                continue;
            }
            let corners = i.hull_points_wide(k);
            let ok = corners.iter().all(|p| {
                p.iter()
                    .all(|v| v.is_finite() && v.abs() <= f64::from(limits.max_coordinate))
            });
            if corners.is_empty() || !ok {
                self.stats.dropped += 1;
                self.hulls.insert((id, k));
                continue;
            }
            // About the hull's middle, in f64, so the points are small numbers.
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for p in &corners {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
            let mid = [0, 1, 2].map(|a| lo[a] * 0.5 + hi[a] * 0.5);
            let points = corners
                .iter()
                .map(|p| [0, 1, 2].map(|a| (p[a] - mid[a]) as f32))
                .collect();
            let built = shape::build(&Shape::Hull { points }, &held, charge)?;
            let at = [0, 1, 2].map(|a| (mid[a] + f64::from(built.offset[a])) as f32);
            self.insert_collider(built.shape, at, rw, limits)?;
            self.hulls.insert((id, k));
            self.stats.hulls += 1;
            self.stats.hulls_replaced += usize::from(built.replaced);
        }
        Ok(())
    }
}
