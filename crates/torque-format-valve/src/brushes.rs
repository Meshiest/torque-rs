//! A map's brushes: the convex solids a brush model (a trigger's, a door's, the world's) is built from, as hulls.
//!
//! **Which brushes are a model's.** The model's tree: from its head node down every child to its leaves, and each leaf's
//! brushes (the leaf-brush lump's indices into the brush lump). A brush in several leaves counts once; the list is in brush
//! order. Each brush is its sides' planes (the side lump's plane numbers; a plane faces out of the brush: inside where
//! `n . p <= dist`), bevel planes among them.
//!
//! **Hulls.** A brush's corners are the points where three of its planes meet that lie inside all of them; their convex hull
//! ([`crate::hull::convex_hull`]) is the brush as a hull, in the model's own space (a brush entity's origin adds to it, as it
//! does to its faces). A brush whose planes enclose no volume gives none.
//!
//! **Bounds.** At most [`MAX_SIDES`] sides a brush (a brush with more is left out), [`MAX_MODEL_BRUSHES`] brushes a model and
//! [`MAX_WALK`] steps a walk (each node, each leaf and each entry a leaf lists is a step; each node and leaf is visited once);
//! every index is checked against its lump. A map's walks and hulls share one work budget ([`MapHulls`], [`MAP_HULL_WORK`]);
//! each head node is walked once and each model's hulls are made once.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::bsp::{Bsp, lump};
use crate::hull::{Hull, convex_hull};
use crate::read::{Result, i32_at, u16_at};

/// The lump of brushes (first side, side count, contents).
pub const BRUSHES: usize = 18;
/// The lump of brush sides (plane, texture placement, displacement, bevel).
pub const BRUSH_SIDES: usize = 19;
/// The lump of leaf brushes (indices into the brushes).
pub const LEAF_BRUSHES: usize = 17;

/// The most sides of one brush made into a hull.
pub const MAX_SIDES: usize = 64;
/// The most brushes of one model.
pub const MAX_MODEL_BRUSHES: usize = 4096;
/// The most nodes and leaves one walk visits.
pub const MAX_WALK: usize = 1 << 20;
/// The hull builder's budget for one brush.
pub const BRUSH_HULL_WORK: u64 = 1 << 20;

/// A brush: its sides' range and its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Brush {
    pub first_side: i32,
    pub sides: i32,
    pub contents: i32,
}

impl Bsp {
    /// The brushes.
    pub fn brushes(&self) -> Result<Vec<Brush>> {
        let b = self.lump(BRUSHES)?;
        (0..b.len() / 12)
            .map(|k| {
                let at = k * 12;
                Ok(Brush {
                    first_side: i32_at(b, at, "a brush")?,
                    sides: i32_at(b, at + 4, "a brush")?,
                    contents: i32_at(b, at + 8, "a brush")?,
                })
            })
            .collect()
    }

    /// Each brush side's plane number.
    pub fn brush_side_planes(&self) -> Result<Vec<u16>> {
        let b = self.lump(BRUSH_SIDES)?;
        (0..b.len() / 8)
            .map(|k| u16_at(b, k * 8, "a brush side"))
            .collect()
    }

    /// The leaf brushes.
    pub fn leaf_brushes(&self) -> Result<Vec<u16>> {
        let b = self.lump(LEAF_BRUSHES)?;
        (0..b.len() / 2)
            .map(|k| u16_at(b, k * 2, "a leaf brush"))
            .collect()
    }

    /// Each leaf's brush range (first, count) in the leaf-brush list.
    pub fn leaf_brush_ranges(&self) -> Result<Vec<(u16, u16)>> {
        let size = if self.lump_version(lump::LEAFS) == 0 && self.version < 20 {
            56
        } else {
            32
        };
        let b = self.lump(lump::LEAFS)?;
        (0..b.len() / size)
            .map(|k| {
                let at = k * size;
                Ok((u16_at(b, at + 24, "a leaf")?, u16_at(b, at + 26, "a leaf")?))
            })
            .collect()
    }

