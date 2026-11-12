//! Bulk channels over a bad link and under hostile input. A sender and a receiver over a link that loses packets, delays them
//! (so they arrive out of order) and reports some delivered packets as lost: every message must arrive exactly once, whole and in
//! the order queued. Then a bounded, seeded fuzz of the section reader (random bytes and mutants of valid sections) and of the
//! patch reader. The counts are bounded for `cargo test`.

use std::collections::VecDeque;
use torque_testkit::Rng;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::chunk::patch;
use torque_net::chunk::upstream::{UpstreamCaps, UpstreamReader, UpstreamServer};
use torque_net::chunk::{
    ChannelLimits, ChunkReceiver, ChunkSender, Delivered, Kind, MAX_RANGES, WINDOW,
};

fn limits() -> Vec<ChannelLimits> {
    vec![
        ChannelLimits::new(300).with_max_message(3 << 20),
        ChannelLimits::new(150).with_max_message(50_000),
        ChannelLimits::new(100).with_max_message(2_000),
    ]
}

struct Fate {
    arrives_at: usize,
    notify_at: usize,
    seq: u32,
    lost: bool,
    reported_lost: bool,
    bytes: Vec<u8>,
}

/// Sends `messages` over a link of `loss` percent loss, up to `delay` rounds of delay (reordering), and `false_loss` percent of
/// delivered packets reported lost anyway. Returns the packets sent.
fn run(
    seed: u64,
    loss: u64,
    delay: usize,
    false_loss: u64,
    messages: &[(usize, Kind, Vec<u8>)],
) -> usize {
    run_with(limits(), seed, loss, delay, false_loss, messages).0
}

/// What reads a section: a plain receiver (a client of a server's channels) or a server's reader of one client's upstream.
trait Reader {
    fn read(&mut self, bytes: &[u8], now: u32, out: &mut Vec<Delivered>) -> Result<(), NetError>;
    fn held(&self) -> usize;
}

impl Reader for ChunkReceiver {
    fn read(&mut self, bytes: &[u8], _now: u32, out: &mut Vec<Delivered>) -> Result<(), NetError> {
        ChunkReceiver::read(self, &mut BitReader::new(bytes), out)
    }

    fn held(&self) -> usize {
        self.held_bytes()
    }
}

impl Reader for UpstreamReader {
    fn read(&mut self, bytes: &[u8], now: u32, out: &mut Vec<Delivered>) -> Result<(), NetError> {
        UpstreamReader::read(self, &mut BitReader::new(bytes), now, out)
    }

    fn held(&self) -> usize {
        self.held_bytes()
    }
}

/// [`run`] over channels with `l` as their limits: the packets sent, and the most bytes the receiver held at once.
fn run_with(
    l: Vec<ChannelLimits>,
    seed: u64,
    loss: u64,
    delay: usize,
    false_loss: u64,
    messages: &[(usize, Kind, Vec<u8>)],
) -> (usize, usize) {
    let rx = ChunkReceiver::new(&l);
    run_over(
        &l,
        ChunkSender::new(&l),
        rx,
        seed,
        loss,
        delay,
        false_loss,
        messages,
    )
}

