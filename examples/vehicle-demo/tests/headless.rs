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

/// The newest tracks and dust of `buggy` on `game` against its wheels as `game` draws them this frame: the farthest along
/// its way of each strip's head and the newest dust's spawn, each the least distance from a contact (along the way and across
/// it within a tire's width). None while it shows neither.
fn under_wheels(game: &VehicleDemo, buggy: u32, dir: [f32; 2]) -> Option<(f32, f32)> {
    let contacts = game.wheel_contacts_of(buggy);
    if contacts.is_empty() {
        return None;
    }
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
            contacts
                .iter()
                .map(|c| (p[0] - c[0]).hypot(p[1] - c[1]))
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
                assert!(
                    head < 0.15,
                    "{who}: a track head {head} from its wheel at {speed} m/s"
                );
                assert!(
                    dust < 0.15,
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
