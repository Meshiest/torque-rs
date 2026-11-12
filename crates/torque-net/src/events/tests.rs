use super::*;
use crate::netstring::NetStringEvent;
use crate::remote::RemoteArg;
use crate::testids::{ANY, TO_CLIENT, TO_SERVER, ids, with_ctx};
use torque_testkit::Rng;

/// A test event: a 16-bit length, then that many bits of a repeated 32-bit pattern, of one of the test classes: `ANY` is direction
/// `Any`, `TO_CLIENT` server to client, `TO_SERVER` client to server.
#[derive(Debug, Clone, PartialEq, Eq)]
struct T {
    class: u32,
    len: u16,
    value: u32,
    g: Guarantee,
}

fn t(len: u16, value: u32) -> T {
    T {
        class: ANY,
        len,
        value,
        g: Guarantee::Ordered,
    }
}

impl WireEvent for T {
    fn class(&self, _: &EventIds) -> u32 {
        self.class
    }
    fn guarantee(&self) -> Guarantee {
        self.g
    }
    fn pack(&self, w: &mut BitWriter, _: &EventCtx<'_>) {
        w.write_bits(u32::from(self.len), 16);
        let mut left = u32::from(self.len);
        while left > 0 {
            let n = left.min(32);
            w.write_bits(self.value, n as u8);
            left -= n;
        }
    }
}

struct TU;

impl EventUnpacker for TU {
    type Event = T;
    fn unpack(&mut self, class: u32, r: &mut BitReader, _: &EventCtx<'_>) -> Result<T, NetError> {
        let len = r.read_bits(16)? as u16;
        let mut left = u32::from(len);
        let mut value = 0;
        let mut first = true;
        while left > 0 {
            let n = left.min(32);
            let v = r.read_bits(n as u8)?;
            if first {
                value = v;
                first = false;
            } else if v != value & if n == 32 { u32::MAX } else { (1 << n) - 1 } {
                return Err(NetError::Invalid("test pattern"));
            }
            left -= n;
        }
        Ok(T {
            class,
            len,
            value,
            g: Guarantee::Ordered,
        })
    }
}

fn params(empty: usize) -> SectionParams {
    SectionParams {
        ghosting: false,
        hold_ordered: false,
        packet_bits: empty,
        min_start_bits: 0,
    }
}

fn big() -> BitWriter {
    let mut w = BitWriter::with_limit(1023 * 8);
    w.enable_string_buffer();
    w
}

fn read_all(
    bytes: &[u8],
    reader: Role,
    rx: &mut EventReceiver<T>,
) -> Result<Vec<Incoming<T>>, NetError> {
    with_ctx(|ctx| {
        let mut r = BitReader::new(bytes);
        r.enable_string_buffer();
        let mut out = Vec::new();
        rx.read_section(&mut r, ctx, reader, &mut TU, &mut out, None)
            .map(|_| out)
    })
}

#[test]
fn the_event_ids_come_from_the_derived_table() {
    let ids = ids();
    // Torque's 13 classes and the 4 test classes: 17, so 5 bits.
    assert_eq!((ids.count(), ids.bits()), (17, 5));
    assert_eq!(
        (
            ids.connection_message,
            ids.net_string,
            ids.remote_command,
            ids.sim_data_block,
            ids.id_of("TestAnyEvent")
        ),
        (0, 5, 7, 11, Some(ANY))
    );
    assert_eq!(ids.name(5), Some("NetStringEvent"));
    assert_eq!(ids.name(12), Some("SimpleMessageEvent"));
    assert_eq!(ids.name(17), None);
    assert_eq!(ids.id_of("NoSuchEvent"), None);
    assert_eq!(ids.direction(TO_SERVER), Some(Direction::ClientToServer));
    assert_eq!(ids.direction(TO_CLIENT), Some(Direction::ServerToClient));
    assert_eq!(ids.direction(ANY), Some(Direction::Any));
    assert_eq!(ids.direction(17), None);
}

#[test]
fn a_command_class_reads_and_writes_as_a_remote_command_of_its_variant() {
    let mut reg = EventRegistry::torque();
    reg.register_command("TestCommandEvent", Direction::Any);
    let ids = reg.ids().expect("the registry derives");
    let class = ids.id_of("TestCommandEvent").expect("registered");
    assert_eq!(ids.command_class(0), Some(ids.remote_command));
    assert_eq!(ids.command_class(1), Some(class));
    assert_eq!(ids.command_class(2), None);
    assert_eq!(ids.command_variant(class), Some(1));
    assert_eq!(ids.command_variant(ids.net_string), None);
    let table = Table::torque();
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 18,
        datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
    };
    let command = NetEvent::Remote(RemoteCommandEvent {
        variant: 1,
        args: vec![RemoteArg::Tag(3), RemoteArg::Int(-5)],
    });
    assert_eq!(command.class(&ids), class);
    let mut w = BitWriter::new();
    command.pack(&mut w, &ctx);
    let bytes = w.into_bytes();
    let back = NetEvent::unpack(class, &mut BitReader::new(&bytes), &ctx);
    assert_eq!(back, Ok(command));
    // Taken out, it is no command class any more.
    assert!(reg.remove("TestCommandEvent"));
    let ids = reg.ids().expect("the registry derives");
    assert_eq!(ids.command_class(1), None);
}

