//! Plugin classes: the field types' wire forms and caps, the set's ids, the `Plugin` arms of TGE's sets, and hostile decodes
//! (never a panic, no allocation a wire count asks for past its cap). The records here are written by hand as the derives
//! write them (the derives' own tests are torque-engine's).

use std::collections::HashMap;
use torque_testkit::Rng;

use torque_net::NetError;
use torque_net::bits::{BitReader, BitWriter};
use torque_net::classes::Kind;
use torque_net::huffman::Table;

use super::field::{DEFAULT_VEC_LEN, MAX_VEC_LEN};
use super::*;
use crate::codec::{Codec, Enc, Layout, NetWidths, R, ReadCtx};
use crate::datablocks::{DatablockCodecSet, TgeDatablockClass, TgeDatablockRecord, TgeDatablocks};
use crate::set::GhostCodecSet;
use crate::tge::{TgeClass, TgeGhosts, TgeRecord};
use crate::{ObjectView, PackCtx, Value, free_charge};

/// A view backed by a map; anything not in it is `Null`.
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

const SPEC_6: FieldSpec = FieldSpec {
    bits: 6,
    max_len: 0,
};
const LABEL: FieldSpec = FieldSpec {
    bits: 0,
    max_len: 8,
};
const KNOTS: FieldSpec = FieldSpec {
    bits: 10,
    max_len: 4,
};

/// A rope's record, as `#[derive(GhostRecord)]` writes it: `segments` in 6 bits, `color` sent only with mask bit 1.
#[derive(Debug, Clone, PartialEq, Default)]
struct Rope {
    segments: u32,
    slack: i32,
    length: f32,
    label: String,
    color: Option<[f32; 3]>,
    knots: Vec<u16>,
    data: DatablockRef,
}

const COLOR_MASK: u32 = 1 << 1;

impl Layout for Rope {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.segments.layout(c, "segments", SPEC_6)?;
        self.slack.layout(c, "slack", SPEC_6)?;
        self.length.layout(c, "length", FieldSpec::DEFAULT)?;
        self.label.layout(c, "label", LABEL)?;
        self.color.layout(c, "color", FieldSpec::DEFAULT)?;
        self.knots.layout(c, "knots", KNOTS)?;
        self.data.layout(c, "dataBlock", FieldSpec::DEFAULT)?;
        Ok(())
    }
}

impl NetRecord for Rope {
    fn merge(&mut self, sent: &Rope) {
        self.segments.merge_from(&sent.segments);
        self.slack.merge_from(&sent.slack);
        self.length.merge_from(&sent.length);
        self.label.merge_from(&sent.label);
        self.color.merge_from(&sent.color);
        self.knots.merge_from(&sent.knots);
        self.data.merge_from(&sent.data);
    }

    fn datablock(&self) -> Option<u32> {
        self.data.datablock()
    }

    fn transform(&self) -> Option<crate::codec::Xf> {
        None
    }

    fn script_fields(&self) -> Vec<(String, u32, Value)> {
        let mut out = Vec::new();
        push_values(&mut out, "segments", &self.segments);
        push_values(&mut out, "label", &self.label);
        push_values(&mut out, "color", &self.color);
        push_values(&mut out, "knots", &self.knots);
        push_values(&mut out, "dataBlock", &self.data);
        out
    }
}

impl GhostRecord for Rope {
    fn pack(view: &dyn ObjectView, mask: u32, _ctx: &PackCtx) -> (Rope, u32) {
        let rope = Rope {
            segments: NetField::from_view(view, "segments", 0, SPEC_6),
            slack: NetField::from_view(view, "slack", 0, SPEC_6),
            length: NetField::from_view(view, "length", 0, FieldSpec::DEFAULT),
            label: NetField::from_view(view, "label", 0, LABEL),
            color: if mask & COLOR_MASK != 0 {
                NetField::from_view(view, "color", 0, FieldSpec::DEFAULT)
            } else {
                None
            },
            knots: NetField::from_view(view, "knots", 0, KNOTS),
            data: NetField::from_view(view, "dataBlock", 0, FieldSpec::DEFAULT),
        };
        (rope, 0)
    }
}

