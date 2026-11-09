//! Joints over the memory transport: what a server's script makes reaches every client's book with its ends as the client's own
//! ghosts, a winch's length follows, a joint goes with its prop, a grapple puts the server's tether on the client's predicted
//! Player bit for bit, and the server's natives refuse what is not theirs to make.

#[path = "net/rig.rs"]
mod rig;

use rig::{Rig, clients};
use torque_game::player_tether::TetherSource;
use torque_plugin_props::ClientJoints;
use torque_plugin_props::joints::KindId;
use torque_plugin_props::joints::client::{ClientTethers, JointBook};
use torque_plugin_props::joints::records::{EndWire, InitWire, JointGhost, LengthWire};

fn number(rig: &mut Rig, code: &str) -> u32 {
    rig.eval(code).parse().unwrap_or(0)
}

fn joints(rig: &Rig, peer: usize) -> ClientJoints {
    rig.peers[peer]
        .engine
        .resource::<ClientJoints>()
        .expect("the client's joints")
        .clone()
}

#[test]
fn a_joint_reaches_every_client_with_its_ends_as_their_own_ghosts_and_goes_with_its_prop() {
    let (mut rig, _files) = clients(2, &[], &[]);
    let a = number(&mut rig, "return propSpawn(Crate, \"0 0 3\");");
    let b = number(&mut rig, "return propSpawn(Crate, \"3 0 3\");");
    let tether = number(&mut rig, &format!("return jointTether({a}, {b}, 3);"));
    let winch = number(
        &mut rig,
        &format!("return jointTether({a} SPC \"0 0 0.5\", \"0 0 9\", 5);"),
    );
    assert!(a > 0 && b > 0 && tether > 0 && winch > 0);
    assert_eq!(rig.eval("return jointCount();"), "2");
    assert_eq!(rig.eval(&format!("return jointCountOn({a});")), "2");
    // An end whose prop the client was not sent yet is sent again once it is.
    let resolved = |r: &Rig, p: usize| {
        let book = joints(r, p);
        let b = book.0.borrow();
        b.len() == 2
            && b.joints()
                .values()
                .all(|j| ![j.a, j.b].iter().any(|e| matches!(e, EndWire::Free)))
    };
    assert!(rig.run_until(200, |r| (0..2).all(|p| resolved(r, p))));
    for p in 0..2 {
        let book = joints(&rig, p);
        let b = book.0.borrow();
        let kinds: Vec<KindId> = b.joints().values().map(|j| j.kind).collect();
        assert_eq!(kinds, [KindId::Tether, KindId::Tether]);
        // Every prop end is a ghost the client holds a prop for; the winch's far end is its point.
        let props = rig.peers[p].book();
        for j in b.joints().values() {
            for end in [j.a, j.b] {
                match end {
                    EndWire::Ghost { ghost, .. } => {
                        assert!(props.0.borrow().get(ghost).is_some(), "{end:?}")
                    }
                    EndWire::Point(at) => assert_eq!(at, [0.0, 0.0, 9.0]),
                    other => panic!("{other:?}"),
                }
            }
        }
    }
    // A winch: its length is the server's on every client.
    assert_eq!(rig.eval(&format!("return jointWinch({winch}, 2, 4);")), "1");
    assert_eq!(rig.eval(&format!("return jointGetKind({winch});")), "winch");
    rig.run(60);
    assert_eq!(rig.eval(&format!("return jointGetLength({winch});")), "2");
    assert!(rig.run_until(60, |r| (0..2).all(|p| {
        joints(r, p)
            .0
            .borrow()
            .joints()
            .values()
            .any(|j| j.kind == KindId::Winch && j.length == 2.0)
    })));
    // The prop goes: its joints go on the server and every client.
    assert_eq!(rig.eval(&format!("return propDelete({a});")), "1");
    rig.run(4);
    assert_eq!(rig.eval("return jointCount();"), "0");
    assert_eq!(rig.objects_of("Joint"), 0);
    assert!(rig.run_until(100, |r| (0..2).all(|p| joints(r, p).0.borrow().is_empty())));
}

