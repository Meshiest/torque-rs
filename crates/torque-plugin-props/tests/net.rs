//! A real server and two engine clients over the memory transport: props spawned by script and by a client's command, the state
//! each client is sent, the share of each packet the props take, sleepers that cost nothing, owners that leave.

#[path = "net/rig.rs"]
mod rig;

use rig::{PeerProps, Rig, RigProps, clients};

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
    // Every packet's prop records within 40% of the packet: 450 bytes in Torque's protocol, where some had to wait for a later
    // packet; in the engine's, the most a packet takes on the memory link (an eighth of a second at the byte rate).
    let engine =
        torque_app_server::plugins::protocol_from_env() == torque_engine::ProtocolKind::Engine;
    let packet_bytes = if engine {
        u64::from(torque_server::net::engine::DEFAULT_BYTE_RATE / 8)
    } else {
        450
    };
    for t in rig.totals() {
        assert!(t.share_bits_max > 0, "{t:?}");
        assert!(t.share_bits_max <= packet_bytes * 8 * 2 / 5, "{t:?}");
        assert!(engine || t.share_deferred > 0, "{t:?}");
    }
}

/// `propCheckPlacement` of a crate at `at` from the eye `eye`: "" or the reason.
fn check(rig: &mut Rig, at: &str, eye: &str) -> String {
    rig.eval(&format!(
        "return propCheckPlacement(Crate, \"{at}\", \"{eye}\");"
    ))
}

#[test]
fn a_placement_is_refused_in_a_wall_a_vehicle_or_a_prop() {
    let (mut rig, _files) = clients(1, &[], &[]);
    let (eye, at) = ("0 -8 2", "0 -5 2");
    assert_eq!(check(&mut rig, at, eye), "");
    // A wall between the eye and the point: the server's ray meets it.
    rig.eval("new StaticShape(Wall) { dataBlock = FloorData; position = \"0 -6.5 0\"; scale = \"6 0.2 8\"; };");
    rig.run(2);
    assert_eq!(check(&mut rig, at, eye), "something is in the way");
    rig.eval("Wall.delete();");
    rig.run(2);
    // A vehicle where the prop would go.
    rig.eval("new WheeledVehicle(Car) { dataBlock = BuggyData; position = \"0 -5 1.2\"; };");
    rig.run(2);
    let why = check(&mut rig, at, eye);
    assert!(
        why == "there is no room there" || why == "something is in the way",
        "{why:?}"
    );
    rig.eval("Car.delete();");
    rig.run(2);
    assert_eq!(check(&mut rig, at, eye), "");
    // A frozen prop already there: no room.
    let blocker = rig.eval(&format!(
        "%c = propSpawn(Crate, \"{at}\"); propFreeze(%c); return %c;"
    ));
    assert_ne!(blocker, "0");
    assert_eq!(check(&mut rig, at, eye), "there is no room there");
    rig.eval(&format!("propDelete({blocker});"));
    assert_eq!(check(&mut rig, at, eye), "");
    // Not a prop datablock, a transform or an eye that is out of the world or not a number.
    assert_eq!(
        rig.eval(&format!(
            "return propCheckPlacement(FloorData, \"{at}\", \"{eye}\");"
        )),
        "that is not a prop datablock"
    );
    assert_eq!(
        check(&mut rig, "0 0 1e40", eye),
        "the transform is out of the world"
    );
    assert_eq!(
        check(&mut rig, at, "nan 0 2"),
        "the eye is out of the world"
    );
    assert_eq!(
        check(&mut rig, at, "0 -8 1e30"),
        "the eye is out of the world"
    );
    // An axis whose length overflows is no turn.
    assert_eq!(
        check(&mut rig, "0 -5 2 1e30 0 0 1", eye),
        "the transform is out of the world"
    );
    assert_eq!(check(&mut rig, "0 -5 2 1 0 0 1", eye), "");
    // Out of reach: 32 units by default, or the reach given.
    assert_eq!(check(&mut rig, at, "0 -45 2"), "that is out of reach");
    let near = |rig: &mut Rig, reach: &str| {
        rig.eval(&format!(
            "return propCheckPlacement(Crate, \"{at}\", \"{eye}\", 0, {reach});"
        ))
    };
    assert_eq!(near(&mut rig, "2"), "that is out of reach");
    assert_eq!(near(&mut rig, "4"), "");
    assert_eq!(near(&mut rig, "-1"), "that is out of reach");
    // Only a player or a controlled object may be left out of the ray: a wall may not.
    rig.eval("new StaticShape(Post) { dataBlock = FloorData; position = \"0 -6.5 0\"; scale = \"6 0.2 8\"; };");
    rig.run(2);
    let post = rig.eval("return Post.getId();");
    assert_eq!(
        rig.eval(&format!(
            "return propCheckPlacement(Crate, \"{at}\", \"{eye}\", {post});"
        )),
        "only a player or a controlled object may be left out of the ray"
    );
    rig.eval("Post.delete();");
    rig.run(2);
    assert_eq!(
        check(&mut rig, "200000 0 2", eye),
        "the transform is out of the world"
    );
    // The caps are still propSpawn's own: a placement that is clear spawns only within them.
    rig.eval("$Pref::Props::MaxProps = 1;");
    rig.run(1);
    assert!(spawn(&mut rig, &format!("return propSpawn(Crate, \"{at}\");")) > 0);
    assert_eq!(check(&mut rig, "4 -5 2", "4 -8 2"), "");
    assert_eq!(spawn(&mut rig, "return propSpawn(Crate, \"4 -5 2\");"), 0);
    assert_eq!(rig.eval("return propCount();"), "1");
}

