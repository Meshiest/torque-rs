//! Synced poses: a server Player with hitboxes and a client drawing it with the synced rules pose it alike. A figure built in
//! code (a torso, a head and two legs with feet, each part a box of its `LOS-9` detail) runs on a floor and jumps. After each
//! tick the client is given the server's state (an update with no latency) and draws two uneven frames over the next tick,
//! the figure carried on along its velocity as a client draws another Player; the server's posed nodes after that tick are
//! compared with the client's drawn ones. TGE's server, without the field, stands at the run's start meanwhile.
//!
//! The poses (the engine's own after Torque3D's): a figure crouched, prone, walking crouched, crawling and changing between them
//! (its triggers given by a test class's moves) is posed alike on both sides, the client drawing the pose the record's move
//! shows; and a hit's reaction (a script thread the server plays) adds to the pose alike on both.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use shapes::Shape;
use shapes::instance::ShapeInstance;
use torque_client::player_anim::{PlayerAnimation, PlayerMotion, ScriptPlay, record_pose};
use torque_game::scene::hitboxes;
use torque_game::sim::player::poses::{Pose, PoseTriggers};
use torque_game::sim_classes::{PlayerKind, SimClasses, set_sim_classes};
use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_shape_builder::{MeshData, SequenceDef, ShapeBuilder, Track, turn};
use ts_vm::host::MemFiles;
use ts_vm::sim::ObjId;
use ts_vm::vm::Vm;

const X: [f32; 3] = [1.0, 0.0, 0.0];
/// The run's ground speed (units a second) and the speed the runner slides at (no run force holds it back).
const RUN_SPEED: f32 = 4.0;
const MOVE_SPEED: f32 = 5.0;
/// The walk's ground speed, and a walking pace under the limit between it and the run's.
const WALK_SPEED: f32 = 1.5;
const WALK_PACE: f32 = 1.8;
/// The back's ground speed, and a slow pace backward (a walk backward).
const BACK_SPEED: f32 = 3.0;
const BACK_PACE: f32 = 1.2;

fn sequence(
    name: &'static str,
    duration: f32,
    cyclic: bool,
    rotations: Vec<Track<[i16; 4]>>,
    translations: Vec<Track<[f32; 3]>>,
    ground: Vec<[f32; 3]>,
) -> SequenceDef {
    let keyframes = rotations
        .iter()
        .map(|t| t.keys.len())
        .chain(translations.iter().map(|t| t.keys.len()))
        .max()
        .unwrap_or(2);
    SequenceDef {
        name,
        duration,
        cyclic,
        priority: 0,
        keyframes,
        rotations,
        translations,
        ground,
    }
}

