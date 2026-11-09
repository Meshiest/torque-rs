//! The scene hierarchy as a client holds it: the links and local poses a server's per-ghost blocks
//! ([`ghosts::block::GhostBlock`]) carried, keyed by ghost index, and where a child is drawn
//! ([`ClientWorld::world_pose`](crate::world::ClientWorld::world_pose)).
//!
//! A child is drawn at its parent's drawn pose times the parent's node times its own local pose, the local blended over one
//! tick from the pose it was drawn at to the new one (a pose sent with `snap`, and every link change, shows at once). A ghost
//! whose link waits for its parent (the server has no ghost of the parent on the connection yet, or names one the client does
//! not hold) is drawn at its own record's pose.
//!
//! **Hostile server.** Every link goes through the same checks a server's tree makes ([`Hierarchy::check_parent`]: no cycle,
//! the hard depth, children, descendants and total caps) and every pose through [`QuantLocal::check`]; what fails is refused
//! and counted ([`LinkStats`]), and the ghost is drawn where its record puts it. A link to a ghost index the client does not
//! hold waits at most [`WAIT_MS`] and at most [`MAX_WAITING`] wait at once.

use std::collections::{BTreeMap, BTreeSet};

use collide::math::{IDENTITY, Matrix, plain_quat_normalize};
use ghosts::block::{GhostBlock, LinkField, PoseField};
use hierarchy::{
    Caps, HARD_CHILDREN, HARD_DEPTH, HARD_DESCENDANTS, HARD_TOTAL, Hierarchy, Host, Keep, Local,
    NodeRef, ObjId, ParentRef, Pose, QuantLocal, QuantRotation, UNIT_SCALE_CODE,
};

use crate::world::Pose as RecordPose;

/// The longest a link to a ghost index the client does not hold waits for that ghost.
pub const WAIT_MS: u32 = 5_000;
/// The most links that wait at once; one past it is refused.
pub const MAX_WAITING: usize = 4_096;
/// How long a new local pose takes to blend in: one server tick.
pub const BLEND_MS: u32 = 32;

/// What the client refused or let go of, counted since the world started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Links the tree's checks refused (a cycle, a cap, the child itself as its parent).
    pub refused: u64,
    /// Poses whose codes were out of range.
    pub bad_poses: u64,
    /// Links that waited past [`WAIT_MS`] for their parent.
    pub expired: u64,
    /// Links refused because [`MAX_WAITING`] were waiting.
    pub over_cap: u64,
}

/// A link to a ghost the client does not hold yet.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wait {
    parent: ParentRef,
    since: u32,
}

/// The waiting links, found by child, by the parent they wait for and by age, so that a ghost's arrival and the clock's
/// passing cost what they resolve, not what waits.
#[derive(Debug, Clone, Default, PartialEq)]
struct Waits {
    by_child: BTreeMap<u32, Wait>,
    by_parent: BTreeMap<u32, BTreeSet<u32>>,
    by_age: BTreeSet<(u32, u32)>,
}

impl Waits {
    fn len(&self) -> usize {
        self.by_child.len()
    }

    fn insert(&mut self, child: u32, wait: Wait) {
        self.remove(child);
        self.by_child.insert(child, wait);
        self.by_parent
            .entry(wait.parent.object.0)
            .or_default()
            .insert(child);
        self.by_age.insert((wait.since, child));
    }

    fn remove(&mut self, child: u32) -> Option<Wait> {
        let wait = self.by_child.remove(&child)?;
        let parent = wait.parent.object.0;
        if let Some(set) = self.by_parent.get_mut(&parent) {
            set.remove(&child);
            if set.is_empty() {
                self.by_parent.remove(&parent);
            }
        }
        self.by_age.remove(&(wait.since, child));
        Some(wait)
    }

    /// The children waiting for `parent`, taken out.
    fn take_for(&mut self, parent: u32) -> Vec<(u32, Wait)> {
        let children = self.by_parent.remove(&parent).unwrap_or_default();
        children
            .into_iter()
            .filter_map(|c| {
                let wait = self.by_child.remove(&c)?;
                self.by_age.remove(&(wait.since, c));
                Some((c, wait))
            })
            .collect()
    }

    /// The oldest waiting link, if it has waited past [`WAIT_MS`] at `now`, taken out.
    fn take_expired(&mut self, now: u32) -> Option<u32> {
        let &(since, child) = self.by_age.first()?;
        (now.wrapping_sub(since) > WAIT_MS).then(|| {
            self.remove(child);
            child
        })
    }
}

