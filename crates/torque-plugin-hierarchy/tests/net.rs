//! The scene hierarchy over the network: a real server and an engine client over the memory transport (with a fifth of the
//! datagrams dropped where it says so), the client drawing each child from its parent's ghost and the local pose the
//! per-ghost block carried.

#[path = "net/rig.rs"]
mod rig;

use std::collections::BTreeMap;

use rig::{Rig, engine_without_hierarchy, error, files, linked, root_pose, three_levels};
use torque_collide::math::Matrix;
use torque_net::ghost::net_flags::SCOPE_LOCAL;
use torque_server::net::ghosts::GhostHub;
use torque_server::net::torque::TgeGhostGame;
use torque_ts_vm::sim::ObjId;

/// How far a client's child may be from the server's, in units: a millimetre.
const MILLIMETRE: f32 = 1e-3;

/// The server's world of each object it was asked about, by the root's translation that tick (its bits): what a client
/// that drew the root at a pose should draw the rest at.
#[derive(Default)]
struct History(BTreeMap<[u32; 3], BTreeMap<ObjId, Matrix>>);

impl History {
    fn take(&mut self, rig: &Rig, root: ObjId, objects: &[ObjId]) {
        let m = rig.server_world(root);
        let key = [m[3], m[7], m[11]].map(f32::to_bits);
        let worlds = objects.iter().map(|&o| (o, rig.server_world(o))).collect();
        self.0.insert(key, worlds);
    }

    /// The server's worlds of the tick whose root the client draws at `drawn`.
    fn at(&self, drawn: &Matrix) -> Option<&BTreeMap<ObjId, Matrix>> {
        self.0
            .get(&[drawn[3], drawn[7], drawn[11]].map(f32::to_bits))
    }
}

#[test]
fn every_childs_client_world_is_within_a_millimetre_of_the_servers_under_loss() {
    let mut rig = Rig::new(0.2);
    let root = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    // A four-level chain and more: a cube on the root's origin, a scaled pistol on that cube, a half-size
    // cube on the pistol's muzzle node (off its origin, scaled by the pistol's scale), a cube on that, and an item on the
    // first cube's node.
    let a = rig.object("return cube(\"2 0 2\", \"1 1 1\");");
    let gun = rig.object("return pistol(\"2 1 2.5\", \"6 6 6\");");
    let b = rig.object("return cube(\"2 2 3\", \"0.5 0.5 0.5\");");
    let c = rig.object("return cube(\"2 3 4\", \"1 1 1\");");
    let d = rig.object("return box(\"-2 0 3\");");
    let script = format!(
        "return {a}.setParent({root}) && {a}.setLocalTransform(\"2 0 0 0 0 1 0.7\") \
         && {gun}.setParent({a}, 1) && {b}.setParent({gun}, 1, \"muzzlePoint\") \
         && {c}.setParent({b}, 0) && {d}.setParent({a}, 1, \"cube\");"
    );
    assert_eq!(
        rig.eval(&script),
        "1",
        "{}",
        rig.eval("return getParentError();")
    );
    let children = [a, gun, b, c, d];
    let parents = [root, a, gun, b, a];
    let pairs: Vec<(ObjId, ObjId)> = children.iter().copied().zip(parents).collect();
    let ghosts = linked(&mut rig, &pairs);
    let root_ghost = rig.ghost(root).expect("the root's ghost");
    let mut history = History::default();
    let all: Vec<ObjId> = children.to_vec();
    history.take(&rig, root, &all);
    let mut worst = (0.0f32, 0.0f32);
    let mut matched = 0;
    let mut roots = std::collections::BTreeSet::new();
    for _ in 0..600 {
        let t = rig.ticks;
        rig.move_to(root, &root_pose(t));
        rig.step();
        history.take(&rig, root, &all);
        let drawn = rig.record_world(root_ghost).expect("the root's record");
        let server = history
            .at(&drawn)
            .expect("the client draws the root where the server had it at some tick");
        matched += 1;
        roots.insert(drawn[3].to_bits());
        for (&child, &ghost) in children.iter().zip(&ghosts) {
            let client = rig.client_world(ghost).expect("the child is drawn");
            let e = error(&client, &server[&child]);
            worst = (worst.0.max(e.0), worst.1.max(e.1));
        }
    }
    assert_eq!(matched, 600);
    assert!(
        roots.len() >= 400,
        "the client drew the root at {} poses",
        roots.len()
    );
    assert!(
        worst.0 <= MILLIMETRE && worst.1 <= 1e-3,
        "the worst child was {} units and {} off",
        worst.0,
        worst.1
    );
    rig.links(|l| {
        assert_eq!(l.stats(), Default::default(), "nothing was refused");
        l.audit().expect("the client's tree keeps its rules");
    });
}