/// The figure: `torso` 1 up (a bob in `root`), `head` on it, the legs at the hips with a foot node 0.9 below each. `run` swings
/// the legs 30 degrees each way over 0.8 seconds, carrying the figure [`RUN_SPEED`] along +y; `jump` tucks them up 80 degrees
/// over 0.5 seconds; `fall` holds them still.
fn figure() -> ShapeBuilder {
    let mut b = ShapeBuilder::new();
    let base = b.node("base", None, [0.0; 3]);
    let torso = b.node("torso", Some(base), [0.0, 0.0, 1.0]);
    let head = b.node("head", Some(torso), [0.0, 0.0, 0.7]);
    let lleg = b.node("lleg", Some(base), [-0.15, 0.0, 1.0]);
    let rleg = b.node("rleg", Some(base), [0.15, 0.0, 1.0]);
    let lfoot = b.node("lfoot", Some(lleg), [0.0, 0.0, -0.9]);
    let rfoot = b.node("rfoot", Some(rleg), [0.0, 0.0, -0.9]);
    let sight = b.detail("LOS-9", -1.0);
    let boxes = [
        ("torso", torso, [-0.25, -0.12, 0.0], [0.25, 0.12, 0.7]),
        ("head", head, [-0.15, -0.15, 0.0], [0.15, 0.15, 0.3]),
        ("lleg", lleg, [-0.08, -0.08, -0.9], [0.08, 0.08, 0.0]),
        ("rleg", rleg, [-0.08, -0.08, -0.9], [0.08, 0.08, 0.0]),
        ("lfoot", lfoot, [-0.06, -0.03, -0.06], [0.06, 0.09, 0.0]),
        ("rfoot", rfoot, [-0.06, -0.03, -0.06], [0.06, 0.09, 0.0]),
    ];
    for (name, node, min, max) in boxes {
        b.object(name, node, sight, MeshData::hull_box(min, max));
    }
    let bob = [
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 1.05],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, 0.95],
    ];
    b.sequence(sequence(
        "root",
        2.0,
        true,
        Vec::new(),
        vec![Track {
            node: torso,
            keys: bob.to_vec(),
        }],
        Vec::new(),
    ));
    let swing = |s: f32| [0.0, 30.0 * s, 0.0, -30.0 * s].map(|d| turn(X, d)).to_vec();
    let ground = (1..=4)
        .map(|k| [0.0, RUN_SPEED * 0.8 * k as f32 / 4.0, 0.0])
        .collect();
    b.sequence(sequence(
        "run",
        0.8,
        true,
        vec![
            Track {
                node: lleg,
                keys: swing(1.0),
            },
            Track {
                node: rleg,
                keys: swing(-1.0),
            },
        ],
        Vec::new(),
        ground,
    ));
    // `back` swings them 20 degrees each way over 0.8 seconds at [`BACK_SPEED`] backward.
    let back_step = |s: f32| [0.0, 20.0 * s, 0.0, -20.0 * s].map(|d| turn(X, d)).to_vec();
    let ground = (1..=4)
        .map(|k| [0.0, -BACK_SPEED * 0.8 * k as f32 / 4.0, 0.0])
        .collect();
    b.sequence(sequence(
        "back",
        0.8,
        true,
        vec![
            Track {
                node: lleg,
                keys: back_step(-1.0),
            },
            Track {
                node: rleg,
                keys: back_step(1.0),
            },
        ],
        Vec::new(),
        ground,
    ));
    // `walk` swings the legs 15 degrees each way over a second at [`WALK_SPEED`], each furthest forward where `run` has it.
    let step = |s: f32| [0.0, 15.0 * s, 0.0, -15.0 * s].map(|d| turn(X, d)).to_vec();
    let ground = (1..=4)
        .map(|k| [0.0, WALK_SPEED * k as f32 / 4.0, 0.0])
        .collect();
    b.sequence(sequence(
        "walk",
        1.0,
        true,
        vec![
            Track {
                node: lleg,
                keys: step(1.0),
            },
            Track {
                node: rleg,
                keys: step(-1.0),
            },
        ],
        Vec::new(),
        ground,
    ));
    let tuck = [0.0, 50.0, 80.0].map(|d| turn(X, d)).to_vec();
    b.sequence(sequence(
        "jump",
        0.5,
        false,
        vec![
            Track {
                node: lleg,
                keys: tuck.clone(),
            },
            Track {
                node: rleg,
                keys: tuck,
            },
        ],
        Vec::new(),
        Vec::new(),
    ));
    let rest = vec![turn(X, 0.0); 2];
    b.sequence(sequence(
        "fall",
        1.0,
        true,
        vec![Track {
            node: lleg,
            keys: rest,
        }],
        Vec::new(),
        Vec::new(),
    ));
    // Crouched: the body lowered 0.4 and the legs bent; `crouch_forward` swings them at [`CROUCH_SPEED`]. Prone: the base turned
    // to lie along +y; `prone_forward` paddles the legs at [`CRAWL_SPEED`]. `flinch_front`, a blend, bends the torso back.
    let low = |dz: f32| vec![[0.0, 0.0, 1.0 - dz]; 4];
    let bent = |s: f32| {
        [40.0 + 15.0 * s, 40.0, 40.0 - 15.0 * s, 40.0]
            .map(|d| turn(X, d))
            .to_vec()
    };
    let along = |speed: f32, duration: f32| {
        (1..=4)
            .map(|k| [0.0, speed * duration * k as f32 / 4.0, 0.0])
            .collect::<Vec<_>>()
    };
    let crouched = |s: f32| {
        vec![
            Track {
                node: lleg,
                keys: bent(s),
            },
            Track {
                node: rleg,
                keys: bent(-s),
            },
        ]
    };
    let lowered = || {
        vec![Track {
            node: torso,
            keys: low(0.4),
        }]
    };
    b.sequence(sequence(
        "crouch_root",
        1.0,
        true,
        crouched(0.0),
        lowered(),
        Vec::new(),
    ));
    b.sequence(sequence(
        "crouch_forward",
        0.8,
        true,
        crouched(1.0),
        lowered(),
        along(CROUCH_SPEED, 0.8),
    ));
    let flat = vec![turn(X, -90.0); 4];
    let paddle = |s: f32| [0.0, 20.0 * s, 0.0, -20.0 * s].map(|d| turn(X, d)).to_vec();
    let lying = |s: f32| {
        vec![
            Track {
                node: base,
                keys: flat.clone(),
            },
            Track {
                node: lleg,
                keys: paddle(s),
            },
            Track {
                node: rleg,
                keys: paddle(-s),
            },
        ]
    };
    b.sequence(sequence(
        "prone_root",
        1.0,
        true,
        lying(0.0),
        Vec::new(),
        Vec::new(),
    ));
    b.sequence(sequence(
        "prone_forward",
        1.2,
        true,
        lying(1.0),
        Vec::new(),
        along(CRAWL_SPEED, 1.2),
    ));
    b.blend_sequence(SequenceDef {
        name: "flinch_front",
        duration: 0.3,
        cyclic: false,
        priority: 0,
        keyframes: 4,
        rotations: vec![Track {
            node: torso,
            keys: [0.0, 25.0, 10.0, 0.0].map(|d| turn(X, d)).to_vec(),
        }],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    b
}

/// The crouched walk's and the crawl's ground speeds.
const CROUCH_SPEED: f32 = 1.2;
const CRAWL_SPEED: f32 = 0.6;

/// The pose triggers the test's class holds for every Player of its kind (crouch 3, prone 4).
#[derive(Debug, Default)]
struct Triggers(Cell<[bool; 6]>);

/// A Player class whose moves hold the test's triggers (and nothing else): a server Player with no client posed by the test.
#[derive(Debug)]
struct Posing(Rc<Triggers>);

impl ::sim::player::PlayerClass for Posing {
    fn ai_move(
        &self,
        _s: &mut ::sim::player::PlayerState,
        _host: &mut dyn ::sim::player::PlayerHost,
    ) -> Option<::sim::Move> {
        Some(::sim::Move {
            triggers: self.0.0.get(),
            ..::sim::Move::default()
        })
    }
}

#[derive(Debug)]
struct PosingKind(Rc<Triggers>);

impl PlayerKind for PosingKind {
    fn class(&self) -> Rc<dyn ::sim::player::PlayerClass> {
        Rc::new(Posing(Rc::clone(&self.0)))
    }
}

struct PosingClasses(Rc<Triggers>);

impl SimClasses for PosingClasses {
    fn player_kind(&self, _vm: &Vm, _db: ObjId) -> Rc<dyn PlayerKind> {
        Rc::new(PosingKind(Rc::clone(&self.0)))
    }
}

/// A floor 100 units square, its top at 0.
fn floor() -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let node = b.node("base", None, [0.0; 3]);
    let solid = b.detail("Collision-1", -1.0);
    b.object(
        "floor",
        node,
        solid,
        MeshData::hull_box([-50.0, -50.0, -1.0], [50.0, 50.0, 0.0]),
    );
    shapes::write::write_dts(&b.build())
}

