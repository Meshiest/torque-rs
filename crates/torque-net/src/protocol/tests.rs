use super::*;

/// A tiny deterministic generator for the loss patterns.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u32) -> u32 {
        ((self.next() >> 32) as u32) % n.max(1)
    }
}

/// A host's own fast rates, past the preferences' clamps (a host sets the fields as it likes): the server 32 packets a second
/// both ways, the client 128, both at the 10-bit field's 1,023 bytes.
fn fast(rate: u32) -> RateConfig {
    RateConfig {
        to_server_rate: rate,
        to_client_rate: rate,
        packet_size: MAX_PACKET_BYTES,
    }
}

/// A server and a client of the fast rates.
fn pair() -> (Protocol, Protocol) {
    let server = Protocol::new(Role::Server, 7, fast(32), 0);
    let client = Protocol::new(Role::Client, 7, fast(128), 0);
    (server, client)
}

/// A server and a client of TGE's standard rates.
fn standard_pair() -> (Protocol, Protocol) {
    let server = Protocol::new(Role::Server, 7, RateConfig::standard(), 0);
    let client = Protocol::new(Role::Client, 7, RateConfig::standard(), 0);
    (server, client)
}

/// A data packet from `p` with a one-byte payload marker.
fn data(p: &mut Protocol, now: u32) -> Vec<u8> {
    let mut w = p.begin_data_packet(now);
    w.write_u8(0xa5);
    w.into_bytes()
}

fn recv(p: &mut Protocol, bytes: &[u8], now: u32) -> Received {
    let mut r = BitReader::new(bytes);
    p.receive(&mut r, now).expect("a valid packet")
}

/// A packet with exactly these header fields (and an optional rate prefix for a data packet).
fn craft(
    connect_bit: bool,
    seq: u16,
    ack: u16,
    ptype: PacketType,
    mask: (u8, u32),
    rate: Option<RatePrefix>,
) -> Vec<u8> {
    let mut w = BitWriter::new();
    Header {
        connect_bit,
        seq,
        ack,
        ptype,
        ack_bytes: mask.0,
        ack_mask: mask.1,
    }
    .write(&mut w);
    if let Some(rate) = rate {
        rate.write(&mut w);
    }
    w.write_u8(0x5a);
    w.into_bytes()
}

struct Snapshot {
    last_seq_recvd: u32,
    highest_acked: u32,
    last_send_seq: u32,
    ack_mask: u32,
    last_recv_ack_ack: u32,
    unacked: usize,
    cur: Rate,
    max: Rate,
}

fn snap(p: &Protocol) -> Snapshot {
    Snapshot {
        last_seq_recvd: p.last_seq_recvd,
        highest_acked: p.highest_acked,
        last_send_seq: p.last_send_seq,
        ack_mask: p.ack_mask,
        last_recv_ack_ack: p.last_recv_ack_ack,
        unacked: p.notifies.len(),
        cur: p.cur,
        max: p.max,
    }
}

fn same(a: &Snapshot, b: &Snapshot) -> bool {
    a.last_seq_recvd == b.last_seq_recvd
        && a.highest_acked == b.highest_acked
        && a.last_send_seq == b.last_send_seq
        && a.ack_mask == b.ack_mask
        && a.last_recv_ack_ack == b.last_recv_ack_ack
        && a.unacked == b.unacked
        && a.cur == b.cur
        && a.max == b.max
}

#[test]
fn a_header_round_trips_with_every_ack_byte_count() {
    let types = [PacketType::Data, PacketType::Ping, PacketType::Ack];
    for ptype in types {
        for ack_bytes in 0..=4u8 {
            for (connect_bit, seq, ack) in [(false, 0u16, 0u16), (true, 511, 300), (true, 1, 510)] {
                let mask_bits = u32::from(ack_bytes) * 8;
                let mask = if mask_bits == 0 {
                    0
                } else {
                    0xdead_beef_u32 & (u32::MAX >> (32 - mask_bits))
                };
                let h = Header {
                    connect_bit,
                    seq,
                    ack,
                    ptype,
                    ack_bytes,
                    ack_mask: mask,
                };
                let mut w = BitWriter::new();
                h.write(&mut w);
                assert_eq!(w.bits(), 1 + 1 + 9 + 9 + 2 + 3 + mask_bits as usize);
                let bytes = w.into_bytes();
                let mut r = BitReader::new(&bytes);
                assert_eq!(Header::read(&mut r), Ok(h));
                assert!(r.bits_left() < 8);
            }
        }
    }
}

