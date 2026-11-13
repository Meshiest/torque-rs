//! Offsets on a networked rope's ends and a rope that follows a driver, end to end: a real server and engine clients over the memory
//! transport.

use crate::rig;

use std::cell::Cell;
use std::rc::Rc;

use rig::{PeerRopes, Rig, RigRopes, create, engine, engine_with, files, off_the_line, rope};
use torque_app::GameFiles;
use torque_engine::{Engine, EngineBuilder, Plugin, PluginId, PluginType};
use torque_plugin_rope::{
    Anchor, Drive, DriveEnd, RopeDriver, RopeDrivers, RopeEnd, RopePlugin, net::NET_VERSION,
};
use torque_ts_vm::vm::Vm;

/// A driver that answers with a closure of the key it is asked with.
struct Answers(Rc<dyn Fn(u32) -> Option<Drive>>);

impl RopeDriver for Answers {
    fn drive(&self, _vm: &Vm, key: u32) -> Option<Drive> {
        (self.0)(key)
    }
}

fn with_driver(
    files: &GameFiles,
    name: &str,
    answer: impl Fn(u32) -> Option<Drive> + 'static,
) -> Engine {
    engine_with(
        files,
        RopePlugin::default().with_driver(name, Answers(Rc::new(answer))),
    )
}

/// The index of client `peer`'s ghost of the server's `Mover`, found by hanging a rope from it and taking the rope away again.
fn mover(rig: &mut Rig, peer: usize) -> u32 {
    let probe = create(rig, "Cable");
    rig.eval(&format!("ropeNetAttach({probe}, 1, Mover);"));
    rig.run(10);
    let ghost = match rope(&rig.peers[peer]).1.anchor(RopeEnd::End) {
        Anchor::Ghost { ghost, .. } => ghost,
        other => panic!("the probe hangs from {other:?}"),
    };
    rig.eval(&format!("ropeNetDelete({probe});"));
    rig.run(10);
    assert!(rig.peers[peer].rope_ids().is_empty());
    ghost
}

/// A server and one client of `engine`, connected and ghosted.
fn one(files: &GameFiles, engine: Engine) -> Rig {
    let mut rig = Rig::new(files, &[]);
    rig.join(files, engine, 50_001, &[]);
    assert!(
        rig.run_until(400, |r| r.peers[0].ghosts() >= 2),
        "{:?}",
        rig.peers[0].client.client_log
    );
    rig
}

fn near(a: [f32; 3], b: [f32; 3], within: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= within)
}

#[test]
fn an_end_attached_at_an_offset_carries_it_to_every_client_and_a_plain_attach_clears_it() {
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, engine(&files, true), 50_001, &[]);
    rig.join(&files, engine(&files, true), 50_002, &[]);
    assert!(rig.run_until(400, |r| r.peers.iter().all(|p| p.ghosts() >= 3)));
    let keys = [mover(&mut rig, 0), mover(&mut rig, 1)];
    let id = create(&mut rig, "Cable");
    rig.eval(&format!(
        "ropeNetAttachAt({id}, 1, Mover, \"\", \"0 2 0.5\");"
    ));
    rig.run(10);
    for (peer, key) in rig.peers.iter().zip(keys) {
        let (_, held) = rope(peer);
        assert_eq!(
            held.anchor(RopeEnd::End),
            Anchor::Ghost {
                ghost: key,
                node: String::new(),
                offset: [0.0, 2.0, 0.5]
            },
            "{:?}",
            peer.client.client_log
        );
    }
    // A client that draws no frame follows the ghost's position plus the offset.
    for k in 0..60 {
        rig.move_mover(k as f32 * 0.05);
        rig.step();
    }
    rig.run(20);
    for peer in &rig.peers {
        let end = rope(peer).1.points().last().unwrap();
        assert!(near(end, [59.0 * 0.05, 2.0, 1.5], 0.3), "{end:?}");
    }
    // The old function holds the object's own point again.
    rig.eval(&format!("ropeNetAttach({id}, 1, Mover);"));
    rig.run(10);
    for peer in &rig.peers {
        assert!(matches!(
            rope(peer).1.anchor(RopeEnd::End),
            Anchor::Ghost {
                offset: [0.0, 0.0, 0.0],
                ..
            }
        ));
    }
}

