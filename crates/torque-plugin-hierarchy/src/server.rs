//! A server's hierarchy: the tree, plugged into the scene's seam ([`torque_game::scene::links`]), and the operations its
//! natives and Rust callers make.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

use torque_collide::math::{Matrix, quat_matrix};
use torque_engine::{Prefs, VmPrefs};
use torque_game::scene::access::{matrix_of, object_field, scale_of};
use torque_game::scene::ghost;
use torque_game::scene::lights;
use torque_game::scene::links::{self, NodeLookup, SceneLinks};
use torque_game::sim::mount;
use torque_ghosts::block::{BlockSections, GhostBlock, LINK_MASK, LinkField, POSE_MASK, PoseField};
use torque_hierarchy::{
    Caps, Hierarchy, Host, Keep, Local, NodeRef, ObjId as Node, ParentError, ParentRef, Pose,
    QuantLocal, UNIT_SCALE_CODE, Violation,
};
use torque_net::bits::BitWriter;
use torque_net::ghost::ConnectionGhosts;
use torque_server::net::blocks::GhostBlocks;
use torque_ts_vm::sim::{ClassId, ObjId};
use torque_ts_vm::vm::Vm;

/// A server's prefs for the hierarchy, read when its first link is made (so a server script sets them before it parents
/// anything) and held to their ranges and the hard caps.
#[derive(Prefs, Clone, Debug, PartialEq)]
#[prefs(prefix = "$Pref::Hierarchy")]
pub struct HierarchyPrefs {
    /// The longest chain below a root, in links.
    #[pref(default = 16, range = 1..=32, name = "MaxDepth")]
    pub max_depth: i32,
    /// The most children of one parent.
    #[pref(default = 256, range = 1..=1024, name = "MaxChildren")]
    pub max_children: i32,
    /// The most descendants of one root: what moving a root may cost.
    #[pref(default = 4096, range = 1..=16384, name = "MaxDescendants")]
    pub max_descendants: i32,
    /// The most parented objects on the server.
    #[pref(default = 32768, range = 1..=131072, name = "MaxTotal")]
    pub max_total: i32,
    /// The most children one resolve writes; the roots past it wait for the next resolve, counted in
    /// [`Stats::carried`].
    #[pref(default = 131072, range = 64..=131072, name = "ResolveBudget")]
    pub resolve_budget: i32,
}

/// Why a link or a local pose was refused. Nothing changed when one is returned; scripts read [`LinkError::name`] from
/// `getParentError()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkError {
    /// The tree's own refusal (a cycle, a cap, a bad value, an unknown object, ...).
    Tree(ParentError),
    /// The child is mounted (an object is mounted or parented, never both).
    Mounted,
    /// A node was named on a parent that has no shape.
    NoShape,
    /// The child or the parent is being removed (an `onRemove` script, a delete held until a method returns).
    Removing,
    /// The child's class cannot be a child yet: a prop, whose pose its physics world owns, until props join the hierarchy.
    Class,
}

impl LinkError {
    /// The name scripts see for this reason.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            LinkError::Tree(e) => e.name(),
            LinkError::Mounted => "mounted",
            LinkError::NoShape => "noShape",
            LinkError::Removing => "removing",
            LinkError::Class => "class",
        }
    }
}

impl From<ParentError> for LinkError {
    fn from(e: ParentError) -> LinkError {
        LinkError::Tree(e)
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Tree(e) => e.fmt(f),
            LinkError::Mounted => f.write_str("the object is mounted"),
            LinkError::NoShape => f.write_str("the parent has no shape to name a node of"),
            LinkError::Removing => f.write_str("the object is being removed"),
            LinkError::Class => f.write_str("objects of this class cannot be children"),
        }
    }
}

impl std::error::Error for LinkError {}

/// What the resolves have counted since the server started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Dirty roots a resolve left for the next one ([`HierarchyPrefs::resolve_budget`]).
    pub carried: u64,
    /// Children placed at their parent's origin because the node they hang on is gone.
    pub node_misses: u64,
    /// Children whose own move could not be written as a local pose (too far from the parent, not finite): they were taken
    /// off their parent where they were.
    pub dropped: u64,
    /// Callbacks not run because too many waited.
    pub callbacks_dropped: u64,
    /// Moves of held objects the tree refused (a world or scale that is not finite or out of range).
    pub refused_moves: u64,
}

/// What a server's clients are told of its hierarchy ([`ServerHierarchy::install`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Clients {
    /// Nothing: a child's record carries its world pose, as every record does (Torque's protocol).
    #[default]
    Records,
    /// Each child's link and local pose, in the per-ghost block of its record ([`torque_ghosts::block`]); clients draw it on
    /// its parent. Every local pose is rounded to the wire's steps, so a client composes the server's world exactly, and a
    /// parent's motion costs its children no update (the engine's own protocol).
    Links,
}

