# Props and joints

Props are rigid bodies a server simulates and every client sees: crates to knock over, barrels to roll, rubble to kick through.
They are engine-level: any game with an interactive world can add them, not only a sandbox. `torque-plugin-props` adds them;
it is not in `DefaultPlugins`.

```rust,ignore
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_props::PropsPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, PropsPlugin))
    .build()?;
```

The server owns the truth: one persistent physics world, stepped after each tick's objects. Clients are sent each prop's state
and draw it as a shape. (Joints, and props a client predicts, come later in this chapter's plugin.)

## The physics world

The props live in one [Rapier](https://rapier.rs) world per server, built by `torque-physics-rapier`, a crate shared with the
vehicles' rigid-body solver. It is set up to give the same bits on every target, native and wasm32: Rapier's
`enhanced-determinism` (scalar math, no threads), a fixed step of 32 ms cut into `$Pref::Props::Substeps` sub-steps (4; 1 to 8,
read when the world is made), bodies and colliders inserted and walked in object-id order, no clock, and every value checked
before Rapier sees it (a value that is not finite is refused; a panic inside Rapier stops the world rather than the server).

What props land on is the mission's map: terrains (as triangles cut along the engine's own diagonal) and interiors (their convex
hulls), built in 64-unit cells the first time a prop comes near, and static shapes (`StaticShape`, `TSStatic`) as boxes of their
bounds. The server looks for new and removed map pieces every 64 ticks; a static shape a script moves is not followed.

**Quantized every tick.** At the end of each tick every awake prop's state is rounded to the wire's precision (positions to
1/1024 of a unit, rotations as the three smallest quaternion components in 15 bits each, speeds to 1/256 of a unit or radian a
second) and written back into the world. What a client receives is therefore the server's state exactly, which is what a
predicting client will restore from.

## PropData

```
datablock PropData(Crate)
{
   shapeFile = "art/crate.dts";   // what it is drawn as (empty: not drawn)
   drawScale = "1 1 1";           // the shape's scale, 0.001 to 256
   size = "1 1 1";                // the collision box, each 0.02 to 128 units
   radius = 0;                    // above 0: a ball of this radius instead
   mass = 20;                     // 0: from density and the volume
   density = 1;
   friction = 0.6;                // 0 to 10
   restitution = 0.1;             // 0 to 1
   linearDamping = 0.1;           // 0 to 100
   angularDamping = 0.3;
   maxSpeed = 100;                // units a second, at most 500
   sleepLinear = 0.1;             // sleeps after half a second slower than these
   sleepAngular = 0.2;            // (0 for either: never sleeps)
   canFreeze = true;
   maxPerPlayer = 0;              // 0: only the server's per-player cap
   clientSpawn = false;           // whether players may ask for one (scripts spawn any kind)
};

function Crate::onPropSpawn(%this, %prop) {}
function Crate::onPropSleep(%this, %prop) {}
function Crate::onPropWake(%this, %prop) {}
```

Every number is clamped to its range when it is written and again when a prop reads it; a value that is not a number reads as the
field's default. A prop collides as its `size` box (or `radius` ball), not as its shape's mesh. `PropData` is a game datablock, so
its callbacks are methods of the datablock's name.

## Console functions

All run on the server.

| Function | Does |
|---|---|
| `propSpawn(datablock, transform [, owner, predictId])` | spawns a prop at `"x y z [ax ay az angle]"`; returns its object id, or prints an error and returns 0 past a cap |
| `propDelete(prop)` | takes it away from the world and every client (`%prop.delete()` does too) |
| `propFreeze(prop)`, `propUnfreeze(prop)` | pins it where it is (others rest on it), or lets it move again |
| `propApplyImpulse(prop, "x y z")` | a push, mass times units a second |
| `propSetVelocity(prop, velocity [, spin])` | sets its speed (held to `maxSpeed`) and spin |
| `propGetVelocity`, `propGetSpin`, `propGetTransform`, `propIsAsleep`, `propIsFrozen`, `propGetOwner` | what the clients are sent |
| `propCount([client])`, `propAwakeCount()` | how many there are (a client's own), how many are awake |

A prop is a `SceneObject`: `getTransform()` and `getPosition()` read the pose the clients are sent. It is put in `MissionCleanup`
when there is one.

A client asks for a prop with `commandToServer('PropSpawn', %datablock [, %predictId])`. The server checks everything: the name is
a `PropData` with `clientSpawn = true`, the client controls a player, the point (2.5 units in front of the player's eye, never one
the client sends) is inside the world, the server's own ray from the eye reaches it (no wall, shape, vehicle, player or item in the
way), the prop's box there overlaps nothing (the map, shapes, vehicles, players, items, other props), and the caps. The command is
limited to 4 a second after a burst of 8, per connection and per remote address (so connecting again does not refill it); a
refusal prints at most one console line a second and sends the client `clientCmdPropRefused(%reason)`. No script text is built from
what a client sends: the prop is made and the client told by native calls.

## Players, vehicles and projectiles

Props are a collision source with their own type bit (`$TypeMasks::PropObjectType`) on the server and on a predicting client,
with a static shape's bit beside it: players, items and projectiles hit them as boxes, and vehicles driven by Torque's solver see
them as static. A player or vehicle whose box touches a prop pushes it toward the speed it moves at (a player's: the speed its move
asks for, since it stops at the prop it walks into), heavier props less. Props collide with each other and with the map inside
the physics world.

## Ownership

A prop's owner is the connection given to `propSpawn` (a client's command makes it the client's). The owner key is the
connection's `propOwnerKey` field when the game sets one (its own identity for the player, compared as its whole text, at most 128
bytes), else the connection itself. A prop outlives its owner's leaving for `$Pref::Props::OrphanSeconds` (120); a connection that
comes back under the same `propOwnerKey` in time takes its props back (unless two connections there claim that key: then neither
does), otherwise they go. Each prop also keeps the remote address its owner spawned it from: the players of one address share
`$Pref::Props::MaxPerAddress` (256) props between them, orphans counted, so leaving and connecting again under a new key starts no
new quota.