fn server() -> Server {
    let mut vm = Vm::new();
    torque_game::specs::install(&mut vm);
    let files = MemFiles::new();
    files.insert("figure.dts", &shapes::write::write_dts(&figure().build()));
    files.insert("floor.dts", &floor());
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::default()));
    torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, state);
    s.run(TICK_MS, |_| false);
    eval(
        &mut s,
        "datablock StaticShapeData(Floor) { shapeFile = \"floor.dts\"; };
new StaticShape(TheFloor) { dataBlock = Floor; position = \"0 0 0\"; };
datablock PlayerData(Boxed) { shapeFile = \"figure.dts\"; boundingBox = \"1 1 2.2\"; maxDamage = 100;
   runForce = 0; };
datablock PlayerData(Posed : Boxed) { hitboxes = true; crouchBoundingBox = \"1 1 1.5\"; proneBoundingBox = \"2 2 0.6\";
   crouchForce = 0; proneForce = 0; };"
    );
    s
}

/// The server with the test's class installed: its Players hold the triggers `triggers` holds.
fn posing_server(triggers: &Rc<Triggers>) -> Server {
    let mut s = server();
    set_sim_classes(&mut s.vm, Rc::new(PosingClasses(Rc::clone(triggers))));
    s
}

fn eval(s: &mut Server, text: &str) -> String {
    String::from_utf8(s.vm.eval(text.as_bytes()).unwrap()).unwrap()
}

