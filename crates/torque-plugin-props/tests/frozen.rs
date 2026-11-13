//! Frozen props over the `props-frozen` bulk channel, with a real server and engine clients over the memory transport:
//! thousands of frozen props reach a client (ghosts under the threshold, the channel for the rest), each drawn once, the
//! handover between a ghost and the channel in both directions and on a lossy link, and the client's collision with the props
//! it was sent that way.

use crate::rig;

use std::collections::BTreeSet;

use rig::{Link, PeerProps, Rig, RigProps, clients};
use torque_engine::net_classes::PluginStills;
use torque_plugin_props::client::ClientSource;
use torque_plugin_props::{ClientJoints, ClientProps};

/// The fastest a client may ask the server to send it packets: 32 a second of 450 bytes.
const FAST: [(&str, &str); 2] = [
    ("$pref::Net::PacketRateToClient", "32"),
    ("$pref::Net::PacketSize", "450"),
];

/// Spawns `n` crates on a grid and freezes each where it is, `batch` to a script.
fn spawn_frozen(rig: &mut Rig, n: usize) {
    let batch = 250;
    for first in (0..n).step_by(batch) {
        let last = (first + batch).min(n);
        rig.eval(&format!(
            "for (%i = {first}; %i < {last}; %i++) {{ \
                %p = propSpawn(Crate, (%i % 50) * 2 - 49 SPC mFloor(%i / 50) * 2 - 40 SPC 0.5); \
                propFreeze(%p); \
             }}"
        ));
    }
}

/// The slots the server's props are in.
fn server_slots(rig: &Rig) -> BTreeSet<u16> {
    rig.server_props().iter().map(|p| p.0).collect()
}

/// What a client has of the props: the slots it holds as ghosts, the slots of its table, and the slots it draws as stills.
struct Held {
    ghosts: Vec<u16>,
    table: Vec<u16>,
    stills: Vec<u16>,
}

impl Held {
    fn of(rig: &Rig, peer: usize) -> Held {
        let book = rig.peers[peer].book();
        let b = book.0.borrow();
        let stills = rig.peers[peer]
            .client
            .vm()
            .extension::<PluginStills>()
            .map(|s| s.with(|m| m.keys().map(|&k| k as u16).collect()))
            .unwrap_or_default();
        Held {
            ghosts: b.props().values().map(|p| p.slot).collect(),
            table: b.frozen().iter().map(|(slot, _)| slot).collect(),
            stills,
        }
    }

    /// How many times the prop in `slot` is drawn: by its ghost, or as a still when it has none.
    fn drawn(&self, slot: u16) -> usize {
        self.ghosts.iter().filter(|&&s| s == slot).count()
            + self.stills.iter().filter(|&&s| s == slot).count()
    }
}

/// The client's collision source over what it holds.
fn collision(rig: &Rig, peer: usize) -> ClientSource {
    ClientSource {
        book: rig.peers[peer].book(),
        joints: rig.peers[peer]
            .engine
            .resource::<ClientJoints>()
            .expect("the joints")
            .clone(),
    }
}

fn counts(rig: &Rig) -> Vec<torque_plugin_props::frozen::server::ClientCount> {
    rig.props().frozen_counts()
}

