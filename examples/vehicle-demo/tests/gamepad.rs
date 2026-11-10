//! The demo with the gamepad plugin (the `gamepad` feature) over scripted pads: a button takes the seat, the left stick drives.
//! No real pad is read.
#![cfg(feature = "gamepad")]

use torque_example_vehicle_demo::client::{ClientOptions, DemoPlugins, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_plugin_gamepad::{FakePads, GamepadPlugin, PadButton};

use torque::engine::EngineBuilder;

fn steps(game: &mut VehicleDemo, ms: u32) {
    for _ in 0..ms / 32 {
        game.step(32);
    }
}

#[test]
fn a_pad_takes_the_seat_and_its_left_stick_drives_the_buggy() {
    let pads = FakePads::default();
    let files = GameFiles::bundled().expect("the game folder");
    let options = ClientOptions {
        net: NetMode::Loopback,
        name: "Ann".to_string(),
        port: 0,
        ..ClientOptions::default()
    };
    let for_plugin = pads.clone();
    let plugins = DemoPlugins::new(&files, &options)
        .expect("the plugins")
        .gamepad(GamepadPlugin::with_backend(move || for_plugin.clone()));
    let engine = EngineBuilder::new()
        .add_plugins(plugins)
        .build()
        .expect("the engine");
    let mut game = VehicleDemo::from_engine(&engine, files, options).expect("the client");
    assert_eq!(game.global("$Pref::Gamepad::DeadZone"), "0.15");
    game.host_game().expect("the hosted game");
    for _ in 0..320 {
        if game.in_world() && game.buggy_ghost().is_some() {
            break;
        }
        steps(&mut game, 32);
    }
    assert!(game.in_world(), "{:#?}", game.server_log);
    steps(&mut game, 2000);
    // Look down at the buggy as the mouse does, then X takes the seat.
    game.host.mouse_motion(0.0, 110.0);
    steps(&mut game, 300);
    pads.plug_in(0);
    pads.press(0, PadButton::West, true);
    steps(&mut game, 64);
    pads.press(0, PadButton::West, false);
    steps(&mut game, 600);
    assert!(game.driving(), "{:#?}", game.server_log);
    // The left stick pushed up is the throttle.
    let from = game.server.as_ref().unwrap().buggy_position().unwrap();
    pads.left_stick(0, 0.0, -1.0);
    steps(&mut game, 3000);
    pads.left_stick(0, 0.0, 0.0);
    let to = game.server.as_ref().unwrap().buggy_position().unwrap();
    assert!(to[1] - from[1] > 10.0, "from {from:?} to {to:?}");
}
