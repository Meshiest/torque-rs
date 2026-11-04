//! The server container's bins: the grid of lists and the overflow list that the `Container` keeps every scene object in that
//! no spatial index holds, with the order `Container::findObjects` walks them in. The grid's shape is a [`BinGeometry`] (TGE
//! 1.4's 16 x 16 cells of 64 units by default; a game may give another).
//!
//! The order is what the ghost layer needs: `objectInScope` takes ghost ids in the order the container's query delivers
//! objects, so the camera's neighbours (lights, zones, emitters, the cameras themselves) get their ids from this walk. As TGE's
//! `Container` has it (`sim/sceneObject.cc`; Torque3D's `scene/sceneContainer.cpp`, `Copyright (c) 2012 GarageGames, LLC`):
//!
//! - **The cell of a coordinate** ([`BinGeometry::range`], `getBinRange`): per axis the coordinate wrapped into the grid,
//!   divided by the cell size; the range is the whole ring when the span is wider than the grid.
//! - **Insertion** ([`Bins::insert`]): an object whose ranges are as wide as the grid on both axes goes to the head of the
//!   overflow list; any other goes to the head of every cell of `y in ys ..= ye`, `x in xs ..= xe` (y outer, x inner). A ring
//!   as wide as the grid on one axis only visits a cell twice: the list holds the object twice.
//! - **A move** ([`Bins::check`], `checkBins`): the four numbers are compared with the stored ones; when they differ the object
//!   leaves every list and is inserted again at the heads (a moved object jumps to the front of its bins).
//! - **The walk** ([`Bins::walk`]): only for a mask with a bit the bins answer for; the cells of the query's ranges, y outer, x
//!   inner, each list from its head, then the overflow list from its head. An object is looked at once per query (a sequence
//!   key marks it before the tests), and delivered when `type & mask != 0` and the world boxes overlap with non-strict
//!   compares.
//!
//! - **Cost**: an insertion, a removal and a move are charged ([`Bins::insert_charged`], [`Bins::remove_charged`],
//!   [`Bins::check_charged`]: [`CELL_WORK`] a cell, [`ENTRY_WORK`] an entry a list's filter looks at), a move the budget
//!   refuses before it starts leaves the object where it was, and the cells hold at most [`MAX_BIN_CELL_ENTRIES`] entries
//!   together (an object past the cap goes to the overflow list). Without the cap a box that spans the ring on one axis of a
//!   128-cell grid sits in 16,256 cells.
//!
//! A container may ask spatial indexes of its own before the bins; their order is theirs.

use std::collections::{HashMap, HashSet};

use crate::geometry::{Charge, Exhausted, Geometry, ray_box};

/// The objects a walk has stamped (TGE's container sequence key): a short list while it is short (a ray or a box meets a
/// handful), a hash set past [`Seen::LIST`] (a hostile cell holds thousands).
#[derive(Default)]
struct Seen {
    list: Vec<u32>,
    set: Option<HashSet<u32>>,
}

impl Seen {
    const LIST: usize = 32;

    /// True when `obj` was not stamped yet (it is now).
    fn insert(&mut self, obj: u32) -> bool {
        if let Some(set) = self.set.as_mut() {
            return set.insert(obj);
        }
        if self.list.contains(&obj) {
            return false;
        }
        self.list.push(obj);
        if self.list.len() > Self::LIST {
            self.set = Some(self.list.drain(..).collect());
        }
        true
    }
}

/// Work (units; 64 make a step) for each cell a walk or a ray looks at.
pub const CELL_WORK: u64 = 2;
/// Work for each list entry a walk or a ray looks at.
pub const ENTRY_WORK: u64 = 2;
/// Work for each grid-wide segment of a ray's general walk.
pub const SEGMENT_WORK: u64 = 8;

/// The most objects the bins hold.
pub const MAX_BIN_SCENE_OBJECTS: usize = 1 << 20;
/// The most entries the cells hold together: an object whose cells would pass it is placed in the overflow list, which every
/// walk and ray looks at. A box that spans the ring on one axis sits in up to 16,256 cells (`128 * 127`) in a 128-cell grid, so
/// 64 of them fill the cap; an ordinary world holds a few thousand entries. Bounds what a removal's filters look at.
pub const MAX_BIN_CELL_ENTRIES: usize = 1 << 20;

