//! The example without a window or a GPU: clients and the server one of them hosts in one process over the loopback address,
//! the keys and the clicks posted as a window posts them, and the figures and the buggy read from the server's simulation.

use torque::client::input::{Key, MouseButton};
use torque::ghosts::TgeClass;
use torque_example_vehicle_demo::client::{ClientOptions, NetMode, VehicleDemo};
use torque_example_vehicle_demo::files::GameFiles;
use torque_example_vehicle_demo::server::WorldServer;

fn client(name: &str) -> VehicleDemo {
    VehicleDemo::new(
        GameFiles::bundled().expect("the game folder"),
        ClientOptions {
            net: NetMode::Loopback,
            name: name.to_string(),
            port: 0,
            ..ClientOptions::default()
        },
    )
    .expect("the client")
}

/// Steps the clients (the host first) `ms` of the clock.
fn steps(games: &mut [&mut VehicleDemo], ms: u32) {
    for _ in 0..ms / 32 {
        for g in games.iter_mut() {
            g.step(32);
        }
    }
}

/// Steps until `done`, at most ten seconds; false when it never is.
fn until(games: &mut [&mut VehicleDemo], done: impl Fn(&[&mut VehicleDemo]) -> bool) -> bool {
    for _ in 0..320 {
        if done(games) {
            return true;
        }
        steps(games, 32);
    }
    done(games)
}

/// A client hosting a game, in the world, the buggy settled.
fn host() -> VehicleDemo {
    let mut a = client("Ann");
    a.host_game().expect("the hosted game");
    assert!(
        until(&mut [&mut a], |g| g[0].in_world()
            && g[0].buggy_ghost().is_some()),
        "{:#?}",
        a.server_log
    );
    steps(&mut [&mut a], 2000);
    a
}

/// A second client joined to `a`'s game.
fn join(a: &mut VehicleDemo) -> VehicleDemo {
    let mut b = client("Bob");
    let address = a.hosted_address().expect("the hosted address");
    b.join(&address.to_string()).expect("the join");
    assert!(
        until(&mut [a, &mut b], |g| g[1].in_world()
            && g[1].buggy_ghost().is_some()),
        "{:#?}",
        a.server_log
    );
    steps(&mut [a, &mut b], 1000);
    b
}

fn server(game: &VehicleDemo) -> &WorldServer {
    game.server.as_ref().expect("the server")
}

fn click(game: &mut VehicleDemo) {
    game.host.mouse_button(MouseButton::Left, true, (400, 300));
    game.host.mouse_button(MouseButton::Left, false, (400, 300));
}

/// Client `i` looks down at the buggy and clicks.
fn get_in(games: &mut [&mut VehicleDemo], i: usize) {
    games[i].host.mouse_motion(0.0, 110.0);
    steps(games, 300);
    click(games[i]);
    steps(games, 600);
}

fn errors(game: &VehicleDemo) -> Vec<&String> {
    game.server_log
        .iter()
        .chain(&game.client_log)
        .filter(|l| {
            l.contains("Unknown command")
                || l.contains("Unable to find")
                || l.contains("error")
                || l.contains("not found")
        })
        .collect()
}

fn horizontal(v: [f32; 3]) -> f32 {
    v[0].hypot(v[1])
}

/// A hosted game ends at once when the window closes: the server stopped (its `onExit` run, its socket closed) and the client
/// let go within a moment, with nothing left waiting on a tick, a socket or a thread.
#[test]
fn a_hosted_game_stops_and_lets_go_within_half_a_second() {
    let mut game = host();
    let address = game.hosted_address().expect("the hosted address");
    let began = std::time::Instant::now();
    game.stop_hosting();
    assert!(game.hosted_address().is_none());
    drop(game);
    let took = began.elapsed();
    assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    // Nothing holds the server's socket: its address can be bound again.
    std::net::UdpSocket::bind(address).expect("the server's address is free");
}

#[test]
fn the_figure_and_the_buggy_stand_on_the_terrain() {
    let game = host();
    let s = server(&game);
    let figure = s.figure_position().expect("the figure");
    let buggy = s.buggy_position().expect("the buggy");
    let ground = |p: [f32; 3]| game.terrain.height(p[0], p[1]).expect("on the terrain");
    assert!(
        (figure[2] - ground(figure)).abs() < 0.1,
        "the figure at {figure:?}"
    );
    // The buggy's origin rides a little above the ground on its springs, level.
    let above = buggy[2] - ground(buggy);
    assert!(
        (-0.3..0.4).contains(&above),
        "the buggy {above} above the ground"
    );
    assert!(s.buggy_axis(2).unwrap()[2] > 0.99, "the buggy is level");
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
}

#[test]
fn a_click_on_the_buggy_seats_the_figure_at_the_wheel_and_e_puts_it_out_beside() {
    let mut game = host();
    // A click looking away (up at the sky) does nothing.
    game.host.mouse_motion(0.0, -300.0);
    steps(&mut [&mut game], 200);
    click(&mut game);
    steps(&mut [&mut game], 300);
    assert_eq!(server(&game).figure_mount(), None);
    game.host.mouse_motion(0.0, 300.0);
    get_in(&mut [&mut game], 0);
    let s = server(&game);
    let figure = s.figure().unwrap();
    assert_eq!(
        s.seat_of(figure),
        s.buggy().map(|b| (b, 0)),
        "{:#?}",
        game.server_log
    );
    assert!(game.driving());
    // The client's ghost of the figure sits in the buggy's ghost, on the driver's seat.
    assert_eq!(game.figure_mount(), game.buggy_ghost().map(|j| (j, 0)));
    game.key(Key::E, true);
    game.key(Key::E, false);
    steps(&mut [&mut game], 1500);
    assert!(!game.seated());
    assert_eq!(game.figure_mount(), None);
    let s = server(&game);
    assert_eq!(s.figure_mount(), None);
    let figure = s.figure_position().unwrap();
    let buggy = s.buggy_position().unwrap();
    let apart = (figure[0] - buggy[0]).hypot(figure[1] - buggy[1]);
    assert!(
        (1.8..3.5).contains(&apart),
        "{apart} from the buggy: {figure:?}"
    );
    // On the driver's side (left), standing on the ground.
    let right = s.buggy_axis(0).unwrap();
    assert!((figure[0] - buggy[0]) * right[0] + (figure[1] - buggy[1]) * right[1] < -1.5);
    let ground = game.terrain.height(figure[0], figure[1]).unwrap();
    assert!((figure[2] - ground).abs() < 0.1, "{figure:?} over {ground}");
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
}

#[test]
fn the_throttle_drives_forward_a_turns_left_the_buggy_coasts_and_space_brakes() {
    let mut game = host();
    get_in(&mut [&mut game], 0);
    assert!(game.driving());
    let from = server(&game).buggy_position().unwrap();
    game.key(Key::W, true);
    steps(&mut [&mut game], 3000);
    game.key(Key::W, false);
    let s = server(&game);
    let to = s.buggy_position().unwrap();
    assert!(to[1] - from[1] > 10.0, "from {from:?} to {to:?}");
    assert!((to[0] - from[0]).abs() < 2.0, "straight ahead: {to:?}");
    assert!(s.buggy_axis(2).unwrap()[2] > 0.9);
    // Off the throttle it rolls on.
    let fast = horizontal(s.buggy_velocity().unwrap());
    steps(&mut [&mut game], 1000);
    let rolling = horizontal(server(&game).buggy_velocity().unwrap());
    assert!(rolling > 0.5 * fast, "coasting from {fast} to {rolling}");
    // A turns the buggy left (toward -x).
    let f0 = server(&game).buggy_axis(1).unwrap();
    game.key(Key::W, true);
    game.key(Key::A, true);
    steps(&mut [&mut game], 1200);
    game.key(Key::A, false);
    game.key(Key::W, false);
    let f1 = server(&game).buggy_axis(1).unwrap();
    let turned = f0[0].atan2(f0[1]) - f1[0].atan2(f1[1]);
    assert!(turned > 0.3, "turned {turned} rad: {f0:?} to {f1:?}");
    // The brake (Space) stops it, and the driver stays in the seat.
    game.key(Key::Space, true);
    steps(&mut [&mut game], 2500);
    game.key(Key::Space, false);
    let speed = horizontal(server(&game).buggy_velocity().unwrap());
    assert!(speed < 0.5, "still {speed} after the brake");
    assert!(game.driving() && server(&game).figure_mount().is_some());
}

#[test]
fn the_buggy_climbs_the_hills_steep_side_upright() {
    let mut game = host();
    get_in(&mut [&mut game], 0);
    game.key(Key::W, true);
    let mut least_up = 1.0f32;
    let mut highest = f32::MIN;
    let mut pressed = 1.0f32;
    for _ in 0..(16_000 / 32) {
        game.step(32);
        let s = server(&game);
        least_up = least_up.min(s.buggy_axis(2).unwrap()[2]);
        highest = highest.max(s.buggy_position().unwrap()[2]);
        if let Some(least) = s.buggy_extensions().into_iter().reduce(f32::min) {
            pressed = pressed.min(least);
        }
        if s.buggy_position().unwrap()[1] > 140.0 {
            break;
        }
    }
    game.key(Key::W, false);
    let at = server(&game).buggy_position().unwrap();
    assert!(at[1] > 130.0, "over the hill: {at:?}");
    assert!(least_up > 0.75, "tipped to {least_up}");
    let top = game.terrain.height(0.0, 95.0).unwrap();
    assert!(
        highest > top - 1.0,
        "climbed to {highest}, the top at {top}"
    );
    assert!(pressed < 0.3, "the springs pressed to {pressed}");
}

#[test]
fn two_players_take_both_seats_one_drives_the_other_rides_and_both_get_out_on_their_sides() {
    let mut a = host();
    let mut b = join(&mut a);
    {
        let s = server(&a);
        assert_eq!(s.figures().len(), 2, "{:#?}", a.server_log);
    }
    // The first, on the left, takes the driver's seat; the second, on the right, the passenger's.
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    let figures = server(&a).figures();
    let buggy = server(&a).buggy().unwrap();
    assert_eq!(server(&a).seat_of(figures[0]), Some((buggy, 0)));
    assert_eq!(server(&a).seat_of(figures[1]), Some((buggy, 1)));
    assert!(a.driving() && b.seat() == Some(1));
    // Each client sees both figures seated.
    steps(&mut [&mut a, &mut b], 500);
    let mut seats_a: Vec<u32> = a.riders().iter().map(|r| r.1).collect();
    let mut seats_b: Vec<u32> = b.riders().iter().map(|r| r.1).collect();
    seats_a.sort_unstable();
    seats_b.sort_unstable();
    assert_eq!((seats_a, seats_b), (vec![0, 1], vec![0, 1]));
    // The passenger's throttle moves nothing.
    b.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 1500);
    b.key(Key::W, false);
    let still = horizontal(server(&a).buggy_velocity().unwrap());
    assert!(still < 0.5, "the passenger drove it at {still}");
    // The driver drives both; the passenger's client sees it go.
    let from = server(&a).buggy_position().unwrap();
    a.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 2500);
    a.key(Key::W, false);
    a.key(Key::Space, true);
    steps(&mut [&mut a, &mut b], 2500);
    a.key(Key::Space, false);
    let to = server(&a).buggy_position().unwrap();
    assert!(to[1] - from[1] > 8.0, "from {from:?} to {to:?}");
    let seen = b.buggy_pose().unwrap().column(3);
    assert!(
        (seen[1] - to[1]).abs() < 1.0,
        "the passenger sees it at {seen:?}, it is at {to:?}"
    );
    // Both get out, each on its own side.
    for game in [&mut a, &mut b] {
        game.key(Key::E, true);
        game.key(Key::E, false);
    }
    steps(&mut [&mut a, &mut b], 1500);
    assert!(!a.seated() && !b.seated());
    let s = server(&a);
    let at = s.buggy_position().unwrap();
    let right = s.buggy_axis(0).unwrap();
    let side = |f: u32| {
        let p = s.figure_position_of(f).unwrap();
        (p[0] - at[0]) * right[0] + (p[1] - at[1]) * right[1]
    };
    assert!(side(figures[0]) < -1.5 && side(figures[1]) > 1.5);
    assert_eq!((s.seat_of(figures[0]), s.seat_of(figures[1])), (None, None));
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

fn tab(game: &mut VehicleDemo) {
    game.key(Key::Tab, true);
    game.key(Key::Tab, false);
    steps(&mut [game], 64);
}

#[test]
fn tab_switches_the_camera_on_foot_and_seated_and_the_seated_eye_rides_at_the_seat() {
    let mut game = host();
    assert!(!game.first_person());
    tab(&mut game);
    assert!(game.first_person());
    tab(&mut game);
    assert!(!game.first_person());
    get_in(&mut [&mut game], 0);
    tab(&mut game);
    assert!(game.first_person() && game.driving());
    // At the driver's eyes: the figure's `eye` node in the seat, a little ahead of the face, on the buggy where it is drawn.
    let local = game.seated_eye_local().expect("a seated eye");
    let node = torque_example_assets::buggy::seated_eye(0);
    let apart = (0..3)
        .map(|k| (local[k] - node[k]).powi(2))
        .sum::<f32>()
        .sqrt();
    assert!(
        apart < 0.3 && local[1] > node[1],
        "{local:?} from the node {node:?}"
    );
    let at = game.seated_eye_point().unwrap();
    let pose = game.buggy_pose().unwrap();
    let expect = pose.mul_p(local);
    assert!(
        (0..3).all(|k| (at[k] - expect[k]).abs() < 1e-3),
        "{at:?} {expect:?}"
    );
    // It moves with the buggy.
    game.key(Key::W, true);
    steps(&mut [&mut game], 2000);
    game.key(Key::W, false);
    let moved = game.seated_eye_point().unwrap();
    let buggy = game.buggy_pose().unwrap().column(3);
    assert!(moved[1] - at[1] > 6.0, "{at:?} to {moved:?}");
    assert!(
        (moved[1] - buggy[1]).abs() < 1.0,
        "{moved:?} on the buggy at {buggy:?}"
    );
    // Third person again.
    tab(&mut game);
    assert!(!game.first_person() && game.driving());
}

/// Steps and draws (no GPU: the frame's motion only) for `ms`.
fn frames(game: &mut VehicleDemo, ms: u32) {
    for _ in 0..ms / 32 {
        game.step(32);
        game.update_scene(0.032);
    }
}