#[test]
fn a_header_with_type_3_or_more_than_4_ack_bytes_is_invalid() {
    let mut w = BitWriter::new();
    w.write_flag(true);
    w.write_flag(false);
    w.write_bits(0, 9);
    w.write_bits(0, 9);
    w.write_bits(3, 2);
    w.write_bits(0, 3);
    let bytes = w.into_bytes();
    assert_eq!(
        Header::read(&mut BitReader::new(&bytes)),
        Err(NetError::Invalid("packet type"))
    );
    for n in 5..=7u32 {
        let mut w = BitWriter::new();
        w.write_flag(true);
        w.write_flag(false);
        w.write_bits(0, 9);
        w.write_bits(0, 9);
        w.write_bits(0, 2);
        w.write_bits(n, 3);
        w.write_bits(0, 24);
        let bytes = w.into_bytes();
        assert_eq!(
            Header::read(&mut BitReader::new(&bytes)),
            Err(NetError::Invalid("ack byte count"))
        );
    }
}

#[test]
fn the_rate_prefix_round_trips_all_four_shapes() {
    for cur in [None, Some((32, 1023))] {
        for max in [None, Some((8, 200))] {
            let p = RatePrefix { cur, max };
            let mut w = BitWriter::new();
            p.write(&mut w);
            assert_eq!(
                w.bits(),
                2 + 20 * (usize::from(cur.is_some()) + usize::from(max.is_some()))
            );
            let bytes = w.into_bytes();
            assert_eq!(RatePrefix::read(&mut BitReader::new(&bytes)), Ok(p));
        }
    }
}

#[test]
fn a_data_packet_is_delivered_acked_and_establishes_the_connection() {
    let (mut s, mut c) = pair();
    let p1 = data(&mut c, 10);
    let got = recv(&mut s, &p1, 11);
    assert_eq!(got.dropped, None);
    assert!(got.payload && got.ptype == Some(PacketType::Data) && got.seq == 1);
    assert_eq!(s.last_seq_recvd(), 1);
    assert_eq!(c.unacked(), 1);
    assert!(!c.acked_once());

    let p2 = data(&mut s, 12);
    let back = recv(&mut c, &p2, 50);
    assert!(back.payload);
    assert_eq!(
        back.notifies,
        vec![Notify {
            send_seq: 1,
            delivered: true
        }]
    );
    assert_eq!(c.unacked(), 0);
    assert_eq!(c.highest_acked(), 1);
    assert!(c.acked_once());
    assert_eq!(c.round_trip(), 20.0);
}

#[test]
fn a_gap_in_the_sequence_nacks_the_lost_packet() {
    let (mut s, mut c) = pair();
    let _p1 = data(&mut c, 0);
    let _p2 = data(&mut c, 1);
    let p3 = data(&mut c, 2);
    let got = recv(&mut s, &p3, 3);
    assert!(got.payload);
    assert_eq!(s.last_seq_recvd(), 3);
    let reply = data(&mut s, 4);
    let back = recv(&mut c, &reply, 5);
    assert_eq!(
        back.notifies,
        vec![
            Notify {
                send_seq: 1,
                delivered: false
            },
            Notify {
                send_seq: 2,
                delivered: false
            },
            Notify {
                send_seq: 3,
                delivered: true
            },
        ]
    );
    assert_eq!(c.highest_acked(), 3);
}

#[test]
fn the_send_window_closes_at_thirty_unacked_packets_and_reopens_on_an_ack() {
    let (mut s, mut c) = pair();
    for i in 0..30 {
        assert!(!c.window_full(), "packet {i}");
        let _ = data(&mut c, i);
    }
    assert!(c.window_full());
    assert!(!c.may_send(10_000, true));
    let ack = s.ack_packet(5);
    // The server never received anything, so its ack says 0: nothing is acked.
    let got = recv(&mut c, &ack, 6);
    assert!(got.notifies.is_empty());
    assert!(c.window_full());

    // Deliver all 30 to the server (each is within 31 of the last received) and let it answer.
    let (mut s, mut c) = pair();
    let mut sent = Vec::new();
    for i in 0..30 {
        sent.push(data(&mut c, i));
    }
    for p in &sent {
        recv(&mut s, p, 100);
    }
    let reply = data(&mut s, 101);
    let back = recv(&mut c, &reply, 102);
    assert_eq!(back.notifies.len(), 30);
    assert!(back.notifies.iter().all(|n| n.delivered));
    assert!(!c.window_full());
    assert!(c.may_send(1_000, true));
}

