use std::collections::HashMap;

use torque_engine::ScriptClass;
use torque_ghosts::codec::{NetWidths, ReadCtx, decode, encode};
use torque_ghosts::plugin::{DatablockRef, GhostRecord, NetRecord};
use torque_ghosts::{Link, ObjectView, PackCtx, Value, free_charge};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use super::*;
use crate::data::{RopeData, RopeLook};
use crate::sim::{Curve, Tangent};

/// A server rope as the packer sees it: console fields by name, and links by field.
#[derive(Default)]
struct View {
    fields: HashMap<(String, usize), Value>,
    links: HashMap<String, Link>,
}

impl View {
    fn with(mut self, name: &str, value: Value) -> View {
        self.fields.insert((name.to_string(), 0), value);
        self
    }

    fn point(mut self, name: &str, at: [f32; 3]) -> View {
        for (k, v) in at.into_iter().enumerate() {
            self.fields.insert((name.to_string(), k), Value::F32(v));
        }
        self
    }

    fn link(mut self, name: &str, link: Link) -> View {
        self.links.insert(name.to_string(), link);
        self
    }
}

impl ObjectView for View {
    fn get(&self, field: &str, index: usize) -> Value {
        self.fields
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn link(&self, field: &str) -> Link {
        self.links.get(field).copied().unwrap_or(Link::Null)
    }
}

fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn bits(record: &RopeGhost) -> usize {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    encode(record, &mut w, &Table::torque(), widths()).unwrap();
    w.bits()
}

/// A rope with something in every group.
fn rope() -> View {
    View::default()
        .with("dataBlock", Value::Int(4))
        .with("predict", Value::Int(9))
        .with("predictOwner", Value::Int(100))
        .with("@connection", Value::Id(Some(100)))
        .with("@now", Value::Int(10_000))
        .with("startMode", Value::Int(1))
        .point("startPoint", [1.0, 2.0, 3.0])
        .with("endMode", Value::Int(2))
        .with("endNode", Value::str("muzzle"))
        .link("endObject", Link::Ghost(5))
        .with("length", Value::F32(7.0))
        .with("lengthReel", Value::F32(2.0))
        .with("lengthAt", Value::Int(9_000))
        .with("lookColor", Value::str("255 0 0 128"))
        .with("lookTexture", Value::str("art/rope.png"))
        .with("lookScroll", Value::str("1.5"))
        .with("lookEmissive", Value::Int(1))
        .with("curveMode", Value::Int(1))
        .with("curveStart", Value::str("mount"))
        .with("curveEnd", Value::str("0 0 1"))
        .with("curveBend", Value::F32(0.4))
        .with("lifeEnd", Value::Int(13_000))
        .with("lifeFade", Value::F32(1.0))
}

fn pack(view: &View, mask: u32) -> (RopeGhost, u32) {
    RopeGhost::pack(view, mask, &PackCtx::new(widths()))
}

#[test]
fn a_new_ghost_carries_every_group_as_the_server_has_it() {
    let (record, kept) = pack(&rope(), u32::MAX);
    assert_eq!(kept, 0);
    assert_eq!(record.data_block, DatablockRef(Some(4)));
    assert_eq!(record.predict, Some(PredictWire(9)));
    assert_eq!(record.start, Some(AnchorWire::Point([1.0, 2.0, 3.0])));
    assert_eq!(
        record.end,
        Some(AnchorWire::Ghost {
            ghost: 5,
            node: "muzzle".to_string(),
            offset: [0.0; 3]
        })
    );
    // The reel has 2 s from 9 s and it is 10 s: one second left.
    assert_eq!(
        record.length,
        Some(LengthWire {
            target: 7.0,
            seconds: 1.0
        })
    );
    assert_eq!(
        record.look,
        Some(LookWire(RopeLook {
            color: Some([255, 0, 0, 128]),
            texture: Some("art/rope.png".to_string()),
            scroll: Some(1.5),
            emissive: Some(true),
        }))
    );
    assert_eq!(
        record.curve,
        Some(CurveWire(Some(Curve {
            start: Tangent::Mount,
            end: Tangent::Fixed([0.0, 0.0, 1.0]),
            bend: 0.4
        })))
    );
    assert_eq!(
        record.life,
        Some(LifeWire {
            left: 3.0,
            fade: 1.0
        })
    );
}

#[test]
fn an_update_carries_only_the_groups_whose_bit_is_set() {
    let view = rope();
    let (record, _) = pack(&view, MASK_LOOK);
    assert!(record.look.is_some());
    assert!(record.start.is_none() && record.end.is_none());
    assert!(record.length.is_none() && record.curve.is_none() && record.life.is_none());
    assert!(record.predict.is_none());
    // The look alone is a small part of the whole.
    let (whole, _) = pack(&view, u32::MAX);
    assert!(
        bits(&record) * 2 < bits(&whole),
        "{} bits of {}",
        bits(&record),
        bits(&whole)
    );
    // Each bit gives its own group, and none gives the datablock alone.
    for (mask, count) in [
        (MASK_START, 1),
        (MASK_END, 1),
        (MASK_LENGTH, 1),
        (MASK_CURVE, 1),
        (MASK_LIFE, 1),
        (0, 0),
    ] {
        let (record, _) = pack(&view, mask);
        let groups = [
            record.start.is_some(),
            record.end.is_some(),
            record.length.is_some(),
            record.look.is_some(),
            record.curve.is_some(),
            record.life.is_some(),
        ];
        assert_eq!(groups.iter().filter(|g| **g).count(), count, "{mask:#x}");
    }
}

#[test]
fn the_merged_record_keeps_the_groups_an_update_did_not_send() {
    let view = rope();
    let (mut kept, _) = pack(&view, u32::MAX);
    let before = kept.clone();
    let changed = rope().with("lookColor", Value::str("0 255 0 255"));
    let (update, _) = pack(&changed, MASK_LOOK);
    kept.merge(&update);
    assert_eq!(kept.start, before.start);
    assert_eq!(kept.life, before.life);
    assert_eq!(
        kept.look.as_ref().and_then(|l| l.0.color),
        Some([0, 255, 0, 255])
    );
}

fn round(record: &RopeGhost) -> RopeGhost {
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

#[test]
fn an_end_hung_from_an_object_with_no_ghost_yet_is_sent_again() {
    let waiting = rope().link("endObject", Link::NotGhosted);
    let (record, kept) = pack(&waiting, u32::MAX);
    assert_eq!(record.end, Some(AnchorWire::Waiting));
    assert_eq!(kept, MASK_END);
    // On the wire it is a free end: the client never sees "waiting".
    assert_eq!(round(&record).end, Some(AnchorWire::Free));
    // An object that is gone is a free end that is not waited for.
    let (record, kept) = pack(&rope().link("endObject", Link::Null), u32::MAX);
    assert_eq!((record.end, kept), (Some(AnchorWire::Free), 0));
}

#[test]
fn the_predict_id_goes_to_the_connection_that_owns_the_rope_alone() {
    let (mine, _) = pack(&rope(), u32::MAX);
    assert_eq!(mine.predict, Some(PredictWire(9)));
    let other = rope().with("@connection", Value::Id(Some(101)));
    assert_eq!(pack(&other, u32::MAX).0.predict, Some(PredictWire(0)));
    // A rope nobody owns, or an id outside 1 to 255, tells no one.
    let unowned = rope().with("predictOwner", Value::Int(0));
    assert_eq!(pack(&unowned, u32::MAX).0.predict, Some(PredictWire(0)));
    let huge = rope().with("predict", Value::Int(4000));
    assert_eq!(pack(&huge, u32::MAX).0.predict, Some(PredictWire(0)));
}

#[test]
fn every_group_round_trips_through_the_wire() {
    let (record, _) = pack(&rope(), u32::MAX);
    assert_eq!(round(&record), record);
    let (nothing, _) = pack(&rope(), 0);
    assert_eq!(round(&nothing), nothing);
    // A look with nothing of its own and a curve that is off are present and empty.
    let plain = rope()
        .with("lookColor", Value::str(""))
        .with("lookTexture", Value::str(""))
        .with("lookScroll", Value::str(""))
        .with("lookEmissive", Value::Int(-1))
        .with("curveMode", Value::Int(0));
    let (record, _) = pack(&plain, u32::MAX);
    assert_eq!(record.look, Some(LookWire(RopeLook::default())));
    assert_eq!(record.curve, Some(CurveWire(None)));
    assert_eq!(round(&record), record);
}

#[test]
fn what_a_hostile_server_sends_never_panics_and_is_cut_to_its_caps() {
    let table = Table::torque();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut read = 0;
    for round in 0..20_000 {
        let len = (next() % 96) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        let mut paid = 0u64;
        let mut charge = |units: u64| {
            paid += units;
            paid <= 4 * 96 + 64
        };
        let got = decode::<RopeGhost>(
            &mut r,
            &table,
            &mut charge,
            &ReadCtx::new(widths(), round % 2 == 0),
        );
        if let Ok(record) = got {
            read += 1;
            for anchor in [&record.start, &record.end].into_iter().flatten() {
                if let AnchorWire::Ghost { node, .. } = anchor {
                    assert!(node.len() <= MAX_NODE_BYTES);
                }
            }
            if let Some(LookWire(look)) = &record.look {
                assert!(
                    look.texture
                        .as_ref()
                        .is_none_or(|t| t.len() <= MAX_TEXTURE_BYTES)
                );
            }
        }
    }
    assert!(read > 0, "some short inputs read");
}

#[test]
fn the_datablock_record_names_every_field_of_the_class() {
    let record = RopeDataRecord::default();
    let mut sent: Vec<String> = record
        .script_fields()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    sent.dedup();
    let mut class: Vec<String> = RopeData::FIELDS
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    sent.sort();
    class.sort();
    assert_eq!(sent, class);
}

#[test]
fn a_server_that_has_run_past_what_an_i32_holds_still_gets_its_times_right() {
    // 3,000,000,000 ms is past 2^31: `@now` reads it as a positive number and the rope's fields hold its bits as an `i32`.
    let now: u32 = 3_000_000_000;
    let bits = |ms: u32| Value::Int(i64::from(ms as i32));
    let view = rope()
        .with("@now", Value::Int(i64::from(now)))
        .with("lengthAt", bits(now - 1_000))
        .with("lifeEnd", bits(now + 3_000));
    let (record, _) = pack(&view, u32::MAX);
    assert_eq!(
        record.length,
        Some(LengthWire {
            target: 7.0,
            seconds: 1.0
        })
    );
    assert_eq!(
        record.life,
        Some(LifeWire {
            left: 3.0,
            fade: 1.0
        })
    );
    // The same across the wrap of the clock itself.
    let view = rope()
        .with("@now", Value::Int(i64::from(100u32)))
        .with("lengthAt", bits(100u32.wrapping_sub(1_000)))
        .with("lifeEnd", bits(100 + 3_000));
    let (record, _) = pack(&view, u32::MAX);
    assert_eq!(record.length.map(|l| l.seconds), Some(1.0));
    assert_eq!(record.life.map(|l| l.left), Some(3.0));
}

#[test]
fn an_offset_end_and_a_driver_round_trip_and_are_cut_to_what_the_world_holds() {
    let view = rope()
        .point("endOffset", [0.5, -2.0, 1.25])
        .with("driverName", Value::str("joint"))
        .link("driverObject", Link::Ghost(12));
    let (record, kept) = pack(&view, u32::MAX);
    assert_eq!(kept, 0);
    assert_eq!(
        record.end,
        Some(AnchorWire::Ghost {
            ghost: 5,
            node: "muzzle".to_string(),
            offset: [0.5, -2.0, 1.25]
        })
    );
    assert_eq!(
        record.driver,
        Some(DriverWire::On {
            name: "joint".to_string(),
            ghost: 12
        })
    );
    assert_eq!(round(&record), record);
    // An offset that is not a number is the node itself, and one past the world is held to it.
    let hostile = rope()
        .point("endOffset", [f32::NAN, f32::INFINITY, 1.0e30])
        .with("driverName", Value::str(""));
    let (record, _) = pack(&hostile, u32::MAX);
    let Some(AnchorWire::Ghost { offset, .. }) = record.end else {
        panic!("a ghost end");
    };
    assert_eq!(offset, [0.0, 0.0, crate::sim::MAX_COORD]);
    assert_eq!(record.driver, Some(DriverWire::Off));
    // The driver group alone, by its own bit; an offset-free end is the short form on the wire.
    let (alone, _) = pack(&view, MASK_DRIVER);
    assert!(alone.driver.is_some() && alone.end.is_none() && alone.look.is_none());
    let with = pack(&rope().point("endOffset", [1.0, 2.0, 3.0]), MASK_END).0;
    let without = pack(&rope(), MASK_END).0;
    assert!(bits(&without) < bits(&with));
}

#[test]
fn a_driver_for_an_object_with_no_ghost_yet_is_sent_again() {
    let waiting = rope()
        .with("driverName", Value::str("joint"))
        .link("driverObject", Link::NotGhosted);
    let (record, kept) = pack(&waiting, u32::MAX);
    assert_eq!(record.driver, Some(DriverWire::Waiting));
    assert_eq!(kept, MASK_DRIVER);
    assert_eq!(round(&record).driver, Some(DriverWire::Off));
    // An object that is gone is no driver, and not waited for.
    let gone = rope()
        .with("driverName", Value::str("joint"))
        .link("driverObject", Link::Null);
    assert_eq!(pack(&gone, u32::MAX).0.driver, Some(DriverWire::Off));
}
