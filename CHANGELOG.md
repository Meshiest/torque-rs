# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The first development version; nothing is published yet. torque-rs follows the engine of TGE and Torque3D (the script
engine, networking, simulation, the GUI and rendering) and is credited where it does. What is listed below is new or
different.

### Added

#### Rendering

- `torque-render-hd`: the HD renderer, beside the fixed-function one that keeps the original look. Cascaded sun shadows
  and contact shadows; a shadow cube for every point and spot light in view, sized by its importance; a physical sky
  with volumetric clouds (`CloudLook`) that water reflects; water with waves, refraction, reflections and sun glint;
  particles lit by the sun, the sky and lights; TAA, ambient occlusion, bloom and automatic exposure; four quality
  levels, Low to Ultra. Optional ground textures that hide tiling (`HdScene::ground_variation`) and a cloud clock of
  their own (`HdScene::cloud_time`). A ghosting check example measures how fast each history settles.
- `torque::app`: interiors as static meshes kept on the GPU and culled there, with their shadows cached, so interiors
  that stay put cost nothing on the CPU each frame.
- `torque-render-core::pipeline_cache`: compiled pipelines kept in a per-user wgpu pipeline cache on Vulkan, keyed by
  adapter and driver and size-capped, so a start no longer compiles every pipeline again.
- `torque-render-scene`: tire tracks (TGE has none) and vehicle dust.
- `torque-render-scene`: the own Player's arms in its first-person view (`ClientWorld::first_person_nodes`), posed by the
  body's animation and kept out of the middle of a level view (`first_person_clear`), where TGE draws none of the body and
  Torque3D draws separate arm shapes.
- `torque-render-hd`, `torque::app`: the view under water (`HdScene::water_zone`, set by `WorldView::set_water` from the
  water's bottom and its `underwater` share): the water's tint and absorption, split at the waterline when the camera
  straddles it.
- `torque-gui`: buttons, popups and slider thumbs drawn as rounded boxes; `GuiExtension::classic_boxes` keeps TGE's
  beveled ones.
- `torque-fonts`: system fonts on Linux and macOS, with metric-compatible free faces in place of missing ones.
- `torque::app::config`: `game.toml`, a small config in a game's files read by the prebuilt engine: the plugins to turn off or on by id, the runner, and the scripts a server and a client run first (TGE fixes them in the executable). Strict: unknown keys are warnings, a size cap, errors with line and column.
- `torque::app::gizmos`: debug lines, boxes, spheres, arrows, crosses and text labels in world space (`GizmosPlugin`, not in
  `DefaultPlugins`; the `gizmos` feature), from Rust and as console functions (`drawDebugLine` and kin); TGE has none.
- `torque-gui`: GUI control classes written in Rust and added by plugins (`GuiControlClass`, `add_gui_control`): their fields
  are console fields, they draw, lay themselves out and take the mouse and keys by their own class, and any number of
  plugins can each add some; supersedes `GuiExtension::draw_control`, now deprecated.

#### Web and networking

- Web builds on WebAssembly and WebGPU: a game folder packed as one block or fetched from a file list (`files.json`,
  with sizes and SHA-256 hashes, checked on arrival), the page's one thread and the browser's clock and fonts, a render
  scale for the canvas, and a bridge between the page and the game's scripts (`torque::app::page_bridge`). The
  workspace's `web` build profile.
- `torque-server`'s `web` feature: browsers join over WebSocket, served the game's page over HTTP or TLS (rustls) on the
  same port, beside UDP players in the same game.
- `torque-server`'s and `torque-client`'s `webrtc` features: WebRTC data channels (str0m) carry a browser's datagrams,
  falling back on the WebSocket when the channel fails.
- `torque-rooms`: a room service for player-hosted games. A host gets a room code; the service carries only the WebRTC
  signaling and hands out STUN and TURN credentials. The `p2p` features let a player host a game with no port forwarded
  (`net::web::p2p::P2pHost`) and others join it through a room (`net::room_join`).
- Hardening against hostile input across the web stack: caps on requests, headers, frames, messages, bytes and
  connections (in all, per address and per second), timeouts, `Host` and origin checks, ICE checks and candidates
  filtered by source, and peers given stand-in addresses so per-address limits and bans hold.

#### Gameplay

- `torque-game`: hitboxes with lag compensation (opt-in): a Player can be hit on its posed shape, checked against where
  the shooter saw it.
- `torque::app::camera_blend` and `camera_boom`: a camera that moves smoothly between first and third person and a
  third-person boom, both kept out of walls and the ground.
- `torque::app::ghosts`: a steady first-person view (`GhostOptions::steady_eye`) carried by the Player's motion rather
  than its animation.
- `torque-sim`, `torque-game`, `torque-client`: Torque3D's crouch, prone and sprint poses on TGE's Player, with timed
  getting down and up, a `walk` action below running pace, and eased transitions between sequences.
- `torque-client`: other players' vehicles drawn where they are now, coasting from their last update, and steering,
  springs and wheel contacts eased between ticks.

#### Content import

- `torque-import`: glTF 2.0 (read and write), Wavefront OBJ and Blockbench (`.bbmodel`) readers, the model layer
  (`torque-modelcore`) into `.dts` and back. A datablock's `shapeFile` may name a `.glb` directly. Shape materials can
  be nearest-sampled (a material flag bit TGE leaves unused), as Blockbench models and glTF `NEAREST` samplers ask.
- `torque-interior-builder`: `.dif` interiors built from convex brushes in code, Quake or Valve 220 `.map` files, or
  glTF, OBJ and Blockbench meshes, with hulls made or authored and a `.map`'s water, triggers and spawn points as
  mission objects. Interior materials can be nearest-sampled or see-through glass.
- `torque-shape-builder`: shapes built from code and written as `.dts`.
- `torque-convert`: the converter command for models and `.map` files.
- Every reader bounds its counts, offsets, sizes and paths, and font and secret files are checked before they are read.

#### Tooling

- `torque`: the umbrella crate, re-exporting the engine crates behind features; `torque-app` (`torque::app`) holds what
  every game shares (its files, a server, a client, a window, the world view and offscreen pictures).
- Math on `glam`.
- `torque-script-reference` (`just script-reference`): writes the guide's script reference (every console function, method, class and pref of a default engine, with the opt-in plugins' marked) from the script engines it makes; CI fails when the checked-in pages are stale.
- Continuous integration on Linux (formatting, clippy, tests, rustdoc, the guide, the MSRV and cargo-deny),
  and one `unsafe` block in the workspace (the pipeline cache).

[Unreleased]: https://github.com/Meshiest/torque-rs/commits/main
