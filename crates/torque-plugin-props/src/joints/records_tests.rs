use std::collections::HashMap;

use torque_ghosts::codec::{NetWidths, ReadCtx, decode, encode};
use torque_ghosts::plugin::GhostRecord;
use torque_ghosts::{Link, ObjectView, PackCtx, Value, free_charge};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use super::*;

/// A server joint as the packer sees it: its fields, and the ghosts its objects are for the connection packed for.
#[derive(Default)]
struct View {
    fields: HashMap<String, Value>,
    links: HashMap<String, Link>,
}

impl View {
    fn with(mut self, name: &str, value: Value) -> View {
        self.fields.insert(name.to_string(), value);
        self
    }

    fn linked(mut self, name: &str, link: Link) -> View {
        self.links.insert(name.to_string(), link);
        self
    }
}

impl ObjectView for View {
    fn get(&self, field: &str, index: usize) -> Value {
        match self.fields.get(field) {
            Some(Value::Str(text)) => String::from_utf8_lossy(text)
                .split(' ')
                .nth(index)
                .and_then(|w| w.parse().ok())
                .map_or(Value::Null, Value::F32),
            Some(v) if index == 0 => v.clone(),
            _ => Value::Null,
        }
    }

    fn link(&self, field: &str) -> Link {
        self.links.get(field).copied().unwrap_or(Link::Null)
    }
}

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
fn grapple(player: Link) -> View {
    View::default()
        .with("kind", Value::Int(4))
        .with("aMode", Value::Int(2))
        .with("aObject", Value::Int(100))
        .with("aOffset", Value::Str(b"0 0 1.25".to_vec()))
        .with("bMode", Value::Int(2))
        .with("bObject", Value::Int(200))
        .with("bOffset", Value::Str(b"0.5 0 0".to_vec()))
        .with("length", Value::F32(4.5))
        .with("minLength", Value::F32(0.0))
        .with("stiffness", Value::F32(900.0))
        .with("damping", Value::F32(60.0))
        .with("atLimit", Value::Int(1))
        .with("broken", Value::Int(0))
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
