# torque-rs

![The example games: a buggy in a turn, a shot in the arena, a sea at golden hour and an office](examples/screenshots/hero.webp)

- **Top left:** the vehicle demo's buggy taken hard through a turn of the race track, kicking up dust and leaving tire tracks.
- **Top right:** the arena shooter: one player's bolt in flight, the muzzle's flare, the other player hit.
- **Bottom left:** the level demo's valley: low over the sea at golden hour, racing clouds reflected in the water, far ranges fading to the horizon.
- **Bottom right:** the level demo's office: desks under their ceiling lights, windows and glass.

Each picture is the engine's own enhanced renderer, drawn offscreen by `just examples/showcase` (see [Examples](#examples)).

A game engine library in Rust inspired by the Torque Game Engine and Torque3D, growing beyond their original design:
TorqueScript and the console, SimObjects and datablocks, client/server networking with ghosting, DTS shapes, convex
collision and the game object model (players, items, projectiles, vehicles).

Games register their own systems (console classes and functions, networked object types, render passes, file formats,
resource loaders) through the engine's registries and change engine behavior only through those hooks. Tools (editors,
debugger, telnet console, journaling) are build-time features, so a finished game can be built without them.

## Crates

| Crate | Description |
|---|---|
| `torque-ts-text` | the C-runtime text and number rules TorqueScript relies on |
| `torque-ts-bytecode` | compiled scripts (DSO files): reading, writing, decoding; dialect profiles |
| `torque-ts-compiler` | the TorqueScript compiler |
| `torque-ts-vm` | the TorqueScript runtime: the console's variables, namespaces, objects, natives and events |
| `torque-collide` | convexes, GJK, polygon lists, working sets, object type masks |
| `torque-shapes` | DTS shapes and DSQ sequences |
| `torque-shape-builder` | a builder of shape models from code: nodes, box and prism meshes, skin meshes, details, materials and sequences |
| `torque-modelcore` | the format-neutral model layer: nodes, meshes, skins, materials, detail levels, sampled animations |
| `torque-import` | model readers and writers: glTF 2.0, OBJ and Blockbench into the model layer, the model layer into `.dts` shapes and back; native glTF shapes |
| `torque-convert` | the converter command: models (glTF, OBJ, Blockbench, DTS) into `.dts` or `.glb`, `.map` brushes into interiors |
| `torque-sim` | the process list, Player, AIPlayer, Item, Projectile, ShapeBase images |
| `torque-vehicle` | rigid bodies, wheeled and flying vehicles |
| `torque-scene` | the scene container: bins, searches, a scene object's transform math |
| `torque-resource` | the resource layer: a game folder, its zips mounted as folders, the scripts' writes in memory |
| `torque-game` | the TGE game classes as console classes: scene objects and their container, shapes, players, items, vehicles |
| `torque-server` | a dedicated server's main-loop pacing, platform values and transports (UDP, WebSocket, WebRTC data channels) |
| `torque-client` | the client's engine half: the connection to a server, the prediction, action maps, the server query, window glue |
| `torque-mapcore` | the format-neutral map layer |
| `torque-maps` | Torque interior, terrain and material-list files; a `.dif` writer |
| `torque-interior-builder` | interiors (`.dif`) built from brushes made in code or read from a `.map`, or from glTF, OBJ and Blockbench meshes |
| `torque-net` | NetConnection, ghosting, events, BitStream, string compression |
| `torque-ghosts` | ghost record codecs: the layout framework, the class registry, the parent parts and the engine's object layouts |
| `torque-ghost-world` | the client's world of ghosts: records merged, datablocks, the control object, mounted images, script threads, effects |
| `torque-gui` | the GUI object model: controls, the canvas, layout and the script methods of GUI classes |
| `torque-images` | image decoding |
| `torque-fonts` | font caches and text rasterizing |
| `torque-audio` | audio decoding and mixing |
| `torque-render-core` | the renderers' device layer: the GPU, offscreen targets, a frame read back, a window's surface, the GUI pass |
| `torque-render-scene` | a frame as plain data: the camera, the sun, textures and draws with their fixed-function state; maps, shapes, particles, decals, tire tracks and the enhanced renderer's scene |
| `torque-render-hd` | the enhanced renderer on the GPU: shadows, sky and clouds, water, reflections, AO, TAA, bloom and exposure |
| `torque-app` | what a game made on the engine shares: its files, a server, a client, a window, the world view and offscreen pictures (needs a GPU) |
| `torque` | the umbrella crate: the crates above behind features, and headless examples |
| `torque-example` | an example dedicated server and client over the loopback address (`just examples/min-server`, `just examples/min-client`) |
| `torque-example-assets` (`examples/assets`) | the example games' asset kit: a cube, a box figure, a pistol, a buggy, terrains, trees, interiors and their textures, made from code (`just examples/assets`) |
| `torque-example-common` (`examples/common`) | what the example games share beyond `torque::app`: the kit as files, the shared scripts, the prediction over the kit's cubes and interiors |
| `torque-example-character` (`examples/character`) | an animated box figure in a window, with an orbit camera |
| `torque-example-level-demo` (`examples/level-demo`) | a singleplayer world: a menu of maps (cubes, a Blockbench office, the valley), native and in the browser |
| `torque-example-fps` (`examples/fps`) | an arena shooter for a few players: LAN, internet, and browsers over WebSocket and WebRTC |
| `torque-example-vehicle-demo` (`examples/vehicle-demo`) | a buggy to drive over hills on a race track through forests, alone or with others |
| `torque-example-showcase` (`examples/showcase`) | the pictures at the top of this page, drawn offscreen |

