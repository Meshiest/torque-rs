//! A real server and two engine clients over the memory transport: props spawned by script and by a client's command, the state
//! each client is sent, the share of each packet the props take, sleepers that cost nothing, owners that leave.

#[path = "net/rig.rs"]
mod rig;

use rig::{Rig, clients};

fn spawn(rig: &mut Rig, code: &str) -> i32 {
    rig.eval(code).parse().unwrap_or(0)
}

#[test]
fn props_reach_both_clients_and_land_where_the_server_has_them() {
    let (mut rig, _files) = clients(2, &[], &[]);
    for k in 0..8 {
        let id = spawn(
            &mut rig,
            &format!("return propSpawn(Crate, \"{} 0 {}\");", k * 2, 2 + k % 3),
        );
        assert!(id > 0);
    }
    assert_eq!(rig.eval("return $Spawned;"), "8");
    // They fall, land and sleep; every client then holds the server's own state, exactly.
    assert!(rig.run_until(600, |r| {
        let s = r.server_props();
        s.len() == 8 && s.iter().all(|p| p.2) && r.peers.iter().all(|p| p.props() == s)
    }));
    assert_eq!(rig.eval("return $Slept;"), "8");
    for (_, at, _) in rig.server_props() {
        assert!((at[2] - 0.5).abs() < 0.01, "{at:?}");
    }
    // Asleep, they cost nothing: no update reaches a client for 100 ticks.
    let updates = |r: &Rig| -> Vec<u32> {
        r.peers
            .iter()
            .flat_map(|p| {
                let book = p.book();
                let b = book.0.borrow();
                b.props().values().map(|c| c.updates).collect::<Vec<_>>()
            })
            .collect()
    };
    let before = updates(&rig);
    rig.run(100);
    assert_eq!(before, updates(&rig));
    // A push wakes one; both clients see it move and settle again.
    assert_eq!(rig.eval("return propCount();"), "8");
    let crate_id = rig.eval("%n = 0; for (%i = 0; %i < MissionCleanup.getCount(); %i++) { %o = MissionCleanup.getObject(%i); if (%o.getClassName() $= \"Prop\") return %o; } return 0;");
    assert_eq!(
        rig.eval(&format!(
            "return propApplyImpulse({crate_id}, \"0 0 400\");"
        )),
        "1"
    );
    rig.run(3);
    assert_eq!(rig.eval(&format!("return propIsAsleep({crate_id});")), "0");
    assert!(rig.run_until(600, |r| {
        let s = r.server_props();
        s.iter().all(|p| p.2) && r.peers.iter().all(|p| p.props() == s)
    }));
    // A deleted prop goes from every client.
    rig.eval(&format!("propDelete({crate_id});"));
    assert!(rig.run_until(100, |r| r.peers.iter().all(|p| p.props().len() == 7)));
}

#[test]
fn many_awake_props_stay_within_their_share_and_near_the_server() {
    // The fastest rate a client may ask for: 32 packets a second of 450 bytes.
    let prefs = [
        ("$pref::Net::PacketRateToClient", "32"),
        ("$pref::Net::PacketSize", "450"),
    ];
    let (mut rig, _files) = clients(2, &[("$Pref::Props::MaxAwake", "512")], &prefs);
    // 512 sliders on the floor in a grid, each sliding slowly its own way.
    rig.eval(
        "for (%i = 0; %i < 512; %i++) { \
            %p = propSpawn(Slider, (%i % 32) * 3 - 48 SPC mFloor(%i / 32) * 3 - 24 SPC 0.25); \
            %a = %i * 0.7; \
            propSetVelocity(%p, mCos(%a) * 0.12 SPC mSin(%a) * 0.12 SPC 0); \
         }",
    );
    assert_eq!(rig.eval("return propCount();"), "512");
    assert!(
        rig.run_until(1200, |r| r.peers.iter().all(|p| p.props().len() == 512)),
        "the first records of every prop"
    );
    let mut worst = 0.0f32;
    for _ in 0..300 {
        rig.step();
        let server = rig.server_props();
        assert!(server.iter().filter(|p| !p.2).count() > 500);
        for peer in &rig.peers {
            for ((slot, at, _), (cslot, cat, _)) in server.iter().zip(peer.props()) {
                assert_eq!(*slot, cslot);
                let d = (0..3).map(|k| (at[k] - cat[k]).powi(2)).sum::<f32>().sqrt();
                worst = worst.max(d);
            }
        }
    }
    assert!(
        worst < 0.5,
        "a client's prop was {worst} units from the server's"
    );
    // Every packet's prop records within 40% of its 450 bytes, and some had to wait for a later packet.
    for t in rig.totals() {
        assert!(t.share_bits_max > 0, "{t:?}");
        assert!(t.share_bits_max <= 450 * 8 * 2 / 5, "{t:?}");
        assert!(t.share_deferred > 0, "{t:?}");
    }
}

