//! Props in the scene hierarchy, on a real server stepped by hand: a prop on a moving object is driven to where the tree puts
//! it, a prop on a dynamic prop is part of its body and never shifts in its frame, an unparented prop leaves at its parent's point
//! velocity, and what must not be parented (a prop with a joint) is refused.

use std::net::{Ipv4Addr, SocketAddr};

use torque_app_server::plugins::{MinimalPlugins, set_protocol};
use torque_app_server::server::{GameServer, Listen, ServerStart};
use torque_collide::math::Matrix;
use torque_engine::ProtocolKind;
use torque_physics_rapier::BodyId;
use torque_plugin_hierarchy::ServerHierarchy;
use torque_plugin_props::world::Link;
use torque_plugin_props::{PropsPlugin, ServerProps, quant};
use torque_ts_vm::host::MemFiles;

const TICKS: u32 = 600;

const SERVER_CS: &str = r#"
datablock StaticShapeData(MarkerData)
{
};

datablock StaticShapeData(FloorData)
{
   shapeFile = "kit/cube.dts";
};

datablock PropData(Crate)
{
   size = "1 1 1";
   mass = 20;
   friction = 0.7;
   sleepLinear = 0.05;
   sleepAngular = 0.05;
};

function startTest()
{
   // A floor 400 units square with its top at z = 0.
   new StaticShape(Floor) { datablock = FloorData; position = "0 0 -0.5"; scale = "400 400 1"; };
}
"#;

struct Rig {
    server: GameServer,
    props: ServerProps,
}

impl Rig {
    fn new() -> Rig {
        let mut builder = torque_engine::EngineBuilder::new();
        builder.add_plugins(MinimalPlugins).add_plugins(PropsPlugin);
        set_protocol(&mut builder, ProtocolKind::Engine);
        let engine = builder.build().expect("the engine builds");
        let props = engine.resource::<ServerProps>().expect("the props").clone();
        let files = MemFiles::new();
        files.insert("server.cs", SERVER_CS.as_bytes());
        files.insert(
            "kit/cube.dts",
            &torque_shapes::write::write_dts(&torque_example_assets::shapes::cube()),
        );
        let server = GameServer::start_engine(
            &engine,
            files,
            &ServerStart {
                listen: Listen::Memory(SocketAddr::from((Ipv4Addr::LOCALHOST, 28_000))),
                globals: &[],
                start: "startTest",
                args: &[],
            },
        )
        .expect("the server starts");
        // The first tick covers the start-up (32 steps at once): the tests' own ticks are one step each.
        let mut rig = Rig { server, props };
        rig.server.tick();
        rig
    }

    fn eval(&mut self, script: &str) -> String {
        let out = self
            .server
            .vm_mut()
            .eval(script.as_bytes())
            .unwrap_or_else(|e| panic!("{script}: {e:?}"));
        String::from_utf8_lossy(&out).into_owned()
    }

    fn id(&mut self, script: &str) -> u32 {
        self.eval(script)
            .parse()
            .unwrap_or_else(|_| panic!("{script} gave no id"))
    }

    fn marker(&mut self, at: &str) -> u32 {
        self.id(&format!(
            "return new StaticShape() {{ datablock = MarkerData; position = \"{at}\"; }};"
        ))
    }

    fn prop(&mut self, at: &str) -> u32 {
        self.id(&format!("return propSpawn(Crate, \"{at}\");"))
    }

    fn hierarchy(&self) -> ServerHierarchy {
        ServerHierarchy::of(self.server.vm()).expect("the hierarchy")
    }

    fn with_world<R>(&self, f: impl FnOnce(&torque_plugin_props::PropWorld) -> R) -> R {
        self.props
            .with(|s| f(s.world.as_ref().expect("the props world")))
            .expect("the props are not being changed")
    }

    fn link(&self, id: u32) -> Link {
        self.with_world(|w| w.link(id)).expect("a prop")
    }

    fn body(&self, id: u32) -> torque_physics_rapier::BodyState {
        self.with_world(|w| w.physics().body(BodyId(id)))
            .expect("a body")
    }

    fn refusal(&mut self, script: &str) -> String {
        let result = self.eval(&format!("return {script};"));
        assert_eq!(result, "0", "{script} was not refused");
        self.eval("return getParentError();")
    }
}