/// [`run_with`] for a given sender and reader.
#[allow(clippy::too_many_arguments)]
fn run_over(
    l: &[ChannelLimits],
    mut tx: ChunkSender,
    mut rx: impl Reader,
    seed: u64,
    loss: u64,
    delay: usize,
    false_loss: u64,
    messages: &[(usize, Kind, Vec<u8>)],
) -> (usize, usize) {
    let mut rng = Rng::star(seed);
    let mut queued: Vec<VecDeque<(Kind, Vec<u8>)>> = vec![VecDeque::new(); l.len()];
    let mut todo: VecDeque<&(usize, Kind, Vec<u8>)> = messages.iter().collect();
    let mut got: Vec<Delivered> = Vec::new();
    let mut fates: Vec<Fate> = Vec::new();
    let mut seq = 0u32;
    let mut round = 0usize;
    let mut peak = 0usize;
    while (!todo.is_empty() || !tx.is_idle() || !fates.is_empty()) && round < 400_000 {
        round += 1;
        // Queue what the backlog takes.
        while let Some((channel, kind, data)) = todo.front().copied() {
            match tx.enqueue(*channel, *kind, data.clone()) {
                Ok(()) => {
                    queued[*channel].push_back((*kind, data.clone()));
                    todo.pop_front();
                }
                Err(_) => break,
            }
        }
        if fates.len() < 25 && tx.has_work() {
            seq += 1;
            let room = (100 + rng.index(900)) * 8;
            let mut w = BitWriter::new();
            tx.write(&mut w, seq, room);
            assert!(w.bits() <= room, "the section stays in its room");
            let lost = rng.chance(loss);
            let d = rng.index(delay + 1);
            fates.push(Fate {
                arrives_at: round + d,
                notify_at: round + d + 1 + rng.index(3),
                seq,
                lost,
                reported_lost: lost || rng.chance(false_loss),
                bytes: w.into_bytes(),
            });
        }
        let mut i = 0;
        while i < fates.len() {
            let f = &fates[i];
            if !f.lost && f.arrives_at <= round && !f.bytes.is_empty() {
                let bytes = std::mem::take(&mut fates[i].bytes);
                rx.read(&bytes, round as u32, &mut got)
                    .expect("a section a sender wrote reads");
                peak = peak.max(rx.held());
                fates[i].bytes = vec![0]; // delivered: read once
                fates[i].arrives_at = usize::MAX;
            }
            let f = &fates[i];
            if f.notify_at <= round && (f.lost || f.arrives_at == usize::MAX) {
                let (seq, reported_lost) = (f.seq, f.reported_lost);
                tx.on_notify(seq, !reported_lost);
                fates.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
    assert!(round < 400_000, "the link did not settle");
    assert!(todo.is_empty() && tx.is_idle());
    // Exactly the messages queued, whole, in each channel's order.
    let mut per: Vec<VecDeque<(Kind, Vec<u8>)>> = vec![VecDeque::new(); l.len()];
    for d in got {
        per[d.channel].push_back((d.kind, d.data));
    }
    assert_eq!(per, queued, "seed {seed}, loss {loss}, delay {delay}");
    (seq as usize, peak)
}

fn blob(rng: &mut Rng, n: usize) -> Vec<u8> {
    rng.bytes(n)
}

#[test]
fn messages_arrive_once_whole_and_in_order_over_a_link_that_loses_delays_and_lies() {
    let mut rng = Rng::star(7);
    for (seed, loss, delay, false_loss) in [
        (1, 0, 0, 0),
        (2, 10, 5, 0),
        (3, 30, 12, 5),
        (4, 50, 20, 10),
        (5, 0, 30, 20),
    ] {
        let messages: Vec<(usize, Kind, Vec<u8>)> = (0..9)
            .map(|i| {
                let channel = i % 3;
                let cap = [200_000, 50_000, 2_000][channel];
                let kind = if i < 3 { Kind::Snapshot } else { Kind::Delta };
                let n = 1 + rng.index(cap);
                (channel, kind, blob(&mut rng, n))
            })
            .collect();
        run(seed, loss, delay, false_loss, &messages);
    }
}

#[test]
fn a_two_megabyte_blob_recovers_from_loss_and_reorder() {
    let mut rng = Rng::star(11);
    let data = blob(&mut rng, 2 << 20);
    let packets = run(21, 20, 10, 5, &[(0, Kind::Snapshot, data)]);
    // 2 MiB at 300 bytes a packet is about 7,000 packets, and a fifth of them go twice.
    assert!(packets > 6_500 && packets < 12_000, "{packets} packets");
}

fn valid_sections(rng: &mut Rng) -> Vec<Vec<u8>> {
    let l = limits();
    let mut tx = ChunkSender::new(&l);
    for (c, cap) in [(0, 5_000), (1, 5_000), (2, 1_900)] {
        let n = 1 + rng.index(cap);
        tx.enqueue(c, Kind::Snapshot, rng.bytes(n)).unwrap();
        let n = 1 + rng.index(cap);
        tx.enqueue(c, Kind::Delta, rng.bytes(n)).unwrap();
    }
    let mut out = Vec::new();
    for seq in 1..=60 {
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, (50 + rng.index(1000)) * 8);
        out.push(w.into_bytes());
        tx.on_notify(seq, rng.chance(70));
    }
    out
}

#[test]
fn the_section_reader_never_panics_and_holds_a_bounded_amount() {
    let l = limits();
    let bound: usize = l.iter().map(|c| c.max_message * WINDOW).sum();
    let mut rng = Rng::star(99);
    for round in 0..300 {
        let mut rx = ChunkReceiver::new(&l);
        let valid = valid_sections(&mut rng);
        for s in &valid {
            let mut mutant = s.clone();
            match rng.index(4) {
                0 => {}
                1 => {
                    for _ in 0..1 + rng.index(4) {
                        let i = rng.index(mutant.len());
                        mutant[i] ^= 1 << rng.index(8);
                    }
                }
                2 => mutant.truncate(rng.index(mutant.len() + 1)),
                _ => mutant = rng.some_bytes(400),
            }
            let _ = rx.read(&mut BitReader::new(&mutant), &mut Vec::new());
            assert!(rx.held_bytes() <= bound, "round {round}");
        }
    }
}

#[test]
fn a_hostile_peer_cannot_make_a_message_fragment_without_bound() {
    use torque_net::chunk::{CHUNK_LEN_BITS, id_bits};
    let l = vec![ChannelLimits::new(100).with_max_message(1 << 20)];
    let size_bits = torque_net::bits::bit_length(1 << 20);
    let mut rx = ChunkReceiver::new(&l);
    let mut failed = false;
    // One byte at every other offset: each is a range of its own.
    for packet in 0..MAX_RANGES {
        let mut w = BitWriter::new();
        w.write_flag(true);
        for k in 0..2 {
            w.write_bits(0, id_bits(1));
            w.write_flag(false);
            w.write_bits(0, 16);
            w.write_bits(1 << 20, size_bits);
            w.write_bits(((packet * 2 + k) * 2) as u32, size_bits);
            w.write_bits(1, CHUNK_LEN_BITS);
            w.write_u8(1);
            w.write_flag(k == 0);
        }
        let bytes = w.into_bytes();
        if rx
            .read(&mut BitReader::new(&bytes), &mut Vec::new())
            .is_err()
        {
            failed = true;
            break;
        }
    }
    assert!(failed, "more than {MAX_RANGES} ranges is refused");
}

#[test]
fn the_patch_reader_never_panics() {
    let mut rng = Rng::star(5);
    let mut base = rng.bytes(3_000);
    for _ in 0..2_000 {
        let before = base.clone();
        let patch_bytes = match rng.index(3) {
            0 => rng.some_bytes(80),
            1 => {
                let mut new = base.clone();
                for _ in 0..rng.index(10) {
                    let i = rng.index(new.len());
                    new[i] = new[i].wrapping_add(1);
                }
                let mut p = patch::diff(&base, &new).unwrap_or_default();
                if !p.is_empty() && rng.chance(50) {
                    let i = rng.index(p.len());
                    p[i] ^= 1 << rng.index(8);
                }
                p
            }
            _ => {
                let mut p = rng.bytes(12);
                p.extend_from_slice(&rng.some_bytes(20));
                p
            }
        };
        match patch::apply(&mut base, &patch_bytes, 4_096) {
            Ok(()) => assert!(base.len() <= 4_096),
            Err(_) => assert_eq!(base, before, "an error changes nothing"),
        }
    }
}

#[test]
fn large_messages_under_loss_with_more_queued_complete_within_the_default_caps() {
    // Three 2 MiB messages queued at once over a link that loses, delays and misreports: the next message starts while the one
    // before may still have ranges to go again, so the receiver holds more than one at a time, and what it holds stays inside
    // the cap derived from the channel (a window of its largest messages).
    let l = vec![ChannelLimits::new(1_000).with_max_message(2 << 20)];
    let default = torque_net::chunk::default_hold_cap(&l);
    assert_eq!(default, 8 << 20);
    let mut rng = Rng::star(31);
    let messages: Vec<(usize, Kind, Vec<u8>)> = (0..3)
        .map(|i| {
            let kind = if i == 0 { Kind::Snapshot } else { Kind::Delta };
            (0, kind, rng.bytes((2 << 20) - 1_000 * i))
        })
        .collect();
    let (packets, peak) = run_with(l, 41, 20, 12, 5, &messages);
    eprintln!("6 MiB in {packets} packets, the receiver held {peak} bytes at most");
    assert!(peak <= default);
}

#[test]
fn the_default_caps_follow_the_channels_and_a_sender_never_queues_more_than_a_receiver_holds() {
    use torque_net::chunk::{DEFAULT_HOLD_CAP, MAX_HOLD_CAP, default_hold_cap};
    let small = [ChannelLimits::new(100).with_max_message(1_000)];
    assert_eq!(default_hold_cap(&small), DEFAULT_HOLD_CAP);
    let big = [ChannelLimits::new(100).with_max_message(1 << 30)];
    assert_eq!(default_hold_cap(&big), MAX_HOLD_CAP);
    // The sender refuses a message the receiver could not hold whole.
    let mut tx = ChunkSender::new(&big);
    assert!(
        tx.enqueue(0, Kind::Snapshot, vec![1; MAX_HOLD_CAP + 1])
            .is_err()
    );
    // Across channels it queues what the receiver of the same channels holds, and no more.
    let l = vec![
        ChannelLimits::new(100)
            .with_max_message(4 << 20)
            .with_max_backlog(64 << 20);
        2
    ];
    let hold = default_hold_cap(&l);
    assert_eq!(hold, 16 << 20);
    let mut tx = ChunkSender::new(&l);
    for i in 0..4 {
        tx.enqueue(i % 2, Kind::Delta, vec![1; 4 << 20]).unwrap();
    }
    assert_eq!(tx.total_backlog(), hold);
    assert!(tx.enqueue(0, Kind::Delta, vec![1; 1]).is_err());
}

/// The server's caps for the tests below: the ones a connection holds, with a rate no honest sender over a memory link reaches.
fn upstream_caps(hold: usize) -> UpstreamCaps {
    UpstreamCaps {
        hold_per_connection: hold,
        hold_total: 4 * hold,
        bytes_per_second: 1 << 30,
        byte_burst: 1 << 30,
        messages_per_second: 1 << 20,
        message_burst: 1 << 20,
        ..UpstreamCaps::default()
    }
}

#[test]
fn a_client_to_server_message_of_four_megabytes_arrives_intact_over_a_link_losing_five_percent() {
    // The roles swapped: the client's sender, the server's capped reader of that client.
    let l = vec![ChannelLimits::new(1_000).with_max_message(4 << 20)];
    let server = UpstreamServer::new(upstream_caps(8 << 20));
    let mut rng = Rng::star(55);
    let data = blob(&mut rng, 4 << 20);
    let tx = ChunkSender::new(&l).with_total_backlog(8 << 20);
    let (packets, peak) = run_over(
        &l,
        tx,
        server.connection(&l),
        56,
        5,
        8,
        2,
        &[(0, Kind::Snapshot, data)],
    );
    eprintln!("4 MiB upstream in {packets} packets, the server held {peak} bytes at most");
    assert!(peak <= 8 << 20);
    assert_eq!(server.held_total(), 0, "the reader gave its bytes back");
}

#[test]
fn upstream_messages_on_several_channels_arrive_once_whole_and_in_order() {
    let l = limits();
    let server = UpstreamServer::new(upstream_caps(8 << 20));
    let mut rng = Rng::star(77);
    for (seed, loss, delay, false_loss) in [(1, 5, 4, 0), (2, 20, 12, 5), (3, 40, 20, 10)] {
        let messages: Vec<(usize, Kind, Vec<u8>)> = (0..12)
            .map(|i| {
                let channel = i % 3;
                let cap = [200_000, 50_000, 2_000][channel];
                let n = 1 + rng.index(cap);
                (channel, Kind::Delta, blob(&mut rng, n))
            })
            .collect();
        run_over(
            &l,
            ChunkSender::new(&l),
            server.connection(&l),
            seed,
            loss,
            delay,
            false_loss,
            &messages,
        );
    }
}

/// A chunk one byte into a message of `total` bytes on channel 0 of a single channel: the reader allocates and charges the whole
/// message at once.
fn first_chunk(total: usize, seq: u16, size_bits: u8) -> Vec<u8> {
    use torque_net::chunk::{CHUNK_LEN_BITS, id_bits};
    let mut w = BitWriter::new();
    w.write_flag(true);
    w.write_bits(0, id_bits(1));
    w.write_flag(false);
    w.write_bits(u32::from(seq), 16);
    w.write_bits(total as u32, size_bits);
    w.write_bits(0, size_bits);
    w.write_bits(1, CHUNK_LEN_BITS);
    w.write_u8(1);
    w.write_flag(false);
    w.into_bytes()
}

fn read_one(rx: &mut UpstreamReader, bytes: &[u8], now: u32) -> Result<Vec<Delivered>, NetError> {
    let mut out = Vec::new();
    rx.read(&mut BitReader::new(bytes), now, &mut out)
        .map(|()| out)
}

#[test]
fn hostile_connections_filling_the_shared_budget_do_not_get_an_honest_one_dropped() {
    let l = vec![ChannelLimits::new(1_000).with_max_message(200_000)];
    let size_bits = torque_net::bits::bit_length(200_000);
    let server = UpstreamServer::new(UpstreamCaps {
        hold_per_connection: 200_000,
        hold_total: 800_000,
        ..upstream_caps(0)
    });
    // Five connections each claim a message of 150,000 bytes with a single chunk and send nothing more: 750,000 of 800,000.
    let mut hostile: Vec<UpstreamReader> = (0..5).map(|_| server.connection(&l)).collect();
    for h in &mut hostile {
        read_one(h, &first_chunk(150_000, 0, size_bits), 0).expect("room for it");
    }
    assert_eq!(server.held_total(), 750_000);
    // An honest connection sends 100,000 bytes over a lossy link: its first chunk finds the budget full.
    let mut rng = Rng::star(3);
    let data = blob(&mut rng, 100_000);
    let mut tx = ChunkSender::new(&l);
    tx.enqueue(0, Kind::Snapshot, data.clone()).unwrap();
    let mut honest = server.connection(&l);
    let mut got = Vec::new();
    for seq in 1..5_000u32 {
        if !tx.has_work() {
            break;
        }
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, 1_000 * 8);
        let lost = rng.chance(5);
        if !lost {
            got.extend(
                read_one(&mut honest, &w.into_bytes(), seq)
                    .expect("the honest connection is never refused"),
            );
        }
        tx.on_notify(seq, !lost);
        assert!(server.held_total() <= 800_000, "the budget holds");
    }
    assert_eq!(got.len(), 1);
    assert!(got[0].data == data, "the honest transfer arrived intact");
    // The room came from the holders of the most bytes, which are ended: they hold nothing, and their next packet is refused.
    assert!(server.evictions() >= 1);
    let dead: Vec<usize> = (0..5).filter(|i| hostile[*i].is_dead()).collect();
    assert_eq!(dead.len() as u64, server.evictions());
    for i in &dead {
        assert_eq!(hostile[*i].held_bytes(), 0);
        assert!(read_one(&mut hostile[*i], &first_chunk(10, 1, size_bits), 9).is_err());
        assert!(hostile[*i].sweep(10).is_err());
    }
    assert!(server.held_total() <= 800_000);
    drop(hostile);
    drop(honest);
    assert_eq!(server.held_total(), 0);
}

#[test]
fn a_connection_that_would_hold_the_most_is_the_one_refused_when_the_budget_is_full() {
    let l = vec![ChannelLimits::new(1_000).with_max_message(200_000)];
    let size_bits = torque_net::bits::bit_length(200_000);
    let server = UpstreamServer::new(UpstreamCaps {
        hold_per_connection: 200_000,
        hold_total: 800_000,
        ..upstream_caps(0)
    });
    let mut holders: Vec<UpstreamReader> = (0..4).map(|_| server.connection(&l)).collect();
    for h in &mut holders {
        read_one(h, &first_chunk(190_000, 0, size_bits), 0).expect("room");
    }
    let mut big = server.connection(&l);
    // 190,000 more would pass the cap and nobody holds more than that: the newcomer is refused and the others are left alone.
    assert!(read_one(&mut big, &first_chunk(200_000, 0, size_bits), 0).is_err());
    assert_eq!(server.evictions(), 0);
    assert!(
        holders
            .iter()
            .all(|h| !h.is_dead() && h.held_bytes() == 190_000)
    );
    // A smaller newcomer does not displace them either when it fits.
    let mut small = server.connection(&l);
    read_one(&mut small, &first_chunk(40_000, 0, size_bits), 0).expect("fits");
    assert_eq!(server.evictions(), 0);
    assert_eq!(server.held_total(), 4 * 190_000 + 40_000);
}

#[test]
fn a_half_received_message_that_goes_stale_ends_its_connection_and_releases_its_bytes() {
    let l = vec![ChannelLimits::new(1_000).with_max_message(100_000)];
    let size_bits = torque_net::bits::bit_length(100_000);
    let server = UpstreamServer::new(UpstreamCaps {
        message_age_ms: 1_000,
        message_min_rate: 100_000,
        ..upstream_caps(1 << 20)
    });
    let mut slow = server.connection(&l);
    read_one(&mut slow, &first_chunk(50_000, 0, size_bits), 100).expect("the first chunk");
    // The whole message is charged at once (what a sender claims is what it pays for).
    assert!(slow.held_bytes() >= 50_000);
    assert_eq!(server.held_total(), slow.held_bytes());
    // Within its time (1,000 ms and 500 ms for its bytes at the least rate) nothing happens.
    assert!(slow.sweep(1_500).is_ok());
    assert!(slow.held_bytes() >= 50_000);
    // Past it, with no more packets from the connection, the host's tick still finds it.
    assert!(slow.sweep(1_700).is_err());
    assert!(slow.is_dead());
    assert_eq!((slow.held_bytes(), server.held_total()), (0, 0));
    assert!(read_one(&mut slow, &first_chunk(10, 1, size_bits), 1_800).is_err());
    // A reader that completes its messages in time is never stale, however long it lives.
    let mut honest = server.connection(&l);
    let mut tx = ChunkSender::new(&l);
    for round in 0..20u32 {
        tx.enqueue(0, Kind::Delta, vec![round as u8; 600]).unwrap();
        let mut w = BitWriter::new();
        tx.write(&mut w, round + 1, 8_000 * 8);
        tx.on_notify(round + 1, true);
        let out = read_one(&mut honest, &w.into_bytes(), round * 700).expect("in time");
        assert_eq!(out.len(), 1);
        assert!(honest.sweep(round * 700 + 600).is_ok());
    }
}

#[test]
fn an_upstream_flood_past_the_rate_or_the_message_size_is_refused_and_delivers_nothing() {
    let l = vec![ChannelLimits::new(1_000).with_max_message(100_000)];
    let server = UpstreamServer::new(UpstreamCaps {
        bytes_per_second: 10_000,
        byte_burst: 20_000,
        messages_per_second: 2,
        message_burst: 3,
        ..UpstreamCaps::default()
    });
    // Messages of one byte each, a packet for each.
    let mut tx = ChunkSender::new(&l);
    let mut rx = server.connection(&l);
    let mut delivered = 0;
    let mut refused_at = None;
    for i in 0..10u32 {
        tx.enqueue(0, Kind::Delta, vec![i as u8]).unwrap();
        let mut w = BitWriter::new();
        tx.write(&mut w, i + 1, 8_000);
        tx.on_notify(i + 1, true);
        let mut out = Vec::new();
        // The same millisecond: no refill between them.
        match rx.read(&mut BitReader::new(&w.into_bytes()), 5, &mut out) {
            Ok(()) => delivered += out.len(),
            Err(_) => {
                assert!(
                    out.is_empty(),
                    "nothing is delivered from the packet that went over"
                );
                refused_at = Some(i);
                break;
            }
        }
    }
    assert_eq!(
        (delivered, refused_at),
        (3, Some(3)),
        "a burst of 3 messages"
    );
    // A message above the cap is refused from its first chunk.
    let big = vec![ChannelLimits::new(1_000).with_max_message(10 << 20)];
    let strict = UpstreamServer::new(UpstreamCaps {
        max_message: 50_000,
        ..UpstreamCaps::default()
    });
    let mut tx = ChunkSender::new(&big);
    tx.enqueue(0, Kind::Snapshot, vec![7; 60_000]).unwrap();
    let mut w = BitWriter::new();
    tx.write(&mut w, 1, 8_000);
    let mut rx = strict.connection(&big);
    assert!(
        rx.read(&mut BitReader::new(&w.into_bytes()), 0, &mut Vec::new())
            .is_err()
    );
    assert_eq!(rx.held_bytes(), 0);
}