#[test]
fn the_wire_grammar_is_unordered_then_ordered_with_the_sequence_flag() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        q.post(t(3, 5)).expect("post");
        q.post(t(3, 6)).expect("post");
        q.post(T {
            class: TO_CLIENT,
            len: 2,
            value: 1,
            g: Guarantee::Guaranteed,
        })
        .expect("post");
        let mut w = big();
        let written = q
            .write_section(&mut w, ctx, 0, params(8000))
            .expect("writes");
        assert_eq!(
            written,
            Written {
                unordered: 1,
                ordered: 2
            }
        );

        let mut e = big();
        e.write_flag(true);
        e.write_bits(TO_CLIENT, 5);
        e.write_bits(2, 16);
        e.write_bits(1, 2);
        e.write_flag(false);
        // First ordered event: flag 1, seq flag 0 (prev is -2), 7-bit seq 0.
        e.write_flag(true);
        e.write_flag(false);
        e.write_bits(0, 7);
        e.write_bits(ANY, 5);
        e.write_bits(3, 16);
        e.write_bits(5, 3);
        // Second: seq 1 == prev + 1, so the flag is set and there are no seq bits.
        e.write_flag(true);
        e.write_flag(true);
        e.write_bits(ANY, 5);
        e.write_bits(3, 16);
        e.write_bits(6, 3);
        e.write_flag(false);
        assert_eq!(w.bits(), e.bits());
        assert_eq!(w.bytes(), e.bytes());
    });
}

#[test]
fn an_empty_queue_writes_two_zero_flags() {
    with_ctx(|ctx| {
        let mut q: EventQueue<T> = EventQueue::new();
        let mut w = big();
        assert_eq!(
            q.write_section(&mut w, ctx, 0, params(8000)),
            Ok(Written::default())
        );
        assert_eq!((w.bits(), w.bytes()), (2, [0u8].as_slice()));
        assert!(q.is_idle());
        assert_eq!(q.packets_in_flight(), 0);
    });
}

#[test]
fn the_window_sends_126_events_until_the_run_from_last_acked_plus_one_is_acked() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        for i in 0..300u32 {
            q.post(t(1, i & 1)).expect("post");
        }
        assert_eq!((q.next_seq(), q.last_acked()), (300, u32::MAX));
        // Two packets, each limited so that about half of the 126 go in each.
        let mut sent = Vec::new();
        for send_seq in 0..6u32 {
            let mut w = BitWriter::with_limit(1000);
            w.enable_string_buffer();
            sent.push(
                q.write_section(&mut w, ctx, send_seq, params(900))
                    .expect("ok")
                    .ordered,
            );
        }
        assert_eq!(
            sent.iter().sum::<usize>(),
            126,
            "seq 0..=125 and no more: {sent:?}"
        );
        assert!(sent[..3].iter().all(|&n| n > 0), "{sent:?}");
        assert_eq!(q.ordered_len(), 300 - 126);
        // Acking a later packet alone moves nothing: the run starts at seq 0.
        let mut settled = Vec::new();
        q.on_notify(1, true, &mut settled);
        assert!(settled.is_empty());
        assert_eq!(q.last_acked(), u32::MAX);
        let mut w = big();
        assert_eq!(
            q.write_section(&mut w, ctx, 9, params(8000))
                .expect("ok")
                .ordered,
            0
        );
        // Acking packet 0 settles it and the run through packet 1.
        q.on_notify(0, true, &mut settled);
        assert!(settled.len() > 1);
        assert_eq!(q.last_acked() as usize, settled.len() - 1);
        assert!(settled.iter().all(|s| s.delivered));
        // The window moved: more go out.
        let mut w = big();
        assert!(
            q.write_section(&mut w, ctx, 10, params(8000))
                .expect("ok")
                .ordered
                > 0
        );
    });
}

#[test]
fn hold_ordered_sends_only_the_unordered_events() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        q.post(t(1, 1)).expect("post");
        q.post(T {
            class: TO_CLIENT,
            len: 1,
            value: 1,
            g: Guarantee::Guaranteed,
        })
        .expect("post");
        let mut w = big();
        let written = q
            .write_section(
                &mut w,
                ctx,
                0,
                SectionParams {
                    ghosting: false,
                    hold_ordered: true,
                    packet_bits: 8000,
                    min_start_bits: 0,
                },
            )
            .expect("ok");
        assert_eq!(
            written,
            Written {
                unordered: 1,
                ordered: 0
            }
        );
        assert_eq!(q.ordered_len(), 1);
    });
}

#[test]
fn an_event_larger_than_an_empty_packet_is_an_error_not_a_stall() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        q.post(t(5000, 1)).expect("post");
        let mut w = BitWriter::with_limit(1600);
        assert!(matches!(
            q.write_section(&mut w, ctx, 0, params(1500)),
            Err(NetError::Cap { .. })
        ));
        // Just under: it does not fit THIS packet (other things in it) but would fit an empty one: no error, it waits.
        let mut q = EventQueue::new();
        q.post(t(1200, 1)).expect("post");
        let mut w = BitWriter::with_limit(1600);
        w.write_bits(0, 20);
        assert!(q.write_section(&mut w, ctx, 0, params(1500)).is_ok());
        assert_eq!(q.ordered_len(), 0, "1,200 bits fit after 20 bits of head");
        let mut q = EventQueue::new();
        q.post(t(1400, 1)).expect("post");
        let mut w = BitWriter::with_limit(1600);
        (0..10).for_each(|_| w.write_bits(0, 30));
        assert_eq!(
            q.write_section(&mut w, ctx, 0, params(1500)),
            Ok(Written::default())
        );
        assert_eq!(q.ordered_len(), 1);
    });
}

fn send_n(q: &mut EventQueue<T>, ctx: &EventCtx<'_>, send_seq: u32, limit: usize) -> Written {
    let mut w = BitWriter::with_limit(limit);
    w.enable_string_buffer();
    q.write_section(&mut w, ctx, send_seq, params(limit))
        .expect("ok")
}

