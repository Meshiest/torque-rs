# Roadmap

What is planned, roughly in order. Items marked *tool* are build-time features.

## Done

- The script virtual machine, compiler and bytecode with pluggable executors and dialect profiles: see [Console and scripting](scripting.md).
- TGE 1.4's networked objects: the ghost and datablock record codecs, the parent parts and the client's world of ghosts:
  see [Networked objects](networked-objects.md).
- An example dedicated server and client over the loopback address: see [Server and client](server-and-client.md).

## Foundations

- Documentation: the API reference and this guide, built with no warnings in CI.
- Tools as build-time features (`tools` and finer features); a shipping build registers no tool classes.
- Extensible collision: game-defined convex builders and hull classes; game-defined object type masks with TGE's as the
  default; host callbacks for contact touches and projectile pose updates.
- Extension points in Player, Projectile, Item and WheeledVehicle, so a game extends a class without forking it.
- Network profiles: handshake, query, event registry, ghost policy and scope scheduling supplied by a game, TGE's by
  default.

## Engine

- The GUI controls and the resource manager.
- Object persistence: `SimObject::save` and deep clone.
- A scene graph with zones, portals and a visibility-set cull path.
- The missing GUI controls (TreeView, FrameSet, MenuBar, TabBook, Stack, ColorPicker, DirectoryTree, ListBox, ...).
- Static meshes with triangle-mesh and convex-hull collision, shared by imported props and mesh-built levels.
- glTF 2.0 import; maybe COLLADA later.
- Source BSP maps through the format-neutral map layer: brushes, displacements, static props, visleafs, lightmaps,
  cubemaps, the 3D skybox, water, overlays and worldlights, read from the player's own installs.
- Full terrain (editable, `.ter` writer), full interiors, lighting (lightmaps and dynamic lights).
- The remaining game classes: WaterBlock, PathedInterior, decals clipped to interiors and terrain (the client clips a
  projectile's decals to the triangles its caller gives), MissionArea, AIConnection, HoverVehicle,
  lightning and precipitation, splash and debris, replicators.
- A modern particle system beside TGE's: GPU simulation and sorting, soft particles, lit and shadowed particles,
  flipbook and ribbon (trail) emitters, curves over a particle's life, collision with the scene, and an editor-friendly
  data format; TGE's ParticleEmitterData stays the exact default.
- `torque::app`: the shared game layer the examples use (game files, server and client wrappers, the window loop,
  screenshots, the shared scripts), so a new game starts from it.
- Voice chat: proximity and channel voice (Opus), relayed by the server under game rules.
- A configurable tick rate: TGE's 32 ms tick as the exact default, shorter ticks as a server setting.
- Script engine performance: benchmarks first, then dispatch, strings, call caches; never at the cost of exact
  behaviour.

## Tools

- GUI editor and inspector *(tool)*.
- Debug draw layer and editor camera *(tool)*.
- World editor: selection, gizmos, undo, Creator, mission area editor *(tool)*.
- Terrain editor, painter and Terraformer *(tool)*.
- Platform tools: journal and demo recording, telnet console, script debugger, profiler *(tool)*.
