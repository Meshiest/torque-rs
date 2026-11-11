//! Live datablocks on a server: an object built from a datablock that is then edited in place (its fields written, or the
//! datablock declared again) is refreshed at the next tick and equals an object built fresh from the edited datablock, field
//! for field; the refresh is coalesced (one light refresh per user per tick for any number of writes, one heavy refresh per
//! user for any number of redeclares, no shape rebuild for a write), script threads keep their sequence and position, and a
//! drain scans nothing while no object uses a dirty datablock. The vehicles' rigid body and wheel records follow their
//! datablock the same way; the add-time checks of the effect and audio datablocks run again on a redeclare; a Projectile
//! whose datablock is deleted deletes itself.

use crate::net_client::{self, NetServer, eval};

use torque_game::scene::{ext, refresh};
use torque_game::sim::{item, player, vehicle, vitals};
use torque_server::server::TICK_MS;
use torque_shapes::synth::Rng;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;
use torque_vehicle::data::WheelRecord;

/// The shapes: boxes of three sizes, each with two sequences a thread can play.
const SHAPES: [(&str, f32); 3] = [("a.dts", 1.0), ("b.dts", 2.0), ("c.dts", 0.5)];

/// The wheeled vehicle's shape: a buggy with four hub nodes.
const BUGGY: &str = "buggy.dts";

fn server() -> NetServer {
    let files = MemFiles::new();
    for (name, half) in SHAPES {
        let b = [-half, -half, 0.0, half, half, half];
        let shape =
            torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient", "spin"]);
        files.insert(name, &torque_shapes::write::write_dts(&shape));
    }
    files.insert(
        BUGGY,
        &torque_shapes::write::write_dts(&torque_example_assets::buggy::buggy()),
    );
    net_client::game_server(files).0
}

fn tick(s: &mut NetServer) {
    s.run(TICK_MS, |_| false);
}

fn id_of(s: &NetServer, name: &str) -> ObjId {
    s.vm.objects().find(name.as_bytes()).expect(name)
}

fn answer(s: &mut NetServer, code: &str) -> String {
    String::from_utf8(s.vm.eval(code.as_bytes()).unwrap()).unwrap()
}

fn logged(s: &mut NetServer) -> Vec<String> {
    s.vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

/// How many shape instances the object's `onNewDataBlock` made so far.
fn instances(s: &NetServer, id: ObjId) -> u32 {
    ext::get(s.vm.objects(), id)
        .and_then(|e| e.shape.as_deref())
        .map_or(0, |st| st.instances)
}

/// The words a Player copies from its datablock.
#[derive(Debug, PartialEq)]
struct PlayerWords {
    mass: f32,
    drag: f32,
    speeds: [f32; 6],
    recharge_rate: f32,
    obj_box: ([f32; 3], [f32; 3]),
    scaled_box: ([f32; 3], [f32; 3]),
    type_mask: u32,
    info: std::rc::Rc<player::PlayerInfo>,
}

/// A box as its two corners.
type Corners = ([f32; 3], [f32; 3]);

/// Everything an object holds that comes from its datablock: the differential oracle compares two of these.
#[derive(Debug, PartialEq)]
struct Snapshot {
    object_box: Corners,
    type_mask: u32,
    shape_bounds: Option<[f32; 6]>,
    shape_sequences: Option<usize>,
    recharge_rate: f32,
    player: Option<PlayerWords>,
    item: Option<(u32, Corners)>,
}

fn snapshot(s: &NetServer, id: ObjId) -> Snapshot {
    let e = ext::get(s.vm.objects(), id).expect("the object's state");
    let scene = e.scene.expect("a scene object");
    let shape = e.shape.as_deref().and_then(|st| st.body.as_ref());
    let player = player::state(&s.vm, id).map(|p| PlayerWords {
        mass: p.mass,
        drag: p.drag,
        speeds: p.speeds,
        recharge_rate: p.recharge_rate,
        obj_box: (p.obj_box.min, p.obj_box.max),
        scaled_box: (p.scaled_box.min, p.scaled_box.max),
        type_mask: p.type_mask,
        info: player::info(&s.vm, id).expect("a Player's products"),
    });
    Snapshot {
        object_box: (scene.object_min, scene.object_max),
        type_mask: e.type_mask,
        shape_bounds: shape.map(|b| b.shape().bounds),
        shape_sequences: shape.map(|b| b.shape().sequences.len()),
        recharge_rate: vitals::get(&s.vm, id).recharge_rate,
        player,
        item: item::state(&s.vm, id)
            .map(|i| (i.type_mask, (i.scene.obj_box.min, i.scene.obj_box.max))),
    }
}

/// The classes the oracle covers, with the fields an edit may change.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Class {
    StaticShape,
    Item,
    Player,
}

impl Class {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Class::StaticShape => ("StaticShape", "StaticShapeData"),
            Class::Item => ("Item", "ItemData"),
            Class::Player => ("Player", "PlayerData"),
        }
    }
}

