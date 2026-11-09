//! The scene hierarchy on a real server, stepped by hand (no client): every refusal, rays at the children of a moving root,
//! deletes, and a Player child that moves in its parent's frame.

use std::net::{Ipv4Addr, SocketAddr};
use std::rc::Rc;

use torque_app_server::plugins::{MinimalPlugins, set_protocol};
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_collide::math::{Matrix, matrix_product, mul_p, quat_matrix};
use torque_engine::{ClientStage, EngineBuilder, ProtocolKind, ServerStage};
use torque_game::scene::access::matrix_of;
use torque_game::scene::links::{self, ChildRefusal, ChildRule};
use torque_plugin_hierarchy::{HierarchyPlugin, ServerHierarchy};
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;
use torque_ts_vm::vm::Vm;

const SERVER_CS: &str = r#"
datablock StaticShapeData(CubeData)
{
   shapeFile = "kit/cube.dts";
};

// No shape: cheap objects for big trees.
datablock StaticShapeData(MarkerData)
{
};

datablock CameraData(EyeData)
{
   mode = "Observer";
};

datablock PlayerData(FigureData)
{
   shapeFile = "kit/figure.dts";
   mass = 90;
   drag = 0.3;
   maxDamage = 100;
   boundingBox = "1 1 2";
   runForce = 3600;
   maxForwardSpeed = 6;
   maxBackwardSpeed = 4;
   maxSideSpeed = 5;
   maxStepHeight = 0.6;
   runSurfaceAngle = 60;
   jumpForce = 720;
   jumpSurfaceAngle = 70;
   minImpactSpeed = 40;
};

function CubeData::onParentChanged(%data, %obj, %old, %new)
{
   $Test::Changed = $Test::Changed @ %obj @ ":" @ %old @ ">" @ %new @ " ";
}

function CubeData::onChildAdded(%data, %obj, %child)
{
   $Test::Added = $Test::Added @ %obj @ "+" @ %child @ " ";
}

function cube(%pos)
{
   return new StaticShape() { datablock = CubeData; position = %pos; };
}

function marker(%pos)
{
   return new StaticShape() { datablock = MarkerData; position = %pos; };
}

// An onRemove script that tries to link the dying object both ways and to move its child.
function CubeData::onRemove(%data, %obj)
{
   if (!$Test::Other)
      return;
   $Test::AsChild = %obj.setParent($Test::Other) SPC getParentError();
   $Test::AsParent = $Test::Other.setParent(%obj) SPC getParentError();
   if ($Test::Kid)
      $Test::KidMoved = $Test::Kid.setLocalTransform("1 0 0") SPC getParentError();
}

function startTest()
{
   // A floor 400 units square with its top at z = 0.
   new StaticShape(Floor) { datablock = CubeData; position = "0 0 -0.5"; scale = "400 400 1"; };
}
"#;

/// A server of the minimal plugins in engine mode (the hierarchy is on), `globals` set before its scripts run.
fn server(globals: &[(&str, &str)]) -> GameServer {
    server_in(ProtocolKind::Engine, globals)
}

fn server_in(protocol: ProtocolKind, globals: &[(&str, &str)]) -> GameServer {
    server_with(protocol, globals, |_| {})
}

/// A server of the minimal plugins, `more` added, speaking `protocol`.
fn server_with(
    protocol: ProtocolKind,
    globals: &[(&str, &str)],
    more: impl FnOnce(&mut EngineBuilder),
) -> GameServer {
    let mut builder = EngineBuilder::new();
    builder.add_plugins(MinimalPlugins);
    more(&mut builder);
    set_protocol(&mut builder, protocol);
    let engine = builder.build().expect("the engine builds");
    let files = MemFiles::new();
    files.insert("server.cs", SERVER_CS.as_bytes());
    let dts = torque_shapes::write::write_dts;
    files.insert("kit/cube.dts", &dts(&torque_example_assets::shapes::cube()));
    files.insert(
        "kit/figure.dts",
        &dts(&torque_example_assets::shapes::figure()),
    );
    GameServer::start_engine(
        &engine,
        files,
        &ServerStart {
            listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
            globals,
            start: "startTest",
            args: &[],
        },
    )
    .expect("the server starts")
}

