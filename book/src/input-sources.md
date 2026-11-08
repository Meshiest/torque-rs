# Input sources from plugins

A plugin can bring a device the engine does not read: a gamepad, a wheel, a motion sensor, a network controller. The plugin adds an
**input source** while the engine is built; the client polls it once per client pass; and what it says arrives in the scripts as
TGE's `joystick<N>` events. A game's scripts bind them in an action map as they would bind a DirectInput joystick:

```
%map.bind(joystick0, xaxis, "I", moveLeftRight);
%map.bindCmd(joystick0, button0, "commandToServer('Jump');", "");
%map.bind(joystick1, xaxis, steer);
```

The device number is the joystick's: a source that says things about device 1 is `joystick1`. A second pad binds apart from the
first, with its own maps.

A function binds to one key of a map: `bind(joystick0, button0, jump)` after `bind(keyboard, space, jump)` moves `jump` to the
pad's button (this is TGE's `ActionMap`). A pad's binds call functions of their own (`function padJump(%v) { jump(%v); }`).

## A source

```rust
use torque::client::input_source::{InputBuilderExt, InputCx, InputSource, JoyAxis};
use torque::engine::{EngineBuilder, Plugin, PluginId};

/// A wheel on joystick 0: its angle is the X axis, its paddles buttons 0 and 1.
#[derive(Clone, Default)]
struct Wheel;

impl InputSource for Wheel {
    fn poll(&mut self, cx: &mut InputCx<'_>) {
        let (angle, left, right) = read_the_wheel();
        cx.axis(0, JoyAxis::X, angle);
        cx.button(0, 0, left);
        cx.button(0, 1, right);
    }
}
# fn read_the_wheel() -> (f32, bool, bool) { (0.0, false, false) }

#[derive(PluginId)]
#[plugin(id = "example.wheel")]
struct WheelPlugin;

impl Plugin for WheelPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_input_source(Wheel);
    }
}
```

`poll` says what the devices are **now**; it does not need to remember what it said before. The client keeps what each joystick
held, and only passes on changes: an axis at the value it already had, a button already down and a hat already there make no
event. A source whose devices come and go says `cx.disconnect(device)` when one goes: the client then lets go of everything that
joystick held (its buttons break, its hats center, its axes move back to 0), so a bound `break` command or a held direction never
sticks.

| `InputCx` call | Reaches the scripts as |
|---|---|
| `button(device, n, down)` | `button<n>` make and break (n from 0 to 31) |
| `axis(device, JoyAxis::X, v)` | a move of `xaxis` (`yaxis`, `zaxis`, `rxaxis`, `ryaxis`, `rzaxis`, `slider`), v from -1 to 1 |
| `pov(device, hat, x, y)` | `xpov` and `ypov` moves, and `upov`, `dpov`, `lpov`, `rpov` make and break (hat 1 is the `...pov2` names) |
| `disconnect(device)` | everything the device held let go |

Y grows downward, as DirectInput's and the mouse's do. A source can read prefs from the client's script engine
(`cx.vm().prefs::<MyPrefs>()`), so a dead zone is a `$Pref::...` the player can set.

`add_input_source` takes a source that is `Clone` and gives each client its own copy; `add_input_source_with(|| ...)` takes a
function that makes one for each client, for a source that opens its devices when it is made. Events go the way a key's do: the
global action map first, then the canvas, then the other maps from the top.

## Testing

`FakeInput` is a source a test drives: it is `Clone`, and the copy the test keeps says what the devices do at the next poll.
`torque-app`'s `tests/input_sources.rs` binds `joystick0` and `joystick1` this way and checks what a script's functions get.

```rust
use torque::client::input_source::{FakeInput, JoyAxis};

let pad = FakeInput::default();
// engine.add_input_source(pad.clone()), then build the client and bind joystick0 in a script ...
pad.axis(0, JoyAxis::X, 0.5);
// ... the next client pass polls it: the bound function gets 0.5.
```

Never read a real device in a test. A test that opens a real gamepad depends on what is plugged in.

## The gamepad plugin

`torque-plugin-gamepad` is a source of this kind: `GamepadPlugin::default()` reads the pads plugged in (gilrs natively, the
browser's Gamepad API on the web), gives each the lowest free `joystick<N>`, and has the prefs `$Pref::Gamepad::DeadZone` and
`$Pref::Gamepad::InvertY`. It is not in `DefaultPlugins`. Its README lists what each stick, trigger and button becomes. The
vehicle demo builds with it behind its `gamepad` feature (`--gamepad` at run time).
