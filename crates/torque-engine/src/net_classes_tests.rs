//! Plugin net classes: what the derives make travels with its widths and caps, the registry sorts the classes into the same
//! ids on every side, refuses what cannot be ghosted, and every script engine gets the set.

use std::collections::HashMap;

use torque_ghosts::codec::{Enc, Layout, NetWidths, ReadCtx, decode, encode};
use torque_ghosts::plugin::{NetRecord, PLUGIN_GHOST_ROOM};
use torque_ghosts::{ObjectView, PackCtx, Value, free_charge};
use torque_net::bits::{BitReader, BitWriter};
use torque_net::classes::Kind;
use torque_net::huffman::Table;
use torque_ts_vm::vm::Vm;

use crate::net_classes::{DatablockRef, PluginClasses, Xf};
use crate::{
    BuildError, ClientStage, DatablockCodec, EngineBuilder, EngineClass, GhostRecord, Plugin,
    PluginId, Replicate, ServerStage,
};

/// A rope as a client sees it.
#[derive(Debug, Clone, PartialEq, Default, GhostRecord)]
struct RopeGhost {
    #[net(bits = 6)]
    segments: u32,
    #[net(bits = 5)]
    sag: i32,
    #[net(max_len = 8)]
    label: String,
    #[net(mask = 0x4)]
    color: Option<[f32; 3]>,
    #[net(bits = 10, max_len = 3)]
    knots: Vec<u16>,
    data_block: DatablockRef,
    #[net(name = "transform")]
    pose: Xf,
    #[net(skip)]
    seen: bool,
}

/// One struct as both the console class and its datablock record.
#[derive(Debug, Clone, PartialEq, Default, EngineClass, DatablockCodec)]
#[class(name = "PropData", parent = "SimDataBlock")]
struct PropData {
    #[field(default = 10.0)]
    mass: f32,
    #[net(max_len = 16)]
    model: String,
    #[field(name = "partCount")]
    parts: i32,
}

#[derive(Default)]
struct MapView(HashMap<(String, usize), Value>);

impl MapView {
    fn with(mut self, name: &str, index: usize, v: Value) -> MapView {
        self.0.insert((name.to_string(), index), v);
        self
    }
}

impl ObjectView for MapView {
    fn get(&self, field: &str, index: usize) -> Value {
        self.0
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(Value::Null)
    }
}

fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn rope_view() -> MapView {
    MapView::default()
        .with("segments", 0, Value::Int(65))
        .with("sag", 0, Value::Int(-9))
        .with("label", 0, Value::str("hemp rope, 20 m"))
        .with("color", 0, Value::F32(0.5))
        .with("color", 1, Value::F32(0.25))
        .with("color", 2, Value::F32(1.0))
        .with("knots", 0, Value::Int(5))
        .with("knots", 1, Value::Int(1500))
        .with("knots", 2, Value::Int(7))
        .with("knots", 3, Value::Int(9))
        .with("dataBlock", 0, Value::Id(Some(44)))
        .with(
            "transform",
            0,
            Value::Xform(Some([
                1.0f32.to_bits(),
                2.0f32.to_bits(),
                3.0f32.to_bits(),
                0,
                0,
                0,
                0,
            ])),
        )
}

fn round_trip<L: Layout + PartialEq + std::fmt::Debug>(rec: &L) -> (L, usize) {
    let table = Table::torque();
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    encode(rec, &mut w, &table, widths()).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::with_bit_len(&bytes, bits);
    r.enable_string_buffer();
    let back = decode(
        &mut r,
        &table,
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    )
    .unwrap();
    assert_eq!(r.bits_left(), 0);
    (back, bits)
}