fn eval(server: &mut GameServer, script: &str) -> String {
    let out = server
        .vm_mut()
        .eval(script.as_bytes())
        .unwrap_or_else(|e| panic!("{script}: {e:?}"));
    String::from_utf8_lossy(&out).into_owned()
}

fn id(server: &mut GameServer, script: &str) -> ObjId {
    let text = eval(server, script);
    text.parse()
        .unwrap_or_else(|_| panic!("{script} gave {text:?}, not an id"))
}

fn hierarchy(server: &GameServer) -> ServerHierarchy {
    ServerHierarchy::of(server.vm()).expect("the server has a hierarchy")
}

fn world(vm: &Vm, id: ObjId) -> Matrix {
    matrix_of(vm, id).expect("a scene object")
}

/// A transform from a position and a turn about z.
fn pose(position: [f32; 3], yaw: f32) -> Matrix {
    let (s, c) = (yaw * 0.5).sin_cos();
    let mut m = quat_matrix([0.0, 0.0, s, c]);
    [m[3], m[7], m[11]] = position;
    m
}

/// `"x y z 0 0 1 angle"` of [`pose`].
fn pose_text(position: [f32; 3], yaw: f32) -> String {
    let [x, y, z] = position;
    format!("{x} {y} {z} 0 0 1 {yaw}")
}

fn max_error(a: &Matrix, b: &Matrix) -> f32 {
    (0..12).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// `setParent` of `child` to `parent` (with the rest of its arguments) and the refusal it named.
fn refusal(server: &mut GameServer, call: &str) -> String {
    assert_eq!(
        eval(server, &format!("return {call};")),
        "0",
        "{call} was taken"
    );
    eval(server, "return getParentError();")
}

// ---------------------------------------------------------------------------------------------------- one test per refusal

#[test]
fn refused_self() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    assert_eq!(refusal(&mut s, &format!("{a}.setParent({a})")), "self");
    assert!(hierarchy(&s).is_empty());
}

#[test]
fn refused_cycle() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {b}.setParent({a});")), "1");
    assert_eq!(refusal(&mut s, &format!("{a}.setParent({b})")), "cycle");
    assert_eq!(hierarchy(&s).parent(a), None);
}

#[test]
fn refused_depth() {
    let mut s = server(&[("$Pref::Hierarchy::MaxDepth", "2")]);
    let ids: Vec<ObjId> = (0..4)
        .map(|i| id(&mut s, &format!("return cube(\"{i} 0 5\");")))
        .collect();
    for pair in ids[..3].windows(2) {
        assert_eq!(
            eval(
                &mut s,
                &format!("return {}.setParent({});", pair[1], pair[0])
            ),
            "1"
        );
    }
    assert_eq!(
        refusal(&mut s, &format!("{}.setParent({})", ids[3], ids[2])),
        "depth"
    );
}

#[test]
fn refused_children() {
    let mut s = server(&[("$Pref::Hierarchy::MaxChildren", "2")]);
    let root = id(&mut s, "return cube(\"0 0 5\");");
    for i in 0..2 {
        let c = id(&mut s, &format!("return cube(\"{i} 2 5\");"));
        assert_eq!(eval(&mut s, &format!("return {c}.setParent({root});")), "1");
    }
    let third = id(&mut s, "return cube(\"5 2 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{third}.setParent({root})")),
        "children"
    );
}

#[test]
fn refused_subtree() {
    let mut s = server(&[("$Pref::Hierarchy::MaxDescendants", "2")]);
    let root = id(&mut s, "return cube(\"0 0 5\");");
    let a = id(&mut s, "return cube(\"1 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    let c = id(&mut s, "return cube(\"3 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {a}.setParent({root});")), "1");
    assert_eq!(eval(&mut s, &format!("return {b}.setParent({a});")), "1");
    assert_eq!(refusal(&mut s, &format!("{c}.setParent({a})")), "subtree");
}

