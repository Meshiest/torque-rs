use torque_shapes::synth::{Rng, dsq};
use torque_shapes::write::{write_dsq, write_dts};

use super::*;

fn limits() -> Limits {
    Limits::default()
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    out.extend_from_slice(&body);
    out.extend_from_slice(&torque_bytes::crc32(&body).to_be_bytes());
}

/// A 1 by 1 gray PNG the decoder takes: its one row stored in a zlib block.
fn png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut header = Vec::new();
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&1u32.to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    // Filter 0 and one gray sample; the Adler-32 of [0, 128] is (1 + 128) | (1 + 129) << 16.
    let mut zlib = vec![0x78, 0x01, 0x01, 0x02, 0x00, 0xfd, 0xff, 0, 128];
    zlib.extend_from_slice(&(129u32 | (130u32 << 16)).to_be_bytes());
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A 1 by 1 baseline JPEG: one component, flat quantization, and tables of one code each (DC difference 0, then end of block).
fn jpeg() -> Vec<u8> {
    let mut out = vec![0xff, 0xd8, 0xff, 0xdb, 0x00, 0x43, 0x00];
    out.extend_from_slice(&[1; 64]);
    out.extend_from_slice(&[
        0xff, 0xc0, 0x00, 0x0b, 0x08, 0x00, 0x01, 0x00, 0x01, 0x01, 0x01, 0x11, 0x00,
    ]);
    for class in [0x00u8, 0x10] {
        out.extend_from_slice(&[0xff, 0xc4, 0x00, 0x14, class, 1]);
        out.extend_from_slice(&[0; 15]);
        out.push(0);
    }
    out.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]);
    out.push(0x3f);
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

/// A mono 8-bit WAV of four samples.
fn wav() -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    for (value, width) in [(1u32, 2), (1, 2), (8000, 4), (8000, 4), (1, 2), (8, 2)] {
        out.extend_from_slice(&value.to_le_bytes()[..width]);
    }
    out.extend_from_slice(b"data");
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&[128, 130, 128, 126]);
    out
}

/// Ogg's CRC-32 (polynomial 0x04c11db7, no reflection, no final xor).
fn ogg_crc(bytes: &[u8]) -> u32 {
    let mut c: u32 = 0;
    for b in bytes {
        c ^= u32::from(*b) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04c1_1db7
            } else {
                c << 1
            };
        }
    }
    c
}

fn ogg_page(sequence: u32, granule: u64, kind: u8, packets: &[&[u8]]) -> Vec<u8> {
    let mut table = Vec::new();
    let mut body = Vec::new();
    for p in packets {
        table.push(p.len() as u8);
        body.extend_from_slice(p);
    }
    let mut out = b"OggS".to_vec();
    out.extend_from_slice(&[0, kind]);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&9u32.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(table.len() as u8);
    out.extend_from_slice(&table);
    out.extend_from_slice(&body);
    let crc = ogg_crc(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Packs values into bytes least significant bit first, as Vorbis headers are.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, width: u32) {
        for bit in 0..width {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let byte = self.bytes.last_mut().expect("a byte was pushed");
            *byte |= (((value >> bit) & 1) as u8) << (self.used % 8);
            self.used += 1;
        }
    }
}