#[test]
fn a_players_own_figure_may_be_left_out_of_the_ray() {
    let (mut rig, _files) = clients(1, &[("$Test::Players", "1")], &[]);
    rig.run(10);
    // From inside the figure's own box, toward a point in front of it: blocked by the figure unless it is left out.
    let player = rig.eval("return ClientGroup.getObject(0).getControlObject();");
    let base = rig.eval("%t = ClientGroup.getObject(0).getControlObject().getTransform(); return getWords(%t, 0, 2);");
    let w: Vec<f32> = base
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    let eye = format!("{} {} {}", w[0], w[1], w[2] + 1.7);
    let at = format!("{} {} {}", w[0], w[1] + 2.5, w[2] + 1.7);
    assert_eq!(
        rig.eval(&format!(
            "return propCheckPlacement(Crate, \"{at}\", \"{eye}\", {player});"
        )),
        ""
    );
}

#[test]
fn a_client_sending_the_old_spawn_command_gets_nothing() {
    let (mut rig, _files) = clients(1, &[("$Test::Players", "1")], &[]);
    rig.run(10);
    for _ in 0..5 {
        rig.peers[0]
            .client
            .vm_mut()
            .eval(b"commandToServer('PropSpawn', \"Crate\");")
            .unwrap();
    }
    rig.run(20);
    assert_eq!(rig.eval("return propCount();"), "0");
    assert_eq!(rig.peers[0].global("$Refused"), "");
    assert!(rig.peers[0].props().is_empty());
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

/// Joins as the account `name` at `port` and runs until the connection is in play; the connection's object.
fn join_as(rig: &mut Rig, files: &torque_app::GameFiles, name: &'static str, port: u16) -> String {
    let n = rig.join(files, rig::signed_in_engine(files, name), port, &[]);
    assert!(
        rig.run_until(400, |r| r.peers[n].ghosts() >= 2),
        "{name} joined: {:?}",
        rig.peers[n].client.client_log
    );
    newest(rig)
}

#[test]
fn a_signed_in_player_keeps_their_role_and_their_props_across_a_reconnect() {
    let files = rig::files();
    let globals = [("$Pref::Props::OrphanSeconds", "30")];
    let mut rig = Rig::with_engine(&files, &globals, rig::signed_in_engine(&files, ""));
    rig.eval("definePermissionRole(\"builder\", \"props.*\");");
    let first = join_as(&mut rig, &files, "ada", 50_041);
    assert_eq!(
        rig.eval(&format!("return {first}.getIdentity();")),
        "id:616461"
    );
    assert_eq!(
        rig.eval(&format!("return {first}.getIdentityName();")),
        "ada"
    );
    rig.eval(&format!("{first}.addRole(\"builder\");"));
    for k in 0..2 {
        spawn(
            &mut rig,
            &format!("return propSpawn(Crate, \"{} 4 1\", {first});", k * 2),
        );
    }
    let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
    assert_eq!(rig.eval(&format!("return propCount({first});")), "2");
    // Ada leaves: her props are orphans, and nothing but her account will take them back.
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    // Bob is another account: he holds no role and gets none of her props.
    let bob = join_as(&mut rig, &files, "bob", 50_042);
    assert_eq!(
        rig.eval(&format!("return {bob}.getIdentity();")),
        "id:626f62"
    );
    assert_eq!(
        rig.eval(&format!("return {bob}.hasRole(\"builder\");")),
        "0"
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    assert_eq!(rig.eval(&format!("return propCount({bob});")), "0");
    // Ada returns on a new connection: the same identity, her role and her props, with no `propOwnerKey` anywhere.
    let back = join_as(&mut rig, &files, "ada", 50_043);
    assert_ne!(back, first, "a new connection");
    assert_eq!(
        rig.eval(&format!("return {back}.getIdentity();")),
        "id:616461"
    );
    assert_eq!(
        rig.eval(&format!("return {back}.hasRole(\"builder\");")),
        "1"
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), back);
    assert_eq!(rig.eval(&format!("return propCount({back});")), "2");
    assert_eq!(rig.eval(&format!("return propCount({bob});")), "0");
}

#[test]
fn a_games_own_owner_key_still_wins_over_the_account() {
    let files = rig::files();
    let globals = [("$Pref::Props::OrphanSeconds", "30")];
    let mut rig = Rig::with_engine(&files, &globals, rig::signed_in_engine(&files, ""));
    let first = join_as(&mut rig, &files, "ada", 50_051);
    rig.eval(&format!("{first}.propOwnerKey = \"team\";"));
    spawn(
        &mut rig,
        &format!("return propSpawn(Crate, \"0 4 1\", {first});"),
    );
    let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    // Ada's account, with no game key, is not the owner now: the prop is held for "team".
    let again = join_as(&mut rig, &files, "ada", 50_052);
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    // Another account the game gives the same key takes it.
    let bob = join_as(&mut rig, &files, "bob", 50_053);
    rig.eval(&format!("{bob}.propOwnerKey = \"team\";"));
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), bob);
    assert_ne!(again, bob);
}

