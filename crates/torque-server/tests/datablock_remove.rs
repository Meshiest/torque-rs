//! Removing datablocks from a running server: `removeDataBlock` refuses over users and referrers, moves them to a fallback or
//! deletes the users; a plain `delete()` over users retires the datablock (its users keep working, no new object takes it, it
//! is freed with its last user); a client that joins after a removal gets the rest with their keys; ids are never reused inside
//! a mission; a ghost never carries a dead datablock id and a Player keeps its last products when its datablock is gone.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::net_client::{self, NetServer, TestClient, connection_of, eval, run};

use torque_game::scene::{refresh, remove};
use torque_game::sim::player;
use torque_net::huffman::Table;
use torque_server::net::NetShared;
use torque_server::net::codecs::net_codecs;
use torque_server::net::datablocks::{ConnDatablocks, DbPost, net_classes_of};
use torque_server::net::torque::TorqueNet;
use torque_server::net::transport::MemoryEnd;
use torque_server::server::TICK_MS;
use torque_ts_vm::host::MemFiles;
use torque_ts_vm::policy::Limits;
use torque_ts_vm::sim::ObjId;

/// A camera for a connection to scope from, and the datablocks the tests remove.
const SCENE: &str = r#"
new SimGroup(ClientGroup);
datablock CameraData(EyeData) {};
new Camera(Eye) { dataBlock = EyeData; };
datablock StaticShapeData(First) { shapeFile = "a.dts"; };
datablock StaticShapeData(Second) { shapeFile = "b.dts"; };
datablock PlayerData(Soldier) { shapeFile = "a.dts"; maxForwardSpeed = 4; };
datablock PlayerData(Medic) { shapeFile = "b.dts"; maxForwardSpeed = 6; };
"#;

fn game_server() -> (NetServer, Rc<RefCell<NetShared<TorqueNet>>>, MemoryEnd) {
    let files = MemFiles::new();
    for (name, half) in [("a.dts", 1.0), ("b.dts", 2.0)] {
        let b = [-half, -half, 0.0, half, half, half];
        let shape = torque_shapes::synth::hull_shape(b, &[("detail2", b)], &[], &["ambient"]);
        files.insert(name, &torque_shapes::write::write_dts(&shape));
    }
    net_client::game_server(files)
}

fn tick(s: &mut NetServer) {
    s.run(TICK_MS, |_| false);
}

fn answer(s: &mut NetServer, code: &str) -> String {
    String::from_utf8(s.vm.eval(code.as_bytes()).unwrap()).unwrap()
}

fn id_of(s: &NetServer, name: &str) -> ObjId {
    s.vm.objects().find(name.as_bytes()).expect(name)
}

fn logged(s: &mut NetServer) -> Vec<String> {
    s.vm.drain_log()
        .into_iter()
        .map(|l| String::from_utf8_lossy(&l.text).into_owned())
        .collect()
}

fn users(s: &NetServer, name: &str) -> u32 {
    refresh::users(&s.vm, id_of(s, name))
}

/// The datablock chain a connection that joins now would get, driven to its end (each event delivered in order): the ids
/// that went out with a body, in group order.
fn joining_client_gets(s: &NetServer) -> Vec<ObjId> {
    let keys = s.state.borrow().datablock_keys.clone();
    let codecs = net_codecs(&s.vm);
    let classes = Rc::new(net_classes_of(&*codecs));
    let mut chain = ConnDatablocks::default();
    let (_, posts, _) = chain.call(s.vm.objects(), &keys, &classes, 1);
    let mut pending: std::collections::VecDeque<DbPost> = posts.into();
    let mut sent = Vec::new();
    while let Some(post) = pending.pop_front() {
        let DbPost::Cell(cell) = post else {
            continue;
        };
        chain
            .render(s.vm.objects(), &keys, &Table::torque(), &*codecs)
            .unwrap();
        if cell.0.rendered().is_some_and(|e| e.body.is_some()) {
            sent.push(cell.0.id);
        }
        pending.extend(chain.settled(s.vm.objects(), &keys, &classes, &*codecs, &cell.0));
    }
    assert!(!chain.active(), "the chain ended");
    sent
}

