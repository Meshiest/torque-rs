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
and a child's ghost carries its world pose as its class always has. A predicting client meets children where the server has
them (see [Prediction](#prediction)).

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
object is being deleted, so an `onRemove` script cannot link a dying object), `joints` (a prop that holds a joint or a hold:
see [Props](#props)) and `class` (a class's own rule refuses it; the engine's classes have none). A refused call
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
rotation: a Player stays upright and keeps only its yaw, an Item its own upright rotation. A Player child's yaw is kept in its
parent's frame: its local rotation is a turn about the parent's up alone, and only its own moves turn it there (the parent's
tilt and turns carry it, they never turn it on the parent), while in the world it stands upright as any Player does.

So a player can drive a turret that sits on another player's tank: the turret is the tank's child, the tank carries it, and
the turret's own controls turn it on the tank (see [A controlled child](#a-controlled-child)).

An object is mounted or parented, never both: a mounted object (a SpotLight riding a shape too) is refused (`mounted`) and
`mountObject` returns 0 for an object that has a parent. Mounts and parents never close a loop between them: a parent that
rides the child, through any chain of mounts and parents, is refused as a `cycle`, and `mountObject` returns 0 for a rider that
carries its mount. Seats stay mounts, with what mounting carries (control, `onMount`, the eye and camera rules).

## Props

A prop may be a child, and a parent. What the physics does about it depends on the parent:

| Parent | The child prop is | Why |
|--------|-------------------|-----|
| a dynamic prop (or a part of one) | **part of its body**: its collider sits on the root's body at the local pose, and the masses add | rigidly attached with no drift and a real mass, as an assembly in other engines; a weld joint would drift and cost a constraint each |
| anything else: a platform, a door, a Player, a frozen prop, a non-prop that hangs on a prop | **driven**: a kinematic body taken to where the hierarchy puts it each tick | the parent is no body of the props' world to attach to; a prop on a moving platform works |

A part's pose in its root's frame never changes by itself: it is bit for bit what it was when the prop joined, however hard the
body is hit, and moves only when a script sets the child's local pose. A driven prop pushes the dynamic props it meets and is
pushed by none, and is within a millimeter of its resolved pose every tick. Unparented (`setParent(0)`, or its parent deleted), a
prop is a dynamic body again where it is, leaving at the speed its place gave it (the body's speed at its point, or the speed of
its last move). Freezing a body takes its parts to driven props where they are, and thawing it takes them back.

A prop that holds a **joint or a hold** cannot be hung on anything: `setParent` returns 0 and `getParentError()` says `joints`,
and nothing changes (its joint stays whole). A child cannot take a joint either, so a hold or a joint is never broken by
parenting. A child cannot be frozen, put to sleep or given a speed (`propFreeze`, `propSleep` and `propSetVelocity` return 0); a
push on a part moves its root. A child is never drawn as resting, whatever its parent does, and costs its clients no update for
its parent's motion (a client draws it on its parent). The props plugin's prediction leaves children, and the props that carry
children, to the server.

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

## Prediction

A client predicts its control object against the ghosts it holds, so a Player standing on a child (a deck hung on a moving
lift, a step on a vehicle) must meet the child where the server has it, not where the child's record was left when it was
linked or where the last frame drew it. Each prediction tick the client's collision world places every child at
`ClientWorld::tick_pose_in`: its parent's tick pose, times the parent's node, times the local pose the server last sent. What
is exact and what is not:

- **Exact:** the local pose (the server rounds every local pose to the wire's steps, so the client's is the server's to the
  bit), the node rule and the compose (the same `torque_hierarchy::compose` the server uses), and the parent's pose as its
  latest record carries it.
- **Held, not extrapolated:** the parent's motion after its latest record. The moves a correction replays all come after the
  server state it sent, so no record covers them; the client holds the parent's latest pose through the replay, as it holds a
  moving object that is no one's child. A rider is therefore corrected no more often on a hung deck than on the same deck
  moved by itself (`cargo test -p torque-plugin-hierarchy --test main ride::`), and the same figure on the same deck held still is
  never corrected.
- **Bodies the client runs itself** (the vehicle it drives, a ghost vehicle it runs ahead) carry their children from where the
  client runs them, and the client's own predicted object carries its children from where it is before each move, as the
  server places a parent's children before each object's turn.
- **Not re-placed:** the objects of a game's static index set (`StaticIndex::set_near`), which a game keeps apart from the
  ghosts; a child among them stays where the game put it.
- **Cost:** each ghost a tick's chains reach is worked out once, whatever children share it, and every link walked is charged
  to the tick's prediction budget, so the largest tree a server can send costs a tick no more than that budget.
- **Nodes at rest:** a child on a node sits on the parent's shape at rest; a parent the server animates moves its nodes there
  only.

The blend a frame shows between two local poses never reaches the collision world: a frame draws children with
`ClientWorld::world_pose`, a tick collides with `ClientWorld::tick_pose`. In Torque's protocol no link reaches a client, so its
prediction is Torque's.

## A controlled child

A client can control a child: a gunner in a turret on a tank someone else drives. The server's state of the child and the
client's prediction of it are compared each packet (the control object's checksum), and the tank's motion since the client's
latest record of it is the one thing the client cannot know. So in the engine's protocol the server sends a controlled child's
state in its parent's frame, and the client predicts it there:

- **On the wire.** The control object's packet data ends with its frame: one bit for the world's, or the parent's ghost index
  and node. In a parent's frame a Player's position is its local translation, its velocity is turned into the frame and its
  yaw is its yaw on the parent. The server names the parent only once that connection has the parent's ghost; before, the
  frame is the world's and the client predicts as before. Only a peer that is sent the per-ghost block gets a frame, and only
  a Player that sends its position (a mounted one sends none) is sent in a parent's; a peer of Torque's own gets the data a
  server with no hierarchy sends.
- **Each move.** The client places the child where the parent's tick pose and the local pose put it (the parent where its
  latest record puts it, or where the client runs it), runs the move in the world as the server does (the same Player
  simulation, against the same collision world, which holds the parent there too), and takes the new local pose back rounded
  to the wire's steps, as the server's tree rounds it (`torque_hierarchy::wire_local`). The server's resolve rounds, its
  carry of the velocity and its Player rules are the same code on both sides.
- **What is compared.** The local translation and the yaw on the parent, which the moves alone change: a parent's drive, turns
  and tilt carry the child without moving either, so the client predicts them exactly whatever the parent did since its
  latest record. The velocity is compared in the frame too; a child standing on its parent has none of its own.
- **Drawing.** The child is drawn on its parent as the frame draws the parent: interpolated when someone else drives it, the
  client's own prediction when it drives the parent itself. Its predicted pose is moved from the parent's tick pose to the
  parent's drawn pose, and so are the eye and the camera, so the turret never lags its tank by a tick.
- **Cost.** The parent's tick pose is worked out once a tick, its chain charged to the prediction budget as the placement of
  children is, from the same tick poses, so no ghost is worked out twice; a tick with no budget left for it, or a parent the
  client holds no ghost of, runs no move (`unplaced` in the predictor's counts), and the next tick that places the parent
  replays the moves that waited.

`cargo test -p torque-plugin-hierarchy --test main turret::` runs the case of the example above: one client drives the tank and a
second controls a turret on its roof, turning it and looking up and down, at latencies of 0 and 3 ticks. The gunner is
corrected no more often than for the same moves on a base that never moves (none, either way), the driver as often with the
gunner as without, and the gunner draws the turret within a millimetre of where its drawn tank and the turret's local pose
put it. The `turret` line of `just trace-hash` hashes that run. A gunner who walks about its parent compares a velocity
the parent's turns have turned, so it may be corrected now and then; a turret, which stands and aims, never is.
