# Input sources from plugins

A plugin can bring a device the engine does not read: a gamepad, a wheel, a motion sensor, a network controller. The plugin adds an
**input source** while the engine is built; the client polls it once per drawn frame; and what it says arrives in the scripts as
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

Polling follows the frame rate, as TGE polls its joysticks once per frame: a bound axis that moves the look or the walk speed
changes at the rate the window draws (144 times a second on a 144 Hz display), not at the 32 ms tick. The moves still go to the
server at the tick, from the `$mv*` variables the binds set, and the look is drawn between the ticks from the same variables
as the mouse's is. A host that draws no frames (a dedicated server, a headless run, an offscreen picture) polls once per client
pass instead; a frame never polls twice.

A device can be read faster than that, apart from the frame rate: see
[Timestamps and reading on a thread](#timestamps-and-reading-on-a-thread).

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

## Timestamps and reading on a thread

A source that reads its device on a thread of its own, faster than the client polls, stamps what it says with the time it
happened. It pushes into an `InputQueue` (a clonable, thread-safe queue) from the thread, and drains it in `poll`:

```rust
use torque::client::input_source::{InputCx, InputQueue, InputSource};

struct Wheel(InputQueue);

impl InputSource for Wheel {
    fn poll(&mut self, cx: &mut InputCx<'_>) {
        // Everything the thread left since the last poll, oldest first.
        cx.drain(&self.0);
    }
}

// On the thread: queue.axis_at(Instant::now(), 0, JoyAxis::X, read_the_wheel());
```

`InputCx::axis_at`, `button_at`, `pov_at`, `disconnect_at` and `push_at` stamp an event directly. The stamp is a monotonic
`Instant` taken before the event was pushed. What the client does with stamps:

- **An axis** is given to the scripts as the **time-weighted mean** of its value over the span since the last poll (a frame's,
  or a client pass's where nothing is drawn). A stick that sat at 0 for 5 ms and at 1 for the other 5 ms of a 10 ms frame reads
  0.5; a span in which the source said nothing holds the last value. A span of no time says nothing new.
- **Buttons, hats and disconnects** keep the exact order they were said in; they are not averaged, and a slow frame loses none
  of them. An axis mean follows the frame's other events.
- An event with **no stamp** is the device's value at the poll, as before; a source that stamps nothing behaves exactly as it
  did. A source should not mix stamped and unstamped events for one axis.
- A poll takes at most 65,536 stamped events (an `InputQueue` holds as many; past that it drops new axis moves and keeps
  the rest).

The mean is taken per poll: in a window that is per drawn frame, and headless per client pass (one per tick). A tick that runs
inside a frame the window already polled uses that frame's values, as it did before.

## Testing

`FakeInput` (behind `torque-client`'s `testing` feature, which a dev-dependency turns on; the `torque` crate forwards it as its own
`testing` feature) is a source a test drives: it is `Clone`, and the copy the test keeps says what the devices do at the next poll.
`torque-app`'s `tests/input_sources.rs` binds `joystick0` and `joystick1` this way and checks what a script's functions get.

```rust
use torque::client::input_source::{FakeInput, JoyAxis};

let pad = FakeInput::default();
// engine.add_input_source(pad.clone()), then build the client and bind joystick0 in a script ...
pad.axis(0, JoyAxis::X, 0.5);
// ... the next poll (a frame's, or a client pass's where nothing is drawn): the bound function gets 0.5.
```

For stamped events use `torque_testkit::FakeClock` (the workspace's test crate, `crates/torque-testkit`, added as a path
dev-dependency): the registry reads its time from it (`input_sources.set_clock(clock.reader())`), and the test stamps
with times of it (`clock.at(5)` is 5 ms after the clock began; `clock.advance(...)` moves it on), so a test never waits and
the means are exact.

```rust
use std::time::Duration;
use torque::client::input_source::{FakeInput, JoyAxis};
use torque_testkit::FakeClock;

let (clock, pad) = (FakeClock::default(), FakeInput::default());
// registry.set_clock(clock.reader()); engine.add_input_source(pad.clone()) ...
pad.axis_at(clock.at(0), 0, JoyAxis::X, 0.0);
pad.axis_at(clock.at(5), 0, JoyAxis::X, 1.0);
clock.advance(Duration::from_millis(10));
// ... the next poll: the bound function gets 0.5.
```

Never read a real device in a test. A test that opens a real gamepad depends on what is plugged in.

## The gamepad plugin

`torque-plugin-gamepad` is a source of this kind: `GamepadPlugin::default()` reads the pads plugged in (gilrs natively, the
browser's Gamepad API on the web), gives each the lowest free `joystick<N>`, and has the prefs `$Pref::Gamepad::DeadZone`,
`$Pref::Gamepad::InvertY`, `$Pref::Gamepad::PollRate` and `$Pref::Gamepad::WebFastPoll`. Natively it reads the pads on a thread
of its own, 500 times a second by default, with stamped events, so a stick is the mean over each frame and not the value at the
instant of the draw (the thread starts with the client's first poll and is joined when the client goes;
`GamepadPlugin::without_thread()` reads at each poll instead). In a browser the Gamepad API exists on the page's thread only, so
the pads are read at each poll; `$Pref::Gamepad::WebFastPoll` (off by default) adds a timer about every 4 ms, which browsers may
stretch and stop on a hidden page. It is not in `DefaultPlugins`. Its README lists what each stick, trigger and button becomes. The
vehicle demo builds with it behind its `gamepad` feature (`--gamepad` at run time).
