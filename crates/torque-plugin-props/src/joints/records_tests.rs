use torque_ghosts::codec::{NetWidths, ReadCtx, decode, encode};
use torque_ghosts::plugin::GhostRecord;
use torque_ghosts::testing::MapView;
use torque_ghosts::{Link, PackCtx, Value, free_charge};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use super::*;

fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn ctx() -> PackCtx {
    PackCtx {
        widths: widths(),
        now_ms: 0,
        compression_point: [0.0; 3],
    }
}

fn round(record: &JointGhost) -> JointGhost {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    encode(record, &mut w, &Table::torque(), widths()).unwrap();
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    decode(
        &mut r,
        &Table::torque(),
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    )
    .unwrap()
}

/// A grapple from a player (ghost `player`) to a prop the connection has (ghost 9), soft, at its limit.
fn grapple(player: Link) -> MapView {
    MapView::default()
        .with("kind", 0, Value::Int(4))
        .with("aMode", 0, Value::Int(2))
        .with("aObject", 0, Value::Int(100))
        .floats("aOffset", &[0.0, 0.0, 1.25])
        .with("bMode", 0, Value::Int(2))
        .with("bObject", 0, Value::Int(200))
        .floats("bOffset", &[0.5, 0.0, 0.0])
        .with("length", 0, Value::F32(4.5))
        .with("minLength", 0, Value::F32(0.0))
        .with("stiffness", 0, Value::F32(900.0))
        .with("damping", 0, Value::F32(60.0))
        .with("atLimit", 0, Value::Int(1))
        .with("broken", 0, Value::Int(0))
        .linked("aObject", player)
        .linked("bObject", Link::Ghost(9))
}

#[test]
fn a_joint_record_travels_whole_and_its_floats_bit_for_bit() {
    let (record, _) = JointGhost::pack(&grapple(Link::Ghost(3)), u32::MAX, &ctx());
    let init = record.init.expect("the init group");
    assert_eq!(init.kind, KindId::Grapple);
    assert_eq!(
        init.a,
        EndWire::Ghost {
            ghost: 3,
            offset: [0.0, 0.0, 1.25]
        }
    );
    assert_eq!(
        init.b,
        EndWire::Ghost {
            ghost: 9,
            offset: [0.5, 0.0, 0.0]
        }
    );
    let back = round(&record);
    assert_eq!(back, record);
    let l = back.length.unwrap();
    assert_eq!(l.length.to_bits(), 4.5f32.to_bits());
    assert_eq!(back.spring.unwrap().stiffness, 900.0);
    assert!(back.flags.unwrap().at_limit);
    // A point end, and a hard joint's spring that sends nothing but its flag.
    let point = JointGhost {
        init: Some(InitWire {
            kind: KindId::Tether,
            a: EndWire::Ghost {
                ghost: 1,
                offset: [0.0; 3],
            },
            b: EndWire::Point([1.5, -2.25, 1.0e4]),
        }),
        spring: Some(SpringWire::default()),
        ..JointGhost::default()
    };
    assert_eq!(round(&point), point);
}

#[test]
fn an_end_whose_object_is_not_ghosted_yet_waits_and_goes_again() {
    let (record, kept) = JointGhost::pack(&grapple(Link::NotGhosted), u32::MAX, &ctx());
    let init = record.init.expect("the init group");
    assert_eq!(init.a, EndWire::Waiting);
    assert!(kept & MASK_INIT != 0, "the init group is sent again");
    // It is sent as free, and the client reads it so.
    assert_eq!(round(&record).init.unwrap().a, EndWire::Free);
}

#[test]
fn hostile_values_are_held_to_their_ranges_on_decode() {
    let hostile = JointGhost {
        init: Some(InitWire {
            kind: KindId::Hinge,
            a: EndWire::Point([f32::NAN, f32::INFINITY, -1.0e30]),
            b: EndWire::Ghost {
                ghost: 2,
                offset: [f32::NEG_INFINITY, 1.0e30, 0.0],
            },
        }),
        length: Some(LengthWire {
            length: 1.0e9,
            min: f32::NAN,
        }),
        spring: Some(SpringWire {
            stiffness: f32::INFINITY,
            damping: -5.0,
        }),
        flags: None,
    };
    let back = round(&hostile);
    let init = back.init.unwrap();
    let EndWire::Point(p) = init.a else {
        panic!("{:?}", init.a)
    };
    assert!(
        p.iter()
            .all(|v| v.is_finite() && v.abs() <= crate::quant::MAX_COORD)
    );
    let EndWire::Ghost { offset, .. } = init.b else {
        panic!("{:?}", init.b)
    };
    assert!(offset.iter().all(|v| v.is_finite()));
    let l = back.length.unwrap();
    assert_eq!((l.length, l.min), (MAX_LENGTH, 0.0));
    let s = back.spring.unwrap();
    assert!(s.stiffness.is_finite() && s.stiffness >= 0.0 && s.damping == 0.0);
}