#[test]
fn the_tires_throw_dust_and_lay_tracks_on_the_terrain_only_while_they_roll_on_it() {
    let mut game = host();
    get_in(&mut [&mut game], 0);
    frames(&mut game, 1000);
    // Standing: no dust, no tracks.
    assert_eq!(game.scene().dust_particles(), 0);
    assert_eq!(game.scene().track_segments(), 0);
    game.key(Key::W, true);
    frames(&mut game, 2500);
    let dust = game.scene().dust_particles();
    let laid = game.scene().track_segments();
    assert!(dust > 20, "{dust} dust particles");
    assert!(laid > 30, "{laid} track segments");
    // The tracks lie on the terrain (a hair above it at most: the drawn bias is not in the mesh).
    let tracks = game.scene().tracks().unwrap();
    for s in tracks.segments() {
        for tri in &s.triangles {
            for (p, _) in tri {
                let ground = game.terrain.height(p[0], p[1]).unwrap();
                assert!((p[2] - ground).abs() < 0.02, "{p:?} over {ground}");
            }
        }
    }
    // Stopped (the brake), the dust settles and no more tracks are laid.
    game.key(Key::W, false);
    game.key(Key::Space, true);
    frames(&mut game, 3000);
    let stopped = game.scene().track_segments();
    frames(&mut game, 2000);
    game.key(Key::Space, false);
    assert_eq!(game.scene().dust_particles(), 0);
    assert_eq!(game.scene().track_segments(), stopped);
}

fn press(game: &mut VehicleDemo, key: Key) {
    game.key(key, true);
    game.key(key, false);
}

#[test]
fn f_switches_the_headlight_for_both_players_and_only_the_driver_may() {
    let mut a = host();
    let mut b = join(&mut a);
    // Ghosted over the link to both, off.
    assert_eq!(
        (a.headlight_on(), b.headlight_on()),
        (Some(false), Some(false))
    );
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(a.driving() && b.seat() == Some(1));
    // The passenger's F does nothing.
    press(&mut b, Key::F);
    steps(&mut [&mut a, &mut b], 500);
    assert_eq!(
        (a.headlight_on(), b.headlight_on()),
        (Some(false), Some(false))
    );
    // The driver's switches it on for both; the passenger's client draws it on the buggy's nose, pointing ahead.
    press(&mut a, Key::F);
    steps(&mut [&mut a, &mut b], 500);
    assert_eq!(
        (a.headlight_on(), b.headlight_on()),
        (Some(true), Some(true))
    );
    let spots = b.spot_lights();
    assert_eq!(spots.len(), 1, "{spots:?}");
    let nose = b
        .buggy_pose()
        .unwrap()
        .mul_p(torque_example_assets::buggy::HEADLIGHT);
    let at = spots[0].light.position;
    assert!(
        (0..3).all(|k| (at[k] - nose[k]).abs() < 0.05),
        "{at:?} {nose:?}"
    );
    let ahead = b.buggy_pose().unwrap().column(1);
    // From the roof, ahead and down.
    let along: f32 = (0..3).map(|k| spots[0].direction[k] * ahead[k]).sum();
    assert!(
        along > 0.95 && spots[0].direction[2] < -0.2,
        "{:?}",
        spots[0].direction
    );
    // The roof lamps glow on both clients.
    assert!(a.roof_lamps_lit() && b.roof_lamps_lit());
    // And off again.
    press(&mut a, Key::F);
    steps(&mut [&mut a, &mut b], 500);
    assert_eq!(
        (a.headlight_on(), b.headlight_on()),
        (Some(false), Some(false))
    );
    assert!(b.spot_lights().is_empty());
    assert!(!a.roof_lamps_lit() && !b.roof_lamps_lit());
}

#[test]
fn the_day_reaches_the_client_and_its_sun_moves_on() {
    let mut game = host();
    frames(&mut game, 200);
    let morning = game.sun().expect("the day is ghosted");
    // Mid-morning: the sun up in the east (+x) side of the sky.
    assert!(morning[2] > 0.3, "{morning:?}");
    frames(&mut game, 10_000);
    let later = game.sun().unwrap();
    let dot: f32 = (0..3).map(|k| morning[k] * later[k]).sum();
    // Ten seconds of a ten-minute day: six degrees on.
    let turned = dot.clamp(-1.0, 1.0).acos().to_degrees();
    assert!((4.0..8.0).contains(&turned), "{turned} degrees");
    assert!(later[2] > morning[2], "rising: {morning:?} to {later:?}");
    // The server's own day agrees (the time it holds).
    let time: f32 = game
        .server
        .as_mut()
        .unwrap()
        .call("eval", &["return WorldDay.time;"])
        .unwrap()
        .parse()
        .unwrap();
    assert!((0.13..0.16).contains(&time), "{time}");
}

#[test]
fn getting_out_stands_the_figure_up_on_the_server_and_both_clients() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    let figures = server(&a).figures();
    let seated = |a: &VehicleDemo, f: u32| server(a).figure_action_of(f).unwrap();
    for g in [&mut a, &mut b] {
        g.update_scene(0.032);
    }
    assert_eq!(
        (
            seated(&a, figures[0]).as_str(),
            seated(&a, figures[1]).as_str()
        ),
        ("sit", "ride")
    );
    let names = |g: &VehicleDemo| -> Vec<String> {
        g.scene()
            .figure_actions()
            .into_iter()
            .map(|x| x.1)
            .collect()
    };
    let mut on_a = names(&a);
    on_a.sort();
    assert_eq!(on_a, ["ride", "sit"]);
    for game in [&mut a, &mut b] {
        game.key(Key::E, true);
        game.key(Key::E, false);
    }
    for _ in 0..1500 / 32 {
        for g in [&mut a, &mut b] {
            g.step(32);
            g.update_scene(0.032);
        }
    }
    assert!(!a.seated() && !b.seated());
    let standing = |n: &str| n == "root" || n == "run";
    for f in &figures {
        let n = server(&a).figure_action_of(*f).unwrap();
        assert!(standing(&n), "the server's figure {f} plays {n}");
    }
    for (who, g) in [("Ann", &a), ("Bob", &b)] {
        let all = names(g);
        assert_eq!(all.len(), 2, "{who} poses {all:?}");
        assert!(all.iter().all(|n| standing(n)), "{who} draws {all:?}");
    }
    // In again (turned right, to the buggy): seated again on both.
    a.host.mouse_motion(524.0, 0.0);
    steps(&mut [&mut a, &mut b], 300);
    click(&mut a);
    steps(&mut [&mut a, &mut b], 600);
    assert!(
        a.driving(),
        "{:?}",
        server(&a).figure_position_of(figures[0])
    );
    for _ in 0..300 / 32 {
        for g in [&mut a, &mut b] {
            g.step(32);
            g.update_scene(0.032);
        }
    }
    assert_eq!(seated(&a, figures[0]), "sit");
    assert!(names(&b).contains(&"sit".to_string()), "{:?}", names(&b));
}

#[test]
fn getting_in_after_turning_the_mouse_leaves_the_wheels_straight() {
    let mut game = host();
    // Turning round and round on foot, then back to face the buggy, still turning as the click lands.
    for _ in 0..20 {
        game.host.mouse_motion(400.0, 0.0);
        steps(&mut [&mut game], 32);
    }
    for _ in 0..20 {
        game.host.mouse_motion(-400.0, 0.0);
        steps(&mut [&mut game], 32);
    }
    game.host.mouse_motion(0.0, 110.0);
    steps(&mut [&mut game], 300);
    game.host.mouse_motion(300.0, 0.0);
    click(&mut game);
    for _ in 0..10 {
        game.host.mouse_motion(300.0, 0.0);
        steps(&mut [&mut game], 32);
    }
    assert!(game.driving());
    // A second at the wheel with no steering key: the wheels stay straight on the server and on the client.
    let mut worst = (0.0f32, 0.0f32);
    for _ in 0..1000 / 32 {
        game.host.mouse_motion(200.0, 0.0);
        steps(&mut [&mut game], 32);
        let s = server(&game).buggy_state().unwrap().steering[0].abs();
        let c = game.driven().map_or(0.0, |d| d.steering.abs());
        worst = (worst.0.max(s), worst.1.max(c));
    }
    assert!(
        worst.0 < 1e-3 && worst.1 < 1e-3,
        "steering on the server {} on the client {}",
        worst.0,
        worst.1
    );
}

#[test]
fn a_game_played_alone_binds_no_socket_and_the_player_drives() {
    let mut game = client("Ann");
    game.set_global("$World::AloneAsked", "1");
    assert!(
        until(&mut [&mut game], |g| g[0].in_world()
            && g[0].buggy_ghost().is_some()),
        "{:#?}",
        game.server_log
    );
    // The server listens on the in-memory link only: nothing on the network can reach or list it.
    let s = server(&game);
    assert!(s.memory_end().is_some());
    assert_eq!(
        s.address(),
        Some(torque_example_vehicle_demo::server::ALONE_ADDR)
    );
    assert!(game.local());
    steps(&mut [&mut game], 2000);
    get_in(&mut [&mut game], 0);
    assert!(game.driving());
    let from = server(&game).buggy_position().unwrap();
    game.key(Key::W, true);
    steps(&mut [&mut game], 2000);
    game.key(Key::W, false);
    let to = server(&game).buggy_position().unwrap();
    assert!(
        horizontal([to[0] - from[0], to[1] - from[1], 0.0]) > 5.0,
        "{from:?} to {to:?}"
    );
    // F, Tab and the day work the same.
    press(&mut game, Key::F);
    frames(&mut game, 300);
    assert_eq!(game.headlight_on(), Some(true));
    assert!(game.sun().is_some());
    tab(&mut game);
    assert!(game.first_person());
    // Back to the menu (Esc, then Leave game): the client's own socket is given back.
    press(&mut game, Key::Escape);
    press_button(&mut game, "LeaveButton");
    steps(&mut [&mut game], 100);
    assert!(game.server.is_none() && !game.local());
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
}

/// Frames drawn 60 a second for `ms` of the clock, from `start_ms` on, by each client: their hosts count them as a window does.
fn drawn_frames(games: &mut [&mut VehicleDemo], start_ms: f64, ms: u32) {
    for i in 0..ms / 16 {
        for g in games.iter_mut() {
            g.host.frame_drawn(start_ms + f64::from(i) * 1000.0 / 60.0);
            g.step(16);
        }
    }
}

fn readout(game: &mut VehicleDemo) -> String {
    game.call("eval", &["return PlayStats.getValue();"])
        .unwrap()
}

/// The corner readout: on the client that joined over the network, the frame rate and the round trip to the server; on the
/// host's own client and played alone (the server in the same process), the frame rate alone.
#[test]
fn the_corner_readout_holds_the_frame_rate_and_the_joiners_round_trip() {
    let mut a = host();
    let mut b = join(&mut a);
    drawn_frames(&mut [&mut a, &mut b], 0.0, 1000);
    assert_eq!(readout(&mut a), "60 fps");
    let text = readout(&mut b);
    let words: Vec<&str> = text.split(' ').collect();
    assert!(
        matches!(words[..], ["60", "fps", "", ping, "ms"] if ping.parse::<u32>().is_ok()),
        "{text:?}"
    );
    let ping: u32 = words[3].parse().unwrap();
    assert!(ping > 0 && ping < 500, "{ping}");
    drop(b);
    drop(a);

    let mut alone = client("Ann");
    alone.set_global("$World::AloneAsked", "1");
    assert!(until(&mut [&mut alone], |g| g[0].in_world()));
    drawn_frames(&mut [&mut alone], 0.0, 1000);
    assert_eq!(readout(&mut alone), "60 fps");
}

#[test]
fn solo_with_host_or_join_is_refused() {
    use torque_example_vehicle_demo::client::Start;
    assert_eq!(Start::from_flags(true, false, None), Ok(Start::Alone));
    assert!(Start::from_flags(true, true, None).is_err());
    assert!(Start::from_flags(true, false, Some("127.0.0.1".into())).is_err());
    assert!(Start::from_flags(false, true, Some("127.0.0.1".into())).is_err());
}

/// One drawn frame's motion and eye.
fn drawn(game: &mut VehicleDemo) -> torque_render_scene::source::Eye {
    game.step(16);
    game.update_scene(0.016);
    game.drawn_eye(0.016)
}

fn apart(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

#[test]
fn tab_eases_the_camera_between_the_seat_and_behind_and_on_foot_hides_the_figure_only_at_its_eyes()
{
    let mut game = host();
    get_in(&mut [&mut game], 0);
    let behind = drawn(&mut game);
    press(&mut game, Key::Tab);
    let mut last = behind;
    let mut biggest = 0.0f32;
    let mut frames = 0;
    loop {
        let e = drawn(&mut game);
        biggest = biggest.max(apart(last.position, e.position));
        last = e;
        frames += 1;
        if !game.camera_blending() {
            break;
        }
        assert!(frames < 60, "the blend never ends");
    }
    let seat = game.seated_eye_point().unwrap();
    let whole = apart(behind.position, seat);
    // 95% of the way in a quarter second, the last millimeter some 0.5 s in at 60 frames a second; no frame's step more than
    // the spring's fastest share of the way.
    assert!((25..=40).contains(&frames), "{frames} frames");
    assert!(
        biggest < whole * 0.13 + 0.05,
        "a step of {biggest} over {whole}"
    );
    assert!(
        apart(last.position, seat) < 1e-4,
        "{:?} at {seat:?}",
        last.position
    );
    // Back, restarted halfway: no jump.
    press(&mut game, Key::Tab);
    for _ in 0..8 {
        last = drawn(&mut game);
    }
    press(&mut game, Key::Tab);
    let next = drawn(&mut game);
    assert!(apart(last.position, next.position) < whole * 0.13 + 0.05);
    for _ in 0..40 {
        drawn(&mut game);
    }
    assert!(!game.camera_blending());
    // On foot: the figure shows until the camera is close to its eyes, and again once it is out of the head.
    press(&mut game, Key::E);
    for _ in 0..60 {
        drawn(&mut game);
    }
    assert!(!game.seated() && game.first_person());
    let hidden = |g: &VehicleDemo| g.with_world(|w| w.hide_control).unwrap();
    assert!(hidden(&game));
    // Out: shown once the camera is out of the head, early in the move.
    let shown_within = |game: &mut VehicleDemo| {
        for _ in 0..12 {
            drawn(game);
            if !hidden(game) {
                return game.camera_blending();
            }
        }
        false
    };
    press(&mut game, Key::Tab);
    assert!(shown_within(&mut game));
    for _ in 0..30 {
        drawn(&mut game);
    }
    // In, and out again before it got there: shown all the while.
    press(&mut game, Key::Tab);
    for _ in 0..4 {
        drawn(&mut game);
        assert!(!hidden(&game) && game.camera_blending());
    }
    press(&mut game, Key::Tab);
    drawn(&mut game);
    assert!(!hidden(&game) && game.camera_blending());
    for _ in 0..40 {
        drawn(&mut game);
    }
    press(&mut game, Key::Tab);
    for _ in 0..40 {
        drawn(&mut game);
    }
    assert!(hidden(&game));
}

/// Each client's wheels' spin over `ms` of frames, unwrapped (turns).
fn turned(games: &mut [&mut VehicleDemo], watched: &[u32], ms: u32) -> Vec<Vec<f32>> {
    let mut last: Vec<Vec<(f32, bool)>> = games
        .iter()
        .zip(watched)
        .map(|(g, &w)| g.wheel_spins_of(w))
        .collect();
    let mut total: Vec<Vec<f32>> = last.iter().map(|l| vec![0.0; l.len()]).collect();
    for _ in 0..ms / 16 {
        for g in games.iter_mut() {
            g.step(16);
            g.update_scene(0.016);
        }
        for (i, g) in games.iter().enumerate() {
            let now = g.wheel_spins_of(watched[i]);
            for (k, (a, _)) in now.iter().enumerate() {
                if let (Some((b, _)), Some(t)) = (last[i].get(k), total[i].get_mut(k)) {
                    let mut d = a - b;
                    d -= d.round();
                    *t += d;
                }
            }
            last[i] = now;
        }
    }
    total
}

#[test]
fn the_wheels_turn_with_the_ground_forward_and_back_for_both_players() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    // Each client's ghost of the driven buggy (the other's own buggy may come nearer it as this one drives off).
    let watched = [a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap()];
    let radius = torque_example_assets::buggy::TIRE_RADIUS;
    for (key, sign) in [(Key::W, 1.0f32), (Key::S, -1.0)] {
        a.key(key, true);
        steps(&mut [&mut a, &mut b], 1500);
        let from = server(&a).buggy_position().unwrap();
        let spins = turned(&mut [&mut a, &mut b], &watched, 1000);
        let to = server(&a).buggy_position().unwrap();
        a.key(key, false);
        // About the distance over the tire's round, the way it goes, on the driver's client and on the other's.
        let expect =
            horizontal([to[0] - from[0], to[1] - from[1], 0.0]) / (std::f32::consts::TAU * radius);
        assert!(expect > 1.0, "{expect}");
        for (who, t) in ["driver", "other"].iter().zip(&spins) {
            assert_eq!(t.len(), 4, "{who}: {t:?}");
            for x in t {
                assert!(
                    x * sign > 0.75 * expect && x * sign < 1.25 * expect,
                    "{who} {key:?}: {t:?} for {expect}"
                );
            }
        }
        a.key(Key::Space, true);
        steps(&mut [&mut a, &mut b], 2500);
        a.key(Key::Space, false);
    }
}