/// The shape of a container's bin grid (TGE's `Container::csmNumBins` and `csmBinSize`, `sim/sceneObject.cc`) and the type bits
/// its walks look at. The default is TGE 1.4's: 16 x 16 cells of 64 units (the grid wraps every 1,024 units) and every type
/// bit; a game whose container differs gives its own ([`BinGeometry::new`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinGeometry {
    /// Cells per axis (TGE `csmNumBins`); at least 1.
    count: u32,
    /// The width of a cell in world units (TGE `csmBinSize`).
    size: f32,
    /// The type bits a walk or a ray over the bins looks at (`mask & bins_mask`; the others are an index's).
    mask: u32,
    /// `count * size`: the width of the grid; coordinates wrap at it (TGE `csmTotalBinSize`).
    wrap: f64,
    /// The float just below the wrap that a coordinate equal to it is taken as (TGE: `csmTotalBinSize - 0.01`).
    below_wrap: f32,
}

impl BinGeometry {
    /// A grid of `count` x `count` cells of `size` units whose walks look at the type bits of `mask`. A `count` of 0 is taken
    /// as 1.
    pub const fn new(count: u32, size: f32, mask: u32) -> BinGeometry {
        let count = if count == 0 { 1 } else { count };
        let wrap = count as f64 * size as f64;
        BinGeometry {
            count,
            size,
            mask,
            wrap,
            below_wrap: (wrap - 0.01) as f32,
        }
    }

    /// Cells per axis.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The width of a cell.
    pub fn size(&self) -> f32 {
        self.size
    }

    /// The type bits the bins look at.
    pub fn mask(&self) -> u32 {
        self.mask
    }

    /// The width of the grid.
    pub fn wrap(&self) -> f64 {
        self.wrap
    }

    /// The float just below the wrap that a wrapped coordinate equal to the wrap is taken as.
    pub fn below_wrap(&self) -> f32 {
        self.below_wrap
    }

    /// The cell number of cell `(x, y)` (each taken modulo the count; y outer).
    fn cell(&self, x: u32, y: u32) -> u32 {
        (y % self.count) * self.count + (x % self.count)
    }

    /// One axis of `getBinRange` (TGE `sceneObject.cc`): the first and last cell a span `min ..= max` covers (the last may pass
    /// the count; cells are taken modulo it), through the [`crate::library`] table ([`plain_bin_range`] by default).
    pub fn range(&self, min: f32, max: f32) -> (u32, u32) {
        (crate::library::get().bin_range)(self, min, max)
    }
}

/// [`BinGeometry::range`] in plain `f32`, as TGE's `getBinRange` writes it: `v = fmod(coord, wrap)`, a negative result gets the
/// wrap added (and a result equal to the wrap becomes the float just below it), a coordinate that is not finite is taken as 0;
/// the first cell is `v_min / size`, the range is the whole ring when `max - min > wrap`, else it ends at `v_max / size`, plus
/// the count when `min != max` and the wrapped end lies below the start.
pub fn plain_bin_range(g: &BinGeometry, min: f32, max: f32) -> (u32, u32) {
    let total = g.wrap as f32;
    let wrap = |c: f32| -> f32 {
        if !c.is_finite() {
            return 0.0;
        }
        let mut v = c % total;
        if v < 0.0 {
            v += total;
            if v == total {
                v = g.below_wrap;
            }
        }
        v
    };
    let cell = |v: f32| -> u32 { (v / g.size) as u32 };
    let (lo, hi) = (wrap(min), wrap(max));
    if max - min > total {
        let start = cell(lo);
        return (start, start + (g.count - 1));
    }
    let start = cell(lo);
    let mut end = cell(hi);
    if min != max && lo > hi {
        end += g.count;
    }
    (start, end)
}

impl Default for BinGeometry {
    /// TGE 1.4's grid: 16 cells of 64 units, every type bit.
    fn default() -> BinGeometry {
        BinGeometry::new(16, 64.0, u32::MAX)
    }
}

