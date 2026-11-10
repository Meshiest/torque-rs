# The minimal example on the engine

The same game as [`examples/minimal`](../minimal) (its scripts and its mission are read from there), a dedicated server and a client
that connect over the loopback address, built the other way: `examples/minimal` wires everything by hand on the lower crates (the
transport, the handshake, the ghost manager, the client's ghost world), this one asks an `Engine` of `DefaultPlugins` and one small
game plugin for the server, the client and the window.

```text
just examples/min-engine-server                       # a dedicated server on 127.0.0.1:28000 (--port, --ticks N via cargo run)
just examples/min-engine-client                       # connects, prints the ghosts it is sent
just examples/min-engine-window                       # the client in a window; with --local it runs the server too
```

## How much code

Lines of the files each example holds (`wc -l`), without the game's own scripts and mission (`example/main.cs` and `tiny.mis`, 76
lines, which both share):

| | `examples/minimal` | `examples/minimal-engine` |
|---|---:|---:|
| Rust in `src/` | 1681 | 310 |
| of which code (no blank or comment lines) | 1386 | 238 |
| scripts of its own | 0 | 82 (`server.cs`, `client.cs`) |
| tests | 151 | 188 |
| server | 706 (`server.rs`) + 152 (`view.rs`) + 69 (`game.rs`) | 40 (`bin/min-engine-server.rs`) |
| console client | 345 (`client.rs`) + 45 (`bin`) | 41 (`bin/min-engine-client.rs`) |
| window client | 277 (`window.rs`) + 20 (`bin`) | 94 (`window.rs`) + 27 (`bin`) |
| library root | 22 (`lib.rs`) | 108 (`lib.rs`: the plugin and three helpers) |

The two are not the same size of program. The hand-wired one is a protocol layer: it keeps its own connection table, event queues and
ghost manager and reads TGE's record codecs, and it leaves the mission phases, datablock transfer and control objects out. The twin
runs the engine's server and client, so it gets all of that (a client is sent its datablocks, then its ghosts, and is given its camera
as its control object), and the little script that asks for it (`game/server.cs`, 30 lines) is the game's own.

## What the code looks like

The server, hand-wired (`examples/minimal/src/bin/min-server.rs`, with `DedicatedServer` behind it):

```rust
let mut server = DedicatedServer::start(&options)?;
server.run(ticks, |server| {
    for note in server.take_notes() {
        match note {
            ServerNote::Console(line) => println!("console: {line}"),
            ServerNote::Connected { conn, from } => println!("client {conn} connected from {from}"),
            ServerNote::Ghosting { conn } => println!("client {conn} is ghosting"),
            ServerNote::Left { conn, reason } => println!("client {conn} left: {reason}"),
        }
    }
});
server.shutdown();
```

The server on the engine (`src/bin/min-engine-server.rs`): the game is a plugin, the server is a runner (the group's window
runner turned off, the dedicated one added).

```rust
EngineBuilder::new()
    .add_plugins((
        DefaultPlugins.disable::<WindowRunnerPlugin>(),
        DedicatedRunnerPlugin,
        ExamplePlugin(files.clone()),
    ))
    .dedicated(Dedicated { ticks, ..dedicated(&files, listen) })
    .run()?;
```

The client (`src/bin/min-engine-client.rs`) builds the same engine and makes a client from it; the connection is a script call, as in
any Torque game:

```rust
let engine = EngineBuilder::new().add_plugins((DefaultPlugins, ExamplePlugin(files()?))).build()?;
let mut client = client(&engine, ClientNet::Loopback)?;
client.call("connectTo", &[&server])?;
client.step(5);                       // every few milliseconds
ghosts(&client)                       // (class, position) of each ghost it holds
```

The window client is the same engine with the engine's window as its runner; the game is an `App` (a client, a title, and what to
draw), and `.app(..)` names it:

```rust
EngineBuilder::new()
    .add_plugins((DefaultPlugins, ExamplePlugin(files()?)))
    .app(move |engine| ExampleGame::new(engine, server))
    .run()?;
```

The game plugin (`src/lib.rs`) is where the example's own pieces go in: its files for the server and the client, and a recipe that
runs on every client script engine the engine makes.

```rust
impl Plugin for ExamplePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.insert_resource(self.0.clone());
        engine.client(ClientStage::Natives, |vm, _| {
            vm.set_global("$Example::Boxes", &MAX_BOXES.to_string());
            Ok(())
        });
    }
}
```

## Where the two differ

- **Natives.** The engine's server and client carry every native the hand-wired example's script engines have, plus what
  `DefaultPlugins` adds (the server game's, the connection's, the game classes on the client). `tests/natives.rs` checks both: its own
  snapshots (`tests/natives/`) and that nothing the hand-wired snapshots list is missing (a native's usage text may differ).
- **Same session.** `tests/loopback.rs` is the hand-wired test again over the engine: the client connects, is ghosted the crate, the gem
  and its own camera, sees the gem move on a circle of radius 4, and leaves (the server deletes its camera). `tests/window_client.rs`
  checks the window game's GUI without a window: the status line reads `3 ghosts` and three boxes are shown.
- **Shapes.** The engine's server loads the datablocks' shapes and registers no object whose datablock failed, so `files()` adds the
  asset kit's cube as `crate.dts` and `gem.dts`. The hand-wired server never loads a shape.
- **The gem.** `game/server.cs` moves the gem with `setTransform` where the game writes its `position` field: the hand-wired server
  finds the change by comparing fields, while the engine's server sends a ghost update when a transform is set.
- **The window.** The `window` feature adds what an `App` names in its signature (`torque-render-core`, `torque-render-scene`); the engine
  window itself needs no feature.
