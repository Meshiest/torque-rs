//! Bulk channels over a bad link and under hostile input. A sender and a receiver over a link that loses packets, delays them
//! (so they arrive out of order) and reports some delivered packets as lost: every message must arrive exactly once, whole and in
//! the order queued. Then a bounded, seeded fuzz of the section reader (random bytes and mutants of valid sections) and of the
//! patch reader. The counts are bounded for `cargo test`.

use std::collections::VecDeque;

use torque_net::bits::{BitReader, BitWriter};
use torque_net::chunk::patch;
use torque_net::chunk::{
    ChannelLimits, ChunkReceiver, ChunkSender, Delivered, Kind, MAX_RANGES, WINDOW,
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
    fn some_bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max);
        self.bytes(n)
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

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
    loss: usize,
    delay: usize,
    false_loss: usize,
    messages: &[(usize, Kind, Vec<u8>)],
) -> usize {
    run_with(limits(), seed, loss, delay, false_loss, messages).0
}

/// [`run`] over channels with `l` as their limits: the packets sent, and the most bytes the receiver held at once.
fn run_with(
    l: Vec<ChannelLimits>,
    seed: u64,
    loss: usize,
    delay: usize,
    false_loss: usize,
    messages: &[(usize, Kind, Vec<u8>)],
) -> (usize, usize) {
    let mut rng = Rng(seed);
    let mut tx = ChunkSender::new(&l);
    let mut rx = ChunkReceiver::new(&l);
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
            let room = (100 + rng.below(900)) * 8;
            let mut w = BitWriter::new();
            tx.write(&mut w, seq, room);
            assert!(w.bits() <= room, "the section stays in its room");
            let lost = rng.chance(loss);
            let d = rng.below(delay + 1);
            fates.push(Fate {
                arrives_at: round + d,
                notify_at: round + d + 1 + rng.below(3),
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
                rx.read(&mut BitReader::new(&bytes), &mut got)
                    .expect("a section a sender wrote reads");
                peak = peak.max(rx.held_bytes());
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
    let mut rng = Rng(7);
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
                let n = 1 + rng.below(cap);
                (channel, kind, blob(&mut rng, n))
            })
            .collect();
        run(seed, loss, delay, false_loss, &messages);
    }
}

#[test]
fn a_two_megabyte_blob_recovers_from_loss_and_reorder() {
    let mut rng = Rng(11);
    let data = blob(&mut rng, 2 << 20);
    let packets = run(21, 20, 10, 5, &[(0, Kind::Snapshot, data)]);
    // 2 MiB at 300 bytes a packet is about 7,000 packets, and a fifth of them go twice.
    assert!(packets > 6_500 && packets < 12_000, "{packets} packets");
}

fn valid_sections(rng: &mut Rng) -> Vec<Vec<u8>> {
    let l = limits();
    let mut tx = ChunkSender::new(&l);
    for (c, cap) in [(0, 5_000), (1, 5_000), (2, 1_900)] {
        let n = 1 + rng.below(cap);
        tx.enqueue(c, Kind::Snapshot, rng.bytes(n)).unwrap();
        let n = 1 + rng.below(cap);
        tx.enqueue(c, Kind::Delta, rng.bytes(n)).unwrap();
    }
    let mut out = Vec::new();
    for seq in 1..=60 {
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, (50 + rng.below(1000)) * 8);
        out.push(w.into_bytes());
        tx.on_notify(seq, rng.chance(70));
    }
    out
}

#[test]
fn the_section_reader_never_panics_and_holds_a_bounded_amount() {
    let l = limits();
    let bound: usize = l.iter().map(|c| c.max_message * WINDOW).sum();
    let mut rng = Rng(99);
    for round in 0..300 {
        let mut rx = ChunkReceiver::new(&l);
        let valid = valid_sections(&mut rng);
        for s in &valid {
            let mut mutant = s.clone();
            match rng.below(4) {
                0 => {}
                1 => {
                    for _ in 0..1 + rng.below(4) {
                        let i = rng.below(mutant.len());
                        mutant[i] ^= 1 << rng.below(8);
                    }
                }
                2 => mutant.truncate(rng.below(mutant.len() + 1)),
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
    let mut rng = Rng(5);
    let mut base = rng.bytes(3_000);
    for _ in 0..2_000 {
        let before = base.clone();
        let patch_bytes = match rng.below(3) {
            0 => rng.some_bytes(80),
            1 => {
                let mut new = base.clone();
                for _ in 0..rng.below(10) {
                    let i = rng.below(new.len());
                    new[i] = new[i].wrapping_add(1);
                }
                let mut p = patch::diff(&base, &new).unwrap_or_default();
                if !p.is_empty() && rng.chance(50) {
                    let i = rng.below(p.len());
                    p[i] ^= 1 << rng.below(8);
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
    let mut rng = Rng(31);
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
