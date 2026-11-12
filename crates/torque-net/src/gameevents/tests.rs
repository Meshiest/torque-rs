use super::*;
use crate::events::{EventRegistry, Guarantee, free_charge};
use crate::profile::DEFAULT_DATABLOCK_BITS;
use crate::testids::{ids, with_ctx};
use torque_testkit::Rng;

fn reader(w: BitWriter) -> (Vec<u8>, usize) {
    let bits = w.bits();
    (w.into_bytes(), bits)
}

#[test]
fn audio_2d_ids_are_the_datablock_width_less_three() {
    // TGE 1.4's 10-bit datablock ids by default (Torque3D's are 14 bits); any other width the profile names.
    for width in [DEFAULT_DATABLOCK_BITS, 10, 13] {
        let top = 3 + (1u32 << width) - 1;
        let mut w = BitWriter::new();
        Sim2DAudioEvent { profile: 3 }.pack(&mut w, width);
        Sim2DAudioEvent { profile: top }.pack(&mut w, width);
        assert_eq!(w.bits(), 2 * usize::from(width));
        let (bytes, bits) = reader(w);
        let mut r = BitReader::with_bit_len(&bytes, bits);
        assert_eq!(r.read_bits(width), Ok(0));
        assert_eq!(r.read_bits(width), Ok((1 << width) - 1));
        let mut r = BitReader::with_bit_len(&bytes, bits);
        assert_eq!(Sim2DAudioEvent::unpack(&mut r, width).unwrap().profile, 3);
        assert_eq!(Sim2DAudioEvent::unpack(&mut r, width).unwrap().profile, top);
        // An id below the first datablock id wraps in the field.
        let mut w = BitWriter::new();
        Sim2DAudioEvent { profile: 2 }.pack(&mut w, width);
        let mut r = BitReader::new(w.bytes());
        assert_eq!(Sim2DAudioEvent::unpack(&mut r, width).unwrap().profile, top);
    }
}

#[test]
fn audio_3d_with_and_without_a_cone_round_trips_against_the_compression_point() {
    let cp = [100.0f32, -50.0, 25.0];
    let width = DEFAULT_DATABLOCK_BITS;
    for cone in [
        None,
        Some(Cone {
            xyz: [0, 128, 255],
            w_negative: true,
        }),
    ] {
        let e = Sim3DAudioEvent {
            profile: 77,
            cone,
            position: [101.5, -48.0, 30.5],
        };
        let mut w = BitWriter::new();
        w.set_compression_point(cp);
        e.pack(&mut w, width);
        let expected = usize::from(width) + 1 + if cone.is_some() { 25 } else { 0 } + 2 + 3 * 16;
        assert_eq!(w.bits(), expected);
        let (bytes, bits) = reader(w);
        let mut r = BitReader::with_bit_len(&bytes, bits);
        r.set_compression_point(cp);
        let back = Sim3DAudioEvent::unpack(&mut r, width).unwrap();
        assert_eq!(back, e);
        assert_eq!(r.bits_left(), 0);
    }
}

#[test]
fn a_negative_quaternion_component_is_written_as_its_low_byte_and_arrives_positive() {
    // (S32)(-0.5 * 255) = -127 -> 0x81; the client reads 129 / 255.
    let cone = Cone::from_quaternion([-0.5, 0.5, 0.0, -0.5]);
    assert_eq!(cone.xyz, [0x81, 127, 0]);
    assert!(cone.w_negative);
    let q = cone.quaternion();
    assert!(q[0] > 0.0);
    assert_eq!(q[0], (129.0f64 / 255.0) as f32);
    assert!(q[3] < 0.0);
    // The identity: w = 1 (sqrt(1 - 0)).
    let id = Cone::from_quaternion([0.0, 0.0, 0.0, 1.0]);
    assert_eq!(id.quaternion(), [0.0, 0.0, 0.0, 1.0]);
    // Bytes whose squares exceed 1 clamp w to 0.
    let big = Cone {
        xyz: [255, 255, 255],
        w_negative: false,
    };
    assert_eq!(big.quaternion()[3], 0.0);
}