/// A datablock's record, as `#[derive(DatablockCodec)]` writes it.
#[derive(Debug, Clone, PartialEq, Default)]
struct PropData {
    mass: f32,
    model: String,
}

impl Layout for PropData {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.mass.layout(c, "mass", FieldSpec::DEFAULT)?;
        self.model.layout(c, "model", FieldSpec::DEFAULT)
    }
}

impl NetRecord for PropData {
    fn merge(&mut self, sent: &PropData) {
        *self = sent.clone();
    }

    fn datablock(&self) -> Option<u32> {
        None
    }

    fn transform(&self) -> Option<crate::codec::Xf> {
        None
    }

    fn script_fields(&self) -> Vec<(String, u32, Value)> {
        let mut out = Vec::new();
        push_values(&mut out, "mass", &self.mass);
        push_values(&mut out, "model", &self.model);
        out
    }
}

impl DatablockCodec for PropData {
    fn pack(view: &dyn ObjectView, _widths: NetWidths) -> PropData {
        PropData {
            mass: NetField::from_view(view, "mass", 0, FieldSpec::DEFAULT),
            model: NetField::from_view(view, "model", 0, FieldSpec::DEFAULT),
        }
    }
}

fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn rope_view() -> MapView {
    MapView::default()
        .with("segments", 0, Value::Int(70))
        .with("slack", 0, Value::Int(-3))
        .with("length", 0, Value::F32(2.5))
        .with("label", 0, Value::str("a long rope name"))
        .with("color", 0, Value::F32(1.0))
        .with("color", 1, Value::F32(0.5))
        .with("color", 2, Value::F32(0.25))
        .with("knots", 0, Value::Int(1))
        .with("knots", 1, Value::str("2000"))
        .with("knots", 2, Value::Int(3))
        .with("dataBlock", 0, Value::Id(Some(40)))
}

fn write(record: &TgeRecord) -> (Vec<u8>, usize) {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    TgeGhosts::encode(record, &mut w, &Table::torque(), widths()).unwrap();
    let bits = w.bits();
    (w.into_bytes(), bits)
}

fn read(class: TgeClass, bytes: &[u8], bits: usize) -> Result<TgeRecord, NetError> {
    let mut r = BitReader::with_bit_len(bytes, bits);
    r.enable_string_buffer();
    TgeGhosts::decode(
        class,
        &mut r,
        &Table::torque(),
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    )
}

#[test]
fn a_plugin_ghost_packs_with_its_widths_and_caps_and_round_trips_through_tges_set() {
    let class = TgeClass::Plugin(PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated));
    let (record, kept) = TgeGhosts::pack(class, &rope_view(), !0, &PackCtx::new(widths()));
    assert_eq!(kept, 0);
    assert_eq!(TgeGhosts::class_of(&record), class);
    let rope = record.plugin::<Rope>().expect("a rope");
    // 70 in 6 bits is 6; -3 in 6 bits is -3 again; the label is cut to its 8 bytes; "2000" in 10 bits is 976.
    assert_eq!(rope.segments, 6);
    assert_eq!(rope.slack, -3);
    assert_eq!(rope.label, "a long r");
    assert_eq!(rope.color, Some([1.0, 0.5, 0.25]));
    assert_eq!(rope.knots, [1, 976, 3]);
    assert_eq!(rope.data, DatablockRef(Some(40)));
    let (bytes, bits) = write(&record);
    let back = read(class, &bytes, bits).unwrap();
    assert_eq!(back, record);
    assert_eq!(back.plugin::<Rope>().unwrap().datablock(), Some(40));
    // Without its mask bit the color is not sent, and an update without it keeps the kept one.
    let (update, _) = TgeGhosts::pack(class, &rope_view(), 0, &PackCtx::new(widths()));
    assert_eq!(update.plugin::<Rope>().unwrap().color, None);
    let (_, update_bits) = write(&update);
    assert_eq!(update_bits, bits - 3 * 32);
    let mut kept = back.clone();
    if let (TgeRecord::Plugin(_, k), TgeRecord::Plugin(_, s)) = (&mut kept, &update) {
        assert!(k.merge_record(&**s));
    }
    assert_eq!(kept.plugin::<Rope>().unwrap().color, Some([1.0, 0.5, 0.25]));
}

