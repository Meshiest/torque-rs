use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Value, json};
use torque_modelcore::math::NO_TURN;
use torque_modelcore::{
    Animation, Detail, Image, Material, Mesh, Model, Node, Primitive, Skin, Track,
};

use super::container::{UriSource, base64, resolve_uri, split};
use super::{read, write_glb};
use crate::{ImportError, Options, caps, dts};

fn png_header(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0]);
    b
}

/// Three nodes, a rigid and a skinned mesh, two materials (one with a carried image), a sequence that loops.
fn sample() -> Model {
    let mut m = Model {
        nodes: vec![
            Node::new("root", None, [0.0, 0.0, 1.0]),
            Node::new("arm", Some(0), [0.5, 0.0, 0.0]),
            Node::new("hand", Some(1), [0.5, 0.0, 0.0]),
        ],
        details: vec![Detail {
            name: "detail2".into(),
            size: 2.0,
        }],
        images: vec![
            Image {
                name: "wood".into(),
                uri: Some("wood.png".into()),
                ..Image::default()
            },
            Image {
                name: "paint".into(),
                data: Some(png_header(4, 4)),
                mime: "image/png".into(),
                ..Image::default()
            },
        ],
        ..Model::default()
    };
    m.materials = vec![
        Material {
            name: "wood".into(),
            base_texture: Some(0),
            ..Material::default()
        },
        Material {
            name: "paint".into(),
            base_texture: Some(1),
            flags: Some(3),
            ..Material::default()
        },
    ];
    let quad = |node, skin: Option<Skin>| Mesh {
        name: format!("part{node}"),
        node,
        detail: Some(0),
        positions: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uv0: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        joints: if skin.is_some() {
            vec![[0, 1, 0, 0]; 4]
        } else {
            Vec::new()
        },
        weights: if skin.is_some() {
            vec![[0.5, 0.5, 0.0, 0.0]; 4]
        } else {
            Vec::new()
        },
        skin,
        primitives: vec![
            Primitive {
                material: Some(0),
                indices: vec![0, 1, 2],
            },
            Primitive {
                material: Some(1),
                indices: vec![0, 2, 3],
            },
        ],
        ..Mesh::default()
    };
    m.meshes.push(quad(1, None));
    let world = m.rest_world();
    m.meshes.push(quad(
        0,
        Some(Skin {
            joints: vec![1, 2],
            inverse_binds: vec![
                torque_modelcore::math::inverse(&world[1]),
                torque_modelcore::math::inverse(&world[2]),
            ],
        }),
    ));
    let h = std::f32::consts::FRAC_1_SQRT_2;
    m.animations.push(Animation {
        name: "wave".into(),
        duration: 1.0,
        keys: 4,
        cyclic: true,
        priority: 2,
        tracks: vec![Track {
            node: 1,
            rotations: vec![
                NO_TURN,
                [0.0, 0.0, h, h],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, h, -h],
            ],
            translations: vec![[0.5, 0.0, 0.0]; 4],
            ..Track::default()
        }],
        ground: vec![[0.0, 0.1, 0.0], [0.0, 0.2, 0.0]],
        ..Animation::default()
    });
    m
}

fn free(_: u64) -> bool {
    true
}

fn read_ok(bytes: &[u8]) -> Result<Model, ImportError> {
    read(bytes, &mut |_, _| None, &Options::default(), &mut free)
}

/// A `.glb`'s JSON as a value and its binary chunk.
fn open(glb: &[u8]) -> (Value, Vec<u8>) {
    let (j, b) = split(glb).unwrap();
    (
        serde_json::from_slice(j).unwrap(),
        b.unwrap_or_default().to_vec(),
    )
}

fn close(doc: &Value, bin: &[u8]) -> Vec<u8> {
    let mut text = serde_json::to_vec(doc).unwrap();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut out = b"glTF".to_vec();
    out.extend(2u32.to_le_bytes());
    out.extend(((28 + text.len() + bin.len()) as u32).to_le_bytes());
    out.extend((text.len() as u32).to_le_bytes());
    out.extend(b"JSON");
    out.extend(text);
    out.extend((bin.len() as u32).to_le_bytes());
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}

