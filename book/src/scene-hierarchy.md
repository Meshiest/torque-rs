# Scene hierarchy

Any scene object can be hung on another at a local pose: a turret on a tank, a lamp on a moving platform, a hat on a player,
a whole rigid cluster under one root. TGE has one level of mounting (a rider on a node of its mount); the hierarchy is a tree of
any depth, for every class, and leaves mounting as it is.

Two crates make it. `torque-hierarchy` is the pure tree (caps, cycle checks, the parents-first resolve, keep-world and keep-local
reparenting, the wire form of a local pose). `torque-plugin-hierarchy` puts it on the server. The plugin is in the default
plugin groups and is on when the engine speaks its own protocol; with Torque's protocol it adds nothing, so a TGE game runs as
it always did. A game that wants it there too sets `HierarchyPlugin::always()` in its group:

```rust,ignore
use torque::app::plugins::{DefaultPlugins, HierarchyPlugin};
use torque::engine::{EngineBuilder, PluginGroup};

let engine = EngineBuilder::new()
    .add_plugins(DefaultPlugins.set(HierarchyPlugin::always()))
    .build()?;
```

In the engine's own protocol the links go to every client and each client draws a child on its parent (see
[On the network](#on-the-network)). With Torque's protocol and `HierarchyPlugin::always()` the hierarchy stays on the server,
and a child's ghost carries its world pose as its class always has. Prediction over parented objects comes later.

## Parenting from a script

```
%turret.setParent(%tank, 1, "mount0");   // on the tank's mount0 node, where it is now
%lamp.setParent(%platform, 0);           // keep its pose: it moves onto the platform's origin
%hat.setParent(%player, 1, 2);           // a mount slot number names the mountN node
%hat.setParent(0);                       // off again, where it is
```

`setParent(%parent, %keepWorld = 1, %node = "")` returns 1, or 0 with `getParentError()` naming why. With `keepWorld` 1 the
object stays where it is and its local pose is rewritten; with 0 its local pose is kept (an object with no parent takes its
world transform as its local pose) and it moves with the parent. `%node` is a node of the parent's shape, by name or as a mount
slot `0` to `31`; `""` is the parent's origin. A parent with no shape takes children only on its origin.

| Method | Does |
|--------|------|
| `getParent()`, `getParentNode()` | the parent's id (0 for none), the node's name (`""` for the origin) |
| `getChildCount()`, `getChild(%i)` | the children, in the order they were hung |
| `getRoot()`, `isAncestorOf(%other)` | the top of the chain; whether the object is above `%other` |
| `getLocalTransform()`, `setLocalTransform(%xf, %snap = 0)` | the pose in the parent, as `getTransform` writes one; `%snap` shows a new pose on clients at once |
| `clearChildren(%keepWorld = 1)` | takes every child off; how many |
| `setDeleteWithParent(%flag)`, `getDeleteWithParent()` | whether the object goes when its parent is deleted |
| `deleteRecursive()` | deletes the object and everything below it, children first; how many |

`getTransform` and `setTransform` stay world space for every object: setting a child's transform rewrites its local pose.
In the engine's own protocol every local pose is rounded to the steps it travels in (1/1024 of a unit, the rotation's 15-bit
parts), so the server's world of a child is the one its clients compose; a keep-world reparent lands within that step.

The refusals `getParentError()` names: `self`, `cycle` (the parent is below the child), `depth`, `children`, `subtree` and `total`
(the caps), `notFinite` and `outOfRange` (a pose), `badNode` (no such node), `noShape` (a node named on a parent without a
shape), `unknown` (not a scene object), `notParented` (a local pose for an object with no parent), `mounted`, `removing` (either
object is being deleted, so an `onRemove` script cannot link a dying object) and `class` (a prop: see below). A refused call
changes nothing.