#[test]
fn a_grapple_gives_the_clients_prediction_the_servers_tether_exactly() {
    let (mut rig, _files) = clients(1, &[("$Test::Players", "1")], &[]);
    let player = number(
        &mut rig,
        "return ClientGroup.getObject(0).getControlObject();",
    );
    assert!(player > 0);
    let grapple = number(
        &mut rig,
        &format!("return jointGrapple({player} SPC \"0 0 1\", \"0 -5 8\", 4.5);"),
    );
    assert!(grapple > 0, "{:?}", rig.server.drain_log());
    assert_eq!(
        rig.eval(&format!("return jointGetKind({grapple});")),
        "grapple"
    );
    // One grapple a player, and none held past the world's bound (the wire would hold it there).
    for code in [
        format!("return jointGrapple({player}, \"0 5 8\", 3);"),
        format!("return jointGrapple({player} SPC \"0 0 1e6\", \"0 5 8\", 3);"),
    ] {
        assert_eq!(number(&mut rig, &code), 0, "{code}");
    }
    assert_eq!(rig.eval("return jointCount();"), "1");
    assert!(rig.run_until(200, |r| joints(r, 0).0.borrow().len() == 1));
    rig.run(30);
    // The hard grapple pulled the player up toward the point: its hand within the length of it.
    let at = rig.eval(&format!("return {player}.getPosition();"));
    let p: Vec<f32> = at.split(' ').filter_map(|w| w.parse().ok()).collect();
    let hand = [p[0], p[1], p[2] + 1.0];
    let d = ((hand[1] + 5.0).powi(2) + (hand[2] - 8.0).powi(2) + hand[0].powi(2)).sqrt();
    assert!(p[2] > 2.0 && d <= 4.5 + 0.05, "the player is at {at}");
    assert_eq!(
        rig.eval(&format!("return jointSetSpring({grapple}, 900, 60);")),
        "1"
    );
    rig.run(10);
    let server = rig
        .props()
        .with(|s| s.world.as_ref().and_then(|w| w.tether_of(player)))
        .flatten()
        .expect("the server's tether");
    // The client knows the Player by its ghost: the tether its prediction is given is the server's, bit for bit.
    let book = joints(&rig, 0);
    let ghost = match book.0.borrow().joints().values().next().map(|j| j.a) {
        Some(EndWire::Ghost { ghost, .. }) => ghost,
        other => panic!("{other:?}"),
    };
    let source = ClientTethers {
        joints: book.clone(),
        props: rig.peers[0].book(),
    };
    let client = source.tether(ghost).expect("the client's tether");
    assert_eq!(client, server);
    assert_eq!(client.anchor, [0.0, -5.0, 8.0]);
    assert_eq!(
        (client.length, client.stiffness, client.damping),
        (4.5, 900.0, 60.0)
    );
    // A player that goes takes its grapple.
    rig.eval(&format!("{player}.delete();"));
    rig.run(4);
    assert_eq!(rig.eval("return jointCount();"), "0");
}

