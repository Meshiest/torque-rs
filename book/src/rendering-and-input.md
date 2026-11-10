# Rendering and input from plugins

What a player sees and does goes through a few more registries a plugin can add to while the engine is built. Each one is
the same shape as the script registry ([Script functions from Rust](script-functions.md)): the plugin names what it adds, the
engine keeps it, and every client makes its own from it when it starts. Nothing here changes what the engine draws or reads
until a plugin adds something, and each registry has caps, so a plugin that misbehaves is bounded.

| A plugin adds | Through | What it is | Chapter |
|---|---|---|---|
| a **render pass** | `engine.render().pass(Stage, factory)` | code that records its own draws into the engine's frame at a named point of it | [below](#render-passes) |
| a **static batch** | `engine.render().static_batch(factory)` | one mesh standing in many places, kept on the GPU | [below](#static-batches) |
| an **emitter kind** | `engine.render().emitter_kind(name, factory)` | a kind of particle emitter beside TGE's | [below](#emitter-kinds) |
| a **GUI control** | `engine.add_gui_control(..)` | a control class with its own drawing and input | [GUI controls from plugins](gui-controls.md) |
| an **input source** | `engine.add_input_source(..)` | a device that arrives as `joystick<N>` | [Input sources from plugins](input-sources.md) |
| a **camera mode** | `engine.add_camera_mode(name, ..)` | where the eye is, picked by name | [Camera modes from plugins](camera-modes.md) |
| an **asset loader** | `engine.assets().loader(..)` | a file format, read through the engine's files with caps | [Assets from plugins](asset-loaders.md) |

The [Writing a plugin](writing-a-plugin.md) chapter uses the first of these (and the script registry, the engine's ticks and
resources) to build a rope.

## Render passes

A pass is code that draws into the engine's frame. The frame has stages, in order, and a pass names the one it wants:

| Stage | When | What it draws into |
|---|---|---|
| `Shadows` | after the sun's cascades are drawn | each cascade's map |
| `Prepass` | after the depth prepass | the depth, the motion and the normals |
| `Opaque` | after the opaque pass, before the decals and the volumetric light | the HDR frame, over the depth |
| `Translucent` | after the translucent passes | the HDR frame, over the depth |
| `AfterTaa` | after the anti-aliasing, before the exposure, the bloom and the tonemap | the anti-aliased HDR frame |
| `Post` | after `AfterTaa` | the same frame, with a copy of it to read |
| `Overlay` | after the tonemap | the finished picture |

```rust
use torque::app::render::{EngineRender as _, HdPass, PassCx, Stage};
use torque::engine::{EngineBuilder, Plugin, PluginId};

struct Outline;

impl HdPass for Outline {
    fn draw(&mut self, cx: &mut PassCx<'_>) {
        // Begin a render pass in `cx.encoder` into `cx.ldr`, within `cx.viewport` ...
    }
}

#[derive(PluginId)]
#[plugin(id = "example.outline")]
struct OutlinePlugin;

impl Plugin for OutlinePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.render().pass(Stage::Overlay, || Outline);
    }
}
```

The pass is `HdPass` (a closure taking the `PassCx` is one too). The factory runs once for each client's world view when the
view attaches to the client, so every view has a pass of its own with its own GPU state (pipelines, buffers). Several passes
at one stage run in the order they were added. A stage no pass uses costs a frame one check.

### What a pass gets

A `PassCx` holds, for the call:

- **the device, the queue and the frame's command encoder**: a pass begins its own render passes in the encoder, which the
  renderer submits with the frame; a pass never submits;
- **the targets the stage has**, as views (`None` where a stage has none): `hdr` (linear light in `HDR_FORMAT`, from `Opaque` to
  `Post`), `depth` (`DEPTH_FORMAT`, **reversed**: 1 near, 0 far, so draw with `GreaterEqual`), `normals` and `motion` (targets at
  `Prepass`), `source` (at `Post`, a copy of the frame to read while writing it), `ldr` with its format (at `Overlay`, the finished
  picture, display values) and `cascades` (at `Shadows`: each one's light matrix, its map and its size);
- **the sizes**: `size` (the stage's targets), `frame_size` (the internal frame, which the depth, normals and motion have),
  `viewport` (where the world is in the color target: the whole target but at `Overlay`); set it on the render pass;
- **the camera** the stage draws with (`camera.view_proj`, column major, and `eye`), the jittered camera of the anti-aliasing up
  to `Translucent` and the plain one after it;
- **the scene's light and clock**: the sun (`towards`, `diffuse`, `ambient`, as the fixed-function frame lights with them, gamma
  0..1), the fog, the lights in view, the effects' time and the farthest the camera sees;
- **the frame's moving objects** (`ghosts`), the shapes the client draws as the frame draws them.

The module docs of `torque::app::render` and `torque_render_hd::passes` say each of these exactly, with the formats.

### Limits

- **The engine's renderer only.** A pass runs in the engine's own renderer. A client that draws with another renderer from the
  slot ([Plugins](plugins.md#renderers)) never runs it, and a dedicated server has no view at all.
- **The frame's targets are the frame's.** They are made again when the size, the quality or the features change, and the
  cascades' maps when the shadows' quality does. Keep nothing that names a target from one frame to the next: make bind groups over
  a target each frame (they are cheap) and keep only a pass's own pipelines, buffers and textures.
- **Where a pass may attach the depth.** `depth_attachable` says whether `depth` can be the depth attachment of a render pass into
  the stage's color target; at `AfterTaa`, `Post` and `Overlay` it is at the internal size and a pass reads it instead (as the
  debug lines do).
- **A shadow pass** draws plain depth (`LessEqual`, cleared to 1) into each cascade's map, and clamps a caster's clip depth to at
  least 0 so what lies between the sun and the cascade still casts. The cascades' maps are copied fresh from their cache every frame
  a shadows pass exists, so a moving caster leaves nothing behind. The frame's surfaces receive the shadows a pass drew; what a pass
  draws at `Opaque` receives none (the stage is not handed the cascades' maps).
- **`Send` on native targets.** A pass must be `Send` natively (`WasmNotSend`), so state it shares with the rest of the plugin sits
  behind an `Arc<Mutex<..>>` rather than an `Rc<RefCell<..>>`.
- **No motion vectors unless the pass draws them** at `Prepass`: what a pass draws at `Opaque` is anti-aliased with the motion of
  what lies behind it, so a fast thing smears a little.
- **Never panic in `draw`, and keep the work bounded.** The pass's CPU time is the frame's; cap what it builds each frame.

### Budgets

| What | Cap |
|---|---|
| passes, static batches and emitter kinds a client's view makes (of each) | `MAX_ADDS`: 64 (the first are kept, with a warning on the console) |
| triangles one static batch lays out | `MAX_BATCH_TRIANGLES`: 1,048,576 |
| triangles a view lays out over all its static batches | `MAX_VIEW_BATCH_TRIANGLES`: 4,194,304, or what `WorldView::set_static_batch_budget` sets |
| copies in one static batch | `MAX_BATCH_COPIES`: 1,048,576 |
| sprites one emitter-kind draw call puts out | `MAX_SPRITES_PER_CALL`: 20,000 |
| the longest step an emitter kind is run on in one frame | `MAX_EMITTER_STEP`: 0.25 s |

A stage's passes are not timed or capped on the GPU: a pass that draws too much costs the frame its time, so a plugin
makes its own caps (the rope plugin's: 256 ropes and 4,096 segments).

## Static batches

A `StaticBatchSource` hands a `StaticBatch`: one mesh and a list of transforms (rotation, uniform scale and translation), the
way a forest of one tree or a field of one rock is. The view cuts it into 64-unit chunks by where each copy stands and keeps each
chunk on the GPU as a cell of its own, culled there and casting into the cached shadows, so a frame costs nothing for it on the
CPU. The copies are laid out once for each change of the source's `generation`, so a batch is for what stays put. Copies past
the budgets are left out, in order, with a warning on the client's console.

## Emitter kinds

An `EmitterKind` is a particle emitter of a plugin's own: the view runs it by the frame's step (`advance`) and asks it to draw
(`draw`) into the frame's particles, where it is sorted with TGE's, faded where it meets a surface and lit by the sun, the sky and
the lights as TGE's smoke is. It reaches its sprites with `EmitterDraws::sprites`. A client makes one of each kind; a plugin
shares state with it through what its factory captures.

## Input, cameras, controls and assets

The other four registries have a chapter each. In short:

- **[Input sources](input-sources.md).** A device the engine does not read (a gamepad, a wheel, a motion sensor) is an
  `InputSource` the client polls once for each drawn frame (once per client pass with no window), and what it says reaches the
  action maps as TGE's `joystick<N>` buttons, axes and hats. A source that reads its device on a thread stamps its events with
  the time, and the client gives each axis as its mean over the span since the last poll. `FakeInput` and `FakeClock` drive
  sources in tests. `torque-plugin-gamepad` is one: gilrs natively, the browser's Gamepad API on the web.
- **[Camera modes](camera-modes.md).** A `CameraMode` says where the eye is for a name (`first-person`, `third-person`, `orbit`
  are the engine's). The client has one in use; `setCameraMode(name)` or `CameraModes::set` switches, and the eye eases from the
  old mode to the new instead of cutting. A mode that follows something keeps its state while another is in use, so a switch
  finds it in place.
- **[GUI controls](gui-controls.md).** A `GuiControlClass` is a control class with `#[derive(EngineClass)]` fields and
  methods for drawing, layout, the pointer and keys. Scripts make it as they make any control, and it is drawn and handled by
  its own class alone. A class cannot shadow one of the engine's own GUI classes (the build fails).
- **[Asset loaders](asset-loaders.md).** `engine.assets().loader(extensions, Spec, load)` reads a plugin's own file format through
  the engine's files, once for each file, with a size cap (`Spec::max_bytes`) and a say on whether files from untrusted
  sources (a server's downloads, a mod) are given to it. A `Filename` field of a datablock can name an asset type, and its
  preload fails with the reason when the file is missing, refused or over the cap. In a debug build an edited file reloads.

## Checking a pass

A pass is checked by drawing a small scene offscreen with and without it and counting the pixels that changed. `torque-app`'s
`tests/render_passes.rs` does this for a pass at the opaque stage and one at the overlay stage, a static batch and an emitter
kind, and the rope plugin's `tests/picture.rs` for a lit, shadow-casting rope between two posts (set
`TORQUE_ROPE_PICTURES=<folder>` to keep its pictures). A test of this kind has no window and skips itself when the machine has no
GPU. The pattern: make an `Engine` of the renderer plugins and the plugin under test, attach a `RendererSlot` made from it to a
`WorldView`, draw the same frames with and without the plugin, and compare pixels in the rectangle where the effect should be
and in the rest of the picture, where nothing should have changed.
