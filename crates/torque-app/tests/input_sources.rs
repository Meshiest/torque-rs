//! Input sources that plugins add: a fake source drives `bind(joystick0, xaxis, f)` and the buttons and hats of a joystick, a
//! second joystick binds apart from the first, a joystick that goes lets go of what it held, and an engine with no source polls
//! nothing.

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_client::input_source::{FakeInput, InputBuilderExt, JoyAxis};
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId};

#[derive(PluginId)]
#[plugin(id = "test.fake-pad")]
struct FakePadPlugin(FakeInput);

impl Plugin for FakePadPlugin {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.add_input_source(self.0.clone());
    }
}

/// A client whose first action map binds `binds` (lines of `TestMap.bind(...)`), over an engine with `pad` as its source.
fn client(pad: &FakeInput, binds: &str) -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, FakePadPlugin(pad.clone())))
        .build()
        .expect("the engine builds");
    client_of(&engine, binds)
}

fn client_of(engine: &Engine, binds: &str) -> GameClient<GameServer> {
    let mut builder =
        ClientBuilder::from_engine(engine, None, ClientNet::None).expect("the client");
    let script = format!(
        "new ActionMap(GlobalActionMap); GlobalActionMap.push(); new ActionMap(TestMap); {binds} TestMap.push();"
    );
    builder.vm.eval(script.as_bytes()).expect("the binds");
    builder.build::<GameServer>()
}

fn log(client: &GameClient<GameServer>, name: &str) -> String {
    String::from_utf8_lossy(&client.vm().global(name).unwrap_or_default()).into_owned()
}

/// A pass of the client's clock: the sources are polled before its tick.
fn pass(client: &mut GameClient<GameServer>) {
    client.step(32);
}

const RECORDERS: &str = "\
    function onX(%v) { $x = $x @ %v @ \";\"; }\
    function onXpad1(%v) { $x1 = $x1 @ %v @ \";\"; }\
    function onXinv(%v) { $xi = $xi @ %v @ \";\"; }\
    function onDown(%v) { $b = $b @ \"d\" @ %v @ \";\"; }\
    function onUp(%v) { $b = $b @ \"u\" @ %v @ \";\"; }\
    function onPad1Down(%v) { $b1 = $b1 @ \"d\" @ %v @ \";\"; }\
    function onPad1Up(%v) { $b1 = $b1 @ \"u\" @ %v @ \";\"; }\
    function onRight(%v) { $pov = $pov @ \"r\" @ %v @ \";\"; }\
    function onRightUp(%v) { $pov = $pov @ \"R\" @ %v @ \";\"; }\
    function onXpov(%v) { $pov = $pov @ \"x\" @ %v @ \";\"; }";

#[test]
fn bind_joystick0_xaxis_gets_the_axis_values_a_fake_source_says() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!("{RECORDERS} TestMap.bind(joystick0, xaxis, onX);"),
    );
    pad.axis(0, JoyAxis::X, 0.5);
    pass(&mut client);
    pad.axis(0, JoyAxis::X, -0.25);
    pass(&mut client);
    // The same value again says nothing; a value past the end is clamped; the center is a move to 0.
    pad.axis(0, JoyAxis::X, -0.25);
    pass(&mut client);
    pad.axis(0, JoyAxis::X, 3.0);
    pass(&mut client);
    pad.axis(0, JoyAxis::X, 0.0);
    pass(&mut client);
    assert_eq!(log(&client, "$x"), "0.5;-0.25;1;0;");
    assert!(client.host.input_errors.is_empty());
}

#[test]
fn a_bind_can_invert_and_scale_a_joysticks_axis_as_it_does_a_directinput_one() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!("{RECORDERS} TestMap.bind(joystick0, xaxis, \"SI\", 2.0, onXinv);"),
    );
    pad.axis(0, JoyAxis::X, 0.25);
    pass(&mut client);
    assert_eq!(log(&client, "$xi"), "-0.5;");
}

