//! The work the sync does, counted: how it grows with the props and the clients, at 4,096 props and 64 clients.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use torque_net::chunk::{ChannelLimits, ChunkSender};

use super::*;
use crate::data::PropNumbers;
use crate::world::PropLimits;

/// A port that takes every message, has every message and ghost acknowledged at once, and counts.
#[derive(Default)]
struct Instant_ {
    messages: Cell<u64>,
    bytes: Cell<u64>,
    hidden: RefCell<BTreeSet<(ObjId, u32)>>,
    /// The ghosts that came back: a ghost is only ever acknowledged for a prop that was shown again.
    shown: RefCell<BTreeSet<(ObjId, u32)>>,
    /// The props a connection's scope does not take (by distance, by a rule of the game, or kept by another plugin).
    blocked: RefCell<BTreeSet<(ObjId, u32)>>,
    /// What other plugins hid or showed since the last tick.
    changes: RefCell<Vec<(ObjId, Audience)>>,
    /// Whether the scope can change with no word of it.
    varies: Cell<bool>,
    /// The log of the other plugins forgot changes (told once).
    forgot: Cell<bool>,
    /// The ops each connection was sent, in order.
    ops: RefCell<Vec<(ConnId, Op)>>,
}

impl Port for Instant_ {
    fn backlog(&self, _: ConnId) -> usize {
        0
    }

    fn send(&self, conn: ConnId, bytes: Arc<[u8]>) -> Result<MessageHandle, SendError> {
        self.ops.borrow_mut().extend(
            messages::read(&bytes)
                .unwrap()
                .into_iter()
                .map(|op| (conn, op)),
        );
        self.messages.set(self.messages.get() + 1);
        self.bytes.set(self.bytes.get() + bytes.len() as u64);
        ChunkSender::new(&[ChannelLimits::new(400)]).enqueue_shared_with_handle(
            0,
            Kind::Delta,
            bytes,
        )
    }

    fn delivered(&self, _: ConnId, _: MessageHandle) -> bool {
        true
    }

    fn hide(&self, conn_obj: ObjId, prop: u32) -> bool {
        self.hidden.borrow_mut().insert((conn_obj, prop));
        true
    }

    fn show(&self, conn_obj: ObjId, prop: u32) {
        self.hidden.borrow_mut().remove(&(conn_obj, prop));
        self.shown.borrow_mut().insert((conn_obj, prop));
    }

    fn ghost_acked(&self, conn_obj: ObjId, prop: u32) -> bool {
        self.shown.borrow().contains(&(conn_obj, prop))
    }

    fn visible_many(&self, conn_obj: ObjId, props: &[u32]) -> Vec<bool> {
        let blocked = self.blocked.borrow();
        props
            .iter()
            .map(|&p| !blocked.contains(&(conn_obj, p)))
            .collect()
    }

    fn scope_changes(&self, _: Option<u64>, out: &mut Vec<(ObjId, Audience)>) -> Result<u64, u64> {
        if self.forgot.replace(false) {
            return Err(0);
        }
        out.append(&mut self.changes.borrow_mut());
        Ok(0)
    }

    fn scope_varies(&self, _: ObjId, _: &[u32]) -> bool {
        self.varies.get()
    }
}

const PROPS: u32 = 4096;
const CLIENTS: u32 = 64;
const THRESHOLD: usize = 256;

fn params() -> crate::data::PropParams {
    PropNumbers {
        can_freeze: true,
        ..PropNumbers::default()
    }
    .held()
}

/// A world of 4,096 frozen crates in a grid.
fn frozen_world() -> PropWorld {
    let mut w = PropWorld::new(
        4,
        PropLimits {
            per_server: PROPS,
            ..PropLimits::default()
        },
    );
    for k in 0..PROPS {
        let at = [(k % 64) as f32 * 2.0, (k / 64) as f32 * 2.0, 0.5];
        w.spawn(100 + k, 9, params(), (at, [0.0, 0.0, 0.0, 1.0]), None, 0)
            .unwrap();
        w.freeze(100 + k).unwrap();
    }
    w
}