#[test]
fn refused_total() {
    let mut s = server(&[("$Pref::Hierarchy::MaxTotal", "2")]);
    let root = id(&mut s, "return cube(\"0 0 5\");");
    for i in 0..2 {
        let c = id(&mut s, &format!("return cube(\"{i} 2 5\");"));
        assert_eq!(eval(&mut s, &format!("return {c}.setParent({root});")), "1");
    }
    let third = id(&mut s, "return cube(\"5 2 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{third}.setParent({root})")),
        "total"
    );
}

#[test]
fn refused_not_finite() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {b}.setParent({a});")), "1");
    let before = hierarchy(&s).local(b);
    assert_eq!(
        refusal(&mut s, &format!("{b}.setLocalTransform(\"1e39 0 0\")")),
        "notFinite"
    );
    assert_eq!(hierarchy(&s).local(b), before);
}

#[test]
fn refused_out_of_range() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {b}.setParent({a});")), "1");
    assert_eq!(
        refusal(&mut s, &format!("{b}.setLocalTransform(\"20000 0 0\")")),
        "outOfRange"
    );
}

#[test]
fn refused_bad_node() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{b}.setParent({a}, 1, \"noSuchNode\")")),
        "badNode"
    );
    // The cube has no mount nodes either.
    assert_eq!(
        refusal(&mut s, &format!("{b}.setParent({a}, 1, 3)")),
        "badNode"
    );
    // Its own node is one.
    assert_eq!(
        eval(&mut s, &format!("return {b}.setParent({a}, 1, \"cube\");")),
        "1"
    );
    assert_eq!(
        eval(&mut s, &format!("return {b}.getParentNode();")),
        "cube"
    );
}

#[test]
fn refused_unknown() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let script = id(&mut s, "return new ScriptObject();");
    assert_eq!(
        refusal(&mut s, &format!("{a}.setParent({script})")),
        "unknown"
    );
    assert_eq!(
        refusal(&mut s, &format!("{a}.setParent(NoSuchObject)")),
        "unknown"
    );
}

#[test]
fn refused_not_parented() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{a}.setLocalTransform(\"1 0 0\")")),
        "notParented"
    );
}

#[test]
fn refused_mounted() {
    let mut s = server(&[]);
    let mount = id(&mut s, "return cube(\"0 0 5\");");
    let rider = id(&mut s, "return cube(\"2 0 5\");");
    let other = id(&mut s, "return cube(\"4 0 5\");");
    eval(&mut s, &format!("{mount}.mountObject({rider}, 0);"));
    assert_eq!(
        eval(&mut s, &format!("return {rider}.getObjectMount();")),
        mount.to_string()
    );
    assert_eq!(
        refusal(&mut s, &format!("{rider}.setParent({other})")),
        "mounted"
    );
    // And a parented object is not mounted.
    let child = id(&mut s, "return cube(\"6 0 5\");");
    assert_eq!(
        eval(&mut s, &format!("return {child}.setParent({other});")),
        "1"
    );
    assert_eq!(
        eval(&mut s, &format!("return {mount}.mountObject({child}, 1);")),
        "0",
        "mountObject says it refused"
    );
    assert_eq!(
        eval(&mut s, &format!("return {child}.getObjectMount();")),
        "0"
    );
}

#[test]
fn refused_no_shape() {
    let mut s = server(&[]);
    let eye = id(&mut s, "return new Camera() { datablock = EyeData; };");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{b}.setParent({eye}, 1, \"mount0\")")),
        "noShape"
    );
    // A shapeless parent takes children on its origin.
    assert_eq!(eval(&mut s, &format!("return {b}.setParent({eye});")), "1");
}

