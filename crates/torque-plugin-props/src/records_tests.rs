use torque_engine::ScriptClass;
use torque_ghosts::codec::{NetWidths, ReadCtx, decode, encode};
use torque_ghosts::plugin::{GhostRecord, NetRecord};
use torque_ghosts::testing::MapView;
use torque_ghosts::{PackCtx, Value, free_charge};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use super::*;
use crate::data::{
    MAX_DRAW_SCALE, MAX_HALF_EXTENT, MAX_SHAPE_BYTES, MIN_HALF_EXTENT, PropData, PropDataRecord,
    PropParams, draw_scale,
};

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

fn prop(q: Quantized) -> MapView {
    let mut v = MapView::default()
        .with("dataBlock", 0, Value::Int(7))
        .with("slot", 0, Value::Int(42))
        .with("ownerConn", 0, Value::Int(100))
        .with("predict", 0, Value::Int(9))
        .with("@connection", 0, Value::Id(Some(100)))
        .with("asleep", 0, Value::Int(0))
        .with("frozen", 0, Value::Int(1));
    for (name, w) in STATE_FIELDS.iter().zip(q.words()) {
        v = v.with(name, 0, Value::Int(i64::from(w)));
    }
    v
}

fn round(record: &PropGhost) -> (PropGhost, usize) {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    encode(record, &mut w, &Table::torque(), widths()).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::new(&bytes);
    r.enable_string_buffer();
    let back = decode(
        &mut r,
        &Table::torque(),
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    )
    .unwrap();
    (back, bits)
}

fn moving() -> Quantized {
    Quantized {
        pos: [123_456, -98_765, 2_048],
        rot_index: 2,
        rot: [100, 20_000, 16_383],
        lin: [512, -77, 3],
        ang: [-9, 0, 40_000],
    }
}

#[test]
fn every_group_round_trips_and_the_state_is_exact() {
    let (record, kept) = PropGhost::pack(&prop(moving()), u32::MAX, &ctx());
    assert_eq!(kept, 0);
    assert_eq!(record.state, Some(StateWire(moving())));
    assert_eq!(
        record.init,
        Some(InitWire {
            slot: 42,
            mine: true,
            predict: 9,
            hulls: None,
        })
    );
    assert_eq!(
        record.flags,
        Some(FlagsWire {
            asleep: false,
            frozen: true,
            held: false,
        })
    );
    let (back, bits) = round(&record);
    assert_eq!(back, record);
    // A moving prop's whole first record, in bits: well under a tenth of a 450-byte packet.
    assert!(bits < 240, "{bits}");
    // Another connection is not its owner and hears no predict id.
    let other = prop(moving()).with("@connection", 0, Value::Id(Some(101)));
    let (record, _) = PropGhost::pack(&other, u32::MAX, &ctx());
    assert_eq!(
        record.init,
        Some(InitWire {
            slot: 42,
            mine: false,
            predict: 0,
            hulls: None,
        })
    );
    // An update of the state alone.
    let (record, _) = PropGhost::pack(&prop(moving()), MASK_STATE, &ctx());
    assert!(record.init.is_none() && record.flags.is_none());
    let (back, bits) = round(&record);
    assert_eq!(back, record);
    assert!(bits < 200, "{bits}");
    // A still prop sends no speeds.
    let (record, _) = PropGhost::pack(&prop(moving().stopped()), MASK_STATE, &ctx());
    let bits = round(&record).1;
    assert!(bits < 140, "{bits}");
}

#[test]
fn the_transform_is_the_state_in_torques_form() {
    let q = moving();
    let xf = StateWire(q).transform().unwrap();
    let [x, y, z] = xf.quat.map(|v| v.get());
    let r = q.rotation();
    assert_eq!([x, y, z], [-r[0], -r[1], -r[2]]);
    assert_eq!(xf.negative_w, r[3] < 0.0);
    assert_eq!(xf.position.map(|v| v.get()), q.position());
    let (record, _) = PropGhost::pack(&prop(q), u32::MAX, &ctx());
    assert_eq!(record.transform(), Some(xf));
}

#[test]
fn what_a_hostile_server_sends_never_panics_and_is_held() {
    let table = Table::torque();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let rounds: usize = std::env::var("TORQUE_FUZZ_PROPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_000);
    let mut read = 0;
    for round in 0..rounds {
        let len = (next() % 64) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let ctx = ReadCtx::new(widths(), round % 2 == 0);
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        if let Ok(record) = decode::<PropGhost>(&mut r, &table, &mut free_charge(), &ctx) {
            read += 1;
            if let Some(StateWire(q)) = record.state {
                assert_eq!(q, q.held());
                let s = q.state();
                assert!(
                    s.position
                        .iter()
                        .chain(&s.rotation)
                        .chain(&s.linvel)
                        .all(|v| v.is_finite())
                );
            }
            if let Some(init) = record.init {
                assert!(init.slot < 4096);
            }
        }
        let mut r = BitReader::new(&bytes);
        r.enable_string_buffer();
        if let Ok(record) = decode::<PropDataRecord>(&mut r, &table, &mut free_charge(), &ctx) {
            assert!(record.shape_file.len() <= MAX_SHAPE_BYTES);
            let p = PropParams::from(&record);
            let h = p.shape.half_extents();
            assert!(
                h.iter()
                    .all(|v| (MIN_HALF_EXTENT..=MAX_HALF_EXTENT).contains(v))
            );
            assert!(p.density.is_finite() && p.max_speed <= crate::quant::MAX_SPEED);
            assert!(
                draw_scale(record.draw_scale)
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= MAX_DRAW_SCALE)
            );
        }
    }
    assert!(read > 0, "some inputs read");
}

#[test]
fn the_datablock_record_names_fields_of_the_class() {
    let class: Vec<String> = PropData::FIELDS
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    let mut sent: Vec<String> = PropDataRecord::default()
        .script_fields()
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    sent.dedup();
    for name in &sent {
        assert!(class.contains(name), "{name} is not a PropData field");
    }
    // Every field but the server's own cap goes to the clients.
    assert_eq!(sent.len() + 1, class.len(), "{sent:?} {class:?}");
}

#[test]
fn the_digest_of_a_props_hulls_round_trips_and_a_prop_of_a_box_sends_none() {
    use crate::hulls::HullDigest;
    let digest = HullDigest {
        hulls: 32,
        points: 2_048,
        volume: 0x3f80_1234,
    };
    let view = prop(moving())
        .with("hullCount", 0, Value::Int(32))
        .with("hullPoints", 0, Value::Int(2_048))
        .with(
            "hullVolume",
            0,
            Value::Int(i64::from(0x3f80_1234_u32 as i32)),
        );
    let (record, _) = PropGhost::pack(&view, MASK_INIT, &ctx());
    assert_eq!(record.init.expect("init").hulls, Some(digest));
    let (back, bits) = round(&record);
    assert_eq!(back, record);
    assert!(bits < 120, "{bits}");
    // A box (a count of 0, or no fields) is none, and the volume's sign bit is a bit of it.
    let (record, _) = PropGhost::pack(&prop(moving()), MASK_INIT, &ctx());
    assert_eq!(record.init.expect("init").hulls, None);
    let negative =
        prop(moving())
            .with("hullCount", 0, Value::Int(1))
            .with("hullVolume", 0, Value::Int(-1));
    let (record, _) = PropGhost::pack(&negative, MASK_INIT, &ctx());
    let hulls = record.init.expect("init").hulls.expect("hulls");
    assert_eq!(hulls.volume, u32::MAX);
    assert_eq!(round(&record).0, record);
}
