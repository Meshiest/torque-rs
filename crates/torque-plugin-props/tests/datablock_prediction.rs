//! A Player's datablock edited mid-game while its client predicts it, in both network protocols, over the memory transport with
//! one tick up, two down and a fifth of the datagrams lost each way: the figure walks forward and back, the server raises its
//! `maxForwardSpeed` from 6 to 9 and sends the change with `updateDataBlocks()`, and the client's corrections are counted in
//! the 60 ticks around the change and the 600 after.
//!
//! Each run has a control: the same walk and the same script with the figure at 9 from the start, so the write changes nothing.
//! The 600 ticks after the change are held to the control's count: what corrects there is the loss itself (a move sent four
//! times and lost each time is skipped, as Torque skips it), not the edit. The engine's protocol holds the change back from the
//! client's control object until the client has the record (`torque_game::scene::refresh::HoldChanges`), so the change makes
//! no correction, and for at most `MAX_HOLD_TICKS` drains however the client acknowledges; Torque's protocol applies it at once, as Torque does, and the client corrects once a server packet until the
//! record arrives.

use crate::rig;

use rig::{Link, Rig, files};
use torque_app::GameFiles;
use torque_app::config::{EngineConfig, GameConfig};
use torque_app::plugins::DefaultPlugins;
use torque_engine::{Engine, EngineBuilder, ProtocolKind};
use torque_plugin_props::PropsPlugin;

/// The speed the figure is declared with and the one the edit writes.
const SLOW: &str = "6";
const FAST: &str = "9";

/// The props engine with `kind` pinned, whatever the environment says.
fn engine_in(files: &GameFiles, kind: ProtocolKind) -> Engine {
    let config = GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder.add_plugins(DefaultPlugins).config(&config);
    torque_app_server::plugins::set_protocol(&mut builder, kind);
    builder.add_plugins(PropsPlugin);
    builder.build().expect("the engine builds")
}

/// What one run counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    /// Corrections in the 30 ticks before the edit and the 30 after.
    around: u64,
    /// Corrections in the 600 ticks after those.
    after: u64,
    /// The forward speed the client's predicted Player has at the end.
    predicted: u32,
    /// Users the server's drains held back from a change, and released.
    held: u64,
    released: u64,
}

fn corrections(rig: &Rig) -> u64 {
    let host = rig.peers[0].client.host.client.borrow();
    host.conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

fn predicted_speed(rig: &Rig) -> f32 {
    let host = rig.peers[0].client.host.client.borrow();
    host.conn
        .conn
        .as_ref()
        .and_then(|c| c.predict.predicted.as_ref())
        .map_or(0.0, |p| p.data.max_forward_speed)
}

/// Forward for 200 ticks, back for 200, so the walk stays on the floor.
fn step(rig: &mut Rig, n: u32, t: &mut u32) {
    for _ in 0..n {
        let forward = (*t / 200).is_multiple_of(2);
        let vm = rig.peers[0].client.vm_mut();
        vm.set_global("$mvForwardAction", if forward { "1" } else { "0" });
        vm.set_global("$mvBackwardAction", if forward { "0" } else { "1" });
        rig.step();
        *t += 1;
    }
}

/// The walk in `kind` with the figure declared at `start`: 720 ticks of warm-up, then 60 ticks into a forward leg the server
/// writes the fast speed and asks for a round.
fn run(kind: ProtocolKind, start: &str) -> Counts {
    let files = files();
    let mut rig = Rig::with_engine(&files, &[("$Test::Players", "1")], engine_in(&files, kind));
    rig.eval(&format!("FigureData.maxForwardSpeed = {start};"));
    rig.join(&files, engine_in(&files, kind), 50_001, &[]);
    assert!(
        rig.run_until(400, |r| r.peers[0].ghosts() >= 2),
        "{:?}",
        rig.peers[0].client.client_log
    );
    rig.link = Link {
        loss_per_mille: 200,
        ..Link::lossy()
    };
    let mut t = 0;
    step(&mut rig, 720, &mut t);
    while t % 400 != 30 {
        step(&mut rig, 1, &mut t);
    }
    let before = corrections(&rig);
    step(&mut rig, 30, &mut t);
    rig.eval(&format!("FigureData.maxForwardSpeed = {FAST};"));
    assert_eq!(rig.eval("return updateDataBlocks();"), "1");
    step(&mut rig, 30, &mut t);
    let edited = corrections(&rig);
    step(&mut rig, 600, &mut t);
    let stats = torque_game::scene::refresh::stats(rig.server.vm());
    Counts {
        around: edited - before,
        after: corrections(&rig) - edited,
        predicted: predicted_speed(&rig).to_bits(),
        held: stats.held,
        released: stats.released,
    }
}

#[test]
fn a_speed_edit_costs_the_engines_protocol_no_correction_and_torques_one_a_packet_until_the_record()
{
    for kind in [ProtocolKind::Engine, ProtocolKind::Tge] {
        let edited = run(kind, SLOW);
        let control = run(kind, FAST);
        let label = format!("{kind:?}: edited {edited:?}, control {control:?}");
        assert_eq!(edited.predicted, 9.0f32.to_bits(), "{label}");
        assert_eq!(edited.after, control.after, "{label}");
        match kind {
            ProtocolKind::Engine => {
                assert_eq!(edited.around, control.around, "{label}");
                assert!(edited.around <= 2, "{label}");
                assert_eq!((edited.held, edited.released), (1, 1), "{label}");
            }
            _ => {
                // The record comes some ticks after the server applied the change (later still when its packet is lost).
                assert!((1..=4).contains(&edited.around), "{label}");
                assert_eq!((edited.held, edited.released), (0, 0), "{label}");
            }
        }
    }
}

/// The forward speed the server's own Player steps with (what every other client sees of it).
fn server_speed(rig: &Rig, player: u32) -> f32 {
    torque_game::sim::player::state(rig.server.vm(), player).map_or(0.0, |s| s.speeds[0])
}

#[test]
fn a_client_that_never_acknowledges_holds_a_change_at_most_sixteen_drains_and_never_holds_the_next()
{
    let files = files();
    let kind = ProtocolKind::Engine;
    let mut rig = Rig::with_engine(&files, &[("$Test::Players", "1")], engine_in(&files, kind));
    rig.join(&files, engine_in(&files, kind), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let player: u32 = rig
        .eval("return ClientGroup.getObject(0).getControlObject();")
        .parse()
        .expect("a player");
    rig.run(30);
    assert_eq!(server_speed(&rig, player), 6.0);
    // Nothing gets through any more: the client never acknowledges the record.
    rig.link = Link {
        loss_per_mille: 1000,
        ..Link::default()
    };
    rig.eval(&format!("FigureData.maxForwardSpeed = {FAST};"));
    assert_eq!(rig.eval("return updateDataBlocks();"), "1");
    let mut drains = 0;
    while server_speed(&rig, player) != 9.0 {
        assert!(
            drains < torque_game::scene::refresh::MAX_HOLD_TICKS,
            "still held after {drains} drains"
        );
        rig.step();
        drains += 1;
    }
    let stats = torque_game::scene::refresh::stats(rig.server.vm());
    assert_eq!((stats.held, stats.released), (1, 1));
    assert_eq!(
        drains,
        torque_game::scene::refresh::MAX_HOLD_TICKS,
        "held until the cap"
    );
    // The client still has not read the record: the next change is not held.
    rig.eval("FigureData.maxForwardSpeed = 12;");
    rig.step();
    assert_eq!(server_speed(&rig, player), 12.0);
    assert_eq!(torque_game::scene::refresh::stats(rig.server.vm()).held, 1);
}