/// The local pose a child is drawn with: the last one sent, the one it blends from and when it arrived.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shown {
    local: Local,
    from: Local,
    at: u32,
    snap: bool,
}

impl Shown {
    fn at_once(local: Local, at: u32) -> Shown {
        Shown {
            local,
            from: local,
            at,
            snap: true,
        }
    }

    /// The pose at `now` (`animate` false: the last one sent).
    fn drawn(&self, now: u32, animate: bool) -> Local {
        if self.snap || !animate {
            return self.local;
        }
        let t = (now.wrapping_sub(self.at) as f32 / BLEND_MS as f32).min(1.0);
        blend(&self.from, &self.local, t)
    }
}

/// `a` to `b` by `t`: the translation and scale lerped, the rotation nlerped the short way round.
fn blend(a: &Local, b: &Local, t: f32) -> Local {
    if t >= 1.0 {
        return *b;
    }
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    let dot: f32 = a.rotation.iter().zip(&b.rotation).map(|(x, y)| x * y).sum();
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let rotation: [f32; 4] = std::array::from_fn(|i| lerp(a.rotation[i], b.rotation[i] * sign));
    Local {
        translation: std::array::from_fn(|i| lerp(a.translation[i], b.translation[i])),
        rotation: plain_quat_normalize(&rotation),
        scale: std::array::from_fn(|i| lerp(a.scale[i], b.scale[i])),
    }
}

/// The client tree's host: the ghosts that exist, all at the origin. The tree keeps the links, the local poses and the
/// caps; where a child is drawn is worked out from the drawn poses
/// ([`ClientWorld::world_pose`](crate::world::ClientWorld::world_pose)), never from the tree's
/// cached worlds.
struct Held<'a>(&'a dyn Fn(u32) -> bool);

impl Host for Held<'_> {
    fn pose(&self, object: ObjId) -> Option<Pose> {
        (self.0)(object.0).then_some(Pose {
            world: IDENTITY,
            scale: [1.0; 3],
        })
    }

    fn node_matrix(&self, _object: ObjId, _node: u16) -> Option<Matrix> {
        Some(IDENTITY)
    }
}

/// The links and local poses the server sent (module docs).
#[derive(Debug, Clone)]
pub struct ClientLinks {
    tree: Hierarchy,
    waiting: Waits,
    shown: BTreeMap<u32, Shown>,
    /// Children whose parent's ghost went, at their last drawn pose, until their own next update.
    held: BTreeMap<u32, (Matrix, [f32; 3])>,
    /// The ghosts made since the waiting links were last tried.
    arrived: Vec<u32>,
    stats: LinkStats,
}

impl Default for ClientLinks {
    fn default() -> ClientLinks {
        ClientLinks {
            tree: Hierarchy::new(Caps {
                depth: HARD_DEPTH,
                children: HARD_CHILDREN,
                descendants: HARD_DESCENDANTS,
                total: HARD_TOTAL,
            }),
            waiting: Waits::default(),
            shown: BTreeMap::new(),
            held: BTreeMap::new(),
            arrived: Vec::new(),
            stats: LinkStats::default(),
        }
    }
}

impl PartialEq for ClientLinks {
    // The world compares by its ghosts; the links follow from what they were sent.
    fn eq(&self, other: &ClientLinks) -> bool {
        self.waiting == other.waiting && self.shown == other.shown && self.stats == other.stats
    }
}

/// The local pose a pose group stands for, `None` when a code is out of range.
fn local_of(pose: &PoseField) -> Option<Local> {
    let quant = QuantLocal {
        translation: pose.translation,
        rotation: QuantRotation {
            dropped: pose.dropped,
            parts: pose.parts,
        },
        scale: pose.scale.unwrap_or([UNIT_SCALE_CODE; 3]),
    };
    quant.check().ok().map(|()| quant.to_local())
}

/// The parent a link group names: its ghost and node.
fn parent_of(ghost: u32, node: Option<u16>) -> ParentRef {
    ParentRef {
        object: ObjId(ghost),
        node: node.map_or(NodeRef::Origin, NodeRef::Node),
    }
}

impl ClientLinks {
    /// What was refused and let go of.
    #[must_use]
    pub fn stats(&self) -> LinkStats {
        self.stats
    }

    /// The parent ghost `index` is drawn on, and its node.
    #[must_use]
    pub fn parent(&self, index: u32) -> Option<ParentRef> {
        self.tree.parent(ObjId(index))
    }