/// How a client shows a new local pose (the pose group's `snap`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Blend {
    /// Blended from where the client drew it.
    #[default]
    Smooth,
    /// Shown at once.
    Snap,
}

/// The classes that cannot be children yet ([`LinkError::Class`]): props, whose poses their physics world owns.
const NOT_CHILDREN: &[&[u8]] = &[b"Prop"];

/// A script callback waiting for a point where scripts may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Callback {
    /// `onParentChanged(%data, %obj, %old, %new)` on the child's datablock.
    ParentChanged {
        child: ObjId,
        old: Option<ObjId>,
        new: Option<ObjId>,
    },
    /// `onChildAdded(%data, %obj, %child)` on the parent's datablock.
    ChildAdded { parent: ObjId, child: ObjId },
    /// `onChildRemoved(%data, %obj, %child)` on the parent's datablock.
    ChildRemoved { parent: ObjId, child: ObjId },
}

impl Callback {
    /// The object whose datablock is called, the callback's name, and the ids after `%obj` (0 for none) with how many there
    /// are.
    fn call(self) -> (ObjId, &'static [u8], [Option<ObjId>; 2], usize) {
        match self {
            Callback::ParentChanged { child, old, new } => {
                (child, b"onParentChanged", [old, new], 2)
            }
            Callback::ChildAdded { parent, child } => {
                (parent, b"onChildAdded", [Some(child), None], 1)
            }
            Callback::ChildRemoved { parent, child } => {
                (parent, b"onChildRemoved", [Some(child), None], 1)
            }
        }
    }
}

/// The most callbacks that wait at once (a server that parents in a loop without a tick); past it they are counted.
const MAX_CALLBACKS: usize = 65_536;

/// How many times one resolve goes round when a class kept a pose of its own (a Player stays upright).
const RESOLVE_ROUNDS: usize = 4;

#[derive(Default)]
struct State {
    /// Made at the first link, from the prefs.
    tree: Option<Hierarchy>,
    budget: usize,
    last_error: Option<LinkError>,
    callbacks: Vec<Callback>,
    /// Children whose last local pose was set with `snap` (the network's to read: a snapped local is not blended).
    snaps: BTreeSet<ObjId>,
    stats: Stats,
    /// Scratch for the moved list.
    moved: Vec<Node>,
    /// The server's clients draw children from their links ([`Clients::Links`]): locals are quantized for the wire, a
    /// parent's motion marks no child, and links and locals mark the per-ghost block.
    net: bool,
    /// The record bits a parent's motion would have marked on each child ([`links::carry_quiet`]), marked again when the
    /// child's link changes, so the record's own pose is current whenever a client draws the child from it.
    held: BTreeMap<ObjId, u32>,
}

impl State {
    fn tree_mut(&mut self, vm: &Vm) -> &mut Hierarchy {
        let budget = &mut self.budget;
        self.tree.get_or_insert_with(|| {
            let prefs = vm.prefs::<HierarchyPrefs>();
            *budget = usize::try_from(prefs.resolve_budget).unwrap_or(usize::MAX);
            let count = |v: i32| usize::try_from(v).unwrap_or(1);
            Hierarchy::new(Caps {
                depth: count(prefs.max_depth),
                children: count(prefs.max_children),
                descendants: count(prefs.max_descendants),
                total: count(prefs.max_total),
            })
            .quantize_locals(self.net)
        })
    }

    /// A child's local pose changed: its block's pose group is due.
    fn pose_changed(&self, vm: &Vm, id: ObjId) {
        pose_changed(self.net, vm, id);
    }

    /// A child's link changed (made, moved or taken off): its block's groups are due, and its record's pose too.
    fn link_changed(&mut self, vm: &Vm, id: ObjId) {
        link_changed(self.net, &mut self.held, vm, id);
    }

    fn queue(&mut self, callback: Callback) {
        if self.callbacks.len() < MAX_CALLBACKS {
            self.callbacks.push(callback);
        } else {
            self.stats.callbacks_dropped += 1;
        }
    }
}

/// [`State::pose_changed`], for a caller that holds the tree.
fn pose_changed(net: bool, vm: &Vm, id: ObjId) {
    if net {
        ghost::mark_record(vm, id, POSE_MASK);
    }
}

/// [`State::link_changed`], for a caller that holds the tree.
fn link_changed(net: bool, held: &mut BTreeMap<ObjId, u32>, vm: &Vm, id: ObjId) {
    if net {
        let held = held.remove(&id).unwrap_or(0);
        ghost::mark_record(vm, id, LINK_MASK | POSE_MASK | held);
    }
}

/// The class ids the hierarchy tests objects against, found once (at the first use, after every plugin's classes are
/// registered).
#[derive(Debug, Clone, Default)]
struct Classes {
    scene: Option<ClassId>,
    not_children: Vec<ClassId>,
}