#[test]
fn a_sequence_is_accepted_up_to_31_ahead_and_dropped_beyond() {
    for distance in 0..=40u32 {
        let (mut s, _) = pair();
        let bytes = craft(true, distance as u16, 0, PacketType::Ack, (0, 0), None);
        let got = recv(&mut s, &bytes, 1);
        if distance <= 31 {
            assert_eq!(got.dropped, None, "distance {distance}");
            assert_eq!(s.last_seq_recvd(), distance);
        } else {
            assert_eq!(got.dropped, Some(Dropped::Window), "distance {distance}");
            assert_eq!(s.last_seq_recvd(), 0);
        }
    }
}

#[test]
fn an_ack_past_the_last_packet_sent_is_dropped_and_changes_nothing() {
    let (mut s, mut c) = pair();
    let p = data(&mut c, 0);
    recv(&mut s, &p, 1);
    let before = snap(&c);
    // The client sent 1 packet; an ack of 2 (and of 5, and 511) names packets that never existed.
    for ack in [2u16, 5, 31, 200, 511] {
        let bytes = craft(true, 1, ack, PacketType::Ack, (1, 0xff), None);
        let got = recv(&mut c, &bytes, 2);
        assert_eq!(got.dropped, Some(Dropped::AckPastSend), "ack {ack}");
        assert!(got.notifies.is_empty());
        assert!(same(&before, &snap(&c)));
    }
    assert!(c.highest_acked() <= c.last_send_seq());
    // An ack of exactly the last packet is fine.
    let ok = craft(true, 1, 1, PacketType::Ack, (1, 1), None);
    let got = recv(&mut c, &ok, 3);
    assert_eq!(got.dropped, None);
    assert_eq!(c.unacked(), 0);
}

#[test]
fn an_old_ack_does_not_move_the_state_backwards() {
    let (mut s, mut c) = pair();
    for i in 0..5 {
        let p = data(&mut c, i);
        recv(&mut s, &p, i);
    }
    let reply = data(&mut s, 10);
    recv(&mut c, &reply, 11);
    assert_eq!(c.highest_acked(), 5);
    let before = snap(&c);
    // An ack of 2 is now 509 ahead of what is acked: dropped as past the send.
    let stale = craft(true, 1, 2, PacketType::Ack, (1, 0x3), None);
    let got = recv(&mut c, &stale, 12);
    assert_eq!(got.dropped, Some(Dropped::AckPastSend));
    assert!(same(&before, &snap(&c)));
}

#[test]
fn a_duplicate_or_reordered_data_packet_is_not_delivered_twice() {
    let (mut s, mut c) = pair();
    let p1 = data(&mut c, 0);
    let p2 = data(&mut c, 1);
    let first = recv(&mut s, &p2, 2);
    assert!(first.payload);
    // Packet 1 arrives after packet 2: 511 behind, dropped.
    let late = recv(&mut s, &p1, 3);
    assert_eq!(late.dropped, Some(Dropped::Window));
    // Packet 2 again: valid header, not new, no payload.
    let again = recv(&mut s, &p2, 4);
    assert_eq!(again.dropped, None);
    assert!(!again.payload);
    assert_eq!(s.last_seq_recvd(), 2);
}

#[test]
fn the_wrong_connect_bit_is_dropped() {
    let (mut s, _) = pair();
    let bytes = craft(false, 1, 0, PacketType::Ack, (0, 0), None);
    let before = snap(&s);
    assert_eq!(recv(&mut s, &bytes, 1).dropped, Some(Dropped::ConnectBit));
    assert!(same(&before, &snap(&s)));
}

#[test]
fn a_truncated_packet_is_dropped_or_an_error_at_every_bit_length() {
    let (mut s, mut c) = pair();
    let packet = data(&mut c, 0);
    for bits in 0..=packet.len() * 8 {
        let mut copy = s.clone();
        let before = snap(&copy);
        let mut r = BitReader::with_bit_len(&packet, bits);
        match copy.receive(&mut r, 5) {
            Ok(got) => {
                if got.dropped.is_some() {
                    assert!(same(&before, &snap(&copy)), "{bits} bits");
                }
            }
            Err(e) => assert_eq!(e, NetError::Overrun, "{bits} bits"),
        }
    }
    // The whole packet still works on the untouched original.
    assert!(recv(&mut s, &packet, 6).payload);
}

