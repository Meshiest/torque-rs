# Live datablocks

A datablock is read once. The objects built from it keep what they took: a Player its mass, speeds and box, a shape its
instance and hulls, a StaticShape the type bits its `dynamicType` gave it. In TGE an edited datablock is a stale object
until the mission is loaded again. Here the server notices the edit and brings the objects that use the datablock up to date
at the next tick, so a game can change a datablock while it runs. This chapter covers the server's refresh, the rounds that
send what changed to connected clients, and the client's refresh.

## The model

Every datablock the server registered is watched in two ways:

- **A field write** (`%db.mass = 200;`, from any script) marks the datablock *written*. The server's static-write observer
  sees every such write; a write to a datablock that is still being declared for the first time has no users and marks nothing.
- **A redeclare** (`datablock PlayerData(Soldier) { ... };` for a name that exists, which keeps the object and its id and
  runs the class's `preload` again) marks it *preloaded*: its shape file may have been read again, its clamps recomputed. A
  preloaded mark outranks a written one.

The marks are kept in one set, so a script that writes a field a thousand times a tick, or declares the same datablock a
thousand times, leaves one mark. Once per server tick, after the scripts the tick's packets ran and before any object steps,
the set is **drained**: for each marked datablock that still has users, the objects whose `dataBlock` it is are refreshed by
the datablock class's behavior; then the datablock's script `onReloaded()` runs. A datablock that no object uses takes no mark
at all: every datablock counts its users (an object's add, `setDataBlock` and removal move the count), a change to one with a
count of zero costs the count read and nothing else (an object made after the change is built from the changed datablock), and
the drain scans the process list for the users only when a marked datablock has one. A mission load, which declares its
datablocks and the objects on them before the first tick, marks, scans and rebuilds nothing.

## The two refreshes

A **written** mark runs the *light* refresh: the fields an object copied from the datablock. A Player takes its mass, drag,
speeds, recharge rate (only when the datablock's value changed, so a `setRechargeRate` stands otherwise) and box, and its damage
and energy are scaled to the new maxima as `onNewDataBlock` scales them for a new datablock; another ShapeBase takes its
recharge rate; a StaticShape and an Item swap the type bits their `dynamicType` gave them for the new ones. Nothing is rebuilt:
ten thousand writes to a PlayerData with a hundred Players rebuild no shape and cost one light refresh per Player.

A **preloaded** mark runs the *heavy* refresh: the class's whole `onNewDataBlock` work again for each user, the same code a
`setDataBlock` runs, so a redeclare that names another shape file gives every user the new shape instance, object box and hulls.
Script threads that were playing (`playThread`) play the same sequence of the new shape, found by name, from the position they
were at; a sequence the new shape lacks stops. A Player's arm action is chosen again by name, and its action thread is picked
again by the next tick's movement, as after a `setDataBlock`.

The oracle for both is differential: an object refreshed in place must equal, field for field, an object built fresh from the
edited datablock. The engine's tests hold that over hundreds of seeded edits of the shape file, mass, box, speeds and
`dynamicType` for StaticShape, Item and Player (`crates/torque-server/tests/datablock_reload.rs`).

What a written `shapeFile` does: nothing until the datablock is declared again. The file is read by `preload`, which a field
write does not run; a redeclare with the new name does.

## Which classes

The engine's classes under `GameBaseData` (ShapeBase and StaticShape, Item, Player, Camera, the vehicles, Projectile) refresh
as above:

- **Vehicles.** A written `WheeledVehicleData` or `FlyingVehicleData` gives each vehicle its rigid body's words again: mass,
  `1/mass`, inertia (a wheeled vehicle's from `massBox`, or the object box), center of mass, friction and restitution. The pose
  and the momenta are kept, so the velocity follows the new mass at the next step (as a client's refresh keeps momenta, not
  velocity). A redeclare runs the whole `onNewDataBlock` again: the shape, the object box and the body box, the wheel records
  from the new shape's `hub<i>` nodes and `spring<i>` sequences, the steering and brake-light sequences; unlike a `setDataBlock`,
  the tires and springs script fitted to the wheels are kept. Tires and springs themselves are read each tick, so a written
  `WheeledVehicleTire` or `WheeledVehicleSpring` needs no refresh.
- **Projectile.** Nothing is refreshed: a Projectile reads its `ProjectileData` each tick and caches nothing. A Projectile
  whose datablock was deleted under it deletes itself at its next tick, so no object reads a dead id and none ticks forever.
- **The checked datablocks** (`ExplosionData`, `DebrisData`, `ParticleData`, `ParticleEmitterData`, `AudioDescription`,
  `AudioProfile`) have no users to refresh; what a redeclare brings up to date is the datablock itself. TGE runs their range
  checks and clamps in `onAdd` only, so a redeclare that puts `lifetimeMS = -5` leaves it there. Here the checks run again from
  the redeclare's `preload`, and the emitter's particle list is built again from `particles`: a redeclared datablock holds the
  same clamped fields as a fresh declaration of the same values. A redeclare whose values the checks refuse (TGE deletes a new
  datablock they refuse) prints the checks' own line, and the datablock keeps its values: no object is left pointing at a
  deleted datablock.
- **Plugin datablocks.** A `PropData` refreshes its props through the hook below: each prop's numbers (size or radius, mass or
  density, friction, restitution, damping, the collision shape's hulls) are read again and its body made again from them at the
  exact pose and velocities it has, asleep or awake as it was, fixed when frozen, with its joints; a prop hung on another keeps
  its body and takes the numbers for when it is freed. `RopeData` is not refreshed: a rope is no object of the engine (it has no
  `dataBlock`, so no count and no scan finds it) and copies its parameters when it is made; a changed `RopeData` reaches the
  ropes made after it.

A game's own datablock class registers its refresh on its class behavior: `EngineClass::datablock_changed(vm, db, users,
change)` is called for the datablock's whole class chain, root first, with the users and the kind of change, and answers `true`
once a behavior took the refresh. A plugin class declared with `#[derive(EngineClass)]` gets the hook beside `preload`:

```rust,ignore
#[derive(EngineClass)]
#[class(name = "PropData", parent = "GameBaseData", preload = PropData::check, refresh = PropData::rebuild)]
struct PropData { mass: f32, /* ... */ }

impl PropData {
    fn rebuild(&self, vm: &mut Vm, id: ObjId, users: &[ObjId], change: DatablockChange) -> Result<(), String> {
        // `self` holds the datablock's fields as they are now; each of `users` is an object whose dataBlock is `id`.
        Ok(())
    }
}
```

A class with users and no refresh is stale, silently: the drain warns once per class
(`datablock class X has no refresh: its N object(s) keep what they were built with`); the engine's own refresh takes
`CameraData`, `ProjectileData` and the `ShapeBaseData` classes, the vehicles' included. The `users` a hook is given are live
objects: a user that an earlier datablock's `onReloaded` deleted in the same tick is left out, and a hook that deletes one
itself leaves the hooks after it in the chain to check.

The users are found by one pass over the process list, where every `GameBase` is. A plugin whose objects are not `GameBase`
(a prop is a `SceneObject` its own world steps) counts them itself: `scene::refresh::user_added` when one is made,
`scene::refresh::user_left` when one is gone. When a dirty datablock's count says it has more users than the pass found, one
pass over every object of the engine follows, once per drain; an object's `dataBlock` is read as an object reference or as
the integer id a plugin class declares it as.

## Limits

- One drain per tick, after the scripts and before the objects step. A drain refreshes dirty datablocks in id order until it
  has refreshed 4,096 objects (the first datablock's users are always taken, however many); the rest stay marked for the next
  tick. The refreshes run under a host work budget of one tick's pass; a refresh the budget stops leaves its datablock marked
  and the tick reports the error.
- The set holds at most one mark per live datablock, so it is bounded by the datablock id range; a mark for an object that is
  not a datablock is dropped.
- Datablocks only reach a server from its own scripts: nothing a client sends writes one.
- A StaticShape's or an Item's damage and energy are not scaled by a write to `maxDamage` or `maxEnergy` (a Player's are).
- An object that writes its `dataBlock` field directly, instead of `setDataBlock`, moves no user count (it stays counted on
  the datablock it was added with, and its removal takes it off that one), so a datablock only such objects point at takes no
  mark; when the datablock has a counted user too, the scan finds the object and refreshes it with the rest. TGE runs no
  `onNewDataBlock` for such a write either.
- A datablock whose users outgrow one tick's work budget is refreshed over several ticks, from where the last one stopped; a
  new change to it starts the refresh over, so a script that changes such a datablock every tick keeps its last users stale.
- A prop's body made again is charged to the physics steps after it, as a spawn's is: a script that edits a `PropData` with a
  thousand props every tick slows those steps and is bounded by the step budget and the props' caps.
- A deleted datablock under live objects: a Projectile deletes itself; a prop keeps the numbers it copied and is unchanged; a
  vehicle runs on with the datablock as its last tick read it, a Player with its last products, an Item with what it was built
  with (the user counts and the checked removal are the removal chapter's work).

## Callbacks

- `%db.onReloaded()` runs once per tick for a datablock that was refreshed and had users, after the refresh, whatever the
  number of writes or redeclares in the tick. A datablock without users runs it never.
- `%db.onNewDataBlock(%obj)` is not run again by a refresh: the object's datablock did not change.

## Sending what changed

A datablock is a script object every client needs a copy of before it can show the objects that use it: a player's shape and
speeds, an item's icon, a sound's file. In TGE the server sends them once, at a mission's start, as the chain
`transmitDataBlocks` runs, and a datablock declared or edited while the mission runs stays on the server until the next
mission. torque-rs keeps that chain exactly as it is, in both network protocols, and adds what a running game needs: a way to
send what changed to the clients that are already connected, with a stock Torque client taking it as well as the engine's own.

Every datablock carries a `modifiedKey`, as TGE's does: a counter the server bumps when the datablock is added and at every
script write to one of its fields. The mission chain sends a connection the datablocks whose key is above the connection's
own, then the `DataBlocksDone` message, and the client's answer runs `GameConnection::onDataBlocksDone(%client, %sequence)`,
which the stock scripts use to start the mission's phases.

## Live rounds

A round is triggered, never automatic: a server script that declared or edited datablocks asks for one, and nothing changes
on the wire until it does.

```
// A new weapon mid-game, then the clients get it.
datablock ItemData(RocketLauncherItem) { ... };
datablock AudioProfile(RocketFireSound) { description = AudioClose3D; ... };
%reached = updateDataBlocks();

function GameConnection::onDataBlocksUpdated(%client, %count)
{
   // %count datablocks reached this client: refresh what lists them.
}
```

`updateDataBlocks()` starts a live round on every connection past its mission chain and returns how many it reached. Each
connection remembers, per datablock, the key it was last sent a body with (the mission chain fills that table too), so a round
sends exactly the datablocks whose key differs from it: an edited one once, a new one once, an unchanged one never. The
datablocks the connection never had go first, in group order, then the changed ones in ascending key order. A referrer that
names a datablock the client already has resolves whatever the order; one edited to name a new datablock follows it, so the
client's preload finds the target even when the target was written again after the referrer (and so carries the larger key).
The events are the same `SimDataBlockEvent`s as the mission's, with the index and total counted within the round, so a stock
Torque client adds and updates its datablocks in place as the events come.

One chain runs per connection at a time. A round asked for while a chain runs (the mission's, or an earlier round) waits for
its end and every further ask until then is the same round; a `transmitDataBlocks` that was queued starts first. A connection
still downloading its mission's datablocks is not sent a round: its own chain, and the round queued behind it, carry whatever
changed in the meantime, so an edit made during a download is never lost. A connection that has not had its mission chain yet
is not reached. A round with nothing to send sends nothing.

The round's `DataBlocksDone` message carries a sequence of its own, so the client's answer runs
`GameConnection::onDataBlocksUpdated(%client, %count)` and never `onDataBlocksDone` again. On the client,
`onDataBlockObjectReceived(%index, %total)` still runs per event, and when the client's scripts define `onDataBlocksChanged()`
it runs after each round, so a list built from `getDataBlock(%i)` can refresh. A client whose script engine is shared with its
server (a local game) is sent the round like any other but never writes the server's own datablock from it.

### Limits on the client

A server, honest or not, cannot grow or busy a client's datablocks without bound:

- The client keeps at most `$Pref::Client::MaxDataBlocks` datablocks (1,024 in Torque's protocol and 4,096 in the engine's when
  the pref is unset), and never more than the wire's id range. An event that would add one past the cap is refused and counted;
  an update of a datablock the client has is always taken.
- After the first `DataBlocksDone`, datablock bodies spend tokens of a bucket: 256 a second with a burst of one id range, so a
  round that sends every datablock once is never cut. A body with no token is refused and counted, and refusals that go on for
  two seconds without the bucket filling back end the connection with the reason `datablock flood`.
- A body larger than one packet, and an id outside the wire's range, drop the connection as before.

The counts are in the client state's `datablocks_refused`.

## The two protocols

| | Torque's protocol | The engine's protocol |
|---|---|---|
| Edit in place and resend | yes, no wire change | yes, the same events |
| Add a datablock mid-game | yes, within the 1,021 ids of a mission | yes, within the id range |
| Who takes it | a stock Torque client and the engine's | the engine's clients |

## Client refresh

A client keeps a datablock in two places: the connection's world holds the **record** as the event carried it (what the
renderer, the collision world and the predictors read), and the script engine holds the **object** made of it (what scripts
and the plugins that read fields through the engine see). Each has a generation.

- `ClientWorld::datablock_generation(id)` moves when an event carries a record that **differs** from the one held (a record
  sent again unchanged costs one comparison and moves nothing); `ClientWorld::datablock_epoch` moves with any of them. Every
  cache built from a record keeps `(id, generation)` and is refreshed when the generation moved:
  - the predicted Player's tuning (`Predicted::refresh_tuning`): mass, drag, speeds, box and recharge rate, as the server's
    own Player takes a new datablock. Its position, velocity and the rest of what the moves made stay the same to the bit.
  - a driven vehicle's and a ghost vehicle's tuning (`DrivenVehicle::refresh_tuning`): the datablock's data and wheel
    records, the rigid body's mass, inertia, friction, restitution and center of mass, the drag and recharge rate, the object
    box when the shape changed; the pose, the momenta and the wheels' motion stay. Each wheel's tire and spring are
    generations of their own records (`refresh_wheels`).
  - a Player's live pose (its shape instance and animation threads) is made again when its datablock moved, even when the
    new shape has the same node count.
  - the scene's preload loads the shapes and textures a changed datablock names, so a new shape is not read in the frame
    of its first use.
- `torque_client::datablocks::DatablockObjects` (a script-engine extension) keeps the objects' generations: moved when an
  event wrote the object, which can be a tick or more after the record arrived (a tick takes a bounded number of events).
  A plugin that copies an object's fields (the props' numbers, `PropBook::refresh_params`) keys the copy on this one, so it
  never records a copy made from an object not yet written.

The refresh work is spread: at most `MAX_REFRESHES_PER_FRAME` (64) ghost vehicles, live poses and props are brought up to date
in one frame or tick; the rest wait for the next one, and a generation is taken as seen only once nothing waits.

A record of another class under a predictor's id, or one whose shape the client cannot read, leaves the predictor's tuning as
it was and takes the generation (the record is not read again until it changes); a shape still loading is tried again on the
next tick.

On the script engine a local client shares with its server, the object at a datablock's id is the server's own. An update
event preloads it but writes nothing into it: the texts the client read off the wire are the wire's roundings of the
server's values, and a server that refreshes its objects on every write would take its own event for an edit. The client's
record store takes the record as on any client. An object the client did not make from an event (a script's own datablock at
that id) is treated the same way.

A server that resends its datablocks mid-play costs a client one record comparison per event; the events of a tick are
capped, every refresh is budgeted per frame, and the tables added (the generations by id) hold only ids the world's record
table and the script engine's datablock range already hold.
