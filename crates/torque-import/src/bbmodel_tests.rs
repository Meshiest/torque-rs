use super::*;
use serde_json::json;

fn free(_: u64) -> bool {
    true
}

fn opts() -> Options {
    Options {
        box_hull: false,
        ..Options::default()
    }
}

fn read_json(v: &Value) -> Result<Model, ImportError> {
    read(v.to_string().as_bytes(), &opts(), &mut free)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in bytes.chunks(3) {
        let w = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for k in 0..4 {
            out.push(if k <= c.len() {
                B64[((w >> (18 - 6 * k)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A PNG's signature and header of `w` x `h` (the reader reads no further).
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 2, 0, 0, 0, 0, 0, 0, 0]);
    b
}

fn texture(name: &str, w: u32, h: u32) -> Value {
    json!({ "name": name, "uuid": format!("t-{name}"), "uv_width": 16, "uv_height": 16,
        "source": format!("data:image/png;base64,{}", b64(&png(w, h))) })
}

fn cube(name: &str, from: [f64; 3], to: [f64; 3]) -> Value {
    let face = |uv: [f64; 4]| json!({ "uv": uv, "texture": 0 });
    json!({ "name": name, "type": "cube", "uuid": format!("u-{name}"), "from": from, "to": to, "origin": [0, 0, 0],
        "faces": { "north": face([0., 0., 16., 16.]), "east": face([0., 0., 16., 16.]), "south": face([0., 0., 16., 16.]),
            "west": face([0., 0., 16., 16.]), "up": face([0., 0., 8., 8.]), "down": face([8., 8., 16., 16.]) } })
}

fn file(elements: Vec<Value>, outliner: Vec<Value>) -> Value {
    json!({ "meta": { "format_version": "4.10", "model_format": "free" }, "resolution": { "width": 16, "height": 16 },
        "elements": elements, "outliner": outliner, "textures": [texture("brick.png", 16, 16)] })
}

fn close(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
}

#[test]
fn a_cube_is_twelve_triangles_clockwise_from_outside_a_block_a_meter() {
    let m = read_json(&file(
        vec![cube("box", [0., 0., 0.], [16., 32., 16.])],
        vec![json!("u-box")],
    ))
    .unwrap();
    assert_eq!(m.meshes.len(), 1);
    let mesh = &m.meshes[0];
    assert_eq!(mesh.name, "box");
    assert_eq!(mesh.triangles(), 12);
    // y up 32 units is z up 2 m; z 16 is y -1.
    let (lo, hi) = mesh
        .positions
        .iter()
        .fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
            (
                [0, 1, 2].map(|k| lo[k].min(p[k])),
                [0, 1, 2].map(|k| hi[k].max(p[k])),
            )
        });
    assert!(
        close(lo, [0.0, -1.0, 0.0]) && close(hi, [1.0, 0.0, 2.0]),
        "{lo:?} {hi:?}"
    );
    let center = glam::Vec3::new(0.5, -0.5, 1.0);
    for t in mesh.primitives[0].indices.chunks(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| glam::Vec3::from(mesh.positions[i as usize]));
        let n = (b - a).cross(c - a);
        let mid = (a + b + c) / 3.0;
        assert!(n.dot(mid - center) < 0.0, "a triangle facing in");
    }
    // The up face's rectangle is the texture's top-left quarter: its corners' coordinates within 0..0.5.
    // Faces in order north, east, south, west, up, down, four corners each.
    let up = &mesh.uv0[16..20];
    assert!(
        mesh.positions[16..20]
            .iter()
            .all(|p| (p[2] - 2.0).abs() < 1e-6)
    );
    assert!(up.iter().all(|uv| uv[0] <= 0.5 && uv[1] <= 0.5));
    assert_eq!(m.materials[0].name, "brick");
    assert_eq!(m.images[0].size, Some((16, 16)));
    assert!(m.images[0].data.is_some());
}

#[test]
fn turns_inflate_and_face_rotation_place_corners_as_blockbench_does() {
    let mut c = cube("turned", [0., 0., 0.], [16., 16., 16.]);
    c["rotation"] = json!([0, 90, 0]);
    c["origin"] = json!([0, 0, 0]);
    c["inflate"] = json!(1);
    c["faces"]["north"]["rotation"] = json!(90);
    let m = read_json(&file(vec![c], vec![])).unwrap();
    let mesh = &m.meshes[0];
    // Inflated to -1..17, then a quarter turn about y: x' = z, z' = -x.
    for p in &mesh.positions {
        // Torque (x, y, z) = Blockbench (x, -z, y) / 16.
        let bb = [p[0] * 16.0, p[2] * 16.0, -p[1] * 16.0];
        assert!(bb[0] >= -1.0 - 1e-3 && bb[0] <= 17.0 + 1e-3, "{bb:?}");
        assert!(bb[2] <= 1.0 + 1e-3 && bb[2] >= -17.0 - 1e-3, "{bb:?}");
    }
    // A face turned a quarter: its top left corner takes the rectangle's bottom left coordinates (0, 1).
    assert!(mesh.uv0.contains(&[0.0, 1.0]));
}

#[test]
fn groups_are_nodes_turned_about_their_origins() {
    let g = json!({ "name": "arm", "origin": [8, 0, 8], "rotation": [0, 0, 90], "uuid": "g1",
        "children": ["u-a", { "name": "hand", "origin": [8, 16, 8], "children": ["u-b"] }] });
    let m = read_json(&file(
        vec![
            cube("a", [8., 0., 8.], [9., 16., 9.]),
            cube("b", [8., 16., 8.], [9., 17., 9.]),
        ],
        vec![g],
    ))
    .unwrap();
    assert_eq!(m.nodes.len(), 3);
    assert_eq!(m.nodes[1].name, "arm");
    assert_eq!(m.nodes[2].parent, Some(1));
    let world = m.rest_world();
    // A quarter turn about z (Blockbench's) at (8, 0, 8): the point (8, 16, 8) goes to (-8, 0, 8).
    let b = m.placed_positions(1, &world);
    let want = [-8.0 / 16.0, -8.0 / 16.0, 0.0];
    let got = b
        .iter()
        .copied()
        .min_by(|p, q| {
            let d = |x: Vec3| glam::Vec3::from(x).distance_squared(want.into());
            d(*p).total_cmp(&d(*q))
        })
        .unwrap();
    assert!(close(got, want), "{got:?}");
}

#[test]
fn mesh_elements_read_their_faces_clockwise() {
    let mesh = json!({ "name": "ramp", "type": "mesh", "uuid": "m1", "origin": [0, 0, 0], "rotation": [0, 0, 0],
        "vertices": { "a": [0, 0, 0], "b": [16, 0, 0], "c": [16, 0, 16], "d": [0, 0, 16], "e": [0, 16, 0] },
        "faces": {
            "bottom": { "vertices": ["a", "b", "c", "d"], "uv": { "a": [0, 0], "b": [16, 0], "c": [16, 16], "d": [0, 16] },
                "texture": 0 },
            "tri": { "vertices": ["a", "e", "b"], "uv": { "a": [0, 0], "e": [0, 16], "b": [16, 0] }, "texture": "t-brick.png" }
        } });
    let m = read_json(&file(vec![mesh], vec![])).unwrap();
    assert_eq!(m.meshes[0].triangles(), 3);
    assert_eq!(m.meshes[0].primitives.len(), 1);
}

#[test]
fn hostile_files_are_errors() {
    let good = file(vec![cube("box", [0., 0., 0.], [16., 16., 16.])], vec![]);
    assert!(read_json(&good).is_ok());
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v = good.clone();
        f(&mut v);
        read_json(&v)
    };
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "a coordinate past the cap",
            Box::new(|v| v["elements"][0]["to"] = json!([1e7, 0, 0])),
        ),
        (
            "a coordinate that is a string",
            Box::new(|v| v["elements"][0]["from"] = json!(["x", 0, 0])),
        ),
        (
            "a texture that is not base64",
            Box::new(|v| v["textures"][0]["source"] = json!("data:image/png;base64,@@@@")),
        ),
        (
            "a texture that is not base64 at all",
            Box::new(|v| v["textures"][0]["source"] = json!("data:image/png,abc")),
        ),
        (
            "a texture from a URL",
            Box::new(|v| v["textures"][0]["source"] = json!("https://example.com/a.png")),
        ),
        (
            "a texture that is no image",
            Box::new(|v| {
                v["textures"][0]["source"] =
                    json!(format!("data:image/png;base64,{}", b64(b"hello there")))
            }),
        ),
        (
            "an enormous texture",
            Box::new(|v| v["textures"][0] = texture("big", 100_000, 4)),
        ),
        (
            "a face of a texture not there",
            Box::new(|v| v["elements"][0]["faces"]["up"]["texture"] = json!(7)),
        ),
        (
            "a face turned 45 degrees",
            Box::new(|v| v["elements"][0]["faces"]["up"]["rotation"] = json!(45)),
        ),
        (
            "a uv not finite",
            Box::new(|v| v["elements"][0]["faces"]["up"]["uv"] = json!([0, 0, 1e300, 1e300])),
        ),
        ("not an object", Box::new(|v| *v = json!([1, 2, 3]))),
        (
            "an element that is not an object",
            Box::new(|v| v["elements"] = json!([3])),
        ),
    ];
    for (what, f) in &cases {
        assert!(edit(f.as_ref()).is_err(), "{what}");
    }
    // A mesh face naming no vertex, and one of too many corners.
    let mesh = |keys: Vec<&str>| {
        file(
            vec![
                json!({ "type": "mesh", "vertices": { "a": [0, 0, 0], "b": [1, 0, 0], "c": [0, 1, 0] },
                "faces": { "f": { "vertices": keys, "texture": 0 } } }),
            ],
            vec![],
        )
    };
    assert!(read_json(&mesh(vec!["a", "b", "z"])).is_err());
    assert!(read_json(&mesh(vec!["a"; MAX_FACE_CORNERS + 1])).is_err());
    // Numbers past f64 and NaN are not JSON numbers: the parse refuses them.
    let text = good.to_string().replacen("[0.0,0.0,0.0]", "[1e999,0,0]", 1);
    assert!(read(text.as_bytes(), &opts(), &mut free).is_err());
    let text = good.to_string().replacen("[0.0,0.0,0.0]", "[NaN,0,0]", 1);
    assert!(read(text.as_bytes(), &opts(), &mut free).is_err());
    // Too many elements, too deep an outliner, too large a file, out of budget.
    let many = json!({ "elements": vec![json!({ "type": "cube" }); MAX_ELEMENTS + 1] });
    assert!(matches!(read_json(&many), Err(ImportError::Cap { .. })));
    let mut deep = json!("u-box");
    for _ in 0..MAX_DEPTH + 2 {
        deep = json!({ "name": "g", "children": [deep] });
    }
    assert!(read_json(&file(vec![cube("box", [0.; 3], [1.; 3])], vec![deep])).is_err());
    let small = Options {
        max_json_bytes: 64,
        ..opts()
    };
    assert!(matches!(
        read(good.to_string().as_bytes(), &small, &mut free),
        Err(ImportError::TooLarge { .. })
    ));
    let mut none = |_: u64| false;
    assert_eq!(
        read(good.to_string().as_bytes(), &opts(), &mut none),
        Err(ImportError::OutOfBudget)
    );
    // An element listed twice and an unknown one in the outliner: read once, nothing else.
    let twice = file(
        vec![cube("box", [0.; 3], [16.; 3])],
        vec![
            json!("u-box"),
            json!({ "name": "g", "children": ["u-box", "nope"] }),
        ],
    );
    assert_eq!(read_json(&twice).unwrap().meshes.len(), 1);
}