#[test]
fn lost_packets_put_ordered_events_back_in_sequence_order() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        for i in 0..6 {
            q.post(t(8, i)).expect("post");
        }
        // 40 + 2 bits per event with the first's seq: three events a packet.
        let a = send_n(&mut q, ctx, 10, 120);
        let b = send_n(&mut q, ctx, 11, 120);
        assert_eq!((a.ordered, b.ordered), (3, 3));
        assert_eq!(q.ordered_len(), 0);
        let mut settled = Vec::new();
        q.on_notify(11, false, &mut settled);
        q.on_notify(10, false, &mut settled);
        assert!(settled.is_empty());
        assert_eq!(q.ordered_len(), 6);
        // Sent again, they carry the same bits as the first time.
        let mut w = big();
        assert_eq!(
            q.write_section(&mut w, ctx, 12, params(8000))
                .expect("ok")
                .ordered,
            6
        );
        let bytes = w.into_bytes();
        let mut rx = EventReceiver::new();
        let got = read_all(&bytes, Role::Server, &mut rx).expect("reads");
        assert_eq!(
            got.iter().map(|i| i.event.value).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4, 5]
        );
        assert_eq!(
            got.iter().map(|i| i.seq).collect::<Vec<_>>(),
            (0..6).map(Some).collect::<Vec<_>>()
        );
    });
}

#[test]
fn acks_out_of_order_settle_in_sequence_when_the_run_is_whole() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        for i in 0..4 {
            q.post(t(8, i)).expect("post");
        }
        send_n(&mut q, ctx, 1, 100);
        send_n(&mut q, ctx, 2, 100);
        let mut settled = Vec::new();
        q.on_notify(2, true, &mut settled);
        assert!(settled.is_empty(), "seq 2,3 are acked but 0,1 are not");
        assert_eq!(q.last_acked(), u32::MAX);
        q.on_notify(1, true, &mut settled);
        assert_eq!(
            settled.iter().map(|s| s.event.value).collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        assert_eq!(q.last_acked(), 3);
        assert!(q.is_idle());
        // An ack for a packet that is not in flight does nothing.
        q.on_notify(77, true, &mut settled);
        q.on_notify(1, false, &mut settled);
        assert_eq!(settled.len(), 4);
    });
}

#[test]
fn guaranteed_unordered_events_go_to_the_front_and_unguaranteed_ones_are_settled_lost() {
    with_ctx(|ctx| {
        let ev = |g, value| T {
            class: TO_CLIENT,
            len: 4,
            value,
            g,
        };
        let mut q = EventQueue::new();
        q.post(ev(Guarantee::Guaranteed, 1)).expect("post");
        q.post(ev(Guarantee::Unguaranteed, 2)).expect("post");
        q.post(ev(Guarantee::Guaranteed, 3)).expect("post");
        send_n(&mut q, ctx, 5, 400);
        q.post(ev(Guarantee::Guaranteed, 4)).expect("post");
        let mut settled = Vec::new();
        q.on_notify(5, false, &mut settled);
        assert_eq!(settled.len(), 1);
        assert_eq!((settled[0].event.value, settled[0].delivered), (2, false));
        // Each requeued node is pushed on the front in chain order (as the loop does), ahead of the event posted meanwhile.
        assert_eq!(q.unordered_len(), 3);
        let mut w = big();
        q.write_section(&mut w, ctx, 6, params(8000)).expect("ok");
        let bytes = w.into_bytes();
        let got = read_all(&bytes, Role::Client, &mut EventReceiver::new()).expect("reads");
        assert_eq!(
            got.iter().map(|i| i.event.value).collect::<Vec<_>>(),
            vec![3, 1, 4]
        );
        // A delivered unordered event is settled at once.
        let mut settled = Vec::new();
        q.on_notify(6, true, &mut settled);
        assert_eq!(settled.len(), 3);
        assert!(settled.iter().all(|s| s.delivered));
    });
}

#[test]
fn the_queues_have_caps_and_hand_a_refused_event_back() {
    let mut q = EventQueue::new();
    for i in 0..NET_EVENT_QUEUE_ORDERED {
        q.post(t(1, i as u32)).expect("room");
    }
    assert_eq!(q.room_ordered(), 0);
    assert_eq!(q.post(t(1, 99)), Err(PostError::Full(t(1, 99))));
    assert_eq!(
        q.next_seq() as usize,
        NET_EVENT_QUEUE_ORDERED,
        "a refused event takes no sequence"
    );
    for i in 0..NET_EVENT_QUEUE_UNORDERED {
        let e = T {
            class: TO_CLIENT,
            len: 1,
            value: i as u32,
            g: Guarantee::Guaranteed,
        };
        q.post(e).expect("room");
    }
    let refused = T {
        class: TO_CLIENT,
        len: 1,
        value: 7,
        g: Guarantee::Unguaranteed,
    };
    assert_eq!(q.post(refused.clone()), Err(PostError::Full(refused)));
}

#[test]
fn a_batch_is_posted_whole_or_handed_back_whole() {
    let mut q = EventQueue::new();
    for i in 0..NET_EVENT_QUEUE_ORDERED - 2 {
        q.post(t(1, i as u32)).expect("room");
    }
    let batch = vec![t(1, 1), t(1, 2), t(1, 3)];
    assert_eq!(q.post_batch(batch.clone()), Err(batch));
    assert_eq!(q.room_ordered(), 2);
    assert_eq!(q.post_batch(vec![t(1, 1), t(1, 2)]), Ok(()));
    assert_eq!(q.room_ordered(), 0);
    q.close();
    assert_eq!(q.post(t(1, 1)), Err(PostError::Closed(t(1, 1))));
    assert_eq!(q.post_batch(vec![t(1, 1)]), Err(vec![t(1, 1)]));
}