    /// The brushes of model `model` (module docs), in brush order.
    pub fn model_brushes(&self, model: usize) -> Result<Vec<usize>> {
        Ok(MapHulls::new(self)?.brushes_of(model).to_vec())
    }

    /// The hull of brush `brush` (module docs), None when it has no volume, too many sides or costs more than one brush's
    /// budget.
    pub fn brush_hull(&self, brush: usize) -> Result<Option<Hull>> {
        let brushes = self.brushes()?;
        let sides = self.brush_side_planes()?;
        let planes = self.planes()?;
        let mut work = brush_cost(&brushes, brush).saturating_add(BRUSH_HULL_WORK);
        Ok(brush_hull_of(&brushes, &sides, &planes, brush, &mut work))
    }

    /// The hulls of model `model`'s brushes whose contents have any bit of `contents` (`!0` for every brush), within one
    /// map's budget ([`MapHulls`] keeps the lumps and the hulls across models).
    pub fn model_hulls(&self, model: usize, contents: i32) -> Result<Vec<Hull>> {
        Ok(MapHulls::new(self)?.model(model, contents).to_vec())
    }
}

/// The work for the hulls of one map: plane triples tested (each against every plane) and the hull builder's steps.
pub const MAP_HULL_WORK: u64 = 1 << 28;

/// A map's brush lumps read once, and its models' hulls made when first asked for and kept, within one work budget for the
/// map ([`MAP_HULL_WORK`]): brush entities that share a model build its hulls once, and a brush that would pass the budget is
/// left out (counted in [`MapHulls::refused`]).
#[derive(Debug, Clone, Default)]
pub struct MapHulls {
    heads: Vec<i32>,
    nodes: Vec<(i32, [i32; 2])>,
    ranges: Vec<(u16, u16)>,
    leaf_brushes: Vec<u16>,
    brushes: Vec<Brush>,
    sides: Vec<u16>,
    planes: Vec<([f32; 3], f32)>,
    cache: HashMap<(usize, i32), Rc<Vec<Hull>>>,
    /// Each head node's brushes, walked once.
    walked: HashMap<i32, Rc<Vec<usize>>>,
    /// The work left.
    pub work_left: u64,
    /// Brushes left out because the budget ran out.
    pub refused: usize,
}

impl MapHulls {
    /// The lumps of `bsp`, with the whole budget.
    pub fn new(bsp: &Bsp) -> Result<MapHulls> {
        Ok(MapHulls {
            heads: bsp.models()?.iter().map(|m| m.head_node).collect(),
            nodes: bsp.nodes()?,
            ranges: bsp.leaf_brush_ranges()?,
            leaf_brushes: bsp.leaf_brushes()?,
            brushes: bsp.brushes()?,
            sides: bsp.brush_side_planes()?,
            planes: bsp.planes()?,
            cache: HashMap::new(),
            walked: HashMap::new(),
            work_left: MAP_HULL_WORK,
            refused: 0,
        })
    }

    /// The brushes of model `model` (module docs): each node and each leaf visited once, at most [`MAX_MODEL_BRUSHES`]
    /// brushes and [`MAX_WALK`] steps, in brush order.
    pub fn brushes_of(&mut self, model: usize) -> Rc<Vec<usize>> {
        let Some(&head) = self.heads.get(model) else {
            return Rc::new(Vec::new());
        };
        if let Some(w) = self.walked.get(&head) {
            return w.clone();
        }
        let limit = (MAX_WALK as u64).min(self.work_left) as usize;
        let (out, steps) = walk(
            &self.nodes,
            &self.ranges,
            &self.leaf_brushes,
            self.brushes.len(),
            head,
            limit,
        );
        self.work_left = self.work_left.saturating_sub(steps as u64);
        let out = Rc::new(out);
        self.walked.insert(head, out.clone());
        out
    }

