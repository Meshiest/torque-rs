//! [`TgeDatablocks`]: its classes are TGE's list, the small classes, every class through the set, and hostile decodes of
//! every class (no panic; a record read back is written and read again unchanged; for the classes without strings the bits
//! written back are the bits read).

use super::effects::{refused, with_field};
use super::*;
use crate::datablocks::misc::{
    NoData, TsShapeConstructor, pack_pathed_interior_data, pack_trigger_data,
    pack_ts_shape_constructor,
};
use crate::datablocks::{
    DatablockCodecSet, MAX_SHAPE_SEQUENCES, TgeDatablockClass, TgeDatablockRecord, TgeDatablocks,
};
use crate::tests::tge_tails::Rng;
use crate::tge::TGE_DATABLOCK_CLASSES;

#[test]
fn the_classes_are_tges_datablock_list_in_its_order() {
    let names: Vec<&str> = TgeDatablocks::classes()
        .iter()
        .map(|&c| TgeDatablocks::name(c))
        .collect();
    assert_eq!(names, TGE_DATABLOCK_CLASSES);
    for &class in TgeDatablocks::classes() {
        assert_eq!(TgeDatablocks::class_by_name(class.name()), Some(class));
    }
    assert_eq!(TgeDatablocks::class_by_name("VehicleData"), None);
}

#[test]
fn the_classes_that_send_nothing_or_little() {
    check(&NoData, "", 0);
    let trigger = pack_trigger_data(&MapView::default().with("tickPeriodMS", 0, Value::Int(100)));
    assert_eq!(trigger.tick_period_ms, 100);
    check(&trigger, "tickPeriodMS:32", 32);
    let v = MapView::default().with("sound", 1, Value::Id(Some(8)));
    check(
        &pack_pathed_interior_data(&v, widths()),
        "sound:1 sound:1 sound:{db} sound:1",
        3 + db(),
    );
}

#[test]
fn a_shape_constructor_sends_its_set_sequence_slots_in_order() {
    let v = MapView::default()
        .with("mShape", 0, Value::str("player.dts"))
        .with("mSequence", 0, Value::str("player_root.dsq root"))
        .with("mSequence", 5, Value::str("player_run.dsq run"));
    let rec = pack_ts_shape_constructor(&v);
    assert_eq!(rec.sequences.len(), 2);
    check_log(&rec, "mShape:s mSequence:7 mSequence:s*2");
    canonical(&rec, &ReadCtx::new(widths(), true));
    check(
        &pack_ts_shape_constructor(&MapView::default()),
        "mShape:s mSequence:7",
        EMPTY + 7,
    );
    // Every slot set: the 7-bit count holds them all.
    let full = TsShapeConstructor {
        shape: b"a.dts".to_vec(),
        sequences: vec![b"s".to_vec(); MAX_SHAPE_SEQUENCES],
    };
    canonical(&full, &ReadCtx::new(widths(), true));
    let short = TsShapeConstructor {
        sequences: vec![Vec::new(); 3],
        ..TsShapeConstructor::default()
    };
    // A count larger than the names that follow can hold is refused.
    let (bytes, bits) = with_field(&short, "mSequence", 100);
    assert!(refused::<TsShapeConstructor>(&bytes, bits).contains("mSequence"));
}

/// A view that answers every field: a short text for the text fields, else a small integer that differs by field and index
/// (so every flag, pointer and list is set, and every list runs to its cap).
struct Everything;

impl ObjectView for Everything {
    fn get(&self, field: &str, index: usize) -> Value {
        if field.contains("Name") || field.contains("name") || field.ends_with("Keys") {
            return Value::str(format!("{field}{index}"));
        }
        if ["mShape", "mSequence", "mFilename"].contains(&field) {
            return Value::str(format!("{field}{index}"));
        }
        let h = field.bytes().fold(index as u64, |h, b| {
            h.wrapping_mul(31).wrapping_add(u64::from(b))
        });
        Value::Int((h % 5) as i64 + 1)
    }
}

