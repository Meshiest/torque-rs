//! [`TgeGhosts`] and TGE's class list: names and ids, every class's default initial update through the set, and hostile
//! decodes of every class (no panic; a record read back is written and read again unchanged; for the classes without strings
//! the bits written back are the bits read).

use torque_net::classes::{Kind, NetClasses};
use torque_net::events::EventRegistry;

use super::tge_shapes::{ALL, Wheels, shape_view};
use super::tge_tails::Rng;
use super::*;
use crate::codec::{ClientFacts, ObjCtx};
use crate::set::GhostCodecSet;
use crate::tge::{
    TGE_DATABLOCK_CLASSES, TGE_OBJECT_CLASSES, TgeClass, TgeGhosts, TgeRecord, tge_reps,
};

#[test]
fn the_codec_classes_are_tges_and_the_ids_derive_from_the_list() {
    for class in TgeClass::ALL {
        assert!(
            TGE_OBJECT_CLASSES.contains(&class.name()),
            "{}",
            class.name()
        );
        assert_eq!(TgeGhosts::class_by_name(class.name()), Some(class));
    }
    assert_eq!(TgeGhosts::class_by_name("TerrainBlock"), None);
    let classes = NetClasses::derive(&tge_reps(&EventRegistry::torque())).unwrap();
    assert_eq!(
        (
            classes.count(Kind::Object),
            classes.count(Kind::DataBlock),
            classes.count(Kind::Event)
        ),
        (45, 35, 13)
    );
    assert_eq!(
        (
            classes.id_bits(Kind::Object),
            classes.id_bits(Kind::DataBlock),
            classes.id_bits(Kind::Event)
        ),
        (6, 6, 4)
    );
    // The names sort as unsigned bytes: upper case before lower case.
    assert_eq!(classes.id_of(Kind::Object, "AIPlayer"), Some(0));
    assert_eq!(classes.id_of(Kind::Object, "fxSunLight"), Some(44));
    assert_eq!(classes.id_of(Kind::Object, "WheeledVehicle"), Some(39));
    assert_eq!(classes.id_of(Kind::DataBlock, "fxLightData"), Some(34));
    assert_eq!(TGE_DATABLOCK_CLASSES.len(), 35);
}

fn string_bits(strings: &[&[u8]]) -> usize {
    let t = table();
    let mut w = writer();
    for s in strings {
        torque_net::strcoder::write_string(&mut w, &t, s, 255);
    }
    w.bits()
}

/// The bits of each class's default initial update (every mask bit, a ShapeBase object with nothing going on, a vehicle
/// without wheels), from the layouts' lists.
fn default_bits(class: TgeClass) -> Option<usize> {
    let part = 227;
    let marker = part + 1 + 193 + 96;
    Some(match class {
        TgeClass::AiPlayer | TgeClass::Player => part + 4 + 105 + 5,
        TgeClass::AiWheeledVehicle | TgeClass::WheeledVehicle => 648 + 4,
        TgeClass::FlyingVehicle => 648 + 4,
        TgeClass::HoverVehicle => 648 + 3,
        TgeClass::Camera => part + 2 + 160,
        TgeClass::Item => part + 101 + 1 + 34 + 195,
        TgeClass::Lightning => 98 + 1 + 552,
        TgeClass::MissionMarker => marker,
        TgeClass::WayPoint => marker + 1 + string_bits(&[b""]) + 33 + 2,
        TgeClass::SpawnSphere => marker + 1 + 128,
        TgeClass::ParticleEmitterNode => 98 + 512 + 96 + 1,
        TgeClass::PhysicalZone => 1 + 512 + 96 + 96 + 64 + 96 + 1,
        TgeClass::Precipitation => 98 + 292 + 33 + 1,
        TgeClass::Projectile => 98 + 65 + 1 + 193,
        TgeClass::ScopeAlwaysShape | TgeClass::StaticShape => part + 1 + 193 + 96 + 1,
        TgeClass::Sky => 938 + string_bits(&[b"", b"", b"", b""]),
        TgeClass::Sun => 353,
        TgeClass::TsStatic => 512 + 96 + string_bits(&[b""]),
        TgeClass::VehicleBlocker => 704,
        TgeClass::FxLight => 98 + 2 + 193 + 8 + 32,
        TgeClass::InteriorInstance => 646 + string_bits(&[b"", b""]),
        // Its own test pins its forms.
        TgeClass::AudioEmitter => return None,
    })
}

fn ctx_with<'a>(facts: &'a dyn ClientFacts, initial: bool, controlled_here: bool) -> ReadCtx<'a> {
    ReadCtx {
        widths: widths(),
        initial,
        facts: Some(facts),
        object: ObjCtx {
            controlled_here,
            datablock: Some(10),
        },
    }
}