#[test]
fn buttons_make_and_break_and_a_second_joystick_binds_apart() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!(
            "{RECORDERS} \
             TestMap.bind(joystick0, xaxis, onX); \
             TestMap.bind(joystick1, xaxis, onXpad1); \
             TestMap.bindCmd(joystick0, button3, \"onDown(1);\", \"onUp(0);\"); \
             TestMap.bindCmd(joystick1, button3, \"onPad1Down(1);\", \"onPad1Up(0);\");"
        ),
    );
    pad.button(1, 3, true);
    pad.axis(1, JoyAxis::X, 0.75);
    pass(&mut client);
    pad.button(0, 3, true);
    pad.axis(0, JoyAxis::X, -1.0);
    pass(&mut client);
    pad.button(1, 3, false);
    pad.button(0, 3, false);
    pass(&mut client);
    assert_eq!(log(&client, "$x"), "-1;");
    assert_eq!(log(&client, "$x1"), "0.75;");
    assert_eq!(log(&client, "$b"), "d1;u0;");
    assert_eq!(log(&client, "$b1"), "d1;u0;");
}

#[test]
fn a_hat_makes_its_direction_and_moves_its_axis() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!(
            "{RECORDERS} \
             TestMap.bindCmd(joystick0, rpov, \"onRight(1);\", \"onRightUp(0);\"); \
             TestMap.bind(joystick0, xpov, onXpov);"
        ),
    );
    pad.pov(0, 0, 1, 0);
    pass(&mut client);
    pad.pov(0, 0, 0, 0);
    pass(&mut client);
    assert_eq!(log(&client, "$pov"), "x1;r1;R0;x0;");
}

#[test]
fn a_joystick_that_is_unplugged_lets_go_of_what_it_held() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!(
            "{RECORDERS} \
             TestMap.bind(joystick1, xaxis, onXpad1); \
             TestMap.bindCmd(joystick1, button3, \"onPad1Down(1);\", \"onPad1Up(0);\");"
        ),
    );
    pad.button(1, 3, true);
    pad.axis(1, JoyAxis::X, 0.5);
    pass(&mut client);
    pad.disconnect(1);
    pass(&mut client);
    assert_eq!(log(&client, "$b1"), "d1;u0;");
    assert_eq!(log(&client, "$x1"), "0.5;0;");
}

#[test]
fn events_reach_the_maps_the_way_a_keys_do_the_top_map_first() {
    let pad = FakeInput::default();
    let mut client = client(
        &pad,
        &format!("{RECORDERS} TestMap.bind(joystick0, xaxis, onX);"),
    );
    // The global map was pushed first and sits below: it takes an event first, when it binds it.
    client
        .vm_mut()
        .eval(b"function onGlobalX(%v) { $g = $g @ %v @ \";\"; } GlobalActionMap.bind(joystick0, xaxis, onGlobalX);")
        .expect("the global bind");
    pad.axis(0, JoyAxis::X, 0.5);
    pass(&mut client);
    assert_eq!(log(&client, "$g"), "0.5;");
    assert_eq!(log(&client, "$x"), "");
}

#[test]
fn an_engine_without_a_source_polls_nothing() {
    let engine = EngineBuilder::new()
        .add_plugins(DefaultPlugins)
        .build()
        .expect("the engine builds");
    let client = client_of(&engine, "");
    assert!(client.host.client.borrow().input_sources.is_empty());
}

#[test]
fn every_client_an_engine_makes_has_its_own_copy_of_each_source() {
    let pad = FakeInput::default();
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, FakePadPlugin(pad.clone())))
        .build()
        .expect("the engine builds");
    let one = client_of(&engine, "");
    let two = client_of(&engine, "");
    assert_eq!(one.host.client.borrow().input_sources.len(), 1);
    assert_eq!(two.host.client.borrow().input_sources.len(), 1);
}