fn clients() -> Vec<(ConnId, ObjId)> {
    (0..CLIENTS).map(|k| (ConnId(k + 1), 10_000 + k)).collect()
}

/// Runs one tick and gives what it cost: the work done, the messages sent and the time taken.
fn tick(
    sync: &mut FrozenSync,
    world: &mut PropWorld,
    port: &Instant_,
) -> (FrozenWork, u64, Duration) {
    let (before, sent) = (sync.work(), port.messages.get());
    let started = Instant::now();
    sync.run(world, None, &clients(), THRESHOLD, port);
    let took = started.elapsed();
    let after = sync.work();
    (
        FrozenWork {
            props_looked_at: after.props_looked_at - before.props_looked_at,
            client_steps: after.client_steps - before.client_steps,
            ack_checks: after.ack_checks - before.ack_checks,
        },
        port.messages.get() - sent,
        took,
    )
}

/// What a run of ticks cost: the work and the messages added up, the slowest tick, and how many ticks there were.
#[derive(Debug, Default)]
struct Run {
    ticks: u32,
    work: FrozenWork,
    messages: u64,
    slowest: Duration,
}

/// Ticks until `done`, checking that no tick looks at more props than the budget allows or asks about more acknowledgements
/// than its cap.
fn run_until(
    sync: &mut FrozenSync,
    world: &mut PropWorld,
    port: &Instant_,
    mut done: impl FnMut(&FrozenSync) -> bool,
) -> Run {
    let mut run = Run::default();
    loop {
        let (work, messages, took) = tick(sync, world, port);
        run.ticks += 1;
        run.messages += messages;
        run.slowest = run.slowest.max(took);
        run.work.props_looked_at += work.props_looked_at;
        run.work.client_steps += work.client_steps;
        run.work.ack_checks += work.ack_checks;
        assert!(
            work.client_steps <= DEAL_STEPS_PER_TICK as u64,
            "a tick looked at {} props",
            work.client_steps
        );
        assert!(
            work.ack_checks <= ACK_CHECKS_PER_TICK as u64 + u64::from(CLIENTS),
            "{work:?}"
        );
        if done(sync) {
            return run;
        }
        assert!(run.ticks < 2_000, "never done: {:?}", sync.counts()[0]);
    }
}

fn whole(sync: &FrozenSync) -> bool {
    sync.counts()
        .iter()
        .all(|c| (c.ghosted, c.channel, c.live) == (THRESHOLD, 3840, 3840))
}

#[test]
fn a_tick_costs_what_changed_at_4096_props_and_64_clients() {
    let mut world = frozen_world();
    let (mut sync, port) = (FrozenSync::default(), Instant_::default());

    // 64 clients join at once and are each dealt every prop, a few thousand a tick: 256 ghosts, the rest hidden and sent, none
    // of it a ghost made to be taken away.
    let join = run_until(&mut sync, &mut world, &port, whole);
    println!("64 clients join: {join:?}");
    assert_eq!(join.work.props_looked_at, u64::from(PROPS));
    assert_eq!(join.work.client_steps, u64::from(PROPS * CLIENTS));
    assert_eq!(join.messages, u64::from(CLIENTS) * 4);
    assert_eq!(sync.stats().to_channel, 0);
    assert_eq!(port.hidden.borrow().len(), (CLIENTS * 3840) as usize);

    // Steady state: nothing changes, nothing is done.
    let mut slowest = Duration::ZERO;
    for _ in 0..50 {
        let (work, messages, took) = tick(&mut sync, &mut world, &port);
        assert_eq!(
            (work, messages),
            (FrozenWork::default(), 0),
            "a tick of nothing"
        );
        slowest = slowest.max(took);
    }
    println!(
        "steady state: 0 props looked at, 0 client steps, 0 acks, slowest of 50 ticks {slowest:?}"
    );

    // One prop thawed: one prop looked at, one step for each client.
    world.unfreeze(100 + 3000).unwrap();
    let (one, _, took) = tick(&mut sync, &mut world, &port);
    println!("one thaw: {one:?} in {took:?}");
    assert_eq!(one.props_looked_at, 1);
    assert_eq!(one.client_steps, u64::from(CLIENTS));
    assert_eq!(one.ack_checks, 0);
    let (rest, ..) = tick(&mut sync, &mut world, &port);
    assert!(rest.ack_checks <= u64::from(CLIENTS));

    // A mass thaw: every prop at once. What the clients owe is spread over the ticks, within the caps, and they are whole
    // again when it is all in.
    for k in 0..PROPS {
        world.unfreeze(100 + k);
    }
    let thaw = run_until(&mut sync, &mut world, &port, |s| {
        s.counts().iter().all(|c| c.channel == 0)
    });
    println!("mass thaw: {thaw:?}");
    assert_eq!(thaw.work.props_looked_at, u64::from(PROPS - 1));
    assert!(port.hidden.borrow().is_empty(), "every prop is shown again");
    // The ghosts are the clients' again, each acknowledged once.
    assert_eq!(sync.stats().to_ghost, u64::from(CLIENTS) * 3840);
    // The props whose entries just went are looked at once more (they are frozen props of nobody now); then a tick of nothing
    // is a tick of nothing again.
    run_until(&mut sync, &mut world, &port, |s| {
        s.clients.values().all(|c| c.todo.is_empty())
    });
    let (work, messages, _) = tick(&mut sync, &mut world, &port);
    assert_eq!((work, messages), (FrozenWork::default(), 0));
}