#[test]
fn the_layout_logs_each_field_with_its_width() {
    let (record, _) = Rope::pack(&rope_view(), !0, &PackCtx::new(widths()));
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    let table = Table::torque();
    let mut enc = Enc::logging(&mut w, &table, widths());
    record.clone().layout(&mut enc).unwrap();
    let log = enc.into_log();
    let names: Vec<&str> = log.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "segments",
            "slack",
            "length",
            "label",
            "color",
            "color",
            "knots",
            "knots",
            "knots",
            "knots",
            "dataBlock",
            "dataBlock"
        ]
    );
    let widths_of: Vec<usize> = log.iter().map(|(_, w)| *w).collect();
    // The knot count takes bitlen(4) = 3 bits, each knot 10.
    assert_eq!(&widths_of[..3], [6, 6, 32]);
    assert_eq!(&widths_of[5..10], [96, 3, 10, 10, 10]);
}

#[test]
fn a_plugin_datablock_round_trips_through_tges_datablock_set_and_names_its_fields() {
    let class = TgeDatablockClass::Plugin(PluginClass::datablock::<PropData>("PropData"));
    let view = MapView::default().with("mass", 0, Value::str("12.5")).with(
        "model",
        0,
        Value::str("crate.glb"),
    );
    let record = TgeDatablocks::pack(class, &view, widths());
    assert_eq!(TgeDatablocks::class_of(&record), class);
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    let table = Table::torque();
    TgeDatablocks::encode(&record, &mut w, &table, widths()).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = BitReader::with_bit_len(&bytes, bits);
    r.enable_string_buffer();
    let back = TgeDatablocks::decode(class, &mut r, &table, &mut free_charge(), widths()).unwrap();
    assert_eq!(back, record);
    assert_eq!(
        crate::datablocks::fields::fields(&back, widths()),
        [
            ("mass".to_string(), 0, Value::F32(12.5)),
            ("model".to_string(), 0, Value::str("crate.glb"))
        ]
    );
    let TgeDatablockRecord::Plugin(_, rec) = &back else {
        panic!("a plugin record");
    };
    assert_eq!(rec.downcast_ref::<PropData>().unwrap().mass, 12.5);
}

#[test]
fn a_set_sorts_each_kind_by_name_and_numbers_it_past_tges_ids() {
    let rope = PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated);
    let anchor = PluginClass::ghost::<Rope>("Anchor", Replicate::Predicted);
    let lower = PluginClass::ghost::<Rope>("anchor2", Replicate::Predicted);
    let prop = PluginClass::datablock::<PropData>("PropData");
    let set = PluginClasses::new([rope, lower, prop, anchor]).unwrap();
    // Unsigned bytes: upper case before lower case.
    assert_eq!(
        set.names(Kind::Object).collect::<Vec<_>>(),
        ["Anchor", "Rope", "anchor2"]
    );
    assert_eq!(set.names(Kind::DataBlock).collect::<Vec<_>>(), ["PropData"]);
    let base = PluginClasses::ghost_base(45);
    assert_eq!(base, 48);
    assert_eq!(set.id_of(Kind::Object, "Rope", base), Some(49));
    assert_eq!(set.by_id(Kind::Object, 49, base), Some(rope));
    assert_eq!(set.by_id(Kind::Object, 47, base), None);
    assert_eq!(set.by_id(Kind::Object, 51, base), None);
    assert_eq!(set.by_id(Kind::Event, 48, base), None);
    let db = PluginClasses::datablock_base(35);
    assert_eq!(set.by_id(Kind::DataBlock, 35, db), Some(prop));
    assert_eq!(set.by_id(Kind::DataBlock, 36, db), None);
    assert_eq!(set.ghost_code("anchor2"), Some(PLUGIN_CODE_BASE + 2));
    assert_eq!(set.ghost_of_code(PLUGIN_CODE_BASE + 1), Some(rope));
    assert_eq!(set.ghost_of_code(3), None);
    // The same class made twice is the same class.
    assert_eq!(
        PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated),
        rope
    );
    assert_ne!(
        PluginClass::ghost::<Rope>("Rope", Replicate::Predicted),
        rope
    );
    assert_eq!(TgeClass::Plugin(rope).name(), "Rope");
    assert_eq!(
        TgeClass::Plugin(rope).net_flags(),
        torque_net::ghost::net_flags::GHOSTABLE
    );
    assert_eq!(TgeClass::Plugin(rope).code(), crate::tge::PLUGIN_CODE);
    assert_eq!(TgeClass::of_code(crate::tge::PLUGIN_CODE), None);
}

