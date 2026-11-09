//! A `.glb` whose import does not fit a staged shape read's room is refused with that room, as any step is: the caller decides
//! whether it can ever fit (a host fails it once the room was a whole tick's share). Each call reads with nothing spent yet and
//! a budget of what the tick has left, as a host does.

use std::cell::Cell;
use std::rc::Rc;

use torque_client::predict::FileReader;
use torque_client::predict::drive::{ShapeCache, Staged};
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

/// A whole tick's share in these tests: room for the largest file's read.
const TICK: u64 = 40 << 20;

#[test]
fn a_glb_too_costly_for_a_whole_tick_is_refused_with_the_whole_room() {
    let (reader, reads) = counting(padded_cube(15 << 20));
    let mut cache = ShapeCache::default();
    let mut work = 0;
    let got = cache.get_staged(b"kit/big.glb", Some(&reader), &mut work, TICK, 0, 16);
    // The room it was refused with is the whole tick's: a host gives it up for good.
    assert!(
        matches!(got, Staged::Refused { room } if room == TICK),
        "{got:?}"
    );
    assert_eq!(reads.get(), 1);
}

#[test]
fn a_glb_refused_after_a_busy_tick_loads_on_the_next() {
    let (reader, reads) = counting(padded_cube(8 << 20));
    let mut cache = ShapeCache::default();
    // Earlier shapes spent half the tick: what is left holds the largest file's read, not this one's import.
    let mut work = 0;
    let got = cache.get_staged(b"kit/cube.glb", Some(&reader), &mut work, TICK / 2, 0, 16);
    assert!(
        matches!(got, Staged::Refused { room } if room == TICK / 2),
        "{got:?}"
    );
    let mut work = 0;
    let got = cache.get_staged(b"kit/cube.glb", Some(&reader), &mut work, TICK, 1, 16);
    assert!(matches!(got, Staged::Loaded(_)), "{got:?}");
    assert_eq!(reads.get(), 2);
}

#[test]
fn a_collision_shape_of_megabytes_loads_once_for_a_small_share_of_the_tick() {
    use torque_client::predict::TICK_WORK;
    use torque_client::predict::drive::SHAPE_LOAD_TICK_WORK;
    let (reader, reads) = counting(padded_cube(3 << 19));
    let mut cache = ShapeCache::default();
    // A tick whose moves left too little room: nothing is read, and nothing is remembered.
    let mut work = TICK_WORK - SHAPE_LOAD_TICK_WORK / 2;
    assert!(
        cache
            .get(b"kit/big.glb", Some(&reader), &mut work, TICK_WORK)
            .is_none()
    );
    assert_eq!(
        (reads.get(), work),
        (0, TICK_WORK - SHAPE_LOAD_TICK_WORK / 2)
    );
    // The next tick loads it, the file's megabytes charged to the load's own budget.
    let mut work = 0;
    assert!(
        cache
            .get(b"kit/big.glb", Some(&reader), &mut work, TICK_WORK)
            .is_some()
    );
    assert_eq!((reads.get(), work), (1, SHAPE_LOAD_TICK_WORK));
    assert!(work * 2 <= TICK_WORK);
    // Kept: later ticks neither read it again nor pay for it.
    let mut work = 0;
    assert!(
        cache
            .get(b"KIT/BIG.GLB", Some(&reader), &mut work, TICK_WORK)
            .is_some()
    );
    assert_eq!((reads.get(), work), (1, 0));
}

#[test]
fn the_loads_of_one_tick_share_one_load_budget_and_the_rest_load_on_later_ticks() {
    use torque_client::predict::TICK_WORK;
    let (a, a_reads) = counting(padded_cube(2 << 20));
    let (b, b_reads) = counting(padded_cube(2 << 20));
    let mut cache = ShapeCache::default();
    cache.load_budget = 10 << 20;
    cache.start_tick();
    let mut work = 0;
    // The first load spends most of the tick's load budget; the second runs out and is tried again, whole, next tick.
    assert!(
        cache
            .get(b"kit/a.glb", Some(&a), &mut work, TICK_WORK)
            .is_some()
    );
    assert!(
        cache
            .get(b"kit/b.glb", Some(&b), &mut work, TICK_WORK)
            .is_none()
    );
    assert_eq!((a_reads.get(), b_reads.get()), (1, 1));
    // Nothing more is begun this tick.
    assert!(
        cache
            .get(b"kit/b.glb", Some(&b), &mut work, TICK_WORK)
            .is_none()
    );
    cache.start_tick();
    let mut work = 0;
    assert!(
        cache
            .get(b"kit/b.glb", Some(&b), &mut work, TICK_WORK)
            .is_some()
    );
    assert!(
        cache
            .get(b"kit/a.glb", Some(&a), &mut work, TICK_WORK)
            .is_some()
    );
    assert_eq!((a_reads.get(), b_reads.get()), (1, 2));
}
