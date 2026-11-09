use super::*;

fn limits() -> Vec<ChannelLimits> {
    vec![
        ChannelLimits::new(200).with_max_message(10_000),
        ChannelLimits::new(100).with_max_message(1_000),
    ]
}

/// One packet from `tx` read by `rx`, delivered.
fn hop(tx: &mut ChunkSender, rx: &mut ChunkReceiver, seq: u32, room: usize) -> Vec<Delivered> {
    let mut w = BitWriter::new();
    tx.write(&mut w, seq, room);
    let bytes = w.into_bytes();
    let mut out = Vec::new();
    rx.read(&mut BitReader::new(&bytes), &mut out)
        .expect("a valid section");
    tx.on_notify(seq, true);
    out
}

#[test]
fn an_idle_sender_writes_one_clear_flag() {
    let mut tx = ChunkSender::new(&limits());
    let mut w = BitWriter::new();
    assert_eq!(tx.write(&mut w, 1, 4_000), 0);
    assert_eq!(w.bits(), 1);
    assert!(tx.is_idle() && !tx.has_work());
}

#[test]
fn a_message_goes_in_chunks_under_the_budget_and_arrives_whole() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    let data: Vec<u8> = (0..9_000u32).map(|i| (i * 7) as u8).collect();
    tx.enqueue(0, Kind::Snapshot, data.clone()).unwrap();
    let mut got = Vec::new();
    let mut seq = 0;
    while !tx.is_idle() {
        seq += 1;
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, 8_000);
        assert!(
            w.bits() <= 200 * 8 + 1,
            "one channel's budget: {}",
            w.bits()
        );
        let bytes = w.into_bytes();
        rx.read(&mut BitReader::new(&bytes), &mut got).unwrap();
        tx.on_notify(seq, true);
    }
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].channel, got[0].kind), (0, Kind::Snapshot));
    assert_eq!(got[0].data, data);
    assert!(seq > 40, "about 9000 / 190 packets, was {seq}");
    assert_eq!(rx.held_bytes(), 0);
}

#[test]
fn messages_arrive_in_the_order_queued_and_channels_share_a_packet() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    tx.enqueue(0, Kind::Snapshot, vec![1; 50]).unwrap();
    tx.enqueue(0, Kind::Delta, vec![2; 50]).unwrap();
    tx.enqueue(1, Kind::Delta, vec![3; 50]).unwrap();
    let got = hop(&mut tx, &mut rx, 1, 8_000);
    // One packet held all three (two channels).
    assert_eq!(got.len(), 3);
    let on0: Vec<_> = got.iter().filter(|d| d.channel == 0).collect();
    assert_eq!((on0[0].kind, on0[0].data[0]), (Kind::Snapshot, 1));
    assert_eq!((on0[1].kind, on0[1].data[0]), (Kind::Delta, 2));
    assert!(tx.is_idle());
}

#[test]
fn a_lost_packet_sends_its_chunks_again() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    let data: Vec<u8> = (0..500u32).map(|i| i as u8).collect();
    tx.enqueue(0, Kind::Snapshot, data.clone()).unwrap();
    let mut got = Vec::new();
    let mut seq = 0;
    while !tx.is_idle() && seq < 100 {
        seq += 1;
        let mut w = BitWriter::new();
        tx.write(&mut w, seq, 8_000);
        let lose = seq % 2 == 1;
        if !lose {
            let bytes = w.into_bytes();
            rx.read(&mut BitReader::new(&bytes), &mut got).unwrap();
        }
        tx.on_notify(seq, !lose);
    }
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].data, data);
}