    /// The children of ghost `index`, in the order they were linked.
    pub fn children(&self, index: u32) -> impl Iterator<Item = u32> + '_ {
        self.tree.children(ObjId(index)).map(|c| c.0)
    }

    /// Whether ghost `index`'s link waits for its parent's ghost.
    #[must_use]
    pub fn is_waiting(&self, index: u32) -> bool {
        self.waiting.by_child.contains_key(&index)
    }

    /// Whether links wait for a ghost at `index`.
    #[must_use]
    pub fn is_awaited(&self, index: u32) -> bool {
        self.waiting.by_parent.contains_key(&index)
    }

    /// How many links wait.
    #[must_use]
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    /// The ghosts that have a parent.
    #[must_use]
    pub fn linked_count(&self) -> usize {
        self.tree.parented_count()
    }

    /// The tree's own rules hold (caps, counts, links both ways).
    ///
    /// # Errors
    /// The first rule found broken.
    pub fn audit(&self) -> Result<(), hierarchy::Violation> {
        self.tree.audit()
    }

    /// The local pose ghost `index` is drawn with at `now` (blended when `animate`), for a ghost with a parent.
    #[must_use]
    pub fn local(&self, index: u32, now: u32, animate: bool) -> Option<Local> {
        self.tree.parent(ObjId(index))?;
        Some(self.shown.get(&index)?.drawn(now, animate))
    }

    /// Where ghost `index` was left when its parent's ghost went (until its next update).
    #[must_use]
    pub fn held(&self, index: u32) -> Option<(Matrix, [f32; 3])> {
        self.held.get(&index).copied()
    }

    /// The block of an update (or the creating record) of ghost `index`, at the world clock `now` (ms) and the animation
    /// clock `anim`. `exists` says which ghost indices the client holds.
    pub fn apply(
        &mut self,
        index: u32,
        block: &GhostBlock,
        exists: &dyn Fn(u32) -> bool,
        now: u32,
        anim: u32,
    ) {
        self.held.remove(&index);
        let local = match &block.pose {
            Some(pose) => match local_of(pose) {
                Some(local) => Some((local, pose.snap)),
                None => {
                    self.stats.bad_poses += 1;
                    None
                }
            },
            None => None,
        };
        if let Some(link) = block.link {
            let local = local.map(|(l, _)| l);
            self.relink(index, link, local, exists, now, anim);
            return;
        }
        let Some((local, snap)) = local else {
            return;
        };
        self.show(index, local, snap, anim);
        // A waiting link takes the pose kept in `shown` when it resolves.
        if self.tree.parent(ObjId(index)).is_some()
            && self.tree.set_local(ObjId(index), local).is_err()
        {
            self.stats.bad_poses += 1;
        }
    }

    /// A new local pose to draw `index` with, blended from the pose it is drawn with now unless `snap`.
    fn show(&mut self, index: u32, local: Local, snap: bool, anim: u32) {
        let shown = match self.shown.get(&index) {
            Some(was) if !snap => Shown {
                local,
                from: was.drawn(anim, true),
                at: anim,
                snap: false,
            },
            _ => Shown::at_once(local, anim),
        };
        self.shown.insert(index, shown);
    }

    fn relink(
        &mut self,
        index: u32,
        link: LinkField,
        local: Option<Local>,
        exists: &dyn Fn(u32) -> bool,
        now: u32,
        anim: u32,
    ) {
        self.waiting.remove(index);
        // A link change shows its pose at once: the parent and the pose change together.
        let local = local
            .or_else(|| self.shown.get(&index).map(|s| s.local))
            .unwrap_or(Local::IDENTITY);
        let (ghost, node) = match link {
            LinkField::Root => {
                self.shown.remove(&index);
                self.unlink(index, exists);
                return;
            }
            LinkField::Waiting => {
                self.shown.insert(index, Shown::at_once(local, anim));
                self.unlink(index, exists);
                return;
            }
            LinkField::Parent { ghost, node } => (ghost, node),
        };
        self.shown.insert(index, Shown::at_once(local, anim));
        let parent = parent_of(ghost, node);
        if !exists(ghost) {
            self.unlink(index, exists);
            if self.waiting.len() >= MAX_WAITING {
                self.stats.over_cap += 1;
                return;
            }
            self.waiting.insert(index, Wait { parent, since: now });
            return;
        }
        self.link_now(index, parent, local, exists);
    }

    /// Hangs `index` on `parent` (which the client holds), or refuses and counts it.
    fn link_now(
        &mut self,
        index: u32,
        parent: ParentRef,
        local: Local,
        exists: &dyn Fn(u32) -> bool,
    ) {
        let child = ObjId(index);
        if self.tree.parent(child) == Some(parent) {
            if self.tree.set_local(child, local).is_err() {
                self.stats.bad_poses += 1;
            }
            return;
        }
        // `link` makes every check `set_parent` makes on a server ([`Hierarchy::check_parent`]) and changes nothing on a
        // refusal.
        if self.tree.link(child, parent, local, &Held(exists)).is_err() {
            self.stats.refused += 1;
            self.unlink(index, exists);
        }
    }

    fn unlink(&mut self, index: u32, exists: &dyn Fn(u32) -> bool) {
        if self.tree.parent(ObjId(index)).is_some() {
            // Taking a link off never fails a check; the host has every ghost it names.
            let _ = self
                .tree
                .set_parent(ObjId(index), None, Keep::Local, &Held(exists));
        }
    }

    /// A ghost was made at `index`: links waiting for it resolve at the next [`ClientLinks::settle`].
    pub fn created(&mut self, index: u32) {
        self.held.remove(&index);
        if self.waiting.by_parent.contains_key(&index) {
            self.arrived.push(index);
        }
    }

    /// Ghost `index` went. Its children are held at `worlds` (their last drawn poses) and taken off it; its own link and pose
    /// are forgotten, and so are the links that waited for it. `exists` must already answer false for `index`.
    pub fn deleted(
        &mut self,
        index: u32,
        worlds: impl IntoIterator<Item = (u32, (Matrix, [f32; 3]))>,
        exists: &dyn Fn(u32) -> bool,
    ) {
        self.held.extend(worlds);
        self.held.remove(&index);
        self.waiting.remove(index);
        // Links that waited for the ghost that went named that object, not whatever ghost the index is given next.
        self.waiting.take_for(index);
        self.shown.remove(&index);
        // The tree asks nothing of a deleted object's host but the children's poses, which it keeps.
        self.tree.delete(
            ObjId(index),
            &Held(&|i| i != index && exists(i)),
            &mut Vec::new(),
        );
    }

    /// Every ghost went (a reset).
    pub fn clear(&mut self) {
        let stats = self.stats;
        *self = ClientLinks {
            stats,
            ..ClientLinks::default()
        };
    }

    /// Resolves the links whose parent's ghost was made since the last call and lets go of those that waited past
    /// [`WAIT_MS`] at `now`. Costs what it resolves and lets go of.
    pub fn settle(&mut self, exists: &dyn Fn(u32) -> bool, now: u32) {
        for parent in std::mem::take(&mut self.arrived) {
            if !exists(parent) {
                continue;
            }
            for (index, wait) in self.waiting.take_for(parent) {
                if exists(index) {
                    let local = self.shown.get(&index).map_or(Local::IDENTITY, |s| s.local);
                    self.link_now(index, wait.parent, local, exists);
                }
            }
        }
        while let Some(child) = self.waiting.take_expired(now) {
            if exists(child) {
                self.stats.expired += 1;
            }
        }
    }
}

