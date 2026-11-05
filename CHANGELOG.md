# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first development version. Nothing is published yet; this is what exists.

### Added

- `torque-ts-text`: the C runtime text and number conversions TorqueScript relies on (`atoi`, `atof`, the `printf`
  formats).
- `torque-ts-bytecode`: compiled TorqueScript (DSO) files: reading, writing and decoding, with dialect profiles.
- `torque-ts-compiler`: the TorqueScript compiler, from source text to DSO bytecode.
- `torque-ts-vm`: the TorqueScript runtime: the console's variables, namespaces, packages, objects, datablocks,
  natives and events, with a pluggable executor.
- `torque-collide`: convexes, GJK, polygon lists, working sets and object type masks.
- `torque-shapes`: DTS shapes and DSQ sequences (reading, writing, collision meshes, animated poses).
- `torque-shape-builder`: a builder of shape models from code (nodes, box and prism meshes, details, materials, keyframed
  sequences), with every derived figure filled in, written as a `.dts` by `torque-shapes`; behind the umbrella's
  `shape-builder` feature.
- `torque-sim`: the process list, players, AI players, items, projectiles and mounted images.
- `torque-vehicle`: rigid bodies, wheeled and flying vehicles.
- `torque-scene`: the scene container (bins, `findObjects` and `castRay` walks, the container searches) and a scene object's
  transform maths, behind a table a host may replace.
- `torque-resource`: the resource layer: a game folder read-only, zips mounted as folders, the scripts' writes in memory.
- `torque-game`: the TGE game classes as console classes: the scene objects and their container, the shapes the datablocks
  load, the simulated classes' hosts and the datablocks' computed values.
- `torque-server`: a dedicated server's main-loop pacing, the platform values its natives report and the datagram
  transport with its bind policy (its errors implement `std::error::Error`).
- `torque-client`: the client's engine half: the connection to a server over a game's feed (TGE's feed and records
  included), the control object's prediction, action maps and input routing, the LAN server query, the eye, zoom, frame
  rate and HUD feed, the console log file, and the window glue behind the `window` feature.
- `torque-mapcore` and `torque-maps`: the format-neutral map layer and readers for Torque interior, terrain and
  material-list files, and a `.dif` writer for every interior version the reader takes.