#[test]
fn a_set_refuses_bad_taken_and_repeated_names_and_more_classes_than_the_table_has_ids_for() {
    let ghost = |name: &str| PluginClass::ghost::<Rope>(name, Replicate::Interpolated);
    let refused = |classes: Vec<PluginClass>| PluginClasses::new(classes).unwrap_err();
    assert_eq!(
        refused(vec![ghost("9Lives")]),
        PluginClassError::BadName("9Lives".into())
    );
    assert_eq!(
        refused(vec![ghost("Has Space")]),
        PluginClassError::BadName("Has Space".into())
    );
    assert_eq!(
        refused(vec![ghost("Player")]),
        PluginClassError::Taken("Player".into())
    );
    assert_eq!(
        refused(vec![ghost("SpotLight")]),
        PluginClassError::Taken("SpotLight".into())
    );
    assert_eq!(
        refused(vec![PluginClass::datablock::<PropData>("ItemData")]),
        PluginClassError::Taken("ItemData".into())
    );
    assert_eq!(
        refused(vec![
            ghost("Rope"),
            PluginClass::datablock::<PropData>("Rope")
        ]),
        PluginClassError::Duplicate("Rope".into())
    );
    assert_eq!((PLUGIN_GHOST_ROOM, PLUGIN_DATABLOCK_ROOM), (16, 29));
    let many: Vec<PluginClass> = (0..=PLUGIN_GHOST_ROOM)
        .map(|k| ghost(&format!("Rope{k}")))
        .collect();
    let error = refused(many);
    assert_eq!(
        error,
        PluginClassError::TooMany {
            kind: Kind::Object,
            count: 17,
            room: 16
        }
    );
    assert_eq!(
        error.to_string(),
        "17 plugin ghost classes, and the class table has ids for 16"
    );
}

#[test]
fn a_string_past_its_cap_and_a_count_past_its_cap_are_refused() {
    let class = TgeClass::Plugin(PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated));
    let table = Table::torque();
    // A label of 9 bytes, one over its cap, written by hand in front of an otherwise empty record.
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    {
        let mut c = Enc::new(&mut w, &table, widths());
        let mut v = 0u32;
        c.int("segments", &mut v, 6).unwrap();
        c.int("slack", &mut v, 6).unwrap();
        c.raw32("length", &mut v).unwrap();
        let mut long = b"ninebytes".to_vec();
        c.string("label", &mut long).unwrap();
    }
    let bits = w.bits();
    let bytes = w.into_bytes();
    assert!(matches!(
        read(class, &bytes, bits),
        Err(NetError::Invalid(_))
    ));
    // A knot count of 7 (3 bits hold it) against a cap of 4.
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    {
        let mut c = Enc::new(&mut w, &table, widths());
        let mut v = 0u32;
        c.int("segments", &mut v, 6).unwrap();
        c.int("slack", &mut v, 6).unwrap();
        c.raw32("length", &mut v).unwrap();
        let mut empty = Vec::new();
        c.string("label", &mut empty).unwrap();
        let mut none = false;
        c.flag("color", &mut none).unwrap();
        let mut seven = 7u32;
        c.int("knots", &mut seven, 3).unwrap();
        for _ in 0..7 {
            let mut k = 1u32;
            c.int("knot", &mut k, 10).unwrap();
        }
        c.flag("dataBlock", &mut none).unwrap();
    }
    let bits = w.bits();
    let bytes = w.into_bytes();
    assert!(matches!(
        read(class, &bytes, bits),
        Err(NetError::Cap { what: "knots", .. })
    ));
}

