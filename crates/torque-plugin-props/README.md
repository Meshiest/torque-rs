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
  `propCount`, `propAwakeCount`; `commandToServer('PropSpawn', %datablock [, %predictId])` for clients (datablocks with
  `clientSpawn = true`), checked on the server (a ray from the eye, a box that overlaps nothing) and limited to 4 a second after a
  burst of 8 per connection and per address.
- A collision source on both sides (`$TypeMasks::PropObjectType`): players, items and projectiles hit props, vehicles see them as
  static, and a player or vehicle that walks into one pushes it.
- The `Prop` ghost class, its updates held to `$Pref::Props::NetShare` (40%) of each packet and ordered for each client (touched
  by its player, then its own, then by how far each may have moved); each client's book of the props it was sent
  (`ClientProps`), drawn as shapes.

## Limits

Props per player (`$Pref::Props::MaxPerPlayer`, 64), per remote address (`$Pref::Props::MaxPerAddress`, 256, orphans counted), per
server (`$Pref::Props::MaxProps`, 1,024) and awake
(`$Pref::Props::MaxAwake`, 256): a spawn past the first two is refused; past the third the props farthest from every player sleep
first. The step is charged to `$Pref::Props::StepBudget`: a step past it does not run and the awake cap is lowered for a while. A
prop outlives its owner by `$Pref::Props::OrphanSeconds` (120). A client holds at most 4,096 props and holds every value a server
sends it to the wire's ranges.

The book's "Props and joints" chapter has the rest.

## Tests

`cargo test -p torque-plugin-props`: the quantization, the records and hostile bytes, the world's caps, sleep, freeze, pushes and
budget (`src/*_tests.rs`); a server and two clients over the memory transport (`tests/net.rs`); a client that speaks only Torque's
protocol (`tests/tge_client.rs`); hostile servers and hostile clients (`tests/fuzz.rs`, `TORQUE_FUZZ_PROPS=<rounds>`). The bench:
`cargo bench -p torque-plugin-props --bench step`. The `props-drop` scenario of `just trace-hash` hashes a server and a predicting
client with 64 props, native and wasm32.