impl Classes {
    fn of(vm: &Vm) -> Classes {
        let classes = &vm.objects().classes;
        Classes {
            scene: classes.find(b"SceneObject"),
            not_children: NOT_CHILDREN
                .iter()
                .filter_map(|n| classes.find(n))
                .collect(),
        }
    }

    fn class_of(vm: &Vm, id: ObjId) -> Option<ClassId> {
        vm.objects().get(id).map(|o| o.class())
    }

    fn is_scene(&self, vm: &Vm, id: ObjId) -> bool {
        let classes = &vm.objects().classes;
        self.scene
            .zip(Classes::class_of(vm, id))
            .is_some_and(|(scene, class)| classes.is_a(class, scene))
    }

    fn may_be_child(&self, vm: &Vm, id: ObjId) -> bool {
        let classes = &vm.objects().classes;
        Classes::class_of(vm, id)
            .is_none_or(|class| !self.not_children.iter().any(|&c| classes.is_a(class, c)))
    }
}

/// The poses and nodes of a server's objects, as the tree asks for them.
struct VmHost<'a> {
    vm: &'a Vm,
    classes: &'a Classes,
}

impl Host for VmHost<'_> {
    fn pose(&self, object: Node) -> Option<Pose> {
        let id = object.0;
        if !self.classes.is_scene(self.vm, id) {
            return None;
        }
        Some(Pose {
            world: matrix_of(self.vm, id)?,
            scale: scale_of(self.vm, id),
        })
    }

    fn node_matrix(&self, object: Node, node: u16) -> Option<Matrix> {
        links::node_matrix(self.vm, object.0, node)
    }
}

/// The host of a delete of an object the tree does not hold: it asks nothing.
struct Unheld;

impl Host for Unheld {
    fn pose(&self, _: Node) -> Option<Pose> {
        None
    }

    fn node_matrix(&self, _: Node, _: u16) -> Option<Matrix> {
        None
    }
}

/// Whether `id` is going: being removed, or its delete held until a method of it returns.
fn dying(vm: &Vm, id: ObjId) -> bool {
    vm.objects()
        .get(id)
        .is_some_and(|o| o.is_removing() || o.delete_pending())
}

/// Whether two transforms are the same pose within a few units in the last place (a pose the tree composed back from the
/// object's own write): the rotations to 1e-6, the translations to four `f32` steps of their size or 1e-5 units.
fn same_pose(a: &Matrix, b: &Matrix) -> bool {
    (0..12).all(|i| {
        let tolerance = if i % 4 == 3 {
            (4.0 * f32::EPSILON * a[i].abs().max(b[i].abs())).max(1e-5)
        } else {
            1e-6
        };
        (a[i] - b[i]).abs() <= tolerance
    })
}

/// A child a resolve writes: where its parent's motion took it, and whether that motion marks no update of its record
/// ([`links::carry_quiet`]).
#[derive(Debug, Clone, Copy)]
struct Carry {
    id: ObjId,
    world: Matrix,
    quiet: bool,
}

/// The hierarchy of one server's script engine. Cheap to clone; [`ServerHierarchy::of`] finds it.
///
/// Every change goes through the server: no client can parent anything, and a game that lets players attach things does it in
/// its own permission-checked server command. Callbacks (`onParentChanged`, `onChildAdded`, `onChildRemoved`) wait for a point
/// where scripts may run: the end of each console method of this plugin, and the end of each tick.
#[derive(Clone, Default)]
pub struct ServerHierarchy(Rc<Shared>);

#[derive(Default)]
struct Shared {
    state: RefCell<State>,
    classes: OnceCell<Classes>,
    /// Scratch of a resolve: the children it writes, then the poses their classes kept.
    writes: RefCell<Vec<Carry>>,
    kept: RefCell<Vec<(ObjId, Matrix)>>,
    /// A resolve is writing transforms: the writes it makes are its own.
    resolving: Cell<bool>,
    /// The tree resolved poses no resolve has written yet.
    unapplied: Cell<bool>,
    /// Moves whenever a link is made or taken off ([`GhostBlocks::revision`]).
    revision: Cell<u64>,
}

impl fmt::Debug for ServerHierarchy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.0.state.borrow();
        f.debug_struct("ServerHierarchy")
            .field("objects", &state.tree.as_ref().map_or(0, Hierarchy::len))
            .field("stats", &state.stats)
            .finish()
    }
}

impl ServerHierarchy {
    /// Makes a hierarchy for `vm` (a server's), plugged into its scene, its clients told of it as `clients` says.
    pub fn install(vm: &mut Vm, clients: Clients) -> ServerHierarchy {
        let hierarchy = ServerHierarchy::default();
        let net = clients == Clients::Links;
        hierarchy.state_mut().net = net;
        vm.set_extension(Rc::new(hierarchy.clone()));
        links::install(vm, Rc::clone(&hierarchy.0) as Rc<dyn SceneLinks>);
        if net {
            torque_server::net::blocks::install(vm, Rc::clone(&hierarchy.0) as Rc<dyn GhostBlocks>);
        }
        hierarchy
    }