/// The sample's `.glb` with its JSON changed by `edit`.
fn edited(edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let (mut doc, bin) = open(&write_glb(&sample()).unwrap());
    edit(&mut doc);
    close(&doc, &bin)
}

#[test]
fn a_written_model_reads_back_as_itself() {
    let m = sample();
    let back = read_ok(&write_glb(&m).unwrap()).unwrap();
    assert_eq!(back.nodes, m.nodes);
    assert_eq!(back.details, m.details);
    assert_eq!(back.meshes, m.meshes);
    assert_eq!(back.animations, m.animations);
    assert_eq!(back.materials.len(), 2);
    assert_eq!(back.materials[1].flags, Some(3));
    assert_eq!(back.images[1].size, Some((4, 4)));
    let shape = dts::to_dts(&back).unwrap();
    let bytes = torque_shapes::write::write_dts(&shape);
    torque_shapes::dts::parse(&bytes, &mut free).unwrap();
}

#[test]
fn a_gltf_with_data_uris_and_side_files_reads_like_its_glb() {
    let glb = write_glb(&sample()).unwrap();
    let (mut doc, bin) = open(&glb);
    // The same buffer once as a side file, once as a data: URI.
    doc["buffers"] = json!([{ "byteLength": bin.len(), "uri": "parts/sample%20data.bin" }]);
    let text = serde_json::to_vec(&doc).unwrap();
    let mut asked = Vec::new();
    let side = read(
        &text,
        &mut |p, max| {
            asked.push((p.to_string(), max));
            Some(bin.clone())
        },
        &Options::default(),
        &mut free,
    )
    .unwrap();
    assert_eq!(asked, [("parts/sample data.bin".to_string(), bin.len())]);
    let b64 = {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in bin.chunks(3) {
            let w = (u32::from(c[0]) << 16)
                | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                | u32::from(*c.get(2).unwrap_or(&0));
            for k in 0..=c.len() {
                s.push(T[((w >> (18 - 6 * k)) & 63) as usize] as char);
            }
        }
        s
    };
    doc["buffers"] = json!([{ "byteLength": bin.len(), "uri": format!("data:application/octet-stream;base64,{b64}") }]);
    let inline = read_ok(&serde_json::to_vec(&doc).unwrap()).unwrap();
    assert_eq!(side.meshes, inline.meshes);
    assert_eq!(inline.meshes, read_ok(&glb).unwrap().meshes);
    // A side file the resolver does not have.
    doc["buffers"] = json!([{ "byteLength": bin.len(), "uri": "missing.bin" }]);
    let e = read(
        &serde_json::to_vec(&doc).unwrap(),
        &mut |_, _| None,
        &Options::default(),
        &mut free,
    );
    assert_eq!(e.unwrap_err(), ImportError::Missing("missing.bin".into()));
}

#[test]
fn uris_are_relative_paths_or_data_only() {
    for bad in [
        "http://example.com/a.bin",
        "https://example.com/a.bin",
        "file:///etc/passwd",
        "C:/Windows/a.bin",
        "c:a.bin",
        "/etc/passwd",
        "../a.bin",
        "a/../../b.bin",
        "%2e%2e/a.bin",
        "a\\..\\b.bin",
        "%5c%5cserver%5cshare",
        "a/%00.bin",
        "",
        "%zz",
        "a/b:c.bin",
    ] {
        assert!(
            matches!(resolve_uri(bad, 100), Err(ImportError::Uri(_))),
            "{bad}"
        );
    }
    assert_eq!(
        resolve_uri("./tex/a%20b.png", 100).unwrap(),
        UriSource::File("tex/a b.png".into())
    );
    assert!(matches!(
        resolve_uri("data:text/plain,hello", 100),
        Err(ImportError::Unsupported(_))
    ));
    assert!(matches!(
        resolve_uri("data:;base64,AAAAAAAA", 5),
        Err(ImportError::TooLarge { .. })
    ));
    assert!(matches!(
        resolve_uri(&"a".repeat(caps::MAX_URI_BYTES + 1), 100),
        Err(ImportError::Uri(_))
    ));
    assert_eq!(base64(b"aGVsbG8=", 10).unwrap(), b"hello");
    assert_eq!(base64(b"aGVsbG8", 10).unwrap(), b"hello");
    assert!(base64(b"aGV$bG8=", 10).is_err());
    assert!(base64(b"a", 10).is_err());
}