fn players(s: &mut NetServer, datablock: &str, count: usize) -> Vec<ObjId> {
    for i in 0..count {
        eval(
            s,
            &format!("new Player(P{i}) {{ dataBlock = {datablock}; position = \"{i} 0 5\"; }};"),
        );
    }
    (0..count).map(|i| id_of(s, &format!("P{i}"))).collect()
}

#[test]
fn a_removal_over_one_user_refuses_and_names_it() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let soldier = id_of(&s, "Soldier");
    let [p] = players(&mut s, "Soldier", 1)[..] else {
        unreachable!()
    };
    tick(&mut s);
    s.vm.drain_log();
    assert_eq!(answer(&mut s, "return removeDataBlock(Soldier);"), "0");
    assert!(s.vm.objects().contains(soldier), "the datablock stays");
    let lines = logged(&mut s);
    assert_eq!(
        lines,
        [format!(
            "datablock PlayerData Soldier ({soldier}) is used by 1 objects (first: Player P0 ({p}))"
        )]
    );
    assert_eq!(answer(&mut s, "return Soldier.getUserCount();"), "1");
    assert_eq!(answer(&mut s, "return Soldier.getUsers();"), p.to_string());
    assert_eq!(
        answer(&mut s, "return getDataBlockUsers(Soldier);"),
        p.to_string()
    );
    // A removal with no users goes through and spends nothing but the id.
    let second = id_of(&s, "Second");
    assert_eq!(answer(&mut s, "return removeDataBlock(Second);"), "1");
    assert!(!s.vm.objects().contains(second));
    assert!(!s.vm.objects().is_retiring(second));
}

#[test]
fn a_fallback_moves_sixty_four_users_and_the_datablock_goes() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let (soldier, medic) = (id_of(&s, "Soldier"), id_of(&s, "Medic"));
    let ids = players(&mut s, "Soldier", 64);
    tick(&mut s);
    assert_eq!((users(&s, "Soldier"), users(&s, "Medic")), (64, 0));
    // A fallback of another class is refused.
    assert_eq!(
        answer(&mut s, "return removeDataBlock(Soldier, First);"),
        "0"
    );
    assert_eq!(
        answer(&mut s, "return removeDataBlock(Soldier, Soldier);"),
        "0"
    );
    assert_eq!(users(&s, "Soldier"), 64);
    s.vm.drain_log();
    assert_eq!(
        answer(&mut s, "return removeDataBlock(Soldier, Medic);"),
        "1"
    );
    assert!(!s.vm.objects().contains(soldier));
    assert_eq!(refresh::users(&s.vm, medic), 64);
    for &p in &ids {
        assert_eq!(
            torque_game::scene::access::object_field(&s.vm, p, b"dataBlock"),
            Some(medic)
        );
        assert_eq!(
            player::state(&s.vm, p).map(|st| st.speeds[0]),
            Some(6.0),
            "the Player runs with the fallback's words"
        );
    }
    tick(&mut s);
    eval(
        &mut s,
        "new Player(Fresh) { dataBlock = Medic; position = \"0 0 5\"; };",
    );
    assert_eq!(
        answer(&mut s, "return P63.getObjectBox();"),
        answer(&mut s, "return Fresh.getObjectBox();"),
        "a moved Player has a fresh one's box"
    );
    assert_eq!(answer(&mut s, "return Medic.getUserCount();"), "65");
    assert_eq!(
        answer(&mut s, "return getWordCount(Medic.getUsers(10));"),
        "10",
        "the list is bounded by the script's cap"
    );
    assert_eq!(
        answer(
            &mut s,
            "return getWordCount(getDataBlockUsers(Medic, 100000));"
        ),
        "65"
    );
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

