# Introduction

torque-rs is a game engine library in Rust inspired by the Torque Game Engine and Torque3D, and it grows beyond their
original design.

The engine keeps Torque's architecture:

- **TorqueScript and the console**: a compiler to compiled-script (DSO) code, a virtual machine, console classes and
  functions, namespaces and packages.
- **SimObjects and datablocks**: named objects in groups and sets, datablocks sent once to every client.
- **Networking**: a client/server model where the server ghosts objects to clients, events, a bit-packed stream and
  string compression.
- **Shapes and collision**: DTS shapes and DSQ sequences, convex collision with GJK, polygon lists and working sets.
- **The game object model**: the process list, players, AI players, items, projectiles, mounted images and vehicles.

A game registers its own systems with the engine (console classes and functions, networked object types, render passes,
file formats, resource loaders) and changes engine behaviour only through those hooks. Tools are build-time features,
so a shipping build carries none of them.

The API reference is generated with `cargo doc`; run `just doc` to build it. The [roadmap](roadmap.md) lists what is
planned.
