//! Swimming in the valley's lake: the figure walks in from the spawn and swims once the water is deep, floats steady with its
//! eye out of the water, swims on, and walks out up a beach and its bank; the client predicts every tick of it as the server
//! runs it. No window and no GPU: the client and the server in one process, the frames placed and not drawn. (One demo a
//! process.)

use torque::client::input::Key;
use torque::game::sim::player::poses::Pose;
use torque::sim::player::PlayerState;
use torque_example_level_demo::client::LevelDemo;
use torque_example_level_demo::files::GameFiles;

/// The lake's surface (the valley's water block's top).
const LEVEL: f32 = 30.0;

fn enter(game: &mut LevelDemo, map: usize) -> bool {
    game.start_map(map).expect("the map starts");
    for _ in 0..600 {
        game.step(32);
        if game.in_world() && game.ghost_count() > 1 {
            return true;
        }
    }
    false
}

/// The figure as the server runs it.
fn server_state(game: &mut LevelDemo) -> PlayerState {
    let server = game.server.as_mut().expect("the demo hosts its map");
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    let text = vm
        .eval(b"return ClientGroup.getObject(0).player.getId();")
        .unwrap();
    let id: torque::script::vm::sim::ObjId = String::from_utf8(text).unwrap().parse().unwrap();
    torque::game::sim::player::state(vm, id).unwrap().clone()
}

/// The figure's datablock on the server: the poses (swimming among them) it gives are the ones the client predicts with.
fn server_poses(game: &mut LevelDemo) -> torque::sim::player::PoseData {
    let server = game.server.as_mut().expect("the demo hosts its map");
    let vm = std::borrow::BorrowMut::<torque::app::server::GameServer>::borrow_mut(server).vm_mut();
    let text = vm
        .eval(b"return ClientGroup.getObject(0).player.getId();")
        .unwrap();
    let id: torque::script::vm::sim::ObjId = String::from_utf8(text).unwrap().parse().unwrap();
    torque::game::sim::player::info(vm, id).unwrap().data.poses
}

/// The prediction's corrections so far.
fn corrections(game: &LevelDemo) -> u64 {
    let st = game.client.host.client.borrow();
    st.conn
        .conn
        .as_ref()
        .expect("the connection")
        .predict
        .stats
        .corrections
}

/// One tick's time in two frames of 16 ms; the figure on the server after it.
fn tick(game: &mut LevelDemo) -> PlayerState {
    for _ in 0..2 {
        game.step(16);
        game.update_scene(0.016);
    }
    server_state(game)
}

/// Ticks until `done` says so, for at most `seconds`; the state then, or none when the time ran out.
fn until(
    game: &mut LevelDemo,
    seconds: f32,
    mut done: impl FnMut(&LevelDemo, &PlayerState) -> bool,
) -> Option<PlayerState> {
    for _ in 0..(seconds / 0.032) as usize {
        let s = tick(game);
        if done(game, &s) {
            return Some(s);
        }
    }
    None
}

fn depth(game: &LevelDemo, p: [f32; 3]) -> f32 {
    LEVEL - game.ground.height(p[0], p[1]).expect("on the terrain")
}

fn flat_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Turns the figure by `radians` (positive: to the left), a little at a time.
fn turn(game: &mut LevelDemo, radians: f32) {
    for _ in 0..10 {
        let step = radians / 10.0 / 0.003;
        game.call("turn", &[&step.to_string()]).unwrap();
        tick(game);
    }
}