/// The clients' refusals heard so far, all clients together.
fn refused(rig: &Rig) -> u32 {
    rig.peers
        .iter()
        .map(|p| p.global("$Refused").parse::<u32>().unwrap_or(0))
        .sum()
}

fn command(rig: &mut Rig, peer: usize, datablock: &str) {
    let code = format!("commandToServer('PropSpawn', \"{datablock}\");");
    rig.peers[peer]
        .client
        .vm_mut()
        .eval(code.as_bytes())
        .unwrap();
}

#[test]
fn a_clients_command_is_rate_limited_by_its_address() {
    // Two clients from one address: twenty calls each in one tick, eight are heard between them.
    let (mut rig, _files) = clients(2, &[("$Test::Players", "1")], &[]);
    rig.run(10);
    for _ in 0..20 {
        command(&mut rig, 0, "Crate");
        command(&mut rig, 1, "Crate");
    }
    rig.run(6);
    // The first spawns; the next seven are heard and refused (the crate is in the way); the rest are dropped unheard.
    let whys: Vec<String> = rig.peers.iter().map(|p| p.global("$RefusedWhy")).collect();
    assert_eq!(
        rig.eval("return propCount();"),
        "1",
        "{} {whys:?}",
        refused(&rig)
    );
    assert_eq!(refused(&rig), 7);
    let owner =
        rig.eval("return propGetOwner(MissionCleanup.getObject(MissionCleanup.getCount() - 1));");
    assert_ne!(owner, "0");
}

#[test]
fn a_clients_spawn_is_checked_on_the_server() {
    let (mut rig, _files) = clients(1, &[("$Test::Players", "1")], &[]);
    rig.run(10);
    let why = |rig: &Rig| rig.peers[0].global("$RefusedWhy");
    // Not a prop datablock, and one players may not spawn.
    command(&mut rig, 0, "FloorData");
    assert!(rig.run_until(20, |r| refused(r) == 1));
    assert!(why(&rig).contains("not a prop"), "{}", why(&rig));
    command(&mut rig, 0, "Locked");
    assert!(rig.run_until(20, |r| refused(r) == 2));
    assert!(why(&rig).contains("may not spawn"), "{}", why(&rig));
    // A wall between the eye and the point: the server's ray meets it.
    rig.eval("new StaticShape(Wall) { dataBlock = FloorData; position = \"0 -6.6 0\"; scale = \"6 0.2 8\"; };");
    rig.run(2);
    command(&mut rig, 0, "Crate");
    assert!(rig.run_until(20, |r| refused(r) == 3));
    assert!(why(&rig).contains("in the way"), "{}", why(&rig));
    rig.eval("Wall.delete();");
    // A frozen crate already at the point: no room.
    let at = rig.eval(
        "%p = ClientGroup.getObject(0).player; %t = %p.getTransform();          return getWord(%t, 0) SPC getWord(%t, 1) + 2.5 SPC getWord(%t, 2) + 1.7;",
    );
    let blocker = rig.eval(&format!(
        "%c = propSpawn(Crate, \"{at}\"); propFreeze(%c); return %c;"
    ));
    assert_ne!(blocker, "0");
    rig.run(70);
    command(&mut rig, 0, "Crate");
    assert!(rig.run_until(20, |r| refused(r) == 4));
    assert!(why(&rig).contains("no room"), "{}", why(&rig));
    // Out of the way, the same request spawns.
    rig.eval(&format!("propDelete({blocker});"));
    rig.run(2);
    command(&mut rig, 0, "Crate");
    rig.run(4);
    assert_eq!(refused(&rig), 4, "{}", why(&rig));
    assert_eq!(rig.eval("return propCount();"), "1");
}

/// The connection that joined last.
fn newest(rig: &mut Rig) -> String {
    rig.eval("return ClientGroup.getObject(ClientGroup.getCount() - 1);")
}

