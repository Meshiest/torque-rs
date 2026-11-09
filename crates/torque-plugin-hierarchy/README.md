# torque-plugin-hierarchy

The scene hierarchy on a torque-rs server: any scene object hung on another at a local pose (on the parent's origin or on a
node of its shape), the server keeping each child's world where its parent puts it.

- **Server only.** The `SceneObject` methods `setParent`, `getParent`, `getParentNode`, `getChildCount`, `getChild`, `getRoot`,
  `isAncestorOf`, `getLocalTransform`, `setLocalTransform`, `clearChildren`, `deleteRecursive`, `setDeleteWithParent`,
  `getDeleteWithParent` and the function `getParentError`. No command or RPC: no client can parent anything.
- **Any class may be a child**, Players and vehicles included. A child with a simulation of its own moves in its parent's
  frame: it ticks after its parent from where the parent left it, and its velocity turns with the parent.
- **Always current.** Children are written before every container query, before each object's turn, after each step, after
  the tick hooks and before a script reads a transform.
- **Deletes.** A deleted parent's children stay where they are, except those flagged with `setDeleteWithParent`;
  `deleteRecursive` takes a whole subtree. An object being removed gains no link.
- **Mounts** stay their own relation and never close a loop with parents. Props cannot be children yet.
- **Caps** (`$Pref::Hierarchy::*`): depth 16, children 256, descendants per root 4,096, parented objects 32,768.

In the default plugin groups and on in engine mode; Torque's protocol leaves it out (`HierarchyPlugin::always()` turns it on
there too). The tree is `torque-hierarchy`; the scene's seam is torque-game's `scene::links`. The book's "Scene hierarchy"
chapter has the model; the crate docs end with a "Maintaining" section. `cargo test -p torque-plugin-hierarchy --test server`
runs the server's checks.
