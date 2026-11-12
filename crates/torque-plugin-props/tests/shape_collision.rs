//! Props that collide as their shape's collision meshes: the hulls a server and a predicting client make from one shape, a
//! datablock's fallbacks (no collision mesh, a cap, a file that is not there) and a hostile shape file.

#[path = "net/rig.rs"]
mod rig;

use std::rc::Rc;

use rig::{PeerProps, Rig, RigProps, files};
use torque_app::GameFiles;
use torque_client::predict::FileReader;
use torque_client::predict::drive::ShapeCache;
use torque_plugin_props::data::PropShape;
use torque_plugin_props::hulls::{
    HullBook, HullCaps, MAX_HULL_VERTICES, MAX_HULLS, PropHulls, Refusal,
};
use torque_plugin_props::predict::{PredictConfig, PropPrediction, PropPredictionPlugin};
use torque_plugin_props::{PropsPlugin, ServerProps};
use torque_shape_builder::{FULL_UV, MeshData, ShapeBuilder, material_flags};
use torque_ts_vm::host::{FileHost, FileRead, MemFiles};
use torque_ts_vm::vm::Vm;

type Box3 = ([f32; 3], [f32; 3]);

/// A shape file whose collision meshes are `hulls` (each one convex mesh: a box), at the root node or, for the second, on a child
/// node moved by `(2, 0, 0)`; a drawn box beside them. No hulls: a shape with no collision mesh.
fn shape_file(hulls: &[Box3]) -> Vec<u8> {
    let mut b = ShapeBuilder::new();
    let root = b.node("root", None, [0.0; 3]);
    let moved = b.node("moved", Some(root), [2.0, 0.0, 0.0]);
    let drawn = b.detail("detail1", 1.0);
    let material = b.material(
        "grid",
        material_flags::S_WRAP | material_flags::T_WRAP | material_flags::NEVER_ENV_MAP,
    );
    let mut mesh = MeshData::new(Some(material));
    mesh.add_box([-1.0, -1.0, -0.25], [1.0, 1.0, 0.25], |_| FULL_UV);
    b.object("body", root, drawn, mesh);
    if !hulls.is_empty() {
        let collision = b.detail("Collision-1", -1.0);
        for (i, &(lo, hi)) in hulls.iter().enumerate() {
            let node = if i == 1 { moved } else { root };
            b.object(
                &format!("Col{i}"),
                node,
                collision,
                MeshData::hull_box(lo, hi),
            );
        }
    }
    torque_shapes::write::write_dts(&b.build())
}

/// The L: two bars two units long and half a unit wide and thick; the second is on a node moved by `(2, 0, 0)`.
fn l_hulls() -> Vec<Box3> {
    vec![
        ([-1.0, -1.0, -0.25], [1.0, -0.5, 0.25]),
        ([-3.0, -0.5, -0.25], [-2.5, 1.0, 0.25]),
    ]
}

fn mem(files: &[(&str, Vec<u8>)]) -> MemFiles {
    let mem = MemFiles::new();
    for (name, bytes) in files {
        mem.insert(name, bytes);
    }
    mem
}

/// A script engine with the shape loader and `files` as its game files.
fn server_vm(files: &MemFiles) -> Vm {
    let mut vm = Vm::new();
    vm.set_file_host(files.clone());
    torque_game::shapes::register(&mut vm);
    vm
}

fn reader(files: &MemFiles) -> FileReader {
    let files = files.clone();
    Rc::new(move |path: &[u8]| match FileHost::read(&files, path) {
        FileRead::Found(bytes) => Some(bytes),
        _ => None,
    })
}

/// What a client makes of the shape at `path`: its cache's read, then the hulls.
fn client_hulls(files: &MemFiles, path: &str, scale: [f32; 3]) -> Result<PropHulls, Refusal> {
    let mut cache = ShapeCache::default();
    let mut work = 0;
    let read = reader(files);
    let shape = cache
        .get(path.as_bytes(), Some(&read), &mut work, 4_000_000)
        .ok_or(Refusal::Unreadable)?;
    PropHulls::of_client(&shape, scale, &HullCaps::default(), &mut |_| true)
}

fn server_hulls(files: &MemFiles, path: &str, scale: [f32; 3]) -> Result<Rc<PropHulls>, Refusal> {
    HullBook::default().load(&mut server_vm(files), path, scale)
}