/// The platform's pose at tick `t`, a slide and a turn by whole steps.
fn platform_pose(t: u32) -> String {
    let t = t as f32;
    format!("{} {} 5 0 0 1 {}", t * 0.1, t * 0.03, t * 0.01)
}

fn translation(m: &Matrix) -> [f32; 3] {
    [m[3], m[7], m[11]]
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

fn speed(v: [f32; 3]) -> f32 {
    distance(v, [0.0; 3])
}

#[test]
fn a_kinematic_child_is_within_a_millimeter_of_its_resolved_pose_every_tick() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let child = rig.prop("1 0 5");
    let other = rig.prop("-1 0 5.5");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({platform});")),
        "1"
    );
    let mut good = 0;
    let mut worst = 0.0f32;
    for t in 0..TICKS {
        rig.eval(&format!(
            "{platform}.setTransform(\"{}\");",
            platform_pose(t)
        ));
        rig.server.tick();
        let world = rig.hierarchy().world(child).expect("the child's world");
        let body = rig.body(child);
        // The body's pose as a transform, against the tree's.
        let at = quant::matrix(body.position, body.rotation);
        let off = (0..16)
            .map(|k| (at[k] - world[k]).abs())
            .fold(0.0f32, f32::max);
        worst = worst.max(off);
        good += usize::from(off < 1e-3 && rig.link(child).is_child());
    }
    println!("driven child: {good} of {TICKS} ticks within 1 mm, worst {worst}");
    assert_eq!(
        good, TICKS as usize,
        "ticks the body was where the tree puts it (worst {worst})"
    );
    // It is driven, not asleep, and the prop that is no child fell to the floor on its own.
    assert!(matches!(rig.link(child), Link::Driven { .. }));
    assert_eq!(rig.eval(&format!("return {child}.asleep;")), "0");
    assert!(rig.body(other).position[2] < 1.0);
}

#[test]
fn a_kinematic_child_pushes_a_dynamic_prop_and_a_prop_resting_on_it_is_carried() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 0.25");
    let slab = rig.prop("0 0 0.5");
    let rider = rig.prop("0 0 1.6");
    assert_eq!(
        rig.eval(&format!("return {slab}.setParent({platform});")),
        "1"
    );
    for t in 0..300 {
        rig.eval(&format!(
            "{platform}.setTransform(\"{} 0 0.25\");",
            t as f32 * 0.02
        ));
        rig.server.tick();
    }
    let slab_at = translation(&rig.hierarchy().world(slab).expect("the slab"));
    let rider_at = rig.body(rider).position;
    assert!(
        slab_at[0] > 5.5,
        "the slab went with its parent: {slab_at:?}"
    );
    assert!(
        (rider_at[0] - slab_at[0]).abs() < 0.6 && rider_at[2] > 0.9,
        "the prop on the slab was carried: {rider_at:?} on {slab_at:?}"
    );
}

#[test]
fn a_compound_childs_local_pose_never_drifts_under_load() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 3");
    let child = rig.prop("1.2 0 3");
    assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
    rig.server.tick();
    assert!(matches!(rig.link(child), Link::Part { root: r, .. } if r == root));
    let local = rig.hierarchy().local(child).expect("a local pose");
    let first = rig
        .with_world(|w| w.physics().part(BodyId(child)))
        .expect("a part");
    let single = 20.0;
    let mass = rig
        .with_world(|w| w.physics().mass_of(BodyId(root)))
        .expect("a mass");
    assert!(mass > 1.9 * single, "both masses are the body's: {mass}");
    let mut bits_equal = 0;
    let mut tumbled = false;
    for t in 0..TICKS {
        // Crates dropped on it from a height and a shove now and then.
        if t % 50 == 10 {
            // Over the child's end of the pair, a little off its middle each time.
            let at = rig.body(root).position;
            let x = at[0] + 1.3 + (t / 50 % 3) as f32 * 0.15;
            let y = at[1] + 0.2 * (t / 50 % 2) as f32;
            rig.eval(&format!("propSpawn(Crate, \"{x} {y} 4\");"));
        }
        if t % 120 == 60 {
            rig.eval(&format!("propApplyImpulse({root}, \"260 40 90\");"));
        }
        rig.server.tick();
        let tree = rig.hierarchy().local(child).expect("a local pose");
        let part = rig
            .with_world(|w| w.physics().part(BodyId(child)))
            .expect("a part");
        let same = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits());
        if same(&tree.translation, &local.translation)
            && same(&tree.rotation, &local.rotation)
            && same(&part.position, &first.position)
            && same(&part.rotation, &first.rotation)
        {
            bits_equal += 1;
        }
        let r = rig.body(root);
        tumbled |= speed(r.angvel) > 0.2;
    }
    println!("part: local pose bit for bit on {bits_equal} of {TICKS} ticks");
    assert_eq!(
        bits_equal, TICKS as usize,
        "ticks the local pose was bit for bit the first"
    );
    assert!(
        tumbled,
        "the shoves and the falling crates spun the body, so the check was under load"
    );
    // The child is where its root's frame puts it, as the tree resolved it.
    let world = rig.hierarchy().world(child).expect("the child's world");
    let at = rig
        .with_world(|w| w.physics().part_world(BodyId(child)))
        .expect("a part");
    assert!(distance(translation(&world), at.position) < 5e-3);
}