#[test]
fn release_all_returns_everything_the_queue_holds_and_closes_it() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        for i in 0..5 {
            q.post(t(8, i)).expect("post");
        }
        send_n(&mut q, ctx, 1, 100);
        send_n(&mut q, ctx, 2, 100);
        let mut settled = Vec::new();
        q.on_notify(2, true, &mut settled);
        let all = q.release_all();
        assert_eq!(all.len(), 5);
        assert!(all.iter().all(|s| s.delivered));
        assert!(q.is_closed() && q.is_idle());
    });
}

// ---- the receiver

/// Builds a section by hand: the unordered events, then the ordered ones with explicit seqs (or the consecutive flag).
fn section(unordered: &[T], ordered: &[(Option<u32>, T)]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    for e in unordered {
        w.write_flag(true);
        w.write_bits(e.class, 5);
        with_ctx(|ctx| e.pack(&mut w, ctx));
    }
    w.write_flag(false);
    for (seq, e) in ordered {
        w.write_flag(true);
        match seq {
            Some(s) => {
                w.write_flag(false);
                w.write_bits(*s, 7);
            }
            None => w.write_flag(true),
        }
        w.write_bits(e.class, 5);
        with_ctx(|ctx| e.pack(&mut w, ctx));
    }
    w.write_flag(false);
    w.into_bytes()
}

fn values(out: &[Incoming<T>]) -> Vec<u32> {
    out.iter().map(|i| i.event.value).collect()
}

fn feed(rx: &mut EventReceiver<T>, seq7: u32, value: u32) -> Vec<Incoming<T>> {
    read_all(
        &section(&[], &[(Some(seq7), t(32, value))]),
        Role::Server,
        rx,
    )
    .expect("reads")
}

#[test]
fn unordered_events_run_at_once_and_ordered_ones_after_the_section_in_sequence() {
    let mut rx = EventReceiver::new();
    let bytes = section(
        &[t(32, 100), t(32, 101)],
        &[
            (Some(2), t(32, 12)),
            (Some(0), t(32, 10)),
            (Some(1), t(32, 11)),
        ],
    );
    let out = read_all(&bytes, Role::Server, &mut rx).expect("reads");
    assert_eq!(values(&out), vec![100, 101, 10, 11, 12]);
    assert_eq!(
        out.iter().map(|i| i.seq).collect::<Vec<_>>(),
        vec![None, None, Some(0), Some(1), Some(2)]
    );
    assert_eq!((rx.next_recv(), rx.waiting()), (3, 0));
}

#[test]
fn an_event_waits_for_the_missing_one_across_packets() {
    let mut rx = EventReceiver::new();
    assert!(feed(&mut rx, 1, 11).is_empty());
    assert!(feed(&mut rx, 3, 13).is_empty());
    assert_eq!(rx.waiting(), 2);
    assert_eq!(values(&feed(&mut rx, 0, 10)), vec![10, 11]);
    assert_eq!(values(&feed(&mut rx, 2, 12)), vec![12, 13]);
    assert_eq!((rx.next_recv(), rx.waiting()), (4, 0));
}

#[test]
fn the_sequence_is_widened_against_next_recv() {
    let mut rx = EventReceiver::new();
    for s in 0..=0x84u32 {
        assert_eq!(values(&feed(&mut rx, s & 0x7f, s)), vec![s]);
    }
    assert_eq!(rx.next_recv(), 0x85);
    // 7 bits equal to nextRecv's own low bits: runs at once.
    assert_eq!(values(&feed(&mut rx, 0x05, 0x85)), vec![0x85]);
    // 0x7f widens to 0x80 | 0x7f = 0xff, 122 ahead of 0x86: it waits.
    assert!(feed(&mut rx, 0x7f, 0xff).is_empty());
    assert_eq!(rx.waiting(), 1);
    // Below the low bits of nextRecv (0x86 -> 6): 0x02 widens to 0x80 | 2 = 0x82 < 0x86, so +128 = 0x102, 0x7c ahead.
    assert!(feed(&mut rx, 0x02, 0x102).is_empty());
    assert_eq!(rx.waiting(), 2);
}

#[test]
fn a_duplicate_in_the_wait_list_is_dropped_once_in_a_packet_and_once_across_packets() {
    let mut rx = EventReceiver::new();
    // Both copies in one packet.
    let bytes = section(&[], &[(Some(5), t(4, 1)), (Some(5), t(4, 2))]);
    let mut out = Vec::new();
    let stats = with_ctx(|ctx| {
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        rx.read_section(&mut r, ctx, Role::Server, &mut TU, &mut out, None)
            .expect("reads")
    });
    assert_eq!(stats.duplicates, 1);
    assert_eq!(rx.waiting(), 1);
    // The copy in a later packet.
    assert!(feed(&mut rx, 5, 3).is_empty());
    assert_eq!(rx.waiting(), 1);
    // The gap fills and sequence 5 runs once, with the first copy.
    for s in 0..5 {
        feed(&mut rx, s, 0);
    }
    assert_eq!(rx.next_recv(), 6);
    assert_eq!(rx.waiting(), 0);
}

#[test]
fn a_duplicate_of_a_processed_event_does_not_wedge_the_channel() {
    let mut rx = EventReceiver::new();
    for s in 0..4u32 {
        feed(&mut rx, s, s);
    }
    assert_eq!(rx.next_recv(), 4);
    // The retransmit of seq 3 widens to 3 + 128 = 131, 127 ahead: dropped, not parked.
    assert!(feed(&mut rx, 3, 3).is_empty());
    assert_eq!(rx.waiting(), 0);
    // The channel goes on, through the 7-bit wrap and well past it.
    for s in 4..400u32 {
        assert_eq!(values(&feed(&mut rx, s & 0x7f, s)), vec![s], "seq {s}");
    }
    assert_eq!(rx.next_recv(), 400);
}