#[test]
fn a_referrer_in_a_datablock_field_refuses_and_a_fallback_rewrites_it() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    eval(
        &mut s,
        r#"
datablock ExplosionData(Boom) {};
datablock ExplosionData(Puff) {};
datablock StaticShapeData(Crate) { shapeFile = "a.dts"; explosion = Boom; };
"#,
    );
    let (boom, puff, crate_db) = (id_of(&s, "Boom"), id_of(&s, "Puff"), id_of(&s, "Crate"));
    let keys = s.state.borrow().datablock_keys.clone();
    s.vm.drain_log();
    assert_eq!(answer(&mut s, "return removeDataBlock(Boom);"), "0");
    assert_eq!(
        answer(&mut s, "return removeDataBlock(Boom, \"deleteUsers\");"),
        "0",
        "deleting users does not free a referenced datablock"
    );
    assert!(s.vm.objects().contains(boom));
    assert_eq!(
        logged(&mut s),
        [
            format!(
                "datablock ExplosionData Boom ({boom}) is referenced by 1 datablock field(s) (first: StaticShapeData Crate ({crate_db}).explosion)"
            ),
            format!(
                "datablock ExplosionData Boom ({boom}) is referenced by 1 datablock field(s) (first: StaticShapeData Crate ({crate_db}).explosion)"
            ),
        ]
    );
    assert_eq!(
        remove::referrers(&s.vm, boom, 10),
        [remove::Referrer {
            datablock: crate_db,
            field: b"explosion".to_vec(),
            index: 0,
        }]
    );
    // The stored id is what counts: a text that happens to name it is no reference.
    eval(&mut s, "Crate.category = \"Boom\";");
    assert_eq!(remove::referrers(&s.vm, boom, 10).len(), 1);

    let crate_key = keys.key_of(crate_db);
    assert_eq!(answer(&mut s, "return removeDataBlock(Boom, Puff);"), "1");
    assert!(!s.vm.objects().contains(boom));
    assert_eq!(
        torque_game::scene::access::object_field(&s.vm, crate_db, b"explosion"),
        Some(puff)
    );
    assert!(
        keys.key_of(crate_db) > crate_key,
        "the referrer's key moved, so its clients read the new field"
    );
    assert!(remove::referrers(&s.vm, puff, 10).len() == 1);
}

#[test]
fn delete_users_deletes_the_users_and_removes() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let soldier = id_of(&s, "Soldier");
    let ids = players(&mut s, "Soldier", 10);
    tick(&mut s);
    assert_eq!(
        answer(&mut s, "return removeDataBlock(Soldier, \"deleteUsers\");"),
        "1"
    );
    assert!(!s.vm.objects().contains(soldier));
    assert!(ids.iter().all(|&p| !s.vm.objects().contains(p)));
    tick(&mut s);
    assert!(s.vm.objects().find(b"Soldier").is_none());
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

#[test]
fn a_client_that_joins_after_a_removal_gets_the_rest_with_their_keys() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    for i in 0..20 {
        eval(
            &mut s,
            &format!("datablock StaticShapeData(Extra{i}) {{ shapeFile = \"a.dts\"; }};"),
        );
    }
    tick(&mut s);
    let before = joining_client_gets(&s);
    let n = before.len();
    assert!(
        before.contains(&id_of(&s, "Extra7")) && n >= 24,
        "{before:?}"
    );
    let keys = s.state.borrow().datablock_keys.clone();
    let keys_before: Vec<(ObjId, i32)> = before.iter().map(|&id| (id, keys.key_of(id))).collect();
    let gone = id_of(&s, "Extra7");
    assert_eq!(answer(&mut s, "return removeDataBlock(Extra7);"), "1");
    tick(&mut s);
    let after = joining_client_gets(&s);
    assert_eq!(after.len(), n - 1);
    assert!(!after.contains(&gone));
    for (id, key) in keys_before {
        if id != gone {
            assert!(after.contains(&id));
            assert_eq!(keys.key_of(id), key, "datablock {id} keeps its key");
        }
    }
    assert_eq!(
        keys.key_of(gone),
        0,
        "the removed datablock's key is forgotten"
    );
}

#[test]
fn ten_thousand_add_and_remove_cycles_end_with_the_limit_error_and_alias_no_id() {
    let (mut s, _, _) = game_server();
    // A range of 1,024 ids, so the cycles run past it.
    s.vm.set_limits(Limits {
        datablock_ids: 1024,
        ..s.vm.limits()
    });
    let mut seen = HashSet::new();
    let mut aliased = 0;
    let mut refused = 0;
    let mut limit_errors = 0;
    for _ in 0..10_000 {
        eval(&mut s, "datablock StaticShapeData(Cycle) {};");
        match s.vm.objects().find(b"Cycle") {
            Some(id) => {
                if !seen.insert(id) {
                    aliased += 1;
                }
                eval(&mut s, "Cycle.delete();");
                assert!(!s.vm.objects().contains(id), "no users: gone at once");
            }
            None => {
                refused += 1;
                limit_errors += logged(&mut s)
                    .iter()
                    .filter(|l| l.contains("Exceeded maximum number of data blocks (1023)"))
                    .count();
            }
        }
    }
    assert_eq!(aliased, 0, "0 aliased ids");
    let max = usize::try_from(s.vm.limits().max_datablocks()).unwrap();
    assert_eq!(seen.len(), max, "the ids of the range, each once");
    assert_eq!(refused, 10_000 - max);
    assert_eq!(
        limit_errors, refused,
        "each refused cycle printed the limit error"
    );
    assert_eq!(s.vm.objects().datablock_count(), 0);
    // `deleteDataBlocks` starts the ids over with the mission.
    eval(
        &mut s,
        "deleteDataBlocks(); datablock StaticShapeData(Cycle) {};",
    );
    assert_eq!(s.vm.objects().find(b"Cycle"), Some(3));
}