#[test]
fn a_and_d_turn_the_steering_wheel_on_both_clients() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    frames_both(&mut a, &mut b, 300);
    let places = |a: &VehicleDemo, b: &VehicleDemo| {
        (a.steering_place().unwrap(), b.steering_place().unwrap())
    };
    let (ca, cb) = places(&a, &b);
    assert!(
        (ca - 0.5).abs() < 0.01 && (cb - 0.5).abs() < 0.01,
        "{ca} {cb}"
    );
    // D: to the right (the thread toward 0) on both; A: to the left (toward 1).
    for (key, right) in [(Key::D, true), (Key::A, false)] {
        a.key(key, true);
        frames_both(&mut a, &mut b, 700);
        let (pa, pb) = places(&a, &b);
        a.key(key, false);
        // At full lock: the wheel's whole turn (the thread at 0.5 -+ the lock over 2).
        let lock = torque_example_assets::buggy::MAX_STEERING * 0.5;
        let full = if right { 0.5 - lock } else { 0.5 + lock };
        for p in [pa, pb] {
            assert!(
                (p - full).abs() < 0.01,
                "{key:?}: driver {pa}, other {pb}, full lock {full}"
            );
        }
        // The driver's hands follow: the driver's figure holds its own `steering` thread at the buggy's place, on both clients.
        for (who, g, p) in [("driver", &a, pa), ("other", &b, pb)] {
            let held = g.scene().figure_steering();
            assert_eq!(held.len(), 1, "{who}: {held:?}");
            assert!((held[0].1 - p).abs() < 0.02, "{who}: {held:?} for {p}");
        }
        frames_both(&mut a, &mut b, 700);
    }
}

fn frames_both(a: &mut VehicleDemo, b: &mut VehicleDemo, ms: u32) {
    for _ in 0..ms / 16 {
        for g in [&mut *a, &mut *b] {
            g.step(16);
            g.update_scene(0.016);
        }
    }
}

#[test]
fn the_mouse_tilts_the_seated_view_up_and_down_for_driver_and_passenger() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    // Both clients' frames; the eye of client `i`.
    let run = |a: &mut VehicleDemo, b: &mut VehicleDemo, i: usize| {
        let mut eye = None;
        for _ in 0..20 {
            a.step(16);
            b.step(16);
            let (ea, eb) = (a.drawn_eye(0.016), b.drawn_eye(0.016));
            a.update_scene(0.016);
            b.update_scene(0.016);
            eye = Some(if i == 0 { ea } else { eb });
        }
        eye.unwrap().pitch
    };
    for first in [false, true] {
        for i in 0..2 {
            if i == 0 {
                a.set_first_person(first)
            } else {
                b.set_first_person(first)
            }
            let before = run(&mut a, &mut b, i);
            if i == 0 {
                a.host.mouse_motion(0.0, -120.0)
            } else {
                b.host.mouse_motion(0.0, -120.0)
            }
            let up = run(&mut a, &mut b, i);
            if i == 0 {
                a.host.mouse_motion(0.0, 240.0)
            } else {
                b.host.mouse_motion(0.0, 240.0)
            }
            let down = run(&mut a, &mut b, i);
            assert!(
                up < before - 0.2 && down > before + 0.2,
                "first person {first}, seat {i}: {before} {up} {down}"
            );
            if i == 0 {
                a.host.mouse_motion(0.0, -120.0)
            } else {
                b.host.mouse_motion(0.0, -120.0)
            }
        }
    }
}

/// The newest tracks and dust of `buggy` on `game` against its wheels as `game` draws them this frame: the farthest along
/// its way of each strip's head and the newest dust's spawn, each the least distance along the way from the bottom of a tire
/// as the frame draws it, across it within a tire's width. None while it shows neither.
fn under_wheels(game: &VehicleDemo, buggy: u32, dir: [f32; 2]) -> Option<(f32, f32)> {
    use torque_example_assets::buggy::{HUB_FRONT, HUB_REAR, HUB_X, HUB_Z, TIRE_RADIUS};
    // `BuggySpring`'s length (server.cs).
    const SPRING_LENGTH: f32 = 0.42;
    if game.wheel_contacts_of(buggy).is_empty() {
        return None;
    }
    let (pose, extensions) = game
        .with_world(|w| {
            let look = w.vehicle_look.get(&buggy)?;
            Some((
                *w.vehicle_render.get(&buggy)?,
                look.wheels.iter().map(|w| w.extension).collect::<Vec<_>>(),
            ))
        })
        .flatten()?;
    let (m, _) = torque_render_scene::mounts::pose_matrix(&pose, None)?;
    // The bottom of each tire as the frame draws it: its hub on the buggy, down the spring and the tire's radius.
    let contacts: Vec<[f32; 3]> = [
        (-HUB_X, HUB_FRONT),
        (HUB_X, HUB_FRONT),
        (-HUB_X, HUB_REAR),
        (HUB_X, HUB_REAR),
    ]
    .into_iter()
    .zip(extensions)
    .map(|((x, y), e)| m.mul_p([x, y, HUB_Z - SPRING_LENGTH * e - TIRE_RADIUS]))
    .collect();
    let along = |p: [f32; 3]| p[0] * dir[0] + p[1] * dir[1];
    let across = |p: [f32; 3]| p[1] * dir[0] - p[0] * dir[1];
    let mut head = 0.0f32;
    for h in game.scene().track_heads(buggy) {
        let far = h
            .iter()
            .copied()
            .fold(h[0], |a, p| if along(p) > along(a) { p } else { a });
        let best = contacts
            .iter()
            .filter(|c| (across(**c) - across(far)).abs() < 0.3)
            .map(|c| (along(*c) - along(far)).abs())
            .fold(f32::MAX, f32::min);
        head = head.max(best);
    }
    let dust = game
        .scene()
        .dust_spots(buggy)
        .iter()
        .filter(|(_, age)| *age < 16)
        .map(|(p, _)| {
            // Along the way (the dust leaves the tire across it, the emitter's offset out to the side).
            contacts
                .iter()
                .filter(|c| (across(**c) - across(*p)).abs() < 0.45)
                .map(|c| (along(*c) - along(*p)).abs())
                .fold(f32::MAX, f32::min)
        })
        .fold(0.0f32, f32::max);
    Some((head, dust))
}

#[test]
fn at_speed_the_tracks_and_dust_start_under_the_wheels_for_the_driver_and_for_another_player() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    // Each client's ghost of the buggy A drives (B's figure stands beside it before it goes).
    let (ga, gb) = (a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap());
    a.key(Key::W, true);
    let mut checked = [0, 0];
    for _ in 0..(8000 / 16) {
        for g in [&mut a, &mut b] {
            g.step(16);
            g.update_scene(0.016);
        }
        let v = server(&a).buggy_velocity().unwrap();
        let speed = horizontal(v);
        if speed < 19.0 {
            continue;
        }
        let dir = [v[0] / speed, v[1] / speed];
        for (k, (who, g, ghost)) in [("driver", &a, ga), ("other", &b, gb)]
            .into_iter()
            .enumerate()
        {
            if let Some((head, dust)) = under_wheels(g, ghost, dir) {
                // A tick's travel at this speed is over 0.6: the head and the dust start under the tire drawn this frame.
                assert!(
                    head < 0.08,
                    "{who}: a track head {head} from its wheel at {speed} m/s"
                );
                assert!(
                    dust < 0.08,
                    "{who}: new dust {dust} from the wheels at {speed} m/s"
                );
                checked[k] += 1;
            }
        }
    }
    a.key(Key::W, false);
    assert!(
        checked[0] > 10 && checked[1] > 10,
        "frames checked {checked:?}"
    );
}

#[test]
fn each_player_gets_a_buggy_both_drive_at_once_and_a_leavers_empty_buggy_goes() {
    let mut a = host();
    let mut b = join(&mut a);
    steps(&mut [&mut a, &mut b], 500);
    let mine = server(&a).client_buggies();
    let (ba, bb) = (mine[0].unwrap(), mine[1].unwrap());
    assert_ne!(ba, bb);
    assert_eq!(server(&a).buggy_count(), 2);
    // Both clients see both buggies.
    let count = |g: &VehicleDemo| {
        g.with_world(|w| w.records.of_class(TgeClass::WheeledVehicle).count())
            .unwrap()
    };
    assert_eq!((count(&a), count(&b)), (2, 2));
    // The first takes the map's buggy, the second turns round to its own behind it.
    get_in(&mut [&mut a, &mut b], 0);
    b.host.mouse_motion(1047.0, 0.0);
    steps(&mut [&mut a, &mut b], 200);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(
        a.driving() && b.driving(),
        "{:?}",
        server(&a).seat_of(server(&a).figures()[1])
    );
    // The second's client follows the buggy it sits in.
    let sits_in = b.with_world(|w| {
        w.records
            .mount_of(w.records.control.object.unwrap())
            .map(|m| m.0)
    });
    assert_eq!(sits_in.flatten(), b.buggy_ghost());
    // Both drive at once; each sees the other's buggy go.
    let from = [ba, bb].map(|id| server(&a).buggy_position_of(id).unwrap());
    a.key(Key::W, true);
    b.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 2000);
    a.key(Key::W, false);
    b.key(Key::W, false);
    let to = [ba, bb].map(|id| server(&a).buggy_position_of(id).unwrap());
    for k in 0..2 {
        let gone = horizontal([to[k][0] - from[k][0], to[k][1] - from[k][1], 0.0]);
        assert!(gone > 4.0, "buggy {k} went {gone}");
    }
    let seen = |g: &VehicleDemo, at: [f32; 3]| {
        g.with_world(|w| {
            w.records.of_class(TgeClass::WheeledVehicle).any(|(i, _)| {
                w.vehicle_render
                    .get(&i)
                    .copied()
                    .or_else(|| w.records.ghosts.get(&i).and_then(|g| g.pose()))
                    .and_then(|p| torque_render_scene::mounts::pose_matrix(&p, None))
                    .is_some_and(|(m, _)| {
                        let c = m.column(3);
                        (c[0] - at[0]).hypot(c[1] - at[1]) < 1.5
                    })
            })
        })
        .unwrap()
    };
    assert!(
        seen(&a, to[1]) && seen(&b, to[0]),
        "each sees the other's buggy where it went"
    );
    // The second leaves from its seat: its buggy goes once nobody is in it (at once here).
    b.key(Key::E, true);
    b.key(Key::E, false);
    steps(&mut [&mut a, &mut b], 600);
    let _ = b.call("disconnect", &[]);
    drop(b);
    steps(&mut [&mut a], 1500);
    assert_eq!(server(&a).buggy_count(), 1);
    assert!(server(&a).buggy_position_of(bb).is_none());
    assert_eq!(count(&a), 1);
}

#[test]
fn the_buggies_are_capped() {
    let mut a = host();
    server_mut(&mut a)
        .0
        .vm_mut()
        .set_global("$World::MaxBuggies", "2");
    let mut b = join(&mut a);
    let mut c = client("Cid");
    let address = a.hosted_address().unwrap();
    c.join(&address.to_string()).unwrap();
    assert!(until(&mut [&mut a, &mut b, &mut c], |g| g[2].in_world()));
    steps(&mut [&mut a, &mut b, &mut c], 500);
    assert_eq!(server(&a).buggy_count(), 2);
    assert_eq!(
        server(&a)
            .client_buggies()
            .iter()
            .filter(|b| b.is_some())
            .count(),
        2
    );
}

fn server_mut(game: &mut VehicleDemo) -> &mut WorldServer {
    game.server.as_mut().expect("the server")
}

/// A name typed in the menu is looked up off the frame (`localhost`, from the system's hosts), then joined.
#[test]
fn a_typed_name_is_looked_up_and_joined() {
    let mut a = host();
    let mut b = client("Bob");
    let port = a.hosted_address().expect("the hosted address").port();
    let typed = format!("AddressEdit.setValue(\"localhost:{port}\");");
    b.call("eval", &[typed.as_str()]).unwrap();
    b.call("joinAddress", &[]).unwrap();
    let said = b.call("eval", &["return MenuStatus.getValue();"]).unwrap();
    assert_eq!(said, "Looking up localhost...");
    assert!(
        until(&mut [&mut a, &mut b], |g| g[1].in_world()),
        "{:#?}",
        a.server_log
    );
}

/// Where `game` draws the vehicle whose ghost is `ghost` this frame (where it runs it, else where its record put it).
fn drawn_at(game: &VehicleDemo, ghost: u32) -> Option<[f32; 3]> {
    game.with_world(|w| {
        w.vehicle_render
            .get(&ghost)
            .copied()
            .or_else(|| w.records.ghosts.get(&ghost).and_then(|g| g.pose()))
            .and_then(|p| torque_render_scene::mounts::pose_matrix(&p, None))
            .map(|(m, _)| m.column(3))
    })
    .flatten()
}

