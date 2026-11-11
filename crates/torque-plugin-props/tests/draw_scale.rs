//! How a client reads a prop datablock's `drawScale` to draw the prop.

use std::collections::BTreeMap;

use torque_engine::net_classes::DatablockFields;
use torque_ghosts::Value;
use torque_ghosts::plugin::DatablockRef;
use torque_plugin_props::client::draw;
use torque_plugin_props::records::PropGhost;

struct Blocks(BTreeMap<(u32, String, u32), Value>);

impl DatablockFields for Blocks {
    fn field(&self, id: u32, name: &str, index: u32) -> Option<&Value> {
        self.0.get(&(id, name.to_string(), index))
    }
}

fn scale_drawn(draw_scale: Option<Value>) -> [f32; 3] {
    let mut fields = BTreeMap::new();
    fields.insert(
        (7, "shapeFile".to_string(), 0),
        Value::Str(b"art/ball.dts".to_vec()),
    );
    if let Some(value) = draw_scale {
        fields.insert((7, "drawScale".to_string(), 0), value);
    }
    let record = PropGhost {
        data_block: DatablockRef(Some(7)),
        ..PropGhost::default()
    };
    draw(&record, &Blocks(fields)).expect("a drawn shape").scale
}

#[test]
fn a_draw_scale_written_as_text_scales_the_drawn_shape() {
    let text = |t: &str| scale_drawn(Some(Value::Str(t.as_bytes().to_vec())));
    assert_eq!(text("1.5 1.5 1.5"), [1.5; 3]);
    assert_eq!(text("1 2 3"), [1.0, 2.0, 3.0]);
    assert_eq!(text("  2   "), [2.0, 1.0, 1.0], "a missing word is 1");
}

#[test]
fn a_draw_scale_that_is_missing_or_a_lone_number_leaves_the_shape_as_it_is() {
    assert_eq!(scale_drawn(None), [1.0; 3]);
    assert_eq!(scale_drawn(Some(Value::F32(2.0))), [2.0, 1.0, 1.0]);
}

#[test]
fn a_draw_scale_that_is_no_safe_number_is_held_to_the_renderers_range_or_one() {
    let text = |t: &str| scale_drawn(Some(Value::Str(t.as_bytes().to_vec())));
    for written in [
        "nan",
        "inf",
        "-inf",
        "1e999",
        "-1",
        "0",
        "abc",
        "",
        "1e-9 1e9 -5",
    ] {
        for s in text(written) {
            assert!(
                s == 1.0 || (0.001..=256.0).contains(&s),
                "{written:?} drew at {s}"
            );
        }
    }
    assert_eq!(text("2\t3\t4"), [2.0, 3.0, 4.0], "tabs separate words too");
    assert_eq!(text("\t2 \t 3"), [2.0, 3.0, 1.0]);
}
