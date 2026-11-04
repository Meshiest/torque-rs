//! Synced poses: a server Player with hitboxes and a client drawing it with the synced rules pose it alike. A figure built in
//! code (a torso, a head and two legs with feet, each part a box of its `LOS-9` detail) runs on a floor and jumps. After each
//! tick the client is given the server's state (an update with no latency) and draws two uneven frames over the next tick,
//! the figure carried on along its velocity as a client draws another Player; the server's posed nodes after that tick are
//! compared with the client's drawn ones. TGE's server, without the field, stands at the run's start meanwhile.

use std::cell::RefCell;
use std::rc::Rc;

use shapes::Shape;
use shapes::instance::ShapeInstance;
use torque_client::player_anim::{PlayerAnimation, PlayerMotion};
use torque_game::scene::hitboxes;
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
    b
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
datablock PlayerData(Posed : Boxed) { hitboxes = true; };"
    );
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
        let s = torque_game::sim::player::state(vm, id).unwrap();
        let m = &s.obj_to_world;
        let mut motion = PlayerMotion {
            velocity: s.velocity,
            yaw: s.rot[2],
            falling: s.falling,
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
