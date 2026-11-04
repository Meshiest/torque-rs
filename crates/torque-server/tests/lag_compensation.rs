//! Lag compensation: with `$Pref::Server::LagCompensation` on, a Projectile from a shooter with a round trip sees the Players as
//! they were that long ago (capped by `$Pref::Server::MaxRewindMs`, the pose too), and the history holds no more than the cap
//! needs.

mod figure;

use std::rc::Rc;

use figure::{across, eval, game_server, id, spawn};
use torque_game::scene::lag;
use torque_server::server::{Server, TICK_MS};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

/// A round trip the test sets for every shooter.
struct Fixed(u32);

impl lag::RewindSource for Fixed {
    fn round_trip_ms(&self, _vm: &mut Vm, _shooter: ObjId) -> Option<u32> {
        Some(self.0)
    }
}

/// The target walks 1 along x a step for `steps` steps from x = `from` (pinned, so it does not fall).
fn walk(s: &mut Server, from: usize, steps: usize) {
    for k in from..from + steps {
        eval(
            s,
            &format!(
                "Target.setTransform(\"{} 0 0 0 0 1 0\"); Target.setVelocity(\"0 0 0\");",
                k as f32
            ),
        );
        s.run(TICK_MS, |_| false);
    }
}

fn fire_at(s: &mut Server, x: f32, z: f32) -> String {
    eval(s, "$hit = \"\";");
    eval(
        s,
        &format!(
            "new Projectile() {{ dataBlock = Bolt; initialPosition = \"{x} -3 {z}\"; initialVelocity = \"0 50 0\"; sourceObject = Shooter.getId(); }};"
        ),
    );
    for _ in 0..3 {
        s.run(TICK_MS, |_| false);
    }
    s.vm.global("$hit")
        .map(|v| String::from_utf8(v).unwrap())
        .unwrap_or_default()
}

fn lag_server(round_trip: u32) -> Server {
    let mut s = game_server();
    lag::set_rewind_source(&mut s.vm, Rc::new(Fixed(round_trip)));
    spawn(&mut s, "Shooter", "Boxed", "50 50 0");
    s
}

#[test]
fn a_shooter_with_latency_hits_the_target_where_it_saw_it() {
    // 96 ms is three steps: the shooter saw the target (1.5 wide each way) at x = 7 when the server has it at 10.
    for on in [false, true] {
        let mut s = lag_server(96);
        eval(
            &mut s,
            &format!("$Pref::Server::LagCompensation = {};", u8::from(on)),
        );
        let t = spawn(&mut s, "Target", "Boxed", "0 0 0");
        walk(&mut s, 0, 11);
        let hit = fire_at(&mut s, 7.0, 1.2);
        if on {
            assert_eq!(hit, t.to_string(), "rewound: hit where it was seen");
        } else {
            assert_eq!(hit, "", "not rewound: the target has moved on");
        }
    }
}

#[test]
fn the_rewound_pose_is_the_one_the_shooter_saw() {
    let mut s = lag_server(96);
    eval(&mut s, "$Pref::Server::LagCompensation = 1;");
    let t = spawn(&mut s, "Target", "Posed", "0 0 0");
    for _ in 0..6 {
        eval(
            &mut s,
            "Target.setTransform(\"0 0 0 0 0 1 0\"); Target.setVelocity(\"0 0 0\");",
        );
        s.run(TICK_MS, |_| false);
    }
    // Crouched for the last two steps and the shot's first (the shot reaches the head in its second): three steps back the
    // shooter saw it standing.
    eval(&mut s, "Target.setActionThread(\"crouch\", true);");
    for _ in 0..2 {
        eval(
            &mut s,
            "Target.setTransform(\"0 0 0 0 0 1 0\"); Target.setVelocity(\"0 0 0\");",
        );
        s.run(TICK_MS, |_| false);
    }
    assert_eq!(across(&mut s, 0.0, 2.0).0, 0, "crouched now");
    assert_eq!(
        fire_at(&mut s, 0.0, 2.0),
        t.to_string(),
        "the head where the shooter saw it"
    );
    let point =
        s.vm.global("$hitPos")
            .map(|v| String::from_utf8(v).unwrap())
            .unwrap();
    assert_eq!(
        eval(
            &mut s,
            &format!("return Target.getDamageLocation(\"{point}\");")
        ),
        "head"
    );
}

#[test]
fn the_rewind_is_capped_and_the_history_trimmed() {
    let mut s = lag_server(400);
    eval(
        &mut s,
        "$Pref::Server::LagCompensation = 1; $Pref::Server::MaxRewindMs = 64;",
    );
    let t = spawn(&mut s, "Target", "Boxed", "0 0 0");
    let shooter = id(&s, "Shooter");
    assert_eq!(lag::rewind_for(&mut s.vm, shooter), 64, "the cap");
    walk(&mut s, 0, 11);
    // Two steps back (x = 8, reaching 6.5 to 9.5): a shot at x = 6, where it was four steps back, misses; one at 7 hits.
    assert_eq!(fire_at(&mut s, 6.0, 1.2), "");
    walk(&mut s, 0, 11);
    assert_eq!(fire_at(&mut s, 7.0, 1.2), t.to_string());
    // Kept: the cap's two steps and three more.
    assert_eq!(lag::history_len(&s.vm, t), 64 / 32 + 3);
    eval(&mut s, "$Pref::Server::MaxRewindMs = \"\";");
    walk(&mut s, 0, 20);
    assert_eq!(lag::history_len(&s.vm, t), 250 / 32 + 3, "the default cap");
    eval(&mut s, "$Pref::Server::LagCompensation = 0;");
    s.run(TICK_MS, |_| false);
    assert_eq!(lag::history_len(&s.vm, t), 0, "off: let go");
    assert_eq!(lag::rewind_for(&mut s.vm, shooter), 0);
}