#[test]
fn a_stale_copy_126_ahead_is_the_one_the_seven_bit_wire_cannot_tell_from_a_real_event() {
    let mut rx = EventReceiver::new();
    for s in 0..10u32 {
        feed(&mut rx, s, s);
    }
    // seq 8 again: 8 + 128 = 136 = nextRecv + 126, a legitimate-looking value, so it waits (as it should) ...
    assert!(feed(&mut rx, 8, 8).is_empty());
    assert_eq!(rx.waiting(), 1);
    // ... a second copy of it is a duplicate and is dropped ...
    assert!(feed(&mut rx, 8, 8).is_empty());
    assert_eq!(rx.waiting(), 1);
    // ... and it cannot wedge anything: the real events keep flowing.
    for s in 10..500u32 {
        feed(&mut rx, s & 0x7f, s);
    }
    assert_eq!(rx.next_recv(), 500);
    assert_eq!(rx.waiting(), 0);
}

#[test]
fn an_event_more_than_126_ahead_is_dropped_silently() {
    let mut rx = EventReceiver::new();
    // From nextRecv 0: seq7 127 is 127 ahead (dropped); 126 is 126 ahead (waits).
    assert!(feed(&mut rx, 127, 1).is_empty());
    assert_eq!(rx.waiting(), 0);
    assert!(feed(&mut rx, 126, 2).is_empty());
    assert_eq!(rx.waiting(), 1);
    // With the flag set first: "prev + 1" with prev -2 reads as 0x7f, which is 127 ahead.
    let bytes = section(&[], &[(None, t(4, 5))]);
    let mut out = Vec::new();
    let stats = with_ctx(|ctx| {
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        rx.read_section(&mut r, ctx, Role::Server, &mut TU, &mut out, None)
            .expect("reads")
    });
    assert_eq!((stats.too_far, rx.waiting()), (1, 1));
}

#[test]
fn a_wait_list_flood_stays_bounded_and_is_never_an_error() {
    let mut rx = EventReceiver::new();
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
    let mut next = || rng.next_u64();
    let mut total = 0usize;
    for _ in 0..3000 {
        let n = (next() % 120) as usize + 1;
        let events: Vec<(Option<u32>, T)> = (0..n)
            .map(|_| (Some((next() % 128) as u32), t(1, 1)))
            .collect();
        let out = read_all(&section(&[], &events), Role::Server, &mut rx).expect("never an error");
        total += out.len();
        assert!(rx.waiting() <= 127, "waiting {}", rx.waiting());
    }
    assert_eq!(
        total as u32,
        rx.next_recv(),
        "every run event advanced nextRecv by one"
    );
}

#[test]
fn a_class_id_at_or_above_the_count_is_a_protocol_error() {
    for class in 17..32u32 {
        let mut w = BitWriter::new();
        w.enable_string_buffer();
        w.write_flag(true);
        w.write_bits(class, 5);
        w.write_bits(0, 40);
        let bytes = w.into_bytes();
        assert_eq!(
            read_all(&bytes, Role::Server, &mut EventReceiver::new()),
            Err(NetError::Invalid("event class id")),
            "class {class}"
        );
    }
    // 16 is the last class and fine.
    let mut e = T {
        class: 16,
        ..t(1, 1)
    };
    e.class = 16;
    let out = read_all(&section(&[e], &[]), Role::Client, &mut EventReceiver::new());
    assert!(out.is_ok());
}

#[test]
fn the_direction_of_the_class_must_suit_the_reader() {
    // A server-to-client class read by a server is wrong; a client-to-server class read by a client is wrong.
    let to_client = T {
        class: TO_CLIENT,
        ..t(1, 1)
    };
    let to_server = T {
        class: TO_SERVER,
        ..t(1, 1)
    };
    let any = t(1, 1);
    for (event, reader, ok) in [
        (&to_client, Role::Server, false),
        (&to_client, Role::Client, true),
        (&to_server, Role::Client, false),
        (&to_server, Role::Server, true),
        (&any, Role::Client, true),
        (&any, Role::Server, true),
    ] {
        let got = read_all(
            &section(std::slice::from_ref(event), &[]),
            reader,
            &mut EventReceiver::new(),
        );
        assert_eq!(got.is_ok(), ok, "class {} read by {reader:?}", event.class);
        if !ok {
            assert_eq!(got, Err(NetError::Invalid("event direction")));
        }
        let ordered = read_all(
            &section(&[], &[(Some(0), event.clone())]),
            reader,
            &mut EventReceiver::new(),
        );
        assert_eq!(ordered.is_ok(), ok);
    }
}

#[test]
fn a_connection_message_uses_the_ghost_width_plus_one() {
    for (ghost_bits, count) in [
        (18u8, 0x7ffffu32),
        (15, 0xffff),
        (20, 1),
        (31, u32::MAX >> 1),
        (255, 77),
    ] {
        let table = crate::testids::table();
        let ids = ids();
        let ctx = EventCtx {
            table: &table,
            ids: &ids,
            ghost_bits,
            datablock_bits: crate::profile::DEFAULT_DATABLOCK_BITS,
        };
        let m = ConnectionMessage {
            seq: 0xdead_beef,
            message: 5,
            count,
        };
        let mut w = BitWriter::new();
        m.pack(&mut w, &ctx);
        let width = u32::from(ghost_bits).saturating_add(1).min(32);
        assert_eq!(w.bits() as u32, 32 + 3 + width);
        let bytes = w.into_bytes();
        assert_eq!(
            ConnectionMessage::unpack(&mut BitReader::new(&bytes), &ctx),
            Ok(m)
        );
    }
    // Message is 3 bits.
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        ConnectionMessage {
            seq: 0,
            message: 0xff,
            count: 0,
        }
        .pack(&mut w, ctx);
        let bytes = w.into_bytes();
        assert_eq!(
            ConnectionMessage::unpack(&mut BitReader::new(&bytes), ctx)
                .expect("reads")
                .message,
            7
        );
    });
}