#[test]
fn two_thousand_frozen_props_reach_a_client_as_ghosts_under_the_threshold_and_over_the_channel() {
    let globals = [("$Pref::Props::MaxProps", "4096")];
    let (mut rig, _files) = clients(2, &globals, &FAST);
    spawn_frozen(&mut rig, 2000);
    assert_eq!(rig.eval("return propCount();"), "2000");
    let slots = server_slots(&rig);
    assert_eq!(slots.len(), 2000);
    assert!(
        rig.run_until(3000, |r| (0..2).all(|k| {
            let h = Held::of(r, k);
            h.ghosts.len() == 256 && h.table.len() == 1744
        })),
        "{:?}",
        Held::of(&rig, 0).table.len()
    );
    rig.run(20);
    for k in 0..2 {
        let held = Held::of(&rig, k);
        // Under the threshold as ghosts, the rest over the channel: no prop both ways, none left out.
        assert_eq!(held.ghosts.len(), 256);
        assert_eq!(held.table.len(), 1744);
        let ghosts: BTreeSet<u16> = held.ghosts.iter().copied().collect();
        let table: BTreeSet<u16> = held.table.iter().copied().collect();
        assert!(ghosts.is_disjoint(&table), "no prop is sent both ways");
        assert_eq!(&(&ghosts | &table), &slots);
        // Drawn once each: a ghost's own picture, or a still for a prop that has no ghost.
        assert!(slots.iter().all(|&s| held.drawn(s) == 1));
        assert_eq!(held.stills.len(), 1744);
        // The client's ghost table holds the 256 props and a few other objects, not 2,000.
        assert!(
            rig.peers[k].ghosts() < 400,
            "{} ghosts",
            rig.peers[k].ghosts()
        );
        // Collision for its prediction has all 2,000, one box in each prop's slot.
        use torque_collide::source::CollisionSource;
        let mut shapes = Vec::new();
        collision(&rig, k).shapes(&mut shapes);
        let keys: BTreeSet<u16> = shapes.iter().map(|s| s.key as u16).collect();
        assert_eq!(shapes.len(), 2000);
        assert_eq!(keys, slots);
    }
    // The server dealt each client the same: 256 ghosts and 1,744 on the channel, all of them there.
    for c in counts(&rig) {
        assert_eq!((c.ghosted, c.channel, c.live), (256, 1744, 1744), "{c:?}");
    }
    let stats = rig.props().frozen_stats();
    // Props the clients never had ghosts of were hidden at once: no ghost was made to be taken away.
    assert_eq!(stats.to_channel, 0);
    assert_eq!(stats.sets, 2 * 1744);
    // Nothing was ever refused or dropped.
    assert_eq!(stats.refused, 0);
    for peer in rig.peers.iter().take(2) {
        let book = peer.book();
        let b = book.0.borrow();
        assert_eq!(b.frozen().rejected, 0);
    }
}

/// Steps a rig while one prop changes hands, checking each tick that it is drawn exactly once.
struct Watch {
    slot: u16,
    lossy: bool,
    /// The most ghosts and the most table entries the client held of the prop at once.
    most: (usize, usize),
}

impl Watch {
    /// Steps `ticks` ticks; `want_channel` says where the prop must have come to rest by the end (none: no say).
    fn run(&mut self, rig: &mut Rig, ticks: usize, want_channel: Option<bool>) {
        let mut settled = false;
        for _ in 0..ticks {
            rig.step();
            let held = Held::of(rig, 0);
            let sizes = (held.ghosts.len(), held.table.len(), held.stills.len());
            // Never drawn twice, never not drawn: one picture on every tick.
            assert_eq!(held.drawn(self.slot), 1, "lossy {}: {sizes:?}", self.lossy);
            self.most = (self.most.0.max(sizes.0), self.most.1.max(sizes.1));
            settled |= match want_channel {
                Some(true) => held.ghosts.is_empty() && held.table == [self.slot],
                Some(false) => held.ghosts.len() == 1 && held.table.is_empty(),
                None => false,
            };
        }
        assert!(
            want_channel.is_none() || settled,
            "lossy {}: settled",
            self.lossy
        );
    }
}

