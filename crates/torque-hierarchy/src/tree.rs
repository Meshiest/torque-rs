//! The tree: links, caps, the dirty roots and the parents-first resolve.

use std::collections::{BTreeMap, BTreeSet};
use std::mem;

use torque_collide::math::{IDENTITY, Matrix};

use crate::caps::{Caps, HARD_DEPTH, ParentError};
use crate::compose;
use crate::quant;
use crate::types::{Host, Keep, Local, NodeRef, ObjId, ParentRef, Pose, matrix_check};

/// An index into the node slab.
pub(crate) type Slot = usize;

/// How many entries past twice the held objects the moved list may reach before it is compacted.
const MOVED_SLACK: usize = 64;

/// A node's parent: its slot and where on it the node hangs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Up {
    pub(crate) slot: Slot,
    pub(crate) node: NodeRef,
}

/// An object that has a parent or children. A root keeps only its scale in `local`; its world is what the host last set.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub(crate) id: ObjId,
    pub(crate) up: Option<Up>,
    pub(crate) children: Vec<Slot>,
    pub(crate) local: Local,
    pub(crate) depth: u8,
    /// Links from this node down to its deepest descendant (0 for a leaf), kept on every attach and detach.
    pub(crate) height: u8,
    /// Descendants, not counting the node itself.
    pub(crate) subtree: usize,
    pub(crate) world: Matrix,
    /// On the dirty queue: this node and everything below it must be resolved.
    pub(crate) queued: bool,
    /// On the moved list.
    pub(crate) moved: bool,
}

impl Node {
    fn pose(&self) -> Pose {
        Pose {
            world: self.world,
            scale: self.local.scale,
        }
    }
}

/// What a [`Hierarchy::flush`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flush {
    /// Nodes whose world was recomputed.
    pub resolved: usize,
    /// Dirty roots still waiting because the budget ran out.
    pub carried: usize,
    /// Children hung on a node the host no longer has (or whose matrix is not usable); they were placed at the parent's
    /// origin.
    pub node_misses: usize,
    /// Children whose composed world overflowed an `f32` (a chain of wild matrices); they were placed at the parent's
    /// attachment.
    pub overflowed: usize,
}

/// What a link change keeps or sets (the private form of [`Keep`] plus a given pose).
#[derive(Debug, Clone, Copy)]
enum Place {
    Keep(Keep),
    Given(Local),
}

/// A link worked out and checked, ready to be applied without a way to fail.
struct Planned {
    parent_slot: Option<Slot>,
    parent_pose: Pose,
    local: Local,
    world: Matrix,
}

const LIVE: &str = "a slot in the index or a child list holds a node";

/// A forest of objects with a local pose each. See the crate docs for the data flow and what the host must do.
#[derive(Debug, Default)]
pub struct Hierarchy {
    pub(crate) caps: Caps,
    pub(crate) slots: Vec<Option<Node>>,
    pub(crate) free: Vec<Slot>,
    pub(crate) index: BTreeMap<ObjId, Slot>,
    pub(crate) queue: Vec<Slot>,
    moved: Vec<ObjId>,
    delete_with_parent: BTreeSet<ObjId>,
    pub(crate) parented: usize,
    refused: u64,
    quantize: bool,
    /// Scratch for walks down a subtree; kept between calls so they do not allocate.
    stack: Vec<Slot>,
    /// Scratch for the path down to the node a delete is removing.
    path: Vec<Slot>,
}

impl Hierarchy {
    /// An empty hierarchy under `caps` (clamped to the hard caps).
    #[must_use]
    pub fn new(caps: Caps) -> Hierarchy {
        Hierarchy {
            caps: caps.clamped(),
            ..Hierarchy::default()
        }
    }

    /// Round every local pose of a child it stores to the wire form ([`quant::quantize`]), so a server's world equals what a
    /// client composes from the sent pose. A keep-world reparent then lands within the quantizer's step rather than float
    /// error.
    #[must_use]
    pub fn quantize_locals(mut self, on: bool) -> Hierarchy {
        self.quantize = on;
        self
    }

    /// The caps in force.
    #[must_use]
    pub fn caps(&self) -> Caps {
        self.caps
    }

    // ---- queries ----

    /// Objects the hierarchy holds (those with a parent or children).
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Objects that have a parent.
    #[must_use]
    pub fn parented_count(&self) -> usize {
        self.parented
    }