    /// The hierarchy of `vm`, when the server has one.
    pub fn of(vm: &Vm) -> Option<ServerHierarchy> {
        vm.extension::<ServerHierarchy>().map(|h| (*h).clone())
    }

    fn state(&self) -> std::cell::Ref<'_, State> {
        self.0.state.borrow()
    }

    fn state_mut(&self) -> std::cell::RefMut<'_, State> {
        self.0.state.borrow_mut()
    }

    fn classes(&self, vm: &Vm) -> &Classes {
        self.0.classes(vm)
    }

    /// The parent node `name` names on `parent`: the origin for an empty name, a mount slot `0..31` or a node's name.
    ///
    /// # Errors
    /// [`LinkError::NoShape`] when `parent` has no shape, `BadNode` when it has no such node.
    pub fn node(vm: &Vm, parent: ObjId, name: &str) -> Result<NodeRef, LinkError> {
        if name.is_empty() {
            return Ok(NodeRef::Origin);
        }
        match links::node_index(vm, parent, name.as_bytes()) {
            NodeLookup::Found(node) => Ok(NodeRef::Node(node)),
            NodeLookup::Missing => Err(ParentError::BadNode.into()),
            NodeLookup::NoShape => Err(LinkError::NoShape),
        }
    }

    /// Hangs `child` on `parent`, or takes it off with `None`, keeping its world pose or its local one. The child then ticks
    /// after its parent; callbacks run at the next point scripts may.
    ///
    /// # Errors
    /// A [`LinkError`]: `unknown` when either is not a scene object, `removing` when either is going, `class` for a prop,
    /// `mounted` for a mounted child (a SpotLight's mount too), `cycle` when the parent rides the child through any chain of
    /// mounts and parents, and the tree's refusals. Nothing changes then.
    pub fn set_parent(
        &self,
        vm: &mut Vm,
        child: ObjId,
        parent: Option<ParentRef>,
        keep: Keep,
    ) -> Result<(), LinkError> {
        let result = self.try_set_parent(vm, child, parent, keep);
        self.state_mut().last_error = result.err();
        result
    }

    fn try_set_parent(
        &self,
        vm: &mut Vm,
        child: ObjId,
        parent: Option<ParentRef>,
        keep: Keep,
    ) -> Result<(), LinkError> {
        let classes = self.classes(vm);
        let ids = std::iter::once(child).chain(parent.map(|p| p.object.0));
        if ids.clone().any(|id| !classes.is_scene(vm, id)) {
            return Err(ParentError::Unknown.into());
        }
        if ids.clone().any(|id| dying(vm, id)) {
            return Err(LinkError::Removing);
        }
        if let Some(parent) = parent.map(|p| p.object.0) {
            if !classes.may_be_child(vm, child) {
                return Err(LinkError::Class);
            }
            if mount::mount_of(vm, child).is_some() || lights::spot_mount(vm, child).is_some() {
                return Err(LinkError::Mounted);
            }
            // The tree sees parents only; a chain through mounts (the parent rides the child) would close a loop too.
            if parent != child && links::carries(vm, child, parent) {
                return Err(ParentError::Cycle.into());
            }
        }
        let old = self.parent(child);
        if old == parent {
            return Ok(());
        }
        {
            let mut state = self.state_mut();
            let tree = state.tree_mut(vm);
            tree.set_parent(Node(child), parent, keep, &VmHost { vm, classes })?;
            state.link_changed(vm, child);
            let new = parent.map(|p| p.object.0);
            let old = old.map(|p| p.object.0);
            if let Some(old) = old
                && Some(old) != new
            {
                state.queue(Callback::ChildRemoved { parent: old, child });
            }
            state.queue(Callback::ParentChanged { child, old, new });
            if let Some(new) = new
                && Some(new) != old
            {
                state.queue(Callback::ChildAdded { parent: new, child });
            }
        }
        self.0.relinked();
        self.0.unapplied.set(true);
        self.order(vm, child, parent.map(|p| p.object.0));
        self.resolve(vm);
        Ok(())
    }

    /// The process-list edge of a link: a child ticks after its parent (unless the parent is the child's control object's,
    /// which ticks it), as a mounted object does.
    fn order(&self, vm: &Vm, child: ObjId, parent: Option<ObjId>) {
        match parent {
            Some(parent) if mount::controlling_object(vm, parent) != Some(child) => {
                torque_game::sim::process_after(vm, child, Some(parent));
            }
            _ => torque_game::sim::clear_after(vm, child),
        }
    }

    /// Sets a child's local pose, and how a client shows it. Written at the next resolve.
    ///
    /// # Errors
    /// `notParented` for an object with no parent; `removing` for one that is going (or whose parent is); `notFinite` or
    /// `outOfRange` for a bad pose.
    pub fn set_local(
        &self,
        vm: &mut Vm,
        child: ObjId,
        local: Local,
        blend: Blend,
    ) -> Result<(), LinkError> {
        let parent = self.parent(child).map(|p| p.object.0);
        let result = if dying(vm, child) || parent.is_some_and(|p| dying(vm, p)) {
            Err(LinkError::Removing)
        } else {
            let mut state = self.state_mut();
            let result = state
                .tree
                .as_mut()
                .ok_or(ParentError::NotParented)
                .and_then(|tree| tree.set_local(Node(child), local));
            if result.is_ok() {
                match blend {
                    Blend::Snap => state.snaps.insert(child),
                    Blend::Smooth => state.snaps.remove(&child),
                };
                state.pose_changed(vm, child);
            }
            result.map_err(LinkError::from)
        };
        self.state_mut().last_error = result.err();
        if result.is_ok() {
            self.0.unapplied.set(true);
            self.resolve(vm);
        }
        result
    }

    /// Takes every child off `id` (keeping their world pose or their local one); the number taken off.
    pub fn clear_children(&self, vm: &mut Vm, id: ObjId, keep: Keep) -> usize {
        let children = self.children(id);
        children
            .iter()
            .filter(|&&child| self.set_parent(vm, child, None, keep).is_ok())
            .count()
    }

    /// Sets whether `id` is deleted with its parent (a turret with its tank) rather than taken off it when the parent is
    /// deleted. False when as many objects are flagged as the total cap allows.
    pub fn set_delete_with_parent(&self, vm: &Vm, id: ObjId, flag: bool) -> bool {
        let mut state = self.state_mut();
        state
            .tree_mut(vm)
            .set_delete_with_parent(Node(id), flag)
            .is_ok()
    }

    /// Whether `id` is deleted with its parent.
    pub fn delete_with_parent(&self, id: ObjId) -> bool {
        self.state()
            .tree
            .as_ref()
            .is_some_and(|t| t.delete_with_parent(Node(id)))
    }

    /// Deletes `id` and every object below it, children first, whatever their flags (`deleteRecursive`); the number of
    /// objects deleted.
    pub fn delete_recursive(&self, vm: &mut Vm, id: ObjId) -> usize {
        let mut out = Vec::new();
        {
            let mut state = self.state_mut();
            let parent = state
                .tree
                .as_ref()
                .and_then(|t| t.parent(Node(id)))
                .map(|p| p.object.0);
            match state.tree.as_mut() {
                Some(tree) => tree.delete_recursive(Node(id), &mut out),
                None => out.push(Node(id)),
            }
            if let Some(parent) = parent {
                state.queue(Callback::ChildRemoved { parent, child: id });
            }
        }
        self.0.relinked();
        self.0.unapplied.set(true);
        delete_all(vm, &out)
    }

    /// `id`'s parent and where it hangs on it.
    pub fn parent(&self, id: ObjId) -> Option<ParentRef> {
        self.state().tree.as_ref()?.parent(Node(id))
    }

    /// `id`'s children in the order they were hung.
    pub fn children(&self, id: ObjId) -> Vec<ObjId> {
        self.state()
            .tree
            .as_ref()
            .map_or_else(Vec::new, |t| t.children(Node(id)).map(|c| c.0).collect())
    }

    /// `id`'s `i`th child.
    pub fn child(&self, id: ObjId, i: usize) -> Option<ObjId> {
        Some(self.state().tree.as_ref()?.child(Node(id), i)?.0)
    }

    /// How many children `id` has.
    pub fn child_count(&self, id: ObjId) -> usize {
        self.state()
            .tree
            .as_ref()
            .map_or(0, |t| t.child_count(Node(id)))
    }

    /// The top of `id`'s chain (`id` itself for a root).
    pub fn root(&self, id: ObjId) -> ObjId {
        self.state()
            .tree
            .as_ref()
            .map_or(id, |t| t.root(Node(id)).0)
    }

    /// Whether `ancestor` is above `id`.
    pub fn is_ancestor(&self, ancestor: ObjId, id: ObjId) -> bool {
        self.state()
            .tree
            .as_ref()
            .is_some_and(|t| t.is_ancestor(Node(ancestor), Node(id)))
    }

    /// `id`'s local pose (none for an object with no parent).
    pub fn local(&self, id: ObjId) -> Option<Local> {
        self.state().tree.as_ref()?.local(Node(id))
    }

    /// `id`'s world as the tree has it (written to the object at the next resolve).
    pub fn world(&self, id: ObjId) -> Option<Matrix> {
        self.state().tree.as_ref()?.world(Node(id))
    }

    /// How a client shows `id`'s last local pose.
    pub fn blend(&self, id: ObjId) -> Blend {
        if self.state().snaps.contains(&id) {
            Blend::Snap
        } else {
            Blend::Smooth
        }
    }

    /// The objects the tree holds (those with a parent or children).
    pub fn len(&self) -> usize {
        self.state().tree.as_ref().map_or(0, Hierarchy::len)
    }

    /// Whether the tree holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The last refusal of [`ServerHierarchy::set_parent`] or [`ServerHierarchy::set_local`], cleared by a success.
    pub fn last_error(&self) -> Option<LinkError> {
        self.state().last_error
    }

    /// Records `error` as the last refusal (a caller's own check that failed before the tree was asked).
    pub fn refuse(&self, error: LinkError) {
        self.state_mut().last_error = Some(error);
    }

    /// The counts of the resolves.
    pub fn stats(&self) -> Stats {
        self.state().stats
    }

    /// Checks the tree's own rules (caps, counts, links both ways).
    ///
    /// # Errors
    /// The first rule found broken.
    pub fn audit(&self) -> Result<(), Violation> {
        self.state().tree.as_ref().map_or(Ok(()), Hierarchy::audit)
    }

    /// Writes every child whose world changed (no script runs).
    pub fn resolve(&self, vm: &mut Vm) {
        self.0.resolve(vm);
    }

    /// Runs the callbacks that wait, in the order they were made; one that stops on an error is printed and the rest run.
    pub fn run_callbacks(&self, vm: &mut Vm) {
        let callbacks = std::mem::take(&mut self.state_mut().callbacks);
        let mut argv: Vec<Vec<u8>> = Vec::with_capacity(3);
        for callback in callbacks {
            let (object, name, args, count) = callback.call();
            if !vm.objects().contains(object) {
                continue;
            }
            let Some(db) = object_field(vm, object, b"dataBlock") else {
                continue;
            };
            argv.clear();
            argv.extend(
                std::iter::once(Some(object))
                    .chain(args.into_iter().take(count))
                    .map(|id| id.unwrap_or(0).to_string().into_bytes()),
            );
            if let Err(e) = vm.call_method(db, name, &argv) {
                vm.warn(format!("{}: {e}", String::from_utf8_lossy(name)));
            }
        }
    }

    /// The local pose `text` gives (`"x y z ax ay az angle"`, as `setTransform` reads it), the values it leaves out kept from
    /// `id`'s local pose now.
    pub fn local_from_text(&self, vm: &Vm, id: ObjId, text: &str) -> Option<Local> {
        let local = self.local(id)?;
        let current = local_matrix(&local);
        let m = torque_game::scene::object::matrix_from_text(
            torque_game::geometry::of(vm),
            &current,
            text.as_bytes(),
        );
        Some(Local {
            translation: [m[3], m[7], m[11]],
            rotation: torque_collide::math::plain_quat_from_matrix(&m),
            scale: local.scale,
        })
    }
}

