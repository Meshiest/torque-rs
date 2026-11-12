use super::*;
use crate::events::free_charge;
use torque_testkit::Rng;

fn mv(seed: u64) -> Move {
    let mut rng = Rng::star(seed | 1);
    let mut angles = [0u32; 3];
    for a in &mut angles {
        *a = if rng.index(2) == 0 {
            0
        } else {
            rng.index(0x1_0000) as u32
        };
    }
    Move::from_raw(
        [
            rng.index(33) as u32,
            rng.index(33) as u32,
            rng.index(33) as u32,
        ],
        angles,
        rng.index(2) == 1,
        std::array::from_fn(|_| rng.index(2) == 1),
    )
}

fn block_bytes(b: &MoveBlock) -> (Vec<u8>, usize) {
    let mut w = BitWriter::new();
    b.write(&mut w);
    let bits = w.bits();
    (w.into_bytes(), bits)
}

#[test]
fn a_games_move_format_carries_fewer_triggers() {
    let five = MoveFormat { trigger_keys: 5 };
    assert_eq!(MoveFormat::default().triggers(), MAX_TRIGGER_KEYS);
    assert_eq!(
        (five.min_bits(), five.max_bits()),
        (MOVE_MIN_BITS - 1, MOVE_MAX_BITS - 1)
    );
    let m = Move::from_raw([16; 3], [0, 0, 0], true, [true; MAX_TRIGGER_KEYS]);
    let mut w = BitWriter::new();
    m.pack_with(&mut w, five);
    assert_eq!(w.bits(), five.min_bits());
    let mut r = BitReader::new(w.bytes());
    let back = Move::unpack_with(&mut r, five).unwrap();
    assert_eq!(back.triggers, [true, true, true, true, true, false]);
    // A list reads its blocks in its own format.
    let block = MoveBlock {
        start: 0,
        moves: vec![m; 3],
    };
    let mut w = BitWriter::new();
    block.write_with(&mut w, five);
    let mut list = MoveList::with_format(five);
    let mut r = BitReader::new(w.bytes());
    assert_eq!(
        list.read_block(&mut r, &mut free_charge())
            .unwrap()
            .appended,
        3
    );
    assert!(r.bits_left() < 8);
}

#[test]
fn axes_are_clamped_to_32_on_unpack_and_convert_to_sixteenths() {
    for (raw, want_raw, want) in [
        (0u32, 0u32, -1.0f32),
        (16, 16, 0.0),
        (31, 31, 0.9375),
        (32, 32, 1.0),
        (33, 32, 1.0),
        (63, 32, 1.0),
    ] {
        let mut w = BitWriter::new();
        for _ in 0..3 {
            w.write_flag(false);
        }
        for _ in 0..3 {
            w.write_bits(raw, 6);
        }
        w.write_bits(0, 6);
        let mut r = BitReader::new(w.bytes());
        let m = Move::unpack(&mut r).unwrap();
        assert_eq!((m.px, m.py, m.pz), (want_raw, want_raw, want_raw), "{raw}");
        assert_eq!((m.x, m.y, m.z), (want, want, want), "{raw}");
    }
}

#[test]
fn a_move_round_trips_in_28_to_76_bits() {
    let mut rng = Rng::star(7);
    let mut sizes = std::collections::BTreeSet::new();
    for i in 0..2000 {
        let m = mv(rng.next_u64() ^ i);
        let mut w = BitWriter::new();
        m.pack(&mut w);
        assert!((MOVE_MIN_BITS..=MOVE_MAX_BITS).contains(&w.bits()));
        sizes.insert(w.bits());
        let mut r = BitReader::new(w.bytes());
        let back = Move::unpack(&mut r).unwrap();
        assert_eq!(back, m);
        assert!(r.bits_left() < 8);
    }
    assert!(sizes.contains(&MOVE_MIN_BITS) || sizes.contains(&(MOVE_MIN_BITS + 16)));
    // An angle of zero costs one bit, a non-zero one 17.
    let mut w = BitWriter::new();
    Move::from_raw([16; 3], [0, 0, 1], false, [false; MAX_TRIGGER_KEYS]).pack(&mut w);
    assert_eq!(w.bits(), 1 + 1 + 17 + 18 + 1 + MAX_TRIGGER_KEYS);
}

#[test]
fn a_block_writes_at_most_31_moves_and_reads_back_whole() {
    let moves: Vec<Move> = (0..40).map(mv).collect();
    let b = MoveBlock { start: 9, moves };
    let (bytes, bits) = block_bytes(&b);
    let mut r = BitReader::with_bit_len(&bytes, bits);
    let back = MoveBlock::read(&mut r, &mut free_charge()).unwrap();
    assert_eq!(back.start, 9);
    assert_eq!(back.moves.len(), 31);
    assert_eq!(&back.moves[..], &b.moves[..31]);
    // Every truncation is an error.
    for cut in 0..bits {
        let mut r = BitReader::with_bit_len(&bytes, cut);
        assert!(
            MoveBlock::read(&mut r, &mut free_charge()).is_err(),
            "{cut}"
        );
    }
    // A budget that says no stops the read.
    let mut r = BitReader::with_bit_len(&bytes, bits);
    assert_eq!(
        MoveBlock::read(&mut r, &mut |_| false),
        Err(NetError::Invalid("work budget"))
    );
}

