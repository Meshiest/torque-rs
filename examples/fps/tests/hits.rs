//! A hit as both players see it: Ann shoots Bob, who stands facing her (`game/fixtures/floor.mis`). Ann's screen shows the X
//! over the crosshair and both clients burst sparks where the bolt hit Bob; Bob's screen flashes its red edge and marks the way
//! the shot came from, straight ahead of him. Knocked out, Bob falls with his aiming arm let go.

use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode};
use torque_example_fps::files::GameFiles;

fn client(name: &str) -> ArenaGame {
    ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: Some("fixtures/floor.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap()
}

fn frame(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for g in [a, b] {
        g.step(ms);
        g.add_frame_time(ms as f32 / 1000.0);
        g.view_frame();
    }
}

#[test]
fn a_hit_bursts_marks_the_shooter_and_shows_the_hurt_its_way() {
    let mut a = client("Ann");
    let mut b = client("Bob");
    a.call("hostGame", &[]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if a.in_game() {
            break;
        }
    }
    let address = a.hosted_address().unwrap().to_string();
    b.call("connectTo", &[&address]).unwrap();
    for _ in 0..300 {
        frame(&mut a, &mut b, 16);
        if b.in_game() && a.scene().others().len() == 1 && b.scene().others().len() == 1 {
            break;
        }
    }
    for _ in 0..60 {
        frame(&mut a, &mut b, 16);
    }
    assert!(a.marks.hit.is_none() && b.marks.hurt.is_none());
    let bob = a.scene().others()[0].0;

    a.call("fire", &["1"]).unwrap();
    let (mut marked, mut hurt) = (false, false);
    for i in 0..60 {
        if i == 4 {
            a.call("fire", &["0"]).unwrap();
        }
        frame(&mut a, &mut b, 16);
        marked |= a.marks.hit_alpha() > 0.0;
        if let Some((angle, _)) = b
            .scene()
            .own()
            .and_then(|o| b.marks.direction(o.eye, o.yaw))
        {
            assert!(
                angle.abs() < 0.1,
                "the shot came from straight ahead: {angle}"
            );
            assert!(b.marks.edge_alpha() > 0.0 || b.marks.hurt.unwrap().age > 0.5);
            let quads: usize = b
                .marks
                .batches([0, 0, 800, 600], [0.0; 3], 0.0)
                .iter()
                .map(|q| q.quads.len())
                .sum();
            assert!(quads > 0);
            hurt = true;
        }
    }
    assert_eq!(a.global("$Arena::Hits"), "1", "the bolt hit");
    let part = a.global("$Arena::HitPart");
    assert!(
        ["head", "torso"].contains(&part.as_str()),
        "Ann is told the part she hit: {part:?}"
    );
    assert!(marked, "Ann saw the X");
    assert!(hurt, "Bob saw where the hurt came from");
    for (who, g) in [("Ann", &a), ("Bob", &b)] {
        let burst = g.scene().hit_bursts.last().copied();
        let burst = burst.unwrap_or_else(|| panic!("{who} saw the sparks"));
        let off = ((burst[0] - bob[0]).powi(2) + (burst[1] - bob[1]).powi(2)).sqrt();
        assert!(
            off < 1.0 && burst[2] > bob[2] && burst[2] < bob[2] + 2.2,
            "{who}'s sparks at {burst:?}, Bob at {bob:?}"
        );
    }
    // The X is gone a moment later.
    for _ in 0..30 {
        frame(&mut a, &mut b, 16);
    }
    assert_eq!(a.marks.hit_alpha(), 0.0);

    assert_eq!(a.scene().others_arms(), [(false, Some("hold".to_string()))]);
    // Bob aims down his sights: Ann sees him raise his pistol to his eye, then lower it.
    b.call("aim", &["1"]).unwrap();
    for _ in 0..20 {
        frame(&mut a, &mut b, 16);
    }
    assert_eq!(a.scene().others_arms(), [(false, Some("aim".to_string()))]);
    b.call("aim", &["0"]).unwrap();
    for _ in 0..20 {
        frame(&mut a, &mut b, 16);
    }
    assert_eq!(a.scene().others_arms(), [(false, Some("hold".to_string()))]);
    // Two more knock Bob out: his fall owns his whole body, the arm that aimed the pistol falls with it.
    for k in 2..=3 {
        a.call("fire", &["1"]).unwrap();
        for i in 0..60 {
            if i == 4 {
                a.call("fire", &["0"]).unwrap();
            }
            frame(&mut a, &mut b, 16);
            if a.global("$Arena::Hits") == k.to_string() && i > 20 {
                break;
            }
        }
    }
    assert_eq!(a.global("$Arena::Hits"), "3");
    for _ in 0..20 {
        frame(&mut a, &mut b, 16);
    }
    assert_eq!(
        a.scene().others_arms(),
        [(true, None)],
        "dying, the arm let go"
    );
}