/// Frames of `frame_ms` on both clients, `keys` pressed on `a` at their times, `a` getting out at `out_ms`, until `end_ms`;
/// `each` sees every frame (its time). The most a frame's drawn step of the buggy `a` drives went past what the server's speed
/// covers in the frame, or back along the server's way, on each client, from half a second before `a` got out.
fn drive_and_get_out(
    a: &mut VehicleDemo,
    b: &mut VehicleDemo,
    keys: &[(u32, Key, bool)],
    out_ms: u32,
    end_ms: u32,
    frame_ms: u32,
    mut each: impl FnMut(&mut VehicleDemo, &mut VehicleDemo, u32),
) -> [f32; 2] {
    let (ga, gb) = (a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap());
    let id = server(a).buggy().unwrap();
    let mut last: [Option<[f32; 3]>; 2] = [None, None];
    let mut worst = [0.0f32; 2];
    let mut t = 0;
    while t < end_ms {
        let now = t..t + frame_ms;
        for (at, k, down) in keys {
            if now.contains(at) {
                a.key(*k, *down);
            }
        }
        if now.contains(&out_ms) {
            press(a, Key::E);
        }
        for g in [&mut *a, &mut *b] {
            g.step(frame_ms);
            g.update_scene(frame_ms as f32 / 1000.0);
        }
        t += frame_ms;
        each(a, b, t);
        let v = server(a).buggy_velocity_of(id).unwrap();
        let expect = apart(v, [0.0; 3]) * frame_ms as f32 / 1000.0;
        let drawn = [drawn_at(a, ga), drawn_at(b, gb)];
        for k in 0..2 {
            if let (Some(p), Some(q)) = (last[k], drawn[k])
                && t + 500 > out_ms
            {
                // Ahead of the server's way, or back along it.
                let step = [0, 1, 2].map(|c| q[c] - p[c]);
                let speed = apart(v, [0.0; 3]).max(1e-3);
                let along = (0..3).map(|c| step[c] * v[c]).sum::<f32>() / speed;
                worst[k] = worst[k].max(apart(p, q) - expect).max(-along);
            }
        }
        last = drawn;
    }
    worst
}

#[test]
fn getting_out_at_speed_or_at_rest_the_buggy_goes_on_smoothly_for_both_players() {
    // At speed after a long drive (more updates than the client keeps for the drive's end), at a high frame rate, and at rest.
    let cases = [
        ("at speed", vec![(0, Key::W, true)], 8000, 16),
        ("at speed, 7 ms frames", vec![(0, Key::W, true)], 4000, 7),
        (
            "at rest",
            vec![
                (0, Key::W, true),
                (2000, Key::W, false),
                (2000, Key::Space, true),
            ],
            5000,
            16,
        ),
    ];
    for (name, keys, out_ms, frame_ms) in cases {
        let mut a = host();
        let mut b = join(&mut a);
        get_in(&mut [&mut a, &mut b], 0);
        let ga = a.buggy_ghost().unwrap();
        let id = server(&a).buggy().unwrap();
        let worst = drive_and_get_out(
            &mut a,
            &mut b,
            &keys,
            out_ms,
            out_ms + 2500,
            frame_ms,
            |_, _, _| {},
        );
        assert!(!a.seated(), "{name}");
        // No frame jumps the buggy ahead or back (one back toward where the drive began was tens of units); it may ease off for
        // a moment while its driver's client goes from its own prediction to the server's updates.
        assert!(worst[0] < 0.1 && worst[1] < 0.1, "{name}: {worst:?}");
        let at = server(&a).buggy_position_of(id).unwrap();
        let drawn = drawn_at(&a, ga).unwrap();
        assert!(apart(at, drawn) < 2.5, "{name}: drawn {drawn:?}, at {at:?}");
    }
}

/// The breaks in `lines` (a strip's segments, oldest first, from `from` on): where one does not start at the last one's end.
fn breaks(lines: &[[[f32; 3]; 2]], from: usize) -> usize {
    lines
        .windows(2)
        .skip(from.saturating_sub(1))
        .filter(|w| apart(w[0][1], w[1][0]) > 1e-3)
        .count()
}

#[test]
fn another_players_buggy_and_one_rolling_on_without_a_driver_lay_unbroken_tracks() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    let (ga, gb) = (a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap());
    let id = server(&a).buggy().unwrap();
    // B watches A drive round at full speed (on the flat round the start), then A gets out and the buggy rolls on.
    let counts = |g: &VehicleDemo, ghost: u32| -> Vec<usize> {
        g.scene()
            .track_lines(ghost)
            .values()
            .map(Vec::len)
            .collect()
    };
    let mut from: Option<[Vec<usize>; 2]> = None;
    let mut fastest = 0.0f32;
    drive_and_get_out(
        &mut a,
        &mut b,
        &[
            (0, Key::W, true),
            (1200, Key::A, true),
            (5000, Key::W, false),
            (5000, Key::A, false),
        ],
        5000,
        6500,
        16,
        |a, b, t| {
            let speed = horizontal(server(a).buggy_velocity_of(id).unwrap());
            if t > 2000 && from.is_none() && speed > 15.0 {
                from = Some([counts(a, ga), counts(b, gb)]);
            }
            fastest = fastest.max(speed);
        },
    );
    let from = from.expect("the buggy got up to speed");
    assert!(fastest > 18.0, "{fastest}");
    let spacing = torque_example_vehicle_demo::scene::track_rules().spacing;
    for (who, g, ghost, k) in [("driver", &a, ga, 0), ("other", &b, gb, 1)] {
        let lines = g.scene().track_lines(ghost);
        assert_eq!(lines.len(), 4, "{who}: four strips");
        for (w, l) in &lines {
            let start = from[k].get(*w).copied().unwrap_or(0);
            assert!(
                l.len() > start + 100,
                "{who} wheel {w}: {} laid",
                l.len() - start
            );
            assert_eq!(breaks(l, start), 0, "{who} wheel {w}: breaks in its strip");
            let longest = l[start..]
                .iter()
                .map(|s| apart(s[0], s[1]))
                .fold(0.0, f32::max);
            assert!(
                longest < spacing + 0.05,
                "{who} wheel {w}: a segment {longest} long"
            );
        }
    }
}

/// Runs `code` on `game`'s server, its value.
fn eval(game: &mut VehicleDemo, code: &str) -> String {
    let out = server_mut(game).0.vm_mut().eval(code.as_bytes());
    String::from_utf8_lossy(&out.expect("the script")).into_owned()
}

/// Puts each client's own buggy at its place ("x y yaw") on the ground, stopped, and seats the client at its wheel.
fn place_and_seat(games: &mut [&mut VehicleDemo], places: &[(f32, f32, f32)]) {
    for (k, (x, y, yaw)) in places.iter().enumerate() {
        eval(
            games[0],
            &format!(
                "%c = ClientGroup.getObject({k}); %v = %c.buggy; \
                 %v.setTransform(\"{x} {y}\" SPC groundAt({x}, {y}) + 1.2 SPC \"0 0 1 {yaw}\"); \
                 %v.setVelocity(\"0 0 0\"); \
                 %p = %c.player; %v.mountObject(%p, 0); %p.setTransform(\"0 0 0 0 0 1 0\"); \
                 %p.setActionThread(\"sit\", true, true); straightenWheels(%v); %p.setControlObject(%v); \
                 commandToClient(%c, 'Seated', 0);"
            ),
        );
    }
    steps(games, 1500);
}

#[test]
fn buggies_driven_into_each_other_head_on_and_side_on_both_stay_where_the_server_has_them() {
    for (name, places) in [
        (
            "head-on",
            [
                (0.0f32, -30.0f32, 0.0f32),
                (0.0, 30.0, std::f32::consts::PI),
            ],
        ),
        (
            "side-on",
            [(0.0, -30.0, 0.0), (-30.0, 0.0, std::f32::consts::FRAC_PI_2)],
        ),
    ] {
        let mut a = host();
        let mut b = join(&mut a);
        place_and_seat(&mut [&mut a, &mut b], &places);
        assert!(a.driving() && b.driving(), "{name}");
        let ids: Vec<u32> = server(&a)
            .client_buggies()
            .into_iter()
            .map(|b| b.unwrap())
            .collect();
        a.key(Key::W, true);
        b.key(Key::W, true);
        let mut met = 0.0f32;
        for f in 0..(6000 / 16) {
            for g in [&mut a, &mut b] {
                g.step(16);
                g.update_scene(0.016);
            }
            let at: Vec<[f32; 3]> = ids
                .iter()
                .map(|&i| server(&a).buggy_position_of(i).expect("both buggies"))
                .collect();
            met = met.max(-apart(at[0], at[1]));
            // Each client draws both buggies near where the server has them (one read against the wrong compression point
            // was drawn some twenty units down, under the ground).
            for (who, g) in [("A", &a), ("B", &b)] {
                let drawn: Vec<[f32; 3]> = g
                    .with_world(|w| {
                        w.records
                            .of_class(TgeClass::WheeledVehicle)
                            .filter_map(|(i, _)| drawn_at(g, i))
                            .collect()
                    })
                    .unwrap();
                assert_eq!(drawn.len(), 2, "{name}: {who} draws {drawn:?} at frame {f}");
                for p in &at {
                    let near = drawn.iter().map(|d| apart(*d, *p)).fold(f32::MAX, f32::min);
                    assert!(
                        near < 4.0,
                        "{name}: {who} draws {drawn:?}, the server has {at:?} at frame {f}"
                    );
                }
            }
        }
        // They met, and both are on the map, upright enough to drive on.
        let at: Vec<[f32; 3]> = ids
            .iter()
            .map(|&i| server(&a).buggy_position_of(i).unwrap())
            .collect();
        assert!(met > -4.0, "{name}: the buggies never met");
        for p in at {
            assert!(p.iter().all(|c| c.is_finite()), "{name}: {p:?}");
            let ground = a.terrain.height(p[0], p[1]).expect("over the terrain");
            assert!((p[2] - ground).abs() < 3.0, "{name}: {p:?} over {ground}");
        }
        assert_eq!(server(&a).buggy_count(), 2, "{name}");
    }
}

fn words(s: &str) -> Vec<f32> {
    s.split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect()
}

/// Buggy `id` is back at its place on `game`'s server: within half a unit of its home across, upright, still, its headlight off,
/// nobody in it.
fn back_home(game: &mut VehicleDemo, id: u32) {
    let home = words(&eval(game, &format!("return {id}.home;")));
    let s = server(game);
    let at = s.buggy_position_of(id).unwrap();
    assert!(
        (at[0] - home[0]).hypot(at[1] - home[1]) < 0.5,
        "{at:?}, home {home:?}"
    );
    let st = torque::game::sim::vehicle::state(s.vm(), id).unwrap();
    assert!(st.obj_to_world[10] > 0.95, "upright: {:?}", st.obj_to_world);
    assert!(
        apart(st.rigid.lin_vel, [0.0; 3]) < 0.5,
        "{:?}",
        st.rigid.lin_vel
    );
    for f in s.figures() {
        assert_ne!(
            s.seat_of(f).map(|x| x.0),
            Some(id),
            "figure {f} still in it"
        );
    }
    assert_eq!(
        eval(game, &format!("return {id}.headlight.isEnabled;")),
        "0"
    );
}

/// Each figure on `game`'s server stands on the ground within `reach` of buggy `id`, and each client is on foot.
fn beside(games: &mut [&mut VehicleDemo], id: u32, reach: f32) {
    let s = server(games[0]);
    let at = s.buggy_position_of(id).unwrap();
    for f in s.figures() {
        let p = s.figure_position_of(f).unwrap();
        assert!(
            (p[0] - at[0]).hypot(p[1] - at[1]) < reach,
            "figure {f} at {p:?}, the buggy at {at:?}"
        );
        assert!(s.seat_of(f).is_none());
    }
    for g in games.iter() {
        assert!(!g.seated());
    }
}

#[test]
fn r_puts_the_players_buggy_back_and_the_player_beside_it_with_anyone_in_it_out() {
    let mut a = host();
    let mut b = join(&mut a);
    // A drives the map's buggy (its own) with B beside, the headlight on.
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(a.driving() && b.seated() && !b.driving());
    let id = server(&a).buggy().unwrap();
    press(&mut a, Key::F);
    a.key(Key::W, true);
    a.key(Key::A, true);
    steps(&mut [&mut a, &mut b], 2500);
    a.key(Key::W, false);
    a.key(Key::A, false);
    // R at the wheel.
    press(&mut a, Key::R);
    steps(&mut [&mut a, &mut b], 1500);
    back_home(&mut a, id);
    beside(&mut [&mut a, &mut b], id, 3.5);
    // A drives off again (turned right, to the buggy) and gets out away from its place; R on foot brings the buggy back.
    a.host.mouse_motion(524.0, 0.0);
    get_in(&mut [&mut a, &mut b], 0);
    assert!(a.driving(), "in again");
    a.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 2000);
    a.key(Key::W, false);
    press(&mut a, Key::E);
    steps(&mut [&mut a, &mut b], 800);
    assert!(horizontal(server(&a).buggy_position().unwrap()) > 8.0);
    press(&mut a, Key::R);
    steps(&mut [&mut a, &mut b], 1500);
    back_home(&mut a, id);
    let figure = server(&a).figure_position().unwrap();
    let at = server(&a).buggy_position().unwrap();
    assert!(
        (figure[0] - at[0]).hypot(figure[1] - at[1]) < 3.5,
        "{figure:?}"
    );
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

#[test]
fn a_buggy_that_falls_off_the_map_comes_back_with_its_riders_beside_it() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(a.driving() && b.seated());
    let id = server(&a).buggy().unwrap();
    // Dropped below the ground, falling.
    eval(
        &mut a,
        &format!("{id}.setTransform(\"40 40 -60 0 0 1 0\");"),
    );
    for _ in 0..2000 / 16 {
        for g in [&mut a, &mut b] {
            g.step(16);
            g.update_scene(0.016);
        }
    }
    back_home(&mut a, id);
    beside(&mut [&mut a, &mut b], id, 3.5);
    // Each camera follows its own figure on foot again (not the buggy), where the server has it.
    let s = server(&a);
    let figures: Vec<[f32; 3]> = s
        .figures()
        .iter()
        .map(|&f| s.figure_position_of(f).unwrap())
        .collect();
    for (who, g, k) in [("A", &mut a, 0), ("B", &mut b, 1)] {
        let own = g.figure().expect("the own figure").position;
        assert!(
            apart(own, figures[k]) < 0.5,
            "{who}: drawn at {own:?}, at {:?}",
            figures[k]
        );
        let eye = g.eye(0.016).position;
        assert!(
            apart(eye, own) < 9.0,
            "{who}: the eye at {eye:?}, the figure at {own:?}"
        );
    }
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

