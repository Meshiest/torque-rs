//! The event layer end to end and under hostile input. A sender queue and a receiver over a link that loses packets
//! and delays their acks (every ordered event must arrive exactly once, in order); a bounded, seeded fuzz of the section reader
//! (structure-aware mutants of valid sections and random bytes), of the tag expansion and of the sender's string table (NetString
//! churn). The counts are bounded for `cargo test`; longer runs belong in a dedicated fuzz job.

use std::collections::VecDeque;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::events::{
    EventCtx, EventIds, EventQueue, EventReceiver, EventRegistry, Incoming, NetEvent,
    NetEventUnpacker, SectionParams, Settled,
};
use torque_net::huffman::Table;
use torque_net::netstring::{NetStringEvent, RemoteStrings, SendStrings};
use torque_net::profile::DEFAULT_DATABLOCK_BITS;
use torque_net::protocol::Role;
use torque_net::remote::{
    RemoteArg, RemoteCommandEvent, TagLookup, atoi, expand, resolve, send_command,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        ((self.next() >> 32) as usize) % n.max(1)
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// Torque's event classes (the registry's default).
fn ids() -> EventIds {
    EventRegistry::torque().ids().expect("ids")
}

fn table() -> Table {
    let mut f = [0u32; 256];
    for (i, slot) in f.iter_mut().enumerate().take(127) {
        *slot = match i as u8 {
            b'a'..=b'z' => 300 + (i as u32 * 37) % 900,
            b' ' => 2000,
            b'0'..=b'9' => 90,
            _ => 1 + (i as u32 * 3) % 7,
        };
    }
    Table::from_frequencies(&f).expect("a table")
}

fn random_event(rng: &mut Rng) -> NetEvent {
    match rng.below(4) {
        0 => NetEvent::NetString(NetStringEvent {
            slot: rng.below(32) as u8,
            text: (0..rng.below(40))
                .map(|_| b'a' + rng.below(26) as u8)
                .collect(),
            tag: 0,
        }),
        1 => NetEvent::ConnectionMessage(torque_net::events::ConnectionMessage {
            seq: rng.next() as u32,
            message: rng.below(8) as u8,
            count: rng.below(1 << 19) as u32,
        }),
        _ => NetEvent::Remote(RemoteCommandEvent {
            variant: 0,
            args: (0..rng.below(8))
                .map(|_| match rng.below(4) {
                    0 => RemoteArg::Empty,
                    1 => RemoteArg::Text(
                        (0..rng.below(30))
                            .map(|_| b'a' + rng.below(26) as u8)
                            .collect(),
                    ),
                    2 => RemoteArg::Tag(rng.below(32) as u8),
                    _ => RemoteArg::Int(rng.next() as i32 >> rng.below(31)),
                })
                .collect(),
        }),
    }
}

/// A sender and a receiver over a link: packets are lost with `loss` percent, acks come `delay` rounds late and always in order.
fn run_link(seed: u64, loss: usize, delay: usize, events: usize) {
    let (table, ids) = (table(), ids());
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 18,
        datablock_bits: DEFAULT_DATABLOCK_BITS,
    };
    let mut rng = Rng(seed);
    let mut queue: EventQueue<NetEvent> = EventQueue::new();
    let mut rx: EventReceiver<NetEvent> = EventReceiver::new();
    let mut posted: Vec<NetEvent> = Vec::new();
    let mut ran: Vec<NetEvent> = Vec::new();
    let mut fates: VecDeque<(usize, u32, bool)> = VecDeque::new();
    let mut send_seq = 0u32;
    let mut round = 0usize;
    let mut max_waiting = 0;
    while (posted.len() < events || !queue.is_idle()) && round < 200_000 {
        round += 1;
        for _ in 0..rng.below(6) {
            if posted.len() < events {
                let e = random_event(&mut rng);
                if queue.post(e.clone()).is_ok() {
                    posted.push(e);
                }
            }
        }
        if queue.packets_in_flight() < 20 {
            let limit = (100 + rng.below(900)) * 8;
            let mut w = BitWriter::with_limit(limit);
            w.enable_string_buffer();
            let written = queue
                .write_section(
                    &mut w,
                    &ctx,
                    send_seq,
                    SectionParams {
                        ghosting: false,
                        hold_ordered: false,
                        packet_bits: limit,
                        min_start_bits: 0,
                    },
                )
                .expect("no event is larger than the smallest packet");
            if written.ordered + written.unordered > 0 {
                let lost = rng.chance(loss);
                if !lost {
                    let bytes = w.into_bytes();
                    let mut r = BitReader::new(&bytes);
                    r.enable_string_buffer();
                    let mut out: Vec<Incoming<NetEvent>> = Vec::new();
                    rx.read_section(
                        &mut r,
                        &ctx,
                        Role::Server,
                        &mut NetEventUnpacker,
                        &mut out,
                        None,
                    )
                    .expect("a valid section reads");
                    ran.extend(out.into_iter().map(|i| i.event));
                    max_waiting = max_waiting.max(rx.waiting());
                }
                fates.push_back((round + delay, send_seq, !lost));
            }
            send_seq += 1;
        }
        // Acks arrive in order, `delay` rounds after the send.
        while fates.front().is_some_and(|f| f.0 <= round) {
            let (_, seq, delivered) = fates.pop_front().expect("front");
            let mut settled: Vec<Settled<NetEvent>> = Vec::new();
            queue.on_notify(seq, delivered, &mut settled);
        }
    }
    assert!(
        queue.is_idle(),
        "seed {seed}: the queue never drained after {round} rounds"
    );
    assert_eq!(posted.len(), events);
    assert_eq!(
        ran.len(),
        posted.len(),
        "seed {seed}, loss {loss}: events run"
    );
    assert_eq!(
        ran, posted,
        "seed {seed}, loss {loss}: exactly once and in order"
    );
    assert!(max_waiting <= 127);
    assert_eq!(queue.last_acked() as usize, events - 1);
}