#[test]
fn the_sender_refuses_what_is_too_big_or_too_much_and_queues_nothing() {
    let mut tx = ChunkSender::new(&limits());
    assert_eq!(
        tx.enqueue(5, Kind::Delta, vec![1]),
        Err(SendError::UnknownChannel)
    );
    assert_eq!(tx.enqueue(1, Kind::Delta, vec![]), Err(SendError::Empty));
    assert_eq!(
        tx.enqueue(1, Kind::Delta, vec![0; 1_001]),
        Err(SendError::TooLarge {
            len: 1_001,
            cap: 1_000
        })
    );
    for _ in 0..2 {
        tx.enqueue(1, Kind::Delta, vec![0; 1_000]).unwrap();
    }
    // The backlog cap is twice the message cap.
    assert_eq!(
        tx.enqueue(1, Kind::Delta, vec![0; 1]),
        Err(SendError::Backlog)
    );
    assert_eq!((tx.backlog(1), tx.queued(1)), (2_000, 2));
    assert_eq!(tx.backlog(0), 0);
}

#[test]
fn unstarted_messages_can_be_dropped_for_a_newer_snapshot_without_a_gap() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    tx.enqueue(1, Kind::Snapshot, vec![1; 400]).unwrap();
    tx.enqueue(1, Kind::Delta, vec![2; 400]).unwrap();
    // The first message starts; the second has not.
    let mut got = hop(&mut tx, &mut rx, 1, 8_000);
    assert_eq!(tx.drop_unstarted(1), 400);
    tx.enqueue(1, Kind::Snapshot, vec![3; 100]).unwrap();
    let mut seq = 1;
    while !tx.is_idle() {
        seq += 1;
        got.extend(hop(&mut tx, &mut rx, seq, 8_000));
    }
    let firsts: Vec<_> = got.iter().map(|d| (d.kind, d.data[0])).collect();
    assert_eq!(firsts, vec![(Kind::Snapshot, 1), (Kind::Snapshot, 3)]);
}

#[test]
fn a_chunk_sent_again_after_it_arrived_is_ignored() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    tx.enqueue(1, Kind::Delta, vec![9; 50]).unwrap();
    let mut w = BitWriter::new();
    tx.write(&mut w, 1, 8_000);
    let bytes = w.into_bytes();
    let mut out = Vec::new();
    rx.read(&mut BitReader::new(&bytes), &mut out).unwrap();
    rx.read(&mut BitReader::new(&bytes), &mut out).unwrap();
    assert_eq!(out.len(), 1);
}

fn section(f: impl FnOnce(&mut BitWriter)) -> Vec<u8> {
    let mut w = BitWriter::new();
    f(&mut w);
    w.into_bytes()
}

/// A section with one chunk of channel `channel` of the test limits, written field by field.
fn chunk(channel: u32, seq: u32, total: u32, offset: u32, len: u32, payload: usize) -> Vec<u8> {
    let l = limits();
    section(|w| {
        w.write_flag(true);
        w.write_bits(channel, id_bits(l.len()));
        w.write_flag(false);
        w.write_bits(seq, 16);
        w.write_bits(total, l[1].size_bits());
        w.write_bits(offset, l[1].size_bits());
        w.write_bits(len, CHUNK_LEN_BITS);
        w.write_bytes(&vec![0; payload]);
        w.write_flag(false);
    })
}

#[test]
fn a_bad_chunk_is_an_error_not_a_panic() {
    let read = |bytes: &[u8]| {
        let mut rx = ChunkReceiver::new(&limits());
        rx.read(&mut BitReader::new(bytes), &mut Vec::new())
    };
    assert!(read(&chunk(1, 0, 10, 0, 10, 10)).is_ok());
    // Past the message cap, a zero length, a range past the end, bytes the packet does not have, a sequence number past the
    // window, and a section with nothing in it.
    assert!(matches!(
        read(&chunk(1, 0, 1_001, 0, 5, 5)),
        Err(NetError::Cap { .. })
    ));
    assert!(read(&chunk(1, 0, 0, 0, 5, 5)).is_err());
    assert!(read(&chunk(1, 0, 10, 0, 0, 0)).is_err());
    assert!(read(&chunk(1, 0, 10, 8, 5, 5)).is_err());
    assert!(read(&chunk(1, 0, 10, 0, 10, 4)).is_err());
    assert!(matches!(
        read(&chunk(1, 9, 10, 0, 10, 10)),
        Err(NetError::Cap { .. })
    ));
    assert!(read(&[]).is_err());
}

