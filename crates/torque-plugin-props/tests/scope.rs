//! The scope control a plugin keeps objects out of a connection's ghosts with, against a real server and clients: an ordinary
//! object, an always-in-scope one and the ghost-always set, for one connection and for every one. (Against a scene hierarchy:
//! torque-plugin-hierarchy's `net` tests.)

use crate::rig;

use rig::{Rig, clients};
use torque_engine::scope::{Audience, ScopeControl};

/// The server's connection objects, in the order the clients joined.
fn connections(rig: &mut Rig) -> Vec<u32> {
    let n: usize = rig.eval("return ClientGroup.getCount();").parse().unwrap();
    (0..n)
        .map(|k| {
            rig.eval(&format!("return ClientGroup.getObject({k});"))
                .parse()
                .unwrap()
        })
        .collect()
}

fn scope(rig: &Rig) -> std::rc::Rc<ScopeControl> {
    ScopeControl::of(rig.server.vm()).expect("a scope control (the props plugin asks for one)")
}

fn ghosts(rig: &Rig) -> Vec<usize> {
    rig.peers.iter().map(|p| p.ghosts()).collect()
}

fn make(rig: &mut Rig, class: &str, at: &str) -> u32 {
    rig.eval(&format!(
        "return new {class}() {{ dataBlock = FloorData; position = \"{at}\"; scale = \"1 1 1\"; }};"
    ))
    .parse()
    .expect("an object")
}

#[test]
fn an_object_is_hidden_from_one_connection_or_from_all_and_comes_back_when_shown() {
    for class in ["StaticShape", "ScopeAlwaysShape"] {
        let (mut rig, _files) = clients(2, &[], &[]);
        let conns = connections(&mut rig);
        let base = ghosts(&rig);
        let obj = make(&mut rig, class, "5 5 0.5");
        assert!(
            rig.run_until(200, |r| ghosts(r) == [base[0] + 1, base[1] + 1]),
            "{class}"
        );
        // Hidden from the first connection alone: its ghost goes (no update was pending), the second client keeps its own.
        let scope = scope(&rig);
        assert_eq!(
            scope.hide(rig.server.vm(), obj, Audience::Connection(conns[0])),
            Ok(true)
        );
        assert!(
            rig.run_until(200, |r| ghosts(r) == [base[0], base[1] + 1]),
            "{class}"
        );
        rig.run(100);
        assert_eq!(
            ghosts(&rig),
            [base[0], base[1] + 1],
            "{class}: it stays hidden"
        );
        // Shown again: a new ghost, whether or not the object is always in scope.
        assert!(scope.show(obj, Audience::Connection(conns[0])));
        assert!(
            rig.run_until(200, |r| ghosts(r) == [base[0] + 1, base[1] + 1]),
            "{class}"
        );
        // Hidden from everyone, then from nobody.
        assert_eq!(
            scope.hide(rig.server.vm(), obj, Audience::Everyone),
            Ok(true)
        );
        assert!(rig.run_until(200, |r| ghosts(r) == base), "{class}");
        assert!(scope.show(obj, Audience::Everyone));
        assert!(
            rig.run_until(200, |r| ghosts(r) == [base[0] + 1, base[1] + 1]),
            "{class}"
        );
        // A script that scopes it to a client is refused while it is hidden, and deleting it forgets the entry.
        scope
            .hide(rig.server.vm(), obj, Audience::Connection(conns[1]))
            .unwrap();
        assert!(
            rig.run_until(200, |r| ghosts(r) == [base[0] + 1, base[1]]),
            "{class}"
        );
        rig.eval(&format!("{}.scopeToClient({obj});", conns[1]));
        rig.run(60);
        assert_eq!(
            ghosts(&rig),
            [base[0] + 1, base[1]],
            "{class}: scopeToClient is refused"
        );
        rig.eval(&format!("{obj}.delete();"));
        rig.run(10);
        assert_eq!(scope.hidden_count(Audience::Connection(conns[1])), 0);
        assert!(rig.run_until(200, |r| ghosts(r) == base), "{class}");
    }
}

#[test]
fn an_always_in_scope_object_hidden_before_a_client_joins_never_reaches_it_and_the_client_is_ready()
{
    let files = rig::files();
    let mut rig = Rig::new(&files, &[]);
    let obj = make(&mut rig, "ScopeAlwaysShape", "5 5 0.5");
    let scope = scope(&rig);
    scope
        .hide(rig.server.vm(), obj, Audience::Everyone)
        .unwrap();
    rig.join(&files, rig::engine(&files, true), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    rig.run(100);
    let held = rig.peers[0].ghosts();
    // The first client has what any client of an empty world has; the hidden object is not in it.
    let mut other = Rig::new(&files, &[]);
    other.join(&files, rig::engine(&files, true), 50_001, &[]);
    assert!(other.run_until(400, |r| r.peers[0].ghosts() >= 2));
    other.run(100);
    assert_eq!(held, other.peers[0].ghosts());
    scope.show(obj, Audience::Everyone);
    assert!(rig.run_until(200, |r| r.peers[0].ghosts() == held + 1));
}

#[test]
fn a_rider_still_sees_the_vehicle_it_rides_when_the_vehicle_is_hidden_from_it() {
    let files = rig::files();
    let mut rig = Rig::new(&files, &[("$Test::Players", "1")]);
    rig.join(&files, rig::engine(&files, true), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let conn = connections(&mut rig)[0];
    let player: u32 = rig
        .eval(&format!("return {conn}.getControlObject();"))
        .parse()
        .expect("a player");
    let car: u32 = rig
        .eval("return new WheeledVehicle(Car) { dataBlock = BuggyData; position = \"8 8 1.2\"; };")
        .parse()
        .expect("a vehicle");
    let far = make(&mut rig, "StaticShape", "20 20 0.5");
    let base = ghosts(&rig)[0];
    assert!(rig.run_until(200, |r| ghosts(r)[0] == base + 2));
    // Both hidden from the player's connection: the ones it is not riding go.
    let scope = scope(&rig);
    for obj in [car, far] {
        scope
            .hide(rig.server.vm(), obj, Audience::Connection(conn))
            .unwrap();
    }
    assert!(rig.run_until(200, |r| ghosts(r)[0] == base));
    // It mounts the vehicle: the veto does not hold for it any more, and the vehicle comes back; the other stays hidden.
    rig.eval(&format!("{car}.mountObject({player}, 0);"));
    assert!(rig.run_until(200, |r| ghosts(r)[0] == base + 1));
    rig.run(100);
    assert_eq!(
        ghosts(&rig)[0],
        base + 1,
        "the player can see what it rides"
    );
    // It gets off: the vehicle is hidden from it again.
    rig.eval(&format!("{car}.unmountObject({player});"));
    assert!(rig.run_until(300, |r| ghosts(r)[0] == base));
}