/// A root dropped on the floor with a child at its side, a sleeping pair drawn as it rests.
#[test]
fn a_child_of_a_sleeping_root_is_never_drawn_as_resting_and_the_root_is() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 0.5");
    let child = rig.prop("1.0 0 0.5");
    assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
    for _ in 0..200 {
        rig.server.tick();
    }
    assert_eq!(rig.eval(&format!("return {root}.asleep;")), "1");
    assert_eq!(rig.eval(&format!("return {child}.asleep;")), "0");
    assert_eq!(rig.eval(&format!("return {child}.frozen;")), "0");
    // A prop that came to rest by itself and is then hung on something is awake and not resting from then on.
    let platform = rig.marker("20 0 5");
    let late = rig.prop("20 0 0.5");
    for _ in 0..200 {
        rig.server.tick();
    }
    assert_eq!(rig.eval(&format!("return {late}.asleep;")), "1");
    assert_eq!(
        rig.eval(&format!("return {late}.setParent({platform});")),
        "1"
    );
    rig.server.tick();
    assert_eq!(rig.eval(&format!("return {late}.asleep;")), "0");
    assert!(matches!(rig.link(late), Link::Driven { .. }));
}

/// A platform moving along x at `v` units a second, from `x0`.
fn slide(t: u32, v: f32) -> String {
    format!("{} 0 5 0 0 1 0", 1.0 + t as f32 * v * 0.032)
}

#[test]
fn an_unparented_prop_leaves_at_its_parents_point_velocity() {
    let mut rig = Rig::new();
    let platform = rig.marker("1 0 5");
    let child = rig.prop("1 2 5");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({platform});")),
        "1"
    );
    let v = 12.0;
    for t in 0..120 {
        rig.eval(&format!("{platform}.setTransform(\"{}\");", slide(t, v)));
        rig.server.tick();
    }
    assert_eq!(rig.eval(&format!("return {child}.setParent(0);")), "1");
    rig.server.tick();
    assert_eq!(rig.link(child), Link::Free);
    let leaving = rig.body(child).linvel;
    let seen = speed([leaving[0], leaving[1], 0.0]);
    println!("driven, unparented: left at {seen} against {v}");
    assert!(
        (seen - v).abs() <= 0.01 * v,
        "left at {seen} units a second, its platform moved at {v}"
    );
}

#[test]
fn a_prop_unparented_from_a_turning_parent_leaves_at_the_speed_its_place_had() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let child = rig.prop("6 0 5");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({platform});")),
        "1"
    );
    let mut before = [0.0; 3];
    let mut now = [0.0; 3];
    for t in 0..120 {
        rig.eval(&format!(
            "{platform}.setTransform(\"0 0 5 0 0 1 {}\");",
            t as f32 * 0.05
        ));
        rig.server.tick();
        before = now;
        now = translation(&rig.hierarchy().world(child).expect("the child's world"));
    }
    let expect = distance(now, before) / torque_physics_rapier::TICK;
    assert!(expect > 8.0, "the point moves fast: {expect}");
    assert_eq!(rig.eval(&format!("return {child}.setParent(0);")), "1");
    rig.server.tick();
    let v = rig.body(child).linvel;
    let seen = speed([v[0], v[1], 0.0]);
    println!("turning parent, unparented: left at {seen} against {expect}");
    assert!(
        (seen - expect).abs() <= 0.01 * expect,
        "left at {seen}, its point moved at {expect}"
    );
}

