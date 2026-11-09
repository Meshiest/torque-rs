//! The `SceneObject` methods that parent objects and read the tree, and `getParentError()`. A server API: the plugin
//! registers them on servers only, and adds no command or RPC a client could send.

use torque_engine::{This, VmMut, VmRef, function, methods};
use torque_hierarchy::{Keep, NodeRef, ParentError, ParentRef};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

use crate::server::{Blend, LinkError, ServerHierarchy, local_matrix};

/// The object a script argument names (an id or a name), 0 and "" for none.
fn object_arg(vm: &Vm, text: &str) -> Option<ObjId> {
    vm.objects().find(text.as_bytes()).filter(|&id| id != 0)
}

/// `0` for none, as Torque prints ids.
fn id_of(id: Option<ObjId>) -> i32 {
    id.and_then(|id| i32::try_from(id).ok()).unwrap_or(0)
}

fn keep_of(keep_world: Option<bool>) -> Keep {
    if keep_world.unwrap_or(true) {
        Keep::World
    } else {
        Keep::Local
    }
}

/// Runs `f` on the server's hierarchy, then the callbacks it queued (a console method is a point where scripts may run).
fn with<R>(vm: &mut Vm, missing: R, f: impl FnOnce(&ServerHierarchy, &mut Vm) -> R) -> R {
    let Some(hierarchy) = ServerHierarchy::of(vm) else {
        return missing;
    };
    let out = f(&hierarchy, vm);
    hierarchy.run_callbacks(vm);
    out
}

/// The methods of `SceneObject` the hierarchy adds.
pub struct ParentMethods;

#[methods(class = "SceneObject")]
impl ParentMethods {
    /// hangs the object on a parent, or takes it off with 0 or ""; true, or false with getParentError() naming why
    ///
    /// `keepWorld` (1 by default) keeps the object where it is and rewrites its local pose; 0 keeps its pose and moves it with
    /// the parent. `node` hangs it on a node of the parent's shape: a mount slot `0` to `31` or a node's name; "" is the
    /// parent's origin. A child ticks after its parent and moves in its parent's frame: its own moves, physics and limits act
    /// on its local pose, and the parent's motion carries it. A mounted object cannot be parented, nor a prop (yet), nor an
    /// object that is being removed; a parent that rides the object, through mounts or parents, is refused as a cycle.
    pub fn set_parent(
        mut vm: VmMut,
        this: This,
        parent: &str,
        keep_world: Option<bool>,
        node: Option<&str>,
    ) -> bool {
        let keep = keep_of(keep_world);
        let parent_id = object_arg(&vm, parent);
        with(&mut vm, false, |h, vm| {
            let parent = match parent_id {
                None if parent.is_empty() || parent == "0" => None,
                None => {
                    h.refuse(LinkError::Tree(ParentError::Unknown));
                    return false;
                }
                Some(object) => match ServerHierarchy::node(vm, object, node.unwrap_or("")) {
                    Ok(node) => Some(ParentRef {
                        object: torque_hierarchy::ObjId(object),
                        node,
                    }),
                    Err(e) => {
                        h.refuse(e);
                        return false;
                    }
                },
            };
            h.set_parent(vm, this.0, parent, keep).is_ok()
        })
    }

    /// the parent's id, 0 for none
    pub fn get_parent(vm: VmRef, this: This) -> i32 {
        id_of(
            ServerHierarchy::of(&vm)
                .and_then(|h| h.parent(this.0))
                .map(|p| p.object.0),
        )
    }

    /// the parent's node the object hangs on, "" for the parent's origin
    pub fn get_parent_node(vm: VmRef, this: This) -> String {
        let Some(parent) = ServerHierarchy::of(&vm).and_then(|h| h.parent(this.0)) else {
            return String::new();
        };
        match parent.node {
            NodeRef::Origin => String::new(),
            NodeRef::Node(node) => torque_game::scene::links::node_name(&vm, parent.object.0, node)
                .map(|name| String::from_utf8_lossy(&name).into_owned())
                .unwrap_or_default(),
        }
    }