/// A Vorbis stream of its three headers (one two-entry codebook, one empty floor, one residue, one mapping and one mode) and
/// three audio packets of silence.
fn ogg() -> Vec<u8> {
    let mut ident = vec![1];
    ident.extend_from_slice(b"vorbis");
    ident.extend_from_slice(&0u32.to_le_bytes());
    ident.push(1);
    ident.extend_from_slice(&22_050u32.to_le_bytes());
    ident.extend_from_slice(&[0; 12]);
    ident.extend_from_slice(&[0xb8, 1]);
    let mut comment = vec![3];
    comment.extend_from_slice(b"vorbis");
    comment.extend_from_slice(&4u32.to_le_bytes());
    comment.extend_from_slice(b"test");
    comment.extend_from_slice(&0u32.to_le_bytes());
    comment.push(1);

    let mut bits = Bits::default();
    bits.put(0, 8); // one codebook
    for byte in *b"BCV" {
        bits.put(u32::from(byte), 8); // the codebook's sync pattern
    }
    bits.put(1, 16); // one dimension
    bits.put(2, 24); // two entries
    bits.put(0, 1); // not ordered
    bits.put(0, 1); // not sparse
    bits.put(0, 5); // each of length 1
    bits.put(0, 5);
    bits.put(0, 4); // no lookup
    bits.put(0, 6); // one time-domain transform
    bits.put(0, 16);
    bits.put(0, 6); // one floor
    bits.put(1, 16); // of type 1
    bits.put(0, 5); // with no partitions
    bits.put(0, 2); // multiplier 1
    bits.put(8, 4); // range bits
    bits.put(0, 6); // one residue
    bits.put(0, 16); // of type 0
    bits.put(0, 24); // begin
    bits.put(0, 24); // end
    bits.put(0, 24); // partition size 1
    bits.put(0, 6); // one classification
    bits.put(0, 8); // classified by the codebook
    bits.put(0, 3); // no cascade
    bits.put(0, 1);
    bits.put(0, 6); // one mapping
    bits.put(0, 16);
    bits.put(0, 1); // one submap
    bits.put(0, 1); // no coupling
    bits.put(0, 2); // reserved
    bits.put(0, 8); // time (unused)
    bits.put(0, 8); // floor 0
    bits.put(0, 8); // residue 0
    bits.put(0, 6); // one mode
    bits.put(0, 1); // short blocks
    bits.put(0, 16);
    bits.put(0, 16);
    bits.put(0, 8); // mapping 0
    bits.put(1, 1); // framing
    let mut setup = vec![5];
    setup.extend_from_slice(b"vorbis");
    setup.extend_from_slice(&bits.bytes);

    let mut out = ogg_page(0, 0, 2, &[&ident]);
    out.extend(ogg_page(1, 0, 0, &[&comment, &setup]));
    out.extend(ogg_page(2, 1_024, 4, &[&[0; 8], &[0; 8], &[0; 8]]));
    out
}

/// A `.uft` with one glyph and one sheet and no remap, as `GFont::write` lays it out.
fn uft() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&torque_fonts::uft::VERSION.to_le_bytes());
    out.push(5);
    out.extend_from_slice(b"Arial");
    for v in [14u32, 0, 9, 7, 7, 2] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&[0; 9]);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&png());
    for v in [3i32, 4, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&i32::MAX.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// A `.gft` with one glyph and one sheet and a remap of nothing.
fn gft() -> Vec<u8> {
    let mut out = Vec::new();
    for v in [torque_fonts::gft::GFT_VERSION, 9, 7, 1] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0; 9]);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&png());
    out.extend_from_slice(&[0xff; 512]);
    out
}

/// A glTF of one triangle; its buffer is a `data:` URI unless `uri` is false, then it is the glb's binary chunk.
fn gltf_json(uri: bool) -> String {
    let mut buffer = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        buffer.extend_from_slice(&v.to_le_bytes());
    }
    for i in [0u16, 1, 2, 0] {
        buffer.extend_from_slice(&i.to_le_bytes());
    }
    let source = if uri {
        format!(
            ",\"uri\":\"data:application/octet-stream;base64,{}\"",
            torque_bytes::base64::encode(&buffer)
        )
    } else {
        String::new()
    };
    format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[0]}}],\"nodes\":[{{\"mesh\":0}}],\
         \"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":0}},\"indices\":1}}]}}],\
         \"accessors\":[{{\"bufferView\":0,\"componentType\":5126,\"count\":3,\"type\":\"VEC3\",\"min\":[0,0,0],\"max\":[1,1,0]}},\
         {{\"bufferView\":1,\"componentType\":5123,\"count\":3,\"type\":\"SCALAR\"}}],\
         \"bufferViews\":[{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":36}},{{\"buffer\":0,\"byteOffset\":36,\"byteLength\":6}}],\
         \"buffers\":[{{\"byteLength\":44{source}}}]}}"
    )
}

