//! The ghost table: `mGhostArray` with its three regions, kept sparse.
//!
//! Torque keeps one array of `Max` GhostInfo pointers: positions `[0, zero)` are ghosts with a non-zero update mask, `[zero, free)`
//! ghosts in use with mask 0, `[free, max)` the unused structs. The **wire id of a ghost is its struct's number** (`GhostInfo.index`,
//! fixed when the array is built), not its position; the position only decides the send order and which struct a new ghost
//! takes. Allocation takes the struct at `array[free]`, a freed ghost is swapped to `array[free - 1]` and `free` drops, so the
//! most recently freed id is reused first (LIFO). The swaps are the array's, step for step, because the priority-less order of the
//! send loop and the ties of the priority sort follow the array order.
//!
//! This table stores only `used` (positions `[0, free)`); the free region is a stack of freed ids (`returned`, top = `array[free]`)
//! in front of two lazy id ranges (the layout `activateGhosting` builds: the ghost-always ids `Max - n .. Max`, then `0 .. Max - n`),
//! so memory and time are O(ghosts in use). `dead` (the ids in `[0, zero)` with `KillGhost && NotYetGhosted`) is kept
//! incrementally for the dead-entry pass, and is empty but for a ghost whose object went before its first record was sent.
//!
//! Credit: where Torque3D has the same rules, this module follows its MIT-licensed `sim/netGhost.cpp` (Copyright (c) 2012
//! GarageGames, LLC); the code here is this crate's own.

use std::collections::{BTreeSet, VecDeque};

use super::source::{ObjId, flags};

