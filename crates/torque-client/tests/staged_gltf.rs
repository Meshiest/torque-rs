//! A `.glb` whose import does not fit a staged shape read's room is tried again on a later tick, unless the room was the whole
//! budget: then it fails once for good and is not read again.

use std::cell::Cell;
use std::rc::Rc;

use torque_client::predict::FileReader;
use torque_client::predict::drive::{MAX_SHAPE_BYTES, SHAPE_WORK_PER_BYTE, ShapeCache, Staged};
use torque_import::modelcore::{Detail, Mesh, Model, Node, Primitive};

/// A cube as a `.glb`, its buffer padded with `n` more bytes: its read, its import and its buffer are each charged by its
/// size.
fn padded_cube(n: usize) -> Vec<u8> {
    let hull = torque_shape_builder::MeshData::hull_box([-0.5; 3], [0.5; 3]);
    let model = Model {
        nodes: vec![Node::new("cube", None, [0.0; 3])],
        details: vec![Detail {
            name: "detail2".into(),
            size: 2.0,
        }],
        meshes: vec![Mesh {
            name: "cube".into(),
            node: 0,
            detail: Some(0),
            positions: hull.positions,
            normals: hull.normals,
            primitives: vec![Primitive {
                material: None,
                indices: hull.indices.iter().map(|&i| u32::from(i)).collect(),
            }],
            ..Mesh::default()
        }],
        ..Model::default()
    };
    let glb = torque_import::gltf::write_glb(&model).unwrap();
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let text = String::from_utf8(glb[20..20 + json_len].to_vec()).unwrap();
    let mut bin = glb[20 + json_len + 8..].to_vec();
    let declared = format!(r#""buffers":[{{"byteLength":{}}}]"#, bin.len());
    assert!(text.contains(&declared));
    bin.resize(bin.len() + n, 0);
    let mut json = text
        .trim_end()
        .replace(
            &declared,
            &format!(r#""buffers":[{{"byteLength":{}}}]"#, bin.len()),
        )
        .into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend(((28 + json.len() + bin.len()) as u32).to_le_bytes());
    out.extend((json.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(json);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}

/// A reader of `file` that counts its reads.
fn counting(file: Vec<u8>) -> (FileReader, Rc<Cell<usize>>) {
    let reads = Rc::new(Cell::new(0));
    let counted = Rc::clone(&reads);
    let reader: FileReader = Rc::new(move |_: &[u8]| {
        counted.set(counted.get() + 1);
        Some(file.clone())
    });
    (reader, reads)
}

#[test]
fn a_glb_too_costly_for_a_whole_ticks_room_fails_once_and_is_not_read_again() {
    let (reader, reads) = counting(padded_cube(6 << 20));
    let mut cache = ShapeCache::default();
    let budget = SHAPE_WORK_PER_BYTE * MAX_SHAPE_BYTES as u64;
    for tick in 0..4 {
        let mut work = 0;
        let got = cache.get_staged(b"kit/big.glb", Some(&reader), &mut work, budget, tick, 16);
        assert!(matches!(got, Staged::Failed), "tick {tick}: {got:?}");
    }
    assert_eq!(reads.get(), 1);
}

#[test]
fn a_glb_refused_after_a_busy_tick_loads_on_the_next() {
    let (reader, reads) = counting(padded_cube(8 << 20));
    let mut cache = ShapeCache::default();
    let budget = 40 << 20;
    // Earlier shapes spent half the tick: room for the largest file's read, not for this one's import.
    let mut work = 20 << 20;
    let got = cache.get_staged(b"kit/cube.glb", Some(&reader), &mut work, budget, 0, 16);
    assert!(matches!(got, Staged::Refused { .. }), "{got:?}");
    let mut work = 0;
    let got = cache.get_staged(b"kit/cube.glb", Some(&reader), &mut work, budget, 1, 16);
    assert!(matches!(got, Staged::Loaded(_)), "{got:?}");
    assert_eq!(reads.get(), 2);
}