#[test]
fn the_natives_refuse_bad_ends_and_hold_values_to_their_ranges() {
    let (mut rig, _files) = clients(1, &[], &[]);
    let a = number(&mut rig, "return propSpawn(Crate, \"0 0 3\");");
    let refused = [
        "return jointTether(\"1 2 3\", \"4 5 6\", 2);".to_string(),
        format!("return jointTether({a}, {a}, 2);"),
        "return jointTether(NoSuchThing, \"0 0 9\", 2);".to_string(),
        format!("return jointTether({a}, \"nan 0 9\", 2);"),
        format!("return jointGrapple({a}, \"0 0 9\", 2);"),
        format!("return jointWeld(\"0 0 9\", {a});"),
        format!("return jointHinge({a}, \"\", \"0 0 3\", \"0 0 0\");"),
    ];
    for code in &refused {
        assert_eq!(number(&mut rig, code), 0, "{code}");
    }
    assert_eq!(rig.eval("return jointCount();"), "0");
    let j = number(
        &mut rig,
        &format!("return jointTether({a}, \"0 0 9\", 1e9);"),
    );
    assert_eq!(rig.eval(&format!("return jointGetLength({j});")), "200");
    assert_eq!(
        rig.eval(&format!("return jointWinch({j}, 3, 1e9, -5);")),
        "1"
    );
    assert_eq!(
        rig.eval(&format!("return jointSetLimits({j}, 0, 1);")),
        "0",
        "a tether is no hinge"
    );
    // The check a game's command calls first: out of reach, then in reach.
    let client = rig.eval("return ClientGroup.getObject(0);");
    assert_eq!(
        rig.eval(&format!(
            "return jointCheck({client}, {a}, \"0 0 9\", \"50 0 3\", 8);"
        )),
        "that is out of reach"
    );
    assert_eq!(
        rig.eval(&format!(
            "return jointCheck({client}, {a}, \"0 0 5\", \"1 0 3\", 8);"
        )),
        ""
    );
    // Joints have no client entry point: a client's command for one does nothing.
    rig.eval(&format!("jointDelete({j});"));
    for name in ["JointTether", "JointWeld", "JointGrapple", "Joint"] {
        rig.peers[0]
            .client
            .vm_mut()
            .eval(format!("commandToServer('{name}', {a}, \"0 0 9\", 2);").as_bytes())
            .unwrap();
    }
    rig.run(20);
    assert_eq!(rig.eval("return jointCount();"), "0");
}

/// The connection that joined last.
fn newest(rig: &mut Rig) -> String {
    rig.eval("return ClientGroup.getObject(ClientGroup.getCount() - 1);")
}

#[test]
fn a_player_may_only_grapple_itself_and_only_where_the_server_has_it() {
    let (mut rig, _files) = clients(2, &[("$Test::Players", "1")], &[]);
    let (first, second) = (
        rig.eval("return ClientGroup.getObject(0);"),
        rig.eval("return ClientGroup.getObject(1);"),
    );
    let mine = rig.eval(&format!("return {first}.getControlObject();"));
    let theirs = rig.eval(&format!("return {second}.getControlObject();"));
    let eye = rig.eval(&format!("return {mine}.getEyePoint();"));
    let check = |rig: &mut Rig, player: &str, eye: &str, others: &str| {
        rig.eval(&format!(
            "return jointCheck({first}, \"{player}\", \"0 -6 4\", \"{eye}\", 8, grapple, \"\"{others});"
        ))
    };
    assert_eq!(check(&mut rig, &mine, &eye, ""), "");
    // Another player: refused, unless the game allows it.
    let other_eye = rig.eval(&format!("return {theirs}.getEyePoint();"));
    assert_eq!(
        check(&mut rig, &theirs, &other_eye, ""),
        "a player may only join itself"
    );
    assert_eq!(check(&mut rig, &theirs, &other_eye, ", 1"), "");
    // The player's position is the server's own, not the eye's: an eye far from it does not stand in for it.
    assert_eq!(
        check(&mut rig, &mine, "60 60 4", ""),
        "that is out of reach"
    );
    // A client that is not there asks nothing.
    assert_eq!(
        rig.eval(&format!(
            "return jointCheck(0, \"{mine}\", \"0 -6 4\", \"{eye}\", 8, grapple);"
        )),
        "there is no such client"
    );
}

