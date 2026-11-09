//! Tests of the ghost manager: the standard cases, the reference client decoding this crate's packets, a differential check of the
//! sparse table against the dense array, and random operation sequences with loss, duplicate notifies and resets.

use std::collections::HashSet;

use crate::NetError;
use crate::bits::{BitReader, BitWriter};
use crate::events::{
    ConnectionMessage, EventCtx, EventIds, EventQueue, EventReceiver, EventUnpacker, Guarantee,
    SectionParams, WireEvent,
};

use super::client::{ClassForm, ClientEvent, GhostClient, GhostEventReader, SectionReport};
use super::manager::{
    GhostConfig, GhostError, GhostEvent, GhostManager, KILL_PRIORITY, MessageOutcome,
    SHARE_FIRST_PRIORITY, SHARE_PRIORITY_MAX, SectionBudget, share_cap, share_priority,
};
use super::policy::{
    GhostPolicy, ObjectPolicy, PendingOrder, ScopeRadius, TableCounts, TorqueGhosts,
};
use super::source::{GhostScripts, ObjId, ObjectSource, flags, net_flags};
use super::table::Table;
use super::world::{Obj, ToyCodec, World};

/// Budgets that pay for everything.
macro_rules! free {
    () => {
        SectionBudget {
            scan: &mut |_| true,
            records: &mut |_| true,
        }
    };
}

#[derive(Default)]
struct Rec {
    ready: usize,
}

impl GhostScripts for Rec {
    fn on_ghost_always_objects_received(&mut self) {
        self.ready += 1;
    }
}

struct Sent {
    bytes: Vec<u8>,
    seq: u32,
    stats: super::manager::SectionStats,
}

struct Rig<P: GhostPolicy = TorqueGhosts> {
    mgr: GhostManager<P>,
    world: World,
    client: GhostClient<ToyCodec>,
    scripts: Rec,
    seq: u32,
    now: u32,
}

impl Rig {
    fn with(max: u32) -> Rig {
        Rig::with_policy(max, TorqueGhosts)
    }

    fn new() -> Rig {
        Rig::with(crate::profile::DEFAULT_GHOST_LIMIT)
    }
}

/// A test policy with every timing and order hook on: a full query every 3,000 ms (the world's recent list walked between), no
/// detach, the array order past 50 pending ghosts (the scope object's ghost first after a full query), and a radius of 40 to 600
/// (600 without a Sky).
#[derive(Debug, Clone, Copy, Default)]
struct Spaced {
    last_query: u32,
}

const SPACED_RADIUS: ScopeRadius = ScopeRadius {
    no_sky: 600.0,
    min: 40.0,
    max: 600.0,
};

impl GhostPolicy for Spaced {
    fn radius(&self) -> ScopeRadius {
        SPACED_RADIUS
    }

    fn full_query_due(&mut self, now: u32, resuming: bool) -> bool {
        if resuming || now.wrapping_sub(self.last_query) >= 3_000 {
            self.last_query = now;
            true
        } else {
            false
        }
    }

    fn detach_out_of_scope(&self) -> bool {
        false
    }

    fn pending_order(&self, pending: usize, full_query: bool) -> PendingOrder {
        if pending <= 50 {
            PendingOrder::Priority
        } else {
            PendingOrder::Array {
                scope_first: full_query,
            }
        }
    }
}

impl ObjectPolicy<World> for Spaced {
    fn between_queries<'a>(&mut self, src: &'a World) -> Option<&'a [ObjId]> {
        Some(&src.recent)
    }
}

/// Torque's ghosting but for a Ready that runs the script once a sequence.
#[derive(Default)]
struct ReadyOnce;

impl GhostPolicy for ReadyOnce {
    fn ready_once(&self) -> bool {
        true
    }
}

impl ObjectPolicy<World> for ReadyOnce {}

#[test]
fn a_repeated_ready_runs_the_script_once_when_the_policy_says_so() {
    let mut rig = Rig::with_policy(crate::profile::DEFAULT_GHOST_LIMIT, ReadyOnce);
    rig.add(1);
    rig.go();
    let seq = rig.mgr.sequence();
    let ready = ConnectionMessage {
        seq,
        message: 1,
        count: 0,
    };
    assert_eq!(
        rig.mgr.on_connection_message(&ready, &mut rig.scripts),
        Ok(MessageOutcome::Ignored)
    );
    assert_eq!(rig.scripts.ready, 1);
    // A new sequence takes its own Ready.
    let _ = rig.mgr.reset();
    let _ = rig.mgr.activate(&rig.world).unwrap();
    let next = ConnectionMessage {
        seq: rig.mgr.sequence(),
        message: 1,
        count: 0,
    };
    assert_eq!(
        rig.mgr.on_connection_message(&next, &mut rig.scripts),
        Ok(MessageOutcome::Ready)
    );
    assert_eq!(rig.scripts.ready, 2);
}

impl Rig<Spaced> {
    fn spaced() -> Rig<Spaced> {
        Rig::with_policy(crate::profile::DEFAULT_GHOST_LIMIT, Spaced::default())
    }
}

impl<P: ObjectPolicy<World>> Rig<P> {
    fn with_policy(max: u32, policy: P) -> Rig<P> {
        let cfg = GhostConfig {
            max_ghosts: max,
            local: false,
            class_bits: 6,
        };
        let mut client = GhostClient::new(ToyCodec, 6, cfg.ghost_bits());
        client.set_implied_class_flag(policy.implied_class_flag());
        Rig {
            mgr: GhostManager::with_policy(cfg, policy).unwrap(),
            world: World::default(),
            client,
            scripts: Rec::default(),
            seq: 0,
            now: 1,
        }
    }

    fn add(&mut self, id: ObjId) {
        self.world.add(id, Obj::new(1));
    }

    /// Activates, delivers the always-events to the client and sends Ready.
    fn go(&mut self) -> Vec<GhostEvent> {
        let events = self.mgr.activate(&self.world).unwrap();
        self.deliver_events(&events);
        let seq = self.mgr.sequence();
        let out = self
            .mgr
            .on_connection_message(
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
                &mut self.scripts,
            )
            .unwrap();
        assert_eq!(out, MessageOutcome::Ready);
        events
    }

    fn deliver_events(&mut self, events: &[GhostEvent]) {
        let table = crate::testids::table();
        let ids = crate::testids::ids();
        let ctx = EventCtx {
            table: &table,
            ids: &ids,
            ghost_bits: self.mgr.config().ghost_bits(),
            datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
        };
        for e in events {
            let mut w = BitWriter::with_limit(12_000);
            {
                let mut pctx = self.mgr.pack_ctx(&mut self.world);
                e.pack_with(&mut w, &ctx, &mut pctx);
            }
            if let GhostEvent::AlwaysObject { .. } = e {
                let mut r = BitReader::new(w.bytes());
                self.client.read_always_event(&mut r).unwrap();
                assert!(r.bits_left() < 8);
            }
        }
    }

    fn send_with(&mut self, packet_bits: usize) -> Sent {
        self.seq += 1;
        let mut w = BitWriter::with_limit(12_000);
        let stats = self.mgr.write_section(
            &mut w,
            packet_bits,
            self.seq,
            self.now,
            &mut self.world,
            free!(),
        );
        Sent {
            bytes: w.bytes().to_vec(),
            seq: self.seq,
            stats,
        }
    }

    fn send(&mut self) -> Sent {
        self.send_with(8 * 1000)
    }

    /// The client reads the packet; the section must be consumed to fewer than 8 bits.
    fn read(&mut self, s: &Sent) -> SectionReport {
        let mut r = BitReader::new(&s.bytes);
        let rep = self.client.read_section(&mut r).unwrap();
        assert!(r.bits_left() < 8, "{} bits left", r.bits_left());
        rep
    }

    /// Delivered: the client reads it and the server hears the ack.
    fn ack(&mut self, s: &Sent) -> SectionReport {
        let rep = self.read(s);
        self.mgr.on_notify(s.seq, true);
        rep
    }

    fn lose(&mut self, s: &Sent) {
        self.mgr.on_notify(s.seq, false);
    }

    fn scope_many(&mut self, first: ObjId, n: u32) {
        for k in 0..n {
            self.add(first + k);
            self.mgr.object_in_scope_now(&self.world, first + k);
        }
    }

    fn id(&self, obj: ObjId) -> u32 {
        self.mgr.view_of(obj).unwrap().index
    }
}

fn sent_bits(rig: &mut Rig) -> usize {
    let mut w = BitWriter::with_limit(12_000);
    rig.seq += 1;
    rig.mgr
        .write_section(&mut w, 8000, rig.seq, rig.now, &mut rig.world, free!());
    w.bits()
}

// ---- ids

#[test]
fn ids_are_struct_numbers_from_zero_in_scope_order() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(100, 3);
    assert_eq!([100, 101, 102].map(|o| rig.id(o)), [0, 1, 2]);
    // Scoping an object that already has a ghost changes nothing.
    rig.mgr.object_in_scope_now(&rig.world, 101);
    assert_eq!(rig.mgr.ghost_count(), 3);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn the_most_recently_freed_id_is_reused_first() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(100, 4);
    let s = rig.send();
    rig.ack(&s);
    for o in [100, 101, 102, 103] {
        rig.mgr.detach_object(o);
        rig.world.remove(o);
    }
    let kills = rig.send();
    let first_written = rig.read(&kills).first_index.unwrap();
    rig.mgr.on_notify(kills.seq, true);
    assert_eq!(rig.mgr.ghost_count(), 0);
    // The ack frees the packet's records newest first, so the FIRST record's id is freed last and comes back first.
    rig.scope_many(200, 1);
    assert_eq!(rig.id(200), first_written);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn the_client_log_names_every_change_in_order_once_enabled() {
    use super::client::GhostChange;
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(100, 2);
    let s = rig.send();
    rig.ack(&s);
    // Off by default: nothing was kept.
    assert!(rig.client.take_log().is_empty());
    rig.client.enable_log();
    rig.mgr.set_mask(100, 1);
    let s = rig.send();
    rig.ack(&s);
    let updated = rig.id(100);
    assert_eq!(rig.client.take_log(), [GhostChange::Updated(updated)]);
    let gone = rig.id(101);
    rig.mgr.detach_object(101);
    rig.world.remove(101);
    rig.scope_many(102, 1);
    let s = rig.send();
    rig.ack(&s);
    let log = rig.client.take_log();
    assert!(log.contains(&GhostChange::Deleted(gone)), "{log:?}");
    assert!(log.contains(&GhostChange::Created(rig.id(102))), "{log:?}");
    assert_eq!(log.len(), 2);
    rig.client.clear();
    assert_eq!(rig.client.take_log(), [GhostChange::Cleared]);
    assert!(rig.client.take_log().is_empty());
}

#[test]
fn freed_ids_come_back_in_reverse_order_of_freeing() {
    let mut t = Table::default();
    t.init(16, 0);
    let ids: Vec<u32> = (0..5).map(|o| t.alloc(o, 0x0a).unwrap()).collect();
    assert_eq!(ids, [0, 1, 2, 3, 4]);
    t.free(1);
    t.free(3);
    t.free(0);
    let again: Vec<u32> = (10..13).map(|o| t.alloc(o, 0x0a).unwrap()).collect();
    assert_eq!(again, [0, 3, 1]);
    assert_eq!(t.alloc(20, 0x0a), Some(5));
    t.check_invariants().unwrap();
}