/// Ada joins and spawns a prop, then `script` runs on the server and Ada leaves. Returns the rig, its files and the prop.
fn ada_with_a_prop(script: &str) -> (Rig, torque_app::GameFiles, String) {
    let files = rig::files();
    let globals = [("$Pref::Props::OrphanSeconds", "30")];
    let mut rig = Rig::with_engine(&files, &globals, rig::signed_in_engine(&files, ""));
    let first = join_as(&mut rig, &files, "ada", 50_071);
    spawn(
        &mut rig,
        &format!("return propSpawn(Crate, \"0 4 1\", {first});"),
    );
    let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
    let _ = rig.server.vm_mut().eval(script.as_bytes());
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    (rig, files, prop)
}

#[test]
fn a_script_that_redefines_getidentity_changes_no_ones_props() {
    // Whoever the script says Bob is, Bob is his own account: he gets none of Ada's props, and she gets them back.
    let (mut rig, files, prop) =
        ada_with_a_prop("function GameConnection::getIdentity(%this) { return \"id:616461\"; }");
    let bob = join_as(&mut rig, &files, "bob", 50_072);
    assert_eq!(
        rig.eval(&format!("return {bob}.getIdentity();")),
        "id:616461"
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    assert_eq!(rig.eval(&format!("return propCount({bob});")), "0");
    // And the other way: a script that gives Ada a different identity does not lose her her props.
    rig.eval("function GameConnection::getIdentity(%this) { return \"id:626f62\"; }");
    let back = join_as(&mut rig, &files, "ada", 50_073);
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), back);
    assert_eq!(rig.eval(&format!("return propCount({back});")), "1");
}

#[test]
fn an_error_in_a_script_getidentity_cannot_orphan_a_players_props() {
    let (mut rig, files, prop) = ada_with_a_prop(
        "function GameConnection::getIdentity(%this) { return %this.getIdentity(); }",
    );
    let back = join_as(&mut rig, &files, "ada", 50_074);
    assert!(
        rig.server
            .vm_mut()
            .eval(format!("return {back}.getIdentity();").as_bytes())
            .is_err(),
        "the script method fails"
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), back);
    assert_eq!(rig.eval(&format!("return propCount({back});")), "1");
}

#[test]
fn an_account_signed_in_twice_has_one_live_session_unless_the_game_allows_more() {
    for (several, port) in [(false, 50_081), (true, 50_083)] {
        let files = rig::files();
        let flag = if several { "0" } else { "1" };
        let globals = [
            ("$Pref::Props::OrphanSeconds", "30"),
            ("$Pref::Server::OneSessionPerAccount", flag),
        ];
        let mut rig = Rig::with_engine(&files, &globals, rig::signed_in_engine(&files, ""));
        let first = join_as(&mut rig, &files, "ada", port);
        spawn(
            &mut rig,
            &format!("return propSpawn(Crate, \"0 4 1\", {first});"),
        );
        let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
        let second = join_as(&mut rig, &files, "ada", port + 1);
        rig.run(5);
        let live = rig.eval("return ClientGroup.getCount();");
        let owner = rig.eval(&format!("return propGetOwner({prop});"));
        if several {
            assert_eq!((live.as_str(), owner.as_str()), ("2", first.as_str()));
        } else {
            // The older session is gone, and the prop follows the live one.
            assert_eq!(live, "1");
            assert_eq!(owner, second);
            assert_eq!(rig.eval(&format!("return propCount({second});")), "1");
        }
    }
}