The datablocks hear each change: `onParentChanged(%data, %obj, %old, %new)` on the child's, `onChildAdded(%data, %obj, %child)`
and `onChildRemoved(%data, %obj, %child)` on the parent's. They run at the end of the call that made the change, or at the
end of the tick for a change the engine made (a delete, a child too far from its parent).

**Only the server parents.** The plugin adds no command or RPC, so no client can attach anything. A game that lets players
attach things (a sandbox) does it in its own server command, which checks who may, then calls `setParent`.

## Any class may be a child

Players, vehicles, items and plugin classes may be children as well as parents. A child with a simulation of its own moves in
its parent's frame: it ticks after its parent, from where its parent's turn left it; its own moves, physics and limits act on
that pose; and what it writes back becomes its new local pose. When a parent turns, the child's velocity turns with it, so a
Player walking on a turning platform keeps walking the same way on the platform. Each class keeps its own rules about its
rotation: a Player stays upright and keeps only its yaw, an Item its own upright rotation.

So a player can drive a turret that sits on another player's tank: the turret is the tank's child, the tank carries it, and
the turret's own controls turn it on the tank.

An object is mounted or parented, never both: a mounted object (a SpotLight riding a shape too) is refused (`mounted`) and
`mountObject` returns 0 for an object that has a parent. Mounts and parents never close a loop between them: a parent that
rides the child, through any chain of mounts and parents, is refused as a `cycle`, and `mountObject` returns 0 for a rider that
carries its mount. Seats stay mounts, with what mounting carries (control, `onMount`, the eye and camera rules).

Props cannot be children yet (`class`): their pose belongs to the props' physics world, and parenting them (a prop on a moving
platform, a prop joined to a dynamic parent's body) comes with its own work. A prop may be a parent.

## Scale and nodes