/// A record whose every field asks for an allocation: strings, a long list of strings and a list of lists.
#[derive(Debug, Clone, PartialEq, Default)]
struct Greedy {
    names: Vec<String>,
    lists: Vec<Vec<u32>>,
    text: String,
}

const MANY: FieldSpec = FieldSpec {
    bits: 0,
    max_len: MAX_VEC_LEN,
};

impl Layout for Greedy {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.names.layout(c, "names", MANY)?;
        self.lists.layout(c, "lists", MANY)?;
        self.text.layout(c, "text", FieldSpec::DEFAULT)
    }
}

impl NetRecord for Greedy {
    fn merge(&mut self, sent: &Greedy) {
        *self = sent.clone();
    }
    fn datablock(&self) -> Option<u32> {
        None
    }
    fn transform(&self) -> Option<crate::codec::Xf> {
        None
    }
    fn script_fields(&self) -> Vec<(String, u32, Value)> {
        Vec::new()
    }
}

impl GhostRecord for Greedy {
    fn pack(_view: &dyn ObjectView, _mask: u32, _ctx: &PackCtx) -> (Greedy, u32) {
        (Greedy::default(), 0)
    }
}

#[test]
fn hostile_bytes_never_panic_and_never_allocate_past_the_caps() {
    let classes = [
        TgeClass::Plugin(PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated)),
        TgeClass::Plugin(PluginClass::ghost::<Greedy>(
            "Greedy",
            Replicate::Interpolated,
        )),
    ];
    let table = Table::torque();
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
    let mut read_ok = 0;
    for round in 0..20_000 {
        let len = (rng.next_u64() % 64) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let bits = bytes.len() * 8 - (rng.next_u64() % 8) as usize * usize::from(len > 0);
        let class = classes[round % classes.len()];
        let mut r = BitReader::with_bit_len(&bytes, bits);
        r.enable_string_buffer();
        // A budget of one unit per byte read (and per element), as a tick's network work budget charges.
        let mut paid = 0u64;
        let mut charge = |units: u64| {
            paid += units;
            paid <= 4 * 64 + 64
        };
        let got = TgeGhosts::decode(
            class,
            &mut r,
            &table,
            &mut charge,
            &ReadCtx::new(widths(), round % 2 == 0),
        );
        if let Ok(TgeRecord::Plugin(_, rec)) = got {
            read_ok += 1;
            if let Some(g) = rec.downcast_ref::<Greedy>() {
                // Every element took at least a bit of the input, and no list is longer than its cap.
                let elements = g.names.len() + g.lists.iter().map(|l| 1 + l.len()).sum::<usize>();
                assert!(elements <= bits, "{elements} elements from {bits} bits");
                assert!(g.names.len() <= MAX_VEC_LEN && g.lists.len() <= MAX_VEC_LEN);
                assert!(g.lists.iter().all(|l| l.len() <= DEFAULT_VEC_LEN));
                assert!(g.text.len() <= 255);
            }
        }
    }
    assert!(read_ok > 0, "some short inputs read");
}

#[test]
fn fields_read_numbers_from_text_and_keep_signs_in_their_widths() {
    let view = MapView::default()
        .with("n", 0, Value::str(" 42 "))
        .with("f", 0, Value::str("2.5"))
        .with("v", 0, Value::str("1 2 3"))
        .with("b", 0, Value::str("0"))
        .with("neg", 0, Value::Int(-1));
    assert_eq!(u8::from_view(&view, "n", 0, FieldSpec::DEFAULT), 42);
    assert_eq!(f32::from_view(&view, "f", 0, FieldSpec::DEFAULT), 2.5);
    assert_eq!(
        <[f32; 3]>::from_view(&view, "v", 0, FieldSpec::DEFAULT),
        [1.0, 2.0, 3.0]
    );
    assert!(!bool::from_view(&view, "b", 0, FieldSpec::DEFAULT));
    assert_eq!(i32::from_view(&view, "neg", 0, SPEC_6), -1);
    assert_eq!(u8::from_view(&view, "neg", 0, FieldSpec::DEFAULT), 255);
    assert_eq!(
        Option::<u8>::from_view(&view, "missing", 0, FieldSpec::DEFAULT),
        None
    );
    let mut values = Vec::new();
    [1.5f32, 2.0, -3.0].values(&mut values);
    assert_eq!(values, [Value::str("1.5 2 -3")]);
}

