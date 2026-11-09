//! The seam a scene hierarchy plugs into (the engine's parent and child transforms, a plugin's): the scene tells it when an
//! object's transform is written and when an object goes, and asks it to resolve its children before anything reads them.
//!
//! Nothing here holds a tree. A server without a hierarchy installed ([`install`]) runs exactly as Torque's: each call below
//! is one extension lookup that finds nothing.
//!
//! **Where the scene calls in.**
//! - [`written`]: every write of an object's transform ([`super::access::set_matrix`]), whoever makes it (a script's
//!   `setTransform`, a mover's tick, a mount), and every write of a static field by script (`%obj.position = ...`,
//!   [`super::object::static_written`]). A child's write becomes its new local pose; a parent's marks its children.
//! - [`resolve`]: before each container query (rays, box and radius searches, a mover's collision phase), before each
//!   object's turn in the process list, after each 32 ms step, and before a script reads a transform or a world box. It is
//!   one comparison when nothing waits.
//! - [`removed`]: `SceneObject::onRemove`, so the hierarchy lets go of the object before its id is reused.
//! - [`child_refusal`]: a class's own say in whether its objects may be hung on a parent (a plugin's [`ChildRule`]).
//! - [`mount_refused`]: mounting refuses an object that has a parent (an object is mounted or parented, never both) and a
//!   mount that would close a loop through parents and mounts ([`carries`]).
//!
//! **What the hierarchy calls back.** [`carry`] places a child where its parent's motion took it: the class's own
//! `setTransform` (a Player keeps its yaw, an Item its upright rotation), the container told, and the velocity of a Player,
//! Item or vehicle turned with the parent, so the child's own simulation goes on in its parent's frame. [`node_index`] and
//! [`node_matrix`] answer a shape's nodes for children hung on them.

use std::cell::RefCell;
use std::rc::Rc;

use collide::math::{IDENTITY, Matrix, matrix_product, mul_v, transpose};
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

use super::object;
use super::shapebase;

/// A scene hierarchy as the scene sees it. Each method takes `&self`: an implementation keeps its state behind a `RefCell`
/// and must not hold a borrow of it while it writes a transform, as that write calls [`SceneLinks::written`] again.
pub trait SceneLinks {
    /// Whether a [`SceneLinks::resolve`] has work: children whose world changed and was not written yet. Called often, so
    /// it should be a comparison.
    fn pending(&self) -> bool;

    /// Writes every child whose world changed (through [`carry`]). No script may run in it: the scene calls it from inside
    /// ticks and collision phases.
    fn resolve(&self, vm: &mut Vm);

    /// `id`'s transform or scale was written. The hierarchy's own writes from [`SceneLinks::resolve`] are among these and
    /// are its to ignore.
    fn written(&self, vm: &Vm, id: ObjId);

    /// `id` is being removed (`onRemove`). Scripts may run here, as they do in Torque's `onRemove`.
    fn removed(&self, vm: &mut Vm, id: ObjId);

    /// `id`'s parent.
    fn parent(&self, id: ObjId) -> Option<ObjId>;
}

/// The hierarchy installed in a server's script engine.
struct Installed(Rc<dyn SceneLinks>);

/// Makes `links` the scene hierarchy of `vm` (a server's), replacing an earlier one.
pub fn install(vm: &mut Vm, links: Rc<dyn SceneLinks>) {
    vm.set_extension(Rc::new(Installed(links)));
}

/// Why a [`ChildRule`] keeps an object from being a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildRefusal {
    /// Objects of the class cannot be children.
    Class,
    /// The object holds joints, which a child cannot.
    HasJoints,
}

/// A class's say in whether its objects may be children: asked before the hierarchy links one, with the parent it would hang
/// on. A leaf: no script runs in it.
pub trait ChildRule {
    /// Why `child` may not be hung on `parent` now (none: it may).
    fn refuse(&self, vm: &Vm, child: ObjId, parent: ObjId) -> Option<ChildRefusal>;
}

/// The rules of a server, in the order they were added.
#[derive(Default)]
struct ChildRules(RefCell<Vec<Rc<dyn ChildRule>>>);