#[test]
fn a_server_and_a_client_hold_the_same_hulls_for_a_shape() {
    let files = mem(&[("kit/l.dts", shape_file(&l_hulls()))]);
    for scale in [[1.0; 3], [1.5, 1.0, 2.0], [0.25, 0.5, 0.125]] {
        let server = server_hulls(&files, "kit/l.dts", scale).expect("the server's hulls");
        let client = client_hulls(&files, "kit/l.dts", scale).expect("the client's hulls");
        assert_eq!(*server, client, "at {scale:?}");
        assert_eq!(server.hulls().len(), 2);
        assert!(server.hulls().iter().all(|h| h.len() == 8));
        assert!(!server.is_bounds_only());
        // The second bar sits on the moved node: its x from -3 + 2 to -2.5 + 2, times the scale.
        let xs: Vec<f32> = server.hulls()[1].iter().map(|p| p[0]).collect();
        assert!(
            xs.iter().all(|&x| x == -scale[0] || x == -0.5 * scale[0]),
            "{xs:?}"
        );
    }
    // Both hold the shape's bounds when it has no collision mesh.
    let bare = mem(&[("kit/bare.dts", shape_file(&[]))]);
    let server = server_hulls(&bare, "kit/bare.dts", [1.0; 3]).expect("the server's hulls");
    let client = client_hulls(&bare, "kit/bare.dts", [1.0; 3]).expect("the client's hulls");
    assert_eq!(*server, client);
    assert!(server.is_bounds_only() && server.hulls().len() == 1);
}

#[test]
fn a_shape_past_a_cap_is_refused_the_same_way_on_both_sides() {
    let many: Vec<Box3> = (0..=MAX_HULLS)
        .map(|i| ([i as f32; 3], [i as f32 + 0.5; 3]))
        .collect();
    let files = mem(&[("kit/many.dts", shape_file(&many))]);
    let server = server_hulls(&files, "kit/many.dts", [1.0; 3]);
    let client = client_hulls(&files, "kit/many.dts", [1.0; 3]);
    assert!(matches!(server, Err(Refusal::TooManyHulls { found }) if found == MAX_HULLS + 1));
    assert_eq!(server.err(), client.err());
    // A hull of more points than the cap.
    let mut b = ShapeBuilder::new();
    let node = b.node("root", None, [0.0; 3]);
    let collision = b.detail("Collision-1", -1.0);
    let mut round = MeshData::new(None);
    for i in 0..=MAX_HULL_VERTICES {
        let a = i as f32 * 0.1;
        round
            .positions
            .push([a.cos(), a.sin(), (i % 5) as f32 * 0.1]);
        round.normals.push([0.0, 0.0, 1.0]);
        round.tverts.push([0.0, 0.0]);
    }
    round.indices.extend([0, 1, 2]);
    b.object("Col", node, collision, round);
    let files = mem(&[("kit/round.dts", torque_shapes::write::write_dts(&b.build()))]);
    let server = server_hulls(&files, "kit/round.dts", [1.0; 3]);
    assert!(
        matches!(server, Err(Refusal::TooManyVertices { found }) if found == MAX_HULL_VERTICES + 1)
    );
    assert_eq!(
        server.err(),
        client_hulls(&files, "kit/round.dts", [1.0; 3]).err()
    );
}

#[test]
fn the_server_holds_the_points_of_all_its_shapes_to_a_total() {
    let files = mem(&[
        ("kit/a.dts", shape_file(&l_hulls())),
        ("kit/b.dts", shape_file(&l_hulls())),
    ]);
    let mut vm = server_vm(&files);
    // Sixteen points a shape (two boxes), room for one and a half.
    let mut book = HullBook::new(HullCaps {
        total: 24,
        ..HullCaps::default()
    });
    let first = book
        .load(&mut vm, "kit/a.dts", [1.0; 3])
        .expect("the first");
    assert_eq!(book.vertices(), 16);
    // The same shape again costs nothing, and is the same hulls.
    let again = book
        .load(&mut vm, "KIT/A.DTS", [1.0; 3])
        .expect("the first again");
    assert!(Rc::ptr_eq(&first, &again));
    assert_eq!(book.vertices(), 16);
    assert_eq!(
        book.load(&mut vm, "kit/b.dts", [1.0; 3]).err(),
        Some(Refusal::ServerTotal)
    );
    assert_eq!(book.vertices(), 16);
    // A file that is not there is refused, and the refusal is kept (a second ask does not look again).
    assert_eq!(
        book.load(&mut vm, "kit/none.dts", [1.0; 3]).err(),
        Some(Refusal::Unreadable)
    );
    files.insert("kit/none.dts", &shape_file(&l_hulls()));
    assert_eq!(
        book.load(&mut vm, "kit/none.dts", [1.0; 3]).err(),
        Some(Refusal::Unreadable),
        "the file came, and the refusal was kept"
    );
}