#[test]
fn refused_cycle_through_mounts_both_ways() {
    let mut s = server(&[]);
    let x = id(&mut s, "return cube(\"0 0 5\");");
    let y = id(&mut s, "return cube(\"2 0 5\");");
    // Y rides X: X cannot become Y's child.
    eval(&mut s, &format!("{x}.mountObject({y}, 0);"));
    assert_eq!(
        eval(&mut s, &format!("return {y}.getObjectMount();")),
        x.to_string()
    );
    assert_eq!(refusal(&mut s, &format!("{x}.setParent({y})")), "cycle");
    // A rider may be a parent; its child Z hangs below X through the mount, so X cannot ride Z.
    let z = id(&mut s, "return cube(\"4 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {z}.setParent({y});")), "1");
    assert_eq!(
        eval(&mut s, &format!("return {z}.mountObject({x}, 0);")),
        "0"
    );
    assert_eq!(eval(&mut s, &format!("return {x}.getObjectMount();")), "0");
    // P is Q's child: Q cannot ride P.
    let p = id(&mut s, "return cube(\"6 0 5\");");
    let q = id(&mut s, "return cube(\"8 0 5\");");
    assert_eq!(eval(&mut s, &format!("return {p}.setParent({q});")), "1");
    assert_eq!(
        eval(&mut s, &format!("return {p}.mountObject({q}, 0);")),
        "0"
    );
    assert_eq!(eval(&mut s, &format!("return {q}.getObjectMount();")), "0");
}

#[test]
fn refused_cycle_through_a_spotlight_mount_both_ways() {
    let mut s = server(&[]);
    let lamp = id(
        &mut s,
        "return new SpotLight() { range = 30; innerAngle = 30; outerAngle = 50; };",
    );
    let shape = id(&mut s, "return cube(\"0 0 5\");");
    // The light rides the shape: the shape cannot become the light's child, and the light is mounted.
    assert_eq!(
        eval(&mut s, &format!("return {shape}.mountObject({lamp}, 0);")),
        "1"
    );
    assert_eq!(
        refusal(&mut s, &format!("{shape}.setParent({lamp})")),
        "cycle"
    );
    let other = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(
        refusal(&mut s, &format!("{lamp}.setParent({other})")),
        "mounted"
    );
    eval(&mut s, &format!("{shape}.unmountObject({lamp});"));
    // The shape is the light's child: the light cannot ride it.
    assert_eq!(
        eval(&mut s, &format!("return {shape}.setParent({lamp});")),
        "1"
    );
    assert_eq!(
        eval(&mut s, &format!("return {shape}.mountObject({lamp}, 0);")),
        "0"
    );
}

#[test]
fn refused_removing() {
    let mut s = server(&[]);
    let dying = id(&mut s, "return cube(\"0 0 5\");");
    let other = id(&mut s, "return cube(\"2 0 5\");");
    let kid = id(&mut s, "return cube(\"4 0 5\");");
    assert_eq!(
        eval(&mut s, &format!("return {kid}.setParent({dying});")),
        "1"
    );
    eval(
        &mut s,
        &format!("$Test::Other = {other}; $Test::Kid = {kid}; {dying}.delete();"),
    );
    assert_eq!(eval(&mut s, "return $Test::AsChild;"), "0 removing");
    assert_eq!(eval(&mut s, "return $Test::AsParent;"), "0 removing");
    assert_eq!(eval(&mut s, "return $Test::KidMoved;"), "0 removing");
    let h = hierarchy(&s);
    assert_eq!(h.parent(other), None);
    assert_eq!(
        h.parent(kid),
        None,
        "the kid was taken off its dying parent"
    );
    h.audit().expect("no dangling link");
}

/// A rule that refuses the objects it is given as children, each for the reason it is given.
struct Refuses(Vec<(ObjId, ChildRefusal)>);

impl ChildRule for Refuses {
    fn refuse(&self, _vm: &Vm, child: ObjId, _parent: ObjId) -> Option<ChildRefusal> {
        self.0.iter().find(|(c, _)| *c == child).map(|&(_, r)| r)
    }
}