#[test]
fn a_moving_root_with_64_still_children_sends_no_child_updates() {
    let mut rig = Rig::new(0.0);
    let root = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    let mut pairs = Vec::new();
    for i in 0..64u8 {
        let (x, y) = (f32::from(i % 8) - 3.5, f32::from(i / 8) - 3.5);
        let child = rig.object(&format!("return box(\"{x} {y} 4\");"));
        assert_eq!(rig.eval(&format!("return {child}.setParent({root});")), "1");
        pairs.push((child, root));
    }
    let ghosts = linked(&mut rig, &pairs);
    let root_ghost = rig.ghost(root).expect("the root's ghost");
    let count = |rig: &Rig| -> u32 { ghosts.iter().map(|&g| rig.updates(g)).sum() };
    let mut last = count(&rig);
    let root_before = rig.updates(root_ghost);
    let mut quiet = 0;
    for tick in 0..610 {
        let t = rig.ticks;
        rig.move_to(root, &root_pose(t));
        rig.step();
        let now = count(&rig);
        if tick >= 10 && now == last {
            quiet += 1;
        }
        last = now;
    }
    assert!(
        quiet >= 590,
        "child updates on {} of 600 ticks",
        600 - quiet
    );
    assert!(
        rig.updates(root_ghost) - root_before >= 600,
        "the root itself was sent every tick: {} quiet {quiet}",
        rig.updates(root_ghost) - root_before
    );
    // And they are where the server has them.
    for (&(child, _), &ghost) in pairs.iter().zip(&ghosts) {
        let e = error(&rig.client_world(ghost).unwrap(), &rig.server_world(child));
        assert!(e.0 <= MILLIMETRE, "{e:?}");
    }
}

#[test]
fn a_child_ahead_of_its_parent_is_drawn_at_its_record_pose_until_its_link_resolves() {
    let mut rig = Rig::new(0.0);
    let child = rig.object("return cube(\"3 1 2\", \"1 1 1\");");
    let mut ghost = None;
    for _ in 0..200 {
        ghost = rig.ghost(child).filter(|&g| rig.record_world(g).is_some());
        if ghost.is_some() {
            break;
        }
        rig.step();
    }
    let ghost = ghost.expect("the child's ghost");
    // The parent is made now: its first record is not acknowledged when the child's link is first written.
    let parent = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({parent});")),
        "1"
    );
    let mut waiting = 0;
    for _ in 0..200 {
        rig.step();
        if rig.links(|l| l.parent(ghost).is_some()) {
            break;
        }
        waiting += 1;
        let drawn = rig.client_world(ghost).expect("drawn");
        assert_eq!(
            drawn,
            rig.record_world(ghost).unwrap(),
            "at its record pose"
        );
        assert!(error(&drawn, &rig.server_world(child)).0 <= MILLIMETRE);
    }
    assert!(waiting >= 1, "the link resolved at once: nothing was ahead");
    assert!(
        rig.links(|l| l.parent(ghost).is_some()),
        "the link resolved"
    );
    // Linked: it follows the moving parent within a millimetre.
    let mut history = History::default();
    let parent_ghost = rig.ghost(parent).expect("the parent's ghost");
    history.take(&rig, parent, &[child]);
    for _ in 0..120 {
        let t = rig.ticks;
        rig.move_to(parent, &root_pose(t));
        rig.step();
        history.take(&rig, parent, &[child]);
        let at = history
            .at(&rig.record_world(parent_ghost).unwrap())
            .expect("a tick");
        let e = error(&rig.client_world(ghost).unwrap(), &at[&child]);
        assert!(e.0 <= MILLIMETRE, "{e:?}");
    }
}

#[test]
fn children_of_a_deleted_parent_are_drawn_where_the_server_left_them() {
    let mut rig = Rig::new(0.0);
    let root = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    let kid = rig.object("return box(\"2 0 2\");");
    assert_eq!(rig.eval(&format!("return {kid}.setParent({root});")), "1");
    let ghost = linked(&mut rig, &[(kid, root)])[0];
    // The root's motion marks nothing of the child: its record falls behind.
    for _ in 0..30 {
        let t = rig.ticks;
        rig.move_to(root, &root_pose(t));
        rig.step();
    }
    rig.eval(&format!("{root}.delete();"));
    rig.run(10);
    assert!(rig.links(|l| l.parent(ghost).is_none()));
    // An item's record carries its position (its turn is not on Torque's wire): it is drawn there.
    let drawn = rig.client_world(ghost).expect("drawn");
    let e = error(&drawn, &rig.server_world(kid));
    assert!(
        e.0 <= MILLIMETRE,
        "{e:?} drawn {drawn:?} record {:?} server {:?} held {:?}",
        rig.record_world(ghost),
        rig.server_world(kid),
        rig.links(|l| l.held(ghost))
    );
    assert_eq!(drawn, rig.record_world(ghost).unwrap());
}

