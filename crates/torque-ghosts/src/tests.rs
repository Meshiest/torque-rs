//! The framework's and the engine layouts' tests: each layout's `(field, width)` list, bit totals, round trips, the dirty-word
//! rules of AudioEmitter and the motion quantisers.

use std::collections::HashMap;
use std::fmt::Debug;

use torque_net::bits::{BitReader, BitWriter};
use torque_net::huffman::Table;

use crate::audio::{self, AudioEffects, AudioEmitter};
use crate::codec::{
    Codec, Enc, Layout, NetStr, NetWidths, R, ReadCtx, datablock_id, decode, encode, link,
    mask_bits, wrap_ranged,
};
use crate::markers::{pack_mission_marker, pack_spawn_sphere, pack_waypoint};
use crate::motion::{CPoint, Velocity, speed_length};
use crate::parts::{GameBase, ShapeBasePart, pack_gamebase};
use crate::props::{pack_ts_static, pack_vehicle_blocker};
use crate::weather::pack_lightning;
use crate::{Link, ObjectView, PackCtx, Value, free_charge};

mod tge_db;
mod tge_set;
mod tge_shapes;
mod tge_tails;
mod tge_world;

// Helpers

/// A view backed by maps; anything not in it is `Null`.
#[derive(Default)]
struct MapView {
    vals: HashMap<(String, usize), Value>,
    links: HashMap<String, Link>,
    nets: HashMap<(String, usize), NetStr>,
    meshes: Option<usize>,
}

impl MapView {
    fn with(mut self, name: &str, index: usize, v: Value) -> MapView {
        self.vals.insert((name.to_string(), index), v);
        self
    }

    fn floats(mut self, name: &str, xs: &[f32]) -> MapView {
        for (i, x) in xs.iter().enumerate() {
            self.vals.insert((name.to_string(), i), Value::F32(*x));
        }
        self
    }
}

impl ObjectView for MapView {
    fn get(&self, field: &str, index: usize) -> Value {
        self.vals
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn link(&self, field: &str) -> Link {
        self.links.get(field).copied().unwrap_or(Link::Null)
    }

    fn net_string(&self, field: &str, index: usize) -> NetStr {
        self.nets
            .get(&(field.to_string(), index))
            .cloned()
            .unwrap_or(NetStr::Null)
    }

    fn mesh_objects(&self) -> Option<usize> {
        self.meshes
    }
}

fn writer() -> BitWriter {
    let mut w = BitWriter::new();
    w.enable_string_buffer();
    w
}

fn reader(bytes: &[u8], bits: usize) -> BitReader<'_> {
    let mut r = BitReader::with_bit_len(bytes, bits);
    r.enable_string_buffer();
    r
}

fn table() -> Table {
    Table::torque()
}

/// The widths the tests run with: torque-net's defaults.
fn widths() -> NetWidths {
    NetWidths::engine_default()
}

fn pack_ctx() -> PackCtx {
    PackCtx::new(widths())
}

/// The datablock id's width, as a bit count.
fn db_bits() -> usize {
    usize::from(widths().datablock_bits)
}

/// The bits `rec` takes on the wire.
fn bit_total<L: Layout>(rec: &L, t: &Table) -> usize {
    let mut w = writer();
    encode(rec, &mut w, t, widths()).unwrap();
    w.bits()
}

/// Writes `rec` and reads it back as the first update of a new object.
fn round_trip<L: Layout + PartialEq + Debug>(rec: &L, t: &Table) -> L {
    let mut w = writer();
    encode(rec, &mut w, t, widths()).unwrap();
    let bits = w.bits();
    let bytes = w.into_bytes();
    let mut r = reader(&bytes, bits);
    let back: L = decode(&mut r, t, &mut free_charge(), &ReadCtx::new(widths(), true)).unwrap();
    assert_eq!(r.bits_left(), 0);
    back
}

/// The `(field, width)` list a layout makes for a record (strings have the width the string coder took).
fn layout_log<L: Layout>(rec: &L, t: &Table, cp: [f32; 3]) -> Vec<(&'static str, usize)> {
    let mut w = writer();
    w.set_compression_point(cp);
    let mut copy = rec.clone();
    let mut c = Enc::logging(&mut w, t, widths());
    copy.layout(&mut c).unwrap();
    c.into_log()
}

