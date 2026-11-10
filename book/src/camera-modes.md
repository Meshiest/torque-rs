# Camera modes from plugins

The eye that draws the world can come from more than one place: the figure's own eyes, a camera on a boom behind it, an orbit around
it, a crane over the map. A plugin adds a **camera mode** while the engine is built; the client has the one in use by name; the
game switches by name, from a script or from Rust; and a switch eases the eye from the old mode to the new one instead of cutting.

```
setCameraMode("third-person");    // 1 if there is a mode by that name, else 0 and a warning in the console
echo(getCameraMode());            // third-person
echo(getCameraModes());           // first-person third-person orbit crane
```

A script's spelling may differ in case. A client starts in the first person, as TGE's does.

## The built-in modes

| Name | The eye |
|---|---|
| `first-person` | the figure's eyes, looking where the figure looks |
| `third-person` | on a boom back from above the figure's feet along its look, pulled in fast in front of what comes between and let out slowly when it goes, looking where the figure looks |
| `orbit` | circling a point above the figure; the frame's look turns it and the wheel moves it in and out |

A game that wants other numbers adds a mode under the same name (the later one replaces the earlier): the level demo gives
`first-person` and `third-person` its own `Rig` (distance, boom height, lift, nearest, clearance); the vehicle demo's modes are
its own, because a seat changes what each of them means. `Tab` in both demos is `setCameraMode(getCameraMode() $= "first-person" ? "third-person" : "first-person")`.

## A mode

```rust
use torque::app::camera_modes::{CameraBuilderExt, CameraCx, CameraMode};
use torque::engine::{EngineBuilder, Plugin, PluginId};
use torque_render_scene::source::Eye;

/// A camera 12 m over the figure, looking straight down.
struct Crane;

impl CameraMode for Crane {
    fn eye(&self, cx: &CameraCx<'_>) -> Option<Eye> {
        let f = cx.control?;
        Some(Eye {
            position: [f.position[0], f.position[1], f.position[2] + 12.0],
            yaw: 0.0,
            pitch: std::f32::consts::FRAC_PI_2,
            fov: None,
        })
    }
}

#[derive(PluginId)]
#[plugin(id = "example.crane")]
struct CranePlugin;

impl Plugin for CranePlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_camera_mode("crane", || Crane);
    }
}
```

Only `eye` is required. It reads a `CameraCx`: the control object (`cx.control`, the figure as the frame placed it; none before
there is one), the frame's time (`cx.dt`), the frame's input (`cx.input.look`, `cx.input.zoom`) and the world's surroundings
(`cx.world`). An eye of `None` means "no view": the game's fallback is drawn. The rest of the trait is optional:

| Method | When |
|---|---|
| `update(cx, active)` | every frame, on every mode, `active` for the one in use: a boom follows even while another mode is in use, so a switch finds it in place |
| `enter(cx)` / `leave()` | the frame a switch starts |
| `blend_in()` / `blend_out()` | the seconds an ease into or out of this mode takes (none: 0.24 s; 0 cuts) |
| `keep(cx, point)` | moves a place the ease puts the camera out of walls and ground |
| `keeps_look(cx)` | the view drawn is this mode's own look whatever the ease did: the ease moves the camera and never turns it |
| `reset()` | a cut: forget what you followed |

`CameraWorld` is what the game answers for a camera to keep clear: `clear_share`, `clear_hit` (a line of sight through the
cubes, interiors, terrain and water) and `ground`; every answer defaults to "nothing in the way". A mode that needs more reads the
game's own type with `cx.world_as::<MyGame>()`.

## A frame

The game makes the context and calls three things on the client's modes (`game.camera_modes()`):

```rust
let cx = CameraCx::new(&game, dt).with_control(figure.as_ref()).with_input(modes.take_input());
modes.begin(&cx);                    // a switch since the last frame starts the ease; the ease's clock moves on
modes.update(&cx);                   // every mode moves on its state
let eye = modes.drawn(&cx, fixed);   // the eye of the mode in use, eased after a switch; `fixed` when it has none
```

A new figure or a teleport is `modes.cut()`: every mode forgets what it followed and the camera is at the new place at once.
`modes.set_now(name)` switches with no ease. `modes.target(&cx, fixed)` is the mode's own eye with no ease, and `modes.push_input(look,
zoom)` adds to the frame's input.

The ease is the engine's `CameraBlend`: the drawn eye is kept as an offset from the new mode's eye, and only the offset falls to
zero (a critically damped spring), so a switch while walking or driving never lags behind or stops the camera.

## Testing

A mode needs no window: make a client over the engine, switch, and call the frame as above with a figure you built. `torque-app`'s
`tests/camera_modes.rs` registers a crane from a plugin, switches to it from a script and from Rust and asserts the eyes frame by
frame. The level and vehicle demos' own camera tests (`tab_*_never_turns_the_view`, the steady eye, the respawn camera) run
through the same modes.