#[test]
fn a_part_unparented_leaves_at_the_speed_its_roots_body_gave_its_point() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 8");
    let child = rig.prop("1.5 0 8");
    assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
    rig.server.tick();
    rig.eval(&format!("propSetVelocity({root}, \"9 -4 0\", \"0 0 3\");"));
    for _ in 0..5 {
        rig.server.tick();
    }
    let at = rig
        .with_world(|w| w.physics().part_world(BodyId(child)))
        .expect("a part");
    let expect = rig
        .with_world(|w| w.physics().velocity_at(BodyId(root), at.position))
        .expect("a velocity");
    assert_eq!(rig.eval(&format!("return {child}.setParent(0);")), "1");
    // Looked at before the step takes gravity into it.
    rig.props
        .0
        .borrow_mut()
        .world
        .as_mut()
        .expect("the world")
        .physics_mut()
        .wake(BodyId(root))
        .expect("the root is a body");
    rig.server.tick();
    assert_eq!(rig.link(child), Link::Free);
    let v = rig.body(child).linvel;
    let want = [expect[0], expect[1], 0.0];
    let got = [v[0], v[1], 0.0];
    let want_speed = speed(want);
    println!(
        "part, unparented: left at {} against {want_speed}, {} apart",
        speed(got),
        distance(got, want)
    );
    assert!(
        (speed(got) - want_speed).abs() <= 0.01 * want_speed,
        "left at {got:?}, its point moved at {want:?}"
    );
    assert!(
        distance(got, want) <= 0.01 * want_speed,
        "left at {got:?}, its point moved at {want:?}"
    );
    // The root alone is lighter.
    let mass = rig
        .with_world(|w| w.physics().mass_of(BodyId(root)))
        .expect("a mass");
    assert!(mass < 25.0, "the part's mass went with it: {mass}");
}

#[test]
fn a_prop_with_a_joint_is_refused_as_a_child_and_a_child_takes_no_joint() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let a = rig.prop("0 0 2");
    let b = rig.prop("2 0 2");
    let joint = rig.id(&format!("return jointTether(\"{a}\", \"{b}\", 5);"));
    assert!(joint > 0);
    assert_eq!(rig.refusal(&format!("{a}.setParent({platform})")), "joints");
    // Nothing changed: it has no parent and its joint is whole.
    assert_eq!(rig.eval(&format!("return {a}.getParent();")), "0");
    assert_eq!(rig.link(a), Link::Free);
    assert_eq!(rig.with_world(|w| w.joints_on(a)), 1);
    // A hold is a joint too.
    let c = rig.prop("-3 0 2");
    assert_eq!(rig.eval(&format!("return {c}.setParent({platform});")), "1");
    rig.server.tick();
    assert!(rig.link(c).is_child());
    let result = rig.eval(&format!("return jointTether(\"{c}\", \"{b}\", 5);"));
    assert!(
        matches!(result.as_str(), "" | "0"),
        "a child took a joint: {result}"
    );
    assert_eq!(rig.with_world(|w| w.joints_on(c)), 0);
    // A prop that carries children may have joints.
    let d = rig.prop("0 4 2");
    let e = rig.prop("0 5 2");
    assert_eq!(rig.eval(&format!("return {e}.setParent({d});")), "1");
    rig.server.tick();
    assert!(rig.id(&format!("return jointTether(\"{d}\", \"{b}\", 9);")) > 0);
}

#[test]
fn freezing_a_root_turns_its_parts_into_driven_props_and_thawing_it_takes_them_back() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 0.5");
    let child = rig.prop("1 0 0.5");
    assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
    for _ in 0..3 {
        rig.server.tick();
    }
    assert!(matches!(rig.link(child), Link::Part { .. }));
    assert_eq!(rig.eval(&format!("return propFreeze({root});")), "1");
    rig.server.tick();
    assert!(matches!(rig.link(child), Link::Driven { .. }));
    assert_eq!(rig.with_world(|w| w.physics().part_ids().count()), 0);
    assert_eq!(rig.eval(&format!("return propUnfreeze({root});")), "1");
    rig.server.tick();
    rig.server.tick();
    assert!(matches!(rig.link(child), Link::Part { root: r, .. } if r == root));
    // A child itself cannot be frozen, slept or given a speed.
    assert_eq!(rig.eval(&format!("return propFreeze({child});")), "0");
    assert_eq!(rig.eval(&format!("return propSleep({child});")), "0");
    assert_eq!(
        rig.eval(&format!("return propSetVelocity({child}, \"1 0 0\");")),
        "0"
    );
}

