use torque_ghosts::codec::{NetWidths, ReadCtx, decode};
use torque_ghosts::free_charge;
use torque_net::bits::BitReader;
use torque_net::huffman::Table;

use super::*;
use crate::data::RopeLook;
use crate::net::records::{CurveWire, LengthWire, LifeWire, LookWire};
use crate::sim::{MAX_BEND, MAX_COORD, MAX_SCROLL, MAX_SECONDS, Rope};

fn world_with_rope() -> (RopeWorld, RopeId) {
    let mut world = RopeWorld::new();
    let id = world
        .create(RopeParams::default(), [0.0, 0.0, 2.0], [4.0, 0.0, 2.0])
        .unwrap();
    (world, id)
}

/// What a rope may hold, whatever a ghost said: every number finite and inside what the script's own calls allow.
fn sound(rope: &Rope) {
    if let Some(curve) = rope.curve() {
        assert!(
            curve.bend.is_finite() && curve.bend.abs() <= MAX_BEND,
            "{curve:?}"
        );
        for t in [curve.start, curve.end] {
            if let Tangent::Fixed(d) = t {
                assert!(
                    d.iter().all(|c| c.is_finite() && c.abs() <= MAX_COORD),
                    "{t:?}"
                );
                assert!(d.iter().any(|c| *c != 0.0), "{t:?}");
            }
        }
    }
    if let Some(scroll) = rope.look().scroll {
        assert!(scroll.is_finite() && scroll.abs() <= MAX_SCROLL, "{scroll}");
    }
    if let Some(left) = rope.life_left() {
        assert!(left.is_finite() && left <= MAX_SECONDS, "{left}");
    }
    assert!(rope.length().is_finite() && rope.length() <= crate::data::MAX_LENGTH);
    assert!(rope.points().all(|p| p.iter().all(|c| c.is_finite())));
}

#[test]
fn a_ghost_with_hostile_values_is_held_to_what_a_script_could_set() {
    let hostile = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1.0e30,
        -1.0e30,
        0.0,
    ];
    for &x in &hostile {
        let (mut world, id) = world_with_rope();
        let record = RopeGhost {
            curve: Some(CurveWire(Some(Curve {
                start: Tangent::Fixed([x, 1.0, 0.0]),
                end: Tangent::Fixed([0.0, 0.0, 0.0]),
                bend: x,
            }))),
            look: Some(LookWire(RopeLook {
                scroll: Some(x),
                ..RopeLook::default()
            })),
            length: Some(LengthWire {
                target: x,
                seconds: x,
            }),
            life: Some(LifeWire { left: x, fade: x }),
            ..RopeGhost::default()
        };
        apply(&mut world, id, &RopeGhost::default(), &record);
        for _ in 0..100 {
            world.advance(0.032);
        }
        if let Some(rope) = world.rope(id) {
            sound(rope);
            // A fixed direction of zeros is toward the other end; a bend that is no number is straight.
            if let Some(curve) = rope.curve() {
                assert_eq!(curve.end, Tangent::Auto);
                if !x.is_finite() {
                    assert_eq!((curve.bend, curve.start), (0.0, Tangent::Auto));
                }
            }
        }
    }
    // The values a script's own calls give pass through unchanged.
    let (mut world, id) = world_with_rope();
    let record = RopeGhost {
        curve: Some(CurveWire(Some(Curve {
            start: Tangent::Fixed([0.0, 1.0, 0.0]),
            end: Tangent::Mount,
            bend: 0.5,
        }))),
        look: Some(LookWire(RopeLook {
            scroll: Some(2.5),
            ..RopeLook::default()
        })),
        ..RopeGhost::default()
    };
    apply(&mut world, id, &RopeGhost::default(), &record);
    let rope = world.rope(id).unwrap();
    assert_eq!(rope.curve().map(|c| c.bend), Some(0.5));
    assert_eq!(rope.look().scroll, Some(2.5));
}

#[test]
fn whatever_bytes_a_server_sends_leave_the_rope_sound() {
    let table = Table::torque();
    let widths = NetWidths::engine_default();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut applied = 0;
    for round in 0..20_000 {
        let len = 24 + (next() % 120) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let Ok(record) = decode::<RopeGhost>(
            &mut r,
            &table,
            &mut free_charge(),
            &ReadCtx::new(widths, round % 2 == 0),
        ) else {
            continue;
        };
        let (mut world, id) = world_with_rope();
        apply(&mut world, id, &RopeGhost::default(), &record);
        // The same again as an update from another state: the groups that differ are applied over it.
        let (mut again, other) = world_with_rope();
        apply(&mut again, other, &record, &RopeGhost::default());
        for w in [&mut world, &mut again] {
            for _ in 0..8 {
                w.advance(0.032);
            }
            w.set_floor(None);
            let _ = (w.mesh(), w.strips());
            for (_, rope) in w.ropes() {
                sound(rope);
            }
        }
        applied += 1;
    }
    assert!(applied > 1_000, "{applied} records applied");
}
