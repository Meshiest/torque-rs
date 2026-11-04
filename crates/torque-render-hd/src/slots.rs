//! Where the moving shapes' vertices stay in the loose vertex buffer between uploads.
//!
//! A window uploads its moving shapes every frame, most of them as they were (a bot standing still, an item on the ground): the
//! scene stamps each control sync's object ([`torque_render_scene::hd::DynamicObject::stamp`]), the same stamp meaning the same draws.
//! The buffer's first `capacity` vertices hold the stamped objects, each in a run of its own that it keeps while its vertex count
//! does; an object whose run already holds its stamp's vertices is not written again, nor its previous positions while they are
//! its own (it did not move). The rest of the buffer (the sky, the planes, the water, the shapes without a stamp, the coronas)
//! follows the runs and is written every upload.

use std::collections::HashMap;

/// One stamped object's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// First vertex and count.
    pub start: usize,
    pub len: usize,
    /// The stamp whose vertices the run holds (None: not written since the buffer was made).
    pub written: Option<u64>,
    /// The stamp whose vertices the previous positions' run holds as they are (a still object's).
    pub previous: Option<u64>,
    /// The upload that last asked for it.
    seen: u64,
}

/// The runs (by ghost), the vertices they take, and the room kept for them.
#[derive(Debug, Default)]
pub struct Runs {
    runs: HashMap<u32, Run>,
    end: usize,
    pub capacity: usize,
    upload: u64,
}

/// Uploads with stamped objects a run is kept through while its ghost has none.
const KEEP_UPLOADS: u64 = 16;

impl Runs {
    /// The runs for `objects` (ghost and vertex count, in order) this upload: each keeps its run while its count is the same; a new
    /// one goes after the last, and when they do not fit every run is laid out again from the start with room to grow. Returns the
    /// runs in order and whether they were laid out again (the buffer's later part moved).
    pub fn place(&mut self, objects: &[(u32, usize)]) -> (Vec<Run>, bool) {
        if objects.is_empty() {
            return (Vec::new(), false);
        }
        self.upload += 1;
        let upload = self.upload;
        self.runs.retain(|_, r| r.seen + KEEP_UPLOADS >= upload);
        let mut out = Vec::with_capacity(objects.len());
        let mut fits = true;
        for &(ghost, len) in objects {
            let run = match self.runs.get(&ghost) {
                Some(r) if r.len == len => *r,
                _ => {
                    if self.end + len > self.capacity {
                        fits = false;
                        break;
                    }
                    let r = Run {
                        start: self.end,
                        len,
                        written: None,
                        previous: None,
                        seen: upload,
                    };
                    self.end += len;
                    r
                }
            };
            let run = Run {
                seen: upload,
                ..run
            };
            self.runs.insert(ghost, run);
            out.push(run);
        }
        if fits {
            return (out, false);
        }
        let total: usize = objects.iter().map(|o| o.1).sum();
        self.runs.clear();
        self.capacity = total + total / 2 + 1024;
        self.end = 0;
        out.clear();
        for &(ghost, len) in objects {
            let r = Run {
                start: self.end,
                len,
                written: None,
                previous: None,
                seen: upload,
            };
            self.end += len;
            self.runs.insert(ghost, r);
            out.push(r);
        }
        (out, true)
    }

    /// Records what `ghost`'s run now holds.
    pub fn set(&mut self, ghost: u32, written: Option<u64>, previous: Option<u64>) {
        if let Some(r) = self.runs.get_mut(&ghost) {
            r.written = written;
            r.previous = previous;
        }
    }

    /// The buffer was made again: no run holds anything.
    pub fn forget_vertices(&mut self) {
        for r in self.runs.values_mut() {
            r.written = None;
            r.previous = None;
        }
    }

    /// The previous positions' buffer was made again (or let go).
    pub fn forget_previous(&mut self) {
        for r in self.runs.values_mut() {
            r.previous = None;
        }
    }
}

/// The most held vertices a span of writes takes along between two runs to write (written again as they are).
pub const BRIDGE: usize = 4096;

/// The runs of `order` (sorted by start) that `need` says to write, in spans: each span's runs lie side by side, any held runs
/// between two to write within [`BRIDGE`] vertices taken along.
pub fn spans(order: &[usize], run: impl Fn(usize) -> Run, need: &[bool]) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut pending: Vec<usize> = Vec::new();
    let mut end = 0;
    for &i in order {
        let r = run(i);
        if !cur.is_empty() && r.start != end {
            out.push(std::mem::take(&mut cur));
            pending.clear();
        }
        if need[i] {
            if !cur.is_empty() {
                let gap: usize = pending.iter().map(|&k| run(k).len).sum();
                if gap <= BRIDGE {
                    cur.append(&mut pending);
                } else {
                    out.push(std::mem::take(&mut cur));
                    pending.clear();
                }
            }
            cur.push(i);
            end = r.start + r.len;
        } else if !cur.is_empty() {
            pending.push(i);
            end = r.start + r.len;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
#[path = "slots_tests.rs"]
mod tests;