/// The matrix of a local pose (its rotation and translation; no scale).
#[must_use]
pub fn local_matrix(local: &Local) -> Matrix {
    let mut m = quat_matrix(local.rotation);
    [m[3], m[7], m[11]] = local.translation;
    m
}

/// Deletes each object of `ids` that is still there; how many were.
fn delete_all(vm: &mut Vm, ids: &[Node]) -> usize {
    let mut deleted = 0;
    for id in ids {
        if !vm.objects().contains(id.0) {
            continue;
        }
        match vm.delete_object(id.0) {
            Ok(()) => deleted += 1,
            Err(e) => vm.warn(format!("deleting {}: {e}", id.0)),
        }
    }
    deleted
}

impl Shared {
    fn classes(&self, vm: &Vm) -> &Classes {
        self.classes.get_or_init(|| Classes::of(vm))
    }

    fn resolve(&self, vm: &mut Vm) {
        if self.resolving.get() {
            return;
        }
        let mut writes = std::mem::take(&mut *self.writes.borrow_mut());
        let mut kept = std::mem::take(&mut *self.kept.borrow_mut());
        for _ in 0..RESOLVE_ROUNDS {
            self.flush(vm, &mut writes);
            if writes.is_empty() {
                break;
            }
            self.resolving.set(true);
            kept.clear();
            for &Carry { id, world, quiet } in &writes {
                if !vm.objects().contains(id) {
                    continue;
                }
                let Some(old) = matrix_of(vm, id) else {
                    continue;
                };
                if same_pose(&old, &world) {
                    continue;
                }
                if quiet {
                    let bits = links::carry_quiet(vm, id, &old, &world);
                    if bits != 0 {
                        *self.state.borrow_mut().held.entry(id).or_default() |= bits;
                    }
                } else {
                    links::carry(vm, id, &old, &world);
                }
                // A class may keep a pose of its own (a Player upright): the tree takes the one it kept.
                if let Some(now) = matrix_of(vm, id)
                    && !same_pose(&now, &world)
                {
                    kept.push((id, now));
                }
            }
            self.resolving.set(false);
            if kept.is_empty() {
                break;
            }
            let classes = self.classes(vm);
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            if let Some(tree) = state.tree.as_mut() {
                for &(id, now) in &kept {
                    if tree
                        .set_world(Node(id), now, &VmHost { vm, classes })
                        .is_err()
                    {
                        state.stats.refused_moves += 1;
                    } else if tree.parent(Node(id)).is_some() {
                        pose_changed(state.net, vm, id);
                    }
                }
            }
        }
        writes.clear();
        kept.clear();
        *self.writes.borrow_mut() = writes;
        *self.kept.borrow_mut() = kept;
    }