#[test]
fn ordered_events_cross_a_lossy_link_exactly_once_and_in_order() {
    for (seed, loss, delay) in [
        (1u64, 0usize, 0usize),
        (2, 10, 1),
        (3, 30, 2),
        (4, 50, 3),
        (5, 70, 1),
        (6, 30, 8),
    ] {
        run_link(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15), loss, delay, 3000);
    }
}

fn section_bytes(rng: &mut Rng, ctx: &EventCtx<'_>) -> Vec<u8> {
    let mut queue: EventQueue<NetEvent> = EventQueue::new();
    for _ in 0..rng.below(12) {
        let _ = queue.post(random_event(rng));
    }
    let mut w = BitWriter::with_limit(1023 * 8);
    w.enable_string_buffer();
    queue
        .write_section(
            &mut w,
            ctx,
            0,
            SectionParams {
                ghosting: false,
                hold_ordered: false,
                packet_bits: 1000 * 8,
                min_start_bits: 0,
            },
        )
        .expect("fits");
    w.into_bytes()
}

#[test]
fn mutated_sections_and_random_bytes_never_panic_and_never_overgrow() {
    let (table, ids) = (table(), ids());
    let ctx = EventCtx {
        table: &table,
        ids: &ids,
        ghost_bits: 18,
        datablock_bits: DEFAULT_DATABLOCK_BITS,
    };
    let mut rng = Rng(0x5eed_5eed_1234_5678);
    let (mut ok, mut err) = (0usize, 0usize);
    for round in 0..30_000 {
        let mut bytes = section_bytes(&mut rng, &ctx);
        match round % 5 {
            0 => {
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(bytes.len().max(1));
                    if let Some(b) = bytes.get_mut(at) {
                        *b ^= 1 << rng.below(8);
                    }
                }
            }
            1 => bytes.truncate(rng.below(bytes.len() + 1)),
            2 => bytes = (0..rng.below(200)).map(|_| rng.next() as u8).collect(),
            3 => {
                let cut = rng.below(bytes.len() + 1);
                bytes.splice(cut..cut, (0..rng.below(20)).map(|_| rng.next() as u8));
            }
            _ => {}
        }
        for reader in [Role::Server, Role::Client] {
            let mut rx: EventReceiver<NetEvent> = EventReceiver::new();
            let mut r = BitReader::new(&bytes);
            r.enable_string_buffer();
            let mut out = Vec::new();
            match rx.read_section(&mut r, &ctx, reader, &mut NetEventUnpacker, &mut out, None) {
                Ok(_) => ok += 1,
                Err(NetError::Overrun | NetError::Cap { .. } | NetError::Invalid(_)) => err += 1,
            }
            assert!(rx.waiting() <= 127);
            assert!(out.len() <= bytes.len() * 8);
        }
    }
    assert!(
        ok > 1000 && err > 1000,
        "the mix reaches both outcomes: {ok} ok, {err} err"
    );
}