#[test]
fn the_container_is_checked() {
    let glb = write_glb(&sample()).unwrap();
    let mut long = glb.clone();
    long[8..12].copy_from_slice(&(glb.len() as u32 + 1).to_le_bytes());
    assert!(matches!(read_ok(&long), Err(ImportError::Malformed(_))));
    let mut chunk = glb.clone();
    chunk[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(read_ok(&chunk), Err(ImportError::Malformed(_))));
    let mut version = glb.clone();
    version[4] = 1;
    assert!(matches!(
        read_ok(&version),
        Err(ImportError::Unsupported(_))
    ));
    for n in [0, 4, 11, 12, 19, 20, 40] {
        assert!(read_ok(&glb[..n]).is_err(), "cut at {n}");
    }
    let mut huge = vec![b' '; caps::MAX_FILE_BYTES + 1];
    huge[0] = b'{';
    assert!(matches!(read_ok(&huge), Err(ImportError::TooLarge { .. })));
}

#[test]
fn oversized_counts_are_refused_before_the_work() {
    type Edit = Box<dyn FnOnce(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "buffer",
            Box::new(|d| d["buffers"][0]["byteLength"] = json!(1u64 << 40)),
        ),
        (
            "accessor count",
            Box::new(|d| d["accessors"][0]["count"] = json!(caps::MAX_ACCESSOR_ELEMENTS + 1)),
        ),
        (
            "accessor past its view",
            Box::new(|d| d["accessors"][0]["count"] = json!(100_000)),
        ),
        (
            "view past its buffer",
            Box::new(|d| d["bufferViews"][0]["byteLength"] = json!(1u64 << 30)),
        ),
        (
            "accessor offset",
            Box::new(|d| d["accessors"][0]["byteOffset"] = json!(1u64 << 33)),
        ),
        (
            "stride",
            Box::new(|d| {
                let v = d["accessors"][0]["bufferView"].as_u64().unwrap() as usize;
                d["bufferViews"][v]["byteStride"] = json!(4);
            }),
        ),
        (
            "sparse",
            Box::new(|d| {
                d["accessors"][0]["sparse"] = json!({
                    "count": 1, "indices": { "bufferView": 0, "componentType": 5125 }, "values": { "bufferView": 0 }
                })
            }),
        ),
        (
            "key count",
            Box::new(|d| {
                d["animations"][0]["extras"]["torque"]["keys"] = json!(caps::MAX_KEYS + 1)
            }),
        ),
        (
            "duration",
            Box::new(|d| d["animations"][0]["extras"]["torque"]["duration"] = json!(1e9)),
        ),
        (
            "image size",
            Box::new(|d| {
                let png = png_header(100_000, 4);
                let b64: String = png
                    .chunks(3)
                    .flat_map(|c| {
                        const T: &[u8; 64] =
                            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
                        let w = (u32::from(c[0]) << 16)
                            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
                            | u32::from(*c.get(2).unwrap_or(&0));
                        (0..=c.len())
                            .map(move |k| T[((w >> (18 - 6 * k)) & 63) as usize] as char)
                            .collect::<Vec<_>>()
                    })
                    .collect();
                d["images"][0] = json!({ "uri": format!("data:image/png;base64,{b64}") });
            }),
        ),
        (
            "image not an image",
            Box::new(|d| d["images"][0] = json!({ "uri": "data:image/png;base64,AAAA" })),
        ),
        (
            "index past vertices",
            Box::new(|d| d["accessors"][0]["count"] = json!(2)),
        ),
    ];
    for (what, edit) in cases {
        let r = read_ok(&edited(edit));
        assert!(r.is_err(), "{what}: {r:?}");
    }
}

