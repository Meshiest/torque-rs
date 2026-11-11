//! Ropes that show joints (feature `rope`): the driver's answer laid on a rope (taut at the joint's limit, hanging when it is
//! slack), and over the memory transport a server's `jointRope` reaching a client bound to the joint's driver, going with its
//! joint. `cargo test -p torque-plugin-props --features rope --test joint_rope`.

#[path = "net/rig.rs"]
mod rig;

use rig::{Rig, files};
use torque_app::config::EngineConfig;
use torque_app::plugins::DefaultPlugins;
use torque_engine::{Engine, EngineBuilder};
use torque_plugin_props::joints::KindId;
use torque_plugin_props::joints::records::{EndWire, FlagsWire, InitWire, JointGhost, LengthWire};
use torque_plugin_props::joints::rope::{DRIVER, JointRopeDriver};
use torque_plugin_props::{ClientJoints, JointRopePlugin};
use torque_plugin_rope::{RopeDriver, RopeDrivers, RopeParams, RopePlugin, RopeShared, RopeWorld};
use torque_ts_vm::vm::Vm;

/// The farthest any of `points` is from the segment between the first and the last.
fn off_the_segment(points: &[[f32; 3]]) -> f32 {
    let (a, b) = (points[0], points[points.len() - 1]);
    let ab: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let len2: f32 = ab.iter().map(|v| v * v).sum();
    points
        .iter()
        .map(|p| {
            let ap: [f32; 3] = std::array::from_fn(|k| p[k] - a[k]);
            let t = if len2 > 0.0 {
                (ap.iter().zip(&ab).map(|(x, y)| x * y).sum::<f32>() / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            (0..3)
                .map(|k| (p[k] - (a[k] + ab[k] * t)).powi(2))
                .sum::<f32>()
                .sqrt()
        })
        .fold(0.0, f32::max)
}

/// A client's book holding one tether between two points, `length` long, at its limit or not.
fn book(length: f32, at_limit: bool) -> ClientJoints {
    let joints = ClientJoints::default();
    joints.0.borrow_mut().heard(
        7,
        &JointGhost {
            init: Some(InitWire {
                kind: KindId::Tether,
                a: EndWire::Point([0.0, 0.0, 3.0]),
                b: EndWire::Point([5.0, 0.0, 3.0]),
            }),
            length: Some(LengthWire { length, min: 0.0 }),
            spring: None,
            flags: Some(FlagsWire {
                at_limit,
                broken: false,
            }),
        },
    );
    joints
}

fn shown(world: &RopeWorld, id: torque_plugin_rope::RopeId) -> Vec<[f32; 3]> {
    world
        .rope(id)
        .unwrap()
        .drawn_points()
        .iter()
        .map(|p| p.to_array())
        .collect()
}

#[test]
fn a_joint_rope_at_its_limit_lies_straight_within_a_millimeter_and_a_slack_one_hangs() {
    let vm = Vm::new();
    let mut world = RopeWorld::new();
    let params = RopeParams {
        segments: 16,
        length: 6.0,
        ..RopeParams::default()
    };
    let id = world
        .create(params, [0.0, 0.0, 3.0], [5.0, 0.0, 3.0])
        .unwrap();
    // Slack: the joint is 6 long and its ends 5 apart.
    let slack = JointRopeDriver(book(6.0, false)).drive(&vm, 7).unwrap();
    assert_eq!(slack.length, Some(6.0));
    for t in 0..120 {
        world.drive(id, &slack).unwrap();
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    assert!(
        off_the_segment(&shown(&world, id)) > 0.05,
        "a slack joint's rope hangs"
    );
    // At its limit: straight between its ends.
    let taut = JointRopeDriver(book(5.0, true)).drive(&vm, 7).unwrap();
    assert!(taut.at_limit);
    for t in 120..240 {
        world.drive(id, &taut).unwrap();
        world.step_frame(t as f32 / 60.0, |_, _| None);
    }
    let off = off_the_segment(&shown(&world, id));
    assert!(off < 1.0e-3, "{off}");
    // A joint the client does not hold lets the rope go.
    assert!(JointRopeDriver(book(5.0, true)).drive(&vm, 8).is_none());
}

/// An engine of the default plugins, the props plugin and the rope plugin.
fn engine(files: &torque_app::GameFiles) -> Engine {
    let config = torque_app::config::GameConfig::load(files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((DefaultPlugins, JointRopePlugin, RopePlugin::default()))
        .config(&config);
    builder.build().expect("the engine builds")
}

#[test]
fn a_joints_rope_reaches_the_client_bound_to_the_joint_and_goes_with_it() {
    let files = files();
    let mut rig = Rig::with_engine(&files, &[], engine(&files));
    rig.eval("datablock RopeData(Cord) { style = \"line\"; thickness = 0.05; };");
    rig.join(&files, engine(&files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let a: u32 = rig
        .eval("return propSpawn(Crate, \"0 0 3\");")
        .parse()
        .unwrap();
    let joint: u32 = rig
        .eval(&format!("return jointTether({a}, \"0 0 8\", 4);"))
        .parse()
        .unwrap();
    let rope: u32 = rig
        .eval(&format!("return jointRope({joint}, Cord);"))
        .parse()
        .unwrap();
    assert!(rope > 0, "{:?}", rig.server.drain_log());
    let peer = &rig.peers[0].engine;
    let drivers = peer.resource::<RopeDrivers>().expect("the drivers").clone();
    assert!(drivers.is_registered(DRIVER));
    let ropes = peer.resource::<RopeShared>().expect("the ropes").clone();
    let joints = peer.resource::<ClientJoints>().expect("the joints").clone();
    assert!(rig.run_until(200, |_| drivers.bound_count() == 1));
    rig.run(60);
    // The client's rope is bound to its ghost of the joint, and follows it: taut, as long as the joint.
    let key = joints.0.borrow().joints().keys().next().copied().unwrap();
    let bound = ropes
        .lock()
        .ropes()
        .find_map(|(id, _)| drivers.bound(id))
        .expect("a bound rope");
    // The name arrives as the string table spells it (the class's `Joint`) and is found all the same.
    assert!(
        bound.0.eq_ignore_ascii_case(DRIVER) && bound.1 == key,
        "{bound:?}"
    );
    {
        let world = ropes.lock();
        let (_, shown) = world.ropes().next().expect("the client's rope");
        assert!(shown.at_limit(), "the crate hangs at the tether's length");
        assert!((shown.length() - 4.0).abs() < 1.0e-3, "{}", shown.length());
    }
    // The joint goes: its rope goes with it.
    assert_eq!(rig.eval(&format!("return jointDelete({joint});")), "1");
    assert_eq!(rig.objects_of("Rope"), 0);
    assert_eq!(rig.objects_of("Joint"), 0);
}

/// A driver of another plugin's that took the joints' name.
struct Other;

impl RopeDriver for Other {
    fn drive(&self, _vm: &Vm, _key: u32) -> Option<torque_plugin_rope::Drive> {
        None
    }
}

#[test]
fn another_plugins_driver_under_the_joints_name_is_warned_about() {
    let files = files();
    let mut rig = Rig::with_engine(&files, &[], engine(&files));
    let config = torque_app::config::GameConfig::load(&files).expect("game.toml");
    let mut builder = EngineBuilder::new();
    builder
        .add_plugins((
            DefaultPlugins,
            JointRopePlugin,
            RopePlugin::default().with_driver("Joint", Other),
        ))
        .config(&config);
    let taken = builder.build().expect("the engine builds");
    rig.join(&files, taken, 50_002, &[]);
    rig.join(&files, engine(&files), 50_003, &[]);
    let warned = |peer: usize, rig: &mut Rig| {
        rig.peers[peer]
            .client
            .vm_mut()
            .drain_log()
            .iter()
            .any(|l| String::from_utf8_lossy(&l.text).contains("another plugin's rope driver"))
    };
    assert!(warned(0, &mut rig));
    assert!(
        !warned(1, &mut rig),
        "the joints' own driver is no other plugin's"
    );
}