/// A golden: `name:width` words, `name:width*n` for a repeat, `name:s` for a string (any width).
fn golden(spec: &str) -> Vec<(String, Option<usize>)> {
    let mut out = Vec::new();
    for word in spec.split_whitespace() {
        let (head, times) = match word.split_once('*') {
            Some((h, n)) => (h, n.parse::<usize>().unwrap()),
            None => (word, 1),
        };
        let (name, width) = head.rsplit_once(':').unwrap();
        let width = if width == "s" {
            None
        } else {
            Some(width.parse::<usize>().unwrap())
        };
        for _ in 0..times {
            out.push((name.to_string(), width));
        }
    }
    out
}

fn check_log<L: Layout>(rec: &L, spec: &str) {
    check_log_at(rec, spec, [0.0; 3]);
}

fn check_log_at<L: Layout>(rec: &L, spec: &str, cp: [f32; 3]) {
    let t = table();
    let got = layout_log(rec, &t, cp);
    let want = golden(spec);
    let got_s: Vec<String> = got.iter().map(|(n, w)| format!("{n}:{w}")).collect();
    let want_s: Vec<String> = want
        .iter()
        .map(|(n, w)| match w {
            Some(w) => format!("{n}:{w}"),
            None => format!("{n}:s"),
        })
        .collect();
    assert_eq!(got.len(), want.len(), "\ngot  {got_s:?}\nwant {want_s:?}");
    for (i, ((gn, gw), (wn, ww))) in got.iter().zip(&want).enumerate() {
        assert_eq!(
            *gn,
            wn.as_str(),
            "field {i}\ngot  {got_s:?}\nwant {want_s:?}"
        );
        if let Some(ww) = ww {
            assert_eq!(gw, ww, "width of field {i} ({gn})\ngot  {got_s:?}");
        }
    }
}

// The engine's layouts

#[test]
fn tsstatic_and_vehicleblocker_are_fixed_float_blocks() {
    let t = table();
    let mut view = MapView::default().with("shape", 0, Value::str("a/b.dts"));
    for i in 0..16 {
        view = view.with("matrix", i, Value::F32(i as f32));
    }
    let view = view
        .floats("scale", &[1.0, 2.0, 3.0])
        .floats("box", &[4.0, 5.0, 6.0]);
    let s = pack_ts_static(&view);
    assert_eq!(s.matrix[5].get(), 5.0);
    assert_eq!(s.shape, b"a/b.dts".to_vec());
    check_log(&s, "ts.matrix:512 ts.scale:96 ts.shape:s");
    let strings = {
        let mut w = writer();
        torque_net::strcoder::write_string(&mut w, &t, b"a/b.dts", 255);
        w.bits()
    };
    assert_eq!(bit_total(&s, &t), 512 + 96 + strings);
    assert_eq!(round_trip(&s, &t), s);
    let b = pack_vehicle_blocker(&view);
    assert_eq!(bit_total(&b, &t), 704);
    check_log(&b, "blocker.matrix:512 blocker.scale:96 blocker.size:96");
    assert_eq!(b.size[2].get(), 6.0);
    assert_eq!(round_trip(&b, &t), b);
}

#[test]
fn audio_emitter_with_only_a_profile_is_222_bits_and_a_datablock_id() {
    let t = table();
    let view = MapView::default()
        .with("profile", 0, Value::Id(Some(30)))
        .with("looping", 0, Value::Bool(true))
        .with("is_3d", 0, Value::Bool(true))
        .with("volume", 0, Value::F32(1.0))
        .with("min_distance", 0, Value::F32(1.0))
        .with("max_distance", 0, Value::F32(100.0))
        .with("cone_inside", 0, Value::Int(360))
        .with("cone_outside", 0, Value::Int(360))
        .with("cone_outside_volume", 0, Value::F32(1.0))
        .floats("cone_vector", &[0.0, 0.0, 1.0])
        .with("loop_count", 0, Value::Int(-1))
        // `outsideAmbient` is true by default, which keeps its group off.
        .with("outside_ambient", 0, Value::Bool(true));
    let (a, effects) = pack_audio(&view, 3);
    assert!(!effects.clear_use_profile_description);
    assert_eq!(bit_total(&a, &t), 222 + db_bits());
    assert_eq!(a.profile, Some(Some(30)));
    assert!(a.description.is_none() && a.file_name.is_none() && a.volume.is_none());
    check_log(
        &a,
        &format!(
            "audio.initial:1 audio.transform?:1 audio.transform:193 audio.profile?:1 audio.has:1 audio.datablock:{} \
             audio.description?:1 audio.fileName?:1 audio.useDescription?:1 audio.volume?:1 audio.looping?:1 audio.is3d?:1 \
             audio.minDistance?:1 audio.maxDistance?:1 audio.coneInside?:1 audio.coneOutside?:1 audio.coneOutsideVolume?:1 \
             audio.coneVector?:1 audio.loopCount?:1 audio.minGap?:1 audio.maxGap?:1 audio.kind?:1 audio.outsideAmbient?:1 \
             audio.enableVisualFeedback:8",
            db_bits()
        ),
    );
}