#[test]
fn every_class_packs_writes_and_reads_through_the_set() {
    let t = table();
    let none = Wheels(0);
    for class in TgeClass::ALL {
        let (rec, _) = TgeGhosts::pack(class, &shape_view(), ALL, &pack_ctx());
        assert_eq!(TgeGhosts::class_of(&rec), class);
        let mut w = writer();
        TgeGhosts::encode(&rec, &mut w, &t, widths()).unwrap();
        let bits = w.bits();
        if let Some(want) = default_bits(class) {
            assert_eq!(bits, want, "{class:?}");
        }
        let bytes = w.into_bytes();
        let mut r = reader(&bytes, bits);
        let back = TgeGhosts::decode(
            class,
            &mut r,
            &t,
            &mut free_charge(),
            &ctx_with(&none, true, false),
        )
        .unwrap();
        assert_eq!(r.bits_left(), 0, "{class:?}");
        assert_eq!(back, rec, "{class:?}");
        let mut w2 = writer();
        TgeGhosts::encode(&back, &mut w2, &t, widths()).unwrap();
        assert_eq!(w2.into_bytes(), bytes, "{class:?}");
    }
}

/// The classes whose layouts hold no string: every stream they read is written back bit for bit.
fn stringless(class: TgeClass) -> bool {
    matches!(
        class,
        TgeClass::Lightning
            | TgeClass::ParticleEmitterNode
            | TgeClass::PhysicalZone
            | TgeClass::Precipitation
            | TgeClass::Projectile
            | TgeClass::Sun
            | TgeClass::VehicleBlocker
            | TgeClass::FxLight
    )
}

/// The first `n` bits of `bytes`, one per element.
fn bit_list(bytes: &[u8], n: usize) -> Vec<bool> {
    let mut r = BitReader::with_bit_len(bytes, n);
    (0..n).map(|_| r.read_flag().unwrap()).collect()
}

#[test]
fn hostile_records_never_panic_and_read_back_unchanged() {
    let t = table();
    let facts = Wheels(4);
    let mut rng = Rng(0x0dd_ba11_cafe_f00d);
    let mut decoded = 0usize;
    for class in TgeClass::ALL {
        for round in 0..400 {
            let len = (rng.next() % 160) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
            // Every other stream opens with its first flags set, so the deep blocks are reached.
            if round % 2 == 0 && !bytes.is_empty() {
                bytes[0] |= 0xf8;
            }
            let initial = rng.next() & 1 == 0;
            let controlled = rng.next() & 3 == 0;
            let ctx = ctx_with(&facts, initial, controlled);
            let mut r = reader(&bytes, bytes.len() * 8);
            let Ok(rec) = TgeGhosts::decode(class, &mut r, &t, &mut free_charge(), &ctx) else {
                continue;
            };
            decoded += 1;
            let consumed = r.position();
            assert_eq!(TgeGhosts::class_of(&rec), class);
            let mut w = writer();
            TgeGhosts::encode(&rec, &mut w, &t, widths()).unwrap();
            let n = w.bits();
            let out = w.into_bytes();
            let again: TgeRecord =
                TgeGhosts::decode(class, &mut reader(&out, n), &t, &mut free_charge(), &ctx)
                    .unwrap();
            assert_eq!(again, rec, "{class:?}");
            if stringless(class) {
                assert_eq!(n, consumed, "{class:?}");
                assert_eq!(bit_list(&out, n), bit_list(&bytes, consumed), "{class:?}");
            }
        }
    }
    // Enough streams decode for the round trips to mean something.
    assert!(decoded > 1000, "{decoded}");
}

#[test]
fn a_spent_budget_stops_every_class() {
    let t = table();
    let facts = Wheels(4);
    for class in TgeClass::ALL {
        let (rec, _) = TgeGhosts::pack(class, &shape_view(), ALL, &pack_ctx());
        let mut w = writer();
        TgeGhosts::encode(&rec, &mut w, &t, widths()).unwrap();
        let n = w.bits();
        let bytes = w.into_bytes();
        let mut deny = |_: u64| false;
        let got = TgeGhosts::decode(
            class,
            &mut reader(&bytes, n),
            &t,
            &mut deny,
            &ctx_with(&facts, true, false),
        );
        assert!(got.is_err(), "{class:?}");
    }
}

#[test]
fn tge_14s_own_widths_are_a_netwidths() {
    let t = table();
    let tge = NetWidths::new(12, 10);
    let view = super::tge_shapes::with_link(
        MapView::default().with("datablock", 0, Value::Id(Some(5))),
        "source_object",
        Link::Ghost(4),
    );
    let (rec, _) = TgeGhosts::pack(TgeClass::Projectile, &view, ALL, &PackCtx::new(tge));
    let mut w = writer();
    TgeGhosts::encode(&rec, &mut w, &t, tge).unwrap();
    // A 10-bit datablock id, and the source as ranged(index, 0, 4096) (13 bits) and its slot (2).
    let bits = w.bits();
    assert_eq!(bits, 98 + 10 + 65 + 13 + 2 + 1 + 193);
    let bytes = w.into_bytes();
    let back = TgeGhosts::decode(
        TgeClass::Projectile,
        &mut reader(&bytes, bits),
        &t,
        &mut free_charge(),
        &ReadCtx::new(tge, true),
    )
    .unwrap();
    assert_eq!(back, rec);
}