#[test]
fn a_channel_id_that_does_not_exist_is_refused() {
    // Three channels take two id bits, so id 3 fits the field and names nothing.
    let three = vec![ChannelLimits::new(100).with_max_message(100); 3];
    let bytes = section(|w| {
        w.write_flag(true);
        w.write_bits(3, id_bits(three.len()));
        w.write_flag(false);
        w.write_bits(0, 16);
        w.write_bits(1, three[0].size_bits());
        w.write_bits(0, three[0].size_bits());
        w.write_bits(1, CHUNK_LEN_BITS);
        w.write_u8(0);
        w.write_flag(false);
    });
    let mut rx = ChunkReceiver::new(&three);
    assert_eq!(
        rx.read(&mut BitReader::new(&bytes), &mut Vec::new()),
        Err(NetError::Invalid("channel id"))
    );
}

#[test]
fn a_chunk_that_changes_its_messages_length_is_refused() {
    let l = limits();
    let mut tx = ChunkSender::new(&l);
    tx.enqueue(1, Kind::Snapshot, vec![1; 300]).unwrap();
    let mut w = BitWriter::new();
    tx.write(&mut w, 1, 8_000);
    let first = w.into_bytes();
    let mut rx = ChunkReceiver::new(&l);
    rx.read(&mut BitReader::new(&first), &mut Vec::new())
        .unwrap();
    // The same message again, but claiming another length.
    let lying = chunk(1, 0, 301, 0, 1, 1);
    assert!(
        rx.read(&mut BitReader::new(&lying), &mut Vec::new())
            .is_err()
    );
}

#[test]
fn a_chunk_at_the_end_of_a_long_message_cannot_make_the_receiver_hold_more_than_its_cap() {
    let l = vec![ChannelLimits::new(100).with_max_message(1 << 20); 3];
    let size_bits = l[0].size_bits();
    // One byte at the last offset of a 1 MiB message makes its buffer 1 MiB at once.
    let far = |channel: u32, seq: u32| {
        section(|w| {
            w.write_flag(true);
            w.write_bits(channel, id_bits(l.len()));
            w.write_flag(false);
            w.write_bits(seq, 16);
            w.write_bits(1 << 20, size_bits);
            w.write_bits((1 << 20) - 1, size_bits);
            w.write_bits(1, CHUNK_LEN_BITS);
            w.write_u8(1);
            w.write_flag(false);
        })
    };
    let mut rx = ChunkReceiver::new(&l).with_hold_cap(2_500_000);
    let mut ok = 0;
    let mut refused = 0;
    for (channel, seq) in [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)] {
        match rx.read(&mut BitReader::new(&far(channel, seq)), &mut Vec::new()) {
            Ok(()) => ok += 1,
            Err(NetError::Cap { .. }) => refused += 1,
            Err(e) => panic!("{e}"),
        }
        assert!(rx.held_bytes() <= 2_500_000, "held {}", rx.held_bytes());
    }
    assert_eq!((ok, refused), (2, 4));
    // The default cap holds 8 MiB across channels and window.
    let mut rx = ChunkReceiver::new(&l);
    for (channel, seq) in [
        (0, 0),
        (0, 1),
        (0, 2),
        (0, 3),
        (1, 0),
        (1, 1),
        (1, 2),
        (1, 3),
        (2, 0),
    ] {
        let _ = rx.read(&mut BitReader::new(&far(channel, seq)), &mut Vec::new());
        assert!(rx.held_bytes() <= DEFAULT_HOLD_CAP);
    }
}