#[test]
fn anonymous_players_are_new_players_on_every_connection() {
    let (mut rig, files) = clients(1, &[("$Pref::Props::OrphanSeconds", "30")], &[]);
    rig.eval("definePermissionRole(\"builder\", \"props.*\");");
    let first = newest(&mut rig);
    let key = rig.eval(&format!("return {first}.getIdentity();"));
    assert!(key.starts_with("conn:"), "{key}");
    assert_eq!(rig.eval(&format!("return {first}.getIdentityName();")), "");
    rig.eval(&format!("{first}.addRole(\"builder\");"));
    spawn(
        &mut rig,
        &format!("return propSpawn(Crate, \"0 4 1\", {first});"),
    );
    let prop = rig.eval("return MissionCleanup.getObject(MissionCleanup.getCount() - 1);");
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    // The same client, joining again: another connection, so another identity, no role and no claim on the old props.
    rig.join(&files, rig::engine(&files, true), 50_061, &[]);
    assert!(rig.run_until(200, |r| r.peers[1].ghosts() >= 2));
    let back = newest(&mut rig);
    let again = rig.eval(&format!("return {back}.getIdentity();"));
    assert!(
        again.starts_with("conn:") && again != key,
        "{key} then {again}"
    );
    assert_eq!(
        rig.eval(&format!("return {back}.hasRole(\"builder\");")),
        "0"
    );
    rig.run(5);
    assert_eq!(rig.eval(&format!("return propGetOwner({prop});")), "0");
    assert_eq!(rig.eval(&format!("return propCount({back});")), "0");
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

/// The props plugin as it was before holds: the same id and everything else, an older network version.
struct OldProps;

impl torque_engine::Plugin for OldProps {
    fn net_version(&self) -> Option<u32> {
        Some(torque_plugin_props::NET_VERSION - 1)
    }

    fn build(&self, engine: &mut torque_engine::EngineBuilder) {
        torque_plugin_props::PropsPlugin.build(engine);
    }
}

impl torque_engine::PluginType for OldProps {
    const ID: torque_engine::PluginId = torque_engine::PluginId::new("torque.props");
}

#[test]
fn a_client_of_the_older_props_version_is_refused_at_the_manifest() {
    use torque_plugin_props::NET_VERSION;
    assert_eq!(
        NET_VERSION, 5,
        "the version moved with the wire format (the hold, the held flag, the datablock's collision mode, a slot of 12 bits, the frozen channel)"
    );
    let files = rig::files();
    let mut rig = Rig::new(&files, &[]);
    let config = torque_app::config::GameConfig::load(&files).expect("game.toml");
    use torque_app::config::EngineConfig as _;
    let mut old = torque_engine::EngineBuilder::new();
    old.add_plugins((torque_app::plugins::DefaultPlugins, OldProps))
        .config(&config);
    rig.join(&files, old.build().expect("the engine builds"), 50_001, &[]);
    rig.run(100);
    let peer = &rig.peers[0];
    assert_eq!(peer.ghosts(), 0);
    let why = peer.global("$Why");
    assert!(
        why.contains("torque.props") && why.contains(&format!("you have {}", NET_VERSION - 1)),
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

#[test]
fn no_script_text_is_built_from_what_a_player_controls() {
    let (mut rig, _files) = clients(1, &[], &[]);
    let conn = newest(&mut rig);
    // An owner key, a datablock name and a transform shaped like script: each is data, never run.
    rig.eval(&format!(
        "{conn}.propOwnerKey = \"\\\"); $Injected = 1; (\\\"\";"
    ));
    let id = spawn(
        &mut rig,
        &format!("return propSpawn(Crate, \"0 4 1\", {conn});"),
    );
    assert!(id > 0);
    let why = rig.eval(
        "return propCheckPlacement(\"Crate\\\"); $Injected = 2; (\\\"\", \"0 -5 2\", \"0 -8 2\");",
    );
    assert_eq!(why, "that is not a prop datablock");
    let _ = rig.eval("return propCheckPlacement(Crate, \"0 -5 2; $Injected = 3;\", \"0 -8 2\");");
    rig.run(5);
    assert_eq!(rig.eval("return $Injected;"), "");
    assert_eq!(rig.eval(&format!("return propCount({conn});")), "1");
}