fn every_kind() -> (Vec<NetEvent>, Vec<NetEvent>) {
    let unordered = vec![NetEvent::NetString(NetStringEvent {
        slot: 3,
        text: b"unordered text".to_vec(),
        tag: 0,
    })];
    let ordered = vec![
        NetEvent::NetString(NetStringEvent {
            slot: 31,
            text: b"ClientJoin".to_vec(),
            tag: 0,
        }),
        NetEvent::Remote(RemoteCommandEvent {
            variant: 0,
            args: vec![
                RemoteArg::Tag(1),
                RemoteArg::Text(b"text arg".to_vec()),
                RemoteArg::Int(-300),
                RemoteArg::Empty,
            ],
        }),
        NetEvent::Remote(RemoteCommandEvent {
            variant: 0,
            args: vec![RemoteArg::Tag(2), RemoteArg::Int(70_000)],
        }),
        NetEvent::ConnectionMessage(ConnectionMessage {
            seq: 9,
            message: 4,
            count: 1234,
        }),
    ];
    (unordered, ordered)
}

#[test]
fn every_event_kind_round_trips_through_a_section_and_every_truncation_is_an_error() {
    with_ctx(|ctx| {
        let (unordered, ordered) = every_kind();
        let mut w = big();
        let mut prev = PREV_START;
        for e in &unordered {
            put_unordered(&mut w, ctx, e);
        }
        w.write_flag(false);
        for (i, e) in ordered.iter().enumerate() {
            put_ordered(&mut w, ctx, &mut prev, i as u32, e);
        }
        w.write_flag(false);
        let bits = w.bits();
        let bytes = w.into_bytes();
        let mut rx = EventReceiver::new();
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let mut out = Vec::new();
        let mut log = Vec::new();
        rx.read_section(
            &mut r,
            ctx,
            Role::Server,
            &mut NetEventUnpacker,
            &mut out,
            Some(&mut log),
        )
        .expect("reads");
        assert_eq!(r.position(), bits);
        let got: Vec<NetEvent> = out.into_iter().map(|i| i.event).collect();
        let want: Vec<NetEvent> = unordered.iter().chain(&ordered).cloned().collect();
        assert_eq!(got, want);
        // The wire log re-encodes to the same bits.
        let mut again = BitWriter::new();
        again.enable_string_buffer();
        write_wire_items(&mut again, ctx, &log);
        assert_eq!((again.bits(), again.bytes()), (bits, bytes.as_slice()));
        for cut in 0..bits {
            let mut r = BitReader::with_bit_len(&bytes, cut);
            r.enable_string_buffer();
            let res = EventReceiver::new().read_section(
                &mut r,
                ctx,
                Role::Server,
                &mut NetEventUnpacker,
                &mut Vec::new(),
                None,
            );
            assert!(res.is_err(), "cut {cut} of {bits}");
        }
    });
}

#[test]
fn a_class_without_a_decoder_is_refused_not_guessed() {
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        w.write_flag(true);
        w.write_bits(ctx.ids.sim_data_block, 5);
        w.write_bits(0, 64);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        let res = EventReceiver::new().read_section(
            &mut r,
            ctx,
            Role::Client,
            &mut NetEventUnpacker,
            &mut Vec::new(),
            None,
        );
        assert_eq!(res, Err(NetError::Invalid("event class has no decoder")));
    });
}

#[test]
fn a_delivered_net_string_sets_the_senders_flag() {
    let mut strings = SendStrings::new();
    let checked = strings.check_string(5, b"Five");
    let event = NetEvent::NetString(checked.event.expect("event"));
    event.on_settled(false, &mut strings);
    assert!(!strings.check_string(5, b"Five").acked);
    event.on_settled(true, &mut strings);
    assert!(strings.check_string(5, b"Five").acked);
    NetEvent::ConnectionMessage(ConnectionMessage {
        seq: 0,
        message: 0,
        count: 0,
    })
    .on_settled(true, &mut strings);
}

#[test]
fn hostile_sections_never_panic() {
    with_ctx(|ctx| {
        let mut rng = Rng::new(0x2545_f491_4f6c_dd1d);
        let mut next = || rng.next_u64();
        for round in 0..20_000 {
            let len = (next() % 160) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Make half of them start like a real section.
            if round % 2 == 0 && !bytes.is_empty() {
                bytes[0] = (bytes[0] & 0xf8) | 0b101;
            }
            let reader = if round % 3 == 0 {
                Role::Server
            } else {
                Role::Client
            };
            let mut rx = EventReceiver::new();
            let mut r = BitReader::new(&bytes);
            r.enable_string_buffer();
            let mut out = Vec::new();
            let _ = rx.read_section(&mut r, ctx, reader, &mut NetEventUnpacker, &mut out, None);
            assert!(rx.waiting() <= 127);
        }
    });
}

// ---- the measure at the margin (hand-built cases)

/// Writes a section into a packet that already holds `start` bits: the writer's hard limit is 12,000 bits (the stream's), the rate's
/// size is `packet_bits`. Returns the result and the bits the packet ends up with.
fn margin<E: WireEvent>(
    q: &mut EventQueue<E>,
    start: usize,
    packet_bits: usize,
    ghosting: bool,
) -> (Result<Written, NetError>, usize) {
    with_ctx(|ctx| {
        let mut w = BitWriter::with_limit(12_000);
        w.enable_string_buffer();
        for _ in 0..start {
            w.write_flag(false);
        }
        let r = q.write_section(
            &mut w,
            ctx,
            0,
            SectionParams {
                ghosting,
                hold_ordered: false,
                packet_bits,
                min_start_bits: 0,
            },
        );
        (r, w.bits())
    })
}

