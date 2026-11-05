//! The example without a window or a GPU: clients and the server one of them hosts in one process over the loopback address,
//! the keys and the clicks posted as a window posts them, and the figures and the buggy read from the server's simulation.

use torque::client::input::{Key, MouseButton};
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