#[test]
fn ghost_always_objects_take_the_top_ids_and_events_go_out_descending() {
    let mut rig = Rig::new();
    for o in 10..13 {
        rig.add(o);
        rig.world.always.push(o);
    }
    let events = rig.go();
    let max = crate::profile::DEFAULT_GHOST_LIMIT;
    assert_eq!(
        events.first(),
        Some(&GhostEvent::Message(ConnectionMessage {
            seq: 1,
            message: 3,
            count: 3
        }))
    );
    assert_eq!(
        events.last(),
        Some(&GhostEvent::Message(ConnectionMessage {
            seq: 1,
            message: 0,
            count: 0
        }))
    );
    let idx: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            GhostEvent::AlwaysObject { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(idx, [max - 1, max - 2, max - 3]);
    assert_eq!([10, 11, 12].map(|o| rig.id(o)), [max - 3, max - 2, max - 1]);
    // The client got them from the events; ordinary ids start at 0.
    assert_eq!(rig.client.len(), 3);
    rig.scope_many(50, 1);
    assert_eq!(rig.id(50), 0);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn an_unghostable_ghost_always_member_leaves_its_id_unused() {
    let mut rig = Rig::new();
    for o in 10..13 {
        rig.add(o);
        rig.world.always.push(o);
    }
    rig.world.objs.get_mut(&11).unwrap().nflags = 0;
    let events = rig.go();
    let max = crate::profile::DEFAULT_GHOST_LIMIT;
    match &events[0] {
        GhostEvent::Message(m) => assert_eq!(m.count, 3, "the count is the set size"),
        e => panic!("{e:?}"),
    }
    let idx: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            GhostEvent::AlwaysObject { index, .. } => Some(*index),
            _ => None,
        })
        .collect();
    assert_eq!(idx, [max - 2, max - 3]);
    // The unused struct Max-1 is the first free struct after the scoped ones: the next ordinary ghost takes it.
    rig.scope_many(50, 1);
    assert_eq!(rig.id(50), max - 1);
}

#[test]
fn dependents_scope_depth_first_in_list_order() {
    let mut rig = Rig::new();
    rig.go();
    for o in [1, 2, 3, 4] {
        rig.add(o);
    }
    rig.world.objs.get_mut(&1).unwrap().deps = vec![2, 3];
    rig.world.objs.get_mut(&2).unwrap().deps = vec![4];
    rig.mgr.object_in_scope_now(&rig.world, 1);
    assert_eq!([1, 2, 4, 3].map(|o| rig.id(o)), [0, 1, 2, 3]);
}

#[test]
fn a_full_table_refuses_silently() {
    let mut rig = Rig::with(4);
    rig.go();
    rig.scope_many(1, 6);
    assert_eq!(rig.mgr.ghost_count(), 4);
    assert!(rig.mgr.view_of(5).is_none());
    rig.mgr.check_invariants().unwrap();
}

// ---- activate and reset

#[test]
fn a_second_activate_is_refused_until_a_reset() {
    let mut rig = Rig::new();
    rig.go();
    assert_eq!(rig.mgr.activate(&rig.world), Err(GhostError::AlreadyActive));
    let ev = rig.mgr.reset();
    assert_eq!(
        ev,
        GhostEvent::Message(ConnectionMessage {
            seq: 1,
            message: 2,
            count: 0
        })
    );
    assert!(!rig.mgr.is_ghosting());
    assert_eq!(rig.mgr.sequence(), 2);
    assert!(rig.mgr.activate(&rig.world).is_ok());
    assert_eq!(rig.mgr.sequence(), 3);
}

#[test]
fn a_reset_drops_everything_and_ids_restart() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 5);
    let s = rig.send();
    rig.mgr.reset();
    assert_eq!(rig.mgr.ghost_count(), 0);
    assert_eq!(rig.mgr.refs_in_flight(), 0);
    // The notify of a packet from before the reset finds nothing.
    rig.mgr.on_notify(s.seq, true);
    rig.mgr.on_notify(s.seq, false);
    rig.client.clear();
    rig.go();
    rig.scope_many(10, 2);
    assert_eq!([10, 11].map(|o| rig.id(o)), [0, 1]);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn a_stale_always_event_packs_its_empty_form() {
    let mut rig = Rig::new();
    rig.add(10);
    rig.world.always.push(10);
    let events = rig.mgr.activate(&rig.world).unwrap();
    rig.mgr.reset();
    rig.mgr.activate(&rig.world).unwrap();
    let table = crate::testids::table();
    let ids = crate::testids::ids();
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 18,
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    let stale = events
        .iter()
        .find(|e| matches!(e, GhostEvent::AlwaysObject { .. }))
        .unwrap();
    let mut w = BitWriter::with_limit(12_000);
    {
        let mut p = rig.mgr.pack_ctx(&mut rig.world);
        stale.pack_with(&mut w, &ctx, &mut p);
    }
    assert_eq!(w.bits(), 18 + 1);
    let mut r = BitReader::new(w.bytes());
    assert_eq!(rig.client.read_always_event(&mut r).unwrap(), None);
    // a client registers a placeholder object for the empty form.
    assert_eq!(rig.client.len(), 1);
    assert_eq!(rig.client.ghost(rig.mgr.config().ghost_bits().into()), None);
    // The same event for the current sequence packs the object.
    let events = rig.mgr.activate(&rig.world);
    assert_eq!(events, Err(GhostError::AlreadyActive));
}

#[test]
fn an_always_event_reflects_the_object_when_it_is_written() {
    let mut rig = Rig::new();
    rig.add(10);
    rig.world.objs.get_mut(&10).unwrap().fields[0] = 7;
    rig.world.always.push(10);
    let events = rig.mgr.activate(&rig.world).unwrap();
    // The object changes between activate and the write: the event carries the new state.
    rig.world.objs.get_mut(&10).unwrap().fields[0] = 9;
    rig.deliver_events(&events);
    let idx = rig.id(10);
    assert_eq!(rig.client.ghost(idx).unwrap().fields[0], Some(9));
}

#[test]
fn an_always_pack_keeps_the_bits_it_could_not_send() {
    let mut rig = Rig::new();
    rig.add(10);
    rig.add(11);
    rig.world.objs.get_mut(&10).unwrap().unsent = 0x10;
    rig.world.always.extend([10, 11]);
    let events = rig.mgr.activate(&rig.world).unwrap();
    let table = crate::testids::table();
    let ids = crate::testids::ids();
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 12,
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    let mut left = Vec::new();
    for e in &events {
        let mut w = BitWriter::with_limit(12_000);
        let mut p = rig.mgr.pack_ctx(&mut rig.world);
        e.pack_with(&mut w, &ctx, &mut p);
        left.extend(p.into_left());
    }
    assert_eq!(left, vec![(10, 0x10)]);
    for (obj, bits) in left {
        rig.mgr.set_mask(obj, bits);
    }
    assert_eq!(rig.mgr.view_of(10).unwrap().mask, 0x10);
    assert_eq!(rig.mgr.view_of(11).unwrap().mask, 0);
}

#[test]
fn always_events_go_through_the_event_queue_with_the_pack_context() {
    let mut rig = Rig::new();
    for o in 10..13 {
        rig.add(o);
        rig.world.always.push(o);
    }
    let events = rig.mgr.activate(&rig.world).unwrap();
    let table = crate::testids::table();
    let ids = crate::testids::ids();
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 12,
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    let mut q: EventQueue<GhostEvent> = EventQueue::new();
    for e in events {
        q.post(e).unwrap();
    }
    let mut w = BitWriter::with_limit(12_000);
    let written = {
        let mut p = rig.mgr.pack_ctx(&mut rig.world);
        q.write_section_with(
            &mut w,
            &ctx,
            &mut p,
            1,
            SectionParams {
                ghosting: false,
                hold_ordered: false,
                packet_bits: 8000,
                min_start_bits: 100,
            },
        )
        .unwrap()
    };
    assert_eq!(written.ordered, 5);
    let mut recv: EventReceiver<ClientEvent> = EventReceiver::new();
    let mut out = Vec::new();
    let mut r = BitReader::new(w.bytes());
    recv.read_section(
        &mut r,
        &ctx,
        crate::protocol::Role::Client,
        &mut GhostEventReader {
            client: &mut rig.client,
        },
        &mut out,
        None,
    )
    .unwrap();
    assert!(r.bits_left() < 8);
    assert_eq!(out.len(), 5);
    assert_eq!(rig.client.len(), 3);
}

// ---- messages

#[test]
fn connection_messages_follow_the_message_rules() {
    let mut rig = Rig::new();
    rig.add(10);
    rig.world.always.push(10);
    let events = rig.mgr.activate(&rig.world).unwrap();
    let id = rig.id(10);
    assert_eq!(
        rig.mgr.view(id).unwrap().flags & flags::SCOPED_EVENT,
        flags::SCOPED_EVENT
    );
    let m = |message, seq| ConnectionMessage {
        seq,
        message,
        count: 0,
    };
    for bad in [0u8, 2, 3] {
        assert_eq!(
            rig.mgr.on_connection_message(&m(bad, 1), &mut rig.scripts),
            Err(GhostError::BadMessageState)
        );
    }
    // A stale Ready is ignored: no script, no error, still not ghosting.
    assert_eq!(
        rig.mgr.on_connection_message(&m(1, 99), &mut rig.scripts),
        Ok(MessageOutcome::Ignored)
    );
    assert_eq!(rig.scripts.ready, 0);
    assert!(!rig.mgr.is_ghosting());
    assert_eq!(
        rig.mgr.on_connection_message(&m(4, 1), &mut rig.scripts),
        Ok(MessageOutcome::NotGhost)
    );
    assert_eq!(
        rig.mgr.on_connection_message(&m(1, 1), &mut rig.scripts),
        Ok(MessageOutcome::Ready)
    );
    assert_eq!(rig.scripts.ready, 1);
    assert!(rig.mgr.is_ghosting());
    assert_eq!(rig.mgr.view(id).unwrap().flags & 0x90, 0);
    // A repeated Ready runs the script again, as it should.
    rig.mgr
        .on_connection_message(&m(1, 1), &mut rig.scripts)
        .unwrap();
    assert_eq!(rig.scripts.ready, 2);
    drop(events);
}

// ---- the ghost section

#[test]
fn a_connection_that_is_not_ghosting_writes_one_zero_flag() {
    let mut rig = Rig::new();
    rig.add(1);
    let mut w = BitWriter::with_limit(12_000);
    let st = rig
        .mgr
        .write_section(&mut w, 8000, 1, 1, &mut rig.world, free!());
    assert!(st.idle);
    assert_eq!(w.bits(), 1);
    assert_eq!(w.bytes(), &[0]);
}

#[test]
fn an_empty_ghosting_section_is_flag_size_and_terminator() {
    let mut rig = Rig::new();
    rig.go();
    let s = rig.send();
    // flag(1), int(5) = 0, flag(0)
    assert_eq!(s.bytes, [0b0000_0001]);
    assert_eq!(s.stats.records, 0);
    let rep = rig.read(&s);
    assert!(rep.ghosting);
}

#[test]
fn the_id_width_follows_the_largest_pending_index_and_never_drops_below_three() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    let s = rig.send();
    assert_eq!((s.bytes[0] >> 1) & 31, 0, "width 3");
    // 9 ghosts: ids 0..8, bitlen(8) = 4.
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 9);
    let s = rig.send();
    assert_eq!((s.bytes[0] >> 1) & 31, 1, "width 4");
    // A ghost-always id at the top forces width 12 (TGE's 4,096 ghosts) while it is pending.
    let mut rig = Rig::new();
    rig.add(10);
    rig.world.always.push(10);
    rig.go();
    rig.world.objs.get_mut(&10).unwrap().fields[1] = 4;
    rig.mgr.set_mask(10, 2);
    let s = rig.send();
    assert_eq!((s.bytes[0] >> 1) & 31, 9, "width 12");
    assert_eq!(rig.read(&s).updated, 1);
}

#[test]
fn a_ghost_freed_before_its_first_send_still_counts_for_the_id_width() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 9);
    rig.mgr.detach_object(9);
    let s = rig.send();
    assert_eq!(
        s.stats.kills, 0,
        "no wire record for a ghost that never went out"
    );
    assert_eq!(rig.mgr.ghost_count(), 8);
    assert_eq!(
        (s.bytes[0] >> 1) & 31,
        1,
        "id 8 was freed in this packet but sized it"
    );
    assert_eq!(rig.read(&s).created, 8);
}

/// A test policy: objects marked `implied` in the toy world are of the implied class.
#[derive(Debug, Clone, Copy, Default)]
struct Implying;

