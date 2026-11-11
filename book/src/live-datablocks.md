# Live datablocks

A datablock is read once. The objects built from it keep what they took: a Player its mass, speeds and box, a shape its
instance and hulls, a StaticShape the type bits its `dynamicType` gave it. In TGE an edited datablock is a stale object
until the mission is loaded again. Here the server notices the edit and brings the objects that use the datablock up to date
at the next tick, so a game can change a datablock while it runs. This chapter covers the server's refresh (the model, the
two kinds of change, what is refreshed for which class, the limits and the script callback) and the client's.

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

The engine's classes under `GameBaseData` (ShapeBase and StaticShape, Item, Player, Camera) refresh as above. A game's own
datablock class registers its refresh on its class behavior: `EngineClass::datablock_changed(vm, db, users, change)` is
called for the datablock's whole class chain, root first, with the users and the kind of change, and answers `true` once a
behavior took the refresh. A plugin class declared with `#[derive(EngineClass)]` gets the hook beside `preload`:

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
`CameraData` and the `ShapeBaseData` classes other than the vehicles', so a `VehicleData` or `ProjectileData` with users is
warned about. Vehicles, projectiles, the particle and explosion datablocks' clamps and the plugin datablocks are not refreshed
yet. The `users` a hook is given are live objects: a user that an earlier datablock's `onReloaded` deleted in the same tick is
left out, and a hook that deletes one itself leaves the hooks after it in the chain to check.

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

## Callbacks

- `%db.onReloaded()` runs once per tick for a datablock that was refreshed and had users, after the refresh, whatever the
  number of writes or redeclares in the tick. A datablock without users runs it never.
- `%db.onNewDataBlock(%obj)` is not run again by a refresh: the object's datablock did not change.

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