/// A seeded edit of a datablock: `shape` only for a redeclare (a written `shapeFile` loads nothing until the next redeclare).
fn random_edit(rng: &mut Rng, class: Class, redeclare: bool) -> String {
    let mut fields = Vec::new();
    if redeclare {
        fields.push(format!(
            "shapeFile = \"{}\";",
            SHAPES[rng.below(SHAPES.len())].0
        ));
    }
    fields.push(format!("mass = {};", 0.5 + rng.below(400) as f32 * 0.25));
    fields.push(format!("maxDamage = {};", 10 + rng.below(90)));
    fields.push(format!("rechargeRate = {};", rng.below(5) as f32 * 0.1));
    match class {
        Class::Player => {
            fields.push(format!("maxForwardSpeed = {};", 1 + rng.below(20)));
            fields.push(format!("maxBackwardSpeed = {};", 1 + rng.below(10)));
            fields.push(format!("maxSideSpeed = {};", 1 + rng.below(10)));
            fields.push(format!("drag = {};", rng.below(4) as f32 * 0.1));
            fields.push(format!(
                "boundingBox = \"{} {} {}\";",
                0.5 + rng.below(4) as f32 * 0.25,
                0.5 + rng.below(4) as f32 * 0.25,
                1.0 + rng.below(8) as f32 * 0.25
            ));
        }
        Class::StaticShape | Class::Item => {
            fields.push(format!("dynamicType = {};", rng.below(1 << 24)));
        }
    }
    let (_, data) = class.names();
    if redeclare {
        format!("datablock {data}(D) {{ {} }};", fields.join(" "))
    } else {
        fields
            .iter()
            .map(|f| format!("D.{f}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The oracle for one class: object A built on D, D edited `edits` times (a redeclare or writes, seeded), a tick; object B
/// built fresh on D; the two snapshots must not differ.
fn differential(class: Class, edits: usize, seed: u64) -> usize {
    let mut s = server();
    let (object, data) = class.names();
    let mut rng = Rng(seed);
    eval(
        &mut s,
        &format!(
            r#"datablock {data}(D) {{ shapeFile = "a.dts"; }};
new {object}(A) {{ dataBlock = D; position = "0 0 5"; }};"#
        ),
    );
    let a = id_of(&s, "A");
    tick(&mut s);
    let mut differing = 0;
    for i in 0..edits {
        let redeclare = rng.one_in(2);
        eval(&mut s, &random_edit(&mut rng, class, redeclare));
        tick(&mut s);
        eval(
            &mut s,
            &format!("new {object}(B) {{ dataBlock = D; position = \"0 0 5\"; }};"),
        );
        let b = id_of(&s, "B");
        tick(&mut s);
        let (at, bt) = (snapshot(&s, a), snapshot(&s, b));
        if at != bt {
            differing += 1;
            eprintln!(
                "{class:?} edit {i} (redeclare {redeclare}): refreshed {at:#?} fresh {bt:#?}"
            );
        }
        eval(&mut s, "B.delete();");
    }
    differing
}

#[test]
fn a_refreshed_static_shape_equals_a_fresh_one_over_seeded_edits() {
    assert_eq!(differential(Class::StaticShape, 200, 1), 0);
}

#[test]
fn a_refreshed_item_equals_a_fresh_one_over_seeded_edits() {
    assert_eq!(differential(Class::Item, 200, 2), 0);
}

#[test]
fn a_refreshed_player_equals_a_fresh_one_over_seeded_edits() {
    assert_eq!(differential(Class::Player, 200, 3), 0);
}

#[test]
fn ten_thousand_writes_to_a_player_data_with_a_hundred_users_rebuild_no_shape_and_refresh_each_user_once_a_tick()
 {
    let mut s = server();
    eval(
        &mut s,
        r#"datablock PlayerData(D) { shapeFile = "a.dts"; maxForwardSpeed = 4; };"#,
    );
    for i in 0..100 {
        eval(
            &mut s,
            &format!("new Player(P{i}) {{ dataBlock = D; position = \"{i} 0 5\"; }};"),
        );
    }
    tick(&mut s);
    let users: Vec<ObjId> = (0..100).map(|i| id_of(&s, &format!("P{i}"))).collect();
    let instances_before: u32 = users.iter().map(|&u| instances(&s, u)).sum();
    let builds_before = player::rebuilds();
    let before = refresh::stats(&s.vm);
    for round in 1..=3u32 {
        eval(
            &mut s,
            "for (%i = 0; %i < 10000; %i++) { D.maxForwardSpeed = 4 + (%i % 7); }",
        );
        tick(&mut s);
        let now = refresh::stats(&s.vm);
        assert_eq!(
            now.marks - before.marks,
            u64::from(round),
            "one mark a tick"
        );
        assert_eq!(
            now.light - before.light,
            100 * u64::from(round),
            "one light refresh per user a tick"
        );
        assert_eq!(now.heavy, before.heavy, "no heavy refresh for a write");
    }
    let instances_after: u32 = users.iter().map(|&u| instances(&s, u)).sum();
    assert_eq!(instances_after, instances_before, "0 shape rebuilds");
    assert_eq!(
        player::rebuilds() - builds_before,
        3,
        "the products are built once a tick, shared by every user"
    );
    assert_eq!(
        player::state(&s.vm, users[99]).map(|p| p.speeds[0]),
        Some(4.0 + (9999 % 7) as f32),
        "the last write is what every Player runs with"
    );
}

#[test]
fn a_thousand_redeclares_in_one_tick_refresh_each_user_once() {
    let mut s = server();
    eval(
        &mut s,
        r#"datablock StaticShapeData(D) { shapeFile = "a.dts"; };"#,
    );
    for i in 0..10 {
        eval(
            &mut s,
            &format!("new StaticShape(S{i}) {{ dataBlock = D; }};"),
        );
    }
    tick(&mut s);
    let users: Vec<ObjId> = (0..10).map(|i| id_of(&s, &format!("S{i}"))).collect();
    let instances_before: Vec<u32> = users.iter().map(|&u| instances(&s, u)).collect();
    let before = refresh::stats(&s.vm);
    for i in 0..1000 {
        eval(
            &mut s,
            &format!(
                "datablock StaticShapeData(D) {{ shapeFile = \"{}\"; mass = {i}; }};",
                SHAPES[i % 2].0
            ),
        );
    }
    tick(&mut s);
    let now = refresh::stats(&s.vm);
    assert_eq!(
        (now.marks - before.marks, now.drains - before.drains),
        (1, 1)
    );
    assert_eq!(now.heavy - before.heavy, 10, "one heavy refresh per user");
    assert_eq!(now.light, before.light);
    let instances_after: Vec<u32> = users.iter().map(|&u| instances(&s, u)).collect();
    assert!(
        instances_before
            .iter()
            .zip(&instances_after)
            .all(|(b, a)| *a == b + 1),
        "one shape rebuild per user: {instances_before:?} -> {instances_after:?}"
    );
    // The last redeclare's shape (b.dts, two units across) is what every user has.
    assert_eq!(answer(&mut s, "return S3.getObjectBox();"), "-2 -2 0 2 2 2");
}

#[test]
fn script_threads_keep_their_sequence_and_position_across_a_refresh() {
    let mut s = server();
    // A, on the datablock that is edited, and C, the control on an identical one that is not.
    eval(
        &mut s,
        r#"
datablock PlayerData(D) { shapeFile = "a.dts"; };
datablock PlayerData(E) { shapeFile = "a.dts"; };
new Player(A) { dataBlock = D; position = "0 0 5"; };
new Player(C) { dataBlock = E; position = "4 0 5"; };
A.playThread(0, "spin"); C.playThread(0, "spin");
A.playThread(1, "ambient"); C.playThread(1, "ambient");
"#,
    );
    let (a, c) = (id_of(&s, "A"), id_of(&s, "C"));
    for _ in 0..5 {
        tick(&mut s);
    }
    let threads = |s: &NetServer, id: ObjId| -> Vec<(Vec<u8>, f32)> {
        let st = ext::get(s.vm.objects(), id)
            .and_then(|e| e.shape.as_deref())
            .expect("a shape state");
        let body = st.body.as_ref().expect("a body");
        st.scripts
            .iter()
            .filter_map(|t| {
                let seq = usize::try_from(t.sequence).ok()?;
                let name = body.shape().names
                    [usize::try_from(body.shape().sequences[seq].name).ok()?]
                .clone();
                Some((name, body.instance.thread(t.thread?)?.pos))
            })
            .collect()
    };
    let before = threads(&s, a);
    assert_eq!(before.len(), 2, "both threads play");
    assert!(
        before.iter().all(|(_, pos)| *pos > 0.0),
        "the threads moved: {before:?}"
    );
    assert_eq!(before, threads(&s, c));
    let built = instances(&s, a);
    // A redeclare that changes the shape: the instance is rebuilt and the threads carried by name.
    eval(
        &mut s,
        r#"datablock PlayerData(D) { shapeFile = "b.dts"; mass = 50; };"#,
    );
    tick(&mut s);
    assert_eq!(instances(&s, a), built + 1);
    assert_eq!(
        threads(&s, a),
        threads(&s, c),
        "the same sequences at the same positions as the control"
    );
    assert_eq!(
        instances(&s, c),
        built,
        "the control's shape was not rebuilt"
    );
}

#[test]
fn a_drain_scans_nothing_while_no_object_uses_a_dirty_datablock() {
    let mut s = server();
    let mut decls = String::new();
    for i in 0..50 {
        decls.push_str(&format!(
            "datablock StaticShapeData(U{i}) {{ shapeFile = \"a.dts\"; }};\n"
        ));
    }
    eval(&mut s, &decls);
    tick(&mut s);
    let before = refresh::stats(&s.vm);
    for round in 0..3 {
        eval(&mut s, &decls);
        eval(&mut s, &format!("U7.mass = {round}; U8.mass = {round};"));
        tick(&mut s);
    }
    let now = refresh::stats(&s.vm);
    assert_eq!(now.scans - before.scans, 0, "0 scans");
    assert_eq!(
        (now.marks, now.drains),
        (before.marks, before.drains),
        "a datablock without users takes no mark"
    );
    assert_eq!((now.light, now.heavy), (before.light, before.heavy));

    // One user of one dirty datablock: one mark, one scan, one refresh.
    eval(&mut s, "new StaticShape(S) { dataBlock = U7; };");
    tick(&mut s);
    eval(&mut s, "U7.mass = 9; U9.mass = 9;");
    tick(&mut s);
    let now = refresh::stats(&s.vm);
    assert_eq!(
        (
            now.marks - before.marks,
            now.scans - before.scans,
            now.light - before.light
        ),
        (1, 1, 1)
    );
}

#[test]
fn on_reloaded_runs_once_per_refreshed_datablock_with_users_and_the_counts_follow_the_users() {
    let mut s = server();
    eval(
        &mut s,
        r#"
datablock StaticShapeData(D) { shapeFile = "a.dts"; };
datablock StaticShapeData(E) { shapeFile = "b.dts"; };
function D::onReloaded(%this) { $reloaded = $reloaded @ "D;"; }
function E::onReloaded(%this) { $reloaded = $reloaded @ "E;"; }
new StaticShape(S) { dataBlock = D; };
new StaticShape(T) { dataBlock = D; };
"#,
    );
    let (d, e) = (id_of(&s, "D"), id_of(&s, "E"));
    assert_eq!((refresh::users(&s.vm, d), refresh::users(&s.vm, e)), (2, 0));
    tick(&mut s);
    eval(&mut s, "D.mass = 1; D.mass = 2; E.mass = 3;");
    tick(&mut s);
    assert_eq!(
        answer(&mut s, "return $reloaded;"),
        "D;",
        "once for D, not for E (no users)"
    );

    // `setDataBlock` moves a user; a deleted object lets go of the datablock it was counted on, not the one a direct field
    // write points it at.
    eval(&mut s, "T.setDataBlock(E);");
    assert_eq!((refresh::users(&s.vm, d), refresh::users(&s.vm, e)), (1, 1));
    eval(
        &mut s,
        "new StaticShape(U) { dataBlock = D; }; U.dataBlock = E;",
    );
    assert_eq!((refresh::users(&s.vm, d), refresh::users(&s.vm, e)), (2, 1));
    eval(&mut s, "U.delete();");
    assert_eq!((refresh::users(&s.vm, d), refresh::users(&s.vm, e)), (1, 1));
    eval(&mut s, "S.delete();");
    assert_eq!((refresh::users(&s.vm, d), refresh::users(&s.vm, e)), (0, 1));
    eval(&mut s, "D.mass = 4; E.mass = 5;");
    tick(&mut s);
    assert_eq!(answer(&mut s, "return $reloaded;"), "D;E;");
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

#[test]
fn a_datablock_whose_users_outgrow_the_budget_is_refreshed_over_several_ticks() {
    let mut s = server();
    eval(
        &mut s,
        r#"datablock StaticShapeData(D) { shapeFile = "a.dts"; };"#,
    );
    eval(&mut s, "new StaticShape(S0) { dataBlock = D; };");
    tick(&mut s);
    // The work one user's heavy refresh costs.
    let before = refresh::stats(&s.vm);
    eval(
        &mut s,
        r#"datablock StaticShapeData(D) { shapeFile = "b.dts"; };"#,
    );
    tick(&mut s);
    let one = refresh::stats(&s.vm).work - before.work;
    assert!(one > 0, "a refresh is charged");
    // A budget for about one chunk of users, far below what all of them need.
    let users = 10 * refresh::REFRESH_CHUNK;
    refresh::set_drain_work(
        &s.vm,
        one * (refresh::REFRESH_CHUNK as u64 + refresh::REFRESH_CHUNK as u64 / 2),
    );
    for i in 1..users {
        eval(
            &mut s,
            &format!("new StaticShape(S{i}) {{ dataBlock = D; }};"),
        );
    }
    tick(&mut s);
    let ids: Vec<ObjId> = (0..users).map(|i| id_of(&s, &format!("S{i}"))).collect();
    let built: Vec<u32> = ids.iter().map(|&u| instances(&s, u)).collect();
    let before = refresh::stats(&s.vm);
    eval(
        &mut s,
        r#"datablock StaticShapeData(D) { shapeFile = "c.dts"; };"#,
    );
    let mut ticks = 0;
    let mut last_heavy = before.heavy;
    while ids.iter().zip(&built).any(|(&u, &b)| instances(&s, u) == b) {
        tick(&mut s);
        ticks += 1;
        let heavy = refresh::stats(&s.vm).heavy;
        assert!(
            heavy > last_heavy,
            "every tick makes progress (tick {ticks})"
        );
        last_heavy = heavy;
        assert!(ticks <= users, "the refresh finishes");
    }
    let now = refresh::stats(&s.vm);
    assert!(
        ticks > 1 && now.deferred > before.deferred,
        "the budget stopped the drain at least once: {ticks} ticks"
    );
    // A chunk the budget stopped mid-way runs again whole, so a few users may have been rebuilt twice; none was missed.
    assert!(
        ids.iter().zip(&built).all(|(&u, &b)| instances(&s, u) > b),
        "every user was rebuilt"
    );
    assert_eq!(
        now.heavy - before.heavy,
        users as u64,
        "each user counted once"
    );
    assert_eq!(
        answer(&mut s, &format!("return S{}.getObjectBox();", users - 1)),
        "-0.5 -0.5 0 0.5 0.5 0.5"
    );
}

/// What a vehicle holds that comes from its datablock: the rigid body's words, its boxes and the wheel records and sequences the
/// datablock's preload gave (a tire or spring is script's, not the datablock's, and is left out).
#[derive(Debug, PartialEq)]
struct VehicleWords {
    mass: f32,
    drag: f32,
    rigid_mass: f32,
    one_over_mass: f32,
    inertia: [f32; 16],
    inv_inertia: [f32; 16],
    com: [f32; 3],
    friction: f32,
    restitution: f32,
    obj_box: Corners,
    body_box: Corners,
    wheels: Vec<WheelRecord>,
    steering_seq: i32,
    brakelight_seq: i32,
    /// Each wheel of the datablock's steering factor (`onNewDataBlock` sets it from the record's side).
    wheel_steering: Vec<f32>,
}

fn vehicle_words(s: &NetServer, id: ObjId) -> VehicleWords {
    let v = vehicle::state(&s.vm, id).expect("a vehicle's state");
    let e = vehicle::ext_of(&s.vm, id).expect("a vehicle");
    let pre = &e.preload;
    let wheel_steering = match e.body.as_ref().expect("the body is in") {
        vehicle::Body::Wheeled(w) => w.k.wheels
            [..pre.wheels.len().min(torque_vehicle::wheeled::WHEELS)]
            .iter()
            .map(|w| w.steering)
            .collect(),
        vehicle::Body::Flying(_) => Vec::new(),
    };
    VehicleWords {
        mass: v.mass,
        drag: v.drag,
        rigid_mass: v.rigid.mass,
        one_over_mass: v.rigid.one_over_mass,
        inertia: v.rigid.inertia,
        inv_inertia: v.rigid.inv_inertia,
        com: v.rigid.com,
        friction: v.rigid.friction,
        restitution: v.rigid.restitution,
        obj_box: (v.obj_box.min, v.obj_box.max),
        body_box: (v.body_box.min, v.body_box.max),
        wheels: pre.wheels.clone(),
        steering_seq: pre.steering_seq,
        brakelight_seq: pre.brakelight_seq,
        wheel_steering,
    }
}

/// The vehicle classes the oracle covers.
#[derive(Debug, Clone, Copy)]
enum VehicleClass {
    Wheeled,
    Flying,
}

impl VehicleClass {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            VehicleClass::Wheeled => ("WheeledVehicle", "WheeledVehicleData"),
            VehicleClass::Flying => ("FlyingVehicle", "FlyingVehicleData"),
        }
    }

    /// The shapes a redeclare picks from: the buggy (four wheels) or a plain box (none) for the wheeled, the boxes for the flying.
    fn shape(self, rng: &mut Rng) -> &'static str {
        match self {
            VehicleClass::Wheeled => {
                if rng.one_in(2) {
                    BUGGY
                } else {
                    SHAPES[rng.below(SHAPES.len())].0
                }
            }
            VehicleClass::Flying => SHAPES[rng.below(SHAPES.len())].0,
        }
    }
}

/// A seeded edit of a vehicle datablock: the rigid body's words, and the shape on a redeclare.
fn random_vehicle_edit(rng: &mut Rng, class: VehicleClass, redeclare: bool) -> String {
    let mut fields = Vec::new();
    if redeclare {
        fields.push(format!("shapeFile = \"{}\";", class.shape(rng)));
    }
    fields.push(format!("mass = {};", 1.0 + rng.below(400) as f32 * 0.5));
    fields.push(format!("drag = {};", rng.below(8) as f32 * 0.1));
    fields.push(format!("bodyFriction = {};", rng.below(10) as f32 * 0.1));
    fields.push(format!("bodyRestitution = {};", rng.below(10) as f32 * 0.1));
    fields.push(format!(
        "massCenter = \"{} {} {}\";",
        rng.below(5) as f32 * 0.1 - 0.2,
        rng.below(5) as f32 * 0.1 - 0.2,
        rng.below(5) as f32 * 0.1
    ));
    // A massBox with a zero side falls back to the object box's size.
    fields.push(format!(
        "massBox = \"{} {} {}\";",
        rng.below(4) as f32 * 0.5,
        rng.below(4) as f32 * 0.5,
        rng.below(4) as f32 * 0.5
    ));
    let (_, data) = class.names();
    if redeclare {
        format!("datablock {data}(D) {{ {} }};", fields.join(" "))
    } else {
        fields
            .iter()
            .map(|f| format!("D.{f}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The oracle for a vehicle class (as [`differential`]): A built on D, D edited, a tick, B fresh on D; the words must not differ.
fn vehicle_differential(class: VehicleClass, edits: usize, seed: u64) -> usize {
    let mut s = server();
    let (object, data) = class.names();
    let mut rng = Rng(seed);
    let first = match class {
        VehicleClass::Wheeled => BUGGY,
        VehicleClass::Flying => SHAPES[0].0,
    };
    eval(
        &mut s,
        &format!(
            r#"datablock {data}(D) {{ shapeFile = "{first}"; mass = 300; }};
new {object}(A) {{ dataBlock = D; position = "0 0 5"; }};"#
        ),
    );
    let a = id_of(&s, "A");
    tick(&mut s);
    if matches!(class, VehicleClass::Wheeled) {
        assert_eq!(
            vehicle_words(&s, a).wheels.len(),
            4,
            "the buggy has four hubs"
        );
    }
    let mut differing = 0;
    for i in 0..edits {
        let redeclare = rng.one_in(2);
        eval(&mut s, &random_vehicle_edit(&mut rng, class, redeclare));
        tick(&mut s);
        eval(
            &mut s,
            &format!("new {object}(B) {{ dataBlock = D; position = \"0 0 5\"; }};"),
        );
        let b = id_of(&s, "B");
        tick(&mut s);
        let (at, bt) = (vehicle_words(&s, a), vehicle_words(&s, b));
        if at != bt {
            differing += 1;
            eprintln!(
                "{class:?} edit {i} (redeclare {redeclare}): refreshed {at:#?} fresh {bt:#?}"
            );
        }
        eval(&mut s, "B.delete();");
    }
    assert!(
        logged(&mut s).iter().all(|l| !l.contains("has no refresh")),
        "the vehicle datablocks are refreshed, not warned about"
    );
    differing
}

#[test]
fn a_refreshed_wheeled_vehicle_equals_a_fresh_one_over_seeded_edits() {
    assert_eq!(vehicle_differential(VehicleClass::Wheeled, 100, 4), 0);
}

#[test]
fn a_refreshed_flying_vehicle_equals_a_fresh_one_over_seeded_edits() {
    assert_eq!(vehicle_differential(VehicleClass::Flying, 100, 5), 0);
}

#[test]
fn a_refreshed_vehicle_keeps_its_pose_momenta_and_wheel_fittings() {
    let mut s = server();
    eval(
        &mut s,
        &format!(
            r#"datablock WheeledVehicleTire(T) {{ shapeFile = "a.dts"; radius = 0.5; }};
datablock WheeledVehicleSpring(S) {{ length = 0.4; force = 3000; damping = 1000; }};
datablock WheeledVehicleData(D) {{ shapeFile = "{BUGGY}"; mass = 300; }};
new WheeledVehicle(A) {{ dataBlock = D; position = "0 0 5"; }};
for (%i = 0; %i < 4; %i++) {{ A.setWheelTire(%i, T); A.setWheelSpring(%i, S); }}
A.setVelocity("1 2 3");"#
        ),
    );
    let a = id_of(&s, "A");
    tick(&mut s);
    let before = vehicle::state(&s.vm, a).expect("the state").clone();
    let fitted = |s: &NetServer| -> Vec<(bool, bool)> {
        vehicle::wheel_fittings(&s.vm, a)
            .expect("wheels")
            .iter()
            .take(4)
            .map(|(t, sp)| (t.is_some(), sp.is_some()))
            .collect()
    };
    assert_eq!(fitted(&s), vec![(true, true); 4]);
    // A write doubles the mass: the momenta and the pose stay, the velocity follows the mass at the next step.
    eval(&mut s, "D.mass = 600;");
    let errors = refresh::drain(&mut s.vm);
    assert!(errors.is_empty(), "{errors:?}");
    let after = vehicle::state(&s.vm, a).expect("the state");
    assert_eq!(after.rigid.mass, 600.0);
    assert_eq!(after.rigid.lin_mom, before.rigid.lin_mom);
    assert_eq!(after.rigid.ang_mom, before.rigid.ang_mom);
    assert_eq!(after.rigid.lin_pos, before.rigid.lin_pos);
    assert_eq!(after.rigid.ang_pos, before.rigid.ang_pos);
    assert_eq!(after.obj_to_world, before.obj_to_world);
    assert_eq!(
        fitted(&s),
        vec![(true, true); 4],
        "a write keeps the fittings"
    );
    // A redeclare rebuilds the wheel records (the same shape: the same four) and keeps the fittings `setDataBlock` would drop.
    eval(
        &mut s,
        &format!(r#"datablock WheeledVehicleData(D) {{ shapeFile = "{BUGGY}"; mass = 450; }};"#),
    );
    let errors = refresh::drain(&mut s.vm);
    assert!(errors.is_empty(), "{errors:?}");
    let after = vehicle::state(&s.vm, a).expect("the state");
    assert_eq!(after.rigid.mass, 450.0);
    assert_eq!(after.rigid.lin_pos, before.rigid.lin_pos);
    assert_eq!(after.rigid.lin_mom, before.rigid.lin_mom);
    assert_eq!(vehicle_words(&s, a).wheels.len(), 4);
    assert_eq!(
        fitted(&s),
        vec![(true, true); 4],
        "a redeclare keeps the fittings"
    );
    tick(&mut s);
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

/// A datablock class whose add-time checks clamp its fields, with values out of range and the fields the checks leave behind.
struct Checked {
    class: &'static str,
    /// The body of the declaration, every value out of range.
    body: &'static str,
    /// The fields compared (an indexed one as `times[1]`).
    fields: &'static [&'static str],
}

const CHECKED: [Checked; 6] = [
    Checked {
        class: "ParticleData",
        body: "dragCoefficient = -1; lifetimeMS = -5; lifetimeVarianceMS = 10; spinRandomMin = -10; spinRandomMax = 10; times[0] = 0.5; times[1] = 0.2; times[2] = 0.9; times[3] = 0.1;",
        fields: &[
            "dragCoefficient",
            "lifetimeMS",
            "lifetimeVarianceMS",
            "times[0]",
            "times[1]",
            "times[2]",
            "times[3]",
        ],
    },
    Checked {
        class: "ParticleEmitterData",
        body: "ejectionPeriodMS = 0; periodVarianceMS = 5; ejectionVelocity = -1; velocityVariance = 3; ejectionOffset = -1; thetaMin = -1; thetaMax = 200; phiVariance = 400; lifetimeMS = -3; lifetimeVarianceMS = 5; particles = \"Dust\";",
        fields: &[
            "ejectionPeriodMS",
            "periodVarianceMS",
            "ejectionVelocity",
            "velocityVariance",
            "ejectionOffset",
            "thetaMin",
            "thetaMax",
            "phiVariance",
            "lifetimeMS",
            "lifetimeVarianceMS",
            "particles",
        ],
    },
    Checked {
        class: "ExplosionData",
        body: "explosionScale = \"0 0 0\"; debrisThetaMin = -5; debrisThetaMax = 200; debrisPhiMin = -1; debrisPhiMax = 400; debrisNum = 2000; debrisNumVariance = 3000; debrisVelocity = 0; debrisVelocityVariance = 5000; playSpeed = 0; lifetimeMS = -5; lifetimeVariance = 5; delayMS = -1; delayVariance = 3; offset = -1;",
        fields: &[
            "explosionScale",
            "debrisThetaMin",
            "debrisThetaMax",
            "debrisPhiMin",
            "debrisPhiMax",
            "debrisNum",
            "debrisNumVariance",
            "debrisVelocity",
            "debrisVelocityVariance",
            "playSpeed",
            "lifetimeMS",
            "lifetimeVariance",
            "delayMS",
            "delayVariance",
            "offset",
        ],
    },
    Checked {
        class: "DebrisData",
        body: "velocity = 1; velocityVariance = 5; friction = 20; elasticity = -20; lifetime = 2000; lifetimeVariance = -1; numBounces = -1; bounceVariance = 5; minSpinSpeed = 5; maxSpinSpeed = 1;",
        fields: &[
            "velocity",
            "velocityVariance",
            "friction",
            "elasticity",
            "lifetime",
            "lifetimeVariance",
            "numBounces",
            "bounceVariance",
            "minSpinSpeed",
            "maxSpinSpeed",
        ],
    },
    Checked {
        class: "AudioDescription",
        body: "volume = 2; loopCount = -5; minLoopGap = 10; maxLoopGap = 5; is3D = true; ReferenceDistance = -1; maxDistance = 0; coneInsideAngle = 400; coneOutsideAngle = -1; coneOutsideVolume = 3; coneVector = \"0 0 0\"; environmentLevel = 5; type = 99;",
        fields: &[
            "volume",
            "loopCount",
            "minLoopGap",
            "maxLoopGap",
            "ReferenceDistance",
            "maxDistance",
            "coneInsideAngle",
            "coneOutsideAngle",
            "coneOutsideVolume",
            "coneVector",
            "environmentLevel",
            "type",
        ],
    },
    Checked {
        class: "AudioProfile",
        body: "description = Voice; fileName = \"a.wav\"; volume = 2;",
        fields: &["description", "fileName", "volume"],
    },
];

/// Each of `fields` of `name`, as script reads it.
fn fields_of(s: &mut NetServer, name: &str, fields: &[&str]) -> Vec<String> {
    fields
        .iter()
        .map(|f| answer(s, &format!("return {name}.{f};")))
        .collect()
}

#[test]
fn a_redeclare_leaves_the_same_clamped_fields_as_a_fresh_add_for_the_six_checked_classes() {
    let mut s = server();
    // The particle the emitters name and the description the profiles name.
    eval(
        &mut s,
        "datablock ParticleData(Dust) {}; datablock AudioDescription(Voice) { volume = 0.5; };",
    );
    for checked in &CHECKED {
        let Checked {
            class,
            body,
            fields,
        } = checked;
        // D declared in range (nothing clamped), then declared again with every value out of range; E fresh with the same.
        eval(&mut s, &format!("datablock {class}(D{class}) {{ }};"));
        eval(
            &mut s,
            &format!("datablock {class}(D{class}) {{ {body} }};"),
        );
        eval(
            &mut s,
            &format!("datablock {class}(E{class}) {{ {body} }};"),
        );
        assert!(
            id_of(&s, &format!("D{class}")) < id_of(&s, &format!("E{class}")),
            "{class}: the redeclare kept the object"
        );
        let (d, e) = (
            fields_of(&mut s, &format!("D{class}"), fields),
            fields_of(&mut s, &format!("E{class}"), fields),
        );
        assert_eq!(
            d, e,
            "{class}: the redeclared and the fresh datablock differ"
        );
    }
    // The clamps themselves, for ParticleData: lifetime 1 ms, the variance below it, the drag at 0, the times ordered after
    // the first, which is always 0.
    assert_eq!(
        fields_of(
            &mut s,
            "DParticleData",
            &[
                "dragCoefficient",
                "lifetimeMS",
                "lifetimeVarianceMS",
                "times[1]",
                "times[3]"
            ]
        ),
        ["0", "1", "0", "0.2", "0.9"]
    );
    // The emitter's particle list is built again by the redeclare: a redeclare naming another particle changes it.
    eval(
        &mut s,
        "datablock ParticleData(Smoke) {}; datablock ParticleEmitterData(DParticleEmitterData) { particles = \"Smoke\"; };",
    );
    let values = torque_game::shapes::shapes_of(&s.vm).expect("the shapes");
    let emitter = id_of(&s, "DParticleEmitterData");
    assert_eq!(
        values.values().emitter_ids(emitter),
        vec![id_of(&s, "Smoke") as i32]
    );
    // A redeclare whose values the checks refuse prints the checks' line, once, and the datablock stays with them.
    logged(&mut s);
    eval(
        &mut s,
        "new AudioDescription(Plain) {}; datablock AudioProfile(DAudioProfile) { description = Plain; };",
    );
    assert!(s.vm.objects().find(b"DAudioProfile").is_some());
    let lines = logged(&mut s);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains("not networkable"))
            .count(),
        1,
        "{lines:?}"
    );
    assert_eq!(lines.len(), 1, "one line: {lines:?}");
    // The emitter's redeclare is checked as a fresh add: an empty particle string is refused, the list stays.
    eval(
        &mut s,
        "datablock ParticleEmitterData(DParticleEmitterData) { particles = \"\"; };",
    );
    assert!(
        logged(&mut s)
            .iter()
            .any(|l| l.contains("ParticleEmitterData(DParticleEmitterData) no particleString"))
    );
    assert_eq!(
        values.values().emitter_ids(emitter),
        vec![id_of(&s, "Smoke") as i32]
    );
}

/// How many objects of `class` the server has.
fn objects_of(s: &NetServer, class: &str) -> usize {
    let objects = s.vm.objects();
    objects
        .iter()
        .filter(|(_, o)| objects.classes.name(o.class()) == class.as_bytes())
        .count()
}

#[test]
fn a_projectile_data_removed_under_live_projectiles_leaves_no_projectile_after_600_ticks() {
    let mut s = server();
    eval(
        &mut s,
        "datablock ProjectileData(Shot) { lifetime = 60000; };",
    );
    for i in 0..10 {
        eval(
            &mut s,
            &format!(
                "new Projectile(P{i}) {{ dataBlock = Shot; initialPosition = \"{i} 0 5\"; initialVelocity = \"0 0 0\"; }};"
            ),
        );
    }
    for _ in 0..5 {
        tick(&mut s);
    }
    assert_eq!(
        objects_of(&s, "Projectile"),
        10,
        "they live while their datablock does"
    );
    eval(&mut s, "Shot.delete();");
    let mut ticks = 0;
    while objects_of(&s, "Projectile") > 0 && ticks < 600 {
        tick(&mut s);
        ticks += 1;
    }
    assert_eq!(
        objects_of(&s, "Projectile"),
        0,
        "0 projectiles after {ticks} ticks"
    );
    assert!(ticks <= 2, "each deletes itself at its next tick: {ticks}");
    for _ in ticks..600 {
        tick(&mut s);
    }
    assert_eq!(objects_of(&s, "Projectile"), 0);
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

#[test]
fn a_written_dynamic_type_swaps_the_bits_the_datablock_gave() {
    let mut s = server();
    // An Item may only take the damageable-item bit (1 << 27) of its `dynamicType`; a StaticShape takes the low 24 bits.
    eval(
        &mut s,
        r#"
datablock StaticShapeData(D) { shapeFile = "a.dts"; dynamicType = 256; };
datablock ItemData(I) { shapeFile = "a.dts"; dynamicType = 134217728; };
new StaticShape(S) { dataBlock = D; };
new Item(J) { dataBlock = I; position = "0 0 5"; };
"#,
    );
    tick(&mut s);
    let mask = |s: &mut NetServer, name: &str| -> i64 {
        answer(s, &format!("return {name}.getType();"))
            .parse()
            .unwrap()
    };
    // The dynamicType bits the scene gave: 256 to the StaticShape, bit 27 to the Item.
    let (first_bits, item_bits): (i64, i64) = (256, 1 << 27);
    let (shape_mask, item_mask) = (mask(&mut s, "S"), mask(&mut s, "J"));
    assert_eq!(
        (shape_mask & first_bits, item_mask & item_bits),
        (first_bits, item_bits)
    );
    eval(&mut s, "D.dynamicType = 512; I.dynamicType = 0;");
    tick(&mut s);
    assert_eq!(mask(&mut s, "S"), shape_mask - first_bits + 512);
    let item_now = mask(&mut s, "J");
    assert_eq!(item_now, item_mask - item_bits);
    assert_eq!(
        item::state(&s.vm, id_of(&s, "J")).map(|i| i64::from(i.type_mask)),
        Some(item_now),
        "the Item's step reads the new mask"
    );
}