    /// how many children the object has
    pub fn get_child_count(vm: VmRef, this: This) -> i32 {
        let count = ServerHierarchy::of(&vm).map_or(0, |h| h.child_count(this.0));
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// the object's child number `index` (from 0, in the order they were hung), 0 past the last
    pub fn get_child(vm: VmRef, this: This, index: i32) -> i32 {
        let child = usize::try_from(index)
            .ok()
            .and_then(|i| ServerHierarchy::of(&vm)?.child(this.0, i));
        id_of(child)
    }

    /// the top of the object's chain of parents: the object itself when it has none
    pub fn get_root(vm: VmRef, this: This) -> i32 {
        id_of(Some(
            ServerHierarchy::of(&vm).map_or(this.0, |h| h.root(this.0)),
        ))
    }

    /// whether the object is above `other` in its chain of parents
    pub fn is_ancestor_of(vm: VmRef, this: This, other: &str) -> bool {
        object_arg(&vm, other).is_some_and(|other| {
            ServerHierarchy::of(&vm).is_some_and(|h| h.is_ancestor(this.0, other))
        })
    }

    /// the object's pose in its parent as "x y z ax ay az angle", "" when it has no parent
    pub fn get_local_transform(vm: VmRef, this: This) -> String {
        let Some(local) = ServerHierarchy::of(&vm).and_then(|h| h.local(this.0)) else {
            return String::new();
        };
        let text = torque_game::scene::object::transform_text(
            torque_game::geometry::of(&vm),
            &local_matrix(&local),
        );
        String::from_utf8_lossy(&text).into_owned()
    }

    /// sets the object's pose in its parent ("x y z ax ay az angle"; the values left out are kept); true, or false with
    /// getParentError() naming why
    ///
    /// `snap` (0 by default) shows the new pose on clients at once instead of blending to it.
    pub fn set_local_transform(
        mut vm: VmMut,
        this: This,
        transform: &str,
        snap: Option<bool>,
    ) -> bool {
        with(&mut vm, false, |h, vm| {
            let Some(local) = h.local_from_text(vm, this.0, transform) else {
                h.refuse(LinkError::Tree(ParentError::NotParented));
                return false;
            };
            let blend = if snap.unwrap_or(false) {
                Blend::Snap
            } else {
                Blend::Smooth
            };
            h.set_local(vm, this.0, local, blend).is_ok()
        })
    }

    /// takes every child off the object (keeping where each is, or with `keepWorld` 0 its local pose); how many
    pub fn clear_children(mut vm: VmMut, this: This, keep_world: Option<bool>) -> i32 {
        let keep = keep_of(keep_world);
        let count = with(&mut vm, 0, |h, vm| h.clear_children(vm, this.0, keep));
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// deletes the object and every object below it, children first; how many objects went
    pub fn delete_recursive(mut vm: VmMut, this: This) -> i32 {
        let count = match ServerHierarchy::of(&vm) {
            Some(h) => {
                let count = h.delete_recursive(&mut vm, this.0);
                h.run_callbacks(&mut vm);
                count
            }
            None => usize::from(vm.delete_object(this.0).is_ok()),
        };
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// sets whether the object is deleted with its parent (a turret with its tank) instead of being taken off it where it
    /// is; false when the server holds as many flags as its cap of parented objects
    pub fn set_delete_with_parent(vm: VmRef, this: This, flag: bool) -> bool {
        ServerHierarchy::of(&vm).is_some_and(|h| h.set_delete_with_parent(&vm, this.0, flag))
    }

    /// whether the object is deleted with its parent
    pub fn get_delete_with_parent(vm: VmRef, this: This) -> bool {
        ServerHierarchy::of(&vm).is_some_and(|h| h.delete_with_parent(this.0))
    }
}

/// why the last setParent or setLocalTransform was refused: self, cycle, depth, children, subtree, total, notFinite,
/// outOfRange, badNode, unknown, notParented, mounted, noShape, removing or class; "" after a success
#[function(name = "getParentError")]
fn get_parent_error(vm: VmRef) -> String {
    ServerHierarchy::of(&vm)
        .and_then(|h| h.last_error())
        .map_or_else(String::new, |e| e.name().to_owned())
}

/// The natives, as the plugin registers them on a server.
pub(crate) fn declared() -> Vec<torque_engine::script::NativeDecl> {
    torque_engine::natives![ParentMethods, get_parent_error]
}