/// Adds `rule` to the rules a link is checked against.
pub fn add_child_rule(vm: &mut Vm, rule: Rc<dyn ChildRule>) {
    let rules = vm.extension::<ChildRules>().unwrap_or_else(|| {
        let rules = Rc::new(ChildRules::default());
        vm.set_extension(Rc::clone(&rules));
        rules
    });
    rules.0.borrow_mut().push(rule);
}

/// The first refusal of a rule to hang `child` on `parent` (none without rules).
pub fn child_refusal(vm: &Vm, child: ObjId, parent: ObjId) -> Option<ChildRefusal> {
    let rules = vm.extension::<ChildRules>()?;
    let rules = rules.0.borrow();
    rules.iter().find_map(|r| r.refuse(vm, child, parent))
}

/// The scene hierarchy of `vm`, if one is installed.
pub fn of(vm: &Vm) -> Option<Rc<dyn SceneLinks>> {
    vm.extension::<Installed>().map(|i| Rc::clone(&i.0))
}

/// Writes the children whose world changed, if any wait (see the module docs for when the scene calls it).
pub fn resolve(vm: &mut Vm) {
    if let Some(links) = of(vm)
        && links.pending()
    {
        links.resolve(vm);
    }
}

/// Tells the hierarchy that `id`'s transform or scale was written.
pub fn written(vm: &Vm, id: ObjId) {
    if let Some(links) = of(vm) {
        links.written(vm, id);
    }
}

/// Tells the hierarchy that `id` is being removed.
pub fn removed(vm: &mut Vm, id: ObjId) {
    if let Some(links) = of(vm) {
        links.removed(vm, id);
    }
}

/// `id`'s parent in the hierarchy (none without one).
pub fn parent_of(vm: &Vm, id: ObjId) -> Option<ObjId> {
    of(vm)?.parent(id)
}

/// What carries `id` one step up: its mount, the shape a SpotLight rides, or its parent (an object has at most one of them).
fn carrier(vm: &Vm, links: &dyn SceneLinks, id: ObjId) -> Option<ObjId> {
    crate::sim::mount::mount_of(vm, id)
        .or_else(|| super::lights::spot_mount(vm, id).map(|(on, _)| on))
        .or_else(|| links.parent(id))
}

/// The longest chain of mounts and parents [`carries`] walks; a longer one counts as a loop.
const MAX_CARRIERS: usize = crate::sim::mount::SIM_MOUNT_CHAIN + 64;

/// Whether `id` carries `object`, through any chain of mounts, SpotLight mounts and parents (or is it): what would close a
/// loop if `object` came to carry `id`. False without a hierarchy (mounting keeps its own check then).
pub fn carries(vm: &Vm, id: ObjId, object: ObjId) -> bool {
    let Some(links) = of(vm) else {
        return false;
    };
    let mut at = Some(object);
    for _ in 0..MAX_CARRIERS {
        match at {
            None => return false,
            Some(a) if a == id => return true,
            Some(a) => at = carrier(vm, &*links, a),
        }
    }
    true
}

/// Whether the hierarchy refuses mounting `rider` on `mount`: the rider has a parent, or the rider carries the mount
/// (`A.mountObject(B)` while B is above A through parents and mounts). False without a hierarchy.
pub fn mount_refused(vm: &Vm, mount: ObjId, rider: ObjId) -> bool {
    parent_of(vm, rider).is_some() || carries(vm, rider, mount)
}

/// The rotation part of `m` (no translation).
fn rotation(m: &Matrix) -> Matrix {
    let mut r = *m;
    r[3] = 0.0;
    r[7] = 0.0;
    r[11] = 0.0;
    r[12..].copy_from_slice(&IDENTITY[12..]);
    r
}