fn glb() -> Vec<u8> {
    let mut json = gltf_json(false).into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut binary = Vec::new();
    for v in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
        binary.extend_from_slice(&v.to_le_bytes());
    }
    for i in [0u16, 1, 2, 0] {
        binary.extend_from_slice(&i.to_le_bytes());
    }
    let mut out = b"glTF".to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + json.len() + 8 + binary.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&binary);
    out
}

fn dso() -> Vec<u8> {
    ScriptFormat::Torque3d
        .compile("main.cs", b"function f() { return 1; }\n$a = f();\n")
        .unwrap()
        .unwrap()
}

/// A good file of every kind the loaders own, and for the settings files both of their names.
fn good_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        (
            "shapes/cube.dts",
            write_dts(&torque_shapes::synth::model(&mut Rng(7), true, 6)),
        ),
        (
            "shapes/walk.dsq",
            write_dsq(&dsq(&mut Rng(3), &[b"root", b"base"], 2)),
        ),
        ("shapes/tri.glb", glb()),
        ("shapes/tri.gltf", gltf_json(true).into_bytes()),
        (
            "interiors/box.dif",
            torque_maps::synth::dif(torque_maps::synth::DifSpec {
                version: 0,
                plain_surfaces: false,
                everything: false,
                border: 0,
            }),
        ),
        ("terrain/hills.ter", torque_maps::synth::ter(6, &["grass"])),
        ("sky/clouds.dml", b"sky/cloud1\nsky/cloud2\n".to_vec()),
        ("textures/dot.png", png()),
        ("textures/dot.jpg", jpeg()),
        ("sounds/tick.wav", wav()),
        ("sounds/tone.ogg", ogg()),
        ("fonts/arial.uft", uft()),
        ("fonts/arial.gft", gft()),
        (
            "fonts/face.ttf",
            torque_fonts::ttf_fuzz::Synth::basic().build(),
        ),
        ("main.cs", b"function f() { return 1; }\n".to_vec()),
        ("ui/menu.gui", b"new GuiControl(Menu) { };\n".to_vec()),
        ("main.cs.dso", dso()),
        ("game.toml", b"[game]\nname = \"x\"\n".to_vec()),
        ("mods/a/mod.toml", b"name = \"a\"\n".to_vec()),
    ]
}

/// Kinds whose loader takes a file cut in half or an empty one by design: a list of names, settings, a script of no statement.
fn takes_a_cut(path: &str) -> bool {
    matches!(Kind::of(path), Some(Kind::MaterialList | Kind::Config))
}

#[test]
fn a_good_file_of_each_kind_opens() {
    for (path, bytes) in good_files() {
        let kind = Kind::of(path).unwrap();
        check(kind, path, &bytes, &limits(), ScriptFormat::Torque3d)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
    }
}

#[test]
fn a_cut_empty_or_garbled_file_of_each_kind_is_refused() {
    for (path, bytes) in good_files() {
        let kind = Kind::of(path).unwrap();
        let mut flipped = bytes.clone();
        for b in &mut flipped {
            *b = !*b;
        }
        let half = &bytes[..bytes.len() / 2];
        let mut cases = Vec::new();
        if kind != Kind::MaterialList {
            cases.push(("garbled", &flipped[..]));
        }
        if !takes_a_cut(path) {
            cases.push(("cut", half));
            cases.push(("empty", &[]));
        }
        for (what, bad) in cases {
            let outcome = check(kind, path, bad, &limits(), ScriptFormat::Torque3d);
            // A script with no statement is a file the compiler takes; every other kind must refuse.
            let is_script = kind == Kind::Script;
            if what == "empty" && is_script {
                continue;
            }
            assert!(outcome.is_err(), "{path} {what}");
        }
    }
}

#[test]
fn a_file_over_its_loaders_cap_is_refused_before_it_is_opened() {
    let big = vec![b' '; torque_maps::MAX_DML_BYTES + 1];
    let error = check(
        Kind::MaterialList,
        "a.dml",
        &big,
        &limits(),
        ScriptFormat::Torque3d,
    )
    .unwrap_err();
    assert!(error.contains("over the limit"), "{error}");
    let small = Limits {
        file_bytes: 8,
        ..limits()
    };
    let error = check(
        Kind::Script,
        "a.cs",
        b"$a = 1; $b = 2;",
        &small,
        ScriptFormat::Torque3d,
    )
    .unwrap_err();
    assert!(error.contains("over the limit of 8"), "{error}");
}