impl GhostPolicy for Implying {
    fn implied_class_flag(&self) -> bool {
        true
    }
}

impl ObjectPolicy<World> for Implying {
    fn implied_class(&self, src: &World, obj: ObjId) -> bool {
        src.objs.get(&obj).is_some_and(|o| o.implied)
    }
}

#[test]
fn torques_new_ghost_record_names_its_class_without_a_flag() {
    let mut rig = Rig::new();
    rig.go();
    rig.add(1);
    rig.world.objs.get_mut(&1).unwrap().class = 9;
    rig.world.objs.get_mut(&1).unwrap().implied = true;
    rig.mgr.object_in_scope_now(&rig.world, 1);
    let s = rig.send();
    // flag(ghosting), the 5-bit id size, flag(record), the id, flag(not a kill), then the class id at once.
    let mut r = BitReader::new(&s.bytes);
    assert_eq!(r.read_flag(), Ok(true));
    let id_bits = r.read_bits(5).unwrap() as u8 + 3;
    assert_eq!(r.read_flag(), Ok(true));
    assert_eq!(r.read_bits(id_bits), Ok(0));
    assert_eq!(r.read_flag(), Ok(false));
    assert_eq!(r.read_bits(6), Ok(9));
    rig.ack(&s);
    assert_eq!(rig.client.ghost(0).unwrap().class, Some(ClassForm::Id(9)));
}

#[test]
fn a_policys_implied_class_is_one_flag_and_any_other_class_a_flag_and_its_id() {
    let mut rig = Rig::with_policy(crate::profile::DEFAULT_GHOST_LIMIT, Implying);
    rig.go();
    rig.add(1);
    rig.add(2);
    rig.world.objs.get_mut(&2).unwrap().implied = true;
    rig.world.objs.get_mut(&1).unwrap().class = 9;
    rig.mgr.object_in_scope_now(&rig.world, 1);
    rig.mgr.object_in_scope_now(&rig.world, 2);
    let s = rig.send();
    rig.ack(&s);
    assert_eq!(rig.client.ghost(0).unwrap().class, Some(ClassForm::Id(9)));
    assert_eq!(rig.client.ghost(1).unwrap().class, Some(ClassForm::Implied));
    // An update carries no class form.
    rig.world.objs.get_mut(&1).unwrap().fields[2] = 5;
    rig.mgr.set_mask(1, 4);
    let s = rig.send();
    assert_eq!(s.stats.records, 1);
    let rep = rig.ack(&s);
    assert_eq!((rep.created, rep.updated), (0, 1));
    assert_eq!(rig.client.ghost(0).unwrap().fields[2], Some(5));
}

#[test]
fn no_second_record_while_the_new_ghost_record_is_in_flight() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    let a = rig.send();
    assert_eq!(a.stats.records, 1);
    rig.mgr.set_mask(1, 1);
    let b = rig.send();
    assert_eq!(b.stats.records, 0, "Ghosting blocks it");
    rig.ack(&a);
    let c = rig.send();
    assert_eq!(c.stats.records, 1);
}

#[test]
fn records_start_while_bitnum_is_not_above_the_packet_size_and_may_overshoot() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    rig.world.objs.get_mut(&1).unwrap().bulk = 100;
    let one = sent_bits(&mut rig);
    rig.mgr.on_notify(rig.seq, false);
    let record = one - 7;
    rig.scope_many(2, 2);
    for o in [2, 3] {
        rig.world.objs.get_mut(&o).unwrap().bulk = 100;
    }
    // Exactly at the boundary after one record: bits == packet_bits is NOT above it, so the next record starts.
    let s = rig.send_with(6 + record);
    assert_eq!(s.stats.records, 2, "second starts at bits == packet size");
    // One bit under: stops.
    let mut rig2 = Rig::new();
    rig2.go();
    rig2.scope_many(1, 3);
    for o in [1, 2, 3] {
        rig2.world.objs.get_mut(&o).unwrap().bulk = 100;
    }
    let s = rig2.send_with(6 + record - 1);
    assert_eq!(s.stats.records, 1);
    // Mid-record start: the second record starts below the size and passes it.
    let mut rig3 = Rig::new();
    rig3.go();
    rig3.scope_many(1, 3);
    for o in [1, 2, 3] {
        rig3.world.objs.get_mut(&o).unwrap().bulk = 100;
    }
    let s = rig3.send_with(6 + record + 10);
    assert_eq!(s.stats.records, 2);
    assert!(
        s.bytes.len() * 8 > 6 + record + 10,
        "the last record overshoots"
    );
    rig3.read(&s);
}

#[test]
fn a_record_that_cannot_fit_the_stream_is_rolled_back_and_skipped() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 3);
    rig.world.objs.get_mut(&2).unwrap().bulk = 1900;
    let s = rig.send_with(8 * 1400);
    assert_eq!(s.stats.refused, 1);
    assert_eq!(s.stats.records, 2);
    let rep = rig.read(&s);
    assert_eq!(rep.created, 2);
    // The refused ghost keeps its mask and is retried.
    assert_eq!(rig.mgr.view_of(2).unwrap().mask, 0xFFFF_FFFF);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn a_share_group_takes_at_most_its_part_of_a_packet_and_the_rest_wait() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 12);
    // Ten objects of one group, each a little over 100 bits, at a fifth of a 4,000-bit packet; two with no share.
    for o in 1..=12 {
        let obj = rig.world.objs.get_mut(&o).unwrap();
        obj.bulk = 12;
        obj.share = (o <= 10).then_some((3, 0.2));
    }
    let mut sent = 0;
    for round in 0..8 {
        let s = rig.send_with(4_000);
        assert!(s.stats.share_bits <= 800, "round {round}: {:?}", s.stats);
        if round == 0 {
            assert!(s.stats.share_deferred > 0, "{:?}", s.stats);
        }
        sent += s.stats.records;
        rig.ack(&s);
        rig.mgr.check_invariants().unwrap();
    }
    // Every object got its first record in the end.
    assert_eq!(sent, 12);
    assert_eq!(share_cap(4_000, 0.2), 800);
    assert_eq!(share_cap(4_000, f32::NAN), 0);
    assert_eq!(share_cap(4_000, 7.0), 4_000);
}

#[test]
fn the_work_budget_ends_a_section_cleanly() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 50);
    let mut w = BitWriter::with_limit(12_000);
    let mut left = 5u64;
    let st = rig.mgr.write_section(
        &mut w,
        8000,
        1,
        1,
        &mut rig.world,
        SectionBudget {
            scan: &mut |_| true,
            records: &mut |n| {
                if n > left {
                    left = 0;
                    false
                } else {
                    left -= n;
                    true
                }
            },
        },
    );
    assert!(st.out_of_budget);
    assert!(st.records < 50);
    let mut r = BitReader::new(w.bytes());
    rig.client.read_section(&mut r).unwrap();
    assert!(r.bits_left() < 8);
}

// ---- notifies

#[test]
fn a_lost_update_rearms_only_bits_no_newer_record_carried() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    let s = rig.send();
    rig.ack(&s);
    rig.mgr.set_mask(1, 0b0111);
    let a = rig.send();
    assert_eq!(rig.mgr.view_of(1).unwrap().mask, 0);
    rig.mgr.set_mask(1, 0b1100);
    let b = rig.send();
    assert_eq!(a.stats.records + b.stats.records, 2);
    // A (older) is lost while B is still in flight: bits 2 was carried again by B.
    rig.lose(&a);
    assert_eq!(rig.mgr.view_of(1).unwrap().mask, 0b0011);
    // B lost too: its own bits come back, ORed in.
    rig.lose(&b);
    assert_eq!(rig.mgr.view_of(1).unwrap().mask, 0b1111);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn a_lost_update_of_a_clean_ghost_moves_it_back_to_the_pending_region() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 2);
    let s = rig.send();
    rig.ack(&s);
    assert_eq!(rig.mgr.pending(), 0);
    rig.mgr.set_mask(2, 0b10);
    let a = rig.send();
    assert_eq!(rig.mgr.pending(), 0);
    rig.lose(&a);
    assert_eq!(rig.mgr.pending(), 1);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn a_lost_new_ghost_record_is_sent_again_with_the_class_form() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    let a = rig.send();
    assert_eq!(
        rig.mgr.view_of(1).unwrap().flags & flags::GHOSTING,
        flags::GHOSTING
    );
    rig.lose(&a);
    let v = rig.mgr.view_of(1).unwrap();
    assert_eq!(
        v.flags & (flags::GHOSTING | flags::NOT_YET_GHOSTED),
        flags::NOT_YET_GHOSTED
    );
    assert_eq!(v.mask, 0xFFFF_FFFF);
    let b = rig.send();
    let rep = rig.ack(&b);
    assert_eq!(rep.created, 1, "the client never saw the first");
    assert!(rig.mgr.ghost_index(1).is_some());
}

#[test]
fn getghostindex_is_none_until_the_new_ghost_record_is_acked() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    assert_eq!(rig.mgr.ghost_index(1), None);
    let a = rig.send();
    assert_eq!(rig.mgr.ghost_index(1), None);
    rig.ack(&a);
    assert_eq!(rig.mgr.ghost_index(1), Some(0));
    assert_eq!(rig.mgr.object_at_index(0), Some(1));
}

#[test]
fn a_kill_record_frees_the_ghost_on_ack_and_is_resent_on_loss() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 2);
    let s = rig.send();
    rig.ack(&s);
    rig.mgr.detach_object(1);
    rig.world.remove(1);
    let k = rig.send();
    assert_eq!(k.stats.kills, 1);
    assert_eq!(
        rig.mgr.view(0).unwrap().flags & flags::KILLING_GHOST,
        flags::KILLING_GHOST
    );
    rig.lose(&k);
    assert_eq!(
        rig.mgr.view(0).unwrap().flags & flags::KILL_GHOST,
        flags::KILL_GHOST
    );
    let k2 = rig.send();
    assert_eq!(k2.stats.kills, 1);
    let rep = rig.ack(&k2);
    assert_eq!(rep.deleted, 1);
    assert!(rig.mgr.view(0).is_none());
    assert_eq!(rig.client.len(), 1);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn notifies_for_unknown_or_repeated_packets_do_nothing() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 3);
    let s = rig.send();
    rig.mgr.on_notify(999, true);
    rig.mgr.on_notify(s.seq, true);
    let v = rig.mgr.view(0).unwrap().flags;
    rig.mgr.on_notify(s.seq, false);
    rig.mgr.on_notify(s.seq, true);
    assert_eq!(rig.mgr.view(0).unwrap().flags, v);
    assert_eq!(rig.mgr.view(0).unwrap().mask, 0);
    assert_eq!(rig.mgr.refs_in_flight(), 0);
}

#[test]
fn a_late_notify_never_touches_the_next_owner_of_a_freed_id() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    let a = rig.send();
    rig.ack(&a);
    rig.mgr.set_mask(1, 2);
    let u = rig.send();
    // The object dies and its kill is acked while the update's notify is still outstanding (out of order delivery).
    rig.mgr.detach_object(1);
    rig.world.remove(1);
    rig.mgr.on_notify(u.seq, true);
    let k = rig.send();
    rig.ack(&k);
    rig.scope_many(7, 1);
    assert_eq!(rig.id(7), 0, "id 0 reused");
    let before = rig.mgr.view(0).unwrap();
    rig.mgr.on_notify(u.seq, false);
    assert_eq!(rig.mgr.view(0).unwrap(), before);
}

// ---- scope

#[test]
fn torques_full_query_runs_with_every_packet_around_the_scope_object() {
    let mut rig = Rig::new();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    for (id, x) in [(2, 700.0), (3, 900.0), (5, 1_100.0)] {
        rig.world.add(id, {
            let mut o = Obj::new(1);
            o.pos = [x, 0.0, 0.0];
            o
        });
    }
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    for now in [1, 2, 33] {
        rig.now = now;
        assert!(rig.send().stats.full_query, "{now}");
    }
    // The scope object first, then the container order, within the Sky's 1,000.
    assert_eq!(rig.mgr.ghost_count(), 3);
    assert_eq!([1, 2, 3].map(|o| rig.id(o)), [0, 1, 2]);
    assert!(rig.mgr.view_of(5).is_none());
}

