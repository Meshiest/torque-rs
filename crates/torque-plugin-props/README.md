# torque-plugin-props

Props for a torque-rs game: crates, barrels and rubble a server simulates as rigid bodies and every client sees. The server keeps
one persistent physics world ([`torque-physics-rapier`](../torque-physics-rapier), deterministic on native and wasm32) and steps
it after each tick's objects; clients are sent each prop's state, at the wire's precision, which is the server's state exactly,
and draw it as a shape. Props are engine-level: any game with an interactive world can add them.

It is not part of the default plugins: a game adds it.

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_props::PropsPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, PropsPlugin))
    .build()?;
```

The server's scripts describe a kind of prop and spawn some:

```
datablock PropData(Crate)
{
   shapeFile = "art/crate.dts";
   size = "1 1 1";          // the collision box
   mass = 20;
   friction = 0.7;
};

%crate = propSpawn(Crate, "0 0 4");
propApplyImpulse(%crate, "0 40 0");
```

## What it adds

- `PropData` (shape, collision box or ball, mass or density, friction, restitution, damping, top speed, sleep thresholds,
  `canFreeze`, `maxPerPlayer`), every number clamped on write and on read; callbacks `onPropSpawn`, `onPropSleep`, `onPropWake`.
- The `Prop` class and `propSpawn`, `propDelete`, `propFreeze`, `propUnfreeze`, `propApplyImpulse`, `propSetVelocity`, the getters,
  `propCount`, `propAwakeCount`, and `propCheckPlacement` (a ray from the eye, a box that overlaps nothing), which a game's own
  server command calls before it spawns for a player. Props have no client entry point.
- A collision source on both sides (`$TypeMasks::PropObjectType`): players, items and projectiles hit props, vehicles see them as
  static, and a player or vehicle that walks into one pushes it.
- The `Prop` ghost class, its updates held to `$Pref::Props::NetShare` (40%) of each packet and ordered for each client (touched
  by its player, then its own, then by how far each may have moved); each client's book of the props it was sent
  (`ClientProps`), drawn as shapes.
- Joints between props, the world and players: `jointTether` (hard, or a spring past its length), `jointWinch`, `jointWeld`,
  `jointHinge` (limits, a motor) and `jointGrapple` (a Player held to a point or a prop inside its own tick, so its client
  predicts it exactly), breakable, with their setters, getters and `jointCheck` for a game's own commands; the `Joint` ghost
  class and each client's book (`ClientJoints`). With feature `rope`, `jointRope` makes a networked rope that follows a joint
  and pulls on nothing.

## Prediction

`PropPredictionPlugin` on a client predicts the props its own Player holds, stands on or pushes (with the props jointed to them, at
most 8) in a small physics world of its own, restored to the server's states and replayed through the moves the server has not
acknowledged, and draws them through a smoothed correction; the rest are drawn between the server's updates. A game shows pushes
at once on predicted props (`propPredictImpulse`, `propPredictBlast`, never sent), and spawns under a predict id
(`propPredictSpawn`). The Player's own shots push the predicted props they meet at once. It sends nothing new: no network version.

```rust
use torque_plugin_props::predict::PropPredictionPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, PropsPlugin, PropPredictionPlugin::default()))
    .build()?;
```

Projectiles push the props they hit (the `ProjectileData`'s `impulse`, else by their speed), and explosions with an
`areaImpulse` and `areaImpulseRadius` push the props around them; `PropData::onCollision` is told.

## Limits

Props per player (`$Pref::Props::MaxPerPlayer`, 64), per remote address (`$Pref::Props::MaxPerAddress`, 256, orphans counted), per
server (`$Pref::Props::MaxProps`, 1,024) and awake
(`$Pref::Props::MaxAwake`, 256): a spawn past the first two is refused; past the third the props farthest from every player sleep
first. The step is charged to `$Pref::Props::StepBudget`: a step past it does not run and the awake cap is lowered for a while. A
prop outlives its owner by `$Pref::Props::OrphanSeconds` (120). A client holds at most 4,096 props and holds every value a server
sends it to the wire's ranges.

Joints per player (`$Pref::Props::MaxJointsPerPlayer`, 64), per address (`$Pref::Props::MaxJointsPerAddress`, 256, orphans
counted), per server (`$Pref::Props::MaxJoints`, 1,024) and per prop
(`$Pref::Props::MaxJointsPerProp`, 16): a joint past one is refused. Lengths are at most 200 units, a winch's rate 20 units a
second, every force and spring held to its range; a client holds at most 4,096 joints.

The book's "Props and joints" chapter has the rest.

## Tests

`cargo test -p torque-plugin-props`: the quantization, the records and hostile bytes, the world's caps, sleep, freeze, pushes and
budget (`src/*_tests.rs`); a server and two clients over the memory transport (`tests/net.rs`); a client that speaks only Torque's
protocol (`tests/tge_client.rs`); hostile servers and hostile clients (`tests/fuzz.rs`, `TORQUE_FUZZ_PROPS=<rounds>`); each joint
kind's hold, the joint caps and what a joint follows (`tests/joints.rs`), joints over the network (`tests/joints_net.rs`), and
with `--features rope` the ropes that follow them (`tests/joint_rope.rs`). The bench: `cargo bench -p torque-plugin-props --bench
step`. The `props-drop` and `props-joints` scenarios of `just trace-hash` hash a server and a predicting client with 64 props, and
with a grappled figure among tethers, a winch, a weld and a hinge (0 corrections), native and wasm32. Prediction: `tests/predict.rs`
(about 100 ms there and back, 2% lost: a carry, a ride and a push, predicted impulses, projectiles' pushes and the client's own
shots), the `carry` and `projectile-props` scenarios of `just trace-hash`, and the replay's cost (`cargo run --release -p
torque-trace-hash --bin replay-bench`).
