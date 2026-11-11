//! Live datablocks on a server: an object built from a datablock that is then edited in place (its fields written, or the
//! datablock declared again) is refreshed at the next tick and equals an object built fresh from the edited datablock, field
//! for field; the refresh is coalesced (one light refresh per user per tick for any number of writes, one heavy refresh per
//! user for any number of redeclares, no shape rebuild for a write), script threads keep their sequence and position, and a
//! drain scans nothing while no object uses a dirty datablock.

use crate::net_client::{self, NetServer, eval};

use torque_game::scene::{ext, refresh};
use torque_game::sim::{item, player, vitals};
use torque_server::server::TICK_MS;
use torque_shapes::synth::Rng;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::sim::ObjId;

/// The shapes: boxes of three sizes, each with two sequences a thread can play.
const SHAPES: [(&str, f32); 3] = [("a.dts", 1.0), ("b.dts", 2.0), ("c.dts", 0.5)];

fn server() -> NetServer {
    let files = MemFiles::new();
    for (name, half) in SHAPES {
        let b = [-half, -half, 0.0, half, half, half];
        let shape =
            torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient", "spin"]);
        files.insert(name, &torque_shapes::write::write_dts(&shape));
    }
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