#[test]
fn the_lightning_indices_follow_the_connections_ghost_width() {
    for bits in [12u8, 18, 20, 24] {
        let top = (1u32 << bits) - 1;
        let width = usize::from(bits) + 1;
        let l = LightningStrikeEvent {
            lightning: Some(top),
            x: 3,
            y: 4,
            target: Some(1 << bits),
        };
        let mut w = BitWriter::new();
        l.pack(&mut w, bits);
        assert_eq!(w.bits(), 1 + width + 20 + 1 + width, "{bits} bits");
        let (bytes, n) = reader(w);
        let mut r = BitReader::with_bit_len(&bytes, n);
        assert_eq!(LightningStrikeEvent::unpack(&mut r, bits).unwrap(), l);
        assert_eq!(r.bits_left(), 0);
    }
    // Without a Lightning ghost the event is a lone clear flag.
    let mut w = BitWriter::new();
    LightningStrikeEvent::default().pack(&mut w, 18);
    assert_eq!(w.bits(), 1);
}

#[test]
fn every_event_here_is_ordered_and_named_by_the_class_table() {
    let i = ids();
    let events = [
        GameEvent::Audio2D(Sim2DAudioEvent { profile: 3 }),
        GameEvent::PathManager(PathManagerEvent::empty()),
        GameEvent::LightningStrike(LightningStrikeEvent::default()),
    ];
    for e in &events {
        assert_eq!(WireEvent::guarantee(e), Guarantee::Ordered);
    }
    assert_eq!(i.name(i.path_manager), Some("PathManagerEvent"));
    assert_eq!(i.name(i.sim_3d_audio), Some("Sim3DAudioEvent"));
}

#[test]
fn path_manager_empty_form_is_97_bits_and_entries_round_trip() {
    let mut w = BitWriter::new();
    PathManagerEvent::empty().pack(&mut w);
    assert_eq!(w.bits(), 97);
    let (bytes, bits) = reader(w);
    let mut r = BitReader::with_bit_len(&bytes, bits);
    assert_eq!(
        PathManagerEvent::unpack(&mut r, &mut free_charge()),
        Ok(PathManagerEvent::empty())
    );
    let e = PathManagerEvent {
        modified_path: 7,
        clear: true,
        total_time: 9,
        entries: vec![
            PathEntry {
                position: [1, 2, 3],
                rotation: [4, 5, 6, 7],
                time: 8,
                smoothing: 9,
            };
            3
        ],
    };
    let mut w = BitWriter::new();
    e.pack(&mut w);
    assert_eq!(w.bits(), 97 + 3 * PATH_ENTRY_BITS);
    let (bytes, bits) = reader(w);
    let mut r = BitReader::with_bit_len(&bytes, bits);
    assert_eq!(PathManagerEvent::unpack(&mut r, &mut free_charge()), Ok(e));
}

#[test]
fn a_path_count_past_the_cap_or_the_bits_left_is_refused() {
    let mk = |count: u32, extra_bits: usize| {
        let mut w = BitWriter::new();
        w.write_u32(0);
        w.write_flag(false);
        w.write_u32(0);
        w.write_u32(count);
        for _ in 0..extra_bits {
            w.write_flag(false);
        }
        w.into_bytes()
    };
    for count in [1u32, NET_PATH_ENTRIES as u32 + 1, 0x7fff_ffff, u32::MAX] {
        let bytes = mk(count, 100);
        let mut r = BitReader::with_bit_len(&bytes, 197);
        let got = PathManagerEvent::unpack(&mut r, &mut free_charge());
        assert!(matches!(got, Err(NetError::Cap { .. })), "{count}: {got:?}");
    }
    // A clamping reader: a negative count is none, a count past the maximum is the maximum (still bounded by the bits left).
    let clamp = PathCount {
        max: 10,
        clamp_signed: true,
    };
    let bytes = mk(u32::MAX, 0);
    let got = PathManagerEvent::unpack_with(&mut BitReader::new(&bytes), &mut free_charge(), clamp);
    assert_eq!(got.map(|e| e.entries.len()), Ok(0));
    let bytes = mk(0x7fff_ffff, 10 * 288);
    let got = PathManagerEvent::unpack_with(&mut BitReader::new(&bytes), &mut free_charge(), clamp);
    assert_eq!(got.map(|e| e.entries.len()), Ok(10));
    let bytes = mk(11, 9 * 288);
    let got = PathManagerEvent::unpack_with(&mut BitReader::new(&bytes), &mut free_charge(), clamp);
    assert!(matches!(got, Err(NetError::Cap { .. })), "{got:?}");
    // A budget that says no stops it.
    let e = PathManagerEvent {
        entries: vec![PathEntry::default(); 2],
        ..PathManagerEvent::empty()
    };
    let mut w = BitWriter::new();
    e.pack(&mut w);
    let mut r = BitReader::new(w.bytes());
    assert_eq!(
        PathManagerEvent::unpack(&mut r, &mut |_| false),
        Err(NetError::Invalid("work budget"))
    );
}