#[test]
fn a_derived_ghost_record_packs_cuts_and_round_trips_with_its_widths() {
    let (rope, kept) = RopeGhost::pack(&rope_view(), !0, &PackCtx::new(widths()));
    assert_eq!(kept, 0);
    assert_eq!(rope.segments, 1, "65 in 6 bits");
    assert_eq!(rope.sag, -9);
    assert_eq!(rope.label, "hemp rop");
    assert_eq!(rope.color, Some([0.5, 0.25, 1.0]));
    assert_eq!(rope.knots, [5, 1500 & 0x3ff, 7], "three knots of 10 bits");
    assert_eq!(rope.data_block, DatablockRef(Some(44)));
    assert_eq!(rope.pose.position[0].get(), 1.0);
    assert!(!rope.seen);
    let (back, bits) = round_trip(&rope);
    assert_eq!(back, rope);
    assert_eq!(NetRecord::datablock(&back), Some(44));
    assert_eq!(NetRecord::transform(&back), Some(rope.pose));
    // The layout's widths, field by field.
    let table = Table::torque();
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    let mut enc = Enc::logging(&mut w, &table, widths());
    rope.clone().layout(&mut enc).unwrap();
    let log = enc.into_log();
    let names: Vec<&str> = log.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "segments",
            "sag",
            "label",
            "color",
            "color",
            "knots",
            "knots",
            "knots",
            "knots",
            "dataBlock",
            "dataBlock",
            "transform"
        ]
    );
    let sizes: Vec<usize> = log.iter().map(|(_, w)| *w).collect();
    assert_eq!(&sizes[..2], [6, 5]);
    assert_eq!(&sizes[3..9], [1, 96, 2, 10, 10, 10]);
    assert_eq!(sizes[11], 193);
    // An update with the color's mask bit clear leaves the color out, and merging it keeps the kept one.
    let (update, _) = RopeGhost::pack(&rope_view(), 0x1, &PackCtx::new(widths()));
    assert_eq!(update.color, None);
    let (_, update_bits) = round_trip(&update);
    assert_eq!(update_bits, bits - 96);
    let mut kept = back;
    kept.merge(&update);
    assert_eq!(kept.color, Some([0.5, 0.25, 1.0]));
    let fields = kept.script_fields();
    assert_eq!(fields[0], ("segments".to_string(), 0, Value::Int(1)));
    assert_eq!(
        fields[3],
        ("color".to_string(), 0, Value::str("0.5 0.25 1"))
    );
    assert_eq!(fields[5], ("knots".to_string(), 1, Value::Int(476)));
}