#[test]
fn the_sender_caps_the_bytes_queued_across_all_channels() {
    let l = vec![ChannelLimits::new(100).with_max_message(10_000); 3];
    let mut tx = ChunkSender::new(&l).with_total_backlog(25_000);
    tx.enqueue(0, Kind::Delta, vec![1; 10_000]).unwrap();
    tx.enqueue(1, Kind::Delta, vec![1; 10_000]).unwrap();
    assert_eq!(
        tx.enqueue(2, Kind::Delta, vec![1; 6_000]),
        Err(SendError::Backlog)
    );
    tx.enqueue(2, Kind::Delta, vec![1; 5_000]).unwrap();
    assert_eq!(tx.total_backlog(), 25_000);
    assert_eq!(
        tx.enqueue(0, Kind::Delta, vec![1; 1]),
        Err(SendError::Backlog)
    );
}

#[test]
fn a_handle_says_whether_its_message_is_still_waiting_for_acknowledgement() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    let first = tx
        .enqueue_shared_with_handle(0, Kind::Snapshot, Arc::from(vec![1u8; 900]))
        .unwrap();
    let second = tx
        .enqueue_shared_with_handle(0, Kind::Delta, Arc::from(vec![2u8; 300]))
        .unwrap();
    assert_ne!(first, second);
    assert_eq!((tx.queued_len(first), tx.queued_len(second)), (900, 300));
    // Part of it acknowledged: it still counts whole, as the backlog does.
    hop(&mut tx, &mut rx, 1, 8_000);
    assert_eq!(tx.queued_len(first), 900);
    assert_eq!(tx.queued_len(first) + tx.queued_len(second), tx.backlog(0));
    let mut seq = 1;
    while tx.queued_len(first) > 0 {
        seq += 1;
        hop(&mut tx, &mut rx, seq, 8_000);
    }
    assert_eq!(tx.queued_len(first), 0);
    // Sequence numbers wrap; a handle never names a later message.
    for _ in 0..70_000 {
        let h = tx
            .enqueue_shared_with_handle(1, Kind::Delta, Arc::from(vec![3u8]))
            .unwrap();
        assert_eq!(tx.queued_len(h), 1);
        tx.drop_unstarted(1);
        assert_eq!(tx.queued_len(h), 0);
    }
    assert_eq!(tx.queued_len(first), 0);
    // `enqueue_shared` still queues without a handle.
    tx.enqueue_shared(1, Kind::Delta, Arc::from(vec![4u8; 10]))
        .unwrap();
    assert_eq!(tx.backlog(1), 10);
}

#[test]
fn a_priority_channel_is_written_first_whatever_channel_has_the_turn() {
    let limits = vec![
        ChannelLimits::new(200).with_max_message(10_000),
        ChannelLimits::new(100)
            .with_max_message(1_000)
            .with_priority(),
        ChannelLimits::new(200).with_max_message(10_000),
    ];
    let (mut tx, mut rx) = (ChunkSender::new(&limits), ChunkReceiver::new(&limits));
    // The bulk channels have more than a packet's room; the priority one has a small message.
    tx.enqueue(0, Kind::Snapshot, vec![1; 9_000]).unwrap();
    tx.enqueue(2, Kind::Snapshot, vec![2; 9_000]).unwrap();
    for round in 0..6 {
        tx.enqueue(1, Kind::Delta, vec![9; 40]).unwrap();
        let mut w = BitWriter::new();
        // A packet with room for one bulk channel's chunk and a little more.
        tx.write(&mut w, round + 1, 240 * 8);
        let bytes = w.into_bytes();
        let mut out = Vec::new();
        rx.read(&mut BitReader::new(&bytes), &mut out).unwrap();
        assert!(
            out.iter().any(|d| d.channel == 1 && d.data.len() == 40),
            "the priority message was in packet {round}"
        );
        tx.on_notify(round + 1, true);
    }
}