#[test]
fn a_spaced_full_query_runs_at_the_schedules_interval() {
    let mut rig = Rig::spaced();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    rig.world.add(2, {
        let mut o = Obj::new(1);
        o.pos = [500.0, 0.0, 0.0];
        o
    });
    rig.world.add(3, {
        let mut o = Obj::new(1);
        o.pos = [700.0, 0.0, 0.0];
        o
    });
    rig.world.add(4, {
        let mut o = Obj::new(1);
        o.pos = [10.0, 0.0, 0.0];
        o.nflags = net_flags::GHOSTABLE | 0x40;
        o
    });
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.now = 2_999;
    let s = rig.send();
    assert!(!s.stats.full_query);
    assert_eq!(rig.mgr.ghost_count(), 0);
    rig.now = 3_000;
    let s = rig.send();
    assert!(s.stats.full_query);
    // The Sky's 1,000 held to the schedule's 600: object 3 (700 away) is out; the ScopeAlways object is not a query result (the
    // host's callback test drops it).
    assert_eq!(rig.mgr.ghost_count(), 2);
    assert_eq!([1, 2].map(|o| rig.id(o)), [0, 1]);
    rig.now = 5_999;
    assert!(!rig.send().stats.full_query);
    rig.now = 6_000;
    assert!(rig.send().stats.full_query);
}

#[test]
fn without_a_sky_the_full_query_runs_with_the_schedules_radius() {
    // Torque's: 1,000 without a Sky.
    let mut rig = Rig::new();
    rig.add(1);
    rig.add(2);
    rig.add(3);
    rig.add(4);
    rig.world.objs.get_mut(&3).unwrap().pos = [999.0, 0.0, 0.0];
    rig.world.objs.get_mut(&4).unwrap().pos = [1_001.0, 0.0, 0.0];
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    assert!(rig.send().stats.full_query);
    assert_eq!(rig.mgr.ghost_count(), 3, "the scope object, 2 and 3");
    assert!(rig.mgr.view_of(4).is_none());
    // The spaced schedule's 600.
    let mut rig = Rig::spaced();
    rig.add(1);
    rig.add(3);
    rig.add(4);
    rig.world.objs.get_mut(&3).unwrap().pos = [599.0, 0.0, 0.0];
    rig.world.objs.get_mut(&4).unwrap().pos = [601.0, 0.0, 0.0];
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.now = 6_000;
    assert!(rig.send().stats.full_query);
    assert_eq!(rig.mgr.ghost_count(), 2);
    assert!(rig.mgr.view_of(4).is_none());
    // The radius of a Sky: as it is in Torque's, within 40..=600 in the spaced one.
    assert_eq!(ScopeRadius::default().of(Some(5_000.0)), 5_000.0);
    assert_eq!(ScopeRadius::default().of(Some(1.0)), 1.0);
    assert_eq!(SPACED_RADIUS.of(Some(5_000.0)), 600.0);
    assert_eq!(SPACED_RADIUS.of(Some(1.0)), 40.0);
    assert_eq!(SPACED_RADIUS.of(Some(f32::NAN)), 600.0);
}

#[test]
fn the_policys_list_is_walked_only_in_a_packet_without_the_full_query() {
    let mut rig = Rig::spaced();
    rig.world.sky = Some(100.0);
    rig.add(1);
    rig.world.add(2, {
        let mut o = Obj::new(1);
        o.pos = [60.0, 0.0, 0.0];
        o
    });
    rig.world.add(3, {
        let mut o = Obj::new(1);
        o.pos = [120.0, 0.0, 0.0];
        o
    });
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.world.recent = vec![2, 3];
    rig.now = 10;
    assert!(!rig.send().stats.full_query);
    // Within the radius (100) only: object 2.
    assert_eq!(rig.mgr.ghost_count(), 1);
    assert!(rig.mgr.view_of(2).is_some());
    // Torque's policy names no list: the full query runs instead, and an object past its radius stays out.
    let mut rig = Rig::new();
    rig.add(1);
    rig.world.add(2, {
        let mut o = Obj::new(1);
        o.pos = [5_000.0, 0.0, 0.0];
        o
    });
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.world.recent = vec![2];
    assert!(rig.send().stats.full_query);
    assert!(rig.mgr.view_of(2).is_none());
}

#[test]
fn scope_to_client_keeps_the_ghost_in_scope_through_a_query() {
    let mut rig = Rig::new();
    rig.go();
    rig.add(1);
    rig.mgr.scope_to_client(&rig.world, 1);
    let id = rig.id(1);
    assert_eq!(
        rig.mgr.view(id).unwrap().flags & flags::SCOPE_LOCAL_ALWAYS,
        flags::SCOPE_LOCAL_ALWAYS
    );
    rig.mgr.clear_scope_to_client(1);
    assert_eq!(
        rig.mgr.view(id).unwrap().flags & flags::SCOPE_LOCAL_ALWAYS,
        0
    );
}

#[test]
fn skip_count_rises_with_each_full_query_and_saturates() {
    let mut rig = Rig::new();
    rig.world.sky = Some(100.0);
    rig.add(1);
    rig.add(2);
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.mgr.object_in_scope_now(&rig.world, 2);
    // Ghost 2 stays pending (its mask is non-zero) because the packets are too small to send it.
    for k in 1..=3 {
        rig.now = 5_000 * k;
        rig.send_with(0);
    }
    assert_eq!(rig.mgr.view_of(2).unwrap().skip, 3);
}

// ---- priority and the send order

fn first_index_with(n: u32, prio: impl Fn(u32) -> f32, now: u32, scope: Option<ObjId>) -> u32 {
    first_index_in(Rig::new(), n, prio, now, scope)
}

fn first_index_in<P: ObjectPolicy<World>>(
    mut rig: Rig<P>,
    n: u32,
    prio: impl Fn(u32) -> f32,
    now: u32,
    scope: Option<ObjId>,
) -> u32 {
    rig.go();
    rig.scope_many(1, n);
    for k in 0..n {
        rig.world.objs.get_mut(&(1 + k)).unwrap().prio = prio(k);
    }
    rig.mgr.set_scope_object(scope);
    rig.now = now;
    let s = rig.send_with(6);
    assert_eq!(s.stats.records, 1);
    let mut r = BitReader::new(&s.bytes);
    let rep = rig.client.read_section(&mut r).unwrap();
    rep.first_index.unwrap()
}

#[test]
fn every_pending_ghost_is_sorted_by_priority_highest_first() {
    // Priority falls with the id: after the sort, the highest priority (id 0) is last in the array and goes first.
    assert_eq!(first_index_with(300, |k| 1000.0 - k as f32, 1, None), 0);
    assert_eq!(first_index_with(3, |k| k as f32, 1, None), 2);
    // Within a policy's limit too.
    let f = |k: u32| 1000.0 - k as f32;
    assert_eq!(first_index_in(Rig::spaced(), 50, f, 1, None), 0);
}

#[test]
fn past_a_policys_limit_the_array_order_is_kept() {
    // Unsorted: the last allocated ghost (id 50) is at the end of the array and goes first, whatever its priority.
    let f = |k: u32| 1000.0 - k as f32;
    assert_eq!(first_index_in(Rig::spaced(), 51, f, 1, None), 50);
}

#[test]
fn past_the_sort_limit_the_scope_objects_ghost_goes_first_only_in_a_full_query_packet() {
    // Object 6 has ghost id 5.
    assert_eq!(first_index_in(Rig::spaced(), 51, |_| 0.0, 1, Some(6)), 50);
    assert_eq!(
        first_index_in(Rig::spaced(), 51, |_| 0.0, 3_000, Some(6)),
        5
    );
}

#[test]
fn kill_records_have_the_highest_priority() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 4);
    let a = rig.send();
    rig.ack(&a);
    for o in 1..=4 {
        rig.world.objs.get_mut(&o).unwrap().fields[0] = 1;
        rig.world.objs.get_mut(&o).unwrap().prio = 100.0 + o as f32;
        rig.mgr.set_mask(o, 1);
    }
    rig.mgr.detach_object(1);
    rig.world.remove(1);
    let s = rig.send();
    let mut r = BitReader::new(&s.bytes);
    let rep = rig.client.read_section(&mut r).unwrap();
    assert_eq!(
        rep.first_index,
        Some(0),
        "the kill (10000) before the updates"
    );
}

#[test]
fn a_pending_kill_goes_before_a_share_update_of_any_priority() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 4);
    let a = rig.send();
    rig.ack(&a);
    for o in 1..=4 {
        let obj = rig.world.objs.get_mut(&o).unwrap();
        obj.fields[0] = 1;
        obj.prio = 1.0e6;
        obj.share = Some((0, 1.0));
        rig.mgr.set_mask(o, 1);
    }
    rig.mgr.detach_object(1);
    rig.world.remove(1);
    let s = rig.send();
    let mut r = BitReader::new(&s.bytes);
    let rep = rig.client.read_section(&mut r).unwrap();
    assert_eq!(rep.first_index, Some(0), "the kill before the 1e6 updates");
    assert_eq!(share_priority(1.0e6), SHARE_PRIORITY_MAX);
    const {
        assert!(
            SHARE_PRIORITY_MAX < SHARE_FIRST_PRIORITY && SHARE_FIRST_PRIORITY < KILL_PRIORITY / 2.0
        )
    };
    assert_eq!(share_priority(f32::NAN), 0.0);
}

#[test]
fn a_share_is_of_the_room_the_section_starts_with() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 12);
    for o in 1..=12 {
        let obj = rig.world.objs.get_mut(&o).unwrap();
        obj.bulk = 12;
        obj.share = (o <= 10).then_some((3, 0.5));
    }
    // A writer already 70% full of a 4,000-bit packet: 1,200 bits are left, so the group may take 600.
    rig.seq += 1;
    let mut w = BitWriter::with_limit(12_000);
    for _ in 0..2_800 {
        w.write_flag(false);
    }
    let stats = rig
        .mgr
        .write_section(&mut w, 4_000, rig.seq, rig.now, &mut rig.world, free!());
    assert!(stats.share_bits > 0 && stats.share_bits <= 600, "{stats:?}");
    assert!(stats.share_deferred > 0, "{stats:?}");
    // The two with no share still went, after the group's room was spent.
    let sent: Vec<ObjId> = (1..=12)
        .filter(|&o| rig.mgr.view_of(o).is_some_and(|v| v.mask != 0xFFFF_FFFF))
        .collect();
    assert!(
        sent.contains(&11) && sent.contains(&12),
        "{sent:?} {stats:?}"
    );
}

#[test]
fn a_share_groups_first_records_go_before_its_updates() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 4);
    for o in 1..=4 {
        let obj = rig.world.objs.get_mut(&o).unwrap();
        obj.share = Some((1, 1.0));
        obj.prio = 1.0e6;
    }
    let a = rig.send();
    rig.ack(&a);
    // Four ghosted objects with updates of the highest priority, and a fifth never sent with none: it goes first.
    for o in 1..=4 {
        rig.world.objs.get_mut(&o).unwrap().fields[0] = 1;
        rig.mgr.set_mask(o, 1);
    }
    rig.scope_many(5, 1);
    let five = rig.world.objs.get_mut(&5).unwrap();
    five.share = Some((1, 1.0));
    five.prio = 0.0;
    let s = rig.send();
    let mut r = BitReader::new(&s.bytes);
    let rep = rig.client.read_section(&mut r).unwrap();
    assert_eq!(rep.first_index, rig.mgr.view_of(5).map(|v| v.index));
}

#[test]
fn a_share_group_is_closed_after_its_first_rewind() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 10);
    for o in 1..=10 {
        let obj = rig.world.objs.get_mut(&o).unwrap();
        obj.bulk = 12;
        obj.share = Some((5, 0.05));
    }
    let before = rig.world.packs;
    let s = rig.send_with(4_000);
    // 200 bits of room: a record or two fit, then one is packed and rolled back, and the rest are not packed at all.
    let packed = rig.world.packs - before;
    assert_eq!(packed, s.stats.records + 1, "{:?}", s.stats);
    assert_eq!(
        s.stats.records + s.stats.share_deferred,
        10,
        "{:?}",
        s.stats
    );
}