    /// Flushes the tree and lists in `writes` the children it moved with their new world, quiet for a child whose clients
    /// draw it from its parent's ghost.
    fn flush(&self, vm: &Vm, writes: &mut Vec<Carry>) {
        self.unapplied.set(false);
        writes.clear();
        let classes = self.classes(vm);
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let Some(tree) = state.tree.as_mut() else {
            return;
        };
        if tree.dirty_count() > 0 {
            let flush = tree.flush(&VmHost { vm, classes }, state.budget);
            state.stats.carried += flush.carried as u64;
            state.stats.node_misses += flush.node_misses as u64;
        }
        state.moved.clear();
        tree.drain_moved(&mut state.moved);
        let net = state.net;
        writes.extend(state.moved.iter().filter_map(|id| {
            let parent = tree.parent(*id)?.object.0;
            // Quiet only when every client can draw it on its parent; else its record goes on carrying its pose.
            let quiet = net && ghost::ghosted_everywhere(vm, parent);
            Some(Carry {
                id: id.0,
                world: tree.world(*id)?,
                quiet,
            })
        }));
    }

    /// A link was made or taken off.
    fn relinked(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
    }
}

impl SceneLinks for Shared {
    fn pending(&self) -> bool {
        self.unapplied.get()
            || self
                .state
                .try_borrow()
                .is_ok_and(|s| s.tree.as_ref().is_some_and(|t| t.dirty_count() > 0))
    }