fn unordered(len: u16) -> T {
    T {
        class: TO_CLIENT,
        len,
        value: 1,
        g: Guarantee::Guaranteed,
    }
}

#[test]
fn the_writers_hard_limit_is_never_passed_whatever_the_rate() {
    let mut q = EventQueue::new();
    for _ in 0..100 {
        q.post(t(1000, 1)).expect("post");
    }
    for _ in 0..5 {
        q.post(unordered(1000)).expect("post");
    }
    let (r, bits) = margin(&mut q, 0, 40_000, false);
    let w = r.expect("no error");
    assert!(w.ordered > 0 && w.unordered > 0);
    assert!(bits <= 12_000, "{bits}");
    // The same with the packet already near the limit: the closing flags still fit.
    let (r, bits) = margin(&mut q, 11_990, 40_000, false);
    assert_eq!(r.expect("no error"), Written::default());
    assert_eq!(bits, 11_992);
}

#[test]
fn the_first_ordered_event_with_the_consecutive_flag_reads_as_seq_0x7f_whatever_next_recv() {
    let mut rx = EventReceiver::new();
    for s in 0..130u32 {
        feed(&mut rx, s & 0x7f, s);
    }
    assert_eq!(rx.next_recv(), 130);
    let run = |rx: &mut EventReceiver<T>, ordered: &[(Option<u32>, T)]| {
        let bytes = section(&[], ordered);
        let mut out = Vec::new();
        with_ctx(|ctx| {
            let mut r = BitReader::new(&bytes);
            r.enable_string_buffer();
            rx.read_section(&mut r, ctx, Role::Server, &mut TU, &mut out, None)
                .expect("reads")
        })
    };
    // prev is -2, so the flag means (-2 + 1) & 0x7f = 0x7f: widened to 128 | 0x7f = 255, 125 ahead of 130: it waits.
    let stats = run(&mut rx, &[(None, t(32, 1))]);
    assert_eq!(stats.too_far, 0);
    assert_eq!(rx.waiting(), 1);
    // After an explicit 0x7f (a duplicate of the waiting 255) the next consecutive flag wraps to 0: widened to 128, below nextRecv, so
    // 256, 126 ahead: it waits.
    let stats = run(&mut rx, &[(Some(0x7f), t(32, 2)), (None, t(32, 3))]);
    assert_eq!((stats.too_far, stats.duplicates), (0, 1));
    assert_eq!(rx.waiting(), 2);
}

#[test]
fn the_charge_callback_pays_per_event_read_and_stops_the_read_when_it_cannot() {
    let bytes = section(
        &[t(32, 1), t(32, 2)],
        &[(Some(0), t(32, 3)), (Some(1), t(32, 4))],
    );
    let mut paid = Vec::new();
    let mut rx = EventReceiver::new();
    let out = with_ctx(|ctx| {
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let mut out = Vec::new();
        let res = rx.read_section_charged(
            &mut r,
            ctx,
            Role::Server,
            &mut TU,
            &mut out,
            None,
            &mut |units| {
                paid.push(units);
                true
            },
        );
        assert!(res.is_ok());
        out
    });
    assert_eq!(out.len(), 4);
    assert_eq!(paid.len(), 4);
    assert!(
        paid.iter().all(|&u| u >= 4),
        "each event is at least its 32-bit body: {paid:?}"
    );
    // A budget that runs out after two events stops the read at the third.
    let mut left = 2;
    let mut rx = EventReceiver::new();
    let mut out = Vec::new();
    let res = with_ctx(|ctx| {
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        rx.read_section_charged(
            &mut r,
            ctx,
            Role::Server,
            &mut TU,
            &mut out,
            None,
            &mut |_| {
                left -= 1;
                left >= 0
            },
        )
    });
    assert_eq!(res, Err(NetError::Invalid("work budget")));
    assert_eq!(
        out.len(),
        2,
        "the two unordered events ran, the first ordered one was refused"
    );
}

#[test]
fn the_measure_of_an_event_with_several_strings_has_no_prefix_sharing() {
    let e = NetEvent::Remote(RemoteCommandEvent {
        variant: 0,
        args: vec![
            RemoteArg::Text(b"SharedPrefixOfSomeLength-one".to_vec()),
            RemoteArg::Text(b"SharedPrefixOfSomeLength-two".to_vec()),
        ],
    });
    with_ctx(|ctx| {
        let mut plain = BitWriter::new();
        e.pack(&mut plain, ctx);
        let mut shared = BitWriter::new();
        shared.enable_string_buffer();
        e.pack(&mut shared, ctx);
        assert!(
            shared.bits() + 8 < plain.bits(),
            "the second string shares a prefix in a packet"
        );
        assert_eq!(
            measure(&e, ctx),
            plain.bits(),
            "the scratch stream of the measure has no string buffer"
        );
    });
}

#[test]
fn a_stalling_section_keeps_an_unsendable_event_at_the_head_and_sends_those_before_it() {
    with_ctx(|ctx| {
        let mut q = EventQueue::new();
        q.post(t(100, 1)).expect("post");
        q.post(t(5000, 2)).expect("post");
        q.post(t(100, 3)).expect("post");
        let mut w = BitWriter::with_limit(1600);
        let written = q
            .write_section_stalling(&mut w, ctx, 0, params(1500))
            .expect("a stall, not an error");
        assert_eq!(
            written,
            Written {
                unordered: 0,
                ordered: 1
            }
        );
        assert_eq!(
            q.ordered_len(),
            2,
            "the big event and the one behind it wait"
        );
        // Nothing is dropped: with room for it, it goes, then the one behind it.
        let mut w = BitWriter::with_limit(12_000);
        let written = q
            .write_section_stalling(&mut w, ctx, 1, params(11_000))
            .expect("writes");
        assert_eq!(written.ordered, 2);
        // The same queue without the mode is an error.
        let mut q = EventQueue::new();
        q.post(t(5000, 1)).expect("post");
        let mut w = BitWriter::with_limit(1600);
        assert!(q.write_section(&mut w, ctx, 0, params(1500)).is_err());
    });
}