#[test]
fn a_client_may_join_only_its_own_props_or_no_ones_and_a_hinge_its_point_in_reach() {
    let (mut rig, _files) = clients(2, &[], &[]);
    let (first, second) = (
        rig.eval("return ClientGroup.getObject(0);"),
        rig.eval("return ClientGroup.getObject(1);"),
    );
    let mine = number(
        &mut rig,
        &format!("return propSpawn(Crate, \"0 0 3\", {first});"),
    );
    let theirs = number(
        &mut rig,
        &format!("return propSpawn(Crate, \"2 0 3\", {second});"),
    );
    let nobodys = number(&mut rig, "return propSpawn(Crate, \"0 2 3\");");
    assert!(mine > 0 && theirs > 0 && nobodys > 0);
    let mut check = |rest: &str| rig.eval(&format!("return jointCheck({first}, {rest});"));
    assert_eq!(check(&format!("{mine}, \"0 0 6\", \"1 1 3\", 8")), "");
    assert_eq!(check(&format!("{nobodys}, {mine}, \"1 1 3\", 8")), "");
    assert_eq!(
        check(&format!("{mine}, {theirs}, \"1 1 3\", 8")),
        "that prop is someone else's"
    );
    assert_eq!(
        check(&format!("{theirs}, \"2 0 6\", \"1 1 3\", 8")),
        "that prop is someone else's"
    );
    assert_eq!(
        check(&format!("{mine}, {theirs}, \"1 1 3\", 8, \"\", \"\", 0, 1")),
        "",
        "the game allows it"
    );
    // A hinge gives its point, in reach; the world is a weld's or a hinge's end, never a tether's.
    assert_eq!(
        check(&format!("{mine}, \"\", \"1 1 3\", 8, hinge")),
        "a hinge needs its point"
    );
    assert_eq!(
        check(&format!("{mine}, \"\", \"1 1 3\", 8, hinge, \"0 0 3.5\"")),
        ""
    );
    assert_eq!(
        check(&format!("{mine}, \"\", \"1 1 3\", 8, hinge, \"40 0 3\"")),
        "that is out of reach"
    );
    assert_eq!(check(&format!("{mine}, \"\", \"1 1 3\", 8, weld")), "");
    assert_eq!(
        check(&format!("{mine}, \"\", \"1 1 3\", 8")),
        "the ends must be two things: props, points, or a player and a prop or point"
    );
    assert_eq!(
        check(&format!("{mine}, \"\", \"1 1 3\", 8, rope")),
        "that is not a kind of joint"
    );
}

#[test]
fn a_callback_that_stops_on_an_error_is_printed_and_the_ones_after_it_still_run() {
    let (mut rig, _files) = clients(1, &[], &[]);
    let a = number(&mut rig, "return propSpawn(Crate, \"0 0 0.5\");");
    let b = number(&mut rig, "return propSpawn(Crate, \"3 0 0.5\");");
    assert!(rig.run_until(600, |r| {
        let asleep = |p: u32| {
            r.props().with(|s| {
                s.world
                    .as_ref()
                    .and_then(|w| w.get(p))
                    .map(|q| q.rest.is_asleep())
            })
        };
        asleep(a) == Some(Some(true)) && asleep(b) == Some(Some(true))
    }));
    // The first prop's wake and the first joint's break stop on an error (a call too deep); the second's still run.
    rig.eval(&format!(
        "$First = {a}; \
         function Crate::onPropWake(%this, %prop) {{ if (%prop == $First) %this.onPropWake(%prop); $Woke++; }} \
         function Joint::onBreak(%this) {{ if (%this == $FirstJoint) %this.onBreak(); $Broke++; }}"
    ));
    let j = number(&mut rig, &format!("return jointTether({a}, {b}, 2.5);"));
    let k = number(&mut rig, &format!("return jointTether({b}, \"3 0 9\", 1);"));
    assert!(j > 0 && k > 0);
    rig.eval(&format!(
        "$FirstJoint = {j}; jointSetBreakForce({j}, 0.001); jointSetBreakForce({k}, 0.001);"
    ));
    rig.server.drain_log();
    rig.step();
    assert_eq!(
        rig.eval(&format!("return jointIsBroken({j}) + jointIsBroken({k});")),
        "2"
    );
    assert_eq!(rig.eval("return $Woke;"), "1", "the second prop's wake ran");
    assert_eq!(
        rig.eval("return $Broke;"),
        "1",
        "the second joint's onBreak ran"
    );
    let log: Vec<String> = rig.server.drain_log();
    for what in [
        format!("prop {a}'s onPropWake"),
        format!("joint {j}'s onBreak"),
    ] {
        assert!(log.iter().any(|l| l.contains(&what)), "{what} in {log:?}");
    }
}

