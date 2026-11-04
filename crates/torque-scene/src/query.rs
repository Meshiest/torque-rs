//! The container's searches: the boxes the scripts' searches turn into, the distances the radius search filters by and every
//! search sorts by, and the search lists `containerSearchNext` and `containerFindNext` walk.
//!
//! **What is where.** The order objects come out of the container is its own. The order the sort leaves equal distances in is
//! the table's ([`crate::library::SceneLibrary::sort_by_distance`]; a stable sort by default). This file holds the arithmetic
//! and the lists around them: [`Search::start`] hands the found objects to that sort and keeps what it returns.
//!
//! These are TGE's `initContainerRadiusSearch`, `initContainerBoxSearch`, `containerSearchNext`, `containerSearchCurrDist`,
//! `containerSearchCurrRadiusDist`, the search comparator `cmpSearchPointers`, `containerFindFirst` and `containerFindNext`
//! (`sim/sceneObject.cc`; Torque3D's `scene/sceneContainer.cpp`, `Copyright (c) 2012 GarageGames, LLC`).

use crate::geometry::{Charge, Exhausted};

/// The work a search is charged per candidate before its distance is worked out (work units; 64 make a step).
pub const CANDIDATE_WORK: u64 = 32;

/// The work a sort is charged per compare, as the compare is made (work units; 64 make a step). Not by an estimate up front: a
/// quicksort, as a host's table may install, is quadratic on lists a script can build.
pub const COMPARE_WORK: u64 = 16;

/// How many results one search list keeps (a scripted search that wants more than a million results is hostile).
pub const MAX_SEARCH_RESULTS: usize = 1_048_576;

/// The box of `initContainerRadiusSearch(pos, radius, ...)`: `pos - radius` to `pos + radius` on every axis
/// (`[min x, min y, min z, max x, max y, max z]`).
pub fn radius_box(center: [f32; 3], radius: f32) -> [f32; 6] {
    let mut out = [0.0; 6];
    for i in 0..3 {
        out[i] = center[i] - radius;
        out[3 + i] = center[i] + radius;
    }
    out
}

/// The box of `initContainerBoxSearch(pos, size, ...)`: `pos` minus and plus half of `size`.
pub fn size_box(center: [f32; 3], size: [f32; 3]) -> [f32; 6] {
    let mut out = [0.0; 6];
    for i in 0..3 {
        let half = size[i] * 0.5;
        out[i] = center[i] - half;
        out[3 + i] = half + center[i];
    }
    out
}

/// The box of a centre and three radii: the centre minus and plus the radii as given (a negative radius turns its axis inside
/// out).
pub fn radii_box(center: [f32; 3], radii: [f32; 3]) -> [f32; 6] {
    let mut out = [0.0; 6];
    for i in 0..3 {
        out[i] = center[i] - radii[i];
        out[3 + i] = radii[i] + center[i];
    }
    out
}

/// The box of `containerFindFirst(mask, pos, xRadius, yRadius, zRadius)`: the centre minus and plus the **absolute** radii.
pub fn find_box(center: [f32; 3], radii: [f32; 3]) -> [f32; 6] {
    radii_box(center, radii.map(f32::abs))
}

/// The distance from `point` to the centre of the world box (the search sort, `containerSearchCurrDist`), through the
/// [`crate::library`] table.
pub fn center_distance(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
    (crate::library::get().center_distance)(min, max, point)
}

/// `containerSearchCurrRadiusDist`: [`center_distance`] less the box's smallest side, but not below zero.
pub fn radius_distance(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
    (crate::library::get().radius_distance)(min, max, point)
}

/// The squared distance from `point` to the world box, the radius search's filter: per axis nothing inside the box's span,
/// else the square of the gap to the near face.
pub fn box_distance_squared(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
    (crate::library::get().box_distance_squared)(min, max, point)
}

/// The radius search keeps an object whose box is strictly closer than the radius: `distance^2 < radius * radius`.
pub fn within_radius(distance_squared: f32, radius: f32) -> bool {
    (crate::library::get().within_radius)(distance_squared, radius)
}

/// The search maths in plain `f32` and a stable sort, as TGE's `Container` search functions write them.
pub mod plain {
    use super::compare_distances;
    use crate::geometry::Exhausted;