#[test]
fn deleting_a_root_sets_its_parts_free_where_they_are_and_a_deleted_part_leaves_its_root_whole() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 4");
    let one = rig.prop("1.2 0 4");
    let two = rig.prop("-1.2 0 4");
    for child in [one, two] {
        assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
    }
    for _ in 0..5 {
        rig.server.tick();
    }
    let before = rig
        .with_world(|w| w.physics().mass_of(BodyId(root)))
        .expect("a mass");
    assert_eq!(rig.eval(&format!("return propDelete({two});")), "1");
    rig.server.tick();
    let after = rig
        .with_world(|w| w.physics().mass_of(BodyId(root)))
        .expect("a mass");
    assert!(
        after < before,
        "a deleted part took its mass: {before} to {after}"
    );
    let at = translation(&rig.hierarchy().world(one).expect("the child's world"));
    assert_eq!(rig.eval(&format!("return propDelete({root});")), "1");
    rig.server.tick();
    assert_eq!(rig.link(one), Link::Free);
    let now = rig.body(one).position;
    assert!(
        distance(now, at) < 1.0,
        "the part went on from where it was: {now:?} from {at:?}"
    );
    rig.hierarchy().audit().expect("no dangling link");
}

#[test]
fn a_stopped_physics_world_is_made_again_with_its_parts_and_driven_props() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let root = rig.prop("0 0 0.5");
    let part = rig.prop("1 0 0.5");
    let driven = rig.prop("0 3 5");
    assert_eq!(rig.eval(&format!("return {part}.setParent({root});")), "1");
    assert_eq!(
        rig.eval(&format!("return {driven}.setParent({platform});")),
        "1"
    );
    for _ in 0..3 {
        rig.server.tick();
    }
    rig.props
        .0
        .borrow_mut()
        .world
        .as_mut()
        .expect("the world")
        .physics_mut()
        .poison();
    for _ in 0..3 {
        rig.server.tick();
    }
    assert!(!rig.with_world(|w| w.physics().is_poisoned()));
    assert!(matches!(rig.link(part), Link::Part { .. }));
    assert!(matches!(rig.link(driven), Link::Driven { .. }));
    assert_eq!(rig.with_world(|w| w.physics().part_ids().count()), 1);
    assert!(
        rig.with_world(|w| w.physics().body(BodyId(driven)))
            .is_some()
    );
}

#[test]
fn a_chain_of_props_and_other_objects_places_each_prop_by_its_own_parent() {
    let mut rig = Rig::new();
    let root = rig.prop("0 0 0.5");
    let mid = rig.prop("1 0 0.5");
    let tip = rig.prop("2 0 0.5");
    let marker = rig.marker("0 6 5");
    let under_marker = rig.prop("0 6 4");
    assert_eq!(rig.eval(&format!("return {mid}.setParent({root});")), "1");
    assert_eq!(rig.eval(&format!("return {tip}.setParent({mid});")), "1");
    assert_eq!(
        rig.eval(&format!("return {under_marker}.setParent({marker});")),
        "1"
    );
    // A prop on a non-prop that hangs on a prop is driven (its parent is not a prop).
    let light = rig.marker("-2 0 0.5");
    assert_eq!(rig.eval(&format!("return {light}.setParent({root});")), "1");
    let on_light = rig.prop("-2 0 1.5");
    assert_eq!(
        rig.eval(&format!("return {on_light}.setParent({light});")),
        "1"
    );
    for _ in 0..30 {
        rig.server.tick();
    }
    assert!(matches!(rig.link(mid), Link::Part { root: r, .. } if r == root));
    assert!(matches!(rig.link(tip), Link::Part { root: r, .. } if r == root));
    assert!(matches!(rig.link(under_marker), Link::Driven { .. }));
    assert!(matches!(rig.link(on_light), Link::Driven { .. }));
    // The tip's pose in the root's frame is the two links' product: 2 units along x.
    let local = rig
        .with_world(|w| w.physics().part(BodyId(tip)))
        .expect("a part");
    assert!((local.position[0] - 2.0).abs() < 1e-3, "{local:?}");
    // The chain follows the root when it is shoved: the tip stays 2 units from it.
    rig.eval(&format!("propApplyImpulse({root}, \"600 0 0\");"));
    for _ in 0..20 {
        rig.server.tick();
    }
    let root_at = rig.body(root).position;
    let tip_at = translation(&rig.hierarchy().world(tip).expect("the tip"));
    assert!(root_at[0] > 1.0, "the root moved: {root_at:?}");
    assert!(
        (distance(root_at, tip_at) - 2.0).abs() < 0.05,
        "{root_at:?} {tip_at:?}"
    );
}