    /// The hulls of model `model`'s brushes whose contents have any bit of `contents`, made once.
    pub fn model(&mut self, model: usize, contents: i32) -> Rc<Vec<Hull>> {
        if let Some(h) = self.cache.get(&(model, contents)) {
            return h.clone();
        }
        let mut out = Vec::new();
        for &b in self.brushes_of(model).iter() {
            if !self
                .brushes
                .get(b)
                .is_some_and(|x| x.contents & contents != 0)
            {
                continue;
            }
            if brush_cost(&self.brushes, b) > self.work_left {
                self.refused += 1;
                continue;
            }
            if let Some(h) = brush_hull_of(
                &self.brushes,
                &self.sides,
                &self.planes,
                b,
                &mut self.work_left,
            ) {
                out.push(h);
            }
        }
        let out = Rc::new(out);
        self.cache.insert((model, contents), out.clone());
        out
    }
}

/// The brushes under node `head` (module docs).
fn walk(
    nodes: &[(i32, [i32; 2])],
    ranges: &[(u16, u16)],
    leaf_brushes: &[u16],
    brushes: usize,
    head: i32,
    limit: usize,
) -> (Vec<usize>, usize) {
    let mut out = Vec::new();
    let mut stack = vec![head];
    // Sets of what was visited, so a walk costs what it visits, not the size of the lumps.
    let mut seen_nodes = HashSet::new();
    let mut seen_leaves = HashSet::new();
    let mut seen_brushes = HashSet::new();
    let mut steps = 0usize;
    'walk: while let Some(n) = stack.pop() {
        steps += 1;
        if steps > limit {
            break;
        }
        if n < 0 {
            let leaf = (-(i64::from(n) + 1)) as usize;
            let Some(&(first, count)) = ranges.get(leaf) else {
                continue;
            };
            if !seen_leaves.insert(leaf) {
                continue;
            }
            for k in usize::from(first)..usize::from(first) + usize::from(count) {
                // Every entry a leaf lists is a step, a brush seen before included.
                steps += 1;
                if steps > limit {
                    break 'walk;
                }
                let Some(&b) = leaf_brushes.get(k) else {
                    break;
                };
                let b = usize::from(b);
                if b < brushes && seen_brushes.insert(b) {
                    out.push(b);
                    if out.len() >= MAX_MODEL_BRUSHES {
                        break 'walk;
                    }
                }
            }
            continue;
        }
        if nodes.get(n as usize).is_none() || !seen_nodes.insert(n) {
            continue;
        }
        let (_, children) = nodes[n as usize];
        stack.extend(children);
    }
    out.sort_unstable();
    (out, steps.min(limit))
}

/// What a brush's hull costs: its plane triples, each tested against every plane (0 for a brush that is not made).
pub fn brush_cost(brushes: &[Brush], brush: usize) -> u64 {
    let Some(n) = brushes
        .get(brush)
        .and_then(|b| usize::try_from(b.sides).ok())
        .filter(|n| (4..=MAX_SIDES).contains(n))
    else {
        return 0;
    };
    let n = n as u64;
    n * (n - 1) * (n - 2) / 6 * (n + 1)
}

/// A brush's hull from the three lumps (module docs), charged to `work` ([`brush_cost`], then the hull builder's steps up to
/// [`BRUSH_HULL_WORK`]); None when it has no volume or `work` cannot pay for it.
pub fn brush_hull_of(
    brushes: &[Brush],
    sides: &[u16],
    planes: &[([f32; 3], f32)],
    brush: usize,
    work: &mut u64,
) -> Option<Hull> {
    let b = brushes.get(brush)?;
    let (first, n) = (
        usize::try_from(b.first_side).ok()?,
        usize::try_from(b.sides).ok()?,
    );
    if !(4..=MAX_SIDES).contains(&n) {
        return None;
    }
    let cost = brush_cost(brushes, brush);
    if cost > *work {
        return None;
    }
    *work -= cost;
    let mut ps: Vec<([f64; 3], f64)> = Vec::with_capacity(n);
    for k in first..first.checked_add(n)? {
        let &p = sides.get(k)?;
        let &(normal, dist) = planes.get(usize::from(p))?;
        if !(normal.iter().all(|v| v.is_finite()) && dist.is_finite()) {
            return None;
        }
        ps.push((normal.map(f64::from), f64::from(dist)));
    }
    let mut points: Vec<[f32; 3]> = Vec::new();
    for i in 0..ps.len() {
        for j in i + 1..ps.len() {
            for k in j + 1..ps.len() {
                let Some(p) = meet(ps[i], ps[j], ps[k]) else {
                    continue;
                };
                if ps
                    .iter()
                    .all(|(nn, d)| nn[0] * p[0] + nn[1] * p[1] + nn[2] * p[2] - d <= 0.01)
                {
                    points.push(p.map(|v| v as f32));
                }
            }
        }
    }
    let budget = BRUSH_HULL_WORK.min(*work);
    let mut left = budget;
    let hull = convex_hull(&points, &mut left);
    *work -= budget - left.min(budget);
    hull
}

