//! Side files of a `.gltf` are read only as far as their buffers' declared lengths, within the buffer cap: a file much longer
//! than its buffer is not read past it, a resolver that answers with more is refused, and buffers whose declared lengths sum
//! past the cap are refused before any file is asked for.

use std::path::PathBuf;

use serde_json::{Value, json};
use torque_import::modelcore::{Detail, Mesh, Model, Node, Primitive};
use torque_import::{ImportError, Options, caps, gltf};

fn free(_: u64) -> bool {
    true
}

/// A one-triangle model's `.gltf` JSON (its buffer a side file `data.bin`) and the buffer's bytes.
fn triangle() -> (Value, Vec<u8>) {
    let model = Model {
        nodes: vec![Node::new("tri", None, [0.0; 3])],
        details: vec![Detail {
            name: "detail2".into(),
            size: 2.0,
        }],
        meshes: vec![Mesh {
            name: "tri".into(),
            node: 0,
            detail: Some(0),
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            primitives: vec![Primitive {
                material: None,
                indices: vec![0, 1, 2],
            }],
            ..Mesh::default()
        }],
        ..Model::default()
    };
    let glb = gltf::write_glb(&model).unwrap();
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let mut doc: Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
    let bin = glb[20 + json_len + 8..].to_vec();
    doc["buffers"][0]["uri"] = json!("data.bin");
    (doc, bin)
}

fn folder(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_side_file_longer_than_its_buffer_is_read_only_that_far() {
    let (doc, bin) = triangle();
    let dir = folder("side-long");
    let mut file = bin.clone();
    file.resize(bin.len() + (32 << 20), 0xab);
    std::fs::write(dir.join("data.bin"), &file).unwrap();
    let mut files = gltf::side_files(&dir);
    let mut asked = Vec::new();
    let mut resolve = |p: &str, max: usize| {
        let got = files(p, max);
        asked.push((max, got.as_ref().map_or(0, Vec::len)));
        got
    };
    let text = serde_json::to_vec(&doc).unwrap();
    let model = gltf::read(&text, &mut resolve, &Options::default(), &mut free).unwrap();
    assert_eq!(model.meshes[0].positions.len(), 3);
    // Asked for the declared length, and given no more.
    assert_eq!(asked, [(bin.len(), bin.len())]);
}

#[test]
fn a_resolver_that_answers_with_more_than_asked_is_refused() {
    let (doc, bin) = triangle();
    let text = serde_json::to_vec(&doc).unwrap();
    let r = gltf::read(
        &text,
        &mut |_, max| Some(vec![0; max + 1]),
        &Options::default(),
        &mut free,
    );
    assert_eq!(
        r.unwrap_err(),
        ImportError::TooLarge {
            what: "a side file's answer",
            bytes: bin.len() as u64 + 1,
            cap: bin.len() as u64,
        }
    );
}

#[test]
fn many_large_side_files_are_refused_before_any_is_read() {
    let (mut doc, _) = triangle();
    let dir = folder("side-many");
    let big = 8usize << 20;
    let count = caps::MAX_BUFFER_BYTES / big + 1;
    std::fs::write(dir.join("big.bin"), vec![0u8; big]).unwrap();
    doc["buffers"] = Value::Array(
        (0..count)
            .map(|_| json!({ "byteLength": big, "uri": "big.bin" }))
            .collect(),
    );
    let text = serde_json::to_vec(&doc).unwrap();
    let mut asked = 0;
    let mut files = gltf::side_files(&dir);
    let mut resolve = |p: &str, max: usize| {
        asked += 1;
        files(p, max)
    };
    let r = gltf::read(&text, &mut resolve, &Options::default(), &mut free);
    assert!(
        matches!(
            r,
            Err(ImportError::TooLarge {
                what: "the buffers",
                ..
            })
        ),
        "{r:?}"
    );
    assert_eq!(asked, 0);
}

#[test]
fn side_files_stay_in_their_folder() {
    let dir = folder("side-inside");
    std::fs::write(dir.join("a.bin"), [1u8, 2, 3]).unwrap();
    let mut files = gltf::side_files(&dir);
    assert_eq!(files("a.bin", 2), Some(vec![1, 2]));
    assert_eq!(files("missing.bin", 2), None);
    // A path that leaves the folder is not read, even if the reader never hands one over.
    assert_eq!(files("../side-inside/a.bin", 8), Some(vec![1, 2, 3]));
    assert!(files("../side-long/data.bin", 8).is_none());
}