#[test]
fn a_mod_manifest_is_held_to_the_size_the_mods_plugin_reads() {
    let long = format!("name = \"{}\"\n", "a".repeat(MAX_MANIFEST_BYTES as usize));
    for name in ["mod.toml", "mods/x/MOD.TOML"] {
        let error = check(
            Kind::Config,
            name,
            long.as_bytes(),
            &limits(),
            ScriptFormat::Torque3d,
        )
        .unwrap_err();
        assert!(error.contains("over the limit of 65536"), "{name}: {error}");
    }
    // Another settings file may be larger.
    check(
        Kind::Config,
        "game.toml",
        long.as_bytes(),
        &limits(),
        ScriptFormat::Torque3d,
    )
    .unwrap();
}

#[test]
fn a_loader_that_charges_work_stops_when_the_budget_is_spent() {
    let (path, bytes) = good_files().into_iter().next().unwrap();
    let none = Limits {
        work: 0,
        ..limits()
    };
    let format = ScriptFormat::Torque3d;
    assert!(check(Kind::Shape, path, &bytes, &none, format).is_err());
    assert!(check(Kind::Shape, path, &bytes, &limits(), format).is_ok());
}

#[test]
fn kinds_follow_the_extensions_the_engine_reads() {
    assert_eq!(Kind::of("a/B.DTS"), Some(Kind::Shape));
    assert_eq!(Kind::of("a.JPG"), Some(Kind::Jpeg));
    // TGE never reads a `.jpeg`.
    assert_eq!(Kind::of("a.jpeg"), None);
    assert_eq!(Kind::of("mods/x/server.cs"), Some(Kind::Script));
    assert_eq!(Kind::of("mods/x/server.lua"), None);
    assert_eq!(Kind::of("noextension"), None);
    assert_eq!(Kind::of("dir.dts/file"), None);
    for name in ["a.obj", "a.bbmodel", "a.map"] {
        assert_eq!(Kind::of(name), None, "{name}");
    }
}

#[test]
fn a_settings_file_that_is_not_toml_is_refused() {
    let format = ScriptFormat::Torque3d;
    let error = check(Kind::Config, "mod.toml", b"name = ", &limits(), format).unwrap_err();
    assert!(!error.is_empty());
    assert!(check(Kind::Config, "mod.toml", &[0xff, 0xfe], &limits(), format).is_err());
}

#[test]
fn an_interior_the_engine_loads_is_not_refused_for_failing_the_cross_reference_check() {
    // `dif::parse` is what the engine loads; `Interior::check` is stricter and is not asked.
    let dif = torque_maps::synth::dif(torque_maps::synth::DifSpec {
        version: 0,
        plain_surfaces: false,
        everything: true,
        border: 0,
    });
    check(
        Kind::Interior,
        "a.dif",
        &dif,
        &limits(),
        ScriptFormat::Torque3d,
    )
    .unwrap();
}

#[test]
fn control_characters_in_a_path_or_a_reason_never_reach_the_message() {
    let refusal = Refusal::new("a\u{1b}]0;title\u{7}.cs", "bad \u{1b}[31m\u{9b}red\r\n");
    assert!(
        !refusal.to_string().chars().any(char::is_control),
        "{refusal:?}"
    );
    assert_eq!(refusal.path, "a?]0;title?.cs");

    // From a script's own syntax error, which quotes its text.
    let error = ScriptFormat::Torque3d
        .compile("bad.cs", b"function f( { \x1b]0;pwned\x07 }\n")
        .unwrap_err();
    assert!(!error.chars().any(char::is_control), "{error:?}");
}

#[test]
fn the_mod_constants_are_the_mods_plugins() {
    assert_eq!(precompile::MOD_MANIFEST, torque_mods::MANIFEST);
    assert_eq!(precompile::MODS_FOLDER, torque_mods::DEFAULT_FOLDER);
    assert_eq!(MAX_MANIFEST_BYTES, torque_mods::MAX_MANIFEST_BYTES as u64);
}