## Limits

| Pref | Default | Past it |
|---|---|---|
| `$Pref::Props::MaxPerPlayer` | 64 (1 to 1,024) | the spawn is refused |
| `$Pref::Props::MaxPerAddress` | 256 (1 to 1,024) | the spawn is refused |
| `$Pref::Props::MaxProps` | 1,024 (1 to 1,024) | the spawn is refused |
| `$Pref::Props::MaxAwake` | 256 (1 to 1,024) | the props farthest from every player (then the highest ids) are put to sleep |
| `$Pref::Props::StepBudget` | 4,000,000 work units | the step does not run that tick, and the awake cap drops to three quarters of what was awake; it eases back an eighth of the cap a tick |

A prop sleeps once it has been slower than its thresholds for half a second; it sends one last exact state and then costs nothing
until something wakes it (a push, a contact that moves it, a call). A panic inside the physics world (caught on native builds)
stops it: the next tick prints one warning and makes it again from the statics and each prop's last state, in id order.

## On the network

The plugin is networked: a server and a client must both have it, and the handshake refuses a client without it before any
ghost. A server that admits TGE-protocol clients sends them no prop.

Each prop is a `Prop` ghost with three groups: what a new ghost is told once (its slot, whether it is this client's own, its
predict id), its state, and its sleep and freeze flags. An update sends only what changed; a still prop sends no speeds.

**The share of each packet.** Prop updates take at most `$Pref::Props::NetShare` (40%) of the room each packet has left when its
ghosts are written; an update past it waits for the next packet, so a flood of props never crowds out the players and the rest.
Within the share a prop the client has never been sent goes first, then the most important for that client: a prop its player
touches, then one it owns, then by how far each may have moved since it was last sent (capped) and how long it has waited, nearer
before farther. A ghost's removal always goes before any prop's update. Asleep and frozen props cost nothing after their
last update. With 512 props moving at once and two clients at 32 packets a second of 450 bytes, every client's props stay within
half a unit of the server's (a test checks it).

**Hostile servers.** Everything a client is sent is held to caps: at most 4,096 props in its book, every state held to the
wire's ranges, a datablock's numbers clamped as the server's are, a shape file of at most 96 bytes drawn at a scale the renderer
accepts. A fuzzer (`TORQUE_FUZZ_PROPS=100000 cargo test -p torque-plugin-props`) feeds random records to all of it.

## Cost

`cargo bench -p torque-plugin-props --bench step`, native release build (one desktop machine, 2026-11-08), against a budget of 6 ms
for 256 awake props and 0.5 ms for 1,024 asleep:

| | 256 props awake | 1,024 props asleep |
|---|---|---|
| the world's tick (step, writeback, sleep, cap) | 0.76 ms | 0.05 ms |
| a whole server tick (the world, the record fields, the engine's own tick) | 0.86 ms | 0.13 ms |

A server whose props all sleep skips the physics step: it costs only the walk over the props.

## Tests

`cargo test -p torque-plugin-props`: the quantization's precision and stability, the records' wire form and hostile bytes, the
caps, sleep, freeze, pushes and the budget in the world alone; a server and two clients over the memory transport (states that
match the server's, sleepers that cost nothing, the share of each packet, the command's checks and rate, orphans, a client
without the plugin refused); a client that speaks only Torque's protocol; and the fuzzers. The `props-drop` scenario of
`just trace-hash` (64 crates dropped and walked through, a server and a predicting client) prints the same hash native and wasm32.