#[test]
fn a_prop_put_off_by_the_budget_is_hidden_before_a_ghost_of_it_is_made_unless_one_was() {
    // Two clients join with more frozen props than one tick looks at: a prop they wait for is still hidden at once, and one
    // whose ghost the client acknowledged meanwhile goes the safe way (its ghost is kept until the channel has the prop).
    let mut world = frozen_world();
    let (mut sync, port) = (FrozenSync::default(), Instant_::default());
    let live = vec![(ConnId(1), 10_000), (ConnId(2), 10_001)];
    sync.run(&mut world, None, &live, THRESHOLD, &port);
    let first = sync.work().client_steps;
    assert_eq!(first, DEAL_STEPS_PER_TICK as u64);
    // Client 2 has had no turn: its props wait, and every one of them is owed the hiding of a prop never exposed.
    assert_eq!(sync.clients[&ConnId(2)].seed, Some(0));
    // A ghost of a waiting prop is acknowledged in the meantime.
    let late = 100 + PROPS - 1;
    port.shown.borrow_mut().insert((10_001, late));
    run_until_quiet(&mut sync, &mut world, &port, &live);
    // Its message is acknowledged at the next look.
    sync.run(&mut world, None, &live, THRESHOLD, &port);
    let c2 = &sync.clients[&ConnId(2)];
    assert!(c2.fresh.is_empty(), "no prop is left waiting to be placed");
    let entry = c2.channel[&late];
    assert!(
        matches!(entry.stage, Stage::Live),
        "the safe way ends hidden too: {entry:?}"
    );
    // The others were hidden without a ghost ever being kept for them.
    assert_eq!(sync.stats().to_channel, 1);
}

fn run_until_quiet(
    sync: &mut FrozenSync,
    world: &mut PropWorld,
    port: &Instant_,
    live: &[(ConnId, ObjId)],
) {
    for _ in 0..200 {
        sync.run(world, None, live, THRESHOLD, port);
        if sync
            .clients
            .values()
            .all(|c| c.todo.is_empty() && c.seed.is_none())
        {
            return;
        }
    }
    panic!("the clients never got through their lists");
}

/// A world of `n` frozen crates, a client dealt none of them as ghosts, and a port with nothing blocked.
fn small(n: u32) -> (PropWorld, FrozenSync, Instant_, Vec<(ConnId, ObjId)>) {
    let mut w = PropWorld::new(4, PropLimits::default());
    for k in 0..n {
        let at = [(k % 32) as f32 * 2.0, (k / 32) as f32 * 2.0, 0.5];
        w.spawn(100 + k, 9, params(), (at, [0.0, 0.0, 0.0, 1.0]), None, 0)
            .unwrap();
        w.freeze(100 + k).unwrap();
    }
    (
        w,
        FrozenSync::default(),
        Instant_::default(),
        vec![(ConnId(1), 10_000), (ConnId(2), 10_001)],
    )
}