/// What the bins read of an object when they place it or test it: its world box.
pub type WorldBox = ([f32; 3], [f32; 3]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Placement {
    /// `xs, xe, ys, ye`: the object's bin range, kept to compare a move against.
    range: [u32; 4],
    overflow: bool,
}

/// The bins of one container.
#[derive(Debug, Default)]
pub struct Bins {
    /// The grid's shape and the type bits it answers for.
    geometry: BinGeometry,
    /// Cell number -> its list; the LAST element is the head.
    cells: HashMap<u32, Vec<u32>>,
    /// The overflow list; the last element is the head.
    overflow: Vec<u32>,
    placed: HashMap<u32, Placement>,
    /// The entries of `cells` together ([`MAX_BIN_CELL_ENTRIES`]).
    entries: usize,
}

/// The cells a placement's four numbers cover (y outer, x inner; a ring visits a cell twice when it is as wide as the grid on
/// one axis).
fn cell_count(r: &[u32; 4]) -> usize {
    let [xs, xe, ys, ye] = *r;
    let span = |lo: u32, hi: u32| (hi.saturating_sub(lo) as usize).saturating_add(1);
    if xe < xs || ye < ys {
        return 0;
    }
    span(xs, xe).saturating_mul(span(ys, ye))
}

fn ranges(g: &BinGeometry, b: &WorldBox) -> [u32; 4] {
    let (xs, xe) = g.range(b.0[0], b.1[0]);
    let (ys, ye) = g.range(b.0[1], b.1[1]);
    [xs, xe, ys, ye]
}

impl Bins {
    /// Empty bins of TGE's grid ([`BinGeometry::default`]).
    pub fn new() -> Bins {
        Bins::default()
    }

    /// Empty bins of the grid `geometry`.
    pub fn with_geometry(geometry: BinGeometry) -> Bins {
        Bins {
            geometry,
            ..Bins::default()
        }
    }

    /// The grid's shape.
    pub fn geometry(&self) -> &BinGeometry {
        &self.geometry
    }

    /// How many objects are placed.
    pub fn len(&self) -> usize {
        self.placed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    pub fn contains(&self, obj: u32) -> bool {
        self.placed.contains_key(&obj)
    }

    /// Places an object that is not placed. False when it already is, or the cap is reached. Uncharged:
    /// [`Bins::insert_charged`] with a free budget.
    pub fn insert(&mut self, obj: u32, world: &WorldBox) -> bool {
        self.insert_charged(obj, world, &mut |_| true)
    }

    /// [`Bins::insert`], each cell it enters charged [`CELL_WORK`]. The insertion is always made whole (a half-placed object
    /// would be lost to [`Bins::check`]); `charge`'s refusal only ends the caller's run. An object whose cells would take the
    /// bins past [`MAX_BIN_CELL_ENTRIES`] goes to the overflow list instead.
    pub fn insert_charged(&mut self, obj: u32, world: &WorldBox, charge: Charge) -> bool {
        if self.placed.contains_key(&obj) || self.placed.len() >= MAX_BIN_SCENE_OBJECTS {
            return false;
        }
        let r = ranges(&self.geometry, world);
        let [xs, xe, ys, ye] = r;
        let count = self.geometry.count;
        let wide = |lo: u32, hi: u32| hi.wrapping_sub(lo).wrapping_add(1) >= count;
        let cells = cell_count(&r);
        if (wide(xs, xe) && wide(ys, ye))
            || self.entries.saturating_add(cells) > MAX_BIN_CELL_ENTRIES
        {
            let _ = charge(ENTRY_WORK);
            self.overflow.push(obj);
            self.placed.insert(
                obj,
                Placement {
                    range: r,
                    overflow: true,
                },
            );
            return true;
        }
        let _ = charge(CELL_WORK.saturating_mul(cells as u64));
        let mut y = ys;
        while y <= ye {
            let mut x = xs;
            while x <= xe {
                let at = self.geometry.cell(x, y);
                self.cells.entry(at).or_default().push(obj);
                x += 1;
            }
            y += 1;
        }
        self.entries += cells;
        self.placed.insert(
            obj,
            Placement {
                range: r,
                overflow: false,
            },
        );
        true
    }

    /// Takes an object out of every list. False when it was not placed. Each list is a `Vec` filtered in place (O(its length),
    /// as a walk of the cell's chain); the cells of an ordinary world hold a handful of objects. Uncharged:
    /// [`Bins::remove_charged`] with a free budget.
    pub fn remove(&mut self, obj: u32) -> bool {
        self.remove_charged(obj, &mut |_| true)
    }

    /// [`Bins::remove`], each cell charged [`CELL_WORK`] and each entry a list's filter looks at [`ENTRY_WORK`]. Always carried
    /// out whole (an object that leaves must not stay in a list); `charge`'s refusal only ends the caller's run. The work is
    /// bounded by [`MAX_BIN_CELL_ENTRIES`].
    pub fn remove_charged(&mut self, obj: u32, charge: Charge) -> bool {
        let Some(p) = self.placed.remove(&obj) else {
            return false;
        };
        if p.overflow {
            let _ = charge(ENTRY_WORK.saturating_mul(self.overflow.len() as u64));
            self.overflow.retain(|o| *o != obj);
            return true;
        }
        let [xs, xe, ys, ye] = p.range;
        let _ = charge(CELL_WORK.saturating_mul(cell_count(&p.range) as u64));
        let mut y = ys;
        while y <= ye {
            let mut x = xs;
            while x <= xe {
                let at = self.geometry.cell(x, y);
                if let Some(list) = self.cells.get_mut(&at) {
                    let _ = charge(ENTRY_WORK.saturating_mul(list.len() as u64));
                    let before = list.len();
                    list.retain(|o| *o != obj);
                    self.entries -= before - list.len();
                    if list.is_empty() {
                        self.cells.remove(&at);
                    }
                }
                x += 1;
            }
            y += 1;
        }
        true
    }

    /// `checkBins`, run after a move: an object that is not placed is placed; one whose four numbers are unchanged
    /// stays where it is; any other leaves its lists and enters the heads of the new ones. Returns whether anything changed.
    /// Uncharged: [`Bins::check_charged`] with a free budget.
    pub fn check(&mut self, obj: u32, world: &WorldBox) -> bool {
        self.check_charged(obj, world, &mut |_| true)
    }

    /// [`Bins::check`] charged: the compare [`ENTRY_WORK`]; a move first pays [`CELL_WORK`] for each cell it leaves and enters,
    /// and when that is refused the object stays where it is (the caller's run ends); then the removal
    /// and the insertion are charged as [`Bins::remove_charged`] and [`Bins::insert_charged`] charge them.
    pub fn check_charged(&mut self, obj: u32, world: &WorldBox, charge: Charge) -> bool {
        if !charge(ENTRY_WORK) {
            return false;
        }
        let Some(p) = self.placed.get(&obj) else {
            return self.insert_charged(obj, world, charge);
        };
        let r = ranges(&self.geometry, world);
        if p.range == r {
            return false;
        }
        let old = if p.overflow { 1 } else { cell_count(&p.range) };
        let cells = old.saturating_add(cell_count(&r));
        if !charge(CELL_WORK.saturating_mul(cells as u64)) {
            return false;
        }
        self.remove_charged(obj, charge);
        self.insert_charged(obj, world, charge)
    }

    /// The entries the cells hold together (an object is in each of its cells once, twice in a cell a one-axis ring visits
    /// twice).
    pub fn cell_entries(&self) -> usize {
        self.entries
    }

    /// `Container::findObjects` over the bins (after the container's indexes), uncharged: [`Bins::find`] with a free budget and
    /// no stop (the ghost layer's scope walk, which bounds itself by the scope box).
    pub fn walk(
        &self,
        query: [f32; 6],
        mask: u32,
        geom: &dyn Geometry,
        found: &mut dyn FnMut(u32),
    ) {
        let _ = self.find(query, mask, geom, &mut |_| true, &mut |o| {
            found(o);
            true
        });
    }

    /// `Container::findObjects` over the bins (after the container's indexes): calls `found` for each object the query meets,
    /// in the bins' order, until it answers false (a caller's limit: the walk stops there). `query` is `[min x, min y, min z,
    /// max x, max y, max z]`. A mask without a bit the bins answer for looks at nothing. Each cell ([`CELL_WORK`]) and each
    /// list entry ([`ENTRY_WORK`]) is paid for before it is looked at.
    pub fn find(
        &self,
        query: [f32; 6],
        mask: u32,
        geom: &dyn Geometry,
        charge: Charge,
        found: &mut dyn FnMut(u32) -> bool,
    ) -> Result<(), Exhausted> {
        let mask = mask & self.geometry.mask;
        if mask == 0 {
            return Ok(());
        }
        let (xs, xe) = self.geometry.range(query[0], query[3]);
        let (ys, ye) = self.geometry.range(query[1], query[4]);
        let mut seen = Seen::default();
        // The stamp is set before the tests; an object whose type does not match or whose collision is disabled is not found;
        // then the world boxes overlap, non-strict.
        let mut test = |obj: u32, charge: &mut dyn FnMut(u64) -> bool| -> Result<bool, Exhausted> {
            if !charge(ENTRY_WORK) {
                return Err(Exhausted);
            }
            if !seen.insert(obj) {
                return Ok(true);
            }
            if geom.type_mask(obj) & mask == 0 || geom.collision_disabled(obj) {
                return Ok(true);
            }
            let (lo, hi) = geom.world_box(obj);
            let meets = (0..3).all(|a| query[a] <= hi[a] && lo[a] <= query[3 + a]);
            Ok(!meets || found(obj))
        };
        let mut y = ys;
        while y <= ye {
            let mut x = xs;
            while x <= xe {
                if !charge(CELL_WORK) {
                    return Err(Exhausted);
                }
                if let Some(list) = self.cells.get(&self.geometry.cell(x, y)) {
                    for &obj in list.iter().rev() {
                        if !test(obj, charge)? {
                            return Ok(());
                        }
                    }
                }
                x += 1;
            }
            y += 1;
        }
        for &obj in self.overflow.iter().rev() {
            if !test(obj, charge)? {
                return Ok(());
            }
        }
        Ok(())
    }

    /// `Container::castRay` over the bins, after the container's indexes: hands `visit` each object the segment `start` to
    /// `end` may hit, in the bins' order, for its own `castRay` (the caller keeps the nearest, strictly). The input gate (six
    /// finite floats) and the indexes are the caller's. As TGE's:
    ///
    /// - only for a mask with a bit the bins answer for;
    /// - the **overflow list first**, head to tail, with no world-box test;
    /// - then only the cells the segment steps through ([`crate::library::SceneLibrary::ray_cells`], [`plain_ray_cells`] by
    ///   default);
    /// - in a cell, each list from its head: the stamp, the mask, the collision-disabled count, then `Box3F::collideLine` of
    ///   the world box ([`crate::geometry::ray_box`]).
    ///
    /// Each segment ([`SEGMENT_WORK`]), cell ([`CELL_WORK`]) and entry ([`ENTRY_WORK`]) is paid for first.
    pub fn cast_ray(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mask: u32,
        geom: &dyn Geometry,
        charge: Charge,
        visit: &mut dyn FnMut(u32) -> Result<(), Exhausted>,
    ) -> Result<(), Exhausted> {
        let mask = mask & self.geometry.mask;
        if mask == 0 {
            return Ok(());
        }
        let g = &self.geometry;
        let mut seen = Seen::default();
        for &obj in self.overflow.iter().rev() {
            if !charge(ENTRY_WORK) {
                return Err(Exhausted);
            }
            if !seen.insert(obj) || geom.type_mask(obj) & mask == 0 || geom.collision_disabled(obj)
            {
                continue;
            }
            visit(obj)?;
        }
        let mut cell =
            |x: u32, y: u32, charge: &mut dyn FnMut(u64) -> bool| -> Result<(), Exhausted> {
                if !charge(CELL_WORK) {
                    return Err(Exhausted);
                }
                let Some(list) = self.cells.get(&g.cell(x, y)) else {
                    return Ok(());
                };
                for &obj in list.iter().rev() {
                    if !charge(ENTRY_WORK) {
                        return Err(Exhausted);
                    }
                    if !seen.insert(obj)
                        || geom.type_mask(obj) & mask == 0
                        || geom.collision_disabled(obj)
                    {
                        continue;
                    }
                    let (lo, hi) = geom.world_box(obj);
                    if ray_box(lo, hi, start, end) {
                        visit(obj)?;
                    }
                }
                Ok(())
            };
        (crate::library::get().ray_cells)(g, start, end, charge, &mut cell)
    }
}

/// The cell callback of the ray walk: an x and a y cell number, and the budget.
pub type CellVisit<'a> =
    dyn FnMut(u32, u32, &mut dyn FnMut(u64) -> bool) -> Result<(), Exhausted> + 'a;

/// The cells `Container::castRay` visits after the overflow list, in plain `f32` as TGE writes it: with `a` the end of smaller
/// x (the start on a tie), one column when the x span is under the wrap and in one cell column, else one row when the y span is
/// under the wrap and in one cell row (TGE takes the column whenever the x cells are one), else the general walk: the x span in
/// pieces of the wrap's width, in each the x cells and in each of those the y cells the segment covers there. Each piece of the
/// general walk is paid [`SEGMENT_WORK`] first; a piece that makes no progress (a coordinate so large that adding the wrap
/// changes nothing) stops the walk with [`Exhausted`].
pub fn plain_ray_cells(
    g: &BinGeometry,
    start: [f32; 3],
    end: [f32; 3],
    charge: Charge,
    cell: &mut CellVisit<'_>,
) -> Result<(), Exhausted> {
    let (a, b) = if start[0] <= end[0] {
        (start, end)
    } else {
        (end, start)
    };
    let min = |p: f32, q: f32| if p > q { q } else { p };
    let max = |p: f32, q: f32| if p > q { p } else { q };
    let total = g.wrap as f32;
    let (xs, xe) = g.range(a[0], b[0]);
    let (ys, ye) = g.range(min(a[1], b[1]), max(a[1], b[1]));
    if ((a[0] - b[0]).abs() < total && xs == xe) || ((a[1] - b[1]).abs() < total && ys == ye) {
        if xs == xe {
            for y in ys..=ye {
                cell(xs, y, charge)?;
            }
        } else {
            for x in xs..=xe {
                cell(x, ys, charge)?;
            }
        }
        return Ok(());
    }
    let size = g.size;
    let mut cur = a[0];
    while cur != b[0] {
        if !charge(SEGMENT_WORK) {
            return Err(Exhausted);
        }
        let cur_end = min(cur + total, b[0]);
        if cur_end == cur {
            return Err(Exhausted);
        }
        let t0 = (cur - a[0]) / (b[0] - a[0]);
        let t1 = (cur_end - a[0]) / (b[0] - a[0]);
        let y1 = a[1] + (b[1] - a[1]) * t0;
        let y2 = a[1] + (b[1] - a[1]) * t1;
        let (sub_first, sub_last) = g.range(cur, cur_end);
        let mut sub_start = cur;
        let mut sub_end = cur;
        if cur < 0.0 {
            sub_end -= sub_end % size;
        } else {
            sub_end += size - sub_end % size;
        }
        for x in sub_first..=sub_last {
            let st = (sub_start - cur) / (cur_end - cur);
            let et = min((sub_end - cur) / (cur_end - cur), 1.0);
            let sy1 = y1 + (y2 - y1) * st;
            let sy2 = y1 + (y2 - y1) * et;
            let (ny0, ny1) = g.range(min(sy1, sy2), max(sy1, sy2));
            for y in ny0..=ny1 {
                cell(x, y, charge)?;
            }
            sub_start = sub_end;
            sub_end = min(sub_end + size, cur_end);
        }
        cur = cur_end;
    }
    Ok(())
}

/// A world point in an object's space as `Container::castRay` moves the segment: `MatrixF::mulP` by the world-to-object matrix
/// `w2o`, then `Point3F::convolveInverse` by the object's `scale` (through the [`crate::library`] table).
pub fn object_point(w2o: &[f32; 16], scale: [f32; 3], p: [f32; 3]) -> [f32; 3] {
    (crate::library::get().object_point)(w2o, scale, p)
}

#[cfg(test)]
#[path = "bins_tests.rs"]
mod tests;