#[test]
fn a_prop_changes_between_a_ghost_and_the_channel_without_a_frame_drawn_twice_or_not_at_all() {
    // No frozen prop is ghosted: every freeze is a handover to the channel, every thaw a handover back.
    for lossy in [false, true] {
        let (mut rig, _files) = clients(1, &[("$Pref::Props::FrozenGhosts", "0")], &[]);
        let id: u32 = rig
            .eval("return propSpawn(Crate, \"0 0 0.5\");")
            .parse()
            .expect("a prop");
        assert!(rig.run_until(400, |r| !Held::of(r, 0).ghosts.is_empty()));
        if lossy {
            rig.link = Link::lossy();
        }
        let slot = rig.server_props()[0].0;
        let mut watch = Watch {
            slot,
            lossy,
            most: (0, 0),
        };
        // Frozen: the ghost is kept until the channel has the prop, then goes.
        rig.eval(&format!("propFreeze({id});"));
        watch.run(&mut rig, 120, Some(true));
        // Both were held at once while it changed hands.
        assert_eq!(watch.most, (1, 1), "lossy {lossy}");
        // Thawed: a ghost again, the channel's copy goes after it.
        rig.eval(&format!("propUnfreeze({id});"));
        watch.run(&mut rig, 120, Some(false));
        // Again, a few times over, with the server's own changes in between.
        for _ in 0..3 {
            rig.eval(&format!("propFreeze({id});"));
            watch.run(&mut rig, 60, None);
            rig.eval(&format!("propUnfreeze({id});"));
            watch.run(&mut rig, 60, None);
        }
        rig.eval(&format!("propFreeze({id});"));
        watch.run(&mut rig, 120, Some(true));
        // Deleted: from the channel and from the stills, and the server forgets it.
        rig.eval(&format!("propDelete({id});"));
        assert!(rig.run_until(200, |r| {
            let h = Held::of(r, 0);
            h.table.is_empty() && h.stills.is_empty() && h.ghosts.is_empty()
        }));
        assert_eq!(counts(&rig).iter().map(|c| c.channel).sum::<usize>(), 0);
    }
}

#[test]
fn a_prop_that_is_frozen_and_hung_on_a_parent_is_a_ghost_and_one_set_free_is_sent_again() {
    // The scene hierarchy is the engine protocol's.
    let files = rig::files();
    let mut rig = Rig::with_engine(
        &files,
        &[("$Pref::Props::FrozenGhosts", "0")],
        rig::engine_pinned(&files),
    );
    rig.join(&files, rig::engine_pinned(&files), 50_001, &[]);
    assert!(rig.run_until(400, |r| r.peers[0].ghosts() >= 2));
    let id: u32 = rig
        .eval("return propSpawn(Crate, \"0 12 5.75\"); ")
        .parse()
        .expect("a prop");
    rig.eval(&format!("propFreeze({id});"));
    let slot = rig.server_props()[0].0;
    assert!(rig.run_until(400, |r| Held::of(r, 0).table == [slot]));
    // Hung on a platform it is held by the hierarchy, whose links name ghosts: it comes back as one.
    let platform: u32 = rig
        .eval("return new StaticShape() { dataBlock = FloorData; position = \"0 12 5\"; scale = \"4 4 0.5\"; };")
        .parse()
        .expect("a platform");
    assert_eq!(
        rig.eval(&format!("return {id}.setParent({platform});")),
        "1"
    );
    for _ in 0..200 {
        rig.step();
        assert_eq!(Held::of(&rig, 0).drawn(slot), 1);
    }
    let held = Held::of(&rig, 0);
    assert_eq!((held.ghosts.len(), held.table.len()), (1, 0));
    // Let go, and frozen again, it goes over the channel (it keeps its place).
    assert_eq!(rig.eval(&format!("return {id}.setParent(0);")), "1");
    rig.eval(&format!("propFreeze({id});"));
    for _ in 0..200 {
        rig.step();
        assert_eq!(Held::of(&rig, 0).drawn(slot), 1);
    }
    let held = Held::of(&rig, 0);
    assert_eq!((held.ghosts.len(), held.table.len()), (0, 1));
}

