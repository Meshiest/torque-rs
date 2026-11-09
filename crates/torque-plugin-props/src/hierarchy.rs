//! Props in the scene hierarchy: what the server reads of the tree before each step to make each prop's physics fit its place in
//! it ([`reconcile`]), and the rule that keeps a prop with joints from being hung on anything ([`PropRule`]).
//!
//! A prop whose parent is a dynamic prop becomes part of that prop's body at the pose the tree gives it (the chain of local poses
//! from the root down, composed the way the tree composes them, so the pose is the tree's own to the last bit and changes only
//! when a link or a local pose does); a prop under any other parent is driven to where the tree puts it; a prop with no parent is
//! its own body. The world side is [`crate::world::Link`].

use std::collections::{BTreeMap, BTreeSet};

use torque_collide::math::{IDENTITY, Matrix};
use torque_game::scene::access::{matrix_of, scale_of};
use torque_game::scene::links::{ChildRefusal, ChildRule};
use torque_hierarchy::{HARD_DEPTH, Local, NodeRef, ParentRef, Pose, compose};
use torque_plugin_hierarchy::ServerHierarchy;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::quant;
use crate::server::ServerProps;
use crate::world::{Link, PropWorld};

/// The links from a part up to the prop whose body it is on: each object with its parent link and local pose. A part is placed
/// again when this changes.
type Chain = Vec<(ObjId, ParentRef, Local)>;

/// What the server remembers between ticks of how it placed each part, and how far into the hierarchy's changes it has read.
#[derive(Debug, Default)]
pub struct Links {
    chains: BTreeMap<u32, Chain>,
    /// The hierarchy's change cursor ([`ServerHierarchy::track_changes`]); none before the first fit.
    cursor: Option<u64>,
    /// Props whose fit was refused for the tick's budget, tried again at the next fit.
    retry: BTreeSet<u32>,
    stats: LinkStats,
}

/// What the fits have cost since the server started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Fits that looked at every prop (the first, and after changes were forgotten).
    pub full: u64,
    /// Props fitted: each is a walk up its chain.
    pub fitted: u64,
    /// Objects of the tree looked at to find the props a change reaches.
    pub visited: u64,
}

impl Links {
    /// What the fits have cost.
    #[must_use]
    pub fn stats(&self) -> LinkStats {
        self.stats
    }
}

/// Refuses to hang a prop that holds a joint (or is held) on anything: its joints are on a body of its own.
#[derive(Debug, Clone)]
pub struct PropRule(pub ServerProps);

impl ChildRule for PropRule {
    fn refuse(&self, _vm: &Vm, child: ObjId, _parent: ObjId) -> Option<ChildRefusal> {
        self.0
            .with(|s| {
                s.world
                    .as_ref()
                    .is_some_and(|w| w.joints_on(child) > 0)
                    .then_some(ChildRefusal::HasJoints)
            })
            .flatten()
    }
}

/// How many links `id` is from a root.
fn depth(h: &ServerHierarchy, id: ObjId) -> usize {
    let mut at = id;
    let mut n = 0;
    while let Some(p) = h.parent(at) {
        n += 1;
        if n > HARD_DEPTH {
            break;
        }
        at = p.object.0;
    }
    n
}

/// The links from `child` up to `root` (a prop above it in the tree).
fn chain_of(h: &ServerHierarchy, child: ObjId, root: ObjId) -> Option<Chain> {
    let mut chain = Chain::new();
    let mut at = child;
    loop {
        let parent = h.parent(at)?;
        chain.push((at, parent, h.local(at)?));
        if parent.object.0 == root {
            return Some(chain);
        }
        at = parent.object.0;
        if chain.len() > HARD_DEPTH {
            return None;
        }
    }
}

/// Where the child at the bottom of `chain` is in the frame of `root`: the tree's own composition from the root's frame down.
fn relative(vm: &Vm, chain: &Chain, root: ObjId) -> Option<Matrix> {
    let mut on = Pose {
        world: IDENTITY,
        scale: scale_of(vm, root),
    };
    for &(object, parent, local) in chain.iter().rev() {
        let node = match parent.node {
            NodeRef::Origin => None,
            NodeRef::Node(n) => Some(torque_game::scene::links::node_matrix(
                vm,
                parent.object.0,
                n,
            )?),
        };
        on = Pose {
            world: compose(&on, node.as_ref(), &local),
            scale: scale_of(vm, object),
        };
    }
    Some(on.world)
}

/// The props the changes reach: those of `starts` and of everything below them (the chains of what hangs below an object that
/// changed may have changed). Each object is looked at once whatever the number of starts that reach it, and the looking stops
/// at `bound` objects (more than a look at every prop would cost): None then, for the caller to fit every prop instead.
fn expand(
    h: &ServerHierarchy,
    world: &PropWorld,
    starts: &BTreeSet<ObjId>,
    bound: usize,
    visited: &mut u64,
) -> Option<BTreeSet<u32>> {
    let mut out = BTreeSet::new();
    let mut seen: BTreeSet<ObjId> = BTreeSet::new();
    let mut stack: Vec<ObjId> = starts.iter().copied().collect();
    while let Some(at) = stack.pop() {
        if !seen.insert(at) {
            continue;
        }
        *visited += 1;
        if seen.len() > bound {
            return None;
        }
        if world.get(at).is_some() {
            out.insert(at);
        }
        stack.extend(h.children(at));
    }
    Some(out)
}