/// The angle (radians) between two of `game`'s sun directions.
fn turned_by(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = (0..3).map(|k| a[k] * b[k]).sum::<f32>() / (apart(a, [0.0; 3]) * apart(b, [0.0; 3]));
    d.clamp(-1.0, 1.0).acos()
}

/// How far the sun turns on each client's sky over `ms`.
fn sun_turn(a: &mut VehicleDemo, b: &mut VehicleDemo, ms: u32) -> [f32; 2] {
    frames_both(a, b, 64);
    let from = [a.sun().unwrap(), b.sun().unwrap()];
    frames_both(a, b, ms);
    [
        turned_by(from[0], a.sun().unwrap()),
        turned_by(from[1], b.sun().unwrap()),
    ]
}

#[test]
fn the_hosts_brackets_run_the_day_faster_slower_or_stop_it_and_others_may_not() {
    let mut a = host();
    let mut b = join(&mut a);
    let normal = sun_turn(&mut a, &mut b, 1000);
    let pace = |g: &mut VehicleDemo| eval(g, "return $World::DaySpeed;");
    // ] three times: 64 times as fast, on both clients' skies; the host's hint says so.
    for _ in 0..3 {
        press(&mut a, Key::BracketRight);
        steps(&mut [&mut a, &mut b], 100);
    }
    assert_eq!(pace(&mut a), "64");
    assert!(
        a.hint_text().starts_with("Time ") && a.hint_text().ends_with(", 64x"),
        "{}",
        a.hint_text()
    );
    let fast = sun_turn(&mut a, &mut b, 1000);
    for k in 0..2 {
        assert!(
            fast[k] > 20.0 * normal[k].max(1e-4),
            "client {k}: {fast:?} against {normal:?}"
        );
    }
    // Another player's key changes nothing, and it is told why.
    press(&mut b, Key::BracketLeft);
    steps(&mut [&mut a, &mut b], 200);
    assert_eq!(pace(&mut a), "64");
    assert_eq!(b.hint_text(), "Only the host can change the time");
    // \ back to its own pace, [ from there stops it.
    press(&mut a, Key::Backslash);
    steps(&mut [&mut a, &mut b], 200);
    assert_eq!(pace(&mut a), "1");
    press(&mut a, Key::BracketLeft);
    steps(&mut [&mut a, &mut b], 200);
    assert_eq!(pace(&mut a), "0");
    assert!(a.hint_text().ends_with(", stopped"), "{}", a.hint_text());
    let stopped = sun_turn(&mut a, &mut b, 1000);
    assert!(stopped[0] < 1e-4 && stopped[1] < 1e-4, "{stopped:?}");
    // The note goes after a moment; it works in a seat too.
    steps(&mut [&mut a, &mut b], 3000);
    assert!(!a.hint_text().starts_with("Time "), "{}", a.hint_text());
    get_in(&mut [&mut a, &mut b], 0);
    press(&mut a, Key::BracketRight);
    steps(&mut [&mut a, &mut b], 200);
    assert_eq!(pace(&mut a), "1");
}

/// The wheel turned `notches` (positive away from the user) on `game`'s window.
fn wheel(game: &mut VehicleDemo, notches: i32) {
    let one = if notches > 0 { 120i32 } else { -120 };
    for _ in 0..notches.abs() {
        game.host.post(
            torque::client::input::WM_MOUSEWHEEL,
            ((one as u32) & 0xffff) << 16,
            0,
        );
    }
}

#[test]
fn the_wheel_zooms_the_camera_on_foot_and_behind_the_buggy_each_its_own_within_limits() {
    use torque_example_vehicle_demo::client::{
        CAMERA_DISTANCE, DRIVE_DISTANCE, DRIVE_ZOOM, FOOT_ZOOM,
    };
    let mut game = host();
    let near = |a: f32, b: f32| (a - b).abs() < 0.02;
    // On foot: three notches in, eased there; the eye that far from the point it circles.
    wheel(&mut game, 3);
    frames(&mut game, 100);
    let mid = game.camera_distance();
    assert!(
        mid < CAMERA_DISTANCE && mid > CAMERA_DISTANCE * 0.85f32.powi(3),
        "{mid}"
    );
    frames(&mut game, 1000);
    let d = game.camera_distance();
    assert!(near(d, CAMERA_DISTANCE * 0.85f32.powi(3)), "{d}");
    let f = game.figure().unwrap();
    let target = [f.position[0], f.position[1], f.position[2] + 1.7];
    let eye = game.eye(0.016).position;
    assert!(
        (apart(eye, target) - d).abs() < 0.3,
        "{eye:?} from {target:?}"
    );
    // Far out: held at the limit.
    wheel(&mut game, -40);
    frames(&mut game, 1500);
    assert!(near(game.camera_distance(), FOOT_ZOOM.1));
    // In a seat: the camera behind the buggy has its own, the one on foot kept.
    get_in(&mut [&mut game], 0);
    assert!(near(game.camera_distance(), DRIVE_DISTANCE));
    wheel(&mut game, 40);
    frames(&mut game, 1500);
    assert!(near(game.camera_distance(), DRIVE_ZOOM.0));
    // In the first person the wheel does nothing.
    tab(&mut game);
    wheel(&mut game, -5);
    frames(&mut game, 1000);
    tab(&mut game);
    assert!(near(game.camera_distance(), DRIVE_ZOOM.0));
    press(&mut game, Key::E);
    frames(&mut game, 800);
    assert!(near(game.camera_distance(), FOOT_ZOOM.1));
}

#[test]
fn a_server_that_sends_a_buggy_at_no_number_puts_nothing_anywhere_on_a_client() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    a.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 1000);
    // The server's buggy at a position and a turn that are no numbers, sent to both (B runs it as a ghost; A drives it).
    let id = server(&a).buggy().unwrap();
    {
        let vm = server_mut(&mut a).0.vm_mut();
        let s = torque::game::sim::vehicle::state_mut(vm, id).unwrap();
        s.rigid.lin_pos = [f32::NAN; 3];
        s.rigid.ang_pos = [f32::NAN; 4];
        s.rigid.lin_mom = [f32::INFINITY; 3];
        s.set_mask(torque::vehicle::state::net_mask::POSE);
    }
    for _ in 0..1500 / 16 {
        for g in [&mut a, &mut b] {
            g.step(16);
            g.update_scene(0.016);
        }
        for (who, g) in [("A", &a), ("B", &b)] {
            let st = g.client.host.client.borrow();
            let Some(conn) = st.conn.conn.as_ref() else {
                continue;
            };
            let finite = |v: &[f32]| v.iter().all(|x| x.is_finite());
            for (i, p) in &conn.world.vehicle_render {
                let (m, _) = torque_render_scene::mounts::pose_matrix(p, None).unwrap();
                assert!(finite(&m.column(3)), "{who}: vehicle {i} drawn at {p:?}");
            }
            for (i, look) in &conn.world.vehicle_look {
                for w in &look.wheels {
                    assert!(
                        finite(&w.point),
                        "{who}: vehicle {i}'s wheel at {:?}",
                        w.point
                    );
                }
            }
            for (i, v) in &conn.predict.ghost_vehicles {
                let s = &v.body.v.s;
                assert!(
                    finite(&s.obj_to_world) && finite(&s.delta_pos) && finite(&s.rigid.lin_mom),
                    "{who}: ghost vehicle {i} at {:?}",
                    s.obj_to_world
                );
            }
        }
    }
}

/// The map places every square of the forest once, each at the origin (the interiors are built in the world's coordinates).
#[test]
fn the_map_places_every_square_of_the_forest_once() {
    use torque_render_scene::Assets as _;
    let files = GameFiles::bundled().unwrap();
    let map = String::from_utf8_lossy(
        files
            .file(torque_example_vehicle_demo::files::MISSION)
            .unwrap(),
    )
    .into_owned();
    let placed: Vec<&str> = map
        .lines()
        .filter(|l| l.contains("InteriorInstance(Forest"))
        .map(|l| {
            assert!(
                l.contains("position = \"0 0 0\"; rotation = \"1 0 0 0\"; scale = \"1 1 1\";"),
                "{l}"
            );
            let at = l.find("kit/").unwrap() + 4;
            &l[at..at + l[at..].find('"').unwrap()]
        })
        .collect();
    let squares: Vec<&str> = torque_example_assets::trees::chunks()
        .iter()
        .map(|c| c.file.as_str())
        .collect();
    assert_eq!(placed, squares);
    for name in squares {
        assert!(files.kit.read(&format!("kit/{name}")).is_some(), "{name}");
    }
}

/// `ms` of frames of one client (its clock, then its scene), and the eye of the last.
fn frames_eye(g: &mut VehicleDemo, ms: u32) -> torque_render_scene::source::Eye {
    let mut eye = None;
    for _ in 0..ms / 16 {
        g.step(16);
        g.update_scene(0.016);
        eye = Some(g.drawn_eye(0.016));
    }
    eye.expect("a frame")
}

/// The difference of two yaws, the short way round.
fn yaw_apart(a: f32, b: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    let d = a - b;
    d - tau * (d / tau).round()
}

#[test]
fn the_seated_look_stops_at_its_edge_and_turns_back_at_once_and_tab_brings_it_into_range() {
    use torque_example_vehicle_demo::client::{HEAD_TURN, MOUSE_PITCH, MOUSE_YAW};
    let mut a = host();
    get_in(&mut [&mut a], 0);
    assert!(a.driving());
    // At the eyes: the mouse pushed 2 radians past the head's turn, then back a tenth of a radian.
    a.set_first_person(true);
    let rest = frames_eye(&mut a, 200).yaw;
    a.host.mouse_motion((HEAD_TURN + 2.0) / MOUSE_YAW, 0.0);
    let edge = frames_eye(&mut a, 200).yaw;
    assert!((a.number("$Drive::OrbitYaw") - HEAD_TURN).abs() < 1e-4);
    assert!(
        (yaw_apart(edge, rest).abs() - HEAD_TURN).abs() < 0.02,
        "{rest} {edge}"
    );
    a.host.mouse_motion(-0.1 / MOUSE_YAW, 0.0);
    let back = frames_eye(&mut a, 32).yaw;
    assert!(
        (yaw_apart(edge, back).abs() - 0.1).abs() < 0.01,
        "the view turned back {} of 0.1",
        yaw_apart(edge, back)
    );
    // The same for the tilt, both ways.
    for dir in [1.0f32, -1.0] {
        a.host.mouse_motion(0.0, dir * 3.0 / MOUSE_PITCH);
        let tilted = frames_eye(&mut a, 100).pitch;
        a.host.mouse_motion(0.0, -dir * 0.1 / MOUSE_PITCH);
        let eased = frames_eye(&mut a, 32).pitch;
        assert!(
            ((tilted - eased).abs() - 0.1).abs() < 0.02,
            "tilt {dir}: {tilted} then {eased}"
        );
    }
    // Behind the buggy, looking back (a half turn and more); Tab: the head turned as far as it goes, and the mouse moves it
    // at once.
    a.set_first_person(false);
    a.set_global("$Drive::OrbitPitch", "0");
    frames_eye(&mut a, 100);
    a.host
        .mouse_motion((std::f32::consts::PI + 7.0) / MOUSE_YAW, 0.0);
    frames_eye(&mut a, 100);
    let behind = a.number("$Drive::OrbitYaw");
    assert!(behind.abs() <= std::f32::consts::PI + 1e-4, "{behind}");
    a.key(Key::Tab, true);
    a.key(Key::Tab, false);
    let at_eyes = frames_eye(&mut a, 600).yaw;
    let yaw = a.number("$Drive::OrbitYaw");
    assert!(yaw.abs() <= HEAD_TURN + 1e-4, "{yaw}");
    a.host.mouse_motion(-yaw.signum() * 0.1 / MOUSE_YAW, 0.0);
    let moved = frames_eye(&mut a, 32).yaw;
    assert!(
        (yaw_apart(at_eyes, moved).abs() - 0.1).abs() < 0.01,
        "after Tab the view turned {} of 0.1",
        yaw_apart(at_eyes, moved)
    );
    // And back behind the buggy: the pitch within the camera's range there.
    a.host.mouse_motion(0.0, -3.0 / MOUSE_PITCH);
    frames_eye(&mut a, 100);
    a.key(Key::Tab, true);
    a.key(Key::Tab, false);
    frames_eye(&mut a, 100);
    let [_, (low, high)] = a.look_range();
    let pitch = a.number("$Drive::OrbitPitch");
    assert!((low..=high).contains(&pitch), "{pitch} in {low}..{high}");
}

/// A click on the GUI control `name` (its middle), the canvas drawn first so its place is known.
fn press_button(game: &mut VehicleDemo, name: &str) {
    game.host.draw_frame();
    let [x, y, w, h] = game.control_rect(name).expect(name);
    let at = (x + w / 2, y + h / 2);
    game.host.mouse_move(at);
    game.host.mouse_button(MouseButton::Left, true, at);
    game.host.mouse_button(MouseButton::Left, false, at);
}

