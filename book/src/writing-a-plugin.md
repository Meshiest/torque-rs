# Writing a plugin

This chapter builds a plugin from nothing: ropes that a script hangs between two points, that sag under their own weight, and
that are drawn into the game's world. It uses most of what a plugin has: a datablock class, console functions, a resource
shared by the parts of the plugin, a per-tick hook and a render pass. Read [Plugins](plugins.md) first for what a plugin
and the engine builder are, and [Script functions from Rust](script-functions.md) and
[Rendering and input from plugins](rendering-and-input.md) for the registries the steps below use.

The code here is not pasted into the page: every listing is included from `crates/torque-plugin-rope/examples/walkthrough`, which
is built with the rest of the workspace and run by `cargo run -p torque-plugin-rope --example walkthrough`, so it compiles
as shown. It is a short version of the real plugin, `torque-plugin-rope`, whose last section says what the real one adds.

## The crate

A plugin is a crate of its own, named `torque-plugin-<what it adds>`, so a game can take it or leave it. It depends on the
crates it uses, never on a private module of the engine:

```toml
[package]
name = "torque-plugin-rope"
edition = "2024"

# Paths into this repository; a game outside it names the crates as it names the rest of the engine.
[dependencies]
torque-app = { path = "../torque-app", default-features = false }   # the render registry and PassCx
torque-engine = { path = "../torque-engine" }                       # Plugin, EngineClass, the macros
torque-render-core = { path = "../torque-render-core" }             # wgpu, as the passes use it
torque-render-hd = { path = "../torque-render-hd" }                 # the frame's formats
torque-ts-text = { path = "../torque-ts-text" }                     # atof, for "x y z"
torque-ts-vm = { path = "../torque-ts-vm" }                         # the script engine's types
```

(A game that uses the umbrella `torque` crate reaches the same things as `torque::app`, `torque::engine` and so on.) A plugin
that is a main difference from TGE or Torque3D goes in the changelog; every plugin gets a README and crate docs with a usage
example, as `torque-plugin-gamepad` and `torque-plugin-rope` have.

## Step 1: the datablock

A rope has a few numbers a script should be able to set and a server could send, so they are a **datablock**: a console class
below `SimDataBlock`, declared as a Rust struct. `#[derive(EngineClass)]` gives the class its fields, their docs, defaults
and ranges:

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/data.rs:data}}
```

A script then writes `datablock RopeData(Cable) { segments = 16; length = 9; };` as it would for any datablock. The `range` is
enforced when a script writes the field (the value is clamped, with a warning), and a typed read gives it back as the struct
(`RopeData::read(&vm, id)`), so Rust never sees a segment count of 100,000. The class is not registered yet; the plugin does that
in step 4.

## Step 2: the rope itself

The rope is plain Rust with no engine in it, which makes it easy to test. Its state is a chain of points; each step moves the
free points by their last move and by gravity, then pulls every piece back to its resting length a few times (position-based
dynamics: stable whatever you do to the ends, which matters when a script can move them anywhere):

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/sim.rs:sim}}
```

Three decisions are worth copying. The count of ropes is **capped** (`MAX_ROPES`): anything a script can make in a loop needs a
cap. The ropes sit in one `Ropes` value behind an `Arc<Mutex<..>>` because the pass that draws them must be `Send` on native
targets, and `lock` survives a poisoned mutex so one panic cannot take the rest down. And a rope **checks what it divides by**
(`clamp(2, 64)`, `max(0.01)`) even though the datablock's ranges make those cases impossible: the data may come from somewhere
else one day.

## Step 3: script functions

A console function is a Rust function with `#[function]`. Its script arguments (`&str`, numbers, `Option<T>` for optional ones)
set the argument counts, the usage text comes from the signature and the doc comment's first line, and **injected parameters**
(`VmRef`, `Res<T>`) are invisible to scripts:

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/natives.rs:natives}}
```

`Res<Ropes>` is the resource the plugin inserts in the next step; `VmRef` is the client's script engine, to look the datablock
up and read it. Returning `Result<_, ScriptError>` prints the error on the console, with the usage, and gives the script the
empty string: the native never panics for a script's mistake. A script that calls `ropeCreate("0 0", ..)` gets
`"0 0" is not "x y z"` on the console instead of a crash.

## Step 4: the plugin

The `Plugin` ties it together. `build` runs once, when the engine is built, and only **records** what the plugin adds:

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/plugin.rs:plugin}}
```

- The `#[derive(PluginId)]` and the `id` give the plugin a stable name for ordering, duplicate checks and `game.toml`.
- `insert_resource` is how the parts reach one another: the natives get it as `Res<Ropes>`, the tick and the pass hold a clone.
- `client_script()` registers for clients only (`script()` would add the class to servers too). `natives![..]` lists the
  functions the attribute declared.