#[test]
fn a_clients_book_with_two_grapples_on_one_player_gives_it_no_tether() {
    let grapple = |ghost: u32| JointGhost {
        init: Some(InitWire {
            kind: KindId::Grapple,
            a: EndWire::Ghost {
                ghost,
                offset: [0.0; 3],
            },
            b: EndWire::Point([0.0, 0.0, 9.0]),
        }),
        length: Some(LengthWire {
            length: 4.0,
            min: 0.0,
        }),
        ..JointGhost::default()
    };
    let mut book = JointBook::default();
    book.heard(1, &grapple(5));
    book.heard(2, &grapple(6));
    assert!(book.grapple_on(5).is_some() && book.grapple_on(6).is_some());
    // The server holds one a player: two in a book are between its deleting one and making the next.
    book.heard(3, &grapple(5));
    assert!(book.grapple_on(5).is_none());
    book.removed(1);
    assert_eq!(book.grapple_on(5), book.get(3));
}

#[test]
fn reconnecting_from_one_address_does_not_start_a_new_joint_quota() {
    let (mut rig, files) = clients(
        1,
        &[
            ("$Pref::Props::MaxJointsPerAddress", "6"),
            ("$Pref::Props::MaxJointsPerPlayer", "4"),
        ],
        &[],
    );
    let crate_id = number(&mut rig, "return propSpawn(Crate, \"0 0 1\");");
    for (round, port) in (0..4).zip(50_020..) {
        let conn = newest(&mut rig);
        for _ in 0..4 {
            let _ = rig
                .server
                .vm_mut()
                .eval(format!("return jointTether({crate_id}, \"0 0 6\", 5, {conn});").as_bytes());
        }
        let count: u32 = rig.eval("return jointCount();").parse().unwrap();
        assert!(count <= 6, "round {round}: {count}");
        // Leave and come back as a new connection (a new key) from the same address.
        let last = rig.peers.len() - 1;
        rig.peers[last].client.call("disconnect", &[]).unwrap();
        rig.run(5);
        let n = rig.join(&files, rig::engine(&files, true), port, &[]);
        assert!(rig.run_until(200, |r| r.peers[n].ghosts() >= 2));
    }
    assert_eq!(rig.eval("return jointCount();"), "6");
}

#[test]
fn a_leaving_owners_joints_are_orphans_and_expire_with_its_props() {
    let (mut rig, files) = clients(1, &[("$Pref::Props::OrphanSeconds", "3")], &[]);
    let first = newest(&mut rig);
    rig.eval(&format!("{first}.propOwnerKey = \"alice\";"));
    let a = number(&mut rig, "return propSpawn(Crate, \"0 4 1\");");
    let j = number(
        &mut rig,
        &format!("return jointTether({a}, \"0 4 6\", 5, {first});"),
    );
    assert!(j > 0);
    assert_eq!(rig.eval(&format!("return jointCount({first});")), "1");
    // The owner leaves: the joint stays, an orphan, and its owner takes it back under the same key.
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    assert_eq!(rig.eval("return jointCount();"), "1");
    rig.join(&files, rig::engine(&files, true), 50_011, &[]);
    assert!(rig.run_until(200, |r| r.peers[1].ghosts() >= 2));
    let back = newest(&mut rig);
    rig.eval(&format!("{back}.propOwnerKey = \"alice\";"));
    rig.run(2);
    assert_eq!(rig.eval(&format!("return jointCount({back});")), "1");
    // It leaves for good: past the orphan time the joint goes, object and all.
    rig.peers[1].client.call("disconnect", &[]).unwrap();
    rig.run(3 * 1000 / 32 + 20);
    assert_eq!(rig.eval("return jointCount();"), "0");
    assert_eq!(rig.objects_of("Joint"), 0);
}