#[test]
fn a_stable_tree_of_nine_hundred_parts_costs_no_fit_a_tick_and_one_change_costs_one() {
    let mut rig = Rig::new();
    // Nine bodies on the floor, a hundred crates hung on each (the hierarchy limits a parent to 256 children).
    rig.eval(
        "for (%r = 0; %r < 9; %r++) { \
            %root = propSpawn(Crate, (%r * 4) SPC 0 SPC 0.5); \
            for (%i = 0; %i < 100; %i++) { \
                %part = propSpawn(Crate, (%r * 4) SPC 0 SPC (1.5 + %i)); \
                if (!%part.setParent(%root)) return 0; \
                $Parts[%r * 100 + %i] = %part; \
            } \
        }",
    );
    assert_eq!(rig.eval("return propCount();"), "909");
    for _ in 0..40 {
        rig.server.tick();
    }
    let parts = rig.with_world(|w| w.props().values().filter(|p| p.link.is_child()).count());
    assert_eq!(parts, 900);
    let before = rig.props.link_stats().expect("stats");
    for _ in 0..30 {
        rig.server.tick();
    }
    let stable = rig.props.link_stats().expect("stats");
    println!("stable: {before:?} to {stable:?}");
    assert_eq!(
        stable, before,
        "a tree that did not change was looked at again"
    );
    // One local pose set: one part fitted, no full look.
    rig.eval("$Parts[450].setLocalTransform(\"0.5 0 3 0 0 1 0.2\");");
    rig.server.tick();
    let changed = rig.props.link_stats().expect("stats");
    println!("one change: {changed:?}");
    assert_eq!(changed.full, stable.full);
    assert!(
        changed.fitted - stable.fitted <= 2,
        "{} props fitted for one change",
        changed.fitted - stable.fitted
    );
}

#[test]
fn a_prop_that_was_frozen_when_it_was_hung_is_frozen_again_when_let_go() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let frozen = rig.prop("0 0 0.5");
    let loose = rig.prop("3 0 0.5");
    assert_eq!(rig.eval(&format!("return propFreeze({frozen});")), "1");
    for child in [frozen, loose] {
        assert_eq!(
            rig.eval(&format!("return {child}.setParent({platform});")),
            "1"
        );
    }
    rig.server.tick();
    assert_eq!(rig.eval(&format!("return propIsFrozen({frozen});")), "0");
    for child in [frozen, loose] {
        assert_eq!(rig.eval(&format!("return {child}.setParent(0);")), "1");
    }
    rig.server.tick();
    assert_eq!(rig.link(frozen), Link::Free);
    assert_eq!(rig.eval(&format!("return propIsFrozen({frozen});")), "1");
    assert_eq!(rig.eval(&format!("return {frozen}.frozen;")), "1");
    assert_eq!(rig.eval(&format!("return propIsFrozen({loose});")), "0");
    let before = rig.body(frozen).position;
    for _ in 0..30 {
        rig.server.tick();
    }
    assert_eq!(
        rig.body(frozen).position,
        before,
        "a frozen prop holds still"
    );
}