#[test]
fn a_hostile_ack_never_pops_an_empty_notify_queue() {
    let (_, mut c) = pair();
    assert_eq!(c.handle_notify(1, true, 0), None);
    assert_eq!(c.handle_notify(1, false, 0), None);
    // Every ack of a client that sent nothing is past the send.
    for ack in 1..512u16 {
        let bytes = craft(true, 0, ack, PacketType::Ack, (4, u32::MAX), None);
        assert_eq!(recv(&mut c, &bytes, 1).dropped, Some(Dropped::AckPastSend));
    }
    assert_eq!(c.unacked(), 0);
}

#[test]
fn the_sequence_wraps_over_thousands_of_lossy_packets_with_every_fate_reported_once_in_order() {
    let (mut a, mut b) = pair();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    // Seqs of a's data packets b took, and the notifies a heard.
    let mut b_got: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut a_notified: Vec<Notify> = Vec::new();
    let mut now = 0u32;
    let mut sent_by_a = 0u32;
    for step in 0..6000u32 {
        now += 7;
        // a sends a data packet when its window allows; 25% are lost.
        if !a.window_full() && rng.below(3) != 0 {
            let pkt = data(&mut a, now);
            sent_by_a += 1;
            if rng.below(4) != 0 {
                let got = recv(&mut b, &pkt, now);
                assert_eq!(got.dropped, None, "step {step}");
                if got.payload {
                    b_got.insert(got.seq);
                }
            }
        }
        // b answers about half the time, with data, an ack or a ping; 25% are lost.
        if rng.below(2) == 0 {
            let reply = match rng.below(3) {
                0 if !b.window_full() => data(&mut b, now),
                1 => b.ack_packet(now),
                _ => b.ping_packet(now),
            };
            if rng.below(4) != 0 {
                let got = recv(&mut a, &reply, now);
                assert_eq!(got.dropped, None, "step {step}: reply");
                a_notified.extend(got.notifies);
            }
        }
        assert!(a.highest_acked().wrapping_sub(a.last_send_seq()) as i32 <= 0);
        assert!(b.highest_acked().wrapping_sub(b.last_send_seq()) as i32 <= 0);
        assert_eq!(
            a.unacked() as u32,
            a.last_send_seq().wrapping_sub(a.highest_acked())
        );
    }
    assert!(
        sent_by_a > 2000,
        "sent {sent_by_a}: the sequence wrapped many times"
    );
    for (next, n) in (1u32..).zip(&a_notified) {
        assert_eq!(n.send_seq, next, "fates arrive once, in order");
        // a's seq counter is not masked, so b_got holds widened seqs on the same scale.
        assert_eq!(
            n.delivered,
            b_got.contains(&n.send_seq),
            "packet {}",
            n.send_seq
        );
    }
    assert!(a_notified.len() as u32 + a.unacked() as u32 == sent_by_a);
}

// ---- rates ---------------------------------------------------------------------------------------------------------------

#[test]
fn the_rates_start_as_the_constructor_leaves_them() {
    let (s, c) = pair();
    assert_eq!(
        s.cur_rate(),
        Rate {
            delay: 102,
            size: 200,
            changed: false
        }
    );
    assert_eq!(
        s.max_rate(),
        Rate {
            delay: 32,
            size: 1023,
            changed: true
        }
    );
    assert_eq!(
        c.max_rate(),
        Rate {
            delay: 8,
            size: 1023,
            changed: true
        }
    );
    // TGE's standard preferences: the maximum is the current rate, so nothing is marked changed.
    let (s, c) = standard_pair();
    for p in [&s, &c] {
        assert_eq!(
            p.max_rate(),
            Rate {
                delay: 102,
                size: 200,
                changed: false
            }
        );
    }
    assert_eq!(c.to_server_delay(), 32);
    // The preferences are clamped (`checkMaxRate`).
    let c = RateConfig::new(1, 99, 50);
    assert_eq!(
        (c.to_server_rate, c.to_client_rate, c.packet_size),
        (8, 32, 100)
    );
    let c = RateConfig::new(99, 0, 1_000);
    assert_eq!(
        (c.to_server_rate, c.to_client_rate, c.packet_size),
        (32, 1, 450)
    );
    assert_eq!(RateConfig::default(), RateConfig::new(32, 10, 200));
    // A changed size or delay is told to the peer.
    let p = Protocol::new(Role::Server, 1, RateConfig::new(32, 10, 450), 0);
    assert!(p.max_rate().changed && p.max_rate().size == 450);
}

