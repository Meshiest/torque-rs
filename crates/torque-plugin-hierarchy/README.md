# torque-plugin-hierarchy

The scene hierarchy in torque-rs: any scene object hung on another at a local pose (on the parent's origin or on a node of its
shape), the server keeping each child's world where its parent puts it and, in the engine's own protocol, its clients drawing
each child on its parent.

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
- **On the network** (the engine's own protocol): each child's link and local pose ride its own update in a per-ghost block;
  a parent's motion costs its children nothing; a link waits until the parent's ghost exists, the child drawn at its own pose
  meanwhile; the plugin is in the manifest, so a client without it is refused at connect. Clients check every link and pose
  as a server's tree does.
- **Prediction.** A predicting client meets each child where the server's latest poses of its parents and its local pose put
  it, every tick, so a Player riding a hung platform is corrected no more often than on the same platform moved by itself.
  A client that controls a child (a turret on another player's tank) predicts it in its parent's frame: the server sends its
  state there, so the parent's motion never corrects it, and the client draws it on the parent as it draws the parent.

In the default plugin groups and on in engine mode; Torque's protocol leaves it out (`HierarchyPlugin::always()` turns it on
there too). The tree is `torque-hierarchy`; the scene's seam is torque-game's `scene::links`. The book's "Scene hierarchy"
chapter has the model; the crate docs end with a "Maintaining" section. `cargo test -p torque-plugin-hierarchy --test server`
runs the server's checks, `--test ride` and `--test turret` the prediction's; the network's, which need a whole client, are
`cargo test -p torque-test-app --test hierarchy_net` (tests/app).
