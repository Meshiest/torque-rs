//! An `InteriorInstance` on the server: its `.dif`'s hulls answer the container's rays and stop a Projectile, a figure behind
//! its wall is out of a ray's reach, and a rewound shot (lag compensation) meets the wall as the live one does. A file that does
//! not read fails the object's `onAdd`.

mod figure;

use std::rc::Rc;

use figure::{eval, game_server, id, spawn};
use torque_game::scene::lag;
use torque_server::server::{Server, TICK_MS};
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

struct Fixed(u32);

impl lag::RewindSource for Fixed {
    fn round_trip_ms(&self, _vm: &mut Vm, _shooter: ObjId) -> Option<u32> {
        Some(self.0)
    }
}

/// `INTERIOR | PLAYER`.
const MASK: u32 = 8 | 16384;

fn with_wall() -> (Server, ObjId) {
    let mut s = game_server();
    eval(
        &mut s,
        "new InteriorInstance(Wall) { interiorFile = \"wall.dif\"; position = \"0 2 0\"; };",
    );
    let w = id(&s, "Wall");
    (s, w)
}

#[test]
fn the_walls_hulls_answer_rays_and_hide_a_figure_behind_it() {
    let (mut s, wall) = with_wall();
    assert_eq!(eval(&mut s, "return Wall.getType() & 8;"), "8");
    let t = spawn(&mut s, "Target", "Posed", "0 4 0");
    let hit = eval(
        &mut s,
        &format!("return containerRayCast(\"0 -5 1.3\", \"0 10 1.3\", {MASK});"),
    );
    let words: Vec<&str> = hit.split(' ').collect();
    assert_eq!(words[0], wall.to_string(), "{hit}");
    // The wall's near face is at y = 2 - 0.25; its normal faces the ray.
    assert!(
        (words[2].parse::<f32>().unwrap() - 1.75).abs() < 1e-4,
        "{hit}"
    );
    assert_eq!(words[5], "-1", "{hit}");
    // Along the wall's far side the figure is hit.
    let past = eval(
        &mut s,
        &format!("return containerRayCast(\"-5 4 1.3\", \"5 4 1.3\", {MASK});"),
    );
    assert_eq!(
        past.split(' ').next(),
        Some(t.to_string().as_str()),
        "{past}"
    );
    // The figure stands on the interior's floor.
    for _ in 0..30 {
        s.run(TICK_MS, |_| false);
    }
    let z: f32 = eval(&mut s, "return getWord(Target.getPosition(), 2);")
        .parse()
        .unwrap();
    assert!(z.abs() < 0.05, "standing at {z}");
}

#[test]
fn a_bolt_and_a_rewound_bolt_stop_at_the_wall() {
    for on in [false, true] {
        let (mut s, wall) = with_wall();
        lag::set_rewind_source(&mut s.vm, Rc::new(Fixed(96)));
        eval(
            &mut s,
            &format!("$Pref::Server::LagCompensation = {};", u8::from(on)),
        );
        spawn(&mut s, "Shooter", "Boxed", "50 50 0");
        spawn(&mut s, "Target", "Boxed", "0 4 0");
        eval(
            &mut s,
            "$hit = \"\"; new Projectile() { dataBlock = Bolt; initialPosition = \"0 -3 1.2\"; initialVelocity = \"0 50 0\"; sourceObject = Shooter.getId(); };",
        );
        for _ in 0..6 {
            s.run(TICK_MS, |_| false);
        }
        assert_eq!(
            s.vm.global("$hit").map(|v| String::from_utf8(v).unwrap()),
            Some(wall.to_string()),
            "lag compensation {on}"
        );
    }
}

#[test]
fn an_interior_whose_file_does_not_read_is_not_added() {
    let mut s = game_server();
    eval(
        &mut s,
        "$made = new InteriorInstance(Missing) { interiorFile = \"none.dif\"; };",
    );
    assert!(s.vm.objects().find(b"Missing").is_none());
}