#[test]
fn refused_class() {
    let mut s = server(&[]);
    let kid = id(&mut s, "return cube(\"0 0 4\");");
    let other = id(&mut s, "return cube(\"1 0 4\");");
    let base = id(&mut s, "return cube(\"2 0 5\");");
    links::add_child_rule(
        s.vm_mut(),
        Rc::new(Refuses(vec![
            (kid, ChildRefusal::Class),
            (other, ChildRefusal::HasJoints),
        ])),
    );
    assert_eq!(
        refusal(&mut s, &format!("{kid}.setParent({base})")),
        "class"
    );
    assert_eq!(
        refusal(&mut s, &format!("{other}.setParent({base})")),
        "joints"
    );
    // The rule is asked of the child only, and only when it is to be hung on something.
    assert_eq!(
        eval(&mut s, &format!("return {base}.setParent({kid});")),
        "1"
    );
    assert_eq!(eval(&mut s, &format!("return {other}.setParent(0);")), "1");
}

#[test]
fn a_prop_may_be_a_child_and_a_parent() {
    let mut s = server_with(ProtocolKind::Engine, &[], |b| {
        b.add_plugins(torque_plugin_props::PropsPlugin);
    });
    eval(
        &mut s,
        "datablock PropData(TestCrate) { shapeFile = \"kit/cube.dts\"; size = \"1 1 1\"; mass = 20; };",
    );
    let prop = id(&mut s, "return propSpawn(TestCrate, \"0 0 4\");");
    let base = id(&mut s, "return cube(\"2 0 5\");");
    assert_eq!(
        eval(&mut s, &format!("return {prop}.setParent({base});")),
        "1"
    );
    assert_eq!(eval(&mut s, &format!("return {prop}.setParent(0);")), "1");
    assert_eq!(
        eval(&mut s, &format!("return {base}.setParent({prop});")),
        "1"
    );
}

#[test]
fn a_scripts_field_write_moves_the_children() {
    let mut s = server(&[]);
    let root = id(&mut s, "return cube(\"0 0 10\");");
    let child = id(&mut s, "return cube(\"3 0 10\");");
    assert_eq!(
        eval(&mut s, &format!("return {child}.setParent({root});")),
        "1"
    );
    eval(&mut s, &format!("{root}.position = \"5 6 7\";"));
    assert_eq!(
        eval(&mut s, &format!("return {child}.getPosition();")),
        "8 6 7"
    );
    // And a child's own field write becomes its local pose.
    eval(&mut s, &format!("{child}.position = \"5 8 7\";"));
    let local = hierarchy(&s).local(child).expect("a child").translation;
    assert!(
        (local[0]).abs() < 1e-5 && (local[1] - 2.0).abs() < 1e-5,
        "{local:?}"
    );
}

#[test]
fn a_childs_wild_move_is_counted_and_takes_it_off() {
    let mut s = server(&[]);
    let root = id(&mut s, "return cube(\"0 0 10\");");
    let child = id(&mut s, "return cube(\"3 0 10\");");
    assert_eq!(
        eval(&mut s, &format!("return {child}.setParent({root});")),
        "1"
    );
    // Past the local translation's range from its parent.
    eval(&mut s, &format!("{child}.setTransform(\"50000 0 10\");"));
    let h = hierarchy(&s);
    assert_eq!(h.parent(child), None);
    let stats = h.stats();
    assert_eq!((stats.refused_moves, stats.dropped), (1, 1), "{stats:?}");
    assert_eq!(world(s.vm(), child)[3], 50_000.0, "left where it was moved");
}

// ------------------------------------------------------------------------------------------------------------ the queries