#[test]
fn tge_standard_rates_send_no_prefix_and_fill_200_byte_packets() {
    let (mut s, mut c) = standard_pair();
    let p = data(&mut c, 0);
    let mut r = BitReader::new(&p);
    Header::read(&mut r).expect("header");
    assert_eq!(RatePrefix::read(&mut r), Ok(RatePrefix::default()));
    recv(&mut s, &p, 1);
    assert_eq!(s.packet_limit_bits(), 200 * 8);
    // The server sends every 102 ms.
    let _ = data(&mut s, 200);
    assert!(!s.may_send(301, false));
    assert!(s.may_send(302, false));
}

#[test]
fn the_first_packets_negotiate_32_ms_and_1023_bytes() {
    let (mut s, mut c) = pair();
    let p = data(&mut c, 0);
    let mut r = BitReader::new(&p);
    let h = Header::read(&mut r).expect("header");
    assert_eq!(h.ptype, PacketType::Data);
    assert_eq!(
        RatePrefix::read(&mut r),
        Ok(RatePrefix {
            cur: None,
            max: Some((8, 1023))
        })
    );
    recv(&mut s, &p, 1);
    assert_eq!(
        s.cur_rate(),
        Rate {
            delay: 32,
            size: 1023,
            changed: true
        }
    );

    let p = data(&mut s, 2);
    let mut r = BitReader::new(&p);
    Header::read(&mut r).expect("header");
    assert_eq!(
        RatePrefix::read(&mut r),
        Ok(RatePrefix {
            cur: Some((32, 1023)),
            max: Some((32, 1023))
        })
    );
    recv(&mut c, &p, 3);
    assert_eq!(c.cur_rate().delay, 32);
    assert_eq!(c.cur_rate().size, 1023);
    // After that neither prefix is sent again.
    let p = data(&mut c, 4);
    recv(&mut s, &p, 5);
    let p = data(&mut s, 6);
    let mut r = BitReader::new(&p);
    Header::read(&mut r).expect("header");
    assert_eq!(RatePrefix::read(&mut r), Ok(RatePrefix::default()));
}

#[test]
fn a_clients_current_rate_is_clamped_to_the_servers_maximum() {
    for (delay, size) in [(0, 0), (1, 1), (31, 5), (0, 1023), (5, 63)] {
        let (mut s, _) = pair();
        let bytes = craft(
            true,
            1,
            0,
            PacketType::Data,
            (0, 0),
            Some(RatePrefix {
                cur: Some((delay, size)),
                max: None,
            }),
        );
        assert!(recv(&mut s, &bytes, 1).payload);
        let cur = s.cur_rate();
        assert!(cur.delay >= 32, "delay {delay} gave {}", cur.delay);
        assert!(
            cur.size >= NET_MIN_PACKET_BYTES,
            "size {size} gave {}",
            cur.size
        );
        assert!(cur.size <= 1023);
    }
    // A slower and smaller request is honoured.
    let (mut s, _) = pair();
    let bytes = craft(
        true,
        1,
        0,
        PacketType::Data,
        (0, 0),
        Some(RatePrefix {
            cur: Some((100, 500)),
            max: None,
        }),
    );
    recv(&mut s, &bytes, 1);
    assert_eq!((s.cur_rate().delay, s.cur_rate().size), (100, 500));
    // The client's maximum rate pair is bounded the same way.
    let (mut s, _) = pair();
    let bytes = craft(
        true,
        1,
        0,
        PacketType::Data,
        (0, 0),
        Some(RatePrefix {
            cur: None,
            max: Some((0, 0)),
        }),
    );
    recv(&mut s, &bytes, 1);
    assert!(s.cur_rate().delay >= 32 && s.cur_rate().size >= NET_MIN_PACKET_BYTES);
}

#[test]
fn a_lost_packet_that_carried_a_rate_pair_arms_it_again() {
    let (mut s, mut c) = pair();
    let lost = data(&mut c, 0);
    assert!(!c.max_rate().changed);
    let second = data(&mut c, 1);
    let _ = lost;
    recv(&mut s, &second, 2);
    let reply = data(&mut s, 3);
    let back = recv(&mut c, &reply, 4);
    assert_eq!(
        back.notifies[0],
        Notify {
            send_seq: 1,
            delivered: false
        }
    );
    assert!(c.max_rate().changed, "the lost packet carried the max pair");
    let again = data(&mut c, 5);
    let mut r = BitReader::new(&again);
    Header::read(&mut r).expect("header");
    assert_eq!(
        RatePrefix::read(&mut r).expect("prefix").max,
        Some((8, 1023))
    );
}