#[test]
fn game_event_dispatch_reads_the_right_body_by_class() {
    let i = ids();
    with_ctx(|ctx| {
        let events = [
            GameEvent::Audio2D(Sim2DAudioEvent { profile: 12 }),
            GameEvent::Audio3D(Sim3DAudioEvent {
                profile: 9,
                cone: None,
                position: [1.0, 2.0, 3.0],
            }),
            GameEvent::PathManager(PathManagerEvent::empty()),
            GameEvent::LightningStrike(LightningStrikeEvent {
                lightning: Some(7),
                x: 1023,
                y: 5,
                target: Some(262_144),
            }),
        ];
        for e in events {
            let mut w = BitWriter::new();
            WireEvent::pack(&e, &mut w, ctx);
            let (bytes, bits) = reader(w);
            let mut r = BitReader::with_bit_len(&bytes, bits);
            let class = WireEvent::class(&e, &i);
            let back = GameEvent::unpack(class, &mut r, ctx, &mut free_charge())
                .unwrap()
                .unwrap();
            assert_eq!(back, e);
            assert_eq!(r.bits_left(), 0, "{e:?}");
        }
        let mut r = BitReader::new(&[0; 8]);
        assert_eq!(
            GameEvent::unpack(i.net_string, &mut r, ctx, &mut free_charge()),
            Ok(None)
        );
    });
}

#[test]
fn torque_ids_come_from_the_registry_by_the_sort_rule() {
    let a = EventIds::torque().unwrap();
    assert_eq!(a, EventRegistry::torque().ids().unwrap());
    assert_eq!(a.count(), 13);
    assert_eq!(a.bits(), 4);
    assert_eq!(a.connection_message, 0);
    assert_eq!(a.name(a.sim_data_block), Some("SimDataBlockEvent"));
    assert_eq!(a.sim_data_block, 11);
}

/// Hostile bytes at every bit offset and truncations of valid bodies: an error or a value, never a panic.
#[test]
fn hostile_event_bodies_never_panic() {
    let i = ids();
    let classes = [
        i.sim_2d_audio,
        i.sim_3d_audio,
        i.path_manager,
        i.lightning_strike,
    ];
    let mut rng = Rng::star(0x1234_5678_9abc_def1);
    with_ctx(|ctx| {
        for round in 0..6000 {
            let len = rng.index(400);
            let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
            let class = classes[round % classes.len()];
            let mut r = BitReader::new(&bytes);
            let skip = rng.index(8);
            let _ = r.read_bits(skip as u8);
            let _ = GameEvent::unpack(class, &mut r, ctx, &mut free_charge());
        }
        // Every truncation of valid events.
        let e = GameEvent::PathManager(PathManagerEvent {
            entries: vec![PathEntry::default(); 2],
            ..PathManagerEvent::empty()
        });
        let mut w = BitWriter::new();
        WireEvent::pack(&e, &mut w, ctx);
        let (bytes, bits) = reader(w);
        for cut in 0..bits {
            let mut r = BitReader::with_bit_len(&bytes, cut);
            assert!(
                GameEvent::unpack(i.path_manager, &mut r, ctx, &mut free_charge()).is_err(),
                "cut at {cut}"
            );
        }
    });
}