#[test]
fn the_node_tree_is_checked() {
    // A cycle, two parents, a node its own child.
    for children in [
        json!([[1], [0], []]),
        json!([[2], [2], []]),
        json!([[0], [], []]),
    ] {
        let r = read_ok(&edited(|d| {
            for (i, c) in children.as_array().unwrap().iter().enumerate() {
                d["nodes"][i]["children"] = c.clone();
            }
        }));
        assert!(
            matches!(r, Err(ImportError::Malformed(_))),
            "{children}: {r:?}"
        );
    }
    // A chain deeper than the cap.
    let n = caps::MAX_NODE_DEPTH + 2;
    let nodes: Vec<Value> = (0..n)
        .map(|i| {
            if i + 1 < n {
                json!({ "children": [i + 1] })
            } else {
                json!({})
            }
        })
        .collect();
    let doc = json!({ "asset": { "version": "2.0" }, "nodes": nodes });
    let r = read_ok(&serde_json::to_vec(&doc).unwrap());
    assert!(matches!(r, Err(ImportError::Malformed(_))), "{r:?}");
    // Too many nodes.
    let nodes: Vec<Value> = (0..caps::MAX_NODES + 1).map(|_| json!({})).collect();
    let doc = json!({ "asset": { "version": "2.0" }, "nodes": nodes });
    assert!(matches!(
        read_ok(&serde_json::to_vec(&doc).unwrap()),
        Err(ImportError::Cap { .. })
    ));
}

#[test]
fn a_budget_that_runs_out_stops_the_read() {
    let glb = write_glb(&sample()).unwrap();
    let mut left = 200u64;
    let r = read(&glb, &mut |_, _| None, &Options::default(), &mut |u| {
        left = left.saturating_sub(u);
        left > 0
    });
    assert_eq!(r.unwrap_err(), ImportError::OutOfBudget);
}

/// A small deterministic generator for the fuzz.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One mutation of `seed`: cut short, bits flipped, words overwritten with hostile counts, or a number of the JSON replaced.
fn mutate(seed: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    match rng.below(4) {
        0 => b.truncate(rng.below(b.len() + 1)),
        1 => {
            for _ in 0..1 + rng.below(8) {
                let at = rng.below(b.len());
                b[at] ^= 1 << rng.below(8);
            }
        }
        2 => {
            const HOSTILE: [u32; 7] = [
                0,
                1,
                0xffff_ffff,
                0x7fff_ffff,
                0x8000_0000,
                0xffff,
                0x1_0000,
            ];
            for _ in 0..1 + rng.below(3) {
                let at = rng.below(b.len().saturating_sub(4)) & !3;
                b[at..at + 4].copy_from_slice(&HOSTILE[rng.below(HOSTILE.len())].to_le_bytes());
            }
        }
        _ => {
            let (doc, bin) = match split(&b) {
                Ok((j, bin)) => (j.to_vec(), bin.map(<[u8]>::to_vec).unwrap_or_default()),
                Err(_) => return b,
            };
            let text = String::from_utf8_lossy(&doc).into_owned();
            let numbers: Vec<(usize, usize)> = {
                let bytes = text.as_bytes();
                let mut out = Vec::new();
                let mut i = 0;
                while i < bytes.len() {
                    if bytes[i].is_ascii_digit() {
                        let s = i;
                        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                            i += 1;
                        }
                        out.push((s, i));
                    }
                    i += 1;
                }
                out
            };
            if numbers.is_empty() {
                return b;
            }
            let (s, e) = numbers[rng.below(numbers.len())];
            let hostile = [
                "0",
                "1",
                "4294967295",
                "4294967296",
                "1e30",
                "-1",
                "65535",
                "65536",
                "3",
                "999999999999",
            ];
            let new = format!(
                "{}{}{}",
                &text[..s],
                hostile[rng.below(hostile.len())],
                &text[e..]
            );
            let doc: Value = match serde_json::from_str(&new) {
                Ok(v) => v,
                Err(_) => return b,
            };
            return close(&doc, &bin);
        }
    }
    b
}