#[test]
fn a_ray_every_tick_hits_a_child_of_a_moving_root() {
    let mut s = server(&[]);
    let root = id(&mut s, "return cube(\"0 0 10\");");
    let child = id(&mut s, "return cube(\"3 0 10\");");
    assert_eq!(
        eval(&mut s, &format!("return {child}.setParent({root});")),
        "1"
    );
    let local = [3.0, 0.0, 0.0];
    let mut hits = 0;
    for tick in 0..600 {
        let t = tick as f32;
        // Each tick the root moves about two units and turns about a tenth of a turn: a child left a tick behind is missed.
        let position = [
            (t * 1.7) % 100.0 - 50.0,
            (t * 0.1).sin() * 6.0,
            10.0 + (t * 0.07).sin() * 3.0,
        ];
        let yaw = t * 0.6;
        eval(
            &mut s,
            &format!("{root}.setTransform(\"{}\");", pose_text(position, yaw)),
        );
        // Where the child must be: the root's transform as the server made it, times the local offset.
        let at = mul_p(&world(s.vm(), root), local);
        let ray = format!(
            "return containerRayCast(\"{} {} {}\", \"{} {} {}\", -1, {root});",
            at[0],
            at[1],
            at[2] + 5.0,
            at[0],
            at[1],
            at[2] - 5.0
        );
        let hit = eval(&mut s, &ray);
        let words: Vec<f32> = hit.split(' ').filter_map(|w| w.parse().ok()).collect();
        let point = [at[0], at[1], at[2] + 0.5];
        if words.first() == Some(&(child as f32))
            && (1..4).all(|k| (words[k] - point[k - 1]).abs() < 1e-3)
        {
            hits += 1;
        }
        s.tick();
    }
    assert_eq!(hits, 600);
}

#[test]
fn a_childs_world_follows_three_levels_on_nodes_and_origins() {
    let mut s = server(&[]);
    let root = id(&mut s, "return cube(\"0 0 10\");");
    let mid = id(&mut s, "return cube(\"0 2 10\");");
    let leaf = id(&mut s, "return cube(\"0 4 10\");");
    assert_eq!(
        eval(
            &mut s,
            &format!("return {mid}.setParent({root}, 1, \"cube\");")
        ),
        "1"
    );
    assert_eq!(
        eval(&mut s, &format!("return {leaf}.setParent({mid});")),
        "1"
    );
    assert_eq!(
        eval(&mut s, &format!("return {leaf}.getRoot();")),
        root.to_string()
    );
    assert_eq!(
        eval(&mut s, &format!("return {root}.isAncestorOf({leaf});")),
        "1"
    );
    let (mid_local, leaf_local) = (pose([0.0, 2.0, 0.0], 0.0), pose([0.0, 2.0, 0.0], 0.0));
    let moved = pose([5.0, -3.0, 12.0], 1.1);
    eval(
        &mut s,
        &format!(
            "{root}.setTransform(\"{}\");",
            pose_text([5.0, -3.0, 12.0], 1.1)
        ),
    );
    let expected = matrix_product(&matrix_product(&moved, &mid_local), &leaf_local);
    // A script reads the leaf where its root put it, before any tick.
    let text = eval(&mut s, &format!("return {leaf}.getPosition();"));
    let read: Vec<f32> = text
        .split(' ')
        .map(|w| w.parse().expect("a number"))
        .collect();
    let error = (0..3)
        .map(|k| (read[k] - expected[3 + 4 * k]).abs())
        .fold(0.0, f32::max);
    assert!(error < 1e-3, "{text} against {expected:?}");
    // Keep-local reparenting moves the object to its old local pose under the new parent.
    assert_eq!(
        eval(&mut s, &format!("return {leaf}.setParent({root}, 0);")),
        "1"
    );
    let error = max_error(&world(s.vm(), leaf), &matrix_product(&moved, &leaf_local));
    assert!(error < 1e-4, "keep-local error {error}");
}

#[test]
fn callbacks_run_on_the_datablocks() {
    let mut s = server(&[]);
    let a = id(&mut s, "return cube(\"0 0 5\");");
    let b = id(&mut s, "return cube(\"2 0 5\");");
    eval(&mut s, &format!("{b}.setParent({a});"));
    eval(&mut s, &format!("{b}.setParent(0);"));
    assert_eq!(
        eval(&mut s, "return $Test::Changed;"),
        format!("{b}:0>{a} {b}:{a}>0 ")
    );
    assert_eq!(eval(&mut s, "return $Test::Added;"), format!("{a}+{b} "));
}

// ---------------------------------------------------------------------------------------------------------------- deletes