struct Tags(Vec<Vec<u8>>);

impl TagLookup for Tags {
    fn tag_text(&self, id: u32) -> Option<Vec<u8>> {
        usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| self.0.get(i).cloned())
    }

    fn table_size(&self) -> u32 {
        self.0.len() as u32
    }
}

#[test]
fn a_hostile_command_expands_within_its_buffer_and_never_panics() {
    let mut rng = Rng(0xfeed_f00d_dead_beef);
    let alphabet = b"%123456789 ab\x01-0\xff";
    for _ in 0..30_000 {
        let mut strings = RemoteStrings::new();
        for slot in 0..32u8 {
            if rng.chance(70) {
                let text: Vec<u8> = (0..rng.below(255))
                    .map(|_| alphabet[rng.below(alphabet.len())])
                    .collect();
                strings.set(slot, 1 + u32::from(slot), &text);
            }
        }
        let argc = rng.below(23);
        let args: Vec<Vec<u8>> = (0..argc)
            .map(|_| {
                let mut a = Vec::new();
                if rng.chance(60) {
                    a.push(1);
                }
                for _ in 0..rng.below(12) {
                    a.push(alphabet[rng.below(alphabet.len())]);
                }
                a
            })
            .collect();
        if let Some(resolved) = resolve(&args, &strings) {
            assert!(resolved.name.len() <= 1023);
            assert!(resolved.args.iter().all(|a| a.len() <= 1023));
        }
        let handle = strings.get(rng.below(40) as i64);
        assert!(expand(handle, &args).len() <= 1023);
        let _ = atoi(&args.concat());
    }
}

#[test]
fn net_string_churn_keeps_every_slot_in_range_and_the_table_consistent() {
    let mut rng = Rng(0x1357_9bdf_2468_ace0);
    let mut strings = SendStrings::new();
    let mut held = std::collections::HashMap::<u8, u32>::new();
    for _ in 0..200_000 {
        let tag = 1 + rng.below(100) as u32;
        let c = strings.check_string(tag, b"t");
        assert!(c.slot < 32);
        assert_eq!(strings.find(tag), Some(c.slot));
        assert_eq!(c.event.is_some(), c.acquired == Some(tag));
        if let Some(old) = c.released {
            assert_eq!(held.get(&c.slot), Some(&old));
        }
        held.insert(c.slot, tag);
        if rng.chance(20) {
            strings.delivered(c.slot, tag);
        }
    }
    assert_eq!(held.len(), 32);
}

#[test]
fn a_sender_flooding_commands_hits_backpressure_not_memory_growth() {
    let texts: Vec<Vec<u8>> = std::iter::once(b"Cmd".to_vec())
        .chain((0..60).map(|i| format!("Arg{i}").into_bytes()))
        .collect();
    let tags = Tags(texts);
    let mut queue: EventQueue<NetEvent> = EventQueue::new();
    let mut strings = SendStrings::new();
    let mut rng = Rng(77);
    let mut refused = 0usize;
    for _ in 0..20_000 {
        let mut args = vec![b"\x011".to_vec()];
        for _ in 0..rng.below(19) {
            args.push(format!("\x01{}", 2 + rng.below(60)).into_bytes());
        }
        if send_command(&mut queue, &mut strings, &tags, &args).is_err() {
            refused += 1;
        }
        assert!(queue.ordered_len() <= torque_net::cap::NET_EVENT_QUEUE_ORDERED);
    }
    assert!(
        refused > 0,
        "nothing drains the queue, so the cap must have refused commands"
    );
}