/// Every class through the set: packed from an empty view and from [`Everything`], written and read back.
#[test]
fn every_class_packs_writes_and_reads_through_the_set() {
    let t = table();
    for &class in TgeDatablocks::classes() {
        for view in [&MapView::default() as &dyn ObjectView, &Everything] {
            let rec = TgeDatablocks::pack(class, view, widths());
            assert_eq!(TgeDatablocks::class_of(&rec), class);
            let mut w = writer();
            TgeDatablocks::encode(&rec, &mut w, &t, widths()).unwrap();
            let n = w.bits();
            let bytes = w.into_bytes();
            let mut r = reader(&bytes, n);
            let back =
                TgeDatablocks::decode(class, &mut r, &t, &mut free_charge(), widths()).unwrap();
            assert_eq!(r.bits_left(), 0, "{class:?}");
            assert_eq!(back, rec, "{class:?}");
        }
    }
}

/// The classes whose records hold no string: their bits written back are the bits read.
fn stringless(class: TgeDatablockClass) -> bool {
    use TgeDatablockClass as C;
    matches!(
        class,
        C::AudioDescription
            | C::AudioEnvironment
            | C::AudioSampleEnvironment
            | C::FireballAtmosphereData
            | C::GameBaseData
            | C::ParticleEmitterData
            | C::ParticleEmitterNodeData
            | C::PathedInteriorData
            | C::SimDataBlock
            | C::TriggerData
            | C::WheeledVehicleSpring
    )
}

fn bit_list(bytes: &[u8], n: usize) -> Vec<bool> {
    let mut r = BitReader::with_bit_len(bytes, n);
    (0..n).map(|_| r.read_flag().unwrap()).collect()
}

/// Decodes `bytes` as `class`; when it reads, checks the record against its own round trip. Returns whether it read.
fn hostile_one(class: TgeDatablockClass, bytes: &[u8], bits: usize) -> bool {
    let t = table();
    let mut r = reader(bytes, bits);
    let Ok(rec) = TgeDatablocks::decode(class, &mut r, &t, &mut free_charge(), widths()) else {
        return false;
    };
    let consumed = r.position();
    assert_eq!(TgeDatablocks::class_of(&rec), class);
    let mut w = writer();
    TgeDatablocks::encode(&rec, &mut w, &t, widths()).unwrap();
    let n = w.bits();
    let out = w.into_bytes();
    let again: TgeDatablockRecord = TgeDatablocks::decode(
        class,
        &mut reader(&out, n),
        &t,
        &mut free_charge(),
        widths(),
    )
    .unwrap();
    assert_eq!(again, rec, "{class:?}");
    if stringless(class) {
        assert_eq!(n, consumed, "{class:?}");
        assert_eq!(bit_list(&out, n), bit_list(bytes, consumed), "{class:?}");
    }
    true
}

#[test]
fn hostile_records_never_panic_and_read_back_unchanged() {
    let t = table();
    let mut rng = Rng(0x5eed_da7a_b10c_0001);
    for &class in TgeDatablocks::classes() {
        let mut decoded = 0usize;
        // Random bytes, half of them zero (so counts and flags are often small or clear).
        for _ in 0..300 {
            let len = (rng.next() % 300) as usize;
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    if rng.next() & 1 == 0 {
                        0
                    } else {
                        rng.next() as u8
                    }
                })
                .collect();
            decoded += usize::from(hostile_one(class, &bytes, len * 8));
        }
        // Single and double bit flips of a full record (every block present), and cuts of it.
        let rec = TgeDatablocks::pack(class, &Everything, widths());
        let mut w = writer();
        TgeDatablocks::encode(&rec, &mut w, &t, widths()).unwrap();
        let n = w.bits();
        let clean = bit_list(&w.into_bytes(), n);
        for round in 0..300 {
            let mut bits = clean.clone();
            if n > 0 {
                for _ in 0..1 + round % 2 {
                    let at = (rng.next() % n as u64) as usize;
                    bits[at] = !bits[at];
                }
            }
            let keep = if round % 5 == 0 {
                (rng.next() % (n as u64 + 1)) as usize
            } else {
                n
            };
            let mut out = BitWriter::new();
            for &b in &bits[..keep] {
                out.write_flag(b);
            }
            decoded += usize::from(hostile_one(class, &out.into_bytes(), keep));
        }
        // Enough streams decode for the round trips to mean something.
        assert!(decoded > 100, "{class:?}: {decoded}");
    }
}