/// No datablocks: a draw function that reads none.
struct NoDatablocks;

impl DatablockFields for NoDatablocks {
    fn field(&self, _: u32, _: &str, _: u32) -> Option<&Value> {
        None
    }
}

/// One datablock whose `model` is a text.
struct OneDatablock(Value);

impl DatablockFields for OneDatablock {
    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value> {
        (id == 7 && name == "model" && index == 0).then_some(&self.0)
    }
}

#[test]
fn a_drawn_class_turns_its_records_into_shapes_and_leaves_out_what_cannot_be_drawn() {
    let class = PluginClass::ghost::<Rope>("Rope", Replicate::Interpolated);
    let other = PluginClass::ghost::<Rope>("Cable", Replicate::Predicted);
    let mut drawn = DrawnShapes::default();
    assert!(drawn.is_empty());
    drawn.insert(
        class,
        ShapeFn::new::<Rope>(|rope, datablocks| {
            let model = datablocks.text(rope.data.datablock()?, "model")?;
            Some(ShapeDraw::new(model).scaled([1.0, 2.0, rope.length]))
        }),
    );
    assert!(drawn.contains(class) && !drawn.contains(other));
    assert!(drawn.interpolates(class));
    let rope = |length: f32, data: Option<u32>| Rope {
        length,
        data: DatablockRef(data),
        ..Rope::default()
    };
    let blocks = OneDatablock(Value::Str(b"art/crate.dts".to_vec()));
    let draw = drawn.draw(class, &rope(3.0, Some(7)), &blocks).unwrap();
    assert_eq!(
        draw,
        ShapeDraw::new("art/crate.dts").scaled([1.0, 2.0, 3.0])
    );
    // No datablock, an unknown one, a class that is not drawn: no shape.
    assert_eq!(drawn.draw(class, &rope(3.0, None), &blocks), None);
    assert_eq!(drawn.draw(class, &rope(3.0, Some(8)), &blocks), None);
    assert_eq!(drawn.draw(other, &rope(3.0, Some(7)), &blocks), None);
    // A hostile scale is not drawn: not finite, or past the cap.
    for scale in [
        f32::NAN,
        f32::INFINITY,
        -f32::INFINITY,
        draw::MAX_SCALE * 2.0,
    ] {
        assert_eq!(drawn.draw(class, &rope(scale, Some(7)), &blocks), None);
    }
    // A record of another type is no shape either.
    let marker = PropData::default();
    assert_eq!(drawn.draw(class, &marker, &NoDatablocks), None);
}

#[test]
fn a_class_is_drawn_once_and_only_interpolated_classes_are_blended() {
    let class = PluginClass::ghost::<Rope>("Rope", Replicate::Predicted);
    let mut drawn = DrawnShapes::default();
    drawn.insert(
        class,
        ShapeFn::new::<Rope>(|_, _| Some(ShapeDraw::new("a.dts"))),
    );
    drawn.insert(
        class,
        ShapeFn::new::<Rope>(|_, _| Some(ShapeDraw::new("b.dts"))),
    );
    assert_eq!(drawn.classes().count(), 1);
    let shape = drawn.draw(class, &Rope::default(), &NoDatablocks).unwrap();
    assert_eq!(shape.shape, "a.dts");
    assert!(!drawn.interpolates(class));
    assert!(class.is_record::<Rope>() && !class.is_record::<PropData>());
}

#[test]
fn an_empty_shape_name_is_not_drawn_and_a_skin_rides_along() {
    assert!(!ShapeDraw::new("").is_drawable());
    let draw = ShapeDraw::new("a.dts").skinned("skin.png");
    assert!(draw.is_drawable());
    assert_eq!(draw.skin.as_deref(), Some("skin.png"));
}

/// A hand-written record whose `level` is sent while bit `READ` is dirty and which keeps the bits `KEEP`.
#[derive(Debug, Clone, PartialEq, Default)]
struct Glow<const READ: u32, const KEEP: u32> {
    level: Option<f32>,
}

