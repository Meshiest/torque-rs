//! The figure's hitboxes and lag compensation on the arena's server (`game/fixtures/floor.mis`, no clients): bolts are shot
//! by script along +y at a figure standing at the origin facing +y. A bolt just beside the arm or between the legs misses; one
//! through the torso hits it, and the hit names the part. A target running sideways is hit by a shooter with a round trip when
//! aimed where that shooter saw it, and missed with lag compensation off. The example's link carries no latency, so the round
//! trip is the server's rewind source set by the test.

use std::net::Ipv4Addr;
use std::rc::Rc;

use torque::app::server::GameServer;
use torque::game::scene::lag;
use torque::script::vm::sim::ObjId;
use torque::script::vm::vm::Vm;
use torque_example_fps::files::GameFiles;
use torque_example_fps::server::{ArenaServer, ServerOptions};

fn server() -> GameServer {
    let mut s = ArenaServer::start(
        &GameFiles::bundled().unwrap(),
        &ServerOptions {
            address: Ipv4Addr::LOCALHOST,
            port: 0,
            name: "Hitboxes".to_string(),
            mission: "fixtures/floor.mis".to_string(),
            public: false,
        },
    )
    .unwrap();
    s.tick();
    s
}

fn eval(s: &mut GameServer, text: &str) -> String {
    String::from_utf8(s.vm_mut().eval(text.as_bytes()).unwrap()).unwrap()
}

/// A figure named `name` with its feet at `x y 0`, facing +y.
fn figure(s: &mut GameServer, name: &str, x: f32, y: f32) {
    eval(
        s,
        &format!(
            "MissionCleanup.add(new Player({name}) {{ dataBlock = ArenaFigure; position = \"{x} {y} 0\"; }});"
        ),
    );
}

/// The figure's damage.
fn damage(s: &mut GameServer, name: &str) -> f32 {
    eval(s, &format!("return {name}.getDamageLevel();"))
        .parse()
        .unwrap()
}

/// A bolt of the shooter's along +y from `x -3 z`, then `ticks` ticks; the server's lines meanwhile.
fn bolt(s: &mut GameServer, x: f32, z: f32, ticks: usize) -> Vec<String> {
    s.drain_log();
    eval(
        s,
        &format!(
            "MissionCleanup.add(new Projectile() {{ dataBlock = ArenaBolt; initialPosition = \"{x} -3 {z}\"; \
             initialVelocity = \"0 150 0\"; sourceObject = Shooter.getId(); }});"
        ),
    );
    for _ in 0..ticks {
        s.tick();
    }
    s.drain_log()
}

/// The part a bolt's hit names in the server's lines, if one hit.
fn part(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|l| {
        l.split_once("hit in the ")
            .map(|(_, p)| p.trim_end_matches('.').to_string())
    })
}

fn standing(s: &mut GameServer) {
    figure(s, "Shooter", 6.0, -8.0);
    figure(s, "Target", 0.0, 0.0);
    for _ in 0..20 {
        s.tick();
    }
}

#[test]
fn a_bolt_beside_the_arm_or_between_the_legs_misses_and_one_through_the_torso_hits() {
    let mut s = server();
    standing(&mut s);
    // The right arm hangs from the shoulder at x 0.33 (0.07 each way), inside the figure's box (0.5 each way).
    let lines = bolt(&mut s, 0.45, 1.4, 3);
    assert_eq!(damage(&mut s, "Target"), 0.0, "beside the arm: {lines:?}");
    // The thighs are 0.12 each side of the middle (0.09 each way).
    let lines = bolt(&mut s, 0.0, 0.6, 3);
    assert_eq!(damage(&mut s, "Target"), 0.0, "between the legs: {lines:?}");
    let lines = bolt(&mut s, 0.0, 1.3, 3);
    assert_eq!(damage(&mut s, "Target"), 34.0, "through the torso");
    assert_eq!(part(&lines).as_deref(), Some("torso"), "{lines:?}");
    let lines = bolt(&mut s, 0.3, 1.4, 3);
    assert_eq!(part(&lines).as_deref(), Some("rarm"), "{lines:?}");
    let lines = bolt(&mut s, 0.0, 1.85, 3);
    assert_eq!(part(&lines).as_deref(), Some("head"), "{lines:?}");
}

/// A round trip the test sets for every shooter.
struct Fixed(u32);

impl lag::RewindSource for Fixed {
    fn round_trip_ms(&self, _vm: &mut Vm, _shooter: ObjId) -> Option<u32> {
        Some(self.0)
    }
}

/// The step the target runs sideways each tick (a side step's speed, about 6 units a second).
const STRIDE: f32 = 0.2;

#[test]
fn a_shooter_with_latency_hits_a_running_target_where_it_saw_it() {
    for on in [false, true] {
        let mut s = server();
        lag::set_rewind_source(s.vm_mut(), Rc::new(Fixed(128)));
        if !on {
            eval(&mut s, "$Pref::Server::LagCompensation = 0;");
        }
        standing(&mut s);
        let mut x = -2.0;
        for _ in 0..12 {
            x += STRIDE;
            eval(
                &mut s,
                &format!(
                    "Target.setTransform(\"{x} 0 0 0 0 1 0\"); Target.setVelocity(\"0 0 0\");"
                ),
            );
            s.tick();
        }
        // A 128 ms round trip rewinds 96 ms, three ticks (the trip less the one its client draws the target ahead): the shooter
        // saw it three strides back, clear of where it is now.
        let seen = x - 3.0 * STRIDE;
        let lines = bolt(&mut s, seen, 1.3, 3);
        if on {
            assert_eq!(damage(&mut s, "Target"), 34.0, "rewound: {lines:?}");
            assert_eq!(part(&lines).as_deref(), Some("torso"));
        } else {
            assert_eq!(damage(&mut s, "Target"), 0.0, "not rewound: {lines:?}");
        }
    }
}

#[test]
fn the_debug_view_finds_every_part_closed_where_the_figure_has_it() {
    let mut s = server();
    standing(&mut s);
    let id: ObjId = eval(&mut s, "return Target.getId();").parse().unwrap();
    let boxes = torque_example_fps::hitbox_view::part_boxes(&mut s, id);
    assert_eq!(boxes.len(), 14);
    assert!(boxes.iter().all(|b| !b.open), "every hull closed");
    let rarm = boxes.iter().find(|b| b.name == "rarm").unwrap();
    let (lo, hi) = (rarm.corners[0], rarm.corners[7]);
    for (got, want) in [lo, hi]
        .iter()
        .flatten()
        .zip([0.265, -0.065, 1.19, 0.395, 0.065, 1.61])
    {
        assert!((got - want).abs() < 0.02, "{lo:?} {hi:?}");
    }
}