#[test]
fn escape_opens_the_pause_menu_at_the_wheel_and_resume_or_escape_gives_the_keys_and_mouse_back() {
    let mut a = host();
    get_in(&mut [&mut a], 0);
    assert!(a.driving());
    a.host.draw_frame();
    assert!(a.host.wants_mouse(), "the view holds the mouse");
    for close in ["ResumeButton", "Escape"] {
        press(&mut a, Key::Escape);
        steps(&mut [&mut a], 32);
        a.host.draw_frame();
        assert_eq!(a.global("$Client::Paused"), "1", "Esc pauses");
        assert!(!a.host.wants_mouse(), "the mouse is free over the menu");
        assert!(a.host.system_cursor(), "the cursor is on");
        // The keys are off the buggy: W does not drive, the mouse turns nothing.
        a.key(Key::W, true);
        assert_eq!(
            a.global("$Drive::Throttle"),
            "0",
            "W does not reach the wheel"
        );
        a.key(Key::W, false);
        let yaw = a.global("$Drive::OrbitYaw");
        a.host.mouse_motion(50.0, 0.0);
        assert_eq!(a.global("$Drive::OrbitYaw"), yaw);
        if close == "Escape" {
            press(&mut a, Key::Escape);
        } else {
            press_button(&mut a, close);
        }
        steps(&mut [&mut a], 32);
        a.host.draw_frame();
        assert_eq!(a.global("$Client::Paused"), "0", "{close} goes back");
        assert!(
            a.host.wants_mouse(),
            "{close}: the view holds the mouse again"
        );
        assert!(a.driving(), "still at the wheel");
        a.key(Key::W, true);
        assert_eq!(a.global("$Drive::Throttle"), "1", "{close}: W drives again");
        a.key(Key::W, false);
    }
    // Leave game: the menu, the server stopped.
    press(&mut a, Key::Escape);
    press_button(&mut a, "LeaveButton");
    steps(&mut [&mut a], 100);
    assert_eq!(a.global("$Client::Paused"), "0");
    assert!(!a.in_world() && a.server.is_none());
    assert_eq!(
        a.call("eval", &["return Canvas.getContent().getName();"])
            .unwrap(),
        "MenuGui"
    );
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

/// The front of the strip wheel `wheel` of `buggy` lays on `game`: its head's end, or its newest segment's when it has no head.
fn track_front(game: &VehicleDemo, buggy: u32, wheel: usize) -> Option<[f32; 3]> {
    let t = game.scene().tracks()?;
    t.heads()
        .find(|(k, _)| **k == (buggy, wheel))
        .map(|(_, s)| s.line[1])
        .or_else(|| {
            t.segments()
                .filter(|s| s.wheel == (buggy, wheel))
                .last()
                .map(|s| s.line[1])
        })
}

#[test]
fn at_speed_the_track_runs_on_under_the_wheel_evenly_at_144_and_59_frames_a_second() {
    for frame_ms in [7u32, 17] {
        let mut a = host();
        let mut b = join(&mut a);
        get_in(&mut [&mut a, &mut b], 0);
        let (ga, gb) = (a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap());
        a.key(Key::W, true);
        // Each client's last front and how far it went the frame before; the most a frame's move differed from the last's, and
        // how many frames differed by more than a few mm (the springs' own motion over the hill's bumps, which grows with the
        // frame's time squared: 3 mm at 144 frames a second, 6 mm at 59).
        let scale = (frame_ms as f32 / 7.0).powi(2).min(2.0);
        let mut last: [Option<([f32; 3], Option<f32>)>; 2] = [None, None];
        let mut worst = [0.0f32; 2];
        let mut over = [0; 2];
        let mut checked = [0; 2];
        for _ in 0..(6000 / frame_ms) {
            for g in [&mut a, &mut b] {
                g.step(frame_ms);
                g.update_scene(frame_ms as f32 / 1000.0);
            }
            let v = server(&a).buggy_velocity().unwrap();
            let speed = horizontal(v);
            if speed < 19.0 {
                continue;
            }
            let dir = [v[0] / speed, v[1] / speed];
            let along = |p: [f32; 3]| p[0] * dir[0] + p[1] * dir[1];
            for (k, (g, ghost)) in [(&a, ga), (&b, gb)].into_iter().enumerate() {
                // A wheel off the ground (a bump at speed) ends its strip: the measure starts again once it is down.
                let down = g
                    .with_world(|w| w.vehicle_look.get(&ghost)?.wheels.get(2).map(|w| w.contact))
                    .flatten()
                    .unwrap_or(false);
                let Some(front) = track_front(g, ghost, 2).filter(|_| down) else {
                    last[k] = None;
                    continue;
                };
                if let Some((before, moved)) = last[k] {
                    let m = along(front) - along(before);
                    assert!(
                        m >= 0.0,
                        "{frame_ms} ms, client {k}: the track's front went back {m}"
                    );
                    if let Some(prev) = moved {
                        worst[k] = worst[k].max((m - prev).abs());
                        over[k] += usize::from((m - prev).abs() > 0.003 * scale);
                        checked[k] += 1;
                    }
                    last[k] = Some((front, Some(m)));
                } else {
                    last[k] = Some((front, None));
                }
            }
        }
        a.key(Key::W, false);
        assert!(checked[0] > 50 && checked[1] > 50, "{checked:?}");
        // A frame drawn at the tick's springs, or a tick's place between frames off its time, moved it by a share of a frame's
        // travel (13 cm at 144 frames a second, 4 cm at 59).
        let most = 0.01 * scale;
        assert!(
            worst[0] < most && worst[1] < most,
            "{frame_ms} ms: a frame's move of the track's front changed by {worst:?}"
        );
        assert!(
            over[0] * 20 <= checked[0] && over[1] * 20 <= checked[1],
            "{frame_ms} ms: {over:?} of {checked:?} frames changed by more than {} mm",
            3.0 * scale
        );
    }
}

#[test]
fn at_speed_another_players_buggy_is_drawn_where_the_server_has_it() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    let (ga, gb) = (a.buggy_ghost().unwrap(), b.buggy_ghost().unwrap());
    a.key(Key::W, true);
    // How far behind the server's buggy each client draws it, along its way (m): the driver's (predicted) and the other's.
    let mut lags = [Vec::new(), Vec::new()];
    for _ in 0..(7000 / 16) {
        for g in [&mut a, &mut b] {
            g.step(16);
            g.update_scene(0.016);
        }
        let v = server(&a).buggy_velocity().unwrap();
        let speed = horizontal(v);
        if speed < 15.0 {
            continue;
        }
        let dir = [v[0] / speed, v[1] / speed];
        let truth = server(&a).buggy_position().unwrap();
        for (k, (g, ghost)) in [(&a, ga), (&b, gb)].into_iter().enumerate() {
            let drawn = g
                .with_world(|w| match w.vehicle_render.get(&ghost)? {
                    torque::ghost_world::world::Pose::Quat { position, .. } => Some(*position),
                    _ => None,
                })
                .flatten();
            if let Some(p) = drawn {
                lags[k].push((truth[0] - p[0]) * dir[0] + (truth[1] - p[1]) * dir[1]);
            }
        }
    }
    a.key(Key::W, false);
    for (k, l) in lags.iter().enumerate() {
        assert!(l.len() > 100, "client {k}: {} frames", l.len());
        let mean = l.iter().sum::<f32>() / l.len() as f32;
        let most = l.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        // The other's was drawn its update's age and a warp behind: 1.7 m on average at 15 to 20 m/s, 2 m at most.
        assert!(
            mean.abs() < 0.4 && most < 0.8,
            "client {k}: drawn {mean} m behind on average, {most} m at most"
        );
    }
}

/// Where `game` draws vehicle ghost `index` this frame, its rotation too.
fn drawn_pose(game: &VehicleDemo, index: u32) -> Option<([f32; 3], [f32; 4])> {
    game.with_world(|w| match w.vehicle_render.get(&index)? {
        torque::ghost_world::world::Pose::Quat { position, rotation } => {
            Some((*position, *rotation))
        }
        _ => None,
    })
    .flatten()
}

#[test]
fn hostile_vehicle_updates_never_draw_another_players_buggy_at_nan_and_the_next_update_heals_it() {
    use torque::client::predict::ClientCollision;
    use torque::client::records::EngineRecord as _;
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    let gb = b.buggy_ghost().unwrap();
    a.key(Key::W, true);
    frames_both(&mut a, &mut b, 1500);
    type Hostile = fn(&mut torque::client::records::VehicleUpdate, &mut [f32; 3]);
    let cases: [(&str, Hostile); 6] = [
        ("a far position", |_, p| *p = [1e38; 3]),
        ("an infinite velocity", |r, _| {
            r.pose.as_mut().unwrap().linear = [f32::INFINITY, 0.0, 0.0]
        }),
        ("a NaN velocity", |r, _| {
            r.pose.as_mut().unwrap().linear = [f32::NAN; 3]
        }),
        ("a huge momentum", |r, _| {
            r.pose.as_mut().unwrap().linear = [3e38; 3]
        }),
        ("a huge spin", |r, _| {
            r.pose.as_mut().unwrap().angular = [3e38; 3]
        }),
        ("a rotation of no length", |r, _| {
            r.pose.as_mut().unwrap().rotation = [0.0; 4]
        }),
    ];
    for (name, hostile) in cases {
        // The newest real update of the buggy, made hostile, as the other client's ghost takes it.
        let (mut rec, mut at) = b
            .with_world(|w| {
                w.vehicle_updates
                    .iter()
                    .rev()
                    .filter(|u| u.index == gb)
                    .find_map(|u| Some((u.record.vehicle()?, u.position?)))
            })
            .flatten()
            .expect("an update of the buggy");
        assert!(rec.pose.is_some(), "{name}");
        hostile(&mut rec, &mut at);
        {
            let mut st = b.client.host.client.borrow_mut();
            let c = st.conn.conn.as_mut().expect("the connection");
            let g = c.predict.ghost_vehicles.get_mut(&gb).expect("the ghost");
            g.unpack(&rec, Some(at), true, &mut ClientCollision::empty());
        }
        for _ in 0..30 {
            frames_both(&mut a, &mut b, 16);
            let (p, q) = drawn_pose(&b, gb).expect(name);
            assert!(
                p.iter().chain(&q).all(|c| c.is_finite()),
                "{name}: drawn at {p:?} {q:?}"
            );
        }
        // The server's next updates bring it back to where the buggy is.
        frames_both(&mut a, &mut b, 1500);
        let (p, _) = drawn_pose(&b, gb).unwrap();
        let truth = server(&a).buggy_position().unwrap();
        assert!(
            apart(p, truth) < 3.0,
            "{name}: drawn at {p:?}, the buggy at {truth:?}"
        );
    }
    a.key(Key::W, false);
}

#[test]
fn a_warp_or_first_sight_update_draws_another_players_buggy_at_its_pose_with_no_offset_left() {
    use torque::client::predict::ClientCollision;
    use torque::client::records::EngineRecord as _;
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    let gb = b.buggy_ghost().unwrap();
    a.key(Key::W, true);
    frames_both(&mut a, &mut b, 1500);
    let newest = |b: &VehicleDemo| {
        b.with_world(|w| {
            w.vehicle_updates
                .iter()
                .rev()
                .filter(|u| u.index == gb)
                .find_map(|u| Some((u.record.vehicle()?, u.position?)))
        })
        .flatten()
        .expect("an update of the buggy")
    };
    for (name, warp, added) in [("a warp", true, true), ("first sight", false, false)] {
        // An update 5 units aside leaves an offset to ease out; then the warp (or first sight) at the buggy's own pose.
        let (rec, at) = newest(&b);
        let aside = [at[0] + 5.0, at[1], at[2]];
        let unpack = |b: &mut VehicleDemo, rec: &_, at: [f32; 3], added: bool| {
            let mut st = b.client.host.client.borrow_mut();
            let c = st.conn.conn.as_mut().expect("the connection");
            let g = c.predict.ghost_vehicles.get_mut(&gb).expect("the ghost");
            g.unpack(rec, Some(at), added, &mut ClientCollision::empty());
        };
        unpack(&mut b, &rec, aside, true);
        frames_both(&mut a, &mut b, 32);
        let (mut rec, at) = newest(&b);
        rec.pose.as_mut().unwrap().warp = warp;
        // One tick of its own straight after (no other update between).
        let mut st = b.client.host.client.borrow_mut();
        let c = st.conn.conn.as_mut().unwrap();
        let g = c.predict.ghost_vehicles.get_mut(&gb).unwrap();
        g.unpack(&rec, Some(at), added, &mut ClientCollision::empty());
        g.tick(&mut ClientCollision::empty(), 0);
        // Its drawn pose at its tick is its state run on by its lead, nothing more.
        let s = &g.body.v.s;
        let want: [f32; 3] =
            std::array::from_fn(|i| s.rigid.lin_pos[i] + s.rigid.lin_vel[i] * g.warp.lead);
        assert!(
            apart(s.delta_pos, want) < 1e-3,
            "{name}: drawn at {:?}, its state at {want:?}",
            s.delta_pos
        );
        let turn = (0..4)
            .map(|i| s.delta_rot[1][i] * s.rigid.ang_pos[i])
            .sum::<f32>()
            .abs();
        assert!(turn > 1.0 - 1e-5, "{name}: turned from its state ({turn})");
    }
    a.key(Key::W, false);
}

/// R far from the start puts a new figure beside the buggy: the camera cuts there, at the eyes and behind, standing or
/// crouched, never drawn on the way between nor anywhere else while the new figure is on its way (across the ground: the new
/// figure may go on into its pose there).
#[test]
fn r_far_from_the_start_cuts_the_camera_to_the_new_figure() {
    for (first, crouched) in [(false, false), (true, false), (false, true), (true, true)] {
        let label = format!("first person {first}, crouched {crouched}");
        let mut game = host();
        if first {
            tab(&mut game);
        }
        // 20 m off, dropped onto the ground there.
        eval(
            &mut game,
            "%p = ClientGroup.getObject(0).player; %t = %p.getTransform();
             %p.setTransform(getWord(%t, 0) + 20 SPC getWord(%t, 1) + 20 SPC getWord(%t, 2) + 4 SPC \"0 0 1 0\");",
        );
        for _ in 0..120 {
            drawn(&mut game);
        }
        if crouched {
            game.call("crouchKey", &["1"]).unwrap();
        }
        for _ in 0..60 {
            drawn(&mut game);
        }
        let before = drawn(&mut game).position;
        game.call("restart", &["1"]).unwrap();
        let eyes: Vec<[f32; 3]> = (0..120).map(|_| drawn(&mut game).position).collect();
        let after = *eyes.last().unwrap();
        let across = |p: [f32; 3], q: [f32; 3]| (p[0] - q[0]).hypot(p[1] - q[1]);
        assert!(
            across(before, after) > 8.0,
            "{label}: {before:?} to {after:?}"
        );
        for (i, p) in eyes.iter().enumerate() {
            let gap = across(*p, before).min(across(*p, after));
            assert!(
                gap < 1.0,
                "{label}: frame {i} drew the eye at {p:?}, {gap} m from {before:?} and {after:?}: {:?}",
                &eyes[i.saturating_sub(3)..(i + 10).min(eyes.len())]
            );
        }
    }
}

/// The pose of the first client's figure on the server.
fn server_pose(game: &mut VehicleDemo) -> torque::game::sim::player::poses::Pose {
    let id: u32 = eval(game, "return ClientGroup.getObject(0).player.getId();")
        .parse()
        .expect("the figure");
    let vm = server_mut(game).0.vm_mut();
    torque::game::sim::player::state(vm, id)
        .expect("the figure's state")
        .pose
}

/// The grade of the terrain at `(x, y)`: the most it rises over a unit step.
fn grade(game: &VehicleDemo, x: f32, y: f32) -> Option<f32> {
    let h = game.terrain.height(x, y)?;
    let mut most = 0.0f32;
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        most = most.max((game.terrain.height(x + dx, y + dy)? - h).abs());
    }
    Some(most)
}