/// A record pose as a transform (row-major, translation in the last column, as `MatrixF`).
#[must_use]
pub fn record_matrix(pose: &RecordPose) -> Matrix {
    use collide::math::quat_matrix;
    let placed = |mut m: Matrix, p: [f32; 3]| {
        [m[3], m[7], m[11]] = p;
        m
    };
    let turn_z = |a: f32| {
        let (s, c) = a.sin_cos();
        let mut m = IDENTITY;
        [m[0], m[1], m[4], m[5]] = [c, s, -s, c];
        m
    };
    match pose {
        RecordPose::Affine { xf, .. } => {
            let [x, y, z] = xf.quat.map(|v| v.get());
            let mut w = (1.0 - (x * x + y * y + z * z)).max(0.0).sqrt();
            if xf.negative_w {
                w = -w;
            }
            placed(quat_matrix([x, y, z, w]), xf.position.map(|v| v.get()))
        }
        RecordPose::Matrix { m, .. } => *m,
        RecordPose::Euler { position, yaw, .. } => placed(turn_z(*yaw), *position),
        RecordPose::Quat { position, rotation } => placed(quat_matrix(*rotation), *position),
        RecordPose::Item { position, rotation } => {
            let (s, c) = (RecordPose::item_angle(*rotation) * 0.5).sin_cos();
            placed(quat_matrix([0.0, 0.0, s, c]), *position)
        }
    }
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