The roadmap is in the guide (`book/src/roadmap.md`).

## What torque-rs adds beyond TGE and Torque3D

Where torque-rs follows TGE 1.4 or Torque3D (TorqueScript, the console, datablocks, ghosting and its records, DTS, DIF and
legacy terrain files, the game classes' physics), it aims to be faithful to them, and those parts are credited where they
are used. The parts below are the engine's own: they are new, or they sit beside the faithful path as an option a game
picks.

- **The enhanced renderer** (`torque-render-hd`, engine-own; the fixed-function path stays for a faithful look): cascaded
  sun shadows with contact shadows, a physical sky with volumetric clouds (`CloudLook`, sunlit cumulus included), particles
  lit by the sun, the sky and the lights, TAA, ambient occlusion (GTAO or SSAO), spot lights with shadows, water with
  waves, refraction, planar or screen-space reflections and the sun's glint, bloom and automatic exposure, at four quality
  steps (Low to Ultra). Opt-in per scene: the ground's textures varied against visible tiling (stochastic tiling and a
  large-scale tint) and the clouds on a clock of their own (a timelapse). The day cycle runs from a server's `TimeOfDay` under a `ScatterSky` (classes after Torque3D's),
  lighting the view as the sun moves.
- **Interiors from more than brushes** (`torque-interior-builder`): `.dif` interiors built from brushes made in code or
  read from a Quake or Valve 220 `.map`, and from meshes read as glTF, OBJ or Blockbench models; a Blockbench model's
  textures can be drawn nearest-sampled and with see-through texels (glass), as Blockbench draws them. Interiors are TGE's
  format; building them from these sources is the engine's own.
- **Model import and conversion** (`torque-import`, `torque-convert`): glTF 2.0, OBJ and Blockbench models into the model
  layer and out as `.dts` (or back to `.glb`), with skins and sequences; `torque-convert` does it on the command line.
- **The web build:** the level demo and the arena run in the browser on WebGPU; a build fetches its files by URL from a
  list (`files.json`: sizes and SHA-256 hashes, kept in the browser's cache) or carries them inside the module. The arena's
  server takes browsers over WebSocket and WebRTC data channels beside desktop players on UDP, all in one game (cross play).
- **Hitboxes with lag compensation** (`torque-game`, engine-own and opt-in): a Player can answer shots with its shape's
  posed LOS meshes instead of its box, and a server can cast a shot against the Players as the shooter saw them.
- **Vehicles on the client:** tire tracks laid behind the wheels (engine-own; TGE has none), the wheels' dust
  (TGE's emitters, through the particle engine), and other players' vehicles drawn smoothly between their updates, with
  corrections eased out over the next ticks.
- **Camera blend** (`torque::app::camera_blend`): the camera eases between first and third person instead of cutting,
  kept out of walls and ground.
- **The app layer** (`torque-app`): what a game shares on the engine (its files in memory, a server, a client, a window,
  the world view and offscreen pictures for tests and screenshots), so an example game is a few hundred lines.

## Examples

The example games are in [`examples/`](examples/README.md), with pictures, their controls, and how to run each natively
and in the browser. In short:

| Example | Native | Web |
|---|---|---|
| The level demo (plaza, maze, ramps, towers, office, valley) | `just examples/level-demo` | `just examples/web-level-demo` (`?map=valley`) |
| The arena shooter | `just examples/fps-client` | `just examples/web-fps` |
| The vehicle demo | `just examples/vehicle-demo` | |
| The character | `just examples/character` | |
| The minimal server and client | `just examples/min-server`, `just examples/min-client` | |

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

MIT; see [LICENSE](LICENSE). Some file formats and behaviors follow Torque3D (MIT, Copyright (c) 2012 GarageGames, LLC),
credited where they are used and in [NOTICE](NOTICE).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md); report security issues as [SECURITY.md](SECURITY.md) describes. Changes are
listed in [CHANGELOG.md](CHANGELOG.md).