#[test]
fn a_client_that_joins_later_is_dealt_the_props_that_are_there() {
    let globals = [("$Pref::Props::MaxProps", "4096")];
    let (mut rig, files) = clients(1, &globals, &FAST);
    spawn_frozen(&mut rig, 600);
    assert!(rig.run_until(800, |r| Held::of(r, 0).table.len() == 600 - 256));
    rig.join(&files, rig::engine(&files, true), 50_077, &FAST);
    assert!(rig.run_until(1500, |r| {
        let h = Held::of(r, 1);
        h.ghosts.len() == 256 && h.table.len() == 344
    }));
    let slots = server_slots(&rig);
    let held = Held::of(&rig, 1);
    assert!(slots.iter().all(|&s| held.drawn(s) == 1));
    // The same server's counts for both: each is ghosted 256 and sent the rest.
    assert!(
        counts(&rig)
            .iter()
            .all(|c| (c.ghosted, c.channel) == (256, 344))
    );
}

#[test]
fn a_client_that_leaves_takes_its_hidden_props_with_it() {
    let (mut rig, _files) = clients(1, &[("$Pref::Props::FrozenGhosts", "0")], &[]);
    spawn_frozen(&mut rig, 30);
    assert!(rig.run_until(400, |r| Held::of(r, 0).table.len() == 30));
    let scope = torque_engine::scope::ScopeControl::of(rig.server.vm()).expect("a scope control");
    let conn: u32 = rig
        .eval("return ClientGroup.getObject(0);")
        .parse()
        .unwrap();
    let hidden = |scope: &torque_engine::scope::ScopeControl| {
        scope.hidden_count(torque_engine::scope::Audience::Connection(conn))
    };
    assert_eq!(hidden(&scope), 30);
    rig.peers[0]
        .client
        .call("disconnect", &[])
        .expect("disconnect");
    assert!(rig.run_until(300, |r| counts(r).is_empty() && hidden(&scope) == 0));
}

#[test]
fn the_book_of_a_client_with_no_channel_prop_is_as_it_was() {
    // A client's trace words are the ghosts' alone until a prop comes over the channel.
    let (mut rig, _files) = clients(1, &[], &[]);
    rig.eval("propSpawn(Crate, \"0 0 0.5\");");
    rig.run(60);
    let book: ClientProps = rig.peers[0].book();
    let words = book.0.borrow().state_words();
    assert_eq!(words.len() % 16, 0);
    assert!(book.0.borrow().frozen().is_empty());
}

/// Whether the client `peer` is told the only frozen prop it holds is its own.
fn owns_the_table_prop(rig: &Rig, peer: usize) -> Option<bool> {
    let book = rig.peers[peer].book();
    let b = book.0.borrow();
    b.frozen().iter().next().map(|(_, p)| p.prop.mine)
}

#[test]
fn a_frozen_prop_on_the_channel_is_sent_again_when_what_a_client_is_told_of_it_changes() {
    let (mut rig, _files) = clients(2, &[("$Pref::Props::FrozenGhosts", "0")], &[]);
    let (a, b) = (
        rig.eval("return ClientGroup.getObject(0);"),
        rig.eval("return ClientGroup.getObject(1);"),
    );
    rig.eval(&format!("{a}.propOwnerKey = \"alice\";"));
    rig.eval(&format!(
        "%p = propSpawn(Crate, \"0 0 0.5\", {a}); propFreeze(%p);"
    ));
    assert!(rig.run_until(400, |r| (0..2).all(|k| Held::of(r, k).table.len() == 1)));
    // Each client is told whether the prop is its own.
    assert_eq!(owns_the_table_prop(&rig, 0), Some(true));
    assert_eq!(owns_the_table_prop(&rig, 1), Some(false));
    let sent = rig.props().frozen_stats().sets;
    assert_eq!(sent, 2);
    // The owner leaves and the other client comes to hold its key: the prop is its own, and nothing else is sent.
    rig.peers[0].client.call("disconnect", &[]).unwrap();
    rig.run(10);
    rig.eval(&format!("{b}.propOwnerKey = \"alice\";"));
    assert!(rig.run_until(200, |r| owns_the_table_prop(r, 1) == Some(true)));
    assert_eq!(rig.props().frozen_stats().sets, sent + 1);
    let slot = rig.server_props()[0].0;
    assert_eq!(Held::of(&rig, 1).drawn(slot), 1);
}

