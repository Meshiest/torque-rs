use super::*;
use crate::testids::{table, with_ctx};

fn tag(n: u32) -> Vec<u8> {
    format!("Tag{n}").into_bytes()
}

#[test]
fn the_first_32_distinct_tags_take_slots_0_to_31_in_order() {
    let mut s = SendStrings::new();
    for n in 0..32u32 {
        let c = s.check_string(100 + n, &tag(n));
        assert_eq!(u32::from(c.slot), n, "tag {n}");
        assert!(!c.acked);
        let e = c.event.expect("a miss posts an event");
        assert_eq!(
            (u32::from(e.slot), e.text.as_slice()),
            (n, tag(n).as_slice())
        );
        assert_eq!(c.acquired, Some(100 + n));
        assert_eq!(c.released, None);
    }
}

#[test]
fn the_33rd_tag_takes_the_least_recently_used_slot_and_lets_go_of_its_tag() {
    let mut s = SendStrings::new();
    for n in 0..32u32 {
        s.check_string(100 + n, &tag(n));
    }
    let c = s.check_string(500, b"New");
    assert_eq!(c.slot, 0);
    assert_eq!(c.released, Some(100));
    assert_eq!(s.find(100), None);
    assert_eq!(s.find(500), Some(0));
    // The next miss takes slot 1, then 2: the re-keyed entry went to the most-recently-used end.
    assert_eq!(s.check_string(501, b"N2").slot, 1);
    assert_eq!(s.check_string(502, b"N3").slot, 2);
}

#[test]
fn a_hit_moves_the_entry_to_the_most_recently_used_end() {
    let mut s = SendStrings::new();
    for n in 0..32u32 {
        s.check_string(100 + n, &tag(n));
    }
    let hit = s.check_string(100, b"x");
    assert_eq!((hit.slot, hit.event), (0, None));
    // Slot 0 was touched, so the LRU head is slot 1 now.
    assert_eq!(s.check_string(600, b"Y").slot, 1);
    assert_eq!(s.check_string(601, b"Z").slot, 2);
    // Touch slot 31 and 3: the next misses skip them.
    s.check_string(131, b"x");
    s.check_string(103, b"x");
    let slots: Vec<u8> = (0..27)
        .map(|i| s.check_string(700 + i, b"w").slot)
        .collect();
    assert_eq!(slots, (4..=30).collect::<Vec<u8>>());
}

#[test]
fn a_miss_clears_the_flag_and_a_delivery_sets_it_only_for_the_same_tag() {
    let mut s = SendStrings::new();
    let c = s.check_string(7, b"Seven");
    assert!(!c.acked);
    s.delivered(c.slot, 7);
    assert!(s.check_string(7, b"Seven").acked);
    // The slot is re-keyed: the flag goes, and a late delivery for the old tag does not set it for the new one.
    for n in 0..32u32 {
        s.check_string(1000 + n, b"fill");
    }
    assert_eq!(s.find(7), None);
    let again = s.check_string(7, b"Seven");
    assert!(!again.acked && again.event.is_some());
    s.delivered(again.slot, 99);
    assert!(!s.check_string(7, b"Seven").acked);
    s.delivered(again.slot, 7);
    assert!(s.check_string(7, b"Seven").acked);
}

#[test]
fn the_event_text_is_cut_at_255_bytes() {
    let mut s = SendStrings::new();
    let long = vec![b'a'; 400];
    let c = s.check_string(1, &long);
    assert_eq!(c.event.expect("event").text.len(), 255);
}

#[test]
fn release_all_hands_back_every_held_tag_once() {
    let mut s = SendStrings::new();
    for n in 0..40u32 {
        s.check_string(n + 1, b"t");
    }
    let mut tags = s.release_all();
    tags.sort_unstable();
    assert_eq!(tags, (9..=40).collect::<Vec<u32>>());
    assert!(s.release_all().is_empty());
    assert_eq!(s.check_string(5, b"t").slot, 0, "the table starts over");
}

#[test]
fn churn_matches_an_independent_least_recently_used_model() {
    // The model keeps a last-use stamp per slot; the LRU head is the smallest stamp (never-used slots stamp 0 in index order).
    let mut s = SendStrings::new();
    let mut slots: [(Option<u32>, u64); 32] = [(None, 0); 32];
    let mut x = 0x1234_5678_9abc_def1u64;
    for clock in 1u64..=20_000 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let t = (x % 90) as u32 + 1;
        let want = match slots.iter().position(|e| e.0 == Some(t)) {
            Some(i) => i,
            None => (0..32).min_by_key(|&i| (slots[i].1, i)).expect("a slot"),
        };
        slots[want] = (Some(t), clock);
        assert_eq!(usize::from(s.check_string(t, b"x").slot), want);
    }
}