    /// Links and poses refused since the start.
    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Whether `id` has a parent or children.
    #[must_use]
    pub fn contains(&self, id: ObjId) -> bool {
        self.index.contains_key(&id)
    }

    /// `id`'s parent, `None` for a root or an object the hierarchy does not hold.
    #[must_use]
    pub fn parent(&self, id: ObjId) -> Option<ParentRef> {
        let up = self.node(self.slot_of(id)?).up?;
        Some(ParentRef {
            object: self.node(up.slot).id,
            node: up.node,
        })
    }

    /// `id`'s local pose, `None` unless it has a parent.
    #[must_use]
    pub fn local(&self, id: ObjId) -> Option<Local> {
        let node = self.node(self.slot_of(id)?);
        node.up.is_some().then_some(node.local)
    }

    /// How many children `id` has.
    #[must_use]
    pub fn child_count(&self, id: ObjId) -> usize {
        self.slot_of(id).map_or(0, |s| self.node(s).children.len())
    }

    /// `id`'s `i`th child in insertion order.
    #[must_use]
    pub fn child(&self, id: ObjId, i: usize) -> Option<ObjId> {
        let slot = *self.node(self.slot_of(id)?).children.get(i)?;
        Some(self.node(slot).id)
    }

    /// `id`'s children in insertion order.
    pub fn children(&self, id: ObjId) -> impl ExactSizeIterator<Item = ObjId> + '_ {
        let slots = self
            .slot_of(id)
            .map_or(&[][..], |s| self.node(s).children.as_slice());
        slots.iter().map(|s| self.node(*s).id)
    }

    /// The top of `id`'s chain: `id` itself when it has no parent.
    #[must_use]
    pub fn root(&self, id: ObjId) -> ObjId {
        self.slot_of(id)
            .map_or(id, |s| self.node(self.root_slot(s)).id)
    }

    /// Whether `ancestor` is above `id` (not `id` itself).
    #[must_use]
    pub fn is_ancestor(&self, ancestor: ObjId, id: ObjId) -> bool {
        let (Some(a), Some(s)) = (self.slot_of(ancestor), self.slot_of(id)) else {
            return false;
        };
        a != s && self.chain_includes(s, a)
    }

    /// Links from `id` up to its root (0 for a root or an unknown object).
    #[must_use]
    pub fn depth(&self, id: ObjId) -> usize {
        self.slot_of(id)
            .map_or(0, |s| usize::from(self.node(s).depth))
    }

    /// Descendants of `id`, all levels.
    #[must_use]
    pub fn descendant_count(&self, id: ObjId) -> usize {
        self.slot_of(id).map_or(0, |s| self.node(s).subtree)
    }

    /// `id`'s cached world transform. It is current once [`Hierarchy::dirty_count`] is 0.
    #[must_use]
    pub fn world(&self, id: ObjId) -> Option<Matrix> {
        Some(self.node(self.slot_of(id)?).world)
    }

    /// `id`'s cached world and scale.
    #[must_use]
    pub fn pose(&self, id: ObjId) -> Option<Pose> {
        Some(self.node(self.slot_of(id)?).pose())
    }

    /// Dirty roots waiting for a flush (stale entries of removed objects may be counted until the next flush).
    #[must_use]
    pub fn dirty_count(&self) -> usize {
        self.queue.len()
    }

    // ---- the validator ----

    /// Whether `child` may be hung on `parent` (`None` always may): the checks [`Hierarchy::set_parent`] makes first, with
    /// no change made, each a few steps along a chain (no walk down a subtree). A client runs it on every link a server
    /// sent.
    ///
    /// # Errors
    /// [`ParentError::SelfParent`], `Cycle`, `Depth`, `Children`, `Subtree` or `Total`.
    pub fn check_parent(&self, child: ObjId, parent: Option<ParentRef>) -> Result<(), ParentError> {
        let Some(parent) = parent else {
            return Ok(());
        };
        if parent.object == child {
            return Err(ParentError::SelfParent);
        }
        let (c, p) = (self.slot_of(child), self.slot_of(parent.object));
        self.check_cycle(c, p)?;
        self.check_depth(c, p)?;
        self.check_children(c, p)?;
        self.check_descendants(c, p)?;
        self.check_total(c)
    }

    /// The new parent must not be the child or below it.
    fn check_cycle(&self, child: Option<Slot>, parent: Option<Slot>) -> Result<(), ParentError> {
        match (child, parent) {
            (Some(c), Some(p)) if self.chain_includes(p, c) => Err(ParentError::Cycle),
            _ => Ok(()),
        }
    }

    /// The child's deepest descendant, hung below the parent, must stay within the depth cap.
    fn check_depth(&self, child: Option<Slot>, parent: Option<Slot>) -> Result<(), ParentError> {
        let height = child.map_or(0, |c| usize::from(self.node(c).height));
        let parent_depth = parent.map_or(0, |p| usize::from(self.node(p).depth));
        if parent_depth + 1 + height > self.caps.depth {
            return Err(ParentError::Depth);
        }
        Ok(())
    }

    /// The parent must have room for one more child (a child already under it needs none).
    fn check_children(&self, child: Option<Slot>, parent: Option<Slot>) -> Result<(), ParentError> {
        let Some(p) = parent else {
            return Ok(());
        };
        let already = child.is_some_and(|c| self.node(c).up.is_some_and(|u| u.slot == p));
        if !already && self.node(p).children.len() >= self.caps.children {
            return Err(ParentError::Children);
        }
        Ok(())
    }

    /// The root of the new parent must have room for the child and what hangs below it (moving within one tree adds nothing).
    fn check_descendants(
        &self,
        child: Option<Slot>,
        parent: Option<Slot>,
    ) -> Result<(), ParentError> {
        let root = parent.map(|p| self.root_slot(p));
        if matches!((child, root), (Some(c), Some(r)) if self.root_slot(c) == r) {
            return Ok(());
        }
        let size = child.map_or(1, |c| 1 + self.node(c).subtree);
        let have = root.map_or(0, |r| self.node(r).subtree);
        if have + size > self.caps.descendants {
            return Err(ParentError::Subtree);
        }
        Ok(())
    }

    /// A child that gains its first parent counts toward the total cap.
    fn check_total(&self, child: Option<Slot>) -> Result<(), ParentError> {
        let has_parent = child.is_some_and(|c| self.node(c).up.is_some());
        if !has_parent && self.parented >= self.caps.total {
            return Err(ParentError::Total);
        }
        Ok(())
    }

    // ---- changes ----

    /// Hang `child` on `parent`, or take it off its parent with `None`, keeping its world pose or its local pose. Returns
    /// the child's pose after the change (its new world pose when it keeps local). An object the hierarchy does not hold is
    /// taken in with the pose [`Host::pose`] gives, and one left with no parent and no children is let go.
    ///
    /// Resolves whatever is queued first, so it costs up to the nodes under the queued roots.
    ///
    /// # Errors
    /// A [`ParentError`] from [`Hierarchy::check_parent`], `BadNode` when the parent has no such node, `Unknown` when the host
    /// has no pose, `NotFinite` or `OutOfRange` for a pose or a result that is. Nothing changes then.
    pub fn set_parent<H: Host + ?Sized>(
        &mut self,
        child: ObjId,
        parent: Option<ParentRef>,
        keep: Keep,
        host: &H,
    ) -> Result<Pose, ParentError> {
        self.counted(|h| h.place(child, parent, Place::Keep(keep), host))
    }

    /// Hang `child` on `parent` with `local` as its pose in one step (a link read off the wire). Costs as
    /// [`Hierarchy::set_parent`] does.
    ///
    /// # Errors
    /// As [`Hierarchy::set_parent`], and [`Local::validate`]'s.
    pub fn link<H: Host + ?Sized>(
        &mut self,
        child: ObjId,
        parent: ParentRef,
        local: Local,
        host: &H,
    ) -> Result<Pose, ParentError> {
        self.counted(|h| h.place(child, Some(parent), Place::Given(local), host))
    }

    /// Set a child's local pose. O(1): it queues a dirty root.
    ///
    /// # Errors
    /// [`ParentError::NotParented`] unless `id` has a parent; [`Local::validate`]'s.
    pub fn set_local(&mut self, id: ObjId, local: Local) -> Result<(), ParentError> {
        self.counted(|h| {
            let slot = h
                .slot_of(id)
                .filter(|s| h.node(*s).up.is_some())
                .ok_or(ParentError::NotParented)?;
            let local = h.finish_local(local)?;
            h.node_mut(slot).local = local;
            h.mark_dirty_slot(slot);
            Ok(())
        })
    }

    /// An object moved in the world: a root takes `world` (O(1)), a child gets the local pose that puts it there (resolving
    /// whatever is queued first). Returns whether the hierarchy holds `id` (nothing happens to an object it does not).
    ///
    /// # Errors
    /// `NotFinite` or `OutOfRange` for a matrix that is, or when the local pose that results is.
    pub fn set_world<H: Host + ?Sized>(
        &mut self,
        id: ObjId,
        world: Matrix,
        host: &H,
    ) -> Result<bool, ParentError> {
        self.counted(|h| {
            matrix_check(&world)?;
            let Some(slot) = h.slot_of(id) else {
                return Ok(false);
            };
            let Some(up) = h.node(slot).up else {
                let node = h.node_mut(slot);
                node.world = world;
                node.world[12..].copy_from_slice(&IDENTITY[12..]);
                h.mark_dirty_slot(slot);
                return Ok(true);
            };
            h.settle(host);
            let (parent_pose, node) = h.attachment_of(up, host)?;
            let scale = h.node(slot).local.scale;
            let local = compose::local_from_world(&world, &parent_pose, node.as_ref(), scale);
            let local = h.finish_local(local)?;
            h.node_mut(slot).local = local;
            h.mark_dirty_slot(slot);
            Ok(true)
        })
    }

    /// An object's own scale changed (a root's, or a child's local scale). Its children's translations are rescaled. O(1). A
    /// child's scale is rounded to the wire form when the hierarchy quantizes locals.
    ///
    /// # Errors
    /// `NotFinite` or `OutOfRange`.
    pub fn set_scale(&mut self, id: ObjId, scale: [f32; 3]) -> Result<bool, ParentError> {
        self.counted(|h| {
            let Some(slot) = h.slot_of(id) else {
                return Ok(false);
            };
            let mut local = h.node(slot).local;
            local.scale = scale;
            local.validate()?;
            let on_wire = h.quantize && h.node(slot).up.is_some();
            h.node_mut(slot).local.scale = if on_wire {
                quant::quantize(&local).scale
            } else {
                scale
            };
            h.mark_dirty_slot(slot);
            Ok(true)
        })
    }

    /// Mark `id` and everything below it for the next flush (its node matrices changed, say). O(1). Does nothing for an
    /// object the hierarchy does not hold.
    pub fn mark_dirty(&mut self, id: ObjId) {
        if let Some(slot) = self.slot_of(id) {
            self.mark_dirty_slot(slot);
        }
    }

    /// Set whether `id` goes with its parent when the parent is deleted by [`Hierarchy::delete`]. Kept apart from the tree
    /// (an object can be flagged before it is parented) and forgotten when the object is deleted.
    ///
    /// # Errors
    /// [`ParentError::Total`] when as many objects are flagged as the total cap allows.
    pub fn set_delete_with_parent(&mut self, id: ObjId, flag: bool) -> Result<(), ParentError> {
        if !flag {
            self.delete_with_parent.remove(&id);
            return Ok(());
        }
        if !self.delete_with_parent.contains(&id)
            && self.delete_with_parent.len() >= self.caps.total
        {
            return Err(ParentError::Total);
        }
        self.delete_with_parent.insert(id);
        Ok(())
    }

    /// Whether `id` goes with its parent.
    #[must_use]
    pub fn delete_with_parent(&self, id: ObjId) -> bool {
        self.delete_with_parent.contains(&id)
    }

    /// `id` is being deleted. Its children that go with their parent are removed too (children first, recursively by the same
    /// rule) and appended to `out` for the host to delete, in that order; the rest keep their world pose as roots.
    /// `id` itself is not in `out`. Resolves whatever is queued first.
    ///
    /// The host calls this (or [`Hierarchy::delete_recursive`]) for an id before it reuses the id.
    pub fn delete<H: Host + ?Sized>(&mut self, id: ObjId, host: &H, out: &mut Vec<ObjId>) {
        self.delete_with_parent.remove(&id);
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        self.settle(host);
        if let Some(parent) = self.remove_node(slot, false, out) {
            self.release_if_idle(parent);
        }
    }

    /// `id` is being deleted along with every descendant (`deleteRecursive` in script): `out` gets them children first, `id`
    /// last, whatever their flags.
    pub fn delete_recursive(&mut self, id: ObjId, out: &mut Vec<ObjId>) {
        let Some(slot) = self.slot_of(id) else {
            self.delete_with_parent.remove(&id);
            out.push(id);
            return;
        };
        if let Some(parent) = self.remove_subtree(slot, out) {
            self.release_if_idle(parent);
        }
    }

    // ---- the resolve ----

    /// Recompute the world of every node below a dirty root, parents first, stopping before the next dirty root once
    /// `budget` nodes are done (the rest stay queued). Each recomputed object joins the list [`Hierarchy::drain_moved`]
    /// reads.
    pub fn flush<H: Host + ?Sized>(&mut self, host: &H, budget: usize) -> Flush {
        let mut queue = mem::take(&mut self.queue);
        queue.retain(|s| self.slots[*s].as_ref().is_some_and(|n| n.queued));
        queue.sort_unstable_by_key(|s| {
            let n = self.node(*s);
            (n.depth, n.id)
        });
        queue.dedup();
        let mut stats = Flush::default();
        let mut next = 0;
        while next < queue.len() && stats.resolved < budget {
            let slot = queue[next];
            next += 1;
            if self.node(slot).queued {
                self.resolve_subtree(slot, host, &mut stats);
            }
        }
        queue.drain(..next);
        queue.retain(|s| self.node(*s).queued);
        stats.carried = queue.len();
        self.queue = queue;
        stats
    }

    /// Append the objects resolved since the last call to `out`, each once, in the order they were first resolved (parents
    /// before their children), and clear the list. Objects deleted meanwhile are left out; one deleted and made again is
    /// listed once. The list holds at most about twice the held objects whether or not it is drained.
    pub fn drain_moved(&mut self, out: &mut Vec<ObjId>) {
        let mut list = mem::take(&mut self.moved);
        for id in list.drain(..) {
            let Some(slot) = self.slot_of(id) else {
                continue;
            };
            if mem::replace(&mut self.node_mut(slot).moved, false) {
                out.push(id);
            }
        }
        self.moved = list;
    }

    // ---- internals ----

    fn counted<T>(
        &mut self,
        op: impl FnOnce(&mut Hierarchy) -> Result<T, ParentError>,
    ) -> Result<T, ParentError> {
        let result = op(self);
        if result.is_err() {
            self.refused += 1;
        }
        result
    }

    pub(crate) fn slot_of(&self, id: ObjId) -> Option<Slot> {
        self.index.get(&id).copied()
    }

    pub(crate) fn node(&self, slot: Slot) -> &Node {
        self.slots[slot].as_ref().expect(LIVE)
    }

    fn node_mut(&mut self, slot: Slot) -> &mut Node {
        self.slots[slot].as_mut().expect(LIVE)
    }

    /// Whether `target` is `from` or one of its ancestors. A chain longer than any cap allows counts as a yes, so a broken
    /// tree refuses links instead of looping.
    pub(crate) fn chain_includes(&self, from: Slot, target: Slot) -> bool {
        let mut at = from;
        for _ in 0..=HARD_DEPTH {
            if at == target {
                return true;
            }
            match self.node(at).up {
                Some(up) => at = up.slot,
                None => return false,
            }
        }
        true
    }

    fn root_slot(&self, from: Slot) -> Slot {
        let mut at = from;
        for _ in 0..=HARD_DEPTH {
            match self.node(at).up {
                Some(up) => at = up.slot,
                None => break,
            }
        }
        at
    }

    fn mark_dirty_slot(&mut self, slot: Slot) {
        let node = self.node_mut(slot);
        if !mem::replace(&mut node.queued, true) {
            self.queue.push(slot);
        }
    }

    /// Resolve whatever is queued, so cached worlds can be read: one comparison when nothing is queued.
    fn settle<H: Host + ?Sized>(&mut self, host: &H) {
        if !self.queue.is_empty() {
            self.flush(host, usize::MAX);
        }
    }

    /// Validate, normalize and (when asked) quantize a local pose.
    fn finish_local(&self, local: Local) -> Result<Local, ParentError> {
        local.validate()?;
        let local = local.normalized();
        Ok(if self.quantize {
            quant::quantize(&local)
        } else {
            local
        })
    }

    /// The parent's pose and the node matrix a link hangs on.
    fn attachment_of<H: Host + ?Sized>(
        &self,
        up: Up,
        host: &H,
    ) -> Result<(Pose, Option<Matrix>), ParentError> {
        let parent = self.node(up.slot);
        Ok((parent.pose(), node_matrix(host, parent.id, up.node)?))
    }

    /// The pose of an object: its cached one when the hierarchy holds it, else the host's.
    fn pose_of<H: Host + ?Sized>(
        &self,
        slot: Option<Slot>,
        id: ObjId,
        host: &H,
    ) -> Result<Pose, ParentError> {
        match slot {
            Some(s) => Ok(self.node(s).pose()),
            None => host.pose(id).ok_or(ParentError::Unknown)?.checked(),
        }
    }

    fn place<H: Host + ?Sized>(
        &mut self,
        child: ObjId,
        parent: Option<ParentRef>,
        place: Place,
        host: &H,
    ) -> Result<Pose, ParentError> {
        if let Place::Given(local) = place {
            local.validate()?;
        }
        self.check_parent(child, parent)?;
        self.settle(host);
        let child_slot = self.slot_of(child);
        let child_pose = self.pose_of(child_slot, child, host)?;
        let Some(parent) = parent else {
            return Ok(self.detach_pose(child_slot, child_pose, place));
        };
        let plan = self.plan_link(child_slot, child_pose, parent, place, host)?;
        Ok(self.apply_link(child, child_slot, child_pose, parent, &plan))
    }

    /// Work out the local pose and world a link will have, failing before anything is changed.
    fn plan_link<H: Host + ?Sized>(
        &self,
        child_slot: Option<Slot>,
        child_pose: Pose,
        parent: ParentRef,
        place: Place,
        host: &H,
    ) -> Result<Planned, ParentError> {
        let parent_slot = self.slot_of(parent.object);
        let parent_pose = self.pose_of(parent_slot, parent.object, host)?;
        let node = node_matrix(host, parent.object, parent.node)?;
        let held_local = child_slot.and_then(|c| {
            let n = self.node(c);
            n.up.is_some().then_some(n.local)
        });
        let local = match (place, held_local) {
            (Place::Keep(Keep::World), _) => compose::local_from_world(
                &child_pose.world,
                &parent_pose,
                node.as_ref(),
                child_pose.scale,
            ),
            (Place::Keep(Keep::Local), Some(held)) => held,
            (Place::Keep(Keep::Local), None) => {
                compose::local_from_matrix(&child_pose.world, child_pose.scale)
            }
            (Place::Given(local), _) => local,
        };
        let local = self.finish_local(local)?;
        let world = compose::compose(&parent_pose, node.as_ref(), &local);
        if world.iter().any(|v| !v.is_finite()) {
            return Err(ParentError::OutOfRange);
        }
        Ok(Planned {
            parent_slot,
            parent_pose,
            local,
            world,
        })
    }

    /// Make a planned link: take in the objects the hierarchy does not hold, move the child, set its pose.
    fn apply_link(
        &mut self,
        child: ObjId,
        child_slot: Option<Slot>,
        child_pose: Pose,
        parent: ParentRef,
        plan: &Planned,
    ) -> Pose {
        let p = plan
            .parent_slot
            .unwrap_or_else(|| self.insert_root(parent.object, plan.parent_pose));
        let c = child_slot.unwrap_or_else(|| self.insert_root(child, child_pose));
        let old = self.detach(c);
        self.attach(c, p, parent.node);
        let node = self.node_mut(c);
        node.local = plan.local;
        node.world = plan.world;
        self.mark_dirty_slot(c);
        if let Some(old) = old {
            self.release_if_idle(old);
        }
        Pose {
            world: plan.world,
            scale: plan.local.scale,
        }
    }

    /// Take `child` off its parent (the `None` case of `place`) and return its pose afterwards.
    fn detach_pose(&mut self, child_slot: Option<Slot>, pose: Pose, place: Place) -> Pose {
        let Some(c) = child_slot.filter(|c| self.node(*c).up.is_some()) else {
            return pose;
        };
        let world = match place {
            Place::Keep(Keep::Local) => compose::matrix_from_local(&self.node(c).local),
            Place::Keep(Keep::World) | Place::Given(_) => pose.world,
        };
        let old = self.detach(c);
        self.node_mut(c).world = world;
        if !self.node(c).children.is_empty() {
            self.mark_dirty_slot(c);
        }
        self.release_if_idle(c);
        if let Some(old) = old {
            self.release_if_idle(old);
        }
        Pose {
            world,
            scale: pose.scale,
        }
    }

    fn insert_root(&mut self, id: ObjId, pose: Pose) -> Slot {
        let node = Node {
            id,
            up: None,
            children: Vec::new(),
            local: Local {
                scale: pose.scale,
                ..Local::IDENTITY
            },
            depth: 0,
            height: 0,
            subtree: 0,
            world: pose.world,
            queued: false,
            moved: false,
        };
        let slot = match self.free.pop() {
            Some(slot) => {
                self.slots[slot] = Some(node);
                slot
            }
            None => {
                self.slots.push(Some(node));
                self.slots.len() - 1
            }
        };
        self.index.insert(id, slot);
        slot
    }

    fn release(&mut self, slot: Slot) {
        if let Some(node) = self.slots[slot].take() {
            self.index.remove(&node.id);
            self.free.push(slot);
        }
    }

    /// Let go of a node that has no parent and no children.
    fn release_if_idle(&mut self, slot: Slot) {
        let node = self.node(slot);
        if node.up.is_none() && node.children.is_empty() {
            self.release(slot);
        }
    }

    /// Add `by` to the descendant count of `from` and each ancestor above it.
    fn add_descendants(&mut self, from: Slot, by: usize) {
        let mut at = Some(from);
        while let Some(slot) = at {
            let node = self.node_mut(slot);
            node.subtree += by;
            at = node.up.map(|u| u.slot);
        }
    }

    /// Take `by` from the descendant count of `from` and each ancestor above it.
    fn remove_descendants(&mut self, from: Slot, by: usize) {
        let mut at = Some(from);
        while let Some(slot) = at {
            let node = self.node_mut(slot);
            node.subtree -= by;
            at = node.up.map(|u| u.slot);
        }
    }

    /// A subtree of `height` links now hangs on `from`: raise it and the ancestors above that were shorter.
    fn raise_heights(&mut self, from: Slot, height: u8) {
        let (mut at, mut height) = (from, height);
        loop {
            let node = self.node_mut(at);
            if node.height >= height {
                return;
            }
            node.height = height;
            let Some(up) = node.up else {
                return;
            };
            (at, height) = (up.slot, height.saturating_add(1));
        }
    }

    /// A child left `from`: lower it and the ancestors above that were as tall only through that child.
    fn lower_heights(&mut self, from: Slot) {
        let mut at = from;
        loop {
            let node = self.node(at);
            let height = node
                .children
                .iter()
                .map(|c| self.node(*c).height.saturating_add(1))
                .max()
                .unwrap_or(0);
            if height == node.height {
                return;
            }
            let up = node.up;
            self.node_mut(at).height = height;
            let Some(up) = up else {
                return;
            };
            at = up.slot;
        }
    }

    /// Set `slot`'s depth and every depth below it.
    fn set_depths(&mut self, slot: Slot, depth: u8) {
        self.node_mut(slot).depth = depth;
        let mut stack = mem::take(&mut self.stack);
        stack.clear();
        stack.push(slot);
        while let Some(at) = stack.pop() {
            let below = self.node(at).depth.saturating_add(1);
            for k in 0..self.node(at).children.len() {
                let child = self.node(at).children[k];
                self.node_mut(child).depth = below;
                stack.push(child);
            }
        }
        self.stack = stack;
    }

    /// Hang `child` (a node with no parent) on `parent`. The caps were checked.
    fn attach(&mut self, child: Slot, parent: Slot, node: NodeRef) {
        let size = 1 + self.node(child).subtree;
        let depth = self.node(parent).depth.saturating_add(1);
        let height = self.node(child).height.saturating_add(1);
        self.node_mut(parent).children.push(child);
        self.node_mut(child).up = Some(Up { slot: parent, node });
        self.parented += 1;
        self.add_descendants(parent, size);
        self.raise_heights(parent, height);
        self.set_depths(child, depth);
    }

    /// Take `child` off its parent; the old parent's slot, if it had one.
    fn detach(&mut self, child: Slot) -> Option<Slot> {
        let up = self.node_mut(child).up.take()?;
        let size = 1 + self.node(child).subtree;
        let siblings = &mut self.node_mut(up.slot).children;
        if let Some(at) = siblings.iter().position(|s| *s == child) {
            siblings.remove(at);
        }
        self.parented -= 1;
        self.remove_descendants(up.slot, size);
        self.lower_heights(up.slot);
        self.set_depths(child, 0);
        Some(up.slot)
    }

    /// Remove `root` from the tree; children that go with it are removed first and pushed to `out`, the others become roots.
    /// Returns the parent it had, which the caller lets go of when that left it idle. Walks down the path of first children
    /// in a reused buffer: the path is at most the depth cap long.
    fn remove_node(
        &mut self,
        root: Slot,
        include_root: bool,
        out: &mut Vec<ObjId>,
    ) -> Option<Slot> {
        let mut path = mem::take(&mut self.path);
        path.clear();
        path.push(root);
        let mut parent = None;
        while let Some(&at) = path.last() {
            if let Some(&child) = self.node(at).children.first() {
                if self.delete_with_parent.contains(&self.node(child).id) {
                    path.push(child);
                } else {
                    self.detach(child);
                    self.release_if_idle(child);
                }
                continue;
            }
            path.pop();
            let top = path.is_empty();
            let above = self.remove_leaf(at, include_root || !top, out);
            if top {
                parent = above;
            }
        }
        self.path = path;
        parent
    }

    /// Remove `root` and everything below it, children first. Returns the parent `root` had.
    fn remove_subtree(&mut self, root: Slot, out: &mut Vec<ObjId>) -> Option<Slot> {
        let mut path = mem::take(&mut self.path);
        path.clear();
        path.push(root);
        let mut parent = None;
        while let Some(&at) = path.last() {
            if let Some(&child) = self.node(at).children.first() {
                path.push(child);
                continue;
            }
            path.pop();
            let above = self.remove_leaf(at, true, out);
            if path.is_empty() {
                parent = above;
            }
        }
        self.path = path;
        parent
    }

    /// Remove a node that has no children left; the parent it had.
    fn remove_leaf(
        &mut self,
        slot: Slot,
        include_self: bool,
        out: &mut Vec<ObjId>,
    ) -> Option<Slot> {
        let id = self.node(slot).id;
        let parent = self.detach(slot);
        self.delete_with_parent.remove(&id);
        self.release(slot);
        if include_self {
            out.push(id);
        }
        parent
    }

    fn resolve_subtree<H: Host + ?Sized>(&mut self, root: Slot, host: &H, stats: &mut Flush) {
        if let Some(up) = self.node(root).up {
            let parent = self.node(up.slot);
            let (pose, id) = (parent.pose(), parent.id);
            self.resolve_child(root, &pose, id, host, stats);
        }
        self.node_mut(root).queued = false;
        let mut stack = mem::take(&mut self.stack);
        stack.clear();
        stack.push(root);
        while let Some(at) = stack.pop() {
            let parent = self.node(at);
            let (pose, id) = (parent.pose(), parent.id);
            for k in 0..self.node(at).children.len() {
                let child = self.node(at).children[k];
                self.resolve_child(child, &pose, id, host, stats);
                stack.push(child);
            }
        }
        self.stack = stack;
    }

    fn resolve_child<H: Host + ?Sized>(
        &mut self,
        child: Slot,
        parent: &Pose,
        parent_id: ObjId,
        host: &H,
        stats: &mut Flush,
    ) {
        let node = self.node(child);
        let local = node.local;
        let attach_node = node.up.map_or(NodeRef::Origin, |u| u.node);
        let matrix = node_matrix(host, parent_id, attach_node).unwrap_or_else(|_| {
            stats.node_misses += 1;
            None
        });
        let mut world = compose::compose(parent, matrix.as_ref(), &local);
        if world.iter().any(|v| !v.is_finite()) {
            stats.overflowed += 1;
            world = parent.world;
        }
        let node = self.node_mut(child);
        node.world = world;
        node.queued = false;
        let first = !mem::replace(&mut node.moved, true);
        let id = node.id;
        stats.resolved += 1;
        if first {
            self.moved.push(id);
            if self.moved.len() > 2 * self.index.len() + MOVED_SLACK {
                self.compact_moved();
            }
        }
    }

    /// Drop the entries of the moved list whose object is gone and the repeats of one made again after it was deleted.
    fn compact_moved(&mut self) {
        let mut list = mem::take(&mut self.moved);
        // An id's first entry clears its flag, so a repeat of it finds the flag clear and is dropped; the flags are set again
        // for the entries kept.
        list.retain(|id| {
            let Some(&slot) = self.index.get(id) else {
                return false;
            };
            self.slots[slot]
                .as_mut()
                .is_some_and(|n| mem::replace(&mut n.moved, false))
        });
        for id in &list {
            if let Some(&slot) = self.index.get(id) {
                self.node_mut(slot).moved = true;
            }
        }
        self.moved = list;
    }
}

/// Node `node` of `object` as a matrix: `None` for the origin, an error when the host has no such node or its matrix is not
/// usable.
fn node_matrix<H: Host + ?Sized>(
    host: &H,
    object: ObjId,
    node: NodeRef,
) -> Result<Option<Matrix>, ParentError> {
    match node {
        NodeRef::Origin => Ok(None),
        NodeRef::Node(i) => host
            .node_matrix(object, i)
            .filter(|m| matrix_check(m).is_ok())
            .map(Some)
            .ok_or(ParentError::BadNode),
    }
}
