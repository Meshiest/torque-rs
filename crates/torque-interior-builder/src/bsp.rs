//! A solid BSP of the brushes: every leaf is solid (inside a brush) or empty (inside none).
//!
//! The splitters are the brushes' own planes. Each cell holds the parts of the brushes in it, each part with the planes of its
//! brush no splitter on the way down has taken yet; a plane taken as a splitter is taken from every part that has it (a brush
//! lies behind its own plane, so its part goes behind and needs that plane no more). A part with no planes left fills its
//! cell: the cell is a solid leaf. A cell with no parts is an empty leaf. Every split takes a plane for good, so the depth is at
//! most the number of planes. The splitter of a cell is the one that cuts fewest parts, then the one that leaves the two sides
//! most even (out of the first [`MAX_CANDIDATES`] planes of the cell's parts).

use crate::brush::{Brush, polygons_of};
use crate::geom::{EPS, Plane, V3};

/// The most planes weighed as a cell's splitter.
pub const MAX_CANDIDATES: usize = 64;

/// Where a node's side leads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Node(u32),
    /// A solid leaf: an index into [`Bsp::solid`].
    Solid(u32),
    Empty,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Node {
    pub plane: Plane,
    pub front: Child,
    pub back: Child,
}

/// The tree: node 0 is the root (a tree of no node is one leaf, [`Bsp::root`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Bsp {
    pub nodes: Vec<Node>,
    /// The brush that fills each solid leaf.
    pub solid: Vec<usize>,
    /// The whole tree when it is a single leaf.
    pub root: Option<Child>,
}

/// The most part-against-plane tests one tree may take (a splitter weighs each candidate against each part of its cell; a
/// part cut in two costs its planes squared).
pub const MAX_WORK: u64 = 400_000_000;

/// A tree past a cap: its nodes, or the tests making it took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooMuch {
    Nodes(usize),
    Work(u64),
}

struct Part {
    brush: usize,
    left: Vec<Plane>,
    clip: Vec<Plane>,
    corners: Vec<V3>,
}

fn corners(brush: &Brush, clip: &[Plane]) -> Option<Vec<V3>> {
    let planes = brush
        .faces
        .iter()
        .map(|f| f.plane)
        .chain(clip.iter().copied());
    let polys = polygons_of(planes).ok()?;
    Some(polys.into_iter().flatten().collect())
}

/// Which way a part lies from `s`: -1 behind, 1 in front, 0 across, 2 on it (a part too thin to hold).
fn side_of(part: &Part, s: &Plane) -> i8 {
    let (mut front, mut back) = (false, false);
    for &c in &part.corners {
        let d = s.dist(c);
        front |= d > EPS;
        back |= d < -EPS;
    }
    match (front, back) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => -1,
        (false, false) => 2,
    }
}

enum Slot {
    Root,
    Front(usize),
    Back(usize),
}

/// The tree of `brushes`, at most `max_nodes` nodes.
pub fn build(brushes: &[Brush], max_nodes: usize) -> Result<Bsp, TooMuch> {
    let mut parts = Vec::new();
    for (i, b) in brushes.iter().enumerate() {
        if let Some(c) = corners(b, &[]) {
            parts.push(Part {
                brush: i,
                left: b.faces.iter().map(|f| f.plane).collect(),
                clip: Vec::new(),
                corners: c,
            });
        }
    }
    let mut bsp = Bsp::default();
    let mut tasks = vec![(parts, Slot::Root)];
    let mut work = 0u64;
    while let Some((parts, slot)) = tasks.pop() {
        let child = if parts.is_empty() {
            Child::Empty
        } else if let Some(p) = parts.iter().find(|p| p.left.is_empty()) {
            bsp.solid.push(p.brush);
            Child::Solid(bsp.solid.len() as u32 - 1)
        } else {
            if bsp.nodes.len() >= max_nodes {
                return Err(TooMuch::Nodes(max_nodes));
            }
            let s = splitter(&parts, &mut work)?;
            let (front, back) = divide(parts, &s, brushes, &mut work);
            if work > MAX_WORK {
                return Err(TooMuch::Work(work));
            }
            let at = bsp.nodes.len();
            bsp.nodes.push(Node {
                plane: s,
                front: Child::Empty,
                back: Child::Empty,
            });
            tasks.push((back, Slot::Back(at)));
            tasks.push((front, Slot::Front(at)));
            Child::Node(at as u32)
        };
        match slot {
            Slot::Root => {
                if !matches!(child, Child::Node(_)) {
                    bsp.root = Some(child);
                }
            }
            Slot::Front(n) => bsp.nodes[n].front = child,
            Slot::Back(n) => bsp.nodes[n].back = child,
        }
    }
    Ok(bsp)
}

