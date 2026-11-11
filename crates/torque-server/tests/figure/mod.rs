//! The rig of the hitbox and lag compensation tests: a server with TGE's game classes and a figure built in code, a torso, a
//! head and a right arm, each a box in the shape's `LOS-9` detail on its own node.

#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use torque_server::server::{Server, TICK_MS};
use torque_server::state::ServerState;
use torque_shape_builder::{MeshData, SequenceDef, ShapeBuilder, Track, turn};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;

const X: [f32; 3] = [1.0, 0.0, 0.0];
const Y: [f32; 3] = [0.0, 1.0, 0.0];

fn still(
    name: &'static str,
    rotations: Vec<Track<[i16; 4]>>,
    translations: Vec<Track<[f32; 3]>>,
) -> SequenceDef {
    SequenceDef {
        name,
        duration: 10.0,
        cyclic: true,
        priority: 0,
        keyframes: 2,
        rotations,
        translations,
        ground: Vec::new(),
    }
}

/// The figure: `base` at the feet, `torso` 1 up (its box from the feet to 1.6), `head` 0.8 above the torso (its box 1.8 to 2.2),
/// `rarm` at the right shoulder (x 0.5, z 1.6; its box hangs to 1.0, x 0.4 to 0.6). `look` raises the arm forward at its end
/// (straight down at its middle); `crouch` lowers the torso by 0.5; `lean` tips the whole figure 30 degrees toward +x.
pub fn figure() -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let base = b.node("base", None, [0.0; 3]);
    let torso = b.node("torso", Some(base), [0.0, 0.0, 1.0]);
    let head = b.node("head", Some(torso), [0.0, 0.0, 0.8]);
    let rarm = b.node("rarm", Some(torso), [0.5, 0.0, 0.6]);
    let sight = b.detail("LOS-9", -1.0);
    b.object(
        "torso",
        torso,
        sight,
        MeshData::hull_box([-0.3, -0.2, -1.0], [0.3, 0.2, 0.6]),
    );
    b.object(
        "head",
        head,
        sight,
        MeshData::hull_box([-0.2, -0.2, 0.0], [0.2, 0.2, 0.4]),
    );
    b.object(
        "rarm",
        rarm,
        sight,
        MeshData::hull_box([-0.1, -0.1, -0.6], [0.1, 0.1, 0.0]),
    );
    let rest = turn(X, 0.0);
    b.sequence(SequenceDef {
        name: "look",
        duration: 1.0,
        cyclic: false,
        priority: 0,
        keyframes: 3,
        rotations: vec![Track {
            node: rarm,
            keys: vec![rest, rest, turn(X, 90.0)],
        }],
        translations: Vec::new(),
        ground: Vec::new(),
    });
    b.sequence(still(
        "crouch",
        Vec::new(),
        vec![Track {
            node: torso,
            keys: vec![[0.0, 0.0, 0.5]; 2],
        }],
    ));
    b.sequence(still(
        "lean",
        vec![Track {
            node: base,
            keys: vec![turn(Y, 30.0); 2],
        }],
        Vec::new(),
    ));
    torque_shapes::write::write_dts(&b.build())
}

/// A shape with no LOS or collision detail: nothing for hitboxes to find.
pub fn plain() -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let node = b.node("base", None, [0.0; 3]);
    let drawn = b.detail("detail1", 1.0);
    let mut mesh = MeshData::new(None);
    mesh.add_box([-0.3, -0.3, 0.0], [0.3, 0.3, 2.0], |_| {
        torque_shape_builder::FULL_UV
    });
    b.object("body", node, drawn, mesh);
    torque_shapes::write::write_dts(&b.build())
}

/// An interior: a wall 4 wide (x -2 to 2), 0.5 thick (y -0.25 to 0.25) and 3 high, standing on a floor slab.
pub fn wall() -> Vec<u8> {
    use torque_interior_builder::{Brush, Options, TexGen, V3, build};
    let grid = |n| TexGen::world(n, 1.0);
    let brushes = [
        Brush::cuboid(
            V3::new(-2.0, -0.25, 0.0),
            V3::new(2.0, 0.25, 3.0),
            "wall",
            grid,
        ),
        Brush::cuboid(
            V3::new(-4.0, -4.0, -1.0),
            V3::new(4.0, 4.0, 0.0),
            "floor",
            grid,
        ),
    ];
    build(&brushes, &Options::default())
        .unwrap()
        .bytes()
        .unwrap()
}

pub fn game_server() -> Server {
    let mut vm = torque_game::testing::vm();
    let files = MemFiles::new();
    files.insert("figure.dts", &figure());
    files.insert("plain.dts", &plain());
    files.insert("wall.dif", &wall());
    vm.set_file_host(files);
    let state = Rc::new(RefCell::new(ServerState::default()));
    torque_server::game::install(&mut vm, &state);
    let mut s = Server::with_state(vm, state);
    s.run(TICK_MS, |_| false);
    s.vm.eval(
        b"datablock ProjectileData(Bolt) { muzzleVelocity = 50; lifetime = 320; };
datablock PlayerData(Boxed) { shapeFile = \"figure.dts\"; boundingBox = \"3 3 2.4\"; maxDamage = 100;
   minLookAngle = -1.5; maxLookAngle = 1.5; };
datablock PlayerData(Posed : Boxed) { hitboxes = true; };
datablock PlayerData(NoParts) { shapeFile = \"plain.dts\"; boundingBox = \"3 3 2.4\"; hitboxes = true; };
function Bolt::onCollision(%this, %obj, %col, %fade, %pos, %normal) { $hit = %col; $hitPos = %pos; }",
    )
    .unwrap();
    s
}

pub fn eval(s: &mut Server, text: &str) -> String {
    String::from_utf8(s.vm.eval(text.as_bytes()).unwrap()).unwrap()
}

pub fn id(s: &Server, name: &str) -> ObjId {
    s.vm.objects().find(name.as_bytes()).unwrap()
}

/// The object a ray from `start` to `end` hits first among the Players (0 for none), and the hit's point.
pub fn ray(s: &mut Server, start: &str, end: &str) -> (ObjId, String) {
    let text = eval(
        s,
        &format!("return containerRayCast(\"{start}\", \"{end}\", 16384);"),
    );
    let mut words = text.splitn(2, ' ');
    let object = words.next().unwrap().parse().unwrap();
    (
        object,
        words
            .next()
            .unwrap_or("")
            .rsplitn(4, ' ')
            .last()
            .unwrap_or("")
            .to_string(),
    )
}

/// A ray along +y at the height `z`, `x` across.
pub fn across(s: &mut Server, x: f32, z: f32) -> (ObjId, String) {
    ray(s, &format!("{x} -5 {z}"), &format!("{x} 5 {z}"))
}

/// A Player of datablock `db` at `at`, ticked once.
pub fn spawn(s: &mut Server, name: &str, db: &str, at: &str) -> ObjId {
    eval(
        s,
        &format!("new Player({name}) {{ dataBlock = {db}; position = \"{at}\"; }};"),
    );
    // The Player's box is set at its first tick.
    s.run(TICK_MS, |_| false);
    id(s, name)
}