#[test]
fn the_renderer_draws_each_child_where_the_server_has_it() {
    let mut rig = Rig::new(0.0);
    let (root, pairs) = three_levels(&mut rig);
    let ghosts = linked(&mut rig, &pairs);
    for _ in 0..40 {
        let t = rig.ticks;
        rig.move_to(root, &root_pose(t));
        rig.step();
    }
    let drawn = rig.drawn();
    for (&(child, _), ghost) in pairs.iter().zip(&ghosts) {
        let at = drawn.get(ghost).expect("the child's shape is drawn");
        let server = rig.server_world(child);
        let off = [3, 7, 11]
            .iter()
            .map(|&i| (at[i] - server[i]).abs())
            .fold(0.0, f32::max);
        assert!(off <= MILLIMETRE, "child {child} is drawn {off} units off");
    }
}

#[test]
fn a_client_without_the_hierarchy_is_turned_away_at_the_manifest() {
    let mut rig = Rig::start(0.0, engine_without_hierarchy(&files()));
    rig.run(100);
    assert_eq!(rig.client.ghost_count(), 0);
    let why = rig.client.global("$Why");
    assert!(
        why.contains("torque.hierarchy") && why.contains("you do not have it"),
        "{why:?}"
    );
}

#[test]
fn every_link_change_moves_the_scope_stamp_and_a_local_pose_does_not() {
    let mut rig = Rig::new(0.0);
    let revision = |rig: &Rig| {
        torque_server::net::blocks::of(rig.server.vm())
            .expect("the server writes the block")
            .revision()
    };
    let a = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    let b = rig.object("return cube(\"2 0 2\", \"1 1 1\");");
    let c = rig.object("return cube(\"4 0 2\", \"1 1 1\");");
    let mut last = revision(&rig);
    let mut moved = |rig: &mut Rig, script: &str| {
        rig.eval(script);
        let now = revision(rig);
        let changed = now != last;
        last = now;
        changed
    };
    assert!(moved(&mut rig, &format!("{b}.setParent({a});")), "a link");
    assert!(
        moved(&mut rig, &format!("{c}.setParent({b});")),
        "a grandchild"
    );
    assert!(
        !moved(&mut rig, &format!("{c}.setLocalTransform(\"1 0 0\");")),
        "a local pose"
    );
    assert!(
        moved(&mut rig, &format!("{c}.setParent({a});")),
        "a reparent"
    );
    assert!(moved(&mut rig, &format!("{c}.setParent(0);")), "an unlink");
    assert!(
        moved(&mut rig, &format!("{a}.delete();")),
        "a parent's delete"
    );
}

/// The server's ghost hub, reached through `obj`'s link to it.
fn hub(rig: &Rig, obj: ObjId) -> GhostHub<TgeGhostGame> {
    let link = torque_game::scene::ext::get(rig.server.vm().objects(), obj)
        .and_then(|e| e.ghost.clone())
        .expect("the object is registered for ghosting");
    link.as_any()
        .downcast_ref::<GhostHub<TgeGhostGame>>()
        .expect("the server's hub")
        .clone()
}

/// Sets or clears `ScopeLocal` on `obj`: only a loopback connection scopes it while set, so the rig's client is not sent it.
fn scope_local(rig: &Rig, obj: ObjId, on: bool) {
    hub(rig, obj).with_mut(|h| {
        if let Some(entry) = h.reg.entries.get_mut(&obj) {
            if on {
                entry.flags |= SCOPE_LOCAL;
            } else {
                entry.flags &= !SCOPE_LOCAL;
            }
        }
    });
}