#[test]
fn a_priority_channel_is_not_held_up_by_the_budget_the_bulk_channels_share() {
    let limits = vec![
        ChannelLimits::new(100).with_max_message(1_000),
        ChannelLimits::new(100)
            .with_max_message(1_000)
            .with_max_backlog(1_000)
            .with_priority(),
    ];
    let mut tx = ChunkSender::new(&limits).with_total_backlog(2_000);
    tx.enqueue(0, Kind::Snapshot, vec![1; 1_000]).unwrap();
    tx.enqueue(0, Kind::Snapshot, vec![1; 1_000]).unwrap();
    assert_eq!(
        tx.enqueue(0, Kind::Snapshot, vec![1; 10]),
        Err(SendError::Backlog)
    );
    // The shared budget is full, and the priority channel still takes its message, up to its own backlog.
    assert_eq!(tx.enqueue(1, Kind::Delta, vec![2; 500]), Ok(()));
    assert_eq!(tx.enqueue(1, Kind::Delta, vec![2; 500]), Ok(()));
    assert_eq!(
        tx.enqueue(1, Kind::Delta, vec![2; 500]),
        Err(SendError::Backlog)
    );
}

#[test]
fn a_receiver_holds_what_an_honest_sender_can_queue_priority_channels_included() {
    let limits = vec![
        ChannelLimits::new(100).with_max_message(1_000_000),
        ChannelLimits::new(100).with_max_message(1_000_000),
        ChannelLimits::new(100)
            .with_max_message(16_000)
            .with_max_backlog(65_536)
            .with_priority(),
    ];
    assert_eq!(priority_allowance(&limits), 65_536);
    let mut tx = ChunkSender::new(&limits);
    // Fill every channel as far as it goes.
    for (channel, size) in [(0, 1_000_000), (1, 1_000_000), (2, 16_000)] {
        while tx.enqueue(channel, Kind::Snapshot, vec![1; size]).is_ok() {}
    }
    let rx = ChunkReceiver::new(&limits);
    assert!(
        tx.total_backlog() <= rx.hold_cap,
        "an honest sender queued {} bytes against a hold cap of {}",
        tx.total_backlog(),
        rx.hold_cap
    );
    assert_eq!(rx.hold_cap, shared_hold_cap(&limits) + 65_536);
}

#[test]
fn bytes_sent_again_are_not_fresh() {
    let (mut tx, mut rx) = (ChunkSender::new(&limits()), ChunkReceiver::new(&limits()));
    tx.enqueue(0, Kind::Snapshot, vec![7u8; 6_000]).unwrap();
    // One packet's section, written once and read again and again: its bytes are fresh the first time only.
    let mut w = BitWriter::new();
    tx.write(&mut w, 1, 1_600);
    let packet = w.into_bytes();
    let mut out = Vec::new();
    rx.read(&mut BitReader::new(&packet), &mut out).unwrap();
    let fresh = rx.fresh_bytes();
    assert!(fresh > 0 && fresh == rx.received_bytes());
    for _ in 0..10 {
        rx.read(&mut BitReader::new(&packet), &mut out).unwrap();
    }
    assert_eq!(rx.fresh_bytes(), fresh, "a replay adds nothing");
    assert_eq!(
        rx.received_bytes(),
        11 * fresh,
        "the cap's count still sees it all"
    );
    // The rest of the message is fresh, and when it is whole a replay of any chunk of it is not.
    tx.on_notify(1, true);
    let mut seq = 1;
    while !tx.is_idle() {
        seq += 1;
        out.extend(hop(&mut tx, &mut rx, seq, 1_600));
    }
    assert_eq!(out.len(), 1);
    assert_eq!(rx.fresh_bytes(), 6_000);
    rx.read(&mut BitReader::new(&packet), &mut Vec::new())
        .unwrap();
    assert_eq!(rx.fresh_bytes(), 6_000);
}