impl<const READ: u32, const KEEP: u32> Layout for Glow<READ, KEEP> {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.level.layout(c, "level", FieldSpec::DEFAULT)
    }
}

impl<const READ: u32, const KEEP: u32> NetRecord for Glow<READ, KEEP> {
    fn merge(&mut self, sent: &Self) {
        self.level.merge_from(&sent.level);
    }

    fn datablock(&self) -> Option<u32> {
        None
    }

    fn transform(&self) -> Option<crate::codec::Xf> {
        None
    }

    fn script_fields(&self) -> Vec<(String, u32, Value)> {
        Vec::new()
    }
}

impl<const READ: u32, const KEEP: u32> GhostRecord for Glow<READ, KEEP> {
    fn pack(view: &dyn ObjectView, mask: u32, _ctx: &PackCtx) -> (Self, u32) {
        let level = (mask & READ != 0).then(|| crate::fld(view, "level", 0));
        (Glow { level }, KEEP)
    }
}

#[test]
fn a_ghost_record_that_uses_the_blocks_mask_bits_is_refused() {
    let class = |c: PluginClass| PluginClasses::new([c]);
    let reads = PluginClass::ghost::<Glow<{ 1 << 30 }, 0>>("Glow", Replicate::Interpolated);
    assert_eq!(
        class(reads).unwrap_err(),
        PluginClassError::BlockMask("Glow".into())
    );
    let keeps = PluginClass::ghost::<Glow<0, { 1 << 31 }>>("Glow", Replicate::Interpolated);
    assert_eq!(
        class(keeps).unwrap_err(),
        PluginClassError::BlockMask("Glow".into())
    );
    // Bits below the block's are the record's own.
    let own = PluginClass::ghost::<Glow<{ 1 << 29 }, { 1 << 29 }>>("Glow", Replicate::Interpolated);
    assert!(class(own).is_ok());
}

fn still(shape: &str, scale: f32) -> Still {
    Still {
        shape: shape.to_string(),
        scale,
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
    }
}

#[test]
fn a_still_is_drawable_only_with_a_shape_a_scale_the_renderer_takes_and_a_finite_pose() {
    assert!(still("a.dts", 1.0).is_drawable());
    assert!(still("a.dts", super::draw::MAX_SCALE).is_drawable());
    for bad in [
        0.0,
        -1.0,
        f32::NAN,
        f32::INFINITY,
        super::draw::MAX_SCALE * 2.0,
    ] {
        assert!(!still("a.dts", bad).is_drawable(), "scale {bad}");
    }
    assert!(!still("", 1.0).is_drawable());
    let mut off = still("a.dts", 1.0);
    off.position[1] = f32::NAN;
    assert!(!off.is_drawable());
    off = still("a.dts", 1.0);
    off.rotation[3] = f32::INFINITY;
    assert!(!off.is_drawable());
}

#[test]
fn the_stills_of_a_handle_are_the_stills_of_its_clones_and_a_new_set_replaces_the_old() {
    let stills = PluginStills::default();
    let shared = stills.clone();
    assert!(stills.is_empty());
    shared.replace([
        (3, still("a.dts", 1.0)),
        (1, still("b.dts", 2.0)),
        (2, still("", 1.0)),
    ]);
    // The undrawable one is left out, and the rest are in key order.
    assert_eq!(stills.len(), 2);
    assert_eq!(
        stills.with(|m| m.keys().copied().collect::<Vec<_>>()),
        [1, 3]
    );
    assert_eq!(stills, shared);
    shared.replace([(9, still("c.dts", 1.0))]);
    assert_eq!(stills.with(|m| m.keys().copied().collect::<Vec<_>>()), [9]);
    // Two handles with the same stills are equal, whatever they share.
    let other = PluginStills::default();
    other.replace([(9, still("c.dts", 1.0))]);
    assert_eq!(stills, other);
    assert_ne!(stills, PluginStills::default());
}

#[test]
fn a_set_of_stills_is_cut_to_its_cap() {
    let stills = PluginStills::default();
    stills.replace((0..super::draw::MAX_STILLS as u32 * 2).map(|k| (k, still("a.dts", 1.0))));
    assert_eq!(stills.len(), super::draw::MAX_STILLS);
}