#[test]
fn standing_on_a_slope_of_the_terrain_z_lies_the_figure_down_and_z_again_stands_it_up() {
    use torque::game::sim::player::poses::Pose;
    let mut game = host();
    // A slope near the start, steep enough that a wide box at the feet would meet the ground uphill (a rise of 0.1 to 0.35
    // over a unit step: well within the figure's run surface).
    let spot = (0..80)
        .flat_map(|i| (0..80).map(move |j| (i as f32 * 2.0 - 80.0, j as f32 * 2.0 - 80.0)))
        .filter(|&(x, y)| grade(&game, x, y).is_some_and(|g| (0.1..0.35).contains(&g)))
        .min_by(|a, b| (a.0.hypot(a.1)).total_cmp(&b.0.hypot(b.1)))
        .expect("a slope near the start");
    eval(
        &mut game,
        &format!(
            "%p = ClientGroup.getObject(0).player;              %p.setTransform(\"{} {}\" SPC groundAt({}, {}) + 0.1 SPC \"0 0 1 0\"); %p.setVelocity(\"0 0 0\");",
            spot.0, spot.1, spot.0, spot.1
        ),
    );
    steps(&mut [&mut game], 1500);
    let feet = server(&game).figure_position().expect("the figure");
    let ground = game
        .terrain
        .height(feet[0], feet[1])
        .expect("on the terrain");
    assert!(
        (feet[2] - ground).abs() < 0.3,
        "standing at {feet:?} over {ground}"
    );
    assert_eq!(server_pose(&mut game), Pose::Stand);
    press(&mut game, Key::Z);
    steps(&mut [&mut game], 2000);
    assert_eq!(
        server_pose(&mut game),
        Pose::Prone,
        "on the slope at {spot:?}"
    );
    let lying = server(&game).figure_position().expect("the figure");
    let ground = game.terrain.height(lying[0], lying[1]).unwrap();
    assert!(
        (lying[2] - ground).abs() < 0.5,
        "lying at {lying:?} over {ground}"
    );
    press(&mut game, Key::Z);
    steps(&mut [&mut game], 2000);
    assert_eq!(server_pose(&mut game), Pose::Stand);
}

/// A tree's trunk: its planes (each an outward normal and offset, `n . p + d` above 0 outside) and its corners, in the world.
struct Trunk {
    planes: Vec<([f64; 3], f64)>,
    corners: Vec<[f64; 3]>,
}

impl Trunk {
    fn of(t: &torque_example_assets::trees::Tree) -> Trunk {
        use torque_example_assets::trees;
        let foot = t.foot.map(f64::from).into();
        let bark = [trees::BARK, trees::BARK_RED, trees::BARK_BIRCH];
        let brush = trees::tree(t.kind, foot, f64::from(t.size), t.seed)
            .into_iter()
            .find(|b| bark.contains(&b.faces[0].material.as_str()))
            .expect("a trunk");
        Trunk {
            planes: brush
                .faces
                .iter()
                .map(|f| ([f.plane.n.x, f.plane.n.y, f.plane.n.z], f.plane.d))
                .collect(),
            corners: brush
                .corners()
                .expect("a solid trunk")
                .iter()
                .map(|c| [c.x, c.y, c.z])
                .collect(),
        }
    }

    /// How deep `p` is inside the trunk (0 outside).
    fn depth(&self, p: [f64; 3]) -> f64 {
        let out = self
            .planes
            .iter()
            .map(|(n, d)| n[0] * p[0] + n[1] * p[1] + n[2] * p[2] + d)
            .fold(f64::MIN, f64::max);
        (-out).max(0.0)
    }
}

/// How deep the buggy's body box at `m` (its object-to-world matrix) and `trunk` reach into each other: the deepest point of
/// either (the trunk's edges and inside, the box's edges) inside the other. A lower bound on how far apart they must move.
fn body_in_trunk(m: &[f32; 16], trunk: &Trunk) -> f64 {
    use torque_example_assets::buggy::{BODY_MAX, BODY_MIN};
    let m: Vec<f64> = m.iter().map(|&v| f64::from(v)).collect();
    let (lo, hi) = (BODY_MIN.map(f64::from), BODY_MAX.map(f64::from));
    let to_world = |l: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|r| {
            m[r * 4] * l[0] + m[r * 4 + 1] * l[1] + m[r * 4 + 2] * l[2] + m[r * 4 + 3]
        })
    };
    let in_box = |w: [f64; 3]| -> f64 {
        let d = [w[0] - m[3], w[1] - m[7], w[2] - m[11]];
        let l: [f64; 3] = std::array::from_fn(|c| m[c] * d[0] + m[4 + c] * d[1] + m[8 + c] * d[2]);
        (0..3)
            .map(|k| (l[k] - lo[k]).min(hi[k] - l[k]))
            .fold(f64::MAX, f64::min)
            .max(0.0)
    };
    let lerp = |a: [f64; 3], b: [f64; 3], t: f64| -> [f64; 3] {
        std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
    };
    const SAMPLES: usize = 40;
    let mut deepest = 0.0f64;
    let c = &trunk.corners;
    for i in 0..c.len() {
        for j in i + 1..c.len() {
            for s in 0..=SAMPLES {
                deepest = deepest.max(in_box(lerp(c[i], c[j], s as f64 / SAMPLES as f64)));
            }
        }
    }
    let corner = |k: usize| -> [f64; 3] {
        to_world(std::array::from_fn(|a| {
            if k >> a & 1 == 1 { hi[a] } else { lo[a] }
        }))
    };
    for i in 0..8 {
        for a in 0..3 {
            let j = i | 1 << a;
            if j == i {
                continue;
            }
            for s in 0..=SAMPLES {
                deepest =
                    deepest.max(trunk.depth(lerp(corner(i), corner(j), s as f64 / SAMPLES as f64)));
            }
        }
    }
    deepest
}

/// A straight tree near the start with a clear, gentle run of `run` m up to it, and the way in (a unit heading).
fn tree_to_drive_into(
    game: &VehicleDemo,
    run: f32,
) -> (torque_example_assets::trees::Tree, [f32; 2]) {
    use torque_example_assets::trees;
    let all = trees::forest();
    let mut picks: Vec<(f32, trees::Tree, [f32; 2])> = Vec::new();
    for t in all.iter().filter(|t| t.kind.conifer() && t.size >= 0.7) {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let dir = [a.cos(), a.sin()];
            // Nothing else within 5 m of the run, and the ground along it within a meter of a straight line.
            let clear = all.iter().filter(|o| *o != t).all(|o| {
                let rel = [o.foot[0] - t.foot[0], o.foot[1] - t.foot[1]];
                let along = rel[0] * dir[0] + rel[1] * dir[1];
                let across = (rel[0] * dir[1] - rel[1] * dir[0]).abs();
                !(along > -run - 5.0 && along < 6.0 && across < 5.0)
            });
            if !clear {
                continue;
            }
            let ground = |s: f32| {
                game.terrain
                    .height(t.foot[0] + dir[0] * s, t.foot[1] + dir[1] * s)
            };
            let Some(near) = ground(-3.0) else { continue };
            let Some(far) = ground(-run) else { continue };
            let gentle = (0..=20).all(|i| {
                let s = 3.0 + (run - 3.0) * i as f32 / 20.0;
                let line = near + (far - near) * (s - 3.0) / (run - 3.0);
                ground(-s).is_some_and(|h| (h - line).abs() < 1.0)
            }) && (far - near).abs() < run * 0.1;
            if gentle {
                picks.push((t.foot[0].hypot(t.foot[1]), *t, dir));
            }
        }
    }
    picks.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (_, t, dir) = picks.first().copied().expect("a tree to drive into");
    (t, dir)
}

/// Full throttle held into a tree trunk, from a standstill a little short of it and from a 28 m run-up: the buggy's body never
/// sinks into the trunk past a small margin on the server or where the driver's client draws it, and after several seconds
/// pushing it is still on its own side. (TGE's contact responses let a steady push creep into the trunk until the contacts
/// were lost, and the buggy drove through.)
#[test]
fn full_throttle_into_a_tree_from_rest_or_at_speed_never_drives_the_buggy_into_its_trunk() {
    let lookout = host();
    let (tree, dir) = tree_to_drive_into(&lookout, 30.0);
    drop(lookout);
    let trunk = Trunk::of(&tree);
    // The trunk's radius at the foot (its farthest corner from the foot's axis, over the ground).
    let radius = trunk
        .corners
        .iter()
        .filter(|c| c[2] > f64::from(tree.foot[2]) - 2.0)
        .map(|c| (c[0] - f64::from(tree.foot[0])).hypot(c[1] - f64::from(tree.foot[1])))
        .fold(0.0, f64::max) as f32;
    // How far ahead of the foot a pose is along the way in.
    let along = |m: &[f32; 16]| (m[3] - tree.foot[0]) * dir[0] + (m[7] - tree.foot[1]) * dir[1];
    for (name, back) in [("from rest", radius + 2.4), ("at speed", 28.0)] {
        let mut a = host();
        let id = server(&a).buggy().unwrap();
        let start = [tree.foot[0] - dir[0] * back, tree.foot[1] - dir[1] * back];
        place_and_seat(&mut [&mut a], &[(start[0], start[1], dir[0].atan2(dir[1]))]);
        assert!(a.driving(), "{name}");
        let fwd = server(&a).buggy_axis(1).unwrap();
        assert!(
            fwd[0] * dir[0] + fwd[1] * dir[1] > 0.95,
            "{name}: facing {fwd:?}, not {dir:?}"
        );
        a.key(Key::W, true);
        let (mut deepest, mut drawn_deepest, mut late, mut fastest) =
            (0.0f64, 0.0f64, 0.0f64, 0.0f32);
        let mut farthest = f32::MIN;
        let frames = 8000 / 16;
        for f in 0..frames {
            a.step(16);
            a.update_scene(0.016);
            let s = torque::game::sim::vehicle::state(server(&a).vm(), id).unwrap();
            let d = body_in_trunk(&s.obj_to_world, &trunk);
            deepest = deepest.max(d);
            if f >= frames - 2000 / 16 {
                late = late.max(d);
            }
            farthest = farthest.max(along(&s.obj_to_world));
            fastest = fastest.max(horizontal(s.rigid.lin_vel));
            let drawn = a.buggy_pose().expect("the drawn buggy");
            drawn_deepest = drawn_deepest.max(body_in_trunk(&drawn.m, &trunk));
        }
        assert!(deepest < 0.03, "{name}: {deepest} m into the trunk");
        assert!(
            drawn_deepest < 0.03,
            "{name}: drawn {drawn_deepest} m into the trunk"
        );
        assert!(late < 0.02, "{name}: {late} m in over the last 2 s");
        assert!(
            farthest < -radius - 1.0,
            "{name}: reached {farthest} m along, the trunk {radius} m round"
        );
        if name == "at speed" {
            assert!(fastest > 12.0, "only {fastest} m/s");
        }
    }
}

/// The prediction's counts on `game`'s client: (corrections, heads that matched).
fn corrections(game: &VehicleDemo) -> (u64, u64) {
    let st = game.client.host.client.borrow();
    let p = &st.conn.conn.as_ref().expect("the connection").predict;
    (p.stats.corrections, p.stats.matching)
}

#[test]
fn a_drivers_client_predicts_the_buggy_as_the_server_drives_it_never_meeting_the_passenger() {
    let mut a = host();
    let mut b = join(&mut a);
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(a.driving() && b.seat() == Some(1));
    let id = server(&a).buggy().unwrap();
    let from = server(&a).buggy_position().unwrap();
    let (before, _) = corrections(&a);
    // Forward, a turn each way, a brake: each tick the driver's client runs the buggy with the passenger seated in it.
    let keys = [
        (0, Key::W, true),
        (1500, Key::A, true),
        (2500, Key::A, false),
        (2500, Key::D, true),
        (3500, Key::D, false),
        (4500, Key::W, false),
        (4500, Key::Space, true),
    ];
    let mut worst = 0.0f32;
    let mut t = 0;
    while t < 7000 {
        for (at, k, down) in keys {
            if (t..t + 16).contains(&at) {
                a.key(k, down);
            }
        }
        for g in [&mut a, &mut b] {
            g.step(16);
            g.update_scene(0.016);
        }
        t += 16;
        // The driver's prediction runs a tick or two ahead of the server: never further than that at the speed.
        let v = horizontal(server(&a).buggy_velocity_of(id).unwrap());
        let at = server(&a).buggy_position_of(id).unwrap();
        assert!(
            a.driven().is_some(),
            "at {t} ms the driver's client runs no buggy"
        );
        let drawn = drawn_at(&a, a.buggy_ghost().unwrap()).unwrap();
        worst = worst.max(apart(at, drawn) - v * 0.1);
    }
    a.key(Key::Space, false);
    let (after, matching) = corrections(&a);
    let to = server(&a).buggy_position().unwrap();
    assert!(apart(from, to) > 10.0, "from {from:?} to {to:?}");
    assert_eq!(
        after - before,
        0,
        "the driver's client predicted the buggy wrong {} times ({matching} heads matched)",
        after - before
    );
    assert!(worst < 0.25, "drawn {worst} m off the server's buggy");
    assert_eq!(server(&a).seat_of(server(&a).figures()[1]), Some((id, 1)));
}

/// The seated yaw of figure `id` on `host`'s server (its turn on its seat, and its head's pitch and turn), and of the figure
/// `game`'s client predicts.
fn seated_yaws(host: &VehicleDemo, game: &VehicleDemo, id: u32) -> ([f32; 3], Option<f32>) {
    let s = torque::game::sim::player::state(server(host).vm(), id).expect("the figure");
    let st = game.client.host.client.borrow();
    let predicted = st
        .conn
        .conn
        .as_ref()
        .and_then(|c| c.predict.predicted.as_ref())
        .map(|p| p.player.s.rot[2]);
    ([s.rot[2], s.head[0], s.head[2]], predicted)
}

/// Stands client `k`'s figure `reach` from its buggy at `angle` round it (radians from the buggy's front, to its right),
/// facing the buggy's middle.
fn stand_round(games: &mut [&mut VehicleDemo], k: usize, angle: f32, reach: f32) {
    eval(
        games[0],
        &format!(
            "%c = ClientGroup.getObject({k}); %v = $World::Buggy; %xf = %v.getTransform(); \
             %d = MatrixMulVector(%xf, mSin({angle}) * {reach} SPC mCos({angle}) * {reach} SPC 0); \
             %x = getWord(%xf, 0) + getWord(%d, 0); %y = getWord(%xf, 1) + getWord(%d, 1); \
             %yaw = mAtan(-getWord(%d, 0), -getWord(%d, 1)); \
             %c.player.setTransform(%x SPC %y SPC groundAt(%x, %y) + 0.1 SPC \"0 0 1\" SPC %yaw); \
             %c.player.setVelocity(\"0 0 0\");"
        ),
    );
    steps(games, 800);
}