/// Makes the physics of prop `id` fit its place in the hierarchy: a part of its parent's root, driven, or its own body. False when
/// the fit was refused for the tick's budget and is to be tried again.
fn fit(vm: &Vm, h: &ServerHierarchy, links: &mut Links, world: &mut PropWorld, id: u32) -> bool {
    let Some(parent) = h.parent(id) else {
        if !world.is_child(id) {
            return true;
        }
        if world.release(id) {
            links.chains.remove(&id);
            return true;
        }
        return !world.build_exhausted();
    };
    let above = parent.object.0;
    let root = match world.link(above) {
        Some(Link::Free) if world.is_dynamic(above) => Some(above),
        Some(Link::Part { root, .. }) => Some(root),
        _ => None,
    };
    let Some(root) = root else {
        links.chains.remove(&id);
        return drive_to_place(vm, world, id) || !world.build_exhausted();
    };
    let Some(chain) = chain_of(h, id, root) else {
        return true;
    };
    let placed = matches!(world.link(id), Some(Link::Part { root: r, .. }) if r == root)
        && links.chains.get(&id) == Some(&chain);
    if placed {
        return true;
    }
    let Some(local) = relative(vm, &chain, root).as_ref().and_then(quant::pose_of) else {
        return true;
    };
    if world.attach(id, root, local) {
        links.chains.insert(id, chain);
        return true;
    }
    !world.build_exhausted()
}

/// Takes driven prop `id` to where the hierarchy has put its object; whether the world took it.
fn drive_to_place(vm: &Vm, world: &mut PropWorld, id: u32) -> bool {
    matrix_of(vm, id)
        .as_ref()
        .and_then(quant::pose_of)
        .is_some_and(|pose| world.drive(id, pose))
}

/// Makes every prop's physics fit its place in the hierarchy, parents first: a prop hung on a dynamic prop joins its body, a
/// prop hung on anything else is driven to where the tree puts it (the tree resolved already), and a prop with no parent is its
/// own body, leaving at the speed its place gave it.
///
/// Only what changed is looked at: the objects the hierarchy says changed a link or a local pose (and what hangs below them), the
/// roots whose bodies were frozen, thawed or removed, and the driven props, which are given their targets each tick (one that is
/// still where it was taken costs a comparison), and the props whose fit was refused for the tick's budget. The changes are
/// gathered into one set and walked once, and a walk past the number of props is a full fit instead. The first fit, and one after
/// the hierarchy forgot changes, looks at every prop.
pub(crate) fn reconcile(vm: &Vm, links: &mut Links, world: &mut PropWorld) {
    // A stopped world is made again by the step; what changed waits for it.
    if world.physics().is_poisoned() {
        return;
    }
    let dirty_roots = world.take_dirty_roots();
    let Some(h) = ServerHierarchy::of(vm) else {
        // Without a hierarchy nothing is a child.
        let children: Vec<u32> = world.children().collect();
        for id in children {
            world.release(id);
        }
        links.chains.clear();
        links.retry.clear();
        links.cursor = None;
        return;
    };
    let mut changed = Vec::new();
    let full = match links.cursor.map(|c| h.changes_since(c, &mut changed)) {
        Some(Ok(now)) => {
            links.cursor = Some(now);
            false
        }
        Some(Err(now)) => {
            links.cursor = Some(now);
            true
        }
        None => {
            links.cursor = Some(h.track_changes());
            true
        }
    };
    let mut todo = std::mem::take(&mut links.retry);
    let starts: BTreeSet<ObjId> = changed.into_iter().chain(dirty_roots).collect();
    let reached = if full {
        None
    } else {
        expand(
            &h,
            world,
            &starts,
            world.len().max(16),
            &mut links.stats.visited,
        )
    };
    if let Some(reached) = reached {
        todo.extend(reached);
    } else {
        links.stats.full += 1;
        todo.extend(
            world
                .props()
                .iter()
                .filter(|(id, p)| p.link.is_child() || h.parent(**id).is_some())
                .map(|(&id, _)| id),
        );
    }
    let mut order: Vec<(usize, u32)> = todo.iter().map(|&id| (depth(&h, id), id)).collect();
    order.sort_unstable();
    for &(_, id) in &order {
        links.stats.fitted += 1;
        if !fit(vm, &h, links, world, id) {
            links.retry.insert(id);
        }
    }
    let driven: Vec<u32> = world.driven().filter(|id| !todo.contains(id)).collect();
    for id in driven {
        if !drive_to_place(vm, world, id) && world.build_exhausted() {
            links.retry.insert(id);
        }
    }
    if links.chains.len() > 2 * world.len() + 16 {
        links
            .chains
            .retain(|id, _| world.link(*id).is_some_and(|l| l.is_child()));
    }
}