    fn resolve(&self, vm: &mut Vm) {
        Shared::resolve(self, vm);
    }

    fn written(&self, vm: &Vm, id: ObjId) {
        if self.resolving.get() {
            return;
        }
        // Held only by the hierarchy's own calls, which write no transform while they hold it.
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        let state = &mut *state;
        let Some(tree) = state.tree.as_mut() else {
            return;
        };
        // The tree is made at the first link, after every plugin's classes are registered.
        let classes = self.classes(vm);
        let node = Node(id);
        let Some(pose) = tree.pose(node) else {
            return;
        };
        let (Some(world), scale) = (matrix_of(vm, id), scale_of(vm, id)) else {
            return;
        };
        // Bit for bit: the tree's copy of the last write, so any change at all is heard.
        if pose.world == world && pose.scale == scale {
            return;
        }
        let host = VmHost { vm, classes };
        let moved = (pose.scale == scale || tree.set_scale(node, scale).is_ok())
            && tree.set_world(node, world, &host).is_ok();
        if !moved {
            state.stats.refused_moves += 1;
        } else if tree.parent(node).is_some() {
            pose_changed(state.net, vm, id);
        }
        if !moved && let Some(old) = tree.parent(node) {
            // A move the tree cannot hold as a local pose (too far from the parent, not finite) takes the child off its
            // parent where it is.
            if tree.set_parent(node, None, Keep::World, &host).is_ok() {
                link_changed(state.net, &mut state.held, vm, id);
                self.relinked();
                state.stats.dropped += 1;
                state.queue(Callback::ChildRemoved {
                    parent: old.object.0,
                    child: id,
                });
                state.queue(Callback::ParentChanged {
                    child: id,
                    old: Some(old.object.0),
                    new: None,
                });
            }
        }
        self.unapplied.set(true);
    }

    fn removed(&self, vm: &mut Vm, id: ObjId) {
        let node = Node(id);
        {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let Some(tree) = state.tree.as_mut() else {
                return;
            };
            if !tree.contains(node) {
                // Only its flags to forget.
                tree.delete(node, &Unheld, &mut Vec::new());
                state.snaps.remove(&id);
                state.held.remove(&id);
                return;
            }
        }
        // Children taken off keep the world they have, so it is written first.
        Shared::resolve(self, vm);
        let classes = self.classes(vm);
        let mut out = Vec::new();
        let detached: Vec<ObjId> = {
            let mut state = self.state.borrow_mut();
            let state = &mut *state;
            let Some(tree) = state.tree.as_mut() else {
                return;
            };
            let parent = tree.parent(node).map(|p| p.object.0);
            let children: Vec<Node> = tree.children(node).collect();
            tree.delete(node, &VmHost { vm, classes }, &mut out);
            state.snaps.remove(&id);
            state.held.remove(&id);
            if let Some(parent) = parent {
                state.queue(Callback::ChildRemoved { parent, child: id });
            }
            let detached: Vec<ObjId> = children
                .into_iter()
                .filter(|c| !out.contains(c))
                .map(|c| c.0)
                .collect();
            for &child in &detached {
                state.link_changed(vm, child);
                state.queue(Callback::ParentChanged {
                    child,
                    old: Some(id),
                    new: None,
                });
            }
            detached
        };
        self.relinked();
        for child in detached {
            torque_game::sim::clear_after(vm, child);
        }
        self.unapplied.set(true);
        delete_all(vm, &out);
    }