// ---- table, differential against the dense array

struct Dense {
    array: Vec<u32>,
    pos: Vec<usize>,
    zero: usize,
    free: usize,
}

impl Dense {
    fn new(max: usize, always: usize) -> Dense {
        let array: Vec<u32> = (0..max)
            .map(|j| {
                if j < always {
                    (max - always + j) as u32
                } else {
                    (j - always) as u32
                }
            })
            .collect();
        let mut pos = vec![0; max];
        for (p, &s) in array.iter().enumerate() {
            pos[s as usize] = p;
        }
        Dense {
            array,
            pos,
            zero: 0,
            free: 0,
        }
    }
    fn swap(&mut self, a: usize, b: usize) {
        self.array.swap(a, b);
        self.pos[self.array[a] as usize] = a;
        self.pos[self.array[b] as usize] = b;
    }
    fn push_nonzero(&mut self, s: u32) {
        let z = self.zero;
        self.swap(self.pos[s as usize], z);
        self.zero += 1;
    }
    fn push_to_zero(&mut self, s: u32) {
        self.zero -= 1;
        let z = self.zero;
        self.swap(self.pos[s as usize], z);
    }
    fn alloc(&mut self) -> Option<u32> {
        if self.free == self.array.len() {
            return None;
        }
        let s = self.array[self.free];
        self.free += 1;
        self.push_nonzero(s);
        Some(s)
    }
    fn free(&mut self, s: u32) {
        if self.pos[s as usize] < self.zero {
            self.push_to_zero(s);
        }
        self.free -= 1;
        let f = self.free;
        self.swap(self.pos[s as usize], f);
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[test]
fn the_sparse_table_matches_the_dense_array_swap_for_swap() {
    for seed in 1..=40u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let max = 40usize;
        let always = rng.below(6) as usize;
        let mut dense = Dense::new(max, always);
        let mut t = Table::default();
        t.init(max as u32, always as u32);
        let mut live: Vec<u32> = Vec::new();
        for step in 0..600 {
            match rng.below(5) {
                0 | 1 => {
                    let d = dense.alloc();
                    let s = t.alloc(1000 + step, rng.below(0x30) as u16 & 0x28 | 0x0a);
                    assert_eq!(d, s, "seed {seed} step {step}: alloc");
                    live.extend(s);
                }
                2 if !live.is_empty() => {
                    let k = rng.below(live.len() as u64) as usize;
                    let s = live.swap_remove(k);
                    dense.free(s);
                    t.free(s);
                }
                3 if !live.is_empty() => {
                    let s = live[rng.below(live.len() as u64) as usize];
                    if dense.pos[s as usize] < dense.zero {
                        dense.push_to_zero(s);
                        t.get_mut(s).unwrap().mask = 0;
                        t.push_to_zero(s);
                    }
                }
                _ if !live.is_empty() => {
                    let s = live[rng.below(live.len() as u64) as usize];
                    if dense.pos[s as usize] >= dense.zero {
                        dense.push_nonzero(s);
                        t.get_mut(s).unwrap().mask = 1;
                        t.push_nonzero(s);
                    } else {
                        let f = t.get(s).unwrap().flags ^ 0x28;
                        t.set_flags(s, f);
                    }
                }
                _ => {}
            }
            t.check_invariants().unwrap();
            let used: Vec<u32> = t.ids_in_use().collect();
            assert_eq!(used, dense.array[..dense.free], "seed {seed} step {step}");
            assert_eq!(t.zero(), dense.zero);
            let naive_max = dense.array[..dense.zero].iter().max().copied();
            assert_eq!(t.max_nonzero_id(), naive_max, "incremental max index");
            let mut naive_dead: Vec<u32> = dense.array[..dense.zero]
                .iter()
                .copied()
                .filter(|&s| t.get(s).unwrap().flags & 0x28 == 0x28)
                .collect();
            naive_dead.reverse();
            assert_eq!(t.dead_by_position_desc(), naive_dead);
        }
    }
}

// ---- random operation sequences

struct Fuzz {
    rig: Rig<Implying>,
    inflight: Vec<Sent>,
    notified: Vec<u32>,
    next_obj: ObjId,
    rng: Rng,
    stats: [usize; 5],
}

impl Fuzz {
    fn new(seed: u64, max: u32) -> Fuzz {
        Fuzz {
            rig: Rig::with_policy(max, Implying),
            inflight: Vec::new(),
            notified: Vec::new(),
            next_obj: 1,
            rng: Rng(seed.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1),
            stats: [0; 5],
        }
    }

    fn live_objs(&self) -> Vec<ObjId> {
        self.rig.world.objs.keys().copied().collect()
    }

    fn pick(&mut self) -> Option<ObjId> {
        let v = self.live_objs();
        if v.is_empty() {
            None
        } else {
            Some(v[self.rng.below(v.len() as u64) as usize])
        }
    }

    /// Delivers the oldest in-flight packet (read by the client, acked) or loses it. The notify reaches the server in packet order.
    fn settle_oldest(&mut self, deliver: bool) {
        if self.inflight.is_empty() {
            return;
        }
        let s = self.inflight.remove(0);
        if deliver {
            self.rig.read(&s);
        }
        self.rig.mgr.on_notify(s.seq, deliver);
        self.notified.push(s.seq);
    }

    fn step(&mut self) {
        match self.rng.below(100) {
            0..=14 => {
                let id = self.next_obj;
                self.next_obj += 1;
                let mut o = Obj::new(1 + self.rng.below(30) as u32);
                o.implied = self.rng.chance(30);
                o.pos = [
                    self.rng.below(1500) as f32,
                    self.rng.below(1500) as f32,
                    0.0,
                ];
                o.prio = self.rng.below(50) as f32;
                if self.rng.chance(40) {
                    o.bulk = self.rng.below(60) as usize;
                }
                if self.rng.chance(10) {
                    o.unsent = 1 << self.rng.below(8);
                }
                if let Some(d) = self.pick()
                    && self.rng.chance(20)
                {
                    o.deps.push(d);
                }
                self.rig.world.add(id, o);
                if self.rng.chance(30) {
                    self.rig.world.recent.push(id);
                }
            }
            15..=29 => {
                if let Some(id) = self.pick() {
                    self.rig.mgr.object_in_scope_now(&self.rig.world, id);
                }
            }
            30..=39 => {
                if let Some(id) = self.pick() {
                    self.rig.mgr.detach_object(id);
                    self.rig.world.remove(id);
                }
            }
            40..=54 => {
                if let Some(id) = self.pick() {
                    let b = self.rng.below(9) as u32;
                    let o = self.rig.world.objs.get_mut(&id).unwrap();
                    if b < 8 {
                        o.fields[b as usize] = self.rng.below(256) as u8;
                    } else {
                        o.bulk = self.rng.below(60) as usize;
                    }
                    self.rig.mgr.set_mask(id, 1 << b);
                    if self.rng.chance(10) {
                        o.unsent = 0;
                    }
                    if self.rng.chance(15) {
                        let m = self.rig.mgr.view_of(id).map_or(0, |v| v.mask);
                        let extra = if self.rng.chance(50) {
                            1 << self.rng.below(9)
                        } else {
                            0
                        };
                        self.rig.mgr.clear_mask(id, m | extra);
                    }
                }
            }
            55..=74 => {
                if self.inflight.len() < 30 {
                    let bits = [0usize, 200, 800, 3000, 8000][self.rng.below(5) as usize];
                    self.rig.now += self.rng.below(2500) as u32;
                    let s = self.rig.send_with(bits);
                    self.stats[0] += s.stats.records;
                    self.stats[1] += s.stats.kills;
                    self.stats[2] += s.stats.refused;
                    self.stats[3] += usize::from(s.stats.full_query);
                    self.inflight.push(s);
                }
            }
            75..=84 => {
                let d = self.rng.chance(75);
                self.settle_oldest(d);
            }
            85..=89 => {
                // A repeated notify of a packet already settled.
                if !self.notified.is_empty() {
                    let k = self.notified[self.rng.below(self.notified.len() as u64) as usize];
                    let d = self.rng.chance(50);
                    self.rig.mgr.on_notify(k, d);
                }
            }
            90..=91 => {
                if let Some(id) = self.pick() {
                    self.rig.mgr.scope_to_client(&self.rig.world, id);
                }
            }
            92..=93 => {
                self.rig
                    .mgr
                    .set_scope_object(self.live_objs().first().copied());
            }
            94 => {
                // A reset with packets in flight: the client reads what arrives (in order), then clears; the server's notifies for them come late.
                let n = self.inflight.len();
                for _ in 0..n {
                    let d = self.rng.chance(60);
                    let s = self.inflight.remove(0);
                    if d {
                        self.rig.read(&s);
                    }
                    self.notified.push(s.seq);
                }
                let stale_events = self.rig.mgr.activate(&self.rig.world).err();
                let _ = stale_events;
                self.stats[4] += 1;
                let ev = self.rig.mgr.reset();
                assert!(matches!(ev, GhostEvent::Message(m) if m.message == 2));
                self.rig.client.clear();
                let events = self.rig.mgr.activate(&self.rig.world).unwrap();
                self.rig.deliver_events(&events);
                let seq = self.rig.mgr.sequence();
                self.rig
                    .mgr
                    .on_connection_message(
                        &ConnectionMessage {
                            seq,
                            message: 1,
                            count: 0,
                        },
                        &mut self.rig.scripts,
                    )
                    .unwrap();
                // Late notifies of the packets from before the reset.
                let seqs = self.notified.clone();
                for k in seqs.into_iter().rev().take(10) {
                    let d = self.rng.chance(50);
                    self.rig.mgr.on_notify(k, d);
                }
            }
            _ => {}
        }
        self.rig.mgr.check_invariants().unwrap();
    }