    /// `(centre - point).len()` with the centre `(min + max) * 0.5`.
    pub fn center_distance(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
        let d = [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5 - point[i]);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    /// The centre distance less the smallest of the box's three lengths, 0 when that is below 0.
    pub fn radius_distance(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
        let mut dist = center_distance(min, max, point);
        let mut side = max[0] - min[0];
        for i in 1..3 {
            if max[i] - min[i] < side {
                side = max[i] - min[i];
            }
        }
        dist -= side;
        if dist < 0.0 { 0.0 } else { dist }
    }

    /// `initRadiusSearch`'s sum: per axis below the box `(p - min)^2`, above it `(p - max)^2`.
    pub fn box_distance_squared(min: [f32; 3], max: [f32; 3], point: [f32; 3]) -> f32 {
        let mut sum = 0.0f32;
        for i in 0..3 {
            if point[i] < min[i] {
                sum += (point[i] - min[i]) * (point[i] - min[i]);
            } else if point[i] > max[i] {
                sum += (point[i] - max[i]) * (point[i] - max[i]);
            }
        }
        sum
    }

    /// `sum < radius * radius`.
    pub fn within_radius(distance_squared: f32, radius: f32) -> bool {
        distance_squared < radius * radius
    }

    /// A stable sort by distance (NaNs last, equal distances in the order given), each compare paid first until a payment is
    /// refused. A merge sort makes at most `n log n` compares, so a refusal only ends the run: the sort finishes unpaid and
    /// answers [`Exhausted`].
    pub fn sort_by_distance(
        items: &mut [(f32, u32)],
        pay: &mut dyn FnMut() -> bool,
    ) -> Result<(), Exhausted> {
        let mut paid = true;
        items.sort_by(|a, b| {
            if paid {
                paid = pay();
            }
            match (a.0.is_nan(), b.0.is_nan()) {
                (false, false) => compare_distances(a.0, b.0).cmp(&0),
                (x, y) => x.cmp(&y),
            }
        });
        if paid { Ok(()) } else { Err(Exhausted) }
    }
}

/// The sort's comparator (`cmpSearchPointers`) on two distances: 1 when the first is greater, -1 when it is less, 0 otherwise
/// (equal, or either not a number).
pub fn compare_distances(a: f32, b: f32) -> i32 {
    if b < a {
        1
    } else if a < b {
        -1
    } else {
        0
    }
}

/// What a search list holds: anything that names the object it was found as (an index's entry, or the container's own record
/// for the objects of its bins).
pub trait SearchItem: Copy {
    fn object(&self) -> u32;
}

/// The list a search keeps for `containerSearchNext` and the two distance calls: the results in sorted order, a cursor that
/// starts before the first, and the point the distances are measured from.
#[derive(Debug, Clone)]
pub struct Search<T: SearchItem> {
    entries: Vec<T>,
    /// -1 before the first result, the index of the current one, or the count when the list is used up.
    cursor: i64,
    center: [f32; 3],
}

impl<T: SearchItem> Default for Search<T> {
    fn default() -> Search<T> {
        Search {
            entries: Vec::new(),
            cursor: -1,
            center: [0.0; 3],
        }
    }
}

impl<T: SearchItem> Search<T> {
    pub fn new() -> Search<T> {
        Search::default()
    }

    /// How many results the list holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The point the last search measured from.
    pub fn center(&self) -> [f32; 3] {
        self.center
    }

    /// Empties the list and puts the cursor before the first result (the start of every search).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = -1;
    }

    /// The end of every search: the point is kept, and the results are sorted by [`center_distance`] to it with the table's
    /// sort. `candidates` are the objects in the order the container gave them (the radius search has already dropped those
    /// outside the radius); a list over [`MAX_SEARCH_RESULTS`] is cut there. `world_box` reads an object's box now. Every
    /// candidate is paid for ([`CANDIDATE_WORK`], before its distance is worked out) and so is every compare the sort makes
    /// ([`COMPARE_WORK`], before the compare): [`Exhausted`] when the run cannot pay, with the list empty.
    pub fn start(
        &mut self,
        center: [f32; 3],
        mut candidates: Vec<T>,
        world_box: &dyn Fn(u32) -> ([f32; 3], [f32; 3]),
        charge: Charge,
    ) -> Result<(), Exhausted> {
        self.clear();
        self.center = center;
        candidates.truncate(MAX_SEARCH_RESULTS);
        if !charge(CANDIDATE_WORK.saturating_mul(candidates.len() as u64)) {
            return Err(Exhausted);
        }
        let mut keyed: Vec<(f32, u32)> = candidates
            .iter()
            .zip(0u32..)
            .map(|(entry, at)| {
                let (min, max) = world_box(entry.object());
                (center_distance(min, max, center), at)
            })
            .collect();
        (crate::library::get().sort_by_distance)(&mut keyed, &mut || charge(COMPARE_WORK))?;
        self.entries = keyed
            .iter()
            .filter_map(|&(_, at)| candidates.get(at as usize).copied())
            .collect();
        Ok(())
    }

    /// `containerSearchNext`: the next result `alive` still says is the object it was found as, or none at the end. The cursor
    /// stays at the count once the list is used up.
    pub fn next(&mut self, alive: &dyn Fn(T) -> bool) -> Option<T> {
        let count = i64::try_from(self.entries.len()).unwrap_or(i64::MAX);
        if self.cursor >= count {
            return None;
        }
        self.cursor += 1;
        while self.cursor < count {
            let entry = self.entries.get(usize::try_from(self.cursor).ok()?)?;
            if alive(*entry) {
                return Some(*entry);
            }
            self.cursor += 1;
        }
        None
    }

    /// The result the cursor is on, if it is on one `alive` still says is the object it was found as (what the two distance
    /// calls measure; none for 0.0).
    pub fn current(&self, alive: &dyn Fn(T) -> bool) -> Option<T> {
        let index = usize::try_from(self.cursor).ok()?;
        let entry = self.entries.get(index)?;
        alive(*entry).then_some(*entry)
    }

    /// The results as the list holds them (a test helper).
    pub fn entries(&self) -> &[T] {
        &self.entries
    }
}

/// The list `containerFindFirst` and `containerFindNext` walk: the objects found, unsorted, in the order the container gave
/// them, and a cursor.
#[derive(Debug, Clone, Default)]
pub struct FindList {
    objects: Vec<u32>,
    cursor: usize,
}

impl FindList {
    pub fn new() -> FindList {
        FindList::default()
    }

    /// A new search: the list, and the cursor back at the start. A list over [`MAX_SEARCH_RESULTS`] is cut there.
    pub fn start(&mut self, mut objects: Vec<u32>) {
        objects.truncate(MAX_SEARCH_RESULTS);
        self.objects = objects;
        self.cursor = 0;
    }

    /// The next object's id, none at the end (an object deleted since is still named by the id it was found as).
    pub fn next_object(&mut self) -> Option<u32> {
        let object = *self.objects.get(self.cursor)?;
        self.cursor += 1;
        Some(object)
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
