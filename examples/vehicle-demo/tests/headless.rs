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
    // Back to the menu: the client's own socket is given back.
    game.key(Key::Escape, true);
    game.key(Key::Escape, false);
    steps(&mut [&mut game], 100);
    assert!(game.server.is_none() && !game.local());
    assert!(errors(&game).is_empty(), "{:?}", errors(&game));
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
    // Some 0.3 s at 60 frames a second, no frame's step more than its eased share of the way.
    assert!((16..=21).contains(&frames), "{frames} frames");
    assert!(
        biggest < whole * 2.0 / 18.0 + 0.05,
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
    assert!(apart(last.position, next.position) < whole * 2.0 / 18.0 + 0.05);
    for _ in 0..30 {
        drawn(&mut game);
    }
    assert!(!game.camera_blending());
    // On foot: the figure shows until the camera reaches its eyes, and again as soon as it leaves them.
    press(&mut game, Key::E);
    for _ in 0..60 {
        drawn(&mut game);
    }
    assert!(!game.seated() && game.first_person());
    let hidden = |g: &VehicleDemo| g.with_world(|w| w.hide_control).unwrap();
    assert!(hidden(&game));
    press(&mut game, Key::Tab);
    drawn(&mut game);
    assert!(!hidden(&game) && game.camera_blending());
    for _ in 0..30 {
        drawn(&mut game);
    }
    press(&mut game, Key::Tab);
    drawn(&mut game);
    assert!(!hidden(&game) && game.camera_blending());
    for _ in 0..30 {
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

#[test]
fn at_speed_the_tracks_end_and_the_dust_starts_under_the_wheels() {
    let mut game = host();
    get_in(&mut [&mut game], 0);
    game.key(Key::W, true);
    let mut checked = 0;
    for _ in 0..(8000 / 16) {
        game.step(16);
        game.update_scene(0.016);
        let speed = horizontal(server(&game).buggy_velocity().unwrap());
        if speed < 12.0 {
            continue;
        }
        let contacts = game.wheel_contacts();
        let near = |p: [f32; 3]| {
            contacts
                .iter()
                .map(|c| (p[0] - c[0]).hypot(p[1] - c[1]))
                .fold(f32::MAX, f32::min)
        };
        // Each head's far edge is across a wheel's contact: its farthest point along the way the buggy goes is level with a
        // contact (within a tire's width to the side).
        let v = server(&game).buggy_velocity().unwrap();
        let dir = [v[0] / speed, v[1] / speed];
        let along = |p: [f32; 3]| p[0] * dir[0] + p[1] * dir[1];
        let across = |p: [f32; 3]| p[1] * dir[0] - p[0] * dir[1];
        for head in game.scene().track_heads() {
            let far = head
                .iter()
                .copied()
                .fold(head[0], |a, p| if along(p) > along(a) { p } else { a });
            let best = contacts
                .iter()
                .filter(|c| (across(**c) - across(far)).abs() < 0.3)
                .map(|c| (along(*c) - along(far)).abs())
                .fold(f32::MAX, f32::min);
            assert!(
                best < 0.1,
                "a head ends {best} from its wheel at {speed} m/s"
            );
        }
        // The newest dust was made at a wheel (within the emitter's ejection offset, 0.1).
        if let Some(p) = game
            .scene()
            .dust_spots()
            .iter()
            .filter(|(_, age)| *age < 16)
            .map(|(p, _)| *p)
            .next()
        {
            let d = near(p);
            assert!(
                d < 0.15,
                "new dust {d} from the wheels at {speed} m/s: {p:?} {contacts:?} vel {v:?}"
            );
            checked += 1;
        }
    }
    game.key(Key::W, false);
    assert!(checked > 10, "{checked} frames checked");
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