#[test]
fn a_data_packet_takes_12000_bits_whatever_the_rate_and_the_rate_is_the_events_size() {
    // The packet stream is built with the rate's size but writes up to 12,000 bits; the size only feeds the event loops.
    let (_, mut c) = pair();
    assert_eq!(c.packet_limit_bits(), 200 * 8);
    let w = c.begin_data_packet(0);
    assert_eq!(w.remaining_bits() + w.bits(), 12_000);
}

// ---- the send gate -------------------------------------------------------------------------------------------------------

#[test]
fn the_gate_waits_one_delay_between_packets_and_carries_credit() {
    let (mut s, _) = pair();
    s.cur.delay = 32;
    // A new connection's last send is time 0: the first packet waits one delay (102 ms before the rates are negotiated).
    let (mut fresh, _) = pair();
    assert!(!fresh.may_send(101, false));
    assert!(fresh.may_send(102, false));
    assert_eq!(fresh.credit, 0);
    let _ = data(&mut s, 0);
    assert!(!s.may_send(31, false));
    assert!(s.may_send(32, false));
    // A late tick stores credit (up to 1000) that lets a later packet go early.
    let _ = data(&mut s, 32);
    assert!(s.may_send(80, false));
    let _ = data(&mut s, 80);
    // 48 ms elapsed against a delay of 32 left 16 ms of credit: the next packet may go 16 ms early.
    assert!(!s.may_send(95, false));
    assert!(s.may_send(96, false));
    // force skips the delay.
    let (mut s2, _) = pair();
    let _ = data(&mut s2, 0);
    assert!(!s2.may_send(1, false));
    assert!(s2.may_send(1, true));
}

/// A pacing made up for the tests: it doubles the delay above a 400 ms round trip and forces the local connection's packets.
#[derive(Debug, Clone, Default)]
struct Doubling {
    calls: u32,
}

impl SendPacing for Doubling {
    fn pace(&mut self, delay: u32, round_trip: f32, local: bool) -> (u32, bool) {
        self.calls += 1;
        let delay = if round_trip > 400.0 { delay * 2 } else { delay };
        (delay, local)
    }

    fn clone_box(&self) -> Box<dyn SendPacing> {
        Box::new(self.clone())
    }
}

#[test]
fn tge_s_gate_waits_the_delay_whatever_the_round_trip() {
    let (mut s, _) = pair();
    s.round_trip = 3000.0;
    let _ = data(&mut s, 0);
    // The current rate's 102 ms until the peer asks for another.
    assert!(!s.may_send(101, false));
    assert!(s.may_send(102, false));
    // A local connection waits as any other.
    let (mut l, _) = pair();
    l.set_local(true);
    let _ = data(&mut l, 0);
    assert!(!l.may_send(1, false));
}

#[test]
fn a_pacing_may_stretch_the_delay_and_force_a_packet() {
    let (mut s, _) = pair();
    s.set_pacing(Some(Box::new(Doubling::default())));
    let _ = data(&mut s, 0);
    s.round_trip = 500.0;
    assert!(!s.may_send(203, false));
    assert!(s.may_send(204, false));
    // The pacing keeps its state, and a copy goes with a clone.
    let copy = s.clone();
    assert!(format!("{:?}", copy.pacing()).contains("calls: 2"));
    s.set_local(true);
    let _ = data(&mut s, 300);
    assert!(s.may_send(301, false), "forced");
}

#[test]
fn a_client_sends_at_the_to_server_delay() {
    let (_, mut c) = pair();
    let _ = data(&mut c, 0);
    // 128 packets a second: 8 ms.
    assert!(!c.may_send(7, false));
    assert!(c.may_send(8, false));
}

// ---- timeouts ------------------------------------------------------------------------------------------------------------

#[test]
fn silence_pings_after_4500_ms_fifteen_times_then_times_out() {
    let (mut s, _) = pair();
    assert_eq!(s.check_timeout(4500), Timeout::Alive);
    let mut now = 4501;
    for i in 0..15 {
        assert_eq!(s.check_timeout(now), Timeout::SendPing, "ping {i}");
        assert_eq!(s.check_timeout(now + 4500), Timeout::Alive);
        now += 4501;
    }
    assert_eq!(s.check_timeout(now), Timeout::TimedOut);
}