/// The client's side: the figure's shape, an instance of it and its animation (synced or TGE's).
struct Drawn {
    shape: Shape,
    instance: ShapeInstance,
    anim: PlayerAnimation,
}

impl Drawn {
    fn new(synced: bool) -> Drawn {
        let shape = Shape::for_server(figure().build());
        let mut instance = ShapeInstance::new(&shape, &mut |_| true).unwrap();
        let mut anim = PlayerAnimation::new(&shape, &mut instance).unwrap();
        anim.set_synced(synced);
        Drawn {
            shape,
            instance,
            anim,
        }
    }

    /// The frames of the next tick from the server Player's state now: two, of 12 and 20 ms, the figure carried on along its
    /// velocity; then the nodes posed for `detail`.
    fn frames(&mut self, vm: &Vm, id: ObjId, detail: usize) -> Vec<[f32; 3]> {
        self.frames_with(vm, id, detail, Pose::Stand, &[None; 4])
    }

    /// [`Self::frames`] in pose `pose` (as the record shows it), with the script threads `plays` over the Player's own.
    fn frames_with(
        &mut self,
        vm: &Vm,
        id: ObjId,
        detail: usize,
        pose: Pose,
        plays: &[Option<ScriptPlay>; 4],
    ) -> Vec<[f32; 3]> {
        let s = torque_game::sim::player::state(vm, id).unwrap();
        let m = &s.obj_to_world;
        let mut motion = PlayerMotion {
            velocity: s.velocity,
            yaw: s.rot[2],
            falling: s.falling,
            pose,
            ..PlayerMotion::default()
        };
        let mut elapsed = 0.0;
        for ms in [12.0f32, 20.0] {
            let dt = ms / 1000.0;
            elapsed += dt;
            motion.position = Some([0, 1, 2].map(|i| m[3 + 4 * i] + s.velocity[i] * elapsed));
            self.anim
                .update(&self.shape, &mut self.instance, &motion, dt);
        }
        self.anim
            .script_threads(&self.shape, &mut self.instance, plays);
        self.instance
            .animate(&self.shape, detail, None, &mut |_| true)
            .unwrap();
        origins(self.instance.node_transforms())
    }
}

fn origins(nodes: &[[f32; 16]]) -> Vec<[f32; 3]> {
    nodes.iter().map(|m| [m[3], m[7], m[11]]).collect()
}

