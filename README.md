# torque-rs

A game engine library in Rust inspired by the Torque Game Engine and Torque3D, growing beyond their original design:
TorqueScript and the
console, SimObjects and datablocks, client/server networking with ghosting, DTS shapes, convex collision and the game
object model (players, items, projectiles, vehicles).

Games register their own systems (console classes and functions, networked object types, render passes, file formats,
resource loaders) through the engine's registries and change engine behaviour only through those hooks. Tools (editors,
debugger, telnet console, journaling) are build-time features, so a finished game can be built without them.

## Crates

| Crate | What it is |
|---|---|
| `torque-ts-text` | the C-runtime text and number rules TorqueScript relies on |
| `torque-ts-bytecode` | compiled scripts (DSO files): reading, writing, decoding; dialect profiles |
| `torque-ts-compiler` | the TorqueScript compiler |
| `torque-collide` | convexes, GJK, polygon lists, working sets, object type masks |
| `torque-shapes` | DTS shapes and DSQ sequences |
| `torque-sim` | the process list, Player, AIPlayer, Item, Projectile, ShapeBase images |
| `torque-vehicle` | rigid bodies, wheeled and flying vehicles |
| `torque-scene` | the scene container: bins, searches, a scene object's transform maths |
| `torque-resource` | the resource layer: a game folder, its zips mounted as folders, the scripts' writes in memory |
| `torque-game` | the TGE game classes as console classes: scene objects and their container, shapes, players, items, vehicles |
| `torque-server` | a dedicated server's main-loop pacing, platform values and datagram transport |
| `torque-client` | the client's engine half: the connection to a server, the prediction, action maps, the server query, window glue |
| `torque-mapcore` | the format-neutral map layer |
| `torque-maps` | Torque interior, terrain and material-list files |
| `torque-net` | NetConnection, ghosting, events, BitStream, string compression |
| `torque-ghosts` | ghost record codecs: the layout framework, the class registry, the parent parts and the engine's object layouts |
| `torque-ghost-world` | the client's world of ghosts: records merged, datablocks, the control object, mounted images, script threads, effects |
| `torque-images` | image decoding |
| `torque-fonts` | font caches and text rasterising |
| `torque-audio` | audio decoding and mixing |
| `torque` | the umbrella crate: the crates above behind features, and headless examples |
| `torque-example` | an example dedicated server and client over the loopback address (`just example-server`, `just example-client`) |

The roadmap is in the guide (`book/src/roadmap.md`).

## Building

Requires Rust (edition 2024) and [just](https://github.com/casey/just). [mdBook](https://rust-lang.github.io/mdBook/) for
the guide.

```
just build      # build the workspace
just test       # run the tests
just doc        # API reference (rustdoc)
just book       # the guide (mdBook), into target/book
just ci         # everything CI checks
```

## License

MIT; see [LICENSE](LICENSE). Some file formats and behaviours follow Torque3D (MIT, Copyright (c) 2012 GarageGames, LLC),
credited where they are used and in [NOTICE](NOTICE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md); report security issues as [SECURITY.md](SECURITY.md) describes. Changes are
listed in [CHANGELOG.md](CHANGELOG.md).