#[test]
fn the_figure_swims_in_the_lake_floats_steady_and_walks_out_up_the_bank() {
    let mut game =
        LevelDemo::new(GameFiles::bundled().expect("the game folder")).expect("the client");
    let index = game
        .maps
        .iter()
        .position(|m| m.path.ends_with("valley.mis"))
        .expect("the valley is listed");
    assert!(enter(&mut game, index), "the valley did not start");
    until(&mut game, 2.0, |_, _| false);
    assert_eq!(
        server_poses(&mut game),
        torque_example_level_demo::figure_poses()
    );
    let before = corrections(&game);

    // West from the spawn down the meadow into the lake: walking in the shallows, swimming before the water is 1.5 m deep.
    game.key(Key::W, true);
    game.key(Key::ShiftLeft, true);
    let mut waded = false;
    let start = until(&mut game, 40.0, |g, s| {
        let p = s.position();
        if s.pose != Pose::Swim && s.water_coverage > 0.0 {
            waded = true;
        }
        assert!(
            s.pose == Pose::Swim || depth(g, p) < 1.5,
            "not swimming at {p:?}, {} m deep",
            depth(g, p)
        );
        s.pose == Pose::Swim
    })
    .expect("the figure never swam");
    assert!(waded, "the figure did not walk in the shallows first");
    eprintln!(
        "swims at {:?}, {} m deep",
        start.position(),
        depth(&game, start.position())
    );
    assert!(game.swimming(), "the client predicts the swim");

    // Twenty meters on, swimming all the way.
    let from = start.position();
    until(&mut game, 15.0, |_, s| {
        assert_eq!(s.pose, Pose::Swim, "at {:?}", s.position());
        flat_distance(s.position(), from) > 20.0
    })
    .expect("the figure did not swim 20 m");

    // Stopped, it floats: after three seconds its height holds within 2 cm, its eye out of the water.
    game.key(Key::W, false);
    until(&mut game, 3.0, |_, _| false);
    let (mut low, mut high) = (f32::MAX, f32::MIN);
    until(&mut game, 2.0, |g, s| {
        assert_eq!(s.pose, Pose::Swim);
        let z = s.position()[2];
        (low, high) = (low.min(z), high.max(z));
        let f = g.figure().expect("the own figure");
        assert!(
            f.eye[2] > LEVEL && f.drawn_eye[2] > LEVEL,
            "the eye at {:?} (drawn {:?}) is under the surface",
            f.eye,
            f.drawn_eye
        );
        false
    });
    assert!(high - low < 0.02, "it bobs {} m", high - low);
    eprintln!(
        "bobs {} m, feet {} m under, eye {:?}",
        high - low,
        LEVEL - low,
        game.figure().map(|f| (f.eye[2], f.drawn_eye[2]))
    );
    assert!(
        LEVEL - low > 1.0,
        "floating with its feet {} m under",
        LEVEL - low
    );

    // On west into the deep water, then south: out on the beach and up the bank behind it within 10 s of reaching it.
    game.key(Key::W, true);
    until(&mut game, 60.0, |_, s| s.position()[0] < -40.0).expect("the figure swam on");
    turn(&mut game, -std::f32::consts::FRAC_PI_2);
    let ashore = until(&mut game, 40.0, |g, s| depth(g, s.position()) < 1.0)
        .expect("the figure never reached the beach");
    let out = until(&mut game, 10.0, |g, s| {
        s.pose == Pose::Stand && s.position()[2] > LEVEL + 5.0 && !g.swimming()
    })
    .unwrap_or_else(|| {
        panic!(
            "from {:?} the figure did not walk up the bank within 10 s",
            ashore.position()
        )
    });
    // The bank it climbed: steep (the ground rising 1 in 4 or more where it came out on top), unlike the lake's shallows.
    let p = out.position();
    let ground = |y: f32| game.ground.height(p[0], y).expect("on the terrain");
    let steep = (ground(p[1] - 2.0) - ground(p[1] + 2.0)) / 4.0;
    eprintln!(
        "ashore {:?}, out {p:?}, the bank {steep} steep",
        ashore.position()
    );
    assert!(steep > 0.25, "the bank at {p:?} is only {steep} steep");
    game.key(Key::W, false);
    game.key(Key::ShiftLeft, false);

    assert_eq!(
        corrections(&game) - before,
        0,
        "the client's prediction was corrected"
    );
}