#[test]
fn a_net_string_event_round_trips_and_every_truncation_is_an_error() {
    let texts: [&[u8]; 4] = [b"", b"ClientJoin", b"Hello there, %1.", &[b'z'; 255]];
    with_ctx(|ctx| {
        for text in texts {
            for slot in [0u8, 7, 31] {
                let e = NetStringEvent {
                    slot,
                    text: text.to_vec(),
                    tag: 0,
                };
                let mut w = BitWriter::new();
                w.enable_string_buffer();
                e.pack(&mut w, ctx);
                let bits = w.bits();
                let bytes = w.into_bytes();
                let mut r = BitReader::new(&bytes);
                r.enable_string_buffer();
                assert_eq!(NetStringEvent::unpack(&mut r, ctx), Ok(e));
                assert_eq!(r.position(), bits);
                for cut in 0..bits {
                    let mut r = BitReader::with_bit_len(&bytes, cut);
                    r.enable_string_buffer();
                    assert!(NetStringEvent::unpack(&mut r, ctx).is_err(), "cut {cut}");
                }
            }
        }
    });
}

#[test]
fn a_string_is_written_after_the_5_bit_slot_with_the_packets_prefix_buffer() {
    with_ctx(|ctx| {
        let mut w = BitWriter::new();
        w.enable_string_buffer();
        for (slot, text) in [(0u8, b"MsgClientJoin".as_slice()), (1, b"MsgClientDrop")] {
            NetStringEvent {
                slot,
                text: text.to_vec(),
                tag: 0,
            }
            .pack(&mut w, ctx);
        }
        // The second string shares "MsgClient" with the first: the packet's buffer sends it once.
        let mut plain = BitWriter::new();
        NetStringEvent {
            slot: 0,
            text: b"MsgClientJoin".to_vec(),
            tag: 0,
        }
        .pack(&mut plain, ctx);
        NetStringEvent {
            slot: 1,
            text: b"MsgClientDrop".to_vec(),
            tag: 0,
        }
        .pack(&mut plain, ctx);
        assert!(w.bits() < plain.bits() + 2);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        assert_eq!(
            NetStringEvent::unpack(&mut r, ctx).expect("first").text,
            b"MsgClientJoin"
        );
        assert_eq!(
            NetStringEvent::unpack(&mut r, ctx).expect("second").text,
            b"MsgClientDrop"
        );
    });
    let _ = table();
}

#[test]
fn remote_handles_are_written_by_set_and_an_index_outside_0_to_31_is_unmapped() {
    let mut r = RemoteStrings::new();
    assert_eq!(r.set(4, 10, b"Four"), None);
    assert_eq!(r.set(4, 11, b"Four again"), Some(10));
    assert_eq!(
        r.get(4).map(|m| (m.id, m.text.as_slice())),
        Some((11, b"Four again".as_slice()))
    );
    for bad in [-1i64, 32, 33, 255, 1 << 40, i64::MIN, i64::MAX] {
        assert_eq!(r.get(bad), None, "{bad}");
    }
    assert_eq!(r.get(5), None);
    assert_eq!(r.mapped(), 1);
    r.set(31, 12, b"x");
    let mut ids = r.release_all();
    ids.sort_unstable();
    assert_eq!(ids, vec![11, 12]);
    assert_eq!(r.mapped(), 0);
}

#[test]
fn the_mapping_line_has_the_engines_form() {
    assert_eq!(
        mapping_line(b"ClientJoin", 6),
        b"Mapping string: ClientJoin to index: 6".to_vec()
    );
    assert_eq!(
        mapping_line(b"", 4),
        b"Mapping string:  to index: 4".to_vec()
    );
}

#[test]
fn local_tags_give_one_id_to_equal_text_counting_from_one() {
    let mut t = LocalTags::new();
    let a = t.add(b"a");
    let b = t.add(b"b");
    assert_eq!((a, b, t.add(b"a")), (1, 2, 1));
    assert_eq!(t.text(2), Some(b"b".as_slice()));
    assert_eq!((t.text(0), t.text(3)), (None, None));
}

#[test]
fn a_miss_resets_the_acked_flag_of_the_entry_it_takes() {
    let mut s = SendStrings::new();
    let first = s.check_string(1, b"One");
    s.delivered(first.slot, 1);
    assert!(s.check_string(1, b"One").acked);
    for n in 0..31u32 {
        s.check_string(200 + n, b"x");
    }
    let miss = s.check_string(100, b"Hundred");
    assert_eq!((miss.slot, miss.released), (first.slot, Some(1)));
    assert!(!miss.acked);
    assert!(
        !s.check_string(100, b"Hundred").acked,
        "the new tag has not been delivered"
    );
}

#[test]
fn an_entry_that_owns_no_reference_gives_none_back() {
    let mut s = SendStrings::new();
    let a = s.check_string(900, b"");
    s.set_owned(a.slot, 900, false);
    // A stale call (the slot holds another tag) changes nothing.
    s.set_owned(a.slot, 901, true);
    for n in 0..31u32 {
        let c = s.check_string(n + 1, b"t");
        assert_eq!(c.released, None);
    }
    // The 33rd tag evicts the unowned entry: nothing to release.
    let c = s.check_string(500, b"x");
    assert_eq!(c.slot, a.slot);
    assert_eq!(c.released, None);
    // Destroying the table returns only what it owns.
    s.set_owned(c.slot, 500, false);
    let mut tags = s.release_all();
    tags.sort_unstable();
    assert_eq!(tags, (1..=31).collect::<Vec<u32>>());
}