#[test]
fn a_part_moved_to_another_root_wakes_the_root_it_left_as_well_as_the_new_one() {
    let mut rig = Rig::new();
    let a = rig.prop("0 0 0.5");
    let part = rig.prop("1 0 0.5");
    let b = rig.prop("10 0 0.5");
    assert_eq!(rig.eval(&format!("return {part}.setParent({a});")), "1");
    for _ in 0..200 {
        rig.server.tick();
    }
    assert_eq!(rig.eval(&format!("return {a}.asleep;")), "1");
    assert_eq!(rig.eval(&format!("return {b}.asleep;")), "1");
    assert_eq!(rig.eval(&format!("return {part}.setParent({b});")), "1");
    rig.server.tick();
    assert!(matches!(rig.link(part), Link::Part { root, .. } if root == b));
    assert_eq!(
        rig.eval(&format!("return {a}.asleep;")),
        "0",
        "the root it left"
    );
    assert_eq!(
        rig.eval(&format!("return {b}.asleep;")),
        "0",
        "the root it joined"
    );
}

#[test]
fn a_body_hung_on_something_takes_its_parts_with_it_as_driven_props() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let root = rig.prop("0 0 3");
    let part = rig.prop("1 0 3");
    assert_eq!(rig.eval(&format!("return {part}.setParent({root});")), "1");
    rig.server.tick();
    assert!(matches!(rig.link(part), Link::Part { .. }));
    assert_eq!(
        rig.eval(&format!("return {root}.setParent({platform});")),
        "1"
    );
    for _ in 0..3 {
        rig.server.tick();
    }
    assert!(matches!(rig.link(root), Link::Driven { .. }));
    assert!(matches!(rig.link(part), Link::Driven { .. }));
    assert_eq!(rig.with_world(|w| w.physics().part_ids().count()), 0);
    assert!(rig.with_world(|w| w.physics().body(BodyId(part))).is_some());
    // Let go, the root is a body again and the part joins it.
    assert_eq!(rig.eval(&format!("return {root}.setParent(0);")), "1");
    for _ in 0..3 {
        rig.server.tick();
    }
    assert_eq!(rig.link(root), Link::Free);
    assert!(matches!(rig.link(part), Link::Part { .. }));
}

#[test]
fn deleting_the_parts_of_a_large_body_and_then_the_body_costs_in_proportion() {
    use std::time::Instant;
    use torque_plugin_props::data::PropNumbers;
    use torque_plugin_props::world::{PropLimits, PropWorld};

    // The time to attach `n` parts to a body and delete them one by one, then `n` more and the body.
    fn run(n: u32) -> std::time::Duration {
        let params = PropNumbers::default().held();
        let mut w = PropWorld::new(1, PropLimits::default());
        let spawn = |w: &mut PropWorld, id: u32| {
            w.spawn(
                id,
                9,
                params,
                ([0.0, 0.0, 50.0], [0.0, 0.0, 0.0, 1.0]),
                None,
                0,
            )
            .expect("a prop");
        };
        spawn(&mut w, 1);
        let started = Instant::now();
        for id in 2..2 + n {
            spawn(&mut w, id);
            let at = [(id % 40) as f32, (id / 40) as f32, 0.0];
            assert!(w.attach(id, 1, (at, [0.0, 0.0, 0.0, 1.0])));
        }
        for id in 2..2 + n {
            assert!(w.remove(id));
        }
        for id in 2..2 + n {
            spawn(&mut w, id);
            assert!(w.attach(id, 1, ([id as f32, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0])));
        }
        assert!(w.remove(1));
        assert_eq!(w.parts_of(1).len(), 0);
        started.elapsed()
    }
    let best = |n: u32| (0..3).map(|_| run(n)).min().expect("a run");
    let (small, large) = (best(250), best(1000));
    let ratio = large.as_secs_f64() / small.as_secs_f64();
    println!("250 parts {small:?}, 1000 parts {large:?}, ratio {ratio:.1}");
    // Four times the parts: linear work is about 4 times the time, a scan of every part for each one about 16.
    assert!(
        ratio < 9.0,
        "the time grew {ratio} times for 4 times the parts"
    );
}

