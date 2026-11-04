//! Another player's figure as a client draws it: the host walks in a circle (forward, turning at a steady rate with the mouse) and
//! the second client's frames draw the host's figure moving and turning steadily: no frame steps back or turns back, and none
//! moves or turns much further than the walk and the turn carry it in that frame's time; and the server sends each client a
//! packet every tick. And a figure jumping while it walks is never drawn below the floor, by its own client or the other one.
//! The game plays `game/fixtures/aim.mis`.

use torque_example_fps::client::{ArenaGame, ClientOptions, NetMode, keys};
use torque_example_fps::files::GameFiles;

fn client(name: &str) -> ArenaGame {
    ArenaGame::new(
        GameFiles::bundled().unwrap(),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            map: Some("fixtures/aim.mis".to_string()),
            ..ClientOptions::default()
        },
    )
    .unwrap()
}

/// `ms` of both clocks and a frame of each.
fn frame(a: &mut ArenaGame, b: &mut ArenaGame, ms: u32) {
    for g in [a, b] {
        g.step(ms);
        g.add_frame_time(ms as f32 / 1000.0);
        g.view_frame();
    }
}

/// Ann hosts, Bob joins; both in the game and Bob seeing Ann.
fn two() -> (ArenaGame, ArenaGame) {
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
        if b.in_game() && b.scene().others().len() == 1 {
            break;
        }
    }
    assert_eq!(b.scene().others().len(), 1, "Bob sees Ann");
    (a, b)
}

#[test]
fn another_player_walking_in_a_circle_moves_and_turns_steadily() {
    let (mut a, mut b) = two();
    // The server sends each client a packet every tick (the scripts' `$pref::Net::PacketRateToClient` 32 on both sides): about
    // 31 a second to each of the two, where TGE's default rate sends 10.
    let sent = |a: &ArenaGame| {
        a.server
            .as_ref()
            .unwrap()
            .shared
            .borrow()
            .counters
            .packets_sent
    };
    let before = sent(&a);
    for _ in 0..64 {
        frame(&mut a, &mut b, 16);
    }
    let rate = sent(&a) - before;
    assert!(rate >= 55, "{rate} packets a second to the two clients");

    // Ann walks forward, turning right at 0.75 radians a second (4 mouse counts each 16 ms).
    a.key(keys::W, true);
    let (speed, turn) = (7.0f32, 0.75f32);
    let mut last: Option<([f32; 3], f32)> = None;
    let mut elapsed = 0u32;
    for i in 0..400 {
        // Frames of uneven length, as a window's are.
        let ms = [7u32, 16, 9, 33][i % 4];
        a.host.mouse_motion(4.0 * ms as f32 / 16.0, 0.0);
        frame(&mut a, &mut b, ms);
        elapsed += ms;
        let now = b.scene().others()[0];
        if let Some((p, yaw)) = last
            && elapsed > 1000
        {
            let dt = ms as f32 / 1000.0;
            let d = [now.0[0] - p[0], now.0[1] - p[1], now.0[2] - p[2]];
            let step = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let ahead = d[0] * yaw.sin() + d[1] * yaw.cos();
            let turned = (now.1 - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            assert!(
                ahead > -0.01,
                "frame {i}: a step back ({ahead}) from {p:?} to {:?}",
                now.0
            );
            assert!(
                step < speed * dt * 1.6 + 0.02,
                "frame {i}: a jump of {step} in {ms} ms"
            );
            assert!(turned > -0.005, "frame {i}: turned back by {turned}");
            assert!(
                turned < turn * dt * 1.8 + 0.01,
                "frame {i}: a turn of {turned} in {ms} ms"
            );
        }
        last = Some(now);
    }
    a.key(keys::W, false);
}

#[test]
fn a_jumping_figure_never_sinks_into_the_floor() {
    let (mut a, mut b) = two();
    // The floor's top is at 0; a standing figure's feet a hair above it.
    a.key(keys::W, true);
    let mut highest = 0.0f32;
    for i in 0..300 {
        if i % 50 == 5 {
            a.key(keys::SPACE, true);
            a.key(keys::SPACE, false);
        }
        frame(&mut a, &mut b, [7u32, 16, 9, 33][i % 4]);
        let own = a.scene().own().unwrap().position[2];
        let other = b.scene().others()[0].0[2];
        highest = highest.max(own);
        assert!(own > -0.002, "frame {i}: Ann draws her feet at {own}");
        assert!(other > -0.002, "frame {i}: Bob draws Ann's feet at {other}");
    }
    assert!(highest > 0.5, "Ann jumped ({highest})");
    a.key(keys::W, false);
}