fn block(start: u32, n: usize) -> MoveBlock {
    MoveBlock {
        start,
        moves: (0..n)
            .map(|i| mv(1000 + u64::from(start) + i as u64))
            .collect(),
    }
}

#[test]
fn the_reader_skips_acknowledged_moves_and_appends_the_rest() {
    let mut l = MoveList::new();
    // Three moves from 0.
    let a = l.absorb(&block(0, 3));
    assert_eq!(
        a,
        Absorbed {
            appended: 3,
            ..Absorbed::default()
        }
    );
    assert_eq!(l.last_move_ack(), 3);
    assert_eq!(l.len(), 3);
    assert_eq!(l.ack(), 0);
    let ids: Vec<u32> = l.moves().map(|m| m.id).collect();
    assert_eq!(ids, [0, 1, 2]);
    // A resend from 1 with 4 moves: 2 are old (1, 2), 2 are new (3, 4).
    let a = l.absorb(&block(1, 4));
    assert_eq!((a.skipped, a.appended, a.jumped), (2, 2, false));
    assert_eq!(l.last_move_ack(), 5);
    let ids: Vec<u32> = l.moves().map(|m| m.id).collect();
    assert_eq!(ids, [0, 1, 2, 3, 4]);
    // A block entirely in the past adds nothing.
    let a = l.absorb(&block(0, 3));
    assert_eq!((a.skipped, a.appended), (3, 0));
    assert_eq!(l.last_move_ack(), 5);
    // The tick takes them.
    let mut credit = 30;
    let taken = l.take_for_tick(&mut credit);
    assert_eq!((taken.len(), credit), (5, 25));
    l.clear_moves(5);
    assert_eq!(l.ack(), 5);
}

#[test]
fn a_start_ahead_of_the_ack_jumps_it_and_the_compare_is_signed_and_wrapping() {
    let mut l = MoveList::new();
    let a = l.absorb(&block(100, 2));
    assert!(a.jumped);
    assert_eq!(l.last_move_ack(), 102);
    // The compare is on the wrapped difference as a signed number: from 0, a start of 0xfffffffe is 2 BEHIND (skip 2), not ahead.
    let mut l = MoveList::new();
    let a = l.absorb(&block(0xffff_fffe, 4));
    assert_eq!((a.skipped, a.appended, a.jumped), (2, 2, false));
    let ids: Vec<u32> = l.moves().map(|m| m.id).collect();
    assert_eq!(ids, [0, 1]);
    // 0x80000000 and beyond is "ahead": a jump.
    let mut l = MoveList::new();
    assert!(l.absorb(&block(0x7fff_fff0, 2)).jumped);
    assert_eq!(l.last_move_ack(), 0x7fff_fff2);
    assert!(l.absorb(&block(0xffff_fff0, 2)).jumped);
    assert_eq!(l.last_move_ack(), 0xffff_fff2);
    l.clear_moves(2);
    // A resend that runs over the wrap is ordinary: one skipped, nineteen appended, ids through 0xffffffff and on to 0..=5.
    let a = l.absorb(&block(0xffff_fff1, 20));
    assert_eq!((a.skipped, a.appended, a.jumped), (1, 19, false));
    assert_eq!(l.last_move_ack(), 5);
    let ids: Vec<u32> = l.moves().map(|m| m.id).collect();
    assert_eq!(ids.len(), 21);
    assert_eq!(ids[..2], [0xffff_fff0, 0xffff_fff1]);
    assert_eq!(ids[2..4], [0xffff_fff2, 0xffff_fff3]);
    assert_eq!(ids[ids.len() - 6..], [0xffff_ffff, 0, 1, 2, 3, 4]);
    assert_eq!(l.ack(), 5u32.wrapping_sub(21));
}

#[test]
fn the_list_is_bounded_and_the_newest_moves_are_dropped() {
    let mut l = MoveList::new();
    let mut start = 0;
    let mut dropped = 0;
    for _ in 0..10 {
        let a = l.absorb(&block(start, 31));
        dropped += a.dropped;
        start += 31;
    }
    assert_eq!(l.len(), NET_MOVE_LIST);
    assert_eq!(dropped, 310 - NET_MOVE_LIST);
    // The oldest are the ones kept.
    assert_eq!(l.moves().next().map(|m| m.id), Some(0));
    // lastMoveAck advanced over all of them. An unbounded list would hold all 310 until the tick, so its ack would still read 0
    // here; this one reads 310 - 64 because the dropped moves are not in the list. Only a client that overruns the bound sees it.
    assert_eq!(l.last_move_ack(), 310);
    assert_eq!(l.ack(), 310 - NET_MOVE_LIST as u32);
}