/// The fuzz: mutated `.glb`s read and made into shapes under `catch_unwind`; every case returns without a panic. The case
/// count comes from `TORQUE_IMPORT_FUZZ_CASES` (default 3,000; the pass condition runs 300,000 in release).
#[test]
fn the_reader_never_panics_on_mutated_files() {
    let cases: usize = std::env::var("TORQUE_IMPORT_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);
    let mut seeds = vec![write_glb(&sample()).unwrap()];
    let mut plain = sample();
    plain.animations.clear();
    plain.meshes.truncate(1);
    seeds.push(write_glb(&plain).unwrap());
    // A file without this crate's extras: the conventions' path.
    let (mut doc, bin) = open(&seeds[0]);
    doc.as_object_mut().unwrap().remove("extras");
    for n in doc["nodes"].as_array_mut().unwrap() {
        n.as_object_mut().unwrap().remove("extras");
    }
    for a in doc["animations"].as_array_mut().unwrap() {
        a.as_object_mut().unwrap().remove("extras");
        for s in a["samplers"].as_array_mut().unwrap() {
            s["interpolation"] = json!("CUBICSPLINE");
        }
    }
    seeds.push(close(&doc, &bin));
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let (mut panics, mut read_back) = (0usize, 0usize);
    for case in 0..cases {
        let seed = &seeds[case % seeds.len()];
        let bytes = mutate(seed, &mut rng);
        let ok = catch_unwind(AssertUnwindSafe(|| {
            let mut budget = 50_000_000u64;
            let mut charge = |u: u64| {
                budget = budget.saturating_sub(u);
                budget > 0
            };
            let Ok(model) = read(
                &bytes,
                &mut |_, max| Some(vec![0; max.min(64)]),
                &Options::default(),
                &mut charge,
            ) else {
                return false;
            };
            let Ok(shape) = dts::to_dts(&model) else {
                return false;
            };
            let out = torque_shapes::write::write_dts(&shape);
            torque_shapes::dts::parse(&out, &mut free).is_ok()
        }));
        match ok {
            Ok(true) => read_back += 1,
            Ok(false) => {}
            Err(_) => panics += 1,
        }
    }
    println!("{cases} cases: {panics} panics, {read_back} read back as shapes");
    assert_eq!(panics, 0);
}

#[test]
fn a_json_of_many_small_values_is_refused_before_it_is_parsed() {
    // 8 MB of empty materials: millions of objects the parse would build, each far larger than its two bytes of text.
    let mut text = br#"{"asset":{"version":"2.0"},"materials":["#.to_vec();
    while text.len() < 8 << 20 {
        text.extend_from_slice(b"{},");
    }
    text.extend_from_slice(b"{}]}");
    let r = read_ok(&text);
    assert!(
        matches!(
            r,
            Err(ImportError::Cap {
                what: "objects, arrays and strings in the JSON",
                ..
            })
        ),
        "{r:?}"
    );
    // Strings are skipped with their escapes: braces inside them are not counted.
    assert_eq!(super::container::json_values(br#"{"a":"x\"{[{"}"#, 100), 3);
    // A file a server sends has a smaller JSON cap.
    let mut big = br#"{"asset":{"version":"2.0"},"extras":""#.to_vec();
    big.resize(caps::MAX_NATIVE_JSON_BYTES + 10, b'a');
    big.extend_from_slice(b"\"}");
    assert!(read_ok(&big).is_ok());
    let r = crate::native::dts_bytes(&big, &mut free);
    assert!(
        matches!(
            r,
            Err(ImportError::TooLarge {
                what: "the JSON",
                ..
            })
        ),
        "{r:?}"
    );
}

#[test]
fn a_shape_whose_sequence_claims_too_many_keys_is_refused() {
    let m = sample();
    let mut shape = dts::to_dts(&m).unwrap();
    shape.shape.sequences[0].num_keyframes = i32::MAX;
    assert!(matches!(
        dts::from_dts(&shape),
        Err(ImportError::Cap { .. })
    ));
}