    /// Everything settles, then packets flow until nothing is pending: the client must equal the server's view of the world.
    fn converge(&mut self) {
        while !self.inflight.is_empty() {
            self.settle_oldest(true);
        }
        for o in self.live_objs() {
            self.rig.world.objs.get_mut(&o).unwrap().unsent = 0;
            // clear_mask dropped bits on purpose during the run: mark everything dirty again so the client can converge.
            self.rig.mgr.set_mask(o, 0x1ff);
        }
        self.rig.now += 6_000;
        for _ in 0..4000 {
            if self.rig.mgr.pending() == 0 && self.inflight.is_empty() {
                break;
            }
            let s = self.rig.send_with(8000);
            self.inflight.push(s);
            while !self.inflight.is_empty() {
                self.settle_oldest(true);
            }
        }
        assert_eq!(
            self.rig.mgr.pending(),
            0,
            "the manager drained its pending ghosts"
        );
        // Every ghost the server counts as live exists on the client with the world's values; the client has nothing else.
        let mut expected: HashSet<u32> = HashSet::new();
        for (&obj, o) in &self.rig.world.objs {
            if let Some(idx) = self.rig.mgr.ghost_index(obj) {
                expected.insert(idx);
                let t = self.rig.client.ghost(idx).expect("ghost on the client");
                for b in 0..8 {
                    assert_eq!(t.fields[b], Some(o.fields[b]), "obj {obj} field {b}");
                }
                if let Some(n) = t.bulk {
                    assert_eq!(n, o.bulk);
                }
            }
        }
        let have: HashSet<u32> = self.rig.client.indices().collect();
        assert_eq!(
            have, expected,
            "the client holds exactly the ghosts the server counts as live"
        );
    }
}

#[test]
fn ten_thousand_random_operations_keep_the_table_valid_and_the_client_in_step() {
    let mut total = 0;
    let mut stats = [0usize; 5];
    for seed in 1..=10u64 {
        let mut f = Fuzz::new(seed, 262_144);
        for k in 0..6 {
            f.rig.add(9_000_000 + k);
            f.rig.world.always.push(9_000_000 + k);
        }
        let events = f.rig.mgr.activate(&f.rig.world).unwrap();
        f.rig.deliver_events(&events);
        let seq = f.rig.mgr.sequence();
        f.rig
            .mgr
            .on_connection_message(
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
                &mut f.rig.scripts,
            )
            .unwrap();
        for _ in 0..1000 {
            f.step();
            total += 1;
        }
        f.converge();
        for (total, mine) in stats.iter_mut().zip(f.stats) {
            *total += mine;
        }
        f.rig.mgr.check_invariants().unwrap();
    }
    assert_eq!(total, 10_000);
    let [records, kills, refused, full, resets] = stats;
    assert!(
        records > 1000 && kills > 50 && full > 50 && resets > 50,
        "{stats:?}"
    );
    let _ = refused;
}

#[test]
fn a_tiny_ghost_limit_under_pressure_keeps_ids_inside_the_region() {
    for seed in 100..110u64 {
        let mut f = Fuzz::new(seed, 24);
        let events = f.rig.mgr.activate(&f.rig.world).unwrap();
        f.rig.deliver_events(&events);
        let seq = f.rig.mgr.sequence();
        f.rig
            .mgr
            .on_connection_message(
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
                &mut f.rig.scripts,
            )
            .unwrap();
        for _ in 0..1500 {
            f.step();
            assert!(f.rig.mgr.ghost_count() <= 24);
            for id in f.rig.mgr.array_ids() {
                assert!(id < 24, "id {id} escapes its region");
            }
        }
    }
}

#[test]
fn mutated_packets_never_panic_the_reference_client() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 40);
    for o in 1..=40u32 {
        rig.world.objs.get_mut(&o).unwrap().bulk = (o % 7) as usize;
    }
    let s = rig.send();
    let mut rng = Rng(0xdead_beef_1234_5671);
    let mut errors = 0;
    for _ in 0..3000 {
        let mut b = s.bytes.clone();
        for _ in 0..1 + rng.below(4) {
            let k = rng.below(b.len() as u64) as usize;
            b[k] ^= 1 << rng.below(8);
        }
        if rng.chance(20) {
            b.truncate(rng.below(b.len() as u64) as usize);
        }
        let mut c = GhostClient::new(ToyCodec, 6, 18);
        if c.read_section(&mut BitReader::new(&b)).is_err() {
            errors += 1;
        }
    }
    assert!(errors > 0);
}

#[test]
fn ten_thousand_short_random_sequences_converge() {
    for seed in 1_000..11_000u64 {
        let mut f = Fuzz::new(seed, 262_144);
        if seed % 3 == 0 {
            f.rig.add(9_000_000);
            f.rig.world.always.push(9_000_000);
        }
        let events = f.rig.mgr.activate(&f.rig.world).unwrap();
        f.rig.deliver_events(&events);
        let seq = f.rig.mgr.sequence();
        f.rig
            .mgr
            .on_connection_message(
                &ConnectionMessage {
                    seq,
                    message: 1,
                    count: 0,
                },
                &mut f.rig.scripts,
            )
            .unwrap();
        for _ in 0..40 {
            f.step();
        }
        f.converge();
        f.rig.mgr.check_invariants().unwrap();
    }
}

// ---- the budget: a scan larger than its budget resumes, records have their own

/// Sends `packets` packets with `scan` units of scan budget each (and free records), acking each; returns the records sent.
fn run_with_scan_budget<P: ObjectPolicy<World>>(
    rig: &mut Rig<P>,
    packets: usize,
    scan: u64,
) -> usize {
    let mut records = 0;
    for _ in 0..packets {
        rig.seq += 1;
        rig.now += 1;
        let mut w = BitWriter::with_limit(12_000);
        let mut left = scan;
        let st = rig.mgr.write_section(
            &mut w,
            8000,
            rig.seq,
            rig.now,
            &mut rig.world,
            SectionBudget {
                scan: &mut |n| {
                    if n > left {
                        left = 0;
                        false
                    } else {
                        left -= n;
                        true
                    }
                },
                records: &mut |_| true,
            },
        );
        records += st.records;
        let s = Sent {
            bytes: w.bytes().to_vec(),
            seq: rig.seq,
            stats: st,
        };
        rig.ack(&s);
    }
    records
}

#[test]
fn a_full_query_larger_than_its_budget_resumes_and_records_are_still_sent() {
    let mut rig = Rig::new();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    for k in 0..300 {
        rig.add(100 + k);
    }
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.now = 5_000;
    let records = run_with_scan_budget(&mut rig, 50, 150);
    assert!(records > 0, "records are written whatever the scan spent");
    assert_eq!(
        rig.mgr.ghost_count(),
        301,
        "every in-range object is ghosted in the end"
    );
    assert_eq!(rig.client.len(), 301);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn dependents_cut_by_the_budget_are_scoped_by_the_next_packets() {
    let mut rig = Rig::new();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    rig.add(2);
    for k in 0..400 {
        rig.add(100 + k);
        // Only object 2 is in the query's reach.
        rig.world.objs.get_mut(&(100 + k)).unwrap().pos = [5_000.0, 0.0, 0.0];
    }
    rig.world.objs.get_mut(&2).unwrap().deps = (0..400).map(|k| 100 + k).collect();
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.now = 5_000;
    run_with_scan_budget(&mut rig, 30, 60);
    assert_eq!(rig.mgr.ghost_count(), 402);
}

#[test]
fn a_recent_list_larger_than_the_budget_is_walked_in_pieces() {
    let mut rig = Rig::spaced();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    for k in 0..50 {
        rig.add(100 + k);
    }
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    // The policy's list is walked between full queries, resuming where the budget cut it.
    rig.world.recent = (0..50).map(|k| 100 + k).collect();
    run_with_scan_budget(&mut rig, 10, 12);
    assert_eq!(rig.mgr.ghost_count(), 50);
}

#[test]
fn records_are_written_even_when_the_scan_budget_is_zero() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 5);
    let records = run_with_scan_budget(&mut rig, 1, 0);
    assert_eq!(records, 5);
}

#[test]
fn a_section_into_an_already_overflowed_stream_writes_nothing() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 3);
    let mut w = BitWriter::with_limit(8);
    w.write_bits(0, 20);
    assert!(w.is_overflowed());
    let st = rig
        .mgr
        .write_section(&mut w, 8000, 1, 1, &mut rig.world, free!());
    assert!(st.idle);
    assert_eq!(w.bits(), 0);
    assert_eq!(rig.mgr.refs_in_flight(), 0);
}

// ---- clearMaskBits

#[test]
fn clear_mask_zeroes_only_an_equal_mask_as_clear_mask_bits_does() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 3);
    let s = rig.send();
    rig.ack(&s);
    rig.mgr.set_mask(1, 0b0110);
    rig.mgr.set_mask(2, 0b0110);
    rig.mgr.set_mask(3, 0b0110);
    assert_eq!(rig.mgr.pending(), 3);
    // mask == bits: zero, and the ghost leaves the pending region.
    rig.mgr.clear_mask(1, 0b0110);
    assert_eq!(rig.mgr.view_of(1).unwrap().mask, 0);
    assert_eq!(rig.mgr.pending(), 2);
    // Part of the mask: the rest stays.
    rig.mgr.clear_mask(2, 0b0010);
    assert_eq!(rig.mgr.view_of(2).unwrap().mask, 0b0100);
    // bits covering the mask without equalling it: the mask is 0 but the ghost STAYS pending (clearMaskBits tests equality only).
    rig.mgr.clear_mask(3, 0b1110);
    assert_eq!(rig.mgr.view_of(3).unwrap().mask, 0);
    assert_eq!(rig.mgr.pending(), 2);
    rig.mgr.check_invariants().unwrap();
    // set_mask on that ghost does not move it again (the array would swap it with array[Zero]).
    rig.mgr.set_mask(3, 1);
    assert_eq!(rig.mgr.pending(), 2);
    rig.mgr.check_invariants().unwrap();
    // The next send writes a record for each pending ghost and settles them.
    let s = rig.send();
    assert_eq!(s.stats.records, 2);
    rig.ack(&s);
    assert_eq!(rig.mgr.pending(), 0);
}

// ---- survivors of the mutation review

#[test]
fn the_sort_is_decided_before_the_dead_entries_are_freed() {
    // 51 pending under a sort limit of 50, one of them dead: the decision is made on 51 (unsorted), and then it is freed.
    let mut rig = Rig::spaced();
    rig.go();
    rig.scope_many(1, 51);
    for k in 0..51u32 {
        rig.world.objs.get_mut(&(1 + k)).unwrap().prio = 1000.0 - k as f32;
    }
    rig.mgr.detach_object(8);
    let s = rig.send_with(6);
    assert_eq!(s.stats.records, 1);
    let first = rig.read(&s).first_index.unwrap();
    // Unsorted: the dead id 7 swaps with the last (id 50); the array end is then id 49. A sort would put id 0 first.
    assert_eq!(first, 49);
}

#[test]
fn skip_count_saturates_at_255() {
    let mut rig = Rig::new();
    rig.add(1);
    rig.add(2);
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig.mgr.object_in_scope_now(&rig.world, 2);
    for k in 1..=260u32 {
        rig.now = 5_000 * k;
        rig.send_with(0);
    }
    assert_eq!(rig.mgr.view_of(2).unwrap().skip, 255);
}

#[test]
fn scope_always_objects_start_with_flags_0e_and_others_0a() {
    let mut rig = Rig::new();
    rig.go();
    rig.add(1);
    rig.world.add(2, {
        let mut o = Obj::new(1);
        o.nflags = net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS;
        o
    });
    rig.mgr.object_in_scope_now(&rig.world, 1);
    rig.mgr.object_in_scope_now(&rig.world, 2);
    assert_eq!(rig.mgr.view_of(1).unwrap().flags, 0x0a);
    assert_eq!(rig.mgr.view_of(2).unwrap().flags, 0x0e);
}

#[test]
fn a_full_query_drops_in_scope_except_for_scope_always_and_scope_local_always() {
    // Torque's schedule detaches the pending ghost the query did not find; a schedule without the detach only clears its flag.
    for torque in [true, false] {
        let mut world = World::default();
        world.add(1, Obj::new(1));
        world.add(3, Obj::new(1));
        world.add(4, {
            let mut o = Obj::new(1);
            o.nflags = net_flags::GHOSTABLE | net_flags::SCOPE_ALWAYS;
            o
        });
        world.add(2, Obj::new(1));
        // Ghost 3 is out of the query's reach.
        world.objs.get_mut(&3).unwrap().pos = [5_000.0, 0.0, 0.0];
        let flags_of =
            |mgr_view: Option<super::GhostView>| mgr_view.map(|v| v.flags & flags::IN_SCOPE);
        let (f2, f3, f4) = if torque {
            let mut rig = Rig::new();
            rig.world = world;
            rig.go();
            rig.mgr.set_scope_object(Some(1));
            rig.mgr.scope_to_client(&rig.world, 2);
            rig.mgr.object_in_scope_now(&rig.world, 3);
            rig.mgr.object_in_scope_now(&rig.world, 4);
            assert!(rig.send_with(0).stats.full_query);
            rig.mgr.check_invariants().unwrap();
            (
                flags_of(rig.mgr.view_of(2)),
                flags_of(rig.mgr.view_of(3)),
                flags_of(rig.mgr.view_of(4)),
            )
        } else {
            let mut rig = Rig::spaced();
            rig.world = world;
            rig.go();
            rig.mgr.set_scope_object(Some(1));
            rig.mgr.scope_to_client(&rig.world, 2);
            rig.mgr.object_in_scope_now(&rig.world, 3);
            rig.mgr.object_in_scope_now(&rig.world, 4);
            rig.now = 3_000;
            assert!(rig.send_with(0).stats.full_query);
            (
                flags_of(rig.mgr.view_of(2)),
                flags_of(rig.mgr.view_of(3)),
                flags_of(rig.mgr.view_of(4)),
            )
        };
        assert_eq!(f2, Some(flags::IN_SCOPE), "ScopeLocalAlways keeps it");
        assert_eq!(f4, Some(flags::IN_SCOPE), "ScopeAlways keeps it");
        if torque {
            assert_eq!(f3, None, "the ghost lost its object");
        } else {
            assert_eq!(f3, Some(0), "an ordinary ghost loses InScope");
        }
    }
}