#[test]
fn a_figure_sits_facing_the_seats_way_from_any_side_it_came_in_turning_the_mouse() {
    let mut a = host();
    let mut b = join(&mut a);
    let figures = server(&a).figures();
    // From ahead, beside the seat, behind it and from the back.
    let angles = [0.5f32, 1.3, 1.9, 2.7];
    for (n, &angle) in angles.iter().enumerate() {
        // The driver comes in from the left, the passenger from the right; each turns the mouse as its click lands and after.
        for (k, side) in [(0usize, -1.0f32), (1, 1.0)] {
            let reach = 2.3 + 0.9 * angle.cos().abs();
            stand_round(&mut [&mut a, &mut b], k, side * angle, reach);
            let games = &mut [&mut a, &mut b];
            // The look all the way up, then down at the buggy as `get_in` looks.
            games[k].host.mouse_motion(0.0, -1000.0);
            steps(games, 100);
            games[k].host.mouse_motion(0.0, 1.4 / 0.003 + 110.0);
            steps(games, 300);
            let turn = if n % 2 == 0 { 120.0 } else { -120.0 };
            games[k].host.mouse_motion(turn * 0.25, 0.0);
            click(games[k]);
            for _ in 0..12 {
                games[k].host.mouse_motion(turn, 0.0);
                steps(games, 32);
            }
            steps(games, 400);
        }
        let buggy = server(&a).buggy().unwrap();
        assert_eq!(
            (
                server(&a).seat_of(figures[0]),
                server(&a).seat_of(figures[1])
            ),
            (Some((buggy, 0)), Some((buggy, 1))),
            "from {angle}"
        );
        // A second seated, the mouse still turning (the seated look turns the view, not the figure).
        for _ in 0..1000 / 32 {
            a.host.mouse_motion(150.0, 0.0);
            b.host.mouse_motion(-150.0, 0.0);
            steps(&mut [&mut a, &mut b], 32);
        }
        for (k, game) in [&a, &b].into_iter().enumerate() {
            let (on_server, predicted) = seated_yaws(&a, game, figures[k]);
            assert!(
                on_server.iter().all(|a| yaw_apart(*a, 0.0).abs() < 1e-3),
                "from {angle}, seat {k}: the server's figure sits {on_server:?} rad (turn, head pitch, head turn) off                  the seat's way"
            );
            if let Some(p) = predicted {
                assert!(
                    yaw_apart(p, 0.0).abs() < 1e-3,
                    "from {angle}, seat {k}: the client's figure sits {p} rad off the seat's way"
                );
            }
        }
        for game in [&mut a, &mut b] {
            press(game, Key::E);
        }
        steps(&mut [&mut a, &mut b], 800);
        assert!(!a.seated() && !b.seated(), "from {angle}");
    }
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

/// Figure `id`'s speed across the ground on `game`'s server over the next `ms`, and its velocity at the end.
fn ground_speed(game: &mut VehicleDemo, id: u32, ms: u32) -> (f32, [f32; 3]) {
    let from = server(game).figure_position_of(id).unwrap();
    steps(&mut [&mut *game], ms);
    let to = server(game).figure_position_of(id).unwrap();
    let v = torque::game::sim::player::state(server(game).vm(), id)
        .unwrap()
        .velocity;
    (
        (to[0] - from[0]).hypot(to[1] - from[1]) * 1000.0 / ms as f32,
        v,
    )
}

/// Crouched and prone the figure goes half as fast again as it did (the datablock's speeds),
/// and its crouched walk and its crawl play as fast as it goes: each loop's ground speed times its rate is the figure's speed,
/// so its feet and hands keep pace with the ground.
#[test]
fn crouched_and_prone_the_figure_goes_faster_and_its_feet_keep_pace_with_the_ground() {
    use torque::client::player_anim::{PlayerAnimation, PlayerMotion};
    use torque::game::sim::player::poses::Pose;
    let assets = torque_example_common::KitAssets::new().unwrap();
    let figure = torque::app::view::SceneShape::load(
        &assets,
        &torque_example_common::kit_path("figure.dts"),
    )
    .unwrap();
    for (pose, key, top, sequence) in [
        (Pose::Crouch, "crouchKey", 1.95f32, "crouch_forward"),
        (Pose::Prone, "proneKey", 1.125, "prone_forward"),
    ] {
        let mut game = host();
        let id = server(&game).figure().unwrap();
        // Off on the flat, facing away from the buggy.
        eval(
            &mut game,
            "%p = ClientGroup.getObject(0).player; %t = %p.getTransform();
             %p.setTransform(getWord(%t, 0) - 30 SPC getWord(%t, 1) SPC getWord(%t, 2) + 4 SPC \"0 0 1 -1.5708\");",
        );
        steps(&mut [&mut game], 1500);
        game.call(key, &["1"]).unwrap();
        steps(&mut [&mut game], 1500);
        // The full pace (Shift: the figure walks a share of its speed without it).
        game.key(Key::ShiftLeft, true);
        game.key(Key::W, true);
        steps(&mut [&mut game], 2000);
        let (speed, velocity) = ground_speed(&mut game, id, 1000);
        game.key(Key::W, false);
        assert!(
            (speed - top).abs() < 0.12 * top,
            "{pose:?}: {speed} m/s across the ground, not {top}"
        );
        // The figure's animation on a client at that velocity.
        let shape = figure.shape();
        let mut instance = figure.instance().unwrap();
        let mut anim = PlayerAnimation::new(shape, &mut instance).unwrap();
        let s = torque::game::sim::player::state(server(&game).vm(), id).unwrap();
        let motion = PlayerMotion {
            velocity,
            yaw: s.rot[2],
            pose,
            ..PlayerMotion::default()
        };
        for _ in 0..60 {
            anim.update(shape, &mut instance, &motion, 1.0 / 60.0);
        }
        let (i, forward) = anim.action();
        let action = &anim.actions()[i];
        assert_eq!(
            (String::from_utf8_lossy(&action.name), forward),
            (sequence.into(), true),
            "{pose:?}"
        );
        let rate = instance.thread(anim.action_thread()).unwrap().time_scale;
        let along = motion
            .local_velocity()
            .iter()
            .zip(action.motion.dir)
            .map(|(v, d)| v * d)
            .sum::<f32>();
        let paced = rate * action.motion.speed;
        assert!(
            (paced - along).abs() < 0.02 * along,
            "{pose:?}: the loop covers {paced} m/s at rate {rate}, the figure goes {along} m/s"
        );
    }
}

/// Empty-handed (the arm thread on `look`, as the demo's figures are), the figure gets down to prone and back up by the
/// empty-handed ways, both hands going down to the ground; holding something (`hold`, the arena's pistol) by the armed ways,
/// its right arm reaching ahead.
#[test]
fn an_empty_handed_figure_gets_down_and_up_with_both_hands_and_an_armed_one_keeps_its_arm_ahead() {
    use torque::client::player_anim::{PlayerAnimation, PlayerMotion};
    use torque::game::sim::player::poses::Pose;
    let assets = torque_example_common::KitAssets::new().unwrap();
    let figure = torque::app::view::SceneShape::load(
        &assets,
        &torque_example_common::kit_path("figure.dts"),
    )
    .unwrap();
    let shape = figure.shape();
    for (arm, down, up) in [
        (None, "prone_down", "prone_up"),
        (Some("look"), "prone_down", "prone_up"),
        (Some("hold"), "prone_down_armed", "prone_up_armed"),
    ] {
        let mut instance = figure.instance().unwrap();
        let mut anim = PlayerAnimation::new(shape, &mut instance).unwrap();
        let arm_action = arm.map(|name| {
            anim.actions()
                .iter()
                .position(|a| a.name == name.as_bytes())
                .unwrap() as u32
        });
        let mut played = Vec::new();
        for pose in [Pose::Crouch, Pose::Prone, Pose::Crouch] {
            let motion = PlayerMotion {
                pose,
                arm_action,
                look: 0.5,
                ..PlayerMotion::default()
            };
            for _ in 0..90 {
                anim.update(shape, &mut instance, &motion, 1.0 / 60.0);
                let name =
                    String::from_utf8_lossy(&anim.actions()[anim.action().0].name).into_owned();
                if played.last() != Some(&name) {
                    played.push(name);
                }
            }
        }
        assert_eq!(
            played,
            ["crouch_root", down, "prone_root", up, "crouch_root"],
            "with the arm on {arm:?}"
        );
    }
}

/// Each client's arm actions for figure `id` on the server and for the other player's figure on `other` (its frame drawn).
fn arms(a: &mut VehicleDemo, b: &mut VehicleDemo, id: u32) -> (Option<String>, Option<String>) {
    for g in [&mut *a, &mut *b] {
        g.update_scene(0.016);
    }
    let on_server = server(a).figure_arm_of(id);
    let seen = b.scene().figure_arms().into_iter().find_map(|(_, arm)| arm);
    (on_server, seen)
}

#[test]
fn a_click_on_nothing_points_the_arm_for_everyone_while_held_or_a_second_and_one_on_the_buggy_seats()
 {
    let mut a = host();
    let mut b = join(&mut a);
    let id = server(&a).figures()[0];
    let point = || (Some("point".to_string()), Some("point".to_string()));
    let empty = || (Some("look".to_string()), Some("look".to_string()));
    assert_eq!(arms(&mut a, &mut b, id), empty());
    // Turned away from the buggy, looking a little up: the click meets nothing.
    a.host.mouse_motion(std::f32::consts::PI / 0.003, -60.0);
    steps(&mut [&mut a, &mut b], 500);
    // Held two seconds: it points as long as the button is down, and lowers the arm soon after it comes up.
    a.host.mouse_button(MouseButton::Left, true, (400, 300));
    steps(&mut [&mut a, &mut b], 300);
    assert_eq!(arms(&mut a, &mut b, id), point());
    steps(&mut [&mut a, &mut b], 1700);
    assert_eq!(arms(&mut a, &mut b, id), point());
    a.host.mouse_button(MouseButton::Left, false, (400, 300));
    steps(&mut [&mut a, &mut b], 400);
    assert_eq!(arms(&mut a, &mut b, id), empty());
    // A quick click points for a second.
    click(&mut a);
    steps(&mut [&mut a, &mut b], 600);
    assert_eq!(arms(&mut a, &mut b, id), point());
    steps(&mut [&mut a, &mut b], 900);
    assert_eq!(arms(&mut a, &mut b, id), empty());
    assert!(!a.seated());
    // Turned back, a click on the buggy seats the figure, its arm empty.
    a.host.mouse_motion(-std::f32::consts::PI / 0.003, 60.0);
    steps(&mut [&mut a, &mut b], 300);
    get_in(&mut [&mut a, &mut b], 0);
    assert!(a.driving());
    assert_eq!(server(&a).figure_arm_of(id).as_deref(), Some("look"));
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

#[test]
fn chat_goes_round_and_at_the_wheel_t_opens_it_without_driving() {
    let mut a = host();
    let mut b = join(&mut a);
    assert_eq!(a.global("$Chat::Last"), "Bob joined the game.");
    b.call("say", &["race you <color:ff0000>up the hill"])
        .unwrap();
    steps(&mut [&mut a, &mut b], 300);
    assert_eq!(a.global("$Chat::Last"), "Bob: race you up the hill");
    assert_eq!(b.global("$Chat::Last"), "Bob: race you up the hill");

    // At the wheel, T opens the chat: the mouse is free, W types and does not drive, Esc closes it and W drives again.
    get_in(&mut [&mut a, &mut b], 0);
    assert!(a.driving());
    press(&mut a, Key::T);
    steps(&mut [&mut a, &mut b], 64);
    assert_eq!(a.global("$Chat::Open"), "1");
    assert!(!a.host.wants_mouse(), "the mouse is free while chatting");
    a.key(Key::W, true);
    assert_eq!(
        a.global("$Drive::Throttle"),
        "0",
        "W types, it does not drive"
    );
    a.key(Key::W, false);
    assert_eq!(
        a.call("eval", &["return ChatEdit.getValue();"]).unwrap(),
        "w"
    );
    press(&mut a, Key::Escape);
    steps(&mut [&mut a, &mut b], 64);
    assert_eq!(a.global("$Chat::Open"), "0");
    assert_ne!(
        a.global("$Client::Paused"),
        "1",
        "Esc closed the chat, not the game"
    );
    assert!(a.host.wants_mouse());
    a.key(Key::W, true);
    assert_eq!(a.global("$Drive::Throttle"), "1", "W drives again");
    a.key(Key::W, false);
    // Enter sends what was typed.
    press(&mut a, Key::T);
    steps(&mut [&mut a, &mut b], 64);
    for key in [Key::G, Key::O] {
        press(&mut a, key);
    }
    press(&mut a, Key::Enter);
    steps(&mut [&mut a, &mut b], 300);
    assert_eq!(b.global("$Chat::Last"), "Ann: go");
    assert!(a.host.input_errors.is_empty(), "{:?}", a.host.input_errors);
    assert!(errors(&a).is_empty(), "{:?}", errors(&a));
}

/// The names the view's HUD has this tick: each name, where its label stands and whether something hides it.
fn names(game: &VehicleDemo) -> Vec<(String, [f32; 3], bool)> {
    game.host
        .gui
        .0
        .borrow()
        .hud
        .names
        .iter()
        .map(|n| {
            (
                String::from_utf8_lossy(&n.name).into_owned(),
                n.position,
                n.hidden,
            )
        })
        .collect()
}

/// The one name `game`'s view has, which must be `who`'s, showing: where its label stands.
fn name_of(game: &VehicleDemo, who: &str) -> [f32; 3] {
    let seen = names(game);
    match seen.as_slice() {
        [(name, at, false)] if name == who => *at,
        _ => panic!("expected only {who}'s name, showing: {seen:?}"),
    }
}

#[test]
fn each_sees_the_others_name_on_foot_and_in_a_seat_never_its_own() {
    let mut a = host();
    let mut b = join(&mut a);
    // On foot: each sees the other's name over the head, never its own.
    let bob = name_of(&a, "Bob");
    name_of(&b, "Ann");
    let feet = server(&a)
        .figure_position_of(server(&a).figures()[1])
        .unwrap();
    assert!(
        horizontal([bob[0] - feet[0], bob[1] - feet[1], 0.0]) < 0.5 && bob[2] - feet[2] > 1.2,
        "Bob's label at {bob:?}, his feet at {feet:?}"
    );

    // Both seated, Ann at the wheel: still each other's name only, over the seats, and it rides along with the buggy.
    get_in(&mut [&mut a, &mut b], 0);
    get_in(&mut [&mut a, &mut b], 1);
    assert!(a.driving() && b.seat() == Some(1));
    steps(&mut [&mut a, &mut b], 500);
    let near = |game: &VehicleDemo, who: &str| {
        let at = name_of(game, who);
        let buggy = game.buggy_pose().unwrap().column(3);
        let off = horizontal([at[0] - buggy[0], at[1] - buggy[1], 0.0]);
        assert!(off < 1.5, "{who}'s label at {at:?}, the buggy at {buggy:?}");
        assert!(at[2] > buggy[2], "{who}'s label over the buggy");
        at
    };
    let before = near(&a, "Bob");
    near(&b, "Ann");
    a.key(Key::W, true);
    steps(&mut [&mut a, &mut b], 2500);
    a.key(Key::W, false);
    a.key(Key::Space, true);
    steps(&mut [&mut a, &mut b], 2000);
    a.key(Key::Space, false);
    let after = near(&a, "Bob");
    near(&b, "Ann");
    assert!(after[1] - before[1] > 6.0, "from {before:?} to {after:?}");
}