/// Places `id` at `to`, where its parent's motion carried it from `from`: the class's own `setTransform` (which marks its
/// ghost as that class does), then the container's bins; a Player's, Item's or vehicle's velocity (and a vehicle's spin) turns
/// by the same turn, so a child that moves by itself keeps moving the same way in its parent's frame.
pub fn carry(vm: &mut Vm, id: ObjId, from: &Matrix, to: &Matrix) {
    let turn = matrix_product(&rotation(to), &transpose(&rotation(from)));
    // A turn within float noise of none leaves the velocity's bits as they are.
    if turn
        .iter()
        .zip(&IDENTITY)
        .any(|(a, b)| (a - b).abs() > 1e-6)
    {
        turn_velocity(vm, id, &turn);
    }
    object::set_transform(vm, id, *to);
    if let Some(container) = object::container_of(vm) {
        object::check_bins(vm, &container, id);
    }
}

/// [`carry`], with the marks the class's `setTransform` makes on the object's ghost taken back: the client draws a child
/// from its parent's pose and its local pose, so the parent's motion costs the child no update. Returns the record bits taken
/// back (bits already marked before stay marked); a hierarchy marks them again when the child's link changes, so its record's
/// own pose is current whenever a client draws the child from it.
pub fn carry_quiet(vm: &mut Vm, id: ObjId, from: &Matrix, to: &Matrix) -> u32 {
    let before = super::ghost::pending_mask(vm, id);
    carry(vm, id, from, to);
    let added = super::ghost::pending_mask(vm, id) & !before;
    super::ghost::drop_pending(vm, id, added);
    added
}

/// Turns the velocity a class keeps by `turn`.
fn turn_velocity(vm: &mut Vm, id: ObjId, turn: &Matrix) {
    if let Some(s) = crate::sim::player::state_mut(vm, id) {
        s.velocity = mul_v(turn, s.velocity);
    } else if let Some(s) = crate::sim::item::state_mut(vm, id) {
        s.velocity = mul_v(turn, s.velocity);
    } else if let Some((linear, angular)) =
        crate::sim::vehicle::state(vm, id).map(|s| (s.rigid.lin_vel, s.rigid.ang_vel))
    {
        crate::sim::vehicle::set_velocity(vm, id, mul_v(turn, linear));
        crate::sim::vehicle::set_angular_velocity(vm, id, mul_v(turn, angular));
    }
}

/// What a node name or slot number names on an object's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeLookup {
    /// The node's index in the shape.
    Found(u16),
    /// The object has a shape, without such a node (or mount slot).
    Missing,
    /// The object has no shape instance.
    NoShape,
}

/// The node `name` names on `id`'s shape: a mount slot number `0..31` (the datablock's `mountN` node) or a node's name,
/// compared without case.
pub fn node_index(vm: &Vm, id: ObjId, name: &[u8]) -> NodeLookup {
    let Some(body) = shapebase::state(vm, id).and_then(|s| s.body.as_ref()) else {
        return NodeLookup::NoShape;
    };
    let slot = std::str::from_utf8(name)
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|slot| *slot < crate::sim::mount::MOUNT_NODES as usize);
    let node = match slot {
        Some(slot) => body
            .kept
            .facts
            .as_ref()
            .and_then(|f| f.mount_nodes.get(slot).copied())
            .and_then(|n| u16::try_from(n).ok()),
        None => {
            let shape = body.shape();
            shape.find_name(name).and_then(|name| {
                shape
                    .nodes
                    .iter()
                    .position(|n| n.name == name)
                    .and_then(|i| u16::try_from(i).ok())
            })
        }
    };
    node.filter(|n| body.instance.node_transform(usize::from(*n)).is_some())
        .map_or(NodeLookup::Missing, NodeLookup::Found)
}

/// The name of node `node` of `id`'s shape.
pub fn node_name(vm: &Vm, id: ObjId, node: u16) -> Option<Vec<u8>> {
    let body = shapebase::state(vm, id)?.body.as_ref()?;
    let shape = body.shape();
    let name = shape.nodes.get(usize::from(node))?.name;
    shape.names.get(usize::try_from(name).ok()?).cloned()
}

/// Node `node` of `id`'s shape in the object's space, as its animation leaves it now (no scale applied).
pub fn node_matrix(vm: &Vm, id: ObjId, node: u16) -> Option<Matrix> {
    let body = shapebase::state(vm, id)?.body.as_ref()?;
    body.instance.node_transform(usize::from(node)).copied()
}