    fn parent(&self, id: ObjId) -> Option<ObjId> {
        let state = self.state.try_borrow().ok()?;
        Some(state.tree.as_ref()?.parent(Node(id))?.object.0)
    }
}

impl Shared {
    /// `obj`'s block for the dirty block bits `mask`, and the bits left for later (a link whose parent has no ghost on the
    /// connection yet).
    fn block(&self, obj: ObjId, mask: u32, ghosts: &dyn ConnectionGhosts) -> (GhostBlock, u32) {
        let Ok(state) = self.state.try_borrow() else {
            // Never while a pack runs (the hierarchy writes no transform then); sent again with the next record.
            return (GhostBlock::default(), mask);
        };
        let Some(tree) = state.tree.as_ref() else {
            return (GhostBlock::default(), 0);
        };
        let parent = tree.parent(Node(obj));
        // A client that draws the child from its own record (no parent there, or one it waits for) needs the pose bits its
        // parent's motion took back.
        let held = state.held.get(&obj).copied().unwrap_or(0);
        let mut kept = 0;
        let link = (mask & LINK_MASK != 0).then(|| match parent {
            None => LinkField::Root,
            // A parent this connection does not ghost may never reach its client (a full table, an object its peer is not
            // sent): no parent there, and the child's record sends the pose bits its parent's motion took back.
            Some(p) if !ghosts.holds(p.object.0) => {
                kept |= held;
                LinkField::Root
            }
            Some(p) => match ghosts.index(p.object.0) {
                Some(ghost) => LinkField::Parent {
                    ghost,
                    node: match p.node {
                        NodeRef::Origin => None,
                        NodeRef::Node(n) => Some(n),
                    },
                },
                None => {
                    kept |= LINK_MASK | held;
                    LinkField::Waiting
                }
            },
        });
        // A link goes with its pose, so a client never shows a new parent with an old local pose.
        let pose = (mask & (LINK_MASK | POSE_MASK) != 0)
            .then(|| tree.local(Node(obj)))
            .flatten()
            .map(|local| pose_field(&local, state.snaps.contains(&obj)));
        (GhostBlock { link, pose }, kept)
    }
}

/// A local pose as the block's pose group carries it.
fn pose_field(local: &Local, snap: bool) -> PoseField {
    let q = QuantLocal::from_local(local);
    PoseField {
        translation: q.translation,
        dropped: q.rotation.dropped,
        parts: q.rotation.parts,
        scale: (q.scale != [UNIT_SCALE_CODE; 3]).then_some(q.scale),
        snap,
    }
}

impl GhostBlocks for Shared {
    fn sections(&self) -> BlockSections {
        BlockSections { links: true }
    }

    fn write(
        &self,
        obj: ObjId,
        mask: u32,
        ghosts: &dyn ConnectionGhosts,
        w: &mut BitWriter,
        ghost_bits: u8,
    ) -> u32 {
        let (block, kept) = self.block(obj, mask, ghosts);
        block.write(w, self.sections(), ghost_bits);
        kept
    }

    fn relatives(&self, obj: ObjId, out: &mut Vec<ObjId>) {
        let Ok(state) = self.state.try_borrow() else {
            return;
        };
        let Some(tree) = state.tree.as_ref() else {
            return;
        };
        out.extend(tree.parent(Node(obj)).map(|p| p.object.0));
        out.extend(tree.children(Node(obj)).map(|c| c.0));
    }

    fn marks_on_ghost(&self, obj: ObjId, out: &mut Vec<(ObjId, u32)>) {
        let Ok(state) = self.state.try_borrow() else {
            return;
        };
        let Some(tree) = state.tree.as_ref() else {
            return;
        };
        out.extend(
            tree.children(Node(obj))
                .map(|c| (c.0, LINK_MASK | POSE_MASK)),
        );
    }

    fn height(&self, obj: ObjId) -> u32 {
        self.state
            .try_borrow()
            .ok()
            .and_then(|s| Some(s.tree.as_ref()?.height(Node(obj))))
            .map_or(0, |h| u32::try_from(h).unwrap_or(u32::MAX))
    }

    fn revision(&self) -> u64 {
        self.revision.get()
    }
}