#[test]
fn fits_refused_for_the_budget_land_on_later_ticks() {
    let mut rig = Rig::new();
    // 1,000 units a tick is room for about 160 bodies made: 400 hung props take a few ticks, and so do 400 let go.
    rig.eval("$Pref::Props::StepBudget = 1000;");
    let a = rig.marker("0 0 10");
    let b = rig.marker("0 20 10");
    rig.eval(&format!(
        "for (%i = 0; %i < 400; %i++) {{ \
            $P[%i] = propSpawn(Crate, (%i % 20) * 2 SPC mFloor(%i / 20) * 2 + 4 SPC 12); \
            if (!$P[%i].setParent(%i < 200 ? {a} : {b})) return 0; \
        }}"
    ));
    let driven =
        |rig: &Rig| rig.with_world(|w| w.props().values().filter(|p| p.link.is_child()).count());
    rig.server.tick();
    let first = driven(&rig);
    assert!(first > 0 && first < 400, "{first} hung in the first tick");
    let mut ticks = 1;
    while driven(&rig) < 400 && ticks < 20 {
        rig.server.tick();
        ticks += 1;
    }
    assert_eq!(driven(&rig), 400, "after {ticks} ticks");
    println!("400 hung: {first} in the first tick, all in {ticks}");
    rig.eval("for (%i = 0; %i < 400; %i++) { $P[%i].setParent(0); }");
    rig.server.tick();
    let left = driven(&rig);
    assert!(
        left > 0 && left < 400,
        "{left} still hung after the first tick"
    );
    let mut ticks = 1;
    while driven(&rig) > 0 && ticks < 20 {
        rig.server.tick();
        ticks += 1;
    }
    assert_eq!(driven(&rig), 0, "after {ticks} ticks");
}

#[test]
fn a_script_changing_a_large_tree_many_times_in_a_tick_costs_one_bounded_look() {
    let mut rig = Rig::new();
    // A root, a top object, sixteen mid objects of 250 leaves each (4,017 below the top), and a prop on the top.
    rig.eval(
        "$Root = new StaticShape() { datablock = MarkerData; position = \"0 0 10\"; }; \
         $Top = new StaticShape() { datablock = MarkerData; position = \"0 0 10\"; }; \
         if (!$Top.setParent($Root)) return 0; \
         for (%i = 0; %i < 16; %i++) { \
            $Mid[%i] = new StaticShape() { datablock = MarkerData; position = \"0 0 10\"; }; \
            if (!$Mid[%i].setParent($Top)) return 0; \
            for (%j = 0; %j < 250; %j++) { \
               %l = new StaticShape() { datablock = MarkerData; position = \"0 0 10\"; }; \
               if (!%l.setParent($Mid[%i])) return 0; \
            } \
         }",
    );
    let rider = rig.prop("0 0 10.5");
    assert_eq!(rig.eval(&format!("return {rider}.setParent($Top);")), "1");
    for _ in 0..5 {
        rig.server.tick();
    }
    let before = rig.props.link_stats().expect("stats");
    // Two objects changed 200 times each, alternately, in one tick.
    rig.eval(
        "for (%i = 0; %i < 200; %i++) { \
            $Top.setLocalTransform(\"0 \" @ (%i * 0.001) @ \" 0\"); \
            $Mid[0].setLocalTransform(\"0 0 \" @ (%i * 0.001)); \
         }",
    );
    rig.server.tick();
    let after = rig.props.link_stats().expect("stats");
    let looked = after.visited - before.visited;
    println!("400 changes of a 4,000-object subtree: {before:?} to {after:?}");
    assert!(
        looked <= 64,
        "{looked} objects looked at for 400 changes of a large tree"
    );
    assert!(after.fitted - before.fitted <= 1);
    assert!(matches!(rig.link(rider), Link::Driven { .. }));
}

#[test]
fn a_parent_set_while_the_physics_world_is_stopped_lands_after_its_rebuild() {
    let mut rig = Rig::new();
    let platform = rig.marker("0 0 5");
    let root = rig.prop("0 0 0.5");
    let part = rig.prop("1 0 0.5");
    let driven = rig.prop("6 0 0.5");
    for _ in 0..3 {
        rig.server.tick();
    }
    rig.props
        .0
        .borrow_mut()
        .world
        .as_mut()
        .expect("the world")
        .physics_mut()
        .poison();
    assert_eq!(rig.eval(&format!("return {part}.setParent({root});")), "1");
    assert_eq!(
        rig.eval(&format!("return {driven}.setParent({platform});")),
        "1"
    );
    for _ in 0..4 {
        rig.server.tick();
    }
    assert!(!rig.with_world(|w| w.physics().is_poisoned()));
    assert!(matches!(rig.link(part), Link::Part { root: r, .. } if r == root));
    assert!(matches!(rig.link(driven), Link::Driven { .. }));
    assert_eq!(rig.with_world(|w| w.physics().part_ids().count()), 1);
}