/// The server's posed nodes now (what a hit is cast at) and its LOS detail.
fn served(s: &mut Server, id: ObjId) -> (Vec<[f32; 3]>, usize) {
    let pose = hitboxes::pose_now(&mut s.vm, id).unwrap().unwrap();
    (origins(&pose.nodes), pose.los[0] as usize)
}

/// The farthest any node is between two poses.
fn apart(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(p, q)| (0..3).map(|i| (p[i] - q[i]).powi(2)).sum::<f32>().sqrt())
        .fold(0.0, f32::max)
}

/// A Player of `db` standing on the floor at `x`, settled, and a client drawing it from then on.
fn runner(s: &mut Server, name: &str, db: &str, x: f32, synced: bool) -> (ObjId, Drawn) {
    eval(
        s,
        &format!("new Player({name}) {{ dataBlock = {db}; position = \"{x} 0 0.01\"; }};"),
    );
    let id = s.vm.objects().find(name.as_bytes()).unwrap();
    let mut drawn = Drawn::new(synced);
    for _ in 0..10 {
        s.run(TICK_MS, |_| false);
        let (_, detail) = served(s, id);
        drawn.frames(&s.vm, id, detail);
    }
    (id, drawn)
}

#[test]
fn a_running_figure_is_posed_alike_by_the_server_and_the_client_over_strides() {
    let mut s = server();
    let (id, mut drawn) = runner(&mut s, "Runner", "Posed", 0.0, true);
    let (old, mut tge) = runner(&mut s, "Old", "Boxed", 4.0, false);
    eval(
        &mut s,
        &format!(
            "Runner.setVelocity(\"0 {MOVE_SPEED} 0\"); Old.setVelocity(\"0 {MOVE_SPEED} 0\");"
        ),
    );
    let (mut worst, mut tge_worst, mut swing) = (0.0f32, 0.0f32, 0.0f32);
    let rest = served(&mut s, id).0;
    // Two strides and more at the run's pace (each 0.8 seconds at speed 1).
    for tick in 0..80 {
        let (_, detail) = served(&mut s, id);
        let client = drawn.frames(&s.vm, id, detail);
        let old_client = tge.frames(&s.vm, old, detail);
        s.run(TICK_MS, |_| false);
        let server = served(&mut s, id).0;
        let old_server = served(&mut s, old).0;
        // Past the blend from `root`.
        if tick >= 20 {
            worst = worst.max(apart(&server, &client));
            tge_worst = tge_worst.max(apart(&old_server, &old_client));
            swing = swing.max(apart(&server, &rest));
        }
    }
    let speed = torque_game::sim::player::state(&s.vm, id).unwrap().velocity[1];
    assert!(speed > 4.0, "the figure runs ({speed})");
    assert!(swing > 0.3, "the feet swing ({swing})");
    assert!(
        worst < 0.01,
        "the server's nodes are within {worst} of the client's"
    );
    assert!(
        tge_worst > 0.2,
        "TGE's server holds the run at its start: {tge_worst} from the client's"
    );
}

#[test]
fn a_jumping_figure_tucks_on_the_server_as_on_the_client() {
    let mut s = server();
    let (id, mut drawn) = runner(&mut s, "Jumper", "Posed", 0.0, true);
    let (rest, _) = served(&mut s, id);
    eval(&mut s, "Jumper.setVelocity(\"0 0 8\");");
    let (mut worst, mut tuck) = (0.0f32, 0.0f32);
    for _ in 0..20 {
        let (_, detail) = served(&mut s, id);
        let client = drawn.frames(&s.vm, id, detail);
        s.run(TICK_MS, |_| false);
        let server = served(&mut s, id).0;
        worst = worst.max(apart(&server, &client));
        tuck = tuck.max(apart(&server, &rest));
    }
    assert!(tuck > 0.5, "the server's feet tuck up ({tuck})");
    assert!(
        worst < 0.01,
        "the server's nodes are within {worst} of the client's"
    );
}