/// Keeps prop `obj` for a loopback connection alone, as a game's rule can: no other connection's scope takes it.
fn keep_for_loopback(rig: &Rig, obj: u32) {
    let link = torque_game::scene::ext::get(rig.server.vm().objects(), obj)
        .and_then(|e| e.ghost.clone())
        .expect("a ghostable object");
    let hub = link
        .as_any()
        .downcast_ref::<torque_server::net::ghosts::GhostHub<torque_server::net::torque::TgeGhostGame>>()
        .expect("the server's hub")
        .clone();
    hub.with_mut(|h| {
        if let Some(entry) = h.reg.entries.get_mut(&obj) {
            entry.flags |= torque_net::ghost::net_flags::SCOPE_LOCAL;
        }
    });
}

#[test]
fn the_channel_never_sends_a_frozen_prop_the_clients_scope_does_not_take() {
    use torque_engine::scope::{Audience, ScopeControl};
    let (mut rig, _files) = clients(1, &[("$Pref::Props::FrozenGhosts", "0")], &[]);
    let conn: u32 = rig
        .eval("return ClientGroup.getObject(0);")
        .parse()
        .unwrap();
    let ids: Vec<u32> = (0..3)
        .map(|k| {
            rig.eval(&format!(
                "%p = propSpawn(Crate, \"{} 0 0.5\"); propFreeze(%p); return %p;",
                k * 3
            ))
            .parse()
            .unwrap()
        })
        .collect();
    let slot_of = |rig: &Rig, id: u32| {
        rig.props()
            .with(|s| s.world.as_ref().and_then(|w| w.get(id)).map(|p| p.slot))
            .flatten()
            .expect("a prop")
    };
    let slots: Vec<u16> = ids.iter().map(|&id| slot_of(&rig, id)).collect();
    // A rule of the server's keeps the first from this connection; another plugin keeps the second from it.
    keep_for_loopback(&rig, ids[0]);
    let scope = ScopeControl::of(rig.server.vm()).expect("a scope control");
    scope
        .hide_as(
            rig.server.vm(),
            "test.other",
            ids[1],
            Audience::Connection(conn),
        )
        .unwrap();
    assert!(rig.run_until(400, |r| Held::of(r, 0).table == [slots[2]]));
    rig.run(100);
    let held = Held::of(&rig, 0);
    assert_eq!(held.table, [slots[2]], "only the one its scope takes");
    assert!(
        held.ghosts.is_empty(),
        "and nothing was ghosted to it either"
    );
    assert_eq!(rig.props().frozen_counts()[0].channel, 1);
    // The third leaves: the other plugin hides it. The client is told to let go of it, and it stays gone.
    scope
        .hide_as(
            rig.server.vm(),
            "test.other",
            ids[2],
            Audience::Connection(conn),
        )
        .unwrap();
    assert!(rig.run_until(200, |r| Held::of(r, 0).table.is_empty()));
    assert!(Held::of(&rig, 0).stills.is_empty());
    // The other plugin lets the second and the third back: they are sent. The first is still not, by the server's own rule.
    for id in [ids[1], ids[2]] {
        scope.show_as("test.other", id, Audience::Connection(conn));
    }
    assert!(rig.run_until(300, |r| {
        let mut table = Held::of(r, 0).table;
        table.sort_unstable();
        table == [slots[1].min(slots[2]), slots[1].max(slots[2])]
    }));
    assert!(!Held::of(&rig, 0).table.contains(&slots[0]));
}