fn splitter(parts: &[Part], work: &mut u64) -> Result<Plane, TooMuch> {
    let mut candidates: Vec<Plane> = Vec::new();
    'outer: for p in parts {
        for q in &p.left {
            if candidates.len() >= MAX_CANDIDATES {
                break 'outer;
            }
            if !candidates.iter().any(|c| c.coplanar(q)) {
                candidates.push(*q);
            }
        }
    }
    *work = work.saturating_add((candidates.len() * parts.len()) as u64);
    if *work > MAX_WORK {
        return Err(TooMuch::Work(*work));
    }
    let mut best = (u64::MAX, candidates[0]);
    for c in candidates {
        let (mut cut, mut front, mut back) = (0u64, 0i64, 0i64);
        for p in parts {
            if p.left.iter().any(|q| q.same(&c)) {
                back += 1;
                continue;
            }
            if p.left.iter().any(|q| q.same(&c.flipped())) {
                front += 1;
                continue;
            }
            match side_of(p, &c) {
                0 => {
                    cut += 1;
                    front += 1;
                    back += 1;
                }
                1 => front += 1,
                -1 => back += 1,
                _ => {}
            }
        }
        let score = cut * 8 + (front - back).unsigned_abs();
        if score < best.0 {
            best = (score, c);
        }
    }
    Ok(best.1)
}

/// The parts in front of and behind `s`; each part cut in two costs `work` its planes squared (its corners made again).
fn divide(
    parts: Vec<Part>,
    s: &Plane,
    brushes: &[Brush],
    work: &mut u64,
) -> (Vec<Part>, Vec<Part>) {
    let (mut front, mut back) = (Vec::new(), Vec::new());
    let flipped = s.flipped();
    for mut p in parts {
        let own = p.left.iter().any(|q| q.same(s));
        let opposite = p.left.iter().any(|q| q.same(&flipped));
        if own || opposite {
            p.left.retain(|q| !q.coplanar(s));
            if own {
                back.push(p);
            } else {
                front.push(p);
            }
            continue;
        }
        match side_of(&p, s) {
            1 => front.push(p),
            -1 => back.push(p),
            0 => {
                let planes = (brushes[p.brush].faces.len() + p.clip.len() + 1) as u64;
                *work = work.saturating_add(2 * planes * planes);
                for (half, out) in [(flipped, &mut front), (*s, &mut back)] {
                    let mut clip = p.clip.clone();
                    clip.push(half);
                    if let Some(c) = corners(&brushes[p.brush], &clip) {
                        out.push(Part {
                            brush: p.brush,
                            left: p.left.clone(),
                            clip,
                            corners: c,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    (front, back)
}

impl Bsp {
    /// The leaf `p` falls in (in front where a node's plane gives 0 or more, as `Interior::getZoneForPoint` walks).
    pub fn leaf(&self, p: V3) -> Child {
        if let Some(r) = self.root {
            return r;
        }
        let mut n = 0usize;
        loop {
            let node = &self.nodes[n];
            let next = if node.plane.dist(p) >= 0.0 {
                node.front
            } else {
                node.back
            };
            match next {
                Child::Node(i) => n = i as usize,
                leaf => return leaf,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::TexGen;

    fn cube(min: V3, max: V3) -> Brush {
        Brush::cuboid(min, max, "m", |n| TexGen::world(n, 1.0))
    }

    #[test]
    fn points_fall_in_solid_leaves_inside_brushes_only() {
        let brushes = vec![
            cube([0.0; 3], [2.0, 2.0, 1.0]),
            cube([1.0, 1.0, 0.5], [3.0, 3.0, 2.0]),
            cube([-4.0, 0.0, 0.0], [-3.0, 1.0, 1.0]),
        ];
        let bsp = build(&brushes, 1000).unwrap();
        let mut seed = 7u64;
        let mut next = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) * 8.0 - 4.0
        };
        for _ in 0..20_000 {
            let p = [next(), next(), next() * 0.5 + 1.0];
            let near = brushes
                .iter()
                .flat_map(|b| &b.faces)
                .any(|f| f.plane.dist(p).abs() < 1e-3);
            if near {
                continue;
            }
            let inside = brushes.iter().any(|b| b.contains(p, 0.0));
            assert_eq!(matches!(bsp.leaf(p), Child::Solid(_)), inside, "{p:?}");
        }
    }

    #[test]
    fn no_brush_is_one_empty_leaf() {
        let bsp = build(&[], 10).unwrap();
        assert_eq!(bsp.root, Some(Child::Empty));
        assert_eq!(bsp.leaf([0.0; 3]), Child::Empty);
    }
}
