//! A cell's clusters: the units the GPU culls and draws. Each batch's quads are ordered by the [`CLUSTER_SIZE`]-unit sub-box of the
//! cell their middle lies in (a stable sort: within a sub-box the quads keep their order), and a cluster is a run of one batch's
//! quads in one sub-box, at most [`MAX_CLUSTER_QUADS`] of them, with the box of its vertices. A cell has at most
//! [`MAX_CELL_CLUSTERS`] (or one a batch, when it has more batches than that): past it the runs are cut by count alone, longer.

use crate::hd::HdVertex;
use crate::hd::cells::{Batch, CELL_SIZE};

/// A sub-box's side (units): four a cell's side.
pub const CLUSTER_SIZE: f32 = 8.0;
/// The most quads of one cluster.
pub const MAX_CLUSTER_QUADS: u32 = 1024;
/// The most clusters of one cell (but one a batch).
pub const MAX_CELL_CLUSTERS: usize = 1024;

/// The sub-boxes along a cell's side.
const SUBS: i32 = (CELL_SIZE / CLUSTER_SIZE) as i32;

/// A run of one batch's quads: indices `first .. first + count` of the cell's index list (six a quad), its slot and translucency
/// (the batch's), and the box of its vertices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cluster {
    pub slot: u32,
    pub translucent: bool,
    pub first: u32,
    pub count: u32,
    pub bounds: [[f32; 3]; 2],
}

/// The sub-box of the cell at `origin` (its low corner) that a quad's middle lies in (one beyond the cell is put in the nearest).
pub fn sub_box(q: &[HdVertex; 4], origin: [f32; 3]) -> u32 {
    sub_box_sized(q, origin, CLUSTER_SIZE)
}

/// [`sub_box`] for a box of four sub-boxes of `size` a side.
fn sub_box_sized(q: &[HdVertex; 4], origin: [f32; 3], size: f32) -> u32 {
    let mut at = 0;
    for k in (0..3).rev() {
        let mid = 0.25 * q.iter().map(|v| v.position[k]).sum::<f32>();
        let s = ((mid - origin[k]) / size).floor();
        let s = if s.is_finite() {
            (s as i32).clamp(0, SUBS - 1)
        } else {
            0
        };
        at = at * SUBS + s;
    }
    at as u32
}

/// A batch's quads ordered by sub-box, each sub-box's in the order they came.
pub fn order(quads: &mut [[HdVertex; 4]], origin: [f32; 3]) {
    // A stable counting sort over the SUBS^3 sub-boxes (a big cell's comparison sort was milliseconds of a small change).
    const BOXES: usize = (SUBS * SUBS * SUBS) as usize;
    let keys: Vec<u32> = quads.iter().map(|q| sub_box(q, origin)).collect();
    if keys.windows(2).all(|w| w[0] <= w[1]) {
        return;
    }
    let mut starts = [0usize; BOXES + 1];
    for &k in &keys {
        starts[k as usize + 1] += 1;
    }
    for i in 0..BOXES {
        starts[i + 1] += starts[i];
    }
    let mut sorted = Vec::with_capacity(quads.len());
    sorted.resize(quads.len(), quads[0]);
    for (q, &k) in quads.iter().zip(&keys) {
        let at = &mut starts[k as usize];
        sorted[*at] = *q;
        *at += 1;
    }
    quads.copy_from_slice(&sorted);
}

/// The sub-box (of four a side) of the block `side` units a side at `origin` that a quad's middle lies in: [`of_block`]'s runs
/// follow it, so a batch's quads are ordered by it first.
pub fn block_sub_box(q: &[HdVertex; 4], origin: [f32; 3], side: f32) -> u32 {
    sub_box_sized(q, origin, side / SUBS as f32)
}

/// The low corner of the cell with grid key `key`.
pub fn cell_origin(key: [i32; 3]) -> [f32; 3] {
    key.map(|k| k as f32 * CELL_SIZE)
}

/// The clusters of a cell's quads (four vertices a quad, laid out in index order) and batches, each batch cut where the sub-box
/// changes (`by_box`) and every `limit` quads.
fn split(
    vertices: &[HdVertex],
    batches: &[Batch],
    origin: [f32; 3],
    size: f32,
    by_box: bool,
    limit: u32,
) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = Vec::new();
    for b in batches {
        let (q0, q1) = (b.first / 6, (b.first + b.count) / 6);
        let mut open: Option<(u32, Cluster)> = None;
        for q in q0..q1 {
            let Some(quad) = vertices.get(q as usize * 4..q as usize * 4 + 4) else {
                break;
            };
            let quad: &[HdVertex; 4] = quad.try_into().expect("four vertices");
            let key = if by_box {
                sub_box_sized(quad, origin, size)
            } else {
                0
            };
            let starts = match &open {
                Some((k, c)) => *k != key || c.count / 6 >= limit,
                None => true,
            };
            if starts && let Some((_, c)) = open.take() {
                out.push(c);
            }
            let (_, c) = open.get_or_insert((
                key,
                Cluster {
                    slot: b.slot,
                    translucent: b.translucent,
                    first: q * 6,
                    count: 0,
                    bounds: [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]],
                },
            ));
            c.count += 6;
            for v in quad {
                for k in 0..3 {
                    c.bounds[0][k] = c.bounds[0][k].min(v.position[k]);
                    c.bounds[1][k] = c.bounds[1][k].max(v.position[k]);
                }
            }
        }
        if let Some((_, c)) = open {
            out.push(c);
        }
    }
    out
}

/// A cell's clusters ([`MAX_CELL_CLUSTERS`] at most, or one a batch).
pub fn of_cell(vertices: &[HdVertex], batches: &[Batch], key: [i32; 3]) -> Vec<Cluster> {
    of_block(vertices, batches, cell_origin(key), CELL_SIZE)
}

/// The clusters of a block `side` units a side with its low corner at `origin` (a cell, or a coarser level's block: four
/// sub-boxes a side either way), [`MAX_CELL_CLUSTERS`] at most (or one a batch).
pub fn of_block(
    vertices: &[HdVertex],
    batches: &[Batch],
    origin: [f32; 3],
    side: f32,
) -> Vec<Cluster> {
    let size = side / SUBS as f32;
    let clusters = split(vertices, batches, origin, size, true, MAX_CLUSTER_QUADS);
    if clusters.len() <= MAX_CELL_CLUSTERS {
        return clusters;
    }
    // Cut by count alone, long enough that the runs fit: each batch makes at most quads / limit + 1.
    let quads = (vertices.len() / 4) as u64;
    let room = MAX_CELL_CLUSTERS.saturating_sub(batches.len()).max(1) as u64;
    let limit = (quads.div_ceil(room) as u32).max(MAX_CLUSTER_QUADS);
    split(vertices, batches, origin, size, false, limit)
}

#[cfg(test)]
#[path = "clusters_tests.rs"]
mod tests;