- `engine.push(ClientTick::new(..))` is work every client does once per tick (32 ms), after its other work, in the order plugins
  pushed it.
- `engine.render().pass(stage, factory)` adds the pass. The factory runs for each client's world view, so every view has its own.

Nothing here runs a script or touches a window: the engine keeps these records and replays them into each client it makes, which
is why one engine can make many clients, and why a dedicated server (which replays the server recipes only) never sees any of
it.

## Step 5: drawing

A render pass is code that records draws into the engine's frame at a **stage**. The rope draws at `Opaque`, over the depth, into
the HDR frame:

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/pass.rs:pass}}
```

Read the comments. The depth is **reversed** (1 near, 0 far), so what is nearer passes `GreaterEqual`. The camera is `cx.camera`; the
frame's own targets live for this call only (so the bind group is made each frame and nothing that names a target is kept). The
buffers are made fresh each frame, which is simple and fine for a few lines and not for a thousand ropes: the real plugin keeps
its buffers and writes them again only when a rope moved. What a pass is handed, and what it must not do, is in
[Rendering and input from plugins](rendering-and-input.md#render-passes).

The lines are flat and unlit. A rope worth looking at is lit, fogged, thick and casts a shadow: that is the last section.

## Step 6: run it

A client made from an engine of `DefaultPlugins` and the plugin takes the script, and stepping the client runs its ticks:

```rust
{{#include ../../crates/torque-plugin-rope/examples/walkthrough/main.rs:main}}
```

In a game the plugin is added beside the default plugins the same way (`add_plugins((DefaultPlugins, RopePlugin))`), and the game's scripts call `ropeCreate` wherever a rope should hang. The level demo does it behind its `rope` feature (`just
examples/level-demo-rope-shots` draws the plaza with a rope across it).

## Step 7: test it

The simulation is plain Rust, so the tests that matter most need no engine: a rope hung between two points ends up below them and
as long as it was made; ten thousand steps with the ends dragged about never make a coordinate that is not finite; the cap
holds; and a datablock of nothing but `NaN` and `inf` still makes a rope that runs. (`torque-plugin-rope`'s `tests/sim.rs`.)

The script side needs a client but no window: build one from the engine the way the walkthrough's `main` does, run a script, step
it, and read the resource back (`tests/script.rs`: the datablock's clamping, the errors and their usage text, a rope hung from a
script object that is then moved and deleted).

The drawing needs a GPU but no window: attach a world view to the engine's renderer, draw a small scene with and without the
plugin and compare pixels (`tests/picture.rs`: the rope's sag is on the picture, the floor under it is darker for its shadow,
and nothing above the posts changed). Look at the pictures too: `TORQUE_ROPE_PICTURES=<folder> cargo test -p torque-plugin-rope
--test picture` writes them.

## What the real plugin adds

`torque-plugin-rope` is this plugin with the parts a game needs:

- **A lit tube, fogged, with a shadow.** The same opaque pass draws an eight-sided tube lit by the scene's sun and ambient light
  and fogged as the frame is, and a second pass at `Stage::Shadows` draws the tubes into each of the sun's cascades. They are one
  type registered at three stages (the third is the ribbons and lines, below):

```rust
{{#include ../../crates/torque-plugin-rope/src/lib.rs:build}}
```

- **Stable numbers.** Fixed substeps (1/120 s, at most 16 a tick, so a stalled frame slows the rope instead of blowing it up), an
  end that follows its anchor at a bounded speed, a floor, stiffness, and a rope that lays itself out again between its ends
  if its points ever stop being finite. Every number that comes from a script is made finite and clamped on the way in.
- **Anchors on objects.** `ropeAttach(rope, end, object)` hangs an end from an object that the client reads each tick, through an
  `AnchorResolver` a game can replace (to read a ghost's place, say).
- **Three looks.** A datablock's `style` picks the tube, a `"ribbon"` (a flat strip that always faces the camera along a smooth
  curve through the simulated points) or a `"line"` (a straight strip between the ends that is not simulated: a laser or a
  tripwire). Strips are alpha blended at `Stage::Translucent` over the frame's depth, so they are hidden by what is in front of
  them, and their triangles are made on the CPU each frame from the camera, which keeps them flat to the screen, at least two
  pixels wide and soft at the edges.
- **Caps** on ropes, segments and coordinates, **tests** for each, a README and crate docs with a usage example.

Its source is a good second read once the walkthrough makes sense: `sim.rs` (the simulation and its caps), `mesh.rs` (the tube),
`strip.rs` (the ribbon and the line), `pass.rs` (the passes) and `natives.rs` (the console functions).