#[test]
fn a_walking_figure_walks_on_the_server_as_on_the_client_and_its_hitboxes_follow() {
    let mut s = server();
    let (id, mut drawn) = runner(&mut s, "Walker", "Posed", 0.0, true);
    let (old, _) = runner(&mut s, "Strider", "Boxed", 4.0, false);
    let walk = torque_client::player_anim::action_list(&drawn.shape)
        .iter()
        .position(|a| a.name == b"walk")
        .unwrap() as u32;
    let rest = served(&mut s, id).0;
    let (mut worst, mut swing, mut walked, mut ran, mut tge_walked) = (0.0f32, 0.0f32, 0, 0, 0);
    // Walking for 2.5 seconds, then running: the server and the client change at the same tick and pose alike throughout,
    // the blend between them included.
    for tick in 0..140 {
        let pace = if tick < 80 { WALK_PACE } else { MOVE_SPEED };
        eval(
            &mut s,
            &format!("Walker.setVelocity(\"0 {pace} 0\"); Strider.setVelocity(\"0 {pace} 0\");"),
        );
        let (_, detail) = served(&mut s, id);
        let client = drawn.frames(&s.vm, id, detail);
        s.run(TICK_MS, |_| false);
        let server = served(&mut s, id).0;
        let action = torque_game::sim::player::state(&s.vm, id)
            .unwrap()
            .action_anim;
        let (drawn_action, forward) = drawn.anim.action();
        if tick >= 20 {
            assert_eq!(
                (action, forward),
                (drawn_action as u32, true),
                "tick {tick}: the server plays what the client draws"
            );
            worst = worst.max(apart(&server, &client));
            if tick < 80 {
                swing = swing.max(apart(&server, &rest));
            }
        }
        walked += usize::from(tick < 80 && action == walk);
        ran += usize::from(tick >= 100 && action == 1);
        tge_walked += usize::from(
            torque_game::sim::player::state(&s.vm, old)
                .unwrap()
                .action_anim
                == walk,
        );
    }
    assert!(
        walked > 70,
        "the server walks at a walking pace ({walked} ticks)"
    );
    assert_eq!(ran, 40, "and runs at a running pace");
    assert!(
        swing > 0.1,
        "the hitboxes' legs swing with the walk ({swing})"
    );
    assert!(
        worst < 0.01,
        "the server's hitbox nodes are within {worst} of the client's"
    );
    // Without hitboxes the server keeps TGE's pick: never the walk.
    assert_eq!(tge_walked, 0);
}

#[test]
fn a_walk_forward_turned_straight_into_a_walk_backward_stays_posed_alike() {
    let mut s = server();
    let (id, mut drawn) = runner(&mut s, "Turner", "Posed", 0.0, true);
    let walk = torque_client::player_anim::action_list(&drawn.shape)
        .iter()
        .position(|a| a.name == b"walk")
        .unwrap() as u32;
    let (mut worst, mut backward) = (0.0f32, 0);
    for tick in 0..100 {
        let pace = if tick < 50 { WALK_PACE } else { -BACK_PACE };
        eval(&mut s, &format!("Turner.setVelocity(\"0 {pace} 0\");"));
        let (_, detail) = served(&mut s, id);
        let client = drawn.frames(&s.vm, id, detail);
        s.run(TICK_MS, |_| false);
        let server = served(&mut s, id).0;
        if tick >= 20 {
            let action = torque_game::sim::player::state(&s.vm, id)
                .unwrap()
                .action_anim;
            assert_eq!(action, walk, "tick {tick}: the server walks");
            assert_eq!(
                drawn.anim.action().0 as u32,
                walk,
                "tick {tick}: the client walks"
            );
            worst = worst.max(apart(&server, &client));
        }
        backward += usize::from(tick >= 60 && !drawn.anim.action().1);
    }
    assert_eq!(backward, 40, "the client walks backward");
    assert!(
        worst < 0.01,
        "the server's hitbox nodes are within {worst} of the client's"
    );
}