#[test]
fn the_tick_takes_at_most_the_credit_and_loses_the_excess() {
    let mut l = MoveList::new();
    l.absorb(&block(0, 31));
    l.absorb(&block(31, 9));
    assert_eq!(l.len(), 40);
    let mut credit = 30;
    let taken = l.take_for_tick(&mut credit);
    assert_eq!((taken.len(), credit, l.len()), (30, 0, 30));
    // With no credit nothing is taken and the list is emptied.
    let taken = l.take_for_tick(&mut credit);
    assert!(taken.is_empty());
    assert!(l.is_empty());
    // clear_moves drops from the front.
    l.absorb(&block(40, 5));
    l.clear_moves(2);
    assert_eq!(l.moves().next().map(|m| m.id), Some(42));
    l.clear_moves(100);
    assert!(l.is_empty());
}

#[test]
fn an_overrunning_block_changes_nothing() {
    let b = block(0, 5);
    let (bytes, bits) = block_bytes(&b);
    let mut l = MoveList::new();
    let mut r = BitReader::with_bit_len(&bytes, bits - 1);
    assert!(l.read_block(&mut r, &mut free_charge()).is_err());
    assert_eq!((l.len(), l.last_move_ack()), (0, 0));
    let mut r = BitReader::with_bit_len(&bytes, bits);
    assert_eq!(
        l.read_block(&mut r, &mut free_charge()).unwrap().appended,
        5
    );
}

#[test]
fn the_client_queue_resends_each_move_four_times_then_only_the_last() {
    let mut q = MoveQueue::new();
    for i in 0..5 {
        assert!(q.push(mv(i)));
    }
    for round in 0..4 {
        let b = q.block();
        assert_eq!((b.start, b.moves.len()), (0, 5), "round {round}");
        assert_eq!(b.moves[0].send_count, round + 1);
    }
    // Every move has been sent four times: the last one goes on being sent, alone.
    let b = q.block();
    assert_eq!((b.start, b.moves.len()), (4, 1));
    // A new move is sent with it.
    q.push(mv(9));
    let b = q.block();
    assert_eq!((b.start, b.moves.len()), (5, 1));
    // The server's ack drops what it has.
    q.on_server_ack(3);
    assert_eq!((q.first_index(), q.len()), (3, 3));
    q.on_server_ack(2);
    assert_eq!(q.first_index(), 3);
    q.on_server_ack(99);
    assert_eq!((q.first_index(), q.len()), (99, 0));
    assert_eq!(
        q.block(),
        MoveBlock {
            start: 99,
            moves: vec![]
        }
    );
}

#[test]
fn the_client_queue_writes_at_most_30_and_holds_at_most_the_bound() {
    let mut q = MoveQueue::new();
    let mut pushed = 0;
    for i in 0..100 {
        if q.push(mv(i)) {
            pushed += 1;
        }
    }
    assert_eq!(pushed, NET_MOVE_LIST);
    let b = q.block();
    assert_eq!(b.moves.len(), 30);
}

/// A client queue and a server list over a lossy link: a move is processed at most once and never out of order. A move that is lost
/// on all four of its sends is skipped for good (the next block starts ahead of the ack and the server's `lastMoveAck` jumps).
#[test]
fn a_lossy_link_never_repeats_or_reorders_a_move() {
    let mut rng = Rng::star(99);
    let mut q = MoveQueue::new();
    let mut l = MoveList::new();
    let mut processed: Vec<u32> = Vec::new();
    for tick in 0..400u64 {
        // The client makes one move a tick, sends a block; the link loses a third of the packets.
        let mut m = mv(tick);
        m.id = tick as u32;
        q.push(m);
        let b = q.block();
        if rng.index(3) != 0 {
            let (bytes, bits) = block_bytes(&b);
            let mut r = BitReader::with_bit_len(&bytes, bits);
            l.read_block(&mut r, &mut free_charge()).unwrap();
            let mut credit = 30;
            let taken = l.take_for_tick(&mut credit);
            let n = taken.len();
            processed.extend(taken.iter().map(|m| m.id));
            l.clear_moves(n);
            // The ack comes back (and may itself be lost).
            if rng.index(3) != 0 {
                q.on_server_ack(l.ack());
            }
        }
    }
    // Strictly increasing, no repeats.
    assert!(processed.windows(2).all(|w| w[0] < w[1]));
    assert!(processed.len() > 200);
}

#[test]
fn hostile_blocks_never_panic() {
    let mut rng = Rng::star(0xfeed_beef);
    let mut l = MoveList::new();
    for _ in 0..5000 {
        let len = rng.index(300);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let mut r = BitReader::new(&bytes);
        let _ = l.read_block(&mut r, &mut free_charge());
        let mut credit = rng.index(40) as u32;
        let _ = l.take_for_tick(&mut credit);
        l.clear_moves(rng.index(40));
        assert!(l.len() <= NET_MOVE_LIST);
    }
}

#[test]
fn an_angle_is_two_pi_over_65536_per_signed_unit() {
    let m = Move::from_raw(
        [16; 3],
        [0x4000, 0xffff, 0x8000],
        false,
        [false; MAX_TRIGGER_KEYS],
    );
    assert_eq!(m.yaw, (std::f64::consts::TAU / 4.0) as f32);
    assert_eq!(m.pitch, (-std::f64::consts::TAU / 65_536.0) as f32);
    assert_eq!(m.roll, -std::f32::consts::PI);
}
