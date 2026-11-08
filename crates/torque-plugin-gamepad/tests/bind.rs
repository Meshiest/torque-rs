//! The gamepad plugin over scripted pads ([`FakePads`]): a script binds `joystick0` and `joystick1`, the dead zone and the
//! inverted Y prefs shape the sticks, pads plug in and out, and the plugin is not in the default plugins. No real pad is read.

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_engine::{EngineBuilder, PluginGroup, PluginType};
use torque_plugin_gamepad::{FakePads, GamepadPlugin, PadButton};

fn client(pads: &FakePads, binds: &str) -> GameClient<GameServer> {
    let pads = pads.clone();
    let engine = EngineBuilder::new()
        .add_plugins((
            DefaultPlugins,
            GamepadPlugin::with_backend(move || pads.clone()),
        ))
        .build()
        .expect("the engine builds");
    let mut builder =
        ClientBuilder::from_engine(&engine, None, ClientNet::None).expect("the client");
    let script = format!(
        "new ActionMap(GlobalActionMap); GlobalActionMap.push(); new ActionMap(TestMap); {binds} TestMap.push();"
    );
    builder.vm.eval(script.as_bytes()).expect("the binds");
    builder.build::<GameServer>()
}

fn pass(client: &mut GameClient<GameServer>) {
    client.step(32);
}

fn values(client: &GameClient<GameServer>, name: &str) -> Vec<f32> {
    log(client, name)
        .split(';')
        .filter(|v| !v.is_empty())
        .map(|v| v.parse().expect("a number"))
        .collect()
}

fn log(client: &GameClient<GameServer>, name: &str) -> String {
    String::from_utf8_lossy(
        &client
            .vm()
            .global(&format!("$log_{name}"))
            .unwrap_or_default(),
    )
    .into_owned()
}

/// The functions the binds call: each one appends its value to the global `$log_<name>`.
const FUNCTIONS: &str = concat!(
    "function x0(%v) { $log_x0 = $log_x0 @ %v @ \";\"; }",
    "function x1(%v) { $log_x1 = $log_x1 @ %v @ \";\"; }",
    "function y0(%v) { $log_y0 = $log_y0 @ %v @ \";\"; }",
    "function ry0(%v) { $log_ry0 = $log_ry0 @ %v @ \";\"; }",
    "function a0(%v) { $log_a0 = $log_a0 @ %v @ \";\"; }",
    "function lb0(%v) { $log_lb0 = $log_lb0 @ %v @ \";\"; }",
    "function lt0(%v) { $log_lt0 = $log_lt0 @ %v @ \";\"; }",
    "function z0(%v) { $log_z0 = $log_z0 @ %v @ \";\"; }",
    "function up0(%v) { $log_up0 = $log_up0 @ %v @ \";\"; }",
);

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{actual:?} against {expected:?}"
    );
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-4, "{actual:?} against {expected:?}");
    }
}

#[test]
fn the_left_stick_binds_as_joystick0_xaxis_through_the_dead_zone() {
    let pads = FakePads::default();
    let mut client = client(
        &pads,
        &format!("{FUNCTIONS} TestMap.bind(joystick0, xaxis, x0);"),
    );
    pads.plug_in(3);
    // Inside the dead zone: nothing.
    pads.left_stick(3, 0.1, 0.0);
    pass(&mut client);
    // Past it, spread over the rest of the travel: (0.5 - 0.15) / 0.85.
    pads.left_stick(3, 0.5, 0.0);
    pass(&mut client);
    pads.left_stick(3, -1.0, 0.0);
    pass(&mut client);
    pads.left_stick(3, -0.05, 0.0);
    pass(&mut client);
    assert_close(&values(&client, "x0"), &[0.411765, -1.0, 0.0]);
    assert!(client.host.input_errors.is_empty());
}

