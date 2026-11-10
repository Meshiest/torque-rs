# torque-plugin-gamepad

Gamepads for a torque-rs game: the pads plugged in appear as the `joystick<N>` devices a script binds, as a DirectInput
joystick does in the Torque Game Engine. Natively the pads come from [gilrs](https://crates.io/crates/gilrs); in a browser from
the Gamepad API (`navigator.getGamepads()`). The same scripts run on both.

It is not part of the default plugins: a game adds it.

```rust
use torque::app::plugins::DefaultPlugins;
use torque::engine::EngineBuilder;
use torque_plugin_gamepad::GamepadPlugin;

let engine = EngineBuilder::new()
    .add_plugins((DefaultPlugins, GamepadPlugin::default()))
    .build()?;
```

The game's scripts then bind it:

```
PlayMap.bind(joystick0, xaxis, "S", 2.0, walkSideways);   // the left stick, scaled
PlayMap.bind(joystick0, ryaxis, lookUpDown);              // the right stick
PlayMap.bind(joystick0, button0, jump);                   // A on an Xbox pad
PlayMap.bind(joystick0, upov, selectPrevious);            // the cross, up
PlayMap.bind(joystick1, button0, jump);                   // a second pad
```

| The pad | The joystick |
|---|---|
| left stick | `xaxis`, `yaxis` |
| right stick | `rxaxis`, `ryaxis` |
| left and right triggers | `zaxis`, `rzaxis` (0 to 1), and `button11`, `button12` when pressed in half way |
| South, East, West, North | `button0` to `button3` |
| shoulders, Select, Start, the sticks pressed, the logo button | `button4` to `button10` |
| the cross | `xpov`, `ypov`, `upov`, `dpov`, `lpov`, `rpov` |

Y grows downward, as the joystick axes and the mouse's do. Pads can be plugged in and out while the game runs: each takes the
lowest free `joystick<N>` and keeps it, and a pad that goes lets go of every button and stick it held.

## Prefs

| Pref | Default | Does |
|---|---|---|
| `$Pref::Gamepad::DeadZone` | 0.15 (0 to 0.9) | takes the middle out of each stick and spreads the rest over the full range |
| `$Pref::Gamepad::InvertY` | 0 | swaps up and down on both sticks |
| `$Pref::Gamepad::PollRate` | 500 (30 to 2000) | how many times a second the reading thread reads the pads (native) |
| `$Pref::Gamepad::WebFastPoll` | 0 | in a browser, also read the pads from a timer about every 4 ms |

They are ordinary `$Pref` globals: a game that exports its prefs saves them, and a settings screen sets them. A change takes
hold at the next poll.

## Reading apart from the frame rate

Natively the pads are read on a thread of their own, 500 times a second, and the client gives each stick as its mean over the
time since its last poll, so a stick is not sampled once per drawn frame. Buttons keep their order, and a slow frame loses
nothing. The thread starts at the first poll of a client and is joined when the client goes. `GamepadPlugin::default()
.without_thread()` reads at each poll instead. In a browser the Gamepad API is available on the page's thread only, so the pads
are read at each poll; `$Pref::Gamepad::WebFastPoll` adds a timer (about every 4 ms; browsers stretch it, and stop it on a
hidden page) that reads them between frames.

## Building

Natively the crate uses gilrs, which is Apache-2.0 or MIT. On Linux gilrs reads the pads through udev, so building needs the
libudev development files (`libudev-dev` on Debian and Ubuntu). For the web the crate uses `web-sys` only (no gilrs), and a
browser reports a pad to a page once the player has pressed a button on it.

## Tests

`FakePads` is a scripted set of pads for tests (`GamepadPlugin::with_backend(move || pads.clone())`): plug pads in and out and
move their sticks, and bind a script to them. No test reads a real pad.
