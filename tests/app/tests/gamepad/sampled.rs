//! The gamepad plugin over stamped samples: sticks read as their mean over a frame, buttons in order, nothing lost in a slow
//! frame, and the reading thread that starts with the first poll and ends with the client. Fake pads and a fake clock only; no
//! real pad is read.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use torque_app::plugins::DefaultPlugins;
use torque_app::{ClientBuilder, ClientNet, GameClient, GameServer};
use torque_client::input_source::{FakeClock, InputBuilderExt};
use torque_engine::{EngineBuilder, Plugin, PluginId, PluginType};
use torque_plugin_gamepad::{
    FakePads, GamepadPlugin, GamepadPrefs, GamepadSource, PadBackend, PadButton, PadSamples,
    PadState,
};

/// A plugin that gives each client a source over `samples`, which the test fills by hand.
#[derive(Clone, PluginId)]
#[plugin(id = "test.sampled-pads")]
struct SampledPads(PadSamples);

impl Plugin for SampledPads {
    fn build(&self, engine: &mut EngineBuilder) {
        engine.client_script().prefs::<GamepadPrefs>();
        let samples = self.0.clone();
        engine.add_input_source_with(move || GamepadSource::sampled(samples.clone(), None));
    }
}

fn client_with(plugin: impl PluginType, binds: &str) -> GameClient<GameServer> {
    let engine = EngineBuilder::new()
        .add_plugins((DefaultPlugins, plugin))
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

const FUNCTIONS: &str = concat!(
    "function x0(%v) { $log_x0 = $log_x0 @ %v @ \";\"; }",
    "function a0(%v) { $log_a0 = $log_a0 @ %v @ \";\"; }",
    "function b0(%v) { $log_b0 = $log_b0 @ %v @ \";\"; }",
);

fn values(client: &GameClient<GameServer>, name: &str) -> Vec<f32> {
    String::from_utf8_lossy(
        &client
            .vm()
            .global(&format!("$log_{name}"))
            .unwrap_or_default(),
    )
    .split(';')
    .filter(|v| !v.is_empty())
    .map(|v| v.parse().expect("a number"))
    .collect()
}

fn close(actual: &[f32], expected: &[f32]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{actual:?} against {expected:?}"
    );
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-4, "{actual:?} against {expected:?}");
    }
}

/// A pad with the left stick at `x` and `buttons` down.
fn pad(x: f32, buttons: &[PadButton]) -> PadState {
    let mut state = PadState::default();
    state.left = (x, 0.0);
    for button in buttons {
        state.set(*button, true);
    }
    state
}

/// A client over hand-filled samples, with its polls reading a fake clock.
fn sampled_client(binds: &str) -> (GameClient<GameServer>, PadSamples, FakeClock) {
    let samples = PadSamples::default();
    let mut client = client_with(
        SampledPads(samples.clone()),
        &format!("{FUNCTIONS} {binds}"),
    );
    client.set_global("$Pref::Gamepad::DeadZone", "0");
    let clock = FakeClock::default();
    client
        .host
        .client
        .borrow_mut()
        .input_sources
        .set_clock(clock.reader());
    (client, samples, clock)
}

#[test]
fn a_stick_that_steps_mid_frame_reads_as_its_mean() {
    let (mut client, samples, clock) = sampled_client("TestMap.bind(joystick0, xaxis, x0);");
    samples.state(clock.at(0), 7, pad(0.0, &[]));
    client.step(32);
    // 5 ms at 0 and 5 ms at 1 of a 10 ms frame; the next frame it sat at 1 all through.
    samples.state(clock.at(5), 7, pad(1.0, &[]));
    clock.advance(Duration::from_millis(10));
    client.step(32);
    clock.advance(Duration::from_millis(10));
    client.step(32);
    close(&values(&client, "x0"), &[0.5, 1.0]);
}