#[test]
fn the_dead_zone_and_inverted_y_prefs_change_what_the_next_poll_says() {
    let pads = FakePads::default();
    let mut client = client(
        &pads,
        &format!(
            "{FUNCTIONS} TestMap.bind(joystick0, yaxis, y0); TestMap.bind(joystick0, ryaxis, ry0);"
        ),
    );
    assert_eq!(client.global("$Pref::Gamepad::DeadZone"), "0.15");
    assert_eq!(client.global("$Pref::Gamepad::InvertY"), "0");
    pads.plug_in(0);
    // Pushed up: negative, as the joystick's Y reads.
    pads.left_stick(0, 0.0, -1.0);
    pads.right_stick(0, 0.0, 0.575);
    pass(&mut client);
    assert_close(&values(&client, "y0"), &[-1.0]);
    assert_close(&values(&client, "ry0"), &[0.5]);
    // A wider dead zone takes the right stick's push back inside it.
    client.set_global("$Pref::Gamepad::DeadZone", "0.6");
    pass(&mut client);
    assert_close(&values(&client, "ry0"), &[0.5, 0.0]);
    // Inverted: the left stick up reads positive.
    client.set_global("$Pref::Gamepad::DeadZone", "0");
    client.set_global("$Pref::Gamepad::InvertY", "1");
    pass(&mut client);
    assert_close(&values(&client, "y0"), &[-1.0, 1.0]);
}

#[test]
fn buttons_triggers_and_the_cross_reach_their_joystick_names() {
    let pads = FakePads::default();
    let mut client = client(
        &pads,
        &format!(
            "{FUNCTIONS} \
             TestMap.bindCmd(joystick0, button0, \"a0(1);\", \"a0(0);\"); \
             TestMap.bindCmd(joystick0, button4, \"lb0(1);\", \"lb0(0);\"); \
             TestMap.bindCmd(joystick0, button11, \"lt0(1);\", \"lt0(0);\"); \
             TestMap.bind(joystick0, zaxis, z0); \
             TestMap.bindCmd(joystick0, upov, \"up0(1);\", \"up0(0);\");"
        ),
    );
    pads.plug_in(0);
    pads.press(0, PadButton::South, true);
    pads.press(0, PadButton::LeftShoulder, true);
    pads.press(0, PadButton::DPadUp, true);
    pads.triggers(0, 0.75, 0.0);
    pass(&mut client);
    pads.press(0, PadButton::South, false);
    pads.press(0, PadButton::DPadUp, false);
    pads.triggers(0, 0.25, 0.0);
    pass(&mut client);
    assert_close(&values(&client, "a0"), &[1.0, 0.0]);
    assert_close(&values(&client, "lb0"), &[1.0]);
    assert_close(&values(&client, "up0"), &[1.0, 0.0]);
    assert_close(&values(&client, "lt0"), &[1.0, 0.0]);
    assert_close(&values(&client, "z0"), &[0.75, 0.25]);
}

#[test]
fn a_second_pad_is_joystick1_and_a_pad_that_goes_lets_go_and_frees_its_number() {
    let pads = FakePads::default();
    let mut client = client(
        &pads,
        &format!(
            "{FUNCTIONS} \
             TestMap.bind(joystick0, xaxis, x0); \
             TestMap.bind(joystick1, xaxis, x1); \
             TestMap.bindCmd(joystick0, button0, \"a0(1);\", \"a0(0);\");"
        ),
    );
    pads.plug_in(40);
    pads.plug_in(41);
    pads.left_stick(40, 1.0, 0.0);
    pads.left_stick(41, -1.0, 0.0);
    pads.press(40, PadButton::South, true);
    pass(&mut client);
    assert_close(&values(&client, "x0"), &[1.0]);
    assert_close(&values(&client, "x1"), &[-1.0]);
    // The first pad is unplugged while it holds A and its stick: both let go.
    pads.unplug(40);
    pass(&mut client);
    assert_close(&values(&client, "x0"), &[1.0, 0.0]);
    assert_close(&values(&client, "a0"), &[1.0, 0.0]);
    // The second keeps its number; a new pad takes the free one.
    pads.plug_in(42);
    pads.left_stick(42, 0.6, 0.0);
    pass(&mut client);
    assert_close(&values(&client, "x1"), &[-1.0]);
    assert_close(&values(&client, "x0"), &[1.0, 0.0, 0.529412]);
}

#[test]
fn the_plugin_is_not_in_the_default_plugins() {
    let ids: Vec<String> = DefaultPlugins
        .build()
        .ids()
        .map(|id| id.as_str().to_owned())
        .collect();
    assert!(
        !ids.iter().any(|id| id == GamepadPlugin::ID.as_str()),
        "{ids:?}"
    );
}