#[test]
fn a_spent_budget_stops_every_class() {
    let t = table();
    for &class in TgeDatablocks::classes() {
        let rec = TgeDatablocks::pack(class, &Everything, widths());
        let mut w = writer();
        TgeDatablocks::encode(&rec, &mut w, &t, widths()).unwrap();
        let n = w.bits();
        let bytes = w.into_bytes();
        let mut deny = |_: u64| false;
        let got = TgeDatablocks::decode(class, &mut reader(&bytes, n), &t, &mut deny, widths());
        assert!(got.is_err(), "{class:?}");
    }
}

#[test]
fn tge_14s_own_datablock_width_is_a_netwidths() {
    let t = table();
    let tge = NetWidths::new(12, 10);
    let v = MapView::default().with("fireball", 0, Value::Id(Some(1029)));
    let rec = TgeDatablocks::pack(TgeDatablockClass::FireballAtmosphereData, &v, tge);
    // 1029 - 3 wraps into 10 bits: 1026 & 1023 = 2, so the id comes back as 5.
    assert_eq!(
        rec,
        TgeDatablockRecord::FireballAtmosphereData(
            crate::datablocks::weather::FireballAtmosphereData { fireball: Some(5) }
        )
    );
    let mut w = writer();
    TgeDatablocks::encode(&rec, &mut w, &t, tge).unwrap();
    assert_eq!(w.bits(), 11);
}

#[test]
fn a_member_is_read_from_the_script_field_tge_registers_it_as() {
    use crate::datablocks::ScriptField::{Absent, Field, Own};
    use TgeDatablockClass::*;
    for (class, member, index, want) in [
        (AudioDescription, "mVolume", 0, Field("volume", 0)),
        (
            AudioSampleEnvironment,
            "mRoomRolloff",
            0,
            Field("roomRolloff", 0),
        ),
        (
            AudioProfile,
            "mDescriptionObject",
            0,
            Field("description", 0),
        ),
        (FxLightData, "mRadius", 0, Field("Radius", 0)),
        (FxLightData, "mUseColour", 0, Field("AnimColour", 0)),
        (PlayerData, "sound", 0, Field("FootSoftSound", 0)),
        (PlayerData, "sound", 17, Field("exitingWater", 0)),
        (PlayerData, "sound", 18, Absent),
        (PlayerData, "shapeName", 0, Field("shapeFile", 0)),
        (PlayerData, "boxSize", 2, Field("boundingBox", 2)),
        (PlayerData, "mass", 0, Own),
        (
            WheeledVehicleData,
            "body.sound",
            1,
            Field("hardImpactSound", 0),
        ),
        (
            WheeledVehicleTire,
            "longitudinalRelaxation",
            0,
            Field("logitudinalRelaxation", 0),
        ),
        (ExplosionData, "emitterList", 3, Field("emitter", 3)),
        (ShapeBaseImageData, "mountOffset", 0, Field("offset", 0)),
        (ShapeBaseImageData, "state.fire", 4, Field("stateFire", 4)),
        (
            ShapeBaseImageData,
            "state.loaded",
            2,
            Field("stateLoadedFlag", 2),
        ),
        // State 1's `Loaded` transition (its pair's second element) and state 2's timeout.
        (
            ShapeBaseImageData,
            "state.transition.loaded",
            3,
            Field("stateTrans", 12),
        ),
        (
            ShapeBaseImageData,
            "state.transition.timeout",
            2,
            Field("stateTrans", 32),
        ),
    ] {
        assert_eq!(
            class.script_field(member, index),
            want,
            "{class:?} {member} {index}"
        );
    }
}