#[test]
fn a_plain_delete_over_players_retires_the_datablock_until_the_last_player_goes() {
    let (mut s, shared, end) = game_server();
    eval(&mut s, SCENE);
    let soldier = id_of(&s, "Soldier");
    let ids = players(&mut s, "Soldier", 10);
    let mut client = TestClient::new(50_020, 7, Default::default());
    client.start(&end);
    run(&mut s, &end, &mut [&mut client], 10);
    assert!(client.connected());
    let conn = connection_of(&shared, client.addr);
    eval(
        &mut s,
        &format!("{conn}.setControlObject(Eye); {conn}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut client], 20);
    let infos: Vec<_> = ids
        .iter()
        .map(|&p| player::info(&s.vm, p).expect("a Player's products"))
        .collect();

    eval(&mut s, "Soldier.delete();");
    assert!(s.vm.objects().contains(soldier), "retiring, not gone");
    assert!(s.vm.objects().is_retiring(soldier));
    assert_eq!(answer(&mut s, "return Soldier.getUserCount();"), "10");
    run(&mut s, &end, &mut [&mut client], 10);
    assert!(s.vm.objects().contains(soldier));
    for (&p, info) in ids.iter().zip(&infos) {
        assert!(s.vm.objects().contains(p));
        assert!(
            player::info(&s.vm, p).is_some_and(|now| Rc::ptr_eq(&now, info)),
            "the Player keeps its products"
        );
        assert_eq!(player::state(&s.vm, p).map(|st| st.speeds[0]), Some(4.0));
    }
    // The ghosts carry the retiring datablock's id, which the client holds: no dead id.
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    let conn_id = shared.borrow().conns[&conn].conn.expect("accepted");
    let mut ghosted = 0;
    for &p in &ids {
        if let Some(index) = hub.ghost_index(conn_id, p) {
            ghosted += 1;
            let db = client.world.ghosts[&index].datablock();
            assert!(db.is_some_and(|db| s.vm.objects().contains(db)), "{db:?}");
        }
    }
    assert_eq!(ghosted, 10);
    assert!(client.errors.is_empty(), "{:?}", client.errors);

    // No new object takes a retiring datablock: by name, by id, through setDataBlock or a datablock field.
    s.vm.drain_log();
    eval(
        &mut s,
        "new Player(Late) { dataBlock = Soldier; position = \"0 0 5\"; };",
    );
    eval(
        &mut s,
        &format!("new Player(Later) {{ dataBlock = {soldier}; position = \"0 0 5\"; }};"),
    );
    assert!(s.vm.objects().find(b"Late").is_none());
    assert!(s.vm.objects().find(b"Later").is_none());
    eval(
        &mut s,
        "new Player(Moved) { dataBlock = Medic; position = \"0 0 5\"; };",
    );
    assert_eq!(answer(&mut s, "return Moved.setDataBlock(Soldier);"), "0");
    assert_eq!(
        answer(&mut s, &format!("return Moved.setDataBlock({soldier});")),
        "0"
    );
    assert_eq!(
        answer(&mut s, "return Moved.getDataBlock();"),
        id_of(&s, "Medic").to_string()
    );
    eval(
        &mut s,
        "datablock StaticShapeData(Crate) { shapeFile = \"a.dts\"; explosion = Soldier; };",
    );
    assert_eq!(
        torque_game::scene::access::object_field(&s.vm, id_of(&s, "Crate"), b"explosion"),
        None
    );
    assert_eq!(
        logged(&mut s)
            .iter()
            .filter(|l| l.contains("is not a datablock of class") || l.contains("Could not find"))
            .count(),
        5
    );
    assert_eq!(answer(&mut s, "return Soldier.getUserCount();"), "10");
    // A declaration of the name makes a new datablock; the retiring one keeps its users.
    eval(
        &mut s,
        "datablock PlayerData(Soldier) { shapeFile = \"b.dts\"; maxForwardSpeed = 9; };",
    );
    let new_soldier = id_of(&s, "Soldier");
    assert_ne!(new_soldier, soldier);
    assert!(!s.vm.objects().is_retiring(new_soldier));
    assert_eq!(
        (
            refresh::users(&s.vm, soldier),
            refresh::users(&s.vm, new_soldier)
        ),
        (10, 0)
    );
    eval(
        &mut s,
        "new Player(Fresh) { dataBlock = Soldier; position = \"0 0 5\"; };",
    );
    assert_eq!(
        torque_game::scene::access::object_field(&s.vm, id_of(&s, "Fresh"), b"dataBlock"),
        Some(new_soldier)
    );

    // The users go one by one; the datablock is freed at the tick after the last.
    for (i, &p) in ids.iter().enumerate() {
        eval(&mut s, &format!("P{i}.delete();"));
        assert!(!s.vm.objects().contains(p));
        run(&mut s, &end, &mut [&mut client], 1);
        assert_eq!(
            s.vm.objects().contains(soldier),
            i < 9,
            "after user {i} went"
        );
    }
    assert!(!s.vm.objects().is_retiring(soldier));
    assert!(s.vm.objects().contains(new_soldier));
    assert!(client.errors.is_empty(), "{:?}", client.errors);
}

#[test]
fn a_datablock_deleted_under_its_players_leaves_them_their_info_and_no_dead_id_on_the_wire() {
    let (mut s, shared, end) = game_server();
    eval(&mut s, SCENE);
    let soldier = id_of(&s, "Soldier");
    let ids = players(&mut s, "Soldier", 10);
    tick(&mut s);
    let infos: Vec<_> = ids
        .iter()
        .map(|&p| player::info(&s.vm, p).expect("a Player's products"))
        .collect();
    // The host's own deletion (what `deleteDataBlocks` does to every datablock): no retire, the users keep a dead pointer.
    s.vm.delete_object(soldier).unwrap();
    assert!(!s.vm.objects().contains(soldier));
    for _ in 0..5 {
        tick(&mut s);
    }
    for (&p, info) in ids.iter().zip(&infos) {
        assert!(s.vm.objects().contains(p));
        assert!(
            player::info(&s.vm, p).is_some_and(|now| Rc::ptr_eq(&now, info)),
            "the Player keeps its last products"
        );
        assert_eq!(player::state(&s.vm, p).map(|st| st.speeds[0]), Some(4.0));
    }
    // A client that joins now ghosts the Players without a datablock id (0 dead ids).
    let mut client = TestClient::new(50_030, 7, Default::default());
    client.start(&end);
    run(&mut s, &end, &mut [&mut client], 10);
    assert!(client.connected());
    let conn = connection_of(&shared, client.addr);
    eval(
        &mut s,
        &format!("{conn}.setControlObject(Eye); {conn}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut client], 20);
    let hub = shared.borrow().ghosts.clone().expect("a hub");
    let conn_id = shared.borrow().conns[&conn].conn.expect("accepted");
    let mut ghosted = 0;
    let mut dead = 0;
    for &p in &ids {
        if let Some(index) = hub.ghost_index(conn_id, p) {
            ghosted += 1;
            if client.world.ghosts[&index].datablock() == Some(soldier) {
                dead += 1;
            }
        }
    }
    assert_eq!((ghosted, dead), (10, 0));
    assert!(client.errors.is_empty(), "{:?}", client.errors);
}

#[test]
fn counts_stay_right_through_set_data_block_ghost_scope_changes_and_mission_end() {
    let (mut s, shared, end) = game_server();
    eval(&mut s, SCENE);
    let soldier = id_of(&s, "Soldier");
    let ids = players(&mut s, "Soldier", 4);
    tick(&mut s);
    assert_eq!((users(&s, "Soldier"), users(&s, "Medic")), (4, 0));
    eval(
        &mut s,
        "P0.setDataBlock(Medic); P1.setDataBlock(Medic); P1.setDataBlock(Soldier);",
    );
    assert_eq!((users(&s, "Soldier"), users(&s, "Medic")), (3, 1));

    // A client that scopes the Players in and out moves no count.
    let mut client = TestClient::new(50_040, 7, Default::default());
    client.start(&end);
    run(&mut s, &end, &mut [&mut client], 10);
    let conn = connection_of(&shared, client.addr);
    eval(
        &mut s,
        &format!("{conn}.setControlObject(Eye); {conn}.activateGhosting();"),
    );
    run(&mut s, &end, &mut [&mut client], 20);
    assert!(!client.world.ghosts.is_empty());
    eval(&mut s, &format!("{conn}.resetGhosting();"));
    run(&mut s, &end, &mut [&mut client], 10);
    eval(&mut s, &format!("{conn}.activateGhosting();"));
    run(&mut s, &end, &mut [&mut client], 10);
    assert_eq!((users(&s, "Soldier"), users(&s, "Medic")), (3, 1));

    // A retiring datablock counts its users until they go; mission end frees everything and the next mission's ids start over.
    eval(&mut s, "Soldier.delete();");
    assert!(s.vm.objects().is_retiring(soldier));
    eval(&mut s, "P1.setDataBlock(Medic);");
    assert_eq!((refresh::users(&s.vm, soldier), users(&s, "Medic")), (2, 2));
    eval(&mut s, "P2.delete(); P3.delete();");
    assert_eq!(refresh::users(&s.vm, soldier), 0);
    tick(&mut s);
    assert!(!s.vm.objects().contains(soldier));
    eval(&mut s, "P0.delete(); P1.delete(); deleteDataBlocks();");
    assert!(ids.iter().all(|&p| !s.vm.objects().contains(p)));
    assert_eq!(s.vm.objects().datablock_count(), 0);
    assert!(s.vm.objects().retiring_datablocks().is_empty());
    tick(&mut s);
    eval(
        &mut s,
        "datablock PlayerData(Soldier) { shapeFile = \"a.dts\"; };",
    );
    assert_eq!(id_of(&s, "Soldier"), 3);
    assert_eq!(users(&s, "Soldier"), 0);
    assert!(logged(&mut s).iter().all(|l| !l.contains("rror")));
}

#[test]
fn the_removal_hook_runs_once_per_datablock() {
    let (mut s, _, _) = game_server();
    eval(&mut s, SCENE);
    let removed: Rc<RefCell<Vec<ObjId>>> = Rc::default();
    let seen = Rc::clone(&removed);
    remove::set_removal_hook(
        &mut s.vm,
        Some(Rc::new(move |vm, id| {
            assert!(vm.objects().contains(id), "the object is still registered");
            seen.borrow_mut().push(id);
        })),
    );
    let (soldier, first, second) = (
        id_of(&s, "Soldier"),
        id_of(&s, "First"),
        id_of(&s, "Second"),
    );
    players(&mut s, "Soldier", 2);
    tick(&mut s);
    // A retire: nothing yet (the datablock stays for its users). A removal of the retiring one by id: nothing (it does not
    // resolve). A delete after its last user left: once.
    eval(&mut s, "Soldier.delete();");
    assert_eq!(
        answer(&mut s, &format!("return removeDataBlock({soldier});")),
        "0"
    );
    eval(&mut s, "P0.delete(); P1.delete(); Soldier.delete();");
    assert!(!s.vm.objects().contains(soldier));
    assert_eq!(*removed.borrow(), [soldier]);
    // A delete without users and a checked removal: once each.
    eval(&mut s, "First.delete();");
    assert_eq!(answer(&mut s, "return removeDataBlock(Second);"), "1");
    assert_eq!(*removed.borrow(), [soldier, first, second]);
    // A retiring datablock forced to its end by id: the hook runs at the deletion, once.
    eval(
        &mut s,
        "datablock PlayerData(Again) { shapeFile = \"a.dts\"; }; new Player(Q) { dataBlock = Again; position = \"0 0 5\"; };",
    );
    let again = id_of(&s, "Again");
    eval(&mut s, "Again.delete();");
    assert_eq!(
        answer(
            &mut s,
            &format!("return removeDataBlock({again}, \"deleteUsers\");")
        ),
        "1"
    );
    assert!(!s.vm.objects().contains(again));
    assert_eq!(*removed.borrow(), [soldier, first, second, again]);
}