#[test]
fn a_slow_frame_keeps_every_button_in_order_and_the_stick_time_weighted() {
    let (mut client, samples, clock) = sampled_client(
        "TestMap.bind(joystick0, xaxis, x0); \
         TestMap.bindCmd(joystick0, button0, \"a0(1);\", \"a0(0);\"); \
         TestMap.bindCmd(joystick0, button1, \"b0(1);\", \"b0(0);\");",
    );
    samples.state(clock.at(0), 0, pad(0.0, &[]));
    client.step(32);
    // 100 samples in one 200 ms frame: A and B take turns, the stick is at 0.01 * n from the nth sample on.
    let mut area = 0.0f64;
    for n in 1..=100u64 {
        let button = if n % 2 == 1 {
            PadButton::South
        } else {
            PadButton::East
        };
        let state = pad(n as f32 * 0.01, &[button]);
        samples.state(clock.at(2 * n - 1), 0, state);
        area += (n as f64 * 0.01) * if n == 100 { 1.0 } else { 2.0 };
    }
    clock.advance(Duration::from_millis(200));
    client.step(32);
    // A goes down at each odd sample and up at each even one; B the other way round, and is down at the end.
    assert_eq!(values(&client, "a0").len(), 100);
    assert_eq!(values(&client, "b0").len(), 99);
    close(&values(&client, "x0"), &[(area / 200.0) as f32]);
}

#[test]
fn a_pad_that_goes_lets_go_at_its_stamp() {
    let (mut client, samples, clock) = sampled_client(
        "TestMap.bind(joystick0, xaxis, x0); \
         TestMap.bindCmd(joystick0, button0, \"a0(1);\", \"a0(0);\");",
    );
    samples.state(clock.at(0), 4, pad(1.0, &[PadButton::South]));
    clock.advance(Duration::from_millis(4));
    client.step(32);
    samples.gone(clock.at(4), 4);
    clock.advance(Duration::from_millis(4));
    client.step(32);
    close(&values(&client, "x0"), &[1.0, 0.0]);
    close(&values(&client, "a0"), &[1.0, 0.0]);
}

/// A scripted backend that counts how many were made and says when one is dropped.
struct Watched {
    pads: FakePads,
    dropped: Arc<AtomicBool>,
}

impl PadBackend for Watched {
    fn poll(&mut self) -> Vec<(u32, PadState)> {
        self.pads.poll()
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_reading_thread_starts_with_the_first_poll_and_ends_with_the_client() {
    let pads = FakePads::default();
    let made = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicBool::new(false));
    let plugin = {
        let (pads, made, dropped) = (pads.clone(), Arc::clone(&made), Arc::clone(&dropped));
        GamepadPlugin::with_threaded_backend(move || {
            made.fetch_add(1, Ordering::SeqCst);
            Watched {
                pads: pads.clone(),
                dropped: Arc::clone(&dropped),
            }
        })
    };
    let mut client = client_with(
        plugin,
        &format!("{FUNCTIONS} TestMap.bind(joystick0, xaxis, x0);"),
    );
    client.set_global("$Pref::Gamepad::DeadZone", "0");
    assert_eq!(client.global("$Pref::Gamepad::PollRate"), "500");
    // An engine and a client that never poll make no backend.
    assert_eq!(made.load(Ordering::SeqCst), 0);
    client.step(32);
    wait_for("the thread to make its backend", || {
        made.load(Ordering::SeqCst) == 1
    });
    // A pad plugged in is seen by the thread and reaches the script.
    pads.plug_in(1);
    pads.left_stick(1, 0.5, 0.0);
    wait_for("the stick to reach the script", || {
        client.step(32);
        values(&client, "x0").last() == Some(&0.5)
    });
    assert!(!dropped.load(Ordering::SeqCst));
    // The thread is joined when the client goes: its backend is dropped by the time the drop returns.
    drop(client);
    assert!(
        dropped.load(Ordering::SeqCst),
        "the thread outlived the client"
    );
}