/// One GhostInfo.
#[derive(Debug, Clone)]
pub(super) struct Ghost {
    /// The struct number: the wire id.
    pub index: u32,
    /// Unique per allocation, so a notify for a freed ghost never hits the next owner of its id.
    pub generation: u64,
    pub obj: Option<ObjId>,
    pub mask: u32,
    pub flags: u16,
    pub skip: u8,
    /// Array position.
    pub pos: usize,
    /// Refs in flight for this ghost, oldest first: `(send_seq, mask the record cleared)`.
    pub chain: VecDeque<(u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Table {
    max: u32,
    used: Vec<u32>,
    zero: usize,
    ghosts: crate::idhash::IdMap<u32, Ghost>,
    by_obj: crate::idhash::IdMap<ObjId, u32>,
    returned: Vec<u32>,
    lazy: [(u32, u32); 2],
    dead: BTreeSet<u32>,
    /// The largest id in `[0, zero)` while `max_known`; it is the one thing about that region a packet asks, and it only changes
    /// when a ghost enters it above the largest (then it is that ghost's) or the largest leaves it (then it is found again when asked).
    max_nonzero: Option<u32>,
    max_known: bool,
    next_generation: u64,
    /// Counts the times an object was let go of by a ghost (or every ghost was): what a scope query that found every object a
    /// ghost has can rely on staying so while it is unchanged.
    links_let_go: u64,
}

impl Table {
    /// The table `activateGhosting` builds: `max` structs, the first `always` positions holding ids `max - always ..`.
    pub fn init(&mut self, max: u32, always: u32) {
        self.clear();
        self.max = max;
        let always = always.min(max);
        self.lazy = [(max - always, max), (0, max - always)];
    }

    /// Drops every ghost (`resetGhosting` frees them all; the array is rebuilt by the next activate).
    pub fn clear(&mut self) {
        self.used.clear();
        self.zero = 0;
        self.ghosts.clear();
        self.by_obj.clear();
        self.returned.clear();
        self.lazy = [(0, 0), (0, 0)];
        self.dead.clear();
        self.max_nonzero = None;
        self.max_known = true;
        self.links_let_go += 1;
    }

    /// How many times an object lost its ghost's link ([`Table::unlink_object`], a freed linked ghost, a clear): a scope query
    /// that left every ghosted object linked and ghosted every object it could stays complete until this changes.
    pub fn links_let_go(&self) -> u64 {
        self.links_let_go
    }

    pub fn len(&self) -> usize {
        self.used.len()
    }

    /// `ZeroUpdateIndex`.
    pub fn zero(&self) -> usize {
        self.zero
    }

    pub fn get(&self, id: u32) -> Option<&Ghost> {
        self.ghosts.get(&id)
    }

    /// A full scope query's start for every ghost with something to send (`[0, zero)`, in position order): its skip count up by one
    /// (at most 255) and, unless it is always in scope, its in-scope flag cleared. One pass with one look-up a ghost (the flag
    /// alone never changes whether a ghost is dead, so the dead set is not touched).
    pub fn start_query(&mut self) {
        let zero = self.zero.min(self.used.len());
        for &id in &self.used[..zero] {
            if let Some(g) = self.ghosts.get_mut(&id) {
                g.skip = g.skip.saturating_add(1);
                if g.flags & (flags::SCOPE_ALWAYS | flags::SCOPE_LOCAL_ALWAYS) == 0 {
                    g.flags &= !flags::IN_SCOPE;
                }
            }
        }
    }

    /// The ghost of `obj` seen by a scope query (what [`Table::start_query`] cleared): its in-scope flag set again.
    pub fn mark_in_scope(&mut self, obj: ObjId) {
        if let Some(id) = self.by_obj.get(&obj)
            && let Some(g) = self.ghosts.get_mut(id)
        {
            g.flags |= flags::IN_SCOPE;
        }
    }

    /// The ghosts of `[0, zero)` the last scope query did not find (`InScope` clear, never always in scope) that still have an
    /// object, last position first.
    pub fn out_of_scope_desc(&self) -> Vec<u32> {
        let zero = self.zero.min(self.used.len());
        let gone = flags::IN_SCOPE | flags::SCOPE_ALWAYS | flags::SCOPE_LOCAL_ALWAYS;
        self.used[..zero]
            .iter()
            .rev()
            .copied()
            .filter(|id| {
                self.ghosts
                    .get(id)
                    .is_some_and(|g| g.obj.is_some() && g.flags & gone == 0)
            })
            .collect()
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Ghost> {
        self.ghosts.get_mut(&id)
    }

    pub fn id_of(&self, obj: ObjId) -> Option<u32> {
        self.by_obj.get(&obj).copied()
    }

    /// The id at an array position.
    pub fn at(&self, pos: usize) -> Option<u32> {
        self.used.get(pos).copied()
    }

    /// The largest id among the ghosts in `[0, zero)` (the loop reads it before it frees any dead entry).
    pub fn max_nonzero_id(&mut self) -> Option<u32> {
        if !self.max_known {
            self.max_nonzero = self.nonzero_ids_by_position().iter().copied().max();
            self.max_known = true;
        }
        self.max_nonzero
    }

    /// Ghosts in `[0, zero)` that are `KillGhost && NotYetGhosted`, in descending array position (the order the loop frees them).
    pub fn dead_by_position_desc(&self) -> Vec<u32> {
        if self.dead.is_empty() {
            return Vec::new();
        }
        let mut v: Vec<(usize, u32)> = self
            .dead
            .iter()
            .filter_map(|&id| self.ghosts.get(&id).map(|g| (g.pos, id)))
            .collect();
        v.sort_unstable_by_key(|e| std::cmp::Reverse(e.0));
        v.into_iter().map(|(_, id)| id).collect()
    }

    pub fn ids_in_use(&self) -> impl Iterator<Item = u32> + '_ {
        self.used.iter().copied()
    }

    pub fn nonzero_ids_by_position(&self) -> &[u32] {
        self.used.get(..self.zero).unwrap_or(&[])
    }

    /// Whether the ghost `id` is dead (see [`Table::dead_by_position_desc`]) and so belongs in the dead set. Only the ghost that
    /// crosses `zero` (or changes its flags) can change: the ghost a swap displaces stays on its side of it.
    fn refresh(&mut self, id: u32) {
        let dead = self.ghosts.get(&id).is_some_and(|g| {
            g.pos < self.zero
                && g.flags & (flags::KILL_GHOST | flags::NOT_YET_GHOSTED)
                    == (flags::KILL_GHOST | flags::NOT_YET_GHOSTED)
        });
        if dead {
            self.dead.insert(id);
        } else if !self.dead.is_empty() {
            self.dead.remove(&id);
        }
    }

    fn swap_positions(&mut self, a: usize, b: usize) {
        if a == b {
            return;
        }
        self.used.swap(a, b);
        let (ia, ib) = (self.used[a], self.used[b]);
        if let Some(g) = self.ghosts.get_mut(&ia) {
            g.pos = a;
        }
        if let Some(g) = self.ghosts.get_mut(&ib) {
            g.pos = b;
        }
    }

    /// Sets the flags and keeps the dead set right.
    pub fn set_flags(&mut self, id: u32, new: u16) {
        // The flags alone change only whether the ghost is dead (its place, and so the non-zero set, stays): the dead set is
        // touched only when that changes (a scope query sets the flags of every ghost in scope).
        let zero = self.zero;
        let dead_of = |g: &Ghost| {
            g.pos < zero
                && g.flags & (flags::KILL_GHOST | flags::NOT_YET_GHOSTED)
                    == (flags::KILL_GHOST | flags::NOT_YET_GHOSTED)
        };
        let Some(g) = self.ghosts.get_mut(&id) else {
            self.refresh(id);
            return;
        };
        let was = dead_of(g);
        g.flags = new;
        let now = dead_of(g);
        if was != now {
            if now {
                self.dead.insert(id);
            } else {
                self.dead.remove(&id);
            }
        }
    }

    /// `pushNonZero`: swap with `array[zero]`, `zero++`. The ghost must be in `[zero, free)`.
    pub fn push_nonzero(&mut self, id: u32) {
        let Some(pos) = self.ghosts.get(&id).map(|g| g.pos) else {
            return;
        };
        if pos < self.zero {
            return;
        }
        self.swap_positions(pos, self.zero);
        self.zero += 1;
        if self.max_known {
            self.max_nonzero = self.max_nonzero.max(Some(id));
        }
        self.refresh(id);
    }

    /// `pushToZero`: `zero--`, swap with `array[zero]`. The ghost must be in `[0, zero)`.
    pub fn push_to_zero(&mut self, id: u32) {
        let Some(pos) = self.ghosts.get(&id).map(|g| g.pos) else {
            return;
        };
        if pos >= self.zero {
            return;
        }
        self.zero -= 1;
        self.swap_positions(pos, self.zero);
        if self.max_nonzero == Some(id) {
            self.max_known = false;
        }
        self.refresh(id);
    }

    fn take_free_id(&mut self) -> Option<u32> {
        if let Some(id) = self.returned.pop() {
            return Some(id);
        }
        for r in &mut self.lazy {
            if r.0 < r.1 {
                let id = r.0;
                r.0 += 1;
                return Some(id);
            }
        }
        None
    }

    /// `objectInScope`'s new-ghost branch: takes the free region's front, mask `0xFFFFFFFF`, then `pushNonZero`. `None` when full.
    pub fn alloc(&mut self, obj: ObjId, ghost_flags: u16) -> Option<u32> {
        let id = self.take_free_id()?;
        self.next_generation = self.next_generation.wrapping_add(1);
        let pos = self.used.len();
        self.used.push(id);
        self.ghosts.insert(
            id,
            Ghost {
                index: id,
                generation: self.next_generation,
                obj: Some(obj),
                mask: 0xFFFF_FFFF,
                flags: ghost_flags,
                skip: 0,
                pos,
                chain: VecDeque::new(),
            },
        );
        self.by_obj.insert(obj, id);
        self.push_nonzero(id);
        Some(id)
    }

    /// The objects of the ghosts `pick` names still linked to them, in array order.
    pub fn linked_objects(&self, pick: impl Fn(u32) -> bool) -> Vec<ObjId> {
        self.used
            .iter()
            .filter(|&&id| pick(id))
            .filter_map(|id| self.ghosts.get(id))
            .filter_map(|g| g.obj)
            .collect()
    }

    /// Unlinks the object from the lookup (the ghost stays; `detachObject`).
    pub fn unlink_object(&mut self, id: u32) {
        let obj = self.ghosts.get_mut(&id).and_then(|g| g.obj.take());
        if let Some(o) = obj {
            self.links_let_go += 1;
            if self.by_obj.get(&o) == Some(&id) {
                self.by_obj.remove(&o);
            }
        }
    }

    /// `free`: a ghost in `[0, zero)` goes to the zero region first (mask 0), then swaps with `array[free - 1]` and the
    /// struct joins the front of the free region.
    pub fn free(&mut self, id: u32) {
        let Some(g) = self.ghosts.get(&id) else {
            return;
        };
        if g.pos < self.zero {
            if let Some(g) = self.ghosts.get_mut(&id) {
                g.mask = 0;
            }
            self.push_to_zero(id);
        }
        let Some(pos) = self.ghosts.get(&id).map(|g| g.pos) else {
            return;
        };
        let last = self.used.len() - 1;
        self.swap_positions(pos, last);
        self.used.pop();
        if let Some(o) = self.ghosts.remove(&id).and_then(|g| g.obj) {
            self.links_let_go += 1;
            if self.by_obj.get(&o) == Some(&id) {
                self.by_obj.remove(&o);
            }
        }
        self.dead.remove(&id);
        self.returned.push(id);
    }

    /// Swaps two array positions (the scope object's ghost to the end of `[0, zero)`, the sort's rewrite).
    pub fn swap_array_positions(&mut self, a: usize, b: usize) {
        if a < self.used.len() && b < self.used.len() {
            self.swap_positions(a, b);
        }
    }

    /// Rewrites `array[0..ids.len()]` as `ids` (the priority sort's result; `ids` is a permutation of the current prefix).
    pub fn set_prefix_order(&mut self, ids: &[u32]) {
        if ids.len() > self.used.len() {
            return;
        }
        for (pos, &id) in ids.iter().enumerate() {
            // A ghost already in its place is left as it is (an order that did not change costs no look-up).
            if self.used[pos] == id {
                continue;
            }
            self.used[pos] = id;
            if let Some(g) = self.ghosts.get_mut(&id) {
                g.pos = pos;
            }
        }
    }

    /// Checks the invariants (tests): regions, unique ids, ids inside their regions, the free stack against the used set.
    #[cfg(test)]
    pub fn check_invariants(&self) -> Result<(), String> {
        if self.zero > self.used.len() || self.used.len() > self.max as usize {
            return Err(format!(
                "zero {} used {} max {}",
                self.zero,
                self.used.len(),
                self.max
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for (pos, &id) in self.used.iter().enumerate() {
            if id >= self.max {
                return Err(format!("id {id} escapes 0..{}", self.max));
            }
            if !seen.insert(id) {
                return Err(format!("id {id} twice"));
            }
            let g = self.ghosts.get(&id).ok_or("used id without a ghost")?;
            if g.pos != pos || g.index != id {
                return Err(format!("ghost {id} pos {} != {pos}", g.pos));
            }
            let nz = pos < self.zero;
            let d = nz && g.flags & 0x28 == 0x28;
            if d != self.dead.contains(&id) {
                return Err(format!("dead set wrong for {id}"));
            }
            // A zero mask in [0, zero) is what clearMaskBits leaves (see GhostManager::clear_mask); the reverse is never allowed.
            if !nz && g.mask != 0 {
                return Err(format!(
                    "ghost {id} in the zero region with mask {:#x}",
                    g.mask
                ));
            }
            if let Some(o) = g.obj
                && self.by_obj.get(&o) != Some(&id)
            {
                return Err(format!("lookup of {o} wrong"));
            }
        }
        if self.max_known && self.max_nonzero != self.used[..self.zero].iter().copied().max() {
            return Err("the cached largest id is wrong".into());
        }
        if self.ghosts.len() != self.used.len() {
            return Err("ghost map and array differ".into());
        }
        for &id in &self.returned {
            if id >= self.max || seen.contains(&id) {
                return Err(format!("free id {id} bad"));
            }
        }
        let free_total = self.returned.len() as u64
            + self
                .lazy
                .iter()
                .map(|&(a, b)| u64::from(b.saturating_sub(a)))
                .sum::<u64>();
        if free_total + self.used.len() as u64 != u64::from(self.max) {
            return Err(format!(
                "free {free_total} + used {} != max {}",
                self.used.len(),
                self.max
            ));
        }
        Ok(())
    }
}
