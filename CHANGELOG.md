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
  material-list files.
- `torque-net`: connections, the packet layer, events, ghosting, bit streams and string compression.
- `torque-ghosts`: the networked objects' record codecs: the layout framework, the class registries, the parent parts, and
  TGE 1.4's ghost and datablock layouts (`TgeGhosts`, `TgeDatablocks`).
- `torque-ghost-world`: the client's world of ghosts over any set of ghost classes, with TGE's set: the records merged, the
  datablocks, the control object and the eye, the mounted images' state machine, the script threads and the effects.
- `torque-images`: PNG and JPEG decoding and mip chains.
- `torque-fonts`: font caches, glyph metrics and text layout.
- `torque-gui`: the GUI object model, layout and the script methods of GUI classes.
- `torque-audio`: WAV and Ogg Vorbis decoding, sources, channels, the listener and a mixer, with device output behind
  the `device` feature.
- `torque`: the umbrella crate, re-exporting the engine crates behind features, with headless examples (`script`,
  `shape`, `collide`, `net`).
- `torque-example`: an example dedicated server (the bundled script mission, TGE's records) and a client that connect over the
  loopback address and print the ghosts the client is sent.
- The guide (mdBook) and the API reference with no rustdoc warnings.
- Continuous integration on Linux and Windows (formatting, clippy, tests, examples, rustdoc, the guide, the MSRV and
  cargo-deny), workspace lints, and the contributing, security and notice files.

[Unreleased]: https://github.com/TODO/torque-rs/commits/main