/// The child ends up drawn on its parent: linked within `limit` ticks, then within a millimetre of the server for 60 ticks
/// of the parent moving.
fn ends_up_on_its_parent(rig: &mut Rig, parent: ObjId, child: ObjId, limit: usize) {
    let mut ghosts = None;
    for _ in 0..limit {
        rig.step();
        if let (Some(c), Some(p)) = (rig.ghost(child), rig.ghost(parent))
            && rig.links(|l| l.parent(c).is_some_and(|q| q.object.0 == p))
        {
            ghosts = Some((c, p));
            break;
        }
    }
    let (ghost, parent_ghost) =
        ghosts.expect("the child is linked once its parent reaches the client");
    let mut history = History::default();
    history.take(rig, parent, &[child]);
    for _ in 0..60 {
        let t = rig.ticks;
        rig.move_to(parent, &root_pose(t));
        rig.step();
        history.take(rig, parent, &[child]);
        let at = history
            .at(&rig.record_world(parent_ghost).expect("the parent's record"))
            .expect("a tick");
        let off = error(&rig.client_world(ghost).expect("drawn"), &at[&child]).0;
        assert!(off <= MILLIMETRE, "{off}");
    }
}

#[test]
fn a_child_of_a_parent_a_client_is_never_sent_is_drawn_at_its_record_pose() {
    let mut rig = Rig::new(0.0);
    let parent = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    scope_local(&rig, parent, true);
    let child = rig.object("return box(\"2 0 2\");");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({parent});")),
        "1"
    );
    rig.run(20);
    let ghost = rig.ghost(child).expect("the child's ghost");
    for _ in 0..120 {
        let t = rig.ticks;
        rig.move_to(parent, &root_pose(t));
        rig.step();
        assert_eq!(rig.ghost(parent), None, "the parent is never sent");
        assert!(rig.links(|l| l.parent(ghost).is_none() && !l.is_waiting(ghost)));
        // An item's record carries its position (its turn is not on Torque's wire): it follows the parent there.
        let drawn = rig.client_world(ghost).expect("drawn");
        assert_eq!(drawn, rig.record_world(ghost).unwrap());
        let off = error(&drawn, &rig.server_world(child)).0;
        assert!(off <= MILLIMETRE, "{off}");
    }
}

#[test]
fn a_child_sent_without_its_parent_is_linked_once_the_parent_is_scoped() {
    let mut rig = Rig::new(0.0);
    let parent = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    scope_local(&rig, parent, true);
    let child = rig.object("return box(\"2 0 2\");");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({parent});")),
        "1"
    );
    rig.run(20);
    assert!(rig.ghost(child).is_some() && rig.ghost(parent).is_none());
    scope_local(&rig, parent, false);
    ends_up_on_its_parent(&mut rig, parent, child, 200);
}

#[test]
fn a_child_sent_without_its_parent_is_linked_once_a_ghost_slot_frees() {
    let mut rig = Rig::start(0.0, rig::engine(&files()));
    // A table of eight ghosts.
    rig.server.shared.borrow().engine.borrow_mut().ghost_size = 8;
    assert!(rig.run_until(600, |r| r.client.ghost_count() >= 1));
    let mut fillers = Vec::new();
    while rig.client.ghost_count() < 7 {
        fillers.push(rig.object("return cube(\"0 9 1\", \"0.2 0.2 0.2\");"));
        rig.run(10);
    }
    let child = rig.object("return box(\"2 0 2\");");
    rig.run(20);
    let parent = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({parent});")),
        "1"
    );
    rig.run(20);
    assert_eq!(rig.client.ghost_count(), 8, "the table is full");
    assert!(rig.ghost(child).is_some() && rig.ghost(parent).is_none());
    let filler = fillers.pop().expect("a filler");
    rig.eval(&format!("{filler}.delete();"));
    ends_up_on_its_parent(&mut rig, parent, child, 400);
}

#[test]
fn a_childs_first_record_ahead_of_its_parents_is_linked_once_the_parent_is_scoped() {
    let mut rig = Rig::new(0.0);
    let child = rig.object("return box(\"2 0 2\");");
    let parent = rig.object("return cube(\"0 0 2\", \"1 1 1\");");
    assert_eq!(
        rig.eval(&format!("return {child}.setParent({parent});")),
        "1"
    );
    // A pass's scan credit too small to scope the child's relatives with it: the parent waits for later passes.
    let hub = hub(&rig, child);
    hub.set_work_per_tick(2);
    let mut ahead = false;
    for _ in 0..400 {
        rig.step();
        if rig.ghost(child).is_some() && rig.ghost(parent).is_none() {
            ahead = true;
        }
        if rig.ghost(parent).is_some() {
            break;
        }
    }
    assert!(ahead, "the child's ghost came first");
    hub.set_work_per_tick(torque_server::net::ghosts::NET_GHOST_WORK_PER_TICK);
    ends_up_on_its_parent(&mut rig, parent, child, 200);
}