- `InteriorInstance` hosted and ghosted: the server reads its `.dif` and collides with its hulls (rays, movers, projectiles,
  lag-compensated shots), its record is TGE 1.4's, the examples' client predicts against it, and `torque-app`'s ghost scene
  draws it (its surfaces as static mesh objects of the world view, `WorldView::mesh_object`, each material's draw through the
  style's `interior_draw`) and puts projectile holes on it.
- `torque-interior-builder`: interiors (`.dif`, version 0 in TGE's form) built from convex brushes made in code or read from a
  Quake or Valve 220 `.map`: hidden faces removed, a solid BSP, the brushes as collision hulls with emit strings and polygon
  lists, texture planes and lightmap rectangles; a `.map`'s water, triggers, physical zones and spawn points as a `.mis`
  fragment.
- `torque-modelcore`: the format-neutral model layer (nodes with rest transforms, meshes with two UV sets, colors and up to
  four weights a vertex, skins, materials with their PBR textures, detail levels, animations sampled at even keys), in
  Torque's frame, with the naming conventions of Torque's shape loader (`detailN`, `Col-N`, `LOSCol-N`, `col_*` and `hull_*`
  hulls, mission volumes).
- `torque-import`: a glTF 2.0 reader (`.glb`, or `.gltf` with side files or `data:` URIs; meshes, skins, materials, images,
  step, linear and cubic-spline animations; every count, offset, stride, URI and image header bounded) and writer, and the
  model layer as a `.dts` (through `torque-shape-builder`) and a `.dts` as the model layer. A shape written as a `.glb` and
  read back makes the same `.dts`, byte for byte.
- Native glTF shapes: a datablock's `shapeFile` may name a `.glb` (or a `.gltf` with `data:` URIs); the server's preload, the
  client's prediction and eye and the shape drawing read it as the `.dts` it makes.
- `torque-convert`: the converter command, its own crate: models (`.glb`, `.gltf`, `.dts`) into `.dts` (with the images the
  model carries) or `.glb`, and `.map` brushes into interiors.
- `torque-shape-builder`: skin meshes, objects with a mesh in several details, primitives of several materials, aligned
  scale keys and blend flags in owned sequence specs; the name table is made at build time (nodes, details, objects,
  sequences).
- The example kit also writes `cube.glb`, `figure.glb` and `pistol.glb`; the arena loads them instead of the `.dts` files when
  `ARENA_SHAPES=glb`.
- `torque-net`: connections, the packet layer, events, ghosting, bit streams and string compression.
- `torque-ghosts`: the networked objects' record codecs: the layout framework, the class registries, the parent parts, and
  TGE 1.4's ghost and datablock layouts (`TgeGhosts`, `TgeDatablocks`).
- `torque-ghost-world`: the client's world of ghosts over any set of ghost classes, with TGE's set: the records merged, the
  datablocks, the control object and the eye, the mounted images' state machine, the script threads and the effects.
- `torque-images`: PNG and JPEG decoding and mip chains.
- `torque-fonts`: font caches, glyph metrics and text layout.
- `torque-gui`: the GUI object model, layout and the script methods of GUI classes.
- `torque-gui`: buttons, popups and their open lists, and slider thumbs drawn as rounded boxes (a solid fill, a one-pixel
  border, hover, pressed and disabled colors from the profile); a host keeps TGE's beveled boxes for a profile with
  `GuiExtension::classic_boxes`.
- `torque-audio`: WAV and Ogg Vorbis decoding, sources, channels, the listener and a mixer, with device output behind
  the `device` feature.
- `torque`: the umbrella crate, re-exporting the engine crates behind features, with headless examples (`script`,
  `shape`, `collide`, `net`).
- `torque-app`, re-exported as `torque::app` (the umbrella's `app` feature, off by default: it needs a GPU): what a game on the engine shares: its game
  folder in memory, a server on UDP or an in-memory link (with a dedicated server's pacing), a client with the server it hosts,
  the world view under the GUI, an orbit camera, a window, offscreen pictures and the command line's flags. The example games
  run on it, with their shared scripts (connection lifecycle, mission loading, figures, falls, notes) in
  `examples/common/game`.
- `torque-example`: a minimal dedicated server (the bundled script mission, TGE's records) and client, `min-server` and
  `min-client` (`just examples/min-server`, `just examples/min-client`), that connect over the loopback address and print the
  ghosts the client is sent. The example recipes are in `examples/justfile`.
- `torque-example-assets` (`examples/assets`): the example games' asset kit, made from code at run time: a unit cube with a
  collision hull, a box figure (eye, camera and mount nodes; idle, walk, run and jump sequences), a pistol with its image
  nodes and a fire sequence, a projectile, and their PNG textures.
- `torque-example-level-demo` (`examples/level-demo`): a singleplayer world in a window: a menu of maps (the `.mis` files in its
  `game/missions` folder), then the kit's box figure walking and jumping on the map's cubes, a server (torque-server with
  TGE's game classes) and a client (TGE's connection half) joined in one process by an in-memory datagram link.
- `torque-server`: `game::install` (TGE's game classes on one container and the process list, `GameBase::onAdd`), the
  GameConnection's ghosting and control methods (`activateGhosting`, `resetGhosting`, `setControlObject`,
  `getControlObject`), the `NetObject` behaviour that enters ghostable objects in the hub, the command natives registered
  with the host, and records packed from the simulated Player's state.
- `torque-client`: TGE's feed reads datablock events and answers `DataBlocksDone`.
- `torque-client`: named keys and mouse buttons (`Key`, `MouseButton`, also `torque::client::Key`) with the client host's
  `key`, `press`, `type_text` and `mouse_button`; the window maps winit's keys through `Key` (F13 to F24, the ISO key next
  to the left Shift and the side buttons now reach the platform layer).
- `torque-gui`: the US layout's scan codes (`map_virtual_key`) cover F13 to F24, so F13 to F15 give their key codes
  (`f13`..`f15`); F16 to F24 have none in the scan-code table.
- `torque-example-vehicle-demo` (`examples/vehicle-demo`): the kit's figure beside a dune buggy on green checkerboard hills (a
  `TerrainBlock`), for one player or a few on a LAN (host, or join from the list or an address); a click on the buggy takes the
  nearest free seat (the driver's or the passenger's), W A S D drive it, Space brakes, E gets out, Tab switches between the eyes
  and the chase camera; its tires throw dust and leave tracks. The kit adds the buggy and its
  wheel, the `sit` and `ride` sequences, the hills' `.ter` and their checkerboard.
- `torque::app::hosting`: hosting a game and joining one (the client's network mode, the address a hosted game binds and is
  joined by, a typed address, the join screen's natives), moved out of the arena shooter; `GameServer::start_with` runs a
  game's setup (its simulated classes) before the game classes are installed.
- `torque-render-scene`: the dust a vehicle kicks up (`vehicle_dust`: TGE's tire emitter by speed and slip, and its lift-off
  dust) and tire tracks a game may ask for (`tracks`: strips clipped to the ground's triangles, multiplied over it, fading);
  `torque::app`'s ghost scene runs both (`GhostScene::set_tire_tracks`). A vehicle look's wheels carry their slip.
- `torque::app::terrain`: a `TerrainBlock` over a legacy `.ter` with its collision on the server (`TerrainBlock::onAdd`), the
  same block as a static object of the client's prediction, and `WorldView::set_terrain` to draw it.
- `torque-server`: Torque's net game ghosts the simulated vehicles (their records from the vehicle's state, its step's dirty
  bits as TGE's), and a Player that drives a WheeledVehicle sends the vehicle's packet data after its own.
- `torque-client`: TGE's feed reads a WheeledVehicleData's wheel count from its shape (the game's files,
  `ClientGame::file_reader`, which the prediction reads the driven vehicle's shape from too).
- `torque::app::lights`: a client's engine spot lights and its day (TimeOfDay, ScatterSky) into the world view
  (`WorldView::set_spots`, `WorldView::set_day`: the sun, ambient, fog, sky and clouds follow the day).
- `torque::app::camera_blend`: `CameraBlend`, a camera that eases from one mode's eye to another's on a switch (first person
  to third and back), restartable mid-way, kept out of walls and ground by the game's own rule; the level demo's V uses it.
- `torque::app::client`: `GameClient::start_local` runs a game alone over an in-memory link, the client's socket set aside
  while it runs.
- `torque-example-vehicle-demo`: the buggy's headlight (F, the driver's) from the lamps on its roof, which glow while it is on, a day that runs through sunrise, noon, sunset and
  night, "Play alone" (and `--solo`: no socket for the game, nobody on the network sees it), and Tab easing between the
  eyes and the chase camera.
- The guide (mdBook) and the API reference with no rustdoc warnings.
- Continuous integration on Linux and Windows (formatting, clippy, tests, examples, rustdoc, the guide, the MSRV and
  cargo-deny), workspace lints, and the contributing, security and notice files.

### Fixed

- `torque-net`: a ghost-always object's update keeps the mask bits its pack could not send (a light mounted on an object
  the connection had no ghost of yet), so a later update sends them.
- `torque-server`: Torque's net game sends a ShapeBase's script threads (`playThread`; it sent none).
- `torque::app::view`: a day whose ScatterSky names no shadow color keeps the daylight's (the shadows were black).
- `torque-client`: a Player that gets off its mount plays `root` and picks its actions by its moves again (it went on
  showing the seated action the server had set).
- `torque-game`: a shape with fewer than eight collision details (the bounds report and the hull slots read past its
  lists); the class specs name TGE's console methods (the crate's natives were refused without them), and `SceneObject`,
  `ShapeBaseData` and `PlayerData` read their constructors' values (a Player made from a script datablock could not move).

[Unreleased]: https://github.com/TODO/torque-rs/commits/main
