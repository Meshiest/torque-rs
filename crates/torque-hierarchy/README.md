# torque-hierarchy

Parent and child transforms for any scene object. The crate is the pure core: a forest of objects, each with a local pose in its
parent (on the parent's origin or on a node of its shape), and the rules around it. It has no engine dependencies; the host
answers the poses and node matrices of its objects through the `Host` trait.

- **Tree and caps.** Depth, children per parent, descendants per root and parented objects in all are capped (defaults 16, 256,
  4,096 and 32,768, clamped to hard caps). A link that would form a cycle or pass a cap is refused with a `ParentError` and
  changes nothing. Any object may be a child or a parent: the tree has no class rules.
- **Reparenting.** `set_parent(child, parent, Keep::World | Keep::Local, host)` keeps the world pose by rewriting the local
  one, or keeps the local pose and moves the object. A parent's scale scales the translation of its node and of the child's
  local pose, never the child's rotation or scale.
- **Resolve.** Changes mark dirty roots; `flush` recomputes each dirty subtree parents-first and `drain_moved` lists the
  objects whose world changed. A budget carries whole roots to the next flush.
- **Deleting.** `delete` detaches a deleted object's children keeping their world pose and hands back the children flagged
  with `set_delete_with_parent`; `delete_recursive` hands back the whole subtree, children first.
- **Wire form.** `QuantLocal` is a local pose as whole numbers (translation in 1/1024 units, smallest-three rotation at 15 bits a component, a 16-bit
  scale code) that rounds the same on every platform; `QuantLocal::check` refuses out-of-range codes, `to_local` always gives a
  valid pose, and `Hierarchy::check_parent` and `link` are what a client runs on a server's links.

```rust
use torque_hierarchy::{Caps, Hierarchy, Keep, ObjId, ParentRef};

let mut tree = Hierarchy::new(Caps::default());
tree.set_parent(ObjId(2), Some(ParentRef::origin(ObjId(1))), Keep::World, &host)?;
tree.set_world(ObjId(1), moved_root_matrix, &host)?;
tree.flush(&host, usize::MAX);
let mut moved = Vec::new();
tree.drain_moved(&mut moved);
```

The crate docs end with a "Maintaining" section: module map, data flow, the invariants and the tests that hold them, and the
check commands. `TORQUE_FUZZ_HIERARCHY=<operations>` sets the length of the hostile-input fuzz (`tests/fuzz.rs`).