fn pack_audio(view: &dyn ObjectView, mask: u32) -> (AudioEmitter, AudioEffects) {
    audio::pack(view, mask, &pack_ctx())
}

#[test]
fn audio_emitter_dirty_word_rules() {
    let t = table();
    // A non-default volume, a non-looping emitter and a flat (non-3D) one on an initial update: the loop group is cleared by the
    // looping flag, the cone group by the 3D flag.
    let view = MapView::default()
        .with("profile", 0, Value::Id(Some(30)))
        .with("file_name", 0, Value::str("a.wav"))
        .with("volume", 0, Value::F32(0.5))
        .with("looping", 0, Value::Bool(false))
        .with("is_3d", 0, Value::Bool(false))
        .with("kind", 0, Value::Int(2));
    let (a, _) = pack_audio(&view, 3);
    assert_eq!(a.volume.map(|f| f.get()), Some(0.5));
    assert_eq!(a.looping, Some(false));
    assert_eq!(a.is_3d, Some(false));
    assert!(a.loop_count.is_none() && a.min_distance.is_none() && a.cone_vector.is_none());
    assert_eq!(a.kind, Some(2));
    assert_eq!(a.file_name.as_deref(), Some(&b"a.wav"[..]));
    assert_eq!(round_trip(&a, &t), a);
    // A later update sends exactly the groups the host marked dirty.
    let later =
        MapView::default()
            .with("dirty", 0, Value::Int(0x10))
            .with("volume", 0, Value::F32(0.25));
    let (a, _) = pack_audio(&later, 0);
    assert_eq!(a.volume.map(|f| f.get()), Some(0.25));
    assert!(a.profile.is_none() && a.transform.is_none());
    // `use profile description` without a profile is switched off and the host is told.
    let bad = MapView::default().with("use_profile_description", 0, Value::Bool(true));
    let (a, effects) = pack_audio(&bad, 1);
    assert!(effects.clear_use_profile_description);
    assert_eq!(a.use_profile_description, Some(false));
}

// The motion quantisers

#[test]
fn the_velocity_block_follows_the_f32_length() {
    assert_eq!(Velocity::pack([0.0; 3], 10), None);
    // f32(0.02) is below the double 0.02 the length is compared with: no block.
    assert_eq!(Velocity::pack([0.02, 0.0, 0.0], 10), None);
    assert!(Velocity::pack([0.021, 0.0, 0.0], 10).is_some());
    assert_eq!(Velocity::pack([f32::NAN, 0.0, 0.0], 10), None);
    let v = Velocity::pack([10.0, 0.0, 0.0], 10).expect("block");
    assert_eq!(v.speed, 320);
    let back = v.value(10);
    // A 10-bit normal is good to about pi / 1024 radians: 0.03 at speed 10.
    assert!(
        (back[0] - 10.0).abs() < 0.05 && back[1].abs() < 0.05 && back[2].abs() < 0.05,
        "{back:?}"
    );
    // An infinite length: the speed is capped.
    let v = Velocity::pack([1e30, 0.0, 0.0], 15).expect("block");
    assert_eq!(v.speed, 8191);
    assert_eq!(speed_length([3.0, 4.0, 0.0]), 5.0);
    assert_eq!(
        Velocity::pack([300.0, 0.0, 0.0], 10).expect("block").speed,
        8191
    );
}

#[test]
fn the_compressed_point_takes_its_four_forms() {
    let p = CPoint::pack([1.0, 2.0, -3.0], [0.0; 3]);
    let CPoint::Relative { form, axes } = p else {
        panic!("{p:?}")
    };
    assert_eq!(form, 0);
    assert_eq!(
        axes.map(|a| (a.negative, a.magnitude)),
        [(false, 100), (false, 200), (true, 300)]
    );
    assert_eq!(p.value([0.0; 3]), [1.0, 2.0, -3.0]);
    let cp = [100.0, 100.0, 100.0];
    for (q, want) in [
        ([500.0, 100.0, 100.0], 1),
        ([2000.0, 100.0, 100.0], 2),
        ([9000.0, 100.0, 100.0], 3),
    ] {
        let c = CPoint::pack(q, cp);
        let form = match c {
            CPoint::Relative { form, .. } => form,
            CPoint::Absolute(_) => 3,
        };
        assert_eq!(form, want, "{q:?}");
        let back = c.value(cp);
        assert!((back[0] - q[0]).abs() <= 0.01, "{back:?}");
    }
    assert!(matches!(
        CPoint::pack([f32::NAN, 0.0, 0.0], [0.0; 3]),
        CPoint::Absolute(_)
    ));
}