#[test]
fn deleting_a_parent_detaches_its_children_in_place() {
    let mut s = server(&[]);
    let root = id(&mut s, "return marker(\"0 0 10\");");
    let mut children = Vec::new();
    for i in 0..64 {
        let (x, y) = ((i % 8) as f32 * 1.5 - 5.0, (i / 8) as f32 * 1.5 - 5.0);
        let c = id(
            &mut s,
            &format!("return marker(\"{x} {y} {}\");", 10.0 + (i % 3) as f32),
        );
        assert_eq!(eval(&mut s, &format!("return {c}.setParent({root});")), "1");
        children.push(c);
    }
    s.tick();
    let locals: Vec<Matrix> = children
        .iter()
        .map(|c| {
            torque_plugin_hierarchy::server::local_matrix(
                &hierarchy(&s).local(*c).expect("a child"),
            )
        })
        .collect();
    // The root moves and is deleted with nothing reading its children in between.
    eval(
        &mut s,
        &format!(
            "{root}.setTransform(\"{}\");",
            pose_text([7.0, 3.0, 11.0], 0.7)
        ),
    );
    let moved = world(s.vm(), root);
    eval(&mut s, &format!("{root}.delete();"));
    let mut worst: f32 = 0.0;
    for (c, local) in children.iter().zip(&locals) {
        assert_eq!(eval(&mut s, &format!("return {c}.getParent();")), "0");
        worst = worst.max(max_error(
            &world(s.vm(), *c),
            &matrix_product(&moved, local),
        ));
    }
    assert!(worst <= 1e-4, "world error {worst}");
    let h = hierarchy(&s);
    assert!(h.is_empty(), "{} objects left in the tree", h.len());
    h.audit().expect("no dangling link");
}

#[test]
fn a_recursive_delete_of_4096_nodes_leaves_nothing() {
    let mut s = server(&[]);
    let before = s.vm().objects().len();
    // A 16-way tree: node i hangs on node (i - 1) / 16, three levels below the root.
    eval(
        &mut s,
        "$Test::Node[0] = marker(\"0 0 0\");\n\
         for (%i = 1; %i < 4096; %i++) {\n\
            $Test::Node[%i] = marker(%i % 64 SPC mFloor(%i / 64) SPC 1);\n\
            if (!$Test::Node[%i].setParent($Test::Node[mFloor((%i - 1) / 16)])) error(\"refused\" SPC getParentError());\n\
            if (%i % 3 == 0) $Test::Node[%i].setDeleteWithParent(1);\n\
         }",
    );
    let h = hierarchy(&s);
    assert_eq!(h.len(), 4096);
    assert_eq!(s.vm().objects().len(), before + 4096);
    let root = id(&mut s, "return $Test::Node[0];");
    assert_eq!(
        eval(&mut s, &format!("return {root}.deleteRecursive();")),
        "4096"
    );
    assert_eq!(
        s.vm().objects().len(),
        before,
        "objects of the subtree left"
    );
    assert!(h.is_empty(), "{} links left", h.len());
    h.audit().expect("no dangling link");
    s.tick();
}

#[test]
fn flagged_children_go_with_their_parent() {
    let mut s = server(&[]);
    let root = id(&mut s, "return marker(\"0 0 0\");");
    let mut flagged = Vec::new();
    let mut kept = Vec::new();
    for i in 0..32 {
        let c = id(&mut s, &format!("return marker(\"{i} 1 0\");"));
        assert_eq!(eval(&mut s, &format!("return {c}.setParent({root});")), "1");
        // Each child has a child of its own, flagged: it goes with its parent whatever happens to that one.
        let g = id(&mut s, &format!("return marker(\"{i} 2 0\");"));
        assert_eq!(eval(&mut s, &format!("return {g}.setParent({c});")), "1");
        eval(&mut s, &format!("{g}.setDeleteWithParent(1);"));
        if i % 2 == 0 {
            eval(&mut s, &format!("{c}.setDeleteWithParent(1);"));
            flagged.push((c, g));
        } else {
            kept.push((c, g));
        }
    }
    eval(&mut s, &format!("{root}.delete();"));
    for (c, g) in flagged {
        assert!(!s.vm().objects().contains(c) && !s.vm().objects().contains(g));
    }
    let h = hierarchy(&s);
    for (c, g) in &kept {
        assert!(s.vm().objects().contains(*c) && s.vm().objects().contains(*g));
        assert_eq!(h.parent(*c), None);
        assert_eq!(h.parent(*g).map(|p| p.object.0), Some(*c));
    }
    assert_eq!(h.len(), 32, "the kept children and their own children");
    h.audit().expect("no dangling link");
}