/// Where three planes meet, if they do at one point.
fn meet(a: ([f64; 3], f64), b: ([f64; 3], f64), c: ([f64; 3], f64)) -> Option<[f64; 3]> {
    let cross = |u: [f64; 3], v: [f64; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let dot = |u: [f64; 3], v: [f64; 3]| u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let bc = cross(b.0, c.0);
    let det = dot(a.0, bc);
    if det.abs() < 1e-9 {
        return None;
    }
    let ca = cross(c.0, a.0);
    let ab = cross(a.0, b.0);
    let p = [0, 1, 2].map(|k| (a.1 * bc[k] + b.1 * ca[k] + c.1 * ab[k]) / det);
    p.iter()
        .all(|v| v.is_finite() && v.abs() < 1e7)
        .then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bsp::layout;

    /// A map of two box brushes: the world (model 0) holds the first, model 1 (a brush entity) the second; each model's tree
    /// is one node over two leaves.
    pub(crate) fn brush_map() -> Vec<u8> {
        let mut lumps: Vec<Vec<u8>> = vec![Vec::new(); 64];
        let f = |v: &mut Vec<u8>, x: f32| v.extend_from_slice(&x.to_le_bytes());
        let i = |v: &mut Vec<u8>, x: i32| v.extend_from_slice(&x.to_le_bytes());
        let h = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        lumps[lump::ENTITIES] = b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec();
        let boxes = [
            ([0.0f32, 0.0, 0.0], [64.0f32, 64.0, 8.0]),
            ([-4.0, -32.0, 0.0], [4.0, 32.0, 96.0]),
        ];
        let mut plane = 0u16;
        for (bi, (lo, hi)) in boxes.iter().enumerate() {
            for (n, d) in [
                ([1.0f32, 0.0, 0.0], hi[0]),
                ([-1.0, 0.0, 0.0], -lo[0]),
                ([0.0, 1.0, 0.0], hi[1]),
                ([0.0, -1.0, 0.0], -lo[1]),
                ([0.0, 0.0, 1.0], hi[2]),
                ([0.0, 0.0, -1.0], -lo[2]),
            ] {
                for c in n {
                    f(&mut lumps[lump::PLANES], c);
                }
                f(&mut lumps[lump::PLANES], d);
                i(&mut lumps[lump::PLANES], 0);
                h(&mut lumps[BRUSH_SIDES], plane);
                h(&mut lumps[BRUSH_SIDES], 0);
                h(&mut lumps[BRUSH_SIDES], 0);
                h(&mut lumps[BRUSH_SIDES], 0);
                plane += 1;
            }
            i(&mut lumps[BRUSHES], bi as i32 * 6);
            i(&mut lumps[BRUSHES], 6);
            i(&mut lumps[BRUSHES], if bi == 0 { 1 } else { 0x0040_0000 });
            h(&mut lumps[LEAF_BRUSHES], bi as u16);
        }
        // Leaves: 0 and 1 for the world (leaf 1 holds brush 0), 2 and 3 for model 1 (leaf 3 holds brush 1).
        for l in 0..4u16 {
            let mut leaf = vec![0u8; 32];
            let (first, count) = match l {
                1 => (0u16, 1u16),
                3 => (1, 1),
                _ => (0, 0),
            };
            leaf[24..26].copy_from_slice(&first.to_le_bytes());
            leaf[26..28].copy_from_slice(&count.to_le_bytes());
            lumps[lump::LEAFS].extend_from_slice(&leaf);
        }
        // Nodes: node 0 over leaves 0 and 1, node 1 over leaves 2 and 3.
        for (children, _) in [([-1i32, -2], 0), ([-3, -4], 1)] {
            let mut node = vec![0u8; 32];
            node[4..8].copy_from_slice(&children[0].to_le_bytes());
            node[8..12].copy_from_slice(&children[1].to_le_bytes());
            lumps[lump::NODES].extend_from_slice(&node);
        }
        for (head, (lo, hi)) in [(0i32, boxes[0]), (1, boxes[1])] {
            for c in lo.iter().chain(hi.iter()).chain([0.0f32; 3].iter()) {
                f(&mut lumps[lump::MODELS], *c);
            }
            i(&mut lumps[lump::MODELS], head);
            i(&mut lumps[lump::MODELS], 0);
            i(&mut lumps[lump::MODELS], 0);
        }
        layout(lumps, &[])
    }

    #[test]
    fn a_model_s_brushes_become_hulls() {
        let map = Bsp::parse(&brush_map()).unwrap();
        assert_eq!(map.brushes().unwrap().len(), 2);
        assert_eq!(map.model_brushes(0).unwrap(), [0]);
        assert_eq!(map.model_brushes(1).unwrap(), [1]);
        assert!(map.model_brushes(9).unwrap().is_empty());
        let hulls = map.model_hulls(1, !0).unwrap();
        assert_eq!(hulls.len(), 1);
        let (lo, hi) = hulls[0].bounds();
        assert_eq!((lo, hi), ([-4.0, -32.0, 0.0], [4.0, 32.0, 96.0]));
        assert_eq!(hulls[0].points.len(), 8);
        assert!(hulls[0].contains([0.0, 0.0, 50.0], 0.0));
        // Contents select: the world's brush is solid (1), the entity's not.
        assert_eq!(map.model_hulls(0, 1).unwrap().len(), 1);
        assert!(map.model_hulls(1, 1).unwrap().is_empty());
    }

    #[test]
    fn hostile_brush_tables_give_nothing_rather_than_panic() {
        let planes = vec![([1.0f32, 0.0, 0.0], 1.0); 3];
        let brushes = [
            Brush {
                first_side: -1,
                sides: 6,
                contents: 1,
            },
            Brush {
                first_side: 0,
                sides: i32::MAX,
                contents: 1,
            },
            Brush {
                first_side: i32::MAX,
                sides: 6,
                contents: 1,
            },
            Brush {
                first_side: 0,
                sides: 3,
                contents: 1,
            },
            Brush {
                first_side: 0,
                sides: 6,
                contents: 1,
            },
        ];
        let sides = [0u16, 1, 2, 99, 0, 0];
        for b in 0..6 {
            let mut work = u64::MAX;
            assert!(brush_hull_of(&brushes, &sides, &planes, b, &mut work).is_none());
        }
    }

    #[test]
    fn mutated_brush_maps_never_panic() {
        use crate::fuzz::{Rng, mutate};
        let seed = brush_map();
        let mut r = Rng::new(0xB2_05E5);
        let mut panics = 0;
        for _ in 0..2_000 {
            let b = mutate(&seed, &mut r);
            let ok = std::panic::catch_unwind(|| {
                let Ok(map) = Bsp::parse(&b) else { return };
                for m in 0..3 {
                    let _ = map.model_hulls(m, !0);
                }
            });
            panics += usize::from(ok.is_err());
        }
        assert_eq!(panics, 0);
    }

    #[test]
    fn a_tree_that_reaches_one_leaf_many_times_walks_it_once() {
        // 100,000 nodes in a chain, each also pointing at leaf 0, which lists 65,535 brushes.
        let nodes: Vec<(i32, [i32; 2])> = (0..100_000).map(|i| (0, [i + 1, -1])).collect();
        let ranges = vec![(0u16, u16::MAX)];
        let leaf_brushes: Vec<u16> = (0..u16::MAX).collect();
        let t0 = std::time::Instant::now();
        let (out, _) = walk(&nodes, &ranges, &leaf_brushes, 70_000, 0, MAX_WALK);
        assert_eq!(out.len(), MAX_MODEL_BRUSHES);
        assert!(out.windows(2).all(|w| w[0] < w[1]));
        assert!(t0.elapsed().as_secs() < 5);
    }

    #[test]
    fn a_map_s_hulls_share_one_budget_and_a_model_is_built_once() {
        // 2,000 brushes of the most sides (a box's six planes over and over), all in one leaf of model 0.
        let planes: Vec<([f32; 3], f32)> = [
            ([1.0, 0.0, 0.0], 8.0),
            ([-1.0, 0.0, 0.0], 8.0),
            ([0.0, 1.0, 0.0], 8.0),
            ([0.0, -1.0, 0.0], 8.0),
            ([0.0, 0.0, 1.0], 8.0),
            ([0.0, 0.0, -1.0], 8.0),
        ]
        .to_vec();
        let n = MAX_SIDES;
        let sides: Vec<u16> = (0..n).map(|k| (k % 6) as u16).collect();
        let brushes = vec![
            Brush {
                first_side: 0,
                sides: n as i32,
                contents: 1,
            };
            2_000
        ];
        let mut mh = MapHulls {
            heads: vec![-1],
            ranges: vec![(0, 2_000)],
            leaf_brushes: (0..2_000).collect(),
            brushes,
            sides,
            planes,
            work_left: MAP_HULL_WORK,
            ..MapHulls::default()
        };
        let a = mh.model(0, !0);
        assert!(!a.is_empty() && a.len() < 2_000);
        assert_eq!(mh.refused, 2_000 - a.len());
        assert_eq!(a[0].bounds(), ([-8.0; 3], [8.0; 3]));
        let left = mh.work_left;
        let b = mh.model(0, !0);
        assert!(Rc::ptr_eq(&a, &b), "built once");
        assert_eq!(mh.work_left, left);
    }

    #[test]
    fn many_leaves_over_one_long_run_are_held_to_the_walk_limit_and_walked_once() {
        // 50,000 distinct leaves, each listing the same 65,535 entries (all one brush): 3.3 billion entries unbounded.
        let leaves = 50_000i32;
        let nodes: Vec<(i32, [i32; 2])> = (0..leaves).map(|i| (0, [i + 1, -(i + 1)])).collect();
        let ranges = vec![(0u16, u16::MAX); leaves as usize + 1];
        let leaf_brushes = vec![0u16; usize::from(u16::MAX)];
        let t0 = std::time::Instant::now();
        let (out, steps) = walk(&nodes, &ranges, &leaf_brushes, 1, 0, MAX_WALK);
        assert_eq!(out, [0]);
        assert_eq!(steps, MAX_WALK);
        let mut mh = MapHulls {
            heads: vec![0, 0, 0],
            nodes,
            ranges,
            leaf_brushes,
            brushes: vec![
                Brush {
                    first_side: 0,
                    sides: 0,
                    contents: 1,
                };
                1
            ],
            work_left: MAP_HULL_WORK,
            ..MapHulls::default()
        };
        for model in 0..3 {
            for contents in [1, 2, !0] {
                let _ = mh.model(model, contents);
            }
        }
        // Three models over one head node and three masks each: one walk charged to the map.
        assert_eq!(mh.work_left, MAP_HULL_WORK - MAX_WALK as u64);
        assert!(t0.elapsed().as_secs() < 10, "{:?}", t0.elapsed());
    }
}