#[test]
fn lightning_is_the_gamebase_part_and_552_bits() {
    let t = table();
    let view = MapView::default().floats("scale", &[1.0, 1.0, 1.0]).with(
        "datablock",
        0,
        Value::Id(Some(5)),
    );
    // Lightning: the GameBase part (98 + the datablock id) + 1 + 552 bits.
    let l = pack_lightning(&view, 0xffff_ffff, &pack_ctx());
    assert_eq!(bit_total(&l, &t), 651 + db_bits());
    check_log(
        &l,
        &format!(
            "gb.scale?:1 gb.scale:96 gb.datablock?:1 gb.datablock:{} lightning.body?:1 lightning.position:96 \
             lightning.scale:96 lightning.strikeWidth:32 lightning.chanceToHitTarget:32 lightning.strikeRadius:32 \
             lightning.boltStartRadius:32 lightning.color:96 lightning.fadeColor:96 lightning.useFog:8 \
             lightning.strikesPerMinute:32",
            db_bits()
        ),
    );
    assert_eq!(round_trip(&l, &t), l);
    // A later update without the body's bits sends the two GameBase flags and the body's flag.
    let later = pack_lightning(&view, 0, &pack_ctx());
    assert_eq!(bit_total(&later, &t), 3);
}

// The framework

/// A record of the framework's compound fields: a link to another ghost, a net string, a datablock reference and a counted
/// list of bytes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Compound {
    link: Option<Option<u32>>,
    name: NetStr,
    datablock: u32,
    list: Vec<u32>,
}

impl Layout for Compound {
    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        link(c, "link", &mut self.link)?;
        c.net_string(&mut self.name)?;
        datablock_id(c, "datablock", &mut self.datablock)?;
        c.seq("list", &mut self.list, 4, 6, 8, |c, v| {
            c.int("list.item", v, 8)
        })
    }
}

#[test]
fn the_compound_fields_have_their_widths_and_round_trip() {
    let t = table();
    let g = usize::from(widths().ghost_bits);
    let rec = Compound {
        link: Some(Some(5)),
        name: NetStr::Slot(7),
        datablock: 40,
        list: vec![1, 2, 3],
    };
    check_log(
        &rec,
        &format!(
            "link:1 link.ghosted:1 link.index:{g} ns.present:1 ns.mapped:1 ns.slot:5 datablock:{} list:4 list.item:8*3",
            db_bits()
        ),
    );
    assert_eq!(round_trip(&rec, &t), rec);
    // An object with no ghost index yet, a null string and the text form.
    for (l, name) in [
        (Some(None), NetStr::Null),
        (None, NetStr::Text(b"text".to_vec())),
    ] {
        let rec = Compound {
            link: l,
            name,
            ..Compound::default()
        };
        assert_eq!(round_trip(&rec, &t).link, rec.link);
        assert_eq!(round_trip(&rec, &t).name, rec.name);
    }
}

#[test]
fn a_datablock_reference_wraps_into_the_connections_range() {
    let w = NetWidths::new(12, 10);
    assert_eq!(w.datablock_range(), (3, 3 + 1023));
    assert_eq!(w.wire_datablock(None), 3);
    assert_eq!(w.wire_datablock(Some(40)), 40);
    // One past the range comes back as the first id.
    assert_eq!(w.wire_datablock(Some(3 + 1024)), 3);
    assert_eq!(wrap_ranged(10, 3, 6), 6);
    assert_eq!(mask_bits(0x1ff, 8), 0xff);
    assert_eq!(mask_bits(7, 32), 7);
}