/// The pose the server takes this tick with the triggers `t` held (the engine's rule: there is room for every pose here),
/// kneeling between standing and prone.
fn next_pose(s: &Server, id: ObjId, t: &[bool; 6]) -> Pose {
    let st = torque_game::sim::player::state(&s.vm, id).unwrap();
    let asked = PoseTriggers::default().asked(t);
    let kneeling = st.pose == Pose::Crouch
        && st.pose_from.far_from(asked)
        && st.pose_ticks + 1 < ::sim::player::pose::KNEEL_TICKS;
    if st.pose.far_from(asked) || kneeling {
        Pose::Crouch
    } else {
        asked
    }
}

/// The pose the server's record shows for Player `id`: its move's triggers as the record packs them (the server's view of the
/// Player), read back with the datablock's triggers.
fn shown_pose(s: &mut Server, id: ObjId) -> Pose {
    let st = torque_game::sim::player::state(&s.vm, id).unwrap();
    let info = torque_game::sim::player::info(&s.vm, id).unwrap();
    let mut triggers = st.last_move.triggers;
    info.data.poses.show(st.pose, &mut triggers);
    info.data.poses.shown(&triggers)
}

#[test]
fn a_crouching_crawling_figure_is_posed_alike_by_the_server_and_the_client_in_each_pose() {
    let triggers = Rc::new(Triggers::default());
    let mut s = posing_server(&triggers);
    let (id, mut drawn) = runner(&mut s, "Croucher", "Posed", 0.0, true);
    let actions = torque_client::player_anim::action_list(&drawn.shape);
    let action = |name: &[u8]| actions.iter().position(|a| a.name == name).unwrap() as u32;
    let (crouch, crouch_walk, prone, crawl) = (
        action(b"crouch_root"),
        action(b"crouch_forward"),
        action(b"prone_root"),
        action(b"prone_forward"),
    );
    let mut seen = Vec::new();
    let (mut worst, mut boxes) = (0.0f32, Vec::new());
    // Standing, crouched still, walking crouched, lying down, crawling, then up again by way of the crouch.
    let plan: [(usize, [usize; 2], f32); 6] = [
        (15, [9, 9], 0.0),
        (30, [3, 9], 0.0),
        (40, [3, 9], CROUCH_SPEED),
        (40, [3, 4], 0.0),
        (50, [3, 4], CRAWL_SPEED),
        (50, [9, 9], 0.0),
    ];
    let mut tick = 0;
    for (ticks, held, pace) in plan {
        let mut t = [false; 6];
        for h in held.into_iter().filter(|h| *h < 6) {
            t[h] = true;
        }
        triggers.0.set(t);
        for _ in 0..ticks {
            eval(&mut s, &format!("Croucher.setVelocity(\"0 {pace} 0\");"));
            let (_, detail) = served(&mut s, id);
            // The pose the record of this tick shows: the server takes the triggers this tick (with room for every pose here),
            // as the velocity set before the tick is the one its record carries.
            let pose = next_pose(&s, id, &t);
            let client = drawn.frames_with(&s.vm, id, detail, pose, &[None; 4]);
            s.run(TICK_MS, |_| false);
            let server = served(&mut s, id).0;
            let st = torque_game::sim::player::state(&s.vm, id).unwrap();
            if tick >= 10 {
                worst = worst.max(apart(&server, &client));
                assert_eq!(
                    (st.action_anim, true),
                    (drawn.anim.action().0 as u32, drawn.anim.action().1),
                    "tick {tick}: the server plays what the client draws"
                );
            }
            if seen.last() != Some(&st.action_anim) {
                seen.push(st.action_anim);
            }
            boxes.push((st.pose, st.obj_box.max[2]));
            tick += 1;
        }
    }
    assert!(
        worst < 0.01,
        "the server's hitbox nodes are within {worst} of the client's"
    );
    for a in [crouch, crouch_walk, prone, crawl] {
        assert!(seen.contains(&a), "action {a} played: {seen:?}");
    }
    // Up from prone it kneels on the way: the crouch comes between the crawl and standing.
    let up = seen
        .iter()
        .rposition(|a| *a == crawl || *a == prone)
        .unwrap();
    assert_eq!(seen[up + 1], crouch, "{seen:?}");
    // Each pose's box is its own.
    for (pose, top) in boxes {
        let want = match pose {
            Pose::Crouch => 1.5,
            Pose::Prone => 0.6,
            _ => 2.2,
        };
        assert_eq!(top, want, "{pose:?}");
    }
}