#[test]
fn a_valid_packet_resets_the_timeout() {
    let (mut s, mut c) = pair();
    let mut now = 4501;
    for _ in 0..10 {
        assert_eq!(s.check_timeout(now), Timeout::SendPing);
        now += 4501;
    }
    let p = data(&mut c, now);
    recv(&mut s, &p, now);
    assert_eq!(s.check_timeout(now + 4500), Timeout::Alive);
    for _ in 0..15 {
        assert_eq!(s.check_timeout(now + 4501), Timeout::SendPing);
        now += 4501;
    }
    // A dropped packet is not a sign of life.
    let bytes = craft(false, 1, 0, PacketType::Ack, (0, 0), None);
    recv(&mut s, &bytes, now + 1);
    assert_eq!(s.check_timeout(now + 4501), Timeout::TimedOut);
}

#[test]
fn a_ping_asks_for_an_ack_and_the_ack_carries_the_current_sequence() {
    let (mut s, mut c) = pair();
    let ping = c.ping_packet(0);
    let got = recv(&mut s, &ping, 1);
    assert!(got.answer_ping && !got.payload);
    assert_eq!(c.last_send_seq(), 0, "a ping does not take a sequence");
    let ack = s.ack_packet(2);
    let mut r = BitReader::new(&ack);
    let h = Header::read(&mut r).expect("header");
    assert_eq!((h.ptype, h.seq), (PacketType::Ack, 0));
    assert!(recv(&mut c, &ack, 3).dropped.is_none());
}

#[test]
fn the_ack_byte_count_follows_what_the_peer_has_not_yet_acknowledged() {
    let (mut s, mut c) = pair();
    // Server receives 20 packets: its next header has to carry 3 mask bytes (20 bits).
    for i in 0..20 {
        let p = data(&mut c, i);
        recv(&mut s, &p, i);
    }
    let out = data(&mut s, 30);
    let h = Header::read(&mut BitReader::new(&out)).expect("header");
    assert_eq!(h.ack, 20);
    assert_eq!(h.ack_bytes, 3);
    assert_eq!(h.ack_mask, 0x000f_ffff);
    // The client acks that; the server, hearing it, needs 0 bytes until new packets arrive.
    recv(&mut c, &out, 31);
    let again = data(&mut c, 32);
    recv(&mut s, &again, 33);
    let next = data(&mut s, 34);
    let h = Header::read(&mut BitReader::new(&next)).expect("header");
    assert!(h.ack_bytes <= 1, "got {}", h.ack_bytes);
    // Several packets in a row never need more than 4 bytes.
    for i in 0..8 {
        let p = data(&mut c, 40 + i);
        recv(&mut s, &p, 40 + i);
    }
    let out = data(&mut s, 80);
    let h = Header::read(&mut BitReader::new(&out)).expect("header");
    assert!(h.ack_bytes <= 4);
}

#[test]
fn reader_at_starts_where_it_is_told() {
    let bytes = [
        0xff, 0x0f, 0xa5, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34,
    ];
    let mut r = reader_at(&bytes, 12);
    assert_eq!(r.read_bits(4), Ok(0));
    let mut r = reader_at(&bytes, 80);
    assert_eq!(r.read_u8(), Ok(0x34));
    assert_eq!(reader_at(&bytes, 10_000).bits_left(), 0);
}

#[test]
fn more_than_32_packets_between_our_sends_clamp_the_ack_history_to_four_bytes() {
    let (mut a, _) = pair();
    for seq in 1..=40u16 {
        let bytes = craft(
            true,
            seq,
            0,
            PacketType::Data,
            (0, 0),
            Some(RatePrefix::default()),
        );
        assert!(recv(&mut a, &bytes, 1).payload);
    }
    assert_eq!(a.last_seq_recvd(), 40);
    assert_eq!(a.last_recv_ack_ack, 8, "clamped to seq - 32");
    let ack = a.ack_packet(2);
    let h = Header::read(&mut BitReader::new(&ack)).expect("header");
    assert_eq!((h.ack, h.ack_bytes, h.ack_mask), (40, 4, u32::MAX));

    // The byte count is also capped on its own: with an ack-ack far behind, 5 or more bytes are never asked for.
    a.last_recv_ack_ack = 0;
    let ack = a.ack_packet(3);
    let h = Header::read(&mut BitReader::new(&ack)).expect("header");
    assert_eq!(h.ack_bytes, 4);
    assert_eq!(h.ack_mask, u32::MAX);
}