#[test]
fn an_owner_leaves_comes_back_under_its_key_and_its_orphans_expire() {
    let (mut rig, files) = clients(1, &[("$Pref::Props::OrphanSeconds", "3")], &[]);
    let first = newest(&mut rig);
    rig.eval(&format!("{first}.propOwnerKey = \"alice\";"));
    for k in 0..3 {
        spawn(
            &mut rig,
            &format!("return propSpawn(Crate, \"{} 4 1\", {first});", k * 2),
        );
    }
    assert_eq!(rig.eval(&format!("return propCount({first});")), "3");
    let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
    // The per-player cap, by script.
    rig.eval("$Pref::Props::MaxPerPlayer = 3;");
    rig.run(1);
    let refused = rig
        .server
        .vm_mut()
        .eval(format!("return propSpawn(Crate, \"9 4 1\", {first});").as_bytes());
    assert!(refused.is_err() || matches!(refused.as_deref(), Ok(b"" | b"0")));
    // The owner leaves: its props stay, owned by nobody.
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    assert_eq!(rig.eval("return propCount();"), "3");
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    // It comes back under the same key: it owns them again.
    rig.join(&files, rig::engine(&files, true), 50_011, &[]);
    assert!(rig.run_until(200, |r| r.peers[1].ghosts() >= 2));
    let back = newest(&mut rig);
    rig.eval(&format!("{back}.propOwnerKey = \"alice\";"));
    rig.run(2);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), back);
    assert_eq!(rig.eval(&format!("return propCount({back});")), "3");
    // It leaves again, and two connections come that both claim the key: neither gets the props.
    rig.peers[1].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    for port in [50_012, 50_013] {
        let n = rig.join(&files, rig::engine(&files, true), port, &[]);
        assert!(rig.run_until(200, |r| r.peers[n].ghosts() >= 2));
    }
    rig.eval(
        "ClientGroup.getObject(ClientGroup.getCount() - 1).propOwnerKey = \"alice\";          ClientGroup.getObject(ClientGroup.getCount() - 2).propOwnerKey = \"alice\";",
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    // Past the orphan time they go.
    assert!(rig.run_until(200, |r| r.server_props().is_empty()));
}

#[test]
fn reconnecting_from_one_address_does_not_start_a_new_quota() {
    let (mut rig, files) = clients(
        1,
        &[
            ("$Pref::Props::MaxPerAddress", "6"),
            ("$Pref::Props::MaxPerPlayer", "4"),
        ],
        &[],
    );
    for (round, port) in (0..4).zip(50_020..) {
        let conn = newest(&mut rig);
        for k in 0..4 {
            let _ = rig.server.vm_mut().eval(
                format!(
                    "return propSpawn(Crate, \"{} {} 1\", {conn});",
                    k * 2,
                    round * 3
                )
                .as_bytes(),
            );
        }
        let count: u32 = rig.eval("return propCount();").parse().unwrap();
        assert!(count <= 6, "round {round}: {count}");
        // Leave and come back as a new connection (a new key) from the same address.
        let last = rig.peers.len() - 1;
        rig.peers[last].client.call("disconnect", &[]).unwrap();
        rig.run(5);
        let n = rig.join(&files, rig::engine(&files, true), port, &[]);
        assert!(rig.run_until(200, |r| r.peers[n].ghosts() >= 2));
    }
    assert_eq!(rig.eval("return propCount();"), "6");
}

#[test]
fn a_client_without_the_props_plugin_is_refused_at_the_manifest() {
    let files = rig::files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, rig::engine(&files, false), 50_001, &[]);
    rig.run(100);
    let peer = &rig.peers[0];
    assert_eq!(peer.ghosts(), 0);
    let why = peer.global("$Why");
    assert!(
        why.contains("torque.props") && why.contains("you do not have it"),
        "{why:?}"
    );
}

#[test]
fn a_prop_lost_when_the_physics_world_is_made_again_is_deleted_everywhere() {
    let (mut rig, _files) = clients(1, &[], &[]);
    for k in 0..3 {
        spawn(
            &mut rig,
            &format!("return propSpawn(Crate, \"{} 4 0.5\");", k * 2),
        );
    }
    assert!(rig.run_until(100, |r| r.peers[0].props().len() == 3));
    rig.server.drain_log();
    {
        let props = rig.props();
        let mut srv = props.0.borrow_mut();
        let world = srv.world.as_mut().expect("a world");
        // Room for the floor and one prop in the world made again.
        world.config_mut().limits.max_bodies = 2;
        world.physics_mut().poison();
    }
    rig.run(2);
    assert_eq!(rig.eval("return propCount();"), "1");
    let log = rig.server.drain_log().join(
        "
",
    );
    assert_eq!(log.matches("could not be made again").count(), 2, "{log}");
    assert_eq!(rig.objects_of("Prop"), 1);
    assert!(rig.run_until(100, |r| r.peers[0].props().len() == 1));
}

#[test]
fn an_owner_with_no_readable_address_is_capped_under_its_connection() {
    let (mut rig, _files) = clients(1, &[("$Pref::Props::MaxPerAddress", "2")], &[]);
    // A connection a script made: it has no address to read.
    let conn = rig.eval("%c = new GameConnection(); return %c;");
    for k in 0..4 {
        let _ = rig
            .server
            .vm_mut()
            .eval(format!("return propSpawn(Crate, \"{} 6 1\", {conn});", k * 2).as_bytes());
    }
    assert_eq!(rig.eval(&format!("return propCount({conn});")), "2");
}
