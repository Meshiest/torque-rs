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
and draw it as a shape. Joints hold props to each other, to the world and to players ([Joints](#joints)). (Props a client
predicts come later in this chapter's plugin.)

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
| `propSpawn(datablock, transform [, owner, predictId])` | spawns a prop at `"x y z [ax ay az angle]"`; returns its object id, or prints an error and returns 0 past a cap (the owner's caps only when an owner is given) |
| `propDelete(prop)` | takes it away from the world and every client (`%prop.delete()` does too) |
| `propFreeze(prop)`, `propUnfreeze(prop)` | pins it where it is (others rest on it), or lets it move again |
| `propSleep(prop)` | puts it to sleep where it lies, stopped, until something pushes or touches it: a map's props placed at rest stay put as the map loads |
| `propRayCast(start, end [, ignore])` | the first prop a segment meets (`"prop x y z"`, or ""); props are not `containerRayCast`'s, so a game casts both and takes the nearer to see what stands in front |
| `propApplyImpulse(prop, "x y z")` | a push, mass times units a second |
| `propSetVelocity(prop, velocity [, spin])` | sets its speed (held to `maxSpeed`) and spin |
| `propGetVelocity`, `propGetSpin`, `propGetTransform`, `propIsAsleep`, `propIsFrozen`, `propGetOwner` | what the clients are sent |
| `propCount([client])`, `propAwakeCount()` | how many there are (a client's own), how many are awake |
| `propCheckPlacement(datablock, transform, eye [, ignore, reach])` | "" when a prop may be spawned there for a player, else the reason |

A prop is a `SceneObject`: `getTransform()` and `getPosition()` read the pose the clients are sent. It is put in `MissionCleanup`
when there is one.

A map's furniture is props spawned asleep where the map places them (`propSleep(propSpawn(Chair, %at))`): the level demo's
office does this for every chair, desk, table, sofa, cabinet, plant and desk item of its Blockbench model (its outliner groups
named `prop <kind> <name>`; `examples/level-demo/src/props.rs`), so the figure pushes what it walks into.

Props have **no client entry point**: no client command spawns, pushes or freezes one. Who may spawn what is the game's decision
(a sandbox mode, a game's own scripts), made in its own server commands. Before it spawns a prop for a player, a game asks
`propCheckPlacement(datablock, transform, eye [, ignore, reach])` (in Rust, `ServerProps::check_placement` with a `Placement`):
it returns "" when the prop may go there, else the reason. The checks: the datablock is a `PropData`; the transform is finite, its
rotation a turn and inside the world; the eye too; the prop is within `reach` of the eye (32 units by default); the server's own
ray from the eye reaches the prop's position (no wall, shape, vehicle, player or item in the way); and the prop's box there
overlaps nothing (the same, and other props). `ignore` is left out of the ray alone, and must be the object the server gives that
client to control (`%client.getControlObject()`), never an id the client sent: anything that is neither a player nor controlled
is refused.

`propSpawn`'s per-owner, per-address and `maxPerPlayer` caps apply only when it is given the owner (`propSpawn(Crate, %at,
%client)`); a prop with no owner counts against the server's cap alone. A game spawning for a player passes the player's
connection.

A server command is limited only when the game declares its limits: without a declaration, TGE's `serverCmd` runs every call. A
game's plugin declares the command's rate and permission once, and its script is then the command:

```rust,ignore
use torque_engine::command::CommandSpec;

engine.server_command_limits(
    "SpawnCrate",
    CommandSpec::new().rate(4, 8).requires("props.spawn"),   // 4 a second after a burst of 8, and the permission
);
```

```
declarePermission("props.spawn", "spawn props in front of you");

function serverCmdSpawnCrate(%client)
{
   %player = %client.getControlObject();
   %eye = %player.getEyePoint();
   %at = VectorAdd(%eye, VectorScale(%player.getEyeVector(), 2.5));
   %why = propCheckPlacement(Crate, %at, %eye, %player);
   if (%why !$= "")
      return messageClient(%client, '', %why);
   propSpawn(Crate, %at, %client);
}
```

No script text is built from what a player controls: props are made with the script engine's native object call, and callbacks
are called natively.

## Players, vehicles and projectiles

Props are a collision source with their own type bit (`$TypeMasks::PropObjectType`) on the server and on a predicting client,
with a static shape's bit beside it: players, items and projectiles hit them as boxes, and vehicles driven by Torque's solver see
them as static. A player or vehicle whose box touches a prop pushes it toward the speed it moves at (a player's: the speed its move
asks for, since it stops at the prop it walks into), heavier props less. Props collide with each other and with the map inside
the physics world.

**Projectiles.** A projectile's ray meets the props (any plugin's collision shapes; one with a static shape's bit as a static
object). A hit pushes the prop at the next step, at the hit point along the shot: by the `ProjectileData`'s `impulse` (newton
seconds, Torque3D's field) when it has one above 0, else by the shot's speed times 0.25. A projectile whose `ProjectileData` has
an `areaImpulse` and an `areaImpulseRadius` above 0 pushes every prop whose center lies within the radius when it explodes, away
from the explosion and falling off to nothing at the radius. Every impulse is held to 5,000 and a radius to 50. The projectile's
`onCollision(%obj, %col, ...)` names the prop as `%col`, and the prop's datablock is told after the step:
`PropData::onCollision(%this, %prop, %projectile, %pos, %normal)`. Without props (or another plugin's shapes) projectiles are
Torque's.

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

## Joints

A joint holds two things together in the props' physics world: two props, a prop and a point of the world, or (a grapple) a
player and a point or a prop. Joints are server-authoritative like props: every function below runs on the server, and there is no
client entry point. Each joint is a `Joint` object (a `SceneObject`, put in `MissionCleanup`) and a `Joint` ghost every client is
sent.

An **end** is written as text: a prop (`%prop`, its middle), a prop and a point in its own frame (`%prop SPC "0 0 0.5"`), or a
point in the world (`"x y z"`). At least one end is a prop (or, for a grapple, the player). A weld's or a hinge's second end may
be left empty for the world.

| Kind | Made with | Holds |
|---|---|---|
| tether | `jointTether(a, b, length [, owner])` | the ends at most `length` apart (0.001 to 200): a hard limit; `jointSetSpring` makes it a spring past the length instead; `jointSetLength(j, length, min)` gives it a shortest too |
| winch | `jointWinch(tether, length, rate [, maxForce])` | a tether whose length moves to `length` at `rate` units a second (at most 20); it stops shortening while it pulls with more than `maxForce`; its length now is state, sent to every client |
| weld | `jointWeld(a, b [, owner])` | two props (or a prop and the world) at the pose they have to each other now |
| hinge | `jointHinge(a, b, point, axis [, owner])` | two props (or a prop and the world) about `axis` through `point`; `jointSetLimits(j, low, high)` (radians) and `jointSetMotor(j, speed, maxTorque)` |
| grapple | `jointGrapple(player, target, length [, owner])` | a player at most `length` from a point or a prop (`%player SPC "0 0 1.2"` holds it at an offset from its position) |
| hold | `jointHold(player, prop, offset [, owner])` | a prop carried at `offset` in the player's own frame (right, forward, up from its feet), upright and facing where the player faces |

A tether's spring (`jointSetSpring(j, stiffness, damping)`: force a unit past the length, and a unit a second) pulls only past the
length and never pushes; a stiffness of 0 makes it hard again. A hanging load stretches it by its weight over the stiffness.

**Holds.** A hold carries a prop for a player: before each step the prop is given the speed and spin that take it to its
point by the step's end (at most 10 units and 20 radians a second, so a fast turn never flings it through a wall), the step's
fall made up, so walls and other props still stop it, and never into another player's box. Nothing pulls the player, and
only the holder's own collision passes through its held prop (a plugin collision shape's `passes`: the server's object or
the client's ghost of the holder), so a held prop never pushes or launches its holder while every other player meets it. A
player holds one prop, a prop has one holder, a frozen prop cannot be held, and a held prop never sleeps. A hold breaks
(`onBreak`) when its prop is frozen or ends a step more than a unit from its point (stuck behind a wall). Every player's and
vehicle's box is a frictionless kinematic body of the props' world (placed, never given a speed), so no prop, held, let go
or shoved by a held one, stays inside a player: the solver pushes it out, walls and other props in its way. The holder's own client predicts it from where each move leaves its Player (the move's
box and facing), so it is drawn at the hand with no lag. `jointDelete` lets go; a holder that goes lets go too. Who may pick up
what, from how far, is the game's: the level demo's office lets a click pick up a prop of at most 15 kg that the figure looks
at within 2.5 units (`propRayCast`, with nothing in front by `containerRayCast`), its arm held out (`setArmThread("hold")`).

**Grapples.** A player is not a physics body: it is simulated by Torque's own Player rules, and its client predicts it. A grapple
pulls inside the Player's own tick, after its move has made the velocity and before the sweep along it: the velocity is changed so
the Player ends the tick within the length (a hard grapple; a Player past it is reeled in at up to 30 units a second), or pulled
back by the spring past it. The sweep's contacts still stop the Player. Because the pull is part of the Player's step, the
predicting client replays it exactly: it is sent the grapple's point, length and spring as whole floats, and its prediction is
never corrected for it (the `props-joints` scenario of `just trace-hash` counts 0 corrections over 600 ticks of a figure running
and jumping on one). A prop at the far end is pulled the other way at the next step, by the momentum of the pair's reduced mass (the
Player's mass times the prop's over their sum, times the Player's change of speed), so a light prop follows its Player
rather than being flung past it; a frozen prop is a fixed end and is not moved. What a pull counts toward the grapple's
break force is the Player's own momentum, whatever is at the far end. A player has one grapple at a time (a
second is refused until the first is deleted, broken or not), so the server and its client always pull it by the same one. A
Player with no grapple runs Torque's step unchanged.

**Breaking.** `jointSetBreakForce(j, force)` (0: never): a joint that holds its ends with more than that (a torque counts at one
unit) at the end of a tick breaks. It holds nothing from then on, `%joint.onBreak()` is called (`function Joint::onBreak(%this)`),
and it stays, marked broken, until a script deletes it. The force is what the joint holds when the tick ends: a weight, a winch, a
motor or a pull that lasts breaks it; a knock that is over within a tick does not.

**Welds under load.** A weld gives about 2 mm in the first tick it takes a load (the solver's give and the wire's
precision), then holds: it does not creep after that (a test loads a cantilever for 600 ticks).

**What a joint follows.** A joint whose prop goes (deleted, or lost when the physics world is made again) goes with it, and so does
a grapple whose player goes. A frozen prop keeps its joints (they hold it to a fixed body), and a world made again after a caught
panic takes every joint again in id order. A sleeping prop a joint changes (a winch reels, a motor turns, a length changes) wakes.

| Function | Does |
|---|---|
| `jointDelete(j)` | takes it away from the world and every client, with its rope |
| `jointGetKind`, `jointGetLength`, `jointGetDistance`, `jointGetForce`, `jointIsAtLimit`, `jointIsBroken` | its kind (`tether`, `winch`, `weld`, `hinge`, `grapple`, `hold`), its length, the distance between its ends, the force it held, whether a tether is at its length, whether it broke |
| `jointCount([client])`, `jointCountOn(prop)` | how many there are (a client's own), how many a prop is an end of |
| `jointCheck(client, a, b, eye [, reach, kind, point, allowOtherPlayers, allowOtherOwners])` | "" when two ends may be joined for a player, else the reason |

**For a game's own commands.** Who may join what is the game's decision. Before it joins two things for a player, a game calls
`jointCheck(%client, a, b, eye, reach, kind, point)` with the eye the server gives that player: `kind` is the joint's (`tether`
when left out; a weld's or a hinge's `b` may be empty, the world), and a hinge gives its `point`. Both ends must be ends that kind
takes and things there are, they and a hinge's point within `reach` of the eye (8 by default), and the caps must allow one more
joint for that player, its address and each prop. A player at an end is checked where the server has it, and it must be the
client's own control object: one player cannot tether another unless the game passes `allowOtherPlayers`. A prop at an end must be
the client's own (by its owner key) or no one's: one player cannot join another's props unless the game passes `allowOtherOwners`.
Both are off by default; in Rust, `ServerProps::check_joint` takes a `JointCheck` whose `players` and `props` are `Whose::Own` or
`Whose::Anyone`. A command that makes joints is declared with its rate and permission like any other
(`CommandSpec::new().rate(4, 8).requires("props.join")`).

**Caps.**

| Pref | Default | Past it |
|---|---|---|
| `$Pref::Props::MaxJointsPerPlayer` | 64 (1 to 1,024) | the joint is refused |
| `$Pref::Props::MaxJointsPerAddress` | 256 (1 to 1,024) | the joint is refused (the players of one address share it, orphans counted) |
| `$Pref::Props::MaxJoints` | 1,024 (1 to 1,024) | the joint is refused |
| `$Pref::Props::MaxJointsPerProp` | 16 (1 to 64) | the joint is refused |

A joint's owner is kept as a prop's is (its key, its remote address): an owner who leaves leaves its joints orphans, it takes
them back if it comes back under the same key within `$Pref::Props::OrphanSeconds`, and past that they go.

Lengths are at most 200 units and a winch's rate 20 units a second; springs, forces, torques and motor speeds are held to their
ranges. A value a script gives that is not a finite number is refused.

**Ropes that follow joints.** With the plugin's `rope` feature and the rope plugin on both sides, `jointRope(j, ropeData)` makes a
networked rope that shows a joint: its ends are where the joint's are as each client draws them, its length is the joint's, and it
is drawn straight while the joint is at its limit. It pulls on nothing: the joint does. It goes with the joint. Each client's rope
follows the rope driver `joint`, asked about the client's own ghost of the joint, so only the rope and its driver's name
cross the network, once.

**On the network.** A `Joint` ghost has four groups: what a new ghost is told once (its kind and its two ends: a point, or the
client's ghost of a prop or player with the point held in its frame), its length and shortest, its spring, and its flags (at its
limit, broken). An end whose prop the client has not been sent yet is sent again once it has. Joints interpolate with the props
at their ends; a client's book (`ClientJoints`) gives the props a joint ties to a prop (`connected`), for a predictor that pulls
them in with it. A client holds at most 4,096 joints and holds every value it is sent to its range; the fuzzers feed it hostile
records.

## Prediction

`PropPredictionPlugin` (a client-side plugin, which needs `PropsPlugin`) draws the props a client's own Player is playing with
where they will be, not where the last update put them. The rest are drawn between the server's updates as before.

```rust
use torque_engine::EngineBuilder;
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::predict::PropPredictionPlugin;

let engine = EngineBuilder::new()
    .add_plugins((PropsPlugin, PropPredictionPlugin::default()))
    .build()?;
```

It is a part of the client's prediction slot: it runs after whichever predictor the game picks (TGE's, the smooth one), and it
sends nothing new, so it has no network version.

**What is predicted.** Each tick: the props the Player holds (the far end of its grapple or its hold), the props under its feet and the
props it touched in the last 16 ticks, each with the props jointed to it, at most 8. **The most is a Rust setting of the
plugin** (`PredictConfig::max_predicted`, up to 32), not a pref. Frozen props are not predicted: they do not move.

**How.** The client keeps a small physics world of its own: the predicted props, the map's pieces near them (terrain and
interiors by their collision, static shapes by their box) and the props around them as fixed boxes. When the server's states
come, each predicted prop is put back to the server's state exactly (the wire's precision is the server's own) and the moves the
server has not acknowledged are replayed by the rules the server uses, inside the Player's own move loop (a correction of the
Player replays what it holds with it): the Player's push as each move left it, the grapple's pull
on the held prop as the Player's own predicted step made it, and one step a move. Between updates it steps once a tick. The replay's work is charged to the client's tick budget for prediction parts: a tick
whose work runs out draws the predicted props as the client shows them, and they are restored at the next. The
Player's own prediction meets the predicted props where the prediction has them, so walking into a prop or standing on a sliding
one is predicted no worse than not predicting.

Joints between predicted props are predicted with them: tethers and winches as tethers of their length now, welds as welds of
the pose they hold now. **A hinge is predicted as a weld** of the pose it lies at: its axis is not sent, and between two updates
the difference does not show.

**Drawing.** The simulated state takes the server's at once; what is drawn eases the jump out (it keeps 75% of it a tick; a jump
over 5 units is drawn at once). A prop that leaves the predicted set fades from where it was drawn to where the client shows it
over 4 ticks; one that joins eases from the server's state. Prediction is not bit for bit the server's (the server's physics
world holds contact caches the client never sees): it is there to draw props where they belong.

**Predicted impulses.** A game shows a push on a predicted prop at once: `PropPrediction::impulse` and `blast` in Rust, and on the
client `propPredictImpulse(%ghost, %point, %impulse)` and `propPredictBlast(%point, %radius, %impulse)` (`propPredicted()` lists
the ghosts predicted). A push is the client's own and is never sent: the server's push for the same cause (a projectile's hit,
an explosion, a game's own command) comes back in its state, and one the server never made eases out from the next update. Only
predicted props take one; a tick takes at most 8, each held to 5,000. They draw only, so they declare no effect.

**Shots.** A projectile the client's own Player fired is followed from its first record, put ahead by the moves the server has
not acknowledged; when it meets a predicted prop, the prop takes the hit's impulse at once, and the predicted props in reach take
its explosion's. A shot whose first record comes after its hit (a short one) waits for the server. **Torque's `ProjectileData`
record carries no `impulse`, `areaImpulse` or `areaImpulseRadius`**: the client reads them where its datablock has them, and
otherwise uses `PredictConfig::shot`, which a game sets to its weapons' values.

**Predicted spawns.** `PropPrediction::spawn` and `propPredictSpawn(%data, %transform)` (the datablock by its id, which the
server and the client share) show a prop at once under a predict id (1
to 255, at most 16 waiting: the scheme ropes use, `torque_engine::predict_ids`), which the game sends with its own spawn command
(`propSpawn(%data, %transform, %owner, %predictId)` on the server). The server's ghost that carries the id adopts it when it comes (each ghost once), fading
from where it was shown; one never confirmed within two seconds fades out. **A waiting spawn has no ghost, so the game draws it**
(`PropPrediction::spawns` gives each with its opacity).

**Its error and cost.** Over the memory transport with about 100 ms there and back and 2% of the datagrams lost
(`cargo test -p torque-plugin-props --test predict`): a held crate dragged on a 600-tick walk is drawn with no jump past the
server's own crate's (the largest change of its drawn step from one tick to the next is the server's), a mean correction of 2.2
cm; riding a sliding raft and pushing a row of four, 0.6 cm, with 4 corrections of the Player's own prediction against 74 without
prop prediction; a predicted push the server makes too, under 1 cm; one it never makes is back where the server has it within
the window, with no jump. A replay of 8 props through 46 moves takes 2.9 ms natively and 7.4 ms in wasm32
(`cargo run --release -p torque-trace-hash --bin replay-bench`, and its wasm32-wasip1 module under `web/wasi-run.mjs`; one desktop
machine, 2026-11-09).

## Cost

`cargo bench -p torque-plugin-props --bench step`, native release build (one desktop machine, 2026-11-08), against a budget of 6 ms
for 256 awake props and 0.5 ms for 1,024 asleep:

| | 256 props awake | 1,024 props asleep |
|---|---|---|
| the world's tick (step, writeback, sleep, cap) | 0.76 ms | 0.05 ms |
| a whole server tick (the world, the record fields, the engine's own tick) | 0.86 ms | 0.13 ms |

A server whose props all sleep skips the physics step: it costs only the walk over the props. On a client a sleeping (or
frozen) prop costs a frame almost nothing too: its draw says it rests, and the engine's ghost scene draws it as a still copy
of its shape's static batch (kept on the GPU, culled there, in the cached shadows) instead of posing and building it every
frame; a prop that wakes, or that the client's prediction moves, is a moving shape again from that frame. In the level demo's
office (197 props, all asleep) that took a frame from about 7.7 to 4.1 ms in its slowest view.

## Tests

`cargo test -p torque-plugin-props`: the quantization's precision and stability, the records' wire form and hostile bytes, the
caps, sleep, freeze, pushes and the budget in the world alone; a server and two clients over the memory transport (states that
match the server's, sleepers that cost nothing, the share of each packet, the command's checks and rate, orphans, a client
without the plugin refused); a client that speaks only Torque's protocol; and the fuzzers. The `props-drop` scenario of
`just trace-hash` (64 crates dropped and walked through, a server and a predicting client) prints the same hash native and wasm32.

Joints: `--test joints` (in the world alone: a hard tether under a 10 unit/s pull stays within 1 cm of its length every tick over
600, a soft one settles within 5% of its rest stretch, a winch reaches its target within 2% of the planned time and stalls past
its force, a weld creeps at most 1 mm over 600 ticks under load, a hinge holds its limits within half a degree, breaking, the caps,
a joint through a freeze, a new world and its prop's going), `--test joints_net` (the clients' books, a winch's length, a
grapple's tether bit for bit on the client), `--features rope --test joint_rope` (a taut joint rope straight within 1 mm, a slack
one hanging, a rope that follows its joint over the network and goes with it). The `props-joints` scenario of `just trace-hash`
(a figure running and jumping on a grapple, with a hard and a soft tether, a winch, a weld and a motored hinge around it) prints
the same hash native and wasm32, with 0 corrections of the figure's prediction.

Prediction: `--test predict` (the carry, the ride and the push, a predicted push the server makes too and one it never makes, the
caps, a projectile's push held to its cap, the shooter's own shot and its explosion). The `carry` and `projectile-props`
scenarios of `just trace-hash` (a crate carried on a grapple; shots and explosions of the figure's own at the crate it holds)
hash a client that predicts props, native and wasm32.