#[test]
fn the_current_rate_pair_is_armed_again_when_its_packet_is_lost() {
    let (mut s, mut c) = pair();
    let p = data(&mut c, 0);
    recv(&mut s, &p, 1);
    assert!(s.cur_rate().changed);
    let lost = data(&mut s, 2);
    let mut r = BitReader::new(&lost);
    Header::read(&mut r).expect("header");
    assert_eq!(
        RatePrefix::read(&mut r).expect("prefix").cur,
        Some((32, 1023))
    );
    assert!(!s.cur_rate().changed);
    // The next packet carries no current pair; the client hears that one only.
    let second = data(&mut s, 3);
    let mut r = BitReader::new(&second);
    Header::read(&mut r).expect("header");
    assert_eq!(RatePrefix::read(&mut r).expect("prefix").cur, None);
    recv(&mut c, &second, 4);
    let reply = data(&mut c, 5);
    let back = recv(&mut s, &reply, 6);
    assert_eq!(
        back.notifies[0],
        Notify {
            send_seq: 1,
            delivered: false
        }
    );
    assert!(
        s.cur_rate().changed,
        "the lost packet carried the current pair"
    );
    let again = data(&mut s, 7);
    let mut r = BitReader::new(&again);
    Header::read(&mut r).expect("header");
    assert_eq!(
        RatePrefix::read(&mut r).expect("prefix").cur,
        Some((32, 1023))
    );
}

#[test]
fn a_max_pair_marks_the_current_rate_changed_only_when_it_changes_it() {
    let (mut s, _) = pair();
    let max_pair = |seq: u16, pair: (u32, u32)| {
        craft(
            true,
            seq,
            0,
            PacketType::Data,
            (0, 0),
            Some(RatePrefix {
                cur: None,
                max: Some(pair),
            }),
        )
    };
    // 102/200 becomes 32/1023: changed, and sending it clears the flag.
    recv(&mut s, &max_pair(1, (8, 1023)), 1);
    assert!(s.cur_rate().changed);
    let _ = data(&mut s, 2);
    assert!(!s.cur_rate().changed);
    // The same maximum again leaves the rate where it is: nothing to tell the peer.
    recv(&mut s, &max_pair(2, (8, 1023)), 3);
    assert_eq!((s.cur_rate().delay, s.cur_rate().size), (32, 1023));
    assert!(!s.cur_rate().changed);
    // A slower or smaller one does change it.
    recv(&mut s, &max_pair(3, (50, 1023)), 4);
    assert_eq!(s.cur_rate().delay, 50);
    assert!(s.cur_rate().changed);
}

#[test]
fn a_packet_size_asked_past_the_streams_bound_is_held_to_it() {
    let mut p = Protocol::new(Role::Server, 0, RateConfig::standard(), 0);
    assert_eq!(p.packet_limit_bits(), 200 * 8);
    p.set_packet_bytes(Some(1_400));
    assert_eq!(p.packet_limit_bits(), 1_400 * 8);
    // A network packet never outgrows the datagram a standard peer reads.
    p.set_packet_bytes(Some(64_000));
    assert_eq!(p.packet_limit_bits(), crate::cap::NET_MAX_DATAGRAM * 8);
    p.set_packet_bytes(None);
    assert_eq!(p.packet_limit_bits(), 200 * 8);
    // A connection in memory may fill larger packets, up to its own bound.
    p.set_in_memory(true);
    p.set_packet_bytes(Some(4_096));
    assert_eq!(p.packet_limit_bits(), 4_096 * 8);
    p.set_packet_bytes(Some(u32::MAX));
    assert_eq!(
        p.packet_limit_bits(),
        MAX_IN_MEMORY_PACKET_BYTES as usize * 8
    );
}

#[test]
fn a_standard_wan_server_sends_at_its_own_rate_and_the_client_sees_what_it_said() {
    // The client asks for the most (every 8 ms, 1,023 bytes); a server at the standard 10 a second sends at the slower of the
    // two: every 102 ms, at 1,023 bytes.
    let rates = RateConfig {
        packet_size: MAX_PACKET_BYTES,
        ..RateConfig::standard()
    };
    let mut s = Protocol::new(Role::Server, 7, rates, 0);
    let mut c = Protocol::new(Role::Client, 7, fast(128), 0);
    assert_eq!((c.max_rate().delay, c.max_rate().size), (8, 1023));
    let p = data(&mut c, 0);
    recv(&mut s, &p, 1);
    assert_eq!((s.cur_rate().delay, s.cur_rate().size), (102, 1023));
    assert_eq!(s.peer_max_rate(), Some((8, 1023)));
    assert_eq!(c.peer_cur_rate(), None);
    let p = data(&mut s, 200);
    recv(&mut c, &p, 201);
    assert_eq!(c.peer_cur_rate(), Some((102, 1023)));
    assert_eq!(c.peer_max_rate(), Some((102, 1023)));
    assert_eq!((c.cur_rate().delay, c.cur_rate().size), (102, 1023));
}