#[test]
fn torques_detached_ghost_is_killed_on_the_wire_and_comes_back_when_found_again() {
    let mut rig = Rig::new();
    rig.world.sky = Some(100.0);
    rig.add(1);
    rig.add(2);
    rig.world.objs.get_mut(&2).unwrap().pos = [50.0, 0.0, 0.0];
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    let s = rig.send();
    rig.ack(&s);
    assert_eq!(rig.client.len(), 2);
    // Object 2 moves out and changes: its pending ghost is not found, so it is detached and its kill record goes out.
    rig.world.objs.get_mut(&2).unwrap().pos = [500.0, 0.0, 0.0];
    rig.world.objs.get_mut(&2).unwrap().fields[0] = 7;
    rig.mgr.set_mask(2, 1);
    let s = rig.send();
    assert_eq!(s.stats.kills, 1);
    rig.ack(&s);
    assert_eq!(rig.client.len(), 1);
    assert!(rig.mgr.view_of(2).is_none());
    // Back in range: a new ghost.
    rig.world.objs.get_mut(&2).unwrap().pos = [50.0, 0.0, 0.0];
    let s = rig.send();
    rig.ack(&s);
    assert_eq!(rig.client.len(), 2);
    rig.mgr.check_invariants().unwrap();
}

// ---- the feed of new objects

/// A rig of the spaced policy after its first full query: the scope object 1 ghosted, the clock past the query.
fn fed_rig(fresh: bool) -> Rig<Spaced> {
    let mut rig = Rig::spaced();
    rig.world.sky = Some(1000.0);
    rig.add(1);
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    if fresh {
        let end = ObjectSource::fresh_end(&rig.world);
        rig.mgr.set_fresh(Some(end));
    }
    rig.now = 3_000;
    assert!(rig.send().stats.full_query);
    rig
}

fn plant<P: GhostPolicy>(rig: &mut Rig<P>, id: ObjId, x: f32) {
    rig.world.add(id, {
        let mut o = Obj::new(1);
        o.pos = [x, 0.0, 0.0];
        o
    });
    rig.world.fresh.push(id);
}

/// A new object whose centre is past the radius but which the full query would take (its box reaches in): a fed connection meets it
/// in the next packet, as one inside the radius; one the full query would not take waits as before.
#[test]
fn a_new_object_the_full_query_would_take_is_met_in_the_next_packet() {
    let mut rig = fed_rig(true);
    plant(&mut rig, 2, 900.0);
    plant(&mut rig, 3, 900.0);
    rig.world.reach.push(2);
    rig.now = 3_032;
    let s = rig.send();
    assert!(!s.stats.full_query);
    assert!(rig.mgr.view_of(2).is_some(), "its box reaches the scope");
    assert!(rig.mgr.view_of(3).is_none(), "past the radius");
}

#[test]
fn a_fed_connection_meets_a_new_object_in_the_next_packet_and_another_at_the_next_full_query() {
    for fresh in [true, false] {
        let mut rig = fed_rig(fresh);
        plant(&mut rig, 2, 10.0);
        plant(&mut rig, 3, 900.0);
        rig.now = 3_032;
        let s = rig.send();
        assert!(!s.stats.full_query);
        assert_eq!(rig.mgr.view_of(2).is_some(), fresh, "fresh {fresh}");
        assert!(rig.mgr.view_of(3).is_none(), "past the radius");
        rig.now = 6_000;
        assert!(rig.send().stats.full_query);
        assert!(rig.mgr.view_of(2).is_some());
    }
}

#[test]
fn the_feed_goes_on_where_a_spent_budget_left_it_and_past_what_the_host_let_go() {
    let mut rig = fed_rig(true);
    for k in 0..40 {
        plant(&mut rig, 100 + k, 1.0 + k as f32);
    }
    rig.now = 3_032;
    let mut left = 10u64;
    let mut scan = |u: u64| {
        if left >= u {
            left -= u;
            true
        } else {
            false
        }
    };
    let mut w = BitWriter::with_limit(12_000);
    rig.mgr.write_section(
        &mut w,
        8_000,
        99,
        rig.now,
        &mut rig.world,
        SectionBudget {
            scan: &mut scan,
            records: &mut |_| true,
        },
    );
    let met = (0..40)
        .filter(|k| rig.mgr.view_of(100 + k).is_some())
        .count();
    assert!(met > 0 && met < 40, "{met} met in the cut packet");
    let cursor = rig.mgr.fresh_cursor().unwrap();
    assert_eq!(cursor, rig.world.fresh_base + met as u64);
    rig.now = 3_064;
    rig.send();
    assert!((0..40).all(|k| rig.mgr.view_of(100 + k).is_some()));
    // The host lets go of objects the reader has not reached: it goes on from the first one held.
    for k in 0..10 {
        plant(&mut rig, 200 + k, 5.0);
    }
    rig.world.fresh.drain(..rig.world.fresh.len() - 3);
    rig.world.fresh_base = 1_000;
    rig.now = 3_096;
    rig.send();
    assert_eq!(rig.mgr.fresh_cursor(), Some(1_003));
    assert!((7..10).all(|k| rig.mgr.view_of(200 + k).is_some()));
}

// ---- the ghost policy's admission, drops and scope rules

/// A test policy: it counts the ghosts of class-2 objects (made, still linked), and a walk may make at most `limit` of them; with
/// `drop_past` set, a full query that ends asks for a drop pass over them that drops the ghosts of objects farther than that, and a
/// later query leaves them out.
#[derive(Debug, Clone, Default)]
struct Grouping {
    limit: usize,
    drop_past: Option<f32>,
    held: HashSet<u32>,
    linked: HashSet<u32>,
    refused: usize,
    dropped: usize,
    queries: usize,
    resets: usize,
}

impl GhostPolicy for Grouping {
    fn query_ended(&mut self, _now: u32, _radius: f32, _table: &TableCounts) -> bool {
        self.queries += 1;
        self.drop_past.is_some()
    }

    fn drop_candidate(&self, id: u32) -> bool {
        self.held.contains(&id)
    }

    fn ghost_unlinked(&mut self, id: u32) {
        self.linked.remove(&id);
    }

    fn ghost_freed(&mut self, id: u32) {
        self.held.remove(&id);
        self.linked.remove(&id);
    }

    fn reset(&mut self) {
        self.resets += 1;
        self.held.clear();
        self.linked.clear();
    }
}

impl ObjectPolicy<World> for Grouping {
    fn admit(
        &mut self,
        src: &World,
        _scope: Option<ObjId>,
        obj: ObjId,
        _table: &TableCounts,
    ) -> bool {
        let counted = src.objs.get(&obj).is_some_and(|o| o.class == 2);
        if counted && self.held.len() >= self.limit {
            self.refused += 1;
            return false;
        }
        true
    }

    fn ghost_made(&mut self, src: &World, obj: ObjId, id: u32) {
        if src.objs.get(&obj).is_some_and(|o| o.class == 2) {
            self.held.insert(id);
            self.linked.insert(id);
        }
    }

    fn query_visit(&mut self, src: &World, scope: ObjId, obj: ObjId) -> bool {
        match self.drop_past {
            Some(r) => src.scope_distance(obj, scope).is_none_or(|d| d <= r),
            None => true,
        }
    }

    fn drop_now(&mut self, src: &World, scope: ObjId, obj: ObjId) -> bool {
        let far = self
            .drop_past
            .zip(src.scope_distance(obj, scope))
            .is_some_and(|(r, d)| d > r);
        self.dropped += usize::from(far);
        far
    }
}

/// A world of `n` class-2 objects along the x axis at 1, 2, .. `n` units from the scope object (object 1, at the origin) and
/// `others` class-3 objects 150 units away after them, under `policy`.
fn grouped_rig(max: u32, n: u32, others: u32, policy: Grouping) -> Rig<Grouping> {
    let mut rig = Rig::with_policy(max, policy);
    rig.add(1);
    for d in 1..=n {
        rig.world.add(1_000 + d, {
            let mut o = Obj::new(2);
            o.pos = [d as f32, 0.0, 0.0];
            o
        });
    }
    for k in 0..others {
        rig.world.add(100 + k, {
            let mut o = Obj::new(3);
            o.pos = [0.0, 150.0, 0.0];
            o
        });
    }
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    rig
}

/// Packets every 32 ms for `ms`, each read by the client and acknowledged.
fn run_for<P: ObjectPolicy<World>>(rig: &mut Rig<P>, ms: u32) {
    let end = rig.now + ms;
    while rig.now < end {
        rig.now += 32;
        let s = rig.send();
        rig.ack(&s);
    }
}

#[test]
fn a_policys_admission_keeps_room_in_the_table_for_the_other_objects() {
    let policy = Grouping {
        limit: 40,
        ..Grouping::default()
    };
    let mut rig = grouped_rig(64, 200, 5, policy);
    run_for(&mut rig, 12_000);
    rig.mgr.check_invariants().unwrap();
    assert_eq!(rig.mgr.policy().held.len(), 40);
    assert_eq!(rig.mgr.policy().linked.len(), 40);
    assert!(rig.mgr.policy().refused > 0);
    // The others and the scope object are in, and the client holds what the server says it holds.
    assert!((0..5).all(|k| rig.mgr.ghost_index(100 + k).is_some()));
    assert!(rig.mgr.ghost_index(1).is_some());
    assert_eq!(rig.client.len(), 40 + 5 + 1);
    // A script's scoping is never refused.
    rig.mgr.scope_to_client(&rig.world, 1_200);
    assert_eq!(rig.mgr.policy().held.len(), 41);
}

#[test]
fn without_a_policy_a_full_table_refuses_every_object_as_torque_does() {
    let mut rig = Rig::with(64);
    rig.add(1);
    for d in 1..=200 {
        rig.world.add(1_000 + d, Obj::new(2));
    }
    for k in 0..5 {
        rig.world.add(100 + k, Obj::new(3));
    }
    rig.go();
    rig.mgr.set_scope_object(Some(1));
    run_for(&mut rig, 12_000);
    assert_eq!(rig.mgr.ghost_count(), 64);
    assert_eq!(rig.mgr.table_counts().ghosts, 64);
    assert!((0..5).any(|k| rig.mgr.view_of(100 + k).is_none()));
}

#[test]
fn a_policys_drop_pass_drops_the_ghosts_it_names_but_never_a_scripted_one() {
    let policy = Grouping {
        limit: usize::MAX,
        ..Grouping::default()
    };
    let mut rig = grouped_rig(1_024, 100, 0, policy);
    // The farthest object, scoped by a script.
    rig.mgr.scope_to_client(&rig.world, 1_100);
    run_for(&mut rig, 6_000);
    assert!((1..=100).all(|d| rig.mgr.ghost_index(1_000 + d).is_some()));
    // From now on the objects past 50 are dropped and left out.
    rig.mgr.policy_mut().drop_past = Some(50.0);
    run_for(&mut rig, 12_000);
    rig.mgr.check_invariants().unwrap();
    let held: Vec<u32> = (1..=100)
        .filter(|d| rig.mgr.ghost_index(1_000 + d).is_some())
        .collect();
    let mut want: Vec<u32> = (1..=50).collect();
    want.push(100);
    assert_eq!(held, want);
    assert_eq!(rig.mgr.policy().dropped, 49);
    assert_eq!(rig.client.len(), 51 + 1);
    // A reset tells the policy.
    rig.mgr.reset();
    assert_eq!(rig.mgr.policy().resets, 1);
    assert!(rig.mgr.policy().held.is_empty());
}