#[test]
fn the_encoder_cuts_a_list_and_the_decoder_refuses_a_long_count() {
    let t = table();
    let rec = Compound {
        list: (0..20).collect(),
        ..Compound::default()
    };
    // The list holds at most 6 (the named cap).
    assert_eq!(round_trip(&rec, &t).list, (0..6).collect::<Vec<_>>());
    // A count of 15 with the bits for 15 items (above the cap), then a count of 5 with the bits for one.
    for (count, items) in [(15, 15), (5, 1)] {
        let mut w = writer();
        w.write_flag(false);
        w.write_flag(false);
        w.write_bits(0, widths().datablock_bits);
        w.write_bits(count, 4);
        for _ in 0..items {
            w.write_bits(0, 8);
        }
        let n = w.bits();
        let bytes = w.into_bytes();
        let mut r = reader(&bytes, n);
        let got: Result<Compound, _> = decode(
            &mut r,
            &t,
            &mut free_charge(),
            &ReadCtx::new(widths(), true),
        );
        assert!(got.is_err(), "count {count}");
    }
}

#[test]
fn a_decoder_stops_when_the_work_budget_is_spent() {
    let t = table();
    let rec = Compound {
        datablock: 3,
        list: vec![1, 2, 3, 4],
        ..Compound::default()
    };
    let mut w = writer();
    encode(&rec, &mut w, &t, widths()).unwrap();
    let n = w.bits();
    let bytes = w.into_bytes();
    let mut left = 3u64;
    let mut charge = |units: u64| {
        if units > left {
            false
        } else {
            left -= units;
            true
        }
    };
    let mut r = reader(&bytes, n);
    let got: Result<Compound, _> = decode(&mut r, &t, &mut charge, &ReadCtx::new(widths(), true));
    assert!(got.is_err());
    let mut r = reader(&bytes, n);
    let got: Compound = decode(
        &mut r,
        &t,
        &mut free_charge(),
        &ReadCtx::new(widths(), true),
    )
    .unwrap();
    assert_eq!(got, rec);
}

#[test]
fn every_prefix_of_a_record_is_an_error() {
    let t = table();
    let mut view = MapView::default().with("profile", 0, Value::Id(Some(30)));
    view = view.with("file_name", 0, Value::str("a.wav"));
    let (a, _) = pack_audio(&view, 3);
    let mut w = writer();
    encode(&a, &mut w, &t, widths()).unwrap();
    let n = w.bits();
    let bytes = w.into_bytes();
    for cut in 0..n {
        let mut r = reader(&bytes, cut);
        let got: Result<AudioEmitter, _> = decode(
            &mut r,
            &t,
            &mut free_charge(),
            &ReadCtx::new(widths(), true),
        );
        assert!(got.is_err(), "a prefix of {cut} bits decoded");
    }
}

/// A ShapeBase part of the GameBase part only, leaving the class bits from 8 up.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct BasePart(GameBase);

impl ShapeBasePart for BasePart {
    const NEXT_FREE_MASK: u32 = 1 << 8;

    fn layout<C: Codec>(&mut self, c: &mut C) -> R {
        self.0.layout(c)
    }

    fn pack(view: &dyn ObjectView, mask: u32, ctx: &PackCtx) -> (BasePart, u32) {
        (BasePart(pack_gamebase(view, mask, ctx)), 0)
    }

    fn game(&self) -> &GameBase {
        &self.0
    }
}

#[test]
fn the_marker_bodies_follow_the_part_and_its_next_free_bits() {
    let t = table();
    let view = MapView::default()
        .with("name", 0, Value::str("spot"))
        .with("team", 0, Value::Int(-2))
        .with("hidden", 0, Value::Bool(true))
        .floats("sphere", &[1.0, 2.0, 3.0, 4.0]);
    let (w, ret) = pack_waypoint::<BasePart>(&view, 0xffff_ffff, &pack_ctx());
    assert_eq!(ret, 0);
    assert_eq!(w.team, Some(0xffff_fffe));
    check_log(
        &w,
        "gb.scale?:1 gb.scale:96 gb.datablock?:1 marker.transform?:1 marker.transform:193 marker.scale:96 \
         waypoint.name?:1 waypoint.name:s waypoint.team?:1 waypoint.team:32 waypoint.hidden?:1 waypoint.hidden:1",
    );
    assert_eq!(round_trip(&w, &t), w);
    let (s, _) = pack_spawn_sphere::<BasePart>(&view, 0xffff_ffff, &pack_ctx());
    check_log(
        &s,
        "gb.scale?:1 gb.scale:96 gb.datablock?:1 marker.transform?:1 marker.transform:193 marker.scale:96 \
         spawn.sphere?:1 spawn.sphere:128",
    );
    assert_eq!(round_trip(&s, &t), s);
    // Only the transform block's bit.
    let (m, _) = pack_mission_marker::<BasePart>(&view, 1 << 8, &pack_ctx());
    assert_eq!(bit_total(&m, &t), 2 + 1 + 193 + 96);
}