fn quiet(sync: &mut FrozenSync, w: &mut PropWorld, port: &Instant_, live: &[(ConnId, ObjId)]) {
    for _ in 0..6 {
        sync.run(w, None, live, 0, port);
    }
}

/// The slots of the sets and removes sent to `conn` so far.
fn sent(port: &Instant_, conn: ConnId) -> (BTreeSet<u16>, Vec<u16>) {
    let ops = port.ops.borrow();
    let sets = ops
        .iter()
        .filter(|(c, op)| *c == conn && matches!(op, Op::Set { .. }))
        .map(|(_, op)| op.slot())
        .collect();
    let removes = ops
        .iter()
        .filter(|(c, op)| *c == conn && matches!(op, Op::Remove { .. }))
        .map(|(_, op)| op.slot())
        .collect();
    (sets, removes)
}

fn slot_of(w: &PropWorld, id: u32) -> u16 {
    w.get(id).unwrap().slot
}

#[test]
fn a_prop_out_of_a_clients_scope_is_never_sent_one_that_leaves_is_removed_and_one_that_enters_is_sent()
 {
    let (mut w, mut sync, port, live) = small(40);
    port.varies.set(true);
    // The first client's scope takes the props 100..120 only (a distance, a rule of the game); the second's takes all.
    for k in 20..40 {
        port.blocked.borrow_mut().insert((10_000, 100 + k));
    }
    quiet(&mut sync, &mut w, &port, &live);
    let (first, _) = sent(&port, ConnId(1));
    let (second, _) = sent(&port, ConnId(2));
    assert_eq!(first.len(), 20, "only what its scope takes");
    assert!((20..40).all(|k| !first.contains(&slot_of(&w, 100 + k))));
    assert_eq!(second.len(), 40);
    // Nothing out of its scope is hidden from its ghosts either, and nothing is counted as its.
    assert!((20..40).all(|k| !port.hidden.borrow().contains(&(10_000, 100 + k))));
    assert_eq!(sync.counts()[0].channel, 20);

    // A prop it holds leaves its scope: the sweep finds it, the client is told to let go, and its ghosts are as before.
    let leaving = 100 + 3;
    port.blocked.borrow_mut().insert((10_000, leaving));
    quiet(&mut sync, &mut w, &port, &live);
    let (_, removes) = sent(&port, ConnId(1));
    assert_eq!(removes, [slot_of(&w, leaving)]);
    assert!(!port.hidden.borrow().contains(&(10_000, leaving)));
    assert_eq!(sync.counts()[0].channel, 19);
    assert_eq!(
        sent(&port, ConnId(2)).1,
        Vec::<u16>::new(),
        "the other client keeps it"
    );

    // And one comes back, or enters: sent again, hidden from its ghosts.
    port.blocked.borrow_mut().remove(&(10_000, leaving));
    port.blocked.borrow_mut().remove(&(10_000, 100 + 25));
    quiet(&mut sync, &mut w, &port, &live);
    let (sets, _) = sent(&port, ConnId(1));
    assert!(sets.contains(&slot_of(&w, leaving)) && sets.contains(&slot_of(&w, 125)));
    assert_eq!(sync.counts()[0].channel, 21);
    assert!(port.hidden.borrow().contains(&(10_000, leaving)));
}