#[test]
fn the_default_policy_changes_nothing_a_grouping_one_would_not() {
    // The same world under Torque's policy and under a group without a limit: the same bytes in every packet.
    let mut a = Rig::with(1_024);
    let mut b = Rig::with_policy(
        1_024,
        Grouping {
            limit: usize::MAX,
            ..Grouping::default()
        },
    );
    for rig_world in [&mut a.world, &mut b.world] {
        rig_world.add(1, Obj::new(1));
        for d in 1..=300 {
            let mut o = Obj::new(2);
            o.pos = [d as f32, 0.0, 0.0];
            rig_world.add(1_000 + d, o);
        }
    }
    a.go();
    b.go();
    a.mgr.set_scope_object(Some(1));
    b.mgr.set_scope_object(Some(1));
    for _ in 0..400 {
        a.now += 32;
        b.now += 32;
        let (sa, sb) = (a.send(), b.send());
        assert_eq!(sa.bytes, sb.bytes);
        a.ack(&sa);
        b.ack(&sb);
    }
    assert_eq!(a.mgr.ghost_count(), 301);
    assert_eq!(b.mgr.policy().held.len(), 300);
    assert_eq!(b.mgr.policy().refused, 0);
}

// ---- wider ghost ids

/// A section creating one toy ghost at `index`, written with ids `id_bits` wide.
fn one_new_ghost(index: u32, id_bits: u8) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_flag(true);
    w.write_bits(u32::from(id_bits - 3), 5);
    w.write_flag(true);
    w.write_bits(index, id_bits);
    w.write_flag(false);
    w.write_flag(false);
    w.write_bits(1, 6);
    for _ in 0..9 {
        w.write_flag(false);
    }
    w.write_flag(false);
    w.into_bytes()
}

#[test]
fn a_ghost_index_past_the_connections_table_is_refused() {
    // An 18-bit table (Torque3D's; TGE 1.4's is 12 bits): an index past 2^18 is a hostile server's, though the section's id width may carry it.
    let mut c = GhostClient::new(ToyCodec, 6, 18);
    assert_eq!(c.table_size(), 1 << 18);
    let bytes = one_new_ghost((1 << 18) - 1, 18);
    assert!(c.read_section(&mut BitReader::new(&bytes)).is_ok());
    let bytes = one_new_ghost(1 << 18, 24);
    assert!(c.read_section(&mut BitReader::new(&bytes)).is_err());
    assert_eq!(c.len(), 1);
    // A 22-bit table takes it, and stops at 2^22.
    let mut c = GhostClient::new(ToyCodec, 6, 22);
    let bytes = one_new_ghost((1 << 22) - 1, 22);
    assert!(c.read_section(&mut BitReader::new(&bytes)).is_ok());
    let bytes = one_new_ghost(1 << 22, 23);
    assert!(c.read_section(&mut BitReader::new(&bytes)).is_err());
    // Past the widest any connection has, the table is 2^24.
    assert_eq!(
        GhostClient::new(ToyCodec, 6, 31).table_size(),
        crate::cap::NET_GHOST_LIMIT
    );
}

#[test]
fn a_table_of_2_pow_22_ghosts_numbers_them_past_20_bits() {
    // A manager of a 22-bit table: the ghost-always ids take its top, events carry 22 bits, and the client reads them.
    let mut rig = Rig::with(1 << 22);
    assert_eq!(rig.mgr.config().ghost_bits(), 22);
    for k in 0..3 {
        rig.world.add(10 + k, Obj::new(1));
        rig.world.always.push(10 + k);
    }
    rig.go();
    let ids: Vec<u32> = (10..13).map(|o| rig.id(o)).collect();
    assert_eq!(ids, [(1 << 22) - 3, (1 << 22) - 2, (1 << 22) - 1]);
    assert_eq!(rig.client.len(), 3);
    rig.scope_many(100, 5);
    let s = rig.send();
    rig.ack(&s);
    assert_eq!(rig.client.len(), 8);
    assert!(
        GhostManager::new(GhostConfig {
            max_ghosts: (1 << 24) + 1,
            local: false,
            class_bits: 6,
        })
        .is_err()
    );
}

// ---- the datagram bound

/// The datagram's bits: a data packet's stream.
const STREAM: usize = crate::cap::NET_MAX_DATAGRAM * 8;

/// A writer of the stream's size with `bits` zero bits written.
fn padded(bits: usize) -> BitWriter {
    let mut w = BitWriter::with_limit(STREAM);
    for _ in 0..bits / 32 {
        w.write_u32(0);
    }
    w.write_bits(0, (bits % 32) as u8);
    w
}

fn skip(r: &mut BitReader, bits: usize) {
    for _ in 0..bits / 32 {
        r.read_bits(32).unwrap();
    }
    r.read_bits((bits % 32) as u8).unwrap();
}

#[test]
fn a_record_that_would_leave_no_room_for_the_closing_flag_waits() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 1);
    rig.world.objs.get_mut(&1).unwrap().bulk = 1_300;
    let record = sent_bits(&mut rig) - 7;
    rig.mgr.on_notify(rig.seq, false);
    // The record would end on the stream's last bit, the closing flag past it.
    let pad = STREAM - 6 - record;
    let mut w = padded(pad);
    let st = rig.mgr.write_section(
        &mut w,
        STREAM,
        rig.seq + 1,
        rig.now,
        &mut rig.world,
        free!(),
    );
    assert!(!w.is_overflowed());
    assert_eq!((st.records, st.refused), (0, 1));
    assert_eq!(w.bits(), pad + 7);
    // One bit more room and it goes, the closing flag on the last bit.
    let mut w = padded(pad - 1);
    let st = rig.mgr.write_section(
        &mut w,
        STREAM,
        rig.seq + 2,
        rig.now,
        &mut rig.world,
        free!(),
    );
    assert!(!w.is_overflowed());
    assert_eq!(st.records, 1);
    assert_eq!(w.bits(), STREAM);
    let mut r = BitReader::new(w.bytes());
    skip(&mut r, pad - 1);
    assert_eq!(rig.client.read_section(&mut r).unwrap().created, 1);
    rig.mgr.check_invariants().unwrap();
}

#[test]
fn a_ghosting_section_with_no_room_for_its_header_says_not_ghosting() {
    let mut rig = Rig::new();
    rig.go();
    rig.scope_many(1, 3);
    for room in 1..super::manager::SECTION_MIN_BITS {
        let mut w = padded(STREAM - room);
        let st = rig
            .mgr
            .write_section(&mut w, STREAM, 1, rig.now, &mut rig.world, free!());
        assert!(!w.is_overflowed(), "room {room}");
        assert!(st.idle);
        assert_eq!(w.bits(), STREAM - room + 1);
    }
    rig.mgr.check_invariants().unwrap();
}

/// A test event: a 16-bit length and that many zero bits; a server-to-client test class unordered, the any-direction one ordered.
#[derive(Debug, Clone)]
struct Pad(u16, Guarantee);

impl WireEvent for Pad {
    fn class(&self, _: &EventIds) -> u32 {
        if self.1 == Guarantee::Ordered {
            crate::testids::ANY
        } else {
            crate::testids::TO_CLIENT
        }
    }
    fn guarantee(&self) -> Guarantee {
        self.1
    }
    fn pack(&self, w: &mut BitWriter, _: &EventCtx<'_>) {
        w.write_bits(u32::from(self.0), 16);
        let mut left = usize::from(self.0);
        while left > 0 {
            let n = left.min(32);
            w.write_bits(0, n as u8);
            left -= n;
        }
    }
}

struct PadU;

impl EventUnpacker for PadU {
    type Event = Pad;
    fn unpack(&mut self, class: u32, r: &mut BitReader, _: &EventCtx<'_>) -> Result<Pad, NetError> {
        let len = r.read_bits(16)? as u16;
        let mut left = usize::from(len);
        while left > 0 {
            let n = left.min(32);
            r.read_bits(n as u8)?;
            left -= n;
        }
        let g = if class == crate::testids::ANY {
            Guarantee::Ordered
        } else {
            Guarantee::Guaranteed
        };
        Ok(Pad(len, g))
    }
}

/// Packets built as the server builds them (a start for the header and the head, the events holding back the smallest ghost section,
/// then the ghosts) from queues of events and ghost records of every size up to about an empty packet's: none passes the datagram bound
/// and the client reads every one whole. A tenth are lost, so the work is sent again at other offsets.
#[test]
fn packets_of_worst_case_events_and_ghosts_never_pass_the_datagram_bound() {
    let table = crate::testids::table();
    let ids = crate::testids::ids();
    // New-ghost records with the implied-class flag: a bit more each.
    let mut rig = Rig::with_policy(crate::profile::DEFAULT_GHOST_LIMIT, Implying);
    rig.go();
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: rig.mgr.config().ghost_bits(),
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    let n = 80u32;
    rig.scope_many(1, n);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move |below: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % below
    };
    let mut q: EventQueue<Pad> = EventQueue::new();
    let mut rx: EventReceiver<Pad> = EventReceiver::new();
    let (mut near, mut records, mut events) = (0, 0, 0);
    for packet in 0..3_000u32 {
        for _ in 0..3 {
            let o = 1 + next(u64::from(n)) as u32;
            rig.world.objs.get_mut(&o).unwrap().bulk = next(1_450) as usize;
            rig.mgr.set_mask(o, 1 << 8);
        }
        while q.room_unordered() > 0 && next(3) == 0 {
            let _ = q.post(Pad(next(10_000) as u16, Guarantee::Guaranteed));
        }
        while q.room_ordered() > 0 && next(3) == 0 {
            let _ = q.post(Pad(next(3_000) as u16, Guarantee::Ordered));
        }
        let seq = packet + 1;
        let start = 40 + next(400) as usize;
        let packet_bits = if next(2) == 0 { 1_400 * 8 } else { 450 * 8 };
        let mut w = padded(start);
        let tail = w.hold_back(super::manager::SECTION_MIN_BITS);
        let params = SectionParams {
            ghosting: next(2) == 0,
            hold_ordered: false,
            packet_bits,
            min_start_bits: 500,
        };
        let written = q.write_section(&mut w, &ctx, seq, params).unwrap();
        w.release(tail);
        let st = rig
            .mgr
            .write_section(&mut w, packet_bits, seq, rig.now, &mut rig.world, free!());
        assert!(!w.is_overflowed(), "packet {packet} overflowed");
        assert!(w.bits() <= STREAM);
        near += usize::from(w.bits() + 64 > STREAM);
        records += st.records;
        events += written.unordered + written.ordered;
        let delivered = next(10) != 0;
        if delivered {
            let mut r = BitReader::new(w.bytes());
            skip(&mut r, start);
            let mut out = Vec::new();
            rx.read_section(
                &mut r,
                &ctx,
                crate::protocol::Role::Client,
                &mut PadU,
                &mut out,
                None,
            )
            .unwrap();
            rig.client.read_section(&mut r).unwrap();
            assert!(r.bits_left() < 8);
        }
        let mut settled = Vec::new();
        q.on_notify(seq, delivered, &mut settled);
        rig.mgr.on_notify(seq, delivered);
        rig.now += 32;
    }
    rig.mgr.check_invariants().unwrap();
    assert!(near > 50, "only {near} packets came near the bound");
    assert!(
        records > 1_000 && events > 1_000,
        "{records} records, {events} events"
    );
}

#[test]
fn the_default_pending_sort_keeps_equal_priorities_in_array_order() {
    let world = World::default();
    let mut items = [
        (1.0f32, 5u32),
        (2.0, 1),
        (1.0, 3),
        (-0.0, 8),
        (0.5, 7),
        (0.0, 2),
    ];
    world.sort_pending(&mut items);
    let ids: Vec<u32> = items.iter().map(|i| i.1).collect();
    assert_eq!(ids, [8, 2, 7, 5, 3, 1]);
    // A NaN priority never makes the sort fail.
    let mut odd = [
        (f32::NAN, 1u32),
        (1.0, 2),
        (f32::INFINITY, 3),
        (-f32::NAN, 4),
    ];
    world.sort_pending(&mut odd);
    assert_eq!(odd.len(), 4);
}