#[test]
fn a_game_fills_the_registry_beside_torques_events() {
    let torque = EventRegistry::torque();
    assert_eq!(torque.classes().len(), TORQUE_EVENTS.len());
    assert_eq!(EventRegistry::default(), torque);
    let plain = torque.ids().unwrap();
    assert_eq!((plain.count(), plain.bits()), (13, 4));
    // A game registers its own classes; the ids follow the sort rule over the whole set.
    let mut game = EventRegistry::torque();
    game.register("AGameEvent", Direction::ServerToClient)
        .register("ZGameEvent", Direction::ClientToServer);
    let ids = game.ids().unwrap();
    assert_eq!(ids.count(), 15);
    assert_eq!(ids.id_of("AGameEvent"), Some(0));
    assert_eq!(ids.connection_message, 1);
    assert_eq!(ids.id_of("ZGameEvent"), Some(14));
    assert_eq!(ids.direction(14), Some(Direction::ClientToServer));
    // A name registered again keeps one entry with the direction given last.
    game.register("AGameEvent", Direction::Any);
    assert_eq!(game.classes().len(), 15);
    assert_eq!(game.ids().unwrap().direction(0), Some(Direction::Any));
    // A Torque event the game does not have can go; one this crate codes cannot.
    assert!(game.remove("FileChunkEvent"));
    assert!(!game.remove("FileChunkEvent"));
    assert_eq!(game.ids().unwrap().count(), 14);
    game.remove("NetStringEvent");
    assert!(game.ids().is_err());
    // An empty name is refused.
    let mut bad = EventRegistry::torque();
    bad.register("", Direction::Any);
    assert!(bad.ids().is_err());
    assert!(EventRegistry::empty().ids().is_err());
}

#[test]
fn an_event_past_the_hard_limit_is_an_error() {
    let mut q = EventQueue::new();
    q.post(t(11_990, 1)).expect("post");
    let (r, _) = margin(&mut q, 0, 40_000, false);
    assert!(
        matches!(r, Err(NetError::Cap { .. })),
        "over the hard limit"
    );
}

#[test]
fn torques_rule_writes_events_while_the_stream_is_not_full() {
    assert!(std::ptr::eq(EventQueue::<T>::new().rule, &TORQUE_SECTION));
    // The unordered phase closes at bit 2: not past a size of 2, so the ordered event goes, whatever it takes.
    let mut q = EventQueue::new();
    q.post(t(10, 0x155)).expect("post");
    let (r, bits) = margin(&mut q, 1, 2, false);
    assert_eq!(r.expect("no error").ordered, 1);
    assert_eq!(bits, 1 + 1 + 40 + 1);
    // One bit more and the stream is full.
    let mut q = EventQueue::new();
    q.post(t(10, 0x155)).expect("post");
    let (r, bits) = margin(&mut q, 2, 2, false);
    assert_eq!(r.expect("no error").ordered, 0);
    assert_eq!((bits, q.ordered_len()), (4, 1));
    // An unordered event at the size itself goes; ghosting changes nothing.
    for ghosting in [false, true] {
        let mut q = EventQueue::new();
        q.post(unordered(100)).expect("post");
        let (r, _) = margin(&mut q, 8, 8, ghosting);
        assert_eq!(r.expect("no error").unordered, 1);
    }
}

/// A rule that takes an event only when its measured body ends within the size, and stops the unordered phase at half the size
/// while ghosting.
static TEST_RULE: SectionRule = SectionRule {
    measures: true,
    unordered_fits: |at, size, body| at + body <= size,
    ordered_fits: |at, size, body| at + body <= size,
    ghosting_stop: |at, size| at * 2 > size,
};

#[test]
fn a_section_rule_tests_each_head_event_with_its_measure() {
    // t(10)'s body is 26 bits; at bit 2 (after the unordered phase's closing flag) it fits a size of 28, not 27.
    for (size, sent) in [(28usize, true), (27, false)] {
        let mut q = EventQueue::new().with_section_rule(&TEST_RULE);
        q.post(t(10, 0x155)).expect("post");
        let (r, _) = margin(&mut q, 1, size, false);
        assert_eq!(
            r.expect("no error").ordered,
            usize::from(sent),
            "size {size}"
        );
        assert_eq!(q.ordered_len(), usize::from(!sent));
    }
    // The ghosting stop applies to the unordered phase only while ghosting.
    for (start, ghosting, sent) in [
        (500usize, true, true),
        (501, true, false),
        (501, false, true),
    ] {
        let mut q = EventQueue::new().with_section_rule(&TEST_RULE);
        q.post(unordered(1)).expect("post");
        let (r, _) = margin(&mut q, start, 1_000, ghosting);
        assert_eq!(
            r.expect("no error").unordered,
            usize::from(sent),
            "start {start}"
        );
    }
    // An event the rule refuses even in an empty packet is an error, not a stall.
    let mut q = EventQueue::new().with_section_rule(&TEST_RULE);
    q.post(t(5000, 1)).expect("post");
    let (r, _) = margin(&mut q, 0, 1500, false);
    assert!(matches!(r, Err(NetError::Cap { .. })));
}
