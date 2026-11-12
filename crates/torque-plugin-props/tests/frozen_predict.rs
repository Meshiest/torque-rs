//! A client's prediction collides with the frozen props the server sent it over the channel as it does with ghosts: a figure
//! that walks into a wall of them stops where the server stops it, with no corrections, while the same walk with the table
//! emptied runs on into the wall and is pulled back by the server over and over.

#[path = "net/rig.rs"]
mod rig;

use rig::{Link, PeerProps, Rig, files};
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_props::PropsPlugin;
use torque_plugin_props::predict::{PredictConfig, PropPredictionPlugin};

fn predicting(files: &torque_app::GameFiles) -> Engine {
    use torque_app::config::EngineConfig;
    let game = torque_app::config::GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins(torque_app::plugins::DefaultPlugins)
        .config(&game);
    builder.add_plugins((
        PropsPlugin,
        PropPredictionPlugin {
            config: PredictConfig::default(),
        },
    ));
    builder.build().expect("the engine builds")
}

/// A figure with a predicting client, and a wall of seven crates in its way, frozen: `ghosts` of them as ghosts at most.
fn setup(ghosts: &str) -> (Rig, u32) {
    let files = files();
    let mut rig = Rig::new(
        &files,
        &[
            ("$Test::Players", "1"),
            ("$Pref::Props::FrozenGhosts", ghosts),
        ],
    );
    rig.join(&files, predicting(&files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let player: u32 = rig
        .eval("return ClientGroup.getObject(0).getControlObject();")
        .parse()
        .expect("a player");
    rig.eval(&format!("{player}.setTransform(\"0 -10 0.5\");"));
    rig.eval(
        "for (%i = 0; %i < 7; %i++) { %p = propSpawn(Crate, (%i - 3) SPC -4 SPC 0.5); propFreeze(%p); }",
    );
    rig.run(60);
    rig.link = Link::lossy();
    (rig, player)
}

fn corrections(rig: &Rig) -> u64 {
    rig.peers[0]
        .client
        .host
        .client
        .borrow()
        .conn
        .conn
        .as_ref()
        .map_or(0, |c| c.predict.stats.corrections)
}

fn y_of(rig: &mut Rig, player: u32) -> f32 {
    rig.eval(&format!("return getWord({player}.getPosition(), 1);"))
        .parse()
        .expect("a number")
}

/// Walks forward for `ticks`, emptying the client's table before each step when `blind`.
fn walk(rig: &mut Rig, ticks: usize, blind: bool) {
    for _ in 0..ticks {
        let vm = rig.peers[0].client.vm_mut();
        vm.set_global("$mvForwardAction", "1");
        if blind {
            rig.peers[0].book().0.borrow_mut().frozen_mut().clear();
        }
        rig.step();
    }
}

#[test]
fn a_figure_walks_into_frozen_props_sent_over_the_channel_and_stops_where_the_server_stops_it() {
    // None of the wall is ghosted: the client knows it only from the channel.
    let (mut rig, player) = setup("0");
    assert!(rig.run_until(400, |r| r.peers[0].book().0.borrow().frozen().len() == 7));
    assert!(rig.peers[0].book().0.borrow().props().is_empty());
    let before = corrections(&rig);
    walk(&mut rig, 300, false);
    let y = y_of(&mut rig, player);
    // The crates' faces are at -4.5 and the figure's box is a unit deep: it stands against them.
    assert!(y < -4.9 && y > -5.6, "the server's figure is at {y}");
    let with = corrections(&rig) - before;
    // The same walk with a wall of ghosts: the table makes no difference.
    let (mut ghosted, player) = setup("1024");
    assert!(ghosted.run_until(400, |r| r.peers[0].book().0.borrow().props().len() == 7));
    let before = corrections(&ghosted);
    walk(&mut ghosted, 300, false);
    let y_ghosts = y_of(&mut ghosted, player);
    assert!((y - y_ghosts).abs() < 0.2, "{y} against {y_ghosts}");
    let reference = corrections(&ghosted) - before;
    assert!(
        with <= reference + 3,
        "{with} corrections over the channel, {reference} with ghosts"
    );
    // Blind to the table, the prediction walks through the wall and the server pulls it back.
    let (mut blind, _) = setup("0");
    assert!(blind.run_until(400, |r| r.peers[0].book().0.borrow().frozen().len() == 7));
    let before = corrections(&blind);
    walk(&mut blind, 300, true);
    let without = corrections(&blind) - before;
    assert!(
        without > with + 5,
        "{without} corrections without the table, {with} with it"
    );
}