#[test]
fn one_struct_is_a_console_class_and_its_datablock_record() {
    use crate::ScriptClass;
    assert_eq!(PropData::NAME, "PropData");
    let view = MapView::default()
        .with("mass", 0, Value::F32(12.5))
        .with("model", 0, Value::str("crates/wooden_crate.glb"))
        .with("partCount", 0, Value::Int(-2));
    let data = PropData::pack(&view, widths());
    assert_eq!(
        data,
        PropData {
            mass: 12.5,
            model: "crates/wooden_cra".chars().take(16).collect(),
            parts: -2
        }
    );
    assert_eq!(round_trip(&data).0, data);
    assert_eq!(
        data.script_fields()
            .iter()
            .map(|(n, _, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["mass", "model", "partCount"]
    );
}

/// A plugin that adds the classes `build` adds.
struct Adds(&'static str, fn(&mut EngineBuilder));

impl Plugin for Adds {
    fn build(&self, engine: &mut EngineBuilder) {
        (self.1)(engine);
    }

    fn id(&self) -> Option<&str> {
        Some(self.0)
    }
}

impl crate::PluginType for Adds {
    const ID: PluginId = PluginId::new("adds");
}

fn build(plugins: Vec<Adds>) -> Result<crate::Engine, BuildError> {
    let mut builder = EngineBuilder::new();
    for p in plugins {
        builder.add_plugins(p);
    }
    builder.build()
}

#[test]
fn the_classes_of_every_plugin_are_sorted_into_one_set_and_reach_every_script_engine() {
    let engine = build(vec![
        Adds("test.ropes", |e| {
            e.net()
                .ghost::<RopeGhost>("Rope", Replicate::Interpolated)
                .ghost::<RopeGhost>("Anchor", Replicate::Predicted);
        }),
        Adds("test.props", |e| {
            e.net().datablock::<PropData>("PropData");
        }),
    ])
    .unwrap();
    let set = engine.net_classes();
    assert_eq!(
        set.names(Kind::Object).collect::<Vec<_>>(),
        ["Anchor", "Rope"]
    );
    assert_eq!(set.names(Kind::DataBlock).collect::<Vec<_>>(), ["PropData"]);
    assert_eq!(
        set.ghost("Anchor").unwrap().replicate(),
        Some(Replicate::Predicted)
    );
    let plugins: Vec<(&str, String)> = engine
        .net_class_plugins()
        .map(|(c, p)| (c.name(), p.to_string()))
        .collect();
    assert_eq!(
        plugins,
        [
            ("Rope", "test.ropes".to_string()),
            ("Anchor", "test.ropes".to_string()),
            ("PropData", "test.props".to_string())
        ]
    );
    let mut server = Vm::new();
    engine.replay(ServerStage::Classes, &mut server).unwrap();
    let mut client = Vm::new();
    engine.replay(ClientStage::Classes, &mut client).unwrap();
    for vm in [&server, &client] {
        assert_eq!(vm.extension::<PluginClasses>().as_deref(), Some(&set));
    }
    // The manifest lists them, so a peer with other classes is refused before any ghost.
    let manifest = engine.net_manifest(&torque_net::profile::HandshakeProfile::default());
    assert!(engine.is_networked());
    assert_eq!(
        manifest.ghosts.names(),
        Some(&["Anchor".to_string(), "Rope".to_string()][..])
    );
    assert_eq!(
        manifest.datablocks.names(),
        Some(&["PropData".to_string()][..])
    );
    // An engine with none installs nothing.
    let plain = EngineBuilder::new().build().unwrap();
    let mut vm = Vm::new();
    plain.replay(ServerStage::Classes, &mut vm).unwrap();
    assert!(vm.extension::<PluginClasses>().is_none());
    assert!(plain.net_classes().is_empty());
}

fn refusal(plugins: Vec<Adds>) -> String {
    match build(plugins) {
        Err(error @ BuildError::Rejected { .. }) => error.to_string(),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("built"),
    }
}

#[test]
fn a_name_taken_twice_or_by_tge_and_more_classes_than_ids_fail_the_build() {
    assert_eq!(
        refusal(vec![
            Adds("test.a", |e| {
                e.net().ghost::<RopeGhost>("Rope", Replicate::Interpolated);
            }),
            Adds("test.b", |e| {
                e.net().datablock::<PropData>("Rope");
            }),
        ]),
        "the plugin `test.b`: `test.a` already added the net class Rope"
    );
    assert_eq!(
        refusal(vec![Adds("test.a", |e| {
            e.net().ghost::<RopeGhost>("Player", Replicate::Predicted);
        })]),
        "the plugin `test.a`: the net class Player: Player is a net class of the engine's own"
    );
    assert_eq!(
        refusal(vec![Adds("test.a", |e| {
            e.net()
                .ghost::<RopeGhost>("Two Words", Replicate::Predicted);
        })]),
        "the plugin `test.a`: the net class Two Words: \"Two Words\" is not a class name (1 to 64 ASCII letters, digits and '_', not starting with a digit)"
    );
    let error = refusal(vec![Adds("test.many", |e| {
        let mut net = e.net();
        for k in 0..=PLUGIN_GHOST_ROOM {
            net.ghost::<RopeGhost>(&format!("Rope{k}"), Replicate::Interpolated);
        }
    })]);
    assert_eq!(
        error,
        "the plugin `test.many`: 17 plugin ghost classes, and the class table has ids for 16"
    );
}