#[test]
fn a_hit_reaction_adds_to_the_pose_alike_on_the_server_and_the_client() {
    let triggers = Rc::new(Triggers::default());
    let mut s = posing_server(&triggers);
    let (id, mut drawn) = runner(&mut s, "Hit", "Posed", 0.0, true);
    let flinch = drawn
        .shape
        .sequences
        .iter()
        .position(|q| drawn.shape.names[q.name as usize] == b"flinch_front");
    let flinch = flinch.unwrap();
    let length = drawn.shape.sequences[flinch].duration;
    let (mut worst, mut moved) = (0.0f32, 0.0f32);
    // Standing, then crouched: a reaction in each, the server playing it on script thread 2 as a script's `playThread` does.
    for (held, label) in [
        ([false; 6], "standing"),
        ([false, false, false, true, false, false], "crouched"),
    ] {
        triggers.0.set(held);
        for _ in 0..20 {
            let (_, detail) = served(&mut s, id);
            let pose = shown_pose(&mut s, id);
            drawn.frames_with(&s.vm, id, detail, pose, &[None; 4]);
            s.run(TICK_MS, |_| false);
        }
        let rest = served(&mut s, id).0;
        eval(&mut s, "Hit.playThread(2, \"flinch_front\");");
        for since in 0..12 {
            let (_, detail) = served(&mut s, id);
            let pose = shown_pose(&mut s, id);
            // The client plays the thread from when it was set, as its record put it, a tick on by the frames' end.
            let pos = ((since + 1) as f32 * TICK_MS as f32 / 1000.0 / length).min(1.0);
            let mut plays = [None; 4];
            plays[2] = Some(ScriptPlay {
                sequence: flinch,
                pos,
            });
            let client = drawn.frames_with(&s.vm, id, detail, pose, &plays);
            s.run(TICK_MS, |_| false);
            let server = served(&mut s, id).0;
            worst = worst.max(apart(&server, &client));
            moved = moved.max(apart(&server, &rest));
        }
        assert!(moved > 0.1, "{label}: the hitboxes flinch ({moved})");
    }
    assert!(
        worst < 0.01,
        "the server's hitbox nodes are within {worst} of the client's through the reactions"
    );
}

#[test]
fn a_record_shows_the_pose_its_server_chose() {
    let triggers = Rc::new(Triggers::default());
    let mut s = posing_server(&triggers);
    let (id, _) = runner(&mut s, "Shown", "Posed", 0.0, true);
    let mut t = [false; 6];
    t[3] = true;
    triggers.0.set(t);
    s.run(TICK_MS, |_| false);
    assert_eq!(shown_pose(&mut s, id), Pose::Crouch);
    // A record of that move read back on a client with the same triggers draws the crouch.
    let st = torque_game::sim::player::state(&s.vm, id).unwrap();
    let mut mv = net::moves::Move {
        triggers: st.last_move.triggers,
        ..Default::default()
    };
    let poses = torque_game::sim::player::info(&s.vm, id)
        .unwrap()
        .data
        .poses;
    poses.show(st.pose, &mut mv.triggers);
    let record = ghosts::TgeRecord::Player(ghosts::player::Player {
        motion: Some(ghosts::player::PlayerMove {
            mv: ghosts::motion::PackedMove::from_move(&mv),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(record_pose(&record, &poses), Some(Pose::Crouch));
}
