//! Ropes that show joints, drawn offscreen by the client that was sent them (feature `rope`): a crate hanging from a point on a
//! tether at its length (the rope taut, straight) and one resting on the floor on a longer tether (the rope slack, sagging).
//! `TORQUE_ROPE_PICTURES=<folder>` also writes the pictures there as PNGs. Skipped where no GPU is found.

#[path = "../../torque-plugin-rope/tests/pictures/common.rs"]
mod common;
#[path = "net/rig.rs"]
mod rig;

use common::{SIZE, WHOLE, changed, gpu, save, shot_from};
use rig::{Rig, files};
use torque_app::config::EngineConfig;
use torque_app::plugins::DefaultPlugins;
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_props::PropsPlugin;
use torque_plugin_rope::{RopePlugin, RopeShared};
use torque_render_scene::hd::kit::look_at;
use torque_render_scene::source::Eye;

fn engine(files: &torque_app::GameFiles) -> Engine {
    let config = torque_app::config::GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins, PropsPlugin, RopePlugin::default()))
        .config(&config);
    builder.build().expect("the engine builds")
}

fn front() -> Eye {
    look_at([0.0, -11.0, 3.0], [0.0, 0.0, 2.0])
}

#[test]
fn a_taut_and_a_slack_joint_rope_are_drawn_straight_and_sagging() {
    let Some(gpu) = gpu() else { return };
    let files = files();
    let mut rig = Rig::with_engine(&files, &[], engine(&files));
    rig.eval("datablock RopeData(Cord) { style = \"tube\"; thickness = 0.08; segments = 24; color = \"220 200 60 255\"; };");
    rig.join(&files, engine(&files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let mut v = common::view(&gpu, &rig.peers[0].engine);
    let bare = shot_from(&mut v, &gpu, front);
    // Hanging at the tether's length: taut. Resting on the floor, its point 3.2 away and the tether 5 long: slack, sagging.
    let hung: u32 = rig
        .eval("return propSpawn(Crate, \"-2 0 1.5\");")
        .parse()
        .unwrap();
    let rests: u32 = rig
        .eval("return propSpawn(Crate, \"1.5 0 0.5\");")
        .parse()
        .unwrap();
    let mut joints = Vec::new();
    for (prop, point, length) in [(hung, "-2 0 4.5", 2.5), (rests, "3.5 0 3.5", 5.0)] {
        let j: u32 = rig
            .eval(&format!(
                "return jointTether({prop} SPC \"0 0 0.5\", \"{point}\", {length});"
            ))
            .parse()
            .unwrap();
        let rope: u32 = rig
            .eval(&format!("return jointRope({j}, Cord);"))
            .parse()
            .unwrap();
        assert!(rope > 0);
        joints.push(j);
    }
    rig.run(200);
    let at_limit = |rig: &mut Rig, j: u32| rig.eval(&format!("return jointIsAtLimit({j});"));
    assert_eq!(at_limit(&mut rig, joints[0]), "1");
    assert_eq!(at_limit(&mut rig, joints[1]), "0");
    let ropes = rig.peers[0]
        .engine
        .resource::<RopeShared>()
        .expect("the ropes")
        .clone();
    {
        let world = ropes.lock();
        let mut taut: Vec<bool> = world.ropes().map(|(_, r)| r.taut()).collect();
        taut.sort_unstable();
        assert_eq!(taut, [false, true], "one taut, one slack");
    }
    let both = shot_from(&mut v, &gpu, front);
    save(&both, "joint-ropes-taut-and-slack");
    assert!(
        changed(&bare, &both, WHOLE, 40) > 400,
        "the ropes are drawn"
    );
    // Each half of the picture has its rope.
    let (left, right) = (
        [0, 0, SIZE.0 / 2, SIZE.1],
        [SIZE.0 / 2, 0, SIZE.0 / 2, SIZE.1],
    );
    assert!(changed(&bare, &both, left, 40) > 100);
    assert!(changed(&bare, &both, right, 40) > 100);
}