/// A cheap xorshift for the fuzz.
fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn a_hostile_shape_file_is_refused_cleanly_whatever_its_counts_say() {
    let good = shape_file(&l_hulls());
    let rounds: usize = std::env::var("TORQUE_FUZZ_PROPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let (mut read, mut refused) = (0, 0);
    for round in 0..rounds {
        let mut bytes = good.clone();
        // A few bytes changed, or a stretch of them set to a huge count; and now and then the file is cut short.
        for _ in 0..1 + next(&mut seed) % 4 {
            let at = (next(&mut seed) as usize) % bytes.len();
            match next(&mut seed) % 3 {
                0 => bytes[at] = next(&mut seed) as u8,
                1 => bytes[at] = 0xff,
                _ => {
                    let end = (at + 4).min(bytes.len());
                    bytes[at..end].fill(0x7f);
                }
            }
        }
        if round % 11 == 0 {
            bytes.truncate((next(&mut seed) as usize) % bytes.len());
        }
        let files = mem(&[("kit/x.dts", bytes)]);
        let server = server_hulls(&files, "kit/x.dts", [1.0; 3]);
        let client = client_hulls(&files, "kit/x.dts", [1.0; 3]);
        match (server, client) {
            (Ok(s), Ok(c)) => {
                read += 1;
                assert_eq!(*s, c, "round {round}");
                assert!(s.hulls().len() <= MAX_HULLS);
                assert!(s.hulls().iter().all(|h| h.len() <= MAX_HULL_VERTICES));
            }
            (Err(_), Err(_)) => refused += 1,
            (s, c) => panic!("round {round}: the server {s:?}, the client {c:?}"),
        }
    }
    assert!(read > 0 && refused > 0, "{read} read, {refused} refused");
}

// ---- a server and a predicting client

/// The test game's files with the shapes `extra` names.
fn game_files(extra: &[(&str, Vec<u8>)]) -> GameFiles {
    let mut files = files();
    for (name, bytes) in extra {
        files.insert(name, bytes);
    }
    files
}

fn predicting(files: &GameFiles) -> torque_engine::Engine {
    use torque_app::config::EngineConfig;
    let game = torque_app::config::GameConfig::load(files).expect("game.toml");
    let mut builder = torque_engine::EngineBuilder::new();
    builder
        .add_plugins(torque_app::plugins::DefaultPlugins)
        .config(&game);
    builder.add_plugins((
        PropsPlugin,
        PropPredictionPlugin {
            config: PredictConfig::default(),
        },
    ));
    builder.build().expect("the engine builds")
}

fn server_prop(rig: &Rig, id: u32) -> (PropShape, Option<Rc<PropHulls>>) {
    let props: ServerProps = rig.props();
    let s = props.0.borrow();
    let p = s
        .world
        .as_ref()
        .and_then(|w| w.get(id))
        .expect("the server's prop");
    (p.params.shape, p.hulls.clone())
}

const DATABLOCKS: &str = r#"
datablock PropData(Lshape) { shapeFile = "kit/l.dts"; collision = "shape"; size = "6 2 0.5"; mass = 50; };
datablock PropData(Bare) { shapeFile = "kit/bare.dts"; collision = "shape"; size = "2 2 0.5"; mass = 10; };
datablock PropData(Many) { shapeFile = "kit/many.dts"; collision = "shape"; size = "1 1 1"; mass = 10; };
datablock PropData(Missing) { shapeFile = "kit/missing.dts"; collision = "shape"; size = "1 1 1"; mass = 10; };
datablock PropData(Plain) { shapeFile = "kit/l.dts"; size = "6 2 0.5"; mass = 50; };
datablock PropData(Odd) { shapeFile = "kit/l.dts"; collision = "meshes"; size = "6 2 0.5"; mass = 50; };
datablock PropData(Spaced) { shapeFile = "kit/l.dts"; collision = "  Shape  "; size = "6 2 0.5"; mass = 50; };
datablock PropData(Long) { shapeFile = "kit/l.dts"; collision = "shape          and more"; size = "6 2 0.5"; mass = 50; };
"#;

#[test]
fn a_datablock_of_shape_collision_gets_its_hulls_and_every_fallback_is_the_box_with_a_note() {
    let many: Vec<Box3> = (0..=MAX_HULLS)
        .map(|i| ([i as f32; 3], [i as f32 + 0.5; 3]))
        .collect();
    let files = game_files(&[
        ("kit/l.dts", shape_file(&l_hulls())),
        ("kit/bare.dts", shape_file(&[])),
        ("kit/many.dts", shape_file(&many)),
    ]);
    let mut rig = Rig::new(&files, &[]);
    rig.eval(DATABLOCKS);
    rig.server.drain_log();
    let spawn = |rig: &mut Rig, name: &str, x: i32| -> u32 {
        rig.eval(&format!("return propSpawn({name}, \"{x} 0 3\");"))
            .parse()
            .unwrap_or_else(|_| panic!("{name} did not spawn: {:?}", rig.server.drain_log()))
    };
    let shaped = spawn(&mut rig, "Lshape", 0);
    let (shape, hulls) = server_prop(&rig, shaped);
    let hulls = hulls.expect("the L's hulls");
    assert_eq!(hulls.hulls().len(), 2);
    // The datablock's box still bounds it, and the mass is the whole shape's.
    assert!(matches!(shape, PropShape::Hulls { half, .. } if half == [3.0, 1.0, 0.25]));
    let mass = {
        let props = rig.props();
        let s = props.0.borrow();
        s.world
            .as_ref()
            .and_then(|w| w.get(shaped))
            .map(|p| p.params.mass())
    };
    assert!((mass.expect("the prop") - 50.0).abs() < 0.05, "{mass:?}");
    assert!(
        rig.server.drain_log().is_empty(),
        "a shape with its meshes is silent"
    );
    // A second prop of the datablock shares the hulls, and nothing is printed again.
    let second = spawn(&mut rig, "Lshape", 10);
    assert!(Rc::ptr_eq(
        &hulls,
        &server_prop(&rig, second).1.expect("hulls")
    ));

    // No collision mesh: the shape's bounds, with a warning.
    let bare = spawn(&mut rig, "Bare", 20);
    let (_, hulls) = server_prop(&rig, bare);
    assert!(hulls.expect("the bounds").is_bounds_only());
    let log = rig.server.drain_log().join("\n");
    assert!(log.contains("no collision mesh"), "{log}");
    // Past a cap, and a file that is not there: the datablock's box, with an error.
    for (name, why) in [("Many", "collision meshes"), ("Missing", "does not load")] {
        let id = spawn(&mut rig, name, 30);
        let (shape, hulls) = server_prop(&rig, id);
        assert!(hulls.is_none(), "{name}");
        assert!(matches!(shape, PropShape::Box(_)), "{name}: {shape:?}");
        let log = rig.server.drain_log().join("\n");
        assert!(
            log.contains(why) && log.contains("collide as its box"),
            "{name}: {log}"
        );
    }
    // A datablock that does not ask, or asks for something else, is a box, silently.
    for name in ["Plain", "Odd"] {
        let id = spawn(&mut rig, name, 40);
        let (shape, hulls) = server_prop(&rig, id);
        assert!(
            hulls.is_none() && matches!(shape, PropShape::Box(_)),
            "{name}"
        );
    }
    assert!(rig.server.drain_log().is_empty());
}

#[test]
fn the_collision_text_is_made_canonical_before_a_client_is_sent_it() {
    let files = game_files(&[("kit/l.dts", shape_file(&l_hulls()))]);
    let mut rig = Rig::new(&files, &[]);
    rig.eval(DATABLOCKS);
    // What is sent is `shape` or `box`, whatever was written: the wire's 8 bytes cannot cut a mode into another.
    for (name, text) in [
        ("Lshape", "shape"),
        ("Spaced", "shape"),
        ("Odd", "box"),
        ("Plain", "box"),
        ("Long", "box"),
    ] {
        assert_eq!(
            rig.eval(&format!("return {name}.collision;")),
            text,
            "{name}"
        );
    }
    // And the server reads what it sends: the padded text is the shape mode, the long one is a box.
    let spawn = |rig: &mut Rig, name: &str| -> u32 {
        rig.eval(&format!("return propSpawn({name}, \"0 0 3\");"))
            .parse()
            .expect("the prop")
    };
    let spaced = spawn(&mut rig, "Spaced");
    assert!(server_prop(&rig, spaced).1.is_some());
    let long = spawn(&mut rig, "Long");
    assert!(server_prop(&rig, long).1.is_none());
}

/// What a server and a predicting client came to for one prop of `datablock` (a name in [`DATABLOCKS`]) that the client's figure
/// touches: the server's hulls, whether the client predicted the prop at any tick, and the hulls it stepped it with.
struct Case {
    server: Option<Rc<PropHulls>>,
    predicted: bool,
    client: Option<Rc<PropHulls>>,
}

/// Runs the case: a server with `server_files` (and its hull book as `book` sets it), a client with `client_files`, the prop
/// spawned beside the figure, `ticks` ticks.
fn touched(
    server_files: &GameFiles,
    client_files: &GameFiles,
    book: Option<HullBook>,
    datablock: &str,
    ticks: usize,
) -> Case {
    let mut rig = Rig::new(server_files, &[("$Test::Players", "1")]);
    rig.eval(DATABLOCKS);
    if let Some(book) = book {
        rig.props().0.borrow_mut().hulls = book;
    }
    rig.join(client_files, predicting(client_files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    // The prop is at rest beside the figure (which stands at 0, -8): its box overlaps the figure's.
    let id: u32 = rig
        .eval(&format!("return propSpawn({datablock}, \"0 -6.9 0.5\");"))
        .parse()
        .expect("the prop");
    let server = server_prop(&rig, id).1;
    let prediction = rig.peers[0]
        .engine
        .resource::<PropPrediction>()
        .expect("the prediction")
        .clone();
    let ghost = |rig: &Rig| {
        let slot = rig
            .props()
            .with(|s| s.world.as_ref()?.get(id).map(|p| p.slot))
            .flatten()?;
        rig.peers[0].book().with(|b| {
            b.props()
                .iter()
                .find_map(|(&g, p)| (p.slot == slot).then_some(g))
        })?
    };
    let mut predicted = false;
    let mut client = None;
    for _ in 0..ticks {
        rig.step();
        if let Some(g) = ghost(&rig)
            && prediction.is_predicted(g)
        {
            predicted = true;
            client = prediction.hulls(g);
            break;
        }
    }
    Case {
        server,
        predicted,
        client,
    }
}

#[test]
fn a_predicting_client_steps_a_prop_it_touches_with_the_hulls_the_server_made() {
    let files = game_files(&[("kit/l.dts", shape_file(&l_hulls()))]);
    let case = touched(&files, &files, None, "Lshape", 400);
    let server = case.server.expect("the server's hulls");
    assert!(case.predicted, "the prop is predicted");
    assert_eq!(*case.client.expect("the client's hulls"), *server);
}

#[test]
fn a_client_that_built_other_hulls_than_the_server_leaves_the_prop_to_the_server() {
    let server = game_files(&[("kit/l.dts", shape_file(&l_hulls()))]);
    // The same path holds another shape on the client (one bar: another count of hulls, points and volume), or nothing.
    let other = shape_file(&l_hulls()[..1]);
    let different = game_files(&[("kit/l.dts", other)]);
    let missing = game_files(&[]);
    for (why, client) in [("another file", &different), ("no file", &missing)] {
        let case = touched(&server, client, None, "Lshape", 250);
        assert!(case.server.is_some(), "{why}: the server has hulls");
        assert!(!case.predicted, "{why}: the prop is not predicted");
    }
    // The control: the same case with the file the server has is predicted within that many ticks.
    assert!(touched(&server, &server, None, "Lshape", 250).predicted);
}

#[test]
fn a_prop_the_server_collides_as_a_box_is_predicted_as_one_whatever_the_client_could_build() {
    let many: Vec<Box3> = (0..=MAX_HULLS)
        .map(|i| ([i as f32; 3], [i as f32 + 0.5; 3]))
        .collect();
    let files = game_files(&[
        ("kit/l.dts", shape_file(&l_hulls())),
        ("kit/many.dts", shape_file(&many)),
    ]);
    // The server's total of points refuses the L (the client has no total): the server has a box, the client could make hulls.
    let tight = HullBook::new(HullCaps {
        total: 4,
        ..HullCaps::default()
    });
    let case = touched(&files, &files, Some(tight), "Lshape", 400);
    assert!(case.server.is_none());
    assert!(case.predicted, "a box is predicted");
    assert!(case.client.is_none(), "as a box");
    // A shape past a cap is a box on both sides.
    let case = touched(&files, &files, None, "Many", 400);
    assert!(case.server.is_none() && case.predicted && case.client.is_none());
    // A file the server cannot read is a box there, and a client that has it predicts the box too.
    let client = game_files(&[("kit/missing.dts", shape_file(&l_hulls()))]);
    let case = touched(&files, &client, None, "Missing", 400);
    assert!(case.server.is_none() && case.predicted && case.client.is_none());
}