// ------------------------------------------------------------------------------------------------- a child of its own

/// A Player hung on a shapeless platform 2 units out and 1 up, set sliding; the platform moved each tick by `platform`.
/// The Player's local poses, tick by tick.
fn player_run(platform: impl Fn(u32) -> ([f32; 3], f32)) -> Vec<[f32; 3]> {
    let mut s = server(&[]);
    let base = id(&mut s, "return marker(\"0 0 0\");");
    let player = id(
        &mut s,
        "return new Player() { datablock = FigureData; position = \"2 0 1\"; };",
    );
    assert_eq!(
        eval(&mut s, &format!("return {player}.setParent({base});")),
        "1"
    );
    eval(&mut s, &format!("{player}.setVelocity(\"0 3 0\");"));
    let mut locals = Vec::new();
    for tick in 0..200 {
        let (position, yaw) = platform(tick);
        eval(
            &mut s,
            &format!("{base}.setTransform(\"{}\");", pose_text(position, yaw)),
        );
        s.tick();
        let local = hierarchy(&s).local(player).expect("still a child");
        locals.push(local.translation);
        // The world is the parent's times the local pose.
        let expected = mul_p(&world(s.vm(), base), local.translation);
        let at = world(s.vm(), player);
        let error = (0..3)
            .map(|k| (at[3 + 4 * k] - expected[k]).abs())
            .fold(0.0, f32::max);
        assert!(error < 1e-3, "tick {tick}: world error {error}");
    }
    locals
}

#[test]
fn a_player_child_moves_in_its_parents_frame() {
    let still = player_run(|_| ([0.0; 3], 0.0));
    // It falls, lands and slides: its own simulation ran.
    assert!(still[199][2] < 0.5, "it did not land: {:?}", still[199]);
    assert!(still[199][1] > 0.5, "it did not slide: {:?}", still[199]);
    let moving = player_run(|t| {
        let t = t as f32;
        ([t * 0.1, t * 0.03, 0.0], t * 0.01)
    });
    let worst = still
        .iter()
        .zip(&moving)
        .map(|(a, b)| (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max))
        .fold(0.0, f32::max);
    assert!(
        worst < 2e-3,
        "its local path differs by {worst} on a moving parent"
    );
}

// ------------------------------------------------------------------------------------------------- where it is not

#[test]
fn torques_protocol_leaves_the_plugin_out() {
    let s = server_in(ProtocolKind::Tge, &[]);
    assert!(ServerHierarchy::of(s.vm()).is_none());
    assert!(
        !s.vm()
            .native_signatures()
            .iter()
            .any(|(_, name, _, _)| name.eq_ignore_ascii_case("setParent"))
    );
}

#[test]
fn the_natives_are_the_servers_alone() {
    let engine = EngineBuilder::new()
        .add_plugins(HierarchyPlugin::always())
        .build()
        .expect("the engine builds");
    let has = |vm: &Vm| {
        vm.native_signatures()
            .iter()
            .any(|(ns, name, _, _)| ns == "SceneObject" && name == "setParent")
    };
    let mut client = Vm::new();
    for &stage in ClientStage::ALL {
        engine
            .replay(stage, &mut client)
            .expect("the client stages");
    }
    assert!(!has(&client), "a client has setParent");
    assert!(ServerHierarchy::of(&client).is_none());
    let mut server = Vm::new();
    for &stage in ServerStage::ALL {
        engine
            .replay(stage, &mut server)
            .expect("the server stages");
    }
    assert!(has(&server));
}