A child's world is its parent's world, then the node (posed as the parent's animation leaves it), then the local pose. The
parent's scale scales the translation of the node and of the local pose, never the child's own rotation or scale, as Torque
does for mounts: no shear ever appears, and the child keeps its own `scale`.

## Deleting

Deleting a parent takes its children off where they are, as TGE does for mounted objects: each keeps its world pose and
becomes a root. A child flagged with `setDeleteWithParent(1)` goes with its parent instead (a wheel with its car), and so on
down its own flagged children. `deleteRecursive()` deletes an object and every object below it, children first, whatever
their flags. Lifetime otherwise stays with SimGroups: being a child does not change which group owns an object.

## Caps

| Pref | Default | Hard cap |
|------|---------|----------|
| `$Pref::Hierarchy::MaxDepth` | 16 links below a root | 32 |
| `$Pref::Hierarchy::MaxChildren` | 256 per parent | 1,024 |
| `$Pref::Hierarchy::MaxDescendants` | 4,096 per root (what moving a root may cost) | 16,384 |
| `$Pref::Hierarchy::MaxTotal` | 32,768 parented objects | 131,072 |
| `$Pref::Hierarchy::ResolveBudget` | 131,072 children written per resolve | 131,072 |

The prefs are read when the server makes its first link. A local translation is at most 10,000 units along each axis, a scale
component between 1e-4 and 1e4, and every value finite. Script languages cannot write these prefs.

## When children move

A write to a parent's transform marks its children; nothing is walked until something needs them. The server writes the
children of moved parents:

- before every container query (`containerRayCast`, the box and radius searches, a mover's collision), so a ray cast right after
  a parent moved hits the child where it is now;
- before each object's turn in the process list, so a child ticks from where its parent's turn left it (a child ticks after its
  parent, as a mounted object does);
- after each 32 ms step, and after every `after` tick hook (a `ServerTick::resolve` hook), so the children of what a plugin
  moved there follow too;
- before a script reads a transform or a world box.

A script's write of `position`, `rotation` or `scale` as a field counts as a move too: a child's becomes its local pose, a
parent's carries its children.

A resolve walks the moved parents in a fixed order (by depth, then id, then each subtree parents first), with no hash map on
the way, so a server gives the same result on every run and every target.

From Rust, `ServerHierarchy::of(vm)` reaches the server's tree: `set_parent`, `set_local`, `parent`, `children`, `root`, `local`,
`world`, `delete_recursive`, `set_delete_with_parent`, and `audit` for a test.

## On the network

In the engine's own protocol the server sends each child's link and local pose with the child's own update, in a block that
follows the class record (the per-ghost block of `torque_ghosts::block`). The plugin lists itself in the connection manifest,
so a client without it is turned away at connect with one line ("server needs plugin torque.hierarchy 1"), before any block is
sent; Torque's protocol never carries the block, and a TGE client never sees one.

**What travels.** The block has two groups, each sent only when it changed:

- the **link**: the parent's ghost index on the connection and the node (none for the parent's origin), or that the child has
  no parent, or that it **waits**;
- the **local pose**: the translation in 1/1024 units, the rotation as three 15-bit parts of its quaternion, the child's scale
  when it is not 1, and whether the client shows it at once (`setLocalTransform(%xf, 1)`, and every link change) or blends to
  it over one tick.

A parent's motion sends nothing for its children: the client draws each child at its parent's drawn pose, times the node,
times the local pose. A moving platform with 64 crates on it costs the platform's own updates only. A child's record keeps the
world pose it had when it was linked; whenever its link changes (made, moved to another parent, taken off, its parent deleted),
the server sends its record's pose again with the new link, so a client that draws it from its record draws it where it is.

**Parents first, or the child waits.** A link names the parent by its ghost on that connection, which exists only once the
parent's first record was acknowledged. Until then the block says the link waits, the server tries again with the next
update, and the client draws the child where its own record puts it. A parent's first record is ranked a little ahead of its
children's (5/100 of a skip count for each level below it), so a new tree's parent usually arrives first; its later updates
rank as any other's. A child that arrived before its parent is drawn at its record pose for a packet or so, then on its parent.
A parent a connection will never ghost (a full ghost table, an object that client is not sent) is no parent there: that client
is told the child has none and draws it at its record pose, which follows the parent's motion.

**Scope.** Every object the server ghosts is in scope for an engine client, and an object first scoped takes its parent and its
children with it, so a resolved link never names a ghost the client lacks. When a parent's ghost goes on the client anyway (a
kill that overtook the child's update), its children stay where they were drawn until their own next update.

**What a client refuses.** Each link runs through the same checks a server's tree makes (no cycle, no self link, the hard
depth, children, descendants and total caps), and each pose through the quantizer's range check; a refused link or pose is
counted and the ghost is drawn at its record pose. A link to a ghost index the client does not hold waits at most 5 seconds,
and at most 4,096 wait at once; it is dropped when a ghost at that index goes, so it never lands on the next ghost given the
index. `ClientWorld::links` holds a client's links, `ClientWorld::world_pose(index, nodes)` gives where
a ghost is, through the hierarchy, and the renderer draws children the same way (`torque_render_scene::mounts`).

## Ropes and joints on children

Anything that hangs from a ghost follows where the client resolves it, not the pose the ghost's own record last carried (a
child's record keeps the pose it had when it was linked, because its parent's motion sends nothing). A rope end hung from a
child, a grandchild or a joint's rope end therefore moves with the parents, frame after frame. A plugin asks
`ClientGhostLookup`, which answers through `ClientWorld::world_pose`; a client that draws no frame reads the ghost's resolved
transform there and turns a rope end's offset by it, taking a parent's node as the parent's origin (it holds no shapes to pose),
so with a parent hung on a node of its parent such an end can differ from where the renderer draws the shape. A client that does
draw the ghost uses the pose the renderer drew, nodes included.

On the server a joint's anchor on a Player is read when the props' tick hook runs, after the hierarchy has written any child
whose parent moved since its last resolve, so a grapple's tether on a Player standing on a moving platform measures from the
platform's pose of this tick.