#[test]
fn texture_names_never_repeat() {
    let mut f = file(vec![cube("box", [0.0; 3], [16.0; 3])], vec![json!("u-box")]);
    f["textures"] =
        json!(["a_2.png", "a.png", "a.png", "A.png", "a_3", "a.jpg"].map(|n| texture(n, 16, 16)));
    let names: Vec<String> = read_json(&f)
        .unwrap()
        .images
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(names, ["a_2", "a", "a_3", "A_4", "a_3_2", "a_5"]);
}

#[test]
fn the_reader_never_panics() {
    let mut g = file(
        vec![
            cube("box", [0., 0., 0.], [16., 16., 16.]),
            json!({ "name": "m", "type": "mesh", "uuid": "m1", "origin": [1, 2, 3], "rotation": [10, 20, 30],
                "vertices": { "a": [0, 0, 0], "b": [16, 0, 0], "c": [16, 0, 16], "d": [0, 16, 16] },
                "faces": { "f": { "vertices": ["a", "b", "c", "d"], "uv": { "a": [0, 0], "b": [16, 0] }, "texture": 0 } } }),
        ],
        vec![
            json!({ "name": "g", "origin": [8, 8, 8], "rotation": [0, 45, 0], "children": ["u-box", "m1"] }),
        ],
    );
    g["elements"][0]["rotation"] = json!([22.5, 0, 0]);
    let base = g.to_string().into_bytes();
    let cases: usize = std::env::var("TORQUE_IMPORT_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let alphabet = b"{}[],:\"0123456789.-e trufalsnmxyzuvAB+/=";
    let mut read_back = 0;
    for _ in 0..cases {
        let mut b = base.clone();
        for _ in 0..1 + next() % 4 {
            let at = (next() % b.len() as u64) as usize;
            match next() % 3 {
                0 => b[at] = alphabet[(next() % alphabet.len() as u64) as usize],
                1 => {
                    b.remove(at);
                }
                _ => b.insert(at, alphabet[(next() % alphabet.len() as u64) as usize]),
            }
        }
        let mut budget = 10_000_000u64;
        let mut charge = |u: u64| {
            budget = budget.saturating_sub(u);
            budget > 0
        };
        if read(&b, &opts(), &mut charge).is_ok() {
            read_back += 1;
        }
    }
    assert!(read_back > 0);
}