#[test]
fn a_driven_rope_follows_what_its_driver_says_on_the_client_and_nothing_else() {
    let files = files();
    let engine = with_driver(&files, "tether", |key| {
        let mut drive = Drive::new(
            DriveEnd::Point([0.0, 0.0, 3.0]),
            DriveEnd::Ghost {
                ghost: key,
                node: String::new(),
                offset: [0.5, 0.0, 0.0],
            },
        );
        drive.length = Some(4.0);
        drive.at_limit = true;
        Some(drive)
    });
    let mut rig = one(&files, engine);
    let key = mover(&mut rig, 0);
    let id = create(&mut rig, "Cable");
    rig.eval(&format!("ropeNetDrive({id}, \"tether\", Mover);"));
    rig.run(20);
    assert_eq!(bound(&rig.peers[0]), 1);
    let (_, held) = rope(&rig.peers[0]);
    assert_eq!(
        held.anchor(RopeEnd::End),
        Anchor::Ghost {
            ghost: key,
            node: String::new(),
            offset: [0.5, 0.0, 0.0]
        }
    );
    assert_eq!(held.anchor(RopeEnd::Start), Anchor::Point([0.0, 0.0, 3.0]));
    assert!((held.length() - 4.0).abs() < 1.0e-3, "{}", held.length());
    // The server's own record says nothing of the ends or the length: the driver gave the client all of it. A later length from the
    // server does not override what the driver says.
    rig.eval(&format!("ropeNetSetLength({id}, 9, 0);"));
    for k in 0..40 {
        rig.move_mover(k as f32 * 0.05);
        rig.step();
    }
    rig.run(30);
    let (_, held) = rope(&rig.peers[0]);
    assert!((held.length() - 4.0).abs() < 1.0e-3, "{}", held.length());
    // At its limit the rope lies on the line between the start and the mover's offset point.
    let end = held.points().last().unwrap();
    assert!(near(end, [39.0 * 0.05 + 0.5, 0.0, 1.0], 0.3), "{end:?}");
    assert!(held.at_limit() && held.taut());
    assert!(off_the_line(&held) < 1.0e-3, "{}", off_the_line(&held));
    // Let go of the driver, the rope is left as it was and no longer bound.
    rig.eval(&format!("ropeNetUndrive({id});"));
    rig.run(10);
    assert!(!rope(&rig.peers[0]).1.at_limit());
    assert_eq!(bound(&rig.peers[0]), 0);
    assert_eq!(rig.peers[0].rope_ids().len(), 1);
}

#[test]
fn a_driver_that_has_nothing_to_say_lets_the_rope_go_and_a_missing_one_warns_once() {
    let files = files();
    let gone = Rc::new(Cell::new(false));
    let flag = Rc::clone(&gone);
    let engine = with_driver(&files, "winch", move |_| {
        (!flag.get()).then(|| {
            Drive::new(
                DriveEnd::Point([0.0, 0.0, 3.0]),
                DriveEnd::Point([1.0, 2.0, 3.0]),
            )
        })
    });
    let mut rig = one(&files, engine);
    let id = create(&mut rig, "Cable");
    rig.eval(&format!("ropeNetDrive({id}, \"winch\", Mover);"));
    rig.run(10);
    let (_, held) = rope(&rig.peers[0]);
    assert_eq!(held.anchor(RopeEnd::End), Anchor::Point([1.0, 2.0, 3.0]));
    gone.set(true);
    rig.run(5);
    assert_eq!(bound(&rig.peers[0]), 0);
    assert_eq!(rig.peers[0].rope_ids().len(), 1);
    // A driver nobody registered: the rope is shown as told, and the client says so once however long it waits.
    let other = create(&mut rig, "Cable");
    rig.eval(&format!("ropeNetDrive({other}, \"nobody\", Mover);"));
    rig.run(60);
    let warned = rig.peers[0]
        .client
        .client_log
        .iter()
        .filter(|l| l.contains("\"nobody\""))
        .count();
    assert_eq!(warned, 1, "{:?}", rig.peers[0].client.client_log);
    assert_eq!(rig.peers[0].rope_ids().len(), 2);
}

#[test]
fn a_rope_made_to_follow_a_bad_driver_name_or_object_is_refused() {
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    let id = create(&mut rig, "Cable");
    let rope_obj = u32::try_from(id).unwrap();
    let mover = rig.server.vm().objects().find(b"Mover").expect("the mover");
    let vm = rig.server.vm_mut();
    for bad in ["", "two words", "a-name-that-is-far-too-long-to-send"] {
        assert!(
            torque_plugin_rope::net::server::set_driver(vm, rope_obj, bad, mover).is_err(),
            "{bad:?}"
        );
    }
    assert!(torque_plugin_rope::net::server::set_driver(vm, rope_obj, "ok", 0).is_err());
    assert!(torque_plugin_rope::net::server::set_driver(vm, 0, "ok", mover).is_err());
    assert!(torque_plugin_rope::net::server::set_driver(vm, rope_obj, "ok", mover).is_ok());
}

/// The rope plugin as it was before offsets and drivers: the same id and everything else, an older network version.
struct OldRope;

impl Plugin for OldRope {
    fn net_version(&self) -> Option<u32> {
        Some(NET_VERSION - 1)
    }

    fn build(&self, engine: &mut EngineBuilder) {
        RopePlugin::default().build(engine);
    }
}

impl PluginType for OldRope {
    const ID: PluginId = PluginId::new("torque.rope");
}

#[test]
fn a_client_of_the_older_rope_version_is_refused_at_the_manifest() {
    assert_eq!(NET_VERSION, 2, "the version moved with the wire format");
    let files = files();
    let mut rig = Rig::new(&files, &[]);
    rig.join(&files, engine_with(&files, OldRope), 50_001, &[]);
    rig.run(100);
    let peer = &rig.peers[0];
    assert_eq!(peer.ghosts(), 0);
    let why = peer.client.global("$Why");
    assert!(
        why.contains("torque.rope") && why.contains(&format!("you have {}", NET_VERSION - 1)),
        "{why:?}"
    );
}

/// How many ropes the client has bound to a driver.
fn bound(peer: &rig::Peer) -> usize {
    peer.engine
        .resource::<RopeDrivers>()
        .expect("the registry")
        .bound_count()
}