#[test]
fn a_prop_another_plugin_keeps_from_a_client_is_never_sent_to_it_and_follows_the_plugins_hides() {
    let (mut w, mut sync, port, live) = small(10);
    // No sweep: only what the log of the other plugins says is heard.
    let hidden_for_first = [100, 101, 102];
    for p in hidden_for_first {
        port.blocked.borrow_mut().insert((10_000, p));
    }
    quiet(&mut sync, &mut w, &port, &live);
    let (first, _) = sent(&port, ConnId(1));
    assert_eq!(first.len(), 7);
    assert!(
        hidden_for_first
            .iter()
            .all(|&p| !first.contains(&slot_of(&w, p)))
    );
    // The other plugin shows one of them to the client: it is sent at the next tick, with no walk of the props.
    port.blocked.borrow_mut().remove(&(10_000, 101));
    port.changes
        .borrow_mut()
        .push((101, Audience::Connection(10_000)));
    let before = sync.work().client_steps;
    sync.run(&mut w, None, &live, 0, &port);
    assert_eq!(sync.work().client_steps - before, 1, "one prop looked at");
    quiet(&mut sync, &mut w, &port, &live);
    assert!(sent(&port, ConnId(1)).0.contains(&slot_of(&w, 101)));
    // It hides one the client holds, from everyone: the client is told to let go of it, the second client too.
    for c in [10_000, 10_001] {
        port.blocked.borrow_mut().insert((c, 105));
    }
    port.changes.borrow_mut().push((105, Audience::Everyone));
    quiet(&mut sync, &mut w, &port, &live);
    let slot = slot_of(&w, 105);
    assert_eq!(sent(&port, ConnId(1)).1, [slot]);
    assert_eq!(sent(&port, ConnId(2)).1, [slot]);
    // A log that forgot changes sends every client over what it holds and keeps from itself again.
    port.blocked.borrow_mut().remove(&(10_000, 102));
    port.forgot.set(true);
    sync.run(&mut w, None, &live, 0, &port);
    quiet(&mut sync, &mut w, &port, &live);
    assert!(sent(&port, ConnId(1)).0.contains(&slot_of(&w, 102)));
}

#[test]
fn a_client_whose_scope_is_not_in_place_is_sent_nothing_and_is_dealt_every_prop_once_it_is() {
    let (mut w, mut sync, port, live) = small(40);
    // Before its scope object is set the connection takes nothing, and its scope is not settled.
    port.varies.set(true);
    for k in 0..40 {
        port.blocked.borrow_mut().insert((10_000, 100 + k));
    }
    quiet(&mut sync, &mut w, &port, &live);
    assert_eq!(
        sent(&port, ConnId(1)).0.len(),
        0,
        "nothing before its scope"
    );
    assert!(port.hidden.borrow().iter().all(|(conn, _)| *conn != 10_000));
    // The scope object is set: no word of any prop changes, but the scope stops varying, and the client is dealt what it takes.
    port.blocked.borrow_mut().clear();
    port.varies.set(false);
    quiet(&mut sync, &mut w, &port, &live);
    assert_eq!(
        sent(&port, ConnId(1)).0.len(),
        40,
        "every prop its scope takes"
    );
}

#[test]
fn a_client_that_fell_behind_the_log_ends_with_exactly_the_live_frozen_props() {
    let (mut w, mut sync, port, live) = small(40);
    // No sweep and no word from the other plugins: only the log, and what the client does when it forgot some.
    quiet(&mut sync, &mut w, &port, &live);
    assert_eq!(sync.counts()[0].channel, 40);
    let toggle = |w: &mut PropWorld, sync: &mut FrozenSync| {
        // (The two props that matter are left alone once they have changed.)
        for thaw in [true, false] {
            for k in (0..40).filter(|&k| k > 2) {
                if thaw {
                    w.unfreeze(100 + k);
                } else {
                    w.freeze(100 + k);
                }
            }
            sync.snap.refresh(w, None, sync.tick);
        }
    };
    // More changes than a tick reads, then the ones that matter (a deletion, a thaw), then more than the log keeps:
    // the clients read none of it, and the log forgets what they have not read.
    for _ in 0..60 {
        toggle(&mut w, &mut sync);
    }
    assert!(w.remove(101));
    w.unfreeze(102);
    for _ in 0..(LOG_CAP / 74 + 150) {
        toggle(&mut w, &mut sync);
    }
    quiet(&mut sync, &mut w, &port, &live);
    quiet(&mut sync, &mut w, &port, &live);
    let live_frozen: BTreeSet<u32> = (100..140).filter(|&id| id != 101 && id != 102).collect();
    for conn in [ConnId(1), ConnId(2)] {
        let held: BTreeSet<u32> = sync.clients[&conn].channel.keys().copied().collect();
        assert_eq!(
            held, live_frozen,
            "{conn:?} holds exactly the live frozen props"
        );
    }
}
